use std::io::{Cursor, Read};

use common_structs::ProviderId;
use varuint::ReadVarint;
use ytermusicd_core::Track;

use crate::{DatabaseError, YTLocalDatabase};

/// Longest single string the reader accepts. A record holds short text
/// fields, so a larger length means the file is corrupt.
const MAX_STRING_LEN: u32 = 1_048_576;

/// Magic bytes at the start of a versioned database file.
/// Files without the magic are version 0: the original format with
/// five strings per record and no provider. The magic starts with
/// `Y`, which an old file cannot start with unless its first record
/// has a title of exactly 89 bytes starting with `TDB`; that case
/// reads as version 1 and fails with a clear error instead of
/// returning wrong data.
pub(crate) const MAGIC: &[u8; 4] = b"YTDB";
/// The only version the writer writes today.
pub(crate) const VERSION: u8 = 1;

/// The format version of a database file: `0` for the original
/// unversioned layout, `1` for the versioned layout. A missing file
/// has no version.
pub(crate) fn file_version(bytes: &[u8]) -> Result<u8, DatabaseError> {
    if bytes.len() < MAGIC.len() || &bytes[..MAGIC.len()] != MAGIC {
        return Ok(0);
    }
    if bytes.len() == MAGIC.len() {
        return Err(DatabaseError::InvalidData {
            message: "database file ends after the version header".to_string(),
        });
    }
    Ok(bytes[MAGIC.len()])
}

impl YTLocalDatabase {
    /// Read the library. `Ok(None)` means no database file exists yet.
    /// A corrupt file returns an error instead of an empty library, so the
    /// caller can tell a missing file from a damaged one.
    /// Version 0 files read each entry as `provider = "youtube"`.
    pub fn read(&self) -> Result<Option<Vec<Track>>, DatabaseError> {
        let path = self.cache_dir.join("db.bin");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(DatabaseError::Io { path, source });
            }
        };
        let version = file_version(&bytes)?;
        if version != 0 && version != VERSION {
            return Err(DatabaseError::InvalidData {
                message: format!(
                    "unsupported database version {version} in {path:?}; this build reads versions 0 and {VERSION}"
                ),
            });
        }
        let mut buffer = Cursor::new(bytes);
        if version == VERSION {
            buffer.set_position((MAGIC.len() + 1) as u64);
        }
        let mut videos = Vec::new();
        while buffer.get_mut().len() > buffer.position() as usize {
            videos.push(if version == VERSION {
                read_video_v1(&mut buffer)?
            } else {
                read_video_v0(&mut buffer)?
            });
        }
        Ok(Some(videos))
    }

    /// Read the library for a display lookup. Log any error and return an
    /// empty list, so a corrupt database shows an empty library instead of
    /// crashing the caller.
    pub fn read_or_empty(&self) -> Vec<Track> {
        match self.read() {
            Ok(videos) => videos.unwrap_or_default(),
            Err(error) => {
                log::error!("database read failed: {error}");
                Vec::new()
            }
        }
    }
}

/// Reads a version 0 video from the cursor: five strings, no provider.
/// The provider is always YouTube for these entries.
fn read_video_v0(buffer: &mut Cursor<Vec<u8>>) -> Result<Track, DatabaseError> {
    Ok(Track {
        title: read_str(buffer)?,
        author: read_str(buffer)?,
        album: read_str(buffer)?,
        track_id: read_str(buffer)?,
        duration: read_str(buffer)?,
        provider: ProviderId::Youtube,
    })
}

/// Reads a version 1 video from the cursor: the five version 0 strings
/// plus the provider name.
fn read_video_v1(buffer: &mut Cursor<Vec<u8>>) -> Result<Track, DatabaseError> {
    let video = Track {
        title: read_str(buffer)?,
        author: read_str(buffer)?,
        album: read_str(buffer)?,
        track_id: read_str(buffer)?,
        duration: read_str(buffer)?,
        provider: ProviderId::Youtube,
    };
    let provider = read_str(buffer)?;
    let provider = provider.parse().map_err(|_| DatabaseError::InvalidData {
        message: format!("unknown provider {provider:?} in database record"),
    })?;
    Ok(Track { provider, ..video })
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
