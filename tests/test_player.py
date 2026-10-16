"""Tests for AudioPlayer."""

import _thread
import subprocess
import sys
import threading
import time
import wave
from concurrent.futures import ThreadPoolExecutor

import numpy as np
import pytest

import pyaudiocast as pyspeaker
from tests.conftest import requires_audio


def silence(player, seconds):
    """Interleaved float32 silence lasting `seconds` at the player's rate/channels."""
    return np.zeros(int(player.sample_rate * seconds) * player.channels, dtype=np.float32)


def timed(fn, *args):
    start = time.monotonic()
    fn(*args)
    return time.monotonic() - start


def warmed_up(player):
    """Wait until the device is actually playing: a cold sound server can take a second
    to start a new stream, which would skew the timing assertions. Then skip the test if
    the device doesn't keep time (GitHub's macOS VMs sometimes play ~60x too slowly)."""
    player.write(silence(player, 0.05))
    player.drain()
    player.write(silence(player, 0.05))
    if timed(player.drain) > 0.5:
        player.stop()
        pytest.skip("the audio device isn't playing in real time")
    return player


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

    @pytest.mark.parametrize("kwargs", [{"sample_rate": 0}, {"channels": 0}])
    def test_invalid_config_raises_value_error(self, kwargs):
        """Zero rate/channels used to panic deep in the resampler."""
        with pytest.raises(ValueError):
            pyspeaker.AudioPlayer(**kwargs)

    @requires_audio
    def test_context_manager(self):
        """Should work as context manager and stop on exit."""
        with pyspeaker.AudioPlayer() as player:
            assert player.is_active is True
        assert player.is_active is False

    @requires_audio
    def test_stop_is_idempotent(self):
        player = pyspeaker.AudioPlayer()
        player.stop()
        player.stop()


class TestAudioPlayerWrite:
    """Test writing audio data."""

    @requires_audio
    def test_write_bytes(self):
        """Should accept int16 LE bytes."""
        with pyspeaker.AudioPlayer() as player:
            player.write(b"\x00\x00" * 100 * player.channels)

    @requires_audio
    def test_write_bytearray(self):
        with pyspeaker.AudioPlayer() as player:
            player.write(bytearray(b"\x00\x00" * 100 * player.channels))

    @requires_audio
    def test_write_bytes_in_unaligned_chunks(self):
        """Streamed PCM arrives in arbitrary chunks: odd lengths and split frames are
        carried over to the next write instead of raising or being dropped."""
        with warmed_up(pyspeaker.AudioPlayer()) as player:
            pcm = np.zeros(int(player.sample_rate * 0.4) * player.channels, np.int16).tobytes()
            for i in range(0, len(pcm), 1001):
                player.write(pcm[i : i + 1001])
            assert 0.35 < timed(player.drain) < 1.5

    @requires_audio
    @pytest.mark.parametrize("dtype", [np.int16, np.int32, np.float32, np.float64])
    def test_write_numpy(self, dtype):
        """Should accept numpy int16/int32/float32/float64 arrays."""
        with pyspeaker.AudioPlayer() as player:
            player.write(np.zeros(100 * player.channels, dtype=dtype))

    @requires_audio
    def test_write_numpy_2d_frames_by_channels(self):
        with pyspeaker.AudioPlayer(channels=2) as player:
            player.write(np.zeros((100, 2), dtype=np.float32))
            with pytest.raises(ValueError, match="frames"):
                player.write(np.zeros((2, 100), dtype=np.float32))

    @requires_audio
    def test_write_numpy_non_contiguous(self):
        with pyspeaker.AudioPlayer(channels=1) as player:
            player.write(np.zeros(200, dtype=np.float32)[::2])

    @requires_audio
    @pytest.mark.parametrize("dtype", [np.int16, np.float32])
    def test_write_misaligned_numpy(self, dtype):
        """An array at an odd address used to be read in place (undefined behaviour; a
        panic in debug builds)."""
        buf = bytearray(1 + 200 * np.dtype(dtype).itemsize)
        arr = np.frombuffer(memoryview(buf)[1:], dtype=dtype)
        assert not arr.flags.aligned
        with pyspeaker.AudioPlayer(channels=1) as player:
            player.write(arr)

    @requires_audio
    def test_write_unsupported_numpy_dtype_raises(self):
        with pyspeaker.AudioPlayer() as player:
            with pytest.raises(TypeError, match="uint16"):
                player.write(np.zeros(100 * player.channels, dtype=np.uint16))

    @requires_audio
    def test_write_list_float(self):
        """Should accept list of floats."""
        with pyspeaker.AudioPlayer() as player:
            player.write([0.0] * 100 * player.channels)

    @requires_audio
    def test_write_partial_frames_carry_over(self):
        with warmed_up(pyspeaker.AudioPlayer(channels=2)) as player:
            samples = silence(player, 0.4)
            for i in range(0, len(samples), 999):
                player.write(samples[i : i + 999])
            assert 0.35 < timed(player.drain) < 1.5

    @requires_audio
    def test_write_without_numpy(self):
        """numpy is optional: rust-numpy's array check used to panic when it couldn't load."""
        code = (
            "import sys; sys.modules['numpy'] = None\n"
            "import pyaudiocast\n"
            "with pyaudiocast.AudioPlayer(channels=1) as p:\n"
            "    p.write([0.0] * 100)\n"
            "    try:\n"
            "        p.write('not audio')\n"
            "    except TypeError:\n"
            "        print('ok')\n"
        )
        out = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True)
        assert out.stdout.strip() == "ok", out.stderr

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
            assert timed(player.drain) < 0.5

    @requires_audio
    def test_write_after_stop_raises(self):
        """Should raise error when writing to stopped player."""
        player = pyspeaker.AudioPlayer()
        player.stop()
        with pytest.raises(RuntimeError, match="closed"):
            player.write(b"\x00\x00" * 10)


