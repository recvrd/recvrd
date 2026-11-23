use common_structs::ReadSeek;
use flume::Sender;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};

use std::fs::File;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{
    CurrentSource, PlayError, PlayerData, PlayerOptions, SEEK_STEP, StreamErrorLog,
    StreamErrorThrottle,
};

pub struct Player {
    sink: Sink,
    stream: OutputStream,
    data: PlayerData,
    error_sender: Sender<PlayError>,
    options: PlayerOptions,
}

impl Player {
    /// Build the error callback for one output stream.
    ///
    /// Rodio logs each stream error by default, so one dead ALSA stream
    /// can flood the log. This callback logs the first error, then it
    /// suppresses repeats, and it reports the failure to the audio thread.
    fn stream_error_callback(
        error_sender: Sender<PlayError>,
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

    fn try_from_device(
        device: rodio::cpal::Device,
        error_sender: &Sender<PlayError>,
    ) -> Result<OutputStream, PlayError> {
        // In rodio 0.21, try_from_device is available on OutputStream
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

    /// Create a stream from the default device or the PipeWire device.
    ///
    /// The daemon must not fall back to other devices. A fallback can
    /// seize a hardware device from PipeWire, as issue #205 shows.
    fn try_default(error_sender: &Sender<PlayError>) -> Result<OutputStream, PlayError> {
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

    pub fn new(error_sender: Sender<PlayError>, options: PlayerOptions) -> Result<Self, PlayError> {
        let stream = Self::try_default(&error_sender)?;

        // sink::try_new requires a reference to the handle
        let sink = Sink::connect_new(stream.mixer());

        sink.set_volume(options.initial_volume_f32());

        Ok(Self {
            sink,
            stream,
            error_sender,
            data: PlayerData::new(options.initial_volume()),
            options,
        })
    }

    pub fn update(&self) -> Result<Self, PlayError> {
        let stream = Self::try_default(&self.error_sender)?;
        let sink = Sink::connect_new(stream.mixer());

        sink.set_volume(self.data.volume_f32());

        Ok(Self {
            sink,
            stream,
            error_sender: self.error_sender.clone(),
            data: self.data.clone(),
            options: self.options.clone(),
        })
    }
    pub fn change_volume(&mut self, positive: bool) {
        self.data.change_volume(positive);
        self.sink.set_volume(self.data.volume_f32());
    }

    pub fn is_finished(&self) -> bool {
        self.sink.empty()
    }

    pub fn play_at(&mut self, path: &Path, time: Duration) -> Result<(), PlayError> {
        log::info!("Playing file: {:?} at time: {:?}", path, time);
        self.play(path)?;
        if let Err(e) = self.sink.try_seek(time) {
            log::error!("Seek error: {}", e);
            let _ = self.error_sender.send(PlayError::SeekError(e));
        }

        Ok(())
    }

    pub fn play(&mut self, path: &Path) -> Result<(), PlayError> {
        log::info!("Playing file: {:?}", path);
        self.data
            .set_current_source(Some(CurrentSource::File(path.to_path_buf())));

        self.stop();

        let file = File::open(path).map_err(PlayError::Io)?;

        if file.metadata().map(|m| m.len()).unwrap_or(0) == 0 {
            return Err(PlayError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "File is empty",
            )));
        }

        let decoder = Decoder::new(file).map_err(PlayError::DecoderError)?;

        self.data.set_total_duration(decoder.total_duration());

        // Check if sink is detached or empty and recreate if necessary
        if self.sink.empty() {
            // Using try_new with the stored handle
            self.sink = Sink::connect_new(self.stream.mixer());
        }

        self.sink.set_volume(self.data.volume_f32());
        self.sink.append(decoder);

        Ok(())
    }

    /// Play a reader with an optional byte length, MIME type and seek flag.
    pub fn play_reader(
        &mut self,
        reader: Box<dyn ReadSeek + Send + Sync>,
        byte_len: Option<u64>,
        mime: Option<&str>,
        seekable: bool,
    ) -> Result<(), PlayError> {
        // A stream replaces any file. Clear the file so a later seek
        // cannot replay the previous track.
        self.data.set_current_source(Some(CurrentSource::Stream));
        self.stop();
        let mut builder = Decoder::builder().with_data(reader);
        if let Some(byte_len) = byte_len {
            builder = builder.with_byte_len(byte_len);
        }
        if let Some(mime) = mime {
            builder = builder.with_mime_type(mime);
        }
        builder = builder.with_seekable(seekable);
        let decoder = builder.build().map_err(PlayError::DecoderError)?;
        self.data.set_total_duration(decoder.total_duration());
        if self.sink.empty() {
            self.sink = Sink::connect_new(self.stream.mixer());
        }
        self.sink.set_volume(self.data.volume_f32());
        self.sink.append(decoder);
        Ok(())
    }

    pub fn stop(&mut self) {
        // rodio 0.21: To stop, you can clear the sink.
        if !self.sink.empty() {
            self.sink.clear();
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.sink.get_pos()
    }

    pub fn duration(&self) -> Option<f64> {
        self.data
            .total_duration()
            .map(|duration| duration.as_secs_f64())
    }

    pub fn toggle_playback(&mut self) {
        if self.sink.is_paused() {
            self.sink.play();
        } else {
            self.sink.pause();
        }
    }

    pub fn seek_fw(&mut self) {
        let current_elapsed = self.elapsed();
        let new_pos = current_elapsed + SEEK_STEP;

        self.seek_to(new_pos);
    }

    pub fn seek_bw(&mut self) {
        let current_elapsed = self.elapsed();
        let new_pos = current_elapsed.saturating_sub(SEEK_STEP);
        self.seek_to(new_pos);
    }

    pub fn seek_to(&mut self, time: Duration) {
        if let Err(error) = self.try_seek_to(time) {
            log::error!("Seek error: {error:?}");
            let _ = self.error_sender.send(error);
        }
    }

    /// Seek and return the result to the caller.
    pub fn try_seek_to(&mut self, time: Duration) -> Result<(), PlayError> {
        log::info!("Seek to: {:?}", time);
        if self.is_finished() {
            return Ok(());
        }

        self.sink.try_seek(time).map_err(PlayError::SeekError)?;

        // If the sink is finished, we need to reset the music
        // This happens when the user seeks to the start of the song before the buffer.
        // Only a file source can restart this way. A stream has no file to replay.
        if self.is_finished()
            && let Some(file) = self.data.seek_replay_file()
        {
            log::info!("Sink is finished while seeking, resetting the music");
            self.play_at(&file, time)?;
        }
        Ok(())
    }

    pub fn percentage(&self) -> f64 {
        self.duration().map_or(0.0, |duration| {
            let elapsed = self.elapsed().as_secs_f64();
            elapsed / duration
        })
    }

    pub fn volume_percent(&self) -> u8 {
        self.data.volume()
    }

    pub fn volume(&self) -> i32 {
        self.data.volume().into()
    }

    pub fn volume_up(&mut self) {
        let volume = self.volume() + 5;
        self.set_volume(volume);
    }

    pub fn volume_down(&mut self) {
        let volume = self.volume() - 5;
        self.set_volume(volume);
    }

    pub fn set_volume(&mut self, mut volume: i32) {
        volume = volume.clamp(0, 100);
        self.data.set_volume(volume as u8);
        self.sink.set_volume((volume as f32) / 100.0);
    }

    pub fn pause(&mut self) {
        if !self.sink.is_paused() {
            self.toggle_playback();
        }
    }

    pub fn resume(&mut self) {
        if self.sink.is_paused() {
            self.toggle_playback();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.sink.is_paused()
    }

    pub fn seek(&mut self, secs: i64) {
        if secs.is_positive() {
            self.seek_fw();
        } else {
            self.seek_bw();
        }
    }

    pub fn get_progress(&self) -> (f64, u32, u32) {
        let position = self.elapsed();
        let duration = self.duration().unwrap_or(99.0) as u32;
        let percent = self.percentage() * 100.0;
        (percent.min(100.0), position.as_secs() as u32, duration)
    }
}
