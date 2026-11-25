use std::sync::{Arc, Mutex};
use std::time::Instant;

use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{OutputStream, OutputStreamBuilder, Sink};

use crate::{PlayError, StreamErrorLog, StreamErrorThrottle};

/// The macOS and Windows output. It uses the cpal backend from rodio.
pub(crate) struct CpalOutput {
    _stream: OutputStream,
}

impl CpalOutput {
    pub(crate) fn create(
        error_sender: flume::Sender<PlayError>,
        _sample_rate: u32,
        _format: common_structs::SampleFormat,
    ) -> Result<(Self, rodio::mixer::Mixer, Sink), PlayError> {
        let stream = Self::open(&error_sender)?;
        let mixer = stream.mixer().clone();
        let sink = Sink::connect_new(&mixer);

        Ok((Self { _stream: stream }, mixer, sink))
    }

    /// Build the error callback for one output stream.
    ///
    /// Rodio logs each stream error by default, so one dead ALSA stream
    /// can flood the log. This callback logs the first error, then it
    /// suppresses repeats, and it reports the failure to the audio thread.
    fn stream_error_callback(
        error_sender: flume::Sender<PlayError>,
    ) -> impl FnMut(rodio::cpal::StreamError) + Send + 'static {
        let throttle = Arc::new(Mutex::new(StreamErrorThrottle::new()));
        move |error| {
            let message = error.to_string();
            let decision = throttle.lock().map_or(StreamErrorLog::LogNow, |mut guard| {
                guard.record(&message, Instant::now())
            });
            match decision {
                StreamErrorLog::LogNow => log::warn!("audio stream error: {message}"),
                StreamErrorLog::LogSummary { suppressed } => {
                    log::warn!("audio stream error: {message} ({suppressed} repeats suppressed)")
                }
                StreamErrorLog::Suppress => {}
            }
            if !matches!(decision, StreamErrorLog::Suppress) {
                let _ = error_sender.send(PlayError::StreamRuntimeError(message));
            }
        }
    }

    fn open(error_sender: &flume::Sender<PlayError>) -> Result<OutputStream, PlayError> {
        // Use rodio's internal cpal re-export
        let host = rodio::cpal::default_host();

        let mut candidates = Vec::new();
        if let Some(device) = host.default_output_device() {
            candidates.push(device);
        }
        if let Ok(devices) = host.output_devices() {
            for device in devices {
                if Self::device_name(&device) == "pipewire"
                    && !candidates
                        .iter()
                        .any(|candidate| Self::device_name(candidate) == "pipewire")
                {
                    candidates.push(device);
                }
            }
        }
        let mut first_error = None;
        for device in candidates {
            let name = Self::device_name(&device);
            match Self::try_from_device(device, error_sender) {
                Ok(stream) => {
                    log::info!("audio output device: {name}");
                    return Ok(stream);
                }
                Err(error) => {
                    log::warn!("audio output device {name} failed: {error:?}");
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        Err(first_error.unwrap_or(PlayError::StreamError(rodio::StreamError::NoDevice)))
    }

    fn try_from_device(
        device: rodio::cpal::Device,
        error_sender: &flume::Sender<PlayError>,
    ) -> Result<OutputStream, PlayError> {
        OutputStreamBuilder::default()
            .with_device(device)
            .with_error_callback(Self::stream_error_callback(error_sender.clone()))
            .open_stream()
            .map_err(PlayError::StreamError)
    }

    /// Name one output device for the log, or "unknown" when it has none.
    fn device_name(device: &rodio::cpal::Device) -> String {
        device.name().unwrap_or_else(|_| "unknown".to_string())
    }
}
