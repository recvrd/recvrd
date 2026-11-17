use async_trait::async_trait;
use std::{collections::HashSet, sync::Arc};
use ytermusicd_api::{
    CreatePlaylistRequest, Playlist, PlaylistPrivacy, SearchResults, Track, UpdatePlaylistRequest,
};
use ytpapi2::{
    Endpoint, PlaylistPrivacy as UpstreamPrivacy, YoutubeMusicError, YoutubeMusicInstance,
    YoutubeMusicPlaylistRef,
};

/// A playlist edit failed because of the request or the upstream service.
#[derive(Debug)]
pub enum PlaylistEditError {
    Invalid(String),
    Upstream(String),
}

#[async_trait]
#[allow(clippy::double_must_use)]
pub trait MusicApi: Send + Sync {
    async fn playlists(&self) -> Result<Vec<Playlist>, String>;
    async fn playlist(&self, id: &str) -> Result<Vec<Track>, String>;
    async fn search(&self, query: &str) -> Result<SearchResults, String>;
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
        video_ids: &[String],
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

pub struct YoutubeMusicApi {
    client: Arc<YoutubeMusicInstance>,
}

impl YoutubeMusicApi {
    pub fn new(client: YoutubeMusicInstance) -> Self {
        Self {
            client: Arc::new(client),
        }
    }
}

#[async_trait]
impl MusicApi for YoutubeMusicApi {
    async fn playlists(&self) -> Result<Vec<Playlist>, String> {
        let mut playlists = Vec::new();
        for endpoint in [Endpoint::MusicLibraryLanding, Endpoint::MusicLikedPlaylists] {
            match self.client.get_library(&endpoint, 2).await {
                Ok(found) => playlists.extend(found.into_iter().map(playlist_dto)),
                Err(error) => log::warn!("library endpoint {endpoint:?} failed: {error:?}"),
            }
        }
        playlists.sort_by(|a, b| a.name.cmp(&b.name));
        playlists.dedup_by(|a, b| a.id == b.id);
        if playlists.is_empty() {
            // Distinguish a genuinely empty library from a total failure.
            return Err("could not load playlists from YouTube Music".to_string());
        }
        Ok(playlists)
    }

    async fn playlist(&self, id: &str) -> Result<Vec<Track>, String> {
        self.client
            .get_playlist_raw(id, 5)
            .await
            .map(|tracks| tracks.into_iter().map(track_dto).collect())
            .map_err(|error| format!("failed to load playlist {id}: {error:?}"))
    }

    async fn search(&self, query: &str) -> Result<SearchResults, String> {
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
            .map_err(edit_error("create playlist"))?;
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
            .map_err(edit_error("rename playlist"))?;
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
            .map_err(edit_error("delete playlist"))
    }

    async fn add_playlist_tracks(
        &self,
        id: &str,
        video_ids: &[String],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        self.client
            .add_playlist_items(id, video_ids)
            .await
            .map_err(edit_error("add tracks"))?;
        self.playlist(id).await.map_err(PlaylistEditError::Upstream)
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
            .map_err(edit_error("load playlist"))?;
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
            .map_err(edit_error("remove tracks"))?;
        self.playlist(id).await.map_err(PlaylistEditError::Upstream)
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
            .map_err(edit_error("load playlist"))?;
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
            .map_err(edit_error("move track"))?;
        self.playlist(id).await.map_err(PlaylistEditError::Upstream)
    }
}

pub struct UnavailableMusicApi;

#[async_trait]
impl MusicApi for UnavailableMusicApi {
    async fn playlists(&self) -> Result<Vec<Playlist>, String> {
        Err("YouTube Music client is unavailable; check auth/network".to_string())
    }

    async fn playlist(&self, _id: &str) -> Result<Vec<Track>, String> {
        Err("YouTube Music client is unavailable; check auth/network".to_string())
    }

    async fn search(&self, _query: &str) -> Result<SearchResults, String> {
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
        _video_ids: &[String],
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

fn edit_error(action: &'static str) -> impl Fn(YoutubeMusicError) -> PlaylistEditError {
    move |error| PlaylistEditError::Upstream(playlist_error_message(action, error))
}

fn playlist_error_message(action: &str, error: YoutubeMusicError) -> String {
    match error {
        YoutubeMusicError::AuthExpired(_) => format!(
            "{action} failed: the YouTube Music session expired. Sign in again and update headers.txt."
        ),
        YoutubeMusicError::RateLimited(_) => {
            format!("{action} failed: the YouTube Music rate limit was reached. Try again later.")
        }
        YoutubeMusicError::YoutubeMusicError(value) => {
            let message = value
                .pointer("/error/message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown YouTube Music error");
            format!("{action} failed: {message}")
        }
        other => format!("{action} failed: {other:?}"),
    }
}

pub fn playlist_dto(playlist: YoutubeMusicPlaylistRef) -> Playlist {
    Playlist {
        id: playlist.browse_id,
        name: playlist.name,
        subtitle: playlist.subtitle,
    }
}

pub fn track_dto(track: ytpapi2::YoutubeMusicVideoRef) -> Track {
    Track {
        provider: track.provider,
        track_id: track.video_id,
        title: track.title,
        artist: track.author,
        album: track.album,
        duration: track.duration,
    }
}

pub fn core_track(track: Track) -> ytermusicd_core::Track {
    ytermusicd_core::Track {
        provider: track.provider,
        track_id: track.track_id,
        title: track.title,
        author: track.artist,
        album: track.album,
        duration: track.duration,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_track_and_playlist() {
        let track = track_dto(ytpapi2::YoutubeMusicVideoRef {
            video_id: "v1".to_string(),
            title: "Song".to_string(),
            author: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
            provider: common_structs::ProviderId::Youtube,
            set_video_id: None,
        });
        assert_eq!(track.artist, "Artist");
        let core = core_track(track);
        assert_eq!(core.author, "Artist");

        let playlist = playlist_dto(YoutubeMusicPlaylistRef {
            name: "Mix".to_string(),
            subtitle: "Artist".to_string(),
            browse_id: "VL1".to_string(),
        });
        assert_eq!(playlist.id, "VL1");
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

    #[test]
    fn playlist_errors_are_actionable() {
        let auth = playlist_error_message(
            "create playlist",
            YoutubeMusicError::AuthExpired(serde_json::json!({})),
        );
        assert!(auth.contains("Sign in again"), "{auth}");

        let rate = playlist_error_message(
            "add tracks",
            YoutubeMusicError::RateLimited(serde_json::json!({})),
        );
        assert!(rate.contains("rate limit"), "{rate}");

        let upstream = playlist_error_message(
            "rename playlist",
            YoutubeMusicError::YoutubeMusicError(
                serde_json::json!({"error": {"message": "Not found"}}),
            ),
        );
        assert!(upstream.contains("Not found"), "{upstream}");
    }
}
