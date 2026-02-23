"""Type stubs for the native _pyspeaker module."""

from typing import Optional

import numpy as np
import numpy.typing as npt

def list_output_devices() -> list[dict[str, str | int]]:
    """List all available audio output devices.

    Returns a list of dicts, each with "name" (str) and "index" (int).
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
        sample_rate: int = 22050,
        channels: int = 1,
    ) -> None: ...
    def write(self, data: bytes) -> None:
        """Write raw audio bytes (int16 little-endian) to the player."""
        ...
    def write_array(self, data: npt.NDArray[np.int16]) -> None:
        """Write a numpy int16 array to the player."""
        ...
    def write_f32(self, data: list[float]) -> None:
        """Write f32 samples directly (values should be in -1.0..1.0 range)."""
        ...
    def drain(self) -> None:
        """Block until all buffered audio has been played."""
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
