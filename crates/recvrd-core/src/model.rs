use common_structs::{
    MusicDownloadStatus, PlaybackSourceKind, ProviderId, TrackKey, TrackMetadata, TrackSource,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A playable track independent of the YouTube Music API client.
/// `provider` defaults to YouTube and `track_id` accepts the old
/// `video_id` name, so state files written before the provider
/// model still load.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    #[serde(default)]
    pub provider: ProviderId,
    #[serde(alias = "video_id")]
    pub track_id: String,
    pub title: String,
    pub author: String,
    pub album: String,
    pub duration: String,
    /// The remote media source, when the provider supplies one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Box<TrackSource>>,
    /// Provider metadata for the track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Box<TrackMetadata>>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStatus {
    #[default]
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    #[default]
    Off,
    One,
    All,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueueItem {
    pub track: Track,
    pub download: MusicDownloadStatus,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct QueueSnapshot {
    pub items: Vec<QueueItem>,
    pub current: Option<usize>,
    pub repeat: RepeatMode,
    pub shuffle: bool,
}

/// The output format of the audio sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioOutput {
    pub sample_rate: u32,
    pub format: common_structs::SampleFormat,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlaybackSnapshot {
    pub status: PlaybackStatus,
    pub current_index: Option<usize>,
    pub current: Option<Track>,
    pub position_secs: f64,
    pub volume: u8,
    pub repeat: RepeatMode,
    pub shuffle: bool,
    pub source_kind: Option<PlaybackSourceKind>,
}

impl Default for PlaybackSnapshot {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            current_index: None,
            current: None,
            position_secs: 0.0,
            volume: 50,
            repeat: RepeatMode::Off,
            shuffle: false,
            source_kind: None,
        }
    }
}

/// Queue state. When shuffle is enabled the items are reordered in place and
/// the original order is retained so disabling shuffle restores it.
#[derive(Debug, Clone)]
pub struct Queue {
    items: Vec<QueueItem>,
    original: Option<Vec<QueueItem>>,
    current: Option<usize>,
    repeat: RepeatMode,
    shuffle: bool,
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

impl Queue {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            original: None,
            current: None,
            repeat: RepeatMode::Off,
            shuffle: false,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }

