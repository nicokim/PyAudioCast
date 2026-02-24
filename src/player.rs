use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use log::{debug, info};
use numpy::{PyReadonlyArray1, PyUntypedArrayMethods};
use pyo3::prelude::*;
use pyo3::types::PyList;
use ringbuf::{
    traits::{Consumer, Observer, Producer, Split},
    HeapRb,
};

use crate::device::find_device;
use crate::error::SpeakerError;

/// Size of the ring buffer in samples.
const RING_BUFFER_SIZE: usize = 48000 * 4; // ~4 seconds at 48kHz

// cpal::Stream is !Send+!Sync on some platforms, so we mark the pyclass as unsendable.
#[pyclass(unsendable)]
pub struct AudioPlayer {
    /// Producer side of ring buffer (Python writes here)
    producer: Option<ringbuf::HeapProd<f32>>,
    /// The cpal stream (kept alive while playing)
    _stream: Option<cpal::Stream>,
    /// Signal for drain: notified when buffer is empty
    drain_signal: Arc<(Mutex<bool>, Condvar)>,
    /// Whether the player has been interrupted (clear was called)
    interrupted: Arc<AtomicBool>,
    /// Whether the stream is active
    active: Arc<AtomicBool>,
    /// User-requested sample rate (what Python sends)
    sample_rate: u32,
    /// User-requested channels (what Python sends, e.g. 1 for mono)
    channels: u16,
    /// Actual device sample rate (may differ from user-requested)
    device_sample_rate: u32,
    /// Actual device channels (may differ from user-requested)
    device_channels: u16,
}

#[pymethods]
impl AudioPlayer {
    /// Create a new AudioPlayer.
    ///
    /// Args:
    ///     device: Device name (substring match) or None for default.
    ///     sample_rate: Sample rate in Hz, or None to auto-detect from device.
    ///     channels: Number of channels, or None to auto-detect from device.
    #[new]
    #[pyo3(signature = (device=None, sample_rate=None, channels=None))]
    fn new(
        device: Option<&str>,
        sample_rate: Option<u32>,
        channels: Option<u16>,
    ) -> PyResult<Self> {
        crate::init_logging();

        info!(
            "AudioPlayer::new(device={:?}, sample_rate={:?}, channels={:?})",
            device, sample_rate, channels
        );

        let cpal_device = find_device(device)?;

        // Auto-detect from device default config if not specified
        let default_config = cpal_device
            .default_output_config()
            .map_err(SpeakerError::from)?;

        let user_sample_rate = sample_rate.unwrap_or(default_config.sample_rate());
        let user_channels = channels.unwrap_or(default_config.channels());
        let device_channels = default_config.channels();

        // Find the best supported config: try user rate first, then fall back to device default
        let supported_configs: Vec<_> = cpal_device
            .supported_output_configs()
            .map_err(SpeakerError::from)?
            .collect();

        let (device_sample_rate, use_f32) = Self::negotiate_config(
            &supported_configs,
            user_sample_rate,
            device_channels,
            default_config.sample_rate(),
        )?;

        info!(
            "Config: user={}Hz {}ch, device={}Hz {}ch, format={} (default: {}Hz, {}ch)",
            user_sample_rate,
            user_channels,
            device_sample_rate,
            device_channels,
            if use_f32 { "f32" } else { "i16" },
            default_config.sample_rate(),
            default_config.channels()
        );

        if device_sample_rate != user_sample_rate {
            info!(
                "Will resample: {}Hz -> {}Hz",
                user_sample_rate, device_sample_rate
            );
        }
        if user_channels != device_channels {
            info!("Will upmix: {}ch -> {}ch", user_channels, device_channels);
        }

        let stream_config = StreamConfig {
            channels: device_channels,
            sample_rate: device_sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        // Create ring buffer (always f32 internally, converted to i16 in callback if needed)
        let rb = HeapRb::<f32>::new(RING_BUFFER_SIZE);
        let (producer, consumer) = rb.split();
        debug!("Ring buffer created: {} samples", RING_BUFFER_SIZE);

        let drain_signal = Arc::new((Mutex::new(false), Condvar::new()));
        let interrupted = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(false));

        let stream = if use_f32 {
            Self::build_f32_stream(
                &cpal_device,
                &stream_config,
                consumer,
                &drain_signal,
                &interrupted,
            )?
        } else {
            Self::build_i16_stream(
                &cpal_device,
                &stream_config,
                consumer,
                &drain_signal,
                &interrupted,
            )?
        };

        stream.play().map_err(SpeakerError::from)?;
        active.store(true, Ordering::SeqCst);
        info!("Stream started");

        Ok(AudioPlayer {
            producer: Some(producer),
            _stream: Some(stream),
            drain_signal,
            interrupted,
            active,
            sample_rate: user_sample_rate,
            channels: user_channels,
            device_sample_rate,
            device_channels,
        })
    }

