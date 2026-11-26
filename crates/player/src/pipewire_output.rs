use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::pod::Pod;
use pw::types::ObjectType;
use rodio::Sink;
use rodio::mixer::{Mixer, MixerSource};
use rtrb::{Consumer, Producer, PushError, RingBuffer};

use crate::{PlayError, StreamErrorLog, StreamErrorThrottle, visualizer};
use common_structs::SampleFormat;

/// The output channels. The mixer converts every source to stereo.
const CHANNELS: usize = 2;
const RING_SECONDS: usize = 1;
const FLUSH_TIMEOUT: Duration = Duration::from_secs(1);

/// The bytes in one sample of the output format.
fn bytes_per_sample(format: SampleFormat) -> usize {
    match format {
        SampleFormat::F32 | SampleFormat::S32 => 4,
        SampleFormat::S24 => 3,
    }
}

/// The bytes in one frame of the output format.
fn frame_bytes(format: SampleFormat) -> usize {
    CHANNELS * bytes_per_sample(format)
}

/// Map a sample format to the PipeWire format.
fn spa_format(format: SampleFormat) -> spa::param::audio::AudioFormat {
    match format {
        SampleFormat::F32 => spa::param::audio::AudioFormat::F32LE,
        SampleFormat::S32 => spa::param::audio::AudioFormat::S32LE,
        SampleFormat::S24 => spa::param::audio::AudioFormat::S24LE,
    }
}

/// Write one clamped float sample in the output format.
fn write_sample(format: SampleFormat, sample: f32, out: &mut [u8]) {
    match format {
        SampleFormat::F32 => out.copy_from_slice(&sample.to_le_bytes()),
        SampleFormat::S32 => {
            let value = (sample * i32::MAX as f32).round() as i32;
            out.copy_from_slice(&value.to_le_bytes());
        }
        SampleFormat::S24 => {
            let value = (sample * 8_388_607.0).round() as i32;
            out.copy_from_slice(&value.to_le_bytes()[..3]);
        }
    }
}

/// The last default sink name. A rebuilt output reuses it and skips the
/// metadata wait at startup.
static KNOWN_SINK: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn known_sink() -> &'static Mutex<Option<String>> {
    KNOWN_SINK.get_or_init(|| Mutex::new(None))
}

struct Shared {
    paused: AtomicBool,
    failed: AtomicBool,
    shutdown: AtomicBool,
    /// The sink has no track source. The decoder thread stops filling
    /// the ring buffer with mixer silence after this flag is set.
    source_finished: AtomicBool,
    sink_name: Mutex<Option<String>>,
    /// The output sample rate in Hz.
    sample_rate: u32,
    /// The output sample format.
    format: SampleFormat,
    produced: AtomicU64,
    consumed: AtomicU64,
    played_frames: AtomicU64,
    base_frames: AtomicU64,
    flush_epoch: AtomicU64,
    flush_ack: AtomicU64,
}

impl Shared {
    fn new(sample_rate: u32, format: SampleFormat) -> Self {
        Self {
            paused: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            source_finished: AtomicBool::new(false),
            sink_name: Mutex::new(
                known_sink()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone(),
            ),
            sample_rate,
            format,
            produced: AtomicU64::new(0),
            consumed: AtomicU64::new(0),
            played_frames: AtomicU64::new(0),
            base_frames: AtomicU64::new(0),
            flush_epoch: AtomicU64::new(0),
            flush_ack: AtomicU64::new(0),
        }
    }

    fn buffered_samples(&self) -> u64 {
        self.produced
            .load(Ordering::Acquire)
            .saturating_sub(self.consumed.load(Ordering::Acquire))
    }
}

/// The Linux output. A decoder thread fills a ring buffer, and a PipeWire
/// thread drains it in the realtime process callback.
pub(crate) struct PipeWireOutput {
    shared: Arc<Shared>,
    quit: Option<pw::channel::Sender<()>>,
}

impl PipeWireOutput {
    pub(crate) fn create(
        error_sender: flume::Sender<PlayError>,
        sample_rate: u32,
        format: SampleFormat,
    ) -> Result<(Self, Mixer, Sink), PlayError> {
        let (mixer, source) = rodio::mixer::mixer(CHANNELS as u16, sample_rate);
        let sink = Sink::connect_new(&mixer);
        let output = Self::new(source, error_sender, sample_rate, format)?;

        Ok((output, mixer, sink))
    }

