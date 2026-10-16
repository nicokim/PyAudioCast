---
type: mechanism
title: Write Path and Input Formats
description: How AudioPlayer.write() turns bytes, bytearray, list[float] and numpy arrays into normalized f32 frames — input detection with optional numpy, shape/dtype/alignment/contiguity handling, sanitization, carry-over of half samples and partial frames between writes, and the blocking push loop into the ring buffer.
tags: [write, input-formats, numpy, pcm, sanitization, ring-buffer, alignment]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-16T04:21:31.514Z
sources:
  - id: openwiki-source-ab97213db176711d486ab180
    resource: repo://src/player.rs
  - id: openwiki-source-a0d32ab49f0e7ff6b43fce77
    resource: repo://tests/test_player.py
generated: { by: "claude-code", at: "2026-10-16T04:21:31.514Z" }
---

# Write Path and Input Formats

`AudioPlayer.write(data)` is the producer side of the player. It accepts
several Python representations of audio, normalizes them to f32, completes
samples and frames split across calls, converts to the device format, and
pushes the result into the ring buffer, blocking while the ring is full.

```rust
fn write(&self, py: Python<'_>, data: &Bound<'_, PyAny>) -> PyResult<()> {
    let epoch = self.shared.epoch.load(Ordering::SeqCst);
    self.shared.check()?;
    let samples = extract_samples(data, self.channels as usize)?;
    py.detach(|| self.push(samples, epoch))
}
```

| Phase | GIL | Where |
|---|---|---|
| Capture the epoch; fail if closed or failed | held | `write` |
| Copy the input out of the Python object | held | `extract_samples` |
| Decode int16 bytes; complete carried sample/frame | released, writer lock | `Writer::begin`, `Writer::frames` |
| Resample and remap to the device format | released, writer lock | `Converter::process` |
| Push into the ring, waiting while full | released, writer lock per step | `push` loop |

Capturing the epoch first means a `clear()` that lands at any point after the
call starts — even during extraction — discards this write's data
([Interruption, Drain and Stop](interruption-and-drain.md)).

## Recognizing the input (`extract_samples`)

Checked in this order:

| Input | Becomes | Notes |
|---|---|---|
| `bytes` | `Samples::Pcm16` (copied) | int16 little-endian PCM; decoded later without the GIL |
| `bytearray` | `Samples::Pcm16` (copied) | same |
| `list` | `Samples::Float` | every element converted with PyO3's f32 extraction, then sanitized |
| `numpy.ndarray` | `Samples::Float` | only if numpy is already loaded (below) |
| anything else | `TypeError` | `Expected bytes, bytearray, list[float] or a numpy array (int16/int32/float32/float64)` |

Only these types are accepted: `memoryview`, `array.array`, tuples and numpy
scalars raise `TypeError`. The data is copied because the conversion runs
after the GIL is released and the Python object may change or disappear.

### numpy is optional

numpy is an optional extra. rust-numpy's array type check panics if numpy
cannot be loaded, so `extract_samples` only asks it about arrays when
`sys.modules["numpy"]` exists and is not `None` — an ndarray cannot exist
before numpy is imported anyway. Setting `sys.modules["numpy"] = None`
(which blocks the import) is how `test_write_without_numpy` proves lists and
bytes still work and that a wrong type still raises `TypeError`.

An earlier version read numpy int16 arrays through the bytes path, so
all-zero int16 arrays lost half their samples; arrays now always take the
numpy path (`test_numpy_int16_silence_keeps_its_length`).

### numpy arrays (`numpy_to_f32`)

- **Shape.** 1-D (interleaved, any length) or 2-D shaped `(frames, channels)`
  with `channels` equal to the player's `channels`. Anything else — including
  `(channels, frames)` — raises `ValueError: Expected a 1-D array or a 2-D
  array shaped (frames, N), got shape [...]`.
- **dtype.**

  | dtype | Conversion |
  |---|---|
  | `float32` | `sanitize` |
  | `float64` | cast to f32, `sanitize` |
  | `int16` | `/ 32768` |
  | `int32` | `/ 2^31`, capped at `MAX_SAMPLE` (`i32::MAX / 2^31` rounds to 1.0 in f32) |
  | other | `TypeError: Unsupported numpy dtype: <dtype>. Expected float32, float64, int16, or int32` |

- **Alignment.** A misaligned array (for example `np.frombuffer` at an odd
  offset) is copied with `.copy()` first: reading it in place would be
  undefined behaviour, and was a panic in debug builds.
- **Contiguity.** A C-contiguous array is read straight from its slice;
  anything else (strided views like `a[::2]`, Fortran order) is iterated in
  logical C order, so `(frames, channels)` data always comes out interleaved.

## Normalization

All float input passes through `sanitize`:

```rust
pub(crate) fn sanitize(s: f32) -> f32 {
    if s.is_finite() { s.clamp(-1.0, MAX_SAMPLE) } else { 0.0 }
}
```

- NaN or ±inf from a misbehaving generator would reach the device as
  full-scale noise; they become silence.
- `MAX_SAMPLE = 8388607/8388608`, just below 1.0: the f32 → 24-bit conversion
  used for some devices does not saturate, and 1.0 would wrap around to
  full-scale negative.

