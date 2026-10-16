---
type: operations
title: Logging and Troubleshooting
description: How to turn on pyaudiocast's Rust-side logging with PYAUDIOCAST_LOG, what each subsystem logs at which level and under which target, and a lookup table from error messages, warnings and symptoms to their causes and fixes.
tags: [logging, troubleshooting, diagnostics, env-logger, errors, operations]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-16T04:21:31.514Z
sources:
  - id: openwiki-source-651d1fb6c9e49916a916ab51
    resource: repo://Cargo.toml
  - id: openwiki-source-c1d203a608ca6409879479d1
    resource: repo://src/device.rs
  - id: openwiki-source-95f84abff7e2ed283d50b3fb
    resource: repo://src/error.rs
  - id: openwiki-source-ed8bf05e307c6278442542c2
    resource: repo://src/lib.rs
  - id: openwiki-source-ab97213db176711d486ab180
    resource: repo://src/player.rs
generated: { by: "claude-code", at: "2026-10-16T04:21:31.514Z" }
---

# Logging and Troubleshooting

## Enabling logs

The Rust core logs through the `log` crate to an `env_logger` writing to
**stderr**; it does not go through Python's `logging` module.

```bash
PYAUDIOCAST_LOG=debug python my_script.py   # everything
PYAUDIOCAST_LOG=info  python my_script.py   # device choice, negotiated config, start/stop
python my_script.py                         # default: warnings and errors only
```

How it is wired (`src/lib.rs`, `init_logging`):

- The logger is built once, on the **first call** into the library —
  `list_output_devices()`, `play_file()` or `AudioPlayer(...)` — not at import.
  Setting `os.environ["PYAUDIOCAST_LOG"]` in Python works if it happens before
  that first call; later changes have no effect.
- The default level is `warn`; `PYAUDIOCAST_LOG` overrides it using
  env_logger's filter syntax. Colour follows env_logger's usual
  `RUST_LOG_STYLE` variable (`auto`/`always`/`never`).
- It is installed with `try_init`, so if a global logger is already installed
  the call is a silent no-op.

### Filter syntax specifics

- Log targets are Rust module paths under the **library crate name
  `_pyaudiocast`** (not `pyaudiocast`): `_pyaudiocast` (module entry and
  `play_file`), `_pyaudiocast::device`, `_pyaudiocast::player`. Example:
  `PYAUDIOCAST_LOG=warn,_pyaudiocast::device=debug` shows device matching in
  detail and everything else at warn.
- env_logger is built without its default `regex` feature, so the optional
  `/text` suffix is a plain substring match on the message:
  `PYAUDIOCAST_LOG=debug/drain` keeps only messages containing "drain".

## What gets logged

| Level | Target | Messages |
|---|---|---|
| error | `player` | `Stream error: <reason>` — once per player, when its stream is latched as failed |
| warn | `device` | `Device not found: <query>` |
| warn | `player` | First recoverable stream error per player: `Stream error: <cpal error>` |
| warn | `player` | `The previous audio ended mid-sample or mid-frame; dropping the remainder` |
| warn | `_pyaudiocast` | `play_file: file ends before its declared length` |
| info | `_pyaudiocast` | `Found N output device(s)`, `play_file: path=…, device=…`, `Playback finished` |
| info | `device` | `Found device: …`, ambiguous-substring notice (`'<q>' matches … and also [...]; pass an id from list_output_devices() to pick one`), `Routing to sink '<sink>' via the <id> device` |
| info | `player` | `AudioPlayer::new(device=…, sample_rate=…, channels=…)`, `Config: user=<rate>Hz <ch>ch, device=<rate>Hz <ch>ch <format>`, `Stream started`, `Stopping AudioPlayer` |
| debug | `device` | `pactl not available or failed`, device counts, `Using default output device` |
| debug | `player` | `write: N f32 samples` (input samples after alignment, before conversion), `drain: waiting for sample N`, `drain: complete`, `clear: discarding buffered audio`, `Stream xrun`, realtime-priority denials, repeated recoverable stream errors |
| debug | `_pyaudiocast` | `Listing output devices`, WAV spec (`WAV: <rate>Hz, <ch>ch, <format>, <bits>bps`) |

The `Config:` info line is the quickest way to see the caller format versus
the format the device was actually opened at
([Stream Setup](../playback/stream-setup-and-negotiation.md)).

## Error messages

Every Rust-side error becomes `RuntimeError` with one of five prefixes
(`Device not found:`, `No default output device available`, `Stream error:`,
`Config error:`, `IO error:`); argument errors are `ValueError`/`TypeError`.

