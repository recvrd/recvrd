//! Podverse music provider: channel search, item metadata and media.

use async_trait::async_trait;
use common_structs::{MediaKind, ProviderId, TrackKey, TrackMetadata, TrackSource};
use download_manager::{DownloadError, TrackDownloader};
use recvrd_api::{
    AuthStatus, MusicApi, Playlist, PlaylistDetail, PlaylistEditError, SearchResults, Track,
    TrackDetail,
};
use recvrd_core::{CoverFetcher, PodverseConfig, Track as CoreTrack};
use std::path::Path;
use tokio::io::AsyncWriteExt;

use crate::client::{ChannelResponse, IdValue, Image, Item, PodverseClient, SearchFeed};

/// Largest cover image the fetcher accepts.
const MAX_COVER_BYTES: usize = 5 * 1024 * 1024;
/// Smallest cover image the fetcher accepts.
const MIN_COVER_BYTES: usize = 1024;
/// Maximum preview length for a playlist description.
const PREVIEW_CHARS: usize = 500;

/// The Podverse provider.
pub struct PodverseProvider {
    client: PodverseClient,
}

impl PodverseProvider {
    /// Create a provider from its configuration.
    pub fn new(config: &PodverseConfig) -> Result<Self, String> {
        let Some(base_url) = config.base_url.as_deref() else {
            return Err("podverse.base_url is not set".to_string());
        };
        let client = PodverseClient::new(base_url, config.token.clone())
            .map_err(|error| error.to_string())?;
        Ok(Self { client })
    }

    async fn load_channel(&self, id: &str) -> Result<ChannelResponse, String> {
        check_id(id)?;
        if id.bytes().all(|byte| byte.is_ascii_digit()) {
            self.client
                .resolve_channel(id)
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("Podverse channel {id} is not synced with this instance"))
        } else {
            self.client
                .channel(id)
                .await
                .map_err(|error| error.to_string())
        }
    }

    async fn load_playlist(&self, id: &str) -> Result<(ChannelResponse, Vec<Item>), String> {
        let channel = self.load_channel(id).await?;
        let channel_id =
            channel_id(&channel).ok_or_else(|| format!("Podverse channel {id} has no ID"))?;
        let items = self
            .client
            .channel_items(&channel_id)
            .await
            .map_err(|error| error.to_string())?;
        Ok((channel, items))
    }
}

/// Return the ID text of a channel, or its numeric ID as text.
fn channel_id(channel: &ChannelResponse) -> Option<String> {
    channel
        .id_text
        .clone()
        .or_else(|| channel.id.as_ref().map(IdValue::as_string))
}

#[async_trait]
impl MusicApi for PodverseProvider {
    fn auth_status(&self, provider: ProviderId) -> Option<AuthStatus> {
        (provider == ProviderId::Podverse).then(AuthStatus::default)
    }

    async fn playlists(&self, provider: ProviderId) -> Result<Vec<Playlist>, String> {
        check_provider(provider)?;
        let channels = self
            .client
            .recent_channels()
            .await
            .map_err(|error| error.to_string())?;
        Ok(channels
            .iter()
            .filter_map(|channel| {
                let id = channel_id(channel)?;
                Some(playlist_from_channel(&id, channel))
            })
            .collect())
    }

    async fn playlist(&self, provider: ProviderId, id: &str) -> Result<Vec<Track>, String> {
        check_provider(provider)?;
        Ok(self.playlist_detail(provider, id).await?.tracks)
    }

    async fn playlist_detail(
        &self,
        provider: ProviderId,
        id: &str,
    ) -> Result<PlaylistDetail, String> {
        check_provider(provider)?;
        let (channel, items) = self.load_playlist(id).await?;
        let playlist = playlist_from_channel(id, &channel);
        let tracks = items
            .iter()
            .filter_map(|item| track_from_item(item, Some(&channel)))
            .collect();
        Ok(PlaylistDetail { playlist, tracks })
    }

