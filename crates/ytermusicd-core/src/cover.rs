//! Cover art fetch for one track key.

use async_trait::async_trait;
use common_structs::TrackKey;

/// Fetch the JPEG bytes for a track.
#[async_trait]
#[allow(clippy::double_must_use)]
pub trait CoverFetcher: Send + Sync {
    async fn fetch(&self, key: &TrackKey) -> Option<Vec<u8>>;
}
