use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
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
    /// Configured sample rate
    sample_rate: u32,
    /// Configured channels
    channels: u16,
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

        let sample_rate = sample_rate.unwrap_or(default_config.sample_rate().0);
        let channels = channels.unwrap_or(default_config.channels());

        info!(
            "Using config: {}Hz, {}ch (device default: {}Hz, {}ch)",
            sample_rate,
            channels,
            default_config.sample_rate().0,
            default_config.channels()
        );

        let desired_config = StreamConfig {
            channels,
            sample_rate: SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        // Determine the best sample format to use
        let supported_configs = cpal_device
            .supported_output_configs()
            .map_err(SpeakerError::from)?;

        let mut supports_f32 = false;
        let mut supports_i16 = false;
        for config in supported_configs {
            if config.min_sample_rate().0 <= sample_rate
                && config.max_sample_rate().0 >= sample_rate
                && config.channels() >= channels
            {
                match config.sample_format() {
                    SampleFormat::F32 => supports_f32 = true,
                    SampleFormat::I16 => supports_i16 = true,
                    _ => {}
                }
            }
        }

        debug!(
            "Device supports: f32={}, i16={}",
            supports_f32, supports_i16
        );

        if !supports_f32 && !supports_i16 {
            return Err(SpeakerError::ConfigError(format!(
                "Device does not support {}Hz {}ch in f32 or i16",
                sample_rate, channels
            ))
            .into());
        }

        // Create ring buffer
        let rb = HeapRb::<f32>::new(RING_BUFFER_SIZE);
        let (producer, mut consumer) = rb.split();
        debug!("Ring buffer created: {} samples", RING_BUFFER_SIZE);

        let drain_signal = Arc::new((Mutex::new(false), Condvar::new()));
        let interrupted = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(false));
        let drain_signal_clone = drain_signal.clone();
        let interrupted_clone = interrupted.clone();

        // Build output stream
        let stream = cpal_device
            .build_output_stream(
                &desired_config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    // If interrupted, discard all buffered samples and output silence
                    if interrupted_clone.load(Ordering::SeqCst) {
                        while consumer.try_pop().is_some() {}
                        for sample in data.iter_mut() {
                            *sample = 0.0;
                        }
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
                        let (lock, cvar) = &*drain_signal_clone;
                        if let Ok(mut drained) = lock.lock() {
                            *drained = true;
                            cvar.notify_all();
                        }
                    }
                },
                move |err| {
                    log::error!("Stream callback error: {}", err);
                },
                None,
            )
            .map_err(SpeakerError::from)?;

        stream.play().map_err(SpeakerError::from)?;
        active.store(true, Ordering::SeqCst);
        info!("Stream started");

        Ok(AudioPlayer {
            producer: Some(producer),
            _stream: Some(stream),
            drain_signal,
            interrupted,
            active,
            sample_rate,
            channels,
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

        let samples = Self::extract_samples(data)?;
        debug!("write: {} f32 samples", samples.len());

        py.allow_threads(|| {
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

        py.allow_threads(move || {
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
        if let Ok(arr) = data.downcast::<numpy::PyUntypedArray>() {
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
        if let Ok(list) = data.downcast::<PyList>() {
            let samples: Vec<f32> = list.extract()?;
            debug!("extract_samples: list[float], {} samples", samples.len());
            return Ok(samples);
        }

        Err(pyo3::exceptions::PyTypeError::new_err(
            "Expected bytes, numpy array (int16/int32/float32/float64), or list[float]",
        ))
    }
}
