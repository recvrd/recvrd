use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use database::YTLocalDatabase;
use tokio::{select, sync::Notify, task::JoinHandle};
use ytermusicd_core::Track;

use common_structs::{MusicDownloadStatus, TrackKey};

/// Error from a track download backend.
#[derive(Debug)]
pub enum DownloadError {
    BackendFailed(String),
    IoError(std::io::Error),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::BackendFailed(msg) => write!(f, "download backend failed: {msg}"),
            DownloadError::IoError(error) => write!(f, "IO error: {error}"),
        }
    }
}

impl std::error::Error for DownloadError {}

impl From<std::io::Error> for DownloadError {
    fn from(error: std::io::Error) -> Self {
        DownloadError::IoError(error)
    }
}

/// Download one track to a file. Providers implement this trait;
/// the manager owns the queue, retries and status, not the backend.
#[async_trait]
#[allow(clippy::double_must_use)]
pub trait TrackDownloader: Send + Sync {
    async fn download(
        &self,
        track: &Track,
        output: &Path,
        progress: &(dyn Fn(usize) + Send + Sync),
    ) -> Result<(), DownloadError>;
}

pub type MessageHandler = Arc<dyn Fn(DownloadManagerMessage) + Send + Sync + 'static>;

/// Check that a previous download is complete. The metadata sidecar
/// and a non-empty audio file must both exist. A sidecar alone is not
/// enough, since the audio file can be deleted on its own.
fn is_complete_download(mp4: &Path, json: &Path) -> bool {
    json.exists()
        && mp4
            .metadata()
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq)]
pub enum DownloadManagerMessage {
    VideoStatusUpdate(TrackKey, MusicDownloadStatus),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DownloadSummary {
    pub queued: usize,
    pub active: usize,
    pub completed: usize,
    pub retrying: usize,
    pub failed: usize,
}

pub struct DownloadManager {
    database: &'static YTLocalDatabase,
    cache_dir: PathBuf,
    parallel_downloads: u16,
    downloader: Arc<dyn TrackDownloader>,
    max_retries: u32,
    retry_delay: Duration,
    handles: Mutex<Vec<JoinHandle<()>>>,
    download_list: Mutex<VecDeque<Track>>,
    in_download: Mutex<HashSet<TrackKey>>,
    statuses: Mutex<HashMap<TrackKey, MusicDownloadStatus>>,
    notify: Notify,
}

impl DownloadManager {
    pub fn new(
        cache_dir: PathBuf,
        database: &'static YTLocalDatabase,
        parallel_downloads: u16,
        downloader: Arc<dyn TrackDownloader>,
    ) -> Self {
        Self {
            database,
            cache_dir,
            parallel_downloads,
            downloader,
            max_retries: 2,
            retry_delay: Duration::from_millis(500),
            handles: Mutex::new(Vec::new()),
            download_list: Mutex::new(VecDeque::new()),
            in_download: Mutex::new(HashSet::new()),
            statuses: Mutex::new(HashMap::new()),
            notify: Notify::new(),
        }
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    pub fn status(&self, key: &TrackKey) -> Option<MusicDownloadStatus> {
        self.statuses.lock().unwrap().get(key).copied()
    }

    pub fn active_downloads(&self) -> usize {
        self.in_download.lock().unwrap().len()
    }

    pub fn summary(&self) -> DownloadSummary {
        let mut summary = DownloadSummary {
            queued: self.download_list.lock().unwrap().len(),
            active: self.in_download.lock().unwrap().len(),
            ..DownloadSummary::default()
        };
        for status in self.statuses.lock().unwrap().values() {
            match status {
                MusicDownloadStatus::Downloaded => summary.completed += 1,
                MusicDownloadStatus::Retrying(_) => summary.retrying += 1,
                MusicDownloadStatus::DownloadFailed => summary.failed += 1,
                _ => {}
            }
        }
        summary
    }

    pub fn remove_from_in_downloads(&self, key: &TrackKey) {
        self.in_download.lock().unwrap().remove(key);
    }

    fn take(&self) -> Option<Track> {
        self.download_list.lock().unwrap().pop_front()
    }

    /// This has to be called as a service stream
    /// HANDLES.lock().unwrap().push(run_service(async move {
    ///     run_service_stream(sender);
    /// }));
    pub fn run_service_stream(
        &'static self,
        cancelation: impl Future<Output = ()> + Clone + Send + 'static,
        sender: MessageHandler,
    ) {
        let fut = async move {
            loop {
                if let Some(id) = self.take() {
                    self.start_download(id, sender.clone()).await;
                } else {
                    self.notify.notified().await;
                }
            }
        };
        let service = tokio::task::spawn(async move {
            select! {
                _ = fut => {},
                _ = cancelation => {},
            }
        });
        self.handles.lock().unwrap().push(service);
    }

    pub fn spawn_system(
        &'static self,
        cancelation: impl Future<Output = ()> + Clone + Send + 'static,
        sender: MessageHandler,
    ) {
        for _ in 0..self.parallel_downloads {
            self.run_service_stream(cancelation.clone(), sender.clone());
        }
    }

    pub fn clean(
        &'static self,
        cancelation: impl Future<Output = ()> + Clone + Send + 'static,
        sender: MessageHandler,
    ) {
        self.download_list.lock().unwrap().clear();
        self.in_download.lock().unwrap().clear();
        {
            let mut handle = self.handles.lock().unwrap();
            for i in handle.iter() {
                i.abort()
            }
            handle.clear();
        }
        self.spawn_system(cancelation, sender);
    }

    pub fn set_download_list(&self, to_add: impl IntoIterator<Item = Track>) {
        let mut list = self.download_list.lock().unwrap();
        list.clear();
        list.extend(to_add);
        drop(list);
        // Wake every idle worker so `parallel_downloads` is actually used.
        self.notify.notify_waiters();
    }

    pub fn add_to_download_list(&self, to_add: impl IntoIterator<Item = Track>) {
        let mut list = self.download_list.lock().unwrap();
        list.extend(to_add);
        drop(list);
        self.notify.notify_waiters();
    }

    fn set_status(&self, key: &TrackKey, status: MusicDownloadStatus) {
        self.statuses.lock().unwrap().insert(key.clone(), status);
    }

    pub async fn start_download(&self, song: Track, s: MessageHandler) -> bool {
        let key = song.key();
        if !key.validate() {
            log::error!("refused download with invalid track ID {key}");
            self.set_status(&key, MusicDownloadStatus::DownloadFailed);
            s(DownloadManagerMessage::VideoStatusUpdate(
                key,
                MusicDownloadStatus::DownloadFailed,
            ));
            return false;
        }
        {
            let mut downloads = self.in_download.lock().unwrap();
            if downloads.contains(&key) {
                return false;
            }
            downloads.insert(key.clone());
        }

        let _guard = InDownloadGuard {
            manager: self,
            key: key.clone(),
        };
        self.start_download_inner(song, s).await
    }

    async fn start_download_inner(&self, song: Track, s: MessageHandler) -> bool {
        let key = song.key();
        self.set_status(&key, MusicDownloadStatus::Downloading(1));
        s(DownloadManagerMessage::VideoStatusUpdate(
            key.clone(),
            MusicDownloadStatus::Downloading(1),
        ));

        let downloads = self.cache_dir.join("downloads");
        let mp4 = key.download_file(&downloads, "mp4");
        let json = key.download_file(&downloads, "json");
        let part = key.download_file(&downloads, "part");
        if let Some(parent) = mp4.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            log::error!("could not create the download directory: {error}");
            self.set_status(&key, MusicDownloadStatus::DownloadFailed);
            s(DownloadManagerMessage::VideoStatusUpdate(
                key,
                MusicDownloadStatus::DownloadFailed,
            ));
            return false;
        }

        if is_complete_download(&mp4, &json) {
            self.set_status(&key, MusicDownloadStatus::Downloaded);
            s(DownloadManagerMessage::VideoStatusUpdate(
                key,
                MusicDownloadStatus::Downloaded,
            ));
            return true;
        }

        let _ = std::fs::remove_file(&mp4);
        let _ = std::fs::remove_file(&part);

        let mut attempt = 0;
        loop {
            let reporter = {
                let manager = self;
                let key = key.clone();
                let s = s.clone();
                move |progress: usize| {
                    let status = MusicDownloadStatus::Downloading(progress.min(100));
                    manager.set_status(&key, status);
                    s(DownloadManagerMessage::VideoStatusUpdate(
                        key.clone(),
                        status,
                    ));
                }
            };

            match self.handle_download(&song, &part, &reporter).await {
                Ok(()) => {
                    if let Err(error) = self.finalize(&song, &part, &mp4, &json) {
                        log::error!("couldn't finalize download for {key}: {error}");
                        let _ = std::fs::remove_file(&mp4);
                        let _ = std::fs::remove_file(&part);
                        self.set_status(&key, MusicDownloadStatus::DownloadFailed);
                        s(DownloadManagerMessage::VideoStatusUpdate(
                            key,
                            MusicDownloadStatus::DownloadFailed,
                        ));
                        return false;
                    }

                    self.set_status(&key, MusicDownloadStatus::Downloaded);
                    s(DownloadManagerMessage::VideoStatusUpdate(
                        key,
                        MusicDownloadStatus::Downloaded,
                    ));
                    return true;
                }
                Err(error) => {
                    let _ = std::fs::remove_file(&part);
                    if attempt < self.max_retries {
                        attempt += 1;
                        self.set_status(&key, MusicDownloadStatus::Retrying(attempt));
                        s(DownloadManagerMessage::VideoStatusUpdate(
                            key.clone(),
                            MusicDownloadStatus::Retrying(attempt),
                        ));
                        tokio::time::sleep(self.retry_delay).await;
                    } else {
                        self.set_status(&key, MusicDownloadStatus::DownloadFailed);
                        s(DownloadManagerMessage::VideoStatusUpdate(
                            key.clone(),
                            MusicDownloadStatus::DownloadFailed,
                        ));
                        log::error!("couldn't download {key}: {error}");
                        return false;
                    }
                }
            }
        }
    }

