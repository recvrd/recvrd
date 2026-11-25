//! Playback modes shared by the daemon and its clients.

use crate::ProviderId;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How the daemon plays a provider track.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PlaybackMode {
    /// Wait for the downloaded file.
    #[default]
    Download,
    /// Play the remote URL and keep no cache.
    Stream,
    /// Play the remote URL and download the cache in the background.
    StreamThenDownload,
}

impl PlaybackMode {
    /// The default mode for a provider.
    pub const fn default_for(provider: ProviderId) -> Self {
        match provider {
            ProviderId::Youtube => Self::Download,
            ProviderId::Rss => Self::Download,
            ProviderId::Podverse => Self::StreamThenDownload,
            ProviderId::Local => Self::Download,
        }
    }

    /// The modes a provider supports.
    pub const fn supported_for(provider: ProviderId) -> &'static [Self] {
        match provider {
            ProviderId::Youtube | ProviderId::Local => &[Self::Download],
            ProviderId::Rss | ProviderId::Podverse => {
                &[Self::Download, Self::Stream, Self::StreamThenDownload]
            }
        }
    }

    /// The JSON name of the mode.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Download => "download",
            Self::Stream => "stream",
            Self::StreamThenDownload => "stream_then_download",
        }
    }
}

/// The source that is currently playing.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PlaybackSourceKind {
    /// A local cache file.
    File,
    /// A remote stream.
    Stream,
}
