---
type: testing
title: Test Suite Map
description: How pyaudiocast is tested — Rust unit tests for the converter, writer, renderer, negotiation, device lookup and WAV handling, and Python integration tests that drive a live stream — mapped to the behaviors and past regressions each one guards, plus how to run and extend them.
tags: [testing, pytest, cargo-test, regression, coverage, test-map]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-16T04:21:31.514Z
sources:
  - id: openwiki-source-164e2da859b5277df81c7d94
    resource: repo://.github/workflows/ci.yml
  - id: openwiki-source-23775c3de52f3ab95a13cb8b
    resource: repo://README.md
  - id: openwiki-source-c1d203a608ca6409879479d1
    resource: repo://src/device.rs
  - id: openwiki-source-ed8bf05e307c6278442542c2
    resource: repo://src/lib.rs
  - id: openwiki-source-ab97213db176711d486ab180
    resource: repo://src/player.rs
  - id: openwiki-source-ea5f93364eb7225247c2fdd1
    resource: repo://tests/test_devices.py
  - id: openwiki-source-a0d32ab49f0e7ff6b43fce77
    resource: repo://tests/test_player.py
generated: { by: "claude-code", at: "2026-10-16T04:21:31.514Z" }
---

# Test Suite Map

## Two layers

| Layer | Where | Run with | Needs | Sees |
|---|---|---|---|---|
| Rust unit tests | `#[cfg(test)] mod tests` at the bottom of `src/lib.rs`, `src/device.rs`, `src/player.rs` | `cargo test` | ALSA headers; libpython (the tests link against it, so `python3-dev` on Linux) | Deterministic logic in isolation: resampling, channel mapping, writer carry-over, rendering, negotiation, device lookup, WAV truncation |
| Python integration tests | `tests/test_player.py`, `tests/test_devices.py`, `tests/conftest.py` | `pytest tests/` after `maturin develop` | An output device for most tests | The installed extension end to end, through a live stream, with real threads, the GIL and signals |

The split follows what each layer can observe. Concurrency, timing, GIL and
signal behavior only exist with a running stream and Python threads, so they
are tested from Python. The Python tests write **silence**, though: they verify
durations, exceptions and liveness, not the sample values reaching the
device. Sample-level correctness — interpolation accuracy, clamping, channel
order, fades — is pinned exactly by the Rust tests.

Whether the Python audio tests run or skip, and how CI gives them a device, is
covered in [Audio Test Environments](audio-test-environments.md).

## Rust unit tests

| Area | Tests | Pinned behavior |
|---|---|---|
| Resampling | `chunked_resampling_matches_one_shot`, `resampled_length_tracks_rate_ratio`, `cubic_resampling_tracks_the_signal`, `resampling_never_overshoots_full_scale` | Output independent of chunking (4 rate pairs × 4 chunk sizes); length follows the rate ratio; Catmull-Rom error < 2e-4 on a sine; no sample beyond −1.0..`MAX_SAMPLE` |
| Channels | `resampling_keeps_channels_apart`, `remaps_channels_by_position` | No crosstalk while resampling; mono → front pair, extra channels silent, downmix keeps the first channels, mono device gets the average |
| Writer | `writes_need_not_hold_whole_samples_or_frames`, `a_drained_stream_cut_mid_sample_does_not_misalign_the_next` | Odd bytes and partial frames carry over; a new epoch or a completed drain drops them |
| Renderer | `renders_whole_frames_then_silence`, `cleared_audio_is_skipped_and_new_audio_plays`, `clear_fades_out_playing_audio`, `a_watermark_past_the_queued_audio_skips_what_arrives_later` | Whole frames only; stale audio skipped, faded only if playing; a watermark ahead of the queue keeps skipping |
| Negotiation | `negotiate_prefers_user_config_then_default` | Caller config → caller rate at default channels → device default; f32 preferred |
| Devices | `test_find_default_device`, `test_find_nonexistent_device`, `every_listed_device_is_selectable_by_id` | Empty/whitespace query → default device; unknown query errors; every listed id resolves back to its device |
| WAV | `a_truncated_wav_reads_as_short` (`lib.rs`) | hound's error for a truncated file is classified as a short read |

The player tests build their subjects over in-memory rings with the helpers
`writer(channels)` and `renderer(samples, channels)` and generate input with
`sine()`, so none of them needs a device. Details:
[Resampling](../playback/resampling-and-channel-mapping.md),
[Audio Callback](../playback/audio-callback-and-rendering.md),
[Write Path](../playback/write-path-and-input-formats.md),
[Device Discovery](../devices/device-discovery-and-selection.md).

## Python integration tests

Many of these tests exist because of a specific past bug — their docstrings
say what used to go wrong. Grouped by behavior:

### Construction — `TestAudioPlayerInit`

| Test | Guards |
|---|---|
| `test_create_auto_detect` | Rate and channels default from the device; `is_active` flips on `stop()` |
| `test_create_partial_override_sample_rate` | Overriding only `sample_rate` |
| `test_create_nonexistent_device_raises` | `RuntimeError("Device not found…")` (no device needed) |
| `test_invalid_config_raises_value_error` | `sample_rate=0` / `channels=0` → `ValueError`; **used to panic deep in the resampler** |
| `test_context_manager`, `test_stop_is_idempotent` | `with` stops the player; double `stop()` is safe |

### Input formats — `TestAudioPlayerWrite`

