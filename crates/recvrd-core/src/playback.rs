use crate::{
    events::{CoreEvent, EventBus, Snapshot},
    model::{PlaybackSnapshot, PlaybackStatus, Queue, RepeatMode, Track},
    state::{PersistedState, STATE_VERSION, StateError},
};
use common_structs::TrackKey;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use thiserror::Error;

/// Abstraction over the audio backend so playback can be tested without a
/// device, and swapped for a real implementation in the daemon.
pub trait AudioSink {
    type Error: std::error::Error + Send + Sync + 'static;

    fn play(&mut self, track: &Track, path: &Path) -> Result<(), Self::Error>;
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
}

impl<S: AudioSink> PlaybackService<S> {
    pub fn new(sink: S, cache_dir: impl Into<PathBuf>, initial_volume: u8) -> Self {
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
    pub fn tick(&mut self) -> Result<(), PlaybackError> {
        if self.status == PlaybackStatus::Playing {
            if self.sink.is_finished() {
                self.on_track_finished()?;
            } else {
                self.publish_playback();
            }
        }
        Ok(())
    }

    /// Apply a command from an API/MPRIS/MCP client.
    pub fn handle_command(&mut self, command: PlaybackCommand) -> Result<(), PlaybackError> {
        match command {
            PlaybackCommand::PlayPlaylist(tracks) => self.play_playlist(tracks),
            PlaybackCommand::PlayTrack(track) => self.play_track(track),
            PlaybackCommand::Enqueue(track) => {
                self.enqueue(track);
                Ok(())
            }
            PlaybackCommand::Remove(index) => self.remove(index),
            PlaybackCommand::MoveItem { from, to } => {
                self.move_item(from, to);
                Ok(())
            }
            PlaybackCommand::Clear => self.clear(),
            PlaybackCommand::Stop => {
                self.stop();
                Ok(())
            }
            PlaybackCommand::PlayIndex(index) => self.play_index(index),
            PlaybackCommand::Pause => self.pause(),
            PlaybackCommand::Resume => self.resume(),
            PlaybackCommand::Toggle => self.toggle(),
            PlaybackCommand::Next => self.next_track(),
            PlaybackCommand::Previous => self.previous_track(),
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
        self.sink
            .set_volume(self.volume)
            .map_err(|error| PlaybackError::Sink(error.to_string()))?;
        self.publish_all();
        Ok(())
    }

    pub fn play_playlist(&mut self, tracks: Vec<Track>) -> Result<(), PlaybackError> {
        self.queue.replace(tracks);
        self.start_current()
    }

    pub fn play_track(&mut self, track: Track) -> Result<(), PlaybackError> {
        self.queue.play_now(track);
        self.start_current()
    }

    pub fn enqueue(&mut self, track: Track) {
        self.queue.enqueue(track);
        self.publish_queue();
    }

    pub fn remove(&mut self, index: usize) -> Result<(), PlaybackError> {
        let was_current = self.queue.current_index() == Some(index);
        self.queue.remove(index);
        if was_current {
            self.start_current()
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
        self.publish_all();
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stop_playback();
    }

    pub fn play_index(&mut self, index: usize) -> Result<(), PlaybackError> {
        if !self.queue.play_index(index) {
            return Err(PlaybackError::IndexOutOfBounds);
        }
        self.start_current()
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

    pub fn toggle(&mut self) -> Result<(), PlaybackError> {
        match self.status {
            PlaybackStatus::Playing => self.pause(),
            PlaybackStatus::Paused => self.resume(),
            PlaybackStatus::Stopped => self.start_current(),
        }
    }

    pub fn next_track(&mut self) -> Result<(), PlaybackError> {
        match self.queue.next_index() {
            Some(index) => {
                self.queue.set_current(Some(index));
                self.start_current()
            }
            None => {
                self.stop_playback();
                Ok(())
            }
        }
    }

    pub fn previous_track(&mut self) -> Result<(), PlaybackError> {
        match self.queue.previous_index() {
            Some(index) => {
                self.queue.set_current(Some(index));
                self.start_current()
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

    pub fn on_track_finished(&mut self) -> Result<(), PlaybackError> {
        if let Some(track) = self.queue.current_track() {
            self.events.publish(CoreEvent::TrackEnded {
                track_id: track.key(),
            });
        }
        if self.queue.repeat() == RepeatMode::One {
            self.start_current()
        } else {
            self.next_track()
        }
    }

    pub fn on_download_failed(&mut self, key: &TrackKey) {
        self.events.publish(CoreEvent::Error {
            message: format!("download failed for {key}"),
        });
        if self
            .queue
            .current_track()
            .is_some_and(|track| track.key() == *key)
        {
            self.pending_start = false;
            let _ = self.next_track();
        }
    }

    /// Mark every queued track as not downloaded, except the kept
    /// track, and publish the new queue. Call this after the cached
    /// files are gone.
    pub fn reset_download_statuses(&mut self, keep: Option<&TrackKey>) {
        self.queue.reset_download_statuses(keep);
        self.publish_queue();
    }

    pub fn set_download_status(
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
            let _ = self.start_current();
            return;
        }
        self.publish_queue();
    }

    fn start_current(&mut self) -> Result<(), PlaybackError> {
        let Some(track) = self.queue.current_track().cloned() else {
            self.stop_playback();
            return Ok(());
        };

        let path = track
            .key()
            .existing_audio_file(&self.cache_dir.join("downloads"))
            // No downloaded file yet. Fall back to the YouTube path,
            // so the sink reports the same error as before.
            .unwrap_or_else(|| {
                track
                    .key()
                    .download_file(&self.cache_dir.join("downloads"), "mp4")
            });
        if self.wait_for_downloads && !path.exists() {
            self.pending_start = true;
            self.status = PlaybackStatus::Stopped;
            self.position = Duration::ZERO;
            self.started_at = None;
            self.publish_all();
            return Ok(());
        }

        self.pending_start = false;
        if let Err(error) = self.sink.play(&track, &path) {
            self.status = PlaybackStatus::Stopped;
            self.position = Duration::ZERO;
            self.started_at = None;
            let message = error.to_string();
            self.events.publish(CoreEvent::Error {
                message: message.clone(),
            });
            self.publish_all();
            return Err(PlaybackError::Sink(message));
        }

        self.status = PlaybackStatus::Playing;
        self.position = Duration::ZERO;
        self.started_at = Some(Instant::now());
        self.sink
            .set_volume(self.volume)
            .map_err(|error| PlaybackError::Sink(error.to_string()))?;
        if let Some(duration) = self.sink.duration() {
            self.queue.set_current_duration(format_duration(duration));
        }
        self.publish_all();
        Ok(())
    }

    fn stop_playback(&mut self) {
        let _ = self.sink.stop();
        self.status = PlaybackStatus::Stopped;
        self.position = Duration::ZERO;
        self.started_at = None;
        self.pending_start = false;
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
    use common_structs::MusicDownloadStatus;
    use std::path::PathBuf;

    #[derive(Default)]
    struct MockSink {
        plays: Vec<(String, PathBuf)>,
        actions: Vec<&'static str>,
        volume: u8,
        finished: bool,
        duration: Option<Duration>,
    }

    impl AudioSink for MockSink {
        type Error = std::io::Error;

        fn play(&mut self, track: &Track, path: &Path) -> Result<(), Self::Error> {
            self.plays
                .push((track.track_id.clone(), path.to_path_buf()));
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
            })
            .collect()
    }

    fn service() -> PlaybackService<MockSink> {
        PlaybackService::new(MockSink::default(), "/tmp/recvrd-test", 50)
    }

    #[test]
    fn play_playlist_starts_first_track() {
        let mut service = service();
        service.play_playlist(tracks(2)).unwrap();
        let snapshot = service.snapshot();
        assert_eq!(snapshot.status, PlaybackStatus::Playing);
        assert_eq!(snapshot.current.unwrap().track_id, "video-0");
        assert_eq!(service.queue_snapshot().current, Some(0));
        assert_eq!(
            service.events.snapshot().playback.status,
            PlaybackStatus::Playing
        );
    }

    #[test]
    fn fills_missing_duration_from_the_sink() {
        let mut service = service();
        service.sink.duration = Some(Duration::from_secs(185));
        let mut playlist = tracks(1);
        playlist[0].duration.clear();
        service.play_playlist(playlist).unwrap();
        assert_eq!(service.snapshot().current.unwrap().duration, "3:05");
        assert_eq!(service.queue_snapshot().items[0].track.duration, "3:05");
        assert_eq!(
            service.events.snapshot().playback.current.unwrap().duration,
            "3:05"
        );
    }

    #[test]
    fn duration_format_includes_hours() {
        assert_eq!(format_duration(Duration::from_secs(3723)), "1:02:03");
        assert_eq!(format_duration(Duration::from_secs(59)), "0:59");
    }

    #[test]
    fn pause_resume_and_toggle() {
        let mut service = service();
        service.play_playlist(tracks(1)).unwrap();
        service.pause().unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Paused);
        service.resume().unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        service.toggle().unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Paused);
    }

    #[test]
    fn next_previous_and_stop() {
        let mut service = service();
        service.play_playlist(tracks(2)).unwrap();
        service.next_track().unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-1");
        service.next_track().unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
        service.previous_track().unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-0");
    }

    #[test]
    fn repeat_all_wraps() {
        let mut service = service();
        service.play_playlist(tracks(2)).unwrap();
        service.set_repeat(RepeatMode::All);
        service.next_track().unwrap();
        service.next_track().unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-0");
    }

    #[test]
    fn repeat_one_restarts_on_finish() {
        let mut service = service();
        service.play_playlist(tracks(2)).unwrap();
        service.set_repeat(RepeatMode::One);
        service.on_track_finished().unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-0");
        assert_eq!(service.sink.plays.len(), 2);
    }

    #[test]
    fn finish_advances_without_repeat() {
        let mut service = service();
        service.play_playlist(tracks(2)).unwrap();
        service.on_track_finished().unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-1");
    }

    #[test]
    fn shuffle_preserves_current_and_restores() {
        let mut service = service();
        service.play_playlist(tracks(6)).unwrap();
        service.play_index(2).unwrap();
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

    #[test]
    fn shuffle_survives_save_and_restore() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let mut first = service();
        first.play_playlist(tracks(6)).unwrap();
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

    #[test]
    fn download_failure_skips_current() {
        let mut service = service();
        service.play_playlist(tracks(2)).unwrap();
        service.on_download_failed(&TrackKey::youtube("video-0"));
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-1");
    }

    #[test]
    fn remove_current_starts_next_or_stops() {
        let mut service = service();
        service.play_playlist(tracks(2)).unwrap();
        service.remove(0).unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-1");
        service.remove(0).unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
    }

    #[test]
    fn volume_is_clamped_and_applied() {
        let mut service = service();
        service.set_volume(150).unwrap();
        assert_eq!(service.snapshot().volume, 100);
        assert_eq!(service.sink.volume, 100);
    }

    #[test]
    fn new_uses_initial_volume() {
        let service = PlaybackService::new(MockSink::default(), "/tmp/recvrd-test", 75);
        assert_eq!(service.snapshot().volume, 75);
    }

    #[test]
    fn new_clamps_initial_volume() {
        let service = PlaybackService::new(MockSink::default(), "/tmp/recvrd-test", 150);
        assert_eq!(service.snapshot().volume, 100);
    }

    #[test]
    fn download_status_is_published() {
        let mut service = service();
        service.play_playlist(tracks(1)).unwrap();
        let (_, mut events) = service.events().subscribe();
        service.set_download_status(
            &TrackKey::youtube("video-0"),
            MusicDownloadStatus::Downloaded,
        );
        let event = events.try_recv().unwrap();
        assert!(matches!(event.event, CoreEvent::DownloadStatus { .. }));
        assert_eq!(
            service.queue_snapshot().items[0].download,
            MusicDownloadStatus::Downloaded
        );
    }

    #[test]
    fn empty_queue_is_safe() {
        let mut service = service();
        service.next_track().unwrap();
        service.previous_track().unwrap();
        service.on_track_finished().unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
        assert!(service.snapshot().current.is_none());
    }

    #[test]
    fn tick_advances_when_sink_is_finished() {
        let mut service = service();
        service.play_playlist(tracks(2)).unwrap();
        service.sink.finished = true;
        service.tick().unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-1");
    }

    #[test]
    fn dispatch_command_controls_service() {
        let mut service = service();
        service
            .handle_command(PlaybackCommand::PlayPlaylist(tracks(2)))
            .unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-0");
        service.handle_command(PlaybackCommand::Pause).unwrap();
        assert_eq!(service.snapshot().status, PlaybackStatus::Paused);
        service
            .handle_command(PlaybackCommand::SetVolume(70))
            .unwrap();
        assert_eq!(service.snapshot().volume, 70);
        service.handle_command(PlaybackCommand::Next).unwrap();
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-1");
    }

    #[test]
    fn seek_by_rejects_non_finite_deltas() {
        let mut service = service();
        service.play_playlist(tracks(1)).unwrap();
        assert!(matches!(
            service.handle_command(PlaybackCommand::SeekBy(f64::INFINITY)),
            Err(PlaybackError::InvalidPosition)
        ));
        assert!(matches!(
            service.handle_command(PlaybackCommand::SeekBy(f64::NAN)),
            Err(PlaybackError::InvalidPosition)
        ));
        assert!(matches!(
            service.handle_command(PlaybackCommand::SeekBy(1e300)),
            Err(PlaybackError::InvalidPosition)
        ));
        service
            .handle_command(PlaybackCommand::SeekBy(5.0))
            .unwrap();
    }

    #[test]
    fn restore_clamps_overflowing_position_to_zero() {
        let mut service = service();
        service.play_playlist(tracks(1)).unwrap();
        let mut state = service.persisted_state();
        state.position_secs = 1e300;
        service.restore_state(&state).unwrap();
        assert_eq!(service.snapshot().position_secs, 0.0);
    }

    #[test]
    fn persists_and_restores_queue_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut source = service();
        source.play_playlist(tracks(3)).unwrap();
        source.play_index(1).unwrap();
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

    #[test]
    fn playback_position_advances_and_freezes_on_pause() {
        let mut service = service();
        service.play_playlist(tracks(1)).unwrap();
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

    #[test]
    fn wait_for_downloads_starts_when_file_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        let mut service = PlaybackService::new(MockSink::default(), dir.path(), 50);
        service.set_wait_for_downloads(true);
        service.play_playlist(tracks(1)).unwrap();

        assert_eq!(service.snapshot().status, PlaybackStatus::Stopped);
        assert_eq!(service.snapshot().current.unwrap().track_id, "video-0");
        assert!(service.sink.plays.is_empty());

        std::fs::create_dir_all(dir.path().join("downloads/youtube")).unwrap();
        std::fs::write(dir.path().join("downloads/youtube/video-0.mp4"), b"audio").unwrap();
        service.set_download_status(
            &TrackKey::youtube("video-0"),
            MusicDownloadStatus::Downloaded,
        );

        assert_eq!(service.snapshot().status, PlaybackStatus::Playing);
        assert_eq!(service.sink.plays.len(), 1);
        assert_eq!(service.sink.plays[0].0, "video-0");
    }

    #[test]
    fn restore_applies_volume_to_the_sink() {
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
}
