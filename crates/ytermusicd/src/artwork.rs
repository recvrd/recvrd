//! Cover art cache for the HTTP API and MPRIS.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;

/// Largest image the cache accepts.
const MAX_COVER_BYTES: usize = 5 * 1024 * 1024;
/// Smallest image the cache accepts. Smaller responses are placeholders.
const MIN_COVER_BYTES: usize = 1024;

/// Fetch the JPEG bytes for a video ID.
#[async_trait]
#[allow(clippy::double_must_use)]
pub trait CoverFetcher: Send + Sync {
    async fn fetch(&self, video_id: &str) -> Option<Vec<u8>>;
}

/// Fetch covers from the public YouTube thumbnail hosts.
pub struct HttpFetcher {
    client: reqwest::Client,
}

impl HttpFetcher {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("ytermusicd/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CoverFetcher for HttpFetcher {
    async fn fetch(&self, video_id: &str) -> Option<Vec<u8>> {
        for name in ["maxresdefault", "hqdefault"] {
            let url = format!("https://i.ytimg.com/vi/{video_id}/{name}.jpg");
            let Ok(response) = self.client.get(url).send().await else {
                continue;
            };
            if !response.status().is_success() {
                continue;
            }
            let image = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("image/"));
            if !image {
                continue;
            }
            let Ok(bytes) = response.bytes().await else {
                continue;
            };
            if bytes.len() < MIN_COVER_BYTES || bytes.len() > MAX_COVER_BYTES {
                continue;
            }
            return Some(bytes.to_vec());
        }
        None
    }
}

/// Store cover art on disk and limit the cache size.
#[derive(Clone)]
pub struct ArtworkCache {
    dir: PathBuf,
    max_bytes: u64,
    fetcher: Arc<dyn CoverFetcher>,
    gates: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

impl ArtworkCache {
    pub fn new(dir: PathBuf, max_bytes: u64) -> Self {
        Self::with_fetcher(dir, max_bytes, Arc::new(HttpFetcher::new()))
    }

    pub fn with_fetcher(dir: PathBuf, max_bytes: u64, fetcher: Arc<dyn CoverFetcher>) -> Self {
        Self {
            dir,
            max_bytes,
            fetcher,
            gates: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Return true when the video ID is safe to use as a file name.
    pub fn is_valid_video_id(video_id: &str) -> bool {
        video_id.len() == 11
            && video_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    }

    /// Return the cache file for a valid video ID.
    pub fn path(&self, video_id: &str) -> Option<PathBuf> {
        Self::is_valid_video_id(video_id).then(|| self.dir.join(format!("{video_id}.jpg")))
    }

    /// Return the cached file when it exists.
    pub fn cached_path(&self, video_id: &str) -> Option<PathBuf> {
        self.path(video_id).filter(|path| path.is_file())
    }

    /// Return the cached file and fetch it when it is missing.
    pub async fn ensure(&self, video_id: &str) -> Option<PathBuf> {
        if let Some(path) = self.cached_path(video_id) {
            return Some(path);
        }
        let path = self.path(video_id)?;
        let gate = self.gate(video_id);
        let _guard = gate.lock().await;

        if path.is_file() {
            self.release(video_id);
            return Some(path);
        }

        let result = match self.fetcher.fetch(video_id).await {
            Some(bytes) => match write_atomic(&path, &bytes) {
                Ok(()) => Some(path.clone()),
                Err(error) => {
                    log::warn!("could not cache cover for {video_id}: {error}");
                    None
                }
            },
            None => None,
        };

        self.release(video_id);
        if result.is_some() {
            self.evict(&path).await;
        }
        result
    }

    fn gate(&self, video_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self.gates.lock().unwrap();
        gates
            .entry(video_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    fn release(&self, video_id: &str) {
        self.gates.lock().unwrap().remove(video_id);
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

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("jpg.tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

fn evict_oldest(dir: &Path, max_bytes: u64, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "tmp") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
            Some((path, metadata.len(), modified))
        })
        .collect();
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
        async fn fetch(&self, _video_id: &str) -> Option<Vec<u8>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (!self.fail).then(|| self.bytes.clone())
        }
    }

    #[test]
    fn validates_video_ids() {
        assert!(ArtworkCache::is_valid_video_id(ID));
        assert!(ArtworkCache::is_valid_video_id("abc_DEF-123"));
        assert!(!ArtworkCache::is_valid_video_id("short"));
        assert!(!ArtworkCache::is_valid_video_id("../abcdefghijk"));
        assert!(!ArtworkCache::is_valid_video_id("abcdefghij."));
        assert!(!ArtworkCache::is_valid_video_id(""));
    }

    #[tokio::test]
    async fn fetches_once_and_reuses_the_file() {
        let dir = tempdir().unwrap();
        let fetcher = Arc::new(CountingFetcher::with_bytes(vec![7; 16]));
        let cache = ArtworkCache::with_fetcher(dir.path().to_path_buf(), 0, fetcher.clone());

        let first = cache.ensure(ID).await.unwrap();
        let second = cache.ensure(ID).await.unwrap();

        assert_eq!(first, second);
        assert_eq!(fetcher.calls(), 1);
        assert_eq!(std::fs::read(&first).unwrap(), vec![7; 16]);
    }

    #[tokio::test]
    async fn caches_concurrent_requests_once() {
        let dir = tempdir().unwrap();
        let fetcher = Arc::new(CountingFetcher::with_bytes(vec![7; 16]));
        let cache = ArtworkCache::with_fetcher(dir.path().to_path_buf(), 0, fetcher.clone());

        let (first, second) = tokio::join!(cache.ensure(ID), cache.ensure(ID));

        assert_eq!(first, second);
        assert_eq!(fetcher.calls(), 1);
    }

    #[tokio::test]
    async fn failed_fetch_is_not_cached() {
        let dir = tempdir().unwrap();
        let fetcher = Arc::new(CountingFetcher::failing());
        let cache = ArtworkCache::with_fetcher(dir.path().to_path_buf(), 0, fetcher.clone());

        assert!(cache.ensure(ID).await.is_none());
        assert!(cache.cached_path(ID).is_none());
        assert_eq!(fetcher.calls(), 1);
    }

    #[tokio::test]
    async fn evicts_old_files_to_the_limit() {
        let dir = tempdir().unwrap();
        let fetcher = Arc::new(CountingFetcher::with_bytes(vec![7; 600]));
        let cache = ArtworkCache::with_fetcher(dir.path().to_path_buf(), 1000, fetcher);

        for id in ["abcdefghij1", "abcdefghij2", "abcdefghij3"] {
            cache.ensure(id).await.unwrap();
        }

        let total: u64 = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.metadata().ok())
            .map(|metadata| metadata.len())
            .sum();
        assert!(total <= 1000, "cache size was {total}");
    }
}
