---
type: mechanism
title: Audio Callback and Rendering
description: The realtime half of AudioPlayer — the lock-free cpal data callback and its Renderer, which plays whole frames from the ring, skips audio below the clear() watermark with a 5 ms fade-out, pads with silence, converts to the device sample type, and publishes the consumed and audible_until values that drain() waits on.
tags: [realtime, audio-callback, renderer, ring-buffer, fade, lock-free, cpal]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-16T04:21:31.514Z
sources:
  - id: openwiki-source-ab97213db176711d486ab180
    resource: repo://src/player.rs
generated: { by: "claude-code", at: "2026-10-16T04:21:31.514Z" }
---

# Audio Callback and Rendering

Once `AudioPlayer` has opened its stream, cpal calls a data callback on the
OS audio thread every time the device needs another buffer. That callback is
built by `build_stream::<T>()` in `src/player.rs` and does its work through a
`Renderer`. This page explains what happens in that callback and why. The
Python-thread side that fills the ring is in
[Write Path and Input Formats](write-path-and-input-formats.md); the protocol
that ties both sides together is in
[Interruption, Drain and Stop Semantics](interruption-and-drain.md).

## Constraints

The callback runs on a realtime thread, so it is written to be lock-free and
allocation-free: it touches only

- the ring **consumer** (`HeapCons<f32>`, a lock-free SPSC ring from the
  `ringbuf` crate), which the `Renderer` owns outright, and
- the atomics in `Shared`.

It never takes the writer mutex, never allocates, never logs, and never
touches Python. Everything expensive — format detection, resampling, channel
mapping — already happened on the Python thread, so the ring holds
interleaved f32 samples in the **device's** rate and channel layout.

## One callback, step by step

```rust
move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
    let now = shared.now();
    shared.callbacks.fetch_add(1, Relaxed);              // heartbeat
    shared.failing_since.store(NOT_FAILING, Relaxed);    // stream is alive
    let n = renderer.render(data, || shared.discard_until.load(Acquire));
    if n > 0 { /* audible_until = now + latency + duration of n samples */ }
    shared.consumed.store(renderer.consumed, Release);   // after audible_until
}
```

1. **Heartbeat.** Incrementing `callbacks` tells waiting `write()`/`drain()`
   calls the device is alive; resetting `failing_since` restarts the
   error-throttling clock. Both are explained in
   [Blocking Waits and Stream Failure Handling](blocking-waits-and-stream-failures.md).
2. **Render** the buffer (below).
3. **Timing.** If audio was rendered, record when it will have been heard.
4. **Publish** how many samples have left the ring.

The callback runs from the moment `stream.play()` succeeds, rendering silence
while the ring is empty, so the heartbeat is present even before the first
`write()`.

## `Renderer::render`

The output buffer is filled in three regions:

```
data: [ fade-out of discarded audio | fresh audio | silence ]
        n samples (0 unless a clear()  m samples      EQUILIBRIUM
        cut audio that was playing)    (whole frames)
```

1. **Snapshot the ring, then read the watermark.** `as_slices()` gives the
   queued samples (`head` + `tail`, since the ring may wrap). Only *then* is
   `discard_until` loaded. The order matters: `push()` raises the watermark
   before it publishes samples that a concurrent `clear()` made stale, so any
   stale sample present in the snapshot is guaranteed to be covered by the
   watermark read afterwards.
2. **Count stale samples:** `stale = min(discard_until − consumed, queued)` —
   everything below the watermark that is in the ring now.
3. **Fade.** If there are stale samples *and* the previous buffer carried
   audio (`playing`), the first stale samples are played with a linear ramp
   instead of being cut, because an abrupt jump in the waveform clicks. The
   ramp covers `min(fade length, stale, buffer)` rounded down to whole frames,
   where the fade length is 5 ms of frames (`FADE`); frame *k* of *F* is scaled
   by `1 − k/F`. If nothing was playing, stale samples are dropped outright.
4. **Fresh audio.** The samples after the stale ones are copied, converted with
   `T::from_sample`, up to the space left — **rounded down to whole frames**.
   A partial frame stays in the ring until the rest of it arrives (the push
   loop may publish part of a frame when the ring is almost full), so channels
   can never rotate.
