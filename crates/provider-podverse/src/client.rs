//! HTTP client for the Podverse music API.

use reqwest::Url;
use serde::{Deserialize, de::DeserializeOwned};
use std::time::Duration;
use thiserror::Error;

/// Maximum pages to fetch for one channel. This guards against
/// an instance that repeats a non-empty page without end.
const MAX_CHANNEL_ITEM_PAGES: u32 = 100;

/// An ID that the API may send as a number or a string.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum IdValue {
    Number(u64),
    Text(String),
}

impl IdValue {
    /// Return the ID as a string.
    pub fn as_string(&self) -> String {
        match self {
            Self::Number(value) => value.to_string(),
            Self::Text(value) => value.clone(),
        }
    }
}

/// A Podverse API error.
#[derive(Debug, Error)]
pub enum PodverseError {
    #[error("Podverse request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("invalid Podverse configuration: {0}")]
    Config(String),
}

/// The search response. The API uses `feeds` for Podcast Index results.
#[derive(Debug, Default, Deserialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub feeds: Vec<SearchFeed>,
    #[serde(default)]
    pub data: Vec<SearchFeed>,
}

/// One Podcast Index feed from a search.
#[derive(Debug, Default, Deserialize)]
pub struct SearchFeed {
    #[serde(default)]
    pub id: Option<IdValue>,
    #[serde(default)]
    pub id_text: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default, rename = "ownerName")]
    pub owner_name: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub artwork: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default, rename = "originalUrl")]
    pub original_url: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub medium: Option<String>,
    #[serde(default)]
    pub item_count: Option<u32>,
    #[serde(default)]
    pub channel_about: Option<ChannelAbout>,
}

/// One Podverse channel.
#[derive(Debug, Default, Deserialize)]
pub struct ChannelResponse {
    #[serde(default)]
    pub id: Option<IdValue>,
    #[serde(default)]
    pub id_text: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub feed: Option<FeedRef>,
    #[serde(default)]
    pub channel_about: Option<ChannelAbout>,
    #[serde(default)]
    pub channel_description: Option<Description>,
    #[serde(default)]
    pub channel_images: Vec<Image>,
    #[serde(default)]
    pub channel_categories: Vec<Category>,
    #[serde(default)]
    pub item_count: Option<u32>,
}

/// The feed reference for a channel.
#[derive(Debug, Default, Deserialize)]
pub struct FeedRef {
    #[serde(default)]
    pub url: Option<String>,
}

/// The about record for a channel.
#[derive(Debug, Default, Deserialize)]
pub struct ChannelAbout {
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub episode_count: Option<u32>,
    #[serde(default)]
    pub website_link_url: Option<String>,
}

/// A text description, with a full value and a preview.
#[derive(Debug, Default, Deserialize)]
pub struct Description {
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
}

/// One image record.
#[derive(Debug, Default, Deserialize)]
pub struct Image {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub original_url: Option<String>,
    #[serde(default)]
    pub medium_url: Option<String>,
}

