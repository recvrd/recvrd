//! Background workers for the daemon event stream and commands.

use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use ytermusicd_api::{CreatePlaylistRequest, PlaylistPrivacy, RepeatMode, UpdatePlaylistRequest};
use ytermusicd_client::{DaemonClient, DaemonError};

use crate::cover;
use crate::event::{Command, DaemonEvent, ToastLevel};

enum Failure {
    Cover(String),
    Playlist(String),
    Search(String),
    Toast,
}

/// Run the event stream. Reconnect with backoff and replay from the last event.
pub async fn run_events(client: DaemonClient, sender: UnboundedSender<DaemonEvent>) {
    let mut last_id = None;
    let mut delay = Duration::from_secs(1);
    loop {
        match client.events(last_id).await {
            Ok(mut stream) => {
                if sender.send(DaemonEvent::Connected).is_err() {
                    return;
                }
                delay = Duration::from_secs(1);
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(event) => {
                            last_id = Some(event.id);
                            if sender.send(DaemonEvent::from_event(event)).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = sender.send(DaemonEvent::Disconnected(error));
                            break;
                        }
                    }
                }
            }
            Err(error) => {
                if sender.send(DaemonEvent::Disconnected(error)).is_err() {
                    return;
                }
            }
        }
        tokio::time::sleep(delay).await;
        delay = delay.saturating_mul(2).min(Duration::from_secs(30));
    }
}

/// Run commands. Mutations run in order. Search and cover requests run in
/// parallel so a slow request cannot delay playback control.
pub async fn run_commands(
    client: DaemonClient,
    mut receiver: UnboundedReceiver<Command>,
    sender: UnboundedSender<DaemonEvent>,
) {
    while let Some(command) = receiver.recv().await {
        if matches!(command, Command::Search(_) | Command::LoadCover(_)) {
            let client = client.clone();
            let sender = sender.clone();
            tokio::spawn(async move {
                run_one(&client, command, &sender).await;
            });
        } else {
            run_one(&client, command, &sender).await;
        }
    }
}

async fn run_one(client: &DaemonClient, command: Command, sender: &UnboundedSender<DaemonEvent>) {
    let failure = match &command {
        Command::LoadCover(video_id) => Failure::Cover(video_id.clone()),
        Command::OpenPlaylist(id) | Command::RefreshPlaylist(id) => Failure::Playlist(id.clone()),
        Command::Search(query) => Failure::Search(query.clone()),
        _ => Failure::Toast,
    };
    match execute(client, command).await {
        Ok(events) => {
            for event in events {
                if sender.send(event).is_err() {
                    return;
                }
            }
        }
        Err(error) => {
            let event = match failure {
                Failure::Cover(video_id) => DaemonEvent::Cover {
                    video_id,
                    art: None,
                },
                Failure::Playlist(id) => DaemonEvent::PlaylistError {
                    id,
                    message: error.message,
                },
                Failure::Search(query) => DaemonEvent::SearchError {
                    query,
                    message: error.message,
                },
                Failure::Toast => DaemonEvent::Toast {
                    level: ToastLevel::Error,
                    message: error.message,
                },
            };
            let _ = sender.send(event);
        }
    }
}

