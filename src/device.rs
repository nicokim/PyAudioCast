use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard, PoisonError};

use cpal::traits::{DeviceTrait, HostTrait};
use log::{debug, info, warn};

use crate::error::SpeakerError;

/// Prefix of PipeWire/PulseAudio sink ids, which could otherwise clash with ALSA ids.
const SINK_PREFIX: &str = "pipewire:";

/// An output device as listed to Python.
pub struct DeviceInfo {
    pub name: String,
    pub id: String,
    /// "pipewire" for sinks, otherwise the cpal host ("alsa", "wasapi", "coreaudio", ...).
    pub kind: String,
}

/// Where to open an output stream.
pub struct OutputTarget {
    pub device: cpal::Device,
    /// PipeWire/PulseAudio sink to route `device` to, as (environment variable, sink name).
    pub sink: Option<(&'static str, String)>,
}

impl OutputTarget {
    fn direct(device: cpal::Device) -> Self {
        OutputTarget { device, sink: None }
    }
}

/// Device lookups and opens read the environment (alsa-lib, libpulse) while `SinkGuard`
/// changes it: hold this for both.
static OPEN: Mutex<()> = Mutex::new(());

pub fn open_lock() -> MutexGuard<'static, ()> {
    OPEN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The platform's audio host. Unlike cpal::default_host(), doesn't panic on a broken
/// ALSA configuration.
fn host() -> Result<cpal::Host, SpeakerError> {
    let id = cpal::available_hosts()
        .into_iter()
        .next()
        .ok_or_else(|| SpeakerError::StreamError("No audio host available".into()))?;
    Ok(cpal::host_from_id(id)?)
}

/// Human-readable name, e.g. "PulseAudio Sound Server". Not unique.
fn device_name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_string())
}

