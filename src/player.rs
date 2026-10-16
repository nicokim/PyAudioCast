use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig, SupportedStreamConfig};
use log::{debug, error, info, warn};
use numpy::{Element, PyArrayDyn, PyArrayMethods, PyUntypedArray, PyUntypedArrayMethods};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyByteArray, PyBytes, PyList};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

use crate::device::{OutputTarget, SinkGuard, find_device, open_lock};
use crate::error::SpeakerError;

/// Ring buffer length, in seconds of device audio.
const RING_SECONDS: usize = 4;
/// How often blocking write()/drain() re-check their exit conditions.
// ponytail: polling keeps the audio callback lock-free; a futex wake-up would shave <=5 ms off drain().
const POLL: Duration = Duration::from_millis(5);
/// How often blocking calls let Python run signal handlers (Ctrl+C).
const SIGNAL_CHECK: Duration = Duration::from_millis(100);
/// A stream that doesn't call back for this long while we wait on it is considered dead.
// ponytail: generous so slow device start-up (e.g. Bluetooth) isn't flagged; lower it if
// faster failure detection matters.
const STALL_TIMEOUT: Duration = Duration::from_secs(5);
/// STALL_TIMEOUT in polls: counted, not timed, so a frozen process (Ctrl+Z, a debugger)
/// isn't mistaken for a dead device when it resumes.
const STALL_POLLS: u32 = (STALL_TIMEOUT.as_millis() / POLL.as_millis()) as u32;
/// Stream errors coming for this long with no audio callback in between: the server has
/// dropped the stream, and cpal retries in a busy loop. Throttle it.
const THROTTLE_ERRORS_AFTER: Duration = Duration::from_secs(1);
/// `Shared::failing_since` when there's been no error since the last audio callback.
const NOT_FAILING: u64 = u64::MAX;
const SAMPLE_RATES: RangeInclusive<u32> = 1_000..=768_000;
const CHANNELS: RangeInclusive<u16> = 1..=64;
/// Largest sample every output format takes without overflowing: dasp's f32 -> 24-bit
/// conversion doesn't saturate, so 1.0 would wrap around to full-scale negative.
pub(crate) const MAX_SAMPLE: f32 = 8_388_607.0 / 8_388_608.0;
/// clear() fades out over this long instead of cutting the waveform, which clicks.
const FADE: Duration = Duration::from_millis(5);
/// Output latencies above this are treated as bogus (drain() waits that long).
const MAX_LATENCY: Duration = Duration::from_secs(1);

/// State shared between the Python-facing player and the audio callback.
/// The callback side only touches atomics: no locks, no allocation.
struct Shared {
    /// Samples ever pushed into the ring (device rate/channels).
    written: AtomicU64,
    /// Samples ever taken out of the ring by the callback (played or discarded).
    consumed: AtomicU64,
    /// The callback discards samples until `consumed` reaches this (set by clear()).
    discard_until: AtomicU64,
    /// Bumped by clear() and stop() so in-flight write()/drain() calls bail out.
    epoch: AtomicU64,
    closed: AtomicBool,
    /// Set (with `error`) when the stream fails for good.
    failed: AtomicBool,
    error: Mutex<String>,
    /// A recoverable stream error has been logged already.
    warned: AtomicBool,
    /// Number of audio callbacks so far.
    callbacks: AtomicU64,
    /// Nanoseconds after `start` of the first stream error since the last audio callback.
    failing_since: AtomicU64,
    /// Nanoseconds after `start` when the last audio handed to the device is audible.
    audible_until: AtomicU64,
    /// `written` when a drain() saw everything play out (u64::MAX: none since the last write).
    drained: AtomicU64,
    start: Instant,
}

impl Shared {
    fn new() -> Self {
        Shared {
            written: AtomicU64::new(0),
            consumed: AtomicU64::new(0),
            discard_until: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            error: Mutex::new(String::new()),
            warned: AtomicBool::new(false),
            callbacks: AtomicU64::new(0),
            failing_since: AtomicU64::new(NOT_FAILING),
            audible_until: AtomicU64::new(0),
            drained: AtomicU64::new(u64::MAX),
            start: Instant::now(),
        }
    }

    fn now(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64
    }

    /// Discard everything written so far and abort in-flight write()/drain().
    fn clear(&self) {
        // SeqCst pairs with push(): either we see its samples in `written`, or it sees our epoch.
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.discard_until
            .fetch_max(self.written.load(Ordering::SeqCst), Ordering::SeqCst);
    }

    fn fail(&self, msg: String) {
        let mut error = lock(&self.error);
        // Message before flag: check() reads it as soon as it sees `failed`.
        if !self.failed.load(Ordering::Acquire) {
            error.clone_from(&msg);
            self.failed.store(true, Ordering::Release);
            drop(error);
            error!("Stream error: {}", msg);
        }
    }

    /// Error out if the player is closed or its stream is dead.
    fn check(&self) -> PyResult<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(SpeakerError::StreamError("Player is closed".into()).into());
        }
        if self.failed.load(Ordering::Acquire) {
            return Err(SpeakerError::StreamError(lock(&self.error).clone()).into());
        }
        Ok(())
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Audio handed to write(), not converted yet.
pub(crate) enum Samples {
    /// int16 little-endian PCM bytes.
    Pcm16(Vec<u8>),
    /// Interleaved floats in -1.0..=MAX_SAMPLE.
    Float(Vec<f32>),
}

