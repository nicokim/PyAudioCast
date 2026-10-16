# PyAudioCast

[![CI](https://github.com/nicokim/PyAudioCast/actions/workflows/ci.yml/badge.svg)](https://github.com/nicokim/PyAudioCast/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/pyaudiocast)](https://pypi.org/project/pyaudiocast/)
[![codecov](https://codecov.io/gh/nicokim/PyAudioCast/graph/badge.svg)](https://codecov.io/gh/nicokim/PyAudioCast)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)

Cross-platform audio output library for Python, powered by Rust and [CPAL](https://github.com/RustAudio/cpal).

Stream audio to any output device — including PipeWire/PulseAudio virtual sinks on Linux — with a simple Python API backed by a high-performance Rust core.

## Features

- **Cross-platform**: Linux (ALSA/PipeWire/PulseAudio), Windows (WASAPI), macOS (CoreAudio)
- **Auto-detect**: Sample rate and channels are detected from the device — zero config needed
- **Auto-negotiate**: If the device can't take your sample rate or channel count, audio is resampled and remixed for it (e.g. 22050Hz mono TTS → 48000Hz stereo device)
- **Device selection**: Select output devices by id or name, including PipeWire virtual sinks
- **Streaming audio**: Write audio in chunks into a ring buffer drained by a lock-free audio callback — ideal for real-time TTS, generative audio, live effects, etc.
- **Unified `write()`**: Accepts `bytes`, `numpy` arrays (int16/int32/float32/float64), or `list[float]` — format is detected automatically
- **Thread-safe interruption**: `clear()` from any thread instantly discards buffered audio (including a `write()` in progress) and unblocks `drain()` — perfect for voice assistant barge-in
- **Context manager**: Clean resource management with `with` statement
- **GIL-friendly**: Blocking writes, `drain()` and device setup release the Python GIL (except for the ~20 ms it takes to open a PipeWire/PulseAudio sink); Ctrl+C interrupts blocking writes and `drain()`
- **Typed**: Ships type stubs
- **Debug logging**: Enable detailed logs with `PYAUDIOCAST_LOG=debug`

## Installation

### From PyPI

```bash
pip install pyaudiocast
```

Wheels are built for Python 3.9+ (one abi3 wheel per platform) on Linux x86_64, macOS (Intel and Apple Silicon) and Windows x86_64. `numpy` is optional.

> **Note (Linux):** Pre-built wheels require ALSA (`libasound.so.2`). Install it if not already present:
> ```bash
> sudo apt install libasound2t64  # Debian 13/Ubuntu 24.04+ (libasound2 on older releases)
> sudo dnf install alsa-lib       # Fedora
> ```

### From source (requires Rust 1.88+)

```bash
# Install Rust if needed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Linux: install ALSA development headers
sudo apt install libasound2-dev  # Debian/Ubuntu
sudo dnf install alsa-lib-devel  # Fedora

# Clone and install into a virtual environment
git clone https://github.com/nicokim/PyAudioCast.git
cd PyAudioCast
python -m venv .venv && source .venv/bin/activate
pip install .
```

### Development setup (with uv)

```bash
uv venv && source .venv/bin/activate
uv pip install maturin numpy pytest
maturin develop
```

## Quick Start

```python
import pyaudiocast

# List all output devices
for dev in pyaudiocast.list_output_devices():
    print(f"[{dev['index']}] {dev['name']} ({dev['type']}, id={dev['id']})")

# Stream audio — sample rate and channels auto-detected from device
with pyaudiocast.AudioPlayer() as player:
    print(f"Using: {player.sample_rate}Hz, {player.channels}ch")
    player.write(audio_bytes)   # bytes, numpy array, or list[float]
    player.drain()              # wait for playback to finish

# Use your source's format; it is converted for the device if needed
with pyaudiocast.AudioPlayer(sample_rate=22050, channels=1) as player:
    player.write(tts_audio)
    player.drain()

# Stream to a specific device
with pyaudiocast.AudioPlayer(device="Virtual-Mic") as player:
    player.write(samples)
    player.drain()

# One-shot WAV playback
pyaudiocast.play_file("audio.wav", device="pulse")
```

## Streaming with Interruption

For real-time applications like voice assistants, stream from one thread and interrupt from another:

```python
import threading
import pyaudiocast

player = pyaudiocast.AudioPlayer(sample_rate=22050, channels=1)
interrupted = threading.Event()

def speak(tts_stream):
    interrupted.clear()
    for chunk in tts_stream:
        if interrupted.is_set():
            break
        player.write(chunk)  # blocks while the buffer is full
    player.drain()           # wait for playback to finish

# From another thread, e.g. when the user starts speaking:
def on_user_speech_detected():
    interrupted.set()  # stop producing this response...
    player.clear()     # ...and silence what's already buffered
```

`clear()` discards all buffered audio immediately. A `write()` in progress in another thread drops the rest of its data and returns, and a blocked `drain()` returns right away. Later `write()` calls start a new stream — so stop your producer loop too, as above, or it will keep feeding the rest of the interrupted response.

## Supported Audio Formats

`write()` auto-detects the input format:

| Input type | Format | Conversion |
|---|---|---|
| `bytes` / `bytearray` | int16 little-endian PCM | Converted to float32 |
| `list[float]` | float32 samples (-1.0 to 1.0) | Direct |
| `numpy.ndarray` (int16) | int16 samples | Converted to float32 |
| `numpy.ndarray` (int32) | int32 samples | Converted to float32 |
| `numpy.ndarray` (float32) | float32 samples | Direct (no conversion) |
| `numpy.ndarray` (float64) | float64 samples | Converted to float32 |

Multi-channel audio is interleaved (`L R L R ...`). numpy arrays can be 1-D interleaved or 2-D shaped `(frames, channels)` (as returned by e.g. `soundfile.read`), contiguous or not. Chunks don't need to be aligned: a write may end mid-sample or mid-frame (as streamed HTTP bodies do), and the remainder is completed by the next write. `drain()` and `clear()` end the stream: a remainder still pending then (a stream cut off mid-sample) is dropped, so the next stream starts aligned. Float samples are clipped to -1.0..1.0, and NaN/inf become silence.

```python
import numpy as np

with pyaudiocast.AudioPlayer(channels=1) as player:
    # All of these work with the same write() method
    player.write(b"\x00\x00" * 100)                    # bytes (int16 LE)
    player.write(np.zeros(100, dtype=np.int16))         # numpy int16
    player.write(np.zeros(100, dtype=np.float32))       # numpy float32 (fastest)
    player.write(np.zeros(100, dtype=np.float64))       # numpy float64
    player.write(np.zeros(100, dtype=np.int32))         # numpy int32
    player.write([0.0] * 100)                           # list[float]
```

## API Reference

### `pyaudiocast.list_output_devices() -> list[dict]`

Returns a list of available output devices. Each dict contains:
- `name` (str): Human-readable device name (not unique: one sound card can have several devices)
- `id` (str): Unique device id, e.g. `"pulse"` or `"hw:CARD=Audio,DEV=0"` on ALSA, `"pipewire:Virtual-Mic"` for a sink — pass it as `device=` to select exactly this device
- `index` (int): Position in the list
- `type` (str): `"pipewire"` for PipeWire/PulseAudio sinks, otherwise the audio host: `"alsa"`, `"wasapi"` or `"coreaudio"`

### `pyaudiocast.AudioPlayer(device=None, sample_rate=None, channels=None)`

Streaming audio player with ring buffer.

| Parameter     | Type          | Default | Description                                                    |
|---------------|---------------|---------|----------------------------------------------------------------|
| `device`      | `str \| None` | `None`  | Device id or name, PipeWire sink name, or None for the default |
| `sample_rate` | `int \| None` | `None`  | Sample rate of your audio in Hz, or None to use the device's   |
| `channels`    | `int \| None` | `None`  | Channel count of your audio, or None to use the device's       |

If `sample_rate` or `channels` don't match what the device supports, pyaudiocast converts to the device's configuration. Invalid values raise `ValueError`.

**Methods** (all safe to call from any thread, or from a signal handler):

| Method          | Description                                                                         |
|-----------------|-------------------------------------------------------------------------------------|
| `write(data)`   | Queue audio data (bytes, numpy array, or list[float]). Blocks while the ~4 s buffer is full |
| `drain()`       | Block until the audio queued so far has been played, device latency included (audio written meanwhile isn't waited for). Returns early on `clear()`/`stop()` |
| `clear()`       | Discard buffered audio (with a 5 ms fade-out, so it doesn't click), abort an in-progress `write()`, unblock `drain()` |
| `stop()`        | Stop playback and release the device (idempotent)                                    |

`write()` and `drain()` raise `RuntimeError` if the device fails or disappears (for example when unplugged), and `KeyboardInterrupt` on Ctrl+C.

**Properties:**

| Property              | Type   | Description                                  |
|-----------------------|--------|----------------------------------------------|
| `sample_rate`         | `int`  | Requested sample rate (what you send)        |
| `channels`            | `int`  | Requested channel count (what you send)      |
| `device_sample_rate`  | `int`  | Actual device sample rate (what plays)       |
| `device_channels`     | `int`  | Actual device channel count (what plays)     |
| `is_active`           | `bool` | Whether the player is open and its stream healthy |

**Context manager:** Supports `with` statement (calls `stop()` on exit).

### `pyaudiocast.play_file(path, device=None)`

Play a WAV file (`str` or path-like) to completion, streaming it from disk. Blocks until playback is done.

## Device Selection

`device` is matched in this order: exact id (`pipewire:<sink>` for a sink), exact name, exact PipeWire/PulseAudio sink name, then a case-insensitive substring of a sink name, device id or device name.

### Default device
```python
player = pyaudiocast.AudioPlayer()  # uses system default
```

### By id or name
```python
player = pyaudiocast.AudioPlayer(device="pulse")                # ALSA device id
player = pyaudiocast.AudioPlayer(device="hw:CARD=Audio,DEV=0")  # a specific sound card
```

### PipeWire/PulseAudio virtual sinks (Linux)
```python
# Virtual sinks are auto-detected via pactl
player = pyaudiocast.AudioPlayer(device="Virtual-Mic")           # sink name
player = pyaudiocast.AudioPlayer(device="pipewire:Virtual-Mic")  # its id
```

PipeWire sinks are routed through the `pulse` (or `pipewire`) ALSA device: `PULSE_SINK` (or `PIPEWIRE_NODE`) is set only while the stream is opened (holding the GIL, and with other pyaudiocast device lookups and opens locked out), then restored. On a PipeWire desktop, prefer sinks over `hw:` devices: the sound server keeps the hardware busy.

## Environment Variables

| Variable        | Description                                                | Example               |
|-----------------|------------------------------------------------------------|-----------------------|
| `PYAUDIOCAST_LOG` | Enable logging. Uses `env_logger` filter syntax.         | `PYAUDIOCAST_LOG=debug` |

### Logging

```bash
# Show everything
PYAUDIOCAST_LOG=debug python my_script.py

# Show info and above
PYAUDIOCAST_LOG=info python my_script.py

# Default (no env var): warnings and errors only
python my_script.py
```

## Cross-Platform Support

| Platform | Backend               | Device listing | Virtual sinks     |
|----------|-----------------------|----------------|-------------------|
| Linux    | ALSA + PipeWire/Pulse | Full           | Yes (via pactl)   |
| Windows  | WASAPI                | cpal devices   | N/A               |
| macOS    | CoreAudio             | cpal devices   | N/A               |

The audio engine (`cpal`) is fully cross-platform. PipeWire/PulseAudio virtual sink detection uses `pactl` and is automatically compiled out on non-Linux systems via `#[cfg(target_os = "linux")]`. On macOS, streams open at the device's current sample rate and channel count (changing them would affect every app), and pyaudiocast converts; cpal may still switch the hardware's sample format to 32-bit float.

## Architecture

```
Python (pyaudiocast)
  |
  +- write(data)  ->  auto-detect format  ->  convert to f32
  |     |
  |     v
  |  [resample + remix channels if needed]
  |     |
  |     v
  |  Lock-free SPSC ring buffer (ringbuf crate, ~4 s of device audio)
  |     |
  |     v
  |  cpal audio callback (OS audio thread)
  |     |                          ^
  |     v                          |
  +- Speaker / Virtual Sink    clear() -> discard watermark
```

- **Lock-free audio path**: The audio callback only touches the ring buffer and atomic counters: no locks, no allocation, no syscalls.
- **GIL release**: Device setup (except opening a PipeWire/PulseAudio sink), blocking `write()`s and `drain()` release the Python GIL. Blocking calls wake every few milliseconds to check `clear()`/`stop()`, device failure and Ctrl+C.
- **Sample conversion**: All input formats are converted to float32 in Rust before entering the ring buffer, then to the device's sample format (f32, i16, i24, i32, ...) in the callback.
- **Auto-resample**: If the device doesn't support the requested sample rate, a stateful cubic (Catmull-Rom) resampler converts it, seamlessly across `write()` calls.
- **Channel mapping**: Mono feeds the front left/right speakers; a mono device gets the average of all channels.
- **Interruption**: `clear()` records how much audio has been written; the callback fades out over 5 ms and discards up to that point, so a `write()` right after `clear()` plays only new audio.
- **Drain**: `drain()` waits until the callback has consumed everything written before the call, plus the device's output latency, so a `stop()` right after doesn't cut the tail.

## Running Tests

```bash
# Python tests (audio tests are skipped without an output device)
pytest tests/ -v

# Rust tests (link against libpython: needs the Python development package)
cargo test
```

## License

MIT
