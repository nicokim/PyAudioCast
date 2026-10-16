# Files

- [Device Discovery and Selection](device-discovery-and-selection.md) - How pyaudiocast picks the cpal host, lists output devices and PipeWire/PulseAudio sinks with their ids, names and types, and resolves a device= query to an output target through an ordered set of exact and substring matching rules.
- [PipeWire/PulseAudio Sink Routing](pipewire-sink-routing.md) - How pyaudiocast plays to a specific PipeWire/PulseAudio sink by opening the pulse or pipewire ALSA device with PULSE_SINK or PIPEWIRE_NODE temporarily set by SinkGuard, and the global open lock, drop order and GIL discipline that make this process-wide environment change safe enough.
