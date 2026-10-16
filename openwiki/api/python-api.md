---
type: api-reference
title: Python API Reference
description: The public contract of the pyaudiocast package — list_output_devices, play_file and AudioPlayer (constructor, write/drain/clear/stop, properties), accepted audio inputs, thread-safety guarantees and the exception raised for each failure mode.
tags: [api, python, audioplayer, play-file, devices, exceptions, typing]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-16T04:21:31.514Z
sources:
  - id: openwiki-source-651d1fb6c9e49916a916ab51
    resource: repo://Cargo.toml
  - id: openwiki-source-ca6cb4b1a14fd7969dfae3ec
    resource: repo://CHANGELOG.md
  - id: openwiki-source-d6c68e12c3efb74a1cd6ef7b
    resource: repo://pyaudiocast/__init__.py
  - id: openwiki-source-57549c36e97e92b8c26de49a
    resource: repo://pyaudiocast/_pyaudiocast.pyi
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
  - id: openwiki-source-ea5f93364eb7225247c2fdd1
    resource: repo://tests/test_devices.py
  - id: openwiki-source-a0d32ab49f0e7ff6b43fce77
    resource: repo://tests/test_player.py
generated: { by: "claude-code", at: "2026-10-16T04:21:31.514Z" }
---

# Python API Reference

`pyaudiocast` is a thin Python package over the native extension module
`pyaudiocast._pyaudiocast` (built from `src/` with PyO3). The package's
`__init__.py` re-exports exactly four names from the native module:

| Name | Kind | Summary |
|---|---|---|
| `AudioPlayer` | class | Streaming player: chunked `write()`, `drain()`, `clear()`, `stop()` |
| `list_output_devices()` | function | Enumerate output devices as dicts |
| `play_file(path, device=None)` | function | Blocking one-shot WAV playback |
| `__version__` | `str` | The crate version (`Cargo.toml`) compiled into the extension |

The native module registers these in `_pyaudiocast()` (`src/lib.rs`), and
`__version__` is `env!("CARGO_PKG_VERSION")`, so it always equals the
`[package] version` the wheel was built from. Type information ships as
`pyaudiocast/_pyaudiocast.pyi` plus a `py.typed` marker.

How these calls are bridged into Rust (GIL handling, error conversion) is
covered in [Native Extension Boundary](../architecture/native-extension-boundary.md).

## `list_output_devices() -> list[DeviceInfo]`

Returns one dict per output device, enumerated without holding the GIL:

| Key | Type | Meaning |
|---|---|---|
| `name` | `str` | Human-readable name. **Not unique** (one card can expose several devices). |
| `id` | `str` | Unique id; pass it as `device=` to select exactly this device. ALSA examples: `"pulse"`, `"hw:CARD=Generic,DEV=0"`. PipeWire/PulseAudio sinks: `"pipewire:<sink name>"`. |
| `index` | `int` | Position in the returned list. |
| `type` | `str` | `"pipewire"` for PipeWire/PulseAudio sinks, otherwise the cpal host id (`"alsa"`, `"wasapi"`, `"coreaudio"`). |

cpal devices come first, then (Linux only) the sinks reported by
`pactl list short sinks`. `DeviceInfo` exists only in the stub as a
`TypedDict`; at runtime these are plain dicts. The tests assert that ids are
unique, that `index` equals the list position, and that every sink id is
`"pipewire:" + name`. Enumeration details and the full `device=` matching rules
live in [Device Discovery and Selection](../devices/device-discovery-and-selection.md).

## `play_file(path, device=None) -> None`

Plays a WAV file to completion and returns. `path` may be a `str` or any
`os.PathLike`; `device` uses the same matching as `AudioPlayer`.

- The player is opened at the file's own sample rate and channel count, and the
  audio is converted for the device if needed.
- The file is streamed from disk in 4096-frame chunks, not decoded up front.
- A file that ends before its header says (truncated, or written through a
  pipe by tools such as sox) plays what is there and returns normally, with a
  warning in the log.
- Any other decode error is raised **after** the audio that decoded has played.
- Ctrl+C interrupts playback; the device is released either way.

See [WAV File Playback](../playback/wav-file-playback.md) for the mechanism.

## `AudioPlayer(device=None, sample_rate=None, channels=None)`

