mod device;
mod error;
mod player;

use std::path::PathBuf;
use std::sync::Once;

use log::{debug, info, warn};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::error::SpeakerError;
use crate::player::{AudioPlayer, MAX_SAMPLE, Samples, sanitize};

static INIT: Once = Once::new();

/// Initialize logging from the PYAUDIOCAST_LOG env var (env_logger filter syntax).
pub(crate) fn init_logging() {
    INIT.call_once(|| {
        // try_init: another extension in the process may already own the global logger.
        let _ = env_logger::Builder::new()
            .filter_level(log::LevelFilter::Warn)
            .parse_env("PYAUDIOCAST_LOG")
            .try_init();
    });
}

/// List all available audio output devices.
#[pyfunction]
fn list_output_devices(py: Python<'_>) -> PyResult<Vec<Bound<'_, PyDict>>> {
    init_logging();
    debug!("Listing output devices");
    let devices = py.detach(device::list_output_devices)?;
    info!("Found {} output device(s)", devices.len());
    devices
        .into_iter()
        .enumerate()
        .map(|(index, d)| {
            let dict = PyDict::new(py);
            dict.set_item("name", d.name)?;
            dict.set_item("id", d.id)?;
            dict.set_item("index", index)?;
            dict.set_item("type", d.kind)?;
            Ok(dict)
        })
        .collect()
}

/// Play a WAV file to an output device. Blocks until playback is done.
#[pyfunction]
#[pyo3(signature = (path, device=None))]
fn play_file(py: Python<'_>, path: PathBuf, device: Option<&str>) -> PyResult<()> {
    init_logging();
    info!("play_file: path={}, device={:?}", path.display(), device);

    // Off the GIL: opening a FIFO blocks until its writer shows up.
    let reader = py
        .detach(|| hound::WavReader::open(&path))
        .map_err(SpeakerError::from)?;
    let spec = reader.spec();
    debug!(
        "WAV: {}Hz, {}ch, {:?}, {}bps",
        spec.sample_rate, spec.channels, spec.sample_format, spec.bits_per_sample
    );
    let player = AudioPlayer::new(py, device, Some(spec.sample_rate), Some(spec.channels))?;

    // Stream the file in chunks instead of decoding it all up front.
    let chunk = 4096 * spec.channels as usize;
    let mut decode_error = None;
    let result = py.detach(|| {
        let mut buf = Vec::with_capacity(chunk);
        let mut push = |s: Result<f32, hound::Error>| -> PyResult<bool> {
            match s {
                Ok(s) => buf.push(s),
                // A truncated file, or one written through a pipe (sox), declares more data
                // than it has: that's the end.
                Err(e) if is_short_read(&e) => {
                    warn!("play_file: file ends before its declared length");
                    return Ok(false);
                }
                Err(e) => {
                    decode_error = Some(e);
                    return Ok(false);
                }
            }
            if buf.len() == chunk {
                let full = std::mem::replace(&mut buf, Vec::with_capacity(chunk));
                player.push(Samples::Float(full), 0)?;
            }
            Ok(true)
        };
        match spec.sample_format {
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
                for s in reader.into_samples::<i32>() {
                    // 32-bit: i32::MAX * 2^-31 rounds to 1.0 in f32.
                    if !push(s.map(|s| (s as f32 * scale).min(MAX_SAMPLE)))? {
                        break;
                    }
                }
            }
            hound::SampleFormat::Float => {
                for s in reader.into_samples::<f32>() {
                    if !push(s.map(sanitize))? {
                        break;
                    }
                }
            }
        }
        // A trailing partial frame (truncated file) stays behind in the writer.
        player.push(Samples::Float(buf), 0)
    });
    // Play everything that decoded, even when the file is corrupt further on.
    let result = result.and_then(|()| player.drain(py));
    player.stop(py);
    result?;
    if let Some(e) = decode_error {
        return Err(SpeakerError::from(e).into());
    }
    info!("Playback finished");
    Ok(())
}

/// hound reports reading past the end of the file as ErrorKind::Other ("Failed to read
/// enough bytes"), which the OS never produces.
fn is_short_read(e: &hound::Error) -> bool {
    use std::io::ErrorKind::{Other, UnexpectedEof};
    matches!(e, hound::Error::IoError(e) if matches!(e.kind(), Other | UnexpectedEof))
}

/// PyAudioCast: Cross-platform audio output for Python.
#[pymodule]
fn _pyaudiocast(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_function(wrap_pyfunction!(list_output_devices, m)?)?;
    m.add_function(wrap_pyfunction!(play_file, m)?)?;
    m.add_class::<AudioPlayer>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_truncated_wav_reads_as_short() {
        let mut wav = Vec::new();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::new(std::io::Cursor::new(&mut wav), spec).unwrap();
        for _ in 0..100 {
            w.write_sample(0i16).unwrap();
        }
        w.finalize().unwrap();
        wav.truncate(wav.len() - 51);
        let reader = hound::WavReader::new(std::io::Cursor::new(wav)).unwrap();
        let err = reader.into_samples::<i32>().find_map(Result::err).unwrap();
        assert!(is_short_read(&err), "{err:?}");
    }
}
