//! Cache size reporting and cleaning for the daemon cache.
//!
//! The cache has two trees below the cache directory: `downloads/`
//! and `covers/`. Each tree holds one subdirectory per provider,
//! so `downloads/youtube/abcdefghijk.mp4` is the download for the
//! YouTube track `abcdefghijk`. Cleaning walks both trees and
//! deletes regular files, except the files of the kept track.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use common_structs::{AUDIO_EXTENSIONS, ProviderId, TrackKey};
use recvrd_api::{CacheCleanResult, CacheScope, CacheSizes};
use recvrd_core::Track;

/// The result of one cache eviction pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CacheEviction {
    pub freed_bytes: u64,
    pub deleted_files: usize,
}

/// One channel's cached tracks: the playlist id and name plus
/// the tracks in it, sorted by title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedPlaylist {
    pub id: String,
    pub name: String,
    pub tracks: Vec<Track>,
}

/// Size reporting and cleaning for the download and cover caches.
#[derive(Debug, Clone)]
pub struct CacheService {
    downloads_dir: PathBuf,
    covers_dir: PathBuf,
}

impl CacheService {
    /// Build a service for the two cache trees. The directories
    /// need not exist yet; missing trees report zero sizes.
    pub fn new(downloads_dir: PathBuf, covers_dir: PathBuf) -> Self {
        Self {
            downloads_dir,
            covers_dir,
        }
    }

    /// List the complete downloads for one provider, from the
    /// metadata sidecars below `downloads/<provider>/`.
    ///
    /// The index scans the sidecars. It does not use a store.
    /// The sidecar already holds the full track, and a scan works
    /// for caches that exist before this change. A scan also
    /// heals itself: a deleted file leaves the index.
    ///
    /// A download is complete when its sidecar parses as a track
    /// whose provider and ID match the sidecar file, and a
    /// non-empty audio file exists for it. The method skips a
    /// sidecar that does not parse, does not match its file, or
    /// has no audio. The result sorts by title, then track ID.
    pub fn cached_tracks(&self, provider: ProviderId) -> Vec<Track> {
        let dir = self.downloads_dir.join(provider.as_str());
        let Ok(entries) = fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut tracks = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext != "json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            let key = TrackKey::new(provider, stem);
            if !key.validate() {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(track) = serde_json::from_str::<Track>(&text) else {
                continue;
            };
            if track.provider != provider || track.track_id != stem {
                continue;
            }
            if !has_audio(&key, &self.downloads_dir) {
                continue;
            }
            tracks.push(track);
        }
        tracks.sort_by(|a, b| a.title.cmp(&b.title).then(a.track_id.cmp(&b.track_id)));
        tracks
    }

    /// Group the complete downloads for one provider into
    /// playlists by channel.
    ///
    /// A track belongs to the playlist of its channel ID when it
    /// has one, otherwise to the playlist of its album. A track
    /// with neither a channel nor an album belongs to no playlist.
    /// The playlists sort by name, then ID. Each playlist holds
    /// its tracks sorted by title.
    pub fn cached_playlists(&self, provider: ProviderId) -> Vec<CachedPlaylist> {
        let mut groups: Vec<CachedPlaylist> = Vec::new();
        for track in self.cached_tracks(provider) {
            let Some((id, name)) = playlist_key(&track) else {
                continue;
            };
            match groups.iter_mut().find(|group| group.id == id) {
                Some(group) => group.tracks.push(track),
                None => groups.push(CachedPlaylist {
                    id,
                    name,
                    tracks: vec![track],
                }),
            }
        }
        groups.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        groups
    }

    /// Report the sizes of both cache trees from disk.
    pub fn sizes(&self) -> CacheSizes {
        let downloads = sum_tree(&self.downloads_dir);
        let covers = sum_tree(&self.covers_dir);
        CacheSizes {
            downloads_bytes: downloads.0,
            downloads_files: downloads.1,
            covers_bytes: covers.0,
            covers_files: covers.1,
        }
    }

