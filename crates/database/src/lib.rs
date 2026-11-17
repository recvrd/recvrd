use std::{fs::OpenOptions, path::PathBuf, sync::RwLock};

mod error;
mod reader;
mod writer;

use common_structs::TrackKey;
pub use error::DatabaseError;
pub use writer::write_video;
use ytermusicd_core::Track;

pub struct YTLocalDatabase {
    cache_dir: PathBuf,
    references: RwLock<Vec<Track>>,
}

impl YTLocalDatabase {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            references: RwLock::new(Vec::new()),
        }
    }

    pub fn clone_from(&self, videos: &Vec<Track>) {
        self.references.write().unwrap().clone_from(videos);
    }

    pub fn remove_video(&self, key: &TrackKey) -> Result<(), DatabaseError> {
        let mut database = self.references.write().unwrap();
        database.retain(|video| video.key() != *key);
        drop(database);
        self.write()
    }

    /// Upgrade a version 0 file to version 1 in place, through the
    /// atomic write path. A missing file or a version 1 file is a
    /// no-op. An unknown version is an error, not a silent pass.
    /// Callers that append must call this first: a version 1
    /// record cannot be appended to a version 0 file. In-memory
    /// entries missing from the file are kept, not dropped.
    pub fn upgrade(&self) -> Result<(), DatabaseError> {
        use crate::reader::{VERSION, file_version};
        use std::collections::HashSet;

        let path = self.cache_dir.join("db.bin");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(DatabaseError::Io { path, source });
            }
        };
        let version = file_version(&bytes)?;
        if version == VERSION {
            return Ok(());
        }
        if version != 0 {
            return Err(DatabaseError::InvalidData {
                message: format!(
                    "unsupported database version {version} in {path:?}; this build reads versions 0 and {VERSION}"
                ),
            });
        }
        let Some(mut videos) = self.read()? else {
            return Ok(());
        };
        let known: HashSet<TrackKey> = videos.iter().map(|video| video.key()).collect();
        for video in self.references.read().unwrap().iter() {
            if !known.contains(&video.key()) {
                videos.push(video.clone());
            }
        }
        self.clone_from(&videos);
        self.write()
    }

    pub fn append(&self, video: Track) -> Result<(), DatabaseError> {
        use crate::reader::{MAGIC, VERSION};

        self.upgrade()?;
        let path = self.cache_dir.join("db.bin");
        if !path.exists() {
            // A new file starts with the version header, so the
            // appended record reads back as version 1.
            let mut header = Vec::new();
            header.extend_from_slice(MAGIC);
            header.push(VERSION);
            std::fs::write(&path, header).map_err(|source| DatabaseError::Io {
                path: path.clone(),
                source,
            })?;
        }
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .map_err(|source| DatabaseError::Io {
                path: path.clone(),
                source,
            })?;
        write_video(&mut file, &video).map_err(|source| DatabaseError::Io {
            path: path.clone(),
            source,
        })?;
        self.references.write().unwrap().push(video);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_structs::ProviderId;
    use tempfile::tempdir;
    use varuint::WriteVarint;

    fn video(id: &str) -> Track {
        Track {
            provider: ProviderId::Youtube,
            track_id: id.to_string(),
            title: format!("Title {id}"),
            author: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
        }
    }

    #[test]
    fn append_then_read_roundtrip() {
        let dir = tempdir().unwrap();
        let db = YTLocalDatabase::new(dir.path().to_path_buf());
        db.append(video("video-1")).unwrap();
        db.append(video("video-2")).unwrap();

        let videos = db.read().unwrap().unwrap();
        assert_eq!(videos.len(), 2);
        assert_eq!(videos[0].track_id, "video-1");
        assert_eq!(videos[1].title, "Title video-2");
    }

    #[test]
    fn append_returns_error_when_directory_is_missing() {
        let dir = tempdir().unwrap();
        let db = YTLocalDatabase::new(dir.path().join("no-such-dir"));
        let error = db.append(video("video-1")).unwrap_err();
        assert!(matches!(error, DatabaseError::Io { .. }), "{error}");
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = tempdir().unwrap();
        let db = YTLocalDatabase::new(dir.path().to_path_buf());
        db.clone_from(&vec![video("video-1"), video("video-2")]);
        db.write().unwrap();

        // No temporary file stays behind after a successful write.
        assert!(!dir.path().join("db.tmp").exists());

        let videos = db.read().unwrap().unwrap();
        assert_eq!(videos.len(), 2);
        assert_eq!(videos[1].track_id, "video-2");
    }

    #[test]
    fn remove_video_rewrites_the_file() {
        let dir = tempdir().unwrap();
        let db = YTLocalDatabase::new(dir.path().to_path_buf());
        db.append(video("video-1")).unwrap();
        db.append(video("video-2")).unwrap();

        db.remove_video(&video("video-1").key()).unwrap();

        let videos = db.read().unwrap().unwrap();
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0].track_id, "video-2");
    }

    #[test]
    fn read_missing_file_returns_none() {
        let dir = tempdir().unwrap();
        let db = YTLocalDatabase::new(dir.path().to_path_buf());
        assert_eq!(db.read().unwrap(), None);
    }

    #[test]
    fn read_rejects_huge_length_without_allocating_it() {
        let dir = tempdir().unwrap();
        let mut bytes = Vec::new();
        bytes.write_varint(u32::MAX).unwrap();
        bytes.extend_from_slice(b"short");
        std::fs::write(dir.path().join("db.bin"), bytes).unwrap();

        let db = YTLocalDatabase::new(dir.path().to_path_buf());
        let error = db.read().unwrap_err();
        assert!(
            matches!(error, DatabaseError::InvalidData { .. }),
            "{error}"
        );
    }

    #[test]
    fn read_returns_error_on_truncated_record() {
        let dir = tempdir().unwrap();
        let db = YTLocalDatabase::new(dir.path().to_path_buf());
        db.append(video("video-1")).unwrap();

        // A second record with a length prefix but no bytes behind it.
        let mut bytes = std::fs::read(dir.path().join("db.bin")).unwrap();
        bytes.write_varint(10u32).unwrap();
        bytes.extend_from_slice(b"abc");
        std::fs::write(dir.path().join("db.bin"), bytes).unwrap();

        let error = db.read().unwrap_err();
        assert!(
            matches!(error, DatabaseError::InvalidData { .. }),
            "{error}"
        );
    }

    #[test]
    fn failed_write_leaves_the_old_file_in_place() {
        let dir = tempdir().unwrap();
        let db = YTLocalDatabase::new(dir.path().to_path_buf());
        db.append(video("video-1")).unwrap();

        // Replace db.bin with a directory, so the rename cannot succeed.
        std::fs::remove_file(dir.path().join("db.bin")).unwrap();
        std::fs::create_dir(dir.path().join("db.bin")).unwrap();

        let error = db.write().unwrap_err();
        assert!(matches!(error, DatabaseError::Io { .. }), "{error}");
        assert!(dir.path().join("db.bin").is_dir());
    }

    fn write_raw_str(buffer: &mut Vec<u8>, value: &str) {
        buffer.write_varint(value.len() as u32).unwrap();
        buffer.extend_from_slice(value.as_bytes());
    }

    /// Hand-crafted version 0 bytes: five strings, no header.
    fn version_zero_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        for field in ["Title video-1", "Artist", "Album", "video-1", "3:00"] {
            write_raw_str(&mut bytes, field);
        }
        bytes
    }

    #[test]
    fn version_zero_file_reads_as_youtube() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("db.bin"), version_zero_bytes()).unwrap();

        let db = YTLocalDatabase::new(dir.path().to_path_buf());
        let videos = db.read().unwrap().unwrap();
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0].track_id, "video-1");
        assert_eq!(videos[0].provider, ProviderId::Youtube);
    }

    #[test]
    fn unknown_version_is_rejected_with_a_clear_error() {
        let dir = tempdir().unwrap();
        let mut bytes = b"YTDB".to_vec();
        bytes.push(9);
        std::fs::write(dir.path().join("db.bin"), bytes).unwrap();

        let db = YTLocalDatabase::new(dir.path().to_path_buf());
        let error = db.read().unwrap_err();
        assert!(
            matches!(error, DatabaseError::InvalidData { .. }),
            "{error}"
        );
        assert!(error.to_string().contains("version 9"), "{error}");
    }

    #[test]
    fn unknown_provider_is_rejected_with_a_clear_error() {
        let dir = tempdir().unwrap();
        let db = YTLocalDatabase::new(dir.path().to_path_buf());
        db.clone_from(&vec![video("video-1")]);
        db.write().unwrap();

        // Rewrite the provider string with an unknown name.
        let mut bytes = std::fs::read(dir.path().join("db.bin")).unwrap();
        let tail = b"youtube";
        let start = bytes.len() - tail.len();
        assert_eq!(&bytes[start..], tail);
        bytes[start..].copy_from_slice(b"tidal\x00\x00");
        // The length prefix still says 7, so the record now reads "tidal\0\0".
        std::fs::write(dir.path().join("db.bin"), bytes).unwrap();

        let error = db.read().unwrap_err();
        assert!(
            matches!(error, DatabaseError::InvalidData { .. }),
            "{error}"
        );
    }

    #[test]
    fn append_upgrades_a_version_zero_file() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("db.bin"), version_zero_bytes()).unwrap();

        let db = YTLocalDatabase::new(dir.path().to_path_buf());
        db.append(video("video-2")).unwrap();

        // The file now starts with the version 1 header.
        let bytes = std::fs::read(dir.path().join("db.bin")).unwrap();
        assert_eq!(&bytes[..4], b"YTDB");
        assert_eq!(bytes[4], 1);

        let videos = db.read().unwrap().unwrap();
        assert_eq!(videos.len(), 2);
        assert!(videos.iter().all(|v| v.provider == ProviderId::Youtube));
    }

    #[test]
    fn version_one_round_trip_keeps_the_provider() {
        let dir = tempdir().unwrap();
        let db = YTLocalDatabase::new(dir.path().to_path_buf());
        db.clone_from(&vec![video("video-1"), video("video-2")]);
        db.write().unwrap();

        let videos = db.read().unwrap().unwrap();
        assert_eq!(videos.len(), 2);
        assert_eq!(videos[0].provider, ProviderId::Youtube);
        assert_eq!(videos[1].track_id, "video-2");
    }
}