    /// Fill an empty track duration from a keyed duration map.
    /// Return true when a track changed.
    pub fn fill_missing_durations(&mut self, durations: &HashMap<TrackKey, String>) -> bool {
        let mut changed = false;
        for item in &mut self.items {
            if item.track.duration.is_empty()
                && let Some(duration) = durations.get(&item.track.key())
            {
                item.track.duration = duration.clone();
                changed = true;
            }
        }
        changed
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    pub fn current_item(&self) -> Option<&QueueItem> {
        self.current.and_then(|index| self.items.get(index))
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.current_item().map(|item| &item.track)
    }

    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn set_repeat(&mut self, repeat: RepeatMode) {
        self.repeat = repeat;
    }

    /// Enable/disable shuffle with a system-generated seed.
    pub fn set_shuffle(&mut self, enabled: bool) {
        let seed = fastrand::u64(..);
        self.set_shuffle_with_seed(enabled, seed);
    }

    /// Deterministic variant used by tests.
    pub fn set_shuffle_with_seed(&mut self, enabled: bool, seed: u64) {
        if enabled == self.shuffle {
            return;
        }
        let current_key = self.current_track().map(|track| track.key());

        if enabled {
            self.original = Some(self.items.clone());
            shuffle_items(&mut self.items, seed);
            self.shuffle = true;
        } else {
            if let Some(original) = self.original.take() {
                self.items = original;
            }
            self.shuffle = false;
        }

        self.current = current_key
            .and_then(|key| self.items.iter().position(|item| item.track.key() == key))
            .or_else(|| (!self.items.is_empty()).then_some(0));
    }

    pub fn replace(&mut self, tracks: Vec<Track>) {
        self.items = tracks
            .into_iter()
            .map(|track| QueueItem {
                track,
                download: MusicDownloadStatus::NotDownloaded,
            })
            .collect();
        self.original = None;
        self.current = (!self.items.is_empty()).then_some(0);

        if self.shuffle && !self.items.is_empty() {
            let seed = fastrand::u64(..);
            self.original = Some(self.items.clone());
            let current_key = self.current_track().map(|track| track.key());
            shuffle_items(&mut self.items, seed);
            self.current = current_key
                .and_then(|key| self.items.iter().position(|item| item.track.key() == key))
                .or_else(|| (!self.items.is_empty()).then_some(0));
        }
    }

    pub fn enqueue(&mut self, track: Track) {
        let item = QueueItem {
            track,
            download: MusicDownloadStatus::NotDownloaded,
        };
        self.items.push(item.clone());
        if let Some(original) = &mut self.original {
            original.push(item);
        }
        if self.current.is_none() {
            self.current = Some(0);
        }
    }

    /// Insert a track after the current track. Keep the current item.
    pub fn enqueue_next(&mut self, track: Track) {
        let item = QueueItem {
            track,
            download: MusicDownloadStatus::NotDownloaded,
        };
        let index = match self.current {
            Some(current) => current + 1,
            None => 0,
        };
        // Keep the unshuffled order in step: insert after the current
        // track there, or at the front when nothing is current.
        let original_index = match self.current {
            Some(current) => {
                let current_key = self.items.get(current).map(|item| item.track.key());
                self.original.as_ref().and_then(|original| {
                    current_key.and_then(|key| {
                        original
                            .iter()
                            .position(|entry| entry.track.key() == key)
                            .map(|position| position + 1)
                    })
                })
            }
            None => Some(0),
        };
        self.items.insert(index, item.clone());
        if let Some(original) = &mut self.original {
            let at = original_index.unwrap_or(original.len());
            original.insert(at.min(original.len()), item);
        }
        if self.current.is_none() {
            self.current = Some(0);
        }
    }

    /// Insert a track after the current track and make it current.
    pub fn play_now(&mut self, track: Track) {
        let item = QueueItem {
            track,
            download: MusicDownloadStatus::NotDownloaded,
        };
        let index = match self.current {
            Some(current) => current + 1,
            None => 0,
        };
        // Keep the unshuffled order in step: insert after the current
        // track there, not at the end.
        let original_index = self.current.and_then(|current| {
            let current_key = self.items.get(current)?.track.key();
            self.original.as_ref().and_then(|original| {
                original
                    .iter()
                    .position(|entry| entry.track.key() == current_key)
                    .map(|position| position + 1)
            })
        });
        self.items.insert(index, item.clone());
        if let Some(original) = &mut self.original {
            let at = original_index.unwrap_or(original.len());
            original.insert(at.min(original.len()), item);
        }
        self.current = Some(index);
    }

    pub fn play_index(&mut self, index: usize) -> bool {
        if index < self.items.len() {
            self.current = Some(index);
            true
        } else {
            false
        }
    }

    pub fn remove(&mut self, index: usize) -> Option<QueueItem> {
        if index >= self.items.len() {
            return None;
        }
        let removed = self.items.remove(index);
        let removed_key = removed.track.key();
        if let Some(original) = &mut self.original {
            original.retain(|item| item.track.key() != removed_key);
        }
        self.current = match self.current {
            None => None,
            Some(current) if current == index => {
                if self.items.is_empty() {
                    None
                } else {
                    Some(index.min(self.items.len() - 1))
                }
            }
            Some(current) if current > index => Some(current - 1),
            current => current,
        };
        Some(removed)
    }

    pub fn move_item(&mut self, from: usize, to: usize) -> bool {
        if from >= self.items.len() || to >= self.items.len() || from == to {
            return false;
        }
        let item = self.items.remove(from);
        let track_key = item.track.key();
        self.items.insert(to, item);

        if let Some(original) = &mut self.original
            && let Some(original_index) = original
                .iter()
                .position(|entry| entry.track.key() == track_key)
        {
            let moved = original.remove(original_index);
            let target = to.min(original.len());
            original.insert(target, moved);
        }

        self.current = match self.current {
            Some(current) if current == from => Some(to),
            Some(current) if from < current && to >= current => Some(current - 1),
            Some(current) if from > current && to <= current => Some(current + 1),
            current => current,
        };
        true
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.original = None;
        self.current = None;
    }

    /// The unshuffled order, when shuffle is enabled.
    pub fn original_tracks(&self) -> Vec<Track> {
        self.original
            .as_ref()
            .map(|items| items.iter().map(|item| item.track.clone()).collect())
            .unwrap_or_default()
    }

    /// Restore a previously persisted queue in its saved playback order.
    /// `original_tracks` holds the unshuffled order when the saved queue
    /// was shuffled, so disabling shuffle restores it after a restart.
    pub fn restore(
        &mut self,
        tracks: Vec<Track>,
        original_tracks: Vec<Track>,
        current: Option<usize>,
        repeat: RepeatMode,
        shuffle: bool,
    ) {
        self.items = tracks
            .into_iter()
            .map(|track| QueueItem {
                track,
                download: MusicDownloadStatus::NotDownloaded,
            })
            .collect();
        self.original = if shuffle && !original_tracks.is_empty() {
            Some(
                original_tracks
                    .into_iter()
                    .map(|track| QueueItem {
                        track,
                        download: MusicDownloadStatus::NotDownloaded,
                    })
                    .collect(),
            )
        } else {
            None
        };
        self.current = current
            .filter(|index| *index < self.items.len())
            .or_else(|| (!self.items.is_empty()).then_some(0));
        self.repeat = repeat;
        self.shuffle = shuffle;
    }

    pub fn set_current(&mut self, index: Option<usize>) {
        self.current = index.filter(|index| *index < self.items.len());
    }

    /// Fill in the current track's duration once the audio backend knows the
    /// real length of the media file.
    pub fn set_current_duration(&mut self, duration: impl Into<String>) -> bool {
        let duration = duration.into();
        let Some(index) = self.current else {
            return false;
        };
        let Some(track_key) = self.items.get(index).map(|item| item.track.key()) else {
            return false;
        };
        let mut changed = false;
        if let Some(item) = self.items.get_mut(index)
            && item.track.duration != duration
        {
            item.track.duration = duration.clone();
            changed = true;
        }
        if let Some(original) = &mut self.original
            && let Some(item) = original
                .iter_mut()
                .find(|item| item.track.key() == track_key)
            && item.track.duration != duration
        {
            item.track.duration = duration;
            changed = true;
        }
        changed
    }

    /// Next index for an explicit next/previous command.
    pub fn next_index(&self) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        let current = self.current.unwrap_or(0);
        if current + 1 < self.items.len() {
            Some(current + 1)
        } else if self.repeat == RepeatMode::All {
            Some(0)
        } else {
            None
        }
    }

