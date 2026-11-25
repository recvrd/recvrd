//! Local-files provider.
//!
//! The provider scans one music directory for audio files and serves
//! them as one read-only playlist. The file name is the track title.
//! Tag metadata and cover art are not read yet.

use async_trait::async_trait;
use common_structs::{MediaKind, ProviderId, TrackSource};
use recvrd_api::{
    CreatePlaylistRequest, MusicApi, Playlist, PlaylistDetail, PlaylistEditError, SearchResults,
    Track, UpdatePlaylistRequest,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The playlist ID for the local library.
pub const LOCAL_PLAYLIST_ID: &str = "local";

/// The file extensions the provider scans. The match is
/// case-insensitive.
const EXTENSIONS: [&str; 5] = ["mp3", "m4a", "ogg", "flac", "wav"];

/// A provider for one local music directory.
pub struct LocalProvider {
    music_dir: PathBuf,
}

impl LocalProvider {
    pub fn new(music_dir: impl Into<PathBuf>) -> Self {
        Self {
            music_dir: music_dir.into(),
        }
    }

    /// The configured music directory.
    pub fn music_dir(&self) -> &Path {
        &self.music_dir
    }

    /// Scan the directory for audio files. The result is sorted by
    /// relative path, so the playlist order is stable.
    fn scan(&self) -> Vec<Track> {
        let mut files = Vec::new();
        collect_files(&self.music_dir, &self.music_dir, &mut files);
        files.sort_by(|left, right| left.0.cmp(&right.0));
        files.into_iter().map(|(_, track)| track).collect()
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
    let title = path.file_stem()?.to_string_lossy().to_string();
    if title.is_empty() {
        return None;
    }
    let album = relative
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "Local files".to_string());
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    Some(Track {
        provider: ProviderId::Local,
        track_id: track_id(relative),
        title,
        artist: String::new(),
        album,
        duration: String::new(),
        source: Some(Box::new(TrackSource {
            url: format!("file://{}", absolute.display()),
            mime: None,
            length: None,
            seekable: true,
            kind: MediaKind::Audio,
        })),
        metadata: None,
        cached: true,
        liked: None,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

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
