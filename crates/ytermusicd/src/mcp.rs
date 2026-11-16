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
    Queue, QueueItem, SearchResults, Status, Track, UpdatePlaylistRequest,
};
use ytermusicd_client::DaemonClient;

fn to_mcp(error: ytermusicd_client::DaemonError) -> McpError {
    McpError::internal_error(
        format!("daemon error ({}): {}", error.code, error.message),
        None,
    )
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
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PlaylistArgs {
    /// Playlist ID from list_playlists or search.
    pub playlist_id: String,
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
    /// Video IDs to add.
    pub video_ids: Vec<String>,
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
pub struct DownloadStatusArgs {
    /// Optional video ID. Omit it to list all queue downloads.
    pub video_id: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DownloadStatusOutput {
    pub summary: DownloadSummary,
    pub tracks: Vec<QueueItem>,
}

#[tool_router]
impl McpServer {
    /// Search YouTube Music for tracks and playlists.
    #[tool(description = "Search YouTube Music for tracks and playlists")]
    async fn search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<Json<SearchResults>, McpError> {
        self.client
            .search(&args.query)
            .await
            .map(Json)
            .map_err(to_mcp)
    }

    /// List the playlists in the YouTube Music library.
    #[tool(description = "List the playlists in the YouTube Music library")]
    async fn list_playlists(&self) -> Result<Json<Vec<Playlist>>, McpError> {
        self.client.playlists().await.map(Json).map_err(to_mcp)
    }

    /// Get one playlist and its tracks.
    #[tool(description = "Get one playlist and its tracks")]
    async fn get_playlist(
        &self,
        Parameters(args): Parameters<PlaylistArgs>,
    ) -> Result<Json<PlaylistDetail>, McpError> {
        self.client
            .playlist(&args.playlist_id)
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
            .add_playlist_tracks(&args.playlist_id, args.video_ids)
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
            .play_playlist(&args.playlist_id)
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

    /// Report download status for queue tracks.
    #[tool(description = "Report download status for queue tracks")]
    async fn download_status(
        &self,
        Parameters(args): Parameters<DownloadStatusArgs>,
    ) -> Result<Json<DownloadStatusOutput>, McpError> {
        let (status, queue) = tokio::join!(self.client.status(), self.client.queue());
        let status = status.map_err(to_mcp)?;
        let queue = queue.map_err(to_mcp)?;
        let tracks = match args.video_id {
            Some(video_id) => {
                let item = queue
                    .items
                    .into_iter()
                    .find(|item| item.track.video_id == video_id);
                match item {
                    Some(item) => vec![item],
                    None => {
                        return Err(McpError::invalid_params(
                            format!("no queued track with video id {video_id}"),
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
}

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Tools for the ytermusicd daemon. Playlist editing is available. Lyrics and HTTP transport are post-v1.",
        )
    }
}

/// Run the stdio MCP proxy. It exits when stdin closes.
pub async fn run(base_url: Option<String>) -> std::io::Result<()> {
    let base_url = base_url.unwrap_or_else(|| ytermusicd_client::DEFAULT_URL.to_string());
    let client = DaemonClient::new(base_url).map_err(std::io::Error::other)?;
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
        routing::{get, post},
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    use ytermusicd_api::{
        AddPlaylistTracksRequest, AuthState, AuthStatus, DownloadStatus, MovePlaylistTrackRequest,
        OkResponse, PlayPlaylistRequest, PlayTrackRequest, PlaybackSnapshot, PlaybackState,
        Playlist, PlaylistPrivacy, QueueItem, QueueRequest, RemovePlaylistTracksRequest,
        RepeatMode,
    };
    use ytermusicd_client::encode;

    #[derive(Clone, Default)]
    struct MockState {
        calls: Arc<Mutex<Vec<String>>>,
        fail: Arc<AtomicBool>,
    }

    fn track(id: &str) -> Track {
        Track {
            video_id: id.to_string(),
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
            .route("/api/queue", get(mock_queue).post(mock_queue_add))
            .route("/api/play/playlist", post(mock_play_playlist))
            .route("/api/play/track", post(mock_play_track))
            .route("/api/pause", post(mock_status_post))
            .route("/api/resume", post(mock_status_post))
            .route("/api/next", post(mock_status_post))
            .route("/api/previous", post(mock_status_post))
            .route("/api/volume", post(mock_volume))
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

    async fn mock_playlist(
        State(state): State<MockState>,
        Path(id): Path<String>,
    ) -> AxumJson<PlaylistDetail> {
        state
            .calls
            .lock()
            .unwrap()
            .push(format!("GET /api/playlists/{id}"));
        AxumJson(PlaylistDetail {
            playlist: Playlist {
                id: id.clone(),
                name: "Playlist One".to_string(),
                subtitle: "Artist".to_string(),
            },
            tracks: vec![track("v1")],
        })
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
                "move_playlist_track",
                "next",
                "now_playing",
                "pause",
                "play_playlist",
                "play_track",
                "previous",
                "queue_add",
                "remove_playlist_tracks",
                "rename_playlist",
                "resume",
                "search",
                "volume",
            ]
        );
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
            }))
            .await
            .unwrap();
        assert!(deleted.ok);

        let Json(added) = server
            .add_playlist_tracks(Parameters(PlaylistTracksArgs {
                playlist_id: "p1".to_string(),
                video_ids: vec!["v1".to_string()],
            }))
            .await
            .unwrap();
        assert_eq!(added[0].video_id, "v1");

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
            }))
            .await
            .unwrap();
        assert_eq!(found.tracks[0].video_id, "v1");

        let Json(playlists) = server.list_playlists().await.unwrap();
        assert_eq!(playlists[0].id, "p1");

        let Json(detail) = server
            .get_playlist(Parameters(PlaylistArgs {
                playlist_id: "p1".to_string(),
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
                video_id: Some("v1".to_string()),
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
    fn encodes_paths_and_queries() {
        assert_eq!(encode("VL/1 & x"), "VL%2F1%20%26%20x");
    }
}
