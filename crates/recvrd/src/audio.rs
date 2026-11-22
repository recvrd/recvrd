use player::{PlayError, Player, PlayerOptions};
use recvrd_core::{AudioSink, PlaybackSource, Track};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

enum AudioCommand {
    Play {
        source: PlaybackSource,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    Pause,
    Resume,
    Stop,
    Seek {
        position: Duration,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    Volume(u8),
}

/// A rodio sink that lives on a dedicated thread, so the non-Send audio stream
/// never enters the async runtime.
pub struct RodioSink {
    tx: mpsc::Sender<AudioCommand>,
    finished: Arc<AtomicBool>,
    duration: Arc<Mutex<Option<Duration>>>,
}

/// Drain queued player errors from the channel.
///
/// The player reports async seek errors through this channel. The audio
/// thread calls this function on each loop turn, so a queued error reaches
/// the log instead of sitting unread.
fn drain_player_errors(receiver: &flume::Receiver<PlayError>) -> Vec<PlayError> {
    receiver.try_iter().collect()
}

impl RodioSink {
    pub fn spawn(initial_volume: u8) -> Self {
        let (tx, rx) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(true));
        let thread_finished = finished.clone();
        let duration = Arc::new(Mutex::new(None));
        let thread_duration = duration.clone();

        thread::Builder::new()
            .name("recvrd-audio".to_string())
            .spawn(move || {
                let (error_tx, error_rx) = flume::unbounded();
                let mut volume = initial_volume.min(100);
                let mut player = match Player::new(error_tx.clone(), PlayerOptions::new(volume)) {
                    Ok(player) => Some(player),
                    Err(error) => {
                        log::error!("audio output unavailable: {error:?}; retrying on play");
                        None
                    }
                };
                let mut playing = false;

                loop {
                    for error in drain_player_errors(&error_rx) {
                        log::warn!("async player error: {error:?}");
                    }
                    match rx.recv_timeout(Duration::from_millis(250)) {
                        Ok(AudioCommand::Play { source, reply }) => {
                            if player.is_none() {
                                match Player::new(error_tx.clone(), PlayerOptions::new(volume)) {
                                    Ok(new_player) => {
                                        log::info!("audio output available");
                                        player = Some(new_player);
                                    }
                                    Err(error) => {
                                        log::error!("audio output unavailable: {error:?}");
                                        thread_finished.store(true, Ordering::SeqCst);
                                        playing = false;
                                        let _ = reply.send(Err(format!("{error:?}")));
                                        continue;
                                    }
                                }
                            }

                            let Some(player) = player.as_mut() else {
                                let _ = reply.send(Err("audio output unavailable".to_string()));
                                continue;
                            };
                            let result = match source {
                                PlaybackSource::File(path) => player.play(&path),
                                PlaybackSource::Stream(reader) => player.play_reader(
                                    reader.reader,
                                    reader.length,
                                    reader.mime.as_deref(),
                                    reader.seekable,
                                ),
                            };
                            match result {
                                Ok(()) => {
                                    *thread_duration.lock().unwrap() =
                                        player.duration().map(Duration::from_secs_f64);
                                    thread_finished.store(false, Ordering::SeqCst);
                                    playing = true;
                                    let _ = reply.send(Ok(()));
                                }
                                Err(error) => {
                                    log::error!("failed to start playback: {error:?}");
                                    *thread_duration.lock().unwrap() = None;
                                    thread_finished.store(true, Ordering::SeqCst);
                                    playing = false;
                                    let _ = reply.send(Err(format!("{error:?}")));
                                }
                            }
                        }
                        Ok(AudioCommand::Pause) => {
                            if let Some(player) = player.as_mut() {
                                player.pause();
                            }
                        }
                        Ok(AudioCommand::Resume) => {
                            if let Some(player) = player.as_mut() {
                                player.resume();
                            }
                        }
                        Ok(AudioCommand::Stop) => {
                            if let Some(player) = player.as_mut() {
                                player.stop();
                            }
                            *thread_duration.lock().unwrap() = None;
                            thread_finished.store(true, Ordering::SeqCst);
                            playing = false;
                        }
                        Ok(AudioCommand::Seek { position, reply }) => {
                            let Some(player) = player.as_mut() else {
                                let _ = reply.send(Err("audio output unavailable".to_string()));
                                continue;
                            };
                            match player.try_seek_to(position) {
                                Ok(()) => {
                                    let _ = reply.send(Ok(()));
                                }
                                Err(error) => {
                                    log::warn!("seek failed: {error:?}");
                                    let _ = reply.send(Err(format!("{error:?}")));
                                }
                            }
                        }
                        Ok(AudioCommand::Volume(new_volume)) => {
                            volume = new_volume;
                            if let Some(player) = player.as_mut() {
                                player.set_volume(volume as i32);
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if playing && player.as_ref().is_some_and(Player::is_finished) {
                                thread_finished.store(true, Ordering::SeqCst);
                                playing = false;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .ok();

        Self {
            tx,
            finished,
            duration,
        }
    }

    fn send(&self, command: AudioCommand) -> Result<(), std::io::Error> {
        self.tx
            .send(command)
            .map_err(|_| std::io::Error::other("audio thread is not running"))
    }
}

impl AudioSink for RodioSink {
    type Error = std::io::Error;

    fn play(&mut self, _track: &Track, source: PlaybackSource) -> Result<(), Self::Error> {
        let (reply, response) = mpsc::sync_channel(1);
        self.send(AudioCommand::Play { source, reply })?;
        match response.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(std::io::Error::other(message)),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "audio start timed out",
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(std::io::Error::other("audio thread stopped"))
            }
        }
    }

    fn pause(&mut self) -> Result<(), Self::Error> {
        self.send(AudioCommand::Pause)
    }

    fn resume(&mut self) -> Result<(), Self::Error> {
        self.send(AudioCommand::Resume)
    }

    fn stop(&mut self) -> Result<(), Self::Error> {
        self.send(AudioCommand::Stop)
    }

    fn seek(&mut self, position: Duration) -> Result<(), Self::Error> {
        let (reply, response) = mpsc::sync_channel(1);
        self.send(AudioCommand::Seek { position, reply })?;
        match response.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(std::io::Error::other(message)),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "audio seek timed out",
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(std::io::Error::other("audio thread stopped"))
            }
        }
    }

    fn set_volume(&mut self, volume: u8) -> Result<(), Self::Error> {
        self.send(AudioCommand::Volume(volume))
    }

    fn duration(&self) -> Option<Duration> {
        *self.duration.lock().unwrap()
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_player_errors_returns_queued_errors() {
        let (sender, receiver) = flume::unbounded();
        sender
            .send(PlayError::Io(std::io::Error::other("seek failed")))
            .unwrap();
        sender
            .send(PlayError::Io(std::io::Error::other("second error")))
            .unwrap();

        let drained = drain_player_errors(&receiver);
        assert_eq!(drained.len(), 2);

        assert!(drain_player_errors(&receiver).is_empty());
    }

    #[test]
    fn drain_player_errors_returns_empty_for_an_empty_channel() {
        let (_sender, receiver) = flume::unbounded::<PlayError>();

        assert!(drain_player_errors(&receiver).is_empty());
    }
}
