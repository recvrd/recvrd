use crate::{
    events::{CoreEvent, EventBus, Snapshot},
    history::{HISTORY_MIN_PLAY_SECS, History, HistoryEntry, now_unix},
    model::{PlaybackSnapshot, PlaybackStatus, Queue, RepeatMode, Track},
    settings::PlaybackSettingsStore,
    state::{PersistedState, STATE_VERSION, StateError},
};
use async_trait::async_trait;
use common_structs::{PlaybackMode, PlaybackSourceKind, ReadSeek, TrackKey, TrackSource};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;

/// Delay before the playback service retries a track after a failed restart.
const STREAM_RESTART_RETRY: Duration = Duration::from_secs(5);
/// Longest delay between playback restart attempts.
const STREAM_RESTART_MAX_DELAY: Duration = Duration::from_secs(60);
/// A restored position within this margin of the end of the track
/// counts as the end. Resume starts from zero instead.
const RESUME_END_MARGIN: Duration = Duration::from_secs(5);

/// A play in progress: the track, when it started, and the
/// position it started from. The history records the play when it
/// ends.
struct CurrentPlay {
    track: Track,
    started_at: u64,
    start_position: Duration,
}

/// A stream reader that the daemon opens before it starts playback.
pub struct StreamReader {
    /// The blocking reader for the audio decoder.
    pub reader: Box<dyn ReadSeek + Send + Sync>,
    /// The MIME type of the stream.
    pub mime: Option<String>,
    /// The byte length of the stream.
    pub length: Option<u64>,
    /// Whether the stream supports seek requests.
    pub seekable: bool,
}

/// The media that the audio sink must play.
pub enum PlaybackSource {
    /// A local cache file.
    File(PathBuf),
    /// A remote stream with an open reader.
    Stream(StreamReader),
}

/// Opens a remote stream without blocking the audio thread.
#[async_trait]
#[allow(clippy::double_must_use)]
pub trait StreamFactory: Send + Sync {
    /// Open a reader for a track source.
    async fn open(&self, source: &TrackSource) -> Result<StreamReader, String>;
}

/// A stream factory for tests and clients without streaming support.
pub struct NoStreamFactory;

#[async_trait]
impl StreamFactory for NoStreamFactory {
    async fn open(&self, _source: &TrackSource) -> Result<StreamReader, String> {
        Err("streaming is not available".to_string())
    }
}

/// Abstraction over the audio backend so playback can be tested without a
/// device, and swapped for a real implementation in the daemon.
pub trait AudioSink {
    type Error: std::error::Error + Send + Sync + 'static;

    fn play(&mut self, track: &Track, source: PlaybackSource) -> Result<(), Self::Error>;
    fn pause(&mut self) -> Result<(), Self::Error>;
    fn resume(&mut self) -> Result<(), Self::Error>;
    fn stop(&mut self) -> Result<(), Self::Error>;
    fn seek(&mut self, position: Duration) -> Result<(), Self::Error>;
    fn set_volume(&mut self, volume: u8) -> Result<(), Self::Error>;
    /// Total length of the loaded media, when the sink knows it.
    fn duration(&self) -> Option<Duration> {
        None
    }
    /// Whether the current track has finished. Real sinks should implement
    /// this; test sinks can leave the default.
    fn is_finished(&self) -> bool {
        false
    }

    /// Return and clear the flag that reports a failed output stream.
    ///
    /// The playback service uses this flag to restart the current track
    /// after the sink rebuilds its output.
    fn take_stream_error(&mut self) -> bool {
        false
    }

    /// The last known playback position of the sink, when it knows it.
    fn position(&self) -> Option<Duration> {
        None
    }
}

/// Commands accepted by the playback service from HTTP, MPRIS and MCP.
#[derive(Debug, Clone)]
pub enum PlaybackCommand {
    PlayPlaylist(Vec<Track>),
    PlayTrack(Track),
    Enqueue(Track),
    Remove(usize),
    MoveItem { from: usize, to: usize },
    Clear,
    Stop,
    PlayIndex(usize),
    Pause,
    Resume,
    Toggle,
    Next,
    Previous,
    Seek(Duration),
    SeekBy(f64),
    SetVolume(u8),
    SetShuffle(bool),
    SetRepeat(RepeatMode),
    SetOffline(bool),
}

#[derive(Debug, Error)]
pub enum PlaybackError {
    #[error("audio sink error: {0}")]
    Sink(String),
    #[error("no current track")]
    NoCurrentTrack,
    #[error("queue index out of bounds")]
    IndexOutOfBounds,
    #[error("invalid seek position")]
    InvalidPosition,
    #[error("unsupported playback mode: {0}")]
    UnsupportedMode(String),
    #[error("track is not available offline")]
    NotCached,
}

/// Headless playback service. Owns the queue, playback state and event bus.
pub struct PlaybackService<S: AudioSink> {
    queue: Queue,
    status: PlaybackStatus,
    position: Duration,
    started_at: Option<Instant>,
    volume: u8,
    sink: S,
    cache_dir: PathBuf,
    events: EventBus,
    wait_for_downloads: bool,
    pending_start: bool,
    offline: bool,
    settings: Arc<PlaybackSettingsStore>,
    streams: Arc<dyn StreamFactory>,
    source_kind: Option<PlaybackSourceKind>,
    restart_pending: Option<Duration>,
    /// Saved position to resume at on the first start after a
    /// restore. A tick never consumes this position: the daemon
    /// stays stopped until the user presses play.
    resume_pending: Option<Duration>,
    restart_at: Option<Instant>,
    restart_delay: Duration,
    /// Listen history, newest first.
    history: History,
    /// History file. The daemon sets this at startup; without it
    /// the history stays in memory.
    history_path: Option<PathBuf>,
    /// The play in progress, if any.
    current_play: Option<CurrentPlay>,
}

impl<S: AudioSink> PlaybackService<S> {
    pub fn new(sink: S, cache_dir: impl Into<PathBuf>, initial_volume: u8) -> Self {
        Self::with_streams(
            sink,
            cache_dir,
            initial_volume,
            Arc::new(PlaybackSettingsStore::new(
                PathBuf::new(),
                crate::settings::PlaybackSettings::default(),
            )),
            Arc::new(NoStreamFactory),
        )
    }

    pub fn with_streams(
        sink: S,
        cache_dir: impl Into<PathBuf>,
        initial_volume: u8,
        settings: Arc<PlaybackSettingsStore>,
        streams: Arc<dyn StreamFactory>,
    ) -> Self {
        let queue = Queue::new();
        let events = EventBus::new(Snapshot {
            playback: PlaybackSnapshot::default(),
            queue: queue.snapshot(),
        });
        Self {
            queue,
            status: PlaybackStatus::Stopped,
            position: Duration::ZERO,
            started_at: None,
            volume: initial_volume.min(100),
            sink,
            cache_dir: cache_dir.into(),
            events,
            wait_for_downloads: false,
            pending_start: false,
            offline: false,
            settings,
            streams,
            source_kind: None,
            restart_pending: None,
            resume_pending: None,
            restart_at: None,
            restart_delay: STREAM_RESTART_RETRY,
            history: History::new(),
            history_path: None,
            current_play: None,
        }
    }

    /// Return the current playback mode for a provider.
    pub fn playback_mode(&self, provider: common_structs::ProviderId) -> PlaybackMode {
        self.settings.mode(provider)
    }

    /// Set the playback mode for a provider.
    pub fn set_playback_mode(
        &self,
        provider: common_structs::ProviderId,
        mode: PlaybackMode,
    ) -> Result<(), PlaybackError> {
        if !PlaybackMode::supported_for(provider).contains(&mode) {
            return Err(PlaybackError::UnsupportedMode(format!(
                "{mode:?} for {provider}"
            )));
        }
        self.settings
            .set_mode(provider, mode)
            .map_err(|error| PlaybackError::Sink(error.to_string()))
    }

    /// Whether offline mode is on.
    pub fn is_offline(&self) -> bool {
        self.offline
    }

    /// Turn offline mode on or off. While offline, the service only
    /// starts cached tracks. Enabling the mode stops an uncached
    /// current track and advances to the next cached track.
    pub async fn set_offline(&mut self, enabled: bool) -> Result<(), PlaybackError> {
        self.offline = enabled;
        self.events.publish(CoreEvent::Offline { enabled });
        if enabled
            && self
                .queue
                .current_track()
                .is_some_and(|track| !self.is_cached(track))
        {
            return self.start_next_cached().await;
        }
        self.publish_all();
        Ok(())
    }

