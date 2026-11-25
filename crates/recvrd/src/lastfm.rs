//! Last.fm scrobble worker.
//!
//! The worker drains play events from the playback service and posts
//! them to Last.fm. It never blocks playback. A scrobble that fails or
//! arrives while offline stays in memory and goes out after a retry
//! delay.
//!
//! The worker uses the Last.fm desktop auth flow. Each user supplies
//! an API key, an API secret, and a session key. The daemon never
//! ships a key.

use crate::scrobble::duration_secs;
use recvrd_core::{LastfmConfig, ScrobbleEvent, Track};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::broadcast;

/// The Last.fm API endpoint.
const API_URL: &str = "https://ws.audioscrobbler.com/2.0/";
/// Delay before a failed call or an offline queue tries again.
const RETRY_DELAY: Duration = Duration::from_secs(30);
/// Maximum queued scrobbles. The oldest leaves when the queue is full.
const MAX_QUEUED: usize = 1000;
/// Request timeout. Last.fm answers fast, or the call is a retry.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Build a scrobble worker when the config enables it. Return true
/// when the worker starts.
pub fn start(
    config: &LastfmConfig,
    offline: Arc<AtomicBool>,
    events: broadcast::Receiver<ScrobbleEvent>,
) -> bool {
    if !config.enabled {
        return false;
    }
    let Some(client) = LastfmClient::from_config(config) else {
        log::warn!(
            "Last.fm scrobbling is on, but the API key, API secret, or session key is missing"
        );
        return false;
    };
    log::info!("Last.fm scrobbling is on");
    start_worker(client, offline, RETRY_DELAY, events);
    true
}

/// Start the worker with a client and a retry delay. Tests use a
/// local server and a short delay.
fn start_worker(
    client: LastfmClient,
    offline: Arc<AtomicBool>,
    retry_delay: Duration,
    events: broadcast::Receiver<ScrobbleEvent>,
) {
    tokio::spawn(
        LastfmWorker {
            client,
            offline,
            events,
            queue: VecDeque::new(),
            retry_delay,
            retry_at: None,
        }
        .run(),
    );
}

/// A queued scrobble with its original start time.
type QueuedScrobble = (Track, u64);

struct LastfmWorker {
    client: LastfmClient,
    offline: Arc<AtomicBool>,
    events: broadcast::Receiver<ScrobbleEvent>,
    queue: VecDeque<QueuedScrobble>,
    retry_delay: Duration,
    retry_at: Option<Instant>,
}

impl LastfmWorker {
    async fn run(mut self) {
        loop {
            self.flush().await;
            let mut wait = self
                .retry_at
                .map(|at| at.saturating_duration_since(Instant::now()))
                .unwrap_or(self.retry_delay);
            // Check a waiting queue and offline mode more often, so a
            // reconnect flush starts within one second.
            if self.offline.load(Ordering::SeqCst) || !self.queue.is_empty() {
                wait = wait.min(Duration::from_secs(1));
            }
            match tokio::time::timeout(wait, self.events.recv()).await {
                Ok(Ok(event)) => self.handle(event).await,
                Ok(Err(broadcast::error::RecvError::Lagged(count))) => {
                    log::warn!("the Last.fm worker lagged by {count} events");
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => return,
                Err(_) => {}
            }
        }
    }

    async fn handle(&mut self, event: ScrobbleEvent) {
        match event {
            ScrobbleEvent::NowPlaying { track, started_at } => {
                if self.offline.load(Ordering::SeqCst) {
                    return;
                }
                if let Err(error) = self.client.now_playing(&track, started_at).await {
                    log::warn!("Last.fm now-playing failed: {error}");
                }
            }
            ScrobbleEvent::Scrobble { track, started_at } => {
                if self.offline.load(Ordering::SeqCst) {
                    self.enqueue(track, started_at);
                    return;
                }
                match self.client.scrobble(&track, started_at).await {
                    Ok(()) => {}
                    Err(ScrobbleError::Permanent(message)) => {
                        log::warn!("Last.fm rejected a scrobble: {message}");
                    }
                    Err(ScrobbleError::Retry(message)) => {
                        log::warn!("Last.fm scrobble failed: {message}");
                        self.enqueue(track, started_at);
                        self.retry_at = Some(Instant::now() + self.retry_delay);
                    }
                }
            }
        }
    }

    /// Send every queued scrobble in order. A failed call stops the
    /// pass and sets the next retry time.
    async fn flush(&mut self) {
        if self.offline.load(Ordering::SeqCst) {
            return;
        }
        if self.retry_at.is_some_and(|at| Instant::now() < at) {
            return;
        }
        while let Some((track, started_at)) = self.queue.pop_front() {
            match self.client.scrobble(&track, started_at).await {
                Ok(()) => {}
                Err(ScrobbleError::Permanent(message)) => {
                    log::warn!("Last.fm rejected a queued scrobble: {message}");
                }
                Err(ScrobbleError::Retry(message)) => {
                    log::warn!("Last.fm queued scrobble failed: {message}");
                    self.queue.push_front((track, started_at));
                    self.retry_at = Some(Instant::now() + self.retry_delay);
                    return;
                }
            }
        }
        self.retry_at = None;
    }

    fn enqueue(&mut self, track: Track, started_at: u64) {
        if self.queue.len() >= MAX_QUEUED {
            self.queue.pop_front();
            log::warn!("the Last.fm scrobble queue is full; dropped the oldest play");
        }
        self.queue.push_back((track, started_at));
    }
}

/// A failure from a Last.fm call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ScrobbleError {
    /// The call may succeed later.
    Retry(String),
    /// The call will not succeed with the same input.
    Permanent(String),
}

