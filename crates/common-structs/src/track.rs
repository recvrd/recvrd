//! Media source and metadata shared by the core and the API.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The kind of media a source points to.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MediaKind {
    /// Audio media.
    #[default]
    Audio,
    /// Video media. The daemon does not play video yet.
    Video,
}

/// A remote media source for a track.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrackSource {
    /// The direct media URL.
    pub url: String,
    /// The MIME type reported by the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    /// The media length in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number | null")]
    pub length: Option<u64>,
    /// True when the source supports seek requests.
    #[serde(default)]
    pub seekable: bool,
    /// The kind of media.
    #[serde(default)]
    pub kind: MediaKind,
}

/// Provider metadata for a track. Empty fields are omitted from JSON.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrackMetadata {
    /// The cover image URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// The publication time, as an RFC 3339 string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    /// The provider channel ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    /// The provider channel title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_title: Option<String>,
    /// The feed URL for a channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed_url: Option<String>,
    /// The website URL for the item or channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
    /// The episode number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_number: Option<u32>,
    /// The season number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season_number: Option<u32>,
    /// The publication year.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    /// The genre or category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    /// The track number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_number: Option<u32>,
    /// The disc number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<u32>,
}