    /// Check whether a track has a cached audio file. A track counts
    /// as cached only with a valid key and a non-empty audio file.
    fn is_cached(&self, track: &Track) -> bool {
        let key = track.key();
        if !key.validate() {
            return false;
        }
        let Some(path) = key.existing_audio_file(&self.cache_dir.join("downloads")) else {
            return false;
        };
        std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > 0)
    }

    /// The next cached track after the current track. With repeat all,
    /// the search wraps to the start of the queue once.
    fn next_cached_index(&self) -> Option<usize> {
        let len = self.queue.len();
        if len == 0 {
            return None;
        }
        let current = self.queue.current_index();
        let start = current.map_or(0, |index| index + 1);
        for index in start..len {
            if self.is_cached(&self.queue.items()[index].track) {
                return Some(index);
            }
        }
        if self.queue.repeat() == RepeatMode::All {
            let end = current.map_or(0, |index| index + 1);
            for index in 0..end {
                if self.is_cached(&self.queue.items()[index].track) {
                    return Some(index);
                }
            }
        }
        None
    }

    /// The previous cached track before the current track. With repeat
    /// all, the search wraps to the end of the queue once. With repeat
    /// off, a first cached track restarts itself like a normal
    /// previous.
    fn previous_cached_index(&self) -> Option<usize> {
        let len = self.queue.len();
        if len == 0 {
            return None;
        }
        let start = self.queue.current_index().unwrap_or(0);
        let mut index = start;
        while index > 0 {
            index -= 1;
            if self.is_cached(&self.queue.items()[index].track) {
                return Some(index);
            }
        }
        if self.queue.repeat() == RepeatMode::All {
            for index in (start..len).rev() {
                if self.is_cached(&self.queue.items()[index].track) {
                    return Some(index);
                }
            }
        } else if self.is_cached(&self.queue.items()[start].track) {
            return Some(start);
        }
        None
    }

    /// Start the next cached track, or stop playback when no cached
    /// track remains in the queue.
    async fn start_next_cached(&mut self) -> Result<(), PlaybackError> {
        match self.next_cached_index() {
            Some(index) => {
                self.queue.set_current(Some(index));
                self.start_current().await
            }
            None => {
                self.stop_playback();
                Ok(())
            }
        }
    }

    /// When enabled, tracks that are not in the download cache yet are queued
    /// for download and started automatically once the `Downloaded` status
    /// arrives. Tests and non-downloading clients leave this disabled.
    pub fn set_wait_for_downloads(&mut self, enabled: bool) {
        self.wait_for_downloads = enabled;
    }

    pub fn events(&self) -> EventBus {
        self.events.clone()
    }

    pub fn snapshot(&self) -> PlaybackSnapshot {
        PlaybackSnapshot {
            status: self.status,
            current_index: self.queue.current_index(),
            current: self.queue.current_track().cloned(),
            position_secs: self.current_position().as_secs_f64(),
            volume: self.volume,
            repeat: self.queue.repeat(),
            shuffle: self.queue.shuffle(),
            source_kind: self.source_kind,
        }
    }

    /// Load the listen history from the state directory. The daemon
    /// calls this at startup; without it the history stays in memory.
    pub fn set_history_path(&mut self, path: &Path) {
        self.history_path = Some(path.to_path_buf());
        self.history = History::load(path);
    }

    /// Listen history, newest first.
    pub fn history(&self) -> Vec<HistoryEntry> {
        self.history.entries()
    }

    /// Record the in-progress play when it ends. A play below the
    /// threshold leaves no record.
    fn record_current_play(&mut self, completed: bool) {
        let Some(play) = self.current_play.take() else {
            return;
        };
        let played = self.current_position().saturating_sub(play.start_position);
        if played.as_secs_f64() < HISTORY_MIN_PLAY_SECS {
            return;
        }
        let entry = HistoryEntry {
            provider: play.track.provider,
            track_id: play.track.track_id.clone(),
            title: play.track.title.clone(),
            author: play.track.author.clone(),
            album: play.track.album.clone(),
            duration: play.track.duration.clone(),
            started_at: play.started_at,
            played_secs: played.as_secs_f64(),
            completed,
        };
        let rewrote = self.history.push(entry.clone(), now_unix());
        if let Some(path) = self.history_path.clone() {
            let result = if rewrote {
                self.history.save(&path)
            } else {
                History::append_entry(&path, &entry)
            };
            if let Err(error) = result {
                log::warn!("could not write listen history {}: {error}", path.display());
            }
        }
    }

    /// Start the history record for a track that starts to play. A
    /// stream-error restart keeps the record of the play in progress.
    fn begin_play(&mut self, track: &Track, was_restart: bool) {
        if was_restart
            && self
                .current_play
                .as_ref()
                .is_some_and(|play| play.track.key() == track.key())
        {
            return;
        }
        self.current_play = Some(CurrentPlay {
            track: track.clone(),
            started_at: now_unix(),
            start_position: self.current_position(),
        });
    }

    /// Current position, including the time elapsed since the last play,
    /// resume or seek.
    pub fn current_position(&self) -> Duration {
        match self.started_at {
            Some(started_at) => self.position + started_at.elapsed(),
            None => self.position,
        }
    }

    /// Clamp a pending restart position to the current track duration.
    fn clamp_pending_position(&self, position: Duration) -> Duration {
        self.queue
            .current_track()
            .and_then(|track| parse_duration(&track.duration))
            .map_or(position, |duration| position.min(duration))
    }

    /// Clamp a restored position to the track duration. Return zero
    /// when the position is at or near the end of the track, so
    /// playback does not start at the end of the track.
    fn resume_position(&self, position: Duration) -> Duration {
        let position = self.clamp_pending_position(position);
        let at_end = self
            .queue
            .current_track()
            .and_then(|track| parse_duration(&track.duration))
            .is_some_and(|duration| position + RESUME_END_MARGIN >= duration);
        if at_end { Duration::ZERO } else { position }
    }

    fn freeze_position(&mut self) {
        self.position = self.current_position();
        self.started_at = None;
    }

    pub fn queue_snapshot(&self) -> crate::model::QueueSnapshot {
        self.queue.snapshot()
    }

    /// Poll the audio sink, publish progress and advance automatically when a
    /// track ends.
    pub async fn tick(&mut self) -> Result<(), PlaybackError> {
        if self.restart_pending.is_some() {
            self.try_pending_restart().await;
            if self.restart_pending.is_some() {
                return Ok(());
            }
        }

        if self.status == PlaybackStatus::Playing {
            if self.sink.take_stream_error() {
                let position = self
                    .sink
                    .position()
                    .unwrap_or_else(|| self.current_position());
                self.events.publish(CoreEvent::Error {
                    message: "the audio stream failed; restarting the track".to_string(),
                });
                self.restart_pending = Some(position);
                self.restart_at = Some(Instant::now());
                self.restart_delay = STREAM_RESTART_RETRY;
                self.try_pending_restart().await;
                return Ok(());
            }
            if self.sink.is_finished() {
                self.on_track_finished().await?;
            } else {
                self.publish_playback();
            }
        }
        Ok(())
    }

    /// Retry a pending restart after a failed output stream.
    async fn try_pending_restart(&mut self) {
        if self.restart_pending.is_none() {
            return;
        }
        if self.restart_at.is_some_and(|at| Instant::now() < at) {
            return;
        }

        match self.start_current().await {
            Ok(()) => {
                if self.restart_pending.is_some() {
                    if self.pending_start {
                        // The track waits for a download. The download
                        // completion applies the pending position.
                        self.restart_at = Some(Instant::now() + self.restart_delay);
                    } else {
                        self.restart_pending = None;
                        self.restart_at = None;
                        self.restart_delay = STREAM_RESTART_RETRY;
                    }
                } else {
                    self.restart_at = None;
                    self.restart_delay = STREAM_RESTART_RETRY;
                }
            }
            Err(_) => {
                self.restart_at = Some(Instant::now() + self.restart_delay);
                self.restart_delay = (self.restart_delay * 2).min(STREAM_RESTART_MAX_DELAY);
            }
        }
    }

    /// Apply a command from an API/MPRIS/MCP client.
    pub async fn handle_command(&mut self, command: PlaybackCommand) -> Result<(), PlaybackError> {
        let clears_restart = matches!(
            &command,
            PlaybackCommand::PlayPlaylist(_)
                | PlaybackCommand::PlayTrack(_)
                | PlaybackCommand::Stop
                | PlaybackCommand::PlayIndex(_)
                | PlaybackCommand::Next
                | PlaybackCommand::Previous
                | PlaybackCommand::Pause
                | PlaybackCommand::Clear
                | PlaybackCommand::SetOffline(_)
        );
        // Pause never clears the resume position. Pause does nothing
        // while stopped, and the position is already consumed while
        // playing. MPRIS Pause reaches this path, so it must keep
        // the position for the next play.
        let clears_resume = clears_restart && !matches!(&command, PlaybackCommand::Pause);
        if clears_restart {
            self.restart_pending = None;
            self.restart_at = None;
            self.restart_delay = STREAM_RESTART_RETRY;
        }
        if clears_resume {
            self.resume_pending = None;
        }

        match command {
            PlaybackCommand::PlayPlaylist(tracks) => self.play_playlist(tracks).await,
            PlaybackCommand::PlayTrack(track) => self.play_track(track).await,
            PlaybackCommand::Enqueue(track) => {
                self.enqueue(track);
                Ok(())
            }
            PlaybackCommand::Remove(index) => self.remove(index).await,
            PlaybackCommand::MoveItem { from, to } => {
                self.move_item(from, to);
                Ok(())
            }
            PlaybackCommand::Clear => self.clear(),
            PlaybackCommand::Stop => {
                self.stop();
                Ok(())
            }
            PlaybackCommand::PlayIndex(index) => self.play_index(index).await,
            PlaybackCommand::Pause => self.pause(),
            PlaybackCommand::Resume => self.play().await,
            PlaybackCommand::Toggle => self.toggle().await,
            PlaybackCommand::Next => self.next_track().await,
            PlaybackCommand::Previous => self.previous_track().await,
            PlaybackCommand::Seek(position) => {
                if self.restart_pending.is_some() {
                    self.restart_pending = Some(self.clamp_pending_position(position));
                    Ok(())
                } else if self.resume_pending.is_some() {
                    self.resume_pending = Some(self.clamp_pending_position(position));
                    Ok(())
                } else {
                    self.seek(position)
                }
            }
            PlaybackCommand::SeekBy(delta) => {
                if !delta.is_finite() {
                    return Err(PlaybackError::InvalidPosition);
                }
                let base = self
                    .restart_pending
                    .or(self.resume_pending)
                    .unwrap_or_else(|| self.current_position());
                let position = (base.as_secs_f64() + delta).max(0.0);
                let position = Duration::try_from_secs_f64(position)
                    .map_err(|_| PlaybackError::InvalidPosition)?;
                if self.restart_pending.is_some() {
                    self.restart_pending = Some(self.clamp_pending_position(position));
                    Ok(())
                } else if self.resume_pending.is_some() {
                    self.resume_pending = Some(self.clamp_pending_position(position));
                    Ok(())
                } else {
                    self.seek(position)
                }
            }
            PlaybackCommand::SetVolume(volume) => self.set_volume(volume),
            PlaybackCommand::SetShuffle(enabled) => {
                self.set_shuffle(enabled);
                Ok(())
            }
            PlaybackCommand::SetRepeat(repeat) => {
                self.set_repeat(repeat);
                Ok(())
            }
            PlaybackCommand::SetOffline(enabled) => self.set_offline(enabled).await,
        }
    }

    /// Snapshot of the state that should survive a restart.
    pub fn persisted_state(&self) -> PersistedState {
        PersistedState {
            version: STATE_VERSION,
            tracks: self
                .queue
                .items()
                .iter()
                .map(|item| item.track.clone())
                .collect(),
            original_tracks: self.queue.original_tracks(),
            current: self.queue.current_index(),
            repeat: self.queue.repeat(),
            shuffle: self.queue.shuffle(),
            volume: self.volume,
            position_secs: self.current_position().as_secs_f64(),
        }
    }

    pub fn save_state(&self, path: &Path) -> Result<(), StateError> {
        self.persisted_state().save(path)
    }

    pub fn load_state(&mut self, path: &Path) -> Result<(), StateError> {
        let state = PersistedState::load(path)?;
        self.restore_state(&state)
            .map_err(|error| StateError::Restore(error.to_string()))
    }

    pub fn restore_state(&mut self, state: &PersistedState) -> Result<(), PlaybackError> {
        self.queue.restore(
            state.tracks.clone(),
            state.original_tracks.clone(),
            state.current,
            state.repeat,
            state.shuffle,
        );
        self.volume = state.volume.min(100);
        // State validation checks finiteness only, so a hand-edited
        // state file can hold a huge finite value that overflows a
        // Duration. Fall back to zero instead of panicking.
        self.position =
            Duration::try_from_secs_f64(state.position_secs.max(0.0)).unwrap_or(Duration::ZERO);
        // Resume at the saved position on the first start after
        // restore. A tick never consumes this position: the daemon
        // stays stopped until the user presses play.
        self.position = self.resume_position(self.position);
        self.resume_pending = if self.position.is_zero() {
            None
        } else {
            Some(self.position)
        };
        self.restart_pending = None;
        self.restart_at = None;
        self.restart_delay = STREAM_RESTART_RETRY;
        self.started_at = None;
        self.pending_start = false;
        self.status = PlaybackStatus::Stopped;
        self.source_kind = None;
        self.current_play = None;
        self.sink
            .set_volume(self.volume)
            .map_err(|error| PlaybackError::Sink(error.to_string()))?;
        self.publish_all();
        Ok(())
    }

    pub async fn play_playlist(&mut self, tracks: Vec<Track>) -> Result<(), PlaybackError> {
        self.resume_pending = None;
        self.queue.replace(tracks);
        self.start_current().await
    }

    pub async fn play_track(&mut self, track: Track) -> Result<(), PlaybackError> {
        if self.offline && !self.is_cached(&track) {
            return Err(PlaybackError::NotCached);
        }
        self.resume_pending = None;
        self.queue.play_now(track);
        self.start_current().await
    }

    pub fn enqueue(&mut self, track: Track) {
        self.queue.enqueue(track);
        self.publish_queue();
    }

    pub async fn remove(&mut self, index: usize) -> Result<(), PlaybackError> {
        let was_current = self.queue.current_index() == Some(index);
        self.queue.remove(index);
        if was_current {
            self.restart_pending = None;
            self.resume_pending = None;
            self.restart_at = None;
            self.restart_delay = STREAM_RESTART_RETRY;
            self.start_current().await
        } else {
            self.publish_all();
            Ok(())
        }
    }

    pub fn move_item(&mut self, from: usize, to: usize) -> bool {
        let moved = self.queue.move_item(from, to);
        if moved {
            self.publish_all();
        }
        moved
    }

    pub fn clear(&mut self) -> Result<(), PlaybackError> {
        self.record_current_play(false);
        let _ = self.sink.stop();
        self.queue.clear();
        self.status = PlaybackStatus::Stopped;
        self.position = Duration::ZERO;
        self.source_kind = None;
        self.restart_pending = None;
        self.resume_pending = None;
        self.restart_at = None;
        self.restart_delay = STREAM_RESTART_RETRY;
        self.publish_all();
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stop_playback();
    }

    pub async fn play_index(&mut self, index: usize) -> Result<(), PlaybackError> {
        if self.offline {
            let Some(track) = self.queue.items().get(index).map(|item| &item.track) else {
                return Err(PlaybackError::IndexOutOfBounds);
            };
            if !self.is_cached(track) {
                return Err(PlaybackError::NotCached);
            }
        }
        if !self.queue.play_index(index) {
            return Err(PlaybackError::IndexOutOfBounds);
        }
        self.resume_pending = None;
        self.start_current().await
    }

    pub fn pause(&mut self) -> Result<(), PlaybackError> {
        if self.status == PlaybackStatus::Playing {
            self.sink
                .pause()
                .map_err(|error| PlaybackError::Sink(error.to_string()))?;
            self.freeze_position();
            self.status = PlaybackStatus::Paused;
            self.publish_playback();
        }
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), PlaybackError> {
        if self.status == PlaybackStatus::Paused {
            self.sink
                .resume()
                .map_err(|error| PlaybackError::Sink(error.to_string()))?;
            self.started_at = Some(Instant::now());
            self.status = PlaybackStatus::Playing;
            self.publish_playback();
        }
        Ok(())
    }

    /// Play: no-op while playing, resume while paused, and start the
    /// current track while stopped. This is the client "play" action;
    /// it keeps a restored `resume_pending` position for the first start.
    pub async fn play(&mut self) -> Result<(), PlaybackError> {
        match self.status {
            PlaybackStatus::Playing => Ok(()),
            PlaybackStatus::Paused => self.resume(),
            PlaybackStatus::Stopped => self.start_current().await,
        }
    }

    pub async fn toggle(&mut self) -> Result<(), PlaybackError> {
        match self.status {
            PlaybackStatus::Playing => self.pause(),
            PlaybackStatus::Paused => self.resume(),
            PlaybackStatus::Stopped => self.start_current().await,
        }
    }

    pub async fn next_track(&mut self) -> Result<(), PlaybackError> {
        self.resume_pending = None;
        if self.offline {
            return self.start_next_cached().await;
        }
        match self.queue.next_index() {
            Some(index) => {
                self.queue.set_current(Some(index));
                self.start_current().await
            }
            None => {
                self.stop_playback();
                Ok(())
            }
        }
    }

    pub async fn previous_track(&mut self) -> Result<(), PlaybackError> {
        self.resume_pending = None;
        if self.offline {
            return match self.previous_cached_index() {
                Some(index) => {
                    self.queue.set_current(Some(index));
                    self.start_current().await
                }
                None => {
                    self.stop_playback();
                    Ok(())
                }
            };
        }
        match self.queue.previous_index() {
            Some(index) => {
                self.queue.set_current(Some(index));
                self.start_current().await
            }
            None => {
                self.stop_playback();
                Ok(())
            }
        }
    }

    pub fn seek(&mut self, position: Duration) -> Result<(), PlaybackError> {
        self.sink
            .seek(position)
            .map_err(|error| PlaybackError::Sink(error.to_string()))?;
        self.position = position;
        self.started_at = if self.status == PlaybackStatus::Playing {
            Some(Instant::now())
        } else {
            None
        };
        self.publish_playback();
        Ok(())
    }

    pub fn set_volume(&mut self, volume: u8) -> Result<(), PlaybackError> {
        let volume = volume.min(100);
        self.sink
            .set_volume(volume)
            .map_err(|error| PlaybackError::Sink(error.to_string()))?;
        self.volume = volume;
        self.publish_playback();
        Ok(())
    }

    pub fn set_repeat(&mut self, repeat: RepeatMode) {
        self.queue.set_repeat(repeat);
        self.publish_all();
    }

    pub fn set_shuffle(&mut self, enabled: bool) {
        let seed = fastrand::u64(..);
        self.set_shuffle_with_seed(enabled, seed);
    }

    pub fn set_shuffle_with_seed(&mut self, enabled: bool, seed: u64) {
        self.queue.set_shuffle_with_seed(enabled, seed);
        self.publish_all();
    }

    pub async fn on_track_finished(&mut self) -> Result<(), PlaybackError> {
        self.record_current_play(true);
        if let Some(track) = self.queue.current_track() {
            self.events.publish(CoreEvent::TrackEnded {
                track_id: track.key(),
            });
        }
        if self.queue.repeat() == RepeatMode::One {
            self.start_current().await
        } else {
            self.next_track().await
        }
    }

    pub async fn on_download_failed(&mut self, key: &TrackKey) {
        self.events.publish(CoreEvent::Error {
            message: format!("download failed for {key}"),
        });
        if self
            .queue
            .current_track()
            .is_some_and(|track| track.key() == *key)
        {
            self.pending_start = false;
            self.restart_pending = None;
            self.resume_pending = None;
            self.restart_at = None;
            self.restart_delay = STREAM_RESTART_RETRY;
            let _ = self.next_track().await;
        }
    }

    /// Mark every queued track as not downloaded, except the kept
    /// track, and publish the new queue. Call this after the cached
    /// files are gone.
    pub fn reset_download_statuses(&mut self, keep: Option<&TrackKey>) {
        self.queue.reset_download_statuses(keep);
        self.publish_queue();
    }

    pub async fn set_download_status(
        &mut self,
        key: &TrackKey,
        status: common_structs::MusicDownloadStatus,
    ) {
        self.queue.set_download_status(key, status);
        self.events.publish(CoreEvent::DownloadStatus {
            track_id: key.clone(),
            status,
        });
        if status == common_structs::MusicDownloadStatus::Downloaded
            && self.pending_start
            && self
                .queue
                .current_track()
                .is_some_and(|track| track.key() == *key)
        {
            self.pending_start = false;
            let _ = self.start_current().await;
            return;
        }
        self.publish_queue();
    }

    async fn start_current(&mut self) -> Result<(), PlaybackError> {
        // Skip uncached tracks while offline. The loop advances the
        // current track until it finds a cached track or none remains.
        loop {
            let Some(track) = self.queue.current_track().cloned() else {
                self.stop_playback();
                return Ok(());
            };

            if self.offline && !self.is_cached(&track) {
                match self.next_cached_index() {
                    Some(index) => {
                        self.queue.set_current(Some(index));
                        continue;
                    }
                    None => {
                        self.stop_playback();
                        return Ok(());
                    }
                }
            }

            return self.start_current_inner(track).await;
        }
    }

    async fn start_current_inner(&mut self, track: Track) -> Result<(), PlaybackError> {
        // A new track ends the in-progress play. A stream-error
        // restart keeps the same play: the restart flag is still set.
        if self.restart_pending.is_none() {
            self.record_current_play(false);
        }
        if let Some(path) = track
            .key()
            .existing_audio_file(&self.cache_dir.join("downloads"))
        {
            return self.start_file(track, path);
        }

        let mode = self.settings.mode(track.provider);
        // A stream with no remote source cannot play. Fail now
        // instead of waiting for a download that never starts.
        if mode == PlaybackMode::Stream && track.source.is_none() {
            return self.fail_start("track has no stream source".to_string());
        }
        if matches!(
            mode,
            PlaybackMode::Stream | PlaybackMode::StreamThenDownload
        ) && let Some(source) = &track.source
        {
            match self.streams.open(source).await {
                Ok(reader) => return self.start_stream(track, reader),
                Err(error) => {
                    if mode == PlaybackMode::StreamThenDownload && self.wait_for_downloads {
                        self.wait_for_download();
                        return Ok(());
                    }
                    return self.fail_start(format!("could not open the stream: {error}"));
                }
            }
        }

        if self.wait_for_downloads {
            self.wait_for_download();
            return Ok(());
        }

        let path = track
            .key()
            .download_file(&self.cache_dir.join("downloads"), "mp4");
        self.start_file(track, path)
    }

    fn wait_for_download(&mut self) {
        self.pending_start = true;
        self.status = PlaybackStatus::Stopped;
        self.position = Duration::ZERO;
        self.started_at = None;
        self.source_kind = None;
        self.publish_all();
    }

    fn start_file(&mut self, track: Track, path: PathBuf) -> Result<(), PlaybackError> {
        self.pending_start = false;
        if let Err(error) = self.sink.play(&track, PlaybackSource::File(path)) {
            return self.fail_start(error.to_string());
        }
        self.finish_start(&track, PlaybackSourceKind::File)
    }

    fn start_stream(&mut self, track: Track, reader: StreamReader) -> Result<(), PlaybackError> {
        self.pending_start = false;
        if let Err(error) = self.sink.play(&track, PlaybackSource::Stream(reader)) {
            return self.fail_start(error.to_string());
        }
        self.finish_start(&track, PlaybackSourceKind::Stream)
    }

    fn finish_start(
        &mut self,
        track: &Track,
        source_kind: PlaybackSourceKind,
    ) -> Result<(), PlaybackError> {
        let was_restart = self.restart_pending.is_some();
        self.status = PlaybackStatus::Playing;
        self.position = Duration::ZERO;
        self.started_at = Some(Instant::now());
        self.source_kind = Some(source_kind);
        self.sink
            .set_volume(self.volume)
            .map_err(|error| PlaybackError::Sink(error.to_string()))?;
        if track.duration.is_empty()
            && let Some(duration) = self.sink.duration()
        {
            self.queue.set_current_duration(format_duration(duration));
        }
        self.publish_all();

        let resume_position = self.resume_pending.take();
        if let Some(position) = self.restart_pending.take().or(resume_position) {
            self.restart_at = None;
            self.restart_delay = STREAM_RESTART_RETRY;
            if !position.is_zero() {
                // A failed seek must not fail the start. A
                // non-seekable stream starts from zero instead.
                let _ = self.seek(position);
            }
        }
        self.begin_play(track, was_restart);
        Ok(())
    }

    fn fail_start(&mut self, message: String) -> Result<(), PlaybackError> {
        self.status = PlaybackStatus::Stopped;
        self.position = Duration::ZERO;
        self.started_at = None;
        self.source_kind = None;
        self.events.publish(CoreEvent::Error {
            message: message.clone(),
        });
        self.publish_all();
        Err(PlaybackError::Sink(message))
    }

    fn stop_playback(&mut self) {
        self.record_current_play(false);
        let _ = self.sink.stop();
        self.status = PlaybackStatus::Stopped;
        self.position = Duration::ZERO;
        self.started_at = None;
        self.pending_start = false;
        self.source_kind = None;
        self.restart_pending = None;
        self.resume_pending = None;
        self.restart_at = None;
        self.restart_delay = STREAM_RESTART_RETRY;
        self.publish_all();
    }

    fn publish_playback(&self) {
        self.events.publish(CoreEvent::Playback(self.snapshot()));
    }

    fn publish_queue(&self) {
        self.events.publish(CoreEvent::Queue(self.queue.snapshot()));
    }

    fn publish_all(&self) {
        self.publish_playback();
        self.publish_queue();
    }
}

