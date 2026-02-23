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
- **Device selection**: List and select output devices by name, including PipeWire virtual sinks
- **Streaming audio**: Write audio data in chunks via a lock-free ring buffer — ideal for real-time TTS, generative audio, live effects, etc.
- **Unified `write()`**: Accepts `bytes`, `numpy` arrays (int16/int32/float32/float64), or `list[float]` — format is detected automatically
- **Interruption**: `clear()` instantly discards buffered audio and unblocks `drain()` — perfect for voice assistant barge-in
- **Context manager**: Clean resource management with `with` statement
- **GIL-friendly**: Releases the Python GIL during audio writes and drain, so other threads run freely
- **Clean output**: ALSA/JACK backend probe noise is automatically suppressed
- **Debug logging**: Enable detailed logs with `PYAUDIOCAST_LOG=debug`

## Installation

### From source (requires Rust toolchain)

```bash
# Install Rust if needed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Linux: install ALSA headers
sudo apt install libasound2-dev  # Debian/Ubuntu
sudo dnf install alsa-lib-devel  # Fedora

# Clone and install
git clone https://github.com/nicokim/PyAudioCast.git
cd PyAudioCast
pip install maturin
maturin develop
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
    print(f"[{dev['index']}] {dev['name']} ({dev['type']})")

# Stream audio — sample rate and channels auto-detected from device
with pyaudiocast.AudioPlayer() as player:
    print(f"Using: {player.sample_rate}Hz, {player.channels}ch")
    player.write(audio_bytes)   # bytes, numpy array, or list[float]
    player.drain()              # wait for playback to finish

# Override sample rate if your source requires it
with pyaudiocast.AudioPlayer(sample_rate=22050) as player:
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

For real-time applications like voice assistants, you can interrupt playback instantly:

```python
import pyaudiocast
import threading

with pyaudiocast.AudioPlayer(sample_rate=22050) as player:
    # Stream TTS chunks as they arrive
    for chunk in tts_stream:
        player.write(chunk)
    player.drain()  # wait for playback to finish
```

```python
# Interrupt from another thread (e.g., when user starts speaking)
def on_user_speech_detected(player):
    player.clear()  # instantly stops audio, unblocks drain()
```

`clear()` discards all buffered audio immediately. Any blocked `drain()` returns right away. Calling `write()` again resumes normal playback.

## Supported Audio Formats

`write()` auto-detects the input format:

| Input type | Format | Conversion |
|---|---|---|
| `bytes` | int16 little-endian PCM | Converted to float32 |
| `numpy.ndarray` (int16) | int16 samples | Converted to float32 |
| `numpy.ndarray` (int32) | int32 samples | Converted to float32 |
| `numpy.ndarray` (float32) | float32 samples | Direct (no conversion) |
| `numpy.ndarray` (float64) | float64 samples | Converted to float32 |
| `list[float]` | float32 samples (-1.0 to 1.0) | Direct |

```python
import numpy as np

with pyaudiocast.AudioPlayer(sample_rate=44100) as player:
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
- `name` (str): Device name
- `index` (int): Device index
- `type` (str): `"alsa"` for ALSA/cpal devices, `"pipewire"` for PipeWire/PulseAudio sinks

### `pyaudiocast.AudioPlayer(device=None, sample_rate=None, channels=None)`

Streaming audio player with ring buffer.

| Parameter     | Type          | Default | Description                                       |
|---------------|---------------|---------|---------------------------------------------------|
| `device`      | `str \| None` | `None`  | Device name (substring match) or None for default |
| `sample_rate` | `int \| None` | `None`  | Sample rate in Hz, or None to auto-detect         |
| `channels`    | `int \| None` | `None`  | Number of audio channels, or None to auto-detect  |

**Methods:**

| Method          | Description                                           |
|-----------------|-------------------------------------------------------|
| `write(data)`   | Write audio data (bytes, numpy array, or list[float]) |
| `drain()`       | Block until all buffered audio is played              |
| `clear()`       | Discard buffer and unblock drain() immediately        |
| `stop()`        | Stop playback and release resources                   |

**Properties:** `sample_rate`, `channels`, `is_active`

**Context manager:** Supports `with` statement (calls `stop()` on exit).

### `pyaudiocast.play_file(path, device=None)`

Play a WAV file to completion. Blocks until playback is done.

## Device Selection

### Default device
```python
player = pyaudiocast.AudioPlayer()  # uses system default
```

### ALSA device (by name substring)
```python
player = pyaudiocast.AudioPlayer(device="pulse")
player = pyaudiocast.AudioPlayer(device="hw:CARD=Audio")
```

### PipeWire/PulseAudio virtual sinks (Linux)
```python
# Virtual sinks are auto-detected via pactl
player = pyaudiocast.AudioPlayer(device="Virtual-Mic")
```

PipeWire sinks are routed transparently through `PULSE_SINK` + the `pulse` ALSA device.

## Environment Variables

| Variable        | Description                                                | Example               |
|-----------------|------------------------------------------------------------|-----------------------|
| `PYAUDIOCAST_LOG` | Enable debug logging. Uses `env_logger` filter syntax.     | `PYAUDIOCAST_LOG=debug` |

### Logging

```bash
# Show everything (including ALSA/JACK backend messages)
PYAUDIOCAST_LOG=debug python my_script.py

# Show info and above
PYAUDIOCAST_LOG=info python my_script.py

# Default (no env var): warnings only, ALSA/JACK noise suppressed
python my_script.py
```

## Cross-Platform Support

| Platform | Backend               | Device listing | Virtual sinks     |
|----------|-----------------------|----------------|-------------------|
| Linux    | ALSA + PipeWire/Pulse | Full           | Yes (via pactl)   |
| Windows  | WASAPI                | cpal devices   | N/A               |
| macOS    | CoreAudio             | cpal devices   | N/A               |

The audio engine (`cpal`) is fully cross-platform. PipeWire/PulseAudio virtual sink detection uses `pactl` and is automatically compiled out on non-Linux systems via `#[cfg(target_os = "linux")]`.

## Architecture

```
Python (pyaudiocast)
  │
  ├─ write(data)  →  auto-detect format  →  convert to f32
  │     │
  │     ▼
  │  Lock-free Ring Buffer (ringbuf crate)
  │     │
  │     ▼
  │  cpal audio callback (OS audio thread)
  │     │                          ▲
  │     ▼                          │
  └─ Speaker / Virtual Sink    clear() → discard + silence
```

- **Ring buffer**: Lock-free producer/consumer. Python pushes samples, the OS audio callback pulls them — no locks in the audio path.
- **GIL release**: `write()` and `drain()` release the Python GIL during blocking operations.
- **Sample conversion**: All input formats are converted to float32 in Rust before entering the ring buffer.
- **Interruption**: `clear()` sets an atomic flag checked by the audio callback, which discards remaining samples and outputs silence.

## Running Tests

```bash
# Python tests
pytest tests/ -v

# Rust tests
cargo test
```

## License

MIT