/// List PipeWire/PulseAudio sinks via pactl (Linux only).
#[cfg(target_os = "linux")]
fn list_pipewire_sinks() -> Vec<String> {
    match std::process::Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output()
    {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split('\t').nth(1).map(str::to_string))
            .collect(),
        _ => {
            debug!("pactl not available or failed");
            Vec::new()
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn list_pipewire_sinks() -> Vec<String> {
    Vec::new()
}

/// List all output devices: cpal devices, then (on Linux) PipeWire/PulseAudio sinks.
pub fn list_output_devices() -> Result<Vec<DeviceInfo>, SpeakerError> {
    let _open = open_lock();
    let host = host()?;
    let kind = host.id().to_string();
    let mut result: Vec<DeviceInfo> = Vec::new();
    for device in host.output_devices()? {
        result.push(DeviceInfo {
            id: device.id().map(|i| i.id().to_string()).unwrap_or_default(),
            name: device_name(&device).unwrap_or_default(),
            kind: kind.clone(),
        });
    }
    for sink in list_pipewire_sinks() {
        result.push(DeviceInfo {
            id: format!("{SINK_PREFIX}{sink}"),
            name: sink,
            kind: "pipewire".into(),
        });
    }
    debug!("{} output device(s)", result.len());
    Ok(result)
}

/// (bare id, "host:id", name, device)
type Listed = (String, String, String, cpal::Device);

/// The usbstream plugin (Tascam US-122L/US-144 only) lists one PCM per sound card, named
/// after the card: keep it out of name matches, or a card's name would select it.
// ponytail: one known offender; prefer hw/plughw PCMs if other card-named plugins turn up.
fn by_name(d: &Listed) -> bool {
    !d.0.starts_with("usbstream:")
}

/// Find an output device. In order: the default device for None/"", a sink id from
/// list_output_devices() ("pipewire:<sink>"), an exact device id (e.g. "pulse", or
/// "alsa:pulse") or name, an exact PipeWire/PulseAudio sink name, then a case-insensitive
/// substring of a sink name, device id or device name.
///
/// Sinks are preferred over ALSA hardware PCMs: on a PipeWire desktop those are held by the
/// sound server and can't be opened. They're routed through the "pulse" (or "pipewire") ALSA
/// device, see `SinkGuard`.
pub fn find_device(query: Option<&str>) -> Result<OutputTarget, SpeakerError> {
    let _open = open_lock();
    let host = host()?;
    let query = query.map(str::trim).filter(|q| !q.is_empty());
    let Some(query) = query else {
        debug!("Using default output device");
        let device = host
            .default_output_device()
            .ok_or(SpeakerError::NoDefaultDevice)?;
        return Ok(OutputTarget::direct(device));
    };

    let devices: Vec<Listed> = host
        .output_devices()?
        .map(|d| {
            let id = d.id().ok();
            let bare = id.as_ref().map(|i| i.id().to_string()).unwrap_or_default();
            let full = id.map(|i| i.to_string()).unwrap_or_default();
            (bare, full, device_name(&d).unwrap_or_default(), d)
        })
        .collect();

    if let Some(sink) = query.strip_prefix(SINK_PREFIX)
        && list_pipewire_sinks().iter().any(|s| s == sink)
    {
        return route(sink, &devices);
    }

    if let Some(d) = devices
        .iter()
        .find(|d| d.0 == query || d.1 == query || (d.2 == query && by_name(d)))
    {
        info!("Found device: {}", d.0);
        return Ok(OutputTarget::direct(d.3.clone()));
    }

    let q = query.to_lowercase();
    let sinks = list_pipewire_sinks();
    let sink = sinks
        .iter()
        .find(|s| s.as_str() == query)
        .or_else(|| sinks.iter().find(|s| s.to_lowercase().contains(&q)));
    if let Some(sink) = sink {
        return route(sink, &devices);
    }

    let mut matches = devices.into_iter().filter(|d| {
        by_name(d) && (d.0.to_lowercase().contains(&q) || d.2.to_lowercase().contains(&q))
    });
    let Some((bare, _, name, device)) = matches.next() else {
        warn!("Device not found: {}", query);
        return Err(SpeakerError::DeviceNotFound(query.into()));
    };
    let others: Vec<_> = matches.map(|m| m.0).collect();
    if others.is_empty() {
        info!("Found device: {} ({})", name, bare);
    } else {
        info!(
            "'{}' matches {} ({}) and also {:?}; pass an id from list_output_devices() to pick one",
            query, name, bare, others
        );
    }
    Ok(OutputTarget::direct(device))
}

/// Route to a PipeWire/PulseAudio sink through the pulse (or pipewire) ALSA device.
fn route(sink: &str, devices: &[Listed]) -> Result<OutputTarget, SpeakerError> {
    // libpulse takes the sink from PULSE_SINK, the PipeWire ALSA plugin from PIPEWIRE_NODE.
    let router = [("pulse", "PULSE_SINK"), ("pipewire", "PIPEWIRE_NODE")]
        .into_iter()
        .find_map(|(id, var)| devices.iter().find(|d| d.0 == id).map(|d| (d, var)));
    let Some((router, var)) = router else {
        return Err(SpeakerError::DeviceNotFound(format!(
            "{} (a PipeWire/PulseAudio sink, but neither the pulse nor the pipewire ALSA \
             device exists to route to it)",
            sink
        )));
    };
    info!("Routing to sink '{}' via the {} device", sink, router.0);
    Ok(OutputTarget {
        device: router.3.clone(),
        sink: Some((var, sink.to_string())),
    })
}

/// Points the sound server's ALSA plugin at a sink while the device is opened, and restores
/// the variable on drop. The plugin reads it when it connects, so the stream keeps its sink
/// afterwards and the override doesn't leak into later opens (ours or other libraries').
pub struct SinkGuard {
    var: &'static str,
    previous: Option<OsString>,
}

impl SinkGuard {
    /// Call with `open_lock()` and the GIL held.
    pub fn set(var: &'static str, sink: &str) -> Self {
        let previous = std::env::var_os(var);
        // SAFETY: the plugins only take the sink from the environment. Our own lookups and
        // opens are serialized by `open_lock()`, and the GIL keeps Python code (os.environ,
        // subprocess) out meanwhile. Other native threads reading the environment at the
        // same time can't be ruled out.
        unsafe { std::env::set_var(var, sink) };
        SinkGuard { var, previous }
    }
}

impl Drop for SinkGuard {
    fn drop(&mut self) {
        // SAFETY: see `SinkGuard::set`.
        match self.previous.take() {
            Some(previous) => unsafe { std::env::set_var(self.var, previous) },
            None => unsafe { std::env::remove_var(self.var) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_default_device() {
        let Some(default) = host().unwrap().default_output_device() else {
            return;
        };
        for query in [None, Some(""), Some("  ")] {
            let found = find_device(query).unwrap().device;
            assert_eq!(found.id().unwrap(), default.id().unwrap(), "{query:?}");
        }
    }

    #[test]
    fn test_find_nonexistent_device() {
        assert!(find_device(Some("__nonexistent_device_12345__")).is_err());
    }

    /// Hardware-independent: ALSA's own config lists devices (e.g. "null") even without cards.
    #[test]
    fn every_listed_device_is_selectable_by_id() {
        let listed = list_output_devices().unwrap();
        let routable = listed
            .iter()
            .any(|d| d.kind != "pipewire" && (d.id == "pulse" || d.id == "pipewire"));
        for (i, d) in listed.iter().enumerate() {
            let found = find_device(Some(&d.id));
            if let Some(sink) = d.id.strip_prefix(SINK_PREFIX) {
                if routable {
                    assert_eq!(found.unwrap().sink.unwrap().1, sink);
                }
                continue;
            }
            let found = found.unwrap();
            let id = found.device.id().unwrap();
            assert_eq!(id.id(), d.id);
            assert!(found.sink.is_none());
            // The "host:id" form too, once (each lookup re-enumerates every device).
            if i == 0 {
                let again = find_device(Some(&id.to_string())).unwrap();
                assert_eq!(again.device.id().unwrap(), id);
            }
        }
    }
}
