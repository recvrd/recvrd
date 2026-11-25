//! Queue position sync with the Podverse API.
//!
//! The local position map stays the source of truth. This client
//! only pushes local positions to the account queue and reads the
//! remote now-playing row. Any failure leaves local state alone.
//!
//! Verified API surface (Podverse API source, routes `queue.ts`,
//! controllers `queue/*`, ORM `queueResource.ts`):
//! - `POST /api/v2/queue/{queue_id_text}/item/{item_id_text}/now-playing`
//! - `POST /api/v2/queue/{queue_id_text}/item/{item_id_text}/history`
//! - `POST /api/v2/queue/{queue_id_text}/item-add-by-rss/now-playing`
//! - `POST /api/v2/queue/{queue_id_text}/item-add-by-rss/history`
//! - `GET /api/v2/queue/{queue_id_text}/resources/now-playing`
//! - `GET /api/v2/queue/all-for-account/private`
//!
//! Bodies carry `playback_position`, `media_file_duration`, and
//! `completed`. Writes need a signed-in account with an unexpired
//! membership. Reads need a signed-in account only. The API has no
//! create-queue endpoint, so sync selects an existing queue.

use crate::client::{PodverseClient, PodverseError};
use reqwest::{StatusCode, Url};
use serde::{Deserialize, Deserializer, Serialize};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::Mutex;

/// Timeout for one sync request. Sync never blocks playback.
const SYNC_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum wait after a rate limit response, in seconds.
const MAX_RETRY_AFTER_SECS: u64 = 60;

/// Default wait after a rate limit response, in seconds.
const DEFAULT_RETRY_AFTER_SECS: u64 = 5;

/// A position to write to the Podverse queue. Positions use seconds.
#[derive(Debug, Clone, Serialize)]
pub struct SyncBody {
    pub playback_position: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_file_duration: Option<f64>,
    pub completed: bool,
}

impl SyncBody {
    /// Build a body from a position, an optional duration, and a flag
    /// that marks the track as finished.
    pub fn new(position: Duration, duration: Option<Duration>, completed: bool) -> Self {
        Self {
            playback_position: position.as_secs_f64(),
            media_file_duration: duration.map(|duration| duration.as_secs_f64()),
            completed,
        }
    }
}

/// A queue item that a position sync targets.
#[derive(Debug, Clone)]
pub enum SyncTarget {
    /// A Podverse item, by its ID text.
    Item(String),
    /// An RSS episode, by its feed URL and enclosure URL.
    AddByRss {
        feed_url: String,
        enclosure_url: String,
    },
}

/// The add-by-RSS resource data that the API stores with a row.
#[derive(Debug, Clone, Serialize)]
pub struct AddByRssData {
    pub feed_url: String,
    pub enclosure_url: String,
}

/// Build the add-by-RSS resource data for an RSS episode.
/// The feed URL loses its userinfo, so the hash stays stable.
pub fn add_by_rss_resource_data(feed_url: &str, enclosure_url: &str) -> AddByRssData {
    AddByRssData {
        feed_url: canonical_feed_url(feed_url),
        enclosure_url: enclosure_url.to_string(),
    }
}

/// Compute the hash the API stores for an add-by-RSS row. The API
/// hashes `{"feed_url":...,"enclosure_url":...}` with MD5. Key order
/// is part of the hash, so build the string in that order.
pub fn add_by_rss_hash_id(feed_url: &str, enclosure_url: &str) -> String {
    let data = add_by_rss_resource_data(feed_url, enclosure_url);
    let json = format!(
        "{{\"feed_url\":{},\"enclosure_url\":{}}}",
        serde_json::to_string(&data.feed_url).unwrap_or_default(),
        serde_json::to_string(&data.enclosure_url).unwrap_or_default()
    );
    format!("{:x}", md5::compute(json))
}

/// Strip userinfo from a feed URL. An unparsable URL passes through.
fn canonical_feed_url(feed_url: &str) -> String {
    let Ok(mut url) = Url::parse(feed_url) else {
        return feed_url.to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.to_string()
}

/// A number that the API may send as a number or a string.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum NumberOrString {
    Number(f64),
    Text(String),
}