    fn new(
        source: MixerSource,
        error_sender: flume::Sender<PlayError>,
        sample_rate: u32,
        format: SampleFormat,
    ) -> Result<Self, PlayError> {
        let shared = Arc::new(Shared::new(sample_rate, format));
        let (quit, quit_rx) = pw::channel::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);

        let thread_shared = shared.clone();
        let thread = thread::Builder::new()
            .name("recvrd-pipewire".to_string())
            .spawn(move || run(source, thread_shared, quit_rx, ready_tx, error_sender))
            .map_err(|error| {
                PlayError::AudioOutput(format!("could not start the PipeWire thread: {error}"))
            })?;

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {
                drop(thread);
                Ok(Self {
                    shared,
                    quit: Some(quit),
                })
            }
            Ok(Err(message)) => {
                let _ = quit.send(());
                drop(thread);
                Err(PlayError::AudioOutput(message))
            }
            Err(error) => {
                let _ = quit.send(());
                drop(thread);
                Err(PlayError::AudioOutput(format!(
                    "the PipeWire output did not start: {error}"
                )))
            }
        }
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Release);
    }

    /// Discard the samples that the decoder buffered. This function waits for
    /// the process callback to acknowledge the discard.
    pub(crate) fn flush(&self) {
        if self.shared.failed.load(Ordering::Acquire) {
            return;
        }

        let epoch = self.shared.flush_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let deadline = Instant::now() + FLUSH_TIMEOUT;

        while self.shared.flush_ack.load(Ordering::Acquire) != epoch {
            if self.shared.failed.load(Ordering::Acquire) {
                return;
            }
            if Instant::now() >= deadline {
                log::warn!("the PipeWire output did not flush the buffer in time");
                return;
            }

            thread::sleep(Duration::from_millis(1));
        }
    }

    pub(crate) fn set_position(&self, position: Duration) {
        self.shared
            .base_frames
            .store(frames(position, self.shared.sample_rate), Ordering::Release);
        self.shared.played_frames.store(0, Ordering::Release);
    }

    pub(crate) fn reset_position(&self) {
        self.shared.base_frames.store(0, Ordering::Release);
        self.shared.played_frames.store(0, Ordering::Release);
    }

    /// Allow the decoder thread to fill the ring buffer for a new source.
    pub(crate) fn reset_source_finished(&self) {
        self.shared.source_finished.store(false, Ordering::Release);
    }

    pub(crate) fn position(&self) -> Option<Duration> {
        let base = self.shared.base_frames.load(Ordering::Acquire);
        let played = self.shared.played_frames.load(Ordering::Acquire);

        Some(Duration::from_secs_f64(
            (base + played) as f64 / f64::from(self.shared.sample_rate),
        ))
    }

    pub(crate) fn is_finished(&self, sink_empty: bool) -> bool {
        if sink_empty {
            // The mixer source keeps yielding silence after the track
            // ends. Stop the decoder thread so the ring buffer can
            // drain the track tail and report the finish.
            self.shared.source_finished.store(true, Ordering::Release);
        }
        // One sample can stay in the callback's pending slot when the
        // total sample count is odd. The track is still finished.
        sink_empty
            && (self.shared.buffered_samples() <= 1 || self.shared.failed.load(Ordering::Acquire))
    }

    pub(crate) fn is_failed(&self) -> bool {
        self.shared.failed.load(Ordering::Acquire)
    }
}

impl Drop for PipeWireOutput {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        if let Some(quit) = self.quit.take() {
            let _ = quit.send(());
        }
    }
}

struct StreamData {
    shared: Arc<Shared>,
    error_sender: flume::Sender<PlayError>,
    error_throttle: Arc<Mutex<StreamErrorThrottle>>,
    consumer: Consumer<f32>,
    visualizer: Option<Producer<f32>>,
    flush: u64,
    pending: Option<f32>,
    scratch: Vec<f32>,
}

impl StreamData {
    fn drain(&mut self) {
        let mut popped = 0;

        while self.consumer.pop().is_ok() {
            popped += 1;
        }

        // The pending sample was popped from the ring earlier and not
        // counted as consumed yet. Count it now that it is discarded.
        if self.pending.take().is_some() {
            popped += 1;
        }

        if popped > 0 {
            self.shared.consumed.fetch_add(popped, Ordering::Release);
        }
    }
}

