use common_structs::{MusicDownloadStatus, TrackKey};
use download_manager::{DownloadManagerMessage, MessageHandler};
use flume::Sender;
use log::{error, trace};
use std::{fs, sync::Arc, time::Duration};
use ytpapi2::YoutubeMusicVideoRef;

use crate::{
    DATABASE, ShutdownSignal,
    consts::CACHE_DIR,
    errors::handle_error_option,
    systems::{DOWNLOAD_MANAGER, player::PlayerState},
};

/// Actions that can be sent to the player from other services
#[derive(Debug, Clone)]
pub enum SoundAction {
    /// Set the volume of the player to the given value
    SetVolume(f32),
    Cleanup,
    PlayPause,
    Plus,
    Minus,
    /// Seek to a specific time in the current song in seconds
    SeekTo(Duration),
    /// Seek forward or backward by the given amount
    SeekByAmount {
        amount: Duration,
        forward: bool,
    },
    Previous(usize),
    Forward,
    Backward,
    Next(usize),
    AddVideosToQueue(Vec<YoutubeMusicVideoRef>),
    AddVideoUnary(YoutubeMusicVideoRef),
    DeleteVideoUnary,
    ReplaceQueue(Vec<YoutubeMusicVideoRef>),
    VideoStatusUpdate(TrackKey, MusicDownloadStatus),
}

impl SoundAction {
    /// Rebuild the player and replay the current track.
    ///
    /// Return true when the player is rebuilt. On failure the error handler
    /// switches to the DeviceLost screen and this function returns false.
    pub fn restart_player(player: &mut PlayerState) -> bool {
        let position = player.sink.elapsed();
        let Some(sink) =
            handle_error_option(&player.updater, "update player", player.sink.update())
        else {
            return false;
        };
        player.sink = sink;
        if let Some(e) = player.current().cloned() {
            player.restore_position = if position.is_zero() {
                None
            } else {
                Some(position)
            };
            Self::AddVideoUnary(e).apply_sound_action(player);
        } else {
            player.restore_position = None;
        }
        true
    }
    fn insert(player: &mut PlayerState, video: TrackKey, status: MusicDownloadStatus) {
        if matches!(
            player.music_status.get(&video),
            Some(&MusicDownloadStatus::DownloadFailed)
        ) {
            DOWNLOAD_MANAGER.remove_from_in_downloads(&video);
        }
        if matches!(
            player.music_status.get(&video),
            Some(&MusicDownloadStatus::Downloading(_) | &MusicDownloadStatus::Downloaded)
        ) && status == MusicDownloadStatus::NotDownloaded
        {
            return;
        }
        player.music_status.insert(video, status);
    }

