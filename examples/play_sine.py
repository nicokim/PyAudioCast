"""Play a 440Hz sine wave for 2 seconds using AudioPlayer."""

import math

import pyaudiocast as pyspeaker

SAMPLE_RATE = 44100
DURATION = 2.0
FREQUENCY = 440.0

# Generate sine wave as f32 samples
num_samples = int(SAMPLE_RATE * DURATION)
samples = [
    0.3 * math.sin(2.0 * math.pi * FREQUENCY * i / SAMPLE_RATE)
    for i in range(num_samples)
]

print(f"Playing {FREQUENCY}Hz sine wave for {DURATION}s...")

with pyspeaker.AudioPlayer(sample_rate=SAMPLE_RATE, channels=1) as player:
    player.write(samples)
    player.drain()

print("Done.")
