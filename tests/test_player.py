"""Tests for AudioPlayer."""

import numpy as np
import pytest

import pyaudiocast as pyspeaker
from tests.conftest import requires_audio


class TestAudioPlayerInit:
    """Test AudioPlayer creation."""

    @requires_audio
    def test_create_auto_detect(self):
        """Should auto-detect sample_rate and channels from device."""
        player = pyspeaker.AudioPlayer()
        assert player.sample_rate > 0
        assert player.channels > 0
        assert player.is_active is True
        player.stop()
        assert player.is_active is False

    @requires_audio
    def test_create_partial_override_sample_rate(self):
        """Should allow overriding only sample_rate."""
        player = pyspeaker.AudioPlayer()
        sr = player.sample_rate
        player.stop()
        # Re-create with the same (known-good) sample rate
        player = pyspeaker.AudioPlayer(sample_rate=sr)
        assert player.sample_rate == sr
        assert player.channels > 0
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
        with pyspeaker.AudioPlayer() as player:
            silence = b"\x00\x00" * 100
            player.write(silence)

    @requires_audio
    def test_write_odd_bytes_raises(self):
        """Should reject odd-length byte data."""
        with pyspeaker.AudioPlayer() as player:
            with pytest.raises(ValueError, match="even"):
                player.write(b"\x00\x00\x00")

    @requires_audio
    def test_write_numpy_int16(self):
        """Should accept numpy int16 array."""
        with pyspeaker.AudioPlayer() as player:
            data = np.zeros(100, dtype=np.int16)
            player.write(data)

    @requires_audio
    def test_write_numpy_float32(self):
        """Should accept numpy float32 array."""
        with pyspeaker.AudioPlayer() as player:
            data = np.zeros(100, dtype=np.float32)
            player.write(data)

    @requires_audio
    def test_write_numpy_float64(self):
        """Should accept numpy float64 array."""
        with pyspeaker.AudioPlayer() as player:
            data = np.zeros(100, dtype=np.float64)
            player.write(data)

    @requires_audio
    def test_write_numpy_int32(self):
        """Should accept numpy int32 array."""
        with pyspeaker.AudioPlayer() as player:
            data = np.zeros(100, dtype=np.int32)
            player.write(data)

    @requires_audio
    def test_write_list_float(self):
        """Should accept list of floats."""
        with pyspeaker.AudioPlayer() as player:
            data = [0.0] * 100
            player.write(data)

    @requires_audio
    def test_write_unsupported_type_raises(self):
        """Should reject unsupported types."""
        with pyspeaker.AudioPlayer() as player:
            with pytest.raises((TypeError, ValueError)):
                player.write("not audio data")

    @requires_audio
    def test_drain_empty_buffer(self):
        """Drain on empty buffer should return immediately."""
        with pyspeaker.AudioPlayer() as player:
            player.drain()

    @requires_audio
    def test_write_after_stop_raises(self):
        """Should raise error when writing to stopped player."""
        player = pyspeaker.AudioPlayer()
        player.stop()
        with pytest.raises(RuntimeError, match="closed"):
            player.write(b"\x00\x00" * 10)


class TestAudioPlayerClear:
    """Test clear/interrupt functionality."""

    @requires_audio
    def test_clear_returns_without_error(self):
        """Clear on a player should not raise."""
        with pyspeaker.AudioPlayer() as player:
            player.write(b"\x00\x00" * 1000)
            player.clear()

    @requires_audio
    def test_clear_then_drain_returns_immediately(self):
        """Drain after clear should return immediately, not block."""
        import time

        with pyspeaker.AudioPlayer() as player:
            player.write(b"\x00\x00" * player.sample_rate)  # ~1 second of audio
            player.clear()
            start = time.monotonic()
            player.drain()
            elapsed = time.monotonic() - start
            assert elapsed < 0.5, f"drain() took {elapsed:.2f}s after clear (should be instant)"

    @requires_audio
    def test_write_after_clear_works(self):
        """Should be able to write new data after clear."""
        with pyspeaker.AudioPlayer() as player:
            player.write(b"\x00\x00" * 1000)
            player.clear()
            player.write(b"\x00\x00" * 100)  # should not raise


class TestPlayFile:
    """Test play_file function."""

    def test_play_nonexistent_file_raises(self):
        """Should raise error for nonexistent file."""
        with pytest.raises(RuntimeError):
            pyspeaker.play_file("/tmp/__nonexistent_audio_file__.wav")