    async fn search(&self, provider: ProviderId, query: &str) -> Result<SearchResults, String> {
        check_provider(provider)?;
        let query = query.trim();
        if query.is_empty() {
            return Err("search query must not be empty".to_string());
        }
        let feeds = self
            .client
            .search(query)
            .await
            .map_err(|error| error.to_string())?;
        let playlists = feeds
            .iter()
            .filter(|feed| is_music_feed(feed))
            .filter_map(playlist_from_search)
            .collect();
        Ok(SearchResults {
            tracks: Vec::new(),
            playlists,
        })
    }

    async fn track_detail(&self, provider: ProviderId, id: &str) -> Result<TrackDetail, String> {
        check_provider(provider)?;
        check_id(id)?;
        let item = self
            .client
            .item(id)
            .await
            .map_err(|error| error.to_string())?;
        let channel = match item.channel_id.as_ref().map(IdValue::as_string) {
            Some(channel_id) => self.client.channel(&channel_id).await.ok(),
            None => None,
        };
        let track = track_from_item(&item, channel.as_ref())
            .ok_or_else(|| format!("Podverse item {id} has no ID"))?;
        let description = item.item_description.as_ref().and_then(|description| {
            description
                .value
                .clone()
                .or_else(|| description.preview.clone())
        });
        Ok(TrackDetail { track, description })
    }

    async fn create_playlist(
        &self,
        _request: &recvrd_api::CreatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError> {
        Err(edit_unsupported("create a playlist"))
    }

    async fn update_playlist(
        &self,
        _id: &str,
        _request: &recvrd_api::UpdatePlaylistRequest,
    ) -> Result<Playlist, PlaylistEditError> {
        Err(edit_unsupported("edit a playlist"))
    }

    async fn delete_playlist(&self, _id: &str) -> Result<(), PlaylistEditError> {
        Err(edit_unsupported("delete a playlist"))
    }

    async fn add_playlist_tracks(
        &self,
        _id: &str,
        _track_ids: &[String],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        Err(edit_unsupported("add tracks"))
    }

    async fn remove_playlist_tracks(
        &self,
        _id: &str,
        _indices: &[usize],
    ) -> Result<Vec<Track>, PlaylistEditError> {
        Err(edit_unsupported("remove tracks"))
    }

    async fn move_playlist_track(
        &self,
        _id: &str,
        _from: usize,
        _to: usize,
    ) -> Result<Vec<Track>, PlaylistEditError> {
        Err(edit_unsupported("move a track"))
    }
}

#[async_trait]
impl TrackDownloader for PodverseProvider {
    async fn download(
        &self,
        track: &CoreTrack,
        output: &Path,
        progress: &(dyn Fn(usize) + Send + Sync),
    ) -> Result<(), DownloadError> {
        let Some(source) = &track.source else {
            return Err(DownloadError::BackendFailed(format!(
                "Podverse track {} has no source URL",
                track.track_id
            )));
        };
        let mut response = self
            .client
            .get_url(&source.url)
            .await
            .map_err(|error| DownloadError::BackendFailed(error.to_string()))?;
        let total = response.content_length().or(source.length).unwrap_or(0);
        let mut file = tokio::fs::File::create(output).await?;
        progress(0);
        let mut downloaded: u64 = 0;
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    file.write_all(&chunk).await?;
                    downloaded += chunk.len() as u64;
                    if let Some(percent) = downloaded.saturating_mul(100).checked_div(total) {
                        progress(percent.min(100) as usize);
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    return Err(DownloadError::BackendFailed(format!(
                        "could not read {}: {error}",
                        source.url
                    )));
                }
            }
        }
        file.flush().await?;
        progress(100);
        Ok(())
    }

    fn audio_extension(&self, track: &CoreTrack) -> String {
        track
            .source
            .as_deref()
            .map(extension_for_source)
            .unwrap_or_else(|| "mp3".to_string())
    }
}

