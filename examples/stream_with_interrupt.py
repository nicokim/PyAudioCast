"""Demonstrate streaming audio with interruption (clear)."""

import math
import threading
import time

import pyaudiocast

SAMPLE_RATE = 44100
FREQUENCY = 440.0

# Generate 5 seconds of sine wave in chunks
CHUNK_DURATION = 0.5
CHUNK_SAMPLES = int(SAMPLE_RATE * CHUNK_DURATION)
TOTAL_CHUNKS = 10


def generate_chunk(freq):
    return [
        0.3 * math.sin(2.0 * math.pi * freq * i / SAMPLE_RATE)
        for i in range(CHUNK_SAMPLES)
    ]


with pyaudiocast.AudioPlayer(sample_rate=SAMPLE_RATE, channels=1) as player:
    print(f"Streaming {TOTAL_CHUNKS * CHUNK_DURATION}s of audio...")
    print(f"  Source: {player.sample_rate}Hz, {player.channels}ch")
    print(f"  Device: {player.device_sample_rate}Hz, {player.device_channels}ch")

    # Simulate interruption after 2 seconds
    def interrupt_later():
        time.sleep(2.0)
        print("\n  [!] Interrupting playback with clear()...")
        player.clear()

    t = threading.Thread(target=interrupt_later)
    t.start()

    for i in range(TOTAL_CHUNKS):
        chunk = generate_chunk(FREQUENCY)
        try:
            player.write(chunk)
            print(f"  Wrote chunk {i + 1}/{TOTAL_CHUNKS}")
        except RuntimeError:
            print("  Player stopped.")
            break

    player.drain()
    t.join()

print("Done.")
