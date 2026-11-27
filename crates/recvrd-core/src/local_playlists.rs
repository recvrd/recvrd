//! Daemon-local playlists that can hold tracks from any provider.

use crate::model::Track;
use common_structs::ProviderId;
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
};
use thiserror::Error;

/// The local playlist store version.
pub const LOCAL_PLAYLISTS_VERSION: u32 = 1;
/// The maximum number of characters in a playlist name.
pub const MAX_LOCAL_PLAYLIST_NAME: usize = 150;

/// One daemon-local playlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPlaylist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub tracks: Vec<Track>,
}

impl LocalPlaylist {
    /// Build the summary for a playlist list response.
    pub fn summary(&self) -> LocalPlaylistSummary {
        LocalPlaylistSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            item_count: self.tracks.len(),
            image_url: self.tracks.first().and_then(|track| {
                track
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.image_url.clone())
            }),
        }
    }
}

/// The summary fields for one local playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPlaylistSummary {
    pub id: String,
    pub name: String,
    pub item_count: usize,
    pub image_url: Option<String>,
}

/// The JSON file that holds every local playlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPlaylistsFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub playlists: Vec<LocalPlaylist>,
}

impl Default for LocalPlaylistsFile {
    fn default() -> Self {
        Self {
            version: LOCAL_PLAYLISTS_VERSION,
            playlists: Vec::new(),
        }
    }
}

impl LocalPlaylistsFile {
    /// Load the file. A missing file returns an empty store.
    pub fn load(path: &Path) -> Result<Self, LocalPlaylistError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(LocalPlaylistError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let state: Self =
            serde_json::from_str(&text).map_err(|source| LocalPlaylistError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        if state.version != LOCAL_PLAYLISTS_VERSION {
            return Err(LocalPlaylistError::UnsupportedVersion {
                found: state.version,
                expected: LOCAL_PLAYLISTS_VERSION,
            });
        }
        Ok(state)
    }

    /// Write the file atomically through a temporary file.
    pub fn save(&self, path: &Path) -> Result<(), LocalPlaylistError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| LocalPlaylistError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let text = serde_json::to_string_pretty(self).map_err(LocalPlaylistError::Serialize)?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, text).map_err(|source| LocalPlaylistError::Io {
            path: temporary.clone(),
            source,
        })?;
        if let Err(error) = fs::rename(&temporary, path) {
            if path.exists() {
                fs::remove_file(path).map_err(|source| LocalPlaylistError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
                fs::rename(&temporary, path).map_err(|source| LocalPlaylistError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
            } else {
                return Err(LocalPlaylistError::Io {
                    path: path.to_path_buf(),
                    source: error,
                });
            }
        }
        Ok(())
    }
}

/// The in-memory local playlist store and its disk file.
pub struct LocalPlaylistStore {
    path: PathBuf,
    state: RwLock<LocalPlaylistsFile>,
}

impl LocalPlaylistStore {
    /// Load the store. A missing file starts empty. A corrupt file
    /// starts empty and moves aside, so a later save cannot destroy it.
    pub fn new(path: PathBuf) -> Self {
        let state = match LocalPlaylistsFile::load(&path) {
            Ok(state) => state,
            Err(error) => {
                log::warn!("{error}; starting with no local playlists");
                if path.exists() && !matches!(error, LocalPlaylistError::Io { .. }) {
                    let backup = path.with_extension("json.bad");
                    if let Err(error) = fs::rename(&path, &backup) {
                        log::warn!("could not move {} aside: {error}", path.display());
                    }
                }
                LocalPlaylistsFile::default()
            }
        };
        Self {
            path,
            state: RwLock::new(state),
        }
    }

    /// The store file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// List every playlist summary in file order.
    pub fn list(&self) -> Vec<LocalPlaylistSummary> {
        self.read()
            .playlists
            .iter()
            .map(LocalPlaylist::summary)
            .collect()
    }

