//! Playback settings that the daemon writes at runtime.

use common_structs::{PlaybackMode, ProviderId};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};

/// The settings file version.
pub const SETTINGS_VERSION: u32 = 1;

/// The settings stored in `settings.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackSettings {
    /// The settings format version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// The selected mode for each provider.
    pub playback: BTreeMap<ProviderId, PlaybackMode>,
    /// Whether the daemon runs in offline mode.
    pub offline: bool,
}

fn default_version() -> u32 {
    SETTINGS_VERSION
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            playback: BTreeMap::new(),
            offline: false,
        }
    }
}

impl PlaybackSettings {
    /// Return the mode for a provider, with the provider default.
    pub fn mode(&self, provider: ProviderId) -> PlaybackMode {
        self.playback
            .get(&provider)
            .copied()
            .unwrap_or_else(|| PlaybackMode::default_for(provider))
    }

    /// Set the mode for a provider.
    pub fn set_mode(&mut self, provider: ProviderId, mode: PlaybackMode) {
        self.playback.insert(provider, mode);
    }

    /// Whether offline mode is on.
    pub fn offline(&self) -> bool {
        self.offline
    }

    /// Set offline mode.
    pub fn set_offline(&mut self, enabled: bool) {
        self.offline = enabled;
    }
}

/// The shared playback settings and their file.
pub struct PlaybackSettingsStore {
    path: PathBuf,
    settings: RwLock<PlaybackSettings>,
}

impl PlaybackSettingsStore {
    /// Load the settings file. A missing file uses defaults. A corrupt
    /// file is moved aside and replaced by defaults.
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let settings = match fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<PlaybackSettings>(&text) {
                Ok(settings) => settings,
                Err(error) => {
                    log::warn!(
                        "could not parse {}: {error}; using defaults",
                        path.display()
                    );
                    let backup = path.with_extension("toml.bad");
                    let _ = fs::rename(&path, backup);
                    PlaybackSettings::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                PlaybackSettings::default()
            }
            Err(error) => {
                log::warn!("could not read {}: {error}; using defaults", path.display());
                PlaybackSettings::default()
            }
        };
        Self {
            path,
            settings: RwLock::new(settings),
        }
    }

    /// Create a store with explicit settings. Used by tests.
    pub fn new(path: impl Into<PathBuf>, settings: PlaybackSettings) -> Self {
        Self {
            path: path.into(),
            settings: RwLock::new(settings),
        }
    }

    /// The settings file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return the mode for a provider.
    pub fn mode(&self, provider: ProviderId) -> PlaybackMode {
        self.settings.read().unwrap().mode(provider)
    }

    /// Set and save the mode for a provider.
    pub fn set_mode(&self, provider: ProviderId, mode: PlaybackMode) -> std::io::Result<()> {
        let mut settings = self.settings.write().unwrap();
        settings.set_mode(provider, mode);
        save_settings(&self.path, &settings)
    }

    /// Whether offline mode is on.
    pub fn offline(&self) -> bool {
        self.settings.read().unwrap().offline()
    }

    /// Set and save offline mode.
    pub fn set_offline(&self, enabled: bool) -> std::io::Result<()> {
        let mut settings = self.settings.write().unwrap();
        settings.set_offline(enabled);
        save_settings(&self.path, &settings)
    }
}

fn save_settings(path: &Path, settings: &PlaybackSettings) -> std::io::Result<()> {
    let text = toml::to_string(settings).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("toml.tmp");
    fs::write(&temporary, text)?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_follow_each_provider() {
        let settings = PlaybackSettings::default();
        assert_eq!(settings.mode(ProviderId::Youtube), PlaybackMode::Download);
        assert_eq!(settings.mode(ProviderId::Rss), PlaybackMode::Download);
        assert_eq!(
            settings.mode(ProviderId::Podverse),
            PlaybackMode::StreamThenDownload
        );
        assert!(!settings.offline());
    }

    #[test]
    fn saves_and_reloads_offline_mode() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let store = PlaybackSettingsStore::load(&path);
        assert!(!store.offline());
        store.set_offline(true).unwrap();

        let reloaded = PlaybackSettingsStore::load(&path);
        assert!(reloaded.offline());
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("offline = true"), "{text}");
    }

    #[test]
    fn saves_and_reloads_a_mode() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let store = PlaybackSettingsStore::load(&path);
        store
            .set_mode(ProviderId::Rss, PlaybackMode::Stream)
            .unwrap();
        let reloaded = PlaybackSettingsStore::load(&path);
        assert_eq!(reloaded.mode(ProviderId::Rss), PlaybackMode::Stream);
        assert_eq!(
            reloaded.mode(ProviderId::Podverse),
            PlaybackMode::StreamThenDownload
        );
    }

    #[test]
    fn quarantines_a_corrupt_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "not = [valid").unwrap();

        let store = PlaybackSettingsStore::load(&path);

        assert_eq!(store.mode(ProviderId::Rss), PlaybackMode::Download);
        assert!(!path.exists());
        assert!(path.with_extension("toml.bad").exists());
    }

    #[test]
    fn missing_file_uses_defaults() {
        let dir = tempdir().unwrap();
        let store = PlaybackSettingsStore::load(dir.path().join("settings.toml"));
        assert_eq!(store.mode(ProviderId::Youtube), PlaybackMode::Download);
    }

    #[test]
    fn an_empty_file_uses_the_current_version() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "").unwrap();

        let store = PlaybackSettingsStore::load(&path);
        assert_eq!(store.settings.read().unwrap().version, SETTINGS_VERSION);
        assert_eq!(
            store.mode(ProviderId::Podverse),
            PlaybackMode::StreamThenDownload
        );
    }

    #[test]
    fn the_saved_file_names_the_provider_and_mode() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let store = PlaybackSettingsStore::load(&path);
        store
            .set_mode(ProviderId::Podverse, PlaybackMode::Stream)
            .unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("version = 1"), "{text}");
        assert!(text.contains("podverse = \"stream\""), "{text}");
        assert!(!path.with_extension("toml.tmp").exists());
    }
}