/// Read an optional number from a number or a string field.
fn opt_number<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<NumberOrString>::deserialize(deserializer)?;
    Ok(match value {
        None => None,
        Some(NumberOrString::Number(number)) => Some(number),
        Some(NumberOrString::Text(text)) => text.parse().ok(),
    })
}

/// The remote now-playing row for a queue.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RemotePosition {
    /// The linked item, when the row is a Podverse item.
    #[serde(default)]
    pub item: Option<RemoteItem>,
    /// The item ID text, when the API sends it at top level.
    #[serde(default)]
    pub item_id_text: Option<String>,
    /// The add-by-RSS hash, when the row is an RSS episode.
    #[serde(default)]
    pub add_by_rss_hash_id: Option<String>,
    /// The remote position, in seconds.
    #[serde(default, deserialize_with = "opt_number")]
    pub playback_position: Option<f64>,
    /// The remote media duration, in seconds.
    #[serde(default, deserialize_with = "opt_number")]
    pub media_file_duration: Option<f64>,
    /// True when the remote side marked the track as finished.
    #[serde(default)]
    pub completed: bool,
    /// The last write time, as an ISO 8601 timestamp.
    #[serde(default)]
    pub last_played_at: Option<String>,
}

/// The item linked to a remote queue row.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RemoteItem {
    #[serde(default)]
    pub id_text: Option<String>,
}

impl RemotePosition {
    /// Return the Podverse item ID text for this row, if any.
    pub fn item_id(&self) -> Option<&str> {
        self.item_id_text
            .as_deref()
            .or_else(|| self.item.as_ref().and_then(|item| item.id_text.as_deref()))
    }

    /// Return the remote position as a duration, if the row has one.
    pub fn position(&self) -> Option<Duration> {
        self.playback_position.map(Duration::from_secs_f64)
    }

    /// Return the remote write time as Unix seconds, if it parses.
    pub fn last_played_unix(&self) -> Option<u64> {
        self.last_played_at.as_deref().and_then(parse_iso8601_unix)
    }
}

/// Parse an ISO 8601 timestamp to Unix seconds. The parser accepts
/// `YYYY-MM-DDTHH:MM:SS` with an optional fraction and a `Z` or a
/// numeric offset.
fn parse_iso8601_unix(value: &str) -> Option<u64> {
    let (date, time) = value.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let (time, offset_secs) = match time.strip_suffix('Z') {
        Some(time) => (time, 0i64),
        None => {
            let index = time.rfind(['+', '-'])?;
            let sign = &time[index..index + 1];
            let rest = &time[index + 1..];
            let (hours, minutes) = rest.split_once(':')?;
            let hours: i64 = hours.parse().ok()?;
            let minutes: i64 = minutes.parse().ok()?;
            let offset = hours * 3600 + minutes * 60;
            let offset = if sign == "-" { -offset } else { offset };
            (&time[..time.len() - rest.len() - 1], offset)
        }
    };
    let time = time.split('.').next()?;
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Days from civil, Howard Hinnant's algorithm.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs;
    u64::try_from(seconds).ok()
}

/// One queue from the account queue list.
#[derive(Debug, Clone, Deserialize)]
struct QueueInfo {
    #[serde(default)]
    id_text: String,
    #[serde(default)]
    is_active_queue: bool,
}

/// A Podverse sync failure. Every variant leaves local state alone.
#[derive(Debug, Error)]
pub enum SyncError {
    #[error("Podverse sync request timed out")]
    Timeout,
    #[error("Podverse sync request failed: {0}")]
    Transport(String),
    #[error("Podverse account is not signed in")]
    Unauthorized,
    #[error("Podverse membership is missing or expired")]
    Membership,
    #[error("Podverse queue was not found")]
    QueueNotFound,
    #[error("Podverse asked to wait {0} seconds")]
    RateLimited(u64),
    #[error("Podverse server error: {0}")]
    Server(String),
    #[error("Podverse sync response was invalid: {0}")]
    Invalid(String),
}

impl From<PodverseError> for SyncError {
    fn from(error: PodverseError) -> Self {
        match error {
            PodverseError::Request(error) => SyncError::Transport(error.to_string()),
            PodverseError::Config(message) => SyncError::Invalid(message),
        }
    }
}

/// The Podverse queue sync client. It selects one account queue and
/// pushes positions to it. It never creates a queue: the API has no
/// create-queue endpoint.
pub struct QueueSyncClient {
    client: PodverseClient,
    queue_id: Mutex<Option<String>>,
}