impl std::fmt::Display for ScrobbleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retry(message) | Self::Permanent(message) => f.write_str(message),
        }
    }
}

/// A Last.fm API client.
struct LastfmClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    api_secret: String,
    session_key: String,
}

impl LastfmClient {
    fn from_config(config: &LastfmConfig) -> Option<Self> {
        let api_key = config
            .api_key
            .as_deref()
            .filter(|value| !value.is_empty())?;
        let api_secret = config
            .api_secret
            .as_deref()
            .filter(|value| !value.is_empty())?;
        let session_key = config
            .session_key
            .as_deref()
            .filter(|value| !value.is_empty())?;
        Some(Self::new(
            API_URL.to_string(),
            api_key.to_string(),
            api_secret.to_string(),
            session_key.to_string(),
        ))
    }

    fn new(base_url: String, api_key: String, api_secret: String, session_key: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            http,
            base_url,
            api_key,
            api_secret,
            session_key,
        }
    }

    async fn now_playing(&self, track: &Track, _started_at: u64) -> Result<(), ScrobbleError> {
        let mut params = self.base_params("track.updateNowPlaying");
        params.extend(track_params(track)?);
        self.post(params).await
    }

    async fn scrobble(&self, track: &Track, started_at: u64) -> Result<(), ScrobbleError> {
        let mut params = self.base_params("track.scrobble");
        params.extend(track_params(track)?);
        params.push(("timestamp".to_string(), started_at.to_string()));
        self.post(params).await
    }

    fn base_params(&self, method: &str) -> Vec<(String, String)> {
        vec![
            ("method".to_string(), method.to_string()),
            ("api_key".to_string(), self.api_key.clone()),
            ("sk".to_string(), self.session_key.clone()),
        ]
    }

    async fn post(&self, mut params: Vec<(String, String)>) -> Result<(), ScrobbleError> {
        let signature = signature(&params, &self.api_secret);
        params.push(("api_sig".to_string(), signature));
        params.push(("format".to_string(), "json".to_string()));
        let response = self
            .http
            .post(&self.base_url)
            .form(&params)
            .send()
            .await
            .map_err(|error| ScrobbleError::Retry(error.to_string()))?;
        let status = response.status();
        let body: serde_json::Value = response.json().await.unwrap_or_default();
        if !status.is_success() {
            let message = format!("HTTP {status}");
            return Err(if status.is_server_error() {
                ScrobbleError::Retry(message)
            } else {
                ScrobbleError::Permanent(message)
            });
        }
        if let Some(code) = body.get("error").and_then(serde_json::Value::as_i64) {
            let message = body
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown error");
            return Err(classify_error(code, message));
        }
        Ok(())
    }
}

/// The Last.fm API signature: the sorted key and value pairs, then
/// the API secret, hashed with MD5.
fn signature(params: &[(String, String)], secret: &str) -> String {
    let mut sorted: Vec<&(String, String)> = params.iter().collect();
    sorted.sort_by(|left, right| left.0.cmp(&right.0));
    let mut text = String::new();
    for (key, value) in sorted {
        text.push_str(key);
        text.push_str(value);
    }
    text.push_str(secret);
    format!("{:x}", md5::compute(text))
}

/// The track fields for a Last.fm call. Last.fm needs an artist and
/// a title; a track without them cannot scrobble.
fn track_params(track: &Track) -> Result<Vec<(String, String)>, ScrobbleError> {
    if track.author.trim().is_empty() || track.title.trim().is_empty() {
        return Err(ScrobbleError::Permanent(
            "the track has no artist or title".to_string(),
        ));
    }
    let mut params = vec![
        ("artist".to_string(), track.author.clone()),
        ("track".to_string(), track.title.clone()),
    ];
    if !track.album.trim().is_empty() {
        params.push(("album".to_string(), track.album.clone()));
    }
    if let Some(duration) = duration_secs(&track.duration) {
        params.push(("duration".to_string(), duration.to_string()));
    }
    Ok(params)
}

