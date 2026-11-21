//! The RSS music provider: feed management, the provider-neutral
//! music API, enclosure downloads and cover fetches.
//!
//! One `RssProvider` owns the feed list and the episode index. The
//! daemon shares it between the music API, the download manager
//! and the cover cache, so all three see the same episodes.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use common_structs::{ProviderId, TrackKey};
use download_manager::{DownloadError, TrackDownloader};
use recvrd_api::{MusicApi, Playlist, PlaylistEditError, RssFeed, SearchResults, Track};
use recvrd_core::{CoverFetcher, Track as CoreTrack};
use reqwest::header::{CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};

use crate::{
    feed::{Feed, episode_id, feed_id, parse_feed},
    store::{RssStore, StoredEpisode, StoredFeed},
};

/// Largest cover image the fetcher accepts.
const MAX_COVER_BYTES: usize = 5 * 1024 * 1024;
/// Smallest cover image the fetcher accepts.
const MIN_COVER_BYTES: usize = 1024;

/// The outcome of a feed fetch.
enum FetchOutcome {
    /// The feed did not change since the last fetch.
    NotModified,
    /// The feed body, with its new conditional GET state.
    Fetched {
        feed: Feed,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

/// The RSS provider. Feeds live in the store file; episodes live
/// in memory and in the store, keyed by their track ID.
pub struct RssProvider {
    client: reqwest::Client,
    store_path: PathBuf,
    state: Arc<RwLock<RssStore>>,
}

impl RssProvider {
    /// Load the provider from its store file. A missing or corrupt
    /// file starts empty.
    pub fn new(store_path: PathBuf) -> Self {
        let state = RssStore::load(&store_path);
        let loaded_empty = state.feeds.is_empty() && state.episodes.is_empty();
        if loaded_empty && store_path.exists() {
            log::warn!(
                "RSS store {} is unreadable; starting with no feeds",
                store_path.display()
            );
        }
        Self {
            client: reqwest::Client::builder()
                .user_agent(concat!("recvrd/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            store_path,
            state: Arc::new(RwLock::new(state)),
        }
    }

    /// The stored feeds, as API DTOs.
    pub fn feeds(&self) -> Vec<RssFeed> {
        let state = self.state.read().unwrap();
        state.feeds.iter().map(feed_dto(&state)).collect()
    }

    /// Look up one stored episode by its track key.
    pub fn episode(&self, key: &TrackKey) -> Option<StoredEpisode> {
        let state = self.state.read().unwrap();
        state
            .episodes
            .iter()
            .find(|episode| episode.track_id == key.id)
            .cloned()
    }

    /// Add a feed URL and fetch it once. Adding a URL twice
    /// returns the existing feed.
    pub async fn add_feed(&self, url: &str) -> Result<RssFeed, String> {
        let url = validate_feed_url(url)?;
        {
            let state = self.state.read().unwrap();
            if let Some(found) = state.feeds.iter().find(|feed| feed.url == url) {
                return Ok(feed_dto(&state)(found));
            }
        }
        let outcome = self.fetch(&url, None, None).await?;
        let fetched = match outcome {
            FetchOutcome::Fetched {
                feed,
                etag,
                last_modified,
            } => (feed, etag, last_modified),
            FetchOutcome::NotModified => {
                return Err("feed did not change".to_string());
            }
        };
        let (feed, etag, last_modified) = fetched;
        let dto = {
            let mut state = self.state.write().unwrap();
            apply_feed(&mut state, &url, &feed, etag, last_modified, now_unix());
            let found = state.feeds.iter().find(|feed| feed.url == url).unwrap();
            let dto = feed_dto(&state)(found);
            persist(&self.store_path, &state);
            dto
        };
        Ok(dto)
    }

    /// True when a feed with this ID is stored.
    pub fn has_feed(&self, id: &str) -> bool {
        let state = self.state.read().unwrap();
        state.feeds.iter().any(|feed| feed_id(&feed.url) == id)
    }

    /// Remove a feed and its episodes. False when the feed is unknown.
    pub fn remove_feed(&self, id: &str) -> bool {
        let mut state = self.state.write().unwrap();
        let Some(index) = state.feeds.iter().position(|feed| feed_id(&feed.url) == id) else {
            return false;
        };
        let url = state.feeds.remove(index).url;
        state.episodes.retain(|episode| episode.feed_url != url);
        persist(&self.store_path, &state);
        true
    }

    /// Refresh one feed with a conditional GET. A 304 response
    /// leaves the episodes untouched.
    pub async fn refresh_feed(&self, id: &str) -> Result<RssFeed, String> {
        let (url, etag, last_modified) = {
            let state = self.state.read().unwrap();
            let Some(feed) = state.feeds.iter().find(|feed| feed_id(&feed.url) == id) else {
                return Err(format!("feed {id} not found"));
            };
            (
                feed.url.clone(),
                feed.etag.clone(),
                feed.last_modified.clone(),
            )
        };
        match self
            .fetch(&url, etag.as_deref(), last_modified.as_deref())
            .await?
        {
            FetchOutcome::NotModified => {
                let mut state = self.state.write().unwrap();
                if let Some(feed) = state.feeds.iter_mut().find(|feed| feed.url == url) {
                    feed.last_fetched = Some(now_unix());
                }
                let dto = state
                    .feeds
                    .iter()
                    .find(|feed| feed.url == url)
                    .map(feed_dto(&state))
                    .unwrap();
                persist(&self.store_path, &state);
                Ok(dto)
            }
            FetchOutcome::Fetched {
                feed,
                etag,
                last_modified,
            } => {
                let mut state = self.state.write().unwrap();
                apply_feed(&mut state, &url, &feed, etag, last_modified, now_unix());
                let dto = state
                    .feeds
                    .iter()
                    .find(|feed| feed.url == url)
                    .map(feed_dto(&state))
                    .unwrap();
                persist(&self.store_path, &state);
                Ok(dto)
            }
        }
    }

    /// Refresh every feed. One feed failure does not stop the rest.
    pub async fn refresh_all(&self) -> Vec<(String, Result<RssFeed, String>)> {
        let ids: Vec<String> = {
            let state = self.state.read().unwrap();
            state.feeds.iter().map(|feed| feed_id(&feed.url)).collect()
        };
        let mut results = Vec::new();
        for id in ids {
            let result = self.refresh_feed(&id).await;
            results.push((id, result));
        }
        results
    }

    /// Fetch a feed, with conditional GET headers when the caller
    /// has stored state for it.
    async fn fetch(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<FetchOutcome, String> {
        let mut request = self.client.get(url);
        if let Some(etag) = etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        if let Some(last_modified) = last_modified {
            request = request.header(IF_MODIFIED_SINCE, last_modified);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("could not fetch feed {url}: {error}"))?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(FetchOutcome::NotModified);
        }
        if !response.status().is_success() {
            return Err(format!("feed {url} returned status {}", response.status()));
        }
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let last_modified = response
            .headers()
            .get(LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response
            .bytes()
            .await
            .map_err(|error| format!("could not read feed {url}: {error}"))?;
        let feed = parse_feed(&bytes)?;
        Ok(FetchOutcome::Fetched {
            feed,
            etag,
            last_modified,
        })
    }

    /// The cover URL for an episode: its own image first, then the
    /// feed image.
    fn cover_url(&self, episode: &StoredEpisode) -> Option<String> {
        if let Some(url) = &episode.cover_url {
            return Some(url.clone());
        }
        let state = self.state.read().unwrap();
        state
            .feeds
            .iter()
            .find(|feed| feed.url == episode.feed_url)
            .and_then(|feed| feed_image(&state, &feed.url))
    }
}

/// Check that a feed URL is an HTTP(S) URL with a host.
fn validate_feed_url(url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| format!("invalid feed URL {url}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!("feed URL {url} must use http or https"));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(format!("feed URL {url} has no host"));
    }
    Ok(parsed.to_string())
}

/// The current Unix time in seconds.
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Save the store and log instead of failing the caller: a feed
/// change already happened in memory, so a bad store file must not
/// undo it.
fn persist(path: &Path, state: &RssStore) {
    if let Err(error) = state.save(path) {
        log::warn!("could not save RSS store {}: {error}", path.display());
    }
}

/// The feed image URL from the stored episodes of one feed.
fn feed_image(state: &RssStore, feed_url: &str) -> Option<String> {
    state
        .episodes
        .iter()
        .find(|episode| episode.feed_url == feed_url)
        .and_then(|episode| episode.cover_url.clone())
}

/// Replace the episodes of one feed with a fresh fetch. Episodes
/// keep their stable track IDs, so a refresh never duplicates one.
fn apply_feed(
    state: &mut RssStore,
    url: &str,
    feed: &Feed,
    etag: Option<String>,
    last_modified: Option<String>,
    now: u64,
) {
    let title = if feed.title.is_empty() {
        url.to_string()
    } else {
        feed.title.clone()
    };
    let album = title.clone();
    state.episodes.retain(|episode| episode.feed_url != url);
    let mut skipped = 0;
    for episode in &feed.episodes {
        let Some(extension) = episode.extension() else {
            skipped += 1;
            continue;
        };
        state.episodes.push(StoredEpisode {
            feed_url: url.to_string(),
            guid: episode.identity.clone(),
            track_id: episode_id(&episode.identity),
            title: episode.title.clone(),
            artist: episode.author.clone(),
            album: album.clone(),
            duration: episode.duration.clone(),
            enclosure_url: episode.enclosure_url.clone(),
            extension: extension.to_string(),
            cover_url: episode.cover_url.clone().or(feed.image_url.clone()),
        });
    }
    if skipped > 0 {
        log::warn!("feed {url} has {skipped} episodes without a supported audio enclosure");
    }
    match state.feeds.iter_mut().find(|stored| stored.url == url) {
        Some(stored) => {
            stored.etag = etag;
            stored.last_modified = last_modified;
            stored.title = title;
            stored.last_fetched = Some(now);
        }
        None => state.feeds.push(StoredFeed {
            url: url.to_string(),
            etag,
            last_modified,
            title,
            last_fetched: Some(now),
        }),
    }
}

/// Build the API DTO for one stored feed.
fn feed_dto(state: &RssStore) -> impl Fn(&StoredFeed) -> RssFeed + '_ {
    move |feed| RssFeed {
        id: feed_id(&feed.url),
        url: feed.url.clone(),
        title: feed.title.clone(),
        episode_count: state
            .episodes
            .iter()
            .filter(|episode| episode.feed_url == feed.url)
            .count(),
        last_fetched: feed.last_fetched,
    }
}

/// Build the API track for one stored episode.
fn track_dto(episode: &StoredEpisode) -> Track {
    Track {
        provider: ProviderId::Rss,
        track_id: episode.track_id.clone(),
        title: episode.title.clone(),
        artist: episode.artist.clone(),
        album: episode.album.clone(),
        duration: episode.duration.clone(),
    }
}

fn check_provider(provider: ProviderId) -> Result<(), String> {
    if provider == ProviderId::Rss {
        Ok(())
    } else {
        Err(format!(
            "provider {provider} is not available; RSS is the only provider for this implementation"
        ))
    }
}

fn edit_unsupported(action: &str) -> PlaylistEditError {
    PlaylistEditError::Invalid(format!(
        "RSS feeds are read-only; {action} is not supported"
    ))
}

#[async_trait]
impl MusicApi for RssProvider {
    async fn playlists(&self, provider: ProviderId) -> Result<Vec<Playlist>, String> {
        check_provider(provider)?;
        let state = self.state.read().unwrap();
        Ok(state
            .feeds
            .iter()
            .map(|feed| Playlist {
                id: feed_id(&feed.url),
                name: feed.title.clone(),
                subtitle: feed.url.clone(),
            })
            .collect())
    }

    async fn playlist(&self, provider: ProviderId, id: &str) -> Result<Vec<Track>, String> {
        check_provider(provider)?;
        let state = self.state.read().unwrap();
        let Some(feed) = state.feeds.iter().find(|feed| feed_id(&feed.url) == id) else {
            return Err(format!("feed {id} not found"));
        };
        let url = feed.url.clone();
        Ok(state
            .episodes
            .iter()
            .filter(|episode| episode.feed_url == url)
            .map(track_dto)
            .collect())
    }

    async fn search(&self, provider: ProviderId, query: &str) -> Result<SearchResults, String> {
        check_provider(provider)?;
        let query = query.to_lowercase();
        let state = self.state.read().unwrap();
        let tracks: Vec<Track> = state
            .episodes
            .iter()
            .filter(|episode| {
                episode.title.to_lowercase().contains(&query)
                    || episode.artist.to_lowercase().contains(&query)
                    || episode.album.to_lowercase().contains(&query)
            })
            .map(track_dto)
            .collect();
        Ok(SearchResults {
            tracks,
            playlists: Vec::new(),
        })
    }

    async fn create_playlist(
        &self,
        _request: &recvrd_api::CreatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError> {
        Err(edit_unsupported("creating playlists"))
    }

    async fn update_playlist(
        &self,
        _id: &str,
        _request: &recvrd_api::UpdatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError> {
        Err(edit_unsupported("renaming playlists"))
    }

    async fn delete_playlist(&self, _id: &str) -> Result<(), PlaylistEditError> {
        Err(edit_unsupported("deleting playlists"))
    }

    async fn add_playlist_tracks(
        &self,
        _id: &str,
        _track_ids: &[String],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        Err(edit_unsupported("adding tracks"))
    }

    async fn remove_playlist_tracks(
        &self,
        _id: &str,
        _indices: &[usize],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        Err(edit_unsupported("removing tracks"))
    }

    async fn move_playlist_track(
        &self,
        _id: &str,
        _from: usize,
        _to: usize,
    ) -> Result<Vec<Track>, PlaylistEditError> {
        Err(edit_unsupported("moving tracks"))
    }
}

#[async_trait]
impl TrackDownloader for RssProvider {
    async fn download(
        &self,
        track: &CoreTrack,
        output: &Path,
        progress: &(dyn Fn(usize) + Send + Sync),
    ) -> Result<(), DownloadError> {
        let key = track.key();
        let Some(episode) = self.episode(&key) else {
            return Err(DownloadError::BackendFailed(format!(
                "unknown RSS track {key}"
            )));
        };
        let mut response = self
            .client
            .get(&episode.enclosure_url)
            .send()
            .await
            .map_err(|error| {
                DownloadError::BackendFailed(format!(
                    "could not fetch enclosure {}: {error}",
                    episode.enclosure_url
                ))
            })?;
        if !response.status().is_success() {
            return Err(DownloadError::BackendFailed(format!(
                "enclosure {} returned status {}",
                episode.enclosure_url,
                response.status()
            )));
        }
        let total = response.content_length().unwrap_or(0);
        let mut file = tokio::fs::File::create(output).await?;
        progress(0);
        let mut downloaded: u64 = 0;
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    use tokio::io::AsyncWriteExt;
                    file.write_all(&chunk).await?;
                    downloaded += chunk.len() as u64;
                    if let Some(percent) = downloaded.saturating_mul(100).checked_div(total) {
                        progress(percent.min(100) as usize);
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    return Err(DownloadError::BackendFailed(format!(
                        "could not read enclosure {}: {error}",
                        episode.enclosure_url
                    )));
                }
            }
        }
        use tokio::io::AsyncWriteExt;
        file.flush().await?;
        progress(100);
        Ok(())
    }

    fn audio_extension(&self, track: &CoreTrack) -> String {
        self.episode(&track.key())
            .map(|episode| episode.extension)
            .unwrap_or_else(|| "mp3".to_string())
    }
}

#[async_trait]
impl CoverFetcher for RssProvider {
    async fn fetch(&self, key: &TrackKey) -> Option<Vec<u8>> {
        let episode = self.episode(key)?;
        let url = self.cover_url(&episode)?;
        let Ok(response) = self.client.get(&url).send().await else {
            return None;
        };
        if !response.status().is_success() {
            return None;
        }
        let image = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("image/"));
        if !image {
            return None;
        }
        let Ok(bytes) = response.bytes().await else {
            return None;
        };
        if bytes.len() < MIN_COVER_BYTES || bytes.len() > MAX_COVER_BYTES {
            return None;
        }
        Some(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::parse_feed;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const RSS: &str = include_str!("../fixtures/rss.xml");

    fn provider() -> (RssProvider, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rss-feeds.json");
        (RssProvider::new(path), dir)
    }

    fn stored_episode() -> StoredEpisode {
        StoredEpisode {
            feed_url: "https://example.com/feed.xml".to_string(),
            guid: "episode-guid-1".to_string(),
            track_id: episode_id("episode-guid-1"),
            title: "First Song".to_string(),
            artist: "The Testers".to_string(),
            album: "Test Tones".to_string(),
            duration: "4:05".to_string(),
            enclosure_url: "https://example.com/ep1.mp3".to_string(),
            extension: "mp3".to_string(),
            cover_url: None,
        }
    }

    /// Build the core track for one stored episode.
    fn core_track(episode: &StoredEpisode) -> CoreTrack {
        CoreTrack {
            provider: ProviderId::Rss,
            track_id: episode.track_id.clone(),
            title: episode.title.clone(),
            author: episode.artist.clone(),
            album: episode.album.clone(),
            duration: episode.duration.clone(),
        }
    }

    /// A tiny HTTP server for tests. It answers each request with
    /// the handler result, and records the request headers.
    type RecordedRequests = Arc<std::sync::Mutex<Vec<(String, Vec<(String, String)>)>>>;

    struct FixtureServer {
        url: String,
        requests: RecordedRequests,
    }

    impl FixtureServer {
        async fn start(
            handler: impl Fn(String, Vec<(String, String)>) -> (u16, Vec<(String, String)>, Vec<u8>)
            + Send
            + Sync
            + 'static,
        ) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
            let recorded = requests.clone();
            let handler = Arc::new(handler);
            tokio::spawn(async move {
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        return;
                    };
                    let handler = handler.clone();
                    let recorded = recorded.clone();
                    tokio::spawn(async move {
                        let mut buffer = [0u8; 8192];
                        let Ok(read) = stream.read(&mut buffer).await else {
                            return;
                        };
                        let text = String::from_utf8_lossy(&buffer[..read]).into_owned();
                        let mut lines = text.lines();
                        let path = lines
                            .next()
                            .unwrap_or("")
                            .split(' ')
                            .nth(1)
                            .unwrap_or("")
                            .to_string();
                        let headers = lines
                            .take_while(|line| !line.is_empty())
                            .filter_map(|line| line.split_once(": "))
                            .map(|(key, value)| (key.to_string(), value.to_string()))
                            .collect::<Vec<_>>();
                        let (status, extra, body) = handler(path.clone(), headers.clone());
                        recorded.lock().unwrap().push((path, headers));
                        let mut response = format!(
                            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
                            body.len()
                        );
                        for (key, value) in extra {
                            response.push_str(&format!("{key}: {value}\r\n"));
                        }
                        response.push_str("\r\n");
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.write_all(&body).await;
                    });
                }
            });
            Self { url, requests }
        }

        fn header(&self, name: &str) -> Option<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .flat_map(|(_, headers)| headers.iter())
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn validates_feed_urls() {
        assert!(validate_feed_url("https://example.com/feed.xml").is_ok());
        assert!(validate_feed_url("http://example.com/feed.xml").is_ok());
        assert!(validate_feed_url("ftp://example.com/feed.xml").is_err());
        assert!(validate_feed_url("not a url").is_err());
        assert!(validate_feed_url("https://").is_err());
    }

    #[test]
    fn apply_feed_replaces_episodes_without_duplicates() {
        let (_provider, _dir) = provider();
        let mut store = RssStore::default();
        let feed = parse_feed(RSS.as_bytes()).unwrap();
        apply_feed(
            &mut store,
            "https://example.com/feed.xml",
            &feed,
            None,
            None,
            100,
        );
        assert_eq!(store.episodes.len(), 2);
        apply_feed(
            &mut store,
            "https://example.com/feed.xml",
            &feed,
            None,
            None,
            200,
        );
        assert_eq!(store.episodes.len(), 2);
        assert_eq!(store.feeds.len(), 1);
        assert_eq!(store.feeds[0].last_fetched, Some(200));
    }

    #[test]
    fn apply_feed_skips_unsupported_enclosures() {
        let mut store = RssStore::default();
        let feed = parse_feed(
            br#"<?xml version="1.0"?><rss version="2.0"><channel><title>F</title>
            <item><title>Wave</title><guid>g1</guid>
            <enclosure url="https://example.com/a.wav" type="audio/wav" length="1"/></item>
            </channel></rss>"#,
        )
        .unwrap();
        apply_feed(
            &mut store,
            "https://example.com/feed.xml",
            &feed,
            None,
            None,
            1,
        );
        assert!(store.episodes.is_empty());
        assert_eq!(store.feeds.len(), 1);
    }

    #[tokio::test]
    async fn add_feed_fetches_and_indexes_episodes() {
        let server = FixtureServer::start(|_, _| {
            (
                200,
                vec![("ETag".to_string(), "\"v1\"".to_string())],
                RSS.as_bytes().to_vec(),
            )
        })
        .await;
        let (provider, _dir) = provider();
        let feed = provider
            .add_feed(&format!("{}/feed.xml", server.url))
            .await
            .unwrap();
        assert_eq!(feed.title, "Test Tones");
        assert_eq!(feed.episode_count, 2);
        assert_eq!(feed.last_fetched, Some(now_unix()));

        let tracks = provider.playlist(ProviderId::Rss, &feed.id).await.unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].provider, ProviderId::Rss);
        assert_eq!(tracks[0].title, "First Song");

        let found = provider.search(ProviderId::Rss, "second").await.unwrap();
        assert_eq!(found.tracks.len(), 1);
        assert_eq!(found.tracks[0].title, "Second Song & More");
    }

    #[tokio::test]
    async fn add_feed_twice_returns_the_same_feed() {
        let server = FixtureServer::start(|_, _| (200, vec![], RSS.as_bytes().to_vec())).await;
        let (provider, _dir) = provider();
        let url = format!("{}/feed.xml", server.url);
        let first = provider.add_feed(&url).await.unwrap();
        let second = provider.add_feed(&url).await.unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(provider.feeds().len(), 1);
    }

    #[tokio::test]
    async fn refresh_uses_conditional_get_and_keeps_episodes() {
        let server = FixtureServer::start(|_, _| {
            (
                200,
                vec![("ETag".to_string(), "\"v1\"".to_string())],
                RSS.as_bytes().to_vec(),
            )
        })
        .await;
        let (provider, _dir) = provider();
        let url = format!("{}/feed.xml", server.url);
        let feed = provider.add_feed(&url).await.unwrap();
        let refreshed = provider.refresh_feed(&feed.id).await.unwrap();
        assert_eq!(refreshed.episode_count, 2);
        assert_eq!(provider.feeds().len(), 1);
        // The refresh request carried the ETag the first fetch stored.
        assert_eq!(server.header("if-none-match").as_deref(), Some("\"v1\""));
    }

    #[tokio::test]
    async fn refresh_with_304_keeps_the_index() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = calls.clone();
        let server = FixtureServer::start(move |_, _| {
            let call = seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if call == 0 {
                (
                    200,
                    vec![("ETag".to_string(), "\"v1\"".to_string())],
                    RSS.as_bytes().to_vec(),
                )
            } else {
                (304, vec![], vec![])
            }
        })
        .await;
        let (provider, _dir) = provider();
        let url = format!("{}/feed.xml", server.url);
        let feed = provider.add_feed(&url).await.unwrap();
        let refreshed = provider.refresh_feed(&feed.id).await.unwrap();
        assert_eq!(refreshed.episode_count, 2);
        let tracks = provider.playlist(ProviderId::Rss, &feed.id).await.unwrap();
        assert_eq!(tracks.len(), 2);
    }