int16 bytes are decoded as `i16::from_le_bytes(pair) / 32768`, which is
already within range.

## Carry-over between writes (`Writer`)

Streamed audio — an HTTP body, a TTS engine's output — arrives in chunks that
need not hold whole samples or whole frames. The `Writer` (behind the player's
writer mutex) carries the leftovers:

- `odd_byte`: the trailing byte of an odd-length int16 chunk, prepended to
  the next `bytes`/`bytearray` write;
- `partial`: trailing samples that do not fill a whole frame (in the caller's
  channel count), prepended to the next write;
- `converter`: the resampler state ([Resampling](resampling-and-channel-mapping.md));
- `epoch`: the epoch this carried state belongs to.

`Writer::frames()` returns only whole frames, so the converter always sees
frame-aligned input. From the unit test, for a stereo writer:

| Write | Returned frames | Carried afterwards |
|---|---|---|
| `Pcm16([0x00, 0x40, 0x00])` | `[]` | sample `0.5` as a partial frame, odd byte `0x00` |
| `Pcm16([0x40, 0x00])` | `[0.5, 0.5]` | odd byte `0x00` |
| `Float([0.1, 0.2, 0.3])` | `[0.1, 0.2]` | partial `[0.3]`; the odd byte stays (float writes don't touch it) |
| `Float([0.4])` | `[0.3, 0.4]` | partial empty; odd byte `0x00` |

`Writer::begin()` runs first in every push and decides whether the carried
state still applies. If the write's epoch differs from the stored one (a
`clear()`/`stop()` happened) or a completed `drain()` ended the stream, it
resets the converter and drops `odd_byte` and `partial` — logging a warning in
the drain case if a remainder is dropped. Otherwise the new chunk continues the
stream. Details in
[Interruption, Drain and Stop Semantics](interruption-and-drain.md#when-a-stream-ends-and-restarts).

## The push loop

After conversion (still holding the writer lock) `push()` releases the lock
and loops:

1. Lock the writer. If the epoch changed, return `check()` — `Ok` after a
   `clear()`, an error after `stop()` or a stream failure — dropping the rest.
2. Take `n = min(free space, remaining)` samples, add `n` to `written`
   **before** publishing them, re-check the epoch (raising the discard
   watermark if a clear raced in), and `push_slice` them into the ring.
3. Unlock. If everything is in, return `Ok`.
4. `check()` for closed/failed, then wait one 5 ms poll (running signal
   handlers every 100 ms and detecting stalls; see
   [Blocking Waits](blocking-waits-and-stream-failures.md)).

Properties of this loop:

- **The writer lock is never held while waiting.** A signal handler or
  finalizer that runs meanwhile can use the player.
- **`written` counts device-format samples.** The ring holds f32 samples after
  conversion, so counters advance in device samples, not caller samples or
  frames.
- **Partial frames may be published** when the ring is nearly full; the
  callback only plays whole frames, so the remainder waits for the next step.
- **`write()` returns when the data is queued, not played.** The ring holds
  four seconds of device audio; a longer write keeps blocking until the
  callback frees space. Use `drain()` to wait for playback.
- **An empty write is valid**: it pushes nothing, but still runs `begin()`, so
  it can end a stream after a `clear()` or `drain()`.

## Tests

| Test | Covers |
|---|---|
| `test_write_bytes`, `test_write_bytearray`, `test_write_list_float`, `test_write_numpy[int16/int32/float32/float64]` | Each accepted type |
| `test_write_numpy_2d_frames_by_channels` | `(frames, channels)` accepted; `(channels, frames)` raises `ValueError` mentioning "frames" |
| `test_write_numpy_non_contiguous`, `test_write_misaligned_numpy` | Strided and misaligned arrays |
| `test_write_unsupported_numpy_dtype_raises`, `test_write_unsupported_type_raises` | `TypeError` for `uint16` and for a `str` |
| `test_write_without_numpy` | numpy blocked: lists work, wrong types still raise `TypeError` |
| `test_write_bytes_in_unaligned_chunks`, `test_write_partial_frames_carry_over` | 1001-byte and 999-sample chunks play for the right duration |
| `test_numpy_int16_silence_keeps_its_length` | int16 arrays keep all their samples |
| `writes_need_not_hold_whole_samples_or_frames`, `a_drained_stream_cut_mid_sample_does_not_misalign_the_next` (Rust) | `Writer` carry-over and resets |

## Extension seams

To accept a new input type, add a branch to `extract_samples` before the
final `TypeError`, doing as little as possible under the GIL (copy the data
out; defer conversion), and produce `Samples::Float` — or a new `Samples`
variant decoded in `Writer::frames`. Update the `TypeError` message, the
`write()` signature in `pyaudiocast/_pyaudiocast.pyi`, the README format table,
and add a test.

## Related pages

- [Resampling and Channel Mapping](resampling-and-channel-mapping.md)
- [Interruption, Drain and Stop Semantics](interruption-and-drain.md)
- [Blocking Waits and Stream Failure Handling](blocking-waits-and-stream-failures.md)
- [Audio Callback and Rendering](audio-callback-and-rendering.md)
- [Python API Reference](../api/python-api.md)
