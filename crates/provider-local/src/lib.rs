//! Local-files provider.
//!
//! The provider scans one music directory for audio files and serves
//! them as one read-only playlist. The file name is the track title.
//! Tag metadata and cover art are not read yet.

use async_trait::async_trait;
use common_structs::{MediaKind, ProviderId, TrackKey, TrackMetadata, TrackSource};
#[cfg(test)]
use lofty::{
    config::WriteOptions,
    picture::{MimeType, Picture, PictureType},
    tag::{Tag, TagType},
};
use lofty::{
    file::{AudioFile, TaggedFileExt},
    tag::Accessor,
};
use recvrd_api::{
    CreatePlaylistRequest, MusicApi, Playlist, PlaylistDetail, PlaylistEditError, SearchResults,
    Track, TrackDetail, UpdatePlaylistRequest,
};
use recvrd_core::CoverFetcher;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

/// The playlist ID for the local library.
pub const LOCAL_PLAYLIST_ID: &str = "local";

/// The file extensions the provider scans. The match is
/// case-insensitive.
const EXTENSIONS: [&str; 5] = ["mp3", "m4a", "ogg", "flac", "wav"];

/// How long a scan stays fresh. New files appear after this time.
const SCAN_TTL: Duration = Duration::from_secs(30);

/// The sidecar cover names, without the extension.
const COVER_STEMS: [&str; 4] = ["cover", "folder", "front", "album"];
/// The sidecar cover extensions.
const COVER_EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// A provider for one local music directory.
pub struct LocalProvider {
    music_dir: PathBuf,
    cache: Mutex<Option<(Instant, Vec<Track>)>>,
}

impl LocalProvider {
    pub fn new(music_dir: impl Into<PathBuf>) -> Self {
        Self {
            music_dir: music_dir.into(),
            cache: Mutex::new(None),
        }
    }

    /// The configured music directory.
    pub fn music_dir(&self) -> &Path {
        &self.music_dir
    }

    /// Scan the directory for audio files. The result is cached for a
    /// short time and sorted by relative path, so the order is stable.
    fn scan(&self) -> Vec<Track> {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((at, tracks)) = cache.as_ref()
            && at.elapsed() < SCAN_TTL
        {
            return tracks.clone();
        }
        let tracks = self.scan_uncached();
        *cache = Some((Instant::now(), tracks.clone()));
        tracks
    }

    fn scan_uncached(&self) -> Vec<Track> {
        let mut files = Vec::new();
        collect_files(&self.music_dir, &self.music_dir, &mut files);
        files.sort_by(|left, right| left.0.cmp(&right.0));
        files.into_iter().map(|(_, track)| track).collect()
    }

    fn track_path(&self, track: &Track) -> Option<PathBuf> {
        let source = track.source.as_ref()?;
        let path = source.url.strip_prefix("file://").unwrap_or(&source.url);
        Some(PathBuf::from(path))
    }
}

fn collect_files(root: &Path, dir: &Path, files: &mut Vec<(PathBuf, Track)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, files);
            continue;
        }
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if !EXTENSIONS
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(extension))
        {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        if let Some(track) = local_track(&path, relative) {
            files.push((relative.to_path_buf(), track));
        }
    }
}