    /// Return one playlist.
    pub fn get(&self, id: &str) -> Result<LocalPlaylist, LocalPlaylistError> {
        self.read()
            .playlists
            .iter()
            .find(|playlist| playlist.id == id)
            .cloned()
            .ok_or_else(|| LocalPlaylistError::not_found(id))
    }

    /// Create an empty playlist.
    pub fn create(&self, name: &str) -> Result<LocalPlaylist, LocalPlaylistError> {
        let name = validate_name(name)?;
        self.update(|state| {
            let playlist = LocalPlaylist {
                id: new_id(&state.playlists),
                name,
                tracks: Vec::new(),
            };
            state.playlists.push(playlist.clone());
            Ok(playlist)
        })
    }

    /// Rename one playlist.
    pub fn rename(&self, id: &str, name: &str) -> Result<LocalPlaylist, LocalPlaylistError> {
        let name = validate_name(name)?;
        self.update(|state| {
            let index = find_playlist(state, id)?;
            state.playlists[index].name = name;
            Ok(state.playlists[index].clone())
        })
    }

    /// Delete one playlist.
    pub fn delete(&self, id: &str) -> Result<(), LocalPlaylistError> {
        self.update(|state| {
            let index = find_playlist(state, id)?;
            state.playlists.remove(index);
            Ok(())
        })
    }

    /// Add one track to a playlist. Duplicate tracks are allowed.
    pub fn add_track(&self, id: &str, track: Track) -> Result<LocalPlaylist, LocalPlaylistError> {
        self.update(|state| {
            let index = find_playlist(state, id)?;
            if !track.key().validate() {
                return Err(LocalPlaylistError::InvalidTrack {
                    provider: track.provider,
                    track_id: track.track_id,
                });
            }
            state.playlists[index].tracks.push(track);
            Ok(state.playlists[index].clone())
        })
    }

    /// Remove one track by index.
    pub fn remove_track(
        &self,
        id: &str,
        index: usize,
    ) -> Result<LocalPlaylist, LocalPlaylistError> {
        self.update(|state| {
            let playlist_index = find_playlist(state, id)?;
            let len = state.playlists[playlist_index].tracks.len();
            if index >= len {
                return Err(LocalPlaylistError::IndexOutOfRange { index, len });
            }
            state.playlists[playlist_index].tracks.remove(index);
            Ok(state.playlists[playlist_index].clone())
        })
    }

    /// Move one track from one index to another.
    pub fn reorder_track(
        &self,
        id: &str,
        from: usize,
        to: usize,
    ) -> Result<LocalPlaylist, LocalPlaylistError> {
        self.update(|state| {
            let playlist_index = find_playlist(state, id)?;
            let len = state.playlists[playlist_index].tracks.len();
            if from >= len {
                return Err(LocalPlaylistError::IndexOutOfRange { index: from, len });
            }
            if to >= len {
                return Err(LocalPlaylistError::IndexOutOfRange { index: to, len });
            }
            let track = state.playlists[playlist_index].tracks.remove(from);
            state.playlists[playlist_index].tracks.insert(to, track);
            Ok(state.playlists[playlist_index].clone())
        })
    }

    fn read(&self) -> RwLockReadGuard<'_, LocalPlaylistsFile> {
        self.state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, LocalPlaylistsFile> {
        self.state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Mutate a copy, save it, then publish it. A failed save keeps
    /// the old in-memory state.
    fn update<T>(
        &self,
        change: impl FnOnce(&mut LocalPlaylistsFile) -> Result<T, LocalPlaylistError>,
    ) -> Result<T, LocalPlaylistError> {
        let mut state = self.write();
        let mut next = state.clone();
        let value = change(&mut next)?;
        next.save(&self.path)?;
        *state = next;
        Ok(value)
    }
}

fn validate_name(name: &str) -> Result<String, LocalPlaylistError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(LocalPlaylistError::EmptyName);
    }
    if name.chars().count() > MAX_LOCAL_PLAYLIST_NAME {
        return Err(LocalPlaylistError::NameTooLong {
            max: MAX_LOCAL_PLAYLIST_NAME,
        });
    }
    Ok(name.to_string())
}