    /// Write audio data to the player.
    ///
    /// Accepts:
    ///   - `bytes`: Raw int16 little-endian PCM data
    ///   - `numpy.ndarray`: int16, int32, float32, or float64 array
    ///   - `list[float]`: Float samples in -1.0..1.0 range
    fn write(&mut self, py: Python<'_>, data: &Bound<'_, pyo3::PyAny>) -> PyResult<()> {
        let producer = self
            .producer
            .as_mut()
            .ok_or_else(|| SpeakerError::StreamError("Player is closed".to_string()))?;

        self.interrupted.store(false, Ordering::SeqCst);
        {
            let (lock, _) = &*self.drain_signal;
            if let Ok(mut drained) = lock.lock() {
                *drained = false;
            }
        }

        let mut samples = Self::extract_samples(data)?;
        debug!("write: {} f32 samples", samples.len());

        // Resample if device rate differs from user rate
        if self.sample_rate != self.device_sample_rate {
            let before = samples.len();
            samples = Self::resample(&samples, self.sample_rate, self.device_sample_rate);
            debug!("resampled: {} -> {} samples", before, samples.len());
        }

        // Upmix channels if needed (e.g. mono -> stereo)
        if self.channels < self.device_channels {
            samples = Self::upmix(&samples, self.channels, self.device_channels);
        }

        py.detach(|| {
            let mut offset = 0;
            while offset < samples.len() {
                let pushed = producer.push_slice(&samples[offset..]);
                offset += pushed;
                if offset < samples.len() {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        });

        Ok(())
    }

    /// Block until all buffered audio has been played.
    /// Returns immediately if the player has been interrupted via `clear()`.
    fn drain(&self, py: Python<'_>) -> PyResult<()> {
        debug!("drain: waiting for buffer to empty");
        let drain_signal = self.drain_signal.clone();
        let interrupted = self.interrupted.clone();

        py.detach(move || {
            let (lock, cvar) = &*drain_signal;
            let mut drained = lock.lock().unwrap();
            while !*drained {
                if interrupted.load(Ordering::SeqCst) {
                    debug!("drain: interrupted");
                    return;
                }
                let result = cvar
                    .wait_timeout(drained, Duration::from_millis(100))
                    .unwrap();
                drained = result.0;
            }
        });

        debug!("drain: complete");
        Ok(())
    }

    /// Clear the audio buffer, stopping playback immediately.
    /// Any in-progress `drain()` call will return immediately.
    /// The audio callback will discard remaining samples and output silence.
    /// Call `write()` again to resume normal playback.
    fn clear(&self) -> PyResult<()> {
        debug!("clear: discarding buffered audio");
        self.interrupted.store(true, Ordering::SeqCst);

        // Wake up any blocked drain()
        let (lock, cvar) = &*self.drain_signal;
        if let Ok(mut drained) = lock.lock() {
            *drained = true;
            cvar.notify_all();
        }

        info!("Buffer cleared");
        Ok(())
    }

    /// Stop the player and release resources.
    fn stop(&mut self) -> PyResult<()> {
        info!("Stopping AudioPlayer");
        self.active.store(false, Ordering::SeqCst);
        self.producer = None;
        self._stream = None;
        Ok(())
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

    /// Returns whether the player is active.
    #[getter]
    fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    // Context manager support
    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
    fn __exit__(
        &mut self,
        _exc_type: Option<&Bound<'_, pyo3::types::PyAny>>,
        _exc_val: Option<&Bound<'_, pyo3::types::PyAny>>,
        _exc_tb: Option<&Bound<'_, pyo3::types::PyAny>>,
    ) -> PyResult<bool> {
        self.stop()?;
        Ok(false)
    }
}

impl AudioPlayer {
    /// Negotiate the best sample rate and format for the device.
    /// Returns (device_sample_rate, use_f32).
    fn negotiate_config(
        supported_configs: &[cpal::SupportedStreamConfigRange],
        user_rate: u32,
        channels: u16,
        default_rate: u32,
    ) -> PyResult<(u32, bool)> {
        // Check what formats/rates are supported
        let mut f32_at_user = false;
        let mut i16_at_user = false;
        let mut f32_at_default = false;
        let mut i16_at_default = false;
        let mut any_f32_rate: Option<u32> = None;
        let mut any_i16_rate: Option<u32> = None;

        for config in supported_configs {
            if config.channels() < channels {
                continue;
            }
            let supports_user =
                config.min_sample_rate() <= user_rate && config.max_sample_rate() >= user_rate;
            let supports_default = config.min_sample_rate() <= default_rate
                && config.max_sample_rate() >= default_rate;

            match config.sample_format() {
                SampleFormat::F32 => {
                    if supports_user {
                        f32_at_user = true;
                    }
                    if supports_default {
                        f32_at_default = true;
                    }
                    if any_f32_rate.is_none() {
                        any_f32_rate = Some(
                            config
                                .max_sample_rate()
                                .max(user_rate.min(config.max_sample_rate())),
                        );
                    }
                }
                SampleFormat::I16 => {
                    if supports_user {
                        i16_at_user = true;
                    }
                    if supports_default {
                        i16_at_default = true;
                    }
                    if any_i16_rate.is_none() {
                        any_i16_rate = Some(
                            config
                                .max_sample_rate()
                                .max(user_rate.min(config.max_sample_rate())),
                        );
                    }
                }
                _ => {}
            }
        }

        debug!(
            "negotiate: f32@user={}, i16@user={}, f32@default={}, i16@default={}",
            f32_at_user, i16_at_user, f32_at_default, i16_at_default
        );

        // Priority: f32 at user rate > i16 at user rate > f32 at default > i16 at default
        if f32_at_user {
            Ok((user_rate, true))
        } else if i16_at_user {
            Ok((user_rate, false))
        } else if f32_at_default {
            info!(
                "Device doesn't support {}Hz, using default {}Hz (will resample)",
                user_rate, default_rate
            );
            Ok((default_rate, true))
        } else if i16_at_default {
            info!(
                "Device doesn't support {}Hz, using default {}Hz i16 (will resample)",
                user_rate, default_rate
            );
            Ok((default_rate, false))
        } else {
            Err(SpeakerError::ConfigError(format!(
                "Device does not support {}Hz or {}Hz in f32 or i16",
                user_rate, default_rate
            ))
            .into())
        }
    }

    /// Build an f32 output stream.
    fn build_f32_stream(
        device: &cpal::Device,
        config: &StreamConfig,
        mut consumer: ringbuf::HeapCons<f32>,
        drain_signal: &Arc<(Mutex<bool>, Condvar)>,
        interrupted: &Arc<AtomicBool>,
    ) -> PyResult<cpal::Stream> {
        let drain_clone = drain_signal.clone();
        let int_clone = interrupted.clone();

        let stream = device
            .build_output_stream(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    if int_clone.load(Ordering::SeqCst) {
                        while consumer.try_pop().is_some() {}
                        data.fill(0.0);
                        return;
                    }
                    let mut all_silence = true;
                    for sample in data.iter_mut() {
                        if let Some(s) = consumer.try_pop() {
                            *sample = s;
                            all_silence = false;
                        } else {
                            *sample = 0.0;
                        }
                    }
                    if all_silence && consumer.is_empty() {
                        let (lock, cvar) = &*drain_clone;
                        if let Ok(mut drained) = lock.lock() {
                            *drained = true;
                            cvar.notify_all();
                        }
                    }
                },
                |err| log::error!("Stream error: {}", err),
                None,
            )
            .map_err(SpeakerError::from)?;
        Ok(stream)
    }

    /// Build an i16 output stream (reads f32 from ring buffer, converts to i16).
    fn build_i16_stream(
        device: &cpal::Device,
        config: &StreamConfig,
        mut consumer: ringbuf::HeapCons<f32>,
        drain_signal: &Arc<(Mutex<bool>, Condvar)>,
        interrupted: &Arc<AtomicBool>,
    ) -> PyResult<cpal::Stream> {
        let drain_clone = drain_signal.clone();
        let int_clone = interrupted.clone();

        let stream = device
            .build_output_stream(
                config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    if int_clone.load(Ordering::SeqCst) {
                        while consumer.try_pop().is_some() {}
                        data.fill(0);
                        return;
                    }
                    let mut all_silence = true;
                    for sample in data.iter_mut() {
                        if let Some(s) = consumer.try_pop() {
                            *sample = (s * 32767.0).clamp(-32768.0, 32767.0) as i16;
                            all_silence = false;
                        } else {
                            *sample = 0;
                        }
                    }
                    if all_silence && consumer.is_empty() {
                        let (lock, cvar) = &*drain_clone;
                        if let Ok(mut drained) = lock.lock() {
                            *drained = true;
                            cvar.notify_all();
                        }
                    }
                },
                |err| log::error!("Stream error: {}", err),
                None,
            )
            .map_err(SpeakerError::from)?;
        Ok(stream)
    }

    /// Upmix audio from fewer channels to more (e.g. mono -> stereo).
    /// For mono->stereo, duplicates each sample. For other cases, duplicates
    /// each frame's samples to fill the target channel count.
    fn upmix(samples: &[f32], src_channels: u16, dst_channels: u16) -> Vec<f32> {
        if src_channels >= dst_channels {
            return samples.to_vec();
        }
        let src_ch = src_channels as usize;
        let dst_ch = dst_channels as usize;
        let num_frames = samples.len() / src_ch;
        let mut output = Vec::with_capacity(num_frames * dst_ch);
        for frame in 0..num_frames {
            let base = frame * src_ch;
            for ch in 0..dst_ch {
                // Copy from source channel, wrapping around if dst > src
                output.push(samples[base + (ch % src_ch)]);
            }
        }
        output
    }

    /// Linear interpolation resampling from src_rate to dst_rate.
    fn resample(samples: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
        if src_rate == dst_rate {
            return samples.to_vec();
        }
        let ratio = src_rate as f64 / dst_rate as f64;
        let out_len = ((samples.len() as f64) / ratio).ceil() as usize;
        let mut output = Vec::with_capacity(out_len);
        for i in 0..out_len {
            let src_pos = i as f64 * ratio;
            let idx = src_pos as usize;
            let frac = (src_pos - idx as f64) as f32;
            let s0 = samples[idx.min(samples.len() - 1)];
            let s1 = samples[(idx + 1).min(samples.len() - 1)];
            output.push(s0 + frac * (s1 - s0));
        }
        output
    }

    /// Convert any supported Python audio data to Vec<f32>.
    fn extract_samples(data: &Bound<'_, pyo3::PyAny>) -> PyResult<Vec<f32>> {
        // bytes → int16 LE PCM
        if let Ok(bytes) = data.extract::<Vec<u8>>() {
            if !bytes.len().is_multiple_of(2) {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "Data length must be even (int16 samples are 2 bytes each)",
                ));
            }
            debug!("extract_samples: {} bytes as int16 LE", bytes.len());
            return Ok(bytes
                .chunks_exact(2)
                .map(|chunk| {
                    let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                    sample as f32 / 32768.0
                })
                .collect());
        }

