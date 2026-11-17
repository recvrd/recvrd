use std::{io::Write, path::Path};

use varuint::WriteVarint;
use ytpapi2::YoutubeMusicVideoRef;

use crate::{DatabaseError, YTLocalDatabase};

impl YTLocalDatabase {
    /// Write the whole library atomically. Records go into a buffer first,
    /// then into a temporary file in the same directory, then the temporary
    /// file replaces db.bin. A failed write leaves the old file in place.
    pub fn write(&self) -> Result<(), DatabaseError> {
        let path = self.cache_dir.join("db.bin");
        let db = self.references.read().unwrap();
        let mut buffer = Vec::new();
        for video in db.iter() {
            write_video(&mut buffer, video).map_err(|source| DatabaseError::Io {
                path: path.clone(),
                source,
            })?;
        }
        drop(db);

        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, buffer).map_err(|source| DatabaseError::Io {
            path: tmp.clone(),
            source,
        })?;
        if let Err(error) = std::fs::rename(&tmp, &path) {
            // Windows refuses to replace an existing file with rename.
            if path.exists() {
                std::fs::remove_file(&path).map_err(|source| DatabaseError::Io {
                    path: path.clone(),
                    source,
                })?;
                std::fs::rename(&tmp, &path).map_err(|source| DatabaseError::Io {
                    path: path.clone(),
                    source,
                })?;
            } else {
                return Err(DatabaseError::Io {
                    path: path.clone(),
                    source: error,
                });
            }
        }
        Ok(())
    }
}
impl YTLocalDatabase {
    pub fn fix_db(&self) {
        let mut db = self.references.write().unwrap();
        db.clear();
        let cache_folder = self.cache_dir.join("downloads");
        if !cache_folder.is_dir() {
            println!(
                "[WARN] The download folder in the cache wasn't found ({:?})",
                cache_folder
            );
            return;
        }
        let entries = match std::fs::read_dir(&cache_folder) {
            Ok(entries) => entries,
            Err(error) => {
                println!(
                    "[ERROR] Could not list the download folder ({cache_folder:?}): {error:?}"
                );
                return;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    println!("[ERROR] Could not read a download folder entry: {error:?}");
                    continue;
                }
            };
            let path = entry.path();
            // Check if the file is a json file (+ sloppy check if there is any files or directory)
            if path.extension().unwrap_or_default() != "json" {
                continue;
            }
            // Read the file if not readable do not add it to the database
            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) => {
                    quarantine_sidecar(
                        &self.cache_dir,
                        &path,
                        &format!("the file is not readable: {error:?}"),
                    );
                    continue;
                }
            };
            // Check if the file is a valid json file
            let video = match serde_json::from_str::<YoutubeMusicVideoRef>(&content) {
                Ok(parsed) => parsed,
                Err(error) => {
                    quarantine_sidecar(
                        &self.cache_dir,
                        &path,
                        &format!("the file is not a valid json file: {error:?}"),
                    );
                    continue;
                }
            };
            // Check if the video file exists
            let video_file = cache_folder.join(format!("{}.mp4", video.video_id));
            if !video_file.exists() {
                quarantine_sidecar(&self.cache_dir, &path, "the video file does not exist");
                continue;
            }
            // Check if the video file starts with an ftyp box
            match has_ftyp_header(&video_file) {
                Ok(true) => {}
                Ok(false) => {
                    quarantine_sidecar(
                        &self.cache_dir,
                        &path,
                        "the video file does not contain an ftyp header",
                    );
                    continue;
                }
                Err(error) => {
                    quarantine_sidecar(
                        &self.cache_dir,
                        &path,
                        &format!("the video file is not readable: {error:?}"),
                    );
                    continue;
                }
            }
            db.push(video);
        }
    }
}

/// Move a suspect sidecar file into the quarantine directory instead of
/// deleting it, so a repair mistake can be undone by hand. A file that
/// cannot be moved stays in place.
fn quarantine_sidecar(cache_dir: &Path, path: &Path, reason: &str) {
    let quarantine_dir = cache_dir.join("quarantine");
    if let Err(error) = std::fs::create_dir_all(&quarantine_dir) {
        println!(
            "[ERROR] Could not create the quarantine directory ({quarantine_dir:?}): {error:?}; keep {path:?}"
        );
        return;
    }
    let Some(name) = path.file_name() else {
        println!("[ERROR] Could not quarantine {path:?}: no file name");
        return;
    };
    let target = quarantine_dir.join(name);
    if target.exists() {
        println!("[WARN] Quarantine file {target:?} already exists; keep {path:?}");
        return;
    }
    match std::fs::rename(path, &target) {
        Ok(()) => println!("[INFO] Quarantined {:?} because {reason}", path.file_name()),
        Err(error) => println!(
            "[ERROR] Could not quarantine {:?}: {error:?}",
            path.file_name()
        ),
    }
}