/// Python-thread side: the ring producer plus conversion state.
struct Writer {
    producer: HeapProd<f32>,
    converter: Converter,
    /// Epoch the carried state belongs to; reset after a clear().
    epoch: u64,
    /// Odd trailing byte of int16 PCM, completed by the next write().
    odd_byte: Option<u8>,
    /// Trailing partial frame, completed by the next write().
    partial: Vec<f32>,
}

impl Writer {
    /// Whole input frames. Completes the sample/frame left over by the previous write and
    /// keeps the new leftover, so chunks needn't be aligned (streamed HTTP bodies aren't).
    fn frames(&mut self, samples: Samples) -> Vec<f32> {
        let mut floats = match samples {
            Samples::Pcm16(mut bytes) => {
                if let Some(b) = self.odd_byte.take() {
                    bytes.insert(0, b);
                }
                if !bytes.len().is_multiple_of(2) {
                    self.odd_byte = bytes.pop();
                }
                i16le_to_f32(&bytes)
            }
            Samples::Float(floats) => floats,
        };
        if !self.partial.is_empty() {
            floats.splice(0..0, self.partial.drain(..));
        }
        let whole = floats.len() - floats.len() % self.converter.src_ch;
        self.partial.extend_from_slice(&floats[whole..]);
        floats.truncate(whole);
        floats
    }

    /// Start of a write(). A clear() (new epoch) or a drain() that saw everything play out
    /// ends the stream: what's left of it (a cut-off stream ends mid-sample) would misalign
    /// the next one.
    fn begin(&mut self, epoch: u64, drained: bool) {
        if drained && (self.odd_byte.is_some() || !self.partial.is_empty()) {
            warn!("The previous audio ended mid-sample or mid-frame; dropping the remainder");
        }
        if self.epoch != epoch || drained {
            self.converter.reset();
            self.odd_byte = None;
            self.partial.clear();
            self.epoch = epoch;
        }
    }
}

#[pyclass(frozen)]
pub struct AudioPlayer {
    shared: Arc<Shared>,
    writer: Mutex<Writer>,
    stream: Mutex<Option<cpal::Stream>>,
    sample_rate: u32,
    channels: u16,
    device_sample_rate: u32,
    device_channels: u16,
}

#[pymethods]
impl AudioPlayer {
    /// Create a new AudioPlayer.
    ///
    /// Args:
    ///     device: Device id or name (exact, then substring match), a PipeWire/PulseAudio
    ///         sink name, or None for the default device.
    ///     sample_rate: Sample rate in Hz, or None to auto-detect from device.
    ///     channels: Number of channels, or None to auto-detect from device.
    #[new]
    #[pyo3(signature = (device=None, sample_rate=None, channels=None))]
    pub fn new(
        py: Python<'_>,
        device: Option<&str>,
        sample_rate: Option<u32>,
        channels: Option<u16>,
    ) -> PyResult<Self> {
        crate::init_logging();
        if sample_rate.is_some_and(|r| !SAMPLE_RATES.contains(&r)) {
            return Err(PyValueError::new_err(format!(
                "sample_rate must be in {}..={} Hz",
                SAMPLE_RATES.start(),
                SAMPLE_RATES.end()
            )));
        }
        if channels.is_some_and(|c| !CHANNELS.contains(&c)) {
            return Err(PyValueError::new_err(format!(
                "channels must be in {}..={}",
                CHANNELS.start(),
                CHANNELS.end()
            )));
        }
        info!(
            "AudioPlayer::new(device={:?}, sample_rate={:?}, channels={:?})",
            device, sample_rate, channels
        );
        // Device lookup and probing can take a while (pactl, opening PCMs): don't hold the GIL...
        let target = py.detach(|| find_device(device))?;
        if target.sink.is_some() {
            // ...except while the sink variable is overridden: the environment is process-global.
            return Ok(Self::open(target, sample_rate, channels)?);
        }
        Ok(py.detach(|| Self::open(target, sample_rate, channels))?)
    }

    /// Write audio data to the player. Blocks while the buffer is full.
    ///
    /// Accepts:
    ///   - `bytes` / `bytearray`: Raw int16 little-endian PCM data
    ///   - `list[float]`: Float samples in -1.0..1.0 range
    ///   - `numpy.ndarray`: int16, int32, float32 or float64, 1-D interleaved or
    ///     2-D shaped (frames, channels)
    ///
    /// Chunks needn't hold whole samples or frames: a remainder is kept for the next write,
    /// until `drain()` or `clear()` ends the stream.
    fn write(&self, py: Python<'_>, data: &Bound<'_, PyAny>) -> PyResult<()> {
        let epoch = self.shared.epoch.load(Ordering::SeqCst);
        self.shared.check()?;
        let samples = extract_samples(data, self.channels as usize)?;
        py.detach(|| self.push(samples, epoch))
    }

    /// Block until all audio written so far has been played.
    /// Returns immediately if `clear()` or `stop()` is called meanwhile.
    /// Ends the stream: the next `write()` starts a new one (see `write()`).
    pub fn drain(&self, py: Python<'_>) -> PyResult<()> {
        let shared = &*self.shared;
        let target = shared.written.load(Ordering::SeqCst);
        let epoch = shared.epoch.load(Ordering::SeqCst);
        debug!("drain: waiting for sample {}", target);
        py.detach(|| {
            let mut waiter = Waiter::new(shared);
            // When the last sample written so far is audible: known once the callback has
            // taken it, and not pushed back by audio written after this call.
            let mut audible_at = None;
            loop {
                if shared.epoch.load(Ordering::SeqCst) != epoch
                    || shared.closed.load(Ordering::SeqCst)
                    || shared.discard_until.load(Ordering::Acquire) >= target
                {
                    return Ok(());
                }
                // Consumed by the callback *and* out of the device buffer, so a
                // stop() right after drain() doesn't cut the tail.
                if audible_at.is_none() && shared.consumed.load(Ordering::Acquire) >= target {
                    audible_at = Some(shared.audible_until.load(Ordering::Acquire));
                }
                if audible_at.is_some_and(|t| shared.now() >= t) {
                    debug!("drain: complete");
                    shared.drained.store(target, Ordering::Release);
                    return Ok(());
                }
                shared.check()?;
                waiter.wait()?;
            }
        })
    }