    pub fn previous_index(&self) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        let current = self.current.unwrap_or(0);
        if current > 0 {
            Some(current - 1)
        } else if self.repeat == RepeatMode::All {
            Some(self.items.len() - 1)
        } else {
            Some(0)
        }
    }

    pub fn set_download_status(&mut self, key: &TrackKey, status: MusicDownloadStatus) {
        for item in &mut self.items {
            if item.track.key() == *key {
                item.download = status;
            }
        }
        if let Some(original) = &mut self.original {
            for item in original {
                if item.track.key() == *key {
                    item.download = status;
                }
            }
        }
    }

    /// Mark every queued track as not downloaded, except the kept
    /// track. Call this after the cached files are gone, so the
    /// queue no longer claims deleted downloads are complete.
    pub fn reset_download_statuses(&mut self, keep: Option<&TrackKey>) {
        for item in &mut self.items {
            if Some(&item.track.key()) != keep {
                item.download = MusicDownloadStatus::NotDownloaded;
            }
        }
        if let Some(original) = &mut self.original {
            for item in original {
                if Some(&item.track.key()) != keep {
                    item.download = MusicDownloadStatus::NotDownloaded;
                }
            }
        }
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            items: self.items.clone(),
            current: self.current,
            repeat: self.repeat,
            shuffle: self.shuffle,
        }
    }
}