class TestDrain:
    """drain() must wait for exactly the audio that was written."""

    @requires_audio
    def test_drain_waits_for_buffered_audio(self):
        """drain() used to return immediately when the audio callback ran while write()
        was still converting its input."""

        class SlowFloat:
            """Keeps write() converting for longer than one audio callback period."""

            def __float__(self):
                time.sleep(0.05)
                return 0.0

        with warmed_up(pyspeaker.AudioPlayer()) as player:
            for _ in range(3):
                data = silence(player, 0.3).tolist()
                data[0] = SlowFloat()
                player.write(data)
                assert 0.25 < timed(player.drain) < 1.5

    @requires_audio
    def test_numpy_int16_silence_keeps_its_length(self):
        """All-zero int16 arrays used to be read as raw bytes (half the samples)."""
        with warmed_up(pyspeaker.AudioPlayer()) as player:
            frames = int(player.sample_rate * 0.4)
            player.write(np.zeros(frames * player.channels, dtype=np.int16))
            assert timed(player.drain) > 0.35

    @requires_audio
    def test_drain_after_stop_returns(self):
        """drain() used to hang forever after stop() with audio still buffered."""
        player = pyspeaker.AudioPlayer()
        player.write(silence(player, 1.0))
        player.stop()
        with ThreadPoolExecutor(1) as ex:
            ex.submit(player.drain).result(timeout=2)

    @requires_audio
    def test_drain_ignores_audio_written_after_it(self):
        """drain() used to wait until the stream went idle, i.e. for later writes too."""
        with warmed_up(pyspeaker.AudioPlayer()) as player:
            player.write(silence(player, 0.3))
            with ThreadPoolExecutor(1) as ex:
                start = time.monotonic()
                drained = ex.submit(player.drain)
                time.sleep(0.05)
                while not drained.done() and time.monotonic() - start < 5:
                    player.write(silence(player, 0.05))
                drained.result(timeout=10)
                assert time.monotonic() - start < 2.0

    @requires_audio
    def test_ctrl_c_interrupts_drain(self):
        with warmed_up(pyspeaker.AudioPlayer()) as player:
            player.write(silence(player, 3.0))
            timer = threading.Timer(0.2, _thread.interrupt_main)
            timer.start()
            start = time.monotonic()
            with pytest.raises(KeyboardInterrupt):
                player.drain()
            assert time.monotonic() - start < 1.5
            timer.join()


