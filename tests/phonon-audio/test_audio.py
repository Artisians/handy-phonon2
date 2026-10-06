"""Run with the locked bundled Python: tests the actual CI fixture converter."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
import wave

import numpy as np
import soundfile as sf

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("prepare_test_audio", ROOT / "scripts/phonon/prepare_test_audio.py")
AUDIO = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AUDIO)


class FixtureConversionTests(unittest.TestCase):
    def test_stereo_44100_pcm24_becomes_mono_16000_pcm16(self):
        with tempfile.TemporaryDirectory(prefix="phonon audio ") as directory:
            source = Path(directory) / "stereo source.flac"
            target = Path(directory) / "mono destination.wav"
            samples = 44_100
            left = np.full(samples, 0.5)
            right = np.zeros(samples)
            sf.write(source, np.column_stack((left, right)), 44_100, subtype="PCM_24")
            AUDIO.convert(source, target)
            with wave.open(str(target), "rb") as wav:
                self.assertEqual((wav.getframerate(), wav.getnchannels(), wav.getsampwidth(), wav.getnframes()), (16_000, 1, 2, 16_000))
            audio, _ = sf.read(target)
            self.assertAlmostEqual(float(np.mean(audio[100:-100])), 0.25, places=3)

    def test_valid_mono_audio_retains_duration(self):
        with tempfile.TemporaryDirectory(prefix="phonon audio ") as directory:
            source = Path(directory) / "source.wav"
            target = Path(directory) / "target.wav"
            sf.write(source, np.zeros(8_000), 16_000, subtype="PCM_16")
            AUDIO.convert(source, target)
            with wave.open(str(target), "rb") as wav:
                self.assertEqual(wav.getnframes(), 8_000)


if __name__ == "__main__":
    unittest.main()
