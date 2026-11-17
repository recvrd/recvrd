use std::path::PathBuf;
use thiserror::Error;

/// Errors from the local download database.
#[derive(Debug, Error)]
pub enum DatabaseError {
    /// A file operation failed. The path names the file that failed.
    #[error("database I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The database file holds data that cannot be read as records.
    #[error("database data is invalid: {message}")]
    InvalidData { message: String },
}
