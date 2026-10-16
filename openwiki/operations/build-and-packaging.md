---
type: operations
title: Build and Packaging
description: How pyaudiocast is built into an abi3 wheel with maturin — Cargo and pyproject settings, module naming, the single-sourced version, Rust edition and MSRV, crate dependencies, system libraries needed to build, test and run, the release profile, and local development commands.
tags: [build, packaging, maturin, cargo, abi3, msrv, wheels, alsa]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-16T04:21:31.514Z
sources:
  - id: openwiki-source-164e2da859b5277df81c7d94
    resource: repo://.github/workflows/ci.yml
  - id: openwiki-source-4d1d392666be6dfdd7a91a2e
    resource: repo://.github/workflows/release.yml
  - id: openwiki-source-16e6e7418602542c69565bde
    resource: repo://Cargo.lock
  - id: openwiki-source-651d1fb6c9e49916a916ab51
    resource: repo://Cargo.toml
  - id: openwiki-source-ca6cb4b1a14fd7969dfae3ec
    resource: repo://CHANGELOG.md
  - id: openwiki-source-d6c68e12c3efb74a1cd6ef7b
    resource: repo://pyaudiocast/__init__.py
  - id: openwiki-source-57549c36e97e92b8c26de49a
    resource: repo://pyaudiocast/_pyaudiocast.pyi
  - id: openwiki-source-56a7321e140af5256a1bdfae
    resource: repo://pyaudiocast/py.typed
  - id: openwiki-source-05ccef8d4cf1698187f20464
    resource: repo://pyproject.toml
  - id: openwiki-source-23775c3de52f3ab95a13cb8b
    resource: repo://README.md
  - id: openwiki-source-ed8bf05e307c6278442542c2
    resource: repo://src/lib.rs
generated: { by: "claude-code", at: "2026-10-16T04:21:31.514Z" }
---

# Build and Packaging

