//! RSS and Atom music provider for recvrd: feed parsing,
//! feed management, the music API, enclosure downloads and
//! cover fetches.

mod feed;
mod provider;
mod store;

pub use feed::{Episode, Feed, episode_id, extension_for_enclosure, feed_id, parse_feed};
pub use provider::RssProvider;
pub use store::{RssStore, StoredEpisode, StoredFeed};
