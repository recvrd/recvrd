//! Shared DTOs for the recvrd HTTP/SSE API.
//!
//! These types are the API contract. The web UI and MCP proxy should consume
//! the generated TypeScript bindings rather than duplicating shapes.

pub mod music;

pub use common_structs::{
    MediaKind, PlaybackMode, PlaybackSourceKind, ProviderId, TrackKey, TrackMetadata, TrackSource,
};
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
    /// The provider supports the like state of a track.
    pub track_like: bool,
    /// The provider supports downloads.
    pub downloads: bool,
    /// The provider supports cover art.
    pub covers: bool,
}

impl ProviderCapabilities {
    /// The capabilities of one provider. YouTube Music supports
    /// every action today. RSS feeds support search, library,
    /// playlists, downloads and covers, but not playlist edits:
    /// feeds are read-only. Podverse supports channel search,
    /// playlists, downloads and covers, but no library or edits.
    /// A provider that lacks an action must set its flag to false,
    /// so clients can hide the action.
    pub fn for_provider(provider: ProviderId) -> Self {
        match provider {
            ProviderId::Youtube => Self {
                search: true,
                library: true,
                playlists: true,
                playlist_create: true,
                playlist_edit: true,
                track_like: true,
                downloads: true,
                covers: true,
            },
            ProviderId::Rss => Self {
                search: true,
                library: true,
                playlists: true,
                playlist_create: false,
                playlist_edit: false,
                track_like: false,
                downloads: true,
                covers: true,
            },
            ProviderId::Podverse => Self {
                search: true,
                library: false,
                playlists: true,
                playlist_create: false,
                playlist_edit: false,
                track_like: false,
                downloads: true,
                covers: true,
            },
            ProviderId::Local => Self {
                search: true,
                library: true,
                playlists: true,
                playlist_create: false,
                playlist_edit: false,
                track_like: false,
                downloads: false,
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
    /// The current playback mode.
    pub playback: PlaybackMode,
    /// The playback modes this provider supports.
    pub playback_modes: Vec<PlaybackMode>,
}

impl ProviderInfo {
    /// Build the description of one provider. The caller sets the
    /// default flag from the available provider list.
    pub fn new(id: ProviderId, auth: AuthStatus, playback: PlaybackMode) -> Self {
        Self {
            id,
            name: id.display_name().to_string(),
            default: false,
            capabilities: ProviderCapabilities::for_provider(id),
            auth,
            playback,
            playback_modes: PlaybackMode::supported_for(id).to_vec(),
        }
    }
}

impl ProviderInfo {
    /// Build the description of one provider with its default mode.
    pub fn with_defaults(id: ProviderId, auth: AuthStatus) -> Self {
        Self::new(id, auth, PlaybackMode::default_for(id))
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlaybackModeRequest {
    /// The requested playback mode.
    pub mode: PlaybackMode,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OfflineRequest {
    /// Whether offline mode is on.
    pub enabled: bool,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
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
    /// The remote media source, when the provider supplies one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Box<TrackSource>>,
    /// Provider metadata for the track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Box<TrackMetadata>>,
    /// True when a local audio file exists for this track. The
    /// daemon sets this field on list responses. Providers leave
    /// it false. Clients use it to show a track as cached.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cached: bool,
    /// True when the upstream account liked the track. Absent when
    /// the provider does not report a like state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liked: Option<bool>,
}

impl Track {
    /// The provider-scoped key of this track.
    pub fn key(&self) -> TrackKey {
        TrackKey::new(self.provider, self.track_id.clone())
    }
}

/// One finished or abandoned play from the listen history,
/// newest first. `played_secs` counts the seconds of the track
/// played, from the start position to the position reached.
/// `completed` is true only when the track ended on its own.
/// `started_at` holds seconds since the Unix epoch.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HistoryEntry {
    pub provider: ProviderId,
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: String,
    #[ts(type = "number")]
    pub started_at: u64,
    pub played_secs: f64,
    pub completed: bool,
}

/// True when the value is false. Serde uses this helper to omit a
/// false boolean from JSON.
fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub subtitle: String,
    /// The cover image URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// The channel or playlist author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// A short description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The feed URL for a channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed_url: Option<String>,
    /// The number of items in the channel or playlist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_count: Option<u32>,
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
pub struct TrackDetail {
    pub track: Track,
    /// The full description. Providers that do not serve one omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Technical facts for one track.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrackStats {
    pub track: Track,
    /// The provider source with the URL query removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<TrackSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<TrackFileStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<TrackAudioStats>,
    pub playback: TrackPlaybackStats,
    pub history: TrackHistoryStats,
}

/// The local file behind one track.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrackFileStats {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidecar_path: Option<String>,
    #[ts(type = "number")]
    pub size_bytes: u64,
    #[ts(type = "number | null")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_unix: Option<u64>,
    pub cached: bool,
}

/// Audio header facts for one file.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct TrackAudioStats {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bits_per_sample: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
}

/// Playback facts for one track.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrackPlaybackStats {
    pub mode: PlaybackMode,
    pub state: PlaybackState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<PlaybackSourceKind>,
    pub position_secs: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_position_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_index: Option<usize>,
}

/// Listen history for one track.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrackHistoryStats {
    pub play_count: u32,
    #[ts(type = "number | null")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_played_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_completed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_played_secs: Option<f64>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetTrackLikeRequest {
    /// True adds the track to Liked Music. False removes it.
    pub liked: bool,
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

/// The name for a daemon-local playlist.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LocalPlaylistNameRequest {
    pub name: String,
}

/// Add one full track to a daemon-local playlist.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AddLocalPlaylistTrackRequest {
    pub track: Track,
}

