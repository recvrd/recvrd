use axum::{
    Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, KeepAlive, Sse},
    },
    routing::{get, post},
};
use common_structs::{ProviderId, TrackKey};
use serde::Deserialize;
use std::{
    collections::HashSet,
    convert::Infallible,
    io,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    net::TcpListener,
    signal,
    sync::{Mutex, mpsc::UnboundedSender},
};
use ytermusicd_api::{
    AddPlaylistTracksRequest, ApiError, AuthState, AuthStatus as ApiAuthStatus,
    CreatePlaylistRequest, DownloadStatus, DownloadSummary, ErrorEnvelope, Event, EventKind,
    MovePlaylistTrackRequest, MusicApi, OkResponse, PlayNextRequest, PlayPlaylistRequest,
    PlayTrackRequest, PlaybackSnapshot as ApiPlaybackSnapshot, PlaybackState, Playlist,
    PlaylistDetail, PlaylistEditError, Queue as ApiQueue, QueueItem as ApiQueueItem, QueueRequest,
    RemovePlaylistTracksRequest, RemoveQueueRequest, ReorderQueueRequest,
    RepeatMode as ApiRepeatMode, RepeatRequest, RetryDownloadRequest, SearchResults, SeekRequest,
    ShuffleRequest, Status, Track, UpdatePlaylistRequest, VolumeRequest,
};
use ytermusicd_core::{
    AudioSink, AuthState as CoreAuthState, AuthStatus, CoreEvent, EventBus, PlaybackCommand,
    PlaybackError, PlaybackService, PlaybackSnapshot, PlaybackStatus, QueueSnapshot,
    RepeatMode as CoreRepeatMode,
};

use crate::{artwork::ArtworkCache, downloads::DownloadControl};

pub struct AppState<S: AudioSink + Send + 'static> {
    started_at: Instant,
    auth: AuthStatus,
    music: Arc<dyn MusicApi>,
    playback: Arc<Mutex<PlaybackService<S>>>,
    events: EventBus,
    downloads: Arc<dyn DownloadControl>,
    artwork: Arc<ArtworkCache>,
}

impl<S: AudioSink + Send + 'static> Clone for AppState<S> {
    fn clone(&self) -> Self {
        Self {
            started_at: self.started_at,
            auth: self.auth.clone(),
            music: self.music.clone(),
            playback: self.playback.clone(),
            events: self.events.clone(),
            downloads: self.downloads.clone(),
            artwork: self.artwork.clone(),
        }
    }
}

impl<S: AudioSink + Send + 'static> AppState<S> {
    pub fn new(
        auth: AuthStatus,
        music: Arc<dyn MusicApi>,
        playback: Arc<Mutex<PlaybackService<S>>>,
        events: EventBus,
        downloads: Arc<dyn DownloadControl>,
        artwork: Arc<ArtworkCache>,
    ) -> Self {
        Self {
            started_at: Instant::now(),
            auth,
            music,
            playback,
            events,
            downloads,
            artwork,
        }
    }
}

#[derive(Debug)]
struct ApiFailure {
    status: StatusCode,
    error: ApiError,
}

impl ApiFailure {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error: ApiError {
                code: "bad_request".to_string(),
                message: message.into(),
            },
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            error: ApiError {
                code: "not_found".to_string(),
                message: message.into(),
            },
        }
    }

    fn upstream(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            error: ApiError {
                code: "upstream_error".to_string(),
                message: message.into(),
            },
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: ApiError {
                code: "internal_error".to_string(),
                message: message.into(),
            },
        }
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        (self.status, Json(ErrorEnvelope { error: self.error })).into_response()
    }
}

pub fn router<S: AudioSink + Send + 'static>(state: AppState<S>) -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/playlists", get(playlists).post(create_playlist))
        .route(
            "/api/playlists/{id}",
            get(playlist).patch(update_playlist).delete(delete_playlist),
        )
        .route("/api/playlists/{id}/tracks", post(add_playlist_tracks))
        .route(
            "/api/playlists/{id}/tracks/remove",
            post(remove_playlist_tracks),
        )
        .route("/api/playlists/{id}/tracks/move", post(move_playlist_track))
        .route("/api/search", get(search))
        .route("/api/tracks/{provider}/{track_id}/cover", get(cover))
        .route("/api/queue", get(queue).post(queue_add).delete(queue_clear))
        .route("/api/queue/reorder", post(queue_reorder))
        .route("/api/queue/remove", post(queue_remove))
        .route("/api/queue/play-next", post(queue_play_next))
        .route("/api/play/playlist", post(play_playlist))
        .route("/api/play/track", post(play_track))
        .route("/api/queue/add", post(queue_add))
        .route("/api/pause", post(pause))
        .route("/api/resume", post(resume))
        .route("/api/next", post(next))
        .route("/api/previous", post(previous))
        .route("/api/volume", post(volume))
        .route("/api/seek", post(seek))
        .route("/api/shuffle", post(shuffle))
        .route("/api/repeat", post(repeat))
        .route("/api/downloads/retry", post(retry_download))
        .route("/api/events", get(events))
        .fallback(crate::web::static_handler)
        .with_state(state)
}

async fn status<S: AudioSink + Send + 'static>(State(state): State<AppState<S>>) -> Json<Status> {
    let playback = state.playback.lock().await;
    Json(status_from_parts(
        &state,
        &playback.snapshot(),
        &playback.queue_snapshot(),
    ))
}

async fn playlists<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
) -> Result<Json<Vec<Playlist>>, ApiFailure> {
    state
        .music
        .playlists()
        .await
        .map(Json)
        .map_err(ApiFailure::upstream)
}

async fn playlist<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<PlaylistDetail>, ApiFailure> {
    let playlist = state
        .music
        .playlists()
        .await
        .map_err(ApiFailure::upstream)?
        .into_iter()
        .find(|playlist| playlist.id == id)
        .ok_or_else(|| ApiFailure::not_found(format!("playlist {id} not found")))?;
    let tracks = state
        .music
        .playlist(&id)
        .await
        .map_err(ApiFailure::upstream)?;
    Ok(Json(PlaylistDetail { playlist, tracks }))
}

async fn create_playlist<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    Json(request): Json<CreatePlaylistRequest>,
) -> Result<(StatusCode, Json<Playlist>), ApiFailure> {
    let request = validate_create_playlist(request)?;
    let playlist = state
        .music
        .create_playlist(&request)
        .await
        .map_err(playlist_failure)?;
    Ok((StatusCode::CREATED, Json(playlist)))
}

async fn update_playlist<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<UpdatePlaylistRequest>,
) -> Result<Json<Playlist>, ApiFailure> {
    validate_playlist_id(&id)?;
    let request = UpdatePlaylistRequest {
        title: validate_title(&request.title)?,
    };
    state
        .music
        .update_playlist(&id, &request)
        .await
        .map(Json)
        .map_err(playlist_failure)
}

async fn delete_playlist<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<OkResponse>, ApiFailure> {
    validate_playlist_id(&id)?;
    state
        .music
        .delete_playlist(&id)
        .await
        .map_err(playlist_failure)?;
    Ok(Json(OkResponse { ok: true }))
}

async fn add_playlist_tracks<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<AddPlaylistTracksRequest>,
) -> Result<Json<Vec<Track>>, ApiFailure> {
    validate_playlist_id(&id)?;
    validate_track_keys(request.provider, &request.track_ids)?;
    state
        .music
        .add_playlist_tracks(&id, &request.track_ids)
        .await
        .map(Json)
        .map_err(playlist_failure)
}

async fn remove_playlist_tracks<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<RemovePlaylistTracksRequest>,
) -> Result<Json<Vec<Track>>, ApiFailure> {
    validate_playlist_id(&id)?;
    validate_indices(&request.indices)?;
    state
        .music
        .remove_playlist_tracks(&id, &request.indices)
        .await
        .map(Json)
        .map_err(playlist_failure)
}

async fn move_playlist_track<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<MovePlaylistTrackRequest>,
) -> Result<Json<Vec<Track>>, ApiFailure> {
    validate_playlist_id(&id)?;
    state
        .music
        .move_playlist_track(&id, request.from, request.to)
        .await
        .map(Json)
        .map_err(playlist_failure)
}

