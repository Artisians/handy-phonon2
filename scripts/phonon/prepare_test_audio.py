"""Convert the pinned public CI speech fixture to Handy's microphone WAV format.

Build/test helper only. Never shipped to users or used to process user recordings.
"""
from __future__ import annotations

import argparse
from math import gcd
from pathlib import Path
import wave


def convert(source: Path, destination: Path) -> None:
    import numpy as np
    from scipy.signal import resample_poly
    import soundfile as sf

    audio, sample_rate = sf.read(source, dtype="float64", always_2d=True)
    if sample_rate <= 0 or not audio.size or not np.isfinite(audio).all():
        raise ValueError("Invalid or empty CI audio fixture")
    mono = audio.mean(axis=1)
    divisor = gcd(sample_rate, 16_000)
    if sample_rate != 16_000:
        mono = resample_poly(mono, 16_000 // divisor, sample_rate // divisor)
    sf.write(destination, mono, 16_000, format="WAV", subtype="PCM_16")
    # Check the actual emitted RIFF header, not just the conversion arguments.
    with wave.open(str(destination), "rb") as wav:
        if (wav.getframerate(), wav.getnchannels(), wav.getsampwidth(), wav.getcomptype()) != (16_000, 1, 2, "NONE"):
            raise ValueError("CI fixture is not 16 kHz mono 16-bit PCM WAV")
        if wav.getnframes() == 0:
            raise ValueError("Converted CI fixture contains no samples")
        print(f"Verified CI audio: 16000 Hz, mono, PCM16, {wav.getnframes()} samples")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    convert(args.source, args.destination)