PyAudioCast is a mixed Rust/Python project built by
[maturin](https://www.maturin.rs/). Cargo compiles the Rust crate into a
native library; maturin drops it into the `pyaudiocast/` package next to the
pure-Python files and produces a wheel.

## At a glance

| Item | Value | Where |
|---|---|---|
| Build backend | `maturin>=1.9.4,<2.0` | `pyproject.toml` `[build-system]` |
| Native library | `cdylib` named `_pyaudiocast` | `Cargo.toml` `[lib]` |
| Import path | `pyaudiocast._pyaudiocast` | `pyproject.toml` `[tool.maturin] module-name` |
| Python ABI | stable ABI, CPython ≥ 3.9 (`pyo3` feature `abi3-py39`) | `Cargo.toml`, `requires-python = ">=3.9"` |
| Version | `Cargo.toml` `version`, read dynamically | `pyproject.toml` `dynamic = ["version"]` |
| Rust edition / MSRV | 2024 / 1.88 | `Cargo.toml` |
| License | MIT | both manifests, `LICENSE` |

## Package layout produced

```
pyaudiocast/
├── __init__.py          # re-exports AudioPlayer, list_output_devices, play_file, __version__
├── _pyaudiocast.pyi     # type stubs for the native module
├── py.typed             # PEP 561 marker (empty)
└── _pyaudiocast.*.so    # the compiled extension (.pyd on Windows), added by maturin
```

Because the module name is `pyaudiocast._pyaudiocast`, maturin treats the
`pyaudiocast/` directory as the Python source of the package and installs the
compiled library inside it. The `[lib] name` must stay `_pyaudiocast` to match
the `#[pymodule] fn _pyaudiocast` in `src/lib.rs`. Compiled libraries (`*.so`,
`*.dll`, `*.dylib`), `target/`, `dist/`, `build/` and virtualenvs are
git-ignored.

## Version: one source of truth

The version lives only in `Cargo.toml` (`0.2.0` at this commit). `pyproject.toml`
declares the version dynamic, so maturin copies it into the wheel metadata, and
the extension exposes it as `pyaudiocast.__version__` via
`env!("CARGO_PKG_VERSION")`. release-please bumps `Cargo.toml` (release type
`rust`) — see [Release Process](release-process.md). Never edit a version in
`pyproject.toml`.

## Rust dependencies

| Crate | Version | Role |
|---|---|---|
| `pyo3` | 0.30, feature `abi3-py39` | Python bindings; one stable-ABI wheel per platform |
| `numpy` | 0.30 | rust-numpy, reading numpy arrays in `write()` |
| `cpal` | 0.18, no extra features | Cross-platform audio output (ALSA / WASAPI / CoreAudio hosts only) |
| `ringbuf` | 0.5 | Heap-allocated SPSC ring between `write()` and the audio callback |
| `hound` | 3.5 | WAV decoding for `play_file` |
| `log` | 0.4 | Logging facade |
| `env_logger` | 0.11, default features off; `auto-color`, `humantime` | `PYAUDIOCAST_LOG` support (no regex filter feature) |

`Cargo.lock` is committed and CI runs every cargo command with `--locked`, so
checks and tests use exactly the pinned versions.

On the Python side there are **no required runtime dependencies**. Extras:
`pyaudiocast[numpy]` adds numpy; `pyaudiocast[dev]` adds pytest and numpy.

## Edition and MSRV

`Cargo.toml` declares `edition = "2024"` and `rust-version = "1.88"`. The source
relies on recent language and library features — let-chains
(`if let … && …` in `find_device` and `extract_samples`), `slice::as_chunks`,
`is_multiple_of` — and edition 2024 is why `std::env::set_var` must be wrapped
in `unsafe` in `SinkGuard`. CI's `msrv` job runs `cargo check --locked` on the
1.88 toolchain, so raising the MSRV means updating both `rust-version` and
that job. Since 0.2.0, building from source requires Rust 1.88+.

## Release profile

`[profile.release]` enables `lto = true`, `codegen-units = 1` and
`strip = true`: smaller, faster wheels at the cost of slower release builds.
`maturin develop` builds the debug profile unless `--release` is passed.

## System dependencies

| Activity | Linux needs | Why |
|---|---|---|
| Building the crate | ALSA headers: `libasound2-dev` (Debian/Ubuntu) or `alsa-lib-devel` (Fedora) | cpal's ALSA backend links libasound |
| `cargo test` | the above plus the Python development package (`python3-dev`) | the Rust unit tests link against libpython |
| Running a wheel | `libasound.so.2` (`libasound2t64` / `libasound2`, `alsa-lib`) | Linux wheels deliberately do not bundle libasound |
| Listing and selecting sinks | `pactl` on `PATH` (optional) | sinks are discovered by running `pactl list short sinks` |
| Playing to sinks | the server's ALSA plugin providing a `pulse` or `pipewire` PCM | sinks are routed through it ([Sink Routing](../devices/pipewire-sink-routing.md)) |

The release workflow repairs Linux wheels with
`auditwheel repair --exclude libasound.so.2`, so the wheel uses the system's
ALSA library — and with it the system's PipeWire/PulseAudio ALSA plugins —
instead of a vendored copy. macOS and Windows need nothing beyond the OS audio
stack.

## Local development

```bash
# With uv (as in the README)
uv venv && source .venv/bin/activate
uv pip install maturin numpy pytest
maturin develop            # builds and installs into the venv; re-run after Rust changes

# Or a plain install from source (release build via maturin)
python -m venv .venv && source .venv/bin/activate
pip install .

# Tests (from the repository root)
pytest tests/ -v           # audio tests skip without an output device
cargo test                 # needs the Python dev package on Linux
cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings   # what CI enforces
```

What the tests cover and how to give them an audio device is described in
[Test Suite Map](../testing/test-suite.md) and
[Audio Test Environments](../testing/audio-test-environments.md).

## Published artifacts

Releases publish one abi3 wheel per platform — Linux x86_64 (manylinux),
macOS x86_64 and arm64, Windows x86_64 — plus an sdist. Linux aarch64 wheels
are not built (ALSA cross-compilation was unsupported). Build matrix and
publishing are covered in [Release Process](release-process.md).

## Related pages

- [Native Extension Boundary (PyO3)](../architecture/native-extension-boundary.md)
- [Continuous Integration](continuous-integration.md)
- [Release Process](release-process.md)
- [PyAudioCast Quickstart](../quickstart.md)
