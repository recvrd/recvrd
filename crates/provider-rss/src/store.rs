//! Persisted RSS provider state: the feed list and the episode
//! index. The daemon reads this file at startup, so downloads and
//! covers still resolve after a restart, even before the feeds
//! refresh. The file lives in the data directory, next to the
//! playback state.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The store format version. A future format bumps this number.
const STORE_VERSION: u32 = 1;

/// One subscribed feed: its URL, its conditional GET state, and
/// its title from the last fetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredFeed {
    pub url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub title: String,
    pub last_fetched: Option<u64>,
}

/// One episode, with everything the provider needs to serve it:
/// its track fields, its enclosure URL and extension, and its
/// cover URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredEpisode {
    pub feed_url: String,
    pub guid: String,
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: String,
    pub enclosure_url: String,
    #[serde(default)]
    pub enclosure_type: String,
    #[serde(default)]
    pub enclosure_length: Option<u64>,
    pub extension: String,
    pub cover_url: Option<String>,
}

/// The whole persisted state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RssStore {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub feeds: Vec<StoredFeed>,
    #[serde(default)]
    pub episodes: Vec<StoredEpisode>,
}

impl Default for RssStore {
    /// An empty store at the current format version. The version
    /// must be current: `load` discards a store whose version does
    /// not match, so a default with version 0 would never survive
    /// a save and reload.
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            feeds: Vec::new(),
            episodes: Vec::new(),
        }
    }
}

impl RssStore {
    /// Load the store. A missing file means an empty store. A
    /// corrupt file also means an empty store, so a bad file never
    /// blocks startup; the caller logs the loss.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(mut store) => {
                if store.version != STORE_VERSION {
                    store = Self::default();
                }
                store
            }
            Err(_) => Self::default(),
        }
    }

    /// Save the store with an atomic rename, so a crash cannot
    /// leave a half-written file behind.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn store() -> RssStore {
        RssStore {
            version: STORE_VERSION,
            feeds: vec![StoredFeed {
                url: "https://example.com/feed.xml".to_string(),
                etag: Some("\"abc\"".to_string()),
                last_modified: None,
                title: "Test Tones".to_string(),
                last_fetched: Some(1_700_000_000),
            }],
            episodes: vec![StoredEpisode {
                feed_url: "https://example.com/feed.xml".to_string(),
                guid: "episode-guid-1".to_string(),
                track_id: "0123456789abcdef0123456789abcdef".to_string(),
                title: "First Song".to_string(),
                artist: "The Testers".to_string(),
                album: "Test Tones".to_string(),
                duration: "4:05".to_string(),
                enclosure_url: "https://example.com/ep1.mp3".to_string(),
                enclosure_type: "audio/mpeg".to_string(),
                enclosure_length: Some(1024),
                extension: "mp3".to_string(),
                cover_url: None,
            }],
        }
    }

    #[test]
    fn round_trip_through_disk() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rss-feeds.json");
        store().save(&path).unwrap();
        assert_eq!(RssStore::load(&path), store());
    }

    #[test]
    fn default_store_has_the_current_version() {
        // A store built from nothing must survive a save and a
        // reload: load discards a store with a stale version.
        let dir = tempdir().unwrap();
        let path = dir.path().join("rss-feeds.json");
        RssStore::default().save(&path).unwrap();
        let loaded = RssStore::load(&path);
        assert_eq!(loaded.version, STORE_VERSION);
        assert!(loaded.feeds.is_empty());
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = tempdir().unwrap();
        let store = RssStore::load(&dir.path().join("missing.json"));
        assert!(store.feeds.is_empty());
        assert!(store.episodes.is_empty());
    }

    #[test]
    fn corrupt_file_loads_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rss-feeds.json");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(RssStore::load(&path), RssStore::default());
    }

    #[test]
    fn unknown_version_loads_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rss-feeds.json");
        std::fs::write(&path, r#"{"version":99,"feeds":[],"episodes":[]}"#).unwrap();
        assert_eq!(RssStore::load(&path), RssStore::default());
    }
}
