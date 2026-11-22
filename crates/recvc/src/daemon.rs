//! Background workers for the daemon event stream and commands.

use std::time::Duration;

use recvrd_api::{
    CreatePlaylistRequest, PlaylistPrivacy, ProviderId, RepeatMode, UpdatePlaylistRequest,
};
use recvrd_client::{DaemonClient, DaemonError};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use recvrd_api::TrackKey;

use crate::cover;
use crate::event::{Command, DaemonEvent, ToastLevel};

enum Failure {
    Cover(TrackKey),
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
/// The worker holds the active provider. Library, playlist and search
/// requests use it. The UI switches it with `Command::SetProvider`.
pub async fn run_commands(
    client: DaemonClient,
    mut receiver: UnboundedReceiver<Command>,
    sender: UnboundedSender<DaemonEvent>,
) {
    let mut provider = ProviderId::Youtube;
    while let Some(command) = receiver.recv().await {
        if matches!(command, Command::SetProvider(_)) {
            if let Command::SetProvider(next) = command {
                provider = next;
                run_one(&client, provider, Command::RefreshPlaylists, &sender).await;
            }
            continue;
        }
        if matches!(command, Command::RefreshProviders) {
            match client.providers().await {
                Ok(providers) => {
                    let _ = sender.send(DaemonEvent::Providers(providers));
                }
                Err(error) => {
                    let _ = sender.send(DaemonEvent::Toast {
                        level: ToastLevel::Error,
                        message: error.message,
                    });
                }
            }
            continue;
        }
        if matches!(command, Command::Search(_) | Command::LoadCover(_)) {
            let client = client.clone();
            let sender = sender.clone();
            tokio::spawn(async move {
                run_one(&client, provider, command, &sender).await;
            });
        } else {
            run_one(&client, provider, command, &sender).await;
        }
    }
}

async fn run_one(
    client: &DaemonClient,
    provider: ProviderId,
    command: Command,
    sender: &UnboundedSender<DaemonEvent>,
) {
    let failure = match &command {
        Command::LoadCover(key) => Failure::Cover(key.clone()),
        Command::OpenPlaylist(id) | Command::RefreshPlaylist(id) => Failure::Playlist(id.clone()),
        Command::Search(query) => Failure::Search(query.clone()),
        _ => Failure::Toast,
    };
    match execute(client, provider, command).await {
        Ok(events) => {
            for event in events {
                if sender.send(event).is_err() {
                    return;
                }
            }
        }
        Err(error) => {
            let event = match failure {
                Failure::Cover(key) => DaemonEvent::Cover { key, image: None },
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

async fn execute(
    client: &DaemonClient,
    provider: ProviderId,
    command: Command,
) -> Result<Vec<DaemonEvent>, DaemonError> {
    match command {
        Command::RefreshPlaylists => Ok(vec![DaemonEvent::Playlists(
            client.playlists(provider).await?,
        )]),
        Command::OpenPlaylist(id) | Command::RefreshPlaylist(id) => {
            Ok(vec![DaemonEvent::Playlist(
                client.playlist(provider, &id).await?,
            )])
        }
        Command::PlayPlaylist(id) => Ok(vec![DaemonEvent::Status(
            client.play_playlist(provider, &id).await?,
        )]),
        Command::SetProvider(_) | Command::RefreshProviders => Ok(Vec::new()),
        Command::SetOffline(enabled) => Ok(vec![DaemonEvent::Status(
            client.set_offline(enabled).await?,
        )]),
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
        Command::RetryDownload(key) => {
            client.retry_download(&key).await?;
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
                DaemonEvent::Playlists(client.playlists(provider).await?),
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
            Ok(vec![DaemonEvent::Playlists(
                client.playlists(provider).await?,
            )])
        }
        Command::DeletePlaylist(id) => {
            client.delete_playlist(&id).await?;
            Ok(vec![
                DaemonEvent::Playlists(client.playlists(provider).await?),
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
        Command::AddPlaylistTracks {
            id,
            provider,
            track_ids,
        } => {
            let tracks = client.add_playlist_tracks(&id, provider, track_ids).await?;
            Ok(vec![
                DaemonEvent::PlaylistTracks { id, tracks },
                DaemonEvent::Toast {
                    level: ToastLevel::Success,
                    message: "Tracks added to the playlist.".to_string(),
                },
            ])
        }
        Command::Search(query) => {
            let results = client.search(provider, &query).await?;
            Ok(vec![DaemonEvent::Search { query, results }])
        }
        Command::LoadCover(key) => {
            let bytes = client.cover(&key).await?;
            let image = tokio::task::spawn_blocking(move || cover::decode_image(&bytes))
                .await
                .ok()
                .flatten();
            Ok(vec![DaemonEvent::Cover { key, image }])
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