    /// Discard all buffered audio immediately. A blocked `drain()` returns and an
    /// in-flight `write()` drops the rest of its data. Safe to call from any thread;
    /// later `write()` calls start a new stream.
    fn clear(&self) {
        debug!("clear: discarding buffered audio");
        self.shared.clear();
    }

    /// Stop the player and release the audio device. Safe to call more than once.
    pub fn stop(&self, py: Python<'_>) {
        info!("Stopping AudioPlayer");
        self.shared.closed.store(true, Ordering::SeqCst);
        self.shared.clear();
        // An in-flight write() notices `closed` within one POLL; the writer (and its ring)
        // goes with the player.
        py.detach(|| drop(lock(&self.stream).take()));
    }

    /// Returns the configured sample rate.
    #[getter]
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Returns the configured channel count.
    #[getter]
    fn channels(&self) -> u16 {
        self.channels
    }

    /// Returns the actual device sample rate.
    #[getter]
    fn device_sample_rate(&self) -> u32 {
        self.device_sample_rate
    }

    /// Returns the actual device channel count.
    #[getter]
    fn device_channels(&self) -> u16 {
        self.device_channels
    }

    /// Returns whether the player is open and its stream healthy.
    #[getter]
    fn is_active(&self) -> bool {
        !self.shared.closed.load(Ordering::SeqCst) && !self.shared.failed.load(Ordering::Acquire)
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
    fn __exit__(
        &self,
        py: Python<'_>,
        _exc_type: Option<&Bound<'_, PyAny>>,
        _exc_val: Option<&Bound<'_, PyAny>>,
        _exc_tb: Option<&Bound<'_, PyAny>>,
    ) {
        self.stop(py);
    }
}

impl AudioPlayer {
    fn open(
        target: OutputTarget,
        sample_rate: Option<u32>,
        channels: Option<u16>,
    ) -> Result<Self, SpeakerError> {
        // Declared first, so it's released after the sink variable is restored.
        let _open = open_lock();
        // Sinks are reached through the pulse/pipewire device: point it at the sink while opening.
        let _sink = target
            .sink
            .as_ref()
            .map(|(var, sink)| SinkGuard::set(var, sink));
        let device = &target.device;
        let id = device.id().map(|id| id.to_string()).unwrap_or_default();
        let context = |e: SpeakerError| e.context(&id);

        let default_config = device
            .default_output_config()
            .map_err(SpeakerError::from)
            .map_err(context)?;
        let user_rate = sample_rate.unwrap_or(default_config.sample_rate());
        let user_channels = channels.unwrap_or(default_config.channels());
        // CoreAudio applies the stream format to the hardware, switching the device's rate
        // for every app: on macOS keep its current rate and channels and convert here instead.
        // (cpal may still switch the hardware's sample format to 32-bit float.)
        let config = if cfg!(target_os = "macos") {
            default_config
        } else {
            let supported: Vec<_> = device
                .supported_output_configs()
                .map_err(SpeakerError::from)
                .map_err(context)?
                .collect();
            negotiate(&supported, default_config, user_rate, user_channels)
        };
        let (device_rate, device_channels) = (config.sample_rate(), config.channels());
        info!(
            "Config: user={}Hz {}ch, device={}Hz {}ch {}",
            user_rate,
            user_channels,
            device_rate,
            device_channels,
            config.sample_format()
        );

        let capacity = device_rate as usize * device_channels as usize * RING_SECONDS;
        let (producer, consumer) = HeapRb::<f32>::new(capacity).split();
        let shared = Arc::new(Shared::new());
        let stream_config = config.config();
        let s = shared.clone();
        let stream = match config.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(device, stream_config, consumer, s),
            SampleFormat::F64 => build_stream::<f64>(device, stream_config, consumer, s),
            SampleFormat::I8 => build_stream::<i8>(device, stream_config, consumer, s),
            SampleFormat::I16 => build_stream::<i16>(device, stream_config, consumer, s),
            SampleFormat::I24 => build_stream::<cpal::I24>(device, stream_config, consumer, s),
            SampleFormat::I32 => build_stream::<i32>(device, stream_config, consumer, s),
            SampleFormat::I64 => build_stream::<i64>(device, stream_config, consumer, s),
            SampleFormat::U8 => build_stream::<u8>(device, stream_config, consumer, s),
            SampleFormat::U16 => build_stream::<u16>(device, stream_config, consumer, s),
            SampleFormat::U24 => build_stream::<cpal::U24>(device, stream_config, consumer, s),
            SampleFormat::U32 => build_stream::<u32>(device, stream_config, consumer, s),
            SampleFormat::U64 => build_stream::<u64>(device, stream_config, consumer, s),
            other => {
                return Err(SpeakerError::ConfigError(format!(
                    "Unsupported sample format: {}",
                    other
                ))
                .context(&id));
            }
        }
        .map_err(context)?;
        stream.play().map_err(SpeakerError::from).map_err(context)?;
        info!("Stream started");

