//! HTTP and SSE client for the `ytermusicd` daemon.
//!
//! The client holds no playback state. It sends requests and returns the DTOs
//! from `ytermusicd-api`. The daemon is the only owner of state.

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::header::ACCEPT;
use std::fmt;
use std::pin::Pin;
use std::time::Duration;
use ytermusicd_api::{
    AddPlaylistTracksRequest, CreatePlaylistRequest, ErrorEnvelope, Event,
    MovePlaylistTrackRequest, OkResponse, PlayNextRequest, PlayPlaylistRequest, PlayTrackRequest,
    Playlist, PlaylistDetail, ProviderId, ProviderInfo, Queue, QueueRequest,
    RemovePlaylistTracksRequest, RemoveQueueRequest, ReorderQueueRequest, RepeatRequest,
    RetryDownloadRequest, SearchResults, SeekRequest, ShuffleRequest, Status, Track, TrackKey,
    UpdatePlaylistRequest, VolumeRequest,
};

/// Default daemon URL.
pub const DEFAULT_URL: &str = "http://127.0.0.1:8080";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const STREAM_TIMEOUT: Duration = Duration::from_secs(45);

/// HTTP client for the daemon. It holds no playback state.
#[derive(Debug, Clone)]
pub struct DaemonClient {
    base_url: String,
    http: reqwest::Client,
    stream_http: reqwest::Client,
}

/// Error returned by the daemon or the transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonError {
    /// Stable error code. Use it for program logic.
    pub code: String,
    /// Human-readable message. Show it to the user.
    pub message: String,
}

impl DaemonError {
    /// Create an error for an unreachable daemon.
    pub fn unavailable(base_url: &str, error: impl fmt::Display) -> Self {
        Self {
            code: "daemon_unavailable".to_string(),
            message: format!(
                "Cannot reach the daemon at {base_url}. Start it with: ytermusicd serve ({error})"
            ),
        }
    }

    /// Create an error from an HTTP error response.
    pub fn response(status: reqwest::StatusCode, envelope: Option<ErrorEnvelope>) -> Self {
        match envelope {
            Some(envelope) => Self {
                code: envelope.error.code,
                message: envelope.error.message,
            },
            None => Self {
                code: "http_error".to_string(),
                message: format!("the daemon returned HTTP {status}"),
            },
        }
    }

    /// Create an error for an invalid response body.
    pub fn invalid_response(error: impl fmt::Display) -> Self {
        Self {
            code: "invalid_response".to_string(),
            message: format!("the daemon returned an invalid response: {error}"),
        }
    }

    /// Create an error for a broken event stream.
    pub fn stream(error: impl fmt::Display) -> Self {
        Self {
            code: "event_stream_error".to_string(),
            message: format!("the daemon event stream failed: {error}"),
        }
    }

    /// Return true when the daemon is unreachable.
    pub fn is_unavailable(&self) -> bool {
        self.code == "daemon_unavailable"
    }
}

impl fmt::Display for DaemonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for DaemonError {}

