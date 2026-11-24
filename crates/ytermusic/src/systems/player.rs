use std::{
    collections::{HashMap, VecDeque},
    sync::atomic::Ordering,
    time::Duration,
};

use common_structs::{MusicDownloadStatus, TrackKey};
use flume::{Receiver, Sender, unbounded};
use log::{error, warn};
use player::{PlayError, Player, PlayerOptions};

use ytpapi2::YoutubeMusicVideoRef;

use crate::{
    DATABASE,
    consts::{CACHE_DIR, CONFIG},
    errors::{handle_error, handle_error_option},
    structures::{media::Media, sound_action::SoundAction},
    systems::DOWNLOAD_MANAGER,
    term::{ManagerMessage, Screens, list_selector::ListSelector, playlist::PLAYER_RUNNING},
};

/// Apply a saved position after a rebuilt player starts.
fn apply_restore_position(
    restore: &mut Option<Duration>,
    seek: impl FnOnce(Duration) -> Result<(), PlayError>,
) {
    if let Some(position) = restore.take()
        && let Err(error) = seek(position)
    {
        warn!("could not restore the playback position: {error:?}");
    }
}

/// Apply the saved position only after playback starts. A failed play clears it.
fn apply_after_play(
    restore: &mut Option<Duration>,
    play_result: Result<(), PlayError>,
    seek: impl FnOnce(Duration) -> Result<(), PlayError>,
) -> Result<(), PlayError> {
    match play_result {
        Ok(()) => {
            apply_restore_position(restore, seek);
            Ok(())
        }
        Err(error) => {
            *restore = None;
            Err(error)
        }
    }
}

pub struct PlayerState {
    pub goto: Screens,
    pub list: Vec<YoutubeMusicVideoRef>,
    pub current: usize,
    pub rtcurrent: Option<YoutubeMusicVideoRef>,
    pub music_status: HashMap<TrackKey, MusicDownloadStatus>,
    pub list_selector: ListSelector,
    pub controls: Media,
    pub sink: Player,
    pub updater: Sender<ManagerMessage>,
    pub soundaction_sender: Sender<SoundAction>,
    pub soundaction_receiver: Receiver<SoundAction>,
    pub stream_error_receiver: Receiver<PlayError>,
    /// Position to restore after the next track starts.
    pub restore_position: Option<Duration>,
    last_download_list: Vec<String>,
}

impl PlayerState {
    fn new(
        soundaction_sender: Sender<SoundAction>,
        soundaction_receiver: Receiver<SoundAction>,
        updater: Sender<ManagerMessage>,
    ) -> Self {
        let (stream_error_sender, stream_error_receiver) = unbounded::<PlayError>();
        let sink = handle_error_option(
            &updater,
            "player creation error",
            Player::new(
                stream_error_sender,
                PlayerOptions::new(CONFIG.player.initial_volume),
            ),
        )
        .unwrap();
        Self {
            controls: Media::new(updater.clone(), soundaction_sender.clone()),
            soundaction_receiver,
            list_selector: ListSelector::default(),
            music_status: HashMap::new(),
            updater,
            stream_error_receiver,
            soundaction_sender,
            sink,
            goto: Screens::Playlist,
            list: Vec::new(),
            current: 0,
            rtcurrent: None,
            restore_position: None,
            last_download_list: Vec::new(),
        }
    }

    pub fn current(&self) -> Option<&YoutubeMusicVideoRef> {
        self.relative_current(0)
    }

    pub fn relative_current(&self, n: isize) -> Option<&YoutubeMusicVideoRef> {
        self.list.get(self.current.saturating_add_signed(n))
    }

    pub fn set_relative_current(&mut self, n: isize) {
        self.current = self.current.saturating_add_signed(n);
    }

    pub fn is_current_download_failed(&self) -> bool {
        self.current()
            .as_ref()
            .map(|x| self.music_status.get(&x.key()) == Some(&MusicDownloadStatus::DownloadFailed))
            .unwrap_or(false)
    }

    pub fn is_current_downloaded(&self) -> bool {
        self.current()
            .as_ref()
            .map(|x| self.music_status.get(&x.key()) == Some(&MusicDownloadStatus::Downloaded))
            .unwrap_or(false)
    }

