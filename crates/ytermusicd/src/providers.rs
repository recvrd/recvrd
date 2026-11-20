//! Route daemon work to the provider that owns it. The daemon holds
//! one music client, one downloader and one cover fetcher, each of
//! which fans out to YouTube or RSS by provider.

use async_trait::async_trait;
use common_structs::{ProviderId, TrackKey};
use download_manager::{DownloadError, TrackDownloader};
use provider_rss::RssProvider;
use std::{path::Path, sync::Arc};
use ytermusicd_api::{
    AuthStatus, CreatePlaylistRequest, MusicApi, Playlist, PlaylistEditError, SearchResults, Track,
    UpdatePlaylistRequest,
};
use ytermusicd_core::{CoverFetcher, Track as CoreTrack};

/// One music client for every provider. Reads route by provider.
/// Playlist edits carry no provider, so they run on YouTube, as
/// before. RSS feeds are read-only, so no edit reaches RSS.
pub struct CompositeMusicApi {
    youtube: Arc<dyn MusicApi>,
    rss: Arc<RssProvider>,
}

impl CompositeMusicApi {
    pub fn new(youtube: Arc<dyn MusicApi>, rss: Arc<RssProvider>) -> Self {
        Self { youtube, rss }
    }

    fn api(&self, provider: ProviderId) -> Arc<dyn MusicApi> {
        match provider {
            ProviderId::Youtube => self.youtube.clone(),
            ProviderId::Rss => {
                let api: Arc<dyn MusicApi> = self.rss.clone();
                api
            }
        }
    }
}

#[async_trait]
impl MusicApi for CompositeMusicApi {
    fn auth_status(&self, provider: ProviderId) -> Option<AuthStatus> {
        self.api(provider).auth_status(provider)
    }

    async fn playlists(&self, provider: ProviderId) -> Result<Vec<Playlist>, String> {
        self.api(provider).playlists(provider).await
    }

    async fn playlist(&self, provider: ProviderId, id: &str) -> Result<Vec<Track>, String> {
        self.api(provider).playlist(provider, id).await
    }

    async fn search(&self, provider: ProviderId, query: &str) -> Result<SearchResults, String> {
        self.api(provider).search(provider, query).await
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
}

impl CompositeDownloader {
    pub fn new(youtube: Arc<dyn TrackDownloader>, rss: Arc<RssProvider>) -> Self {
        Self { youtube, rss }
    }

    fn backend(&self, provider: ProviderId) -> Arc<dyn TrackDownloader> {
        match provider {
            ProviderId::Youtube => self.youtube.clone(),
            ProviderId::Rss => {
                let backend: Arc<dyn TrackDownloader> = self.rss.clone();
                backend
            }
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
        self.backend(track.provider)
            .download(track, output, progress)
            .await
    }

    fn audio_extension(&self, track: &CoreTrack) -> String {
        self.backend(track.provider).audio_extension(track)
    }
}

/// One cover fetcher for every provider. The backend follows the
/// provider of the track key.
pub struct CompositeCoverFetcher {
    youtube: Arc<dyn CoverFetcher>,
    rss: Arc<RssProvider>,
}

impl CompositeCoverFetcher {
    pub fn new(youtube: Arc<dyn CoverFetcher>, rss: Arc<RssProvider>) -> Self {
        Self { youtube, rss }
    }
}

#[async_trait]
impl CoverFetcher for CompositeCoverFetcher {
    async fn fetch(&self, key: &TrackKey) -> Option<Vec<u8>> {
        match key.provider {
            ProviderId::Youtube => self.youtube.fetch(key).await,
            ProviderId::Rss => self.rss.fetch(key).await,
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
        let composite = CompositeMusicApi::new(Arc::new(NamedApi { name: "youtube" }), rss());
        let youtube = composite.playlists(ProviderId::Youtube).await.unwrap();
        assert_eq!(youtube[0].id, "youtube");
        // RSS has no feeds yet, so its list is empty.
        let rss_lists = composite.playlists(ProviderId::Rss).await.unwrap();
        assert!(rss_lists.is_empty());
        let created = composite
            .create_playlist(&CreatePlaylistRequest {
                title: "New".to_string(),
                description: String::new(),
                privacy: ytermusicd_api::PlaylistPrivacy::Private,
            })
            .await
            .unwrap();
        assert_eq!(created.id, "youtube");
    }

    #[tokio::test]
    async fn downloader_routes_by_track_provider() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("rss-feeds.json");
        let provider = Arc::new(RssProvider::new(store));
        let composite = CompositeDownloader::new(Arc::new(NamedDownloader), provider.clone());
        let rss_track = CoreTrack {
            provider: ProviderId::Rss,
            track_id: "0123456789abcdef0123456789abcdef".to_string(),
            title: "Song".to_string(),
            author: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
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