impl DaemonClient {
    /// Create a client for `base_url`. A trailing slash is removed.
    pub fn new(base_url: impl Into<String>) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        let stream_http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http,
            stream_http,
        })
    }

    /// Return the base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, DaemonError> {
        self.request(reqwest::Method::GET, path, None).await
    }

    async fn post<T: serde::de::DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, DaemonError> {
        self.request(
            reqwest::Method::POST,
            path,
            Some(serde_json::to_value(body).map_err(DaemonError::invalid_response)?),
        )
        .await
    }

    async fn patch<T: serde::de::DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, DaemonError> {
        self.request(
            reqwest::Method::PATCH,
            path,
            Some(serde_json::to_value(body).map_err(DaemonError::invalid_response)?),
        )
        .await
    }

    async fn delete<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, DaemonError> {
        self.request(reqwest::Method::DELETE, path, None).await
    }

    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, DaemonError> {
        let response = self.send(method, path, body).await?;
        response
            .json::<T>()
            .await
            .map_err(DaemonError::invalid_response)
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<reqwest::Response, DaemonError> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base_url));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| DaemonError::unavailable(&self.base_url, error))?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let envelope = response.json::<ErrorEnvelope>().await.ok();
        Err(DaemonError::response(status, envelope))
    }

    async fn post_empty<B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<(), DaemonError> {
        let body = serde_json::to_value(body).map_err(DaemonError::invalid_response)?;
        self.send(reqwest::Method::POST, path, Some(body)).await?;
        Ok(())
    }

    /// Return the daemon status.
    pub async fn status(&self) -> Result<Status, DaemonError> {
        self.get("/api/status").await
    }

    /// Return every provider the daemon knows, with its capabilities
    /// and its auth state.
    pub async fn providers(&self) -> Result<Vec<ProviderInfo>, DaemonError> {
        self.get("/api/providers").await
    }

    /// Return all library playlists for one provider.
    pub async fn playlists(&self, provider: ProviderId) -> Result<Vec<Playlist>, DaemonError> {
        self.get(&format!("/api/playlists?provider={}", provider.as_str()))
            .await
    }

    /// Return one playlist and its tracks.
    pub async fn playlist(
        &self,
        provider: ProviderId,
        playlist_id: &str,
    ) -> Result<PlaylistDetail, DaemonError> {
        self.get(&format!(
            "/api/playlists/{}?provider={}",
            encode(playlist_id),
            provider.as_str()
        ))
        .await
    }

    /// Create a playlist.
    pub async fn create_playlist(
        &self,
        request: &CreatePlaylistRequest,
    ) -> Result<Playlist, DaemonError> {
        self.post("/api/playlists", request).await
    }

    /// Rename a playlist.
    pub async fn update_playlist(
        &self,
        playlist_id: &str,
        request: &UpdatePlaylistRequest,
    ) -> Result<Playlist, DaemonError> {
        self.patch(&format!("/api/playlists/{}", encode(playlist_id)), request)
            .await
    }

    /// Delete a playlist.
    pub async fn delete_playlist(&self, playlist_id: &str) -> Result<OkResponse, DaemonError> {
        self.delete(&format!("/api/playlists/{}", encode(playlist_id)))
            .await
    }

    /// Add tracks to a playlist.
    pub async fn add_playlist_tracks(
        &self,
        playlist_id: &str,
        provider: ProviderId,
        track_ids: Vec<String>,
    ) -> Result<Vec<Track>, DaemonError> {
        self.post(
            &format!("/api/playlists/{}/tracks", encode(playlist_id)),
            &AddPlaylistTracksRequest {
                provider,
                track_ids,
            },
        )
        .await
    }

    /// Remove tracks from a playlist.
    pub async fn remove_playlist_tracks(
        &self,
        playlist_id: &str,
        indices: Vec<usize>,
    ) -> Result<Vec<Track>, DaemonError> {
        self.post(
            &format!("/api/playlists/{}/tracks/remove", encode(playlist_id)),
            &RemovePlaylistTracksRequest { indices },
        )
        .await
    }

    /// Move a playlist track.
    pub async fn move_playlist_track(
        &self,
        playlist_id: &str,
        from: usize,
        to: usize,
    ) -> Result<Vec<Track>, DaemonError> {
        self.post(
            &format!("/api/playlists/{}/tracks/move", encode(playlist_id)),
            &MovePlaylistTrackRequest { from, to },
        )
        .await
    }

    /// Search tracks and playlists for one provider.
    pub async fn search(
        &self,
        provider: ProviderId,
        query: &str,
    ) -> Result<SearchResults, DaemonError> {
        self.get(&format!(
            "/api/search?provider={}&q={}",
            provider.as_str(),
            encode(query)
        ))
        .await
    }

    /// Return the queue snapshot.
    pub async fn queue(&self) -> Result<Queue, DaemonError> {
        self.get("/api/queue").await
    }

    /// Replace the queue with a playlist and start playback.
    pub async fn play_playlist(
        &self,
        provider: ProviderId,
        playlist_id: &str,
    ) -> Result<Status, DaemonError> {
        self.post(
            "/api/play/playlist",
            &PlayPlaylistRequest {
                playlist_id: playlist_id.to_string(),
                provider,
            },
        )
        .await
    }

    /// Play one track now.
    pub async fn play_track(&self, track: Track) -> Result<Status, DaemonError> {
        self.post("/api/play/track", &PlayTrackRequest { track })
            .await
    }

    /// Add one track to the queue.
    pub async fn queue_add(&self, track: Track) -> Result<Queue, DaemonError> {
        self.post("/api/queue", &QueueRequest { track }).await
    }

    /// Remove one queue item.
    pub async fn queue_remove(&self, index: usize) -> Result<Queue, DaemonError> {
        self.post("/api/queue/remove", &RemoveQueueRequest { index })
            .await
    }

    /// Move one queue item.
    pub async fn queue_reorder(&self, from: usize, to: usize) -> Result<Queue, DaemonError> {
        self.post("/api/queue/reorder", &ReorderQueueRequest { from, to })
            .await
    }

    /// Move one queue item to play next.
    pub async fn queue_play_next(&self, index: usize) -> Result<Queue, DaemonError> {
        self.post("/api/queue/play-next", &PlayNextRequest { index })
            .await
    }

    /// Remove all queue items.
    pub async fn queue_clear(&self) -> Result<Queue, DaemonError> {
        self.delete("/api/queue").await
    }

    /// Pause playback.
    pub async fn pause(&self) -> Result<Status, DaemonError> {
        self.post("/api/pause", &serde_json::json!({})).await
    }

    /// Resume playback.
    pub async fn resume(&self) -> Result<Status, DaemonError> {
        self.post("/api/resume", &serde_json::json!({})).await
    }

    /// Skip to the next track.
    pub async fn next(&self) -> Result<Status, DaemonError> {
        self.post("/api/next", &serde_json::json!({})).await
    }

    /// Skip to the previous track.
    pub async fn previous(&self) -> Result<Status, DaemonError> {
        self.post("/api/previous", &serde_json::json!({})).await
    }

    /// Set the volume from 0 to 100.
    pub async fn volume(&self, volume: u8) -> Result<Status, DaemonError> {
        self.post("/api/volume", &VolumeRequest { volume }).await
    }

    /// Seek to a position in seconds.
    pub async fn seek(&self, position_secs: f64) -> Result<Status, DaemonError> {
        self.post("/api/seek", &SeekRequest { position_secs }).await
    }

    /// Enable or disable shuffle.
    pub async fn shuffle(&self, enabled: bool) -> Result<Status, DaemonError> {
        self.post("/api/shuffle", &ShuffleRequest { enabled }).await
    }

    /// Set the repeat mode.
    pub async fn repeat(&self, repeat: ytermusicd_api::RepeatMode) -> Result<Status, DaemonError> {
        self.post("/api/repeat", &RepeatRequest { repeat }).await
    }

    /// Retry a failed download.
    pub async fn retry_download(&self, key: &TrackKey) -> Result<(), DaemonError> {
        self.post_empty(
            "/api/downloads/retry",
            &RetryDownloadRequest {
                provider: key.provider,
                track_id: key.id.clone(),
            },
        )
        .await
    }

    /// Return the cover URL for a track.
    pub fn cover_url(&self, key: &TrackKey) -> String {
        format!(
            "{}/api/tracks/{}/{}/cover",
            self.base_url,
            encode(key.provider.as_str()),
            encode(&key.id)
        )
    }

    /// Fetch cover art as JPEG bytes.
    pub async fn cover(&self, key: &TrackKey) -> Result<Vec<u8>, DaemonError> {
        let response = self
            .http
            .get(self.cover_url(key))
            .send()
            .await
            .map_err(|error| DaemonError::unavailable(&self.base_url, error))?;
        if !response.status().is_success() {
            let status = response.status();
            let envelope = response.json::<ErrorEnvelope>().await.ok();
            return Err(DaemonError::response(status, envelope));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(DaemonError::invalid_response)
    }

    /// Open the daemon event stream.
    ///
    /// Send the last seen event id to replay missed events. The daemon sends a
    /// fresh snapshot when the id is outside the replay window.
    pub async fn events(&self, last_id: Option<u64>) -> Result<EventStream, DaemonError> {
        let mut request = self
            .stream_http
            .get(format!("{}/api/events", self.base_url))
            .header(ACCEPT, "text/event-stream");
        if let Some(id) = last_id {
            request = request.header("Last-Event-ID", id.to_string());
        }
        let response = request
            .send()
            .await
            .map_err(|error| DaemonError::unavailable(&self.base_url, error))?;
        let status = response.status();
        if !status.is_success() {
            let envelope = response.json::<ErrorEnvelope>().await.ok();
            return Err(DaemonError::response(status, envelope));
        }
        Ok(EventStream {
            inner: Box::pin(response.bytes_stream()),
            buffer: Vec::new(),
            done: false,
        })
    }
}

