//! Cover art cache for the HTTP API and MPRIS.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use common_structs::TrackKey;
use recvrd_core::CoverFetcher;

/// Store cover art on disk and limit the cache size.
#[derive(Clone)]
pub struct ArtworkCache {
    dir: PathBuf,
    max_bytes: u64,
    fetcher: Arc<dyn CoverFetcher>,
    gates: Arc<Mutex<HashMap<TrackKey, Arc<tokio::sync::Mutex<()>>>>>,
    offline: Arc<AtomicBool>,
}

impl ArtworkCache {
    #[cfg(test)]
    pub fn with_fetcher(dir: PathBuf, max_bytes: u64, fetcher: Arc<dyn CoverFetcher>) -> Self {
        Self::with_offline(dir, max_bytes, fetcher, Arc::new(AtomicBool::new(false)))
    }

    /// Build a cache that shares the daemon's offline flag. While
    /// offline, `ensure` never fetches and returns a cached file only.
    pub fn with_offline(
        dir: PathBuf,
        max_bytes: u64,
        fetcher: Arc<dyn CoverFetcher>,
        offline: Arc<AtomicBool>,
    ) -> Self {
        Self {
            dir,
            max_bytes,
            fetcher,
            gates: Arc::new(Mutex::new(HashMap::new())),
            offline,
        }
    }

    /// Return the cache file for a valid track key:
    /// `covers/<provider>/<id>.jpg`.
    pub fn path(&self, key: &TrackKey) -> Option<PathBuf> {
        key.validate().then(|| key.cover_file(&self.dir))
    }

    /// Return the cached file when it exists.
    pub fn cached_path(&self, key: &TrackKey) -> Option<PathBuf> {
        self.path(key).filter(|path| path.is_file())
    }

    /// Set the offline flag shared with the daemon.
    pub fn set_offline(&self, enabled: bool) {
        self.offline.store(enabled, Ordering::SeqCst);
    }

    /// Return the cached file and fetch it when it is missing.
    pub async fn ensure(&self, key: &TrackKey) -> Option<PathBuf> {
        if let Some(path) = self.cached_path(key) {
            return Some(path);
        }
        if self.offline.load(Ordering::SeqCst) {
            return None;
        }
        let path = self.path(key)?;
        let gate = self.gate(key);
        let _guard = gate.lock().await;

        if path.is_file() {
            self.release(key);
            return Some(path);
        }

        let result = match self.fetcher.fetch(key).await {
            Some(bytes) => match write_atomic(&path, &bytes) {
                Ok(()) => Some(path.clone()),
                Err(error) => {
                    log::warn!("could not cache cover for {key}: {error}");
                    None
                }
            },
            None => None,
        };

        self.release(key);
        if result.is_some() {
            self.evict(&path).await;
        }
        result
    }

    fn gate(&self, key: &TrackKey) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self.gates.lock().unwrap();
        gates
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    fn release(&self, key: &TrackKey) {
        self.gates.lock().unwrap().remove(key);
    }

    async fn evict(&self, keep: &Path) {
        if self.max_bytes == 0 {
            return;
        }
        let dir = self.dir.clone();
        let keep = keep.to_path_buf();
        let max_bytes = self.max_bytes;
        let _ = tokio::task::spawn_blocking(move || evict_oldest(&dir, max_bytes, &keep)).await;
    }
}

