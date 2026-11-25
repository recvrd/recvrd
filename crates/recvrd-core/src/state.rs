use crate::model::{RepeatMode, Track};
use common_structs::TrackKey;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;

pub const STATE_VERSION: u32 = 1;
/// The maximum number of per-track positions to keep.
pub const MAX_TRACK_POSITIONS: usize = 1000;

/// The last saved position of one track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackPosition {
    pub position_secs: f64,
    #[serde(default)]
    pub updated_at: u64,
}

/// Saved positions keyed by `provider:track_id`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TrackPositions(BTreeMap<String, TrackPosition>);

impl TrackPositions {
    /// Get the saved position for a track.
    pub fn get(&self, key: &TrackKey) -> Option<Duration> {
        let position = self.0.get(&key.to_string())?;
        Duration::try_from_secs_f64(position.position_secs).ok()
    }

    /// Get the write time of the saved position for a track.
    pub fn updated_at(&self, key: &TrackKey) -> Option<u64> {
        self.0
            .get(&key.to_string())
            .map(|position| position.updated_at)
    }

    /// Save a position and evict the oldest entry when the map is full.
    /// A zero position removes the entry.
    pub fn set(&mut self, key: &TrackKey, position: Duration, updated_at: u64) {
        let key = key.to_string();
        if position.is_zero() {
            self.0.remove(&key);
            return;
        }
        if self.0.len() >= MAX_TRACK_POSITIONS && !self.0.contains_key(&key) {
            self.evict_oldest();
        }
        self.0.insert(
            key,
            TrackPosition {
                position_secs: position.as_secs_f64(),
                updated_at,
            },
        );
    }

    /// Remove the saved position for a track.
    pub fn remove(&mut self, key: &TrackKey) {
        self.0.remove(&key.to_string());
    }

    /// Keep only the positions for the given track keys.
    pub fn retain_for(&mut self, keys: &[TrackKey]) {
        let keys: BTreeSet<String> = keys.iter().map(ToString::to_string).collect();
        self.0.retain(|key, _| keys.contains(key));
    }

    /// Remove all saved positions.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn contains(&self, key: &TrackKey) -> bool {
        self.0.contains_key(&key.to_string())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &TrackPosition)> {
        self.0.iter()
    }

    /// Remove the oldest entries until the map is within the cap.
    pub fn enforce_cap(&mut self) {
        while self.0.len() > MAX_TRACK_POSITIONS {
            self.evict_oldest();
        }
    }

    fn evict_oldest(&mut self) {
        let oldest = self
            .0
            .iter()
            .min_by(|(left_key, left), (right_key, right)| {
                left.updated_at
                    .cmp(&right.updated_at)
                    .then_with(|| left_key.cmp(right_key))
            })
            .map(|(key, _)| key.clone());
        if let Some(key) = oldest {
            self.0.remove(&key);
        }
    }
}

/// Versioned player state persisted between daemon runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedState {
    pub version: u32,
    pub tracks: Vec<Track>,
    /// The unshuffled queue order, when the saved queue was shuffled.
    /// Empty in state files that predate this field.
    #[serde(default)]
    pub original_tracks: Vec<Track>,
    pub current: Option<usize>,
    pub repeat: RepeatMode,
    pub shuffle: bool,
    pub volume: u8,
    pub position_secs: f64,
    /// Saved positions for tracks in the queue, keyed by
    /// `provider:track_id`. Empty in state files that predate this field.
    #[serde(default)]
    pub positions: TrackPositions,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            tracks: Vec::new(),
            original_tracks: Vec::new(),
            current: None,
            repeat: RepeatMode::Off,
            shuffle: false,
            volume: 50,
            position_secs: 0.0,
            positions: TrackPositions::default(),
        }
    }
}

