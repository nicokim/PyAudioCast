"""Play a WAV file using play_file()."""

import sys

import pyaudiocast

if len(sys.argv) < 2:
    print("Usage: python play_wav.py <path-to-wav>")
    sys.exit(1)

path = sys.argv[1]
print(f"Playing {path}...")
pyaudiocast.play_file(path)
print("Done.")
