use crate::{
    events::{CoreEvent, EventBus, Snapshot},
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
    settings: Arc<PlaybackSettingsStore>,
    streams: Arc<dyn StreamFactory>,
    source_kind: Option<PlaybackSourceKind>,
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
            settings,
            streams,
            source_kind: None,
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

    /// Current position, including the time elapsed since the last play,
    /// resume or seek.
    pub fn current_position(&self) -> Duration {
        match self.started_at {
            Some(started_at) => self.position + started_at.elapsed(),
            None => self.position,
        }
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
        if self.status == PlaybackStatus::Playing {
            if self.sink.is_finished() {
                self.on_track_finished().await?;
            } else {
                self.publish_playback();
            }
        }
        Ok(())
    }

    /// Apply a command from an API/MPRIS/MCP client.
    pub async fn handle_command(&mut self, command: PlaybackCommand) -> Result<(), PlaybackError> {
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
            PlaybackCommand::Resume => self.resume(),
            PlaybackCommand::Toggle => self.toggle().await,
            PlaybackCommand::Next => self.next_track().await,
            PlaybackCommand::Previous => self.previous_track().await,
            PlaybackCommand::Seek(position) => self.seek(position),
            PlaybackCommand::SeekBy(delta) => {
                if !delta.is_finite() {
                    return Err(PlaybackError::InvalidPosition);
                }
                let position = (self.current_position().as_secs_f64() + delta).max(0.0);
                let position = Duration::try_from_secs_f64(position)
                    .map_err(|_| PlaybackError::InvalidPosition)?;
                self.seek(position)
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
        self.started_at = None;
        self.pending_start = false;
        self.status = PlaybackStatus::Stopped;
        self.source_kind = None;
        self.sink
            .set_volume(self.volume)
            .map_err(|error| PlaybackError::Sink(error.to_string()))?;
        self.publish_all();
        Ok(())
    }

    pub async fn play_playlist(&mut self, tracks: Vec<Track>) -> Result<(), PlaybackError> {
        self.queue.replace(tracks);
        self.start_current().await
    }

    pub async fn play_track(&mut self, track: Track) -> Result<(), PlaybackError> {
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
        let _ = self.sink.stop();
        self.queue.clear();
        self.status = PlaybackStatus::Stopped;
        self.position = Duration::ZERO;
        self.source_kind = None;
        self.publish_all();
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stop_playback();
    }

    pub async fn play_index(&mut self, index: usize) -> Result<(), PlaybackError> {
        if !self.queue.play_index(index) {
            return Err(PlaybackError::IndexOutOfBounds);
        }
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

    pub async fn toggle(&mut self) -> Result<(), PlaybackError> {
        match self.status {
            PlaybackStatus::Playing => self.pause(),
            PlaybackStatus::Paused => self.resume(),
            PlaybackStatus::Stopped => self.start_current().await,
        }
    }

    pub async fn next_track(&mut self) -> Result<(), PlaybackError> {
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
        let Some(track) = self.queue.current_track().cloned() else {
            self.stop_playback();
            return Ok(());
        };

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
        let _ = self.sink.stop();
        self.status = PlaybackStatus::Stopped;
        self.position = Duration::ZERO;
        self.started_at = None;
        self.pending_start = false;
        self.source_kind = None;
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
        duration: Option<Duration>,
    }

    impl AudioSink for MockSink {
        type Error = std::io::Error;

        fn play(&mut self, track: &Track, source: PlaybackSource) -> Result<(), Self::Error> {
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
