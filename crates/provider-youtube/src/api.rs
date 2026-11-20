//! YouTube Music API client. It implements the provider-neutral
//! `MusicApi` trait on top of `ytpapi2`.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use common_structs::ProviderId;
use ytermusicd_api::{
    AuthState, AuthStatus, CreatePlaylistRequest, MusicApi, Playlist, PlaylistEditError,
    PlaylistPrivacy, SearchResults, Track, UpdatePlaylistRequest,
};
use ytpapi2::{
    COOKIE, Endpoint, HeaderMap, HeaderValue, PlaylistPrivacy as UpstreamPrivacy, USER_AGENT,
    YoutubeMusicError, YoutubeMusicInstance, YoutubeMusicPlaylistRef, YoutubeMusicVideoRef,
};

/// Message shown when YouTube Music rejects the stored session.
/// It names both steps. Update headers.txt with a fresh Cookie.
/// Restart the daemon to load it. Search and playback still work
/// with an expired session.
pub const AUTH_EXPIRED_MESSAGE: &str = "The YouTube Music session expired. Update headers.txt \
    with a fresh Cookie and restart the daemon. Search and playback still work.";

/// Build an authenticated YouTube Music client from core auth.
/// The header types come from `ytpapi2`, so they match the client.
pub async fn connect(
    auth: &ytermusicd_core::Auth,
) -> Result<YoutubeMusicInstance, YoutubeMusicError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        COOKIE,
        HeaderValue::from_str(auth.cookie()).expect("cookie was validated"),
    );
    headers.insert(
        USER_AGENT,
        HeaderValue::from_str(auth.user_agent()).expect("user agent was validated"),
    );
    YoutubeMusicInstance::new(headers, auth.account_id().map(str::to_string)).await
}

/// Reject a provider this implementation does not serve. YouTube Music
/// is the only provider today. A second provider adds its own
/// implementation instead of extending this check.
fn check_provider(provider: ProviderId) -> Result<(), String> {
    if provider == ProviderId::Youtube {
        Ok(())
    } else {
        Err(format!(
            "provider {provider} is not available; YouTube Music is the only provider"
        ))
    }
}

/// The YouTube Music implementation of the provider-neutral API.
pub struct YoutubeMusicApi {
    client: Arc<YoutubeMusicInstance>,
    auth: Mutex<AuthStatus>,
}

impl YoutubeMusicApi {
    /// Build the API client. The homepage login flag sets the first
    /// auth state. A later library request can change it.
    pub fn new(client: YoutubeMusicInstance, source: Option<String>) -> Self {
        let (state, message) = match client.logged_in() {
            Some(false) => (AuthState::Expired, Some(AUTH_EXPIRED_MESSAGE.to_string())),
            _ => (AuthState::Ok, None),
        };
        if state == AuthState::Expired {
            log::warn!("{AUTH_EXPIRED_MESSAGE}");
        }
        Self {
            client: Arc::new(client),
            auth: Mutex::new(AuthStatus {
                state,
                source,
                message,
            }),
        }
    }

    fn auth_lock(&self) -> std::sync::MutexGuard<'_, AuthStatus> {
        self.auth
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn mark_expired(&self) {
        let mut auth = self.auth_lock();
        if auth.state != AuthState::Expired {
            log::warn!("{AUTH_EXPIRED_MESSAGE}");
        }
        auth.state = AuthState::Expired;
        auth.message = Some(AUTH_EXPIRED_MESSAGE.to_string());
    }

    fn mark_ok(&self) {
        let mut auth = self.auth_lock();
        if auth.state != AuthState::Ok {
            log::info!("YouTube Music session is valid");
        }
        auth.state = AuthState::Ok;
        auth.message = None;
    }

    fn auth_failure(&self, action: &str) -> String {
        self.mark_expired();
        format!("{action} failed: {AUTH_EXPIRED_MESSAGE}")
    }

    fn edit_error(&self, action: &'static str) -> impl Fn(YoutubeMusicError) -> PlaylistEditError {
        move |error| {
            let result = playlist_edit_error(action, error);
            if matches!(result, PlaylistEditError::AuthExpired(_)) {
                self.mark_expired();
            }
            result
        }
    }
}

#[async_trait]
impl MusicApi for YoutubeMusicApi {
    fn auth_status(&self, provider: ProviderId) -> Option<AuthStatus> {
        (provider == ProviderId::Youtube).then(|| self.auth_lock().clone())
    }

