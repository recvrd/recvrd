use std::{fs::OpenOptions, path::PathBuf, sync::RwLock};

mod error;
mod reader;
mod writer;

pub use error::DatabaseError;
pub use writer::write_video;
use ytpapi2::YoutubeMusicVideoRef;

pub struct YTLocalDatabase {
    cache_dir: PathBuf,
    references: RwLock<Vec<YoutubeMusicVideoRef>>,
}

impl YTLocalDatabase {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            references: RwLock::new(Vec::new()),
        }
    }

    pub fn clone_from(&self, videos: &Vec<YoutubeMusicVideoRef>) {
        self.references.write().unwrap().clone_from(videos);
    }

    pub fn remove_video(&self, video: &YoutubeMusicVideoRef) -> Result<(), DatabaseError> {
        let mut database = self.references.write().unwrap();
        database.retain(|v| v.video_id != video.video_id);
        drop(database);
        self.write()
    }

    pub fn append(&self, video: YoutubeMusicVideoRef) -> Result<(), DatabaseError> {
        let path = self.cache_dir.join("db.bin");
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
    use tempfile::tempdir;
    use varuint::WriteVarint;

    fn video(id: &str) -> YoutubeMusicVideoRef {
        YoutubeMusicVideoRef {
            title: format!("Title {id}"),
            author: "Artist".to_string(),
            album: "Album".to_string(),
            video_id: id.to_string(),
            duration: "3:00".to_string(),
            set_video_id: None,
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
        assert_eq!(videos[0].video_id, "video-1");
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
        assert_eq!(videos[1].video_id, "video-2");
    }

    #[test]
    fn remove_video_rewrites_the_file() {
        let dir = tempdir().unwrap();
        let db = YTLocalDatabase::new(dir.path().to_path_buf());
        db.append(video("video-1")).unwrap();
        db.append(video("video-2")).unwrap();

        db.remove_video(&video("video-1")).unwrap();

        let videos = db.read().unwrap().unwrap();
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0].video_id, "video-2");
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
}