#[async_trait]
impl CoverFetcher for PodverseProvider {
    async fn fetch(&self, key: &TrackKey) -> Option<Vec<u8>> {
        if key.provider != ProviderId::Podverse || !key.validate() {
            return None;
        }
        let item = self.client.item(&key.id).await.ok()?;
        let mut url = item_image(&item);
        if url.is_none()
            && let Some(channel_id) = item.channel_id.as_ref().map(IdValue::as_string)
            && let Ok(channel) = self.client.channel(&channel_id).await
        {
            url = image_from_images(&channel.channel_images);
        }
        let response = self.client.get_url(&url?).await.ok()?;
        if !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("image/"))
        {
            return None;
        }
        let bytes = response.bytes().await.ok()?;
        (bytes.len() >= MIN_COVER_BYTES && bytes.len() <= MAX_COVER_BYTES).then(|| bytes.to_vec())
    }
}

fn check_provider(provider: ProviderId) -> Result<(), String> {
    if provider == ProviderId::Podverse {
        Ok(())
    } else {
        Err(format!(
            "provider {provider} is not available; Podverse is the only provider for this implementation"
        ))
    }
}

fn check_id(id: &str) -> Result<(), String> {
    if ProviderId::Podverse.validate_id(id) {
        Ok(())
    } else {
        Err(format!("invalid Podverse ID {id:?}"))
    }
}

fn edit_unsupported(action: &str) -> PlaylistEditError {
    PlaylistEditError::Invalid(format!(
        "Podverse channels are read-only; {action} is not supported"
    ))
}

fn is_music_feed(feed: &SearchFeed) -> bool {
    feed.medium
        .as_deref()
        .is_some_and(|medium| medium.eq_ignore_ascii_case("music"))
}

fn playlist_from_search(feed: &SearchFeed) -> Option<Playlist> {
    let id = feed
        .id_text
        .clone()
        .or_else(|| feed.id.as_ref().map(IdValue::as_string))?;
    let name = feed.title.clone()?;
    let author = feed.author.clone().or_else(|| feed.owner_name.clone());
    Some(Playlist {
        id,
        name,
        subtitle: author.clone().unwrap_or_default(),
        image_url: feed.image.clone().or_else(|| feed.artwork.clone()),
        author,
        description: preview(feed.description.as_deref()),
        feed_url: feed.url.clone().or_else(|| feed.original_url.clone()),
        item_count: feed.item_count.or_else(|| {
            feed.channel_about
                .as_ref()
                .and_then(|about| about.episode_count)
        }),
    })
}

fn playlist_from_channel(requested_id: &str, channel: &ChannelResponse) -> Playlist {
    let id = channel
        .id_text
        .clone()
        .or_else(|| channel.id.as_ref().map(IdValue::as_string))
        .unwrap_or_else(|| requested_id.to_string());
    let author = channel
        .channel_about
        .as_ref()
        .and_then(|about| about.author.clone());
    Playlist {
        id,
        name: channel.title.clone().unwrap_or_default(),
        subtitle: author.clone().unwrap_or_default(),
        image_url: image_from_images(&channel.channel_images),
        author,
        description: channel
            .channel_description
            .as_ref()
            .and_then(|description| description.preview.clone())
            .map(|preview| truncate(&preview)),
        feed_url: channel.feed.as_ref().and_then(|feed| feed.url.clone()),
        item_count: channel.item_count.or_else(|| {
            channel
                .channel_about
                .as_ref()
                .and_then(|about| about.episode_count)
        }),
    }
}