    async fn handle_download(
        &self,
        song: &Track,
        output: &Path,
        progress: &(dyn Fn(usize) + Send + Sync),
    ) -> Result<(), DownloadError> {
        self.downloader.download(song, output, progress).await
    }

    fn finalize(
        &self,
        song: &Track,
        part: &std::path::Path,
        mp4: &std::path::Path,
        json: &std::path::Path,
    ) -> Result<(), DownloadError> {
        std::fs::rename(part, mp4)?;
        let serialized = serde_json::to_string(song)
            .map_err(|error| DownloadError::IoError(std::io::Error::other(error)))?;
        std::fs::write(json, serialized)?;
        self.database
            .append(song.clone())
            .map_err(|error| DownloadError::IoError(std::io::Error::other(error)))?;
        Ok(())
    }

    pub fn start_task_unary(
        &'static self,
        s: MessageHandler,
        song: Track,
        cancelation: impl Future<Output = ()> + Send + 'static,
    ) {
        let fut = async move {
            self.start_download(song, s).await;
        };
        let service = tokio::task::spawn(async move {
            tokio::select! {
                _ = fut => {},
                _ = cancelation => {},
            }
        });
        self.handles.lock().unwrap().push(service);
    }
}

struct InDownloadGuard<'a> {
    manager: &'a DownloadManager,
    key: TrackKey,
}