const MAX_PLAYLIST_TITLE: usize = 150;
const MAX_PLAYLIST_DESCRIPTION: usize = 5000;
const MAX_PLAYLIST_TRACKS_PER_REQUEST: usize = 100;

fn validate_title(title: &str) -> Result<String, ApiFailure> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ApiFailure::bad_request("playlist title must not be empty"));
    }
    if title.chars().count() > MAX_PLAYLIST_TITLE {
        return Err(ApiFailure::bad_request(format!(
            "playlist title must be {MAX_PLAYLIST_TITLE} characters or fewer"
        )));
    }
    if title.chars().any(char::is_control) {
        return Err(ApiFailure::bad_request(
            "playlist title must not contain control characters",
        ));
    }
    if title.contains('<') || title.contains('>') {
        return Err(ApiFailure::bad_request(
            "playlist title must not contain < or >",
        ));
    }
    Ok(title.to_string())
}

fn validate_create_playlist(
    mut request: CreatePlaylistRequest,
) -> Result<CreatePlaylistRequest, ApiFailure> {
    request.title = validate_title(&request.title)?;
    request.description = request.description.trim().to_string();
    if request.description.chars().count() > MAX_PLAYLIST_DESCRIPTION {
        return Err(ApiFailure::bad_request(format!(
            "playlist description must be {MAX_PLAYLIST_DESCRIPTION} characters or fewer"
        )));
    }
    Ok(request)
}

fn validate_playlist_id(id: &str) -> Result<(), ApiFailure> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(ApiFailure::bad_request("invalid playlist ID"));
    }
    Ok(())
}

/// Validate playlist track IDs for one provider. Playlist edits run
/// through the YouTube Music client, so only that provider is valid
/// here until another provider implements them.
fn validate_track_keys(provider: ProviderId, track_ids: &[String]) -> Result<(), ApiFailure> {
    if provider != ProviderId::Youtube {
        return Err(ApiFailure::bad_request(format!(
            "playlist editing is only supported for the {provider} provider"
        )));
    }
    if track_ids.is_empty() {
        return Err(ApiFailure::bad_request("select at least one track"));
    }
    if track_ids.len() > MAX_PLAYLIST_TRACKS_PER_REQUEST {
        return Err(ApiFailure::bad_request(format!(
            "add no more than {MAX_PLAYLIST_TRACKS_PER_REQUEST} tracks"
        )));
    }
    let mut seen = HashSet::new();
    for track_id in track_ids {
        if !provider.validate_id(track_id) {
            return Err(ApiFailure::bad_request(format!(
                "invalid track ID {track_id}"
            )));
        }
        if !seen.insert(track_id) {
            return Err(ApiFailure::bad_request(format!(
                "duplicate track ID {track_id}"
            )));
        }
    }
    Ok(())
}

/// The provider-scoped key of an API track.
fn track_key(track: &Track) -> TrackKey {
    TrackKey::new(track.provider, track.track_id.clone())
}

fn validate_indices(indices: &[usize]) -> Result<(), ApiFailure> {
    if indices.is_empty() {
        return Err(ApiFailure::bad_request("select at least one track"));
    }
    if indices.len() > MAX_PLAYLIST_TRACKS_PER_REQUEST {
        return Err(ApiFailure::bad_request(format!(
            "remove no more than {MAX_PLAYLIST_TRACKS_PER_REQUEST} tracks"
        )));
    }
    let mut seen = HashSet::new();
    for index in indices {
        if !seen.insert(*index) {
            return Err(ApiFailure::bad_request(format!(
                "duplicate track index {index}"
            )));
        }
    }
    Ok(())
}

/// Convert an API track to a core track. The fields match one to one.
fn core_track(track: Track) -> ytermusicd_core::Track {
    ytermusicd_core::Track {
        provider: track.provider,
        track_id: track.track_id,
        title: track.title,
        author: track.artist,
        album: track.album,
        duration: track.duration,
    }
}

fn playlist_failure(error: PlaylistEditError) -> ApiFailure {
    match error {
        PlaylistEditError::Invalid(message) => ApiFailure::bad_request(message),
        PlaylistEditError::Upstream(message) => ApiFailure::upstream(message),
    }
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    #[serde(default)]
    last_event_id: Option<u64>,
}

async fn search<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResults>, ApiFailure> {
    if query.q.trim().is_empty() {
        return Err(ApiFailure::bad_request("search query must not be empty"));
    }
    state
        .music
        .search(&query.q)
        .await
        .map(Json)
        .map_err(ApiFailure::upstream)
}

/// Serve a cached cover image.
async fn cover<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    AxumPath((provider, track_id)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiFailure> {
    let Ok(provider) = provider.parse::<ProviderId>() else {
        return Err(ApiFailure::not_found(format!(
            "unknown provider {provider}"
        )));
    };
    let key = TrackKey::new(provider, track_id);
    let Some(path) = state.artwork.ensure(&key).await else {
        let message = if key.validate() {
            format!("cover art for {key} is not available")
        } else {
            format!("invalid track ID {key}")
        };
        return Err(ApiFailure::not_found(message));
    };
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|error| ApiFailure::internal(format!("could not read cover art: {error}")))?;
    let etag = format!("\"{key}-{}\"", bytes.len());
    let matches = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(etag.as_str());
    if matches {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        set_cover_headers(response.headers_mut(), &etag);
        return Ok(response);
    }
    let mut response = bytes.into_response();
    set_cover_headers(response.headers_mut(), &etag);
    Ok(response)
}

fn set_cover_headers(headers: &mut HeaderMap, etag: &str) {
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=604800, immutable"),
    );
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, value);
    }
}

async fn queue<S: AudioSink + Send + 'static>(State(state): State<AppState<S>>) -> Json<ApiQueue> {
    let playback = state.playback.lock().await;
    Json(api_queue(&playback.queue_snapshot()))
}

async fn play_playlist<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    Json(request): Json<PlayPlaylistRequest>,
) -> Result<Json<Status>, ApiFailure> {
    let tracks = state
        .music
        .playlist(&request.playlist_id)
        .await
        .map_err(ApiFailure::upstream)?;
    if tracks.is_empty() {
        return Err(ApiFailure::bad_request("playlist has no tracks"));
    }
    let tracks: Vec<_> = tracks.into_iter().map(core_track).collect();
    state.downloads.queue_playlist(&tracks);
    let mut playback = state.playback.lock().await;
    playback.play_playlist(tracks).map_err(playback_failure)?;
    Ok(Json(status_from_parts(
        &state,
        &playback.snapshot(),
        &playback.queue_snapshot(),
    )))
}

async fn play_track<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    Json(request): Json<PlayTrackRequest>,
) -> Result<Json<Status>, ApiFailure> {
    let key = track_key(&request.track);
    if !key.validate() {
        return Err(ApiFailure::bad_request(format!("invalid track ID {key}")));
    }
    let track = core_track(request.track);
    state.downloads.queue_tracks(std::slice::from_ref(&track));
    let mut playback = state.playback.lock().await;
    playback.play_track(track).map_err(playback_failure)?;
    Ok(Json(status_from_parts(
        &state,
        &playback.snapshot(),
        &playback.queue_snapshot(),
    )))
}

async fn queue_add<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    Json(request): Json<QueueRequest>,
) -> Result<Json<ApiQueue>, ApiFailure> {
    let key = track_key(&request.track);
    if !key.validate() {
        return Err(ApiFailure::bad_request(format!("invalid track ID {key}")));
    }
    let track = core_track(request.track);
    state.downloads.queue_tracks(std::slice::from_ref(&track));
    let mut playback = state.playback.lock().await;
    playback.enqueue(track);
    Ok(Json(api_queue(&playback.queue_snapshot())))
}

async fn pause<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
) -> Result<Json<Status>, ApiFailure> {
    let mut playback = state.playback.lock().await;
    playback.pause().map_err(playback_failure)?;
    Ok(Json(status_from_parts(
        &state,
        &playback.snapshot(),
        &playback.queue_snapshot(),
    )))
}