async fn execute(client: &DaemonClient, command: Command) -> Result<Vec<DaemonEvent>, DaemonError> {
    match command {
        Command::RefreshPlaylists => Ok(vec![DaemonEvent::Playlists(client.playlists().await?)]),
        Command::OpenPlaylist(id) | Command::RefreshPlaylist(id) => {
            Ok(vec![DaemonEvent::Playlist(client.playlist(&id).await?)])
        }
        Command::PlayPlaylist(id) => {
            Ok(vec![DaemonEvent::Status(client.play_playlist(&id).await?)])
        }
        Command::PlayTrack(track) => Ok(vec![DaemonEvent::Status(client.play_track(track).await?)]),
        Command::QueueTrack(track) => Ok(vec![DaemonEvent::Queue(client.queue_add(track).await?)]),
        Command::Pause => Ok(vec![DaemonEvent::Status(client.pause().await?)]),
        Command::Resume => Ok(vec![DaemonEvent::Status(client.resume().await?)]),
        Command::Next => Ok(vec![DaemonEvent::Status(client.next().await?)]),
        Command::Previous => Ok(vec![DaemonEvent::Status(client.previous().await?)]),
        Command::Seek(position) => Ok(vec![DaemonEvent::Status(client.seek(position).await?)]),
        Command::Volume(volume) => Ok(vec![DaemonEvent::Status(client.volume(volume).await?)]),
        Command::Shuffle(enabled) => Ok(vec![DaemonEvent::Status(client.shuffle(enabled).await?)]),
        Command::Repeat(repeat) => Ok(vec![DaemonEvent::Status(client.repeat(repeat).await?)]),
        Command::QueueRemove(index) => {
            Ok(vec![DaemonEvent::Queue(client.queue_remove(index).await?)])
        }
        Command::QueueReorder { from, to } => Ok(vec![DaemonEvent::Queue(
            client.queue_reorder(from, to).await?,
        )]),
        Command::QueuePlayNext(index) => Ok(vec![DaemonEvent::Queue(
            client.queue_play_next(index).await?,
        )]),
        Command::QueueClear => Ok(vec![DaemonEvent::Queue(client.queue_clear().await?)]),
        Command::RetryDownload(video_id) => {
            client.retry_download(&video_id).await?;
            Ok(vec![DaemonEvent::Toast {
                level: ToastLevel::Success,
                message: "Download retry requested.".to_string(),
            }])
        }
        Command::CreatePlaylist { title } => {
            client
                .create_playlist(&CreatePlaylistRequest {
                    title,
                    description: String::new(),
                    privacy: PlaylistPrivacy::Private,
                })
                .await?;
            Ok(vec![
                DaemonEvent::Playlists(client.playlists().await?),
                DaemonEvent::Toast {
                    level: ToastLevel::Success,
                    message: "Playlist created.".to_string(),
                },
            ])
        }
        Command::RenamePlaylist { id, title } => {
            client
                .update_playlist(&id, &UpdatePlaylistRequest { title })
                .await?;
            Ok(vec![DaemonEvent::Playlists(client.playlists().await?)])
        }
        Command::DeletePlaylist(id) => {
            client.delete_playlist(&id).await?;
            Ok(vec![
                DaemonEvent::Playlists(client.playlists().await?),
                DaemonEvent::Toast {
                    level: ToastLevel::Success,
                    message: "Playlist deleted.".to_string(),
                },
            ])
        }
        Command::RemovePlaylistTrack { id, index } => {
            let tracks = client.remove_playlist_tracks(&id, vec![index]).await?;
            Ok(vec![DaemonEvent::PlaylistTracks { id, tracks }])
        }
        Command::MovePlaylistTrack { id, from, to } => {
            let tracks = client.move_playlist_track(&id, from, to).await?;
            Ok(vec![DaemonEvent::PlaylistTracks { id, tracks }])
        }
        Command::AddPlaylistTracks { id, video_ids } => {
            let tracks = client.add_playlist_tracks(&id, video_ids).await?;
            Ok(vec![
                DaemonEvent::PlaylistTracks { id, tracks },
                DaemonEvent::Toast {
                    level: ToastLevel::Success,
                    message: "Tracks added to the playlist.".to_string(),
                },
            ])
        }
        Command::Search(query) => {
            let results = client.search(&query).await?;
            Ok(vec![DaemonEvent::Search { query, results }])
        }
        Command::LoadCover(video_id) => {
            let bytes = client.cover(&video_id).await?;
            let art = tokio::task::spawn_blocking(move || cover::render_jpeg(&bytes, 10, 5))
                .await
                .ok()
                .flatten();
            Ok(vec![DaemonEvent::Cover { video_id, art }])
        }
    }
}

/// Return the repeat mode after a cycle.
pub fn next_repeat(repeat: RepeatMode) -> RepeatMode {
    match repeat {
        RepeatMode::Off => RepeatMode::All,
        RepeatMode::All => RepeatMode::One,
        RepeatMode::One => RepeatMode::Off,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_repeat_modes() {
        assert_eq!(next_repeat(RepeatMode::Off), RepeatMode::All);
        assert_eq!(next_repeat(RepeatMode::All), RepeatMode::One);
        assert_eq!(next_repeat(RepeatMode::One), RepeatMode::Off);
    }
}