fn shuffle_items<T>(items: &mut [T], seed: u64) {
    let mut rng = fastrand::Rng::with_seed(seed);
    for i in (1..items.len()).rev() {
        let j = rng.usize(0..=i);
        items.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_structs::MusicDownloadStatus;

    fn tracks(count: usize) -> Vec<Track> {
        (0..count)
            .map(|index| Track {
                provider: ProviderId::Youtube,
                track_id: format!("video-{index}"),
                title: format!("Track {index}"),
                author: "Artist".to_string(),
                album: "Album".to_string(),
                duration: "3:00".to_string(),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn replace_and_navigation() {
        let mut queue = Queue::new();
        queue.replace(tracks(3));
        assert_eq!(queue.current_index(), Some(0));
        assert_eq!(queue.next_index(), Some(1));
        queue.set_current(Some(1));
        assert_eq!(queue.next_index(), Some(2));
        queue.set_current(Some(2));
        assert_eq!(queue.next_index(), None);
        assert_eq!(queue.previous_index(), Some(1));
    }

    #[test]
    fn repeat_all_wraps_navigation() {
        let mut queue = Queue::new();
        queue.replace(tracks(2));
        queue.set_current(Some(1));
        queue.set_repeat(RepeatMode::All);
        assert_eq!(queue.next_index(), Some(0));
        assert_eq!(queue.previous_index(), Some(0));
    }

    #[test]
    fn enqueue_and_play_now() {
        let mut queue = Queue::new();
        let tracks = tracks(3);
        queue.enqueue(tracks[0].clone());
        queue.enqueue(tracks[1].clone());
        assert_eq!(queue.len(), 2);
        queue.play_now(tracks[2].clone());
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.current_index(), Some(1));
        assert_eq!(queue.current_track().unwrap().track_id, "video-2");
    }

    #[test]
    fn enqueue_next_inserts_after_the_current_track() {
        let mut queue = Queue::new();
        let tracks = tracks(4);
        queue.replace(tracks[..3].to_vec());
        queue.set_current(Some(1));
        queue.enqueue_next(tracks[3].clone());
        assert_eq!(queue.len(), 4);
        assert_eq!(queue.current_index(), Some(1));
        assert_eq!(queue.current_track().unwrap().track_id, "video-1");
        let ids: Vec<_> = queue
            .items()
            .iter()
            .map(|item| item.track.track_id.as_str())
            .collect();
        assert_eq!(ids, vec!["video-0", "video-1", "video-3", "video-2"]);
    }

    #[test]
    fn enqueue_next_starts_an_empty_queue() {
        let mut queue = Queue::new();
        let mut items = tracks(1);
        queue.enqueue_next(items.remove(0));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.current_index(), Some(0));
    }

    #[test]
    fn enqueue_next_uses_the_front_without_a_current_track() {
        let mut queue = Queue::new();
        let tracks = tracks(3);
        queue.replace(tracks[..2].to_vec());
        queue.set_current(None);
        queue.enqueue_next(tracks[2].clone());
        assert_eq!(queue.current_index(), Some(0));
        assert_eq!(queue.items()[0].track.track_id, "video-2");
        assert_eq!(queue.items()[1].track.track_id, "video-0");
    }

    #[test]
    fn fill_missing_durations_updates_empty_tracks() {
        let mut queue = Queue::new();
        let mut items = tracks(2);
        items[0].duration.clear();
        queue.replace(items);
        let key = queue.items()[0].track.key();
        let mut durations = HashMap::new();
        durations.insert(key, "4:05".to_string());

        assert!(queue.fill_missing_durations(&durations));
        assert_eq!(queue.items()[0].track.duration, "4:05");
        assert_eq!(queue.items()[1].track.duration, "3:00");
        assert!(!queue.fill_missing_durations(&durations));
    }

    #[test]
    fn remove_adjusts_current() {
        let mut queue = Queue::new();
        queue.replace(tracks(3));
        queue.set_current(Some(2));
        queue.remove(0);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.current_index(), Some(1));
        queue.remove(1);
        assert_eq!(queue.current_index(), Some(0));
        queue.remove(0);
        assert_eq!(queue.current_index(), None);
    }

    #[test]
    fn move_item_adjusts_current() {
        let mut queue = Queue::new();
        queue.replace(tracks(3));
        queue.set_current(Some(1));
        assert!(queue.move_item(1, 2));
        assert_eq!(queue.current_index(), Some(2));
        assert_eq!(queue.current_track().unwrap().track_id, "video-1");
    }

    #[test]
    fn shuffle_is_stable_and_restorable() {
        let mut queue = Queue::new();
        queue.replace(tracks(8));
        let original: Vec<_> = queue
            .items()
            .iter()
            .map(|item| item.track.track_id.clone())
            .collect();

        queue.set_shuffle_with_seed(true, 42);
        let shuffled: Vec<_> = queue
            .items()
            .iter()
            .map(|item| item.track.track_id.clone())
            .collect();
        assert_ne!(original, shuffled);

        queue.set_shuffle(false);
        let restored: Vec<_> = queue
            .items()
            .iter()
            .map(|item| item.track.track_id.clone())
            .collect();
        assert_eq!(original, restored);

        queue.set_shuffle_with_seed(true, 42);
        let shuffled_again: Vec<_> = queue
            .items()
            .iter()
            .map(|item| item.track.track_id.clone())
            .collect();
        assert_eq!(shuffled, shuffled_again);
    }

    #[test]
    fn shuffle_preserves_current_track() {
        let mut queue = Queue::new();
        queue.replace(tracks(5));
        queue.set_current(Some(2));
        let current = queue.current_track().unwrap().track_id.clone();
        queue.set_shuffle_with_seed(true, 7);
        assert_eq!(queue.current_track().unwrap().track_id, current);
    }

    #[test]
    fn download_status_updates_items() {
        let mut queue = Queue::new();
        queue.replace(tracks(2));
        queue.set_download_status(
            &TrackKey::youtube("video-1"),
            MusicDownloadStatus::Downloaded,
        );
        assert_eq!(queue.items()[1].download, MusicDownloadStatus::Downloaded);
    }

    #[test]
    fn reset_download_statuses_keeps_the_kept_track() {
        let mut queue = Queue::new();
        queue.replace(tracks(3));
        for id in ["video-0", "video-1", "video-2"] {
            queue.set_download_status(&TrackKey::youtube(id), MusicDownloadStatus::Downloaded);
        }
        queue.reset_download_statuses(Some(&TrackKey::youtube("video-1")));
        assert_eq!(
            queue.items()[0].download,
            MusicDownloadStatus::NotDownloaded
        );
        assert_eq!(queue.items()[1].download, MusicDownloadStatus::Downloaded);
        assert_eq!(
            queue.items()[2].download,
            MusicDownloadStatus::NotDownloaded
        );
    }

    #[test]
    fn reset_download_statuses_without_a_kept_track_resets_all() {
        let mut queue = Queue::new();
        queue.replace(tracks(2));
        queue.set_download_status(
            &TrackKey::youtube("video-0"),
            MusicDownloadStatus::Downloaded,
        );
        queue.reset_download_statuses(None);
        assert!(
            queue
                .items()
                .iter()
                .all(|item| item.download == MusicDownloadStatus::NotDownloaded)
        );
    }

    #[test]
    fn download_status_by_key_touches_only_the_matching_track() {
        let mut queue = Queue::new();
        queue.replace(tracks(3));
        queue.set_download_status(
            &TrackKey::youtube("video-1"),
            MusicDownloadStatus::Downloaded,
        );
        assert_eq!(
            queue.items()[0].download,
            MusicDownloadStatus::NotDownloaded
        );
        assert_eq!(queue.items()[1].download, MusicDownloadStatus::Downloaded);
        assert_eq!(
            queue.items()[2].download,
            MusicDownloadStatus::NotDownloaded
        );
    }

    #[test]
    fn track_key_carries_the_provider() {
        let track = tracks(1).into_iter().next().unwrap();
        assert_eq!(track.key(), TrackKey::youtube("video-0"));
        assert_eq!(track.key().provider, ProviderId::Youtube);
    }

    #[test]
    fn track_reads_the_old_video_id_field() {
        // State files written before the provider model have
        // `video_id` and no `provider`. They still load, as YouTube.
        let json = r#"{"video_id":"dQw4w9WgXcQ","title":"Song","author":"Artist","album":"Album","duration":"3:00"}"#;
        let track: Track = serde_json::from_str(json).unwrap();
        assert_eq!(track.track_id, "dQw4w9WgXcQ");
        assert_eq!(track.provider, ProviderId::Youtube);
    }

    fn track_ids(queue: &Queue) -> Vec<String> {
        queue
            .items()
            .iter()
            .map(|item| item.track.track_id.clone())
            .collect()
    }

    #[test]
    fn restore_keeps_unshuffled_order() {
        let mut queue = Queue::new();
        queue.replace(tracks(6));
        let original = tracks(6);
        queue.set_shuffle_with_seed(true, 42);
        let shuffled: Vec<Track> = queue
            .items()
            .iter()
            .map(|item| item.track.clone())
            .collect();
        assert_ne!(shuffled, original);

        let mut restored = Queue::new();
        restored.restore(shuffled, original.clone(), Some(0), RepeatMode::Off, true);
        restored.set_shuffle(false);
        let expected: Vec<String> = original
            .iter()
            .map(|track| track.track_id.clone())
            .collect();
        assert_eq!(track_ids(&restored), expected);
    }

    #[test]
    fn play_now_inserts_after_current_in_unshuffled_order() {
        let mut queue = Queue::new();
        queue.replace(tracks(5));
        queue.set_current(Some(2));
        queue.set_shuffle_with_seed(true, 7);
        let new_track = Track {
            provider: ProviderId::Youtube,
            track_id: "video-new".to_string(),
            title: "New".to_string(),
            author: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
            ..Default::default()
        };
        queue.play_now(new_track);
        assert_eq!(queue.current_track().unwrap().track_id, "video-new");

        queue.set_shuffle(false);
        assert_eq!(
            track_ids(&queue),
            [
                "video-0",
                "video-1",
                "video-2",
                "video-new",
                "video-3",
                "video-4",
            ]
        );
    }
}