        // numpy array
        if let Ok(arr) = data.cast::<numpy::PyUntypedArray>() {
            let dtype = arr.dtype();
            let dtype_str = dtype.to_string();

            if dtype_str.contains("float32") {
                let arr: PyReadonlyArray1<f32> = data.extract()?;
                let slice = arr.as_slice()?;
                debug!("extract_samples: numpy float32, {} samples", slice.len());
                return Ok(slice.to_vec());
            } else if dtype_str.contains("int16") {
                let arr: PyReadonlyArray1<i16> = data.extract()?;
                let slice = arr.as_slice()?;
                debug!("extract_samples: numpy int16, {} samples", slice.len());
                return Ok(slice.iter().map(|&s| s as f32 / 32768.0).collect());
            } else if dtype_str.contains("float64") {
                let arr: PyReadonlyArray1<f64> = data.extract()?;
                let slice = arr.as_slice()?;
                debug!("extract_samples: numpy float64, {} samples", slice.len());
                return Ok(slice.iter().map(|&s| s as f32).collect());
            } else if dtype_str.contains("int32") {
                let arr: PyReadonlyArray1<i32> = data.extract()?;
                let slice = arr.as_slice()?;
                debug!("extract_samples: numpy int32, {} samples", slice.len());
                return Ok(slice.iter().map(|&s| s as f32 / 2147483648.0).collect());
            } else {
                return Err(pyo3::exceptions::PyTypeError::new_err(format!(
                    "Unsupported numpy dtype: {}. Expected float32, float64, int16, or int32",
                    dtype_str
                )));
            }
        }

        // list[float]
        if let Ok(list) = data.cast::<PyList>() {
            let samples: Vec<f32> = list.extract()?;
            debug!("extract_samples: list[float], {} samples", samples.len());
            return Ok(samples);
        }

        Err(pyo3::exceptions::PyTypeError::new_err(
            "Expected bytes, numpy array (int16/int32/float32/float64), or list[float]",
        ))
    }
}
