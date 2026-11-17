use std::io::Write;

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
        for entry in std::fs::read_dir(&cache_folder).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            // Check if the file is a json file (+ sloppy check if there is any files or directory)
            if path.extension().unwrap_or_default() != "json" {
                continue;
            }
            // Read the file if not readable do not add it to the database
            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(e) => {
                    match std::fs::remove_file(&path) {
                        Ok(_) => println!(
                            "[INFO] Removing file {:?} because the file is not readable: {e:?}",
                            path.file_name()
                        ),
                        Err(ef) => println!(
                            "[ERROR] file {:?} is not readable: {e:?}, but could not be deleted: {ef:?}",
                            path.file_name()
                        ),
                    }
                    continue;
                }
            };
            // Check if the file is a valid json file
            let video = match serde_json::from_str::<YoutubeMusicVideoRef>(&content) {
                Ok(parsed) => parsed,
                Err(e) => {
                    match std::fs::remove_file(&path) {
                        Ok(_) => println!(
                            "[INFO] Removing file {:?} because the file is not a valid json file: {e:?}",
                            path.file_name()
                        ),
                        Err(ef) => println!(
                            "[ERROR] file {:?} is not a valid json file: {e:?}, but could not be deleted: {ef:?}",
                            path.file_name()
                        ),
                    }
                    continue;
                }
            };
            // Check if the video file exists
            let video_file = cache_folder.join(format!("{}.mp4", video.video_id));
            if !video_file.exists() {
                match std::fs::remove_file(&path) {
                    Ok(_) => println!(
                        "[INFO] Removing file {:?} because the video file does not exist",
                        path.file_name()
                    ),
                    Err(ef) => println!(
                        "[ERROR] video assocated to file {:?} does not exist, but the file could not be deleted: {ef:?}",
                        path.file_name()
                    ),
                }
                continue;
            }
            // Read the video file
            let video_file = match std::fs::read(&video_file) {
                Ok(video_file) => video_file,
                Err(e) => {
                    match std::fs::remove_file(&path) {
                        Ok(_) => println!(
                            "[INFO] Removing file {:?} because the video file is not readable: {e:?}",
                            path.file_name()
                        ),
                        Err(ef) => println!(
                            "[ERROR] video associated to file {:?} is not readable: {e:?}, but the file could not be deleted: {ef:?}",
                            path.file_name()
                        ),
                    }
                    continue;
                }
            };
            // Check if the video file contains the header
            if !video_file.starts_with(&[
                0, 0, 0, 24, 102, 116, 121, 112, 100, 97, 115, 104, 0, 0, 0, 0,
            ]) {
                match std::fs::remove_file(&path) {
                    Ok(_) => println!(
                        "[INFO] Removing file {:?} because the video file does not contain the header",
                        path.file_name()
                    ),
                    Err(ef) => println!(
                        "[ERROR] video associated to file {:?} does not contain the header, but the file could not be deleted: {ef:?}",
                        path.file_name()
                    ),
                }
                continue;
            }
            db.push(video);
        }
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
