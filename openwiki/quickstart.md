---
type: quickstart
title: PyAudioCast Quickstart
description: Orientation for the PyAudioCast repository — what the pyaudiocast library does, how the Rust and Python parts are laid out, how to build and test it, the core mental model, and a task-routing map naming the wiki page and source file to open first for common changes.
tags: [quickstart, overview, onboarding, routing, development]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-16T04:21:31.514Z
sources:
  - id: openwiki-source-46fa34397e41ebf7491c7359
    resource: repo://.github/workflows/release-please.yml
  - id: openwiki-source-4d1d392666be6dfdd7a91a2e
    resource: repo://.github/workflows/release.yml
  - id: openwiki-source-651d1fb6c9e49916a916ab51
    resource: repo://Cargo.toml
  - id: openwiki-source-d6c68e12c3efb74a1cd6ef7b
    resource: repo://pyaudiocast/__init__.py
  - id: openwiki-source-05ccef8d4cf1698187f20464
    resource: repo://pyproject.toml
  - id: openwiki-source-23775c3de52f3ab95a13cb8b
    resource: repo://README.md
  - id: openwiki-source-482fa4ca84f42b04ba025fc1
    resource: repo://release-please-config.json
  - id: openwiki-source-c1d203a608ca6409879479d1
    resource: repo://src/device.rs
  - id: openwiki-source-95f84abff7e2ed283d50b3fb
    resource: repo://src/error.rs
  - id: openwiki-source-ed8bf05e307c6278442542c2
    resource: repo://src/lib.rs
  - id: openwiki-source-ab97213db176711d486ab180
    resource: repo://src/player.rs
  - id: openwiki-source-f0a6e7dc03522b2682f88655
    resource: repo://tests/conftest.py
generated: { by: "claude-code", at: "2026-10-16T04:21:31.514Z" }
---

# PyAudioCast Quickstart