        Ok(AudioPlayer {
            shared,
            writer: Mutex::new(Writer {
                producer,
                converter: Converter::new(user_rate, device_rate, user_channels, device_channels),
                epoch: 0,
                odd_byte: None,
                partial: Vec::new(),
            }),
            stream: Mutex::new(Some(stream)),
            sample_rate: user_rate,
            channels: user_channels,
            device_sample_rate: device_rate,
            device_channels,
        })
    }

    /// Convert and push samples, blocking while the ring is full. Call without the GIL.
    /// Gives up silently if clear() runs meanwhile, with an error on stop()/stream failure.
    pub(crate) fn push(&self, samples: Samples, epoch: u64) -> PyResult<()> {
        let shared = &*self.shared;
        let samples = {
            let mut w = lock(&self.writer);
            if shared.epoch.load(Ordering::SeqCst) != epoch {
                return shared.check();
            }
            let drained = shared.drained.swap(u64::MAX, Ordering::AcqRel)
                == shared.written.load(Ordering::SeqCst);
            w.begin(epoch, drained);
            let frames = w.frames(samples);
            debug!("write: {} f32 samples", frames.len());
            w.converter.process(frames)
        };
        let mut offset = 0;
        let mut waiter = Waiter::new(shared);
        loop {
            {
                // Not held while waiting: a signal handler or finalizer that runs meanwhile
                // may use the player.
                let mut w = lock(&self.writer);
                if shared.epoch.load(Ordering::SeqCst) != epoch {
                    // clear()/stop(): drop the rest.
                    return shared.check();
                }
                let n = w.producer.vacant_len().min(samples.len() - offset);
                // Count samples before publishing them: a racing clear() either covers them,
                // or we see its epoch and cover them, before the callback can see them.
                let end = shared.written.fetch_add(n as u64, Ordering::SeqCst) + n as u64;
                if shared.epoch.load(Ordering::SeqCst) != epoch {
                    shared.discard_until.fetch_max(end, Ordering::SeqCst);
                }
                w.producer.push_slice(&samples[offset..offset + n]);
                offset += n;
            }
            if offset == samples.len() {
                return Ok(());
            }
            shared.check()?;
            waiter.wait()?;
        }
    }
}

/// One step of a blocking wait: sleeps one POLL, runs Python signal handlers now and then
/// (raises on Ctrl+C), and fails the stream if it stopped calling back while we wait.
struct Waiter<'a> {
    shared: &'a Shared,
    signals_checked: Instant,
    callbacks: u64,
    /// Polls in a row without an audio callback.
    stalled: u32,
}

impl<'a> Waiter<'a> {
    fn new(shared: &'a Shared) -> Self {
        Waiter {
            shared,
            signals_checked: Instant::now(),
            callbacks: shared.callbacks.load(Ordering::Relaxed),
            stalled: 0,
        }
    }

    fn wait(&mut self) -> PyResult<()> {
        std::thread::sleep(POLL);
        let callbacks = self.shared.callbacks.load(Ordering::Relaxed);
        if callbacks != self.callbacks {
            self.callbacks = callbacks;
            self.stalled = 0;
        } else {
            self.stalled += 1;
            if self.stalled >= STALL_POLLS {
                self.shared.fail("Audio device stopped responding".into());
            }
        }
        if self.signals_checked.elapsed() >= SIGNAL_CHECK {
            self.signals_checked = Instant::now();
            // try_attach: attaching while the interpreter shuts down would park this thread.
            Python::try_attach(|py| py.check_signals()).unwrap_or(Ok(()))?;
        }
        Ok(())
    }
}

/// Pick the device config: the caller's rate and channel count if the device takes them
/// (so the system mixer converts channels), else the caller's rate at the device's default
/// channel count, else the device default. Within those, cpal's own sample-format preference.
fn negotiate(
    supported: &[cpal::SupportedStreamConfigRange],
    default: SupportedStreamConfig,
    rate: u32,
    channels: u16,
) -> SupportedStreamConfig {
    let pick = |channels: u16| {
        supported
            .iter()
            .filter(|c| {
                c.channels() == channels && c.contains_rate(rate) && !c.sample_format().is_dsd()
            })
            .max_by(|a, b| a.cmp_default_heuristics(b))
            .map(|c| c.with_sample_rate(rate))
    };
    pick(channels)
        .or_else(|| pick(default.channels()))
        .unwrap_or(default)
}