async fn resume<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
) -> Result<Json<Status>, ApiFailure> {
    let mut playback = state.playback.lock().await;
    playback.resume().map_err(playback_failure)?;
    Ok(Json(status_from_parts(
        &state,
        &playback.snapshot(),
        &playback.queue_snapshot(),
    )))
}

async fn next<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
) -> Result<Json<Status>, ApiFailure> {
    let mut playback = state.playback.lock().await;
    playback.next_track().map_err(playback_failure)?;
    Ok(Json(status_from_parts(
        &state,
        &playback.snapshot(),
        &playback.queue_snapshot(),
    )))
}

async fn previous<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
) -> Result<Json<Status>, ApiFailure> {
    let mut playback = state.playback.lock().await;
    playback.previous_track().map_err(playback_failure)?;
    Ok(Json(status_from_parts(
        &state,
        &playback.snapshot(),
        &playback.queue_snapshot(),
    )))
}

async fn volume<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    Json(request): Json<VolumeRequest>,
) -> Result<Json<Status>, ApiFailure> {
    let mut playback = state.playback.lock().await;
    playback
        .set_volume(request.volume)
        .map_err(playback_failure)?;
    Ok(Json(status_from_parts(
        &state,
        &playback.snapshot(),
        &playback.queue_snapshot(),
    )))
}

async fn seek<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    Json(request): Json<SeekRequest>,
) -> Result<Json<Status>, ApiFailure> {
    let position = match Duration::try_from_secs_f64(request.position_secs) {
        Ok(position) => position,
        Err(_) => {
            return Err(ApiFailure::bad_request(
                "position must be a finite number of seconds >= 0",
            ));
        }
    };
    let mut playback = state.playback.lock().await;
    playback.seek(position).map_err(playback_failure)?;
    Ok(Json(status_from_parts(
        &state,
        &playback.snapshot(),
        &playback.queue_snapshot(),
    )))
}

async fn shuffle<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    Json(request): Json<ShuffleRequest>,
) -> Result<Json<Status>, ApiFailure> {
    let mut playback = state.playback.lock().await;
    playback.set_shuffle(request.enabled);
    Ok(Json(status_from_parts(
        &state,
        &playback.snapshot(),
        &playback.queue_snapshot(),
    )))
}

async fn repeat<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    Json(request): Json<RepeatRequest>,
) -> Result<Json<Status>, ApiFailure> {
    let mut playback = state.playback.lock().await;
    playback.set_repeat(core_repeat(request.repeat));
    Ok(Json(status_from_parts(
        &state,
        &playback.snapshot(),
        &playback.queue_snapshot(),
    )))
}

async fn retry_download<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    Json(request): Json<RetryDownloadRequest>,
) -> Result<StatusCode, ApiFailure> {
    let key = TrackKey::new(request.provider, request.track_id.clone());
    if state.downloads.retry(&key) {
        Ok(StatusCode::ACCEPTED)
    } else {
        Err(ApiFailure::not_found(format!(
            "no queued track with track ID {key}"
        )))
    }
}

async fn queue_clear<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
) -> Result<Json<ApiQueue>, ApiFailure> {
    let mut playback = state.playback.lock().await;
    playback.clear().map_err(playback_failure)?;
    Ok(Json(api_queue(&playback.queue_snapshot())))
}

async fn queue_reorder<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    Json(request): Json<ReorderQueueRequest>,
) -> Result<Json<ApiQueue>, ApiFailure> {
    let mut playback = state.playback.lock().await;
    if !playback.move_item(request.from, request.to) {
        return Err(ApiFailure::bad_request("invalid queue indices"));
    }
    Ok(Json(api_queue(&playback.queue_snapshot())))
}

async fn queue_remove<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    Json(request): Json<RemoveQueueRequest>,
) -> Result<Json<ApiQueue>, ApiFailure> {
    let mut playback = state.playback.lock().await;
    if request.index >= playback.queue_snapshot().items.len() {
        return Err(ApiFailure::bad_request("invalid queue index"));
    }
    playback.remove(request.index).map_err(playback_failure)?;
    Ok(Json(api_queue(&playback.queue_snapshot())))
}

async fn queue_play_next<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    Json(request): Json<PlayNextRequest>,
) -> Result<Json<ApiQueue>, ApiFailure> {
    let mut playback = state.playback.lock().await;
    let current = playback.queue_snapshot().current.unwrap_or(0);
    let target = (current + 1).min(playback.queue_snapshot().items.len().saturating_sub(1));
    if !playback.move_item(request.index, target) {
        return Err(ApiFailure::bad_request("invalid queue index"));
    }
    Ok(Json(api_queue(&playback.queue_snapshot())))
}

