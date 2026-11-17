use souvlaki::{MediaControlEvent, MediaMetadata, MediaPlayback, MediaPosition, SeekDirection};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};
use tokio::sync::mpsc::UnboundedSender;
use url::Url;
use ytermusicd_core::{CoreEvent, EventBus, PlaybackCommand, PlaybackSnapshot, PlaybackStatus};

use crate::artwork::ArtworkCache;

pub struct MprisHandle;

enum MprisUpdate {
    Snapshot(PlaybackSnapshot),
    Cover { video_id: String, path: PathBuf },
}

#[cfg(target_os = "linux")]
pub fn spawn(
    commands: UnboundedSender<PlaybackCommand>,
    events: EventBus,
    artwork: Arc<ArtworkCache>,
) -> Result<MprisHandle, String> {
    use souvlaki::{MediaControls, PlatformConfig};

    let (ready_tx, ready_rx) = mpsc::channel();
    let (update_tx, update_rx) = mpsc::channel();
    let thread_artwork = artwork.clone();

    thread::Builder::new()
        .name("ytermusicd-mpris".to_string())
        .spawn(move || {
            let mut controls = match MediaControls::new(PlatformConfig {
                dbus_name: "ytermusic",
                display_name: "ytermusicd",
                hwnd: None,
            }) {
                Ok(controls) => controls,
                Err(error) => {
                    let _ = ready_tx.send(Err(format!("{error:?}")));
                    return;
                }
            };

            if let Err(error) = controls.attach(move |event| {
                if let Some(command) = map_event(event) {
                    let _ = commands.send(command);
                }
            }) {
                let _ = ready_tx.send(Err(format!("{error:?}")));
                return;
            }
            let _ = ready_tx.send(Ok(()));

            let mut current = PlaybackSnapshot::default();
            while let Ok(update) = update_rx.recv() {
                match update {
                    MprisUpdate::Snapshot(snapshot) => {
                        current = snapshot;
                        let cover = current
                            .current
                            .as_ref()
                            .and_then(|track| thread_artwork.cached_path(&track.video_id));
                        update_controls(&mut controls, &current, cover.as_deref());
                    }
                    MprisUpdate::Cover { video_id, path } => {
                        if current
                            .current
                            .as_ref()
                            .is_some_and(|track| track.video_id == video_id)
                        {
                            update_controls(&mut controls, &current, Some(&path));
                        }
                    }
                }
            }
        })
        .map_err(|error| error.to_string())?;

    ready_rx.recv().map_err(|error| error.to_string())??;

    let (initial, mut receiver) = events.subscribe();
    let initial_id = initial
        .playback
        .current
        .as_ref()
        .map(|track| track.video_id.clone());
    let current_id = Arc::new(Mutex::new(initial_id.clone()));
    let _ = update_tx.send(MprisUpdate::Snapshot(initial.playback));
    if let Some(video_id) = initial_id {
        prefetch_cover(
            artwork.clone(),
            update_tx.clone(),
            current_id.clone(),
            video_id,
        );
    }
    tokio::spawn(async move {
        while let Ok(envelope) = receiver.recv().await {
            if let CoreEvent::Playback(snapshot) = envelope.event {
                let video_id = snapshot
                    .current
                    .as_ref()
                    .map(|track| track.video_id.clone());
                *current_id.lock().unwrap() = video_id.clone();
                let _ = update_tx.send(MprisUpdate::Snapshot(snapshot));
                if let Some(video_id) = video_id {
                    prefetch_cover(
                        artwork.clone(),
                        update_tx.clone(),
                        current_id.clone(),
                        video_id,
                    );
                }
            }
        }
    });

    Ok(MprisHandle)
}

#[cfg(not(target_os = "linux"))]
pub fn spawn(
    _commands: UnboundedSender<PlaybackCommand>,
    _events: EventBus,
    _artwork: Arc<ArtworkCache>,
) -> Result<MprisHandle, String> {
    Ok(MprisHandle)
}

fn prefetch_cover(
    artwork: Arc<ArtworkCache>,
    updates: mpsc::Sender<MprisUpdate>,
    current_id: Arc<Mutex<Option<String>>>,
    video_id: String,
) {
    tokio::spawn(async move {
        if artwork.cached_path(&video_id).is_some() {
            return;
        }
        let Some(path) = artwork.ensure(&video_id).await else {
            return;
        };
        if is_current_track(&current_id, &video_id) {
            let _ = updates.send(MprisUpdate::Cover { video_id, path });
        }
    });
}

