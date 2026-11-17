//! YouTube Music API client. It implements the provider-neutral
//! `MusicApi` trait on top of `ytpapi2`.

use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use ytermusicd_api::{
    CreatePlaylistRequest, MusicApi, Playlist, PlaylistEditError, PlaylistPrivacy, SearchResults,
    Track, UpdatePlaylistRequest,
};
use ytpapi2::{
    COOKIE, Endpoint, HeaderMap, HeaderValue, PlaylistPrivacy as UpstreamPrivacy, USER_AGENT,
    YoutubeMusicError, YoutubeMusicInstance, YoutubeMusicPlaylistRef, YoutubeMusicVideoRef,
};

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

/// The YouTube Music implementation of the provider-neutral API.
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
        track_ids: &[String],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        self.client
            .add_playlist_items(id, track_ids)
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