    async fn playlists(&self, provider: ProviderId) -> Result<Vec<Playlist>, String> {
        check_provider(provider)?;
        let mut playlists = Vec::new();
        let mut succeeded = false;
        for endpoint in [Endpoint::MusicLibraryLanding, Endpoint::MusicLikedPlaylists] {
            match self.client.get_library(&endpoint, 2).await {
                Ok(found) => {
                    succeeded = true;
                    playlists.extend(found.into_iter().map(playlist_dto));
                }
                Err(YoutubeMusicError::AuthExpired(_)) => {
                    return Err(self.auth_failure("load playlists"));
                }
                Err(error) => log::warn!("library endpoint {endpoint:?} failed: {error:?}"),
            }
        }
        if !succeeded {
            return Err("could not load playlists from YouTube Music".to_string());
        }
        self.mark_ok();
        playlists.sort_by(|a, b| a.name.cmp(&b.name));
        playlists.dedup_by(|a, b| a.id == b.id);
        Ok(playlists)
    }

    async fn playlist(&self, provider: ProviderId, id: &str) -> Result<Vec<Track>, String> {
        check_provider(provider)?;
        self.client
            .get_playlist_raw(id, 5)
            .await
            .map(|tracks| tracks.into_iter().map(track_dto).collect())
            .map_err(|error| match error {
                YoutubeMusicError::AuthExpired(_) => self.auth_failure("load playlist"),
                other => format!("failed to load playlist {id}: {other:?}"),
            })
    }

    async fn search(&self, provider: ProviderId, query: &str) -> Result<SearchResults, String> {
        check_provider(provider)?;
        self.client
            .search(query, 2)
            .await
            .map(|results| SearchResults {
                tracks: results.videos.into_iter().map(track_dto).collect(),
                playlists: results.playlists.into_iter().map(playlist_dto).collect(),
            })
            .map_err(|error| format!("search failed: {error:?}"))
    }