class TestAudioPlayerClear:
    """Test clear/interrupt functionality."""

    @requires_audio
    def test_clear_returns_without_error(self):
        """Clear on a player should not raise."""
        with pyspeaker.AudioPlayer() as player:
            player.write(b"\x00\x00" * 1000 * player.channels)
            player.clear()

    @requires_audio
    def test_clear_then_drain_returns_immediately(self):
        """Drain after clear should return immediately, not block."""
        with pyspeaker.AudioPlayer() as player:
            player.write(silence(player, 1.0))
            player.clear()
            assert timed(player.drain) < 0.5

    @requires_audio
    def test_clear_then_write_drops_old_audio(self):
        """write() right after clear() used to resurrect the cleared audio."""
        with warmed_up(pyspeaker.AudioPlayer()) as player:
            player.write(silence(player, 2.0))
            player.clear()
            player.write(silence(player, 0.2))
            assert timed(player.drain) < 1.0

    @requires_audio
    def test_empty_write_after_clear_keeps_audio_cleared(self):
        with warmed_up(pyspeaker.AudioPlayer()) as player:
            player.write(silence(player, 2.0))
            player.clear()
            player.write(b"")
            assert timed(player.drain) < 0.5

    @requires_audio
    def test_barge_in_does_not_replay_interrupted_audio(self):
        """A writer blocked on a full ring used to keep pushing the stale chunk after clear()."""
        with warmed_up(pyspeaker.AudioPlayer()) as player:
            with ThreadPoolExecutor(1) as ex:
                writer = ex.submit(player.write, silence(player, 10.0))
                time.sleep(0.3)
                player.clear()
                writer.result(timeout=2)
            player.write(silence(player, 0.2))
            assert timed(player.drain) < 1.0

    @requires_audio
    def test_clear_from_another_thread(self):
        """The barge-in pattern: used to raise PanicException (unsendable)."""
        with warmed_up(pyspeaker.AudioPlayer()) as player:
            player.write(silence(player, 0.5))
            with ThreadPoolExecutor(1) as ex:
                ex.submit(player.clear).result(timeout=2)

    @requires_audio
    def test_clear_unblocks_drain_in_another_thread(self):
        with pyspeaker.AudioPlayer() as player:
            player.write(silence(player, 3.0))
            with ThreadPoolExecutor(1) as ex:
                drained = ex.submit(player.drain)
                time.sleep(0.2)
                player.clear()
                drained.result(timeout=1)

    @requires_audio
    def test_stop_unblocks_writer_in_another_thread(self):
        """A write() larger than the ring blocks; stop() must release it."""
        player = pyspeaker.AudioPlayer()
        with ThreadPoolExecutor(1) as ex:
            written = ex.submit(player.write, silence(player, 10.0))
            time.sleep(0.3)
            player.stop()
            with pytest.raises(RuntimeError, match="closed"):
                written.result(timeout=2)

    @requires_audio
    @pytest.mark.skipif(sys.platform == "win32", reason="POSIX signals")
    @pytest.mark.parametrize("call", ["stop()", "write([0.0] * 10)"])
    def test_signal_handler_can_use_a_blocked_player(self, call):
        """A handler run during a blocked write() used to deadlock on the writer lock."""
        warmed_up(pyspeaker.AudioPlayer()).stop()
        code = (
            "import os, signal, threading, pyaudiocast\n"
            "p = pyaudiocast.AudioPlayer(channels=1)\n"
            f"signal.signal(signal.SIGUSR1, lambda *a: p.{call})\n"
            "threading.Timer(0.5, os.kill, (os.getpid(), signal.SIGUSR1)).start()\n"
            "try:\n"
            "    p.write([0.0] * (p.sample_rate * 6))  # more than the ring holds\n"
            "except RuntimeError as e:\n"
            "    print(e)\n"
            "print('done')\n"
        )
        out = subprocess.run(
            [sys.executable, "-c", code], capture_output=True, text=True, timeout=30
        )
        assert "done" in out.stdout, out.stderr

    @requires_audio
    def test_write_after_clear_works(self):
        """Should be able to write new data after clear."""
        with pyspeaker.AudioPlayer() as player:
            player.write(b"\x00\x00" * 1000 * player.channels)
            player.clear()
            player.write(b"\x00\x00" * 100 * player.channels)  # should not raise


class TestPlayFile:
    """Test play_file function."""

    def test_play_nonexistent_file_raises(self):
        """Should raise error for nonexistent file."""
        with pytest.raises(RuntimeError):
            pyspeaker.play_file("/tmp/__nonexistent_audio_file__.wav")

    @requires_audio
    def test_play_file_accepts_pathlike(self, tmp_path):
        path = tmp_path / "silence.wav"
        with wave.open(str(path), "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(22050)
            w.writeframes(b"\x00\x00" * 4410)  # 0.2 s
        warmed_up(pyspeaker.AudioPlayer()).stop()
        assert 0.15 < timed(pyspeaker.play_file, path) < 5.0

    @requires_audio
    @pytest.mark.parametrize("damage", ["truncated", "pipe_header"])
    def test_play_file_shorter_than_its_header(self, tmp_path, damage):
        """A cut-off file, or one written through a pipe (sox declares ~2 GB), plays what's
        there; it used to raise after playing."""
        path = tmp_path / "short.wav"
        with wave.open(str(path), "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(22050)
            w.writeframes(b"\x00\x00" * 4410)
        data = bytearray(path.read_bytes())
        if damage == "truncated":
            del data[-2001:]  # ends mid-sample
        else:
            data[40:44] = (0x7FFFF000).to_bytes(4, "little")  # the data chunk's size
        path.write_bytes(data)
        warmed_up(pyspeaker.AudioPlayer()).stop()
        assert timed(pyspeaker.play_file, path) < 5.0


def test_version():
    assert isinstance(pyspeaker.__version__, str) and pyspeaker.__version__