Opens an output stream immediately; construction fails if the device cannot be
found or opened.

| Parameter | Type | Default | Meaning |
|---|---|---|---|
| `device` | `str \| None` | `None` | Device id, `"host:id"`, name, PipeWire/PulseAudio sink name, or a case-insensitive substring of one. `None`, `""` or whitespace selects the default device. |
| `sample_rate` | `int \| None` | `None` | Sample rate **of the audio you will write**, 1000–768000 Hz. `None` adopts the device's default rate. |
| `channels` | `int \| None` | `None` | Channel count **of the audio you will write**, 1–64. `None` adopts the device's default channel count. |

Out-of-range `sample_rate` or `channels` raise `ValueError` before any device
work happens. The device is then opened at the closest configuration it
supports and the player resamples and remixes between the two formats; see
[Stream Setup and Config Negotiation](../playback/stream-setup-and-negotiation.md)
and [Resampling and Channel Mapping](../playback/resampling-and-channel-mapping.md).
The class is marked `@final` in the stub and cannot be subclassed.

### Properties

| Property | Type | Meaning |
|---|---|---|
| `sample_rate` | `int` | The caller's sample rate (what you write). |
| `channels` | `int` | The caller's channel count (what you write). |
| `device_sample_rate` | `int` | The rate the device stream actually runs at. |
| `device_channels` | `int` | The channel count the device stream actually uses. |
| `is_active` | `bool` | `True` until `stop()` is called or the stream fails permanently. |

Always size and shape your buffers from `sample_rate`/`channels`; the
`device_*` values are informational.

### `write(data) -> None`

Queues audio and returns once all of it is in the ring buffer, blocking while
the buffer (4 seconds of device audio) is full.

| Input | Interpreted as |
|---|---|
| `bytes`, `bytearray` | int16 little-endian PCM |
| `list[float]` | float samples in −1.0..1.0 |
| `numpy.ndarray` int16 / int32 / float32 / float64 | samples scaled to −1.0..1.0 |

- Multi-channel audio is interleaved (`L R L R …`). numpy arrays may be 1-D
  (interleaved) or 2-D shaped `(frames, channels)`, where `channels` must equal
  the player's `channels`; strided and misaligned arrays are accepted.
- Chunks need not be aligned: a write may end mid-sample (odd byte count) or
  mid-frame, and the remainder is completed by the next write — unless a
  `drain()` or `clear()` ended the stream in between, in which case the
  remainder is dropped so the next stream starts aligned.
- Float input is clipped to −1.0..1.0 (just below +1.0, see
  [Write Path](../playback/write-path-and-input-formats.md)); NaN and ±inf
  become silence.
- numpy is optional. Arrays are only recognized once numpy has been imported;
  everything else works without it.
- If `clear()` runs while a `write()` is in progress (for example blocked on a
  full buffer), the rest of that write's data is discarded and `write()`
  returns normally.
- `write()` is meant for one producer at a time: the writer lock is released
  between ring-buffer pushes, so concurrent writers would interleave.

### `drain() -> None`

Blocks until the audio written **before the call** has been played, including
the device's output latency, so a `stop()` right afterwards does not cut the
tail. Audio written by other threads while it waits is not waited for. It
returns immediately (without error) if nothing is pending, and returns early if
`clear()` or `stop()` is called meanwhile — including when the player is
already stopped. `drain()` ends the stream: the next `write()` starts a new one
(see [Interruption, Drain and Stop Semantics](../playback/interruption-and-drain.md)).

### `clear() -> None`

Discards all buffered audio. The audio that was playing fades out over 5 ms
instead of being cut (which would click). A blocked `drain()` returns, an
in-flight `write()` drops the rest of its data, and later writes start a new
stream. `clear()` never blocks and never raises, so it is safe from any thread
or signal handler — the basis of voice-assistant "barge-in". Stop your producer
loop too, or it will keep feeding the rest of the interrupted response:

```python
interrupted = threading.Event()

def speak(chunks):                 # producer thread
    interrupted.clear()
    for chunk in chunks:
        if interrupted.is_set():
            break
        player.write(chunk)        # blocks while the buffer is full
    player.drain()

def on_user_speech():              # any other thread
    interrupted.set()              # stop producing...
    player.clear()                 # ...and silence what is buffered
```