impl PersistedState {
    /// Write the state atomically via a temporary file in the same directory.
    pub fn save(&self, path: &Path) -> Result<(), StateError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| StateError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let text = toml::to_string_pretty(self).map_err(StateError::Serialize)?;
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, text).map_err(|source| StateError::Io {
            path: tmp.clone(),
            source,
        })?;

        if let Err(error) = fs::rename(&tmp, path) {
            // Windows refuses to replace an existing file with rename.
            if path.exists() {
                fs::remove_file(path).map_err(|source| StateError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
                fs::rename(&tmp, path).map_err(|source| StateError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
            } else {
                return Err(StateError::Io {
                    path: path.to_path_buf(),
                    source: error,
                });
            }
        }
        Ok(())
    }

    /// Load state. A missing file returns defaults; malformed or incompatible
    /// files return an error so the caller can quarantine them.
    pub fn load(path: &Path) -> Result<Self, StateError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(StateError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };

        let mut state: Self = toml::from_str(&text).map_err(|source| StateError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        if state.version != STATE_VERSION {
            return Err(StateError::UnsupportedVersion {
                found: state.version,
                expected: STATE_VERSION,
            });
        }
        state.validate()?;
        state.positions.enforce_cap();
        Ok(state)
    }

    fn validate(&self) -> Result<(), StateError> {
        if let Some(current) = self.current
            && current >= self.tracks.len()
        {
            return Err(StateError::Invalid(format!(
                "current index {current} is out of range for {} tracks",
                self.tracks.len()
            )));
        }
        if self.volume > 100 {
            return Err(StateError::Invalid(format!(
                "volume {} is outside 0-100",
                self.volume
            )));
        }
        if !self.position_secs.is_finite() || self.position_secs < 0.0 {
            return Err(StateError::Invalid(format!(
                "position {} is invalid",
                self.position_secs
            )));
        }
        for (key, position) in self.positions.iter() {
            if !position.position_secs.is_finite() || position.position_secs < 0.0 {
                return Err(StateError::Invalid(format!(
                    "position for {key} is invalid"
                )));
            }
        }
        if !self.original_tracks.is_empty() {
            let mut saved: Vec<_> = self.tracks.iter().map(|track| track.key()).collect();
            let mut original: Vec<_> = self
                .original_tracks
                .iter()
                .map(|track| track.key())
                .collect();
            saved.sort_unstable();
            original.sort_unstable();
            if saved != original {
                return Err(StateError::Invalid(
                    "original tracks do not match the saved tracks".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum StateError {
    #[error("failed to access state {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse state {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to serialize state: {0}")]
    Serialize(#[source] toml::ser::Error),
    #[error("unsupported state version {found}, expected {expected}")]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("invalid state: {0}")]
    Invalid(String),
    #[error("failed to restore state: {0}")]
    Restore(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn track(index: usize) -> Track {
        Track {
            provider: common_structs::ProviderId::Youtube,
            track_id: format!("video-{index}"),
            title: format!("Track {index}"),
            author: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        let state = PersistedState {
            tracks: vec![track(0), track(1)],
            current: Some(1),
            repeat: RepeatMode::All,
            shuffle: true,
            volume: 80,
            position_secs: 42.5,
            ..PersistedState::default()
        };

        state.save(&path).unwrap();
        let loaded = PersistedState::load(&path).unwrap();
        assert_eq!(state, loaded);
        assert!(!dir.path().join("state.tmp").exists());
    }

    #[test]
    fn round_trip_keeps_source_and_metadata() {
        use common_structs::{MediaKind, TrackMetadata, TrackSource};
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut track = track(0);
        track.provider = common_structs::ProviderId::Podverse;
        track.track_id = "YvIRJrjZVe".to_string();
        track.source = Some(Box::new(TrackSource {
            url: "https://cdn.example/track.mp3".to_string(),
            mime: Some("audio/mpeg".to_string()),
            length: Some(1024),
            seekable: true,
            kind: MediaKind::Audio,
        }));
        track.metadata = Some(Box::new(TrackMetadata {
            channel_title: Some("Rock The Halls".to_string()),
            season_number: Some(1),
            ..Default::default()
        }));
        let state = PersistedState {
            tracks: vec![track],
            ..PersistedState::default()
        };

        state.save(&path).unwrap();
        assert_eq!(PersistedState::load(&path).unwrap(), state);
    }

    #[test]
    fn missing_file_uses_defaults() {
        let dir = tempdir().unwrap();
        let state = PersistedState::load(&dir.path().join("missing.json")).unwrap();
        assert_eq!(state, PersistedState::default());
    }

    #[test]
    fn corrupt_file_is_reported() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(&path, "not = [valid").unwrap();
        let error = PersistedState::load(&path).unwrap_err();
        assert!(matches!(error, StateError::Parse { .. }));
        assert!(error.to_string().contains("state.json"));
    }

    #[test]
    fn unsupported_version_is_reported() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        let state = PersistedState {
            version: 99,
            ..PersistedState::default()
        };
        state.save(&path).unwrap();
        let error = PersistedState::load(&path).unwrap_err();
        assert!(matches!(
            error,
            StateError::UnsupportedVersion {
                found: 99,
                expected: 1
            }
        ));
    }

    #[test]
    fn invalid_current_index_is_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        let state = PersistedState {
            tracks: vec![track(0)],
            current: Some(4),
            ..PersistedState::default()
        };
        state.save(&path).unwrap();
        let error = PersistedState::load(&path).unwrap_err();
        assert!(matches!(error, StateError::Invalid(_)));
    }

    #[test]
    fn invalid_volume_is_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        let state = PersistedState {
            volume: 101,
            ..PersistedState::default()
        };
        state.save(&path).unwrap();
        let error = PersistedState::load(&path).unwrap_err();
        assert!(error.to_string().contains("volume"));
    }

    #[test]
    fn state_without_original_tracks_loads() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.toml");
        fs::write(
            &path,
            "version = 1\ntracks = []\nrepeat = \"off\"\nshuffle = true\nvolume = 50\nposition_secs = 0.0\n",
        )
        .unwrap();
        let state = PersistedState::load(&path).unwrap();
        assert!(state.original_tracks.is_empty());
        assert!(state.positions.is_empty());
        assert!(state.shuffle);
    }

    #[test]
    fn state_without_positions_loads() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.toml");
        fs::write(
            &path,
            "version = 1\ncurrent = 0\nrepeat = \"off\"\nshuffle = false\nvolume = 50\nposition_secs = 12.0\n[[tracks]]\ntrack_id = \"video-0\"\ntitle = \"Track\"\nauthor = \"Artist\"\nalbum = \"Album\"\nduration = \"3:00\"\n",
        )
        .unwrap();
        let state = PersistedState::load(&path).unwrap();
        assert!(state.positions.is_empty());
        assert_eq!(state.position_secs, 12.0);
    }

    #[test]
    fn position_map_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        let tracks = vec![track(0), track(1), track(2)];
        let mut state = PersistedState {
            tracks: tracks.clone(),
            current: Some(1),
            position_secs: 15.0,
            ..PersistedState::default()
        };
        state
            .positions
            .set(&tracks[0].key(), Duration::from_secs(42), 100);
        state
            .positions
            .set(&tracks[1].key(), Duration::from_secs(15), 200);
        state
            .positions
            .set(&tracks[2].key(), Duration::from_secs(90), 300);

        state.save(&path).unwrap();
        let loaded = PersistedState::load(&path).unwrap();

        assert_eq!(loaded, state);
        assert_eq!(
            loaded.positions.get(&tracks[0].key()),
            Some(Duration::from_secs(42))
        );
    }

    #[test]
    fn position_map_cap_evicts_the_oldest_entry() {
        let mut positions = TrackPositions::default();
        for index in 0..MAX_TRACK_POSITIONS {
            positions.set(
                &TrackKey::youtube(format!("{index:011}")),
                Duration::from_secs(index as u64 + 1),
                index as u64,
            );
        }
        assert_eq!(positions.len(), MAX_TRACK_POSITIONS);

        let newest = TrackKey::youtube("aaaaaaaaaaa");
        positions.set(&newest, Duration::from_secs(1), MAX_TRACK_POSITIONS as u64);

        assert_eq!(positions.len(), MAX_TRACK_POSITIONS);
        assert!(positions.contains(&newest));
        assert!(!positions.contains(&TrackKey::youtube("00000000000")));
    }

    #[test]
    fn updating_a_position_does_not_evict_it() {
        let mut positions = TrackPositions::default();
        let first = TrackKey::youtube("aaaaaaaaaaa");
        positions.set(&first, Duration::from_secs(10), 0);
        for index in 0..MAX_TRACK_POSITIONS - 1 {
            positions.set(
                &TrackKey::youtube(format!("{index:011}")),
                Duration::from_secs(index as u64 + 1),
                index as u64 + 1,
            );
        }
        assert_eq!(positions.len(), MAX_TRACK_POSITIONS);

        positions.set(&first, Duration::from_secs(20), u64::MAX);
        let newest = TrackKey::youtube("bbbbbbbbbbb");
        positions.set(&newest, Duration::from_secs(1), u64::MAX);

        assert_eq!(positions.len(), MAX_TRACK_POSITIONS);
        assert!(positions.contains(&first));
        assert!(positions.contains(&newest));
    }

    #[test]
    fn zero_positions_are_not_stored() {
        let mut positions = TrackPositions::default();
        let key = TrackKey::youtube("dQw4w9WgXcQ");
        positions.set(&key, Duration::from_secs(10), 1);
        positions.set(&key, Duration::ZERO, 2);
        assert!(positions.is_empty());
    }

    #[test]
    fn invalid_track_position_is_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.toml");
        fs::write(
            &path,
            "version = 1\ntracks = []\nrepeat = \"off\"\nshuffle = false\nvolume = 50\nposition_secs = 0.0\n\n[positions.\"youtube:abc\"]\nposition_secs = -1.0\nupdated_at = 0\n",
        )
        .unwrap();
        let error = PersistedState::load(&path).unwrap_err();
        assert!(matches!(error, StateError::Invalid(_)));
        assert!(error.to_string().contains("youtube:abc"));
    }

    #[test]
    fn old_state_file_with_video_id_loads_as_youtube() {
        // State files written before the provider model have
        // `video_id` and no `provider`. They still load.
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.toml");
        fs::write(
            &path,
            "version = 1\ncurrent = 0\nrepeat = \"off\"\nshuffle = false\nvolume = 50\nposition_secs = 0.0\n[[tracks]]\nvideo_id = \"dQw4w9WgXcQ\"\ntitle = \"Song\"\nauthor = \"Artist\"\nalbum = \"Album\"\nduration = \"3:00\"\n",
        )
        .unwrap();
        let state = PersistedState::load(&path).unwrap();
        assert_eq!(state.tracks.len(), 1);
        assert_eq!(state.tracks[0].track_id, "dQw4w9WgXcQ");
        assert_eq!(
            state.tracks[0].provider,
            common_structs::ProviderId::Youtube
        );
    }

    #[test]
    fn mismatched_original_tracks_are_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        let state = PersistedState {
            tracks: vec![track(0)],
            original_tracks: vec![track(0), track(1)],
            ..PersistedState::default()
        };
        state.save(&path).unwrap();
        let error = PersistedState::load(&path).unwrap_err();
        assert!(matches!(error, StateError::Invalid(_)));
        assert!(error.to_string().contains("original tracks"));
    }
}
