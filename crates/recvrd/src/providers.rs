//! Route daemon work to the provider that owns it. The daemon holds
//! one music client, one downloader and one cover fetcher, each of
//! which fans out to YouTube or RSS by provider.

use async_trait::async_trait;
use common_structs::{ProviderId, TrackKey};
use download_manager::{DownloadError, TrackDownloader};
use provider_podverse::PodverseProvider;
use provider_rss::RssProvider;
use provider_youtube::{AUTH_EXPIRED_MESSAGE, YoutubeMusicApi, YoutubeMusicError, connect};
use recvrd_api::{
    AuthStatus, CreatePlaylistRequest, MusicApi, Playlist, PlaylistDetail, PlaylistEditError,
    SearchResults, Track, TrackDetail, UpdatePlaylistRequest,
};
use recvrd_core::{Auth, CoverFetcher, Track as CoreTrack};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::RwLock;

use crate::music::UnavailableMusicApi;

/// Error text returned when a provider call is blocked by offline mode.
pub const OFFLINE_MESSAGE: &str = "offline mode is on";

/// A music API that connects the YouTube client lazily.
///
/// The daemon starts with offline mode possibly persisted on. Building
/// the YouTube client fetches the homepage, so it must not run while
/// offline. The first online provider call initializes the client and
/// later calls reuse it. RSS and Podverse providers are local clients
/// and are built with the composite.
pub struct ReloadableMusicApi {
    auth: Option<Auth>,
    auth_source: Option<String>,
    rss: Arc<RssProvider>,
    podverse: Option<Arc<PodverseProvider>>,
    offline: Arc<AtomicBool>,
    inner: RwLock<Option<Arc<dyn MusicApi>>>,
    /// Auth state learned while connecting. A failed login is sticky
    /// until the process restarts, like the old startup path.
    auth_override: std::sync::Mutex<Option<AuthStatus>>,
    #[cfg(test)]
    ensure_calls: std::sync::atomic::AtomicUsize,
}