fn build_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    consumer: HeapCons<f32>,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, SpeakerError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let ns_per_sample = 1e9 / (config.sample_rate as f64 * channels as f64);
    let err_shared = shared.clone();
    let fade_frames = (config.sample_rate as f64 * FADE.as_secs_f64()) as usize;
    let mut renderer = Renderer::new(consumer, channels, fade_frames);
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            let now = shared.now();
            shared.callbacks.fetch_add(1, Ordering::Relaxed);
            shared.failing_since.store(NOT_FAILING, Ordering::Relaxed);
            let n = renderer.render(data, || shared.discard_until.load(Ordering::Acquire));
            if n > 0 {
                let ts = info.timestamp();
                let delay = ts.playback.duration_since(ts.callback).min(MAX_LATENCY);
                shared.audible_until.store(
                    now + delay.as_nanos() as u64 + (n as f64 * ns_per_sample) as u64,
                    Ordering::Release,
                );
            }
            // After audible_until: drain() reads consumed first.
            shared.consumed.store(renderer.consumed, Ordering::Release);
        },
        move |err: cpal::Error| match err.kind() {
            cpal::ErrorKind::Xrun => debug!("Stream xrun"),
            cpal::ErrorKind::RealtimeDenied => debug!("Stream: {}", err),
            cpal::ErrorKind::DeviceNotAvailable
            | cpal::ErrorKind::StreamInvalidated
            | cpal::ErrorKind::HostUnavailable => err_shared.fail(err.to_string()),
            // Recoverable (e.g. DeviceChanged: rerouted to the new default), unless it keeps
            // coming with no audio in between: then the server has dropped the stream, and
            // cpal retries in a busy loop.
            _ => {
                let s = &*err_shared;
                let now = s.now();
                // Timed from the first error, not the last callback: a process frozen for a
                // while (Ctrl+Z) gets a burst of errors when it resumes, then recovers.
                let since = s
                    .failing_since
                    .compare_exchange(NOT_FAILING, now, Ordering::Relaxed, Ordering::Relaxed)
                    .map_or_else(|since| since, |_| now);
                let failing = Duration::from_nanos(now.saturating_sub(since));
                if failing > THROTTLE_ERRORS_AFTER {
                    if failing > STALL_TIMEOUT {
                        s.fail(err.to_string());
                    }
                    std::thread::sleep(Duration::from_millis(50));
                } else if !s.warned.swap(true, Ordering::Relaxed) {
                    warn!("Stream error: {}", err);
                } else {
                    debug!("Stream error: {}", err);
                }
            }
        },
        None,
    )?;
    Ok(stream)
}

/// The audio callback's work: plays the ring, and skips what clear() discarded, fading out
/// the audio that was playing so the cut doesn't click. No locks, no allocation.
struct Renderer {
    consumer: HeapCons<f32>,
    channels: usize,
    fade_samples: usize,
    /// Samples taken out of the ring so far (played or discarded).
    consumed: u64,
    /// The previous buffer carried audio.
    playing: bool,
}

impl Renderer {
    fn new(consumer: HeapCons<f32>, channels: usize, fade_frames: usize) -> Self {
        Renderer {
            consumer,
            channels,
            fade_samples: fade_frames * channels,
            consumed: 0,
            playing: false,
        }
    }

    /// Fill `data` (whole frames only, so channels can never rotate); returns how many
    /// samples carried audio. Samples before the `discard_until()` watermark are dropped.
    fn render<T: SizedSample + FromSample<f32>>(
        &mut self,
        data: &mut [T],
        discard_until: impl FnOnce() -> u64,
    ) -> usize {
        let ch = self.channels;
        // Look at the ring before reading the watermark: push() raises the watermark before
        // it publishes samples a clear() made stale, so none of these can slip past it.
        let (head, tail) = self.consumer.as_slices();
        let queued = head.len() + tail.len();
        let stale = discard_until()
            .saturating_sub(self.consumed)
            .min(queued as u64) as usize;
        let samples = head.iter().chain(tail);
        let mut n = 0;
        if stale > 0 && self.playing {
            let fade = self.fade_samples.min(stale).min(data.len()) / ch * ch;
            let frames = (fade / ch) as f32;
            for (k, (out, &s)) in data[..fade].iter_mut().zip(samples.clone()).enumerate() {
                *out = T::from_sample(s * (1.0 - (k / ch) as f32 / frames));
            }
            n = fade;
        }
        let m = (queued - stale).min(data.len() - n) / ch * ch;
        for (out, &s) in data[n..n + m].iter_mut().zip(samples.skip(stale)) {
            *out = T::from_sample(s);
        }
        self.consumer.skip(stale + m);
        self.consumed += (stale + m) as u64;
        data[n + m..].fill(T::EQUILIBRIUM);
        self.playing = m > 0;
        n + m
    }
}

/// Source frames of history the cubic resampler keeps between calls.
const HISTORY: usize = 3;

/// Converts interleaved f32 audio from the caller's rate/channels to the device's.
/// Resampler state is carried across calls so chunked input stays seamless.
struct Converter {
    src_rate: u32,
    dst_rate: u32,
    src_ch: usize,
    dst_ch: usize,
    /// Position of the next output frame in 1/dst_rate source frames (exact, so chunking
    /// can't change the output), history included: frames 0..HISTORY are the end of the
    /// previous input, the next input starts at HISTORY.
    pos: u64,
    /// Last HISTORY source frames, oldest first.
    history: Vec<f32>,
    /// `history` holds real audio (false after new()/reset()).
    primed: bool,
}

impl Converter {
    fn new(src_rate: u32, dst_rate: u32, src_ch: u16, dst_ch: u16) -> Self {
        Converter {
            src_rate,
            dst_rate,
            src_ch: src_ch as usize,
            dst_ch: dst_ch as usize,
            pos: HISTORY as u64 * dst_rate as u64,
            history: vec![0.0; HISTORY * src_ch as usize],
            primed: false,
        }
    }

    /// Forget resampler history so old audio doesn't bleed into new audio.
    fn reset(&mut self) {
        self.primed = false;
    }