| Message | Raised by | Cause | What to do |
|---|---|---|---|
| `Device not found: <query>` | constructor, `play_file` | Nothing matched the `device=` string ([matching rules](../devices/device-discovery-and-selection.md#resolving-a-device-query)). On Linux, sink names need `pactl`. | Call `list_output_devices()` and pass an `id`. Check `pactl list short sinks` works. |
| `Device not found: <sink> (a PipeWire/PulseAudio sink, but neither the pulse nor the pipewire ALSA device exists to route to it)` | constructor | The sink exists, but there is no ALSA `pulse`/`pipewire` PCM to route through ([Sink Routing](../devices/pipewire-sink-routing.md)). | Install the sound server's ALSA plugin package. |
| `No default output device available` | constructor | The host reports no default output. | Pass a device explicitly; check the sound server is running. |
| `Stream error: No audio host available` | anything touching devices | cpal found no audio host. | Check the platform audio stack. |
| `Config error: <host:id>: …` | constructor | The device rejected the stream configuration, or only offers a sample format pyaudiocast cannot drive (e.g. DSD). | Try another device id, or leave `sample_rate`/`channels` unset. |
| `Stream error: <host:id>: …` | constructor | Opening or starting the stream failed — for example an ALSA `hw:` PCM held by PipeWire. | Prefer the sink or the `pulse`/`pipewire` device over `hw:` devices on a desktop. |
| `Stream error: Player is closed` | `write()` | The player was stopped, before or during the call. | Create a new player. |
| `Stream error: Audio device stopped responding` | `write()`, `drain()` | No audio callback arrived for 5 s while the call waited ([stall detection](../playback/blocking-waits-and-stream-failures.md)). | The device is suspended, frozen or gone; recreate the player. |
| `Stream error: <cpal error>` | `write()`, `drain()` | cpal reported the device unavailable, the stream invalidated or the host unavailable, or kept reporting errors with no audio for over 5 s (e.g. the sound server restarted). | Recreate the player once the device is back. `is_active` is `False` from now on. |
| `IO error: …` | `play_file` | The WAV file could not be opened (raised immediately) or decoded (raised after the decodable part has played). | Check the path and file format. |
| `sample_rate must be in 1000..=768000 Hz`, `channels must be in 1..=64` (`ValueError`) | constructor, `play_file` | Out-of-range argument or WAV header. | Fix the argument. |
| `Expected bytes, bytearray, list[float] or a numpy array (int16/int32/float32/float64)` (`TypeError`) | `write()` | Unsupported type: `str`, `memoryview`, `array.array`, a numpy array while numpy isn't imported, … | Convert to one of the accepted types. |
| `Unsupported numpy dtype: <dtype>. Expected float32, float64, int16, or int32` (`TypeError`) | `write()` | e.g. `uint16`, `int8`, `float16`. | `arr.astype(np.float32)` (scaled to −1..1) or a supported integer type. |
| `Expected a 1-D array or a 2-D array shaped (frames, <channels>), got shape …` (`ValueError`) | `write()` | A `(channels, frames)` array, a channel count different from the player's, or 3-D input. | Transpose, or create the player with the data's channel count. |

## Symptoms without an exception

| Symptom | Likely cause |
|---|---|
| Audio too fast/slow or wrong pitch | The data's rate differs from the player's `sample_rate` (the caller format). Construct the player with the data's real rate. |
| Garbled or swapped channels | Data not interleaved in the player's `channels`, or a 2-D array in `(channels, frames)` order. |
| `write()` returned but part of the audio never played | A `clear()` happened during the call; the rest of that chunk was discarded by design ([Interruption](../playback/interruption-and-drain.md)). |
| Warning about audio ending mid-sample or mid-frame | A stream ended (`drain()` completed) with an odd byte or partial frame pending; the remainder was dropped so the next stream starts aligned. |
| `debug: Stream xrun` messages, crackles | Buffer underruns: the producer is not keeping up. Write larger chunks or earlier. |
| A short click when calling `clear()` | Expected to be minimal: the playing audio fades out over 5 ms. |
| Other Python threads pause ~20 ms when opening a sink | Expected: sink opens hold the GIL ([Sink Routing](../devices/pipewire-sink-routing.md#safety-argument)). |
| Ctrl+C does not interrupt a blocked call | CPython delivers signals to the main thread; a call blocked in a worker thread is not interrupted. |
| Python audio tests all skipped | No default output device could be opened ([Audio Test Environments](../testing/audio-test-environments.md)). |
| Two players' constructors are slow together | Device lookups and opens are serialized by a global lock. |

## Related pages

- [Native Extension Boundary (PyO3)](../architecture/native-extension-boundary.md)
- [Blocking Waits and Stream Failure Handling](../playback/blocking-waits-and-stream-failures.md)
- [Device Discovery and Selection](../devices/device-discovery-and-selection.md)
- [PipeWire/PulseAudio Sink Routing](../devices/pipewire-sink-routing.md)
- [Audio Test Environments](../testing/audio-test-environments.md)
