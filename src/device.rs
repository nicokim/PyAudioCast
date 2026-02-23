use cpal::traits::{DeviceTrait, HostTrait};
use log::{debug, info, warn};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::error::SpeakerError;

/// List PipeWire/PulseAudio sinks via pactl (Linux only).
#[cfg(target_os = "linux")]
fn list_pipewire_sinks() -> Vec<String> {
    use std::process::Command;
    if let Ok(output) = Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            return stdout
                .lines()
                .filter_map(|line| {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if parts.len() >= 2 {
                        Some(parts[1].to_string())
                    } else {
                        None
                    }
                })
                .collect();
        }
    }
    debug!("pactl not available or failed");
    Vec::new()
}

#[cfg(not(target_os = "linux"))]
fn list_pipewire_sinks() -> Vec<String> {
    Vec::new()
}

/// List all available output devices.
/// Returns a list of dicts with "name", "index", and "type" keys.
/// On Linux, includes both ALSA/cpal devices and PipeWire/PulseAudio sinks.
pub fn list_output_devices_impl(py: Python<'_>) -> PyResult<Vec<Py<PyDict>>> {
    let host = cpal::default_host();
    let devices = host.output_devices().map_err(SpeakerError::from)?;

    let mut result: Vec<Py<PyDict>> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut index: usize = 0;

    // Add cpal devices
    for device in devices {
        let name = device.name().unwrap_or_else(|_| format!("Unknown-{}", index));
        debug!("cpal device [{}]: {}", index, name);
        let dict = PyDict::new(py);
        dict.set_item("name", &name)?;
        dict.set_item("index", index)?;
        dict.set_item("type", "alsa")?;
        names.push(name);
        result.push(dict.into());
        index += 1;
    }

    // Add PipeWire/PulseAudio sinks (Linux only, if pactl is available)
    for sink_name in list_pipewire_sinks() {
        if !names.iter().any(|n| n == &sink_name) {
            debug!("pipewire sink [{}]: {}", index, sink_name);
            let dict = PyDict::new(py);
            dict.set_item("name", &sink_name)?;
            dict.set_item("index", index)?;
            dict.set_item("type", "pipewire")?;
            names.push(sink_name);
            result.push(dict.into());
            index += 1;
        }
    }

    Ok(result)
}

/// Check if a sink name is a PipeWire/PulseAudio sink (Linux only).
fn is_pipewire_sink(name: &str) -> bool {
    list_pipewire_sinks().iter().any(|s| s == name)
}

/// Find a device by name (substring match) or return default.
/// On Linux, PipeWire/PulseAudio sinks are routed via PULSE_SINK + the "pulse" ALSA device.
pub fn find_device(device_name: Option<&str>) -> Result<cpal::Device, SpeakerError> {
    let host = cpal::default_host();

    match device_name {
        Some(name) => {
            // On Linux, check if it's a PipeWire/PulseAudio sink first
            if is_pipewire_sink(name) {
                info!("Routing to PipeWire sink '{}' via PULSE_SINK", name);
                std::env::set_var("PULSE_SINK", name);
                let devices = host.output_devices()?;
                for device in devices {
                    if let Ok(dev_name) = device.name() {
                        if dev_name == "pulse" {
                            debug!("Found 'pulse' ALSA device for routing");
                            return Ok(device);
                        }
                    }
                }
                return Err(SpeakerError::DeviceNotFound(
                    "pulse ALSA device not found (needed for PipeWire sink routing)".to_string(),
                ));
            }

            // Otherwise search cpal devices by substring
            debug!("Searching cpal devices for '{}'", name);
            let devices = host.output_devices()?;
            for device in devices {
                if let Ok(dev_name) = device.name() {
                    if dev_name.contains(name) {
                        info!("Found device: {}", dev_name);
                        return Ok(device);
                    }
                }
            }
            warn!("Device not found: {}", name);
            Err(SpeakerError::DeviceNotFound(name.to_string()))
        }
        None => {
            debug!("Using default output device");
            host.default_output_device()
                .ok_or(SpeakerError::NoDefaultDevice)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_default_device() {
        let _result = find_device(None);
    }

    #[test]
    fn test_find_nonexistent_device() {
        let result = find_device(Some("__nonexistent_device_12345__"));
        assert!(result.is_err());
    }
}
