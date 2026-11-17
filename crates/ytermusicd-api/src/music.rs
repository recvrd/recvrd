//! Provider-neutral music API. Providers implement this trait;
//! the daemon and its clients only use these methods.

use async_trait::async_trait;
use common_structs::ProviderId;

use crate::{CreatePlaylistRequest, Playlist, SearchResults, Track, UpdatePlaylistRequest};

/// A playlist edit failed because of the request or the upstream service.
#[derive(Debug)]
pub enum PlaylistEditError {
    Invalid(String),
    Upstream(String),
}

#[async_trait]
#[allow(clippy::double_must_use)]
pub trait MusicApi: Send + Sync {
    async fn playlists(&self, provider: ProviderId) -> Result<Vec<Playlist>, String>;
    async fn playlist(&self, provider: ProviderId, id: &str) -> Result<Vec<Track>, String>;
    async fn search(&self, provider: ProviderId, query: &str) -> Result<SearchResults, String>;
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