fn run(
    source: MixerSource,
    shared: Arc<Shared>,
    quit: pw::channel::Receiver<()>,
    ready: SyncSender<Result<(), String>>,
    error_sender: flume::Sender<PlayError>,
) {
    pw::init();

    let mainloop = match pw::main_loop::MainLoopRc::new(None) {
        Ok(mainloop) => mainloop,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "could not create the PipeWire main loop: {error}"
            )));
            return;
        }
    };
    let context = match pw::context::ContextRc::new(&mainloop, None) {
        Ok(context) => context,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "could not create the PipeWire context: {error}"
            )));
            return;
        }
    };
    let core = match context.connect_rc(None) {
        Ok(core) => core,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "could not connect to PipeWire: {error}. Start the PipeWire server."
            )));
            return;
        }
    };

    // Log the default sink name from the PipeWire metadata. The stream uses
    // AUTOCONNECT, so it follows the default sink.
    let registry = match core.get_registry_rc() {
        Ok(registry) => registry,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "could not read the PipeWire registry: {error}"
            )));
            return;
        }
    };
    let registry_weak = registry.downgrade();
    let metadata: Rc<RefCell<Vec<(pw::metadata::Metadata, pw::metadata::MetadataListener)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let metadata = metadata.clone();
            let sink_name = shared.clone();
            move |object| {
                if object.type_ != ObjectType::Metadata {
                    return;
                }

                let is_default = object
                    .props
                    .and_then(|properties| properties.get("metadata.name"))
                    .is_some_and(|name| name == "default");
                if !is_default {
                    return;
                }

                let Some(registry) = registry_weak.upgrade() else {
                    return;
                };
                let Ok(proxy) = registry.bind::<pw::metadata::Metadata, _>(object) else {
                    return;
                };
                let listener = proxy
                    .add_listener_local()
                    .property({
                        let sink_name = sink_name.clone();
                        move |_, key, _, value| {
                            if key == Some("default.audio.sink")
                                && let Some(name) = value.and_then(sink_name_from_json)
                            {
                                log::info!("the default PipeWire sink is {name}");
                                *sink_name
                                    .sink_name
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                    Some(name.clone());
                                *known_sink()
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(name);
                            }
                            0
                        }
                    })
                    .register();

                metadata.borrow_mut().push((proxy, listener));
            }
        })
        .register();

    // Process the initial registry and metadata events before the stream
    // connects, so the connect log can name the default sink.
    let deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < deadline
        && shared
            .sink_name
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none()
    {
        mainloop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(Duration::from_millis(10)));
    }

    let ring_samples = shared.sample_rate as usize * CHANNELS * RING_SECONDS;
    let (producer, consumer) = RingBuffer::new(ring_samples);
    let (visualizer_producer, visualizer_consumer) = RingBuffer::new(16_384);
    visualizer::install(visualizer::start(visualizer_consumer, shared.sample_rate));
    let error_throttle = Arc::new(Mutex::new(StreamErrorThrottle::new()));
    let stream_data = StreamData {
        shared: shared.clone(),
        error_sender,
        error_throttle,
        consumer,
        visualizer: Some(visualizer_producer),
        flush: 0,
        pending: None,
        scratch: vec![0.0; ring_samples],
    };

    let stream = match pw::stream::StreamBox::new(
        &core,
        "recvrd",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::AUDIO_CHANNELS => "2",
            *pw::keys::NODE_NAME => "recvrd",
        },
    ) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "could not create the PipeWire stream: {error}"
            )));
            return;
        }
    };

    let listener = match stream
        .add_local_listener_with_user_data(stream_data)
        .state_changed(|_, data, old, new| {
            if is_connection_lost(
                &old,
                &new,
                data.shared.shutdown.load(Ordering::Acquire),
                data.shared.failed.load(Ordering::Acquire),
            ) {
                let message = "the PipeWire connection was lost";
                report_stream_failure(
                    data,
                    message,
                    &format!("the PipeWire stream changed from {old:?} to {new:?}: {message}"),
                );
                return;
            }

            match new {
                pw::stream::StreamState::Error(message) => {
                    report_stream_failure(
                        data,
                        &message,
                        &format!("the PipeWire stream changed from {old:?} to Error: {message}"),
                    );
                }
                pw::stream::StreamState::Paused | pw::stream::StreamState::Streaming => {
                    let sink = data
                        .shared
                        .sink_name
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone();
                    match sink {
                        Some(name) => {
                            log::info!("the PipeWire output is connected to {name}")
                        }
                        None => {
                            log::info!("the PipeWire output is connected to the default sink")
                        }
                    }
                }
                _ => log::info!("the PipeWire stream changed from {old:?} to {new:?}"),
            }
        })
        .process(fill)
        .register()
    {
        Ok(listener) => listener,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "could not register the PipeWire stream callbacks: {error}"
            )));
            return;
        }
    };

    let _quit = quit.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });

    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa_format(shared.format));
    audio_info.set_rate(shared.sample_rate);
    audio_info.set_channels(CHANNELS as u32);

    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    audio_info.set_position(position);

    let values = match spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::sys::SPA_TYPE_OBJECT_Format,
            id: spa::sys::SPA_PARAM_EnumFormat,
            properties: audio_info.into(),
        }),
    ) {
        Ok(values) => values.0.into_inner(),
        Err(error) => {
            let _ = ready.send(Err(format!(
                "could not serialize the PipeWire audio format: {error}"
            )));
            return;
        }
    };

    let pod = match Pod::from_bytes(&values) {
        Some(pod) => pod,
        None => {
            let _ = ready.send(Err("could not read the PipeWire audio format".to_string()));
            return;
        }
    };
    let mut params = [pod];

    if let Err(error) = stream.connect(
        spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    ) {
        let _ = ready.send(Err(format!(
            "could not connect the PipeWire output stream: {error}"
        )));
        return;
    }

    let decoder = match spawn_decoder(source, shared.clone(), producer) {
        Ok(decoder) => decoder,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "could not start the audio decoder thread: {error}"
            )));
            return;
        }
    };

    log::info!(
        "the PipeWire audio output is connected: {} Hz, {CHANNELS} channels, {}",
        shared.sample_rate,
        shared.format.as_str()
    );

    if ready.send(Ok(())).is_err() {
        shared.shutdown.store(true, Ordering::Release);
        return;
    }

    mainloop.run();
    shared.shutdown.store(true, Ordering::Release);
    drop(decoder);
    drop(listener);
    drop(stream);
}