fn local_track(path: &Path, relative: &Path) -> Option<Track> {
    let file_stem = path.file_stem()?.to_string_lossy().to_string();
    if file_stem.is_empty() {
        return None;
    }
    let tagged = lofty::read_from_path(path).ok();
    let tag = tagged
        .as_ref()
        .and_then(|file| file.primary_tag().or_else(|| file.first_tag()));

    let title = tag
        .and_then(|tag| tag.title())
        .filter(|title| !title.trim().is_empty())
        .map(|title| title.to_string())
        .unwrap_or(file_stem);
    let artist = tag
        .and_then(|tag| tag.artist())
        .unwrap_or_default()
        .to_string();
    let fallback_album = relative
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "Local files".to_string());
    let album = tag
        .and_then(|tag| tag.album())
        .filter(|album| !album.trim().is_empty())
        .map(|album| album.to_string())
        .unwrap_or(fallback_album);
    let duration = tagged
        .as_ref()
        .map(|file| file.properties().duration())
        .filter(|duration| !duration.is_zero())
        .map(format_duration)
        .unwrap_or_default();

    let mut metadata = TrackMetadata::default();
    if let Some(track_number) = tag.and_then(|tag| tag.track()) {
        metadata.track_number = Some(track_number);
    }
    let metadata = (metadata != TrackMetadata::default()).then(|| Box::new(metadata));

    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    Some(Track {
        provider: ProviderId::Local,
        track_id: track_id(relative),
        title,
        artist,
        album,
        duration,
        source: Some(Box::new(TrackSource {
            url: format!("file://{}", absolute.display()),
            mime: None,
            length: None,
            seekable: true,
            kind: MediaKind::Audio,
        })),
        metadata,
        cached: true,
        liked: None,
    })
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Read the embedded cover from the first tag.
fn embedded_cover(path: &Path) -> Option<Vec<u8>> {
    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    tag.pictures()
        .first()
        .map(|picture| picture.data().to_vec())
}

/// Read a sidecar cover from the track directory. The name match is
/// case-insensitive.
fn sidecar_cover(path: &Path) -> Option<Vec<u8>> {
    let dir = path.parent()?;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let Some((stem, extension)) = name.rsplit_once('.') else {
            continue;
        };
        if COVER_STEMS.contains(&stem) && COVER_EXTENSIONS.contains(&extension) {
            return fs::read(entry.path()).ok();
        }
    }
    None
}

/// A stable track ID from the relative path. The first 32 hex
/// characters match the provider ID rule.
fn track_id(relative: &Path) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(relative.to_string_lossy().as_bytes());
    let mut id = String::with_capacity(64);
    for byte in digest {
        let _ = write!(id, "{byte:02x}");
    }
    id.truncate(32);
    id
}

fn check_provider(provider: ProviderId) -> Result<(), String> {
    if provider == ProviderId::Local {
        Ok(())
    } else {
        Err(format!("provider {provider} is not local"))
    }
}

fn edit_unsupported(action: &str) -> PlaylistEditError {
    PlaylistEditError::Invalid(format!("local files do not support {action}"))
}

#[async_trait]
impl MusicApi for LocalProvider {
    async fn playlists(&self, provider: ProviderId) -> Result<Vec<Playlist>, String> {
        check_provider(provider)?;
        Ok(vec![Playlist {
            id: LOCAL_PLAYLIST_ID.to_string(),
            name: "Local files".to_string(),
            subtitle: self.music_dir.display().to_string(),
            ..Default::default()
        }])
    }

    async fn playlist(&self, provider: ProviderId, id: &str) -> Result<Vec<Track>, String> {
        check_provider(provider)?;
        if id != LOCAL_PLAYLIST_ID {
            return Err(format!("local playlist {id} not found"));
        }
        Ok(self.scan())
    }

    async fn playlist_detail(
        &self,
        provider: ProviderId,
        id: &str,
    ) -> Result<PlaylistDetail, String> {
        check_provider(provider)?;
        if id != LOCAL_PLAYLIST_ID {
            return Err(format!("local playlist {id} not found"));
        }
        Ok(PlaylistDetail {
            playlist: Playlist {
                id: LOCAL_PLAYLIST_ID.to_string(),
                name: "Local files".to_string(),
                subtitle: self.music_dir.display().to_string(),
                ..Default::default()
            },
            tracks: self.scan(),
        })
    }

    async fn track_detail(&self, provider: ProviderId, id: &str) -> Result<TrackDetail, String> {
        check_provider(provider)?;
        let track = self
            .scan()
            .into_iter()
            .find(|track| track.track_id == id)
            .ok_or_else(|| format!("local track {id} not found"))?;
        Ok(TrackDetail {
            track,
            description: None,
        })
    }