    /// True when a non-empty audio file exists for the track. An
    /// invalid key is never cached. A sidecar alone does not count:
    /// playback needs the audio file.
    pub fn is_cached(&self, key: &TrackKey) -> bool {
        if !key.validate() {
            return false;
        }
        let Some(path) = key.existing_audio_file(&self.downloads_dir) else {
            return false;
        };
        std::fs::metadata(&path)
            .map(|data| data.len() > 0)
            .unwrap_or(false)
    }

    /// Set the `cached` flag on every track in the list, from disk.
    /// Tracks without a local audio file keep a false flag.
    pub fn annotate_tracks(&self, tracks: &mut [recvrd_api::Track]) {
        for track in tracks {
            track.cached = self.is_cached(&track.key());
        }
    }

    /// Evict the oldest complete downloads until the download cache
    /// is at or below `max_bytes`. A value of 0 disables the limit.
    /// Track keys in `keep` are never evicted. Incomplete downloads
    /// and `.part` files are left alone, so an active download is
    /// never damaged. Returns the freed bytes and deleted file count.
    pub fn enforce_limit(&self, max_bytes: u64, keep: &[TrackKey]) -> CacheEviction {
        if max_bytes == 0 {
            return CacheEviction::default();
        }
        let mut total = sum_tree(&self.downloads_dir).0;
        if total <= max_bytes {
            return CacheEviction::default();
        }

        #[derive(Default)]
        struct Group {
            paths: Vec<PathBuf>,
            bytes: u64,
            newest: Option<SystemTime>,
            has_json: bool,
            has_audio: bool,
        }

        let mut groups: HashMap<TrackKey, Group> = HashMap::new();
        let Ok(providers) = fs::read_dir(&self.downloads_dir) else {
            return CacheEviction::default();
        };
        for provider_dir in providers.flatten().filter(|entry| entry.path().is_dir()) {
            let Some(provider) = provider_dir
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<ProviderId>().ok())
            else {
                continue;
            };
            let Ok(files) = fs::read_dir(provider_dir.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
                    continue;
                };
                if extension == "part" || !path.is_file() {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
                    continue;
                };
                let key = TrackKey::new(provider, stem);
                if !key.validate() {
                    continue;
                }
                let Ok(metadata) = file.metadata() else {
                    continue;
                };
                let group = groups.entry(key).or_default();
                group.bytes += metadata.len();
                if let Ok(modified) = metadata.modified() {
                    group.newest = Some(group.newest.map_or(modified, |old| old.max(modified)));
                }
                group.has_json |= extension == "json";
                group.has_audio |= AUDIO_EXTENSIONS.contains(&extension);
                group.paths.push(path);
            }
        }

        let mut groups: Vec<(TrackKey, Group)> = groups
            .into_iter()
            .filter(|(key, group)| {
                group.has_json && group.has_audio && !keep.iter().any(|keep| keep == key)
            })
            .collect();
        groups.sort_by_key(|(_, group)| group.newest);

        let mut eviction = CacheEviction::default();
        for (_, group) in groups {
            if total <= max_bytes {
                break;
            }
            for path in group.paths {
                if fs::remove_file(&path).is_ok() {
                    eviction.deleted_files += 1;
                }
            }
            total = total.saturating_sub(group.bytes);
            eviction.freed_bytes += group.bytes;
        }
        eviction
    }

    /// Delete cached files for `scope`, except the files of the
    /// kept track. Returns the freed bytes and deleted file counts.
    /// Stopping active downloads is the caller's job; this only
    /// touches the filesystem.
    pub fn clean(&self, scope: CacheScope, keep: Option<&TrackKey>) -> CacheCleanResult {
        let (downloads_freed_bytes, downloads_deleted_files) =
            if scope == CacheScope::Downloads || scope == CacheScope::All {
                clean_tree(&self.downloads_dir, keep)
            } else {
                (0, 0)
            };
        let (covers_freed_bytes, covers_deleted_files) =
            if scope == CacheScope::Covers || scope == CacheScope::All {
                clean_tree(&self.covers_dir, keep)
            } else {
                (0, 0)
            };
        CacheCleanResult {
            scope,
            downloads_freed_bytes,
            downloads_deleted_files,
            covers_freed_bytes,
            covers_deleted_files,
            freed_bytes: downloads_freed_bytes + covers_freed_bytes,
        }
    }
}

