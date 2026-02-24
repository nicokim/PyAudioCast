"""PyAudioCast: Cross-platform audio output for Python, powered by Rust and CPAL."""

from importlib.metadata import version

from pyaudiocast._pyaudiocast import AudioPlayer, list_output_devices, play_file

__all__ = ["AudioPlayer", "list_output_devices", "play_file"]
__version__ = version("pyaudiocast")
