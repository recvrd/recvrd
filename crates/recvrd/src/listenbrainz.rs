//! ListenBrainz scrobble worker.
//!
//! The worker drains play events from the playback service and posts
//! them to ListenBrainz. It never blocks playback. A scrobble that
//! fails or arrives while offline stays in memory and goes out after a
//! retry delay.
//!
//! ListenBrainz needs one user token. The daemon never ships a token.

use crate::scrobble::duration_secs;
use recvrd_core::{ListenBrainzConfig, ScrobbleEvent, Track};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::broadcast;

/// The ListenBrainz submit endpoint.
const API_URL: &str = "https://api.listenbrainz.org/1/submit-listens";
/// Delay before a failed call or an offline queue tries again.
const RETRY_DELAY: Duration = Duration::from_secs(30);
/// Maximum queued scrobbles. The oldest leaves when the queue is full.
const MAX_QUEUED: usize = 1000;
/// Request timeout. ListenBrainz answers fast, or the call is a retry.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Build a scrobble worker when the config enables it. Return true
/// when the worker starts.
pub fn start(
    config: &ListenBrainzConfig,
    offline: Arc<AtomicBool>,
    events: broadcast::Receiver<ScrobbleEvent>,
) -> bool {
    if !config.enabled {
        return false;
    }
    let Some(token) = config.token.as_deref().filter(|value| !value.is_empty()) else {
        log::warn!("ListenBrainz scrobbling is on, but the user token is missing");
        return false;
    };
    log::info!("ListenBrainz scrobbling is on");
    start_worker(
        ListenBrainzClient::new(API_URL.to_string(), token.to_string()),
        offline,
        RETRY_DELAY,
        events,
    );
    true
}

/// Start the worker with a client and a retry delay. Tests use a
/// local server and a short delay.
fn start_worker(
    client: ListenBrainzClient,
    offline: Arc<AtomicBool>,
    retry_delay: Duration,
    events: broadcast::Receiver<ScrobbleEvent>,
) {
    tokio::spawn(
        ListenBrainzWorker {
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

struct ListenBrainzWorker {
    client: ListenBrainzClient,
    offline: Arc<AtomicBool>,
    events: broadcast::Receiver<ScrobbleEvent>,
    queue: VecDeque<QueuedScrobble>,
    retry_delay: Duration,
    retry_at: Option<Instant>,
}

impl ListenBrainzWorker {
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
                    log::warn!("the ListenBrainz worker lagged by {count} events");
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => return,
                Err(_) => {}
            }
        }
    }

    async fn handle(&mut self, event: ScrobbleEvent) {
        match event {
            ScrobbleEvent::NowPlaying { track, .. } => {
                if self.offline.load(Ordering::SeqCst) {
                    return;
                }
                if let Err(error) = self.client.now_playing(&track).await {
                    log::warn!("ListenBrainz now-playing failed: {error}");
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
                        log::warn!("ListenBrainz rejected a scrobble: {message}");
                    }
                    Err(ScrobbleError::Retry(message)) => {
                        log::warn!("ListenBrainz scrobble failed: {message}");
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
                    log::warn!("ListenBrainz rejected a queued scrobble: {message}");
                }
                Err(ScrobbleError::Retry(message)) => {
                    log::warn!("ListenBrainz queued scrobble failed: {message}");
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
            log::warn!("the ListenBrainz scrobble queue is full; dropped the oldest play");
        }
        self.queue.push_back((track, started_at));
    }
}

/// A failure from a ListenBrainz call.
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

/// A ListenBrainz API client.
struct ListenBrainzClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl ListenBrainzClient {
    fn new(base_url: String, token: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            http,
            base_url,
            token,
        }
    }

    async fn now_playing(&self, track: &Track) -> Result<(), ScrobbleError> {
        self.submit("playing_now", None, track).await
    }

    async fn scrobble(&self, track: &Track, started_at: u64) -> Result<(), ScrobbleError> {
        self.submit("single", Some(started_at), track).await
    }

    async fn submit(
        &self,
        listen_type: &str,
        listened_at: Option<u64>,
        track: &Track,
    ) -> Result<(), ScrobbleError> {
        let mut listen = json!({ "track_metadata": track_metadata(track)? });
        if let Some(started_at) = listened_at {
            listen["listened_at"] = json!(started_at);
        }
        let body = json!({ "listen_type": listen_type, "payload": [listen] });
        let response = self
            .http
            .post(&self.base_url)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Token {}", self.token),
            )
            .json(&body)
            .send()
            .await
            .map_err(|error| ScrobbleError::Retry(error.to_string()))?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or_default();
        if status.is_success() {
            if body
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|value| value != "ok")
            {
                return Err(ScrobbleError::Permanent(format!(
                    "ListenBrainz returned {body}"
                )));
            }
            return Ok(());
        }
        let message = format!("HTTP {status}");
        Err(
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                ScrobbleError::Retry(message)
            } else {
                ScrobbleError::Permanent(message)
            },
        )
    }
}

