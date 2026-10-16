---
type: testing
title: Audio Test Environments
description: How pyaudiocast's audio-dependent tests decide to run or skip, how CI gives them a real output device (headless PulseAudio with a null sink on Linux, the Null Audio Device on macOS), why timing assertions warm the device up first, and how subprocess and thread-based tests isolate numpy, signal and concurrency scenarios.
tags: [testing, audio, pytest, pulseaudio, ci, timing, skip]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-16T04:21:31.514Z
sources:
  - id: openwiki-source-164e2da859b5277df81c7d94
    resource: repo://.github/workflows/ci.yml
  - id: openwiki-source-c1d203a608ca6409879479d1
    resource: repo://src/device.rs
  - id: openwiki-source-ab97213db176711d486ab180
    resource: repo://src/player.rs
  - id: openwiki-source-f0a6e7dc03522b2682f88655
    resource: repo://tests/conftest.py
  - id: openwiki-source-ea5f93364eb7225247c2fdd1
    resource: repo://tests/test_devices.py
  - id: openwiki-source-a0d32ab49f0e7ff6b43fce77
    resource: repo://tests/test_player.py
generated: { by: "claude-code", at: "2026-10-16T04:21:31.514Z" }
---

# Audio Test Environments

Most of pyaudiocast's behavior can only be observed by actually playing
audio: `drain()` timing, `clear()` discarding audio, blocked writers being
released. The Python suite therefore needs a working output device, and the
project goes to some length to provide one in CI. What each test covers is in
[Test Suite Map](test-suite.md); this page is about the environment.

## Run or skip: `requires_audio`

`tests/conftest.py` probes once, when the test modules are imported:

```python
def _has_audio_device():
    try:
        player = pyspeaker.AudioPlayer()
        player.stop()
        return True
    except RuntimeError:
        return False

has_audio = _has_audio_device()
requires_audio = pytest.mark.skipif(not has_audio, reason="No audio output device available")
```

- The probe opens the **default device with default settings** — exactly
  what most tests do — so "audio available" means "the default device can be
  opened", not merely "a device is listed".
- Only `RuntimeError` counts as "no device" (every device or stream failure
  surfaces as `RuntimeError`).
- The package is imported as `pyspeaker`, the project's former name.

Tests that need no device always run: device listing, the
nonexistent-device and invalid-argument errors, the missing-file error, and
`__version__`. A few tests add their own conditions:

| Test | Extra condition |
|---|---|
| `test_select_device_by_id` | Linux only; skips if ALSA lists no `pulse` device |
| `test_select_sink_by_id` | Linux only; skips if `pactl` lists no sinks |
| `test_signal_handler_can_use_a_blocked_player` | not on Windows (POSIX signals) |

The Rust unit tests never open an output stream: player tests drive the
`Converter`, `Writer` and `Renderer` with in-memory ring buffers, and device
tests only enumerate and resolve devices (ALSA's configuration lists devices
such as `null` even without sound cards; the default-device test returns
early when there is none). `cargo test` therefore needs ALSA headers and
libpython but no sound server.

## How CI provides a device

### Linux: headless PulseAudio

The `python` job in `.github/workflows/ci.yml`:

1. installs `pulseaudio` and `libasound2-plugins` (which provides ALSA's
   `pulse` PCM);