/// Collect cache files below a directory, one level into provider
/// subdirectories, skipping temporary files.
fn collect_files(dir: &Path, files: &mut Vec<(PathBuf, u64, std::time::SystemTime)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "tmp") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect_files(&path, files);
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
        files.push((path, metadata.len(), modified));
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("jpg.tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

fn evict_oldest(dir: &Path, max_bytes: u64, keep: &Path) {
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    collect_files(dir, &mut files);
    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    if total <= max_bytes {
        return;
    }
    files.sort_by_key(|(_, _, modified)| *modified);
    for (path, size, _) in files {
        if total <= max_bytes {
            break;
        }
        if path != keep && std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    const ID: &str = "abcdefghijk";

    struct CountingFetcher {
        calls: AtomicUsize,
        bytes: Vec<u8>,
        fail: bool,
    }

    impl CountingFetcher {
        fn with_bytes(bytes: Vec<u8>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                bytes,
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                bytes: Vec::new(),
                fail: true,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl CoverFetcher for CountingFetcher {
        async fn fetch(&self, _key: &TrackKey) -> Option<Vec<u8>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (!self.fail).then(|| self.bytes.clone())
        }
    }

    fn key(id: &str) -> TrackKey {
        TrackKey::youtube(id)
    }

    #[test]
    fn validates_track_keys() {
        assert!(key(ID).validate());
        assert!(key("abc_DEF-123").validate());
        assert!(!key("short").validate());
        assert!(!key("../abcdefghijk").validate());
        assert!(!key("abcdefghij.").validate());
        assert!(!key("").validate());
    }

    #[test]
    fn cover_paths_are_scoped_by_provider() {
        let dir = tempdir().unwrap();
        let cache = ArtworkCache::with_fetcher(
            dir.path().to_path_buf(),
            0,
            Arc::new(CountingFetcher::with_bytes(vec![7; 16])),
        );
        assert_eq!(
            cache.path(&key(ID)).unwrap(),
            dir.path().join("youtube").join(format!("{ID}.jpg"))
        );
        assert!(cache.path(&key("short")).is_none());
    }

    #[tokio::test]
    async fn fetches_once_and_reuses_the_file() {
        let dir = tempdir().unwrap();
        let fetcher = Arc::new(CountingFetcher::with_bytes(vec![7; 16]));
        let cache = ArtworkCache::with_fetcher(dir.path().to_path_buf(), 0, fetcher.clone());

        let first = cache.ensure(&key(ID)).await.unwrap();
        let second = cache.ensure(&key(ID)).await.unwrap();

        assert_eq!(first, second);
        assert_eq!(fetcher.calls(), 1);
        assert_eq!(std::fs::read(&first).unwrap(), vec![7; 16]);
    }

    #[tokio::test]
    async fn offline_uses_only_cached_covers() {
        let dir = tempdir().unwrap();
        let fetcher = Arc::new(CountingFetcher::with_bytes(vec![7; 16]));
        let offline = Arc::new(AtomicBool::new(false));
        let cache = ArtworkCache::with_offline(
            dir.path().to_path_buf(),
            0,
            fetcher.clone(),
            offline.clone(),
        );

        let first = cache.ensure(&key(ID)).await.unwrap();
        offline.store(true, Ordering::SeqCst);
        let second = cache.ensure(&key(ID)).await.unwrap();
        assert_eq!(first, second);
        assert!(cache.ensure(&key("abcdefghij2")).await.is_none());
        assert_eq!(fetcher.calls(), 1);
    }

    #[tokio::test]
    async fn caches_concurrent_requests_once() {
        let dir = tempdir().unwrap();
        let fetcher = Arc::new(CountingFetcher::with_bytes(vec![7; 16]));
        let cache = ArtworkCache::with_fetcher(dir.path().to_path_buf(), 0, fetcher.clone());

        let first_key = key(ID);
        let second_key = key(ID);
        let (first, second) = tokio::join!(cache.ensure(&first_key), cache.ensure(&second_key));

        assert_eq!(first, second);
        assert_eq!(fetcher.calls(), 1);
    }

    #[tokio::test]
    async fn failed_fetch_is_not_cached() {
        let dir = tempdir().unwrap();
        let fetcher = Arc::new(CountingFetcher::failing());
        let cache = ArtworkCache::with_fetcher(dir.path().to_path_buf(), 0, fetcher.clone());

        assert!(cache.ensure(&key(ID)).await.is_none());
        assert!(cache.cached_path(&key(ID)).is_none());
        assert_eq!(fetcher.calls(), 1);
    }

    #[tokio::test]
    async fn evicts_old_files_to_the_limit() {
        let dir = tempdir().unwrap();
        let fetcher = Arc::new(CountingFetcher::with_bytes(vec![7; 600]));
        let cache = ArtworkCache::with_fetcher(dir.path().to_path_buf(), 1000, fetcher);

        for id in ["abcdefghij1", "abcdefghij2", "abcdefghij3"] {
            cache.ensure(&key(id)).await.unwrap();
        }

        let mut files = Vec::new();
        collect_files(dir.path(), &mut files);
        let total: u64 = files.iter().map(|(_, size, _)| size).sum();
        assert!(total <= 1000, "cache size was {total}");
    }
}
