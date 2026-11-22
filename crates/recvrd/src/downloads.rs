use common_structs::TrackKey;
use database::YTLocalDatabase;
use download_manager::{
    DownloadManager, DownloadManagerMessage, DownloadSummary as ManagerSummary, MessageHandler,
    TrackDownloader,
};
use recvrd_core::{AudioSink, PlaybackService, Track};
use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::sync::Mutex as AsyncMutex;

/// Download operations needed by the HTTP layer. A trait keeps API tests
/// independent of the download backend, the filesystem and the real download manager.
pub trait DownloadControl: Send + Sync {
    fn queue_playlist(&self, tracks: &[Track]);
    fn queue_tracks(&self, tracks: &[Track]);
    fn retry(&self, key: &TrackKey) -> bool;
    fn summary(&self) -> ManagerSummary;
    /// Stop every active download and clear the queue. Workers restart
    /// idle, so later downloads still run. A no-op before `spawn`.
    fn stop_all_downloads(&self);
    /// Forget every recorded download status.
    fn reset_statuses(&self);
}

struct Inner {
    manager: &'static DownloadManager,
    tracks: Mutex<HashMap<TrackKey, Track>>,
    /// The status handler installed by `spawn`. It is needed to
    /// restart the workers after `stop_all_downloads`.
    handler: Mutex<Option<MessageHandler>>,
}

/// Owns the daemon's download manager. It remembers track metadata so a failed
/// download can be retried without asking the UI to resend the whole track.
#[derive(Clone)]
pub struct DownloadService {
    inner: Arc<Inner>,
}

impl DownloadService {
    pub fn new(
        cache_dir: PathBuf,
        parallel_downloads: u16,
        downloader: Arc<dyn TrackDownloader>,
    ) -> Self {
        let database: &'static YTLocalDatabase =
            Box::leak(Box::new(YTLocalDatabase::new(cache_dir.clone())));
        let manager: &'static DownloadManager = Box::leak(Box::new(DownloadManager::new(
            cache_dir,
            database,
            parallel_downloads,
            downloader,
        )));
        Self {
            inner: Arc::new(Inner {
                manager,
                tracks: Mutex::new(HashMap::new()),
                handler: Mutex::new(None),
            }),
        }
    }

    /// Spawn the download workers and forward status updates into playback.
    /// The future must be created inside a Tokio runtime.
    pub fn spawn<S>(&self, playback: Arc<AsyncMutex<PlaybackService<S>>>)
    where
        S: AudioSink + Send + 'static,
    {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let handler: MessageHandler = Arc::new(move |message| match message {
            DownloadManagerMessage::VideoStatusUpdate(key, status) => {
                let _ = sender.send((key, status));
            }
        });
        *self.inner.handler.lock().unwrap() = Some(handler.clone());
        self.inner.manager.spawn_system(Never, handler);

        tokio::spawn(async move {
            while let Some((key, status)) = receiver.recv().await {
                let mut playback = playback.lock().await;
                if status == common_structs::MusicDownloadStatus::DownloadFailed {
                    playback.on_download_failed(&key);
                } else {
                    playback.set_download_status(&key, status);
                }
            }
        });
    }

    fn remember(&self, tracks: &[Track]) -> Vec<Track> {
        let mut known = self.inner.tracks.lock().unwrap();
        tracks
            .iter()
            .map(|track| {
                known.insert(track.key(), track.clone());
                track.clone()
            })
            .collect()
    }
}

impl DownloadControl for DownloadService {
    fn queue_playlist(&self, tracks: &[Track]) {
        let references = self.remember(tracks);
        self.inner.manager.set_download_list(references);
    }

    fn queue_tracks(&self, tracks: &[Track]) {
        let references = self.remember(tracks);
        self.inner.manager.add_to_download_list(references);
    }

    fn retry(&self, key: &TrackKey) -> bool {
        let track = self.inner.tracks.lock().unwrap().get(key).cloned();
        match track {
            Some(track) => {
                self.inner.manager.add_to_download_list([track]);
                true
            }
            None => false,
        }
    }

    fn summary(&self) -> ManagerSummary {
        self.inner.manager.summary()
    }

    fn stop_all_downloads(&self) {
        if let Some(handler) = self.inner.handler.lock().unwrap().clone() {
            self.inner.manager.clean(Never, handler);
        }
    }

    fn reset_statuses(&self) {
        self.inner.manager.reset_statuses();
    }
}

/// The download workers live as long as the daemon process; this future never
/// completes and is cloned for each worker by `spawn_system`.
#[derive(Clone, Copy)]
struct Never;

impl Future for Never {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}
