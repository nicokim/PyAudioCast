---
type: algorithm
title: Resampling and Channel Mapping
description: How the stateful Converter turns the caller's sample rate and channel count into the device's — exact integer positions that make Catmull-Rom resampling independent of chunking, priming and reset at stream boundaries, a two-frame look-ahead with no end-of-stream flush, clamping, and positional upmix/downmix rules.
tags: [resampling, catmull-rom, channel-mapping, upmix, downmix, dsp, converter]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-16T04:21:31.514Z
sources:
  - id: openwiki-source-ab97213db176711d486ab180
    resource: repo://src/player.rs
generated: { by: "claude-code", at: "2026-10-16T04:21:31.514Z" }
---

# Resampling and Channel Mapping

When the device runs at a different rate or channel count than the caller's
audio, `write()` converts the audio before it enters the ring. The work is
done by `Converter` in `src/player.rs`, owned by the player's `Writer` and
called on the writing thread (GIL released, writer lock held) for every
`write()`. The audio callback never resamples.

```text
caller frames (sample_rate × channels)
    └─ Writer::frames()      whole frames only (see Write Path)
        └─ Converter::process()
              resample: Catmull-Rom at exact integer positions
              remap:    per output frame, by channel position
            └─ ring (device_sample_rate × device_channels, f32)
```

The converter is created in `open()` as
`Converter::new(sample_rate, device_sample_rate, channels, device_channels)`.
Whether conversion is needed at all is decided by
[config negotiation](stream-setup-and-negotiation.md): off macOS, the device
is opened at the caller's rate and channel count whenever it supports them.

## Same rate: passthrough or remap only

If both rates are equal, `process()` returns the input untouched (same
channel count) or remaps each frame. No history is kept and nothing is held
back, so this path is stateless.

## Different rates: exact-position cubic resampling

### Position units

The converter tracks where the next output frame falls as an integer `pos`
measured in **1/`dst_rate` of a source frame**:

- source frame *i* sits at `pos = i × dst_rate`;
- each output frame lasts `src_rate / dst_rate` source frames, so `pos`
  advances by exactly `src_rate` per output frame;
- the integer part `pos / dst_rate` is the source frame, the remainder gives
  the fraction `t = (pos % dst_rate) / dst_rate`.

Because positions are exact integers — no accumulated floating-point step —
the sequence of output instants is the same however the input is split into
`write()` calls. That is the property the
`chunked_resampling_matches_one_shot` test checks: for 22.05→48 kHz,
44.1→48 kHz, 48→44.1 kHz and 96→48 kHz, feeding chunks of 1, 7, 160 or 1024
frames yields the same number of samples as one call, with values within 1e-5.

### State carried between calls

Each call works in a virtual frame index space: indices 0–2 are the
`history` (the last `HISTORY = 3` source frames of previous input, oldest
first) and the new input starts at index 3. After the call:

- `history` becomes the last three frames of that combined sequence;
- `pos` is shifted back by `n × dst_rate` (n = input frames) into the next
  call's coordinates.

### Interpolation

For each output position the converter evaluates a 4-point Catmull-Rom
spline — a cubic Hermite curve through frames *i* and *i+1* with tangents
taken from *i−1* and *i+2* — independently per channel:

```rust
fn catmull_rom(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}
```

- **Clamped.** The curve can overshoot the input range, so every
  interpolated sample is clamped to −1.0..`MAX_SAMPLE` (the largest value every
  output format accepts; see [Audio Callback](audio-callback-and-rendering.md#sample-format-conversion)).
- **Accuracy.** For a 1 kHz sine upsampled from 24 kHz to 48 kHz the test
  requires an error below 2e-4; its comment puts Catmull-Rom at about 5e-5
  versus about 4e-3 for linear interpolation.
- **No anti-aliasing filter.** Nothing band-limits the signal before
  downsampling, so content above the new Nyquist frequency aliases. The code
  marks this as a deliberate ceiling — fine for upsampling speech, the common
  TTS case — with a sinc resampler (for example `rubato`) as the upgrade path
  if downsampling quality matters.

### Look-ahead and the missing flush

Interpolating at a position needs the two source frames after it, so a call
only emits positions before `(HISTORY + n − 2) × dst_rate` — output for the
last two source frames of each call waits for the next call. Consequences:

- across a stream, the output length tracks the rate ratio within a few
  samples (`resampled_length_tracks_rate_ratio`: 100 × 2205 frames at
  22.05→48 kHz gives 480 000 ± 6);
- **there is no flush**: when a stream ends (a completed `drain()`, a
  `clear()`, or `stop()`), the held-back output for the final two source
  frames is never emitted — at 22.05 kHz that is under a tenth of a
  millisecond.

### Priming and reset

A fresh converter, or one that was `reset()`, is unprimed. On its next
non-empty input it fills the history with copies of that input's **first
frame** and sets `pos` exactly on it. The first output sample therefore equals
the first input sample (t = 0 returns `y1`) and the stream does not ramp up
from zero, which would click.

`reset()` only clears the `primed` flag. `Writer::begin` calls it whenever a
stream ends — a new epoch after `clear()`/`stop()`, or a write after a
completed `drain()` — so resampler history from old audio never bleeds into
new audio (see [Interruption, Drain and Stop](interruption-and-drain.md#when-a-stream-ends-and-restarts)).

## Channel mapping (`remap`)

Every output frame goes through `remap(frame, dst_ch)`, which maps channels
**by position**, relying on front-left/front-right coming first in every
backend's channel layout:

| Source → device | Result |
|---|---|
| any → mono | average of all source channels |
| mono → 2 or more | the sample on both front channels; any further channels silent |
| N → M, M ≥ N | the N channels in order; extra channels (centre, LFE, rears) silent |
| N → M, 2 ≤ M < N | the first M channels; the rest are dropped |

Examples from `remaps_channels_by_position`: mono → 5.1 gives
`[s, s, 0, 0, 0, 0]` (never the LFE); stereo → 4 gives `[L, R, 0, 0]`; stereo →
mono gives the average; 5.1 → stereo keeps the first two channels.

Known limitation, flagged in the code: downmixing 5.1 to stereo **drops the
centre channel** (where dialogue usually lives). Per-layout downmix matrices
are the stated upgrade path if multichannel sources become common.

Resampling and remapping keep channels independent:
`resampling_keeps_channels_apart` resamples constant stereo `(0.5, −0.5)` from
44.1 to 48 kHz and requires every output frame to stay `(0.5, −0.5)`.

## Tests

| Test (`src/player.rs`) | Property |
|---|---|
| `chunked_resampling_matches_one_shot` | Chunking never changes output length or values |
| `resampled_length_tracks_rate_ratio` | Output length follows the rate ratio |
| `cubic_resampling_tracks_the_signal` | Interpolation accuracy on a sine |
| `resampling_never_overshoots_full_scale` | Clamping on a full-scale square wave |
| `resampling_keeps_channels_apart` | No channel crosstalk |
| `remaps_channels_by_position` | Upmix/downmix rules above |

## Extension seams

- A higher-quality resampler (sinc, band-limited) can replace the
  interpolation loop inside `Converter::process` as long as it keeps the
  carried-state contract: identical output under any chunking, `reset()` at
  stream boundaries, whole frames in and out.
- Layout-aware downmixing would replace `remap`, which is the single place
  where channel counts change.

## Related pages

- [Write Path and Input Formats](write-path-and-input-formats.md)
- [Stream Setup and Config Negotiation](stream-setup-and-negotiation.md)
- [Interruption, Drain and Stop Semantics](interruption-and-drain.md)
- [Test Suite Map](../testing/test-suite.md)
