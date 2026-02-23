# PyAudioCast

[![CI](https://github.com/nicokim/PyAudioCast/actions/workflows/ci.yml/badge.svg)](https://github.com/nicokim/PyAudioCast/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/pyaudiocast)](https://pypi.org/project/pyaudiocast/)
[![codecov](https://codecov.io/gh/nicokim/PyAudioCast/graph/badge.svg)](https://codecov.io/gh/nicokim/PyAudioCast)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)

Cross-platform audio output library for Python, powered by Rust and [CPAL](https://github.com/RustAudio/cpal).

Stream audio to any output device — including PipeWire/PulseAudio virtual sinks on Linux — with a simple Python API backed by a high-performance Rust core.

## Features

- **Cross-platform**: Linux (ALSA/PipeWire/PulseAudio), Windows (WASAPI), macOS (CoreAudio)
- **Device selection**: List and select output devices by name, including PipeWire virtual sinks
- **Streaming audio**: Write audio data in chunks via a lock-free ring buffer — ideal for real-time TTS, generative audio, live effects, etc.
- **Multiple input formats**: `bytes` (int16 LE), `numpy` int16 arrays, or `float32` lists
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
git clone https://github.com/yourusername/pyaudiocast.git
cd pyaudiocast
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

# Stream audio to default device
with pyaudiocast.AudioPlayer(sample_rate=44100, channels=1) as player:
    player.write(audio_bytes)       # bytes (int16 LE)
    player.write_array(np_array)    # numpy int16 array
    player.write_f32(float_list)    # list of floats (-1.0 to 1.0)
    player.drain()                  # wait for playback to finish

# Stream to a specific device (substring match)
with pyaudiocast.AudioPlayer(device="Virtual-Mic", sample_rate=48000, channels=2) as player:
    player.write_f32(samples)
    player.drain()

# One-shot WAV playback
pyaudiocast.play_file("audio.wav", device="pulse")
```

## API Reference

### `pyaudiocast.list_output_devices() -> list[dict]`

Returns a list of available output devices. Each dict contains:
- `name` (str): Device name
- `index` (int): Device index
- `type` (str): `"alsa"` for ALSA/cpal devices, `"pipewire"` for PipeWire/PulseAudio sinks

### `pyaudiocast.AudioPlayer(device=None, sample_rate=22050, channels=1)`

Streaming audio player with ring buffer.

| Parameter     | Type          | Default | Description                                       |
|---------------|---------------|---------|---------------------------------------------------|
| `device`      | `str \| None` | `None`  | Device name (substring match) or None for default |
| `sample_rate` | `int`         | `22050` | Sample rate in Hz                                 |
| `channels`    | `int`         | `1`     | Number of audio channels                          |

**Methods:**

| Method                         | Description                                  |
|--------------------------------|----------------------------------------------|
| `write(data: bytes)`           | Write int16 little-endian audio bytes        |
| `write_array(data: ndarray)`   | Write numpy int16 array                      |
| `write_f32(data: list[float])` | Write float32 samples (-1.0 to 1.0)         |
| `drain()`                      | Block until all buffered audio is played     |
| `stop()`                       | Stop playback and release resources          |

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
  ├─ write() / write_array() / write_f32()
  │     │
  │     ▼
  │  Lock-free Ring Buffer (ringbuf crate)
  │     │
  │     ▼
  │  cpal audio callback (OS audio thread)
  │     │
  │     ▼
  └─ ALSA / WASAPI / CoreAudio → Speaker / Virtual Sink
```

- **Ring buffer**: Lock-free producer/consumer. Python pushes samples, the OS audio callback pulls them — no locks in the audio path.
- **GIL release**: `write()` and `drain()` release the Python GIL during blocking operations.
- **Sample conversion**: Input int16 data is converted to float32 in Rust before entering the ring buffer.

## Running Tests

```bash
# Python tests
pytest tests/ -v

# Rust tests
cargo test
```

## License

MIT