/// True when a stream that had started connecting lost its PipeWire
/// connection. The initial `Unconnected -> Connecting` transition and the
/// shutdown/drop path are not failures.
fn is_connection_lost(
    old: &pw::stream::StreamState,
    new: &pw::stream::StreamState,
    shutdown: bool,
    failed: bool,
) -> bool {
    !shutdown
        && !failed
        && matches!(new, pw::stream::StreamState::Unconnected)
        && matches!(
            old,
            pw::stream::StreamState::Connecting
                | pw::stream::StreamState::Paused
                | pw::stream::StreamState::Streaming
        )
}

/// Mark the output failed and send a runtime error to the audio thread,
/// throttling repeated log lines and error sends.
fn report_stream_failure(data: &mut StreamData, message: &str, log_message: &str) {
    data.shared.failed.store(true, Ordering::Release);
    let decision = data
        .error_throttle
        .lock()
        .map_or(StreamErrorLog::LogNow, |mut throttle| {
            throttle.record(message, Instant::now())
        });
    match decision {
        StreamErrorLog::LogNow => log::error!("{log_message}"),
        StreamErrorLog::LogSummary { suppressed } => {
            log::error!("{log_message} ({suppressed} repeats suppressed)")
        }
        StreamErrorLog::Suppress => {}
    }
    if !matches!(decision, StreamErrorLog::Suppress) {
        let _ = data
            .error_sender
            .send(PlayError::StreamRuntimeError(message.to_string()));
    }
}

fn fill(stream: &pw::stream::Stream, data: &mut StreamData) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let datas = buffer.datas_mut();
    if datas.is_empty() {
        return;
    }

    let out = &mut datas[0];
    let Some(slice) = out.data() else {
        return;
    };
    let frame_bytes = frame_bytes(data.shared.format);
    if slice.len() < frame_bytes {
        return;
    }

    let (paused, flushed, written_samples) = write_samples(data, slice);
    let size = (slice.len() / frame_bytes) * frame_bytes;

    if !paused && !flushed {
        let written_frames = written_samples / CHANNELS;
        data.shared
            .played_frames
            .fetch_add(written_frames as u64, Ordering::Release);
    }

    let chunk = out.chunk_mut();
    *chunk.offset_mut() = 0;
    *chunk.stride_mut() = frame_bytes as i32;
    *chunk.size_mut() = size as u32;
}