/// Check whether a file starts with a generic `ftyp` box. Any `ftyp`
/// brand counts: DASH, M4A, and other MP4 variants all carry it. Only the
/// first 8 bytes are read. A file shorter than 8 bytes has no header.
fn has_ftyp_header(path: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut header = [0u8; 8];
    match file.read_exact(&mut header) {
        Ok(()) => Ok(&header[4..8] == b"ftyp"),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

/// Writes a video to a file
pub fn write_video(buffer: &mut impl Write, video: &YoutubeMusicVideoRef) -> std::io::Result<()> {
    write_str(buffer, &video.title)?;
    write_str(buffer, &video.author)?;
    write_str(buffer, &video.album)?;
    write_str(buffer, &video.video_id)?;
    write_str(buffer, &video.duration)
}

/// Writes a string from the cursor
fn write_str(cursor: &mut impl Write, value: &str) -> std::io::Result<()> {
    write_u32(cursor, value.len() as u32)?;
    cursor.write_all(value.as_bytes())
}

/// Writes a u32 from the cursor
fn write_u32(cursor: &mut impl Write, value: u32) -> std::io::Result<()> {
    cursor.write_varint(value).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    const M4A_HEADER: &[u8] = &[
        0, 0, 0, 32, b'f', b't', b'y', b'p', b'M', b'4', b'A', b' ', 0, 0, 0, 0,
    ];
    const DASH_HEADER: &[u8] = &[
        0, 0, 0, 24, b'f', b't', b'y', b'p', b'd', b'a', b's', b'h', 0, 0, 0, 0,
    ];

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

    fn downloads_dir(cache_dir: &std::path::Path) -> PathBuf {
        let downloads = cache_dir.join("downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        downloads
    }

    fn write_sidecar(downloads: &std::path::Path, video: &YoutubeMusicVideoRef, header: &[u8]) {
        std::fs::write(
            downloads.join(format!("{}.json", video.video_id)),
            serde_json::to_string(video).unwrap(),
        )
        .unwrap();
        std::fs::write(downloads.join(format!("{}.mp4", video.video_id)), header).unwrap();
    }

    fn library(cache_dir: &std::path::Path) -> Vec<YoutubeMusicVideoRef> {
        let db = YTLocalDatabase::new(cache_dir.to_path_buf());
        db.fix_db();
        db.write().unwrap();
        db.read().unwrap().unwrap()
    }

    #[test]
    fn fix_db_keeps_m4a_and_dash_downloads() {
        let dir = tempdir().unwrap();
        let downloads = downloads_dir(dir.path());
        write_sidecar(&downloads, &video("video-m4a"), M4A_HEADER);
        write_sidecar(&downloads, &video("video-dash"), DASH_HEADER);

        let videos = library(dir.path());
        assert_eq!(videos.len(), 2);
        assert!(videos.iter().any(|v| v.video_id == "video-m4a"));
        assert!(videos.iter().any(|v| v.video_id == "video-dash"));
    }

    #[test]
    fn fix_db_quarantines_bad_header_instead_of_deleting() {
        let dir = tempdir().unwrap();
        let downloads = downloads_dir(dir.path());
        write_sidecar(&downloads, &video("video-bad"), b"not a video file at all");

        let videos = library(dir.path());
        assert!(videos.is_empty());
        assert!(!downloads.join("video-bad.json").exists());
        assert!(downloads.join("video-bad.mp4").exists());
        assert!(
            dir.path()
                .join("quarantine")
                .join("video-bad.json")
                .exists()
        );
    }

    #[test]
    fn fix_db_quarantines_invalid_json() {
        let dir = tempdir().unwrap();
        let downloads = downloads_dir(dir.path());
        std::fs::write(downloads.join("video-bad.json"), "{not json").unwrap();

        let videos = library(dir.path());
        assert!(videos.is_empty());
        assert!(!downloads.join("video-bad.json").exists());
        assert!(
            dir.path()
                .join("quarantine")
                .join("video-bad.json")
                .exists()
        );
    }

    #[test]
    fn fix_db_quarantines_missing_video_file() {
        let dir = tempdir().unwrap();
        let downloads = downloads_dir(dir.path());
        std::fs::write(
            downloads.join("video-gone.json"),
            serde_json::to_string(&video("video-gone")).unwrap(),
        )
        .unwrap();

        let videos = library(dir.path());
        assert!(videos.is_empty());
        assert!(!downloads.join("video-gone.json").exists());
        assert!(
            dir.path()
                .join("quarantine")
                .join("video-gone.json")
                .exists()
        );
    }

    #[test]
    fn fix_db_quarantines_short_file() {
        let dir = tempdir().unwrap();
        let downloads = downloads_dir(dir.path());
        write_sidecar(&downloads, &video("video-short"), b"tiny");

        let videos = library(dir.path());
        assert!(videos.is_empty());
        assert!(
            dir.path()
                .join("quarantine")
                .join("video-short.json")
                .exists()
        );
    }

    #[test]
    fn ftyp_header_accepts_any_brand() {
        let dir = tempdir().unwrap();
        let m4a = dir.path().join("m4a.mp4");
        std::fs::write(&m4a, M4A_HEADER).unwrap();
        assert!(has_ftyp_header(&m4a).unwrap());

        let dash = dir.path().join("dash.mp4");
        std::fs::write(&dash, DASH_HEADER).unwrap();
        assert!(has_ftyp_header(&dash).unwrap());

        let garbage = dir.path().join("garbage.mp4");
        std::fs::write(&garbage, b"not a video file at all").unwrap();
        assert!(!has_ftyp_header(&garbage).unwrap());

        let short = dir.path().join("short.mp4");
        std::fs::write(&short, b"tiny").unwrap();
        assert!(!has_ftyp_header(&short).unwrap());
    }
}
