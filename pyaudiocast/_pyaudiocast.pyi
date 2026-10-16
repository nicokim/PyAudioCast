"""Type stubs for the native _pyaudiocast module."""

import os
from typing import Optional, TypedDict, Union, final

import numpy as np
import numpy.typing as npt

__version__: str

class DeviceInfo(TypedDict):
    name: str
    """Human-readable name (not unique)."""
    id: str
    """Unique id, e.g. "pulse" or "hw:CARD=Generic,DEV=0" (ALSA), "pipewire:<sink>" (a sink)."""
    index: int
    type: str
    """"pipewire" for PipeWire/PulseAudio sinks, else the host: "alsa", "wasapi", "coreaudio"."""

def list_output_devices() -> list[DeviceInfo]:
    """List all available audio output devices."""
    ...

def play_file(path: Union[str, os.PathLike[str]], device: Optional[str] = None) -> None:
    """Play a WAV file to an output device. Blocks until playback is done.

    Args:
        path: Path to a .wav file.
        device: Device id or name, PipeWire sink name, or None for default.
    """
    ...

@final
class AudioPlayer:
    """Cross-platform audio player with streaming support.

    Thread-safe: `clear()`, `stop()` and `drain()` may be called from other threads, or
    from signal handlers, while a `write()` is in progress. Supports the context manager
    protocol.
    """

    def __init__(
        self,
        device: Optional[str] = None,
        sample_rate: Optional[int] = None,
        channels: Optional[int] = None,
    ) -> None: ...
    def write(
        self,
        data: Union[
            bytes,
            bytearray,
            npt.NDArray[np.int16],
            npt.NDArray[np.int32],
            npt.NDArray[np.float32],
            npt.NDArray[np.float64],
            list[float],
        ],
    ) -> None:
        """Write audio data to the player. Blocks while the buffer is full.

        Accepts bytes/bytearray (int16 LE), numpy arrays (int16/int32/float32/float64,
        1-D interleaved or 2-D shaped (frames, channels)), or list[float]. A chunk may end
        mid-sample or mid-frame; the remainder is completed by the next write, unless
        `drain()` or `clear()` ends the stream first.
        """
        ...
    def drain(self) -> None:
        """Block until all audio written so far has been played. Ends the stream.

        Returns early if `clear()` or `stop()` is called.
        """
        ...
    def clear(self) -> None:
        """Discard buffered audio and unblock drain() immediately."""
        ...
    def stop(self) -> None:
        """Stop the player and release the device. Safe to call more than once."""
        ...
    @property
    def sample_rate(self) -> int: ...
    @property
    def channels(self) -> int: ...
    @property
    def device_sample_rate(self) -> int: ...
    @property
    def device_channels(self) -> int: ...
    @property
    def is_active(self) -> bool: ...
    def __enter__(self) -> "AudioPlayer": ...
    def __exit__(self, exc_type: object, exc_val: object, exc_tb: object) -> None: ...
