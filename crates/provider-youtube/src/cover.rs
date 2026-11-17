//! Cover art fetch from the public YouTube thumbnail hosts.

use std::time::Duration;

use async_trait::async_trait;
use common_structs::{ProviderId, TrackKey};
use ytermusicd_core::CoverFetcher;

/// Largest image the fetcher accepts.
const MAX_COVER_BYTES: usize = 5 * 1024 * 1024;
/// Smallest image the fetcher accepts. Smaller responses are placeholders.
const MIN_COVER_BYTES: usize = 1024;

/// Build the cover URLs for a YouTube track ID, best first.
pub fn cover_urls(track_id: &str) -> [String; 2] {
    ["maxresdefault", "hqdefault"]
        .map(|name| format!("https://i.ytimg.com/vi/{track_id}/{name}.jpg"))
}

/// Fetch covers from the public YouTube thumbnail hosts.
pub struct YoutubeCoverFetcher {
    client: reqwest::Client,
}

impl YoutubeCoverFetcher {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("ytermusicd/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl Default for YoutubeCoverFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CoverFetcher for YoutubeCoverFetcher {
    async fn fetch(&self, key: &TrackKey) -> Option<Vec<u8>> {
        if key.provider != ProviderId::Youtube {
            return None;
        }
        let video_id = &key.id;
        for url in cover_urls(video_id) {
            let Ok(response) = self.client.get(url).send().await else {
                continue;
            };
            if !response.status().is_success() {
                continue;
            }
            let image = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("image/"));
            if !image {
                continue;
            }
            let Ok(bytes) = response.bytes().await else {
                continue;
            };
            if bytes.len() < MIN_COVER_BYTES || bytes.len() > MAX_COVER_BYTES {
                continue;
            }
            return Some(bytes.to_vec());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_urls_try_the_large_image_first() {
        assert_eq!(
            cover_urls("dQw4w9WgXcQ"),
            [
                "https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg".to_string(),
                "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg".to_string(),
            ]
        );
    }
}