fn track_from_item(item: &Item, channel: Option<&ChannelResponse>) -> Option<Track> {
    let track_id = item
        .id_text
        .clone()
        .or_else(|| item.id.as_ref().map(IdValue::as_string))?;
    let channel_title = channel.and_then(|channel| channel.title.clone());
    let channel_author = channel
        .and_then(|channel| channel.channel_about.as_ref())
        .and_then(|about| about.author.clone());
    let artist = item
        .author
        .clone()
        .or(channel_author)
        .or_else(|| channel_title.clone())
        .unwrap_or_default();
    Some(Track {
        provider: ProviderId::Podverse,
        track_id,
        title: item.title.clone().unwrap_or_default(),
        artist,
        album: channel_title.unwrap_or_default(),
        duration: normalize_duration(
            item.item_about
                .as_ref()
                .and_then(|about| about.duration.as_deref())
                .unwrap_or_default(),
        ),
        source: source_from_item(item).map(Box::new),
        metadata: Some(Box::new(metadata_from_item(item, channel))),
        cached: false,
    })
}

fn source_from_item(item: &Item) -> Option<TrackSource> {
    let url = item
        .guid_enclosure_url
        .clone()
        .or_else(|| item.media_url.clone())
        .or_else(|| item.enclosure_url.clone())
        .or_else(|| {
            item.item_enclosures.iter().find_map(|enclosure| {
                enclosure
                    .item_enclosure_sources
                    .iter()
                    .find_map(|source| source.uri.clone())
            })
        })?;
    let mime = item
        .item_enclosures
        .iter()
        .find_map(|enclosure| enclosure.mime.clone())
        .or_else(|| {
            item.item_enclosures.iter().find_map(|enclosure| {
                enclosure
                    .item_enclosure_sources
                    .iter()
                    .find_map(|source| source.content_type.clone())
            })
        });
    let length = item.item_enclosures.iter().find_map(|enclosure| {
        enclosure
            .length
            .as_deref()
            .and_then(|length| length.parse().ok())
    });
    let kind = if mime
        .as_deref()
        .is_some_and(|mime| mime.starts_with("video/"))
    {
        MediaKind::Video
    } else {
        MediaKind::Audio
    };
    Some(TrackSource {
        url,
        mime,
        length,
        seekable: length.is_some(),
        kind,
    })
}

fn metadata_from_item(item: &Item, channel: Option<&ChannelResponse>) -> TrackMetadata {
    TrackMetadata {
        image_url: item_image(item)
            .or_else(|| channel.and_then(|channel| image_from_images(&channel.channel_images))),
        published_at: item.pub_date.clone(),
        channel_id: item
            .channel_id
            .as_ref()
            .map(IdValue::as_string)
            .or_else(|| channel.and_then(|channel| channel.id.as_ref().map(IdValue::as_string))),
        channel_title: channel.and_then(|channel| channel.title.clone()),
        feed_url: channel
            .and_then(|channel| channel.feed.as_ref())
            .and_then(|feed| feed.url.clone()),
        website_url: item
            .item_about
            .as_ref()
            .and_then(|about| about.website_link_url.clone())
            .or_else(|| {
                channel
                    .and_then(|channel| channel.channel_about.as_ref())
                    .and_then(|about| about.website_link_url.clone())
            }),
        episode_number: item
            .item_about
            .as_ref()
            .and_then(|about| about.episode_number),
        season_number: item
            .item_season
            .as_ref()
            .and_then(|season| season.channel_season.as_ref())
            .and_then(|season| season.number)
            .or_else(|| {
                item.item_about
                    .as_ref()
                    .and_then(|about| about.season_number)
            }),
        year: item.pub_date.as_deref().and_then(year_from_date),
        genre: channel
            .and_then(|channel| channel.channel_categories.first())
            .and_then(|category| category.name.clone().or_else(|| category.title.clone())),
        track_number: None,
        disc_number: None,
    }
}

fn item_image(item: &Item) -> Option<String> {
    image_from_images(&item.item_images)
}

fn image_from_images(images: &[Image]) -> Option<String> {
    images.iter().find_map(|image| {
        image
            .original_url
            .clone()
            .or_else(|| image.url.clone())
            .or_else(|| image.medium_url.clone())
    })
}

