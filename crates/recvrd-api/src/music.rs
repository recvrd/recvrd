//! Provider-neutral music API. Providers implement this trait;
//! the daemon and its clients only use these methods.

use async_trait::async_trait;
use common_structs::ProviderId;

use crate::{
    AuthStatus, CreatePlaylistRequest, Playlist, PlaylistDetail, SearchResults, Track, TrackDetail,
    UpdatePlaylistRequest,
};

/// A playlist edit failed because of the request or the upstream service.
#[derive(Debug)]
pub enum PlaylistEditError {
    Invalid(String),
    /// YouTube Music rejected the stored session.
    AuthExpired(String),
    Upstream(String),
}

#[async_trait]
#[allow(clippy::double_must_use)]
pub trait MusicApi: Send + Sync {
    /// Report the live auth state of a provider. Providers that check
    /// the upstream session override this method. A `None` value means
    /// the provider has no live check.
    fn auth_status(&self, _provider: ProviderId) -> Option<AuthStatus> {
        None
    }

    async fn playlists(&self, provider: ProviderId) -> Result<Vec<Playlist>, String>;
    async fn playlist(&self, provider: ProviderId, id: &str) -> Result<Vec<Track>, String>;

    /// Load one playlist with its tracks. The default reads the
    /// library first, then the playlist. Providers that serve a
    /// playlist without a library read it direct.
    async fn playlist_detail(
        &self,
        provider: ProviderId,
        id: &str,
    ) -> Result<PlaylistDetail, String> {
        let playlist = self
            .playlists(provider)
            .await?
            .into_iter()
            .find(|playlist| playlist.id == id)
            .ok_or_else(|| format!("playlist {id} not found"))?;
        let tracks = self.playlist(provider, id).await?;
        Ok(PlaylistDetail { playlist, tracks })
    }

    async fn search(&self, provider: ProviderId, query: &str) -> Result<SearchResults, String>;

    /// Load the full detail for one track. Providers that do not
    /// serve detail return an error.
    async fn track_detail(&self, _provider: ProviderId, _id: &str) -> Result<TrackDetail, String> {
        Err("track detail is not supported".to_string())
    }

    async fn create_playlist(
        &self,
        request: &CreatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError>;
    async fn update_playlist(
        &self,
        id: &str,
        request: &UpdatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError>;
    async fn delete_playlist(&self, id: &str) -> Result<(), PlaylistEditError>;
    async fn add_playlist_tracks(
        &self,
        id: &str,
        track_ids: &[String],
    ) -> Result<Vec<Track>, PlaylistEditError>;
    async fn remove_playlist_tracks(
        &self,
        id: &str,
        indices: &[usize],
    ) -> Result<Vec<Track>, PlaylistEditError>;
    async fn move_playlist_track(
        &self,
        id: &str,
        from: usize,
        to: usize,
    ) -> Result<Vec<Track>, PlaylistEditError>;
}
