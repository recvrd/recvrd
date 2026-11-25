//! Podverse queue sync worker.
//!
//! The worker drains position sync events from the playback service
//! and sends them to the Podverse queue. It never blocks playback:
//! the event channel is bounded and the service drops events when
//! it is full. Sync runs only when the config enables it, a token
//! is set, and offline mode is off. Every failure leaves local
//! state alone.

use common_structs::{ProviderId, TrackKey};
use provider_podverse::{QueueSyncClient, SyncBody, SyncError, SyncTarget, add_by_rss_hash_id};
use recvrd_core::{AudioSink, PlaybackService, PodverseConfig, PositionSyncEvent};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex as AsyncMutex, mpsc};

/// Maximum pending events that one drain pass collapses.
const MAX_DRAIN: usize = 32;

/// Build the sync worker when the config enables it. Return the
/// event sender, or `None` when sync cannot run.
pub fn start_sync<S: AudioSink + Send + 'static>(
    config: &PodverseConfig,
    playback: Arc<AsyncMutex<PlaybackService<S>>>,
    offline: Arc<AtomicBool>,
) -> Option<mpsc::Sender<PositionSyncEvent>> {
    if !config.sync {
        return None;
    }
    let base_url = config.base_url.as_deref()?;
    let token = config.token.as_deref().filter(|token| !token.is_empty())?;
    match provider_podverse::PodverseClient::new(base_url, Some(token.to_string())) {
        Ok(client) => {
            let sync = QueueSyncClient::new(client);
            Some(PodverseSyncWorker::spawn(sync, playback, offline))
        }
        Err(error) => {
            log::warn!("Podverse sync is off: {error}");
            None
        }
    }
}

/// The worker that sends position sync events to the Podverse queue.
struct PodverseSyncWorker<S: AudioSink> {
    client: QueueSyncClient,
    playback: Arc<AsyncMutex<PlaybackService<S>>>,
    offline: Arc<AtomicBool>,
    events: mpsc::Receiver<PositionSyncEvent>,
    blocked_until: Option<Instant>,
    disabled: bool,
}