5. **Consume.** `skip(stale + m)` removes everything handled, including stale
   samples beyond the fade, and adds it to the renderer's `consumed` total.
   The stale count is deliberately *not* rounded to frames: skipping exactly up
   to the watermark puts the next sample read at the first frame of the new
   stream, even if an interrupted push left half a frame behind.
6. **Silence.** The rest of the buffer is filled with `T::EQUILIBRIUM` (zero
   for signed and float formats, the midpoint for unsigned ones).
7. `playing = m > 0`; return `n + m`, the number of samples that carried
   audio.

If `clear()` counted samples that `push()` had not published yet, the
watermark is ahead of the queue: this callback skips what is there, and later
callbacks keep skipping the late arrivals until `consumed` reaches the
watermark. Those late stale samples are not faded, because nothing is playing
any more.

### Worked example (from the unit tests)

With one channel and a 2-frame fade: six samples of `0.8` are queued, a
2-sample buffer plays two of them, then `clear()` sets the watermark to 6. The
next 4-sample buffer becomes `[0.8, 0.4, 0.0, 0.0]` — a two-frame ramp of the
discarded audio, then silence — and `consumed` is 6.

## Sample-format conversion

`open()` picks one monomorphized `build_stream::<T>` for the device's sample
format: `f32`, `f64`, `i8`, `i16`, `I24`, `i32`, `i64`, `u8`, `u16`, `U24`,
`u32` or `u64` (anything else is a `Config error`). Conversion from f32 happens
per sample in the callback via cpal's `FromSample`. All samples entering the
ring are already limited to −1.0..`MAX_SAMPLE`, where
`MAX_SAMPLE = 8388607/8388608`: the f32 → 24-bit conversion does not saturate,
so a full-scale `1.0` would wrap around to full-scale negative.

## Timing for `drain()`

When a callback renders audio (`n > 0`) it stores

```
audible_until = now + min(playback − callback, 1 s) + n × ns_per_sample
```

- `now` is nanoseconds since the player was created;
- `playback − callback` is cpal's estimate of the output latency (the time
  from this callback until its first sample reaches the device), saturating at
  zero; values above `MAX_LATENCY` (1 s) are treated as bogus and capped, so a
  broken report delays `drain()` by at most a second;
- `ns_per_sample = 1e9 / (rate × channels)`, so the last term is the duration
  of the audio rendered in this buffer.

**Store ordering.** `audible_until` is stored first and `consumed` second,
both with `Release`; `drain()` loads `consumed` first and `audible_until`
second, both with `Acquire`. So once `drain()` sees `consumed` reach its
target, it reads the `audible_until` of the callback that got it there (or a
later one) — never a stale value from an earlier buffer. How `drain()` uses
the value is in [Interruption, Drain and Stop Semantics](interruption-and-drain.md#drain).

## Error callback

`build_stream` also installs cpal's error callback. It does not render
anything: it classifies `cpal::Error`s (xruns and realtime-priority denials are
only logged; device loss fails the stream; other errors are throttled and fail
the stream after 5 s without audio). See
[Blocking Waits and Stream Failure Handling](blocking-waits-and-stream-failures.md#stream-errors-from-cpal).

## Tests

`src/player.rs` unit-tests the renderer directly with an f32 buffer and a
2-frame fade:

| Test | Shows |
|---|---|
| `renders_whole_frames_then_silence` | 5 queued samples, stereo: 2 whole frames play, the odd sample stays queued, the rest is silence. |
| `cleared_audio_is_skipped_and_new_audio_plays` | Not playing: cleared samples are dropped without a fade and the new sample plays immediately. |
| `clear_fades_out_playing_audio` | Playing: a two-frame ramp, then silence; all six samples consumed. |
| `a_watermark_past_the_queued_audio_skips_what_arrives_later` | A watermark ahead of the queue keeps skipping samples published later, then plays new audio. |

## Related pages

- [Interruption, Drain and Stop Semantics](interruption-and-drain.md)
- [Blocking Waits and Stream Failure Handling](blocking-waits-and-stream-failures.md)
- [Stream Setup and Config Negotiation](stream-setup-and-negotiation.md)
- [Write Path and Input Formats](write-path-and-input-formats.md)
