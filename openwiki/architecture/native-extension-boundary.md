---
type: architecture
title: Native Extension Boundary (PyO3)
description: How the Rust crate becomes the pyaudiocast._pyaudiocast extension module — module registration, the frozen AudioPlayer pyclass and its thread-safe state, which calls release the GIL and when, signal-handler re-entry, and how Rust errors become Python exceptions.
tags: [pyo3, ffi, gil, threading, errors, python-binding, abi3]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-16T04:21:31.514Z
sources:
  - id: openwiki-source-651d1fb6c9e49916a916ab51
    resource: repo://Cargo.toml
  - id: openwiki-source-d6c68e12c3efb74a1cd6ef7b
    resource: repo://pyaudiocast/__init__.py
  - id: openwiki-source-05ccef8d4cf1698187f20464
    resource: repo://pyproject.toml
  - id: openwiki-source-c1d203a608ca6409879479d1
    resource: repo://src/device.rs
  - id: openwiki-source-95f84abff7e2ed283d50b3fb
    resource: repo://src/error.rs
  - id: openwiki-source-ed8bf05e307c6278442542c2
    resource: repo://src/lib.rs
  - id: openwiki-source-ab97213db176711d486ab180
    resource: repo://src/player.rs
  - id: openwiki-source-a0d32ab49f0e7ff6b43fce77
    resource: repo://tests/test_player.py
generated: { by: "claude-code", at: "2026-10-16T04:21:31.514Z" }
---

# Native Extension Boundary (PyO3)

Everything Python sees is implemented in Rust and exposed through PyO3. This
page explains how that boundary is built and the rules that keep it safe:
which state is shared across threads, when the GIL is released, how blocked
calls stay interruptible, and how Rust errors turn into Python exceptions.
For the user-facing contract see [Python API Reference](../api/python-api.md).

## From crate to import path

| Piece | Where | What it does |
|---|---|---|
| `[lib] name = "_pyaudiocast"`, `crate-type = ["cdylib"]` | `Cargo.toml` | Builds a shared library whose init symbol matches the `_pyaudiocast` module name. |
| `pyo3 = { features = ["abi3-py39"] }` | `Cargo.toml` | Targets CPython's stable ABI, so one wheel per platform serves Python 3.9+. |
| `[tool.maturin] module-name = "pyaudiocast._pyaudiocast"` | `pyproject.toml` | maturin installs the library *inside* the `pyaudiocast/` package. |
| `#[pymodule] fn _pyaudiocast` | `src/lib.rs` | Registers `__version__`, `list_output_devices`, `play_file` and the `AudioPlayer` class. |
| `from pyaudiocast._pyaudiocast import …` | `pyaudiocast/__init__.py` | Re-exports the four public names (and lists them in `__all__`). |
| `_pyaudiocast.pyi`, `py.typed` | `pyaudiocast/` | Type stubs for the native module; the marker makes the package typed. |

Build mechanics (maturin, versions, system libraries) are in
[Build and Packaging](../operations/build-and-packaging.md).

## The `AudioPlayer` pyclass

`AudioPlayer` is declared `#[pyclass(frozen)]`. A frozen class never receives
`&mut self` and carries no PyO3 runtime borrow flag, so two threads calling
methods at the same time cannot trip a "already borrowed" error. All mutation
therefore goes through interior mutability chosen per field:

| Field | Type | Touched by |
|---|---|---|
| `shared` | `Arc<Shared>` — atomics plus a `Mutex<String>` for the failure message | Python threads **and** the realtime audio callback (which only uses the atomics) |
| `writer` | `Mutex<Writer>` — ring producer, resampler, carried partial samples | Python threads inside `write()` / `play_file` |
| `stream` | `Mutex<Option<cpal::Stream>>` | `stop()`, which takes and drops it |
| `sample_rate`, `channels`, `device_sample_rate`, `device_channels` | plain integers | read-only after construction |

Earlier versions used a class that could only be touched from its creating
thread; calling `clear()` from another thread raised `PanicException`. The
current layout is what makes the barge-in pattern (`clear()` from any thread)
legal. The meaning of the shared counters is covered in
[Interruption, Drain and Stop Semantics](../playback/interruption-and-drain.md).

Locks are poison-tolerant: both the player's `lock()` helper and the device
module's `open_lock()` recover the guard from a `PoisonError`, so one panic
while a lock is held does not make every later call panic.

`play_file` does not go through Python at all: it builds the same Rust
`AudioPlayer` with `AudioPlayer::new` and drives it with the crate-internal
`push`, `drain` and `stop` methods, so it inherits identical validation, device
lookup and GIL behavior.

## GIL policy

Device work and every wait run without the GIL (`py.detach`); only work that
touches Python objects — or that must keep Python code out — keeps it.

