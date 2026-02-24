"""Play a 440Hz sine wave for 2 seconds using AudioPlayer."""

import math

import pyaudiocast

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

with pyaudiocast.AudioPlayer(sample_rate=SAMPLE_RATE, channels=1) as player:
    print(f"  Source: {player.sample_rate}Hz, {player.channels}ch")
    print(f"  Device: {player.device_sample_rate}Hz, {player.device_channels}ch")
    player.write(samples)
    player.drain()

print("Done.")