async fn events<S: AudioSink + Send + 'static>(
    State(state): State<AppState<S>>,
    headers: HeaderMap,
    Query(query): Query<EventsQuery>,
) -> impl IntoResponse {
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .or(query.last_event_id);
    let subscription = state.events.subscribe_since(last_event_id);
    let snapshot_status = status_from_parts(
        &state,
        &subscription.snapshot.playback,
        &subscription.snapshot.queue,
    );
    let snapshot_queue = api_queue(&subscription.snapshot.queue);

    let stream = async_stream::stream! {
        let mut last_sent = if subscription.needs_snapshot {
            let snapshot = Event {
                id: subscription.latest_id,
                event: EventKind::Snapshot {
                    status: Box::new(snapshot_status),
                    queue: snapshot_queue,
                },
            };
            yield Ok::<SseEvent, Infallible>(SseEvent::default()
                .event("snapshot")
                .id(subscription.latest_id.to_string())
                .json_data(snapshot)
                .expect("snapshot serializes"));
            subscription.latest_id
        } else {
            last_event_id.unwrap_or(0)
        };

        for envelope in subscription.replay {
            last_sent = envelope.id;
            if let Some(event) = api_event(envelope.event) {
                let payload = Event {
                    id: envelope.id,
                    event,
                };
                yield Ok::<SseEvent, Infallible>(SseEvent::default()
                    .id(envelope.id.to_string())
                    .json_data(payload)
                    .expect("event serializes"));
            }
        }

        let mut receiver = subscription.receiver;
        loop {
            match receiver.recv().await {
                Ok(envelope) => {
                    // The receiver is subscribed before history is read, so an
                    // event published in that window can be replayed twice.
                    if envelope.id <= last_sent {
                        continue;
                    }
                    last_sent = envelope.id;
                    if let Some(event) = api_event(envelope.event) {
                        let payload = Event {
                            id: envelope.id,
                            event,
                        };
                        yield Ok::<SseEvent, Infallible>(SseEvent::default()
                            .id(envelope.id.to_string())
                            .json_data(payload)
                            .expect("event serializes"));
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    log::warn!("SSE client lagged by {count} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    let sse = Sse::new(stream).keep_alive(KeepAlive::default());
    (
        [(
            axum::http::header::HeaderName::from_static("x-accel-buffering"),
            "no",
        )],
        sse,
    )
}

#[allow(clippy::too_many_arguments)]
pub async fn serve<S: AudioSink + Send + 'static>(
    bind: &str,
    port: u16,
    auth: AuthStatus,
    music: Arc<dyn MusicApi>,
    playback: Arc<Mutex<PlaybackService<S>>>,
    downloads: Arc<dyn DownloadControl>,
    artwork: Arc<ArtworkCache>,
    commands: UnboundedSender<PlaybackCommand>,
    dbus: bool,
) -> io::Result<()> {
    let addr: SocketAddr = format!("{bind}:{port}").parse().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid bind address {bind}:{port}: {error}"),
        )
    })?;

    let events = playback.lock().await.events();
    if dbus {
        match crate::mpris::spawn(commands, events.clone(), artwork.clone()) {
            Ok(_) => log::info!("MPRIS media controls enabled"),
            Err(error) => log::warn!("MPRIS unavailable: {error}"),
        }
    }

    let listener = TcpListener::bind(addr)
        .await
        .map_err(|error| io::Error::new(error.kind(), format!("failed to bind {addr}: {error}")))?;
    let local_addr = listener.local_addr()?;
    println!("ytermusicd listening on http://{local_addr}");

    let state = AppState::new(auth, music, playback, events, downloads, artwork);
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    log::info!("shutdown signal received");
}

fn status_from_parts<S: AudioSink + Send + 'static>(
    state: &AppState<S>,
    playback: &PlaybackSnapshot,
    queue: &QueueSnapshot,
) -> Status {
    Status {
        name: "ytermusicd".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        status: "ok".to_string(),
        uptime_seconds: state.started_at.elapsed().as_secs(),
        auth: api_auth(&state.auth),
        playback: api_playback(playback),
        queue_length: queue.items.len(),
        downloads: api_download_summary(state.downloads.summary()),
        providers: vec![ProviderId::Youtube],
    }
}

fn api_download_summary(summary: download_manager::DownloadSummary) -> DownloadSummary {
    DownloadSummary {
        queued: summary.queued,
        active: summary.active,
        completed: summary.completed,
        retrying: summary.retrying,
        failed: summary.failed,
    }
}

fn api_auth(auth: &AuthStatus) -> ApiAuthStatus {
    ApiAuthStatus {
        state: match auth.state {
            CoreAuthState::Ok => AuthState::Ok,
            CoreAuthState::Missing => AuthState::Missing,
            CoreAuthState::Invalid => AuthState::Invalid,
        },
        source: auth.source.as_ref().map(|path| path.display().to_string()),
        message: auth.message.clone(),
    }
}

fn api_playback(snapshot: &PlaybackSnapshot) -> ApiPlaybackSnapshot {
    ApiPlaybackSnapshot {
        state: match snapshot.status {
            PlaybackStatus::Stopped => PlaybackState::Stopped,
            PlaybackStatus::Playing => PlaybackState::Playing,
            PlaybackStatus::Paused => PlaybackState::Paused,
        },
        current: snapshot.current.as_ref().map(api_track),
        position_secs: snapshot.position_secs,
        volume: snapshot.volume,
        repeat: api_repeat(snapshot.repeat),
        shuffle: snapshot.shuffle,
    }
}

fn api_queue(queue: &QueueSnapshot) -> ApiQueue {
    ApiQueue {
        items: queue
            .items
            .iter()
            .map(|item| ApiQueueItem {
                track: api_track(&item.track),
                download: api_download(item.download),
            })
            .collect(),
        current: queue.current,
        repeat: api_repeat(queue.repeat),
        shuffle: queue.shuffle,
    }
}

fn api_track(track: &ytermusicd_core::Track) -> Track {
    Track {
        provider: track.provider,
        track_id: track.track_id.clone(),
        title: track.title.clone(),
        artist: track.author.clone(),
        album: track.album.clone(),
        duration: track.duration.clone(),
    }
}

fn api_download(status: common_structs::MusicDownloadStatus) -> DownloadStatus {
    match status {
        common_structs::MusicDownloadStatus::NotDownloaded => DownloadStatus::NotDownloaded,
        common_structs::MusicDownloadStatus::Downloaded => DownloadStatus::Downloaded,
        common_structs::MusicDownloadStatus::Downloading(progress) => DownloadStatus::Downloading {
            progress: progress.min(100) as u8,
        },
        common_structs::MusicDownloadStatus::Retrying(attempt) => {
            DownloadStatus::Retrying { attempt }
        }
        common_structs::MusicDownloadStatus::DownloadFailed => {
            DownloadStatus::Failed { message: None }
        }
    }
}

fn api_repeat(repeat: CoreRepeatMode) -> ApiRepeatMode {
    match repeat {
        CoreRepeatMode::Off => ApiRepeatMode::Off,
        CoreRepeatMode::One => ApiRepeatMode::One,
        CoreRepeatMode::All => ApiRepeatMode::All,
    }
}

fn core_repeat(repeat: ApiRepeatMode) -> CoreRepeatMode {
    match repeat {
        ApiRepeatMode::Off => CoreRepeatMode::Off,
        ApiRepeatMode::One => CoreRepeatMode::One,
        ApiRepeatMode::All => CoreRepeatMode::All,
    }
}

fn api_event(event: CoreEvent) -> Option<EventKind> {
    match event {
        CoreEvent::Playback(snapshot) => Some(EventKind::Playback {
            playback: api_playback(&snapshot),
        }),
        CoreEvent::Queue(queue) => Some(EventKind::Queue {
            queue: api_queue(&queue),
        }),
        CoreEvent::DownloadStatus { track_id, status } => Some(EventKind::Download {
            provider: track_id.provider,
            track_id: track_id.id,
            status: api_download(status),
        }),
        CoreEvent::TrackEnded { track_id } => Some(EventKind::TrackEnded {
            provider: track_id.provider,
            track_id: track_id.id,
        }),
        CoreEvent::Error { message } => Some(EventKind::Error {
            error: ApiError {
                code: "core_error".to_string(),
                message,
            },
        }),
        CoreEvent::LibraryChanged => Some(EventKind::LibraryChanged),
    }
}

fn playback_failure(error: PlaybackError) -> ApiFailure {
    match error {
        PlaybackError::NoCurrentTrack => ApiFailure::bad_request("no current track"),
        PlaybackError::IndexOutOfBounds => ApiFailure::bad_request("queue index out of bounds"),
        PlaybackError::InvalidPosition => ApiFailure::bad_request("invalid seek position"),
        PlaybackError::Sink(message) => ApiFailure::internal(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures_util::StreamExt;
    use std::{
        path::Path,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use ytermusicd_core::{CoverFetcher, Track as CoreTrack};

    #[derive(Default)]
    struct MockSink;

    impl AudioSink for MockSink {
        type Error = std::io::Error;

        fn play(&mut self, _track: &CoreTrack, _path: &Path) -> Result<(), Self::Error> {
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
        fn seek(&mut self, _position: Duration) -> Result<(), Self::Error> {
            Ok(())
        }
        fn set_volume(&mut self, _volume: u8) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[derive(Default, Clone)]
    struct MockMusicApi {
        edits: Arc<std::sync::Mutex<Vec<String>>>,
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

    #[async_trait]
    impl MusicApi for MockMusicApi {
        async fn playlists(&self) -> Result<Vec<Playlist>, String> {
            Ok(vec![Playlist {
                id: "p1".to_string(),
                name: "Playlist One".to_string(),
                subtitle: "Artist".to_string(),
            }])
        }

        async fn playlist(&self, _id: &str) -> Result<Vec<Track>, String> {
            Ok(vec![track("v1"), track("v2")])
        }

        async fn search(&self, _query: &str) -> Result<SearchResults, String> {
            Ok(SearchResults {
                tracks: vec![track("v3")],
                playlists: vec![],
            })
        }

        async fn create_playlist(
            &self,
            request: &CreatePlaylistRequest,
        ) -> Result<Playlist, PlaylistEditError> {
            self.edits
                .lock()
                .unwrap()
                .push(format!("create {}", request.title));
            Ok(Playlist {
                id: "VLnew".to_string(),
                name: request.title.clone(),
                subtitle: "Private playlist".to_string(),
            })
        }

        async fn update_playlist(
            &self,
            id: &str,
            request: &UpdatePlaylistRequest,
        ) -> Result<Playlist, PlaylistEditError> {
            self.edits
                .lock()
                .unwrap()
                .push(format!("rename {id} {}", request.title));
            Ok(Playlist {
                id: id.to_string(),
                name: request.title.clone(),
                subtitle: String::new(),
            })
        }

        async fn delete_playlist(&self, id: &str) -> Result<(), PlaylistEditError> {
            self.edits.lock().unwrap().push(format!("delete {id}"));
            Ok(())
        }

        async fn add_playlist_tracks(
            &self,
            id: &str,
            track_ids: &[String],
        ) -> Result<Vec<Track>, PlaylistEditError> {
            self.edits
                .lock()
                .unwrap()
                .push(format!("add {id} {}", track_ids.join(",")));
            Ok(vec![track("v1")])
        }

        async fn remove_playlist_tracks(
            &self,
            id: &str,
            indices: &[usize],
        ) -> Result<Vec<Track>, PlaylistEditError> {
            self.edits
                .lock()
                .unwrap()
                .push(format!("remove {id} {indices:?}"));
            Ok(vec![track("v1")])
        }

        async fn move_playlist_track(
            &self,
            id: &str,
            from: usize,
            to: usize,
        ) -> Result<Vec<Track>, PlaylistEditError> {
            self.edits
                .lock()
                .unwrap()
                .push(format!("move {id} {from} {to}"));
            Ok(vec![track("v1")])
        }
    }

    struct FailingMusicApi;

    #[async_trait]
    impl MusicApi for FailingMusicApi {
        async fn playlists(&self) -> Result<Vec<Playlist>, String> {
            Err("upstream unavailable".to_string())
        }

        async fn playlist(&self, _id: &str) -> Result<Vec<Track>, String> {
            Err("upstream unavailable".to_string())
        }

        async fn search(&self, _query: &str) -> Result<SearchResults, String> {
            Err("upstream unavailable".to_string())
        }

        async fn create_playlist(
            &self,
            _request: &CreatePlaylistRequest,
        ) -> Result<Playlist, PlaylistEditError> {
            Err(PlaylistEditError::Upstream(
                "upstream unavailable".to_string(),
            ))
        }

        async fn update_playlist(
            &self,
            _id: &str,
            _request: &UpdatePlaylistRequest,
        ) -> Result<Playlist, PlaylistEditError> {
            Err(PlaylistEditError::Upstream(
                "upstream unavailable".to_string(),
            ))
        }

        async fn delete_playlist(&self, _id: &str) -> Result<(), PlaylistEditError> {
            Err(PlaylistEditError::Upstream(
                "upstream unavailable".to_string(),
            ))
        }

        async fn add_playlist_tracks(
            &self,
            _id: &str,
            _track_ids: &[String],
        ) -> Result<Vec<Track>, PlaylistEditError> {
            Err(PlaylistEditError::Upstream(
                "upstream unavailable".to_string(),
            ))
        }

        async fn remove_playlist_tracks(
            &self,
            _id: &str,
            _indices: &[usize],
        ) -> Result<Vec<Track>, PlaylistEditError> {
            Err(PlaylistEditError::Upstream(
                "upstream unavailable".to_string(),
            ))
        }

        async fn move_playlist_track(
            &self,
            _id: &str,
            _from: usize,
            _to: usize,
        ) -> Result<Vec<Track>, PlaylistEditError> {
            Err(PlaylistEditError::Upstream(
                "upstream unavailable".to_string(),
            ))
        }
    }

    #[derive(Default)]
    struct MockDownloads {
        playlists: std::sync::Mutex<Vec<Vec<String>>>,
        queued: std::sync::Mutex<Vec<String>>,
        retryable: bool,
    }

    impl DownloadControl for MockDownloads {
        fn queue_playlist(&self, tracks: &[CoreTrack]) {
            self.playlists
                .lock()
                .unwrap()
                .push(tracks.iter().map(|track| track.track_id.clone()).collect());
        }

        fn queue_tracks(&self, tracks: &[CoreTrack]) {
            self.queued
                .lock()
                .unwrap()
                .extend(tracks.iter().map(|track| track.track_id.clone()));
        }

        fn retry(&self, _key: &TrackKey) -> bool {
            self.retryable
        }

        fn summary(&self) -> download_manager::DownloadSummary {
            download_manager::DownloadSummary::default()
        }
    }

    struct FakeCovers {
        calls: AtomicUsize,
    }

    impl FakeCovers {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl CoverFetcher for FakeCovers {
        async fn fetch(&self, key: &TrackKey) -> Option<Vec<u8>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (key.id == "abcdefghijk").then(|| b"fake-jpeg-data".to_vec())
        }
    }

    fn test_state() -> AppState<MockSink> {
        test_state_with(Arc::new(MockMusicApi::default()))
    }

    fn test_state_with(music: Arc<dyn MusicApi>) -> AppState<MockSink> {
        test_state_with_downloads(music, Arc::new(MockDownloads::default()))
    }

    fn test_state_with_downloads(
        music: Arc<dyn MusicApi>,
        downloads: Arc<dyn DownloadControl>,
    ) -> AppState<MockSink> {
        let artwork = Arc::new(ArtworkCache::with_fetcher(
            std::env::temp_dir().join("ytermusicd-test-covers"),
            0,
            Arc::new(FakeCovers::new()),
        ));
        test_state_with_downloads_and_artwork(music, downloads, artwork)
    }

    fn test_state_with_artwork(artwork: Arc<ArtworkCache>) -> AppState<MockSink> {
        test_state_with_downloads_and_artwork(
            Arc::new(MockMusicApi::default()),
            Arc::new(MockDownloads::default()),
            artwork,
        )
    }

    fn test_state_with_downloads_and_artwork(
        music: Arc<dyn MusicApi>,
        downloads: Arc<dyn DownloadControl>,
        artwork: Arc<ArtworkCache>,
    ) -> AppState<MockSink> {
        let service = PlaybackService::new(MockSink, "/tmp/ytermusicd-test", 50);
        let events = service.events();
        let playback = Arc::new(Mutex::new(service));
        AppState::new(
            AuthStatus::ok("/tmp/headers.txt"),
            music,
            playback,
            events,
            downloads,
            artwork,
        )
    }

    async fn read_until<B, S>(stream: &mut S, buffer: &mut String, needle: &str)
    where
        B: AsRef<[u8]>,
        S: futures_util::Stream<Item = Result<B, reqwest::Error>> + Unpin,
    {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !buffer.contains(needle) {
            let chunk = tokio::time::timeout_at(deadline, stream.next())
                .await
                .expect("timed out waiting for SSE data")
                .expect("SSE stream ended")
                .expect("SSE stream error");
            buffer.push_str(&String::from_utf8_lossy(chunk.as_ref()));
        }
    }

    fn last_event_id(buffer: &str) -> u64 {
        buffer
            .lines()
            .filter_map(|line| line.strip_prefix("id:"))
            .filter_map(|id| id.trim().parse::<u64>().ok())
            .max()
            .expect("at least one SSE event id")
    }

    fn last_sse_payload(buffer: &str) -> serde_json::Value {
        let data = buffer
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .next_back()
            .expect("at least one SSE data line");
        serde_json::from_str(data.trim()).expect("SSE data is JSON")
    }

    async fn start_server(state: AppState<MockSink>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router(state)).await.unwrap();
        });
        (format!("http://{addr}"), server)
    }

    #[tokio::test]
    async fn status_and_library_endpoints() {
        let (base, server) = start_server(test_state()).await;
        let client = reqwest::Client::new();

        let status: serde_json::Value = client
            .get(format!("{base}/api/status"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(status["auth"]["state"], "ok");
        assert_eq!(status["playback"]["state"], "stopped");

        let playlists: serde_json::Value = client
            .get(format!("{base}/api/playlists"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(playlists[0]["id"], "p1");

        let detail: serde_json::Value = client
            .get(format!("{base}/api/playlists/p1"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(detail["tracks"].as_array().unwrap().len(), 2);

        let search: serde_json::Value = client
            .get(format!("{base}/api/search?q=test"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(search["tracks"][0]["track_id"], "v3");
        server.abort();
    }

    #[tokio::test]
    async fn playback_and_queue_commands() {
        let (base, server) = start_server(test_state()).await;
        let client = reqwest::Client::new();

        let started: serde_json::Value = client
            .post(format!("{base}/api/play/playlist"))
            .json(&PlayPlaylistRequest {
                playlist_id: "p1".to_string(),
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(started["playback"]["state"], "playing");
        assert_eq!(started["playback"]["current"]["track_id"], "v1");
        assert_eq!(started["queue_length"], 2);

        let volume: serde_json::Value = client
            .post(format!("{base}/api/volume"))
            .json(&VolumeRequest { volume: 80 })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(volume["playback"]["volume"], 80);

        let clamped: serde_json::Value = client
            .post(format!("{base}/api/volume"))
            .json(&VolumeRequest { volume: 255 })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(clamped["playback"]["volume"], 100);

        let paused: serde_json::Value = client
            .post(format!("{base}/api/pause"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(paused["playback"]["state"], "paused");

        let next: serde_json::Value = client
            .post(format!("{base}/api/next"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(next["playback"]["current"]["track_id"], "v2");

        let previous: serde_json::Value = client
            .post(format!("{base}/api/previous"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(previous["playback"]["current"]["track_id"], "v1");

        let queue: serde_json::Value = client
            .get(format!("{base}/api/queue"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(queue["items"].as_array().unwrap().len(), 2);
        server.abort();
    }

    #[tokio::test]
    async fn queue_mutations() {
        let (base, server) = start_server(test_state()).await;
        let client = reqwest::Client::new();
        client
            .post(format!("{base}/api/play/playlist"))
            .json(&PlayPlaylistRequest {
                playlist_id: "p1".to_string(),
            })
            .send()
            .await
            .unwrap();
        let added: serde_json::Value = client
            .post(format!("{base}/api/queue"))
            .json(&QueueRequest {
                track: track("9bZkp7q19f0"),
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(added["items"].as_array().unwrap().len(), 3);

        let played_next: serde_json::Value = client
            .post(format!("{base}/api/queue/play-next"))
            .json(&PlayNextRequest { index: 2 })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(played_next["items"][1]["track"]["track_id"], "9bZkp7q19f0");

        let reordered: serde_json::Value = client
            .post(format!("{base}/api/queue/reorder"))
            .json(&ReorderQueueRequest { from: 0, to: 1 })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(reordered["items"][0]["track"]["track_id"], "9bZkp7q19f0");

        let removed: serde_json::Value = client
            .post(format!("{base}/api/queue/remove"))
            .json(&RemoveQueueRequest { index: 0 })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(removed["items"].as_array().unwrap().len(), 2);

        let cleared: serde_json::Value = client
            .delete(format!("{base}/api/queue"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(cleared["items"].as_array().unwrap().len(), 0);
        server.abort();
    }

    #[tokio::test]
    async fn play_track_transport_and_modes() {
        let (base, server) = start_server(test_state()).await;
        let client = reqwest::Client::new();

        let started: serde_json::Value = client
            .post(format!("{base}/api/play/track"))
            .json(&PlayTrackRequest {
                track: track("dQw4w9WgXcQ"),
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(started["playback"]["state"], "playing");
        assert_eq!(started["playback"]["current"]["track_id"], "dQw4w9WgXcQ");

        let seeked: serde_json::Value = client
            .post(format!("{base}/api/seek"))
            .json(&SeekRequest {
                position_secs: 42.5,
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(seeked["playback"]["state"], "playing");
        let position = seeked["playback"]["position_secs"].as_f64().unwrap();
        assert!((position - 42.5).abs() < 1.0, "position was {position}");

        let shuffled: serde_json::Value = client
            .post(format!("{base}/api/shuffle"))
            .json(&ShuffleRequest { enabled: true })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(shuffled["playback"]["shuffle"], true);

        let repeated: serde_json::Value = client
            .post(format!("{base}/api/repeat"))
            .json(&RepeatRequest {
                repeat: ApiRepeatMode::All,
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(repeated["playback"]["repeat"], "all");

        let paused: serde_json::Value = client
            .post(format!("{base}/api/pause"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(paused["playback"]["state"], "paused");

        let resumed: serde_json::Value = client
            .post(format!("{base}/api/resume"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(resumed["playback"]["state"], "playing");
        server.abort();
    }

    #[tokio::test]
    async fn play_track_and_queue_add_reject_invalid_track_ids() {
        let (base, server) = start_server(test_state()).await;
        let client = reqwest::Client::new();

        for track_id in ["../../x", "too-short", ""] {
            let response = client
                .post(format!("{base}/api/play/track"))
                .json(&PlayTrackRequest {
                    track: track(track_id),
                })
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
            let body: serde_json::Value = response.json().await.unwrap();
            assert_eq!(body["error"]["code"], "bad_request");

            let response = client
                .post(format!("{base}/api/queue"))
                .json(&QueueRequest {
                    track: track(track_id),
                })
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
            let body: serde_json::Value = response.json().await.unwrap();
            assert_eq!(body["error"]["code"], "bad_request");
        }
        server.abort();
    }

    #[tokio::test]
    async fn invalid_requests_are_rejected() {
        let (base, server) = start_server(test_state()).await;
        let client = reqwest::Client::new();

        let empty_search = client
            .get(format!("{base}/api/search?q="))
            .send()
            .await
            .unwrap();
        assert_eq!(empty_search.status(), reqwest::StatusCode::BAD_REQUEST);

        let bad_seek = client
            .post(format!("{base}/api/seek"))
            .json(&SeekRequest {
                position_secs: -1.0,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(bad_seek.status(), reqwest::StatusCode::BAD_REQUEST);

        let bad_remove = client
            .post(format!("{base}/api/queue/remove"))
            .json(&RemoveQueueRequest { index: 99 })
            .send()
            .await
            .unwrap();
        assert_eq!(bad_remove.status(), reqwest::StatusCode::BAD_REQUEST);

        let bad_reorder = client
            .post(format!("{base}/api/queue/reorder"))
            .json(&ReorderQueueRequest { from: 99, to: 0 })
            .send()
            .await
            .unwrap();
        assert_eq!(bad_reorder.status(), reqwest::StatusCode::BAD_REQUEST);

        // Pause/resume are idempotent even when nothing is playing.
        let idle_pause = client
            .post(format!("{base}/api/pause"))
            .send()
            .await
            .unwrap();
        assert_eq!(idle_pause.status(), reqwest::StatusCode::OK);
        let idle_status: serde_json::Value = idle_pause.json().await.unwrap();
        assert_eq!(idle_status["playback"]["state"], "stopped");
        server.abort();
    }

    #[tokio::test]
    async fn seek_rejects_non_finite_and_overflowing_positions() {
        let (base, server) = start_server(test_state()).await;
        let client = reqwest::Client::new();

        // 1e999 parses to positive infinity, and 1e300 is finite but
        // overflows a Duration. Both panicked the handler before the fix.
        // A raw body is needed here: a typed SeekRequest cannot hold
        // either value through serde_json.
        // 1e999 never reaches the handler: serde_json rejects the
        // out-of-range number during extraction, so the daemon answers
        // a bare 400. That is still a 400, not a panic.
        let response = client
            .post(format!("{base}/api/seek"))
            .header("Content-Type", "application/json")
            .body("{\"position_secs\":1e999}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        let response = client
            .post(format!("{base}/api/seek"))
            .header("Content-Type", "application/json")
            .body("{\"position_secs\":1e300}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], "bad_request");

        let valid = client
            .post(format!("{base}/api/seek"))
            .header("Content-Type", "application/json")
            .body("{\"position_secs\":42.5}")
            .send()
            .await
            .unwrap();
        assert_eq!(valid.status(), reqwest::StatusCode::OK);
        server.abort();
    }

    #[tokio::test]
    async fn upstream_failures_are_502_with_envelope() {
        let (base, server) = start_server(test_state_with(Arc::new(FailingMusicApi))).await;
        let response = reqwest::Client::new()
            .get(format!("{base}/api/playlists"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], "upstream_error");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("upstream unavailable")
        );
        server.abort();
    }

    #[tokio::test]
    async fn commands_queue_downloads() {
        let downloads = Arc::new(MockDownloads::default());
        let (base, server) = start_server(test_state_with_downloads(
            Arc::new(MockMusicApi::default()),
            downloads.clone(),
        ))
        .await;
        let client = reqwest::Client::new();

        client
            .post(format!("{base}/api/play/playlist"))
            .json(&PlayPlaylistRequest {
                playlist_id: "p1".to_string(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(
            downloads.playlists.lock().unwrap()[0],
            vec!["v1".to_string(), "v2".to_string()]
        );

        client
            .post(format!("{base}/api/queue"))
            .json(&QueueRequest {
                track: track("9bZkp7q19f0"),
            })
            .send()
            .await
            .unwrap();
        client
            .post(format!("{base}/api/play/track"))
            .json(&PlayTrackRequest {
                track: track("dQw4w9WgXcQ"),
            })
            .send()
            .await
            .unwrap();

        let queued = downloads.queued.lock().unwrap().clone();
        assert!(queued.contains(&"9bZkp7q19f0".to_string()));
        assert!(queued.contains(&"dQw4w9WgXcQ".to_string()));
        server.abort();
    }

    #[tokio::test]
    async fn retry_download_accepts_known_tracks_and_rejects_unknown() {
        let downloads = Arc::new(MockDownloads {
            retryable: true,
            ..MockDownloads::default()
        });
        let (base, server) = start_server(test_state_with_downloads(
            Arc::new(MockMusicApi::default()),
            downloads,
        ))
        .await;
        let client = reqwest::Client::new();

        let accepted = client
            .post(format!("{base}/api/downloads/retry"))
            .json(&RetryDownloadRequest {
                provider: ProviderId::Youtube,
                track_id: "v1".to_string(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(accepted.status(), reqwest::StatusCode::ACCEPTED);
        server.abort();

        let (base, server) = start_server(test_state()).await;
        let missing = client
            .post(format!("{base}/api/downloads/retry"))
            .json(&RetryDownloadRequest {
                provider: ProviderId::Youtube,
                track_id: "missing".to_string(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);
        server.abort();
    }

    #[tokio::test]
    async fn sse_sends_snapshot_then_live_updates() {
        let (base, server) = start_server(test_state()).await;
        let client = reqwest::Client::new();
        let response = client
            .get(format!("{base}/api/events"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/event-stream")
        );
        assert_eq!(response.headers()["cache-control"], "no-cache");
        assert_eq!(response.headers()["x-accel-buffering"], "no");

        let mut stream = Box::pin(response.bytes_stream());
        let mut buffer = String::new();
        read_until(&mut stream, &mut buffer, "\n\n").await;
        let snapshot = last_sse_payload(&buffer);
        assert_eq!(snapshot["event"]["type"], "snapshot");
        assert_eq!(snapshot["event"]["status"]["playback"]["state"], "stopped");

        client
            .post(format!("{base}/api/volume"))
            .json(&VolumeRequest { volume: 80 })
            .send()
            .await
            .unwrap();
        read_until(
            &mut stream,
            &mut buffer,
            "\"volume\":80,\"repeat\":\"off\",\"shuffle\":false}}}",
        )
        .await;
        let live = last_sse_payload(&buffer);
        assert_eq!(live["id"].as_u64(), Some(last_event_id(&buffer)));
        assert_eq!(live["event"]["type"], "playback");
        assert_eq!(live["event"]["playback"]["volume"], 80);
        server.abort();
    }

    #[tokio::test]
    async fn sse_replays_events_after_last_event_id() {
        let (base, server) = start_server(test_state()).await;
        let client = reqwest::Client::new();

        let response = client
            .get(format!("{base}/api/events"))
            .send()
            .await
            .unwrap();
        let mut stream = Box::pin(response.bytes_stream());
        let mut buffer = String::new();
        read_until(&mut stream, &mut buffer, "\n\n").await;

        client
            .post(format!("{base}/api/volume"))
            .json(&VolumeRequest { volume: 80 })
            .send()
            .await
            .unwrap();
        read_until(
            &mut stream,
            &mut buffer,
            "\"volume\":80,\"repeat\":\"off\",\"shuffle\":false}}}",
        )
        .await;
        let resume_from = last_event_id(&buffer);
        drop(stream);

        // This update happens while the client is disconnected.
        client
            .post(format!("{base}/api/volume"))
            .json(&VolumeRequest { volume: 60 })
            .send()
            .await
            .unwrap();

        let response = client
            .get(format!("{base}/api/events"))
            .header("Last-Event-ID", resume_from.to_string())
            .send()
            .await
            .unwrap();
        let mut stream = Box::pin(response.bytes_stream());
        let mut replay = String::new();
        read_until(
            &mut stream,
            &mut replay,
            "\"volume\":60,\"repeat\":\"off\",\"shuffle\":false}}}",
        )
        .await;
        assert!(!replay.contains("event: snapshot"));
        assert!(last_event_id(&replay) > resume_from);
        let replayed = last_sse_payload(&replay);
        assert_eq!(replayed["id"].as_u64(), Some(last_event_id(&replay)));
        assert_eq!(replayed["event"]["type"], "playback");
        assert_eq!(replayed["event"]["playback"]["volume"], 60);
        server.abort();
    }

    #[tokio::test]
    async fn sse_replays_events_from_the_query_parameter() {
        let (base, server) = start_server(test_state()).await;
        let client = reqwest::Client::new();

        let response = client
            .get(format!("{base}/api/events"))
            .send()
            .await
            .unwrap();
        let mut stream = Box::pin(response.bytes_stream());
        let mut buffer = String::new();
        read_until(&mut stream, &mut buffer, "\n\n").await;

        client
            .post(format!("{base}/api/volume"))
            .json(&VolumeRequest { volume: 80 })
            .send()
            .await
            .unwrap();
        read_until(
            &mut stream,
            &mut buffer,
            "\"volume\":80,\"repeat\":\"off\",\"shuffle\":false}}}",
        )
        .await;
        let resume_from = last_event_id(&buffer);
        drop(stream);

        client
            .post(format!("{base}/api/volume"))
            .json(&VolumeRequest { volume: 60 })
            .send()
            .await
            .unwrap();

        let response = client
            .get(format!("{base}/api/events?last_event_id={resume_from}"))
            .send()
            .await
            .unwrap();
        let mut stream = Box::pin(response.bytes_stream());
        let mut replay = String::new();
        read_until(
            &mut stream,
            &mut replay,
            "\"volume\":60,\"repeat\":\"off\",\"shuffle\":false}}}",
        )
        .await;
        assert!(!replay.contains("event: snapshot"));
        server.abort();
    }

    #[tokio::test]
    async fn sse_unknown_last_event_id_resyncs_with_snapshot() {
        let (base, server) = start_server(test_state()).await;
        let client = reqwest::Client::new();
        let response = client
            .get(format!("{base}/api/events"))
            .header("Last-Event-ID", "9999")
            .send()
            .await
            .unwrap();
        let mut stream = Box::pin(response.bytes_stream());
        let mut buffer = String::new();
        read_until(&mut stream, &mut buffer, "\n\n").await;
        assert_eq!(last_event_id(&buffer), 0);
        server.abort();
    }

    #[tokio::test]
    async fn serves_web_ui_and_spa_fallback() {
        let (base, server) = start_server(test_state()).await;
        let client = reqwest::Client::new();

        let root = client.get(format!("{base}/")).send().await.unwrap();
        assert_eq!(root.status(), 200);
        assert!(
            root.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
        assert!(root.text().await.unwrap().contains("id=\"app\""));

        let deep = client
            .get(format!("{base}/library/playlist"))
            .send()
            .await
            .unwrap();
        assert_eq!(deep.status(), 200);
        assert!(deep.text().await.unwrap().contains("id=\"app\""));

        let missing_asset = client
            .get(format!("{base}/assets/missing.js"))
            .send()
            .await
            .unwrap();
        assert_eq!(missing_asset.status(), reqwest::StatusCode::NOT_FOUND);

        let missing_api = client
            .get(format!("{base}/api/missing"))
            .send()
            .await
            .unwrap();
        assert_eq!(missing_api.status(), reqwest::StatusCode::NOT_FOUND);
        server.abort();
    }

    #[tokio::test]
    async fn serves_cached_cover_art_with_headers() {
        let dir = tempfile::tempdir().unwrap();
        let covers = Arc::new(FakeCovers::new());
        let artwork = Arc::new(ArtworkCache::with_fetcher(
            dir.path().join("covers"),
            0,
            covers.clone(),
        ));
        let (base, server) = start_server(test_state_with_artwork(artwork)).await;
        let client = reqwest::Client::new();

        let response = client
            .get(format!("{base}/api/tracks/youtube/abcdefghijk/cover"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "image/jpeg");
        assert_eq!(
            response.headers()["cache-control"],
            "public, max-age=604800, immutable"
        );
        let etag = response.headers()["etag"].to_str().unwrap().to_string();
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"fake-jpeg-data");

        let cached = client
            .get(format!("{base}/api/tracks/youtube/abcdefghijk/cover"))
            .header("If-None-Match", etag)
            .send()
            .await
            .unwrap();
        assert_eq!(cached.status(), reqwest::StatusCode::NOT_MODIFIED);
        assert_eq!(covers.calls(), 1);
        assert!(
            std::fs::read_dir(dir.path().join("covers"))
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| entry
                    .path()
                    .extension()
                    .is_none_or(|extension| extension != "tmp"))
        );
        server.abort();
    }

    #[tokio::test]
    async fn playlist_mutations_forward_validated_requests() {
        let music = Arc::new(MockMusicApi::default());
        let (base, server) = start_server(test_state_with(music.clone())).await;
        let client = reqwest::Client::new();

        let created = client
            .post(format!("{base}/api/playlists"))
            .json(&serde_json::json!({
                "title": "  New Mix  ",
                "description": "  mix  ",
                "privacy": "unlisted"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(created.status(), reqwest::StatusCode::CREATED);
        let created: serde_json::Value = created.json().await.unwrap();
        assert_eq!(created["id"], "VLnew");
        assert_eq!(created["name"], "New Mix");

        let renamed = client
            .patch(format!("{base}/api/playlists/VLnew"))
            .json(&UpdatePlaylistRequest {
                title: "  Renamed  ".to_string(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(renamed.status(), reqwest::StatusCode::OK);
        let renamed: serde_json::Value = renamed.json().await.unwrap();
        assert_eq!(renamed["name"], "Renamed");

        let deleted = client
            .delete(format!("{base}/api/playlists/VLnew"))
            .send()
            .await
            .unwrap();
        assert_eq!(deleted.status(), reqwest::StatusCode::OK);
        assert_eq!(
            deleted.json::<serde_json::Value>().await.unwrap()["ok"],
            true
        );

        let added = client
            .post(format!("{base}/api/playlists/VLnew/tracks"))
            .json(&AddPlaylistTracksRequest {
                provider: ProviderId::Youtube,
                track_ids: vec!["abcdefghijk".to_string()],
            })
            .send()
            .await
            .unwrap();
        assert_eq!(added.status(), reqwest::StatusCode::OK);
        assert_eq!(added.json::<Vec<Track>>().await.unwrap().len(), 1);

        let removed = client
            .post(format!("{base}/api/playlists/VLnew/tracks/remove"))
            .json(&RemovePlaylistTracksRequest { indices: vec![0] })
            .send()
            .await
            .unwrap();
        assert_eq!(removed.status(), reqwest::StatusCode::OK);

        let moved = client
            .post(format!("{base}/api/playlists/VLnew/tracks/move"))
            .json(&MovePlaylistTrackRequest { from: 0, to: 1 })
            .send()
            .await
            .unwrap();
        assert_eq!(moved.status(), reqwest::StatusCode::OK);

        let calls = music.edits.lock().unwrap().clone();
        assert!(calls.contains(&"create New Mix".to_string()));
        assert!(calls.contains(&"rename VLnew Renamed".to_string()));
        assert!(calls.contains(&"delete VLnew".to_string()));
        assert!(calls.contains(&"add VLnew abcdefghijk".to_string()));
        assert!(calls.contains(&"remove VLnew [0]".to_string()));
        assert!(calls.contains(&"move VLnew 0 1".to_string()));
        server.abort();
    }

    #[tokio::test]
    async fn rejects_invalid_and_unavailable_covers() {
        let dir = tempfile::tempdir().unwrap();
        let covers = Arc::new(FakeCovers::new());
        let artwork = Arc::new(ArtworkCache::with_fetcher(
            dir.path().join("covers"),
            0,
            covers.clone(),
        ));
        let (base, server) = start_server(test_state_with_artwork(artwork)).await;
        let client = reqwest::Client::new();

        let invalid = client
            .get(format!("{base}/api/tracks/youtube/short/cover"))
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), reqwest::StatusCode::NOT_FOUND);
        let body: serde_json::Value = invalid.json().await.unwrap();
        assert_eq!(body["error"]["code"], "not_found");
        assert_eq!(covers.calls(), 0);

        let missing = client
            .get(format!("{base}/api/tracks/youtube/zzzzzzzzzzz/cover"))
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);
        assert_eq!(covers.calls(), 1);
        server.abort();
    }

    #[tokio::test]
    async fn playlist_mutations_reject_invalid_input() {
        let music = Arc::new(MockMusicApi::default());
        let (base, server) = start_server(test_state_with(music.clone())).await;
        let client = reqwest::Client::new();

        for body in [
            serde_json::json!({"title": "   "}),
            serde_json::json!({"title": "Bad <name>"}),
            serde_json::json!({"title": "x".repeat(151)}),
            serde_json::json!({"title": "ok", "description": "x".repeat(5001)}),
        ] {
            let response = client
                .post(format!("{base}/api/playlists"))
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                reqwest::StatusCode::BAD_REQUEST,
                "{body}"
            );
        }

        let bad_video = client
            .post(format!("{base}/api/playlists/VL1/tracks"))
            .json(&AddPlaylistTracksRequest {
                provider: ProviderId::Youtube,
                track_ids: vec!["short".to_string()],
            })
            .send()
            .await
            .unwrap();
        assert_eq!(bad_video.status(), reqwest::StatusCode::BAD_REQUEST);

        let duplicate_video = client
            .post(format!("{base}/api/playlists/VL1/tracks"))
            .json(&AddPlaylistTracksRequest {
                provider: ProviderId::Youtube,
                track_ids: vec!["abcdefghijk".to_string(), "abcdefghijk".to_string()],
            })
            .send()
            .await
            .unwrap();
        assert_eq!(duplicate_video.status(), reqwest::StatusCode::BAD_REQUEST);

        let empty_remove = client
            .post(format!("{base}/api/playlists/VL1/tracks/remove"))
            .json(&RemovePlaylistTracksRequest { indices: vec![] })
            .send()
            .await
            .unwrap();
        assert_eq!(empty_remove.status(), reqwest::StatusCode::BAD_REQUEST);

        let duplicate_remove = client
            .post(format!("{base}/api/playlists/VL1/tracks/remove"))
            .json(&RemovePlaylistTracksRequest {
                indices: vec![0, 0],
            })
            .send()
            .await
            .unwrap();
        assert_eq!(duplicate_remove.status(), reqwest::StatusCode::BAD_REQUEST);

        let bad_id = client
            .patch(format!("{base}/api/playlists/bad!id"))
            .json(&UpdatePlaylistRequest {
                title: "ok".to_string(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(bad_id.status(), reqwest::StatusCode::BAD_REQUEST);

        assert!(music.edits.lock().unwrap().is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn playlist_edit_upstream_failures_are_502() {
        let (base, server) = start_server(test_state_with(Arc::new(FailingMusicApi))).await;
        let response = reqwest::Client::new()
            .post(format!("{base}/api/playlists"))
            .json(&CreatePlaylistRequest {
                title: "Mix".to_string(),
                description: String::new(),
                privacy: Default::default(),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], "upstream_error");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("upstream unavailable")
        );
        server.abort();
    }

    #[test]
    fn playlist_failures_map_to_http_statuses() {
        let invalid = playlist_failure(PlaylistEditError::Invalid("bad".to_string()));
        assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
        assert_eq!(invalid.error.code, "bad_request");

        let upstream = playlist_failure(PlaylistEditError::Upstream("down".to_string()));
        assert_eq!(upstream.status, StatusCode::BAD_GATEWAY);
        assert_eq!(upstream.error.code, "upstream_error");
    }

    #[tokio::test]
    async fn missing_playlist_is_404() {
        let (base, server) = start_server(test_state()).await;
        let response = reqwest::Client::new()
            .get(format!("{base}/api/playlists/missing"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
        server.abort();
    }
}
