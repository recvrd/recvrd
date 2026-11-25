use std::io::{Read, Seek};

/// A blocking reader that supports seek. Used for stream playback.
pub trait ReadSeek: Read + Seek {}

impl<T: Read + Seek> ReadSeek for T {}

mod app_status;
mod audio;
mod music_download_status;
mod playback;
mod provider;
mod track;

pub use app_status::AppStatus;
pub use audio::SampleFormat;
pub use music_download_status::MusicDownloadStatus;
pub use playback::{PlaybackMode, PlaybackSourceKind};
pub use provider::{AUDIO_EXTENSIONS, ProviderId, TrackKey, UnknownProvider};
pub use track::{MediaKind, TrackMetadata, TrackSource};