/// Last.fm classifies some errors as retryable and some as final.
fn classify_error(code: i64, message: &str) -> ScrobbleError {
    let text = format!("Last.fm error {code}: {message}");
    match code {
        8 | 11 | 16 | 29 => ScrobbleError::Retry(text),
        _ => ScrobbleError::Permanent(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn track() -> Track {
        Track {
            provider: common_structs::ProviderId::Youtube,
            track_id: "vid123".to_string(),
            title: "Title".to_string(),
            author: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
            ..Default::default()
        }
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        while let Ok(count) = socket.read(&mut chunk).await {
            if count == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..count]);
            if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                let header_end = position + 4;
                let headers = String::from_utf8_lossy(&buffer[..header_end]).to_string();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if buffer.len() >= header_end + length {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&buffer).to_string()
    }

    /// Serve every request with the same JSON body and record the
    /// request bodies. The server stops when the task aborts.
    async fn mock_server(
        body: &'static str,
    ) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let request = read_request(&mut socket).await;
                if let Some(body) = request.split("\r\n\r\n").nth(1) {
                    recorded.lock().unwrap().push(body.to_string());
                }
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        (format!("http://{address}/2.0/"), requests, handle)
    }

    fn client(base_url: String) -> LastfmClient {
        LastfmClient::new(
            base_url,
            "abc".to_string(),
            "secret".to_string(),
            "xyz".to_string(),
        )
    }

    async fn wait_for(check: impl Fn() -> bool) -> bool {
        for _ in 0..200 {
            if check() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[test]
    fn signs_params_in_sorted_order() {
        let params = vec![
            ("method".to_string(), "track.scrobble".to_string()),
            ("track".to_string(), "Title".to_string()),
            ("api_key".to_string(), "abc".to_string()),
            ("timestamp".to_string(), "1700000000".to_string()),
            ("artist".to_string(), "Artist".to_string()),
            ("sk".to_string(), "xyz".to_string()),
        ];
        assert_eq!(
            signature(&params, "secret"),
            "1f8afa3f6bccc27dc9f7f2d186539c84"
        );
    }

    #[tokio::test]
    async fn sends_now_playing_and_scrobble() {
        let (base_url, requests, server) = mock_server("{}").await;
        let client = client(base_url);

        client.now_playing(&track(), 1_700_000_000).await.unwrap();
        client.scrobble(&track(), 1_700_000_000).await.unwrap();

        assert!(wait_for(|| requests.lock().unwrap().len() == 2).await);
        let requests = requests.lock().unwrap();
        let now_playing = &requests[0];
        assert!(now_playing.contains("method=track.updateNowPlaying"));
        assert!(now_playing.contains("artist=Artist"));
        assert!(now_playing.contains("track=Title"));
        assert!(now_playing.contains("duration=180"));
        assert!(now_playing.contains("format=json"));

        let scrobble = &requests[1];
        assert!(scrobble.contains("method=track.scrobble"));
        assert!(scrobble.contains("timestamp=1700000000"));
        assert!(scrobble.contains("api_sig="));
        server.abort();
    }

    #[tokio::test]
    async fn queues_an_offline_scrobble_and_flushes_on_reconnect() {
        let (base_url, requests, server) =
            mock_server(r#"{"scrobbles":{"@attr":{"accepted":1}}}"#).await;
        let client = client(base_url);
        let offline = Arc::new(AtomicBool::new(true));
        let (sender, events) = broadcast::channel(16);
        start_worker(client, offline.clone(), Duration::from_millis(10), events);

        sender
            .send(ScrobbleEvent::Scrobble {
                track: track(),
                started_at: 1_700_000_000,
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(requests.lock().unwrap().is_empty());

        offline.store(false, Ordering::SeqCst);
        sender
            .send(ScrobbleEvent::NowPlaying {
                track: track(),
                started_at: 1_700_000_000,
            })
            .unwrap();

        assert!(
            wait_for(|| {
                let requests = requests.lock().unwrap();
                requests
                    .iter()
                    .any(|body| body.contains("method=track.scrobble"))
                    && requests
                        .iter()
                        .any(|body| body.contains("method=track.updateNowPlaying"))
            })
            .await
        );
        server.abort();
    }

    #[tokio::test]
    async fn retries_a_failed_scrobble() {
        // The first reply is an error, the second is a success.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let server = tokio::spawn(async move {
            for body in [
                r#"{"error":8,"message":"operation failed"}"#,
                r#"{"scrobbles":{"@attr":{"accepted":1}}}"#,
            ] {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let request = read_request(&mut socket).await;
                if let Some(value) = request.split("\r\n\r\n").nth(1) {
                    recorded.lock().unwrap().push(value.to_string());
                }
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        let client = client(format!("http://{address}/2.0/"));
        let (sender, events) = broadcast::channel(16);
        start_worker(
            client,
            Arc::new(AtomicBool::new(false)),
            Duration::from_millis(10),
            events,
        );

        sender
            .send(ScrobbleEvent::Scrobble {
                track: track(),
                started_at: 1_700_000_000,
            })
            .unwrap();

        assert!(
            wait_for(|| {
                let requests = requests.lock().unwrap();
                requests
                    .iter()
                    .filter(|body| body.contains("method=track.scrobble"))
                    .count()
                    >= 2
            })
            .await
        );
        server.abort();
    }
}