/// Sum the bytes and file count of every regular file below `dir`.
/// A missing directory sums to zero. Symlinks are not followed.
fn sum_tree(dir: &Path) -> (u64, u64) {
    let mut bytes = 0;
    let mut files = 0;
    collect_files(dir, &mut |path, size| {
        let _ = path;
        bytes += size;
        files += 1;
    });
    (bytes, files)
}

/// Delete every regular file below `dir`, except the files of the
/// kept track. Returns the freed bytes and deleted file count.
/// Empty directories are removed after their files are gone.
fn clean_tree(dir: &Path, keep: Option<&TrackKey>) -> (u64, u64) {
    let mut freed = 0;
    let mut deleted = 0;
    let mut paths = Vec::new();
    collect_files(dir, &mut |path, size| {
        let _ = size;
        paths.push(path.to_path_buf());
    });
    for path in paths {
        if keep.is_some_and(|key| file_belongs_to(&path, key)) {
            continue;
        }
        let size = fs::metadata(&path).map(|data| data.len()).unwrap_or(0);
        if fs::remove_file(&path).is_ok() {
            freed += size;
            deleted += 1;
        }
    }
    remove_empty_dirs(dir);
    (freed, deleted)
}

/// Visit every regular file below `dir`, deepest first. Symlinks
/// are skipped, so cleaning never deletes outside the cache.
fn collect_files(dir: &Path, visit: &mut dyn FnMut(&Path, u64)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            collect_files(&path, visit);
        } else if metadata.is_file() {
            visit(&path, metadata.len());
        }
    }
}

/// Remove every empty directory below `dir`, except `dir` itself.
fn remove_empty_dirs(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            remove_empty_dirs(&path);
            let _ = fs::remove_dir(&path);
        }
    }
}

/// Check whether a complete audio file exists for a track key.
/// The file must exist and must not be empty.
fn has_audio(key: &TrackKey, downloads_dir: &Path) -> bool {
    key.existing_audio_file(downloads_dir)
        .and_then(|path| fs::metadata(path).ok())
        .is_some_and(|metadata| metadata.len() > 0)
}

/// Find the playlist id and name for a track. The channel ID
/// wins when the track has one. The album is the fallback. A
/// track with neither has no playlist.
fn playlist_key(track: &Track) -> Option<(String, String)> {
    let metadata = track.metadata.as_deref();
    let channel_id = metadata
        .and_then(|metadata| metadata.channel_id.as_deref())
        .filter(|id| !id.is_empty());
    if let Some(id) = channel_id {
        let name = metadata
            .and_then(|metadata| metadata.channel_title.clone())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| track.album.clone());
        let name = if name.is_empty() {
            id.to_string()
        } else {
            name
        };
        return Some((id.to_string(), name));
    }
    if !track.album.is_empty() {
        return Some((track.album.clone(), track.album.clone()));
    }
    None
}

