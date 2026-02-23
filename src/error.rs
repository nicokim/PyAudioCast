use pyo3::exceptions::PyRuntimeError;
use pyo3::PyErr;
use std::fmt;

#[derive(Debug)]
pub enum SpeakerError {
    DeviceNotFound(String),
    NoDefaultDevice,
    StreamError(String),
    ConfigError(String),
    IoError(String),
    BufferFull,
}

impl fmt::Display for SpeakerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpeakerError::DeviceNotFound(name) => write!(f, "Device not found: {}", name),
            SpeakerError::NoDefaultDevice => write!(f, "No default output device available"),
            SpeakerError::StreamError(msg) => write!(f, "Stream error: {}", msg),
            SpeakerError::ConfigError(msg) => write!(f, "Config error: {}", msg),
            SpeakerError::IoError(msg) => write!(f, "IO error: {}", msg),
            SpeakerError::BufferFull => write!(f, "Audio buffer is full"),
        }
    }
}

impl From<SpeakerError> for PyErr {
    fn from(err: SpeakerError) -> PyErr {
        PyRuntimeError::new_err(err.to_string())
    }
}

impl From<cpal::DevicesError> for SpeakerError {
    fn from(err: cpal::DevicesError) -> Self {
        SpeakerError::StreamError(err.to_string())
    }
}

impl From<cpal::DeviceNameError> for SpeakerError {
    fn from(err: cpal::DeviceNameError) -> Self {
        SpeakerError::StreamError(err.to_string())
    }
}

impl From<cpal::DefaultStreamConfigError> for SpeakerError {
    fn from(err: cpal::DefaultStreamConfigError) -> Self {
        SpeakerError::ConfigError(err.to_string())
    }
}

impl From<cpal::BuildStreamError> for SpeakerError {
    fn from(err: cpal::BuildStreamError) -> Self {
        SpeakerError::StreamError(err.to_string())
    }
}

impl From<cpal::PlayStreamError> for SpeakerError {
    fn from(err: cpal::PlayStreamError) -> Self {
        SpeakerError::StreamError(err.to_string())
    }
}

impl From<cpal::SupportedStreamConfigsError> for SpeakerError {
    fn from(err: cpal::SupportedStreamConfigsError) -> Self {
        SpeakerError::ConfigError(err.to_string())
    }
}

impl From<std::io::Error> for SpeakerError {
    fn from(err: std::io::Error) -> Self {
        SpeakerError::IoError(err.to_string())
    }
}

impl From<hound::Error> for SpeakerError {
    fn from(err: hound::Error) -> Self {
        SpeakerError::IoError(err.to_string())
    }
}