fn find_playlist(state: &LocalPlaylistsFile, id: &str) -> Result<usize, LocalPlaylistError> {
    state
        .playlists
        .iter()
        .position(|playlist| playlist.id == id)
        .ok_or_else(|| LocalPlaylistError::not_found(id))
}

fn new_id(playlists: &[LocalPlaylist]) -> String {
    loop {
        let id = format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..));
        if !playlists.iter().any(|playlist| playlist.id == id) {
            return id;
        }
    }
}

#[derive(Debug, Error)]
pub enum LocalPlaylistError {
    #[error("playlist {id} not found")]
    NotFound { id: String },
    #[error("playlist name must not be empty")]
    EmptyName,
    #[error("playlist name must be at most {max} characters")]
    NameTooLong { max: usize },
    #[error("track index {index} is out of range for {len} tracks")]
    IndexOutOfRange { index: usize, len: usize },
    #[error("invalid track ID {track_id} for provider {provider}")]
    InvalidTrack {
        provider: ProviderId,
        track_id: String,
    },
    #[error("failed to access local playlists {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse local playlists {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to serialize local playlists: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("unsupported local playlist version {found}, expected {expected}")]
    UnsupportedVersion { found: u32, expected: u32 },
}

impl LocalPlaylistError {
    fn not_found(id: &str) -> Self {
        Self::NotFound { id: id.to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_structs::TrackMetadata;
    use tempfile::tempdir;

    fn track(provider: ProviderId, track_id: &str) -> Track {
        Track {
            provider,
            track_id: track_id.to_string(),
            title: format!("Track {track_id}"),
            author: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
            ..Default::default()
        }
    }

    fn store() -> LocalPlaylistsFile {
        LocalPlaylistsFile {
            version: LOCAL_PLAYLISTS_VERSION,
            playlists: vec![
                LocalPlaylist {
                    id: "a".repeat(32),
                    name: "Mix".to_string(),
                    tracks: vec![
                        track(ProviderId::Youtube, "abcdefghijk"),
                        track(ProviderId::Local, &"b".repeat(32)),
                    ],
                },
                LocalPlaylist {
                    id: "c".repeat(32),
                    name: "Podcasts".to_string(),
                    tracks: vec![track(ProviderId::Podverse, "YvIRJrjZVe")],
                },
            ],
        }
    }

    #[test]
    fn round_trip_through_disk() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("playlists.json");
        store().save(&path).unwrap();
        assert_eq!(LocalPlaylistsFile::load(&path).unwrap(), store());
        assert!(!dir.path().join("playlists.json.tmp").exists());
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = tempdir().unwrap();
        let loaded = LocalPlaylistsFile::load(&dir.path().join("missing.json")).unwrap();
        assert_eq!(loaded, LocalPlaylistsFile::default());
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("playlists.json");
        std::fs::write(&path, r#"{"version":99,"playlists":[]}"#).unwrap();
        let error = LocalPlaylistsFile::load(&path).unwrap_err();
        assert!(matches!(
            error,
            LocalPlaylistError::UnsupportedVersion { found: 99, .. }
        ));
    }

    #[test]
    fn corrupt_file_is_moved_aside() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("playlists.json");
        std::fs::write(&path, "{not json").unwrap();

        let store = LocalPlaylistStore::new(path.clone());

        assert!(store.list().is_empty());
        assert!(!path.exists());
        assert!(path.with_extension("json.bad").exists());
    }

    #[test]
    fn crud_survives_a_reload() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("playlists.json");
        let store = LocalPlaylistStore::new(path.clone());

        let created = store.create("  Mix  ").unwrap();
        assert_eq!(created.name, "Mix");
        assert_eq!(created.id.len(), 32);
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].id, created.id);

        let created = store
            .add_track(&created.id, track(ProviderId::Youtube, "abcdefghijk"))
            .unwrap();
        assert_eq!(created.tracks.len(), 1);
        let created = store
            .add_track(&created.id, track(ProviderId::Local, &"b".repeat(32)))
            .unwrap();
        assert_eq!(created.tracks.len(), 2);

        let renamed = store.rename(&created.id, "  Road trip  ").unwrap();
        assert_eq!(renamed.name, "Road trip");

        let reloaded = LocalPlaylistStore::new(path);
        let playlist = reloaded.get(&created.id).unwrap();
        assert_eq!(playlist.name, "Road trip");
        assert_eq!(playlist.tracks.len(), 2);
        assert_eq!(reloaded.list()[0].item_count, 2);

        reloaded.delete(&created.id).unwrap();
        assert!(reloaded.list().is_empty());
    }

    #[test]
    fn duplicate_tracks_are_kept() {
        let dir = tempdir().unwrap();
        let store = LocalPlaylistStore::new(dir.path().join("playlists.json"));
        let playlist = store.create("Mix").unwrap();
        let track = track(ProviderId::Youtube, "abcdefghijk");
        store.add_track(&playlist.id, track.clone()).unwrap();
        let playlist = store.add_track(&playlist.id, track).unwrap();
        assert_eq!(playlist.tracks.len(), 2);
    }

    #[test]
    fn remove_and_reorder_tracks() {
        let dir = tempdir().unwrap();
        let store = LocalPlaylistStore::new(dir.path().join("playlists.json"));
        let playlist = store.create("Mix").unwrap();
        for id in ["abcdefghijk", "bcdefghijkl", "cdefghijklm"] {
            store
                .add_track(&playlist.id, track(ProviderId::Youtube, id))
                .unwrap();
        }

        let playlist = store.reorder_track(&playlist.id, 0, 2).unwrap();
        let ids: Vec<_> = playlist
            .tracks
            .iter()
            .map(|track| track.track_id.as_str())
            .collect();
        assert_eq!(ids, ["bcdefghijkl", "cdefghijklm", "abcdefghijk"]);

        let playlist = store.remove_track(&playlist.id, 1).unwrap();
        assert_eq!(playlist.tracks.len(), 2);
        assert_eq!(playlist.tracks[0].track_id, "bcdefghijkl");
    }

    #[test]
    fn invalid_names_and_indices_are_rejected() {
        let dir = tempdir().unwrap();
        let store = LocalPlaylistStore::new(dir.path().join("playlists.json"));

        assert!(matches!(
            store.create("   "),
            Err(LocalPlaylistError::EmptyName)
        ));
        assert!(matches!(
            store.create(&"x".repeat(MAX_LOCAL_PLAYLIST_NAME + 1)),
            Err(LocalPlaylistError::NameTooLong { .. })
        ));

        let playlist = store.create("Mix").unwrap();
        assert!(matches!(
            store.add_track(&playlist.id, track(ProviderId::Youtube, "short")),
            Err(LocalPlaylistError::InvalidTrack { .. })
        ));
        assert!(matches!(
            store.remove_track(&playlist.id, 0),
            Err(LocalPlaylistError::IndexOutOfRange { index: 0, len: 0 })
        ));
        assert!(matches!(
            store.reorder_track(&playlist.id, 0, 1),
            Err(LocalPlaylistError::IndexOutOfRange { index: 0, len: 0 })
        ));
        assert!(matches!(
            store.get("missing"),
            Err(LocalPlaylistError::NotFound { .. })
        ));
    }

    #[test]
    fn summary_uses_the_first_cover() {
        let mut playlist = LocalPlaylist {
            id: "a".repeat(32),
            name: "Mix".to_string(),
            tracks: vec![track(ProviderId::Youtube, "abcdefghijk")],
        };
        playlist.tracks[0].metadata = Some(Box::new(TrackMetadata {
            image_url: Some("https://example.com/cover.jpg".to_string()),
            ..Default::default()
        }));
        let summary = playlist.summary();
        assert_eq!(summary.item_count, 1);
        assert_eq!(
            summary.image_url.as_deref(),
            Some("https://example.com/cover.jpg")
        );
    }
}