    fn process(&mut self, input: Vec<f32>) -> Vec<f32> {
        let (src_ch, dst_ch) = (self.src_ch, self.dst_ch);
        if self.src_rate == self.dst_rate {
            if src_ch == dst_ch {
                return input;
            }
            let mut out = Vec::with_capacity(input.len() / src_ch * dst_ch);
            for frame in input.chunks_exact(src_ch) {
                remap(frame, dst_ch, &mut out);
            }
            return out;
        }
        let n = input.len() / src_ch;
        if n == 0 {
            return Vec::new();
        }
        if !self.primed {
            // Start exactly on the first frame, with the history padded by it (no ramp up).
            for past in self.history.chunks_exact_mut(src_ch) {
                past.copy_from_slice(&input[..src_ch]);
            }
            self.pos = HISTORY as u64 * self.dst_rate as u64;
            self.primed = true;
        }
        // ponytail: 4-point cubic (Catmull-Rom) interpolation, no anti-aliasing filter;
        // swap in a sinc resampler (e.g. rubato) if downsampling quality matters.
        let (src, dst) = (self.src_rate as u64, self.dst_rate as u64);
        let mut out = Vec::with_capacity((n as u64 * dst / src) as usize * dst_ch + 2 * dst_ch);
        let mut frame = vec![0.0f32; src_ch];
        let history = &self.history;
        let at = |i: usize, c: usize| {
            if i < HISTORY {
                history[i * src_ch + c]
            } else {
                input[(i - HISTORY) * src_ch + c]
            }
        };
        // Interpolating at pos needs frames floor(pos)-1 ..= floor(pos)+2.
        let end = (HISTORY + n - 2) as u64 * dst;
        let mut pos = self.pos;
        while pos < end {
            let i = (pos / dst) as usize;
            let t = (pos % dst) as f32 / dst as f32;
            for (c, s) in frame.iter_mut().enumerate() {
                let y = catmull_rom(at(i - 1, c), at(i, c), at(i + 1, c), at(i + 2, c), t);
                // The curve can overshoot the input range.
                *s = y.clamp(-1.0, MAX_SAMPLE);
            }
            remap(&frame, dst_ch, &mut out);
            pos += src;
        }
        let history: Vec<f32> = (n..n + HISTORY)
            .flat_map(|i| (0..src_ch).map(move |c| (i, c)))
            .map(|(i, c)| at(i, c))
            .collect();
        self.history = history;
        self.pos = pos - n as u64 * dst;
        out
    }
}

/// Cubic Hermite through y1 (t=0) and y2 (t=1), with Catmull-Rom tangents from y0 and y3.
fn catmull_rom(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// Map one frame to `dst_ch` channels by position (front left/right come first in every
/// backend's layout): mono feeds both fronts, extra outputs (centre, LFE, rears) stay silent,
/// a mono device gets the average.
// ponytail: downmixing to 2+ channels keeps the first channels (5.1 -> stereo loses the
// centre); add per-layout matrices if multichannel sources become common.
fn remap(frame: &[f32], dst_ch: usize, out: &mut Vec<f32>) {
    let src_ch = frame.len();
    if dst_ch == 1 {
        out.push(frame.iter().sum::<f32>() / src_ch as f32);
        return;
    }
    let fronts: &[f32] = if src_ch == 1 {
        &[frame[0], frame[0]]
    } else {
        frame
    };
    let n = fronts.len().min(dst_ch);
    out.extend_from_slice(&fronts[..n]);
    out.extend(std::iter::repeat_n(0.0, dst_ch - n));
}

/// Take any supported Python audio data (converted later, without the GIL).
fn extract_samples(data: &Bound<'_, PyAny>, channels: usize) -> PyResult<Samples> {
    if let Ok(b) = data.cast::<PyBytes>() {
        return Ok(Samples::Pcm16(b.as_bytes().to_vec()));
    }
    if let Ok(b) = data.cast::<PyByteArray>() {
        return Ok(Samples::Pcm16(b.to_vec()));
    }
    if let Ok(list) = data.cast::<PyList>() {
        let mut samples = list.extract::<Vec<f32>>()?;
        samples.iter_mut().for_each(|s| *s = sanitize(*s));
        return Ok(Samples::Float(samples));
    }
    if numpy_loaded(data.py())
        && let Ok(arr) = data.cast::<PyUntypedArray>()
    {
        return numpy_to_f32(arr, channels).map(Samples::Float);
    }
    Err(PyTypeError::new_err(
        "Expected bytes, bytearray, list[float] or a numpy array (int16/int32/float32/float64)",
    ))
}

/// numpy is an optional dependency; only ask rust-numpy about arrays once numpy is
/// imported, since its type check panics when numpy can't be loaded (an ndarray can't
/// exist before that anyway). `sys.modules["numpy"] = None` blocks the import.
fn numpy_loaded(py: Python<'_>) -> bool {
    py.import("sys")
        .and_then(|sys| sys.getattr("modules"))
        .and_then(|modules| modules.get_item("numpy"))
        .is_ok_and(|numpy| !numpy.is_none())
}

/// NaN/inf from a misbehaving generator would reach the device as full-scale noise, and
/// values past MAX_SAMPLE overflow 24-bit output.
pub(crate) fn sanitize(s: f32) -> f32 {
    if s.is_finite() {
        s.clamp(-1.0, MAX_SAMPLE)
    } else {
        0.0
    }
}

fn i16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| i16::from_le_bytes(b) as f32 / 32768.0)
        .collect()
}