    pub fn apply_sound_action(self, player: &mut PlayerState) {
        match self {
            Self::SetVolume(volume) => player.sink.set_volume((volume * 100.) as i32),
            Self::SeekTo(time) => player.sink.seek_to(time),
            Self::SeekByAmount { amount, forward } => {
                let target = Self::seek_target(player.sink.elapsed(), amount, forward);
                player.sink.seek_to(target);
            }
            Self::Backward => player.sink.seek_bw(),
            Self::Forward => player.sink.seek_fw(),
            Self::PlayPause => player.sink.toggle_playback(),
            Self::Cleanup => {
                player.list.clear();
                player.current = 0;
                player.music_status.clear();
                player.restore_position = None;
                player.sink.stop();
            }
            Self::Plus => player.sink.volume_up(),
            Self::Minus => player.sink.volume_down(),
            Self::Next(a) => {
                player.restore_position = None;
                player.sink.stop();

                player.set_relative_current(a as _);
            }
            Self::VideoStatusUpdate(key, status) => {
                player.music_status.insert(key, status);
            }
            Self::AddVideosToQueue(video) => {
                let db = DATABASE.read_or_empty();
                for v in video {
                    Self::insert(
                        player,
                        v.key(),
                        if db.iter().any(|e| e.key() == v.key()) {
                            MusicDownloadStatus::Downloaded
                        } else {
                            MusicDownloadStatus::NotDownloaded
                        },
                    );
                    player.list.push(v)
                }
            }
            Self::Previous(a) => {
                player.restore_position = None;
                player.set_relative_current(-(a as isize));
                player.sink.stop();
            }
            Self::AddVideoUnary(video) => {
                Self::insert(
                    player,
                    video.key(),
                    if DATABASE
                        .read_or_empty()
                        .iter()
                        .any(|e| e.key() == video.key())
                    {
                        MusicDownloadStatus::Downloaded
                    } else {
                        MusicDownloadStatus::NotDownloaded
                    },
                );
                if player.list.is_empty() {
                    player.list.push(video);
                } else {
                    player.list.insert(player.current + 1, video);
                }
            }
            Self::DeleteVideoUnary => {
                let index_list = player.list_selector.get_relative_position();
                let video = player.relative_current(index_list).cloned().unwrap();
                if matches!(
                    player.music_status.get(&video.key()), // not sure abt conditions, needs testing
                    Some(
                        &MusicDownloadStatus::DownloadFailed
                            | &MusicDownloadStatus::Downloading(_)
                            | &MusicDownloadStatus::NotDownloaded
                    )
                ) {
                    DOWNLOAD_MANAGER.remove_from_in_downloads(&video.key());
                }
                player.music_status.remove(&video.key()); // maybe not necessary to do it

                //manage deleting in the list
                player.list.retain(|vid| *vid != video);
                player.list_selector.list_size -= 1;
                if index_list < 0 {
                    player.set_relative_current(-1);
                }
                if index_list == 0 {
                    Self::Next(0).apply_sound_action(player);
                }

                // manage deleting physically
                if let Err(error) = DATABASE.remove_video(&video.key()) {
                    error!("could not remove video from database: {error}");
                }

                let cache_folder = CACHE_DIR.join("downloads");
                let json_path = video.key().download_file(&cache_folder, "json");
                match fs::remove_file(json_path) {
                    Ok(_) => trace!("Deleted JSON file"),
                    Err(e) => error!("Error deleting JSON video file: {}", e),
                }

                let mp4_path = video.key().download_file(&cache_folder, "mp4");
                match fs::remove_file(mp4_path) {
                    Ok(_) => trace!("Deleted MP4 file"),
                    Err(e) => error!("Error deleting MP4 video file: {}", e),
                }
            }
            Self::ReplaceQueue(videos) => {
                player.list.truncate(player.current + 1);
                DOWNLOAD_MANAGER.clean(
                    ShutdownSignal,
                    download_manager_handler(player.soundaction_sender.clone()),
                );
                Self::AddVideosToQueue(videos).apply_sound_action(player);
                Self::Next(1).apply_sound_action(player);
            }
        }
    }

    /// Target position for a relative seek. A backward seek stops at zero.
    fn seek_target(current: Duration, amount: Duration, forward: bool) -> Duration {
        if forward {
            current + amount
        } else {
            current.saturating_sub(amount)
        }
    }
}

pub fn download_manager_handler(sender: Sender<SoundAction>) -> MessageHandler {
    Arc::new(move |message| match message {
        DownloadManagerMessage::VideoStatusUpdate(video, status) => {
            sender
                .send(SoundAction::VideoStatusUpdate(video, status))
                .unwrap();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_target_moves_forward() {
        assert_eq!(
            SoundAction::seek_target(Duration::from_secs(10), Duration::from_secs(5), true),
            Duration::from_secs(15)
        );
    }

    #[test]
    fn seek_target_moves_backward() {
        assert_eq!(
            SoundAction::seek_target(Duration::from_secs(10), Duration::from_secs(5), false),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn seek_target_stops_at_zero() {
        assert_eq!(
            SoundAction::seek_target(Duration::from_secs(3), Duration::from_secs(5), false),
            Duration::ZERO
        );
    }
}
