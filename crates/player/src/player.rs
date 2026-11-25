use common_structs::ReadSeek;
use flume::Sender;
use rodio::{Decoder, Sink, Source};

use std::fs::File;
use std::path::Path;
use std::time::Duration;

use crate::{CurrentSource, PlayError, PlayerData, PlayerOptions, SEEK_STEP};

#[cfg(not(target_os = "linux"))]
use crate::cpal_output::CpalOutput as Output;
#[cfg(target_os = "linux")]
use crate::pipewire_output::PipeWireOutput as Output;

pub struct Player {
    sink: Sink,
    mixer: rodio::mixer::Mixer,
    output: Output,
    data: PlayerData,
    error_sender: Sender<PlayError>,
    options: PlayerOptions,
}

impl Player {
    pub fn new(error_sender: Sender<PlayError>, options: PlayerOptions) -> Result<Self, PlayError> {
        let (output, mixer, sink) = Output::create(error_sender.clone())?;

        sink.set_volume(options.initial_volume_f32());

        Ok(Self {
            sink,
            mixer,
            output,
            data: PlayerData::new(options.initial_volume()),
            error_sender,
            options,
        })
    }

    pub fn update(&self) -> Result<Self, PlayError> {
        let (output, mixer, sink) = Output::create(self.error_sender.clone())?;

        sink.set_volume(self.data.volume_f32());

        Ok(Self {
            sink,
            mixer,
            output,
            error_sender: self.error_sender.clone(),
            data: self.data.clone(),
            options: self.options.clone(),
        })
    }

    fn prepare_new_source(&mut self) {
        #[cfg(target_os = "linux")]
        {
            self.output.set_paused(false);
            self.output.flush();
            self.output.reset_position();
            self.output.reset_source_finished();
        }
    }

    pub fn change_volume(&mut self, positive: bool) {
        self.data.change_volume(positive);
        self.sink.set_volume(self.data.volume_f32());
    }

    pub fn is_finished(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.output.is_finished(self.sink.empty())
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.sink.empty()
        }
    }

    pub fn play_at(&mut self, path: &Path, time: Duration) -> Result<(), PlayError> {
        log::info!("Playing file: {:?} at time: {:?}", path, time);
        self.play(path)?;
        match self.sink.try_seek(time) {
            Ok(()) => {
                #[cfg(target_os = "linux")]
                {
                    self.output.flush();
                    self.output.set_position(time);
                }
            }
            Err(e) => {
                log::error!("Seek error: {}", e);
                let _ = self.error_sender.send(PlayError::SeekError(e));
            }
        }

        Ok(())
    }

    pub fn play(&mut self, path: &Path) -> Result<(), PlayError> {
        #[cfg(target_os = "linux")]
        if self.output.is_failed() {
            return Err(PlayError::StreamRuntimeError(
                "the PipeWire output failed".to_string(),
            ));
        }

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
        self.prepare_new_source();

        // A Sink that ran out of sources is removed from the mixer. Connect a
        // new Sink so the mixer receives the next source.
        if self.sink.empty() {
            self.sink = Sink::connect_new(&self.mixer);
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
        #[cfg(target_os = "linux")]
        if self.output.is_failed() {
            return Err(PlayError::StreamRuntimeError(
                "the PipeWire output failed".to_string(),
            ));
        }

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
        self.prepare_new_source();

        if self.sink.empty() {
            self.sink = Sink::connect_new(&self.mixer);
        }

        self.sink.set_volume(self.data.volume_f32());
        self.sink.append(decoder);

        Ok(())
    }

    pub fn stop(&mut self) {
        let failed = {
            #[cfg(target_os = "linux")]
            {
                self.output.is_failed()
            }
            #[cfg(not(target_os = "linux"))]
            {
                false
            }
        };

        if !self.sink.empty() {
            if failed {
                self.sink.stop();
            } else {
                self.sink.clear();
            }
        }

        #[cfg(target_os = "linux")]
        {
            self.output.set_paused(true);
            self.output.flush();
            self.output.reset_position();
        }
    }

    pub fn elapsed(&self) -> Duration {
        #[cfg(target_os = "linux")]
        {
            self.output
                .position()
                .unwrap_or_else(|| self.sink.get_pos())
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.sink.get_pos()
        }
    }

    pub fn duration(&self) -> Option<f64> {
        self.data
            .total_duration()
            .map(|duration| duration.as_secs_f64())
    }

    pub fn toggle_playback(&mut self) {
        if self.sink.is_paused() {
            self.resume();
        } else {
            self.pause();
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

        #[cfg(target_os = "linux")]
        if self.output.is_failed() {
            return Err(PlayError::StreamRuntimeError(
                "the PipeWire output failed".to_string(),
            ));
        }

        #[cfg(target_os = "linux")]
        {
            self.output.flush();
        }

        let result = self.sink.try_seek(time).map_err(PlayError::SeekError);
        result?;

        #[cfg(target_os = "linux")]
        {
            self.output.flush();
            self.output.set_position(time);
        }

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
            self.sink.pause();

            #[cfg(target_os = "linux")]
            {
                self.output.set_paused(true);
                self.output.flush();
            }
        }
    }

    pub fn resume(&mut self) {
        if self.sink.is_paused() {
            #[cfg(target_os = "linux")]
            self.output.flush();

            self.sink.play();

            #[cfg(target_os = "linux")]
            self.output.set_paused(false);
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