    async fn create_playlist(
        &self,
        request: &CreatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError> {
        let id = self
            .client
            .create_playlist(
                &request.title,
                &request.description,
                upstream_privacy(request.privacy),
            )
            .await
            .map_err(self.edit_error("create playlist"))?;
        Ok(Playlist {
            id,
            name: request.title.clone(),
            subtitle: privacy_subtitle(request.privacy),
        })
    }

    async fn update_playlist(
        &self,
        id: &str,
        request: &UpdatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError> {
        self.client
            .rename_playlist(id, &request.title)
            .await
            .map_err(self.edit_error("rename playlist"))?;
        Ok(Playlist {
            id: id.to_string(),
            name: request.title.clone(),
            subtitle: String::new(),
        })
    }

    async fn delete_playlist(&self, id: &str) -> Result<(), PlaylistEditError> {
        self.client
            .delete_playlist(id)
            .await
            .map_err(self.edit_error("delete playlist"))
    }

    async fn add_playlist_tracks(
        &self,
        id: &str,
        track_ids: &[String],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        self.client
            .add_playlist_items(id, track_ids)
            .await
            .map_err(self.edit_error("add tracks"))?;
        self.playlist(ProviderId::Youtube, id)
            .await
            .map_err(playlist_read_error)
    }

    async fn remove_playlist_tracks(
        &self,
        id: &str,
        indices: &[usize],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        let items = self
            .client
            .get_playlist_raw(id, 5)
            .await
            .map_err(self.edit_error("load playlist"))?;
        let mut seen = HashSet::new();
        let mut remove = Vec::new();
        for index in indices {
            if !seen.insert(*index) {
                return Err(PlaylistEditError::Invalid(format!(
                    "duplicate track index {index}"
                )));
            }
            let item = items.get(*index).ok_or_else(|| {
                PlaylistEditError::Invalid(format!("track index {index} is out of range"))
            })?;
            let set_video_id = item.set_video_id.clone().ok_or_else(|| {
                PlaylistEditError::Upstream(
                    "this playlist is not editable; it may belong to another account".to_string(),
                )
            })?;
            remove.push((item.video_id.clone(), set_video_id));
        }
        if remove.is_empty() {
            return Err(PlaylistEditError::Invalid(
                "select at least one track".to_string(),
            ));
        }
        self.client
            .remove_playlist_items(id, &remove)
            .await
            .map_err(self.edit_error("remove tracks"))?;
        self.playlist(ProviderId::Youtube, id)
            .await
            .map_err(playlist_read_error)
    }

    async fn move_playlist_track(
        &self,
        id: &str,
        from: usize,
        to: usize,
    ) -> Result<Vec<Track>, PlaylistEditError> {
        let items = self
            .client
            .get_playlist_raw(id, 5)
            .await
            .map_err(self.edit_error("load playlist"))?;
        if from >= items.len() {
            return Err(PlaylistEditError::Invalid(format!(
                "track index {from} is out of range"
            )));
        }
        if to >= items.len() {
            return Err(PlaylistEditError::Invalid(format!(
                "track index {to} is out of range"
            )));
        }
        if from == to {
            return Ok(items.into_iter().map(track_dto).collect());
        }
        let set_video_id = items[from].set_video_id.clone().ok_or_else(|| {
            PlaylistEditError::Upstream(
                "this playlist is not editable; it may belong to another account".to_string(),
            )
        })?;
        let successor = match move_successor_index(items.len(), from, to) {
            Some(index) => Some(items[index].set_video_id.clone().ok_or_else(|| {
                PlaylistEditError::Upstream(
                    "this playlist is not editable; it may belong to another account".to_string(),
                )
            })?),
            None => None,
        };
        self.client
            .move_playlist_item(id, &set_video_id, successor.as_deref())
            .await
            .map_err(self.edit_error("move track"))?;
        self.playlist(ProviderId::Youtube, id)
            .await
            .map_err(playlist_read_error)
    }
}

/// Index of the item that the moved item must precede. `None` means the end.
fn move_successor_index(len: usize, from: usize, to: usize) -> Option<usize> {
    let index = if from < to { to + 1 } else { to };
    (index < len).then_some(index)
}

fn upstream_privacy(privacy: PlaylistPrivacy) -> UpstreamPrivacy {
    match privacy {
        PlaylistPrivacy::Public => UpstreamPrivacy::Public,
        PlaylistPrivacy::Private => UpstreamPrivacy::Private,
        PlaylistPrivacy::Unlisted => UpstreamPrivacy::Unlisted,
    }
}

fn privacy_subtitle(privacy: PlaylistPrivacy) -> String {
    match privacy {
        PlaylistPrivacy::Public => "Public playlist",
        PlaylistPrivacy::Private => "Private playlist",
        PlaylistPrivacy::Unlisted => "Unlisted playlist",
    }
    .to_string()
}

/// Map a re-read after a playlist edit. `playlist()` returns a plain
/// string error, so an expiry detected on the re-read is recognized
/// by the shared message instead of being reported as a 502.
fn playlist_read_error(error: String) -> PlaylistEditError {
    if error.contains(AUTH_EXPIRED_MESSAGE) {
        PlaylistEditError::AuthExpired(error)
    } else {
        PlaylistEditError::Upstream(error)
    }
}

/// Map one upstream edit error to the provider-neutral edit error.
fn playlist_edit_error(action: &str, error: YoutubeMusicError) -> PlaylistEditError {
    match error {
        YoutubeMusicError::AuthExpired(_) => {
            PlaylistEditError::AuthExpired(format!("{action} failed: {AUTH_EXPIRED_MESSAGE}"))
        }
        YoutubeMusicError::RateLimited(_) => PlaylistEditError::Upstream(format!(
            "{action} failed: the YouTube Music rate limit was reached. Try again later."
        )),
        YoutubeMusicError::YoutubeMusicError(value) => {
            let message = value
                .pointer("/error/message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown YouTube Music error");
            PlaylistEditError::Upstream(format!("{action} failed: {message}"))
        }
        other => PlaylistEditError::Upstream(format!("{action} failed: {other:?}")),
    }
}

pub fn playlist_dto(playlist: YoutubeMusicPlaylistRef) -> Playlist {
    Playlist {
        id: playlist.browse_id,
        name: playlist.name,
        subtitle: playlist.subtitle,
    }
}

pub fn track_dto(track: YoutubeMusicVideoRef) -> Track {
    Track {
        provider: track.provider,
        track_id: track.video_id,
        title: track.title,
        artist: track.author,
        album: track.album,
        duration: track.duration,
    }
}

/// Convert a YouTube reference to a provider-neutral core track.
/// The playlist item ID does not survive the conversion.
pub fn track_from_ref(reference: &YoutubeMusicVideoRef) -> ytermusicd_core::Track {
    ytermusicd_core::Track {
        provider: reference.provider,
        track_id: reference.video_id.clone(),
        title: reference.title.clone(),
        author: reference.author.clone(),
        album: reference.album.clone(),
        duration: reference.duration.clone(),
    }
}

/// Convert a core track back to a YouTube reference. The playlist
/// item ID is empty: a plain track has no playlist position.
pub fn ref_from_track(track: &ytermusicd_core::Track) -> YoutubeMusicVideoRef {
    YoutubeMusicVideoRef {
        video_id: track.track_id.clone(),
        title: track.title.clone(),
        author: track.author.clone(),
        album: track.album.clone(),
        duration: track.duration.clone(),
        provider: track.provider,
        set_video_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video_ref() -> YoutubeMusicVideoRef {
        YoutubeMusicVideoRef {
            video_id: "v1".to_string(),
            title: "Song".to_string(),
            author: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
            provider: common_structs::ProviderId::Youtube,
            set_video_id: None,
        }
    }

    #[test]
    fn converts_track_and_playlist() {
        let track = track_dto(video_ref());
        assert_eq!(track.artist, "Artist");
        assert_eq!(track.track_id, "v1");

        let playlist = playlist_dto(YoutubeMusicPlaylistRef {
            name: "Mix".to_string(),
            subtitle: "Artist".to_string(),
            browse_id: "VL1".to_string(),
        });
        assert_eq!(playlist.id, "VL1");
    }

    #[test]
    fn reference_conversions_round_trip() {
        let reference = YoutubeMusicVideoRef {
            set_video_id: Some("set-1".to_string()),
            ..video_ref()
        };
        let track = track_from_ref(&reference);
        assert_eq!(track.track_id, "v1");
        assert_eq!(track.author, "Artist");

        let back = ref_from_track(&track);
        assert_eq!(back.video_id, "v1");
        assert_eq!(back.title, "Song");
        assert_eq!(back.set_video_id, None);
    }

    #[test]
    fn maps_privacy_labels() {
        assert_eq!(
            privacy_subtitle(PlaylistPrivacy::Private),
            "Private playlist"
        );
        assert_eq!(upstream_privacy(PlaylistPrivacy::Public).as_str(), "PUBLIC");
        assert_eq!(
            upstream_privacy(PlaylistPrivacy::Unlisted).as_str(),
            "UNLISTED"
        );
    }

    #[test]
    fn selects_the_move_successor() {
        assert_eq!(move_successor_index(5, 1, 3), Some(4));
        assert_eq!(move_successor_index(5, 1, 4), None);
        assert_eq!(move_successor_index(5, 3, 0), Some(0));
        assert_eq!(move_successor_index(5, 3, 2), Some(2));
    }

    /// Serve `body` to every browse request and return the base URL.
    async fn mock_server(body: serde_json::Value) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let body = body.to_string();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                // Read one request: headers, then the body per
                // content-length, so the client is not left hanging.
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let count = match socket.read(&mut chunk).await {
                        Ok(count) => count,
                        Err(_) => return,
                    };
                    if count == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..count]);
                    if let Some(position) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
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
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                if socket.write_all(reply.as_bytes()).await.is_err() {
                    return;
                }
            }
        });
        format!("http://{address}")
    }

    fn library_body(logged_in: &str) -> serde_json::Value {
        serde_json::json!({
            "responseContext": {"serviceTrackingParams": [{
                "service": "GFEEDBACK",
                "params": [{"key": "logged_in", "value": logged_in}]
            }]},
            "contents": {}
        })
    }

    fn api_with(base: &str, logged_in: Option<bool>) -> YoutubeMusicApi {
        let client = YoutubeMusicInstance::for_test_logged_in(
            base.to_string(),
            "test-key".to_string(),
            "1.0".to_string(),
            "SAPISID=x".to_string(),
            None,
            logged_in,
        );
        YoutubeMusicApi::new(client, Some("/tmp/headers.txt".to_string()))
    }

    fn auth_state(api: &YoutubeMusicApi) -> AuthState {
        api.auth_status(ProviderId::Youtube)
            .expect("youtube auth status")
            .state
    }

    #[test]
    fn new_sets_the_initial_auth_state_from_the_homepage_flag() {
        // The mock server is never contacted by `new`; any base works.
        let base = "http://127.0.0.1:9";
        assert_eq!(auth_state(&api_with(base, Some(false))), AuthState::Expired);
        assert_eq!(auth_state(&api_with(base, Some(true))), AuthState::Ok);
        assert_eq!(auth_state(&api_with(base, None)), AuthState::Ok);
        let message = api_with(base, Some(false))
            .auth_status(ProviderId::Youtube)
            .unwrap()
            .message
            .unwrap();
        assert!(message.contains("restart the daemon"), "{message}");
    }

    #[tokio::test]
    async fn playlists_propagates_auth_expired_and_marks_the_state() {
        let base = mock_server(library_body("0")).await;
        let api = api_with(&base, Some(true));
        assert_eq!(auth_state(&api), AuthState::Ok);
        let error = api
            .playlists(ProviderId::Youtube)
            .await
            .expect_err("signed-out library must fail");
        assert!(error.contains("restart the daemon"), "{error}");
        assert_eq!(auth_state(&api), AuthState::Expired);
    }

    #[tokio::test]
    async fn playlists_empty_signed_in_marks_the_state_ok() {
        let base = mock_server(library_body("1")).await;
        let api = api_with(&base, Some(false));
        assert_eq!(auth_state(&api), AuthState::Expired);
        let playlists = api.playlists(ProviderId::Youtube).await.unwrap();
        assert!(playlists.is_empty());
        assert_eq!(auth_state(&api), AuthState::Ok);
    }

    #[tokio::test]
    async fn playlist_propagates_auth_expired() {
        let base = mock_server(library_body("0")).await;
        let api = api_with(&base, Some(true));
        let error = api
            .playlist(ProviderId::Youtube, "VL1")
            .await
            .expect_err("signed-out playlist must fail");
        assert!(error.contains("restart the daemon"), "{error}");
        assert_eq!(auth_state(&api), AuthState::Expired);
    }

    #[tokio::test]
    async fn network_error_does_not_mark_the_session_expired() {
        // Nothing listens on 127.0.0.1:9, so every request fails at
        // the transport. The state must not move either way.
        let ok = api_with("http://127.0.0.1:9", Some(true));
        let error = ok
            .playlists(ProviderId::Youtube)
            .await
            .expect_err("unreachable library must fail");
        assert_eq!(error, "could not load playlists from YouTube Music");
        assert_eq!(auth_state(&ok), AuthState::Ok);

        let expired = api_with("http://127.0.0.1:9", Some(false));
        let _ = expired.playlists(ProviderId::Youtube).await;
        assert_eq!(auth_state(&expired), AuthState::Expired);
    }

    #[test]
    fn reread_errors_keep_the_auth_code() {
        let expired = playlist_read_error(format!("load playlist failed: {AUTH_EXPIRED_MESSAGE}"));
        match expired {
            PlaylistEditError::AuthExpired(message) => {
                assert!(message.contains("restart the daemon"), "{message}");
            }
            other => panic!("expected an auth error, got {other:?}"),
        }
        let upstream = playlist_read_error("upstream unavailable".to_string());
        assert!(matches!(upstream, PlaylistEditError::Upstream(_)));
    }

    #[test]
    fn playlist_errors_are_actionable() {
        let auth = playlist_edit_error(
            "create playlist",
            YoutubeMusicError::AuthExpired(serde_json::json!({})),
        );
        match auth {
            PlaylistEditError::AuthExpired(message) => {
                assert!(message.contains("restart the daemon"), "{message}");
            }
            other => panic!("expected an auth error, got {other:?}"),
        }

        let rate = playlist_edit_error(
            "add tracks",
            YoutubeMusicError::RateLimited(serde_json::json!({})),
        );
        match rate {
            PlaylistEditError::Upstream(message) => {
                assert!(message.contains("rate limit"), "{message}");
            }
            other => panic!("expected an upstream error, got {other:?}"),
        }

        let upstream = playlist_edit_error(
            "rename playlist",
            YoutubeMusicError::YoutubeMusicError(
                serde_json::json!({"error": {"message": "Not found"}}),
            ),
        );
        match upstream {
            PlaylistEditError::Upstream(message) => {
                assert!(message.contains("Not found"), "{message}");
            }
            other => panic!("expected an upstream error, got {other:?}"),
        }
    }
}