/// Remove one track from a daemon-local playlist by index.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RemoveLocalPlaylistTrackRequest {
    pub index: usize,
}

/// Reorder one track in a daemon-local playlist.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReorderLocalPlaylistTrackRequest {
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
    /// The source that is currently playing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<PlaybackSourceKind>,
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
    /// Whether the daemon is in offline mode.
    #[serde(default)]
    pub offline: bool,
    /// The providers the daemon knows. Old payloads without the
    /// field read as an empty list.
    #[serde(default)]
    pub providers: Vec<ProviderId>,
    /// The default provider for a fresh install. Old payloads
    /// without the field read as YouTube.
    #[serde(default)]
    pub default_provider: ProviderId,
    /// The output sample rate in Hz, when the sink reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_sample_rate: Option<u32>,
    /// The output sample format name, when the sink reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_format: Option<String>,
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

/// Where to insert a queued track.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum QueuePosition {
    /// Insert after the current track.
    Next,
    /// Append to the end of the queue.
    #[default]
    Last,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QueueRequest {
    pub track: Track,
    /// Where to insert the track. The default is `last`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<QueuePosition>,
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
    /// The manual offline switch changed.
    Offline {
        enabled: bool,
    },
    LibraryChanged,
}

/// One audio spectrum frame for the visualizer.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct VisualizerFrame {
    pub bands: Vec<u8>,
    pub peak: u8,
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
            ..Default::default()
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
    fn local_playlist_requests_round_trip() {
        let name = LocalPlaylistNameRequest {
            name: "Mix".to_string(),
        };
        let json = serde_json::to_string(&name).unwrap();
        assert_eq!(
            serde_json::from_str::<LocalPlaylistNameRequest>(&json).unwrap(),
            name
        );

        let add = AddLocalPlaylistTrackRequest { track: track() };
        let json = serde_json::to_string(&add).unwrap();
        assert_eq!(
            serde_json::from_str::<AddLocalPlaylistTrackRequest>(&json).unwrap(),
            add
        );

        let remove = RemoveLocalPlaylistTrackRequest { index: 2 };
        let json = serde_json::to_string(&remove).unwrap();
        assert_eq!(
            serde_json::from_str::<RemoveLocalPlaylistTrackRequest>(&json).unwrap(),
            remove
        );

        let reorder = ReorderLocalPlaylistTrackRequest { from: 0, to: 2 };
        let json = serde_json::to_string(&reorder).unwrap();
        assert_eq!(
            serde_json::from_str::<ReorderLocalPlaylistTrackRequest>(&json).unwrap(),
            reorder
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
            ProviderId::Rss,
            AuthStatus {
                state: AuthState::Ok,
                source: Some("headers.txt".to_string()),
                message: None,
            },
            PlaybackMode::Download,
        );
        assert_eq!(info.name, "RSS");
        assert!(!info.default);
        assert!(info.capabilities.search);
        assert!(!info.capabilities.playlist_edit);
        let json = serde_json::to_string(&info).unwrap();
        assert_eq!(serde_json::from_str::<ProviderInfo>(&json).unwrap(), info);
    }

    #[test]
    fn status_round_trip() {
        let status = Status {
            name: "recvrd".to_string(),
            version: "0.1.0".to_string(),
            status: "ok".to_string(),
            uptime_seconds: 12,
            auth: AuthStatus::default(),
            playback: PlaybackSnapshot::default(),
            queue_length: 0,
            downloads: DownloadSummary::default(),
            offline: false,
            providers: vec![ProviderId::Youtube],
            default_provider: ProviderId::Youtube,
            audio_sample_rate: None,
            audio_format: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(serde_json::from_str::<Status>(&json).unwrap(), status);
    }
}
