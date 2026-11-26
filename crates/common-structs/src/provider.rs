//! Provider identity for tracks.
//!
//! A track belongs to one provider. The provider owns the track ID
//! namespace, so the same local ID from two providers names two
//! different tracks. The composite key is [`TrackKey`].

use std::{
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A music provider. Local files are the default when configured.
/// RSS is next. YouTube Music is the last provider.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    /// The default for old payloads without a provider field.
    #[default]
    Youtube,
    Rss,
    Podverse,
    Local,
}

impl ProviderId {
    /// All providers the daemon knows, in display order. Local files
    /// come first, YouTube Music comes last.
    pub const fn all() -> &'static [Self] {
        &[Self::Local, Self::Rss, Self::Podverse, Self::Youtube]
    }

    /// The default provider for a fresh install. Local files need a
    /// configured directory, so RSS is the fallback in practice.
    pub const fn default_provider() -> Self {
        Self::Local
    }

    /// The first provider in display order that is available. A fresh
    /// install always has RSS, so the list is never empty in practice.
    pub fn default_for(providers: &[Self]) -> Self {
        Self::all()
            .iter()
            .copied()
            .find(|provider| providers.contains(provider))
            .unwrap_or_else(Self::default_provider)
    }

    /// The display name for the provider, as shown in clients.
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::Youtube => "YouTube Music",
            Self::Rss => "RSS",
            Self::Podverse => "Podverse",
            Self::Local => "Local files",
        }
    }

    /// The provider name as it appears in JSON and in cache paths.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Youtube => "youtube",
            Self::Rss => "rss",
            Self::Podverse => "podverse",
            Self::Local => "local",
        }
    }

    /// Check that a track ID is safe to use for this provider.
    /// A YouTube Music track ID has 11 characters from `[A-Za-z0-9_-]`.
    /// An RSS track ID has 32 lowercase hexadecimal characters: the
    /// first 32 characters of the SHA-256 hash of the episode identity.
    /// A local track ID is the same hash of the relative file path.
    /// Any other value can escape the cache directory when it is
    /// joined into a path, so callers must reject it.
    pub fn validate_id(&self, id: &str) -> bool {
        match self {
            Self::Youtube => {
                id.len() == 11
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            }
            Self::Rss | Self::Local => {
                id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
            }
            Self::Podverse => {
                (1..=64).contains(&id.len())
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            }
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProviderId {
    type Err = UnknownProvider;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "youtube" => Ok(Self::Youtube),
            "rss" => Ok(Self::Rss),
            "podverse" => Ok(Self::Podverse),
            "local" => Ok(Self::Local),
            _ => Err(UnknownProvider),
        }
    }
}

/// The audio file extensions the daemon can download and play.
/// YouTube tracks use `mp4`. RSS enclosures use the extension of
/// the enclosure. The download manager and the playback service
/// both use this list, so they agree on the file for a track.
pub const AUDIO_EXTENSIONS: [&str; 6] = ["mp4", "mp3", "m4a", "ogg", "flac", "wav"];

/// Error for a provider name that has no variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownProvider;

impl fmt::Display for UnknownProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unknown provider")
    }
}

impl std::error::Error for UnknownProvider {}

/// The composite identity of a track: its provider and its local ID.
/// Two tracks with the same local ID from different providers have
/// different keys and never collide.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
pub struct TrackKey {
    pub provider: ProviderId,
    pub id: String,
}

impl TrackKey {
    /// Build a key for the YouTube Music provider.
    pub fn youtube(id: impl Into<String>) -> Self {
        Self {
            provider: ProviderId::Youtube,
            id: id.into(),
        }
    }

    /// Build a key for the RSS provider.
    pub fn rss(id: impl Into<String>) -> Self {
        Self {
            provider: ProviderId::Rss,
            id: id.into(),
        }
    }

    /// Build a key for the Podverse provider.
    pub fn podverse(id: impl Into<String>) -> Self {
        Self {
            provider: ProviderId::Podverse,
            id: id.into(),
        }
    }

    /// Build a key for any provider.
    pub fn new(provider: ProviderId, id: impl Into<String>) -> Self {
        Self {
            provider,
            id: id.into(),
        }
    }

    /// Check that the local ID is valid for the provider.
    pub fn validate(&self) -> bool {
        self.provider.validate_id(&self.id)
    }

    /// The download file for this track below the downloads directory:
    /// `downloads/<provider>/<id>.<extension>`.
    /// Callers must call [`TrackKey::validate`] before they use the path.
    pub fn download_file(&self, downloads_dir: &Path, extension: &str) -> PathBuf {
        downloads_dir
            .join(self.provider.as_str())
            .join(format!("{}.{extension}", self.id))
    }

    /// The downloaded audio file for this track, in any supported
    /// extension. The download manager keeps at most one extension
    /// per track, so the first file that exists is the track.
    pub fn existing_audio_file(&self, downloads_dir: &Path) -> Option<PathBuf> {
        AUDIO_EXTENSIONS
            .iter()
            .map(|extension| self.download_file(downloads_dir, extension))
            .find(|path| path.is_file())
    }