impl<S: AudioSink + Send + 'static> PodverseSyncWorker<S> {
    /// Spawn the worker task. Return the event sender.
    fn spawn(
        client: QueueSyncClient,
        playback: Arc<AsyncMutex<PlaybackService<S>>>,
        offline: Arc<AtomicBool>,
    ) -> mpsc::Sender<PositionSyncEvent> {
        let (sender, receiver) = mpsc::channel(16);
        let worker = Self {
            client,
            playback,
            offline,
            events: receiver,
            blocked_until: None,
            disabled: false,
        };
        tokio::spawn(async move { worker.run().await });
        sender
    }

    /// Drain events until the sender closes.
    async fn run(mut self) {
        while let Some(event) = self.events.recv().await {
            let mut pending = vec![event];
            while pending.len() < MAX_DRAIN {
                match self.events.try_recv() {
                    Ok(event) => pending.push(event),
                    Err(_) => break,
                }
            }
            // Keep only the latest event per track. A newer event
            // for a track replaces an older one.
            let mut latest: Vec<PositionSyncEvent> = Vec::new();
            for event in pending {
                if let Some(position) = latest.iter().position(|other| other.key() == event.key()) {
                    latest[position] = event;
                } else {
                    latest.push(event);
                }
            }
            for event in latest {
                self.handle(event).await;
            }
        }
    }

    /// Handle one event. Skip it when sync is off, offline, or
    /// backed off. Never touch local state on failure.
    async fn handle(&mut self, event: PositionSyncEvent) {
        if self.disabled {
            return;
        }
        if self.offline.load(Ordering::SeqCst) {
            return;
        }
        if let Some(until) = self.blocked_until
            && Instant::now() < until
        {
            return;
        }
        self.blocked_until = None;
        match event {
            PositionSyncEvent::Write {
                key,
                position,
                duration,
                completed,
                feed_url,
                enclosure_url,
            } => {
                let Some(target) = target_for(&key, &feed_url, &enclosure_url) else {
                    return;
                };
                let body = SyncBody::new(position, duration, completed);
                if let Err(error) = self.client.push_now_playing(&target, &body).await {
                    self.note_error(&error);
                }
            }
            PositionSyncEvent::Finished {
                key,
                duration,
                feed_url,
                enclosure_url,
            } => {
                let Some(target) = target_for(&key, &feed_url, &enclosure_url) else {
                    return;
                };
                let position = duration.unwrap_or(Duration::ZERO);
                let body = SyncBody::new(position, duration, true);
                if let Err(error) = self.client.push_history(&target, &body).await {
                    self.note_error(&error);
                }
            }
            PositionSyncEvent::Read {
                key,
                feed_url,
                enclosure_url,
            } => {
                let Some(target) = target_for(&key, &feed_url, &enclosure_url) else {
                    return;
                };
                match self.client.fetch_now_playing().await {
                    Ok(Some(row)) => self.adopt(&key, &target, &row).await,
                    Ok(None) => {}
                    Err(error) => self.note_error(&error),
                }
            }
        }
    }

    /// Adopt a remote position when it matches the track and its
    /// write is newer than the last local write.
    async fn adopt(
        &mut self,
        key: &TrackKey,
        target: &SyncTarget,
        row: &provider_podverse::RemotePosition,
    ) {
        let matches = match target {
            SyncTarget::Item(item_id) => row.item_id() == Some(item_id.as_str()),
            SyncTarget::AddByRss {
                feed_url,
                enclosure_url,
            } => {
                row.add_by_rss_hash_id.as_deref()
                    == Some(&*add_by_rss_hash_id(feed_url, enclosure_url))
            }
        };
        if !matches {
            return;
        }
        let (Some(position), Some(updated_at)) = (row.position(), row.last_played_unix()) else {
            return;
        };
        let adopted = self
            .playback
            .lock()
            .await
            .apply_remote_position(key, position, updated_at);
        if adopted {
            log::debug!("adopted the remote position for {key}");
        }
    }

    /// Record a sync failure. Auth and membership failures stop
    /// sync for the rest of the run. Rate limits pause sync until
    /// the server allows it again. Other failures only skip the event.
    fn note_error(&mut self, error: &SyncError) {
        match error {
            SyncError::Unauthorized | SyncError::Membership => {
                self.disabled = true;
                log::warn!("Podverse sync is off: {error}");
            }
            SyncError::RateLimited(seconds) => {
                self.blocked_until = Some(Instant::now() + Duration::from_secs(*seconds));
                log::warn!("Podverse sync is backed off: {error}");
            }
            _ => {
                log::debug!("Podverse sync skipped an event: {error}");
            }
        }
    }
}

