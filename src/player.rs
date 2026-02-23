use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use log::{debug, info};
use numpy::PyReadonlyArray1;
use pyo3::prelude::*;
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
    ///     sample_rate: Sample rate in Hz (default: 22050).
    ///     channels: Number of channels (default: 1).
    #[new]
    #[pyo3(signature = (device=None, sample_rate=22050, channels=1))]
    fn new(device: Option<&str>, sample_rate: u32, channels: u16) -> PyResult<Self> {
        crate::init_logging();

        info!(
            "AudioPlayer::new(device={:?}, sample_rate={}, channels={})",
            device, sample_rate, channels
        );

        let cpal_device = find_device(device)?;

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
        let active = Arc::new(AtomicBool::new(false));
        let drain_signal_clone = drain_signal.clone();

        // Build output stream
        let stream = cpal_device
            .build_output_stream(
                &desired_config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
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
            active,
            sample_rate,
            channels,
        })
    }

    /// Write raw audio bytes (int16 little-endian) to the player.
    fn write(&mut self, py: Python<'_>, data: &[u8]) -> PyResult<()> {
        if !data.len().is_multiple_of(2) {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "Data length must be even (int16 samples are 2 bytes each)",
            ));
        }

        let producer = self
            .producer
            .as_mut()
            .ok_or_else(|| SpeakerError::StreamError("Player is closed".to_string()))?;

        {
            let (lock, _) = &*self.drain_signal;
            if let Ok(mut drained) = lock.lock() {
                *drained = false;
            }
        }

        let num_samples = data.len() / 2;
        debug!(
            "write: {} bytes ({} int16 samples)",
            data.len(),
            num_samples
        );

        let samples: Vec<f32> = data
            .chunks_exact(2)
            .map(|chunk| {
                let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                sample as f32 / 32768.0
            })
            .collect();

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

    /// Write a numpy int16 array to the player.
    fn write_array(&mut self, py: Python<'_>, data: PyReadonlyArray1<i16>) -> PyResult<()> {
        let producer = self
            .producer
            .as_mut()
            .ok_or_else(|| SpeakerError::StreamError("Player is closed".to_string()))?;

        {
            let (lock, _) = &*self.drain_signal;
            if let Ok(mut drained) = lock.lock() {
                *drained = false;
            }
        }

        let slice = data.as_slice()?;
        debug!("write_array: {} int16 samples", slice.len());

        let samples: Vec<f32> = slice.iter().map(|&s| s as f32 / 32768.0).collect();

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

    /// Write f32 samples directly (values should be in -1.0..1.0 range).
    fn write_f32(&mut self, py: Python<'_>, data: Vec<f32>) -> PyResult<()> {
        let producer = self
            .producer
            .as_mut()
            .ok_or_else(|| SpeakerError::StreamError("Player is closed".to_string()))?;

        {
            let (lock, _) = &*self.drain_signal;
            if let Ok(mut drained) = lock.lock() {
                *drained = false;
            }
        }

        debug!("write_f32: {} samples", data.len());

        py.allow_threads(|| {
            let mut offset = 0;
            while offset < data.len() {
                let pushed = producer.push_slice(&data[offset..]);
                offset += pushed;
                if offset < data.len() {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        });

        Ok(())
    }

    /// Block until all buffered audio has been played.
    fn drain(&self, py: Python<'_>) -> PyResult<()> {
        debug!("drain: waiting for buffer to empty");
        let drain_signal = self.drain_signal.clone();

        py.allow_threads(move || {
            let (lock, cvar) = &*drain_signal;
            let mut drained = lock.lock().unwrap();
            while !*drained {
                let result = cvar
                    .wait_timeout(drained, Duration::from_millis(100))
                    .unwrap();
                drained = result.0;
            }
        });

        debug!("drain: complete");
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
