//! Events and commands that connect the terminal UI to the daemon.

use image::DynamicImage;
use recvrd_api::{
    ApiError, DownloadStatus, Event, EventKind, PlaybackSnapshot, Playlist, PlaylistDetail,
    ProviderId, ProviderInfo, Queue, RepeatMode, SearchResults, Status, Track, TrackKey,
};
use recvrd_client::DaemonError;

/// A message from the daemon worker to the UI.
#[derive(Debug)]
pub enum DaemonEvent {
    /// The event stream is open.
    Connected,
    /// The event stream stopped. A reconnect follows.
    Disconnected(DaemonError),
    /// A full state snapshot.
    Snapshot { status: Status, queue: Queue },
    /// A playback update.
    Playback(PlaybackSnapshot),
    /// A full queue update.
    Queue(Queue),
    /// A download status update.
    Download {
        key: TrackKey,
        status: DownloadStatus,
    },
    /// A track ended. Playback events follow.
    TrackEnded,
    /// The daemon reported an error.
    Error(ApiError),
    /// The library changed on the daemon.
    LibraryChanged,
    /// A status response from a command.
    Status(Status),
    /// A playlist list response.
    Playlists(Vec<Playlist>),
    /// A playlist detail response.
    Playlist(PlaylistDetail),
    /// A playlist request failed.
    PlaylistError { id: String, message: String },
    /// The provider list response.
    Providers(Vec<ProviderInfo>),
    /// A track list response for one playlist.
    PlaylistTracks { id: String, tracks: Vec<Track> },
    /// A search response.
    Search {
        query: String,
        results: SearchResults,
    },
    /// A search request failed.
    SearchError { query: String, message: String },
    /// A cover image response. The decoded image is `None` when the
    /// fetch or decode failed; the UI clears the request so the next
    /// playback event retries it.
    Cover {
        key: TrackKey,
        image: Option<DynamicImage>,
    },
    /// A user-visible message.
    Toast { level: ToastLevel, message: String },
}

impl DaemonEvent {
    /// Convert a daemon event to a UI event.
    pub fn from_event(event: Event) -> Self {
        match event.event {
            EventKind::Snapshot { status, queue } => Self::Snapshot {
                status: *status,
                queue,
            },
            EventKind::Playback { playback } => Self::Playback(playback),
            EventKind::Queue { queue } => Self::Queue(queue),
            EventKind::Download {
                provider,
                track_id,
                status,
            } => Self::Download {
                key: TrackKey::new(provider, track_id),
                status,
            },
            EventKind::TrackEnded { .. } => Self::TrackEnded,
            EventKind::Error { error } => Self::Error(error),
            EventKind::LibraryChanged => Self::LibraryChanged,
        }
    }
}

/// A request from the UI to the daemon worker.
#[derive(Debug, Clone)]
pub enum Command {
    /// Reload all playlists.
    RefreshPlaylists,
    /// Switch the active provider for library and search requests.
    SetProvider(ProviderId),
    /// Reload the provider list.
    RefreshProviders,
    /// Open one playlist.
    OpenPlaylist(String),
    /// Reload one playlist.
    RefreshPlaylist(String),
    /// Replace the queue and play a playlist.
    PlayPlaylist(String),
    /// Play one track now.
    PlayTrack(Track),
    /// Add one track to the queue.
    QueueTrack(Track),
    /// Pause playback.
    Pause,
    /// Resume playback.
    Resume,
    /// Skip to the next track.
    Next,
    /// Skip to the previous track.
    Previous,
    /// Seek to a position in seconds.
    Seek(f64),
    /// Set the volume from 0 to 100.
    Volume(u8),
    /// Enable or disable shuffle.
    Shuffle(bool),
    /// Set the repeat mode.
    Repeat(RepeatMode),
    /// Remove one queue item.
    QueueRemove(usize),
    /// Move one queue item.
    QueueReorder { from: usize, to: usize },
    /// Move one queue item to play next.
    QueuePlayNext(usize),
    /// Remove all queue items.
    QueueClear,
    /// Retry a failed download.
    RetryDownload(TrackKey),
    /// Create a playlist.
    CreatePlaylist { title: String },
    /// Rename a playlist.
    RenamePlaylist { id: String, title: String },
    /// Delete a playlist.
    DeletePlaylist(String),
    /// Remove one playlist track.
    RemovePlaylistTrack { id: String, index: usize },
    /// Move one playlist track.
    MovePlaylistTrack { id: String, from: usize, to: usize },
    /// Add one or more tracks to a playlist.
    AddPlaylistTracks {
        id: String,
        provider: ProviderId,
        track_ids: Vec<String>,
    },
    /// Search tracks and playlists.
    Search(String),
    /// Load cover art for a track.
    LoadCover(TrackKey),
}

/// The severity of a toast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    /// A neutral message.
    Info,
    /// A success message.
    Success,
    /// An error message.
    Error,
}