impl ReloadableMusicApi {
    pub fn new(
        auth: Option<Auth>,
        auth_source: Option<String>,
        rss: Arc<RssProvider>,
        podverse: Option<Arc<PodverseProvider>>,
        offline: Arc<AtomicBool>,
    ) -> Self {
        Self {
            auth,
            auth_source,
            rss,
            podverse,
            offline,
            inner: RwLock::new(None),
            auth_override: std::sync::Mutex::new(None),
            #[cfg(test)]
            ensure_calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Initialize the provider client now. Called at startup when the
    /// daemon is online, and again after switching offline off.
    pub async fn connect_now(&self) -> Result<(), String> {
        self.ensure().await.map(|_| ())
    }

    /// Route a non-YouTube provider without touching the YouTube
    /// client. RSS and Podverse are local clients.
    fn route(&self, provider: ProviderId) -> Result<Arc<dyn MusicApi>, String> {
        match provider {
            ProviderId::Youtube => Err("YouTube needs the lazy client".to_string()),
            ProviderId::Rss => Ok(self.rss.clone()),
            ProviderId::Podverse => self
                .podverse
                .clone()
                .map(|provider| provider as Arc<dyn MusicApi>)
                .ok_or_else(|| "Podverse is not configured".to_string()),
        }
    }

    async fn api_for(&self, provider: ProviderId) -> Result<Arc<dyn MusicApi>, String> {
        if provider == ProviderId::Youtube {
            self.ensure().await
        } else {
            self.route(provider)
        }
    }

    #[cfg(test)]
    fn ensure_calls(&self) -> usize {
        self.ensure_calls.load(Ordering::SeqCst)
    }

    async fn ensure(&self) -> Result<Arc<dyn MusicApi>, String> {
        #[cfg(test)]
        self.ensure_calls.fetch_add(1, Ordering::SeqCst);
        if self.offline.load(Ordering::SeqCst) {
            return Err(OFFLINE_MESSAGE.to_string());
        }
        if let Some(api) = self.inner.read().await.as_ref() {
            return Ok(api.clone());
        }
        let mut guard = self.inner.write().await;
        if let Some(api) = guard.as_ref() {
            return Ok(api.clone());
        }

        let youtube: Arc<dyn MusicApi> = match &self.auth {
            Some(auth) => match connect(auth).await {
                Ok(client) => {
                    log::info!("YouTube Music client ready");
                    Arc::new(YoutubeMusicApi::new(client, self.auth_source.clone()))
                }
                Err(error) => {
                    log::error!("could not create YouTube Music client: {error:?}");
                    if matches!(error, YoutubeMusicError::NeedToLogin) {
                        let source = self
                            .auth_source
                            .clone()
                            .unwrap_or_else(|| "headers.txt".to_string());
                        *self.auth_override.lock().unwrap() = Some(AuthStatus {
                            state: recvrd_api::AuthState::Expired,
                            source: Some(source),
                            message: Some(AUTH_EXPIRED_MESSAGE.to_string()),
                        });
                    }
                    return Err(format!("could not create YouTube Music client: {error:?}"));
                }
            },
            None => Arc::new(UnavailableMusicApi),
        };

        let composite: Arc<dyn MusicApi> = Arc::new(CompositeMusicApi::new(
            youtube,
            self.rss.clone(),
            self.podverse.clone(),
        ));
        *guard = Some(composite.clone());
        Ok(composite)
    }
}

#[async_trait]
impl MusicApi for ReloadableMusicApi {
    fn auth_status(&self, provider: ProviderId) -> Option<AuthStatus> {
        if let Ok(inner) = self.inner.try_read()
            && let Some(api) = inner.as_ref()
            && let Some(status) = api.auth_status(provider)
        {
            return Some(status);
        }
        if provider == ProviderId::Youtube {
            self.auth_override.lock().unwrap().clone()
        } else {
            None
        }
    }

    async fn playlists(&self, provider: ProviderId) -> Result<Vec<Playlist>, String> {
        self.api_for(provider).await?.playlists(provider).await
    }

    async fn playlist(&self, provider: ProviderId, id: &str) -> Result<Vec<Track>, String> {
        self.api_for(provider).await?.playlist(provider, id).await
    }

    async fn playlist_detail(
        &self,
        provider: ProviderId,
        id: &str,
    ) -> Result<PlaylistDetail, String> {
        self.api_for(provider)
            .await?
            .playlist_detail(provider, id)
            .await
    }

    async fn search(&self, provider: ProviderId, query: &str) -> Result<SearchResults, String> {
        self.api_for(provider).await?.search(provider, query).await
    }

    async fn track_detail(&self, provider: ProviderId, id: &str) -> Result<TrackDetail, String> {
        self.api_for(provider)
            .await?
            .track_detail(provider, id)
            .await
    }

    async fn set_offline(&self, enabled: bool) -> Result<(), String> {
        self.offline.store(enabled, Ordering::SeqCst);
        if enabled {
            return Ok(());
        }
        self.ensure().await.map(|_| ())
    }

    async fn create_playlist(
        &self,
        request: &CreatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError> {
        self.ensure()
            .await
            .map_err(PlaylistEditError::Upstream)?
            .create_playlist(request)
            .await
    }

    async fn update_playlist(
        &self,
        id: &str,
        request: &UpdatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError> {
        self.ensure()
            .await
            .map_err(PlaylistEditError::Upstream)?
            .update_playlist(id, request)
            .await
    }

    async fn delete_playlist(&self, id: &str) -> Result<(), PlaylistEditError> {
        self.ensure()
            .await
            .map_err(PlaylistEditError::Upstream)?
            .delete_playlist(id)
            .await
    }

    async fn add_playlist_tracks(
        &self,
        id: &str,
        track_ids: &[String],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        self.ensure()
            .await
            .map_err(PlaylistEditError::Upstream)?
            .add_playlist_tracks(id, track_ids)
            .await
    }

    async fn remove_playlist_tracks(
        &self,
        id: &str,
        indices: &[usize],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        self.ensure()
            .await
            .map_err(PlaylistEditError::Upstream)?
            .remove_playlist_tracks(id, indices)
            .await
    }

    async fn move_playlist_track(
        &self,
        id: &str,
        from: usize,
        to: usize,
    ) -> Result<Vec<Track>, PlaylistEditError> {
        self.ensure()
            .await
            .map_err(PlaylistEditError::Upstream)?
            .move_playlist_track(id, from, to)
            .await
    }
}

/// One music client for every provider. Reads route by provider.
/// Playlist edits carry no provider, so they run on YouTube, as
/// before. RSS feeds are read-only, so no edit reaches RSS.
pub struct CompositeMusicApi {
    youtube: Arc<dyn MusicApi>,
    rss: Arc<RssProvider>,
    podverse: Option<Arc<PodverseProvider>>,
}

impl CompositeMusicApi {
    pub fn new(
        youtube: Arc<dyn MusicApi>,
        rss: Arc<RssProvider>,
        podverse: Option<Arc<PodverseProvider>>,
    ) -> Self {
        Self {
            youtube,
            rss,
            podverse,
        }
    }

    fn api(&self, provider: ProviderId) -> Result<Arc<dyn MusicApi>, String> {
        match provider {
            ProviderId::Youtube => Ok(self.youtube.clone()),
            ProviderId::Rss => Ok(self.rss.clone()),
            ProviderId::Podverse => self
                .podverse
                .clone()
                .map(|provider| provider as Arc<dyn MusicApi>)
                .ok_or_else(|| "Podverse is not configured".to_string()),
        }
    }
}

#[async_trait]
impl MusicApi for CompositeMusicApi {
    fn auth_status(&self, provider: ProviderId) -> Option<AuthStatus> {
        self.api(provider).ok()?.auth_status(provider)
    }

    async fn playlists(&self, provider: ProviderId) -> Result<Vec<Playlist>, String> {
        self.api(provider)?.playlists(provider).await
    }

    async fn playlist(&self, provider: ProviderId, id: &str) -> Result<Vec<Track>, String> {
        self.api(provider)?.playlist(provider, id).await
    }

    async fn playlist_detail(
        &self,
        provider: ProviderId,
        id: &str,
    ) -> Result<PlaylistDetail, String> {
        self.api(provider)?.playlist_detail(provider, id).await
    }

    async fn search(&self, provider: ProviderId, query: &str) -> Result<SearchResults, String> {
        self.api(provider)?.search(provider, query).await
    }

    async fn track_detail(&self, provider: ProviderId, id: &str) -> Result<TrackDetail, String> {
        self.api(provider)?.track_detail(provider, id).await
    }

    async fn create_playlist(
        &self,
        request: &CreatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError> {
        self.youtube.create_playlist(request).await
    }

    async fn update_playlist(
        &self,
        id: &str,
        request: &UpdatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError> {
        self.youtube.update_playlist(id, request).await
    }

    async fn delete_playlist(&self, id: &str) -> Result<(), PlaylistEditError> {
        self.youtube.delete_playlist(id).await
    }

    async fn add_playlist_tracks(
        &self,
        id: &str,
        track_ids: &[String],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        self.youtube.add_playlist_tracks(id, track_ids).await
    }

    async fn remove_playlist_tracks(
        &self,
        id: &str,
        indices: &[usize],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        self.youtube.remove_playlist_tracks(id, indices).await
    }

    async fn move_playlist_track(
        &self,
        id: &str,
        from: usize,
        to: usize,
    ) -> Result<Vec<Track>, PlaylistEditError> {
        self.youtube.move_playlist_track(id, from, to).await
    }
}

/// One downloader for every provider. The backend follows the
/// provider of the track, so an RSS enclosure downloads with the
/// RSS fetcher and a YouTube track with yt-dlp.
pub struct CompositeDownloader {
    youtube: Arc<dyn TrackDownloader>,
    rss: Arc<RssProvider>,
    podverse: Option<Arc<PodverseProvider>>,
}

impl CompositeDownloader {
    pub fn new(
        youtube: Arc<dyn TrackDownloader>,
        rss: Arc<RssProvider>,
        podverse: Option<Arc<PodverseProvider>>,
    ) -> Self {
        Self {
            youtube,
            rss,
            podverse,
        }
    }

    fn backend(&self, provider: ProviderId) -> Option<Arc<dyn TrackDownloader>> {
        match provider {
            ProviderId::Youtube => Some(self.youtube.clone()),
            ProviderId::Rss => Some(self.rss.clone()),
            ProviderId::Podverse => self
                .podverse
                .clone()
                .map(|provider| provider as Arc<dyn TrackDownloader>),
        }
    }
}

#[async_trait]
impl TrackDownloader for CompositeDownloader {
    async fn download(
        &self,
        track: &CoreTrack,
        output: &Path,
        progress: &(dyn Fn(usize) + Send + Sync),
    ) -> Result<(), DownloadError> {
        let Some(backend) = self.backend(track.provider) else {
            return Err(DownloadError::BackendFailed(format!(
                "provider {} is not configured",
                track.provider
            )));
        };
        backend.download(track, output, progress).await
    }

    fn audio_extension(&self, track: &CoreTrack) -> String {
        self.backend(track.provider)
            .map(|backend| backend.audio_extension(track))
            .unwrap_or_else(|| "mp3".to_string())
    }
}

/// One cover fetcher for every provider. The backend follows the
/// provider of the track key.
pub struct CompositeCoverFetcher {
    youtube: Arc<dyn CoverFetcher>,
    rss: Arc<RssProvider>,
    podverse: Option<Arc<PodverseProvider>>,
}

impl CompositeCoverFetcher {
    pub fn new(
        youtube: Arc<dyn CoverFetcher>,
        rss: Arc<RssProvider>,
        podverse: Option<Arc<PodverseProvider>>,
    ) -> Self {
        Self {
            youtube,
            rss,
            podverse,
        }
    }
}

#[async_trait]
impl CoverFetcher for CompositeCoverFetcher {
    async fn fetch(&self, key: &TrackKey) -> Option<Vec<u8>> {
        match key.provider {
            ProviderId::Youtube => self.youtube.fetch(key).await,
            ProviderId::Rss => self.rss.fetch(key).await,
            ProviderId::Podverse => self.podverse.as_ref()?.fetch(key).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NamedApi {
        name: &'static str,
    }

    #[async_trait]
    impl MusicApi for NamedApi {
        async fn playlists(&self, _provider: ProviderId) -> Result<Vec<Playlist>, String> {
            Ok(vec![Playlist {
                id: self.name.to_string(),
                name: self.name.to_string(),
                subtitle: String::new(),
                ..Default::default()
            }])
        }

        async fn playlist(&self, _provider: ProviderId, _id: &str) -> Result<Vec<Track>, String> {
            Err(format!("{} playlist", self.name))
        }

        async fn search(
            &self,
            _provider: ProviderId,
            _query: &str,
        ) -> Result<SearchResults, String> {
            Err(format!("{} search", self.name))
        }

        async fn create_playlist(
            &self,
            _request: &CreatePlaylistRequest,
        ) -> Result<Playlist, PlaylistEditError> {
            Ok(Playlist {
                id: self.name.to_string(),
                name: self.name.to_string(),
                subtitle: String::new(),
                ..Default::default()
            })
        }

        async fn update_playlist(
            &self,
            _id: &str,
            _request: &UpdatePlaylistRequest,
        ) -> Result<Playlist, PlaylistEditError> {
            Err(PlaylistEditError::Invalid(format!("{} update", self.name)))
        }

        async fn delete_playlist(&self, _id: &str) -> Result<(), PlaylistEditError> {
            Err(PlaylistEditError::Invalid(format!("{} delete", self.name)))
        }

        async fn add_playlist_tracks(
            &self,
            _id: &str,
            _track_ids: &[String],
        ) -> Result<Vec<Track>, PlaylistEditError> {
            Err(PlaylistEditError::Invalid(format!("{} add", self.name)))
        }

        async fn remove_playlist_tracks(
            &self,
            _id: &str,
            _indices: &[usize],
        ) -> Result<Vec<Track>, PlaylistEditError> {
            Err(PlaylistEditError::Invalid(format!("{} remove", self.name)))
        }

        async fn move_playlist_track(
            &self,
            _id: &str,
            _from: usize,
            _to: usize,
        ) -> Result<Vec<Track>, PlaylistEditError> {
            Err(PlaylistEditError::Invalid(format!("{} move", self.name)))
        }
    }

    fn rss() -> Arc<RssProvider> {
        let dir = tempfile::tempdir().unwrap();
        Arc::new(RssProvider::new(dir.path().join("rss-feeds.json")))
    }

    #[tokio::test]
    async fn reads_route_by_provider_and_edits_stay_on_youtube() {
        let composite = CompositeMusicApi::new(Arc::new(NamedApi { name: "youtube" }), rss(), None);
        let youtube = composite.playlists(ProviderId::Youtube).await.unwrap();
        assert_eq!(youtube[0].id, "youtube");
        // RSS has no feeds yet, so its list is empty.
        let rss_lists = composite.playlists(ProviderId::Rss).await.unwrap();
        assert!(rss_lists.is_empty());
        let created = composite
            .create_playlist(&CreatePlaylistRequest {
                title: "New".to_string(),
                description: String::new(),
                privacy: recvrd_api::PlaylistPrivacy::Private,
            })
            .await
            .unwrap();
        assert_eq!(created.id, "youtube");
    }

    #[tokio::test]
    async fn podverse_routes_to_the_configured_provider() {
        let podverse = Arc::new(
            PodverseProvider::new(&recvrd_core::PodverseConfig {
                base_url: Some("http://127.0.0.1".to_string()),
                token: None,
            })
            .unwrap(),
        );
        let composite = CompositeMusicApi::new(
            Arc::new(NamedApi { name: "youtube" }),
            rss(),
            Some(podverse),
        );
        assert!(
            composite
                .playlists(ProviderId::Podverse)
                .await
                .unwrap()
                .is_empty()
        );
        let unconfigured =
            CompositeMusicApi::new(Arc::new(NamedApi { name: "youtube" }), rss(), None);
        assert!(unconfigured.playlists(ProviderId::Podverse).await.is_err());
    }

    #[tokio::test]
    async fn downloader_routes_by_track_provider() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("rss-feeds.json");
        let provider = Arc::new(RssProvider::new(store));
        let composite = CompositeDownloader::new(Arc::new(NamedDownloader), provider.clone(), None);
        let rss_track = CoreTrack {
            provider: ProviderId::Rss,
            track_id: "0123456789abcdef0123456789abcdef".to_string(),
            title: "Song".to_string(),
            author: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
            ..Default::default()
        };
        // The RSS backend reports mp3 for an unknown track, while
        // the YouTube backend reports mp4.
        assert_eq!(composite.audio_extension(&rss_track), "mp3");
        let youtube_track = CoreTrack {
            provider: ProviderId::Youtube,
            ..rss_track.clone()
        };
        assert_eq!(composite.audio_extension(&youtube_track), "mp4");
    }

    #[tokio::test]
    async fn offline_music_api_refuses_provider_calls() {
        let api = ReloadableMusicApi::new(None, None, rss(), None, Arc::new(AtomicBool::new(true)));
        let error = api.search(ProviderId::Youtube, "test").await.unwrap_err();
        assert_eq!(error, OFFLINE_MESSAGE);
        assert!(api.auth_status(ProviderId::Youtube).is_none());
    }

    #[tokio::test]
    async fn non_youtube_calls_do_not_initialize_youtube() {
        let api =
            ReloadableMusicApi::new(None, None, rss(), None, Arc::new(AtomicBool::new(false)));
        let _ = api.search(ProviderId::Rss, "test").await;
        assert_eq!(api.ensure_calls(), 0);

        let _ = api.search(ProviderId::Youtube, "test").await;
        assert_eq!(api.ensure_calls(), 1);
    }

    #[tokio::test]
    async fn online_without_auth_uses_the_unavailable_api() {
        let api =
            ReloadableMusicApi::new(None, None, rss(), None, Arc::new(AtomicBool::new(false)));
        let error = api.search(ProviderId::Youtube, "test").await.unwrap_err();
        assert!(error.contains("unavailable"), "{error}");
    }

    struct NamedDownloader;

    #[async_trait]
    impl TrackDownloader for NamedDownloader {
        async fn download(
            &self,
            _track: &CoreTrack,
            _output: &Path,
            _progress: &(dyn Fn(usize) + Send + Sync),
        ) -> Result<(), DownloadError> {
            Ok(())
        }
    }
}
