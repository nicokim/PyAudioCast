---
type: mechanism
title: Blocking Waits and Stream Failure Handling
description: How AudioPlayer's blocking write() and drain() wait without the GIL — 5 ms polls, 100 ms signal checks, poll-counted stall detection — and how cpal stream errors are classified, throttled, latched into the player and surfaced as RuntimeError and is_active=False.
tags: [blocking, polling, signals, stall-detection, errors, cpal, reliability]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-16T04:21:31.514Z
sources:
  - id: openwiki-source-95f84abff7e2ed283d50b3fb
    resource: repo://src/error.rs
  - id: openwiki-source-ab97213db176711d486ab180
    resource: repo://src/player.rs
  - id: openwiki-source-a0d32ab49f0e7ff6b43fce77
    resource: repo://tests/test_player.py
generated: { by: "claude-code", at: "2026-10-16T04:21:31.514Z" }
---

# Blocking Waits and Stream Failure Handling

Two calls can block for a long time: `write()` while the ring is full, and
`drain()` while queued audio plays out (`play_file` blocks through both). This
page covers how they wait, how they stay interruptible, how a dead device is
detected, and how errors reported by cpal turn into Python exceptions. The
conditions they wait *for* are described in
[Interruption, Drain and Stop Semantics](interruption-and-drain.md).

## Timing constants (`src/player.rs`)

| Constant | Value | Role |
|---|---|---|
| `POLL` | 5 ms | Sleep between re-checks in a blocking loop |
| `SIGNAL_CHECK` | 100 ms | Interval between Python signal checks while blocked |
| `STALL_TIMEOUT` | 5 s | No audio callback for this long while waiting ⇒ device considered dead |
| `STALL_POLLS` | 1000 | `STALL_TIMEOUT / POLL`: the stall threshold, counted in polls |
| `THROTTLE_ERRORS_AFTER` | 1 s | Recoverable errors with no callback for this long ⇒ start throttling |
| (literal) | 50 ms | Sleep per error while throttling |
| `NOT_FAILING` | `u64::MAX` | `failing_since` sentinel: no error since the last callback |

## Why poll?

The audio callback must stay lock-free (see
[Audio Callback and Rendering](audio-callback-and-rendering.md)), so it cannot
signal a condition variable. Waiters therefore poll the shared atomics every
5 ms. The code records the trade-off: a futex-style wake-up would shave at most
5 ms off `drain()`.

## The wait loops

Both loops have the same shape — check exit conditions, check health, wait:

```rust
// push loop (write)                         // drain loop
loop {                                       loop {
    /* lock writer; epoch check;                 /* cleared/closed? return Ok */
       push what fits; unlock */                 /* consumed + audible? return Ok */
    if offset == samples.len() { return Ok(()); }
    shared.check()?;                             shared.check()?;
    waiter.wait()?;                              waiter.wait()?;
}                                            }
```

