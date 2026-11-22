use std::{path::PathBuf, time::Duration};

use crate::VOLUME_STEP;

/// The source of the current media.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CurrentSource {
    /// A local file. A seek can replay it from its path.
    File(PathBuf),
    /// A reader, for example a stream. It has no file to replay.
    Stream,
}

#[derive(Clone)]
pub struct PlayerData {
    total_duration: Option<Duration>,
    current_source: Option<CurrentSource>,
    volume: u8,
}

impl PlayerData {
    pub fn new(volume: u8) -> Self {
        Self {
            total_duration: None,
            current_source: None,
            volume,
        }
    }

    /// Changes the volume by the volume step. If positive is true, the volume is increased, otherwise it is decreased.
    pub fn change_volume(&mut self, positive: bool) {
        if positive {
            self.set_volume(self.volume().saturating_add(VOLUME_STEP).min(100));
        } else {
            self.set_volume(self.volume().saturating_sub(VOLUME_STEP));
        }
    }

    /// Returns the volume as a f32 between 0.0 and 1.0
    pub fn volume_f32(&self) -> f32 {
        f32::from(self.volume()) / 100.0
    }

    /// Returns the volume as a u8 between 0 and 100
    pub fn volume(&self) -> u8 {
        self.volume
    }

    /// Sets the volume to the given value
    pub fn set_volume(&mut self, volume: u8) {
        self.volume = volume;
    }

    /// Returns the total duration of the current file
    pub fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    /// Sets the total duration of the current file
    pub fn set_total_duration(&mut self, total_duration: Option<Duration>) {
        self.total_duration = total_duration;
    }

    /// Sets the source of the current media
    pub fn set_current_source(&mut self, current_source: Option<CurrentSource>) {
        self.current_source = current_source;
    }

    /// Returns the file to replay when a seek finishes the sink.
    /// Only a file source can replay. A stream has no file.
    pub fn seek_replay_file(&self) -> Option<PathBuf> {
        match &self.current_source {
            Some(CurrentSource::File(path)) => Some(path.clone()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_player_data_has_no_replay_file() {
        let data = PlayerData::new(50);

        assert_eq!(data.seek_replay_file(), None);
    }

    #[test]
    fn file_source_replays_from_its_path() {
        let mut data = PlayerData::new(50);
        data.set_current_source(Some(CurrentSource::File(PathBuf::from("/music/track.mp3"))));

        assert_eq!(
            data.seek_replay_file(),
            Some(PathBuf::from("/music/track.mp3"))
        );
    }

    #[test]
    fn stream_source_clears_the_replay_file() {
        let mut data = PlayerData::new(50);
        data.set_current_source(Some(CurrentSource::File(PathBuf::from("/music/track.mp3"))));
        assert_eq!(
            data.seek_replay_file(),
            Some(PathBuf::from("/music/track.mp3"))
        );
        data.set_current_source(Some(CurrentSource::Stream));

        assert_eq!(data.seek_replay_file(), None);
    }
}
