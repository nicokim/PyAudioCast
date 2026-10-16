---
type: subsystem
title: Device Discovery and Selection
description: How pyaudiocast picks the cpal host, lists output devices and PipeWire/PulseAudio sinks with their ids, names and types, and resolves a device= query to an output target through an ordered set of exact and substring matching rules.
tags: [devices, cpal, alsa, pipewire, pulseaudio, pactl, device-selection]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-16T04:21:31.514Z
sources:
  - id: openwiki-source-c1d203a608ca6409879479d1
    resource: repo://src/device.rs
  - id: openwiki-source-95f84abff7e2ed283d50b3fb
    resource: repo://src/error.rs
  - id: openwiki-source-ea5f93364eb7225247c2fdd1
    resource: repo://tests/test_devices.py
generated: { by: "claude-code", at: "2026-10-16T04:21:31.514Z" }
---

# Device Discovery and Selection

All device handling lives in `src/device.rs`. It answers two questions:
*which output devices exist* (`list_output_devices`) and *which one does this
`device=` string mean* (`find_device`). Both are called with the GIL released —
from `pyaudiocast.list_output_devices()` and from the `AudioPlayer`
constructor (and therefore `play_file`). Opening a matched sink is a separate
mechanism, described in [PipeWire/PulseAudio Sink Routing](pipewire-sink-routing.md).

## Choosing the host

Every listing and lookup starts by building the platform's audio host with
`host()`: the **first host cpal reports as available**, created with
`cpal::host_from_id`. This deliberately avoids `cpal::default_host()`, which
panics on a broken ALSA configuration. If cpal reports no host at all the call
fails with `RuntimeError: Stream error: No audio host available`.

Because cpal is built without optional host features, that host is ALSA on
Linux, WASAPI on Windows and CoreAudio on macOS. Its lowercase id (`"alsa"`,
`"wasapi"`, `"coreaudio"`) becomes the `type` of every cpal device.

## Listing devices

