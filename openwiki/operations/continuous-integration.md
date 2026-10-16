---
type: operations
title: Continuous Integration
description: What the GitHub Actions CI workflow checks — Rust formatting, clippy and unit tests, the Rust 1.88 MSRV check, and a Linux/macOS/Windows × Python 3.9/3.14/3.15 pytest matrix with a headless PulseAudio server, a macOS null audio device and Codecov upload — and how to reproduce each job locally.
tags: [ci, github-actions, rust, pytest, msrv, pulseaudio, coverage]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-16T04:21:31.514Z
sources:
  - id: openwiki-source-164e2da859b5277df81c7d94
    resource: repo://.github/workflows/ci.yml
  - id: openwiki-source-4d1d392666be6dfdd7a91a2e
    resource: repo://.github/workflows/release.yml
  - id: openwiki-source-d6c68e12c3efb74a1cd6ef7b
    resource: repo://pyaudiocast/__init__.py
generated: { by: "claude-code", at: "2026-10-16T04:21:31.514Z" }
---

# Continuous Integration

`.github/workflows/ci.yml` runs on every push to `main` and on every pull
request. It has three independent jobs.

| Job | Runner | What it proves |
|---|---|---|
| `rust` — "Rust checks" | `ubuntu-latest` | Code is rustfmt-clean, clippy-clean with warnings as errors, and the Rust unit tests pass. |
| `msrv` — "MSRV (Rust 1.88)" | `ubuntu-latest` | The library still compiles on the declared minimum Rust version. |
| `python` — "Python *X* tests (*OS*)" | 3 OSes × 3 Pythons | The extension builds with maturin and the pytest suite passes, with real audio output where a device can be provided. |

Releases are a separate workflow; see [Release Process](release-process.md).

## `rust` job

1. Installs `libasound2-dev` (ALSA headers for cpal) and `python3-dev` — the
   Rust unit tests link against libpython.
2. Installs the stable toolchain with `clippy` and `rustfmt`, restores the
   cargo cache.
3. `cargo fmt --check`
4. `cargo clippy --locked --all-targets -- -D warnings` — any warning in the
   library *or the tests* fails the build.
5. `cargo test --locked` — the unit tests in `src/` (resampler, renderer,
   writer alignment, negotiation, device lookup, WAV truncation). The device
   tests use whatever ALSA lists on the runner; see
   [Test Suite Map](../testing/test-suite.md).

## `msrv` job

Installs `libasound2-dev`, the 1.88 toolchain, and runs `cargo check --locked`.
It only type-checks the library: tests are neither built nor run on 1.88. Keep
it in sync with `rust-version` in `Cargo.toml`
([Build and Packaging](build-and-packaging.md#edition-and-msrv)).

## `python` job

Matrix: `os` ∈ {`ubuntu-24.04`, `macos-latest`, `windows-latest`} ×
`python-version` ∈ {`3.9`, `3.14`, `3.15`}, with `fail-fast: false` so one
failing cell does not cancel the others.

- Ubuntu is pinned to 24.04 because `ubuntu-latest` (26.04) has no Python 3.9
  build.
- Only the oldest supported and the newest Pythons are tested, because the
  wheel is a single abi3 build shared by all versions.

Steps:

1. **Linux system packages:** `libasound2-dev`, `pulseaudio`,
   `libasound2-plugins` (the latter provides ALSA's `pulse` PCM).
2. **Linux headless sound server:** sets `XDG_RUNTIME_DIR` (defaulting to
   `$RUNNER_TEMP/xdg`, exported for later steps), writes `~/.asoundrc` with
   `pcm.!default pulse` / `ctl.!default pulse`, starts
   `pulseaudio --start --exit-idle-time=-1`, unloads
   `module-suspend-on-idle` and prints `pactl info`. If PulseAudio fails to
   start, the step emits a `::warning::` and continues — the audio tests then
   skip instead of failing.
3. Stable Rust with cache, `actions/setup-python`, a fresh `.venv`, then
   `pip install maturin numpy pytest pytest-cov` (bash on POSIX, `cmd` on
   Windows).
4. **`maturin develop`** builds the extension (debug profile, since
   `--release` is not passed) and installs it into the venv.
5. **macOS null device:** installs `switchaudio-osx` and selects
   `"Null Audio Device"` as the output, because the VM's own sound device
   sometimes plays a stream about 60× too slowly, which breaks the timing
   assertions. A warning is printed if the device is missing.
6. `pytest tests/ -v --tb=short --cov=pyaudiocast --cov-report=xml`.
7. **Codecov:** only the Linux / Python 3.14 cell uploads `coverage.xml`,
   using the `CODECOV_TOKEN` secret.

Windows gets no audio setup step: audio-dependent tests run only if the runner
exposes a default output device, and are skipped otherwise. How tests decide to
skip is explained in [Audio Test Environments](../testing/audio-test-environments.md).

### What the coverage number means

`--cov=pyaudiocast` instruments Python code only, i.e. the five-line
`pyaudiocast/__init__.py` shim. The Rust core — where all behavior lives — is
exercised by the tests but not measured by the coverage report.

### Debug builds catch more

Because the matrix tests a debug build, Rust debug assertions are active. Some
regressions only show up that way: for example, reading a misaligned numpy
array in place was a panic in debug builds (and undefined behavior in release),
which `test_write_misaligned_numpy` now covers.

## What CI does not do

- **No release builds.** Wheels and the sdist are built only by the
  tag-triggered release workflow (`maturin-action`, manylinux, `auditwheel
  repair --exclude libasound.so.2`). Changes to packaging or to
  `release.yml` are not exercised by pull-request CI, so verify them
  separately before a release.
- **No Rust coverage**, and the MSRV job does not run tests.

`.github/workflows/openwiki-update.yml`, if present, is OpenWiki's scheduled
refresh of this wiki and is not part of the project's own checks.

## Reproducing CI locally

```bash
# rust job
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked                       # Linux: needs libasound2-dev and python3-dev

# msrv job
cargo +1.88 check --locked

# python job (Linux): a sound server with a sink, then
maturin develop
pytest tests/ -v --tb=short
```

## Related pages

- [Audio Test Environments](../testing/audio-test-environments.md)
- [Test Suite Map](../testing/test-suite.md)
- [Build and Packaging](build-and-packaging.md)
- [Release Process](release-process.md)
