mod error;

use std::time::Duration;

pub use error::PlayError;

mod player;
pub use player::Player;

#[cfg(not(target_os = "linux"))]
mod cpal_output;
#[cfg(target_os = "linux")]
mod pipewire_output;

mod player_options;
pub use player_options::PlayerOptions;

mod probe;
pub use probe::{AudioFacts, probe_audio};

mod visualizer;
pub use visualizer::{VisualizerFrame, VisualizerGuard, visualizer_frame};

mod player_data;
pub(crate) use player_data::{CurrentSource, PlayerData};

mod stream_error;
pub(crate) use stream_error::{StreamErrorLog, StreamErrorThrottle};

pub(crate) static VOLUME_STEP: u8 = 5;
pub(crate) static SEEK_STEP: Duration = Duration::from_secs(5);
