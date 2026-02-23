"""List all available audio output devices."""

import pyaudiocast as pyspeaker

devices = pyspeaker.list_output_devices()

if not devices:
    print("No output devices found.")
else:
    print(f"Found {len(devices)} output device(s):\n")
    for dev in devices:
        print(f"  [{dev['index']}] {dev['name']}")
