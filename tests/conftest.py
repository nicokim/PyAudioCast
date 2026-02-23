"""Shared fixtures for pyaudiocast tests."""

import pyaudiocast as pyspeaker
import pytest


def _has_audio_device():
    """Check if an audio output device is available."""
    try:
        player = pyspeaker.AudioPlayer()
        player.stop()
        return True
    except RuntimeError:
        return False


has_audio = _has_audio_device()

requires_audio = pytest.mark.skipif(
    not has_audio, reason="No audio output device available"
)
