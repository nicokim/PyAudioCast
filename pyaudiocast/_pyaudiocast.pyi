"""Type stubs for the native _pyaudiocast module."""

from typing import Optional, Union

import numpy as np
import numpy.typing as npt

def list_output_devices() -> list[dict[str, str | int]]:
    """List all available audio output devices.

    Returns a list of dicts with "name" (str), "index" (int), and "type" (str).
    """
    ...

def play_file(path: str, device: Optional[str] = None) -> None:
    """Play a WAV file to an output device.

    Args:
        path: Path to a .wav file.
        device: Device name (substring match) or None for default.
    """
    ...

class AudioPlayer:
    """Cross-platform audio player with streaming support.

    Supports context manager protocol (with statement).
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
            npt.NDArray[np.int16],
            npt.NDArray[np.int32],
            npt.NDArray[np.float32],
            npt.NDArray[np.float64],
            list[float],
        ],
    ) -> None:
        """Write audio data to the player.

        Accepts bytes (int16 LE), numpy arrays (int16/int32/float32/float64),
        or list[float]. Format is auto-detected.
        """
        ...
    def drain(self) -> None:
        """Block until all buffered audio has been played."""
        ...
    def clear(self) -> None:
        """Discard buffered audio and unblock drain() immediately."""
        ...
    def stop(self) -> None:
        """Stop the player and release resources."""
        ...
    @property
    def sample_rate(self) -> int: ...
    @property
    def channels(self) -> int: ...
    @property
    def is_active(self) -> bool: ...
    def __enter__(self) -> "AudioPlayer": ...
    def __exit__(self, exc_type: object, exc_val: object, exc_tb: object) -> bool: ...
