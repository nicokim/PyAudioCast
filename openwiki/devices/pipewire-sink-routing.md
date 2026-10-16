---
type: mechanism
title: PipeWire/PulseAudio Sink Routing
description: How pyaudiocast plays to a specific PipeWire/PulseAudio sink by opening the pulse or pipewire ALSA device with PULSE_SINK or PIPEWIRE_NODE temporarily set by SinkGuard, and the global open lock, drop order and GIL discipline that make this process-wide environment change safe enough.
tags: [pipewire, pulseaudio, alsa, sinks, environment, locking, gil, linux]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-16T04:21:31.514Z
sources:
  - id: openwiki-source-164e2da859b5277df81c7d94
    resource: repo://.github/workflows/ci.yml
  - id: openwiki-source-c1d203a608ca6409879479d1
    resource: repo://src/device.rs
  - id: openwiki-source-ed8bf05e307c6278442542c2
    resource: repo://src/lib.rs
  - id: openwiki-source-ab97213db176711d486ab180
    resource: repo://src/player.rs
  - id: openwiki-source-ea5f93364eb7225247c2fdd1
    resource: repo://tests/test_devices.py
generated: { by: "claude-code", at: "2026-10-16T04:21:31.514Z" }
---

# PipeWire/PulseAudio Sink Routing

On Linux, pyaudiocast only talks to ALSA: cpal is built without its native
PipeWire/PulseAudio hosts. A sink — including a virtual one such as a
"Virtual-Mic" loopback — is not an ALSA device, so it cannot be opened
directly. Instead the sound server's own ALSA plugin device is opened and told
which sink to play to through an environment variable:

| Routing device (ALSA id) | Variable the plugin reads | Provided by |
|---|---|---|
| `pulse` (preferred) | `PULSE_SINK` | the PulseAudio ALSA plugin, via libpulse |
| `pipewire` | `PIPEWIRE_NODE` | PipeWire's ALSA plugin |

How a query becomes a sink match is covered in
[Device Discovery and Selection](device-discovery-and-selection.md); this page
covers what happens after `find_device` returns a target with `sink` set.

## Choosing the routing device

`route(sink, devices)` scans the enumerated ALSA devices for bare id `pulse`
first and `pipewire` second, and returns an `OutputTarget` whose `device` is
that plugin device and whose `sink` is `(variable, sink name)`. It logs
`Routing to sink '<sink>' via the <id> device` at info level. If neither
device exists, lookup fails with:

```
Device not found: <sink> (a PipeWire/PulseAudio sink, but neither the pulse nor the pipewire ALSA device exists to route to it)
```

The usual fix is installing the server's ALSA plugin package (the one that
provides the `pulse` or `pipewire` PCM).

## Opening with the override

`AudioPlayer::open` (in `src/player.rs`) is where the override is applied:

```rust
// Declared first, so it's released after the sink variable is restored.
let _open = open_lock();
// Sinks are reached through the pulse/pipewire device: point it at the sink while opening.
let _sink = target.sink.as_ref().map(|(var, sink)| SinkGuard::set(var, sink));
// ... default_output_config(), negotiate(), build_output_stream(), play() ...
```

1. The global open lock is taken.
2. `SinkGuard::set` saves the variable's current value and sets it to the sink
   name.
3. Everything that touches the device — querying the default and supported
   configs, building the stream, and starting it — runs while the variable is
   set. The plugin reads it when it connects to the sound server, so the
   stream stays attached to that sink for its whole life.
4. When `open` returns (successfully or with an error), locals drop in reverse
   declaration order: `_sink` first — restoring the previous value, or removing
   the variable if it was unset — and only then `_open`, releasing the lock.

Restoring the variable keeps the override from leaking into later opens, both
pyaudiocast's own and those of other libraries in the same process.

## Safety argument

Rust 2024 makes `std::env::set_var`/`remove_var` `unsafe`, because another
thread reading the environment at the same moment is undefined behavior.
`SinkGuard` documents why it accepts that risk:

- the plugins take the sink **only** from the environment, so there is no
  safer channel;
- pyaudiocast's own device lookups and opens are serialized by `open_lock()`
  (see below), so none of them read the environment mid-change;
- the GIL is held, which keeps Python code — `os.environ`, `subprocess` and
  anything else that reads or copies the environment — from running meanwhile;
- **acknowledged limit:** other native threads reading the environment at the
  same time cannot be ruled out.

The third point is why the `AudioPlayer` constructor makes an exception to its
GIL policy: for a sink target it calls `open` **with the GIL held**, whereas
direct devices are opened with the GIL released. The README puts the cost at
roughly 20 ms of GIL hold time per sink open. See
[Native Extension Boundary](../architecture/native-extension-boundary.md#gil-policy).

## Locking

`device.rs` owns one process-wide `static OPEN: Mutex<()>`, acquired through
the poison-tolerant `open_lock()`. It is held for the full duration of:

| Holder | GIL while holding the lock |
|---|---|
| `list_output_devices()` (cpal enumeration and `pactl`) | released |
| `find_device()` (enumeration, `pactl`, matching) | released |
| `AudioPlayer::open()` for a direct device | released |
| `AudioPlayer::open()` for a sink target | held |

The reason is spelled out at the lock's definition: device lookups and opens
read the environment (alsa-lib, libpulse) while `SinkGuard` changes it, so both
must hold it.

Two consequences:

- **Lock order is always GIL → open lock.** Code that holds the open lock never
  attaches to Python, so a sink open that waits for the lock while holding the
  GIL cannot deadlock against a lookup that holds the lock with the GIL
  released.
- **Device work is serialized process-wide.** A slow open (for example a
  Bluetooth device starting up) delays every other player construction and
  `list_output_devices()` call in the process until it finishes. Once a stream
  is open, playback itself never takes this lock.

## Testing

- `test_devices.py::test_select_sink_by_id` (Linux, needs audio) checks that
  every sink id is `"pipewire:" + name` and opens the first sink by its id.
- `device.rs::every_listed_device_is_selectable_by_id` checks that each listed
  sink id resolves to a target carrying that sink, whenever a `pulse` or
  `pipewire` ALSA device exists to route through.
- CI's Linux jobs install `pulseaudio` and `libasound2-plugins`, start a
  headless PulseAudio server and point ALSA's default PCM at `pulse`, so the
  `pulse` routing device and a sink exist on the runner. See
  [Audio Test Environments](../testing/audio-test-environments.md).

## Extension seams

- Another routing device is one more `(alsa id, variable)` pair in the
  ordered list inside `route()`.
- Any new code that opens devices or reads the environment the audio plugins
  use must hold `open_lock()`, and must not attach to Python while holding it.

## Related pages

- [Device Discovery and Selection](device-discovery-and-selection.md)
- [Stream Setup and Config Negotiation](../playback/stream-setup-and-negotiation.md)
- [Native Extension Boundary (PyO3)](../architecture/native-extension-boundary.md)
- [Audio Test Environments](../testing/audio-test-environments.md)