/// Write one PipeWire buffer. Return whether the output is paused, whether
/// this buffer followed a flush, and the number of samples written.
fn write_samples(data: &mut StreamData, slice: &mut [u8]) -> (bool, bool, usize) {
    let format = data.shared.format;
    let sample_bytes = bytes_per_sample(format);
    let frame_bytes = frame_bytes(format);
    let max_frames = slice.len() / frame_bytes;
    let max_samples = (max_frames * CHANNELS).min(data.scratch.len());
    let paused = data.shared.paused.load(Ordering::Acquire);
    let epoch = data.shared.flush_epoch.load(Ordering::Acquire);
    let flushed = epoch != data.flush;

    if flushed {
        data.drain();
        data.flush = epoch;
        data.shared.flush_ack.store(epoch, Ordering::Release);
    }

    let written_samples = if paused {
        data.drain();
        0
    } else {
        let mut written = 0;

        if let Some(sample) = data.pending.take() {
            data.scratch[0] = sample;
            written = 1;
        }

        let (popped, _) = data
            .consumer
            .pop_partial_slice(&mut data.scratch[written..max_samples]);
        written += popped.len();

        if written % CHANNELS != 0 {
            data.pending = Some(data.scratch[written - 1]);
            written -= 1;
        }

        if written > 0 {
            data.shared
                .consumed
                .fetch_add(written as u64, Ordering::Release);
        }

        written
    };

    let frames_to_write = written_samples / CHANNELS;

    if let Some(visualizer) = &mut data.visualizer {
        for frame in 0..frames_to_write {
            let left = data.scratch[frame * CHANNELS].clamp(-1.0, 1.0);
            let right = data.scratch[frame * CHANNELS + 1].clamp(-1.0, 1.0);
            let _ = visualizer.push((left + right) * 0.5);
        }
    }

    for frame in 0..max_frames {
        let offset = frame * frame_bytes;

        if frame < frames_to_write {
            for channel in 0..CHANNELS {
                let sample = data.scratch[frame * CHANNELS + channel].clamp(-1.0, 1.0);
                let start = offset + channel * sample_bytes;
                write_sample(format, sample, &mut slice[start..start + sample_bytes]);
            }
        } else {
            slice[offset..offset + frame_bytes].fill(0);
        }
    }

    (paused, flushed, written_samples)
}

