use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};
use ytermusicd_api::{
    CreatePlaylistRequest, DownloadSummary, OkResponse, Playlist, PlaylistDetail, PlaylistPrivacy,
    ProviderId, ProviderInfo, Queue, QueueItem, RepeatMode, RssFeed, SearchResults, Status, Track,
    TrackKey, UpdatePlaylistRequest,
};
use ytermusicd_client::DaemonClient;

fn to_mcp(error: ytermusicd_client::DaemonError) -> McpError {
    // Keep the daemon code in the error data, so a client can tell a bad
    // argument from an upstream failure without parsing the message.
    let data = serde_json::json!({ "code": error.code });
    let message = format!("daemon error ({}): {}", error.code, error.message);
    match error.code.as_str() {
        // An expired session is actionable by the caller: refresh
        // headers.txt and restart the daemon. It is not an upstream
        // failure, so it gets its own code instead of internal_error.
        "bad_request" | "not_found" | "auth_expired" => {
            McpError::invalid_params(message, Some(data))
        }
        _ => McpError::internal_error(message, Some(data)),
    }
}

#[derive(Debug, Clone)]
pub struct McpServer {
    client: DaemonClient,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    pub fn new(client: DaemonClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// Text to search for.
    pub query: String,
    /// Provider to search. Default: youtube.
    #[serde(default)]
    pub provider: ProviderId,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PlaylistArgs {
    /// Playlist ID from list_playlists or search.
    pub playlist_id: String,
    /// Provider the playlist belongs to. Default: youtube.
    #[serde(default)]
    pub provider: ProviderId,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListPlaylistsArgs {
    /// Provider to list. Default: youtube.
    #[serde(default)]
    pub provider: ProviderId,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreatePlaylistArgs {
    /// Playlist title.
    pub title: String,
    /// Optional playlist description.
    #[serde(default)]
    pub description: Option<String>,
    /// Playlist privacy: public, private or unlisted. Default: private.
    #[serde(default)]
    pub privacy: Option<PlaylistPrivacy>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenamePlaylistArgs {
    /// Playlist ID from list_playlists or search.
    pub playlist_id: String,
    /// New playlist title.
    pub title: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PlaylistTracksArgs {
    /// Playlist ID from list_playlists or search.
    pub playlist_id: String,
    /// Provider of the tracks. Default: youtube.
    #[serde(default)]
    pub provider: ProviderId,
    /// Track IDs to add.
    pub track_ids: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RemovePlaylistTracksArgs {
    /// Playlist ID from list_playlists or search.
    pub playlist_id: String,
    /// Track indices from get_playlist.
    pub indices: Vec<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MovePlaylistTrackArgs {
    /// Playlist ID from list_playlists or search.
    pub playlist_id: String,
    /// Current track index.
    pub from: usize,
    /// New track index.
    pub to: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TrackArgs {
    /// Track object from search or get_playlist.
    pub track: Track,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VolumeArgs {
    /// Volume from 0 to 100.
    pub volume: u8,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SeekArgs {
    /// Position in seconds from the start of the track.
    pub position_secs: f64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ShuffleArgs {
    /// Enable shuffle when true.
    pub enabled: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RepeatArgs {
    /// Repeat mode: off, one or all.
    pub repeat: RepeatMode,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueueIndexArgs {
    /// Queue index from the queue view.
    pub index: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReorderQueueArgs {
    /// Current queue index.
    pub from: usize,
    /// New queue index.
    pub to: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RetryDownloadArgs {
    /// Provider of the failed download. Default: youtube.
    #[serde(default)]
    pub provider: ProviderId,
    /// Track ID of the failed download.
    pub track_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DownloadStatusArgs {
    /// Optional provider. Omit it to list all queue downloads.
    #[serde(default)]
    pub provider: Option<ProviderId>,
    /// Optional track ID. Omit it to list all queue downloads.
    #[serde(default)]
    pub track_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RssAddFeedArgs {
    /// Feed URL to add.
    pub url: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RssFeedArgs {
    /// Feed ID from rss_list_feeds.
    pub feed_id: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DownloadStatusOutput {
    pub summary: DownloadSummary,
    pub tracks: Vec<QueueItem>,
}

#[tool_router]
impl McpServer {
    /// List every provider the daemon knows, with its capabilities
    /// and its auth state.
    #[tool(description = "List the providers the daemon knows, with capabilities and auth state")]
    async fn list_providers(&self) -> Result<Json<Vec<ProviderInfo>>, McpError> {
        self.client.providers().await.map(Json).map_err(to_mcp)
    }

    /// Search YouTube Music for tracks and playlists.
    #[tool(description = "Search one provider for tracks and playlists")]
    async fn search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<Json<SearchResults>, McpError> {
        self.client
            .search(args.provider, &args.query)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// List the playlists in the YouTube Music library.
    #[tool(description = "List the playlists in one provider library")]
    async fn list_playlists(
        &self,
        Parameters(args): Parameters<ListPlaylistsArgs>,
    ) -> Result<Json<Vec<Playlist>>, McpError> {
        self.client
            .playlists(args.provider)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Get one playlist and its tracks.
    #[tool(description = "Get one playlist and its tracks")]
    async fn get_playlist(
        &self,
        Parameters(args): Parameters<PlaylistArgs>,
    ) -> Result<Json<PlaylistDetail>, McpError> {
        self.client
            .playlist(args.provider, &args.playlist_id)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Create a playlist.
    #[tool(description = "Create a playlist")]
    async fn create_playlist(
        &self,
        Parameters(args): Parameters<CreatePlaylistArgs>,
    ) -> Result<Json<Playlist>, McpError> {
        let request = CreatePlaylistRequest {
            title: args.title,
            description: args.description.unwrap_or_default(),
            privacy: args.privacy.unwrap_or_default(),
        };
        self.client
            .create_playlist(&request)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Rename a playlist.
    #[tool(description = "Rename a playlist")]
    async fn rename_playlist(
        &self,
        Parameters(args): Parameters<RenamePlaylistArgs>,
    ) -> Result<Json<Playlist>, McpError> {
        self.client
            .update_playlist(
                &args.playlist_id,
                &UpdatePlaylistRequest { title: args.title },
            )
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Delete a playlist.
    #[tool(description = "Delete a playlist")]
    async fn delete_playlist(
        &self,
        Parameters(args): Parameters<PlaylistArgs>,
    ) -> Result<Json<OkResponse>, McpError> {
        self.client
            .delete_playlist(&args.playlist_id)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Add tracks to a playlist.
    #[tool(description = "Add tracks to a playlist")]
    async fn add_playlist_tracks(
        &self,
        Parameters(args): Parameters<PlaylistTracksArgs>,
    ) -> Result<Json<Vec<Track>>, McpError> {
        self.client
            .add_playlist_tracks(&args.playlist_id, args.provider, args.track_ids)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Remove tracks from a playlist by index.
    #[tool(description = "Remove tracks from a playlist by index")]
    async fn remove_playlist_tracks(
        &self,
        Parameters(args): Parameters<RemovePlaylistTracksArgs>,
    ) -> Result<Json<Vec<Track>>, McpError> {
        self.client
            .remove_playlist_tracks(&args.playlist_id, args.indices)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Move a track within a playlist.
    #[tool(description = "Move a track within a playlist")]
    async fn move_playlist_track(
        &self,
        Parameters(args): Parameters<MovePlaylistTrackArgs>,
    ) -> Result<Json<Vec<Track>>, McpError> {
        self.client
            .move_playlist_track(&args.playlist_id, args.from, args.to)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Replace the queue with a playlist and start playback.
    #[tool(description = "Replace the queue with a playlist and start playback")]
    async fn play_playlist(
        &self,
        Parameters(args): Parameters<PlaylistArgs>,
    ) -> Result<Json<Status>, McpError> {
        self.client
            .play_playlist(args.provider, &args.playlist_id)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Play a track immediately.
    #[tool(description = "Play a track immediately")]
    async fn play_track(
        &self,
        Parameters(args): Parameters<TrackArgs>,
    ) -> Result<Json<Status>, McpError> {
        self.client
            .play_track(args.track)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Add a track to the queue.
    #[tool(description = "Add a track to the queue")]
    async fn queue_add(
        &self,
        Parameters(args): Parameters<TrackArgs>,
    ) -> Result<Json<Queue>, McpError> {
        self.client
            .queue_add(args.track)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Get the current track and playback state.
    #[tool(description = "Get the current track and playback state")]
    async fn now_playing(&self) -> Result<Json<Status>, McpError> {
        self.client.status().await.map(Json).map_err(to_mcp)
    }

    /// Pause playback.
    #[tool(description = "Pause playback")]
    async fn pause(&self) -> Result<Json<Status>, McpError> {
        self.client.pause().await.map(Json).map_err(to_mcp)
    }

    /// Resume playback.
    #[tool(description = "Resume playback")]
    async fn resume(&self) -> Result<Json<Status>, McpError> {
        self.client.resume().await.map(Json).map_err(to_mcp)
    }

    /// Skip to the next track.
    #[tool(description = "Skip to the next track")]
    async fn next(&self) -> Result<Json<Status>, McpError> {
        self.client.next().await.map(Json).map_err(to_mcp)
    }

    /// Go to the previous track.
    #[tool(description = "Go to the previous track")]
    async fn previous(&self) -> Result<Json<Status>, McpError> {
        self.client.previous().await.map(Json).map_err(to_mcp)
    }

    /// Set the playback volume.
    #[tool(description = "Set the playback volume (0-100)")]
    async fn volume(
        &self,
        Parameters(args): Parameters<VolumeArgs>,
    ) -> Result<Json<Status>, McpError> {
        if args.volume > 100 {
            return Err(McpError::invalid_params(
                "volume must be between 0 and 100",
                None,
            ));
        }
        self.client
            .volume(args.volume)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Seek to a position in the current track.
    #[tool(description = "Seek to a position in the current track in seconds")]
    async fn seek(&self, Parameters(args): Parameters<SeekArgs>) -> Result<Json<Status>, McpError> {
        if !args.position_secs.is_finite() || args.position_secs < 0.0 {
            return Err(McpError::invalid_params(
                "position_secs must be a finite number at or after 0",
                None,
            ));
        }
        self.client
            .seek(args.position_secs)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Enable or disable shuffle.
    #[tool(description = "Enable or disable shuffle")]
    async fn shuffle(
        &self,
        Parameters(args): Parameters<ShuffleArgs>,
    ) -> Result<Json<Status>, McpError> {
        self.client
            .shuffle(args.enabled)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Set the repeat mode.
    #[tool(description = "Set the repeat mode (off, one or all)")]
    async fn repeat(
        &self,
        Parameters(args): Parameters<RepeatArgs>,
    ) -> Result<Json<Status>, McpError> {
        self.client
            .repeat(args.repeat)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Remove one track from the queue.
    #[tool(description = "Remove one track from the queue by index")]
    async fn queue_remove(
        &self,
        Parameters(args): Parameters<QueueIndexArgs>,
    ) -> Result<Json<Queue>, McpError> {
        self.client
            .queue_remove(args.index)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Move one track within the queue.
    #[tool(description = "Move one track within the queue")]
    async fn queue_reorder(
        &self,
        Parameters(args): Parameters<ReorderQueueArgs>,
    ) -> Result<Json<Queue>, McpError> {
        self.client
            .queue_reorder(args.from, args.to)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Move one queue track to play next.
    #[tool(description = "Move one queue track to play next")]
    async fn queue_play_next(
        &self,
        Parameters(args): Parameters<QueueIndexArgs>,
    ) -> Result<Json<Queue>, McpError> {
        self.client
            .queue_play_next(args.index)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Remove all tracks from the queue.
    #[tool(description = "Remove all tracks from the queue")]
    async fn queue_clear(&self) -> Result<Json<Queue>, McpError> {
        self.client.queue_clear().await.map(Json).map_err(to_mcp)
    }

    /// Retry a failed download.
    #[tool(description = "Retry a failed download by track ID")]
    async fn retry_download(
        &self,
        Parameters(args): Parameters<RetryDownloadArgs>,
    ) -> Result<Json<OkResponse>, McpError> {
        self.client
            .retry_download(&TrackKey::new(args.provider, args.track_id))
            .await
            .map(|()| Json(OkResponse { ok: true }))
            .map_err(to_mcp)
    }

    /// Report download status for queue tracks.
    #[tool(description = "Report download status for queue tracks")]
    async fn download_status(
        &self,
        Parameters(args): Parameters<DownloadStatusArgs>,
    ) -> Result<Json<DownloadStatusOutput>, McpError> {
        let (status, queue) = tokio::join!(self.client.status(), self.client.queue());
        let status = status.map_err(to_mcp)?;
        let queue = queue.map_err(to_mcp)?;
        let tracks = match args.track_id {
            Some(track_id) => {
                let key = TrackKey::new(args.provider.unwrap_or_default(), track_id);
                let item = queue.items.into_iter().find(|item| {
                    item.track.provider == key.provider && item.track.track_id == key.id
                });
                match item {
                    Some(item) => vec![item],
                    None => {
                        return Err(McpError::invalid_params(
                            format!("no queued track with track ID {key}"),
                            None,
                        ));
                    }
                }
            }
            None => queue.items,
        };
        Ok(Json(DownloadStatusOutput {
            summary: status.downloads,
            tracks,
        }))
    }

    /// List the RSS feeds the daemon follows.
    #[tool(description = "List the RSS feeds the daemon follows")]
    async fn rss_list_feeds(&self) -> Result<Json<Vec<RssFeed>>, McpError> {
        self.client.rss_feeds().await.map(Json).map_err(to_mcp)
    }

    /// Add an RSS feed URL. The daemon fetches it at once.
    #[tool(description = "Add an RSS feed URL and fetch its episodes")]
    async fn rss_add_feed(
        &self,
        Parameters(args): Parameters<RssAddFeedArgs>,
    ) -> Result<Json<RssFeed>, McpError> {
        self.client
            .add_rss_feed(&args.url)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Remove an RSS feed and drop its episodes from the index.
    #[tool(description = "Remove an RSS feed and its episodes")]
    async fn rss_remove_feed(
        &self,
        Parameters(args): Parameters<RssFeedArgs>,
    ) -> Result<Json<OkResponse>, McpError> {
        self.client
            .remove_rss_feed(&args.feed_id)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Refresh one RSS feed by its ID.
    #[tool(description = "Refresh one RSS feed by its ID")]
    async fn rss_refresh_feed(
        &self,
        Parameters(args): Parameters<RssFeedArgs>,
    ) -> Result<Json<RssFeed>, McpError> {
        self.client
            .refresh_rss_feed(&args.feed_id)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// Refresh every RSS feed. A feed that fails keeps its old episodes.
    #[tool(description = "Refresh every RSS feed")]
    async fn rss_refresh_feeds(&self) -> Result<Json<Vec<RssFeed>>, McpError> {
        self.client
            .refresh_rss_feeds()
            .await
            .map(Json)
            .map_err(to_mcp)
    }
}

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Tools for the ytermusicd daemon. Playlist editing, seek, shuffle, repeat, and queue management are available. Lyrics and HTTP transport are post-v1.",
        )
    }
}

/// Run the stdio MCP proxy. It exits when stdin closes.
pub async fn run(base_url: Option<String>, debug: bool) -> std::io::Result<()> {
    let base_url = base_url.unwrap_or_else(|| ytermusicd_client::DEFAULT_URL.to_string());
    let mut client = DaemonClient::new(base_url).map_err(std::io::Error::other)?;
    client.set_debug(debug);
    let server = McpServer::new(client);
    log::info!(
        "MCP server ready with tools: {}",
        server.tool_names().join(", ")
    );
    let service = server.serve(stdio()).await.map_err(std::io::Error::other)?;
    service.waiting().await.map_err(std::io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json as AxumJson, Router,
        extract::{Path, State},
        http::StatusCode,
        response::{IntoResponse, Response},
        routing::{delete, get, post},
    };
    use rmcp::model::ErrorCode;
    use serde_json::json;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    use ytermusicd_api::{
        AddPlaylistTracksRequest, ApiError, AuthState, AuthStatus, DownloadStatus, ErrorEnvelope,
        MovePlaylistTrackRequest, OkResponse, PlayPlaylistRequest, PlayTrackRequest,
        PlaybackSnapshot, PlaybackState, Playlist, PlaylistPrivacy, QueueItem, QueueRequest,
        RemovePlaylistTracksRequest, RepeatMode,
    };
    use ytermusicd_client::{DaemonError, encode};

    #[derive(Clone, Default)]
    struct MockState {
        calls: Arc<Mutex<Vec<String>>>,
        fail: Arc<AtomicBool>,
        missing: Arc<AtomicBool>,
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
            version: "0.1.0".to_string(),
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
            downloads: DownloadSummary {
                queued: 1,
                active: 0,
                completed: 1,
                retrying: 0,
                failed: 0,
            },
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
            .route(
                "/api/playlists",
                get(mock_playlists).post(mock_create_playlist),
            )
            .route(
                "/api/playlists/{id}",
                get(mock_playlist)
                    .patch(mock_update_playlist)
                    .delete(mock_delete_playlist),
            )
            .route("/api/playlists/{id}/tracks", post(mock_add_playlist_tracks))
            .route(
                "/api/playlists/{id}/tracks/remove",
                post(mock_remove_playlist_tracks),
            )
            .route(
                "/api/playlists/{id}/tracks/move",
                post(mock_move_playlist_track),
            )
            .route("/api/search", get(mock_search))
            .route(
                "/api/queue",
                get(mock_queue)
                    .post(mock_queue_add)
                    .delete(mock_queue_delete),
            )
            .route("/api/queue/remove", post(mock_queue_op))
            .route("/api/queue/reorder", post(mock_queue_op))
            .route("/api/queue/play-next", post(mock_queue_op))
            .route("/api/play/playlist", post(mock_play_playlist))
            .route("/api/play/track", post(mock_play_track))
            .route("/api/pause", post(mock_status_post))
            .route("/api/resume", post(mock_status_post))
            .route("/api/next", post(mock_status_post))
            .route("/api/previous", post(mock_status_post))
            .route("/api/seek", post(mock_status_op))
            .route("/api/shuffle", post(mock_status_op))
            .route("/api/repeat", post(mock_status_op))
            .route("/api/downloads/retry", post(mock_retry_download))
            .route("/api/volume", post(mock_volume))
            .route(
                "/api/rss/feeds",
                get(mock_rss_feeds).post(mock_rss_add_feed),
            )
            .route("/api/rss/feeds/refresh", post(mock_rss_refresh_feeds))
            .route("/api/rss/feeds/{id}", delete(mock_rss_remove_feed))
            .route("/api/rss/feeds/{id}/refresh", post(mock_rss_refresh_feed))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{addr}"), handle)
    }

    async fn mock_status(State(state): State<MockState>) -> Result<AxumJson<Status>, StatusCode> {
        state
            .calls
            .lock()
            .unwrap()
            .push("GET /api/status".to_string());
        if state.fail.load(Ordering::SeqCst) {
            return Err(StatusCode::BAD_GATEWAY);
        }
        Ok(AxumJson(status()))
    }

    async fn mock_providers(State(state): State<MockState>) -> AxumJson<Vec<ProviderInfo>> {
        state
            .calls
            .lock()
            .unwrap()
            .push("GET /api/providers".to_string());
        AxumJson(vec![ProviderInfo::new(
            ProviderId::Youtube,
            AuthStatus {
                state: AuthState::Ok,
                source: None,
                message: None,
            },
        )])
    }

    async fn mock_playlists(State(state): State<MockState>) -> AxumJson<Vec<Playlist>> {
        state
            .calls
            .lock()
            .unwrap()
            .push("GET /api/playlists".to_string());
        AxumJson(vec![Playlist {
            id: "p1".to_string(),
            name: "Playlist One".to_string(),
            subtitle: "Artist".to_string(),
        }])
    }

    async fn mock_playlist(State(state): State<MockState>, Path(id): Path<String>) -> Response {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("GET /api/playlists/{id}"));
        if state.missing.load(Ordering::SeqCst) {
            return (
                StatusCode::NOT_FOUND,
                AxumJson(ErrorEnvelope {
                    error: ApiError {
                        code: "not_found".to_string(),
                        message: format!("playlist {id} not found"),
                    },
                }),
            )
                .into_response();
        }
        AxumJson(PlaylistDetail {
            playlist: Playlist {
                id: id.clone(),
                name: "Playlist One".to_string(),
                subtitle: "Artist".to_string(),
            },
            tracks: vec![track("v1")],
        })
        .into_response()
    }

    async fn mock_create_playlist(
        State(state): State<MockState>,
        AxumJson(request): AxumJson<CreatePlaylistRequest>,
    ) -> (StatusCode, AxumJson<Playlist>) {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST /api/playlists {}", request.title));
        (
            StatusCode::CREATED,
            AxumJson(Playlist {
                id: "p9".to_string(),
                name: request.title,
                subtitle: "Private playlist".to_string(),
            }),
        )
    }

    async fn mock_update_playlist(
        State(state): State<MockState>,
        Path(id): Path<String>,
        AxumJson(request): AxumJson<UpdatePlaylistRequest>,
    ) -> AxumJson<Playlist> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("PATCH /api/playlists/{id}"));
        AxumJson(Playlist {
            id,
            name: request.title,
            subtitle: "Artist".to_string(),
        })
    }

    async fn mock_delete_playlist(
        State(state): State<MockState>,
        Path(id): Path<String>,
    ) -> AxumJson<OkResponse> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("DELETE /api/playlists/{id}"));
        AxumJson(OkResponse { ok: true })
    }

    async fn mock_add_playlist_tracks(
        State(state): State<MockState>,
        Path(id): Path<String>,
        AxumJson(_request): AxumJson<AddPlaylistTracksRequest>,
    ) -> AxumJson<Vec<Track>> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST /api/playlists/{id}/tracks"));
        AxumJson(vec![track("v1")])
    }

    async fn mock_remove_playlist_tracks(
        State(state): State<MockState>,
        Path(id): Path<String>,
        AxumJson(_request): AxumJson<RemovePlaylistTracksRequest>,
    ) -> AxumJson<Vec<Track>> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST /api/playlists/{id}/tracks/remove"));
        AxumJson(vec![track("v1")])
    }

    async fn mock_move_playlist_track(
        State(state): State<MockState>,
        Path(id): Path<String>,
        AxumJson(_request): AxumJson<MovePlaylistTrackRequest>,
    ) -> AxumJson<Vec<Track>> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST /api/playlists/{id}/tracks/move"));
        AxumJson(vec![track("v1")])
    }

    async fn mock_search(State(state): State<MockState>) -> AxumJson<SearchResults> {
        state
            .calls
            .lock()
            .unwrap()
            .push("GET /api/search".to_string());
        AxumJson(SearchResults {
            tracks: vec![track("v1")],
            playlists: vec![],
        })
    }

    async fn mock_queue(State(state): State<MockState>) -> AxumJson<Queue> {
        state
            .calls
            .lock()
            .unwrap()
            .push("GET /api/queue".to_string());
        AxumJson(queue())
    }

    async fn mock_play_playlist(
        State(state): State<MockState>,
        AxumJson(_request): AxumJson<PlayPlaylistRequest>,
    ) -> AxumJson<Status> {
        state
            .calls
            .lock()
            .unwrap()
            .push("POST /api/play/playlist".to_string());
        AxumJson(status())
    }

    async fn mock_play_track(
        State(state): State<MockState>,
        AxumJson(_request): AxumJson<PlayTrackRequest>,
    ) -> AxumJson<Status> {
        state
            .calls
            .lock()
            .unwrap()
            .push("POST /api/play/track".to_string());
        AxumJson(status())
    }

    async fn mock_queue_add(
        State(state): State<MockState>,
        AxumJson(_request): AxumJson<QueueRequest>,
    ) -> AxumJson<Queue> {
        state
            .calls
            .lock()
            .unwrap()
            .push("POST /api/queue".to_string());
        AxumJson(queue())
    }

    async fn mock_status_post(State(state): State<MockState>) -> AxumJson<Status> {
        state
            .calls
            .lock()
            .unwrap()
            .push("POST transport".to_string());
        AxumJson(status())
    }

    async fn mock_status_op(
        State(state): State<MockState>,
        uri: axum::http::Uri,
    ) -> AxumJson<Status> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST {}", uri.path()));
        AxumJson(status())
    }

    async fn mock_queue_op(
        State(state): State<MockState>,
        uri: axum::http::Uri,
    ) -> AxumJson<Queue> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST {}", uri.path()));
        AxumJson(queue())
    }

    async fn mock_queue_delete(State(state): State<MockState>) -> AxumJson<Queue> {
        state
            .calls
            .lock()
            .unwrap()
            .push("DELETE /api/queue".to_string());
        AxumJson(queue())
    }

    async fn mock_retry_download(State(state): State<MockState>) -> StatusCode {
        state
            .calls
            .lock()
            .unwrap()
            .push("POST /api/downloads/retry".to_string());
        StatusCode::OK
    }

    fn rss_feed() -> RssFeed {
        RssFeed {
            id: "0123456789abcdef".to_string(),
            url: "https://example.com/feed.xml".to_string(),
            title: "Test Tones".to_string(),
            episode_count: 2,
            last_fetched: Some(100),
        }
    }

    async fn mock_rss_feeds(State(state): State<MockState>) -> AxumJson<Vec<RssFeed>> {
        state
            .calls
            .lock()
            .unwrap()
            .push("GET /api/rss/feeds".to_string());
        AxumJson(vec![rss_feed()])
    }

    async fn mock_rss_add_feed(
        State(state): State<MockState>,
        AxumJson(request): AxumJson<serde_json::Value>,
    ) -> (StatusCode, AxumJson<RssFeed>) {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST /api/rss/feeds {}", request["url"]));
        (StatusCode::CREATED, AxumJson(rss_feed()))
    }

    async fn mock_rss_remove_feed(
        State(state): State<MockState>,
        Path(id): Path<String>,
    ) -> AxumJson<OkResponse> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("DELETE /api/rss/feeds/{id}"));
        AxumJson(OkResponse { ok: true })
    }

    async fn mock_rss_refresh_feed(
        State(state): State<MockState>,
        Path(id): Path<String>,
    ) -> AxumJson<RssFeed> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST /api/rss/feeds/{id}/refresh"));
        AxumJson(rss_feed())
    }

    async fn mock_rss_refresh_feeds(State(state): State<MockState>) -> AxumJson<Vec<RssFeed>> {
        state
            .calls
            .lock()
            .unwrap()
            .push("POST /api/rss/feeds/refresh".to_string());
        AxumJson(vec![rss_feed()])
    }

    async fn mock_volume(
        State(state): State<MockState>,
        AxumJson(request): AxumJson<serde_json::Value>,
    ) -> AxumJson<Status> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("POST /api/volume {}", request["volume"]));
        AxumJson(status())
    }

    async fn server() -> (McpServer, MockState, tokio::task::JoinHandle<()>) {
        let state = MockState::default();
        let (base, handle) = mock_router(state.clone()).await;
        (
            McpServer::new(DaemonClient::new(base).unwrap()),
            state,
            handle,
        )
    }

    fn expect_error<T>(result: Result<T, McpError>) -> McpError {
        match result {
            Ok(_) => panic!("expected an MCP error"),
            Err(error) => error,
        }
    }

    #[test]
    fn lists_the_tools() {
        let server = McpServer::new(DaemonClient::new("http://127.0.0.1:1").unwrap());
        let mut names = server.tool_names();
        names.sort();
        assert_eq!(
            names,
            vec![
                "add_playlist_tracks",
                "create_playlist",
                "delete_playlist",
                "download_status",
                "get_playlist",
                "list_playlists",
                "list_providers",
                "move_playlist_track",
                "next",
                "now_playing",
                "pause",
                "play_playlist",
                "play_track",
                "previous",
                "queue_add",
                "queue_clear",
                "queue_play_next",
                "queue_remove",
                "queue_reorder",
                "remove_playlist_tracks",
                "rename_playlist",
                "repeat",
                "resume",
                "retry_download",
                "rss_add_feed",
                "rss_list_feeds",
                "rss_refresh_feed",
                "rss_refresh_feeds",
                "rss_remove_feed",
                "search",
                "seek",
                "shuffle",
                "volume",
            ]
        );
    }

    #[tokio::test]
    async fn lists_providers_with_capabilities() {
        let (server, state, handle) = server().await;
        let Json(providers) = server.list_providers().await.unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, ProviderId::Youtube);
        assert_eq!(providers[0].name, "YouTube Music");
        assert!(providers[0].default);
        assert!(providers[0].capabilities.search);
        assert!(
            state
                .calls
                .lock()
                .unwrap()
                .contains(&"GET /api/providers".to_string())
        );
        handle.abort();
    }

    #[tokio::test]
    async fn playlist_tools_call_the_daemon() {
        let (server, state, handle) = server().await;

        let Json(created) = server
            .create_playlist(Parameters(CreatePlaylistArgs {
                title: "Mix".to_string(),
                description: Some("desc".to_string()),
                privacy: Some(PlaylistPrivacy::Unlisted),
            }))
            .await
            .unwrap();
        assert_eq!(created.id, "p9");

        let Json(renamed) = server
            .rename_playlist(Parameters(RenamePlaylistArgs {
                playlist_id: "p1".to_string(),
                title: "New".to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(renamed.name, "New");

        let Json(deleted) = server
            .delete_playlist(Parameters(PlaylistArgs {
                playlist_id: "p1".to_string(),
                provider: ProviderId::Youtube,
            }))
            .await
            .unwrap();
        assert!(deleted.ok);

        let Json(added) = server
            .add_playlist_tracks(Parameters(PlaylistTracksArgs {
                playlist_id: "p1".to_string(),
                provider: ProviderId::Youtube,
                track_ids: vec!["v1".to_string()],
            }))
            .await
            .unwrap();
        assert_eq!(added[0].track_id, "v1");

        let Json(removed) = server
            .remove_playlist_tracks(Parameters(RemovePlaylistTracksArgs {
                playlist_id: "p1".to_string(),
                indices: vec![0],
            }))
            .await
            .unwrap();
        assert_eq!(removed.len(), 1);

        let Json(moved) = server
            .move_playlist_track(Parameters(MovePlaylistTrackArgs {
                playlist_id: "p1".to_string(),
                from: 0,
                to: 1,
            }))
            .await
            .unwrap();
        assert_eq!(moved.len(), 1);

        let calls = state.calls.lock().unwrap().clone();
        assert!(calls.contains(&"POST /api/playlists Mix".to_string()));
        assert!(calls.contains(&"PATCH /api/playlists/p1".to_string()));
        assert!(calls.contains(&"DELETE /api/playlists/p1".to_string()));
        assert!(calls.contains(&"POST /api/playlists/p1/tracks".to_string()));
        assert!(calls.contains(&"POST /api/playlists/p1/tracks/remove".to_string()));
        assert!(calls.contains(&"POST /api/playlists/p1/tracks/move".to_string()));
        handle.abort();
    }

    #[tokio::test]
    async fn tools_call_the_daemon_and_return_structured_data() {
        let (server, state, handle) = server().await;
        let track = track("v1");

        let Json(found) = server
            .search(Parameters(SearchArgs {
                query: "test".to_string(),
                provider: ProviderId::Youtube,
            }))
            .await
            .unwrap();
        assert_eq!(found.tracks[0].track_id, "v1");

        let Json(playlists) = server
            .list_playlists(Parameters(ListPlaylistsArgs {
                provider: ProviderId::Youtube,
            }))
            .await
            .unwrap();
        assert_eq!(playlists[0].id, "p1");

        let Json(detail) = server
            .get_playlist(Parameters(PlaylistArgs {
                playlist_id: "p1".to_string(),
                provider: ProviderId::Youtube,
            }))
            .await
            .unwrap();
        assert_eq!(detail.tracks.len(), 1);

        let Json(played) = server
            .play_track(Parameters(TrackArgs {
                track: track.clone(),
            }))
            .await
            .unwrap();
        assert_eq!(played.playback.state, PlaybackState::Playing);

        let Json(queued) = server
            .queue_add(Parameters(TrackArgs { track }))
            .await
            .unwrap();
        assert_eq!(queued.items.len(), 1);

        let Json(now) = server.now_playing().await.unwrap();
        assert_eq!(now.playback.volume, 50);

        let Json(volume) = server
            .volume(Parameters(VolumeArgs { volume: 75 }))
            .await
            .unwrap();
        assert_eq!(volume.playback.volume, 50);

        let Json(downloads) = server
            .download_status(Parameters(DownloadStatusArgs {
                provider: Some(ProviderId::Youtube),
                track_id: Some("v1".to_string()),
            }))
            .await
            .unwrap();
        assert_eq!(downloads.tracks.len(), 1);
        assert_eq!(downloads.summary.completed, 1);

        let calls = state.calls.lock().unwrap().clone();
        assert!(calls.contains(&"GET /api/search".to_string()));
        assert!(calls.contains(&"POST /api/play/track".to_string()));
        assert!(calls.contains(&"POST /api/volume 75".to_string()));
        handle.abort();
    }

    #[tokio::test]
    async fn command_tools_cover_transport() {
        let (server, _state, handle) = server().await;
        assert_eq!(
            server.pause().await.unwrap().0.playback.state,
            PlaybackState::Playing
        );
        assert_eq!(
            server.resume().await.unwrap().0.playback.state,
            PlaybackState::Playing
        );
        assert_eq!(
            server.next().await.unwrap().0.playback.state,
            PlaybackState::Playing
        );
        assert_eq!(
            server.previous().await.unwrap().0.playback.state,
            PlaybackState::Playing
        );
        assert_eq!(
            server
                .play_playlist(Parameters(PlaylistArgs {
                    playlist_id: "p1".to_string(),
                    provider: ProviderId::Youtube,
                }))
                .await
                .unwrap()
                .0
                .playback
                .state,
            PlaybackState::Playing
        );
        handle.abort();
    }

    #[tokio::test]
    async fn rejects_out_of_range_volume() {
        let (server, _state, handle) = server().await;
        let error = expect_error(server.volume(Parameters(VolumeArgs { volume: 101 })).await);
        assert!(error.message.contains("0 and 100"));
        handle.abort();
    }

    #[tokio::test]
    async fn rss_tools_call_the_daemon() {
        let (server, state, handle) = server().await;

        let Json(feeds) = server.rss_list_feeds().await.unwrap();
        assert_eq!(feeds.len(), 1);
        assert_eq!(feeds[0].title, "Test Tones");

        let Json(added) = server
            .rss_add_feed(Parameters(RssAddFeedArgs {
                url: "https://example.com/feed.xml".to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(added.episode_count, 2);

        let Json(refreshed) = server
            .rss_refresh_feed(Parameters(RssFeedArgs {
                feed_id: "0123456789abcdef".to_string(),
            }))
            .await
            .unwrap();
        assert_eq!(refreshed.id, "0123456789abcdef");

        let Json(all) = server.rss_refresh_feeds().await.unwrap();
        assert_eq!(all.len(), 1);

        let Json(removed) = server
            .rss_remove_feed(Parameters(RssFeedArgs {
                feed_id: "0123456789abcdef".to_string(),
            }))
            .await
            .unwrap();
        assert!(removed.ok);

        let calls = state.calls.lock().unwrap();
        assert!(calls.contains(&"GET /api/rss/feeds".to_string()));
        assert!(calls.contains(&"POST /api/rss/feeds/refresh".to_string()));
        assert!(calls.contains(&"DELETE /api/rss/feeds/0123456789abcdef".to_string()));
        handle.abort();
    }

    #[tokio::test]
    async fn new_tools_call_the_daemon() {
        let (server, state, handle) = server().await;

        let Json(seeked) = server
            .seek(Parameters(SeekArgs {
                position_secs: 30.0,
            }))
            .await
            .unwrap();
        assert_eq!(seeked.playback.state, PlaybackState::Playing);

        let error = expect_error(
            server
                .seek(Parameters(SeekArgs {
                    position_secs: -1.0,
                }))
                .await,
        );
        assert!(error.message.contains("position_secs"));

        let Json(shuffled) = server
            .shuffle(Parameters(ShuffleArgs { enabled: true }))
            .await
            .unwrap();
        assert_eq!(shuffled.playback.state, PlaybackState::Playing);

        let Json(repeated) = server
            .repeat(Parameters(RepeatArgs {
                repeat: RepeatMode::One,
            }))
            .await
            .unwrap();
        assert_eq!(repeated.playback.state, PlaybackState::Playing);

        let Json(removed) = server
            .queue_remove(Parameters(QueueIndexArgs { index: 0 }))
            .await
            .unwrap();
        assert_eq!(removed.items.len(), 1);

        let Json(reordered) = server
            .queue_reorder(Parameters(ReorderQueueArgs { from: 0, to: 1 }))
            .await
            .unwrap();
        assert_eq!(reordered.items.len(), 1);

        let Json(next) = server
            .queue_play_next(Parameters(QueueIndexArgs { index: 0 }))
            .await
            .unwrap();
        assert_eq!(next.items.len(), 1);

        let Json(cleared) = server.queue_clear().await.unwrap();
        assert_eq!(cleared.items.len(), 1);

        let Json(retried) = server
            .retry_download(Parameters(RetryDownloadArgs {
                provider: ProviderId::Youtube,
                track_id: "v1".to_string(),
            }))
            .await
            .unwrap();
        assert!(retried.ok);

        let calls = state.calls.lock().unwrap().clone();
        assert!(calls.contains(&"POST /api/seek".to_string()));
        assert!(calls.contains(&"POST /api/shuffle".to_string()));
        assert!(calls.contains(&"POST /api/repeat".to_string()));
        assert!(calls.contains(&"POST /api/queue/remove".to_string()));
        assert!(calls.contains(&"POST /api/queue/reorder".to_string()));
        assert!(calls.contains(&"POST /api/queue/play-next".to_string()));
        assert!(calls.contains(&"DELETE /api/queue".to_string()));
        assert!(calls.contains(&"POST /api/downloads/retry".to_string()));
        handle.abort();
    }

    #[tokio::test]
    async fn unavailable_daemon_is_actionable() {
        let client = DaemonClient::new("http://127.0.0.1:1").unwrap();
        let server = McpServer::new(client);
        let error = expect_error(server.now_playing().await);
        assert!(error.message.contains("Cannot reach the daemon"));
        assert!(error.message.contains("ytermusicd serve"));
    }

    #[tokio::test]
    async fn upstream_errors_keep_the_daemon_message() {
        let state = MockState::default();
        state.fail.store(true, Ordering::SeqCst);
        let (base, handle) = mock_router(state).await;
        let server = McpServer::new(DaemonClient::new(base).unwrap());
        let error = expect_error(server.now_playing().await);
        assert!(error.message.contains("HTTP 502"));
        handle.abort();
    }

    #[test]
    fn to_mcp_maps_client_errors_to_invalid_params() {
        for code in ["bad_request", "not_found", "auth_expired"] {
            let error = to_mcp(DaemonError {
                code: code.to_string(),
                message: "playlist p9 not found".to_string(),
            });
            assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
            assert!(error.message.contains("playlist p9 not found"));
            assert_eq!(error.data, Some(json!({ "code": code })));
        }
    }

    #[test]
    fn to_mcp_auth_expired_keeps_the_restart_message() {
        let error = to_mcp(DaemonError {
            code: "auth_expired".to_string(),
            message: "The YouTube Music session expired. Update headers.txt \
                with a fresh Cookie and restart the daemon. Search and playback still work."
                .to_string(),
        });
        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
        assert!(error.message.contains("restart the daemon"));
        assert_eq!(error.data, Some(json!({ "code": "auth_expired" })));
    }

    #[test]
    fn to_mcp_keeps_other_errors_internal() {
        for code in [
            "upstream_error",
            "daemon_unavailable",
            "http_error",
            "internal_error",
        ] {
            let error = to_mcp(DaemonError {
                code: code.to_string(),
                message: "boom".to_string(),
            });
            assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
            assert!(error.message.contains(code));
            assert!(error.message.contains("boom"));
            assert_eq!(error.data, Some(json!({ "code": code })));
        }
    }

    #[tokio::test]
    async fn unknown_playlist_maps_to_invalid_params() {
        let state = MockState::default();
        state.missing.store(true, Ordering::SeqCst);
        let (base, handle) = mock_router(state).await;
        let server = McpServer::new(DaemonClient::new(base).unwrap());
        let error = expect_error(
            server
                .get_playlist(Parameters(PlaylistArgs {
                    playlist_id: "p9".to_string(),
                    provider: ProviderId::Youtube,
                }))
                .await,
        );
        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
        assert!(error.message.contains("playlist p9 not found"));
        assert_eq!(error.data, Some(json!({ "code": "not_found" })));
        handle.abort();
    }

    #[test]
    fn encodes_paths_and_queries() {
        assert_eq!(encode("VL/1 & x"), "VL%2F1%20%26%20x");
    }
}
