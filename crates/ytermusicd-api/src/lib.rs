//! Shared DTOs for the ytermusicd HTTP/SSE API.
//!
//! These types are the API contract. The web UI and MCP proxy should consume
//! the generated TypeScript bindings rather than duplicating shapes.

pub mod music;

pub use common_structs::{ProviderId, TrackKey};
pub use music::{MusicApi, PlaylistEditError};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct ProviderCapabilities {
    /// The provider supports track and playlist search.
    pub search: bool,
    /// The provider supports a library of playlists.
    pub library: bool,
    /// The provider supports reading playlists.
    pub playlists: bool,
    /// The provider supports creating playlists.
    pub playlist_create: bool,
    /// The provider supports editing playlists.
    pub playlist_edit: bool,
    /// The provider supports downloads.
    pub downloads: bool,
    /// The provider supports cover art.
    pub covers: bool,
}

impl ProviderCapabilities {
    /// The capabilities of one provider. YouTube Music supports
    /// every action today. RSS feeds support search, library,
    /// playlists, downloads and covers, but not playlist edits:
    /// feeds are read-only. A provider that lacks an action must
    /// set its flag to false, so clients can hide the action.
    pub fn for_provider(provider: ProviderId) -> Self {
        match provider {
            ProviderId::Youtube => Self {
                search: true,
                library: true,
                playlists: true,
                playlist_create: true,
                playlist_edit: true,
                downloads: true,
                covers: true,
            },
            ProviderId::Rss => Self {
                search: true,
                library: true,
                playlists: true,
                playlist_create: false,
                playlist_edit: false,
                downloads: true,
                covers: true,
            },
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProviderInfo {
    /// The provider id, as used in the `provider` parameter.
    pub id: ProviderId,
    /// The display name, as shown in clients.
    pub name: String,
    /// True for the default provider of a fresh install.
    pub default: bool,
    /// The actions this provider supports.
    pub capabilities: ProviderCapabilities,
    /// The auth state of this provider.
    pub auth: AuthStatus,
}

impl ProviderInfo {
    /// Build the description of one provider.
    pub fn new(id: ProviderId, auth: AuthStatus) -> Self {
        Self {
            id,
            name: id.display_name().to_string(),
            default: id == ProviderId::default_provider(),
            capabilities: ProviderCapabilities::for_provider(id),
            auth,
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Track {
    /// The provider this track came from. Old payloads without the
    /// field read as YouTube.
    #[serde(default)]
    pub provider: ProviderId,
    /// The provider-scoped track ID. Accepts the old `video_id` name.
    #[serde(alias = "video_id")]
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: String,
}

impl Track {
    /// The provider-scoped key of this track.
    pub fn key(&self) -> TrackKey {
        TrackKey::new(self.provider, self.track_id.clone())
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub subtitle: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlaylistDetail {
    pub playlist: Playlist,
    pub tracks: Vec<Track>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RssFeed {
    /// The feed ID: the first 16 characters of the SHA-256 hash
    /// of the feed URL.
    pub id: String,
    /// The feed URL, as added by the user.
    pub url: String,
    /// The feed title, from the feed itself.
    pub title: String,
    /// The number of episodes in the feed.
    pub episode_count: usize,
    /// Unix time of the last fetch, in seconds. Empty when the
    /// feed was never fetched.
    pub last_fetched: Option<u64>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AddRssFeedRequest {
    /// The URL of an RSS 2.0 or Atom feed.
    pub url: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PlaylistPrivacy {
    Public,
    #[default]
    Private,
    Unlisted,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreatePlaylistRequest {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub privacy: PlaylistPrivacy,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdatePlaylistRequest {
    pub title: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AddPlaylistTracksRequest {
    /// The provider the track IDs belong to. Old payloads without
    /// the field read as YouTube.
    #[serde(default)]
    pub provider: ProviderId,
    pub track_ids: Vec<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RemovePlaylistTracksRequest {
    pub indices: Vec<usize>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MovePlaylistTrackRequest {
    pub from: usize,
    pub to: usize,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OkResponse {
    pub ok: bool,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchResults {
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PlaybackState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RepeatMode {
    #[default]
    Off,
    One,
    All,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct PlaybackSnapshot {
    pub state: PlaybackState,
    pub current: Option<Track>,
    pub position_secs: f64,
    pub volume: u8,
    pub repeat: RepeatMode,
    pub shuffle: bool,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
#[ts(export)]
pub enum DownloadStatus {
    NotDownloaded,
    Downloading { progress: u8 },
    Downloaded,
    Retrying { attempt: u32 },
    Failed { message: Option<String> },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QueueItem {
    pub track: Track,
    pub download: DownloadStatus,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct Queue {
    pub items: Vec<QueueItem>,
    pub current: Option<usize>,
    pub repeat: RepeatMode,
    pub shuffle: bool,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AuthState {
    #[default]
    Ok,
    Missing,
    Invalid,
    /// The stored session is well-formed, but YouTube Music rejected it.
    Expired,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct AuthStatus {
    pub state: AuthState,
    pub source: Option<String>,
    pub message: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct DownloadSummary {
    pub queued: usize,
    pub active: usize,
    pub completed: usize,
    pub retrying: usize,
    pub failed: usize,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CacheScope {
    Downloads,
    Covers,
    #[default]
    All,
}

/// Sizes of the daemon cache, in bytes and files. Sizes come from
/// the cache directories on disk.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct CacheSizes {
    #[ts(type = "number")]
    pub downloads_bytes: u64,
    #[ts(type = "number")]
    pub downloads_files: u64,
    #[ts(type = "number")]
    pub covers_bytes: u64,
    #[ts(type = "number")]
    pub covers_files: u64,
}

/// Request body for `POST /api/cache/clean`. The scope defaults to
/// `all` when it is missing.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct CacheCleanRequest {
    #[serde(default)]
    pub scope: CacheScope,
}

/// Result of `POST /api/cache/clean`. `freed_bytes` is the sum of
/// the download and cover bytes freed.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CacheCleanResult {
    pub scope: CacheScope,
    #[ts(type = "number")]
    pub downloads_freed_bytes: u64,
    #[ts(type = "number")]
    pub downloads_deleted_files: u64,
    #[ts(type = "number")]
    pub covers_freed_bytes: u64,
    #[ts(type = "number")]
    pub covers_deleted_files: u64,
    #[ts(type = "number")]
    pub freed_bytes: u64,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Status {
    pub name: String,
    pub version: String,
    pub status: String,
    #[ts(type = "number")]
    pub uptime_seconds: u64,
    pub auth: AuthStatus,
    pub playback: PlaybackSnapshot,
    pub queue_length: usize,
    pub downloads: DownloadSummary,
    /// The providers the daemon knows. Old payloads without the
    /// field read as an empty list.
    #[serde(default)]
    pub providers: Vec<ProviderId>,
    /// The default provider for a fresh install. Old payloads
    /// without the field read as YouTube.
    #[serde(default)]
    pub default_provider: ProviderId,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ErrorEnvelope {
    pub error: ApiError,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlayPlaylistRequest {
    pub playlist_id: String,
    /// The provider the playlist belongs to. Old payloads without
    /// the field read as YouTube.
    #[serde(default)]
    pub provider: ProviderId,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlayTrackRequest {
    pub track: Track,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QueueRequest {
    pub track: Track,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RemoveQueueRequest {
    pub index: usize,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlayNextRequest {
    pub index: usize,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShuffleRequest {
    pub enabled: bool,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RepeatRequest {
    pub repeat: RepeatMode,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RetryDownloadRequest {
    /// The provider the track ID belongs to. Old payloads without
    /// the field read as YouTube.
    #[serde(default)]
    pub provider: ProviderId,
    pub track_id: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VolumeRequest {
    pub volume: u8,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SeekRequest {
    pub position_secs: f64,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReorderQueueRequest {
    pub from: usize,
    pub to: usize,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum EventKind {
    Snapshot {
        status: Box<Status>,
        queue: Queue,
    },
    Playback {
        playback: PlaybackSnapshot,
    },
    Queue {
        queue: Queue,
    },
    Download {
        provider: ProviderId,
        track_id: String,
        status: DownloadStatus,
    },
    TrackEnded {
        provider: ProviderId,
        track_id: String,
    },
    Error {
        error: ApiError,
    },
    LibraryChanged,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Event {
    #[ts(type = "number")]
    pub id: u64,
    pub event: EventKind,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Track {
        Track {
            provider: ProviderId::Youtube,
            track_id: "v1".to_string(),
            title: "Song".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
        }
    }

    #[test]
    fn track_round_trip() {
        let track = track();
        let json = serde_json::to_string(&track).unwrap();
        assert_eq!(serde_json::from_str::<Track>(&json).unwrap(), track);
    }

    #[test]
    fn playlist_requests_round_trip() {
        let create = CreatePlaylistRequest {
            title: "Mix".to_string(),
            description: String::new(),
            privacy: PlaylistPrivacy::Private,
        };
        let json = serde_json::to_string(&create).unwrap();
        assert_eq!(
            serde_json::from_str::<CreatePlaylistRequest>(&json).unwrap(),
            create
        );
        assert_eq!(
            serde_json::to_value(PlaylistPrivacy::Unlisted).unwrap(),
            serde_json::json!("unlisted")
        );

        let update = UpdatePlaylistRequest {
            title: "New name".to_string(),
        };
        let json = serde_json::to_string(&update).unwrap();
        assert_eq!(
            serde_json::from_str::<UpdatePlaylistRequest>(&json).unwrap(),
            update
        );
    }

    #[test]
    fn download_status_is_tagged() {
        let status = DownloadStatus::Downloading { progress: 42 };
        let json = serde_json::to_value(status).unwrap();
        assert_eq!(json["state"], "downloading");
        assert_eq!(json["progress"], 42);
    }

    #[test]
    fn event_is_tagged() {
        let event = Event {
            id: 7,
            event: EventKind::Download {
                provider: ProviderId::Youtube,
                track_id: "v1".to_string(),
                status: DownloadStatus::Downloaded,
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["id"], 7);
        assert_eq!(json["event"]["type"], "download");
        assert_eq!(json["event"]["status"]["state"], "downloaded");
    }

    #[test]
    fn track_reads_the_old_video_id_field() {
        // Payloads written before the provider model have `video_id`
        // and no `provider`. They still load, as YouTube.
        let json = r#"{"video_id":"dQw4w9WgXcQ","title":"Song","artist":"Artist","album":"Album","duration":"3:00"}"#;
        let track: Track = serde_json::from_str(json).unwrap();
        assert_eq!(track.track_id, "dQw4w9WgXcQ");
        assert_eq!(track.provider, ProviderId::Youtube);
    }

    #[test]
    fn provider_info_round_trip() {
        let info = ProviderInfo::new(
            ProviderId::Youtube,
            AuthStatus {
                state: AuthState::Ok,
                source: Some("headers.txt".to_string()),
                message: None,
            },
        );
        assert_eq!(info.name, "YouTube Music");
        assert!(info.default);
        assert!(info.capabilities.search);
        assert!(info.capabilities.playlist_edit);
        let json = serde_json::to_string(&info).unwrap();
        assert_eq!(serde_json::from_str::<ProviderInfo>(&json).unwrap(), info);
    }

    #[test]
    fn status_round_trip() {
        let status = Status {
            name: "ytermusicd".to_string(),
            version: "0.1.0".to_string(),
            status: "ok".to_string(),
            uptime_seconds: 12,
            auth: AuthStatus::default(),
            playback: PlaybackSnapshot::default(),
            queue_length: 0,
            downloads: DownloadSummary::default(),
            providers: vec![ProviderId::Youtube],
            default_provider: ProviderId::Youtube,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(serde_json::from_str::<Status>(&json).unwrap(), status);
    }
}
