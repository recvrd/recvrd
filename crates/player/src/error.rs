// Custom Error Enum to handle different failures
#[derive(Debug)]
pub enum PlayError {
    Io(std::io::Error),
    DecoderError(rodio::decoder::DecoderError),
    SeekError(rodio::source::SeekError),
    /// A runtime error from the audio stream, for example an ALSA poll
    /// error after suspend. The stream that reported it is dead.
    StreamRuntimeError(String),
    /// The audio output could not start. The message names the backend and
    /// the cause.
    #[cfg(target_os = "linux")]
    AudioOutput(String),
    #[cfg(not(target_os = "linux"))]
    StreamError(rodio::StreamError),
    #[cfg(not(target_os = "linux"))]
    PlayError(rodio::PlayError),
}

#[cfg(not(target_os = "linux"))]
impl From<rodio::PlayError> for PlayError {
    fn from(err: rodio::PlayError) -> Self {
        PlayError::PlayError(err)
    }
}
