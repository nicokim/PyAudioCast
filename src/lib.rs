mod device;
mod error;
mod player;

use std::sync::Once;

use log::{debug, info};
use pyo3::prelude::*;

use crate::device::list_output_devices_impl;
use crate::error::SpeakerError;
use crate::player::AudioPlayer;

static INIT: Once = Once::new();

/// Initialize logging from PYAUDIOCAST_LOG env var and suppress ALSA/JACK noise.
pub(crate) fn init_logging() {
    INIT.call_once(|| {
        let log_enabled = std::env::var("PYAUDIOCAST_LOG").is_ok();

        env_logger::Builder::new()
            .filter_level(log::LevelFilter::Warn)
            .parse_env("PYAUDIOCAST_LOG")
            .init();

        if !log_enabled {
            suppress_backend_noise();
        }
    });
}

/// Suppress noisy ALSA/JACK error messages without affecting Python's stderr.
/// ALSA: set a no-op error handler (persistent, does not touch stderr).
/// JACK: set JACK_NO_START_SERVER=1 to prevent JACK from trying to start a server.
#[cfg(target_os = "linux")]
fn suppress_backend_noise() {
    extern "C" {
        fn snd_lib_error_set_handler(handler: *const std::ffi::c_void) -> std::ffi::c_int;
        fn pyspeaker_silent_alsa_handler();
    }

    // Suppress ALSA error handler
    unsafe {
        snd_lib_error_set_handler(pyspeaker_silent_alsa_handler as *const std::ffi::c_void);
    }

    // Prevent JACK from trying to start a server (suppresses connection messages)
    if std::env::var("JACK_NO_START_SERVER").is_err() {
        std::env::set_var("JACK_NO_START_SERVER", "1");
    }
}

#[cfg(not(target_os = "linux"))]
fn suppress_backend_noise() {}

/// List all available audio output devices.
#[pyfunction]
fn list_output_devices(py: Python<'_>) -> PyResult<Vec<Py<pyo3::types::PyDict>>> {
    init_logging();
    debug!("Listing output devices");
    let devices = list_output_devices_impl(py)?;
    info!("Found {} output device(s)", devices.len());
    Ok(devices)
}

/// Play a WAV file to an output device.
#[pyfunction]
#[pyo3(signature = (path, device=None))]
fn play_file(py: Python<'_>, path: &str, device: Option<&str>) -> PyResult<()> {
    init_logging();
    info!("play_file: path={}, device={:?}", path, device);

    let cpal_device = device::find_device(device)?;

    let reader = hound::WavReader::open(path).map_err(SpeakerError::from)?;
    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let channels = spec.channels;

    debug!(
        "WAV: {}Hz, {}ch, {:?}, {}bps",
        sample_rate, channels, spec.sample_format, spec.bits_per_sample
    );

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .into_samples::<i32>()
            .filter_map(|s| s.ok())
            .map(|s| {
                let max_val = (1i64 << (spec.bits_per_sample - 1)) as f32;
                s as f32 / max_val
            })
            .collect(),
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .filter_map(|s| s.ok())
            .collect(),
    };

    debug!("Loaded {} samples", samples.len());

    let config = cpal::StreamConfig {
        channels,
        sample_rate,
        buffer_size: cpal::BufferSize::Default,
    };

    let samples = std::sync::Arc::new(samples);
    let position = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let samples_clone = samples.clone();
    let position_clone = position.clone();
    let finished_clone = finished.clone();

    use cpal::traits::{DeviceTrait, StreamTrait};

    let stream = cpal_device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                for sample in data.iter_mut() {
                    let pos = position_clone.load(std::sync::atomic::Ordering::Relaxed);
                    if pos < samples_clone.len() {
                        *sample = samples_clone[pos];
                        position_clone.store(pos + 1, std::sync::atomic::Ordering::Relaxed);
                    } else {
                        *sample = 0.0;
                        finished_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            },
            |err| log::error!("Stream error: {}", err),
            None,
        )
        .map_err(SpeakerError::from)?;

    stream.play().map_err(SpeakerError::from)?;
    info!("Playback started");

    py.detach(|| {
        while !finished.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    });

    info!("Playback finished");
    Ok(())
}

/// PySpeaker: Cross-platform audio output for Python.
#[pymodule]
fn _pyaudiocast(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(list_output_devices, m)?)?;
    m.add_function(wrap_pyfunction!(play_file, m)?)?;
    m.add_class::<AudioPlayer>()?;
    Ok(())
}