/// The track fields for a ListenBrainz call. ListenBrainz needs an
/// artist and a title; a track without them cannot scrobble.
fn track_metadata(track: &Track) -> Result<Value, ScrobbleError> {
    if track.author.trim().is_empty() || track.title.trim().is_empty() {
        return Err(ScrobbleError::Permanent(
            "the track has no artist or title".to_string(),
        ));
    }
    let mut additional = json!({ "submission_client": "recvrd" });
    if let Some(duration) = duration_secs(&track.duration) {
        additional["duration_ms"] = json!(duration * 1000);
    }
    let mut metadata = json!({
        "artist_name": track.author,
        "track_name": track.title,
        "additional_info": additional,
    });
    if !track.album.trim().is_empty() {
        metadata["release_name"] = json!(track.album);
    }
    Ok(metadata)
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
    /// requests. The server stops when the task aborts.
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
                recorded.lock().unwrap().push(request);
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        (
            format!("http://{address}/1/submit-listens"),
            requests,
            handle,
        )
    }

    fn client(base_url: String) -> ListenBrainzClient {
        ListenBrainzClient::new(base_url, "token-value".to_string())
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

    fn body_of(request: &str) -> Value {
        let body = request.split("\r\n\r\n").nth(1).expect("body");
        serde_json::from_str(body).expect("JSON body")
    }

    #[tokio::test]
    async fn sends_playing_now_and_single_listen() {
        let (base_url, requests, server) = mock_server(r#"{"status":"ok"}"#).await;
        let client = client(base_url);

        client.now_playing(&track()).await.unwrap();
        client.scrobble(&track(), 1_700_000_000).await.unwrap();

        assert!(wait_for(|| requests.lock().unwrap().len() == 2).await);
        let requests = requests.lock().unwrap();
        assert!(requests[0].contains("authorization: Token token-value"));

        let now_playing = body_of(&requests[0]);
        assert_eq!(now_playing["listen_type"], "playing_now");
        assert!(now_playing["payload"][0].get("listened_at").is_none());
        assert_eq!(
            now_playing["payload"][0]["track_metadata"]["artist_name"],
            "Artist"
        );
        assert_eq!(
            now_playing["payload"][0]["track_metadata"]["track_name"],
            "Title"
        );
        assert_eq!(
            now_playing["payload"][0]["track_metadata"]["release_name"],
            "Album"
        );
        assert_eq!(
            now_playing["payload"][0]["track_metadata"]["additional_info"]["duration_ms"],
            180000
        );
        assert_eq!(
            now_playing["payload"][0]["track_metadata"]["additional_info"]["submission_client"],
            "recvrd"
        );

        let single = body_of(&requests[1]);
        assert_eq!(single["listen_type"], "single");
        assert_eq!(single["payload"][0]["listened_at"], 1_700_000_000);
        server.abort();
    }

    #[tokio::test]
    async fn queues_an_offline_scrobble_and_flushes_on_reconnect() {
        let (base_url, requests, server) = mock_server(r#"{"status":"ok"}"#).await;
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
                requests.iter().any(|request| {
                    let body = body_of(request);
                    body["listen_type"] == "single"
                }) && requests.iter().any(|request| {
                    let body = body_of(request);
                    body["listen_type"] == "playing_now"
                })
            })
            .await
        );
        server.abort();
    }

    #[tokio::test]
    async fn retries_a_rate_limited_scrobble() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let server = tokio::spawn(async move {
            for (status, body) in [
                (
                    "429 Too Many Requests",
                    r#"{"code":429,"error":"rate limit"}"#,
                ),
                ("200 OK", r#"{"status":"ok"}"#),
            ] {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let request = read_request(&mut socket).await;
                recorded.lock().unwrap().push(request);
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        let client = client(format!("http://{address}/1/submit-listens"));
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
                requests
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|request| body_of(request)["listen_type"] == "single")
                    .count()
                    >= 2
            })
            .await
        );
        server.abort();
    }
}
