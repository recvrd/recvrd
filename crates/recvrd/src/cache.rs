//! Cache size reporting and cleaning for the daemon cache.
//!
//! The cache has two trees below the cache directory: `downloads/`
//! and `covers/`. Each tree holds one subdirectory per provider,
//! so `downloads/youtube/abcdefghijk.mp4` is the download for the
//! YouTube track `abcdefghijk`. Cleaning walks both trees and
//! deletes regular files, except the files of the kept track.

use std::{
    fs,
    path::{Path, PathBuf},
};

use common_structs::TrackKey;
use recvrd_api::{CacheCleanResult, CacheScope, CacheSizes};

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
}