| Test | Guards |
|---|---|
| `test_write_bytes`, `test_write_bytearray`, `test_write_list_float`, `test_write_numpy[...]` | Every accepted type and dtype |
| `test_write_numpy_2d_frames_by_channels` | `(frames, channels)` accepted; `(channels, frames)` raises `ValueError` |
| `test_write_numpy_non_contiguous` | Strided arrays |
| `test_write_misaligned_numpy` | Misaligned arrays; **used to be read in place (undefined behavior, a panic in debug builds)** |
| `test_write_unsupported_numpy_dtype_raises`, `test_write_unsupported_type_raises` | `TypeError` for `uint16` and for `str` |
| `test_write_without_numpy` | Works with numpy blocked; **rust-numpy's array check used to panic when numpy couldn't load** |
| `test_write_bytes_in_unaligned_chunks`, `test_write_partial_frames_carry_over` | 1001-byte / 999-sample chunks play for the full duration — odd lengths and split frames are carried over instead of raising or being dropped |
| `test_drain_empty_buffer` | `drain()` with nothing written returns at once |
| `test_write_after_stop_raises` | `RuntimeError` matching "closed" |

### Drain — `TestDrain`

| Test | Regression guarded |
|---|---|
| `test_drain_waits_for_buffered_audio` | `drain()` returned immediately when the callback ran while `write()` was still converting (a slow `__float__` stretches the conversion) |
| `test_numpy_int16_silence_keeps_its_length` | All-zero int16 arrays were read as raw bytes — half the samples |
| `test_drain_after_stop_returns` | `drain()` hung forever after `stop()` with audio buffered |
| `test_drain_ignores_audio_written_after_it` | `drain()` waited until the stream went idle, i.e. for later writes too |
| `test_ctrl_c_interrupts_drain` | `KeyboardInterrupt` within 1.5 s of Ctrl+C |

### Clear and barge-in — `TestAudioPlayerClear`

| Test | Regression guarded |
|---|---|
| `test_clear_then_write_drops_old_audio` | `write()` right after `clear()` resurrected the cleared audio |
| `test_empty_write_after_clear_keeps_audio_cleared` | An empty write must not bring cleared audio back |
| `test_barge_in_does_not_replay_interrupted_audio` | A writer blocked on a full ring kept pushing its stale chunk after `clear()` |
| `test_clear_from_another_thread` | Clearing from another thread raised `PanicException` (the class was thread-bound) |
| `test_clear_unblocks_drain_in_another_thread`, `test_clear_then_drain_returns_immediately` | `drain()` returns promptly after `clear()` |
| `test_stop_unblocks_writer_in_another_thread` | A writer larger than the ring is released by `stop()` with "closed" |
| `test_signal_handler_can_use_a_blocked_player` | A signal handler run during a blocked `write()` deadlocked on the writer lock |
| `test_clear_returns_without_error`, `test_write_after_clear_works` | Basic `clear()` use |

### `play_file` — `TestPlayFile`

| Test | Guards |
|---|---|
| `test_play_nonexistent_file_raises` | `RuntimeError` for a missing file (no device needed) |
| `test_play_file_accepts_pathlike` | `pathlib.Path` accepted; a 0.2 s file plays to completion |
| `test_play_file_shorter_than_its_header[truncated, pipe_header]` | Truncated and pipe-written files play what is there; **they used to raise after playing** |

`test_version` checks that `__version__` is a non-empty string.

### Devices — `tests/test_devices.py`

`test_list_output_devices_returns_list` and `test_device_dict_has_expected_keys`
check the dict shape, that `index` equals the position, that `type` is one of
`alsa`, `pipewire`, `wasapi`, `coreaudio`, and that ids are unique. On Linux,
`test_select_device_by_id` opens `device="pulse"` and `test_select_sink_by_id`
checks sink ids are `"pipewire:" + name` and opens the first sink.

## Running tests

```bash
maturin develop                      # rebuild after every Rust change: pytest imports the installed extension
pytest tests/ -v                     # from the repository root, as CI does
pytest tests/test_player.py -k drain -v
cargo test                           # Rust unit tests (Linux: libasound2-dev + python3-dev)
cargo test resampl                   # filter Rust tests by name
```

CI runs `cargo fmt --check`, `cargo clippy --all-targets -D warnings` and
`cargo test` on Linux, and pytest on Linux, macOS and Windows with Python
3.9/3.14/3.15 ([Continuous Integration](../operations/continuous-integration.md)).

## Gaps

- **Stream failures** — stall detection and the cpal error classification —
  have no automated tests; they need a device that dies or stops calling back
  ([Blocking Waits](../playback/blocking-waits-and-stream-failures.md)).
- **Sink-routing failure** (a sink with no `pulse`/`pipewire` device) is not
  tested.
- **Waveform content through a live stream** is not checked; Python tests
  write silence.
- **Coverage numbers** cover only the Python shim, not the Rust core.

## Adding tests

- **Pure logic** (conversion, rendering, alignment, negotiation, matching):
  add a Rust unit test next to the code, reusing `writer()`, `renderer()` and
  `sine()`.
- **Live-stream behavior:** add a Python test marked `@requires_audio`; call
  `warmed_up(player)` before timing anything; size audio with
  `silence(player, seconds)`; run cross-thread operations through a
  `ThreadPoolExecutor` with a timeout; use a subprocess for process-global
  state such as signal handlers or `sys.modules`. Name the regression in the
  docstring, as the existing tests do.

## Related pages

- [Audio Test Environments](audio-test-environments.md)
- [Continuous Integration](../operations/continuous-integration.md)
- [Interruption, Drain and Stop Semantics](../playback/interruption-and-drain.md)
- [Write Path and Input Formats](../playback/write-path-and-input-formats.md)
- [Resampling and Channel Mapping](../playback/resampling-and-channel-mapping.md)