impl QueueSyncClient {
    /// Build a sync client on a Podverse client. The client must
    /// carry a bearer token, or every call fails with `Unauthorized`.
    pub fn new(client: PodverseClient) -> Self {
        Self {
            client,
            queue_id: Mutex::new(None),
        }
    }

    /// Select the account queue. Prefer the active queue. Fall back
    /// to the first queue. Cache the choice for later calls.
    pub async fn ensure_queue(&self) -> Result<String, SyncError> {
        if let Some(queue_id) = self.queue_id.lock().await.clone() {
            return Ok(queue_id);
        }
        let response = tokio::time::timeout(
            SYNC_TIMEOUT,
            self.client.get_response("/queue/all-for-account/private"),
        )
        .await
        .map_err(|_| SyncError::Timeout)??;
        let status = response.status();
        if let Some(error) = map_status(&response, status) {
            return Err(error);
        }
        let queues: Vec<QueueInfo> = response
            .json()
            .await
            .map_err(|error| SyncError::Invalid(error.to_string()))?;
        let queue = queues
            .iter()
            .find(|queue| queue.is_active_queue)
            .or_else(|| queues.first())
            .ok_or(SyncError::QueueNotFound)?;
        let queue_id = queue.id_text.clone();
        *self.queue_id.lock().await = Some(queue_id.clone());
        Ok(queue_id)
    }

    /// Push a position to the now-playing row for a target.
    pub async fn push_now_playing(
        &self,
        target: &SyncTarget,
        body: &SyncBody,
    ) -> Result<(), SyncError> {
        self.post_position(target, body, "now-playing").await
    }

    /// Push a position to the history row for a target. Call this
    /// when a track finishes.
    pub async fn push_history(
        &self,
        target: &SyncTarget,
        body: &SyncBody,
    ) -> Result<(), SyncError> {
        self.post_position(target, body, "history").await
    }

    /// Read the remote now-playing row for the account queue.
    /// `None` means the queue has no now-playing row.
    pub async fn fetch_now_playing(&self) -> Result<Option<RemotePosition>, SyncError> {
        let queue_id = self.ensure_queue().await?;
        let path = format!("/queue/{queue_id}/resources/now-playing");
        let response = tokio::time::timeout(SYNC_TIMEOUT, self.client.get_response(&path))
            .await
            .map_err(|_| SyncError::Timeout)??;
        let status = response.status();
        if let Some(error) = map_status(&response, status) {
            if matches!(error, SyncError::QueueNotFound) {
                *self.queue_id.lock().await = None;
            }
            return Err(error);
        }
        let text = response
            .text()
            .await
            .map_err(|error| SyncError::Invalid(error.to_string()))?;
        if text.trim() == "null" || text.trim().is_empty() {
            return Ok(None);
        }
        let row: RemotePosition =
            serde_json::from_str(&text).map_err(|error| SyncError::Invalid(error.to_string()))?;
        Ok(Some(row))
    }

    async fn post_position(
        &self,
        target: &SyncTarget,
        body: &SyncBody,
        zone: &str,
    ) -> Result<(), SyncError> {
        let queue_id = self.ensure_queue().await?;
        let path = match target {
            SyncTarget::Item(item_id) => {
                format!("/queue/{queue_id}/item/{item_id}/{zone}")
            }
            SyncTarget::AddByRss { .. } => {
                format!("/queue/{queue_id}/item-add-by-rss/{zone}")
            }
        };
        let response = match target {
            SyncTarget::Item(_) => {
                tokio::time::timeout(SYNC_TIMEOUT, self.client.post_json(&path, body))
                    .await
                    .map_err(|_| SyncError::Timeout)??
            }
            SyncTarget::AddByRss {
                feed_url,
                enclosure_url,
            } => {
                let rss_body = RssSyncBody {
                    body,
                    add_by_rss_resource_data: add_by_rss_resource_data(feed_url, enclosure_url),
                };
                tokio::time::timeout(SYNC_TIMEOUT, self.client.post_json(&path, &rss_body))
                    .await
                    .map_err(|_| SyncError::Timeout)??
            }
        };
        let status = response.status();
        if let Some(error) = map_status(&response, status) {
            if matches!(error, SyncError::QueueNotFound) {
                *self.queue_id.lock().await = None;
            }
            return Err(error);
        }
        Ok(())
    }
}