fn preview(text: Option<&str>) -> Option<String> {
    text.map(truncate)
}

fn truncate(text: &str) -> String {
    let mut value: String = text.chars().take(PREVIEW_CHARS).collect();
    if text.chars().count() > PREVIEW_CHARS {
        value.push('…');
    }
    value
}

fn year_from_date(date: &str) -> Option<u32> {
    date.get(..4)?.parse().ok()
}

fn normalize_duration(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    if raw.contains(':') {
        return raw.to_string();
    }
    let Ok(seconds) = raw.parse::<f64>() else {
        return String::new();
    };
    if !seconds.is_finite() || seconds < 0.0 {
        return String::new();
    }
    format_seconds(seconds.round() as u64)
}

fn format_seconds(total: u64) -> String {
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn extension_for_source(source: &TrackSource) -> String {
    if let Some(mime) = &source.mime {
        let extension = match mime.as_str() {
            "audio/mpeg" | "audio/mp3" => Some("mp3"),
            "audio/mp4" | "audio/x-m4a" | "audio/aac" => Some("m4a"),
            "audio/ogg" | "audio/vorbis" => Some("ogg"),
            "audio/flac" | "audio/x-flac" => Some("flac"),
            _ => None,
        };
        if let Some(extension) = extension {
            return extension.to_string();
        }
    }
    let path = source.url.split(['?', '#']).next().unwrap_or(&source.url);
    let suffix = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match suffix.as_str() {
        "mp3" | "m4a" | "ogg" | "flac" => suffix,
        _ => "mp3".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recvrd_api::MusicApi;
    use std::sync::{Arc, Mutex};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    const SEARCH: &str = include_str!("../fixtures/search.json");
    const CHANNEL: &str = include_str!("../fixtures/channel.json");
    const CHANNELS_RECENT: &str = include_str!("../fixtures/channels-recent.json");
    const CHANNEL_PAGED: &str = include_str!("../fixtures/channel-paged.json");
    const ITEMS: &str = include_str!("../fixtures/items.json");
    const ITEMS_PAGE_1: &str = include_str!("../fixtures/items-page1.json");
    const ITEMS_PAGE_2: &str = include_str!("../fixtures/items-page2.json");
    const ITEM: &str = include_str!("../fixtures/item.json");

    /// A recorded request: path and authorization header value.
    type RecordedRequest = (String, Option<String>);

    struct FixtureServer {
        base_url: String,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        handle: JoinHandle<()>,
    }

    impl FixtureServer {
        async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let server_url = base_url.clone();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let recorded = requests.clone();
            let handle = tokio::spawn(async move {
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        break;
                    };
                    let base = server_url.clone();
                    let recorded = recorded.clone();
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 4096];
                        let read = stream.read(&mut buffer).await.unwrap_or(0);
                        let request = String::from_utf8_lossy(&buffer[..read]);
                        let path = request
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or("/");
                        let authorization = request.lines().skip(1).find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("authorization")
                                .then(|| value.trim().to_string())
                        });
                        recorded
                            .lock()
                            .unwrap()
                            .push((path.to_string(), authorization));
                        let (status, content_type, body) = response(path, &base);
                        let header = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(header.as_bytes()).await;
                        let _ = stream.write_all(&body).await;
                    });
                }
            });
            Self {
                base_url,
                requests,
                handle,
            }
        }

        fn provider(&self) -> PodverseProvider {
            PodverseProvider::new(&PodverseConfig {
                base_url: Some(self.base_url.clone()),
                token: None,
            })
            .unwrap()
        }

        fn provider_with_token(&self) -> PodverseProvider {
            PodverseProvider::new(&PodverseConfig {
                base_url: Some(self.base_url.clone()),
                token: Some("test-token".to_string()),
            })
            .unwrap()
        }

        /// Return the authorization header the server saw for one path.
        /// The outer option is None when the server saw no such request.
        fn authorization_for(&self, path: &str) -> Option<Option<String>> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .find(|(recorded, _)| recorded == path)
                .map(|(_, authorization)| authorization.clone())
        }
    }

    impl Drop for FixtureServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    fn response(path: &str, base: &str) -> (&'static str, &'static str, Vec<u8>) {
        let (path, query) = match path.split_once('?') {
            Some((path, query)) => (path, query),
            None => (path, ""),
        };
        let page = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("page="))
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(1);
        match path {
            "/external-services/podcast-index/search/podcasts" => (
                "200 OK",
                "application/json",
                SEARCH.replace("BASE", base).into_bytes(),
            ),
            "/channel/podcast-index/6654007" | "/channel/LJ24O9e4mv" | "/channel/2984" => (
                "200 OK",
                "application/json",
                CHANNEL.replace("BASE", base).into_bytes(),
            ),
            "/channel/global/recent" => (
                "200 OK",
                "application/json",
                CHANNELS_RECENT.replace("BASE", base).into_bytes(),
            ),
            "/channel/PAGED123" => (
                "200 OK",
                "application/json",
                CHANNEL_PAGED.replace("BASE", base).into_bytes(),
            ),
            "/channel/podcast-index/999" => ("200 OK", "application/json", b"null".to_vec()),
            "/item/channel/recent/LJ24O9e4mv" => {
                if page == 1 {
                    (
                        "200 OK",
                        "application/json",
                        ITEMS.replace("BASE", base).into_bytes(),
                    )
                } else {
                    ("200 OK", "application/json", b"{\"data\": []}".to_vec())
                }
            }
            "/item/channel/recent/PAGED123" => {
                if page == 1 {
                    (
                        "200 OK",
                        "application/json",
                        ITEMS_PAGE_1.replace("BASE", base).into_bytes(),
                    )
                } else if page == 2 {
                    (
                        "200 OK",
                        "application/json",
                        ITEMS_PAGE_2.replace("BASE", base).into_bytes(),
                    )
                } else {
                    ("200 OK", "application/json", b"{\"data\": []}".to_vec())
                }
            }
            "/item/YvIRJrjZVe" | "/item/52369" => (
                "200 OK",
                "application/json",
                ITEM.replace("BASE", base).into_bytes(),
            ),
            "/image.jpg" => ("200 OK", "image/jpeg", vec![b'x'; 2048]),
            "/audio.mp3" => ("200 OK", "audio/mpeg", b"audio-data".to_vec()),
            _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
        }
    }

    fn core_track(source: TrackSource) -> CoreTrack {
        CoreTrack {
            provider: ProviderId::Podverse,
            track_id: "YvIRJrjZVe".to_string(),
            title: "New Year".to_string(),
            author: "Jon Ross".to_string(),
            album: "Rock The Halls".to_string(),
            duration: "4:03".to_string(),
            source: Some(Box::new(source)),
            metadata: None,
        }
    }

    #[tokio::test]
    async fn playlists_lists_recently_updated_channels() {
        let server = FixtureServer::start().await;
        let provider = server.provider();
        let playlists = provider.playlists(ProviderId::Podverse).await.unwrap();
        assert_eq!(playlists.len(), 2);
        assert_eq!(playlists[0].id, "LJ24O9e4mv");
        assert_eq!(playlists[0].name, "Rock The Halls");
        assert_eq!(playlists[0].author.as_deref(), Some("Jon Ross"));
        assert_eq!(playlists[0].item_count, Some(3));
        assert_eq!(playlists[1].id, "6654007");
        assert_eq!(playlists[1].name, "Lofi Music");
        assert_eq!(playlists[1].item_count, Some(12));
    }

    #[tokio::test]
    async fn search_returns_music_channels() {
        let server = FixtureServer::start().await;
        let provider = server.provider();
        let results = provider.search(ProviderId::Podverse, "lofi").await.unwrap();
        assert!(results.tracks.is_empty());
        assert_eq!(results.playlists.len(), 1);
        let playlist = &results.playlists[0];
        assert_eq!(playlist.id, "6654007");
        assert_eq!(playlist.name, "Lofi Music");
        assert_eq!(playlist.author.as_deref(), Some("Test Author"));
        assert_eq!(playlist.item_count, Some(12));
        assert_eq!(
            playlist.feed_url.as_deref(),
            Some("https://feed.example/lofi.xml")
        );
    }

    #[tokio::test]
    async fn playlist_detail_maps_the_channel_and_items() {
        let server = FixtureServer::start().await;
        let provider = server.provider();
        let detail = provider
            .playlist_detail(ProviderId::Podverse, "6654007")
            .await
            .unwrap();
        assert_eq!(detail.playlist.id, "LJ24O9e4mv");
        assert_eq!(detail.playlist.name, "Rock The Halls");
        assert_eq!(detail.playlist.author.as_deref(), Some("Jon Ross"));
        assert_eq!(detail.playlist.item_count, Some(3));
        assert_eq!(detail.tracks.len(), 1);
        let track = &detail.tracks[0];
        assert_eq!(track.track_id, "YvIRJrjZVe");
        assert_eq!(track.title, "New Year");
        assert_eq!(track.artist, "Jon Ross");
        assert_eq!(track.album, "Rock The Halls");
        assert_eq!(track.duration, "4:03");
        let source = track.source.as_ref().unwrap();
        assert_eq!(source.url, format!("{}/audio.mp3", server.base_url));
        assert_eq!(source.mime.as_deref(), Some("audio/mpeg"));
        assert_eq!(source.length, Some(5_845_483));
        assert!(source.seekable);
        let metadata = track.metadata.as_ref().unwrap();
        assert_eq!(metadata.channel_title.as_deref(), Some("Rock The Halls"));
        assert_eq!(metadata.season_number, Some(1));
        assert_eq!(metadata.year, Some(2026));
        assert_eq!(
            metadata.website_url.as_deref(),
            Some("https://www.wavlake.com/album/1")
        );
    }

    #[tokio::test]
    async fn playlist_detail_pages_through_all_channel_items() {
        let server = FixtureServer::start().await;
        let provider = server.provider();
        let detail = provider
            .playlist_detail(ProviderId::Podverse, "PAGED123")
            .await
            .unwrap();
        assert_eq!(detail.playlist.id, "PAGED123");
        assert_eq!(detail.playlist.name, "Paged Channel");
        assert_eq!(detail.tracks.len(), 3);
        assert_eq!(detail.tracks[0].title, "New Year");
        assert_eq!(detail.tracks[1].title, "Second Song");
        assert_eq!(detail.tracks[2].title, "Third Song");
    }

    #[tokio::test]
    async fn channel_items_stops_at_an_empty_page() {
        let server = FixtureServer::start().await;
        let provider = server.provider();
        let items = provider.client.channel_items("LJ24O9e4mv").await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title.as_deref(), Some("New Year"));
    }

    #[tokio::test]
    async fn track_detail_returns_the_description() {
        let server = FixtureServer::start().await;
        let provider = server.provider();
        let detail = provider
            .track_detail(ProviderId::Podverse, "YvIRJrjZVe")
            .await
            .unwrap();
        assert_eq!(detail.track.title, "New Year");
        assert_eq!(detail.description.as_deref(), Some("A full description."));
        assert_eq!(
            detail
                .track
                .metadata
                .as_ref()
                .unwrap()
                .channel_title
                .as_deref(),
            Some("Rock The Halls")
        );
    }

    #[tokio::test]
    async fn cover_fetch_returns_the_item_image() {
        let server = FixtureServer::start().await;
        let provider = server.provider();
        let bytes = provider
            .fetch(&TrackKey::podverse("YvIRJrjZVe"))
            .await
            .unwrap();
        assert_eq!(bytes.len(), 2048);
    }

    #[tokio::test]
    async fn download_writes_the_source_bytes() {
        let server = FixtureServer::start().await;
        let provider = server.provider();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("track.mp3");
        let track = core_track(TrackSource {
            url: format!("{}/audio.mp3", server.base_url),
            mime: Some("audio/mpeg".to_string()),
            length: Some(10),
            seekable: true,
            kind: MediaKind::Audio,
        });
        let progress = std::sync::Mutex::new(Vec::new());
        provider
            .download(&track, &path, &|percent| {
                progress.lock().unwrap().push(percent);
            })
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"audio-data");
        let progress = progress.lock().unwrap();
        assert_eq!(progress.first(), Some(&0));
        assert_eq!(progress.last(), Some(&100));
    }

    #[test]
    fn requires_a_base_url() {
        assert!(PodverseProvider::new(&PodverseConfig::default()).is_err());
    }

    #[test]
    fn audio_extension_uses_the_mime_type_then_the_url() {
        assert_eq!(
            extension_for_source(&TrackSource {
                url: "https://example.com/track".to_string(),
                mime: Some("audio/mpeg".to_string()),
                length: None,
                seekable: false,
                kind: MediaKind::Audio,
            }),
            "mp3"
        );
        assert_eq!(
            extension_for_source(&TrackSource {
                url: "https://example.com/track.m4a".to_string(),
                mime: None,
                length: None,
                seekable: false,
                kind: MediaKind::Audio,
            }),
            "m4a"
        );
    }

    #[tokio::test]
    async fn download_sends_the_token_to_the_api_origin() {
        let server = FixtureServer::start().await;
        let provider = server.provider_with_token();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("track.mp3");
        let track = core_track(TrackSource {
            url: format!("{}/audio.mp3", server.base_url),
            mime: Some("audio/mpeg".to_string()),
            length: Some(10),
            seekable: true,
            kind: MediaKind::Audio,
        });
        provider.download(&track, &path, &|_| {}).await.unwrap();
        assert_eq!(
            server.authorization_for("/audio.mp3"),
            Some(Some("Bearer test-token".to_string()))
        );
    }

    #[tokio::test]
    async fn download_withholds_the_token_from_another_origin() {
        let server = FixtureServer::start().await;
        let peer = FixtureServer::start().await;
        let provider = server.provider_with_token();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("track.mp3");
        let track = core_track(TrackSource {
            url: format!("{}/audio.mp3", peer.base_url),
            mime: Some("audio/mpeg".to_string()),
            length: Some(10),
            seekable: true,
            kind: MediaKind::Audio,
        });
        provider.download(&track, &path, &|_| {}).await.unwrap();
        assert_eq!(peer.authorization_for("/audio.mp3"), Some(None));
    }

    #[tokio::test]
    async fn cover_fetch_sends_the_token_to_the_api_origin() {
        let server = FixtureServer::start().await;
        let provider = server.provider_with_token();
        let bytes = provider
            .fetch(&TrackKey::podverse("YvIRJrjZVe"))
            .await
            .unwrap();
        assert_eq!(bytes.len(), 2048);
        assert_eq!(
            server.authorization_for("/image.jpg"),
            Some(Some("Bearer test-token".to_string()))
        );
    }

    #[tokio::test]
    async fn rejects_an_unsynced_search_result() {
        let server = FixtureServer::start().await;
        let provider = server.provider();
        let error = provider
            .playlist_detail(ProviderId::Podverse, "999")
            .await
            .unwrap_err();
        assert!(error.contains("is not synced"), "{error}");
    }

    #[tokio::test]
    async fn rejects_invalid_ids() {
        let server = FixtureServer::start().await;
        let provider = server.provider();
        assert!(
            provider
                .playlist_detail(ProviderId::Podverse, "../item")
                .await
                .is_err()
        );
        assert!(
            provider
                .track_detail(ProviderId::Podverse, "../item")
                .await
                .is_err()
        );
    }
}