`list_output_devices()` holds the global open lock (see
[Sink Routing](pipewire-sink-routing.md#locking)) and builds the list in two
parts:

1. **cpal output devices**, in the host's order:
   - `id` — the device's bare cpal id, e.g. `"pulse"` or
     `"hw:CARD=Generic,DEV=0"` (empty if cpal cannot report one);
   - `name` — the device description's name, e.g. "PulseAudio Sound Server"
     (human-readable, **not unique**);
   - `type` — the host id.
2. **PipeWire/PulseAudio sinks** (Linux only), from
   `pactl list short sinks`:
   - `id` — `"pipewire:" + <sink name>`;
   - `name` — the sink name;
   - `type` — `"pipewire"` (for PulseAudio sinks too).

The sink list is the second tab-separated column of `pactl`'s output. If
`pactl` is not installed or exits with an error, no sinks are listed (logged at
debug level). On non-Linux targets the sink lister is compiled to return an
empty list. The `pipewire:` prefix exists because a sink name could otherwise
collide with an ALSA device id.

`src/lib.rs` turns each entry into a dict and adds `index`, the position in
the list. Device `index` values are not stable across calls (devices and sinks
come and go); pass `id` to select a device.

## Resolving a `device=` query

`find_device(query)` holds the open lock, trims the query, and returns an
`OutputTarget`. The first rule that matches wins:

1. **No query.** `None`, `""` or whitespace only → the host's default output
   device. If there is none: `No default output device available`.
2. **Prefixed sink id.** The query starts with `pipewire:` and the remainder
   is exactly the name of a sink `pactl` lists → route to that sink.
3. **Exact device match** (case-sensitive), checked per device in enumeration
   order: the bare id (`"pulse"`), the `host:id` form (`"alsa:pulse"`), or the
   device name. Name matches skip `usbstream:` devices (see below). → direct
   target.
4. **Sink name.** `pactl` is asked again; an exact sink name wins, otherwise
   the first sink whose name contains the query case-insensitively → route to
   that sink.
5. **Device substring.** Devices whose bare id or name contains the query
   case-insensitively, again skipping `usbstream:` devices. The first match in
   enumeration order is used; if more devices match, an info-level log lists
   them and suggests passing an id from `list_output_devices()`.
6. **Nothing matched.** Logged at warn level, then
   `RuntimeError: Device not found: <query>`.

Consequences worth knowing:

- **Exact beats fuzzy, devices beat sinks for exact names, sinks beat devices
  for substrings.** A sink whose name equals an ALSA device id or name is
  shadowed by rule 3; its `pipewire:` id (rule 2) still reaches it.
- **Why sinks are preferred over substring-matched devices:** on a PipeWire
  desktop the ALSA `hw:` PCMs are held by the sound server and cannot be
  opened, so a fuzzy query like `"usb"` should land on the server's sink rather
  than the busy hardware PCM.
- **The `host:id` form is exact only** — substrings are tried against bare ids
  and names, not against `host:id`.
- **A `pipewire:` query for a sink that isn't listed** (for example because
  `pactl` is unavailable) falls through to the remaining rules and usually ends
  in `Device not found`.
- **Every lookup re-enumerates** the host's devices and may run `pactl` twice,
  so resolving a device costs a few process spawns on Linux.

### The `usbstream` exclusion

ALSA's `usbstream` plugin (used only by the Tascam US-122L/US-144) lists one
PCM per sound card, named after that card. Without the exclusion, asking for a
card by name would select its `usbstream` PCM instead of the card. `by_name()`
keeps `usbstream:` devices out of name and substring matches; they remain
selectable by exact id. The code marks this as a single known offender, with
"prefer `hw`/`plughw` PCMs" as the upgrade path if other card-named plugins
appear.

## Output targets

`find_device` returns an `OutputTarget { device, sink }`:

| Match | `device` | `sink` |
|---|---|---|
| default, exact or substring device | the matched cpal device | `None` |
| sink (rule 2 or 4) | the `pulse` ALSA device, else the `pipewire` one | `Some(("PULSE_SINK" or "PIPEWIRE_NODE", sink name))` |

If a sink matches but neither routing device exists, the lookup fails with
`Device not found: <sink> (a PipeWire/PulseAudio sink, but neither the pulse
nor the pipewire ALSA device exists to route to it)`. What happens to the
`sink` override when the stream opens is the subject of
[Sink Routing](pipewire-sink-routing.md); the device itself is then configured
as described in [Stream Setup](../playback/stream-setup-and-negotiation.md).

## Tests

| Test | Guards |
|---|---|
| `device.rs::test_find_default_device` | `None`, `""` and `"  "` all resolve to the host default device (skipped when there is none). |
| `device.rs::test_find_nonexistent_device` | An unknown name is an error, not a fallback. |
| `device.rs::every_listed_device_is_selectable_by_id` | Every listed id resolves back to that same device with no sink override; sink ids route to their sink when a `pulse`/`pipewire` device exists; the `host:id` form works. Hardware-independent: ALSA's own configuration lists devices such as `null` even without sound cards. |
| `test_devices.py::test_device_dict_has_expected_keys` | Dict keys and types, `index` equals position, `type` ∈ {alsa, pipewire, wasapi, coreaudio}, ids are unique. |
| `test_devices.py::test_select_device_by_id` | On Linux, `device="pulse"` opens an active player. |
| `test_devices.py::test_select_sink_by_id` | Sink ids are `"pipewire:" + name` and the first sink opens an active player. |

## Extension seams

- New matching tiers belong in `find_device`; keep the guarantee that every id
  `list_output_devices()` reports selects exactly that device — the
  `every_listed_device_is_selectable_by_id` test enforces it.
- Additional devices to keep out of name matches go in `by_name()`.
- Another sink source (for example a native PipeWire API) would extend
  `list_pipewire_sinks()`, which is already split into Linux and non-Linux
  variants.

## Related pages

- [PipeWire/PulseAudio Sink Routing](pipewire-sink-routing.md)
- [Stream Setup and Config Negotiation](../playback/stream-setup-and-negotiation.md)
- [Python API Reference](../api/python-api.md)
- [Logging and Troubleshooting](../operations/logging-and-troubleshooting.md)
