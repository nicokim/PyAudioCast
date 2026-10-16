use pyo3::PyErr;
use pyo3::exceptions::PyRuntimeError;
use std::fmt;

#[derive(Debug)]
pub enum SpeakerError {
    DeviceNotFound(String),
    NoDefaultDevice,
    StreamError(String),
    ConfigError(String),
    IoError(String),
}

impl fmt::Display for SpeakerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpeakerError::DeviceNotFound(name) => write!(f, "Device not found: {}", name),
            SpeakerError::NoDefaultDevice => write!(f, "No default output device available"),
            SpeakerError::StreamError(msg) => write!(f, "Stream error: {}", msg),
            SpeakerError::ConfigError(msg) => write!(f, "Config error: {}", msg),
            SpeakerError::IoError(msg) => write!(f, "IO error: {}", msg),
        }
    }
}

impl SpeakerError {
    /// Prefix the message with what failed, e.g. the device id.
    pub fn context(self, what: &str) -> Self {
        match self {
            SpeakerError::StreamError(msg) => SpeakerError::StreamError(format!("{what}: {msg}")),
            SpeakerError::ConfigError(msg) => SpeakerError::ConfigError(format!("{what}: {msg}")),
            other => other,
        }
    }
}

impl From<SpeakerError> for PyErr {
    fn from(err: SpeakerError) -> PyErr {
        PyRuntimeError::new_err(err.to_string())
    }
}

impl From<cpal::Error> for SpeakerError {
    fn from(err: cpal::Error) -> Self {
        match err.kind() {
            cpal::ErrorKind::UnsupportedConfig | cpal::ErrorKind::InvalidInput => {
                SpeakerError::ConfigError(err.to_string())
            }
            _ => SpeakerError::StreamError(err.to_string()),
        }
    }
}

impl From<hound::Error> for SpeakerError {
    fn from(err: hound::Error) -> Self {
        SpeakerError::IoError(err.to_string())
    }
}
