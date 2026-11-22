use directories::ProjectDirs;
use std::path::{Path, PathBuf};
use thiserror::Error;

const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "suorcd";
const APPLICATION: &str = "recvrd";
const PREVIOUS_APPLICATION: &str = "ytermusicd";
const LEGACY_ORGANIZATION: &str = "ccgauche";
const LEGACY_APPLICATION: &str = "ytermusic";

#[derive(Debug, Error)]
pub enum PathsError {
    #[error("could not determine the user's home directory")]
    NoHome,
}

/// Resolved filesystem locations for the daemon and its legacy client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
    pub previous_config_dir: PathBuf,
    pub previous_cache_dir: PathBuf,
    pub previous_data_dir: PathBuf,
    pub previous_state_dir: PathBuf,
    pub legacy_config_dir: PathBuf,
    pub legacy_cache_dir: PathBuf,
    pub legacy_data_dir: PathBuf,
    pub legacy_state_dir: PathBuf,
}

impl Paths {
    /// Discover the platform directories for `recvrd`, the previous
    /// `ytermusicd` install and the legacy TUI.
    pub fn discover() -> Result<Self, PathsError> {
        let current =
            ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION).ok_or(PathsError::NoHome)?;
        let previous = ProjectDirs::from(QUALIFIER, ORGANIZATION, PREVIOUS_APPLICATION)
            .ok_or(PathsError::NoHome)?;
        let legacy = ProjectDirs::from(QUALIFIER, LEGACY_ORGANIZATION, LEGACY_APPLICATION)
            .ok_or(PathsError::NoHome)?;

        Ok(Self {
            config_dir: current.config_dir().to_path_buf(),
            cache_dir: current.cache_dir().to_path_buf(),
            data_dir: current.data_dir().to_path_buf(),
            state_dir: current
                .state_dir()
                .unwrap_or_else(|| current.data_local_dir())
                .to_path_buf(),
            previous_config_dir: previous.config_dir().to_path_buf(),
            previous_cache_dir: previous.cache_dir().to_path_buf(),
            previous_data_dir: previous.data_dir().to_path_buf(),
            previous_state_dir: previous
                .state_dir()
                .unwrap_or_else(|| previous.data_local_dir())
                .to_path_buf(),
            legacy_config_dir: legacy.config_dir().to_path_buf(),
            legacy_cache_dir: legacy.cache_dir().to_path_buf(),
            legacy_data_dir: legacy.data_dir().to_path_buf(),
            legacy_state_dir: legacy
                .state_dir()
                .unwrap_or_else(|| legacy.data_local_dir())
                .to_path_buf(),
        })
    }

    /// Create the directories the daemon writes to. Move the previous
    /// `ytermusicd` directories first, so the daemon does not start
    /// with an empty state when an old install exists.
    pub fn ensure(&self) -> std::io::Result<()> {
        self.migrate()?;
        for dir in [
            &self.config_dir,
            &self.cache_dir,
            &self.data_dir,
            &self.state_dir,
            &self.downloads_dir(),
            &self.covers_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Move or copy the previous `ytermusicd` directories to the new
    /// `recvrd` paths. A cache directory that cannot be moved is left
    /// alone, because the daemon can download the files again.
    fn migrate(&self) -> std::io::Result<()> {
        self.migrate_dir(&self.previous_config_dir, &self.config_dir, true)?;
        self.migrate_dir(&self.previous_data_dir, &self.data_dir, true)?;
        self.migrate_dir(&self.previous_state_dir, &self.state_dir, true)?;
        self.migrate_dir(&self.previous_cache_dir, &self.cache_dir, false)
    }

    fn migrate_dir(&self, previous: &Path, current: &Path, copy: bool) -> std::io::Result<()> {
        let metadata = match std::fs::symlink_metadata(previous) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if !metadata.is_dir() || previous == current {
            return Ok(());
        }
        if current.exists() {
            if !current.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("{} is not a directory", current.display()),
                ));
            }
            if !is_empty_dir(current)? {
                log::info!(
                    "keeping {}; the previous directory {} is not empty",
                    current.display(),
                    previous.display()
                );
                return Ok(());
            }
            std::fs::remove_dir(current)?;
        }
        if let Some(parent) = current.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::rename(previous, current) {
            Ok(()) => {
                log::info!("moved {} to {}", previous.display(), current.display());
                return Ok(());
            }
            Err(error) => {
                log::warn!(
                    "could not move {} to {}: {error}",
                    previous.display(),
                    current.display()
                );
            }
        }
        #[cfg(unix)]
        if std::os::unix::fs::symlink(previous, current).is_ok() {
            log::info!("linked {} to {}", current.display(), previous.display());
            return Ok(());
        }
        if copy {
            copy_dir(previous, current)?;
            log::info!("copied {} to {}", previous.display(), current.display());
            return Ok(());
        }
        log::warn!(
            "left {} in place; the daemon can download the cache again",
            previous.display()
        );
        Ok(())
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn headers_file(&self) -> PathBuf {
        self.config_dir.join("headers.txt")
    }

    pub fn legacy_headers_file(&self) -> PathBuf {
        self.legacy_config_dir.join("headers.txt")
    }

    pub fn account_id_file_near(headers: &Path) -> PathBuf {
        headers
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("account_id.txt")
    }

    pub fn downloads_dir(&self) -> PathBuf {
        self.cache_dir.join("downloads")
    }

    pub fn covers_dir(&self) -> PathBuf {
        self.cache_dir.join("covers")
    }

    pub fn database_file(&self) -> PathBuf {
        self.cache_dir.join("db.bin")
    }

    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }

    /// The RSS provider store: feed URLs, ETags and the episode
    /// index. Lives in the data directory, not the cache, because
    /// feed URLs are user data.
    pub fn rss_feeds_file(&self) -> PathBuf {
        self.data_dir.join("rss-feeds.json")
    }

    pub fn log_file(&self) -> PathBuf {
        self.state_dir.join("recvrd.log")
    }

    /// Project directories used by the legacy TUI, kept for migrations.
    pub fn legacy_project_dirs() -> Option<ProjectDirs> {
        ProjectDirs::from(QUALIFIER, LEGACY_ORGANIZATION, LEGACY_APPLICATION)
    }
}