    async fn search(&self, provider: ProviderId, query: &str) -> Result<SearchResults, String> {
        check_provider(provider)?;
        let query = query.to_lowercase();
        let tracks = self
            .scan()
            .into_iter()
            .filter(|track| {
                track.title.to_lowercase().contains(&query)
                    || track.artist.to_lowercase().contains(&query)
                    || track.album.to_lowercase().contains(&query)
            })
            .collect();
        Ok(SearchResults {
            tracks,
            playlists: Vec::new(),
        })
    }

    async fn create_playlist(
        &self,
        _request: &CreatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError> {
        Err(edit_unsupported("playlist creation"))
    }

    async fn update_playlist(
        &self,
        _id: &str,
        _request: &UpdatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError> {
        Err(edit_unsupported("playlist edits"))
    }

    async fn delete_playlist(&self, _id: &str) -> Result<(), PlaylistEditError> {
        Err(edit_unsupported("playlist deletion"))
    }

    async fn add_playlist_tracks(
        &self,
        _id: &str,
        _track_ids: &[String],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        Err(edit_unsupported("playlist edits"))
    }

    async fn remove_playlist_tracks(
        &self,
        _id: &str,
        _indices: &[usize],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        Err(edit_unsupported("playlist edits"))
    }

    async fn move_playlist_track(
        &self,
        _id: &str,
        _from: usize,
        _to: usize,
    ) -> Result<Vec<Track>, PlaylistEditError> {
        Err(edit_unsupported("playlist edits"))
    }
}

#[async_trait]
impl CoverFetcher for LocalProvider {
    async fn fetch(&self, key: &TrackKey) -> Option<Vec<u8>> {
        if key.provider != ProviderId::Local {
            return None;
        }
        let track = self
            .scan()
            .into_iter()
            .find(|track| track.track_id == key.id)?;
        let path = self.track_path(&track)?;
        embedded_cover(&path).or_else(|| sidecar_cover(&path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, seconds: u32) {
        let rate = 8_000u32;
        let data_len = rate * seconds * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.resize(bytes.len() + data_len as usize, 0);
        fs::write(path, bytes).unwrap();
    }

    fn tag_wav(path: &Path, cover: Option<Vec<u8>>) {
        let mut tagged = lofty::read_from_path(path).unwrap();
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title("Tagged Title".to_string());
        tag.set_artist("Tagged Artist".to_string());
        tag.set_album("Tagged Album".to_string());
        tag.set_track(3);
        if let Some(cover) = cover {
            tag.push_picture(
                Picture::unchecked(cover)
                    .mime_type(MimeType::Png)
                    .pic_type(PictureType::CoverFront)
                    .build(),
            );
        }
        tagged.insert_tag(tag);
        tagged.save_to_path(path, WriteOptions::default()).unwrap();
    }

    fn provider() -> (tempfile::TempDir, LocalProvider) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Album One")).unwrap();
        fs::create_dir_all(dir.path().join("Album Two")).unwrap();
        fs::write(dir.path().join("One.mp3"), b"audio").unwrap();
        fs::write(dir.path().join("Album One/Two.FLAC"), b"audio").unwrap();
        fs::write(dir.path().join("Album Two/Three.wav"), b"audio").unwrap();
        fs::write(dir.path().join("notes.txt"), b"text").unwrap();
        fs::write(dir.path().join(".hidden.mp3"), b"audio").unwrap();
        let provider = LocalProvider::new(dir.path());
        (dir, provider)
    }

    #[tokio::test]
    async fn track_detail_returns_a_scanned_track() {
        let (_dir, provider) = provider();
        let tracks = provider
            .playlist(ProviderId::Local, LOCAL_PLAYLIST_ID)
            .await
            .unwrap();
        let detail = provider
            .track_detail(ProviderId::Local, &tracks[0].track_id)
            .await
            .unwrap();
        assert_eq!(detail.track.track_id, tracks[0].track_id);
        assert_eq!(detail.track.title, tracks[0].title);
    }

    #[tokio::test]
    async fn reads_tags_and_duration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tagged.wav");
        write_wav(&path, 1);
        tag_wav(&path, None);
        let provider = LocalProvider::new(dir.path());

        let tracks = provider.scan();

        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Tagged Title");
        assert_eq!(tracks[0].artist, "Tagged Artist");
        assert_eq!(tracks[0].album, "Tagged Album");
        assert_eq!(tracks[0].duration, "0:01");
        assert_eq!(tracks[0].metadata.as_ref().unwrap().track_number, Some(3));
    }

    #[tokio::test]
    async fn keeps_the_file_name_when_tags_are_missing() {
        let (_dir, provider) = provider();
        let tracks = provider.scan();
        let track = tracks
            .iter()
            .find(|track| track.title == "One")
            .expect("the plain file");
        assert_eq!(track.artist, "");
        assert_eq!(track.duration, "");
    }

    #[tokio::test]
    async fn serves_an_embedded_cover() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tagged.wav");
        write_wav(&path, 1);
        let png = b"\x89PNG\r\n\x1a\ncover-data".to_vec();
        tag_wav(&path, Some(png.clone()));
        let provider = LocalProvider::new(dir.path());
        let track = &provider.scan()[0];
        let key = TrackKey::new(ProviderId::Local, track.track_id.clone());

        assert_eq!(provider.fetch(&key).await, Some(png));
    }

    #[tokio::test]
    async fn serves_a_sidecar_cover() {
        let dir = tempfile::tempdir().unwrap();
        write_wav(&dir.path().join("plain.wav"), 1);
        fs::write(dir.path().join("Cover.PNG"), b"sidecar").unwrap();
        let provider = LocalProvider::new(dir.path());
        let track = &provider.scan()[0];
        let key = TrackKey::new(ProviderId::Local, track.track_id.clone());

        assert_eq!(provider.fetch(&key).await, Some(b"sidecar".to_vec()));
    }

    #[tokio::test]
    async fn caches_the_scan() {
        let (_dir, provider) = provider();
        assert_eq!(provider.scan().len(), 3);
        fs::write(provider.music_dir().join("New.mp3"), b"audio").unwrap();

        assert_eq!(
            provider.scan().len(),
            3,
            "the scan cache hides a new file until the TTL"
        );
    }

    #[tokio::test]
    async fn lists_the_local_playlist() {
        let (_dir, provider) = provider();
        let playlists = provider.playlists(ProviderId::Local).await.unwrap();
        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].id, LOCAL_PLAYLIST_ID);
        assert_eq!(playlists[0].name, "Local files");
    }

    #[tokio::test]
    async fn scans_supported_files_in_order() {
        let (_dir, provider) = provider();
        let tracks = provider
            .playlist(ProviderId::Local, LOCAL_PLAYLIST_ID)
            .await
            .unwrap();
        let titles: Vec<&str> = tracks.iter().map(|track| track.title.as_str()).collect();
        assert_eq!(titles, ["Two", "Three", "One"]);
        assert_eq!(tracks[0].album, "Album One");
        assert_eq!(tracks[1].album, "Album Two");
        assert_eq!(tracks[2].album, "Local files");
        assert!(tracks.iter().all(|track| track.cached));
        assert!(
            tracks
                .iter()
                .all(|track| track.provider == ProviderId::Local)
        );
        for track in &tracks {
            assert!(ProviderId::Local.validate_id(&track.track_id));
            assert!(track.source.as_ref().unwrap().url.starts_with("file://"));
        }
    }

    #[tokio::test]
    async fn search_filters_by_name_and_album() {
        let (_dir, provider) = provider();
        let results = provider.search(ProviderId::Local, "three").await.unwrap();
        assert_eq!(results.tracks.len(), 1);
        assert_eq!(results.tracks[0].title, "Three");

        let results = provider
            .search(ProviderId::Local, "album one")
            .await
            .unwrap();
        assert_eq!(results.tracks.len(), 1);
        assert_eq!(results.tracks[0].title, "Two");
    }

    #[tokio::test]
    async fn rejects_other_providers_and_edits() {
        let (_dir, provider) = provider();
        assert!(provider.playlists(ProviderId::Rss).await.is_err());
        assert!(provider.delete_playlist(LOCAL_PLAYLIST_ID).await.is_err());
    }
}
