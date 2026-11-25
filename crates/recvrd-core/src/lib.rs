//! Headless core for `recvrd`: paths, configuration, authentication,
//! playback/queue state and the event bus.

pub mod auth;
pub mod config;
pub mod cover;
pub mod events;
pub mod history;
pub mod model;
pub mod paths;
pub mod playback;
pub mod settings;
pub mod state;

pub use auth::{Auth, AuthError, AuthState, AuthStatus, LoadedAuth, resolve_auth};
pub use config::{ConfigError, DaemonConfig, Downloader, PodverseConfig, is_loopback_bind};
pub use cover::CoverFetcher;
pub use events::{CoreEvent, EventBus, EventEnvelope, Snapshot};
pub use history::{
    HISTORY_MAX_ENTRIES, HISTORY_MIN_PLAY_SECS, HISTORY_RETENTION_SECS, History, HistoryEntry,
};
pub use model::{
    PlaybackSnapshot, PlaybackStatus, Queue, QueueItem, QueueSnapshot, RepeatMode, Track,
};
pub use paths::{Paths, PathsError};
pub use playback::{
    AudioSink, NoStreamFactory, PlaybackCommand, PlaybackError, PlaybackService, PlaybackSource,
    PositionSyncEvent, StreamFactory, StreamReader,
};
pub use settings::{PlaybackSettings, PlaybackSettingsStore, SETTINGS_VERSION};
pub use state::{PersistedState, STATE_VERSION, StateError};