    #[tokio::test]
    async fn remove_feed_drops_its_episodes() {
        let server = FixtureServer::start(|_, _| (200, vec![], RSS.as_bytes().to_vec())).await;
        let (provider, _dir) = provider();
        let feed = provider
            .add_feed(&format!("{}/feed.xml", server.url))
            .await
            .unwrap();
        assert!(provider.remove_feed(&feed.id));
        assert!(!provider.remove_feed(&feed.id));
        assert!(provider.feeds().is_empty());
        assert!(provider.playlist(ProviderId::Rss, &feed.id).await.is_err());
    }

    #[tokio::test]
    async fn download_fetches_the_enclosure_with_progress() {
        let audio = vec![7u8; 1024];
        let sent = audio.clone();
        let server = FixtureServer::start(move |_, _| {
            (
                200,
                vec![("Content-Type".to_string(), "audio/mpeg".to_string())],
                sent.clone(),
            )
        })
        .await;
        let (provider, _dir) = provider();
        {
            let mut state = provider.state.write().unwrap();
            let mut episode = stored_episode();
            episode.enclosure_url = format!("{}/ep1.mp3", server.url);
            state.episodes.push(episode);
        }
        let dir = tempdir().unwrap();
        let output = dir.path().join("out.part");
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_clone = seen.clone();
        let track = core_track(&stored_episode());
        provider
            .download(&track, &output, &|percent| {
                seen_clone.lock().unwrap().push(percent);
            })
            .await
            .unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), audio);
        assert_eq!(*seen.lock().unwrap().last().unwrap(), 100);
    }

    #[tokio::test]
    async fn download_of_an_unknown_track_fails() {
        let (provider, _dir) = provider();
        let dir = tempdir().unwrap();
        let track = core_track(&stored_episode());
        let error = provider
            .download(&track, &dir.path().join("out.part"), &|_| {})
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unknown RSS track"));
    }

    #[tokio::test]
    async fn rss_rejects_playlist_edits() {
        let (provider, _dir) = provider();
        let result = provider
            .create_playlist(&recvrd_api::CreatePlaylistRequest {
                title: "Mix".to_string(),
                description: String::new(),
                privacy: recvrd_api::PlaylistPrivacy::Private,
            })
            .await;
        assert!(matches!(result, Err(PlaylistEditError::Invalid(_))));
    }
}
