"""PyAudioCast: Cross-platform audio output for Python, powered by Rust and CPAL."""

from pyaudiocast._pyaudiocast import AudioPlayer, list_output_devices, play_file

__all__ = ["AudioPlayer", "list_output_devices", "play_file"]
__version__ = "0.1.0"