    /// The cover file for this track below the covers directory:
    /// `covers/<provider>/<id>.jpg`.
    /// Callers must call [`TrackKey::validate`] before they use the path.
    pub fn cover_file(&self, covers_dir: &Path) -> PathBuf {
        covers_dir
            .join(self.provider.as_str())
            .join(format!("{}.jpg", self.id))
    }
}

impl fmt::Display for TrackKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.provider.as_str(), self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_list_puts_local_and_rss_first() {
        assert_eq!(
            ProviderId::all(),
            &[
                ProviderId::Local,
                ProviderId::Rss,
                ProviderId::Podverse,
                ProviderId::Youtube
            ]
        );
        assert_eq!(ProviderId::default_provider(), ProviderId::Local);
        assert_eq!(
            ProviderId::default_for(&[ProviderId::Youtube, ProviderId::Rss]),
            ProviderId::Rss
        );
        assert_eq!(
            ProviderId::default_for(&[ProviderId::Local, ProviderId::Youtube]),
            ProviderId::Local
        );
        assert_eq!(
            ProviderId::default_for(&[ProviderId::Youtube]),
            ProviderId::Youtube
        );
        assert_eq!(ProviderId::Youtube.display_name(), "YouTube Music");
        assert_eq!(ProviderId::Rss.display_name(), "RSS");
        assert_eq!(ProviderId::Podverse.display_name(), "Podverse");
        assert_eq!(ProviderId::Local.display_name(), "Local files");
    }

    #[test]
    fn youtube_accepts_eleven_char_ids() {
        assert!(ProviderId::Youtube.validate_id("dQw4w9WgXcQ"));
        assert!(ProviderId::Youtube.validate_id("abc_DEF-123"));
        assert!(!ProviderId::Youtube.validate_id("short"));
        assert!(!ProviderId::Youtube.validate_id("../abcdefghijk"));
        assert!(!ProviderId::Youtube.validate_id(""));
    }

    #[test]
    fn rss_accepts_32_hex_char_ids() {
        assert!(ProviderId::Rss.validate_id("0123456789abcdef0123456789abcdef"));
        assert!(!ProviderId::Rss.validate_id("0123456789abcdef0123456789abcdeg"));
        assert!(!ProviderId::Rss.validate_id("0123456789abcdef"));
        assert!(!ProviderId::Rss.validate_id("../3456789abcdef0123456789abcdef"));
        assert!(!ProviderId::Rss.validate_id(""));
    }

    #[test]
    fn podverse_accepts_item_text_and_numeric_ids() {
        assert!(ProviderId::Podverse.validate_id("YvIRJrjZVe"));
        assert!(ProviderId::Podverse.validate_id("52369"));
        assert!(ProviderId::Podverse.validate_id("item-id_1"));
        assert!(!ProviderId::Podverse.validate_id(""));
        assert!(!ProviderId::Podverse.validate_id("../item"));
        assert!(!ProviderId::Podverse.validate_id("item/1"));
    }

    #[test]
    fn provider_parses_from_its_name() {
        assert_eq!(
            "youtube".parse::<ProviderId>().unwrap(),
            ProviderId::Youtube
        );
        assert_eq!("rss".parse::<ProviderId>().unwrap(), ProviderId::Rss);
        assert_eq!(
            "podverse".parse::<ProviderId>().unwrap(),
            ProviderId::Podverse
        );
        assert!("tidal".parse::<ProviderId>().is_err());
    }

    #[test]
    fn same_local_id_from_different_providers_is_a_different_key() {
        let key = TrackKey::youtube("dQw4w9WgXcQ");
        assert_eq!(key, TrackKey::new(ProviderId::Youtube, "dQw4w9WgXcQ"));
        assert_ne!(key, TrackKey::rss("dQw4w9WgXcQ"));
        assert_ne!(key, TrackKey::podverse("dQw4w9WgXcQ"));
    }

    #[test]
    fn cache_paths_are_scoped_by_provider() {
        let key = TrackKey::youtube("dQw4w9WgXcQ");
        assert_eq!(
            key.download_file(Path::new("/cache/downloads"), "mp4"),
            PathBuf::from("/cache/downloads/youtube/dQw4w9WgXcQ.mp4")
        );
        assert_eq!(
            key.download_file(Path::new("/cache/downloads"), "json"),
            PathBuf::from("/cache/downloads/youtube/dQw4w9WgXcQ.json")
        );
        assert_eq!(
            key.cover_file(Path::new("/cache/covers")),
            PathBuf::from("/cache/covers/youtube/dQw4w9WgXcQ.jpg")
        );
    }

    #[test]
    fn provider_serializes_as_its_name() {
        assert_eq!(
            serde_json::to_string(&ProviderId::Youtube).unwrap(),
            "\"youtube\""
        );
        assert_eq!(
            serde_json::from_str::<ProviderId>("\"youtube\"").unwrap(),
            ProviderId::Youtube
        );
        assert_eq!(serde_json::to_string(&ProviderId::Rss).unwrap(), "\"rss\"");
        assert_eq!(
            serde_json::from_str::<ProviderId>("\"rss\"").unwrap(),
            ProviderId::Rss
        );
        assert!(serde_json::from_str::<ProviderId>("\"tidal\"").is_err());
    }
}