fn format_duration(duration: Duration) -> String {
    let total = duration.as_secs_f64().round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Parse a `H:MM:SS` or `M:SS` duration string.
fn parse_duration(value: &str) -> Option<Duration> {
    if value.is_empty() {
        return None;
    }
    let mut seconds = 0u64;
    for part in value.split(':') {
        let part: u64 = part.trim().parse().ok()?;
        seconds = seconds.checked_mul(60)?.checked_add(part)?;
    }
    Some(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::CoreEvent;
    use async_trait::async_trait;
    use common_structs::MusicDownloadStatus;
    use std::path::PathBuf;

    #[derive(Default)]
    struct MockSink {
        plays: Vec<(String, PathBuf)>,
        sources: Vec<PlaybackSourceKind>,
        actions: Vec<&'static str>,
        volume: u8,
        finished: bool,
        stream_error: bool,
        position: Option<Duration>,
        play_failures: usize,
        seek_fails: bool,
        duration: Option<Duration>,
    }

    impl AudioSink for MockSink {
        type Error = std::io::Error;

        fn play(&mut self, track: &Track, source: PlaybackSource) -> Result<(), Self::Error> {
            if self.play_failures > 0 {
                self.play_failures -= 1;
                self.actions.push("play-failed");
                return Err(std::io::Error::other("play failed"));
            }
            let (kind, path) = match source {
                PlaybackSource::File(path) => (PlaybackSourceKind::File, path),
                PlaybackSource::Stream(_) => (PlaybackSourceKind::Stream, PathBuf::new()),
            };
            self.plays.push((track.track_id.clone(), path));
            self.sources.push(kind);
            self.actions.push("play");
            Ok(())
        }

        fn pause(&mut self) -> Result<(), Self::Error> {
            self.actions.push("pause");
            Ok(())
        }

        fn resume(&mut self) -> Result<(), Self::Error> {
            self.actions.push("resume");
            Ok(())
        }

        fn stop(&mut self) -> Result<(), Self::Error> {
            self.actions.push("stop");
            Ok(())
        }

        fn seek(&mut self, _position: Duration) -> Result<(), Self::Error> {
            self.actions.push("seek");
            if self.seek_fails {
                return Err(std::io::Error::other("seek failed"));
            }
            Ok(())
        }

        fn set_volume(&mut self, volume: u8) -> Result<(), Self::Error> {
            self.volume = volume;
            self.actions.push("volume");
            Ok(())
        }

        fn is_finished(&self) -> bool {
            self.finished
        }

        fn take_stream_error(&mut self) -> bool {
            std::mem::take(&mut self.stream_error)
        }

        fn position(&self) -> Option<Duration> {
            self.position
        }

        fn duration(&self) -> Option<Duration> {
            self.duration
        }
    }

    fn tracks(count: usize) -> Vec<Track> {
        (0..count)
            .map(|index| Track {
                provider: common_structs::ProviderId::Youtube,
                track_id: format!("video-{index}"),
                title: format!("Track {index}"),
                author: "Artist".to_string(),
                album: "Album".to_string(),
                duration: "3:00".to_string(),
                ..Default::default()
            })
            .collect()
    }

    fn service() -> PlaybackService<MockSink> {
        PlaybackService::new(MockSink::default(), "/tmp/recvrd-test", 50)
    }

    #[tokio::test]
    async fn play_playlist_starts_first_track() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        let snapshot = service.snapshot();
        assert_eq!(snapshot.status, PlaybackStatus::Playing);
        assert_eq!(snapshot.current.unwrap().track_id, "video-0");
        assert_eq!(service.queue_snapshot().current, Some(0));
        assert_eq!(
            service.events.snapshot().playback.status,
            PlaybackStatus::Playing
        );
    }

    #[tokio::test]
    async fn fills_missing_duration_from_the_sink() {
        let mut service = service();
        service.sink.duration = Some(Duration::from_secs(185));
        let mut playlist = tracks(1);
        playlist[0].duration.clear();
        service.play_playlist(playlist).await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().duration, "3:05");
        assert_eq!(service.queue_snapshot().items[0].track.duration, "3:05");
        assert_eq!(
            service.events.snapshot().playback.current.unwrap().duration,
            "3:05"
        );
    }

    #[tokio::test]
    async fn duration_format_includes_hours() {
        assert_eq!(format_duration(Duration::from_secs(3723)), "1:02:03");
        assert_eq!(format_duration(Duration::from_secs(59)), "0:59");
    }

    #[tokio::test]
    async fn pause_resume_and_toggle() {
        let mut service = service();
        service.play_playlist(tracks(1)).await.unwrap();
        service.pause().unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Paused);
        service.resume().unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        service.toggle().await.unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Paused);
    }

    #[tokio::test]
    async fn next_previous_and_stop() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.next_track().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-1");
        service.next_track().await.unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
        service.previous_track().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-0");
    }

    #[tokio::test]
    async fn repeat_all_wraps() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.set_repeat(RepeatMode::All);
        service.next_track().await.unwrap();
        service.next_track().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-0");
    }

    #[tokio::test]
    async fn repeat_one_restarts_on_finish() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.set_repeat(RepeatMode::One);
        service.on_track_finished().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-0");
        assert_eq!(service.sink.plays.len(), 2);
    }

    #[tokio::test]
    async fn finish_advances_without_repeat() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.on_track_finished().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-1");
    }

    #[tokio::test]
    async fn finished_track_is_recorded() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.seek(Duration::from_secs(45)).unwrap();
        service.on_track_finished().await.unwrap();

        let history = service.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].track_id, "video-0");
        assert_eq!(history[0].title, "Track 0");
        assert_eq!(history[0].author, "Artist");
        assert!(history[0].completed);
        assert!(history[0].started_at > 0);
        assert!((history[0].played_secs - 45.0).abs() < 1.0);
    }

    #[tokio::test]
    async fn short_play_is_not_recorded() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.seek(Duration::from_secs(10)).unwrap();
        service.on_track_finished().await.unwrap();

        assert!(service.history().is_empty());
    }

    #[tokio::test]
    async fn skipped_track_is_recorded_when_past_threshold() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.seek(Duration::from_secs(45)).unwrap();
        service.next_track().await.unwrap();

        let history = service.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].track_id, "video-0");
        assert!(!history[0].completed);
        assert!((history[0].played_secs - 45.0).abs() < 1.0);
    }

    #[tokio::test]
    async fn skipped_track_below_threshold_is_not_recorded() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.seek(Duration::from_secs(10)).unwrap();
        service.next_track().await.unwrap();

        assert!(service.history().is_empty());
    }

    #[tokio::test]
    async fn stop_records_an_abandoned_play() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.seek(Duration::from_secs(45)).unwrap();
        service.stop();

        let history = service.history();
        assert_eq!(history.len(), 1);
        assert!(!history[0].completed);
    }

    #[tokio::test]
    async fn stream_error_restart_keeps_one_play() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.sink.position = Some(Duration::from_secs(30));
        service.sink.stream_error = true;
        service.tick().await.unwrap();
        assert_eq!(service.sink.plays.len(), 2);

        service.seek(Duration::from_secs(45)).unwrap();
        service.on_track_finished().await.unwrap();

        let history = service.history();
        assert_eq!(history.len(), 1);
        assert!(history[0].completed);
        assert!((history[0].played_secs - 45.0).abs() < 1.0);
    }

    #[tokio::test]
    async fn repeat_one_records_each_completed_play() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.set_repeat(RepeatMode::One);
        service.seek(Duration::from_secs(45)).unwrap();
        service.on_track_finished().await.unwrap();
        service.seek(Duration::from_secs(45)).unwrap();
        service.on_track_finished().await.unwrap();

        let history = service.history();
        assert_eq!(history.len(), 2);
        assert!(history.iter().all(|entry| entry.completed));
        assert!(history.iter().all(|entry| entry.track_id == "video-0"));
    }

    #[tokio::test]
    async fn history_survives_a_new_service() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let mut first = service();
        first.set_history_path(&path);
        first.play_playlist(tracks(2)).await.unwrap();
        first.seek(Duration::from_secs(45)).unwrap();
        first.on_track_finished().await.unwrap();
        assert!(path.exists());

        let mut restarted = service();
        restarted.set_history_path(&path);
        let history = restarted.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].track_id, "video-0");
        assert!(history[0].completed);
    }

    #[tokio::test]
    async fn stream_error_restarts_the_current_track() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.sink.position = Some(Duration::from_secs(30));
        service.sink.stream_error = true;

        service.tick().await.unwrap();

        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-0");
        assert_eq!(service.sink.plays.len(), 2);
        assert!(service.sink.actions.contains(&"seek"));
        let position = service.current_position().as_secs_f64();
        assert!((position - 30.0).abs() < 0.1);
    }

    #[tokio::test]
    async fn stream_error_retries_the_restart_on_a_later_tick() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.sink.position = Some(Duration::from_secs(12));
        service.sink.stream_error = true;
        service.sink.play_failures = 1;

        service.tick().await.unwrap();

        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
        assert!(service.restart_pending.is_some());

        service.restart_at = Some(Instant::now() - Duration::from_secs(1));
        service.tick().await.unwrap();

        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-0");
        assert!(service.restart_pending.is_none());
        assert_eq!(service.sink.plays.len(), 2);
        assert!(service.sink.actions.contains(&"seek"));
        let position = service.current_position().as_secs_f64();
        assert!((position - 12.0).abs() < 0.1);
        assert_eq!(service.restart_delay, STREAM_RESTART_RETRY);
    }

    #[tokio::test]
    async fn restart_retry_backs_off() {
        let mut service = service();
        service.play_playlist(tracks(1)).await.unwrap();
        service.sink.stream_error = true;
        service.sink.play_failures = 2;

        service.tick().await.unwrap();
        assert_eq!(service.restart_delay, Duration::from_secs(10));

        service.restart_at = Some(Instant::now() - Duration::from_secs(1));
        service.tick().await.unwrap();
        assert_eq!(service.restart_delay, Duration::from_secs(20));
    }

    #[tokio::test]
    async fn pause_clears_a_pending_restart() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.sink.position = Some(Duration::from_secs(12));
        service.sink.stream_error = true;
        service.sink.play_failures = 1;
        service.tick().await.unwrap();
        assert!(service.restart_pending.is_some());

        service
            .handle_command(PlaybackCommand::Pause)
            .await
            .unwrap();
        assert!(service.restart_pending.is_none());

        service.restart_at = Some(Instant::now() - Duration::from_secs(1));
        service.tick().await.unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
    }

    #[tokio::test]
    async fn seek_updates_a_pending_restart() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.sink.position = Some(Duration::from_secs(12));
        service.sink.stream_error = true;
        service.sink.play_failures = 1;
        service.tick().await.unwrap();
        assert_eq!(service.restart_pending, Some(Duration::from_secs(12)));

        service
            .handle_command(PlaybackCommand::Seek(Duration::from_secs(20)))
            .await
            .unwrap();
        assert_eq!(service.restart_pending, Some(Duration::from_secs(20)));

        service.restart_at = Some(Instant::now() - Duration::from_secs(1));
        service.tick().await.unwrap();

        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        let position = service.current_position().as_secs_f64();
        assert!((position - 20.0).abs() < 0.1);
    }

    #[tokio::test]
    async fn seek_past_the_end_clamps_a_pending_restart() {
        let mut service = service();
        service.play_playlist(tracks(1)).await.unwrap();
        service.sink.position = Some(Duration::from_secs(12));
        service.sink.stream_error = true;
        service.sink.play_failures = 1;
        service.tick().await.unwrap();

        service
            .handle_command(PlaybackCommand::Seek(Duration::from_secs(999)))
            .await
            .unwrap();
        assert_eq!(service.restart_pending, Some(Duration::from_secs(180)));

        service
            .handle_command(PlaybackCommand::SeekBy(999.0))
            .await
            .unwrap();
        assert_eq!(service.restart_pending, Some(Duration::from_secs(180)));
    }

    #[tokio::test]
    async fn a_wait_for_download_keeps_a_pending_restart() {
        let mut service = service();
        service.play_playlist(tracks(1)).await.unwrap();
        service.sink.position = Some(Duration::from_secs(8));
        service.sink.stream_error = true;
        service.wait_for_downloads = true;

        service.tick().await.unwrap();

        assert!(service.pending_start);
        assert_eq!(service.restart_pending, Some(Duration::from_secs(8)));

        service.wait_for_downloads = false;
        service
            .set_download_status(
                &TrackKey::youtube("video-0"),
                MusicDownloadStatus::Downloaded,
            )
            .await;

        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        assert!(service.restart_pending.is_none());
        let position = service.current_position().as_secs_f64();
        assert!((position - 8.0).abs() < 0.1);
    }

    #[tokio::test]
    async fn shuffle_preserves_current_and_restores() {
        let mut service = service();
        service.play_playlist(tracks(6)).await.unwrap();
        service.play_index(2).await.unwrap();
        let current = service.snapshot().current.unwrap().track_id;
        let original: Vec<_> = service
            .queue_snapshot()
            .items
            .iter()
            .map(|item| item.track.track_id.clone())
            .collect();
        service.set_shuffle_with_seed(true, 42);
        assert_eq!(service.snapshot().current.unwrap().track_id, current);
        let shuffled: Vec<_> = service
            .queue_snapshot()
            .items
            .iter()
            .map(|item| item.track.track_id.clone())
            .collect();
        assert_ne!(original, shuffled);
        service.set_shuffle(false);
        let restored: Vec<_> = service
            .queue_snapshot()
            .items
            .iter()
            .map(|item| item.track.track_id.clone())
            .collect();
        assert_eq!(original, restored);
    }

    #[tokio::test]
    async fn shuffle_survives_save_and_restore() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let mut first = service();
        first.play_playlist(tracks(6)).await.unwrap();
        let original: Vec<_> = first
            .queue_snapshot()
            .items
            .iter()
            .map(|item| item.track.track_id.clone())
            .collect();
        first.set_shuffle_with_seed(true, 42);
        let shuffled: Vec<_> = first
            .queue_snapshot()
            .items
            .iter()
            .map(|item| item.track.track_id.clone())
            .collect();
        assert_ne!(original, shuffled);
        first.save_state(&path).unwrap();

        let mut restored = service();
        restored.load_state(&path).unwrap();
        let loaded: Vec<_> = restored
            .queue_snapshot()
            .items
            .iter()
            .map(|item| item.track.track_id.clone())
            .collect();
        assert_eq!(loaded, shuffled);
        restored.set_shuffle(false);
        let unshuffled: Vec<_> = restored
            .queue_snapshot()
            .items
            .iter()
            .map(|item| item.track.track_id.clone())
            .collect();
        assert_eq!(unshuffled, original);
    }

    #[tokio::test]
    async fn download_failure_skips_current() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service
            .on_download_failed(&TrackKey::youtube("video-0"))
            .await;
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-1");
    }

    fn offline_service(cache: &Path) -> PlaybackService<MockSink> {
        PlaybackService::new(MockSink::default(), cache, 50)
    }

    fn cache_track(cache: &Path, track_id: &str) {
        std::fs::create_dir_all(cache.join("downloads/youtube")).unwrap();
        std::fs::write(
            cache.join(format!("downloads/youtube/{track_id}.mp4")),
            b"audio",
        )
        .unwrap();
    }

    /// Tracks with valid 11-character YouTube ids, so the offline
    /// cached check accepts them.
    fn offline_tracks(count: usize) -> Vec<Track> {
        (0..count)
            .map(|index| Track {
                provider: common_structs::ProviderId::Youtube,
                track_id: format!("{index:011}"),
                title: format!("Track {index}"),
                author: "Artist".to_string(),
                album: "Album".to_string(),
                duration: "3:00".to_string(),
                ..Default::default()
            })
            .collect()
    }

    #[tokio::test]
    async fn offline_next_skips_uncached_tracks() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        cache_track(dir.path(), "00000000000");
        cache_track(dir.path(), "00000000002");
        service.play_playlist(offline_tracks(3)).await.unwrap();
        service.set_offline(true).await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000000");

        service.next_track().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000002");
        assert_eq!(service.sink.plays.len(), 2);

        service.next_track().await.unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
        assert_eq!(service.sink.plays.len(), 2);
    }

    #[tokio::test]
    async fn offline_previous_skips_uncached_tracks() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        cache_track(dir.path(), "00000000000");
        cache_track(dir.path(), "00000000002");
        service.play_playlist(offline_tracks(3)).await.unwrap();
        service.set_offline(true).await.unwrap();
        service.next_track().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000002");

        service.previous_track().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000000");
    }

    #[tokio::test]
    async fn offline_repeat_all_wraps_to_cached_track() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        cache_track(dir.path(), "00000000000");
        cache_track(dir.path(), "00000000002");
        service.play_playlist(offline_tracks(3)).await.unwrap();
        service.set_repeat(RepeatMode::All);
        service.set_offline(true).await.unwrap();
        service.next_track().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000002");

        service.next_track().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000000");
    }

    #[tokio::test]
    async fn offline_repeat_all_restarts_the_only_cached_track() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        cache_track(dir.path(), "00000000000");
        service.play_playlist(offline_tracks(2)).await.unwrap();
        service.set_repeat(RepeatMode::All);
        service.set_offline(true).await.unwrap();
        assert_eq!(service.sink.plays.len(), 1);

        service.next_track().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000000");
        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        assert_eq!(service.sink.plays.len(), 2);
    }

    #[tokio::test]
    async fn offline_previous_at_the_first_track_restarts_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        cache_track(dir.path(), "00000000000");
        service.play_playlist(offline_tracks(2)).await.unwrap();
        service.set_offline(true).await.unwrap();
        assert_eq!(service.sink.plays.len(), 1);

        service.previous_track().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000000");
        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        assert_eq!(service.sink.plays.len(), 2);
    }

    #[tokio::test]
    async fn offline_flip_on_skips_uncached_current() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        cache_track(dir.path(), "00000000001");
        service.play_playlist(offline_tracks(2)).await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000000");

        service.set_offline(true).await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000001");
        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        assert_eq!(service.sink.plays.len(), 2);
    }

    #[tokio::test]
    async fn offline_flip_on_with_no_cached_track_stops() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        service.play_playlist(offline_tracks(2)).await.unwrap();
        service.set_offline(true).await.unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
        assert_eq!(service.sink.plays.len(), 1);
        assert!(service.sink.actions.contains(&"stop"));
    }

    #[tokio::test]
    async fn offline_flip_on_keeps_cached_current_playing() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        cache_track(dir.path(), "00000000000");
        service.play_playlist(offline_tracks(2)).await.unwrap();
        service.set_offline(true).await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000000");
        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        assert_eq!(service.sink.plays.len(), 1);
    }

    #[tokio::test]
    async fn offline_repeat_one_does_not_loop_on_uncached_track() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        cache_track(dir.path(), "00000000000");
        service.play_playlist(offline_tracks(2)).await.unwrap();
        service.set_repeat(RepeatMode::One);
        service.set_offline(true).await.unwrap();
        assert_eq!(service.sink.plays.len(), 1);

        std::fs::remove_file(dir.path().join("downloads/youtube/00000000000.mp4")).unwrap();
        service.on_track_finished().await.unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
        assert_eq!(service.sink.plays.len(), 1);

        service.on_track_finished().await.unwrap();
        assert_eq!(service.sink.plays.len(), 1);
    }

    #[tokio::test]
    async fn offline_repeat_one_restarts_cached_track() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        cache_track(dir.path(), "00000000000");
        service.play_playlist(offline_tracks(2)).await.unwrap();
        service.set_repeat(RepeatMode::One);
        service.set_offline(true).await.unwrap();
        service.on_track_finished().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000000");
        assert_eq!(service.sink.plays.len(), 2);
    }

    #[tokio::test]
    async fn offline_play_index_rejects_uncached_track() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        cache_track(dir.path(), "00000000000");
        service.play_playlist(offline_tracks(2)).await.unwrap();
        service.set_offline(true).await.unwrap();
        assert!(matches!(
            service.play_index(1).await,
            Err(PlaybackError::NotCached)
        ));
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000000");
        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
    }

    #[tokio::test]
    async fn offline_play_track_rejects_uncached_track() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        service.set_offline(true).await.unwrap();
        assert!(matches!(
            service.play_track(offline_tracks(1).remove(0)).await,
            Err(PlaybackError::NotCached)
        ));
        assert!(service.queue_snapshot().items.is_empty());
    }

    #[tokio::test]
    async fn offline_play_playlist_starts_at_first_cached_track() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        cache_track(dir.path(), "00000000001");
        service.set_offline(true).await.unwrap();
        service.play_playlist(offline_tracks(3)).await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000001");
        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
    }

    #[tokio::test]
    async fn offline_all_uncached_queue_stops() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        service.set_offline(true).await.unwrap();
        service.play_playlist(offline_tracks(2)).await.unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
        assert!(service.sink.plays.is_empty());
    }

    #[tokio::test]
    async fn offline_off_restores_normal_advance() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = offline_service(dir.path());
        cache_track(dir.path(), "00000000000");
        service.play_playlist(offline_tracks(2)).await.unwrap();
        service.set_offline(true).await.unwrap();
        service.next_track().await.unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);

        service.set_offline(false).await.unwrap();
        assert!(!service.is_offline());
        service.next_track().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "00000000001");
        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
    }

    #[tokio::test]
    async fn set_offline_command_flips_the_switch() {
        let mut service = service();
        let (_, mut events) = service.events().subscribe();
        service
            .handle_command(PlaybackCommand::SetOffline(true))
            .await
            .unwrap();
        assert!(service.is_offline());
        assert!(matches!(
            events.try_recv().unwrap().event,
            CoreEvent::Offline { enabled: true }
        ));
        service
            .handle_command(PlaybackCommand::SetOffline(false))
            .await
            .unwrap();
        assert!(!service.is_offline());
    }

    #[tokio::test]
    async fn remove_current_starts_next_or_stops() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.remove(0).await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-1");
        service.remove(0).await.unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
    }

    #[tokio::test]
    async fn volume_is_clamped_and_applied() {
        let mut service = service();
        service.set_volume(150).unwrap();
        assert_eq!(service.snapshot().volume, 100);
        assert_eq!(service.sink.volume, 100);
    }

    #[tokio::test]
    async fn new_uses_initial_volume() {
        let service = PlaybackService::new(MockSink::default(), "/tmp/recvrd-test", 75);
        assert_eq!(service.snapshot().volume, 75);
    }

    #[tokio::test]
    async fn new_clamps_initial_volume() {
        let service = PlaybackService::new(MockSink::default(), "/tmp/recvrd-test", 150);
        assert_eq!(service.snapshot().volume, 100);
    }

    #[tokio::test]
    async fn download_status_is_published() {
        let mut service = service();
        service.play_playlist(tracks(1)).await.unwrap();
        let (_, mut events) = service.events().subscribe();
        service
            .set_download_status(
                &TrackKey::youtube("video-0"),
                MusicDownloadStatus::Downloaded,
            )
            .await;
        let event = events.try_recv().unwrap();
        assert!(matches!(event.event, CoreEvent::DownloadStatus { .. }));
        assert_eq!(
            service.queue_snapshot().items[0].download,
            MusicDownloadStatus::Downloaded
        );
    }

    #[tokio::test]
    async fn empty_queue_is_safe() {
        let mut service = service();
        service.next_track().await.unwrap();
        service.previous_track().await.unwrap();
        service.on_track_finished().await.unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
        assert!(service.snapshot().current.is_none());
    }

    #[tokio::test]
    async fn tick_advances_when_sink_is_finished() {
        let mut service = service();
        service.play_playlist(tracks(2)).await.unwrap();
        service.sink.finished = true;
        service.tick().await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-1");
    }

    #[tokio::test]
    async fn dispatch_command_controls_service() {
        let mut service = service();
        service
            .handle_command(PlaybackCommand::PlayPlaylist(tracks(2)))
            .await
            .unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-0");
        service
            .handle_command(PlaybackCommand::Pause)
            .await
            .unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Paused);
        service
            .handle_command(PlaybackCommand::SetVolume(70))
            .await
            .unwrap();
        assert_eq!(service.snapshot().volume, 70);
        service.handle_command(PlaybackCommand::Next).await.unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-1");
    }

    #[tokio::test]
    async fn seek_by_rejects_non_finite_deltas() {
        let mut service = service();
        service.play_playlist(tracks(1)).await.unwrap();
        assert!(matches!(
            service
                .handle_command(PlaybackCommand::SeekBy(f64::INFINITY))
                .await,
            Err(PlaybackError::InvalidPosition)
        ));
        assert!(matches!(
            service
                .handle_command(PlaybackCommand::SeekBy(f64::NAN))
                .await,
            Err(PlaybackError::InvalidPosition)
        ));
        assert!(matches!(
            service.handle_command(PlaybackCommand::SeekBy(1e300)).await,
            Err(PlaybackError::InvalidPosition)
        ));
        service
            .handle_command(PlaybackCommand::SeekBy(5.0))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn restore_clamps_overflowing_position_to_zero() {
        let mut service = service();
        service.play_playlist(tracks(1)).await.unwrap();
        let mut state = service.persisted_state();
        state.position_secs = 1e300;
        service.restore_state(&state).unwrap();
        assert_eq!(service.snapshot().position_secs, 0.0);
    }

    #[tokio::test]
    async fn persists_and_restores_queue_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut source = service();
        source.play_playlist(tracks(3)).await.unwrap();
        source.play_index(1).await.unwrap();
        source.set_volume(80).unwrap();
        source.set_repeat(RepeatMode::All);
        source.set_shuffle_with_seed(true, 42);
        source.seek(Duration::from_secs(12)).unwrap();

        let expected = source.queue_snapshot();
        source.save_state(&path).unwrap();

        let mut restored = service();
        restored.load_state(&path).unwrap();
        let queue = restored.queue_snapshot();
        assert_eq!(queue.items, expected.items);
        assert_eq!(queue.current, expected.current);
        assert_eq!(queue.repeat, RepeatMode::All);
        assert!(queue.shuffle);
        assert_eq!(restored.snapshot().volume, 80);
        assert_eq!(restored.snapshot().repeat, RepeatMode::All);
        assert!(restored.snapshot().shuffle);
        let restored_position = restored.snapshot().position_secs;
        assert!(
            (restored_position - 12.0).abs() < 0.5,
            "position was {restored_position}"
        );
        assert_eq!(restored.snapshot().status, PlaybackStatus::Stopped);
    }

    #[tokio::test]
    async fn restore_resumes_at_the_saved_position_when_play_is_pressed() {
        let mut service = service();
        let state = PersistedState {
            version: STATE_VERSION,
            tracks: tracks(1),
            original_tracks: Vec::new(),
            current: Some(0),
            repeat: RepeatMode::Off,
            shuffle: false,
            volume: 50,
            position_secs: 12.0,
        };
        service.restore_state(&state).unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);

        // A tick must not start playback on its own. Resume is manual.
        service.tick().await.unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
        assert!(service.sink.plays.is_empty());

        service.play().await.unwrap();

        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        assert_eq!(service.sink.plays.len(), 1);
        assert!(service.sink.actions.contains(&"seek"));
        assert!(service.resume_pending.is_none());
        let position = service.current_position().as_secs_f64();
        assert!((position - 12.0).abs() < 0.5, "position was {position}");
    }

    #[tokio::test]
    async fn a_seek_before_the_first_start_moves_the_resume_position() {
        let mut service = service();
        let state = PersistedState {
            version: STATE_VERSION,
            tracks: tracks(1),
            original_tracks: Vec::new(),
            current: Some(0),
            repeat: RepeatMode::Off,
            shuffle: false,
            volume: 50,
            position_secs: 12.0,
        };
        service.restore_state(&state).unwrap();

        service
            .handle_command(PlaybackCommand::Seek(Duration::from_secs(30)))
            .await
            .unwrap();
        service.play().await.unwrap();

        assert!(service.sink.actions.contains(&"seek"));
        let position = service.current_position().as_secs_f64();
        assert!((position - 30.0).abs() < 0.5, "position was {position}");
    }

    #[tokio::test]
    async fn pause_while_stopped_keeps_the_resume_position() {
        let mut service = service();
        let state = PersistedState {
            version: STATE_VERSION,
            tracks: tracks(1),
            original_tracks: Vec::new(),
            current: Some(0),
            repeat: RepeatMode::Off,
            shuffle: false,
            volume: 50,
            position_secs: 12.0,
        };
        service.restore_state(&state).unwrap();

        // MPRIS Pause reaches handle_command, unlike the REST pause
        // route, which calls pause directly.
        service
            .handle_command(PlaybackCommand::Pause)
            .await
            .unwrap();
        assert_eq!(service.resume_pending, Some(Duration::from_secs(12)));

        service.play().await.unwrap();

        assert!(service.sink.actions.contains(&"seek"));
        let position = service.current_position().as_secs_f64();
        assert!((position - 12.0).abs() < 0.5, "position was {position}");
    }

    #[tokio::test]
    async fn a_seek_by_before_the_first_start_moves_the_resume_position() {
        // The track lasts 3:00. A delta of 18 seconds moves the
        // resume position from 12 to 30 seconds. A large delta
        // clamps it to the track duration, and a large negative
        // delta clamps it to zero.
        for (delta, expected, seeks) in [
            (18.0, 30.0, true),
            (999.0, 180.0, true),
            (-30.0, 0.0, false),
        ] {
            let mut service = service();
            let state = PersistedState {
                version: STATE_VERSION,
                tracks: tracks(1),
                original_tracks: Vec::new(),
                current: Some(0),
                repeat: RepeatMode::Off,
                shuffle: false,
                volume: 50,
                position_secs: 12.0,
            };
            service.restore_state(&state).unwrap();

            service
                .handle_command(PlaybackCommand::SeekBy(delta))
                .await
                .unwrap();
            assert_eq!(
                service.resume_pending,
                Some(Duration::from_secs_f64(expected))
            );

            service.play().await.unwrap();

            assert_eq!(service.sink.actions.contains(&"seek"), seeks);
            let position = service.current_position().as_secs_f64();
            assert!((position - expected).abs() < 0.5, "position was {position}");
        }
    }

    #[tokio::test]
    async fn a_pending_resume_position_waits_for_a_download() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = PlaybackService::new(MockSink::default(), dir.path(), 50);
        service.set_wait_for_downloads(true);
        let state = PersistedState {
            version: STATE_VERSION,
            tracks: tracks(1),
            original_tracks: Vec::new(),
            current: Some(0),
            repeat: RepeatMode::Off,
            shuffle: false,
            volume: 50,
            position_secs: 12.0,
        };
        service.restore_state(&state).unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);

        service.play().await.unwrap();

        // The track is not cached yet, so playback waits for the
        // download instead of starting.
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
        assert!(service.pending_start);
        assert!(service.sink.plays.is_empty());
        assert_eq!(service.resume_pending, Some(Duration::from_secs(12)));

        std::fs::create_dir_all(dir.path().join("downloads/youtube")).unwrap();
        std::fs::write(dir.path().join("downloads/youtube/video-0.mp4"), b"audio").unwrap();
        service
            .set_download_status(
                &TrackKey::youtube("video-0"),
                MusicDownloadStatus::Downloaded,
            )
            .await;

        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        assert_eq!(service.sink.plays.len(), 1);
        assert!(service.sink.actions.contains(&"seek"));
        assert!(service.resume_pending.is_none());
        let position = service.current_position().as_secs_f64();
        assert!((position - 12.0).abs() < 0.5, "position was {position}");
    }

    #[tokio::test]
    async fn restore_near_the_end_of_the_track_starts_from_zero() {
        // The track lasts 3:00. A position of 179 seconds is within
        // the end margin, and 999 seconds is past the end.
        for position_secs in [179.0, 999.0] {
            let mut service = service();
            let state = PersistedState {
                version: STATE_VERSION,
                tracks: tracks(1),
                original_tracks: Vec::new(),
                current: Some(0),
                repeat: RepeatMode::Off,
                shuffle: false,
                volume: 50,
                position_secs,
            };
            service.restore_state(&state).unwrap();
            assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);

            service.toggle().await.unwrap();

            assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
            assert!(!service.sink.actions.contains(&"seek"));
            let position = service.current_position().as_secs_f64();
            assert!(position < 0.5, "position was {position}");
        }
    }

    #[tokio::test]
    async fn restore_with_a_failed_seek_starts_from_zero() {
        let mut service = service();
        service.sink.seek_fails = true;
        let state = PersistedState {
            version: STATE_VERSION,
            tracks: tracks(1),
            original_tracks: Vec::new(),
            current: Some(0),
            repeat: RepeatMode::Off,
            shuffle: false,
            volume: 50,
            position_secs: 12.0,
        };
        service.restore_state(&state).unwrap();

        service.toggle().await.unwrap();

        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        assert!(service.sink.actions.contains(&"seek"));
        let position = service.current_position().as_secs_f64();
        assert!(position < 0.5, "position was {position}");
    }

    #[tokio::test]
    async fn playback_position_advances_and_freezes_on_pause() {
        let mut service = service();
        service.play_playlist(tracks(1)).await.unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let advanced = service.current_position();
        assert!(advanced >= Duration::from_millis(20), "{advanced:?}");

        service.pause().unwrap();
        let frozen = service.snapshot().position_secs;
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(service.current_position().as_secs_f64(), frozen);

        service.resume().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(service.current_position() > Duration::from_secs_f64(frozen));
    }

    #[tokio::test]
    async fn wait_for_downloads_starts_when_file_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = PlaybackService::new(MockSink::default(), dir.path(), 50);
        service.set_wait_for_downloads(true);
        service.play_playlist(tracks(1)).await.unwrap();

        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-0");
        assert!(service.sink.plays.is_empty());

        std::fs::create_dir_all(dir.path().join("downloads/youtube")).unwrap();
        std::fs::write(dir.path().join("downloads/youtube/video-0.mp4"), b"audio").unwrap();
        service
            .set_download_status(
                &TrackKey::youtube("video-0"),
                MusicDownloadStatus::Downloaded,
            )
            .await;

        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        assert_eq!(service.sink.plays.len(), 1);
        assert_eq!(service.sink.plays[0].0, "video-0");
    }

    #[tokio::test]
    async fn restore_applies_volume_to_the_sink() {
        let mut service = service();
        let state = PersistedState {
            version: STATE_VERSION,
            tracks: tracks(1),
            original_tracks: Vec::new(),
            current: Some(0),
            repeat: RepeatMode::All,
            shuffle: false,
            volume: 80,
            position_secs: 7.5,
        };
        service.restore_state(&state).unwrap();

        assert_eq!(service.snapshot().volume, 80);
        assert_eq!(service.sink.volume, 80);
        assert_eq!(service.snapshot().position_secs, 7.5);
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
    }

    struct MockStreams;

    #[async_trait]
    impl StreamFactory for MockStreams {
        async fn open(&self, _source: &TrackSource) -> Result<StreamReader, String> {
            Ok(StreamReader {
                reader: Box::new(std::io::Cursor::new(vec![0u8; 16])),
                mime: Some("audio/mpeg".to_string()),
                length: Some(16),
                seekable: true,
            })
        }
    }

    struct FailingStreams;

    #[async_trait]
    impl StreamFactory for FailingStreams {
        async fn open(&self, _source: &TrackSource) -> Result<StreamReader, String> {
            Err("stream failed".to_string())
        }
    }

    fn stream_track() -> Track {
        Track {
            provider: common_structs::ProviderId::Podverse,
            track_id: "YvIRJrjZVe".to_string(),
            title: "Stream".to_string(),
            author: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
            source: Some(Box::new(TrackSource {
                url: "https://example.com/stream.mp3".to_string(),
                mime: Some("audio/mpeg".to_string()),
                length: Some(16),
                seekable: true,
                kind: common_structs::MediaKind::Audio,
            })),
            metadata: None,
        }
    }

    fn mode_settings(mode: PlaybackMode) -> Arc<PlaybackSettingsStore> {
        let dir = tempfile::Builder::new()
            .prefix("recvrd-settings-")
            .tempdir()
            .unwrap()
            .keep();
        let store = PlaybackSettingsStore::load(dir.join("settings.toml"));
        store
            .set_mode(common_structs::ProviderId::Podverse, mode)
            .unwrap();
        Arc::new(store)
    }

    #[tokio::test]
    async fn stream_mode_uses_the_stream_reader() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = PlaybackService::with_streams(
            MockSink::default(),
            dir.path(),
            50,
            mode_settings(PlaybackMode::Stream),
            Arc::new(MockStreams),
        );
        service.play_playlist(vec![stream_track()]).await.unwrap();

        assert_eq!(service.sink.sources, vec![PlaybackSourceKind::Stream]);
        assert_eq!(
            service.snapshot().source_kind,
            Some(PlaybackSourceKind::Stream)
        );
    }

    #[tokio::test]
    async fn an_existing_cache_file_wins_over_stream_mode() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("downloads/podverse")).unwrap();
        std::fs::write(
            dir.path().join("downloads/podverse/YvIRJrjZVe.mp3"),
            b"audio",
        )
        .unwrap();
        let mut service = PlaybackService::with_streams(
            MockSink::default(),
            dir.path(),
            50,
            mode_settings(PlaybackMode::Stream),
            Arc::new(MockStreams),
        );
        service.play_playlist(vec![stream_track()]).await.unwrap();

        assert_eq!(service.sink.sources, vec![PlaybackSourceKind::File]);
        assert_eq!(
            service.snapshot().source_kind,
            Some(PlaybackSourceKind::File)
        );
    }

    #[tokio::test]
    async fn stream_then_download_falls_back_to_download() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = PlaybackService::with_streams(
            MockSink::default(),
            dir.path(),
            50,
            mode_settings(PlaybackMode::StreamThenDownload),
            Arc::new(FailingStreams),
        );
        service.set_wait_for_downloads(true);
        service.play_playlist(vec![stream_track()]).await.unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);

        std::fs::create_dir_all(dir.path().join("downloads/podverse")).unwrap();
        std::fs::write(
            dir.path().join("downloads/podverse/YvIRJrjZVe.mp3"),
            b"audio",
        )
        .unwrap();
        service
            .set_download_status(
                &TrackKey::podverse("YvIRJrjZVe"),
                MusicDownloadStatus::Downloaded,
            )
            .await;

        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        assert_eq!(service.sink.sources, vec![PlaybackSourceKind::File]);
    }

    #[tokio::test]
    async fn stream_mode_does_not_fall_back_to_download() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = PlaybackService::with_streams(
            MockSink::default(),
            dir.path(),
            50,
            mode_settings(PlaybackMode::Stream),
            Arc::new(FailingStreams),
        );
        service.set_wait_for_downloads(true);
        assert!(service.play_playlist(vec![stream_track()]).await.is_err());
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
    }

    #[tokio::test]
    async fn stream_mode_with_no_source_fails_fast() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = PlaybackService::with_streams(
            MockSink::default(),
            dir.path(),
            50,
            mode_settings(PlaybackMode::Stream),
            Arc::new(MockStreams),
        );
        service.set_wait_for_downloads(true);
        let mut track = stream_track();
        track.source = None;
        assert!(service.play_playlist(vec![track]).await.is_err());
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
        assert!(service.sink.plays.is_empty());
    }
}
