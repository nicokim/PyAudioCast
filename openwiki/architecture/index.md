# Files

- [Native Extension Boundary (PyO3)](native-extension-boundary.md) - How the Rust crate becomes the pyaudiocast._pyaudiocast extension module — module registration, the frozen AudioPlayer pyclass and its thread-safe state, which calls release the GIL and when, signal-handler re-entry, and how Rust errors become Python exceptions.
- [Architecture Overview](overview.md) - How PyAudioCast is layered from the pyaudiocast Python package through the Rust/PyO3 extension to cpal and the OS audio hosts, which thread runs which code, who owns which state, and how audio data and control signals (clear, stop, drain, failures) flow end to end.