/// Map a track key to a sync target. YouTube and other providers
/// have no Podverse identity, so they never sync.
fn target_for(
    key: &TrackKey,
    feed_url: &Option<String>,
    enclosure_url: &Option<String>,
) -> Option<SyncTarget> {
    match key.provider {
        ProviderId::Podverse => Some(SyncTarget::Item(key.id.clone())),
        ProviderId::Rss => match (feed_url, enclosure_url) {
            (Some(feed_url), Some(enclosure_url)) => Some(SyncTarget::AddByRss {
                feed_url: feed_url.clone(),
                enclosure_url: enclosure_url.clone(),
            }),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_structs::{MediaKind, TrackMetadata, TrackSource};
    use recvrd_core::{PlaybackService, Track};
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    const QUEUE_LIST: &str = r#"[{"id_text":"Q1","is_active_queue":true}]"#;

    /// A recorded request: method and path.
    #[derive(Debug, Clone)]
    struct RecordedRequest {
        method: String,
        path: String,
        body: String,
    }

    /// A canned response: status and body.
    type Canned = (u16, String);

    struct MockServer {
        base_url: String,
        requests: Arc<StdMutex<Vec<RecordedRequest>>>,
        routes: Arc<StdMutex<HashMap<String, Canned>>>,
    }

    impl MockServer {
        async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(StdMutex::new(Vec::new()));
            let routes = Arc::new(StdMutex::new(HashMap::new()));
            let recorded = requests.clone();
            let server_routes = routes.clone();
            tokio::spawn(async move {
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
                            if let Some(end) = find_subslice(&buffer, b"\r\n\r\n") {
                                let headers = String::from_utf8_lossy(&buffer[..end]);
                                let length = content_length(&headers);
                                if buffer.len() >= end + 4 + length {
                                    break;
                                }
                            }
                        }
                        let end = find_subslice(&buffer, b"\r\n\r\n").unwrap();
                        let headers = String::from_utf8_lossy(&buffer[..end]);
                        let mut lines = headers.lines();
                        let first = lines.next().unwrap_or("");
                        let mut parts = first.split_whitespace();
                        let method = parts.next().unwrap_or("").to_string();
                        let path = parts.next().unwrap_or("/").to_string();
                        let length = content_length(&headers);
                        let body =
                            String::from_utf8_lossy(&buffer[end + 4..end + 4 + length]).to_string();
                        recorded.lock().unwrap().push(RecordedRequest {
                            method,
                            path: path.clone(),
                            body,
                        });
                        let (status, response_body) = routes
                            .lock()
                            .unwrap()
                            .get(&path)
                            .cloned()
                            .unwrap_or((404, "not found".to_string()));
                        let header = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            response_body.len()
                        );
                        let _ = stream.write_all(header.as_bytes()).await;
                        let _ = stream.write_all(response_body.as_bytes()).await;
                    });
                }
            });
            Self {
                base_url,
                requests,
                routes,
            }
        }

        fn route(&self, path: &str, status: u16, body: &str) {
            self.routes
                .lock()
                .unwrap()
                .insert(path.to_string(), (status, body.to_string()));
        }

        fn requests(&self) -> Vec<RecordedRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn content_length(headers: &str) -> usize {
        headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0)
    }

    /// A test sink that records seeks.
    #[derive(Default)]
    struct TestSink {
        seeks: Vec<Duration>,
    }

    impl AudioSink for TestSink {
        type Error = std::io::Error;

        fn play(
            &mut self,
            _track: &Track,
            _source: recvrd_core::PlaybackSource,
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        fn pause(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn resume(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn stop(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn seek(&mut self, position: Duration) -> Result<(), Self::Error> {
            self.seeks.push(position);
            Ok(())
        }

        fn set_volume(&mut self, _volume: u8) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn podverse_track() -> Track {
        Track {
            provider: ProviderId::Podverse,
            track_id: "YvIRJrjZVe".to_string(),
            title: "Episode".to_string(),
            author: "Show".to_string(),
            album: "Show".to_string(),
            duration: "30:00".to_string(),
            ..Default::default()
        }
    }

    fn rss_track() -> Track {
        Track {
            provider: ProviderId::Rss,
            track_id: "episode-1".to_string(),
            title: "Episode 1".to_string(),
            author: "Show".to_string(),
            album: "Show".to_string(),
            duration: "30:00".to_string(),
            source: Some(Box::new(TrackSource {
                url: "https://example.com/ep1.mp3".to_string(),
                mime: Some("audio/mpeg".to_string()),
                length: None,
                seekable: true,
                kind: MediaKind::Audio,
            })),
            metadata: Some(Box::new(TrackMetadata {
                feed_url: Some("https://example.com/feed.xml".to_string()),
                ..Default::default()
            })),
        }
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

    fn start_worker(
        server: &MockServer,
        playback: Arc<AsyncMutex<PlaybackService<TestSink>>>,
        offline: bool,
    ) -> mpsc::Sender<PositionSyncEvent> {
        let client = provider_podverse::PodverseClient::new(
            &server.base_url,
            Some("test-token".to_string()),
        )
        .unwrap();
        let sync = QueueSyncClient::new(client);
        PodverseSyncWorker::spawn(sync, playback, Arc::new(AtomicBool::new(offline)))
    }

    #[tokio::test]
    async fn write_event_posts_now_playing() {
        let server = MockServer::start().await;
        server.route("/queue/all-for-account/private", 200, QUEUE_LIST);
        server.route("/queue/Q1/item/YvIRJrjZVe/now-playing", 201, "{}");
        let dir = tempfile::tempdir().unwrap();
        let playback = Arc::new(AsyncMutex::new(PlaybackService::new(
            TestSink::default(),
            dir.path(),
            50,
        )));
        let sender = start_worker(&server, playback, false);
        sender
            .send(PositionSyncEvent::Write {
                key: TrackKey::podverse("YvIRJrjZVe"),
                position: Duration::from_secs(60),
                duration: Some(Duration::from_secs(1800)),
                completed: false,
                feed_url: None,
                enclosure_url: None,
            })
            .await
            .unwrap();
        assert!(
            wait_for(|| server
                .requests()
                .iter()
                .any(|request| request.path == "/queue/Q1/item/YvIRJrjZVe/now-playing"))
            .await
        );
        let request = server
            .requests()
            .into_iter()
            .find(|request| request.path.ends_with("now-playing"))
            .unwrap();
        assert_eq!(request.method, "POST");
        let value: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(value["playback_position"], 60.0);
    }

    #[tokio::test]
    async fn read_event_adopts_newer_remote_position() {
        let server = MockServer::start().await;
        server.route("/queue/all-for-account/private", 200, QUEUE_LIST);
        server.route(
            "/queue/Q1/resources/now-playing",
            200,
            r#"{"item":{"id_text":"YvIRJrjZVe"},"playback_position":"752.5","media_file_duration":"1800","completed":false,"last_played_at":"2026-11-24T16:33:00.000Z"}"#,
        );
        let dir = tempfile::tempdir().unwrap();
        let playback = Arc::new(AsyncMutex::new(PlaybackService::new(
            TestSink::default(),
            dir.path(),
            50,
        )));
        {
            let mut service = playback.lock().await;
            service.play_playlist(vec![podverse_track()]).await.unwrap();
        }
        let sender = start_worker(&server, playback.clone(), false);
        sender
            .send(PositionSyncEvent::Read {
                key: TrackKey::podverse("YvIRJrjZVe"),
                feed_url: None,
                enclosure_url: None,
            })
            .await
            .unwrap();
        let key = TrackKey::podverse("YvIRJrjZVe");
        assert!(
            wait_for(|| playback
                .try_lock()
                .map(|service| service.saved_position(&key) == Some(Duration::from_secs_f64(752.5)))
                .unwrap_or(false))
            .await
        );
    }

    #[tokio::test]
    async fn read_event_ignores_remote_for_another_track() {
        let server = MockServer::start().await;
        server.route("/queue/all-for-account/private", 200, QUEUE_LIST);
        server.route(
            "/queue/Q1/resources/now-playing",
            200,
            r#"{"item":{"id_text":"OtherTrack"},"playback_position":"100","completed":false,"last_played_at":"2026-11-24T16:33:00.000Z"}"#,
        );
        let dir = tempfile::tempdir().unwrap();
        let playback = Arc::new(AsyncMutex::new(PlaybackService::new(
            TestSink::default(),
            dir.path(),
            50,
        )));
        {
            let mut service = playback.lock().await;
            service.play_playlist(vec![podverse_track()]).await.unwrap();
        }
        let sender = start_worker(&server, playback.clone(), false);
        sender
            .send(PositionSyncEvent::Read {
                key: TrackKey::podverse("YvIRJrjZVe"),
                feed_url: None,
                enclosure_url: None,
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let key = TrackKey::podverse("YvIRJrjZVe");
        assert_eq!(playback.lock().await.saved_position(&key), None);
    }

    #[tokio::test]
    async fn failure_skips_the_event_and_keeps_working() {
        let server = MockServer::start().await;
        server.route("/queue/all-for-account/private", 500, "{}");
        let dir = tempfile::tempdir().unwrap();
        let playback = Arc::new(AsyncMutex::new(PlaybackService::new(
            TestSink::default(),
            dir.path(),
            50,
        )));
        let sender = start_worker(&server, playback.clone(), false);
        sender
            .send(PositionSyncEvent::Write {
                key: TrackKey::podverse("YvIRJrjZVe"),
                position: Duration::from_secs(60),
                duration: None,
                completed: false,
                feed_url: None,
                enclosure_url: None,
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        server.route("/queue/all-for-account/private", 200, QUEUE_LIST);
        server.route("/queue/Q1/item/YvIRJrjZVe/now-playing", 201, "{}");
        sender
            .send(PositionSyncEvent::Write {
                key: TrackKey::podverse("YvIRJrjZVe"),
                position: Duration::from_secs(61),
                duration: None,
                completed: false,
                feed_url: None,
                enclosure_url: None,
            })
            .await
            .unwrap();
        assert!(
            wait_for(|| server
                .requests()
                .iter()
                .any(|request| request.path == "/queue/Q1/item/YvIRJrjZVe/now-playing"))
            .await
        );
    }

    #[tokio::test]
    async fn youtube_tracks_and_offline_mode_never_sync() {
        let server = MockServer::start().await;
        server.route("/queue/all-for-account/private", 200, QUEUE_LIST);
        let dir = tempfile::tempdir().unwrap();
        let playback = Arc::new(AsyncMutex::new(PlaybackService::new(
            TestSink::default(),
            dir.path(),
            50,
        )));
        let sender = start_worker(&server, playback.clone(), false);
        sender
            .send(PositionSyncEvent::Write {
                key: TrackKey::youtube("video-1"),
                position: Duration::from_secs(60),
                duration: None,
                completed: false,
                feed_url: None,
                enclosure_url: None,
            })
            .await
            .unwrap();
        let offline_playback = Arc::new(AsyncMutex::new(PlaybackService::new(
            TestSink::default(),
            dir.path(),
            50,
        )));
        let offline_sender = start_worker(&server, offline_playback, true);
        offline_sender
            .send(PositionSyncEvent::Write {
                key: TrackKey::podverse("YvIRJrjZVe"),
                position: Duration::from_secs(60),
                duration: None,
                completed: false,
                feed_url: None,
                enclosure_url: None,
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(server.requests().is_empty());
    }

    #[tokio::test]
    async fn rss_write_uses_add_by_rss_path() {
        let server = MockServer::start().await;
        server.route("/queue/all-for-account/private", 200, QUEUE_LIST);
        server.route("/queue/Q1/item-add-by-rss/now-playing", 201, "{}");
        let dir = tempfile::tempdir().unwrap();
        let playback = Arc::new(AsyncMutex::new(PlaybackService::new(
            TestSink::default(),
            dir.path(),
            50,
        )));
        let track = rss_track();
        let sender = start_worker(&server, playback, false);
        sender
            .send(PositionSyncEvent::Write {
                key: track.key(),
                position: Duration::from_secs(60),
                duration: Some(Duration::from_secs(1800)),
                completed: false,
                feed_url: track
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.feed_url.clone()),
                enclosure_url: track.source.as_ref().map(|source| source.url.clone()),
            })
            .await
            .unwrap();
        assert!(
            wait_for(|| server
                .requests()
                .iter()
                .any(|request| request.path == "/queue/Q1/item-add-by-rss/now-playing"))
            .await
        );
    }
}