    pub fn update(&mut self) {
        PLAYER_RUNNING.store(self.current().is_some(), Ordering::SeqCst);
        self.update_controls();
        if self.handle_stream_errors() {
            return;
        }
        if self.current > self.list.len() {
            self.current = self.list.len();
        }
        while let Ok(e) = self.soundaction_receiver.try_recv() {
            e.apply_sound_action(self);
        }
        if self.is_current_download_failed() {
            SoundAction::Next(1).apply_sound_action(self);
        }
        if self.sink.is_finished() {
            if self.is_current_downloaded() && self.rtcurrent.as_ref() == self.current() {
                self.set_relative_current(1);
            }
            if self.handle_stream_errors() {
                return;
            }
            self.update_controls();
            // If the current song is finished, we play the next one but if the next one has failed to download, we skip it
            // TODO(optimize this)
            while self
                .current()
                .map(|x| {
                    self.music_status.get(&x.key()) == Some(&MusicDownloadStatus::DownloadFailed)
                })
                .unwrap_or(false)
            {
                self.set_relative_current(1);
            }

            if self.is_current_downloaded()
                && let Some(video) = self.current().cloned()
            {
                let k = video
                    .key()
                    .download_file(&CACHE_DIR.join("downloads"), "mp4");
                let play_result = self.sink.play(k.as_path());
                let restore = &mut self.restore_position;
                let sink = &mut self.sink;
                match apply_after_play(restore, play_result, |position| sink.try_seek_to(position))
                {
                    Ok(()) => {}
                    Err(e) => {
                        if matches!(e, PlayError::DecoderError(_)) {
                            // Cleaning the file

                            if let Err(error) = DATABASE.remove_video(&video.key()) {
                                error!("could not remove video from database: {error}");
                            }
                            handle_error(
                                &self.updater,
                                "invalid cleaning MP4",
                                std::fs::remove_file(k),
                            );
                            handle_error(
                                &self.updater,
                                "invalid cleaning JSON",
                                std::fs::remove_file(
                                    video
                                        .key()
                                        .download_file(&CACHE_DIR.join("downloads"), "json"),
                                ),
                            );
                            self.current = 0;
                            if let Err(error) = DATABASE.write() {
                                error!("could not save database: {error}");
                            }
                        } else {
                            self.updater
                                .send(ManagerMessage::PassTo(
                                    Screens::DeviceLost,
                                    Box::new(ManagerMessage::Error(
                                        format!("{e:?}"),
                                        Box::new(None),
                                    )),
                                ))
                                .unwrap();
                        }
                    }
                }
            }
        } else {
            self.rtcurrent = self.current().cloned();
        }
        let to_download = self
            .list
            .iter()
            .skip(self.current)
            .chain(self.list.iter().take(self.current).rev())
            .filter(|x| {
                self.music_status.get(&x.key()) == Some(&MusicDownloadStatus::NotDownloaded)
            })
            .take(12)
            .cloned()
            .collect::<VecDeque<_>>();
        let new_ids: Vec<String> = to_download.iter().map(|v| v.video_id.clone()).collect();
        if new_ids != self.last_download_list {
            self.last_download_list = new_ids;
            DOWNLOAD_MANAGER
                .set_download_list(to_download.iter().map(provider_youtube::track_from_ref));
        }
    }

    fn handle_stream_errors(&mut self) -> bool {
        let mut failed = false;

        while let Ok(e) = self.stream_error_receiver.try_recv() {
            if matches!(e, PlayError::StreamRuntimeError(_)) {
                error!("Stream error: {:?}", e);
                failed = true;
                handle_error(&self.updater, "audio device stream error", Err(e));
            } else {
                warn!("audio error: {:?}", e);
            }
        }

        failed
    }
    fn update_controls(&mut self) {
        let current = self.current().cloned();
        let result = self
            .controls
            .update(current, &self.sink)
            .map_err(|x| format!("{x:?}"));
        handle_error::<String>(&self.updater, "Can't update finished media control", result);
    }
}

pub fn player_system(updater: Sender<ManagerMessage>) -> (Sender<SoundAction>, PlayerState) {
    let (tx, rx) = flume::unbounded::<SoundAction>();
    (tx.clone(), PlayerState::new(tx, rx, updater))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn restore_position_seeks_once() {
        let mut restore = Some(Duration::from_secs(30));
        let seen = Cell::new(None);

        apply_restore_position(&mut restore, |position| {
            seen.set(Some(position));
            Ok(())
        });

        assert_eq!(seen.get(), Some(Duration::from_secs(30)));
        assert_eq!(restore, None);
    }

    #[test]
    fn restore_position_skips_an_empty_value() {
        let mut restore = None;
        let called = Cell::new(false);

        apply_restore_position(&mut restore, |_| {
            called.set(true);
            Ok(())
        });

        assert!(!called.get());
        assert_eq!(restore, None);
    }

    #[test]
    fn restore_position_clears_after_a_failure() {
        let mut restore = Some(Duration::from_secs(5));

        apply_restore_position(&mut restore, |_| {
            Err(PlayError::Io(std::io::Error::other("seek failed")))
        });

        assert_eq!(restore, None);
    }

    #[test]
    fn a_successful_play_applies_the_restore_position() {
        let mut restore = Some(Duration::from_secs(30));
        let seen = Cell::new(None);

        let result = apply_after_play(&mut restore, Ok(()), |position| {
            seen.set(Some(position));
            Ok(())
        });

        assert!(result.is_ok());
        assert_eq!(seen.get(), Some(Duration::from_secs(30)));
        assert_eq!(restore, None);
    }

    #[test]
    fn a_failed_play_clears_the_restore_position_without_seeking() {
        let mut restore = Some(Duration::from_secs(30));
        let called = Cell::new(false);

        let result = apply_after_play(
            &mut restore,
            Err(PlayError::Io(std::io::Error::other("play failed"))),
            |_| {
                called.set(true);
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(!called.get());
        assert_eq!(restore, None);
    }
}
