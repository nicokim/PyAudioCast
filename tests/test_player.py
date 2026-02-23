"""Tests for AudioPlayer."""

import struct

import numpy as np
import pytest

import pyaudiocast as pyspeaker
from tests.conftest import requires_audio


class TestAudioPlayerInit:
    """Test AudioPlayer creation."""

    @requires_audio
    def test_create_default_device(self):
        """Should create a player with default device."""
        player = pyspeaker.AudioPlayer()
        assert player.sample_rate == 22050
        assert player.channels == 1
        assert player.is_active is True
        player.stop()
        assert player.is_active is False

    @requires_audio
    def test_create_custom_sample_rate(self):
        """Should create a player with custom sample rate."""
        player = pyspeaker.AudioPlayer(sample_rate=44100, channels=1)
        assert player.sample_rate == 44100
        player.stop()

    def test_create_nonexistent_device_raises(self):
        """Should raise error for nonexistent device."""
        with pytest.raises(RuntimeError, match="Device not found"):
            pyspeaker.AudioPlayer(device="__nonexistent_device_xyz__")

    @requires_audio
    def test_context_manager(self):
        """Should work as context manager and stop on exit."""
        with pyspeaker.AudioPlayer() as player:
            assert player.is_active is True
        assert player.is_active is False


class TestAudioPlayerWrite:
    """Test writing audio data."""

    @requires_audio
    def test_write_bytes(self):
        """Should accept int16 LE bytes."""
        with pyspeaker.AudioPlayer(sample_rate=22050) as player:
            # 100 samples of silence as int16 LE
            silence = b"\x00\x00" * 100
            player.write(silence)

    @requires_audio
    def test_write_odd_bytes_raises(self):
        """Should reject odd-length byte data."""
        with pyspeaker.AudioPlayer(sample_rate=22050) as player:
            with pytest.raises(ValueError, match="even"):
                player.write(b"\x00\x00\x00")

    @requires_audio
    def test_write_numpy_array(self):
        """Should accept numpy int16 array."""
        with pyspeaker.AudioPlayer(sample_rate=22050) as player:
            data = np.zeros(100, dtype=np.int16)
            player.write_array(data)

    @requires_audio
    def test_write_f32(self):
        """Should accept list of f32 samples."""
        with pyspeaker.AudioPlayer(sample_rate=22050) as player:
            data = [0.0] * 100
            player.write_f32(data)

    @requires_audio
    def test_drain_empty_buffer(self):
        """Drain on empty buffer should return immediately."""
        with pyspeaker.AudioPlayer(sample_rate=22050) as player:
            player.drain()

    @requires_audio
    def test_write_after_stop_raises(self):
        """Should raise error when writing to stopped player."""
        player = pyspeaker.AudioPlayer(sample_rate=22050)
        player.stop()
        with pytest.raises(RuntimeError, match="closed"):
            player.write(b"\x00\x00" * 10)


class TestAudioPlayerClear:
    """Test clear/interrupt functionality."""

    @requires_audio
    def test_clear_returns_without_error(self):
        """Clear on a player should not raise."""
        with pyspeaker.AudioPlayer(sample_rate=22050) as player:
            player.write(b"\x00\x00" * 1000)
            player.clear()

    @requires_audio
    def test_clear_then_drain_returns_immediately(self):
        """Drain after clear should return immediately, not block."""
        import time

        with pyspeaker.AudioPlayer(sample_rate=22050) as player:
            player.write(b"\x00\x00" * 22050)  # ~1 second of audio
            player.clear()
            start = time.monotonic()
            player.drain()
            elapsed = time.monotonic() - start
            assert elapsed < 0.5, f"drain() took {elapsed:.2f}s after clear (should be instant)"

    @requires_audio
    def test_write_after_clear_works(self):
        """Should be able to write new data after clear."""
        with pyspeaker.AudioPlayer(sample_rate=22050) as player:
            player.write(b"\x00\x00" * 1000)
            player.clear()
            player.write(b"\x00\x00" * 100)  # should not raise


class TestPlayFile:
    """Test play_file function."""

    def test_play_nonexistent_file_raises(self):
        """Should raise error for nonexistent file."""
        with pytest.raises(RuntimeError):
            pyspeaker.play_file("/tmp/__nonexistent_audio_file__.wav")
