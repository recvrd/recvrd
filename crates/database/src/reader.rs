use std::io::{Cursor, Read};

use varuint::ReadVarint;
use ytpapi2::YoutubeMusicVideoRef;

use crate::{DatabaseError, YTLocalDatabase};

/// Longest single string the reader accepts. A record holds short text
/// fields, so a larger length means the file is corrupt.
const MAX_STRING_LEN: u32 = 1_048_576;

impl YTLocalDatabase {
    /// Read the library. `Ok(None)` means no database file exists yet.
    /// A corrupt file returns an error instead of an empty library, so the
    /// caller can tell a missing file from a damaged one.
    pub fn read(&self) -> Result<Option<Vec<YoutubeMusicVideoRef>>, DatabaseError> {
        let path = self.cache_dir.join("db.bin");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(DatabaseError::Io { path, source });
            }
        };
        let mut buffer = Cursor::new(bytes);
        let mut videos = Vec::new();
        while buffer.get_mut().len() > buffer.position() as usize {
            videos.push(read_video(&mut buffer)?);
        }
        Ok(Some(videos))
    }

    /// Read the library for a display lookup. Log any error and return an
    /// empty list, so a corrupt database shows an empty library instead of
    /// crashing the caller.
    pub fn read_or_empty(&self) -> Vec<YoutubeMusicVideoRef> {
        match self.read() {
            Ok(videos) => videos.unwrap_or_default(),
            Err(error) => {
                log::error!("database read failed: {error}");
                Vec::new()
            }
        }
    }
}

/// Reads a video from the cursor
fn read_video(buffer: &mut Cursor<Vec<u8>>) -> Result<YoutubeMusicVideoRef, DatabaseError> {
    Ok(YoutubeMusicVideoRef {
        title: read_str(buffer)?,
        author: read_str(buffer)?,
        album: read_str(buffer)?,
        video_id: read_str(buffer)?,
        duration: read_str(buffer)?,
        set_video_id: None,
    })
}

/// Reads a string from the cursor
fn read_str(cursor: &mut Cursor<Vec<u8>>) -> Result<String, DatabaseError> {
    let len = read_u32(cursor)?;
    if len > MAX_STRING_LEN {
        return Err(DatabaseError::InvalidData {
            message: format!("string length {len} exceeds the {MAX_STRING_LEN} byte limit"),
        });
    }
    let mut buf = vec![0u8; len as usize];
    cursor
        .read_exact(&mut buf)
        .map_err(|_| DatabaseError::InvalidData {
            message: "truncated record".to_string(),
        })?;
    String::from_utf8(buf).map_err(|_| DatabaseError::InvalidData {
        message: "record is not valid UTF-8".to_string(),
    })
}

/// Reads a u32 from the cursor
fn read_u32(cursor: &mut Cursor<Vec<u8>>) -> Result<u32, DatabaseError> {
    ReadVarint::<u32>::read_varint(cursor).map_err(|_| DatabaseError::InvalidData {
        message: "truncated record".to_string(),
    })
}
