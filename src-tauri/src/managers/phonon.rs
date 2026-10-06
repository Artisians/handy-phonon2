//! Phonon-2's official local HTTP API. No model weights or Python interpreter
//! are bundled. A separately started Fermion server keeps the model resident.
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::Value;
use std::io::{Cursor, Read};
use std::time::Duration;

const ENDPOINT: &str = "http://127.0.0.1:8010";
const MAX_SAMPLES: usize = 16_000 * 120;
const MAX_RESPONSE_BYTES: u64 = 512_000;
const SETUP: &str =
    "Start the official local server first: fermion serve phonon-2 --host 127.0.0.1 --port 8010";

pub struct PhononEngine;

fn client(timeout: Duration) -> Result<Client> {
    Ok(Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(timeout)
        .build()?)
}

fn read_json(response: reqwest::blocking::Response) -> Result<Value> {
    // Do not echo response bodies: an error may contain private transcript data.
    if !response.status().is_success() {
        bail!(
            "Phonon-2 local server returned HTTP {}. Check its console and setup instructions.",
            response.status()
        );
    }
    let mut body = Vec::new();
    response
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut body)?;
    if body.len() as u64 > MAX_RESPONSE_BYTES {
        bail!("Phonon-2 response exceeded the safety limit");
    }
    serde_json::from_slice(&body).context("Phonon-2 returned malformed JSON")
}

fn validate_health(health: &Value) -> Result<()> {
    if health.get("status").and_then(Value::as_str) != Some("ok")
        || health.get("kind").and_then(Value::as_str) != Some("speech")
        || health.get("model").and_then(Value::as_str) != Some("FermionResearch/Phonon-2")
    {
        bail!("Port 8010 is not serving the expected Phonon-2 speech model. {SETUP}");
    }
    Ok(())
}