/// Parsed SSE stream from `/api/events`.
pub struct EventStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: Vec<u8>,
    done: bool,
}

impl EventStream {
    /// Return the next daemon event. Return `None` when the stream ends.
    pub async fn next(&mut self) -> Option<Result<Event, DaemonError>> {
        loop {
            if let Some((end, delimiter_len)) = find_event_end(&self.buffer) {
                let block: Vec<u8> = self.buffer.drain(..end + delimiter_len).collect();
                let block = &block[..end];
                let Ok(text) = std::str::from_utf8(block) else {
                    self.buffer.clear();
                    return Some(Err(DaemonError::invalid_response(
                        "the event stream is not valid UTF-8",
                    )));
                };
                match parse_event(text) {
                    Some(Ok(event)) => return Some(Ok(event)),
                    Some(Err(error)) => return Some(Err(error)),
                    None => continue,
                }
            }
            if self.done {
                return None;
            }
            match tokio::time::timeout(STREAM_TIMEOUT, self.inner.next()).await {
                Ok(Some(Ok(bytes))) => self.buffer.extend_from_slice(&bytes),
                Ok(Some(Err(error))) => {
                    self.done = true;
                    return Some(Err(DaemonError::stream(error)));
                }
                Ok(None) => {
                    self.done = true;
                }
                Err(_) => {
                    self.done = true;
                    return Some(Err(DaemonError::stream(
                        "the daemon sent no keepalive for 45 seconds",
                    )));
                }
            }
        }
    }
}