**PyAudioCast** (`pip install pyaudiocast`) is a cross-platform audio
**output** library for Python with a Rust core built on
[cpal](https://github.com/RustAudio/cpal). It is designed for streaming audio
produced on the fly — text-to-speech, generated audio — with instant,
thread-safe interruption ("barge-in"), device selection that includes
PipeWire/PulseAudio sinks on Linux, and one-shot WAV playback. Version 0.2.0,
MIT-licensed, CPython ≥ 3.9 (one abi3 wheel per platform), Linux, macOS and
Windows.

Names used throughout: **PyAudioCast** is the project; **`pyaudiocast`** the
Python package; **`pyaudiocast._pyaudiocast`** the native module compiled from
`src/`. The tests import the package as `pyspeaker`, a former name.

```python
import pyaudiocast

for d in pyaudiocast.list_output_devices():
    print(d["index"], d["id"], d["name"], d["type"])

with pyaudiocast.AudioPlayer(sample_rate=22050, channels=1) as player:
    player.write(pcm_bytes)     # int16 LE bytes, list[float] or a numpy array
    player.drain()              # wait until it has been heard

pyaudiocast.play_file("speech.wav", device="pulse")
```

## Repository layout

| Path | Contents |
|---|---|
| `src/lib.rs` | `#[pymodule]` entry, `list_output_devices`, `play_file`, logging setup |
| `src/player.rs` | The `AudioPlayer` engine: stream setup and negotiation, `write()` input handling, resampler, realtime renderer, and the clear/drain/stop protocol — plus most Rust unit tests |
| `src/device.rs` | Host choice, device and sink listing, `device=` matching, sink routing, the global open lock |
| `src/error.rs` | `SpeakerError` and its mapping to `RuntimeError` |
| `pyaudiocast/` | `__init__.py` re-exports, `_pyaudiocast.pyi` stubs, `py.typed` |
| `tests/` | pytest integration tests; `conftest.py` defines `requires_audio` |
| `examples/` | `list_devices.py`, `play_sine.py`, `play_wav.py`, `stream_with_interrupt.py` |
| `.github/workflows/` | `ci.yml`, `release-please.yml`, `release.yml` |
| `Cargo.toml`, `pyproject.toml` | Crate (cdylib `_pyaudiocast`) and maturin packaging |
| `CHANGELOG.md`, `release-please-config.json`, `.release-please-manifest.json` | Release automation state |

## Build and test

```bash
uv venv && source .venv/bin/activate
uv pip install maturin numpy pytest
maturin develop                 # build + install the extension; re-run after Rust changes
pytest tests/ -v                # from the repo root; audio tests skip without an output device
cargo test                      # Rust unit tests
cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings
```

Linux needs the ALSA headers (`libasound2-dev`), plus `python3-dev` for
`cargo test`; building needs Rust ≥ 1.88. See
[Build and Packaging](operations/build-and-packaging.md) and
[Audio Test Environments](testing/audio-test-environments.md).

## Mental model

1. **Python is a shim.** All behavior is Rust behind PyO3; device work and
   every wait release the GIL
   ([Native Extension Boundary](architecture/native-extension-boundary.md)).
2. **Two formats.** The caller format (`sample_rate`, `channels`) is what you
   write; the device format (`device_sample_rate`, `device_channels`) is what
   plays. `write()` converts on the calling thread
   ([Stream Setup](playback/stream-setup-and-negotiation.md)).
3. **One ring, one realtime reader.** Converted f32 samples go into a 4-second
   ring; a lock-free cpal callback plays them
   ([Audio Callback](playback/audio-callback-and-rendering.md)).
4. **Interruption is arithmetic.** `clear()` bumps an epoch and sets a discard
   watermark in sample counts; writers in flight stop, the callback skips and
   fades out, `drain()` returns
   ([Interruption, Drain and Stop](playback/interruption-and-drain.md)).
5. **Devices are ALSA-first on Linux.** Sinks found with `pactl` are reached
   through the `pulse`/`pipewire` ALSA device with an environment override
   held under a global lock
   ([Sink Routing](devices/pipewire-sink-routing.md)).

The [Architecture Overview](architecture/overview.md) ties these together.

## Task routing

| I want to… | Read first | Then open |
|---|---|---|
| Understand the design end to end | [Architecture Overview](architecture/overview.md) | `src/player.rs` |
| Know exactly what a public call accepts, returns or raises | [Python API Reference](api/python-api.md) | `pyaudiocast/_pyaudiocast.pyi`, `src/player.rs` |
| Add or change an accepted input type or numpy dtype | [Write Path and Input Formats](playback/write-path-and-input-formats.md) | `extract_samples` / `numpy_to_f32` in `src/player.rs`; then the `.pyi`, README table, `TestAudioPlayerWrite` |
| Change how `device=` strings match | [Device Discovery and Selection](devices/device-discovery-and-selection.md) | `find_device` in `src/device.rs`; keep `every_listed_device_is_selectable_by_id` passing |
| Fix playback to a PipeWire/PulseAudio sink | [PipeWire/PulseAudio Sink Routing](devices/pipewire-sink-routing.md) | `route`, `SinkGuard` in `src/device.rs`; `open` in `src/player.rs` |
| Debug `clear()` replaying, cutting or clicking | [Interruption, Drain and Stop Semantics](playback/interruption-and-drain.md) | `Shared::clear`, `push`, `Renderer::render`; `TestAudioPlayerClear` |
| Debug `drain()` returning early, late or never | [Interruption, Drain and Stop Semantics](playback/interruption-and-drain.md#drain) | `drain` and `build_stream` in `src/player.rs`; `TestDrain` |
| Investigate underruns, fades or format conversion | [Audio Callback and Rendering](playback/audio-callback-and-rendering.md) | `Renderer`, `build_stream` |
| Change resampling quality or channel mixing | [Resampling and Channel Mapping](playback/resampling-and-channel-mapping.md) | `Converter`, `remap`; Rust resampler tests |
| Change which device configuration is opened | [Stream Setup and Config Negotiation](playback/stream-setup-and-negotiation.md) | `open`, `negotiate` |
| Handle device loss, stalls or Ctrl+C | [Blocking Waits and Stream Failure Handling](playback/blocking-waits-and-stream-failures.md) | `Waiter`, the error callback in `build_stream` |
| Change WAV playback | [WAV File Playback](playback/wav-file-playback.md) | `play_file` in `src/lib.rs` |
| Expose a new function or method to Python | [Native Extension Boundary](architecture/native-extension-boundary.md) | `src/lib.rs`, `pyaudiocast/__init__.py`, the `.pyi` |
| Understand an error message or enable logs | [Logging and Troubleshooting](operations/logging-and-troubleshooting.md) | run with `PYAUDIOCAST_LOG=debug` |
| Bump dependencies, MSRV or build settings | [Build and Packaging](operations/build-and-packaging.md) | `Cargo.toml`, `pyproject.toml`, the `msrv` job in `ci.yml` |
| Change CI | [Continuous Integration](operations/continuous-integration.md) | `.github/workflows/ci.yml` |
| Cut or repair a release | [Release Process](operations/release-process.md) | `release-please-config.json`, `.github/workflows/release.yml` |
| Find the test guarding a behavior | [Test Suite Map](testing/test-suite.md) | `tests/test_player.py`, Rust tests in `src/` |
| Make skipped audio tests run | [Audio Test Environments](testing/audio-test-environments.md) | `tests/conftest.py`, `ci.yml` |

## Conventions worth knowing

- **`ponytail:` comments** mark deliberate simplifications together with their
  ceiling and upgrade path: 5 ms polling instead of wake-ups, the generous 5 s
  stall timeout, the cubic resampler without anti-aliasing, first-channels
  downmixing, and the `usbstream` name-match exclusion. Read them before
  "fixing" those spots.
- **Regression tests name the bug.** Python test docstrings say what used to
  go wrong ("used to …"); keep that habit for new tests.
- **Conventional commits drive releases.** `fix:` and `feat:` bump the patch
  version, `feat!:` the minor (pre-1.0); release-please writes
  `CHANGELOG.md`, whose 0.2.0 entry lists the latest breaking behavior changes.

## Related pages

- [Architecture Overview](architecture/overview.md)
- [Python API Reference](api/python-api.md)
- [Interruption, Drain and Stop Semantics](playback/interruption-and-drain.md)
- [Test Suite Map](testing/test-suite.md)
- [Build and Packaging](operations/build-and-packaging.md)