### `stop() -> None`

Marks the player closed, discards buffered audio and releases the device.
Idempotent. Afterwards `write()` raises `RuntimeError("Stream error: Player is
closed")` — as does a `write()` that was blocked in another thread when `stop()`
ran — while `drain()` returns immediately. If `stop()` is never called, the
device is released when the object is garbage-collected.

### Context manager

`with AudioPlayer(...) as player:` returns the player itself; leaving the block
calls `stop()`. Exceptions raised inside the block propagate.

## Thread safety

`AudioPlayer` is a frozen PyO3 class whose mutable state lives behind atomics
and mutexes, so every method may be called from any thread — and `clear()`,
`stop()` and `drain()` from other threads or signal handlers — while a `write()`
is blocked. Blocking calls (`write()`, `drain()`, `play_file()`) release the GIL
while they wait and run Python signal handlers every 100 ms, so Ctrl+C raises
`KeyboardInterrupt` promptly.

## Exceptions

All errors originating in the Rust core (`SpeakerError`) surface as
`RuntimeError`; argument problems use `ValueError`/`TypeError`.

| Situation | Exception | Message |
|---|---|---|
| `sample_rate` / `channels` out of range | `ValueError` | `sample_rate must be in 1000..=768000 Hz` / `channels must be in 1..=64` |
| No device matches `device=` | `RuntimeError` | `Device not found: <query>` |
| Sink matched but no `pulse`/`pipewire` ALSA device to route through | `RuntimeError` | `Device not found: <sink> (a PipeWire/PulseAudio sink, but neither …)` |
| No default output device | `RuntimeError` | `No default output device available` |
| No audio host at all | `RuntimeError` | `Stream error: No audio host available` |
| Device rejects the configuration while opening | `RuntimeError` | `Config error: <host:id>: …` (e.g. `alsa:pulse`) |
| Device or stream fails while opening | `RuntimeError` | `Stream error: <host:id>: …` |
| Unsupported `write()` type | `TypeError` | `Expected bytes, bytearray, list[float] or a numpy array (int16/int32/float32/float64)` |
| Unsupported numpy dtype | `TypeError` | `Unsupported numpy dtype: <dtype>. Expected float32, float64, int16, or int32` |
| numpy array of the wrong shape | `ValueError` | `Expected a 1-D array or a 2-D array shaped (frames, <channels>), got shape …` |
| `write()` after or during `stop()` | `RuntimeError` | `Stream error: Player is closed` |
| Device failed, vanished or stopped calling back | `RuntimeError` (from `write()`/`drain()`) | `Stream error: <reason>`, e.g. `Stream error: Audio device stopped responding` |
| WAV cannot be opened or decoded | `RuntimeError` | `IO error: …` |
| Ctrl+C (or another signal handler raising) during a blocking call | `KeyboardInterrupt` / the handler's exception | — |

Stream failures are latched: once a stream has failed, every later `write()`
and `drain()` raises the same message and `is_active` is `False`. See
[Blocking Waits and Stream Failure Handling](../playback/blocking-waits-and-stream-failures.md).

## Behavior changes in 0.2.0

The 0.2.0 release changed several observable behaviors: `type` reports the
real host (`"wasapi"`, `"coreaudio"`) instead of `"alsa"` everywhere; sink ids
became `"pipewire:<sink>"`; `write()` accepts chunks ending mid-sample or
mid-frame instead of raising `ValueError`, and `drain()`/`clear()` end the
stream; `play_file` raises decode errors after playing what decoded; and on
macOS streams keep the device's sample rate and channel count.

## Examples

Runnable scripts live in `examples/`: `list_devices.py` (enumeration),
`play_sine.py` (list input at 44.1 kHz mono, printing caller vs device
format), `play_wav.py` (`play_file`), and `stream_with_interrupt.py` (chunked
streaming with a `clear()` from a timer thread).

## Related pages

- [Native Extension Boundary (PyO3)](../architecture/native-extension-boundary.md)
- [Write Path and Input Formats](../playback/write-path-and-input-formats.md)
- [Interruption, Drain and Stop Semantics](../playback/interruption-and-drain.md)
- [Device Discovery and Selection](../devices/device-discovery-and-selection.md)
- [WAV File Playback (play_file)](../playback/wav-file-playback.md)
