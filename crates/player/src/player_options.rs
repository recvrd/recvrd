use common_structs::SampleFormat;

/// The default output sample rate.
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, Clone)]
pub struct PlayerOptions {
    initial_volume: u8,
    sample_rate: u32,
    format: SampleFormat,
}

impl PlayerOptions {
    /// Creates a new PlayerOptions with the given initial volume and
    /// the default 48 kHz float output.
    pub fn new(initial_volume: u8) -> Self {
        Self {
            initial_volume: initial_volume.min(100),
            sample_rate: DEFAULT_SAMPLE_RATE,
            format: SampleFormat::F32,
        }
    }

    /// Set the output sample rate and format.
    pub fn with_audio(mut self, sample_rate: u32, format: SampleFormat) -> Self {
        self.sample_rate = sample_rate;
        self.format = format;
        self
    }

    /// Returns the initial volume as a u8 between 0 and 100
    pub fn initial_volume(&self) -> u8 {
        self.initial_volume
    }

    /// Returns the initial volume as a f32 between 0.0 and 1.0
    pub fn initial_volume_f32(&self) -> f32 {
        f32::from(self.initial_volume()) / 100.0
    }

    /// The output sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The output sample format.
    pub fn format(&self) -> SampleFormat {
        self.format
    }
}