/// The add-by-RSS write body: the position fields plus the
/// resource data that identifies the RSS episode.
#[derive(Debug, Serialize)]
struct RssSyncBody<'a> {
    #[serde(flatten)]
    body: &'a SyncBody,
    add_by_rss_resource_data: AddByRssData,
}

/// Map a sync response status to an error. `None` means success.
/// The caller reads the status before it consumes the body.
fn map_status(response: &reqwest::Response, status: StatusCode) -> Option<SyncError> {
    if status.is_success() {
        return None;
    }
    match status {
        StatusCode::UNAUTHORIZED => Some(SyncError::Unauthorized),
        StatusCode::FORBIDDEN => Some(SyncError::Membership),
        StatusCode::NOT_FOUND => Some(SyncError::QueueNotFound),
        StatusCode::TOO_MANY_REQUESTS => {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(DEFAULT_RETRY_AFTER_SECS)
                .min(MAX_RETRY_AFTER_SECS);
            Some(SyncError::RateLimited(retry_after))
        }
        _ => Some(SyncError::Server(format!("HTTP {status}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    const QUEUE_LIST: &str = include_str!("../fixtures/queue-list.json");
    const NOW_PLAYING: &str = include_str!("../fixtures/queue-now-playing.json");

    /// A recorded request: method, path, body, authorization header.
    #[derive(Debug, Clone)]
    struct RecordedRequest {
        method: String,
        path: String,
        body: String,
        authorization: Option<String>,
    }

    /// A canned response: status, body, extra headers.
    type Canned = (u16, String, Vec<(String, String)>);

    struct MockServer {
        base_url: String,
        requests: Arc<StdMutex<Vec<RecordedRequest>>>,
        routes: Arc<StdMutex<HashMap<String, Canned>>>,
        handle: JoinHandle<()>,
    }

    impl MockServer {
        async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(StdMutex::new(Vec::new()));
            let routes = Arc::new(StdMutex::new(HashMap::new()));
            let recorded = requests.clone();
            let server_routes = routes.clone();
            let handle = tokio::spawn(async move {
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        break;
                    };
                    let recorded = recorded.clone();
                    let routes = server_routes.clone();
                    tokio::spawn(async move {
                        let mut buffer = Vec::new();
                        let mut chunk = [0u8; 4096];
                        loop {
                            let Ok(read) = stream.read(&mut chunk).await else {
                                return;
                            };
                            if read == 0 {
                                return;
                            }
                            buffer.extend_from_slice(&chunk[..read]);
                            if let Some(header_end) = find_subslice(&buffer, b"\r\n\r\n") {
                                let headers = String::from_utf8_lossy(&buffer[..header_end]);
                                let length = headers
                                    .lines()
                                    .find_map(|line| {
                                        let (name, value) = line.split_once(':')?;
                                        name.eq_ignore_ascii_case("content-length")
                                            .then(|| value.trim().parse::<usize>().ok())?
                                    })
                                    .unwrap_or(0);
                                if buffer.len() >= header_end + 4 + length {
                                    break;
                                }
                            }
                        }
                        let header_end = find_subslice(&buffer, b"\r\n\r\n").unwrap();
                        let headers = String::from_utf8_lossy(&buffer[..header_end]);
                        let mut lines = headers.lines();
                        let first = lines.next().unwrap_or("");
                        let mut parts = first.split_whitespace();
                        let method = parts.next().unwrap_or("").to_string();
                        let path = parts.next().unwrap_or("/").to_string();
                        let authorization = lines.find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("authorization")
                                .then(|| value.trim().to_string())
                        });
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        let body_start = header_end + 4;
                        let body =
                            String::from_utf8_lossy(&buffer[body_start..body_start + length])
                                .to_string();
                        recorded.lock().unwrap().push(RecordedRequest {
                            method,
                            path: path.clone(),
                            body,
                            authorization,
                        });
                        let (status, response_body, extra) = routes
                            .lock()
                            .unwrap()
                            .get(&path)
                            .cloned()
                            .unwrap_or_else(|| (404, "not found".to_string(), Vec::new()));
                        let mut header = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                            response_body.len()
                        );
                        for (name, value) in &extra {
                            header.push_str(&format!("{name}: {value}\r\n"));
                        }
                        header.push_str("\r\n");
                        let _ = stream.write_all(header.as_bytes()).await;
                        let _ = stream.write_all(response_body.as_bytes()).await;
                    });
                }
            });
            Self {
                base_url,
                requests,
                routes,
                handle,
            }
        }

        fn route(&self, path: &str, status: u16, body: &str) {
            self.routes
                .lock()
                .unwrap()
                .insert(path.to_string(), (status, body.to_string(), Vec::new()));
        }

        fn route_with_headers(
            &self,
            path: &str,
            status: u16,
            body: &str,
            headers: &[(&str, &str)],
        ) {
            self.routes.lock().unwrap().insert(
                path.to_string(),
                (
                    status,
                    body.to_string(),
                    headers
                        .iter()
                        .map(|(name, value)| (name.to_string(), value.to_string()))
                        .collect(),
                ),
            );
        }

        fn requests(&self) -> Vec<RecordedRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn sync_client(base_url: &str) -> QueueSyncClient {
        let client = PodverseClient::new(base_url, Some("test-token".to_string())).unwrap();
        QueueSyncClient::new(client)
    }

    fn body(position: f64, duration: Option<f64>, completed: bool) -> SyncBody {
        SyncBody {
            playback_position: position,
            media_file_duration: duration,
            completed,
        }
    }

    #[tokio::test]
    async fn push_now_playing_posts_item_path_and_body() {
        let server = MockServer::start().await;
        server.route("/queue/all-for-account/private", 200, QUEUE_LIST);
        server.route("/queue/Qactive1/item/YvIRJrjZVe/now-playing", 201, "{}");
        let client = sync_client(&server.base_url);
        client
            .push_now_playing(
                &SyncTarget::Item("YvIRJrjZVe".to_string()),
                &body(12.5, Some(1800.0), false),
            )
            .await
            .unwrap();
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        let write = &requests[1];
        assert_eq!(write.method, "POST");
        assert_eq!(write.path, "/queue/Qactive1/item/YvIRJrjZVe/now-playing");
        assert_eq!(write.authorization.as_deref(), Some("Bearer test-token"));
        let value: serde_json::Value = serde_json::from_str(&write.body).unwrap();
        assert_eq!(value["playback_position"], 12.5);
        assert_eq!(value["media_file_duration"], 1800.0);
        assert_eq!(value["completed"], false);
    }

    #[tokio::test]
    async fn push_history_posts_history_path_with_completed() {
        let server = MockServer::start().await;
        server.route("/queue/all-for-account/private", 200, QUEUE_LIST);
        server.route("/queue/Qactive1/item/YvIRJrjZVe/history", 201, "{}");
        let client = sync_client(&server.base_url);
        client
            .push_history(
                &SyncTarget::Item("YvIRJrjZVe".to_string()),
                &body(1800.0, Some(1800.0), true),
            )
            .await
            .unwrap();
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].path, "/queue/Qactive1/item/YvIRJrjZVe/history");
        let value: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
        assert_eq!(value["completed"], true);
    }

    #[tokio::test]
    async fn push_now_playing_posts_add_by_rss_path_and_resource_data() {
        let server = MockServer::start().await;
        server.route("/queue/all-for-account/private", 200, QUEUE_LIST);
        server.route("/queue/Qactive1/item-add-by-rss/now-playing", 201, "{}");
        let client = sync_client(&server.base_url);
        client
            .push_now_playing(
                &SyncTarget::AddByRss {
                    feed_url: "https://example.com/feed.xml".to_string(),
                    enclosure_url: "https://example.com/ep1.mp3".to_string(),
                },
                &body(60.0, Some(3600.0), false),
            )
            .await
            .unwrap();
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].path,
            "/queue/Qactive1/item-add-by-rss/now-playing"
        );
        let value: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
        assert_eq!(value["playback_position"], 60.0);
        assert_eq!(
            value["add_by_rss_resource_data"]["feed_url"],
            "https://example.com/feed.xml"
        );
        assert_eq!(
            value["add_by_rss_resource_data"]["enclosure_url"],
            "https://example.com/ep1.mp3"
        );
    }

    #[tokio::test]
    async fn ensure_queue_prefers_active_queue() {
        let server = MockServer::start().await;
        server.route("/queue/all-for-account/private", 200, QUEUE_LIST);
        let client = sync_client(&server.base_url);
        assert_eq!(client.ensure_queue().await.unwrap(), "Qactive1");
    }

    #[tokio::test]
    async fn ensure_queue_falls_back_to_first_queue() {
        let server = MockServer::start().await;
        server.route(
            "/queue/all-for-account/private",
            200,
            r#"[{"id_text":"Qonly","is_active_queue":false}]"#,
        );
        let client = sync_client(&server.base_url);
        assert_eq!(client.ensure_queue().await.unwrap(), "Qonly");
    }

    #[tokio::test]
    async fn ensure_queue_errors_when_no_queue_exists() {
        let server = MockServer::start().await;
        server.route("/queue/all-for-account/private", 200, "[]");
        let client = sync_client(&server.base_url);
        assert!(matches!(
            client.ensure_queue().await,
            Err(SyncError::QueueNotFound)
        ));
    }

    #[tokio::test]
    async fn fetch_now_playing_parses_remote_position() {
        let server = MockServer::start().await;
        server.route("/queue/all-for-account/private", 200, QUEUE_LIST);
        server.route("/queue/Qactive1/resources/now-playing", 200, NOW_PLAYING);
        let client = sync_client(&server.base_url);
        let row = client.fetch_now_playing().await.unwrap().unwrap();
        assert_eq!(row.item_id(), Some("YvIRJrjZVe"));
        assert_eq!(row.playback_position, Some(752.5));
        assert_eq!(row.media_file_duration, Some(1800.0));
        assert!(!row.completed);
        assert_eq!(row.position(), Some(Duration::from_secs_f64(752.5)));
        assert_eq!(row.last_played_unix(), Some(1795537980));
    }

    #[tokio::test]
    async fn fetch_now_playing_returns_none_for_null() {
        let server = MockServer::start().await;
        server.route("/queue/all-for-account/private", 200, QUEUE_LIST);
        server.route("/queue/Qactive1/resources/now-playing", 200, "null");
        let client = sync_client(&server.base_url);
        assert!(client.fetch_now_playing().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn status_errors_map_to_sync_errors() {
        for status in [401u16, 403, 404, 500] {
            let server = MockServer::start().await;
            server.route("/queue/all-for-account/private", status, "{}");
            let client = sync_client(&server.base_url);
            let error = client.ensure_queue().await.unwrap_err();
            let ok = match status {
                401 => matches!(error, SyncError::Unauthorized),
                403 => matches!(error, SyncError::Membership),
                404 => matches!(error, SyncError::QueueNotFound),
                _ => matches!(error, SyncError::Server(_)),
            };
            assert!(ok, "status {status} gave {error}");
        }
    }

    #[tokio::test]
    async fn rate_limit_keeps_retry_after() {
        let server = MockServer::start().await;
        server.route_with_headers(
            "/queue/all-for-account/private",
            429,
            "{}",
            &[("retry-after", "30")],
        );
        let client = sync_client(&server.base_url);
        assert!(matches!(
            client.ensure_queue().await,
            Err(SyncError::RateLimited(30))
        ));
    }

    #[tokio::test]
    async fn transport_error_is_a_sync_error() {
        let client = sync_client("http://127.0.0.1:1");
        let error = client.ensure_queue().await.unwrap_err();
        assert!(matches!(error, SyncError::Transport(_)));
    }

    #[test]
    fn add_by_rss_hash_matches_md5_of_json() {
        assert_eq!(
            add_by_rss_hash_id(
                "https://example.com/feed.xml",
                "https://example.com/ep1.mp3"
            ),
            "24f390e129523cfa6784d00141b6381e"
        );
    }

    #[test]
    fn canonical_feed_url_strips_userinfo() {
        assert_eq!(
            canonical_feed_url("https://user:pass@example.com/feed.xml"),
            "https://example.com/feed.xml"
        );
    }

    #[test]
    fn parse_iso8601_unix_parses_timestamps() {
        assert_eq!(
            parse_iso8601_unix("2026-11-24T16:33:00.000Z"),
            Some(1795537980)
        );
        assert_eq!(parse_iso8601_unix("not a date"), None);
    }
}