2. sets `XDG_RUNTIME_DIR` (the daemon's socket directory) and exports it for
   later steps;
3. writes `~/.asoundrc` with `pcm.!default pulse` and `ctl.!default pulse`, so
   ALSA's default device — and therefore `AudioPlayer()` — plays into
   PulseAudio;
4. starts `pulseaudio --start --exit-idle-time=-1`, so the daemon never exits
   between tests;
5. unloads `module-suspend-on-idle`, so the sink never suspends (resuming a
   suspended sink would add start-up delay to timing tests);
6. prints `pactl info`.

On a runner without sound hardware the server plays into a **null sink**,
which consumes audio in real time without making a sound — timing assertions
hold and the sink shows up in `pactl list short sinks`, so the sink-routing
tests run too. If PulseAudio fails to start, the step only emits a warning
and the audio tests skip.

### macOS: the Null Audio Device

The job installs `switchaudio-osx` and runs
`SwitchAudioSource -t output -s "Null Audio Device"`. The VM's own sound
device sometimes plays a stream about 60× too slowly, which breaks timing
assertions; the null device keeps time.

### Windows

No setup. Audio tests run if the runner has a default output device and skip
otherwise.

### Locally

- **Linux desktop:** tests play to your default device. They only write
  silence, but they do take the device. To keep runs isolated you can create a
  PulseAudio/PipeWire null sink (`pactl load-module module-null-sink …`) and
  make it the default sink for the duration of the run.
- **CI-like:** reproduce the Linux steps above (PulseAudio,
  `libasound2-plugins`, `~/.asoundrc`) in a container.
- Then `maturin develop` and `pytest tests/ -v` from the repository root.

## Timing assertions

Many player tests measure how long `drain()` or `play_file()` takes:

| Assertion | Proves |
|---|---|
| 0.4 s written in 1001-byte or 999-sample chunks drains in 0.35–1.5 s | unaligned chunks lose no audio |
| 0.4 s of int16 zeros drains in more than 0.35 s | int16 arrays keep all samples |
| after `clear()`, 0.2 s of new audio drains in under 1 s | cleared audio is not replayed |
| `drain()` on an empty or cleared player takes under 0.5 s | it returns promptly |
| `KeyboardInterrupt` arrives under 1.5 s into a 3 s drain | signals are checked while blocked |
| a 0.2 s WAV takes 0.15–5 s in `play_file` | the file is played, and played to completion |

Lower bounds catch dropped audio; upper bounds catch replayed audio and
hangs. The windows are wide because `drain()` includes the device's output
latency and because CI machines are noisy.

Two helpers keep them meaningful:

- `silence(player, seconds)` builds float32 zeros sized from the player's
  `sample_rate` × `channels` — the caller format.
- `warmed_up(player)` writes and drains 50 ms of silence before the timed
  part, because a cold sound server can take a second to start a new stream.

Consequently the timing tests fail on devices that don't play in real time
(the macOS VM problem above), on sinks that suspend, or on heavily overloaded
machines — check the environment before suspecting the code.

## Isolation techniques

- **Subprocesses** (`sys.executable -c …`, so the extension must be
  installed in that interpreter):
  - `test_write_without_numpy` sets `sys.modules['numpy'] = None` before
    importing pyaudiocast — a state the test process itself, which has numpy
    loaded, cannot enter.
  - `test_signal_handler_can_use_a_blocked_player` installs a `SIGUSR1`
    handler that calls `stop()` or `write()` on a player blocked writing 6 s of
    audio (more than the 4 s ring), sends the signal from a timer after 0.5 s,
    and checks the script reaches `done` within 30 s — a deadlock would only
    kill the subprocess.
- **Threads:** cross-thread tests run the blocking call (`write`, `drain`,
  `clear`) in a `ThreadPoolExecutor` and wait on the future with a timeout, so a
  regression shows up as a timeout instead of hanging the suite. Ctrl+C is
  simulated with `threading.Timer(0.2, _thread.interrupt_main)`.

## When audio tests skip unexpectedly

1. Try `pyaudiocast.AudioPlayer()` in the same interpreter: the
   `RuntimeError` message says why the probe failed.
2. On Linux, check `pactl info` and that ALSA's default device reaches the
   sound server (the `pulse` plugin is installed).
3. In CI, look for the "No sound server: audio tests will be skipped" warning.
4. Run with `PYAUDIOCAST_LOG=debug` to see device selection
   ([Logging and Troubleshooting](../operations/logging-and-troubleshooting.md)).

## Related pages

- [Test Suite Map](test-suite.md)
- [Continuous Integration](../operations/continuous-integration.md)
- [PipeWire/PulseAudio Sink Routing](../devices/pipewire-sink-routing.md)
- [Logging and Troubleshooting](../operations/logging-and-troubleshooting.md)
