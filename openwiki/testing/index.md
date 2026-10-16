# Files

- [Audio Test Environments](audio-test-environments.md) - How pyaudiocast's audio-dependent tests decide to run or skip, how CI gives them a real output device (headless PulseAudio with a null sink on Linux, the Null Audio Device on macOS), why timing assertions warm the device up first, and how subprocess and thread-based tests isolate numpy, signal and concurrency scenarios.
- [Test Suite Map](test-suite.md) - How pyaudiocast is tested — Rust unit tests for the converter, writer, renderer, negotiation, device lookup and WAV handling, and Python integration tests that drive a live stream — mapped to the behaviors and past regressions each one guards, plus how to run and extend them.