| Call | Phase | GIL |
|---|---|---|
| `list_output_devices()` | cpal enumeration + `pactl` | released |
| | building the result dicts | held |
| `AudioPlayer(...)` | range checks of `sample_rate` / `channels` | held |
| | `find_device` (enumeration, `pactl`, matching) | released |
| | `open` for a direct device (config query, negotiation, build + start stream) | released |
| | `open` for a PipeWire/PulseAudio **sink** | **held** — the sink is selected through a process-global environment variable, and holding the GIL keeps Python code (`os.environ`, `subprocess`) from running meanwhile; see [Sink Routing](../devices/pipewire-sink-routing.md) |
| `write(data)` | closed/failed check, extracting samples from the Python object | held |
| | alignment, resampling, pushing into the ring, waiting for space | released |
| `drain()` | waiting for playback | released |
| `clear()` | two atomic updates | held (never blocks) |
| `stop()` | set `closed`, clear | held |
| | dropping the cpal stream (backend teardown) | released |
| `play_file()` | opening the WAV file (blocks on a FIFO until its writer appears) | released |
| | creating the player | as `AudioPlayer(...)` |
| | decoding + pushing, draining | released |

### Staying interruptible while detached

Blocking loops (`write()`'s push loop, `drain()`, and through them
`play_file`) sleep in 5 ms steps via `Waiter`. Every 100 ms the waiter
re-attaches with `Python::try_attach` and calls `check_signals()`, so Ctrl+C
raises `KeyboardInterrupt` within a fraction of a second. `try_attach` returns
nothing while the interpreter is shutting down — attaching then would park the
thread forever — and the check is simply skipped. (CPython runs signal
handlers only on the main thread, so this matters for calls made from it.)

Because a signal handler runs *inside* that check, it can re-enter the player.
`write()` deliberately releases the writer lock before each wait, so a handler
that calls `stop()` or even `write()` on the same player does not deadlock — a
past bug that `test_signal_handler_can_use_a_blocked_player` pins down. Details
of the wait loop and stall detection are in
[Blocking Waits and Stream Failure Handling](../playback/blocking-waits-and-stream-failures.md).

## Errors crossing the boundary

`src/error.rs` defines the crate's error type and its conversions:

| `SpeakerError` variant | Python message | Produced by |
|---|---|---|
| `DeviceNotFound(q)` | `Device not found: q` | device matching, sink routing |
| `NoDefaultDevice` | `No default output device available` | default-device lookup |
| `StreamError(m)` | `Stream error: m` | cpal errors not classified as config errors, closed or failed players |
| `ConfigError(m)` | `Config error: m` | cpal `UnsupportedConfig` / `InvalidInput`, unsupported sample formats |
| `IoError(m)` | `IO error: m` | every `hound` (WAV) error |

- `impl From<SpeakerError> for PyErr` turns every variant into
  `RuntimeError(<Display message>)`, so Rust code returns `SpeakerError` with
  `?` and PyO3 raises it.
- `SpeakerError::context(what)` prefixes `StreamError`/`ConfigError` messages;
  stream setup uses it to prepend the device's `host:id` (for example
  `Config error: alsa:hw:CARD=X,DEV=0: …`). Other variants pass through
  unchanged.
- Argument problems bypass `SpeakerError` and are raised directly:
  `ValueError` for out-of-range `sample_rate`/`channels` and mis-shaped numpy
  arrays, `TypeError` for unsupported input types and numpy dtypes.
- Exceptions raised by Python code during a call — a signal handler's
  `KeyboardInterrupt`, or a list element whose `__float__` raises — propagate
  unchanged through `PyResult`.
- Failures that happen on the audio thread cannot raise there. They are
  latched into `Shared` and reported by the next `write()`/`drain()` check.

The full message catalog, with causes, is in
[Logging and Troubleshooting](../operations/logging-and-troubleshooting.md).

## Logging initialization

`init_logging()` runs once (`std::sync::Once`) on the first call to
`list_output_devices()`, `play_file()` or the `AudioPlayer` constructor — not
at import time. It builds an `env_logger` with a default `warn` level,
overridden by the `PYAUDIOCAST_LOG` variable, and installs it with `try_init`
so an already-installed global logger is not an error. See
[Logging and Troubleshooting](../operations/logging-and-troubleshooting.md).

## Extension seams

- **New module-level function:** add a `#[pyfunction]` in `src/lib.rs`,
  register it in `_pyaudiocast()`, re-export it from `pyaudiocast/__init__.py`
  (including `__all__`), and declare it in `_pyaudiocast.pyi`.
- **New `AudioPlayer` method:** keep it `&self`; put any new mutable state
  behind an atomic or a mutex; wrap blocking work in `py.detach` and wait with
  `Waiter` so it stays interruptible; never hold the writer lock across a wait.
- **New failure kind:** add a `SpeakerError` variant and its `Display` arm; it
  becomes a `RuntimeError` automatically.

## Related pages

- [Python API Reference](../api/python-api.md)
- [Architecture Overview](overview.md)
- [Blocking Waits and Stream Failure Handling](../playback/blocking-waits-and-stream-failures.md)
- [PipeWire/PulseAudio Sink Routing](../devices/pipewire-sink-routing.md)
- [Logging and Troubleshooting](../operations/logging-and-troubleshooting.md)
- [Build and Packaging](../operations/build-and-packaging.md)