impl Drop for InDownloadGuard<'_> {
    fn drop(&mut self) {
        self.manager.in_download.lock().unwrap().remove(&self.key);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use common_structs::ProviderId;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use tempfile::{TempDir, tempdir};
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, BufReader},
        process::Command,
        sync::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard},
    };

    static TEST_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

    async fn serial() -> AsyncMutexGuard<'static, ()> {
        TEST_LOCK.lock().await
    }

    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-downloader");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    fn song(id: &str) -> Track {
        Track {
            provider: ProviderId::Youtube,
            track_id: id.to_string(),
            title: format!("Track {id}"),
            author: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
        }
    }

    fn scoped(downloads: &Path) -> PathBuf {
        downloads.join("youtube")
    }

    /// A small fake downloader for manager tests. It runs a fake
    /// script and parses its `download:` progress lines. The real
    /// download invocation lives in the provider crate, so the manager
    /// tests cannot use it.
    struct FakeDownloader {
        script: PathBuf,
    }

    fn parse_progress(line: &str) -> Option<usize> {
        let value = line
            .trim()
            .strip_prefix("download:")?
            .trim()
            .trim_end_matches('%');
        value
            .parse::<f64>()
            .ok()
            .map(|percent| percent.clamp(0.0, 100.0) as usize)
    }

    #[async_trait]
    impl TrackDownloader for FakeDownloader {
        async fn download(
            &self,
            _track: &Track,
            output: &Path,
            progress: &(dyn Fn(usize) + Send + Sync),
        ) -> Result<(), DownloadError> {
            progress(0);
            let mut child = Command::new(&self.script)
                .arg("-o")
                .arg(output)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()?;
            let stdout = child.stdout.take().ok_or_else(|| {
                DownloadError::IoError(std::io::Error::other("fake stdout was not captured"))
            })?;
            let stderr = child.stderr.take().ok_or_else(|| {
                DownloadError::IoError(std::io::Error::other("fake stderr was not captured"))
            })?;
            let stderr_task = tokio::spawn(async move {
                let mut buffer = String::new();
                let _ = BufReader::new(stderr).read_to_string(&mut buffer).await;
                buffer
            });
            let mut lines = BufReader::new(stdout).lines();
            while let Some(line) = lines.next_line().await? {
                if let Some(percent) = parse_progress(&line) {
                    progress(percent);
                }
            }
            let status = child.wait().await?;
            let stderr = stderr_task.await.unwrap_or_default();
            if !status.success() {
                return Err(DownloadError::BackendFailed(stderr));
            }
            progress(100);
            Ok(())
        }
    }

    fn manager(dir: &TempDir, script: PathBuf, max_retries: u32) -> (DownloadManager, PathBuf) {
        let cache = dir.path().join("cache");
        let downloads = cache.join("downloads");
        fs::create_dir_all(&downloads).unwrap();
        let database: &'static YTLocalDatabase =
            Box::leak(Box::new(YTLocalDatabase::new(cache.clone())));
        let manager = DownloadManager::new(cache, database, 1, Arc::new(FakeDownloader { script }))
            .with_max_retries(max_retries)
            .with_retry_delay(Duration::ZERO);
        (manager, downloads)
    }

    fn collector() -> (MessageHandler, Arc<Mutex<Vec<DownloadManagerMessage>>>) {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let sink = messages.clone();
        let handler: MessageHandler = Arc::new(move |message| {
            sink.lock().unwrap().push(message);
        });
        (handler, messages)
    }

    fn success_script(progress: bool) -> String {
        let progress = if progress {
            "echo 'download:  0.0%'\necho 'download: 50.0%'"
        } else {
            ""
        };
        format!(
            "out=\"\"\nwhile [ $# -gt 0 ]; do case \"$1\" in -o) out=\"$2\"; shift 2;; *) shift;; esac; done\n{progress}\necho fake-audio > \"$out\"\necho 'download:100.0%'"
        )
    }

    #[tokio::test]
    async fn successful_download_is_atomic_and_reports_progress() {
        let _guard = serial().await;
        let dir = tempdir().unwrap();
        let path = script(dir.path(), &success_script(true));
        let (manager, downloads) = manager(&dir, path, 0);
        let (handler, messages) = collector();

        assert!(manager.start_download(song("dQw4w9WgXcQ"), handler).await);
        assert!(scoped(&downloads).join("dQw4w9WgXcQ.mp4").exists());
        assert!(scoped(&downloads).join("dQw4w9WgXcQ.json").exists());
        assert!(!scoped(&downloads).join("dQw4w9WgXcQ.part").exists());
        assert_eq!(
            manager.status(&TrackKey::youtube("dQw4w9WgXcQ")),
            Some(MusicDownloadStatus::Downloaded)
        );
        assert_eq!(manager.active_downloads(), 0);
        assert_eq!(manager.summary().completed, 1);

        let messages = messages.lock().unwrap();
        assert!(messages.iter().any(|message| matches!(
            message,
            DownloadManagerMessage::VideoStatusUpdate(_, MusicDownloadStatus::Downloading(50))
        )));
        assert!(messages.iter().any(|message| matches!(
            message,
            DownloadManagerMessage::VideoStatusUpdate(_, MusicDownloadStatus::Downloaded)
        )));
    }

    #[tokio::test]
    async fn failed_download_cleans_up_and_records_failure() {
        let _guard = serial().await;
        let dir = tempdir().unwrap();
        let path = script(dir.path(), "echo boom >&2\nexit 1");
        let (manager, downloads) = manager(&dir, path, 0);
        let (handler, _messages) = collector();

        assert!(!manager.start_download(song("dQw4w9WgXcQ"), handler).await);
        assert!(!scoped(&downloads).join("dQw4w9WgXcQ.mp4").exists());
        assert!(!scoped(&downloads).join("dQw4w9WgXcQ.json").exists());
        assert!(!scoped(&downloads).join("dQw4w9WgXcQ.part").exists());
        assert_eq!(
            manager.status(&TrackKey::youtube("dQw4w9WgXcQ")),
            Some(MusicDownloadStatus::DownloadFailed)
        );
        assert_eq!(manager.active_downloads(), 0);
        assert_eq!(manager.summary().failed, 1);
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let _guard = serial().await;
        let dir = tempdir().unwrap();
        let lock = dir.path().join("first-attempt-lock");
        let body = format!(
            "out=\"\"\nwhile [ $# -gt 0 ]; do case \"$1\" in -o) out=\"$2\"; shift 2;; *) shift;; esac; done\nif mkdir {lock} 2>/dev/null; then echo 'temporary failure' >&2; exit 1; fi\necho fake-audio > \"$out\"\necho 'download:100.0%'",
            lock = lock.display()
        );
        let path = script(dir.path(), &body);
        let (manager, downloads) = manager(&dir, path, 1);
        let (handler, messages) = collector();

        assert!(manager.start_download(song("dQw4w9WgXcQ"), handler).await);
        assert!(scoped(&downloads).join("dQw4w9WgXcQ.mp4").exists());
        assert_eq!(
            manager.status(&TrackKey::youtube("dQw4w9WgXcQ")),
            Some(MusicDownloadStatus::Downloaded)
        );
        assert_eq!(manager.active_downloads(), 0);
        let messages = messages.lock().unwrap();
        assert!(messages.iter().any(|message| matches!(
            message,
            DownloadManagerMessage::VideoStatusUpdate(_, MusicDownloadStatus::Retrying(1))
        )));
    }

    #[tokio::test]
    async fn already_downloaded_is_a_noop() {
        let _guard = serial().await;
        let dir = tempdir().unwrap();
        let marker = dir.path().join("invoked");
        let body = format!("touch {}", marker.display());
        let path = script(dir.path(), &body);
        let (manager, downloads) = manager(&dir, path, 0);
        fs::create_dir_all(scoped(&downloads)).unwrap();
        fs::write(scoped(&downloads).join("dQw4w9WgXcQ.json"), "{}").unwrap();
        fs::write(scoped(&downloads).join("dQw4w9WgXcQ.mp4"), "fake-audio").unwrap();
        let (handler, _messages) = collector();

        assert!(manager.start_download(song("dQw4w9WgXcQ"), handler).await);
        assert!(!marker.exists());
        assert_eq!(
            manager.status(&TrackKey::youtube("dQw4w9WgXcQ")),
            Some(MusicDownloadStatus::Downloaded)
        );
        assert_eq!(manager.active_downloads(), 0);
    }

    #[tokio::test]
    async fn downloads_land_in_the_provider_directory() {
        let _guard = serial().await;
        let dir = tempdir().unwrap();
        let path = script(dir.path(), &success_script(false));
        let (manager, downloads) = manager(&dir, path, 0);
        let (handler, _messages) = collector();

        assert!(manager.start_download(song("dQw4w9WgXcQ"), handler).await);
        // The provider subdirectory is created by the manager, and
        // no file lands in the flat downloads directory.
        assert!(scoped(&downloads).join("dQw4w9WgXcQ.mp4").exists());
        assert!(scoped(&downloads).join("dQw4w9WgXcQ.json").exists());
        assert!(!downloads.join("dQw4w9WgXcQ.mp4").exists());
        assert_eq!(
            manager.status(&TrackKey::youtube("dQw4w9WgXcQ")),
            Some(MusicDownloadStatus::Downloaded)
        );
    }

    #[tokio::test]
    async fn invalid_video_id_is_rejected_without_starting_a_download() {
        let _guard = serial().await;
        let dir = tempdir().unwrap();
        let marker = dir.path().join("invoked");
        let body = format!("touch {}", marker.display());
        let path = script(dir.path(), &body);
        let (manager, _downloads) = manager(&dir, path, 0);
        let (handler, _messages) = collector();

        assert!(!manager.start_download(song("../../evil"), handler).await);
        assert!(!marker.exists());
        assert!(!dir.path().join("evil.mp4").exists());
        assert!(!dir.path().join("evil.json").exists());
        assert_eq!(
            manager.status(&TrackKey::youtube("../../evil")),
            Some(MusicDownloadStatus::DownloadFailed)
        );
        assert_eq!(manager.active_downloads(), 0);
    }

    #[tokio::test]
    async fn missing_audio_is_redownloaded_despite_sidecar() {
        let _guard = serial().await;
        let dir = tempdir().unwrap();
        let path = script(dir.path(), &success_script(false));
        let (manager, downloads) = manager(&dir, path, 0);
        fs::create_dir_all(scoped(&downloads)).unwrap();
        fs::write(scoped(&downloads).join("dQw4w9WgXcQ.json"), "{}").unwrap();
        let (handler, _messages) = collector();

        assert!(manager.start_download(song("dQw4w9WgXcQ"), handler).await);
        assert!(scoped(&downloads).join("dQw4w9WgXcQ.mp4").exists());
        assert!(
            scoped(&downloads)
                .join("dQw4w9WgXcQ.mp4")
                .metadata()
                .unwrap()
                .len()
                > 0
        );
        assert_eq!(
            manager.status(&TrackKey::youtube("dQw4w9WgXcQ")),
            Some(MusicDownloadStatus::Downloaded)
        );
        assert_eq!(manager.active_downloads(), 0);
    }

    #[tokio::test]
    async fn empty_audio_is_redownloaded_despite_sidecar() {
        let _guard = serial().await;
        let dir = tempdir().unwrap();
        let path = script(dir.path(), &success_script(false));
        let (manager, downloads) = manager(&dir, path, 0);
        fs::create_dir_all(scoped(&downloads)).unwrap();
        fs::write(scoped(&downloads).join("dQw4w9WgXcQ.json"), "{}").unwrap();
        fs::write(scoped(&downloads).join("dQw4w9WgXcQ.mp4"), "").unwrap();
        let (handler, _messages) = collector();

        assert!(manager.start_download(song("dQw4w9WgXcQ"), handler).await);
        assert!(
            scoped(&downloads)
                .join("dQw4w9WgXcQ.mp4")
                .metadata()
                .unwrap()
                .len()
                > 0
        );
        assert_eq!(
            manager.status(&TrackKey::youtube("dQw4w9WgXcQ")),
            Some(MusicDownloadStatus::Downloaded)
        );
        assert_eq!(manager.active_downloads(), 0);
    }

    #[tokio::test]
    async fn cancellation_clears_active_download_state() {
        let _guard = serial().await;
        let dir = tempdir().unwrap();
        let path = script(dir.path(), "sleep 2");
        let (manager, _downloads) = manager(&dir, path, 0);
        let (handler, _messages) = collector();

        let mut future = Box::pin(manager.start_download(song("dQw4w9WgXcQ"), handler));
        tokio::select! {
            _ = &mut future => panic!("download unexpectedly completed"),
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
        drop(future);

        assert_eq!(manager.active_downloads(), 0);
    }

    /// A dead child holds no file handles and writes nothing, so a zombie
    /// counts as stopped.
    fn process_running(pid: u32) -> bool {
        let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        let Some(after_comm) = stat.rsplit_once(") ") else {
            return false;
        };
        !after_comm.1.starts_with('Z')
    }

    #[tokio::test]
    async fn cancellation_kills_the_child_process() {
        let _guard = serial().await;
        let dir = tempdir().unwrap();
        let pid_file = dir.path().join("fake-downloader.pid");
        // Record the child pid, then write to the part file in a loop so
        // a surviving child is visible as a growing file. The loop is
        // bounded so a failing run cannot leak a writer forever.
        let body = format!(
            "echo $$ > {pid}\nout=\"\"\nwhile [ $# -gt 0 ]; do case \"$1\" in -o) out=\"$2\"; shift 2;; *) shift;; esac; done\ni=0\nwhile [ $i -lt 600 ]; do echo \"chunk $i\" >> \"$out\"; i=$((i+1)); sleep 0.1; done",
            pid = pid_file.display()
        );
        let path = script(dir.path(), &body);
        let (manager, downloads) = manager(&dir, path, 0);
        let (handler, _messages) = collector();

        let mut future = Box::pin(manager.start_download(song("dQw4w9WgXcQ"), handler));
        let part = scoped(&downloads).join("dQw4w9WgXcQ.part");
        // Poll the download until the child starts. The future must run
        // for the child to spawn.
        let mut waited = 0;
        while !part.exists() && waited < 100 {
            tokio::select! {
                _ = &mut future => panic!("download unexpectedly completed"),
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
            waited += 1;
        }
        assert!(part.exists(), "fake downloader never started");
        let pid: u32 = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        drop(future);

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while process_running(pid) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "downloader child {pid} still runs after cancel"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        let size = fs::metadata(&part).unwrap().len();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(fs::metadata(&part).unwrap().len(), size);
    }
}
