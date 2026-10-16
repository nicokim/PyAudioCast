"""Demonstrate streaming audio with interruption (barge-in) from another thread."""

import math
import threading

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

    interrupted = threading.Event()

    # Simulate the user interrupting after 2 seconds
    def interrupt():
        print("\n  [!] Interrupting playback with clear()...")
        interrupted.set()  # stop producing...
        player.clear()  # ...and silence what's already buffered

    timer = threading.Timer(2.0, interrupt)
    timer.start()

    for i in range(TOTAL_CHUNKS):
        if interrupted.is_set():
            print("  Producer stopped.")
            break
        player.write(generate_chunk(FREQUENCY))  # blocks while the buffer is full
        print(f"  Wrote chunk {i + 1}/{TOTAL_CHUNKS}")

    player.drain()
    timer.join()

print("Done.")
