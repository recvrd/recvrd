//! Audio output format shared by the config and the player.

use serde::{Deserialize, Serialize};

/// A sample format for the audio output.
///
/// F32 is 32-bit float. S32 and S24 are packed integer formats; S24 is
/// not more precise than F32.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleFormat {
    #[default]
    F32,
    S32,
    S24,
}

impl SampleFormat {
    /// The lowercase name used in config and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::S32 => "s32",
            Self::S24 => "s24",
        }
    }
}