fn numpy_to_f32(arr: &Bound<'_, PyUntypedArray>, channels: usize) -> PyResult<Vec<f32>> {
    let shape = arr.shape();
    match shape {
        [_] => {}
        [_, c] if *c == channels => {}
        _ => {
            return Err(PyValueError::new_err(format!(
                "Expected a 1-D array or a 2-D array shaped (frames, {}), got shape {:?}",
                channels, shape
            )));
        }
    }
    if let Ok(a) = arr.cast::<PyArrayDyn<f32>>() {
        collect(a, sanitize)
    } else if let Ok(a) = arr.cast::<PyArrayDyn<i16>>() {
        collect(a, |s| s as f32 / 32768.0)
    } else if let Ok(a) = arr.cast::<PyArrayDyn<f64>>() {
        collect(a, |s| sanitize(s as f32))
    } else if let Ok(a) = arr.cast::<PyArrayDyn<i32>>() {
        // i32::MAX / 2^31 rounds to 1.0 in f32.
        collect(a, |s| (s as f32 / 2147483648.0).min(MAX_SAMPLE))
    } else {
        Err(PyTypeError::new_err(format!(
            "Unsupported numpy dtype: {}. Expected float32, float64, int16, or int32",
            arr.dtype()
        )))
    }
}

/// Copy a numpy array in logical (C) order, so (frames, channels) arrays come out interleaved.
fn collect<T: Element + Copy>(
    arr: &Bound<'_, PyArrayDyn<T>>,
    convert: impl Fn(T) -> f32,
) -> PyResult<Vec<f32>> {
    if !arr.is_aligned() {
        // e.g. np.frombuffer() at an odd offset: reading it in place is undefined behaviour.
        let copy = arr.call_method0("copy")?.cast_into::<PyArrayDyn<T>>()?;
        return collect(&copy, convert);
    }
    let arr = arr.readonly();
    Ok(match arr.as_slice() {
        Ok(s) if arr.is_c_contiguous() => s.iter().map(|&x| convert(x)).collect(),
        _ => arr.as_array().iter().map(|&x| convert(x)).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, rate: f64, ch: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate).sin() as f32;
                std::iter::repeat_n(v, ch)
            })
            .collect()
    }

    #[test]
    fn chunked_resampling_matches_one_shot() {
        for (sr, dr, ch) in [
            (22050, 48000, 1),
            (44100, 48000, 2),
            (48000, 44100, 2),
            (96000, 48000, 1),
        ] {
            let input = sine(5000, sr as f64, ch as usize);
            let whole = Converter::new(sr, dr, ch, ch).process(input.clone());
            for frames in [1, 7, 160, 1024] {
                let mut c = Converter::new(sr, dr, ch, ch);
                let chunked: Vec<f32> = input
                    .chunks(frames * ch as usize)
                    .flat_map(|part| c.process(part.to_vec()))
                    .collect();
                assert_eq!(
                    chunked.len(),
                    whole.len(),
                    "{sr}->{dr} ch{ch} chunk{frames}"
                );
                let diff = chunked
                    .iter()
                    .zip(&whole)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0, f32::max);
                assert!(diff < 1e-5, "{sr}->{dr} ch{ch} chunk{frames}: {diff}");
            }
        }
    }

    #[test]
    fn resampled_length_tracks_rate_ratio() {
        let mut c = Converter::new(22050, 48000, 1, 1);
        let total: usize = (0..100).map(|_| c.process(vec![0.0; 2205]).len()).sum();
        // Two source frames of look-ahead are held back.
        assert!(total.abs_diff(480_000) <= 6, "{total}");
    }

    #[test]
    fn cubic_resampling_tracks_the_signal() {
        // 1 kHz at 24 kHz -> 48 kHz: compare with the exact sine at each output instant.
        let input: Vec<f32> = (0..2400)
            .map(|i| (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 24000.0).sin() as f32 * 0.5)
            .collect();
        let out = Converter::new(24000, 48000, 1, 1).process(input);
        // Past the start, where the unknown history is padded with the first frame.
        let err = out
            .iter()
            .enumerate()
            .skip(4)
            .map(|(k, &y)| {
                let exact = (2.0 * std::f64::consts::PI * 1000.0 * k as f64 / 48000.0).sin() * 0.5;
                (y as f64 - exact).abs()
            })
            .fold(0.0, f64::max);
        // Linear interpolation is off by ~4e-3 here; Catmull-Rom by ~5e-5.
        assert!(err < 2e-4, "{err}");
    }

    #[test]
    fn resampling_never_overshoots_full_scale() {
        let square: Vec<f32> = (0..400)
            .map(|i| if i / 10 % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let out = Converter::new(22050, 48000, 1, 1).process(square);
        assert!(out.iter().all(|&s| (-1.0..=MAX_SAMPLE).contains(&s)));
    }

    fn writer(channels: u16) -> Writer {
        let (producer, _) = HeapRb::<f32>::new(16).split();
        Writer {
            producer,
            converter: Converter::new(48000, 48000, channels, channels),
            epoch: 0,
            odd_byte: None,
            partial: Vec::new(),
        }
    }

    #[test]
    fn writes_need_not_hold_whole_samples_or_frames() {
        let mut w = writer(2);
        // 0x4000 = 16384 -> 0.5; split mid-sample and mid-frame.
        assert_eq!(
            w.frames(Samples::Pcm16(vec![0x00, 0x40, 0x00])),
            Vec::<f32>::new()
        );
        assert_eq!(w.frames(Samples::Pcm16(vec![0x40, 0x00])), [0.5, 0.5]);
        assert_eq!(w.frames(Samples::Float(vec![0.1, 0.2, 0.3])), [0.1, 0.2]);
        assert_eq!(w.frames(Samples::Float(vec![0.4])), [0.3, 0.4]);
        // A clear() (new epoch) drops the leftover.
        w.frames(Samples::Float(vec![0.9]));
        w.begin(1, false);
        assert_eq!(w.frames(Samples::Float(vec![0.1, 0.2])), [0.1, 0.2]);
    }

    #[test]
    fn a_drained_stream_cut_mid_sample_does_not_misalign_the_next() {
        let mut w = writer(1);
        w.frames(Samples::Pcm16(vec![0x00, 0x40, 0x00]));
        // More of the same stream: completes the sample.
        w.begin(0, false);
        assert_eq!(w.frames(Samples::Pcm16(vec![0x40])), [0.5]);
        // Cut off after an odd byte, then drained: the next stream starts clean.
        w.frames(Samples::Pcm16(vec![0x00]));
        w.begin(0, true);
        assert_eq!(w.frames(Samples::Pcm16(vec![0x00, 0x40])), [0.5]);
    }

    fn renderer(samples: &[f32], channels: usize) -> Renderer {
        let (mut producer, consumer) = HeapRb::<f32>::new(64).split();
        producer.push_slice(samples);
        Renderer::new(consumer, channels, 2)
    }

    #[test]
    fn renders_whole_frames_then_silence() {
        let mut r = renderer(&[0.1, 0.2, 0.3, 0.4, 0.5], 2);
        let mut out = [9.0f32; 8];
        assert_eq!(r.render(&mut out, || 0), 4);
        assert_eq!(out, [0.1, 0.2, 0.3, 0.4, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(r.consumed, 4);
    }

    #[test]
    fn cleared_audio_is_skipped_and_new_audio_plays() {
        // Nothing was playing: drop the cleared samples outright.
        let mut r = renderer(&[0.5, 0.5, 0.5, 0.25], 1);
        let mut out = [9.0f32; 4];
        assert_eq!(r.render(&mut out, || 3), 1);
        assert_eq!(out, [0.25, 0.0, 0.0, 0.0]);
        assert_eq!(r.consumed, 4);
    }

    #[test]
    fn clear_fades_out_playing_audio() {
        let mut r = renderer(&[0.8, 0.8, 0.8, 0.8, 0.8, 0.8], 1);
        let mut out = [0.0f32; 2];
        r.render(&mut out, || 0);
        // Playing; the rest (4 samples) gets cleared: a 2-frame ramp, then silence.
        let mut out = [9.0f32; 4];
        r.render(&mut out, || 6);
        assert_eq!(out, [0.8, 0.4, 0.0, 0.0]);
        assert_eq!(r.consumed, 6);
    }

    #[test]
    fn a_watermark_past_the_queued_audio_skips_what_arrives_later() {
        // clear() counted samples push() hasn't published yet.
        let (mut producer, consumer) = HeapRb::<f32>::new(64).split();
        producer.push_slice(&[0.5, 0.5]);
        let mut r = Renderer::new(consumer, 1, 2);
        let mut out = [9.0f32; 4];
        assert_eq!(r.render(&mut out, || 4), 0);
        producer.push_slice(&[0.5, 0.5, 0.25]);
        assert_eq!(r.render(&mut out, || 4), 1);
        assert_eq!(out, [0.25, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn resampling_keeps_channels_apart() {
        let input: Vec<f32> = (0..1000).flat_map(|_| [0.5f32, -0.5]).collect();
        let out = Converter::new(44100, 48000, 2, 2).process(input);
        assert!(out.len().is_multiple_of(2));
        assert!(
            out.as_chunks::<2>()
                .0
                .iter()
                .all(|f| (f[0] - 0.5).abs() < 1e-6 && (f[1] + 0.5).abs() < 1e-6)
        );
    }

    #[test]
    fn remaps_channels_by_position() {
        let remap_all = |src: u16, dst: u16, input: Vec<f32>| {
            Converter::new(48000, 48000, src, dst).process(input)
        };
        assert_eq!(remap_all(1, 2, vec![0.1, 0.2]), [0.1, 0.1, 0.2, 0.2]);
        // 5.1: mono feeds the front pair only, never the LFE.
        assert_eq!(remap_all(1, 6, vec![0.5]), [0.5, 0.5, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(remap_all(2, 4, vec![0.1, 0.2]), [0.1, 0.2, 0.0, 0.0]);
        assert_eq!(remap_all(2, 1, vec![0.2, 0.4, -1.0, 1.0]), [0.3, 0.0]);
        assert_eq!(remap_all(6, 2, vec![1., 2., 3., 4., 5., 6.]), [1.0, 2.0]);
    }

    #[test]
    fn negotiate_prefers_user_config_then_default() {
        use cpal::{SupportedBufferSize, SupportedStreamConfigRange};
        let range = |ch, fmt, min, max| {
            SupportedStreamConfigRange::new(ch, min, max, SupportedBufferSize::Unknown, fmt)
        };
        let default =
            SupportedStreamConfig::new(2, 48000, SupportedBufferSize::Unknown, SampleFormat::I32);
        let supported = [
            range(2, SampleFormat::I16, 8000, 192000),
            range(2, SampleFormat::F32, 44100, 48000),
            range(2, SampleFormat::I32, 48000, 48000),
            range(1, SampleFormat::F32, 16000, 16000),
        ];
        let pick = |rate, ch| {
            let c = negotiate(&supported, default, rate, ch);
            (c.sample_rate(), c.channels(), c.sample_format())
        };
        assert_eq!(pick(16000, 1), (16000, 1, SampleFormat::F32));
        assert_eq!(pick(22050, 1), (22050, 2, SampleFormat::I16));
        assert_eq!(pick(44100, 2), (44100, 2, SampleFormat::F32));
        assert_eq!(pick(7000, 1), (48000, 2, SampleFormat::I32));
    }
}