fn find_event_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for delimiter in [b"\n\n".as_slice(), b"\r\n\r\n".as_slice()] {
        if let Some(position) = find_bytes(buffer, delimiter) {
            best = Some(match best {
                Some((current, current_len)) if current <= position => (current, current_len),
                _ => (position, delimiter.len()),
            });
        }
    }
    best
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn parse_event(block: &str) -> Option<Result<Event, DaemonError>> {
    let mut data = String::new();
    let mut has_data = false;
    for line in block.lines() {
        if line.starts_with(':') || line.is_empty() {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        if field == "data" {
            if has_data {
                data.push('\n');
            }
            data.push_str(value);
            has_data = true;
        }
    }
    if !has_data {
        return None;
    }
    Some(serde_json::from_str::<Event>(&data).map_err(DaemonError::invalid_response))
}

/// Percent-encode a path segment or query value.
pub fn encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        extract::State,
        http::{HeaderMap, StatusCode, header::CONTENT_TYPE},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use std::sync::{Arc, Mutex};
    use ytermusicd_api::{
        ApiError, AuthState, AuthStatus, DownloadStatus, DownloadSummary, ErrorEnvelope, EventKind,
        PlaybackSnapshot, PlaybackState, QueueItem, RepeatMode,
    };

    #[derive(Clone, Default)]
    struct MockState {
        calls: Arc<Mutex<Vec<String>>>,
        last_event_id: Arc<Mutex<Option<String>>>,
        fail: Arc<std::sync::atomic::AtomicBool>,
    }

    fn track(id: &str) -> Track {
        Track {
            provider: ProviderId::Youtube,
            track_id: id.to_string(),
            title: format!("Track {id}"),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
        }
    }

    fn status() -> Status {
        Status {
            name: "ytermusicd".to_string(),
            version: "0.2.0".to_string(),
            status: "ok".to_string(),
            uptime_seconds: 1,
            auth: AuthStatus {
                state: AuthState::Ok,
                source: None,
                message: None,
            },
            playback: PlaybackSnapshot {
                state: PlaybackState::Playing,
                current: Some(track("v1")),
                position_secs: 12.0,
                volume: 50,
                repeat: RepeatMode::Off,
                shuffle: false,
            },
            queue_length: 1,
            downloads: DownloadSummary::default(),
            providers: vec![ProviderId::Youtube],
            default_provider: ProviderId::Youtube,
        }
    }

    fn queue() -> Queue {
        Queue {
            items: vec![QueueItem {
                track: track("v1"),
                download: DownloadStatus::Downloaded,
            }],
            current: Some(0),
            repeat: RepeatMode::Off,
            shuffle: false,
        }
    }

    async fn mock_router(state: MockState) -> (String, tokio::task::JoinHandle<()>) {
        let router = Router::new()
            .route("/api/status", get(mock_status))
            .route("/api/providers", get(mock_providers))
            .route("/api/playlists", get(mock_playlists))
            .route("/api/search", get(mock_search))
            .route("/api/queue/remove", post(mock_queue_remove))
            .route("/api/downloads/retry", post(mock_retry))
            .route("/api/events", get(mock_events))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{addr}"), handle)
    }

    async fn mock_providers(State(state): State<MockState>) -> Response {
        state
            .calls
            .lock()
            .unwrap()
            .push("GET /api/providers".to_string());
        axum::Json(vec![ProviderInfo::new(
            ProviderId::Youtube,
            AuthStatus {
                state: AuthState::Ok,
                source: None,
                message: None,
            },
        )])
        .into_response()
    }

    async fn mock_playlists(
        State(state): State<MockState>,
        axum::extract::Query(query): axum::extract::Query<
            std::collections::HashMap<String, String>,
        >,
    ) -> Response {
        state.calls.lock().unwrap().push(format!(
            "GET /api/playlists?provider={}",
            query.get("provider").map(String::as_str).unwrap_or("")
        ));
        axum::Json(vec![Playlist {
            id: "p1".to_string(),
            name: "Playlist One".to_string(),
            subtitle: "Artist".to_string(),
        }])
        .into_response()
    }

    async fn mock_search(
        State(state): State<MockState>,
        axum::extract::Query(query): axum::extract::Query<
            std::collections::HashMap<String, String>,
        >,
    ) -> Response {
        state.calls.lock().unwrap().push(format!(
            "GET /api/search?provider={}&q={}",
            query.get("provider").map(String::as_str).unwrap_or(""),
            query.get("q").map(String::as_str).unwrap_or("")
        ));
        axum::Json(SearchResults {
            tracks: vec![track("v3")],
            playlists: vec![],
        })
        .into_response()
    }

    async fn mock_status(State(state): State<MockState>) -> Response {
        state
            .calls
            .lock()
            .unwrap()
            .push("GET /api/status".to_string());
        if state.fail.load(std::sync::atomic::Ordering::SeqCst) {
            return (
                StatusCode::BAD_GATEWAY,
                axum::Json(ErrorEnvelope {
                    error: ApiError {
                        code: "upstream_error".to_string(),
                        message: "YouTube Music rejected the request".to_string(),
                    },
                }),
            )
                .into_response();
        }
        axum::Json(status()).into_response()
    }

    async fn mock_queue_remove(
        State(state): State<MockState>,
        axum::Json(request): axum::Json<RemoveQueueRequest>,
    ) -> impl IntoResponse {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST /api/queue/remove {}", request.index));
        axum::Json(queue())
    }

    async fn mock_retry(
        State(state): State<MockState>,
        axum::Json(request): axum::Json<RetryDownloadRequest>,
    ) -> impl IntoResponse {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST /api/downloads/retry {}", request.track_id));
        StatusCode::ACCEPTED
    }

    async fn mock_events(State(state): State<MockState>, headers: HeaderMap) -> Response {
        let last_id = headers
            .get("last-event-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        *state.last_event_id.lock().unwrap() = last_id;
        let event = Event {
            id: 8,
            event: EventKind::LibraryChanged,
        };
        let body = format!(
            ": keepalive\n\nid: 8\nevent: library_changed\ndata: {}\n\n",
            serde_json::to_string(&event).unwrap()
        );
        Response::builder()
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn provider_calls_send_the_provider() {
        let state = MockState::default();
        let (base, handle) = mock_router(state.clone()).await;
        let client = DaemonClient::new(base).unwrap();
        let providers = client.providers().await.unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, ProviderId::Youtube);
        assert_eq!(providers[0].name, "YouTube Music");
        assert!(providers[0].default);
        let playlists = client.playlists(ProviderId::Youtube).await.unwrap();
        assert_eq!(playlists[0].id, "p1");
        let results = client.search(ProviderId::Youtube, "test").await.unwrap();
        assert_eq!(results.tracks[0].track_id, "v3");
        let calls = state.calls.lock().unwrap();
        assert!(calls.contains(&"GET /api/providers".to_string()));
        assert!(calls.contains(&"GET /api/playlists?provider=youtube".to_string()));
        assert!(calls.contains(&"GET /api/search?provider=youtube&q=test".to_string()));
        handle.abort();
    }

    #[tokio::test]
    async fn returns_status_and_maps_errors() {
        let state = MockState::default();
        let (base, handle) = mock_router(state.clone()).await;
        let client = DaemonClient::new(base).unwrap();
        let status = client.status().await.unwrap();
        assert_eq!(status.name, "ytermusicd");
        assert!(
            state
                .calls
                .lock()
                .unwrap()
                .contains(&"GET /api/status".to_string())
        );
        handle.abort();
    }

    #[tokio::test]
    async fn maps_an_error_envelope() {
        let state = MockState::default();
        state.fail.store(true, std::sync::atomic::Ordering::SeqCst);
        let (base, handle) = mock_router(state).await;
        let client = DaemonClient::new(base).unwrap();
        let error = client.status().await.unwrap_err();
        assert_eq!(error.code, "upstream_error");
        assert_eq!(error.message, "YouTube Music rejected the request");
        handle.abort();
    }

    #[tokio::test]
    async fn sends_queue_mutations_and_accepts_empty_success() {
        let state = MockState::default();
        let (base, handle) = mock_router(state.clone()).await;
        let client = DaemonClient::new(base).unwrap();
        let queue = client.queue_remove(2).await.unwrap();
        assert_eq!(queue.items.len(), 1);
        client
            .retry_download(&TrackKey::youtube("v9"))
            .await
            .unwrap();
        let calls = state.calls.lock().unwrap();
        assert!(calls.contains(&"POST /api/queue/remove 2".to_string()));
        assert!(calls.contains(&"POST /api/downloads/retry v9".to_string()));
        handle.abort();
    }

    #[tokio::test]
    async fn reads_sse_events_and_sends_the_last_id() {
        let state = MockState::default();
        let (base, handle) = mock_router(state.clone()).await;
        let client = DaemonClient::new(base).unwrap();
        let mut events = client.events(Some(7)).await.unwrap();
        let event = events.next().await.unwrap().unwrap();
        assert_eq!(event.id, 8);
        assert_eq!(*state.last_event_id.lock().unwrap(), Some("7".to_string()));
        assert!(events.next().await.is_none());
        handle.abort();
    }

    #[test]
    fn parses_multiline_data_and_keeps_comments_out() {
        let event = parse_event(
            ": ping\ndata: {\"id\": 1,\ndata: \"event\": {\"type\": \"library_changed\"}}",
        )
        .unwrap()
        .unwrap();
        assert_eq!(event.id, 1);
        assert!(matches!(event.event, EventKind::LibraryChanged));
    }

    #[test]
    fn finds_both_line_endings() {
        assert_eq!(find_event_end(b"a\n\nb"), Some((1, 2)));
        assert_eq!(find_event_end(b"a\r\n\r\nb"), Some((1, 4)));
    }

    #[test]
    fn encodes_path_segments() {
        assert_eq!(encode("VLPL 123/x"), "VLPL%20123%2Fx");
    }
}