fn encode_wav(audio: &[f32]) -> Result<Vec<u8>> {
    if audio.is_empty() {
        bail!("No audio to transcribe");
    }
    if audio.len() > MAX_SAMPLES {
        bail!("Phonon-2 prototype supports recordings up to 120 seconds; record a shorter passage");
    }
    if audio.iter().any(|sample| !sample.is_finite()) {
        bail!("Recording contains invalid audio samples");
    }
    let mut cursor = Cursor::new(Vec::with_capacity(audio.len() * 2 + 44));
    {
        let mut writer = hound::WavWriter::new(
            &mut cursor,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        for sample in audio {
            writer.write_sample((sample.clamp(-1.0, 1.0) * 32767.0).round() as i16)?;
        }
        writer.finalize()?;
    }
    Ok(cursor.into_inner())
}

fn transcript(value: Value) -> Result<String> {
    let text = value
        .get("text")
        .and_then(Value::as_str)
        .context("Phonon-2 response is missing text")?
        .trim();
    if text.len() > 100_000 || text.contains('\0') {
        bail!("Phonon-2 returned invalid transcript text");
    }
    Ok(text.to_owned())
}

fn request_transcription(endpoint: &str, wav: Vec<u8>, timeout: Duration) -> Result<String> {
    let client = client(timeout)?;
    // Recheck identity in case a different service replaced the backend.
    validate_health(&read_json(
        client
            .get(format!("{endpoint}/health"))
            .timeout(Duration::from_secs(5))
            .send()
            .with_context(|| format!("Phonon-2 disconnected. {SETUP}"))?,
    )?)?;
    let form = reqwest::blocking::multipart::Form::new()
        .text("model", "phonon-2")
        .text("response_format", "json")
        .part(
            "file",
            reqwest::blocking::multipart::Part::bytes(wav)
                .file_name("dictation.wav")
                .mime_str("audio/wav")?,
        );
    let response = client
        .post(format!("{endpoint}/v1/audio/transcriptions"))
        .multipart(form)
        .send()
        .context("Phonon-2 transcription failed or timed out; check the local server")?;
    transcript(read_json(response)?)
}

impl PhononEngine {
    pub fn connect() -> Result<Self> {
        // reqwest's blocking client creates a runtime. Isolate it from Tauri's
        // async context, including client destruction, to avoid nested-runtime panics.
        std::thread::spawn(|| {
            let response = client(Duration::from_secs(5))?
                .get(format!("{ENDPOINT}/health"))
                .send()
                .with_context(|| format!("Cannot reach Phonon-2. {SETUP}"))?;
            validate_health(&read_json(response)?)
        })
        .join()
        .map_err(|_| anyhow::anyhow!("Phonon-2 connection worker stopped"))??;
        Ok(Self)
    }

    pub fn transcribe(&self, audio: &[f32]) -> Result<String> {
        let wav = encode_wav(audio)?;
        std::thread::spawn(move || request_transcription(ENDPOINT, wav, Duration::from_secs(180)))
            .join()
            .map_err(|_| anyhow::anyhow!("Phonon-2 transcription worker stopped"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wav_is_mono_16k_pcm_and_clamps() {
        let bytes = encode_wav(&[-2.0, 0.0, 2.0]).unwrap();
        let mut reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().sample_rate, 16000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(
            reader
                .samples::<i16>()
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
            vec![-32767, 0, 32767]
        );
    }
    #[test]
    fn rejects_empty_oversized_and_nonfinite_audio() {
        assert!(encode_wav(&[]).is_err());
        assert!(encode_wav(&[f32::NAN]).is_err());
        assert!(encode_wav(&[f32::INFINITY]).is_err());
        assert!(encode_wav(&vec![0.0; MAX_SAMPLES + 1]).is_err());
        assert!(encode_wav(&vec![0.0; MAX_SAMPLES]).is_ok());
    }
    #[test]
    fn checks_exact_backend_identity() {
        assert!(validate_health(
            &json!({"status":"ok", "kind":"speech", "model":"FermionResearch/Phonon-2"})
        )
        .is_ok());
        for value in [
            json!({}),
            json!({"status":"ok","kind":"speech","model":"phonon-1"}),
            json!({"status":"loading","kind":"speech","model":"FermionResearch/Phonon-2"}),
        ] {
            assert!(validate_health(&value).is_err());
        }
    }
    #[test]
    fn parses_and_validates_transcripts() {
        assert_eq!(transcript(json!({"text":" hello "})).unwrap(), "hello");
        assert_eq!(transcript(json!({"text":" "})).unwrap(), "");
        for value in [
            json!({}),
            json!({"text":4}),
            json!({"text":"bad\u{0000}text"}),
            json!({"text":"a".repeat(100001)}),
        ] {
            assert!(transcript(value).is_err());
        }
    }
    fn mock_server(
        responses: Vec<(&'static str, String)>,
    ) -> (String, std::thread::JoinHandle<Vec<Vec<u8>>>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let header_end = loop {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                    if bytes.ends_with(b"\r\n\r\n") {
                        break bytes.len();
                    }
                };
                let headers = String::from_utf8_lossy(&bytes).to_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .map(|n| n.parse::<usize>().unwrap())
                    .unwrap_or(0);
                bytes.resize(header_end + length, 0);
                socket.read_exact(&mut bytes[header_end..]).unwrap();
                requests.push(bytes);
                write!(socket, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        (endpoint, worker)
    }
    fn health_json() -> String {
        r#"{"status":"ok","kind":"speech","model":"FermionResearch/Phonon-2"}"#.into()
    }
    #[test]
    fn mock_http_upload_has_length_model_and_in_memory_wav() {
        let (url, server) = mock_server(vec![
            ("200 OK", health_json()),
            ("200 OK", r#"{"text":"Hello world."}"#.into()),
        ]);
        let result = request_transcription(
            &url,
            encode_wav(&[0.1; 3200]).unwrap(),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(result, "Hello world.");
        let requests = server.join().unwrap();
        let request = String::from_utf8_lossy(&requests[1]);
        assert!(request.starts_with("POST /v1/audio/transcriptions "));
        assert!(request.to_lowercase().contains("content-length:"));
        assert!(!request.to_lowercase().contains("transfer-encoding:"));
        assert!(request.contains("phonon-2"));
        assert!(request.contains("audio/wav"));
        assert!(request.contains("RIFF"));
    }
    #[test]
    fn mock_http_refuses_wrong_model_before_audio_upload() {
        let (url, server) = mock_server(vec![(
            "200 OK",
            r#"{"status":"ok","kind":"speech","model":"wrong"}"#.into(),
        )]);
        assert!(request_transcription(&url, vec![1], Duration::from_secs(5)).is_err());
        assert_eq!(server.join().unwrap().len(), 1);
    }
    #[test]
    fn mock_http_rejects_status_malformed_and_oversized_response() {
        for (status, body) in [
            ("503 Service Unavailable", "private words".into()),
            ("200 OK", "not json".into()),
            ("200 OK", " ".repeat(MAX_RESPONSE_BYTES as usize + 1)),
        ] {
            let (url, server) = mock_server(vec![("200 OK", health_json()), (status, body)]);
            let error = request_transcription(&url, vec![1], Duration::from_secs(5))
                .unwrap_err()
                .to_string();
            assert!(!error.contains("private words"));
            server.join().unwrap();
        }
    }
    #[test]
    fn mock_http_never_follows_redirect() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).unwrap() > 0);
            socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://192.0.2.1/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let response = client(Duration::from_secs(2))
            .unwrap()
            .get(url)
            .send()
            .unwrap();
        assert_eq!(response.status().as_u16(), 302);
        assert!(read_json(response).is_err());
        server.join().unwrap();
    }
    #[test]
    fn mock_http_timeout_is_bounded() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (_socket, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(250));
        });
        let started = std::time::Instant::now();
        assert!(client(Duration::from_millis(50))
            .unwrap()
            .get(url)
            .send()
            .is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        server.join().unwrap();
    }
    #[test]
    fn repeated_requests_reuse_external_model_service() {
        let (url, server) = mock_server(vec![
            ("200 OK", health_json()),
            ("200 OK", r#"{"text":"First"}"#.into()),
            ("200 OK", health_json()),
            ("200 OK", r#"{"text":"Second"}"#.into()),
        ]);
        assert_eq!(
            request_transcription(&url, vec![1], Duration::from_secs(5)).unwrap(),
            "First"
        );
        assert_eq!(
            request_transcription(&url, vec![2], Duration::from_secs(5)).unwrap(),
            "Second"
        );
        assert_eq!(server.join().unwrap().len(), 4);
    }
}
