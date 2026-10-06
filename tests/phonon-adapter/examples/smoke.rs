//! Manual real-server smoke test. Supply a 16 kHz mono PCM16 WAV you may transcribe.
use handy_phonon_adapter_tests::phonon::PhononEngine;
fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("Usage: smoke <mono-16khz-pcm16.wav>"))?;
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    anyhow::ensure!(
        spec.channels == 1
            && spec.sample_rate == 16000
            && spec.bits_per_sample == 16
            && spec.sample_format == hound::SampleFormat::Int,
        "Use 16 kHz mono PCM16 WAV"
    );
    let audio = reader
        .samples::<i16>()
        .map(|sample| sample.map(|value| value as f32 / 32768.0))
        .collect::<Result<Vec<_>, _>>()?;
    let engine = PhononEngine::connect()?;
    println!("{}", engine.transcribe(&audio)?);
    Ok(())
}