/// Check whether a cache file belongs to a track key. A file
/// belongs to a key when its parent directory names the provider
/// and its file stem is the track ID.
fn file_belongs_to(path: &Path, key: &TrackKey) -> bool {
    let Some(parent) = path.parent().and_then(|dir| dir.file_name()) else {
        return false;
    };
    let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
        return false;
    };
    parent == key.provider.as_str() && stem == key.id
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_structs::ProviderId;
    use std::os::unix::fs::symlink;

    const ID: &str = "abcdefghijk";

    fn write_file(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn key(id: &str) -> TrackKey {
        TrackKey::new(ProviderId::Youtube, id.to_string())
    }

    fn service(dir: &Path) -> CacheService {
        CacheService::new(dir.join("downloads"), dir.join("covers"))
    }

    fn seed(dir: &Path) {
        write_file(
            &dir.join("downloads/youtube/abcdefghijk.mp4"),
            b"video-data",
        );
        write_file(&dir.join("downloads/youtube/abcdefghijk.json"), b"{}");
        write_file(
            &dir.join("downloads/youtube/zyxwvutsrqp.mp4"),
            b"other-video",
        );
        write_file(&dir.join("covers/youtube/abcdefghijk.jpg"), b"jpeg-data");
        write_file(&dir.join("covers/youtube/zyxwvutsrqp.jpg"), b"other-jpeg");
    }

    fn set_age(path: &Path, seconds: u64) {
        let file = fs::File::open(path).unwrap();
        file.set_modified(SystemTime::now() - std::time::Duration::from_secs(seconds))
            .unwrap();
    }

    fn seed_download_pair(dir: &Path, id: &str) {
        write_file(
            &dir.join(format!("downloads/youtube/{id}.mp4")),
            b"0123456789",
        );
        write_file(&dir.join(format!("downloads/youtube/{id}.json")), b"{}");
    }

    #[test]
    fn enforce_limit_evicts_the_oldest_complete_download() {
        let dir = tempfile::tempdir().unwrap();
        seed_download_pair(dir.path(), "abcdefghijk");
        seed_download_pair(dir.path(), "zyxwvutsrqp");
        set_age(&dir.path().join("downloads/youtube/abcdefghijk.mp4"), 120);
        set_age(&dir.path().join("downloads/youtube/abcdefghijk.json"), 120);

        let eviction = service(dir.path()).enforce_limit(12, &[]);

        assert_eq!(eviction.deleted_files, 2);
        assert_eq!(eviction.freed_bytes, 12);
        assert!(
            !dir.path()
                .join("downloads/youtube/abcdefghijk.mp4")
                .exists()
        );
        assert!(
            !dir.path()
                .join("downloads/youtube/abcdefghijk.json")
                .exists()
        );
        assert!(
            dir.path()
                .join("downloads/youtube/zyxwvutsrqp.mp4")
                .exists()
        );
        assert!(
            dir.path()
                .join("downloads/youtube/zyxwvutsrqp.json")
                .exists()
        );
    }

    #[test]
    fn enforce_limit_never_evicts_a_protected_track() {
        let dir = tempfile::tempdir().unwrap();
        seed_download_pair(dir.path(), "abcdefghijk");
        seed_download_pair(dir.path(), "zyxwvutsrqp");
        set_age(&dir.path().join("downloads/youtube/abcdefghijk.mp4"), 120);
        set_age(&dir.path().join("downloads/youtube/abcdefghijk.json"), 120);

        let eviction = service(dir.path()).enforce_limit(12, &[key("abcdefghijk")]);

        assert_eq!(eviction.deleted_files, 2);
        assert!(
            dir.path()
                .join("downloads/youtube/abcdefghijk.mp4")
                .exists()
        );
        assert!(
            !dir.path()
                .join("downloads/youtube/zyxwvutsrqp.mp4")
                .exists()
        );
    }

    #[test]
    fn enforce_limit_leaves_part_files_alone() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            &dir.path().join("downloads/youtube/abcdefghijk.part"),
            b"part",
        );

        let eviction = service(dir.path()).enforce_limit(1, &[]);

        assert_eq!(eviction, CacheEviction::default());
        assert!(
            dir.path()
                .join("downloads/youtube/abcdefghijk.part")
                .exists()
        );
    }

    #[test]
    fn enforce_limit_zero_disables_eviction() {
        let dir = tempfile::tempdir().unwrap();
        seed_download_pair(dir.path(), "abcdefghijk");

        let eviction = service(dir.path()).enforce_limit(0, &[]);

        assert_eq!(eviction, CacheEviction::default());
        assert!(
            dir.path()
                .join("downloads/youtube/abcdefghijk.mp4")
                .exists()
        );
    }

    #[test]
    fn is_cached_needs_a_non_empty_audio_file() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let cache = service(dir.path());
        assert!(cache.is_cached(&key(ID)));
        assert!(!cache.is_cached(&key("qqqqqqqqqqq")));
    }

    #[test]
    fn is_cached_ignores_an_empty_audio_file() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir.path().join("downloads/youtube/abcdefghijk.mp4"), b"");
        assert!(!service(dir.path()).is_cached(&key(ID)));
    }

    #[test]
    fn is_cached_ignores_a_part_file_and_a_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            &dir.path().join("downloads/youtube/abcdefghijk.part"),
            b"partial",
        );
        write_file(
            &dir.path().join("downloads/youtube/abcdefghijk.json"),
            b"{}",
        );
        assert!(!service(dir.path()).is_cached(&key(ID)));
    }

    #[test]
    fn is_cached_rejects_an_invalid_key() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let key = TrackKey::new(ProviderId::Youtube, "../abcdefghijk".to_string());
        assert!(!service(dir.path()).is_cached(&key));
    }

    #[test]
    fn annotate_tracks_marks_only_cached_tracks() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let cache = service(dir.path());
        let mut tracks = vec![
            recvrd_api::Track {
                provider: ProviderId::Youtube,
                track_id: ID.to_string(),
                ..Default::default()
            },
            recvrd_api::Track {
                provider: ProviderId::Youtube,
                track_id: "qqqqqqqqqqq".to_string(),
                ..Default::default()
            },
        ];
        cache.annotate_tracks(&mut tracks);
        assert!(tracks[0].cached);
        assert!(!tracks[1].cached);
    }

    #[test]
    fn sizes_sum_both_trees() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let sizes = service(dir.path()).sizes();
        assert_eq!(sizes.downloads_files, 3);
        assert_eq!(sizes.downloads_bytes, 10 + 2 + 11);
        assert_eq!(sizes.covers_files, 2);
        assert_eq!(sizes.covers_bytes, 9 + 10);
    }

    #[test]
    fn sizes_are_zero_for_missing_directories() {
        let dir = tempfile::tempdir().unwrap();
        let sizes = service(dir.path()).sizes();
        assert_eq!(sizes, CacheSizes::default());
    }

    #[test]
    fn clean_downloads_keeps_covers() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let result = service(dir.path()).clean(CacheScope::Downloads, None);
        assert_eq!(result.downloads_deleted_files, 3);
        assert_eq!(result.downloads_freed_bytes, 10 + 2 + 11);
        assert_eq!(result.covers_deleted_files, 0);
        assert_eq!(result.freed_bytes, 23);
        assert!(!dir.path().join("downloads/youtube").exists());
        assert!(dir.path().join("covers/youtube/abcdefghijk.jpg").exists());
    }

    #[test]
    fn clean_covers_keeps_downloads() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let result = service(dir.path()).clean(CacheScope::Covers, None);
        assert_eq!(result.covers_deleted_files, 2);
        assert_eq!(result.covers_freed_bytes, 9 + 10);
        assert_eq!(result.downloads_deleted_files, 0);
        assert!(
            dir.path()
                .join("downloads/youtube/abcdefghijk.mp4")
                .exists()
        );
    }

    #[test]
    fn clean_all_keeps_the_kept_track_files() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());
        let result = service(dir.path()).clean(CacheScope::All, Some(&key(ID)));
        assert_eq!(result.downloads_deleted_files, 1);
        assert_eq!(result.covers_deleted_files, 1);
        assert!(
            dir.path()
                .join("downloads/youtube/abcdefghijk.mp4")
                .exists()
        );
        assert!(
            dir.path()
                .join("downloads/youtube/abcdefghijk.json")
                .exists()
        );
        assert!(dir.path().join("covers/youtube/abcdefghijk.jpg").exists());
        assert!(
            !dir.path()
                .join("downloads/youtube/zyxwvutsrqp.mp4")
                .exists()
        );
        assert!(!dir.path().join("covers/youtube/zyxwvutsrqp.jpg").exists());
    }

    #[test]
    fn clean_removes_files_from_unknown_provider_directories() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            &dir.path().join("downloads/rss/abcdefghijk.mp4"),
            b"rss-video",
        );
        write_file(&dir.path().join("covers/rss/abcdefghijk.jpg"), b"rss-jpeg");
        let result = service(dir.path()).clean(CacheScope::All, Some(&key(ID)));
        assert_eq!(result.downloads_deleted_files, 1);
        assert_eq!(result.covers_deleted_files, 1);
    }

    #[test]
    fn clean_removes_flat_legacy_files() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir.path().join("downloads/abcdefghijk.mp4"), b"flat-video");
        write_file(&dir.path().join("covers/abcdefghijk.jpg"), b"flat-jpeg");
        let result = service(dir.path()).clean(CacheScope::All, None);
        assert_eq!(result.downloads_deleted_files, 1);
        assert_eq!(result.covers_deleted_files, 1);
    }

    #[test]
    fn clean_does_not_follow_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("keep-me.mp4");
        fs::write(&target, b"keep me").unwrap();
        fs::create_dir_all(dir.path().join("downloads/youtube")).unwrap();
        symlink(&target, dir.path().join("downloads/youtube/link.mp4")).unwrap();
        let result = service(dir.path()).clean(CacheScope::All, None);
        assert_eq!(result.downloads_deleted_files, 0);
        assert_eq!(result.downloads_freed_bytes, 0);
        assert!(target.exists());
        assert_eq!(fs::read(&target).unwrap(), b"keep me");
    }

    fn cached_track(
        provider: ProviderId,
        id: &str,
        title: &str,
        album: &str,
        channel_id: Option<&str>,
        channel_title: Option<&str>,
    ) -> Track {
        Track {
            provider,
            track_id: id.to_string(),
            title: title.to_string(),
            author: "Artist".to_string(),
            album: album.to_string(),
            duration: "3:00".to_string(),
            source: None,
            metadata: Some(Box::new(common_structs::TrackMetadata {
                channel_id: channel_id.map(str::to_string),
                channel_title: channel_title.map(str::to_string),
                ..Default::default()
            })),
            liked: None,
        }
    }

    fn seed_download(dir: &Path, track: &Track, audio: &[u8]) {
        let base = dir.join("downloads").join(track.provider.as_str());
        write_file(&base.join(format!("{}.mp3", track.track_id)), audio);
        write_file(
            &base.join(format!("{}.json", track.track_id)),
            serde_json::to_string(track).unwrap().as_bytes(),
        );
    }

    #[test]
    fn cached_tracks_lists_complete_downloads_per_provider() {
        let dir = tempfile::tempdir().unwrap();
        let first = cached_track(
            ProviderId::Podverse,
            "1001",
            "Beta song",
            "Channel One",
            Some("42"),
            Some("Channel One"),
        );
        let second = cached_track(
            ProviderId::Podverse,
            "1002",
            "Alpha song",
            "Channel One",
            Some("42"),
            Some("Channel One"),
        );
        let youtube = cached_track(
            ProviderId::Youtube,
            "abcdefghijk",
            "Video song",
            "Album",
            None,
            None,
        );
        seed_download(dir.path(), &first, b"audio");
        seed_download(dir.path(), &second, b"audio");
        seed_download(dir.path(), &youtube, b"audio");

        let podverse = service(dir.path()).cached_tracks(ProviderId::Podverse);
        assert_eq!(podverse.len(), 2);
        assert_eq!(podverse[0].title, "Alpha song");
        assert_eq!(podverse[1].title, "Beta song");
        let listed = service(dir.path()).cached_tracks(ProviderId::Youtube);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].track_id, "abcdefghijk");
    }

    #[test]
    fn cached_tracks_skips_incomplete_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("downloads").join("podverse");
        // Sidecar without audio.
        let orphan = cached_track(
            ProviderId::Podverse,
            "2001",
            "Orphan",
            "Channel",
            Some("42"),
            Some("Channel"),
        );
        write_file(
            &base.join("2001.json"),
            serde_json::to_string(&orphan).unwrap().as_bytes(),
        );
        // Empty audio file.
        let empty = cached_track(
            ProviderId::Podverse,
            "2002",
            "Empty",
            "Channel",
            Some("42"),
            Some("Channel"),
        );
        seed_download(dir.path(), &empty, b"");
        // Audio without a sidecar.
        write_file(&base.join("2003.mp3"), b"audio");
        // Sidecar that does not parse.
        write_file(&base.join("2004.json"), b"{not json");
        write_file(&base.join("2004.mp3"), b"audio");
        // Sidecar whose track ID does not match its file.
        let mismatch = cached_track(
            ProviderId::Podverse,
            "9999",
            "Mismatch",
            "Channel",
            Some("42"),
            Some("Channel"),
        );
        write_file(
            &base.join("2005.json"),
            serde_json::to_string(&mismatch).unwrap().as_bytes(),
        );
        write_file(&base.join("2005.mp3"), b"audio");
        // Sidecar that names another provider.
        let foreign = cached_track(
            ProviderId::Youtube,
            "abcdefghijk",
            "Foreign",
            "Album",
            None,
            None,
        );
        write_file(
            &base.join("abcdefghijk.json"),
            serde_json::to_string(&foreign).unwrap().as_bytes(),
        );
        write_file(&base.join("abcdefghijk.mp3"), b"audio");

        assert!(
            service(dir.path())
                .cached_tracks(ProviderId::Podverse)
                .is_empty()
        );
    }

    #[test]
    fn cached_tracks_is_empty_for_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            service(dir.path())
                .cached_tracks(ProviderId::Podverse)
                .is_empty()
        );
    }

    #[test]
    fn cached_playlists_groups_by_channel() {
        let dir = tempfile::tempdir().unwrap();
        let first = cached_track(
            ProviderId::Podverse,
            "1001",
            "Beta song",
            "Channel One",
            Some("42"),
            Some("Channel One"),
        );
        let second = cached_track(
            ProviderId::Podverse,
            "1002",
            "Alpha song",
            "Channel One",
            Some("42"),
            Some("Channel One"),
        );
        let third = cached_track(
            ProviderId::Podverse,
            "1003",
            "Other song",
            "Channel Two",
            Some("43"),
            Some("Channel Two"),
        );
        let album_only = cached_track(
            ProviderId::Podverse,
            "1004",
            "Album song",
            "Lone Album",
            None,
            None,
        );
        let ungrouped = cached_track(ProviderId::Podverse, "1005", "No home", "", None, None);
        for track in [&first, &second, &third, &album_only, &ungrouped] {
            seed_download(dir.path(), track, b"audio");
        }

        let playlists = service(dir.path()).cached_playlists(ProviderId::Podverse);
        assert_eq!(playlists.len(), 3);
        assert_eq!(playlists[0].id, "42");
        assert_eq!(playlists[0].name, "Channel One");
        assert_eq!(playlists[0].tracks.len(), 2);
        assert_eq!(playlists[0].tracks[0].title, "Alpha song");
        assert_eq!(playlists[1].id, "43");
        assert_eq!(playlists[1].tracks.len(), 1);
        assert_eq!(playlists[2].id, "Lone Album");
        assert_eq!(playlists[2].name, "Lone Album");
        assert_eq!(playlists[2].tracks.len(), 1);

        let tracks = service(dir.path()).cached_tracks(ProviderId::Podverse);
        assert_eq!(tracks.len(), 5);
        assert!(tracks.iter().any(|track| track.track_id == "1005"));
    }

    #[test]
    fn cached_playlists_uses_the_id_when_the_name_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let track = cached_track(ProviderId::Podverse, "1001", "Song", "", Some("77"), None);
        seed_download(dir.path(), &track, b"audio");

        let playlists = service(dir.path()).cached_playlists(ProviderId::Podverse);
        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].id, "77");
        assert_eq!(playlists[0].name, "77");
    }
}