fn spawn_decoder(
    mut source: MixerSource,
    shared: Arc<Shared>,
    mut producer: Producer<f32>,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("recvrd-decode".to_string())
        .spawn(move || {
            let mut flush = 0;

            loop {
                if shared.shutdown.load(Ordering::Acquire) {
                    return;
                }

                if shared.failed.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }

                let epoch = shared.flush_epoch.load(Ordering::Acquire);
                if epoch != flush {
                    while shared.flush_ack.load(Ordering::Acquire) != epoch {
                        if shared.shutdown.load(Ordering::Acquire) {
                            return;
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                    flush = epoch;
                    continue;
                }

                if shared.source_finished.load(Ordering::Acquire) {
                    // The track source ended. Do not fill the ring with
                    // mixer silence; let the buffered tail drain.
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }

                let Some(sample) = source.next() else {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                };
                let mut sample = sample;

                loop {
                    shared.produced.fetch_add(1, Ordering::Release);
                    if shared.failed.load(Ordering::Acquire) {
                        shared.produced.fetch_sub(1, Ordering::Release);
                        break;
                    }
                    match producer.push(sample) {
                        Ok(()) => break,
                        Err(PushError::Full(value)) => {
                            shared.produced.fetch_sub(1, Ordering::Release);
                            sample = value;
                            if shared.shutdown.load(Ordering::Acquire) {
                                return;
                            }
                            thread::sleep(Duration::from_millis(1));
                        }
                    }
                }
            }
        })
}

fn frames(position: Duration, sample_rate: u32) -> u64 {
    (position.as_secs_f64() * f64::from(sample_rate)).round() as u64
}

/// Read the sink name from the PipeWire default sink metadata value.
fn sink_name_from_json(value: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(value).ok()?;

    value.get("name")?.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_samples_converts_to_s32() {
        let (mut producer, mut data) = stream_data_with(SampleFormat::S32);
        producer.push(0.5).unwrap();
        producer.push(-0.5).unwrap();
        let mut slice = vec![0; frame_bytes(SampleFormat::S32)];

        assert_eq!(write_samples(&mut data, &mut slice), (false, false, 2));

        assert_eq!(sample_s32(&slice, 0), 1_073_741_824);
        assert_eq!(sample_s32(&slice, 1), -1_073_741_824);
    }

    #[test]
    fn write_samples_converts_to_s24() {
        let (mut producer, mut data) = stream_data_with(SampleFormat::S24);
        producer.push(0.5).unwrap();
        producer.push(-0.5).unwrap();
        let mut slice = vec![0; frame_bytes(SampleFormat::S24)];

        assert_eq!(write_samples(&mut data, &mut slice), (false, false, 2));

        assert_eq!(sample_s24(&slice, 0), 4_194_304);
        assert_eq!(sample_s24(&slice, 1), -4_194_304);
    }

    #[test]
    fn position_uses_the_configured_sample_rate() {
        let output = shared_output_with(96_000);
        output.set_position(Duration::from_secs(1));
        output.shared.played_frames.store(96_000, Ordering::Release);

        assert_eq!(output.position(), Some(Duration::from_secs(2)));
    }

    #[test]
    fn position_uses_the_base_and_the_played_frames() {
        let output = shared_output();
        output.set_position(Duration::from_secs(2));
        output.shared.played_frames.store(48_000, Ordering::Release);

        assert_eq!(output.position(), Some(Duration::from_secs(3)));
    }

    #[test]
    fn reset_position_clears_the_base_and_the_played_frames() {
        let output = shared_output();
        output.set_position(Duration::from_secs(2));
        output.reset_position();

        assert_eq!(output.position(), Some(Duration::ZERO));
    }

    #[test]
    fn an_output_with_no_buffered_samples_is_finished() {
        let output = shared_output();
        output.shared.produced.store(10, Ordering::Release);
        output.shared.consumed.store(10, Ordering::Release);

        assert_eq!(output.shared.buffered_samples(), 0);
        assert!(output.is_finished(true));
    }

    #[test]
    fn an_output_with_buffered_samples_is_not_finished() {
        let output = shared_output();
        output.shared.produced.store(10, Ordering::Release);
        output.shared.consumed.store(4, Ordering::Release);

        assert_eq!(output.shared.buffered_samples(), 6);
        assert!(!output.is_finished(true));
    }

    #[test]
    fn an_empty_sink_with_one_pending_sample_is_finished() {
        let output = shared_output();
        output.shared.produced.store(11, Ordering::Release);
        output.shared.consumed.store(10, Ordering::Release);

        assert_eq!(output.shared.buffered_samples(), 1);
        assert!(output.is_finished(true));
    }

    #[test]
    fn draining_counts_the_pending_sample() {
        let (mut producer, mut data) = stream_data();
        data.shared.produced.store(3, Ordering::Release);
        for sample in [0.25, -0.5, 0.75] {
            producer.push(sample).unwrap();
        }
        let mut slice = vec![0; 2 * frame_bytes(SampleFormat::F32)];
        write_samples(&mut data, &mut slice);

        assert_eq!(data.shared.buffered_samples(), 1);
        assert_eq!(data.pending, Some(0.75));

        data.drain();

        assert_eq!(data.shared.buffered_samples(), 0);
        assert_eq!(data.pending, None);
    }

    #[test]
    fn an_empty_sink_marks_the_source_finished() {
        let output = shared_output();
        output.shared.produced.store(10, Ordering::Release);
        output.shared.consumed.store(4, Ordering::Release);

        assert!(!output.is_finished(true));
        assert!(output.shared.source_finished.load(Ordering::Acquire));
    }

    #[test]
    fn a_nonempty_sink_keeps_the_source_running() {
        let output = shared_output();
        output.shared.produced.store(10, Ordering::Release);
        output.shared.consumed.store(4, Ordering::Release);

        assert!(!output.is_finished(false));
        assert!(!output.shared.source_finished.load(Ordering::Acquire));
    }

    #[test]
    fn reset_source_finished_clears_the_flag() {
        let output = shared_output();
        output.shared.source_finished.store(true, Ordering::Release);

        output.reset_source_finished();

        assert!(!output.shared.source_finished.load(Ordering::Acquire));
    }

    #[test]
    fn a_failed_output_finishes_with_an_empty_sink() {
        let output = shared_output();
        output.shared.produced.store(10, Ordering::Release);
        output.shared.consumed.store(4, Ordering::Release);
        output.shared.failed.store(true, Ordering::Release);

        assert!(output.is_finished(true));
    }

    #[test]
    fn a_full_frame_has_one_frame_of_samples() {
        assert_eq!(frame_bytes(SampleFormat::F32), CHANNELS * 4);
    }

    #[test]
    fn write_samples_writes_interleaved_frames_and_clamps() {
        let (mut producer, mut data) = stream_data();
        for sample in [0.25, -0.5, 0.75, 1.5] {
            producer.push(sample).unwrap();
        }
        let mut slice = vec![0; 2 * frame_bytes(SampleFormat::F32)];

        assert_eq!(write_samples(&mut data, &mut slice), (false, false, 4));

        assert_eq!(sample_at(&slice, 0), 0.25);
        assert_eq!(sample_at(&slice, 1), -0.5);
        assert_eq!(sample_at(&slice, 2), 0.75);
        assert_eq!(sample_at(&slice, 3), 1.0);
        assert_eq!(data.shared.consumed.load(Ordering::Acquire), 4);
    }

    #[test]
    fn write_samples_fills_silence_when_the_buffer_is_empty() {
        let (_producer, mut data) = stream_data();
        let mut slice = vec![1; 2 * frame_bytes(SampleFormat::F32)];

        assert_eq!(write_samples(&mut data, &mut slice), (false, false, 0));

        assert!(slice.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn write_samples_discards_samples_while_paused() {
        let (mut producer, mut data) = stream_data();
        producer.push(0.5).unwrap();
        producer.push(-0.5).unwrap();
        data.shared.paused.store(true, Ordering::Release);
        let mut slice = vec![1; frame_bytes(SampleFormat::F32)];

        let (paused, _, written) = write_samples(&mut data, &mut slice);

        assert!(paused);
        assert_eq!(written, 0);

        assert!(slice.iter().all(|byte| *byte == 0));
        assert_eq!(data.shared.consumed.load(Ordering::Acquire), 2);
    }

    #[test]
    fn write_samples_acknowledges_a_flush() {
        let (mut producer, mut data) = stream_data();
        producer.push(0.5).unwrap();
        producer.push(-0.5).unwrap();
        data.shared.flush_epoch.store(1, Ordering::Release);
        let mut slice = vec![0; frame_bytes(SampleFormat::F32)];

        let (_, flushed, written) = write_samples(&mut data, &mut slice);

        assert!(flushed);
        assert_eq!(written, 0);
        assert_eq!(data.flush, 1);
        assert_eq!(data.shared.flush_ack.load(Ordering::Acquire), 1);
        assert_eq!(data.shared.consumed.load(Ordering::Acquire), 2);
        assert!(slice.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn write_samples_holds_an_odd_sample_for_the_next_buffer() {
        let (mut producer, mut data) = stream_data();
        for sample in [0.25, -0.5, 0.75] {
            producer.push(sample).unwrap();
        }
        let mut slice = vec![0; 2 * frame_bytes(SampleFormat::F32)];

        let (_, _, written) = write_samples(&mut data, &mut slice);

        assert_eq!(written, 2);
        assert_eq!(data.pending, Some(0.75));
        assert_eq!(sample_at(&slice, 0), 0.25);
        assert_eq!(sample_at(&slice, 1), -0.5);
        assert_eq!(sample_at(&slice, 2), 0.0);
        assert_eq!(sample_at(&slice, 3), 0.0);
    }

    #[test]
    fn write_samples_reports_only_the_frames_from_the_ring() {
        let (mut producer, mut data) = stream_data();
        producer.push(0.25).unwrap();
        producer.push(-0.5).unwrap();
        let mut slice = vec![0; 4 * frame_bytes(SampleFormat::F32)];

        let (_, _, written) = write_samples(&mut data, &mut slice);

        assert_eq!(written, 2);
    }

    #[test]
    fn sink_name_from_json_reads_a_name() {
        let value = r#"{"name":"alsa_output.pci-0000_00_1f.3.analog-stereo","monitor":false}"#;

        assert_eq!(
            sink_name_from_json(value),
            Some("alsa_output.pci-0000_00_1f.3.analog-stereo".to_string())
        );
    }

    #[test]
    fn sink_name_from_json_accepts_spacing() {
        let value = r#"{"name": "scarlett-2i2", "monitor": false}"#;

        assert_eq!(sink_name_from_json(value), Some("scarlett-2i2".to_string()));
    }

    #[test]
    fn sink_name_from_json_handles_an_escaped_quote() {
        let value = r#"{"name":"scarlett \"2i2\"","monitor":false}"#;

        assert_eq!(
            sink_name_from_json(value),
            Some("scarlett \"2i2\"".to_string())
        );
    }

    #[test]
    fn sink_name_from_json_rejects_a_value_without_a_name() {
        assert_eq!(sink_name_from_json(r#"{"monitor":false}"#), None);
    }

    #[test]
    fn losing_the_connection_after_connecting_is_a_failure() {
        for old in [
            pw::stream::StreamState::Connecting,
            pw::stream::StreamState::Paused,
            pw::stream::StreamState::Streaming,
        ] {
            assert!(is_connection_lost(
                &old,
                &pw::stream::StreamState::Unconnected,
                false,
                false
            ));
        }
    }

    #[test]
    fn the_initial_unconnected_state_is_not_a_failure() {
        assert!(!is_connection_lost(
            &pw::stream::StreamState::Unconnected,
            &pw::stream::StreamState::Connecting,
            false,
            false
        ));
    }

    #[test]
    fn a_shutdown_or_existing_failure_does_not_report_the_connection_again() {
        assert!(!is_connection_lost(
            &pw::stream::StreamState::Streaming,
            &pw::stream::StreamState::Unconnected,
            true,
            false
        ));
        assert!(!is_connection_lost(
            &pw::stream::StreamState::Streaming,
            &pw::stream::StreamState::Unconnected,
            false,
            true
        ));
    }

    #[test]
    fn a_connection_loss_marks_the_output_failed_and_reports_it() {
        let (_producer, consumer) = RingBuffer::new(16);
        let (error_sender, error_receiver) = flume::unbounded();
        let mut data = StreamData {
            shared: Arc::new(Shared::new(48_000, SampleFormat::F32)),
            error_sender,
            error_throttle: Arc::new(Mutex::new(StreamErrorThrottle::new())),
            consumer,
            visualizer: None,
            flush: 0,
            pending: None,
            scratch: vec![0.0; 16],
        };

        report_stream_failure(
            &mut data,
            "the PipeWire connection was lost",
            "the PipeWire stream changed to Unconnected",
        );

        assert!(data.shared.failed.load(Ordering::Acquire));
        match error_receiver.try_recv() {
            Ok(PlayError::StreamRuntimeError(message)) => {
                assert_eq!(message, "the PipeWire connection was lost");
            }
            other => panic!("unexpected stream error: {other:?}"),
        }
    }

    fn sample_at(slice: &[u8], index: usize) -> f32 {
        let start = index * 4;
        f32::from_le_bytes(slice[start..start + 4].try_into().unwrap())
    }

    fn sample_s32(slice: &[u8], index: usize) -> i32 {
        let start = index * 4;
        i32::from_le_bytes(slice[start..start + 4].try_into().unwrap())
    }

    fn sample_s24(slice: &[u8], index: usize) -> i32 {
        let start = index * 3;
        let bytes = &slice[start..start + 3];
        let value = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]);
        if value & 0x80_0000 != 0 {
            value | !0xFF_FFFF
        } else {
            value
        }
    }

    fn stream_data() -> (Producer<f32>, StreamData) {
        stream_data_with(SampleFormat::F32)
    }

    fn stream_data_with(format: SampleFormat) -> (Producer<f32>, StreamData) {
        let (producer, consumer) = RingBuffer::new(16);
        let data = StreamData {
            shared: Arc::new(Shared::new(48_000, format)),
            error_sender: flume::unbounded().0,
            error_throttle: Arc::new(Mutex::new(StreamErrorThrottle::new())),
            consumer,
            visualizer: None,
            flush: 0,
            pending: None,
            scratch: vec![0.0; 16],
        };

        (producer, data)
    }

    fn shared_output() -> PipeWireOutput {
        shared_output_with(48_000)
    }

    fn shared_output_with(sample_rate: u32) -> PipeWireOutput {
        PipeWireOutput {
            shared: Arc::new(Shared::new(sample_rate, SampleFormat::F32)),
            quit: None,
        }
    }
}
