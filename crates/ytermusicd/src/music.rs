//! Music API used when no provider client is available.

use async_trait::async_trait;
use common_structs::ProviderId;
use ytermusicd_api::{
    CreatePlaylistRequest, MusicApi, Playlist, PlaylistEditError, SearchResults, Track,
    UpdatePlaylistRequest,
};

pub struct UnavailableMusicApi;

#[async_trait]
impl MusicApi for UnavailableMusicApi {
    async fn playlists(&self, _provider: ProviderId) -> Result<Vec<Playlist>, String> {
        Err("YouTube Music client is unavailable; check auth/network".to_string())
    }

    async fn playlist(&self, _provider: ProviderId, _id: &str) -> Result<Vec<Track>, String> {
        Err("YouTube Music client is unavailable; check auth/network".to_string())
    }

    async fn search(&self, _provider: ProviderId, _query: &str) -> Result<SearchResults, String> {
        Err("YouTube Music client is unavailable; check auth/network".to_string())
    }

    async fn create_playlist(
        &self,
        _request: &CreatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError> {
        Err(unavailable())
    }

    async fn update_playlist(
        &self,
        _id: &str,
        _request: &UpdatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError> {
        Err(unavailable())
    }

    async fn delete_playlist(&self, _id: &str) -> Result<(), PlaylistEditError> {
        Err(unavailable())
    }

    async fn add_playlist_tracks(
        &self,
        _id: &str,
        _track_ids: &[String],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        Err(unavailable())
    }

    async fn remove_playlist_tracks(
        &self,
        _id: &str,
        _indices: &[usize],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        Err(unavailable())
    }

    async fn move_playlist_track(
        &self,
        _id: &str,
        _from: usize,
        _to: usize,
    ) -> Result<Vec<Track>, PlaylistEditError> {
        Err(unavailable())
    }
}

fn unavailable() -> PlaylistEditError {
    PlaylistEditError::Upstream(
        "YouTube Music client is unavailable; check auth/network".to_string(),
    )
}