fn is_current_track(current_id: &Mutex<Option<String>>, video_id: &str) -> bool {
    current_id.lock().unwrap().as_deref() == Some(video_id)
}

fn update_controls(
    controls: &mut souvlaki::MediaControls,
    snapshot: &PlaybackSnapshot,
    cover: Option<&Path>,
) {
    let cover_url = cover.and_then(cover_uri);
    let metadata = media_metadata(snapshot, cover_url.as_deref());
    if let Err(error) = controls.set_metadata(metadata) {
        log::warn!("MPRIS metadata update failed: {error:?}");
    }
    let playback = media_playback(snapshot);
    if let Err(error) = controls.set_playback(playback) {
        log::warn!("MPRIS playback update failed: {error:?}");
    }
    if let Err(error) = controls.set_volume(volume_fraction(snapshot.volume)) {
        log::warn!("MPRIS volume update failed: {error:?}");
    }
}

/// Build a `file://` URL for MPRIS cover art.
pub fn cover_uri(path: &Path) -> Option<String> {
    Url::from_file_path(path).ok().map(|url| url.to_string())
}

pub fn media_metadata<'a>(
    snapshot: &'a PlaybackSnapshot,
    cover_url: Option<&'a str>,
) -> MediaMetadata<'a> {
    let current = snapshot.current.as_ref();
    let duration = current
        .and_then(|track| parse_duration(&track.duration))
        .map(Duration::from_secs);
    MediaMetadata {
        title: current.map(|track| track.title.as_str()),
        album: current.map(|track| track.album.as_str()),
        artist: current.map(|track| track.author.as_str()),
        cover_url,
        duration,
    }
}

pub fn media_playback(snapshot: &PlaybackSnapshot) -> MediaPlayback {
    let progress = Some(MediaPosition(Duration::from_secs_f64(
        snapshot.position_secs.max(0.0),
    )));
    match snapshot.status {
        PlaybackStatus::Stopped => MediaPlayback::Stopped,
        PlaybackStatus::Playing => MediaPlayback::Playing { progress },
        PlaybackStatus::Paused => MediaPlayback::Paused { progress },
    }
}

/// Convert a 0-100 volume value to the MPRIS 0.0-1.0 range.
fn volume_fraction(volume: u8) -> f64 {
    volume as f64 / 100.0
}

pub fn map_event(event: MediaControlEvent) -> Option<PlaybackCommand> {
    match event {
        MediaControlEvent::Toggle => Some(PlaybackCommand::Toggle),
        MediaControlEvent::Play => Some(PlaybackCommand::Resume),
        MediaControlEvent::Pause => Some(PlaybackCommand::Pause),
        MediaControlEvent::Next => Some(PlaybackCommand::Next),
        MediaControlEvent::Previous => Some(PlaybackCommand::Previous),
        MediaControlEvent::Stop => Some(PlaybackCommand::Stop),
        MediaControlEvent::Seek(SeekDirection::Forward) => Some(PlaybackCommand::SeekBy(5.0)),
        MediaControlEvent::Seek(SeekDirection::Backward) => Some(PlaybackCommand::SeekBy(-5.0)),
        MediaControlEvent::SeekBy(SeekDirection::Forward, amount) => {
            Some(PlaybackCommand::SeekBy(amount.as_secs_f64()))
        }
        MediaControlEvent::SeekBy(SeekDirection::Backward, amount) => {
            Some(PlaybackCommand::SeekBy(-amount.as_secs_f64()))
        }
        MediaControlEvent::SetPosition(position) => Some(PlaybackCommand::Seek(position.0)),
        MediaControlEvent::SetVolume(volume) => Some(PlaybackCommand::SetVolume(
            (volume * 100.0).round().clamp(0.0, 100.0) as u8,
        )),
        MediaControlEvent::OpenUri(uri) => {
            log::info!("MPRIS OpenUri ignored: {uri}");
            None
        }
        MediaControlEvent::Raise => None,
        MediaControlEvent::Quit => Some(PlaybackCommand::Stop),
    }
}