/// One channel category.
#[derive(Debug, Default, Deserialize)]
pub struct Category {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

/// The channel items response.
#[derive(Debug, Default, Deserialize)]
pub struct ItemsResponse {
    #[serde(default)]
    pub data: Vec<Item>,
    #[serde(default)]
    pub meta: Option<ItemsMeta>,
}

/// Pagination metadata for a channel items page.
#[derive(Debug, Default, Deserialize)]
pub struct ItemsMeta {
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// One Podverse item.
#[derive(Debug, Default, Deserialize)]
pub struct Item {
    #[serde(default)]
    pub id: Option<IdValue>,
    #[serde(default)]
    pub id_text: Option<String>,
    #[serde(default)]
    pub channel_id: Option<IdValue>,
    #[serde(default)]
    pub guid_enclosure_url: Option<String>,
    #[serde(default, rename = "mediaUrl")]
    pub media_url: Option<String>,
    #[serde(default, rename = "enclosureUrl")]
    pub enclosure_url: Option<String>,
    #[serde(default)]
    pub pub_date: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub item_about: Option<ItemAbout>,
    #[serde(default)]
    pub item_description: Option<Description>,
    #[serde(default)]
    pub item_enclosures: Vec<Enclosure>,
    #[serde(default)]
    pub item_images: Vec<Image>,
    #[serde(default)]
    pub item_season: Option<ItemSeason>,
}

/// The about record for an item.
#[derive(Debug, Default, Deserialize)]
pub struct ItemAbout {
    #[serde(default)]
    pub duration: Option<String>,
    #[serde(default)]
    pub website_link_url: Option<String>,
    #[serde(default)]
    pub episode_number: Option<u32>,
    #[serde(default)]
    pub season_number: Option<u32>,
}

/// One enclosure for an item.
#[derive(Debug, Default, Deserialize)]
pub struct Enclosure {
    #[serde(default, rename = "type")]
    pub mime: Option<String>,
    #[serde(default)]
    pub length: Option<String>,
    #[serde(default)]
    pub item_enclosure_sources: Vec<EnclosureSource>,
}

/// One enclosure source.
#[derive(Debug, Default, Deserialize)]
pub struct EnclosureSource {
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
}

/// The season record for an item.
#[derive(Debug, Default, Deserialize)]
pub struct ItemSeason {
    #[serde(default)]
    pub channel_season: Option<ChannelSeason>,
}

/// The channel season record.
#[derive(Debug, Default, Deserialize)]
pub struct ChannelSeason {
    #[serde(default)]
    pub number: Option<u32>,
}

/// The Podverse HTTP client.
pub struct PodverseClient {
    base_url: String,
    base: Url,
    token: Option<String>,
    http: reqwest::Client,
}

impl PodverseClient {
    /// Create a client for a configured API base URL.
    pub fn new(base_url: &str, token: Option<String>) -> Result<Self, PodverseError> {
        let base_url = base_url.trim().trim_end_matches('/');
        if base_url.is_empty() {
            return Err(PodverseError::Config(
                "podverse.base_url must not be empty".to_string(),
            ));
        }
        let url = Url::parse(base_url)
            .map_err(|error| PodverseError::Config(format!("invalid base_url: {error}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(PodverseError::Config(
                "podverse.base_url must use http or https".to_string(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(concat!("recvrd/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            base_url: base_url.to_string(),
            base: url,
            token,
            http,
        })
    }

    /// Search Podcast Index feeds through the instance.
    pub async fn search(&self, query: &str) -> Result<Vec<SearchFeed>, PodverseError> {
        let response: SearchResponse = self
            .request("/external-services/podcast-index/search/podcasts")
            .query(&[("q", query)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(if response.feeds.is_empty() {
            response.data
        } else {
            response.feeds
        })
    }

    /// Resolve a Podcast Index ID to a Podverse channel. A null
    /// response means the instance has not synced the feed.
    pub async fn resolve_channel(
        &self,
        id: &str,
    ) -> Result<Option<ChannelResponse>, PodverseError> {
        self.channel_by_podcast_index(id).await
    }

    /// Fetch one channel by ID or ID text.
    pub async fn channel(&self, id: &str) -> Result<ChannelResponse, PodverseError> {
        self.get_json(&format!("/channel/{id}")).await
    }

    async fn channel_by_podcast_index(
        &self,
        id: &str,
    ) -> Result<Option<ChannelResponse>, PodverseError> {
        self.get_json(&format!("/channel/podcast-index/{id}")).await
    }

    /// Fetch all recent items for a channel. Page through the results
    /// until a page returns no items, a short page, or the reported total.
    pub async fn channel_items(&self, id: &str) -> Result<Vec<Item>, PodverseError> {
        let mut items = Vec::new();
        let mut page = 1;
        loop {
            let response: ItemsResponse = self
                .get_json(&format!("/item/channel/recent/{id}?page={page}"))
                .await?;
            let meta = response.meta;
            let total = meta.as_ref().and_then(|meta| meta.count);
            let short = meta
                .as_ref()
                .and_then(|meta| meta.limit)
                .is_some_and(|limit| response.data.len() < limit as usize);
            let empty = response.data.is_empty();
            items.extend(response.data);
            if empty
                || short
                || total.is_some_and(|total| items.len() >= total as usize)
                || page >= MAX_CHANNEL_ITEM_PAGES
            {
                break;
            }
            page += 1;
        }
        Ok(items)
    }

    /// Fetch one item by ID or ID text.
    pub async fn item(&self, id: &str) -> Result<Item, PodverseError> {
        self.get_json(&format!("/item/{id}")).await
    }

    /// Fetch a media URL. Send the token only when the URL shares
    /// the origin of the configured API base URL.
    pub async fn get_url(&self, url: &str) -> Result<reqwest::Response, PodverseError> {
        let mut request = self.http.get(url);
        if self.same_origin(url)
            && let Some(token) = &self.token
        {
            request = request.bearer_auth(token);
        }
        Ok(request.send().await?.error_for_status()?)
    }

    /// Check whether a URL shares the scheme, host, and port of the base URL.
    fn same_origin(&self, url: &str) -> bool {
        let Ok(url) = Url::parse(url) else {
            return false;
        };
        url.scheme() == self.base.scheme()
            && url.host() == self.base.host()
            && url.port_or_known_default() == self.base.port_or_known_default()
    }

    fn request(&self, path: &str) -> reqwest::RequestBuilder {
        let mut request = self.http.get(format!("{}{path}", self.base_url));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        request
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, PodverseError> {
        Ok(self
            .request(path)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}
