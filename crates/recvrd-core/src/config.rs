use serde::{Deserialize, Serialize};
use std::{
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Downloader {
    #[default]
    Ytdlp,
    RustyYtdl,
}

/// Podverse provider configuration.
#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct PodverseConfig {
    /// The Podverse API base URL, including the API version.
    pub base_url: Option<String>,
    /// An optional bearer token for the API.
    pub token: Option<String>,
}

impl std::fmt::Debug for PodverseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PodverseConfig")
            .field("base_url", &self.base_url)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct DaemonConfig {
    /// Bind address for the HTTP API. CLI/env override this.
    pub bind: Option<String>,
    /// Port for the HTTP API. CLI/env override this.
    pub port: Option<u16>,
    /// Explicit headers file. CLI/env override this.
    pub headers: Option<PathBuf>,
    /// Maximum number of parallel downloads.
    pub parallel_downloads: u16,
    pub downloader: Downloader,
    /// Enable MPRIS/D-Bus media controls.
    pub dbus: bool,
    /// Initial volume, 0-100.
    pub initial_volume: u8,
    /// Maximum size of the cover cache in MiB. 0 disables the limit.
    pub cover_cache_mb: u64,
    /// Podverse provider configuration.
    pub podverse: PodverseConfig,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            bind: None,
            port: None,
            headers: None,
            parallel_downloads: 4,
            downloader: Downloader::Ytdlp,
            dbus: true,
            initial_volume: 50,
            cover_cache_mb: 100,
            podverse: PodverseConfig::default(),
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid config {}: {message}", path.display())]
    Invalid { path: PathBuf, message: String },
}

/// Check whether a bind address stays on loopback.
///
/// The daemon API has no authentication, so only a loopback bind keeps
/// control of playback and playlists local. `localhost` counts as loopback.
/// Any other value, including a value that is not an IP address, does not.
pub fn is_loopback_bind(bind: &str) -> bool {
    if bind == "localhost" {
        return true;
    }
    bind.parse::<IpAddr>()
        .map(|address| address.is_loopback())
        .unwrap_or(false)
}

impl DaemonConfig {
    /// Load and validate a config file. Parse and validation errors are fatal.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Self = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        config.validate(path)?;
        Ok(config)
    }

    /// Load a config file, using defaults only when the file does not exist.
    pub fn load_or_default(path: &Path) -> Result<Self, ConfigError> {
        match fs::metadata(path) {
            Ok(_) => Self::load(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    fn validate(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(bind) = &self.bind
            && bind.parse::<IpAddr>().is_err()
        {
            return Err(ConfigError::Invalid {
                path: path.to_path_buf(),
                message: format!("bind must be a valid IP address, got {bind:?}"),
            });
        }
        if self.parallel_downloads == 0 {
            return Err(ConfigError::Invalid {
                path: path.to_path_buf(),
                message: "parallel_downloads must be at least 1".to_string(),
            });
        }
        if self.initial_volume > 100 {
            return Err(ConfigError::Invalid {
                path: path.to_path_buf(),
                message: "initial_volume must be between 0 and 100".to_string(),
            });
        }
        if let Some(base_url) = &self.podverse.base_url
            && base_url.trim().is_empty()
        {
            return Err(ConfigError::Invalid {
                path: path.to_path_buf(),
                message: "podverse.base_url must not be empty".to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_are_sane() {
        let config = DaemonConfig::default();
        assert_eq!(config.parallel_downloads, 4);
        assert_eq!(config.downloader, Downloader::Ytdlp);
        assert!(config.dbus);
        assert_eq!(config.initial_volume, 50);
        assert_eq!(config.cover_cache_mb, 100);
        assert_eq!(config.bind, None);
        assert_eq!(config.port, None);
        assert_eq!(config.headers, None);
        assert_eq!(config.podverse, PodverseConfig::default());
    }

    #[test]
    fn loads_valid_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
bind = "0.0.0.0"
port = 9000
headers = "/tmp/headers.txt"
parallel_downloads = 2
downloader = "rusty_ytdl"
dbus = false
initial_volume = 75
cover_cache_mb = 25

[podverse]
base_url = "https://podverse.example.com/api/v2"
token = "secret"
"#,
        )
        .unwrap();

        let config = DaemonConfig::load(&path).unwrap();
        assert_eq!(config.bind.as_deref(), Some("0.0.0.0"));
        assert_eq!(config.port, Some(9000));
        assert_eq!(config.headers, Some(PathBuf::from("/tmp/headers.txt")));
        assert_eq!(config.parallel_downloads, 2);
        assert_eq!(config.downloader, Downloader::RustyYtdl);
        assert!(!config.dbus);
        assert_eq!(config.initial_volume, 75);
        assert_eq!(config.cover_cache_mb, 25);
        assert_eq!(
            config.podverse.base_url.as_deref(),
            Some("https://podverse.example.com/api/v2")
        );
        assert_eq!(config.podverse.token.as_deref(), Some("secret"));
        assert!(!format!("{:?}", config.podverse).contains("secret"));
    }

    #[test]
    fn missing_file_uses_defaults() {
        let dir = tempdir().unwrap();
        let config = DaemonConfig::load_or_default(&dir.path().join("missing.toml")).unwrap();
        assert_eq!(config, DaemonConfig::default());
    }

    #[test]
    fn parse_errors_include_the_path() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "port = [").unwrap();

        let error = DaemonConfig::load(&path).unwrap_err().to_string();
        assert!(error.contains("config.toml"), "{error}");
        assert!(error.contains("failed to parse"), "{error}");
    }

    #[test]
    fn rejects_invalid_parallel_downloads() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "parallel_downloads = 0").unwrap();

        let error = DaemonConfig::load(&path).unwrap_err().to_string();
        assert!(error.contains("parallel_downloads"), "{error}");
    }

    #[test]
    fn rejects_invalid_volume() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "initial_volume = 101").unwrap();

        let error = DaemonConfig::load(&path).unwrap_err().to_string();
        assert!(error.contains("initial_volume"), "{error}");
    }

    #[test]
    fn rejects_an_empty_podverse_base_url() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[podverse]\nbase_url = \"  \"").unwrap();

        let error = DaemonConfig::load(&path).unwrap_err().to_string();
        assert!(error.contains("podverse.base_url"), "{error}");
    }

    #[test]
    fn rejects_invalid_bind() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "bind = \"not-an-ip\"").unwrap();

        let error = DaemonConfig::load(&path).unwrap_err().to_string();
        assert!(error.contains("bind"), "{error}");
    }

    #[test]
    fn accepts_ip_binds() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "bind = \"0.0.0.0\"").unwrap();

        let config = DaemonConfig::load(&path).unwrap();
        assert_eq!(config.bind.as_deref(), Some("0.0.0.0"));
    }

    #[test]
    fn loopback_bind_check() {
        assert!(is_loopback_bind("127.0.0.1"));
        assert!(is_loopback_bind("::1"));
        assert!(is_loopback_bind("localhost"));
        assert!(!is_loopback_bind("0.0.0.0"));
        assert!(!is_loopback_bind("192.168.1.2"));
        assert!(!is_loopback_bind("not-an-ip"));
    }
}