`shared.check()` runs before every sleep, so a stop or a latched failure is
noticed within one poll. Both loops run with the GIL released
([Native Extension Boundary](../architecture/native-extension-boundary.md#gil-policy)).

### `Waiter::wait`

A `Waiter` is created at the start of each blocking call and remembers the
callback count it last saw. Each `wait()`:

1. sleeps `POLL`;
2. compares `Shared.callbacks` (incremented by every audio callback) with the
   remembered value — a change resets the stall counter, no change increments
   it, and reaching `STALL_POLLS` calls
   `fail("Audio device stopped responding")`;
3. if `SIGNAL_CHECK` has elapsed since the last check, attaches to Python with
   `Python::try_attach` and calls `check_signals()`, returning any exception
   raised by a signal handler.

## Stall detection

- **Counted, not timed.** The stall threshold is a number of polls, not a wall
  clock duration. If the process is frozen — Ctrl+Z, a debugger breakpoint —
  wall time keeps running while no callbacks arrive; measuring time would
  declare the device dead the moment the process resumes. Counting polls only
  counts time the waiter actually spent awake and waiting.
- **Generous on purpose.** 5 s avoids flagging slow device start-up (for
  example Bluetooth); the code notes it can be lowered if faster failure
  detection matters.
- **Only while waiting.** Stall detection lives in `Waiter`, so it runs only
  while a `write()` waits for space or a `drain()` waits for playback. A
  `write()` that finds room in the ring, or an idle player, does no stall
  check.

## Stream errors from cpal

The error callback installed by `build_stream` classifies each `cpal::Error`:

| `ErrorKind` | Handling |
|---|---|
| `Xrun` | `debug` log only (`Stream xrun`) |
| `RealtimeDenied` | `debug` log only |
| `DeviceNotAvailable`, `StreamInvalidated`, `HostUnavailable` | **fatal**: `fail(err)` immediately |
| anything else (e.g. `DeviceChanged`, after which cpal reroutes to the new default) | **recoverable**, with throttling (below) |

For recoverable errors the callback tracks how long errors have been arriving
with no audio callback in between:

- The first error after an audio callback stores its time in
  `failing_since` (a compare-and-swap from `NOT_FAILING`); every audio
  callback resets it to `NOT_FAILING`.
- While errors have been arriving for **≤ 1 s**: the first recoverable error in
  the player's life is logged at `warn`, later ones at `debug`.
- After **> 1 s**: each error sleeps 50 ms before returning. When a sound
  server drops the stream, cpal retries in a busy loop; the sleep throttles it.
- After **> 5 s**: the stream is failed with that error's message.

The clock starts at the **first error**, not at the last callback: a process
that was frozen for a while gets a burst of errors when it resumes and then
recovers, and timing from the last callback would fail it instantly.

## Latching and surfacing failures

`Shared::fail(msg)` locks the error mutex and, only if the stream has not
failed yet, stores the message and then sets `failed` (release ordering). The
message is written before the flag so that `check()`, which reads the flag
with acquire ordering, always finds the message. The first failure wins;
later ones are ignored. It also logs the message once at error level.

`Shared::check()` turns state into exceptions:

| State | Raised (`RuntimeError`) |
|---|---|
| `closed` (after `stop()`) | `Stream error: Player is closed` |
| `failed` | `Stream error: <first failure message>` |

Where it runs and what the caller sees:

- `write()` checks at entry, so writing to a stopped or failed player fails
  immediately; the push loop checks before each wait; and when a push notices
  a new epoch it returns `check()`'s result — `Ok` after a `clear()`, an error
  after a `stop()` or a failure.
- `drain()` first returns `Ok` if the player was cleared or closed, and only
  then checks for failure, so `drain()` after `stop()` never raises.
- `is_active` is `!closed && !failed`.

There is no recovery path: `failed` is never reset. A failed player keeps
raising the same message until it is replaced; call `stop()` (or drop it) to
release the device and create a new `AudioPlayer`.

## Signals and re-entrancy

- Ctrl+C during a blocking call raises `KeyboardInterrupt` within about
  100 ms. Samples a `write()` had already pushed stay queued and will play
  unless the caller then calls `clear()` or `stop()`.
- `try_attach` returns nothing while the interpreter is shutting down
  (attaching then would park the thread), and the signal check is skipped.
- A signal handler runs *inside* the waiter's signal check, on the blocked
  thread. Because the push loop releases the writer lock before every wait, the
  handler may call `stop()`, `clear()` or even `write()` on the same player
  without deadlocking.

## Tests

| Test | Guards |
|---|---|
| `TestDrain::test_ctrl_c_interrupts_drain` | `_thread.interrupt_main()` during a 3 s drain raises `KeyboardInterrupt` in under 1.5 s. |
| `TestAudioPlayerClear::test_signal_handler_can_use_a_blocked_player` | A `SIGUSR1` handler calling `stop()` or `write()` during a blocked write finishes instead of deadlocking (subprocess; POSIX only). |
| `TestAudioPlayerClear::test_stop_unblocks_writer_in_another_thread` | A writer blocked on a full ring raises `RuntimeError` matching "closed" after `stop()`. |
| `TestDrain::test_drain_after_stop_returns` | `drain()` after `stop()` with audio buffered returns instead of hanging. |
| `TestAudioPlayerWrite::test_write_after_stop_raises` | `write()` on a stopped player raises "closed". |

Stall detection and the error-callback classification have no automated
tests: exercising them needs a device that disappears or stops calling back.

## Related pages

- [Interruption, Drain and Stop Semantics](interruption-and-drain.md)
- [Audio Callback and Rendering](audio-callback-and-rendering.md)
- [Native Extension Boundary (PyO3)](../architecture/native-extension-boundary.md)
- [Logging and Troubleshooting](../operations/logging-and-troubleshooting.md)