fn is_empty_dir(path: &Path) -> std::io::Result<bool> {
    Ok(std::fs::read_dir(path)?.next().is_none())
}

fn copy_dir(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target_path)?;
        } else {
            std::fs::copy(entry.path(), target_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_paths(root: &Path) -> Paths {
        Paths {
            config_dir: root.join("config"),
            cache_dir: root.join("cache"),
            data_dir: root.join("data"),
            state_dir: root.join("state"),
            previous_config_dir: root.join("old-config"),
            previous_cache_dir: root.join("old-cache"),
            previous_data_dir: root.join("old-data"),
            previous_state_dir: root.join("old-state"),
            legacy_config_dir: root.join("legacy-config"),
            legacy_cache_dir: root.join("legacy-cache"),
            legacy_data_dir: root.join("legacy-data"),
            legacy_state_dir: root.join("legacy-state"),
        }
    }

    #[test]
    fn file_paths_are_derived_from_roots() {
        let paths = Paths {
            config_dir: PathBuf::from("/config"),
            cache_dir: PathBuf::from("/cache"),
            data_dir: PathBuf::from("/data"),
            state_dir: PathBuf::from("/state"),
            previous_config_dir: PathBuf::from("/old-config"),
            previous_cache_dir: PathBuf::from("/old-cache"),
            previous_data_dir: PathBuf::from("/old-data"),
            previous_state_dir: PathBuf::from("/old-state"),
            legacy_config_dir: PathBuf::from("/legacy-config"),
            legacy_cache_dir: PathBuf::from("/legacy-cache"),
            legacy_data_dir: PathBuf::from("/legacy-data"),
            legacy_state_dir: PathBuf::from("/legacy-state"),
        };

        assert_eq!(paths.config_file(), PathBuf::from("/config/config.toml"));
        assert_eq!(paths.headers_file(), PathBuf::from("/config/headers.txt"));
        assert_eq!(
            paths.legacy_headers_file(),
            PathBuf::from("/legacy-config/headers.txt")
        );
        assert_eq!(paths.downloads_dir(), PathBuf::from("/cache/downloads"));
        assert_eq!(paths.covers_dir(), PathBuf::from("/cache/covers"));
        assert_eq!(paths.database_file(), PathBuf::from("/cache/db.bin"));
        assert_eq!(paths.state_file(), PathBuf::from("/state/state.json"));
        assert_eq!(
            paths.rss_feeds_file(),
            PathBuf::from("/data/rss-feeds.json")
        );
        assert_eq!(
            Paths::account_id_file_near(Path::new("/config/headers.txt")),
            PathBuf::from("/config/account_id.txt")
        );
    }

    #[test]
    fn moves_an_existing_install_to_the_new_paths() {
        let root = tempdir().unwrap();
        let paths = test_paths(root.path());
        std::fs::create_dir_all(&paths.previous_config_dir).unwrap();
        std::fs::write(paths.previous_config_dir.join("config.toml"), "old").unwrap();
        std::fs::create_dir_all(paths.previous_cache_dir.join("downloads")).unwrap();
        std::fs::write(
            paths.previous_cache_dir.join("downloads/track.mp4"),
            "audio",
        )
        .unwrap();

        paths.ensure().unwrap();

        assert_eq!(
            std::fs::read_to_string(paths.config_dir.join("config.toml")).unwrap(),
            "old"
        );
        assert!(paths.cache_dir.join("downloads/track.mp4").is_file());
        assert!(!paths.previous_config_dir.exists());
        assert!(!paths.previous_cache_dir.exists());
    }

    #[test]
    fn replaces_an_empty_new_directory() {
        let root = tempdir().unwrap();
        let paths = test_paths(root.path());
        std::fs::create_dir_all(&paths.previous_config_dir).unwrap();
        std::fs::write(paths.previous_config_dir.join("config.toml"), "old").unwrap();
        std::fs::create_dir_all(&paths.config_dir).unwrap();

        paths.ensure().unwrap();

        assert_eq!(
            std::fs::read_to_string(paths.config_dir.join("config.toml")).unwrap(),
            "old"
        );
        assert!(!paths.previous_config_dir.exists());
    }

    #[test]
    fn keeps_a_new_directory_that_is_not_empty() {
        let root = tempdir().unwrap();
        let paths = test_paths(root.path());
        std::fs::create_dir_all(&paths.previous_config_dir).unwrap();
        std::fs::write(paths.previous_config_dir.join("config.toml"), "old").unwrap();
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::write(paths.config_dir.join("config.toml"), "new").unwrap();

        paths.ensure().unwrap();

        assert_eq!(
            std::fs::read_to_string(paths.config_dir.join("config.toml")).unwrap(),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(paths.previous_config_dir.join("config.toml")).unwrap(),
            "old"
        );
    }

    #[test]
    fn creates_the_new_directories_without_an_old_install() {
        let root = tempdir().unwrap();
        let paths = test_paths(root.path());

        paths.ensure().unwrap();

        assert!(paths.config_dir.is_dir());
        assert!(paths.cache_dir.is_dir());
        assert!(paths.data_dir.is_dir());
        assert!(paths.state_dir.is_dir());
        assert!(!paths.previous_config_dir.exists());
    }

    #[test]
    fn copies_a_directory_tree() {
        let root = tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("nested/file"), "data").unwrap();

        copy_dir(&source, &target).unwrap();

        assert_eq!(
            std::fs::read_to_string(target.join("nested/file")).unwrap(),
            "data"
        );
    }
}
