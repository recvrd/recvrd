//! Audio spectrum frames for the web visualizer.

use std::{
    collections::VecDeque,
    f32::consts::PI,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use rtrb::Consumer;
use rustfft::{FftPlanner, num_complex::Complex};

/// The number of spectrum bands.
pub const BANDS: usize = 64;

const FFT_SIZE: usize = 1024;
const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const STALE_AFTER: Duration = Duration::from_secs(1);

/// One visualizer frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VisualizerFrame {
    pub bands: Vec<u8>,
    pub peak: u8,
}

pub(crate) struct State {
    bands: Vec<u8>,
    peak: u8,
    updated: Instant,
}

static SLOT: OnceLock<Mutex<Option<Arc<Mutex<State>>>>> = OnceLock::new();
static CLIENTS: AtomicUsize = AtomicUsize::new(0);

/// Keep the visualizer worker active while a client listens.
///
/// The worker does no spectrum work while no guard exists.
pub struct VisualizerGuard;

impl VisualizerGuard {
    pub fn new() -> Self {
        CLIENTS.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Default for VisualizerGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for VisualizerGuard {
    fn drop(&mut self) {
        CLIENTS.fetch_sub(1, Ordering::Relaxed);
    }
}

fn slot() -> &'static Mutex<Option<Arc<Mutex<State>>>> {
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Start the visualizer worker for one output stream.
pub(crate) fn start(mut consumer: Consumer<f32>, sample_rate: u32) -> Arc<Mutex<State>> {
    let state = Arc::new(Mutex::new(State {
        bands: vec![0; BANDS],
        peak: 0,
        updated: Instant::now(),
    }));
    let thread_state = state.clone();
    let _ = thread::Builder::new()
        .name("recvrd-visualizer".to_string())
        .spawn(move || run(&mut consumer, sample_rate, thread_state));
    state
}

/// Install the current worker state for the API.
pub(crate) fn install(state: Arc<Mutex<State>>) {
    let mut slot = slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = Some(state);
}

/// Read the latest visualizer frame.
///
/// Return `None` when no PipeWire output has started. Return a zero
/// frame when the worker has not updated the frame for one second.
pub fn visualizer_frame() -> Option<VisualizerFrame> {
    let state = {
        let slot = slot()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        slot.as_ref()?.clone()
    };
    let state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.updated.elapsed() > STALE_AFTER {
        return Some(VisualizerFrame {
            bands: vec![0; BANDS],
            peak: 0,
        });
    }
    Some(VisualizerFrame {
        bands: state.bands.clone(),
        peak: state.peak,
    })
}

fn run(consumer: &mut Consumer<f32>, sample_rate: u32, state: Arc<Mutex<State>>) {
    let hann: Vec<f32> = (0..FFT_SIZE)
        .map(|index| 0.5 - 0.5 * (2.0 * PI * index as f32 / (FFT_SIZE - 1) as f32).cos())
        .collect();
    let bins = band_bins(sample_rate);
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut window = VecDeque::with_capacity(FFT_SIZE);
    let mut smoothed = vec![0u8; BANDS];

    loop {
        if consumer.is_abandoned() {
            break;
        }

        if CLIENTS.load(Ordering::Relaxed) == 0 {
            while consumer.pop().is_ok() {}
            thread::sleep(Duration::from_millis(200));
            continue;
        }

        let mut fresh = 0;
        while let Ok(sample) = consumer.pop() {
            window.push_back(sample);
            if window.len() > FFT_SIZE {
                window.pop_front();
            }
            fresh += 1;
        }

        if fresh == 0 {
            for value in &mut smoothed {
                *value = (*value as f32 * 0.8) as u8;
            }
            publish(&state, &smoothed);
            thread::sleep(FRAME_INTERVAL);
            continue;
        }

        if window.len() < FFT_SIZE {
            thread::sleep(FRAME_INTERVAL);
            continue;
        }

        let mut buffer: Vec<Complex<f32>> = window
            .iter()
            .zip(&hann)
            .map(|(sample, window)| Complex::new(sample * window, 0.0))
            .collect();
        fft.process(&mut buffer);
        let levels = band_levels(&buffer, &bins);
        for (index, level) in levels.iter().enumerate() {
            smoothed[index] = (*level).max((smoothed[index] as f32 * 0.7) as u8);
        }
        publish(&state, &smoothed);
        thread::sleep(FRAME_INTERVAL);
    }

    if let Ok(mut state) = state.lock() {
        state.bands.fill(0);
        state.peak = 0;
        state.updated = Instant::now();
    }
}

fn publish(state: &Arc<Mutex<State>>, bands: &[u8]) {
    let peak = bands.iter().copied().max().unwrap_or(0);
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.bands.clear();
    state.bands.extend_from_slice(bands);
    state.peak = peak;
    state.updated = Instant::now();
}

fn band_bins(sample_rate: u32) -> Vec<(usize, usize)> {
    let nyquist = FFT_SIZE / 2;
    let hz_to_bin = |hz: f32| (hz * FFT_SIZE as f32 / sample_rate as f32) as usize;
    let low = hz_to_bin(40.0).clamp(1, nyquist - 1);
    let high = hz_to_bin(16_000.0).clamp(low + 1, nyquist);
    (0..BANDS)
        .map(|band| {
            let start = low as f32 * (high as f32 / low as f32).powf(band as f32 / BANDS as f32);
            let end =
                low as f32 * (high as f32 / low as f32).powf((band + 1) as f32 / BANDS as f32);
            let start = (start as usize).min(nyquist - 1);
            let end = (end as usize).clamp(start + 1, nyquist);
            (start, end)
        })
        .collect()
}

fn band_levels(spectrum: &[Complex<f32>], bins: &[(usize, usize)]) -> Vec<u8> {
    bins.iter()
        .map(|(start, end)| {
            let end = (*end).min(spectrum.len());
            let start = (*start).min(end);
            let sum: f32 = spectrum[start..end].iter().map(|bin| bin.norm()).sum();
            let magnitude = (sum / (end - start).max(1) as f32) / (FFT_SIZE as f32 / 4.0);
            (magnitude.sqrt() * 255.0).clamp(0.0, 255.0) as u8
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sine_bin_reaches_a_band() {
        let bins = band_bins(48_000);
        let mut spectrum = vec![Complex::new(0.0, 0.0); FFT_SIZE];
        spectrum[21] = Complex::new(512.0, 0.0);
        let levels = band_levels(&spectrum, &bins);
        assert!(levels.iter().any(|level| *level > 0));
        assert!(levels.iter().copied().max().unwrap() > 0);
    }

    #[test]
    fn silence_is_zero() {
        let bins = band_bins(48_000);
        let spectrum = vec![Complex::new(0.0, 0.0); FFT_SIZE];
        let levels = band_levels(&spectrum, &bins);
        assert_eq!(levels.len(), BANDS);
        assert!(levels.iter().all(|level| *level == 0));
    }
}