fn parse_duration(value: &str) -> Option<u64> {
    let parts: Vec<_> = value.split(':').collect();
    let seconds = match parts.as_slice() {
        [minutes, seconds] => minutes.parse::<u64>().ok()? * 60 + seconds.parse::<u64>().ok()?,
        [hours, minutes, seconds] => {
            hours.parse::<u64>().ok()? * 3600
                + minutes.parse::<u64>().ok()? * 60
                + seconds.parse::<u64>().ok()?
        }
        _ => return None,
    };
    Some(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ytermusicd_core::{RepeatMode, Track};

    fn snapshot(status: PlaybackStatus) -> PlaybackSnapshot {
        PlaybackSnapshot {
            status,
            current_index: Some(0),
            current: Some(Track {
                video_id: "v1".to_string(),
                title: "Song".to_string(),
                author: "Artist".to_string(),
                album: "Album".to_string(),
                duration: "3:05".to_string(),
            }),
            position_secs: 12.0,
            volume: 50,
            repeat: RepeatMode::Off,
            shuffle: false,
        }
    }

    #[test]
    fn metadata_comes_from_the_snapshot() {
        let snapshot = snapshot(PlaybackStatus::Playing);
        let metadata = media_metadata(&snapshot, None);
        assert_eq!(metadata.title, Some("Song"));
        assert_eq!(metadata.artist, Some("Artist"));
        assert_eq!(metadata.album, Some("Album"));
        assert_eq!(metadata.duration, Some(Duration::from_secs(185)));
        assert_eq!(metadata.cover_url, None);
    }

    #[test]
    fn cover_urls_use_the_file_scheme() {
        assert_eq!(
            cover_uri(Path::new("/tmp/covers/v1.jpg")).as_deref(),
            Some("file:///tmp/covers/v1.jpg")
        );
        assert_eq!(
            cover_uri(Path::new("/tmp/a b.jpg")).as_deref(),
            Some("file:///tmp/a%20b.jpg")
        );
        assert_eq!(cover_uri(Path::new("relative.jpg")), None);

        let snapshot = snapshot(PlaybackStatus::Playing);
        let metadata = media_metadata(&snapshot, Some("file:///tmp/covers/v1.jpg"));
        assert_eq!(metadata.cover_url, Some("file:///tmp/covers/v1.jpg"));
    }

    #[test]
    fn stale_cover_updates_are_ignored() {
        let current = Mutex::new(Some("abcdefghijk".to_string()));
        assert!(is_current_track(&current, "abcdefghijk"));
        assert!(!is_current_track(&current, "zzzzzzzzzzz"));

        *current.lock().unwrap() = None;
        assert!(!is_current_track(&current, "abcdefghijk"));
    }

    #[test]
    fn playback_state_maps() {
        assert!(matches!(
            media_playback(&snapshot(PlaybackStatus::Stopped)),
            MediaPlayback::Stopped
        ));
        match media_playback(&snapshot(PlaybackStatus::Paused)) {
            MediaPlayback::Paused {
                progress: Some(MediaPosition(position)),
            } => assert_eq!(position, Duration::from_secs(12)),
            _ => panic!("expected paused progress"),
        }
    }

    #[test]
    fn volume_conversion_maps_percent_to_fraction() {
        assert_eq!(volume_fraction(0), 0.0);
        assert_eq!(volume_fraction(20), 0.2);
        assert_eq!(volume_fraction(50), 0.5);
        assert_eq!(volume_fraction(100), 1.0);
    }

    #[test]
    fn media_events_map_to_commands() {
        assert!(matches!(
            map_event(MediaControlEvent::Play),
            Some(PlaybackCommand::Resume)
        ));
        assert!(matches!(
            map_event(MediaControlEvent::Next),
            Some(PlaybackCommand::Next)
        ));
        assert!(matches!(
            map_event(MediaControlEvent::SetVolume(0.42)),
            Some(PlaybackCommand::SetVolume(42))
        ));
        assert!(matches!(
            map_event(MediaControlEvent::SeekBy(
                SeekDirection::Backward,
                Duration::from_secs(10)
            )),
            Some(PlaybackCommand::SeekBy(-10.0))
        ));
        assert!(map_event(MediaControlEvent::Raise).is_none());
    }

    #[test]
    fn duration_parser_handles_formats() {
        assert_eq!(parse_duration("3:05"), Some(185));
        assert_eq!(parse_duration("1:02:03"), Some(3723));
        assert_eq!(parse_duration("bad"), None);
    }
}
