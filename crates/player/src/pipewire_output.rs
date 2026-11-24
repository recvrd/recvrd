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

use crate::{PlayError, StreamErrorLog, StreamErrorThrottle};

/// The output format is fixed. The mixer converts every source to this
/// format, and PipeWire negotiates it with the device.
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const BYTES_PER_SAMPLE: usize = std::mem::size_of::<f32>();
const FRAME_BYTES: usize = CHANNELS * BYTES_PER_SAMPLE;
const RING_SECONDS: usize = 1;
const RING_SAMPLES: usize = SAMPLE_RATE as usize * CHANNELS * RING_SECONDS;
const FLUSH_TIMEOUT: Duration = Duration::from_secs(1);

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
    sink_name: Mutex<Option<String>>,
    produced: AtomicU64,
    consumed: AtomicU64,
    played_frames: AtomicU64,
    base_frames: AtomicU64,
    flush_epoch: AtomicU64,
    flush_ack: AtomicU64,
}

impl Shared {
    fn new() -> Self {
        Self {
            paused: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            sink_name: Mutex::new(
                known_sink()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone(),
            ),
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
    ) -> Result<(Self, Mixer, Sink), PlayError> {
        let (mixer, source) = rodio::mixer::mixer(CHANNELS as u16, SAMPLE_RATE);
        let sink = Sink::connect_new(&mixer);
        let output = Self::new(source, error_sender)?;

        Ok((output, mixer, sink))
    }

    fn new(source: MixerSource, error_sender: flume::Sender<PlayError>) -> Result<Self, PlayError> {
        let shared = Arc::new(Shared::new());
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
            .store(frames(position), Ordering::Release);
        self.shared.played_frames.store(0, Ordering::Release);
    }

    pub(crate) fn reset_position(&self) {
        self.shared.base_frames.store(0, Ordering::Release);
        self.shared.played_frames.store(0, Ordering::Release);
    }

    pub(crate) fn position(&self) -> Option<Duration> {
        let base = self.shared.base_frames.load(Ordering::Acquire);
        let played = self.shared.played_frames.load(Ordering::Acquire);

        Some(Duration::from_secs_f64(
            (base + played) as f64 / f64::from(SAMPLE_RATE),
        ))
    }

    pub(crate) fn is_finished(&self, sink_empty: bool) -> bool {
        sink_empty
            && (self.shared.buffered_samples() == 0 || self.shared.failed.load(Ordering::Acquire))
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

        if popped > 0 {
            self.shared.consumed.fetch_add(popped, Ordering::Release);
        }
        self.pending = None;
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

    let (producer, consumer) = RingBuffer::new(RING_SAMPLES);
    let error_throttle = Arc::new(Mutex::new(StreamErrorThrottle::new()));
    let stream_data = StreamData {
        shared: shared.clone(),
        error_sender,
        error_throttle,
        consumer,
        flush: 0,
        pending: None,
        scratch: vec![0.0; RING_SAMPLES],
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
        .state_changed(|_, data, old, new| match new {
            pw::stream::StreamState::Error(message) => {
                data.shared.failed.store(true, Ordering::Release);
                let decision = data
                    .error_throttle
                    .lock()
                    .map_or(StreamErrorLog::LogNow, |mut throttle| {
                        throttle.record(&message, Instant::now())
                    });
                match decision {
                    StreamErrorLog::LogNow => {
                        log::error!("the PipeWire stream changed from {old:?} to Error: {message}")
                    }
                    StreamErrorLog::LogSummary { suppressed } => log::error!(
                        "the PipeWire stream changed from {old:?} to Error: {message} \
                         ({suppressed} repeats suppressed)"
                    ),
                    StreamErrorLog::Suppress => {}
                }
                if !matches!(decision, StreamErrorLog::Suppress) {
                    let _ = data
                        .error_sender
                        .send(PlayError::StreamRuntimeError(message.clone()));
                }
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
    audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
    audio_info.set_rate(SAMPLE_RATE);
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

    log::info!("the PipeWire audio output is connected: {SAMPLE_RATE} Hz, {CHANNELS} channels");

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
    if slice.len() < FRAME_BYTES {
        return;
    }

    let (paused, flushed, written_samples) = write_samples(data, slice);
    let size = (slice.len() / FRAME_BYTES) * FRAME_BYTES;

    if !paused && !flushed {
        let written_frames = written_samples / CHANNELS;
        data.shared
            .played_frames
            .fetch_add(written_frames as u64, Ordering::Release);
    }

    let chunk = out.chunk_mut();
    *chunk.offset_mut() = 0;
    *chunk.stride_mut() = FRAME_BYTES as i32;
    *chunk.size_mut() = size as u32;
}

/// Write one PipeWire buffer. Return whether the output is paused, whether
/// this buffer followed a flush, and the number of samples written.
fn write_samples(data: &mut StreamData, slice: &mut [u8]) -> (bool, bool, usize) {
    let max_frames = slice.len() / FRAME_BYTES;
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

    for frame in 0..max_frames {
        let offset = frame * FRAME_BYTES;

        if frame < frames_to_write {
            for channel in 0..CHANNELS {
                let sample = data.scratch[frame * CHANNELS + channel].clamp(-1.0, 1.0);
                let start = offset + channel * BYTES_PER_SAMPLE;
                slice[start..start + BYTES_PER_SAMPLE].copy_from_slice(&sample.to_le_bytes());
            }
        } else {
            slice[offset..offset + FRAME_BYTES].fill(0);
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

fn frames(position: Duration) -> u64 {
    (position.as_secs_f64() * f64::from(SAMPLE_RATE)).round() as u64
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
    fn a_failed_output_finishes_with_an_empty_sink() {
        let output = shared_output();
        output.shared.produced.store(10, Ordering::Release);
        output.shared.consumed.store(4, Ordering::Release);
        output.shared.failed.store(true, Ordering::Release);

        assert!(output.is_finished(true));
    }

    #[test]
    fn a_full_frame_has_one_frame_of_samples() {
        assert_eq!(FRAME_BYTES, CHANNELS * BYTES_PER_SAMPLE);
    }

    #[test]
    fn write_samples_writes_interleaved_frames_and_clamps() {
        let (mut producer, mut data) = stream_data();
        for sample in [0.25, -0.5, 0.75, 1.5] {
            producer.push(sample).unwrap();
        }
        let mut slice = vec![0; 2 * FRAME_BYTES];

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
        let mut slice = vec![1; 2 * FRAME_BYTES];

        assert_eq!(write_samples(&mut data, &mut slice), (false, false, 0));

        assert!(slice.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn write_samples_discards_samples_while_paused() {
        let (mut producer, mut data) = stream_data();
        producer.push(0.5).unwrap();
        producer.push(-0.5).unwrap();
        data.shared.paused.store(true, Ordering::Release);
        let mut slice = vec![1; FRAME_BYTES];

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
        let mut slice = vec![0; FRAME_BYTES];

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
        let mut slice = vec![0; 2 * FRAME_BYTES];

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
        let mut slice = vec![0; 4 * FRAME_BYTES];

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

    fn sample_at(slice: &[u8], index: usize) -> f32 {
        let start = index * BYTES_PER_SAMPLE;
        f32::from_le_bytes(slice[start..start + BYTES_PER_SAMPLE].try_into().unwrap())
    }

    fn stream_data() -> (Producer<f32>, StreamData) {
        let (producer, consumer) = RingBuffer::new(16);
        let data = StreamData {
            shared: Arc::new(Shared::new()),
            error_sender: flume::unbounded().0,
            error_throttle: Arc::new(Mutex::new(StreamErrorThrottle::new())),
            consumer,
            flush: 0,
            pending: None,
            scratch: vec![0.0; 16],
        };

        (producer, data)
    }

    fn shared_output() -> PipeWireOutput {
        PipeWireOutput {
            shared: Arc::new(Shared::new()),
            quit: None,
        }
    }
}
