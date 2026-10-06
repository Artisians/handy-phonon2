# Prototype verification record

Base: cjpais/Handy commit `a94b403e0610049fafa54b0a4077db2945084dd8`
(package version 0.9.8). This is an unpublished local source fork.

## Passed in Linux cloud development environment

- Exact production Rust adapter compiled through `tests/phonon-adapter`.
- 10 unit/local mock-HTTP tests passed: PCM16 mono 16 kHz WAV conversion,
  sample clipping, non-finite/empty/oversized audio rejection, exact backend
  identity, transcript validation, multipart request with Content-Length,
  redirect refusal, wrong-model refusal before upload, HTTP/error/malformed and
  oversized responses, bounded timeout, and repeated requests.
- `cargo clippy --manifest-path tests/phonon-adapter/Cargo.toml --all-targets -- -D warnings`.
- Rust formatting check and `git diff --check`.
- Frontend TypeScript check, changed-file ESLint, production build, and setup-card
  server-rendered smoke checks (reported by frontend implementation worker).

The local HTTP tests use a deliberately small mock server, not Phonon inference.
The harness imports the production Rust adapter rather than a separate copy.

## Remaining validation

- Full desktop Rust build was attempted and stopped at missing Linux system
  dependency `glib-2.0 >= 2.70`; GTK/WebKit development libraries are unavailable.
- Windows packaging, microphone capture, global hotkeys, focus/clipboard/paste,
  cancellation, and regression tests for the existing engines need native checks.
- At snapshot time, real model speech inference is being checked separately and
  is not claimed as passed here. See any accompanying later test report.

The existing Handy generation/cancellation checks remain in place. Cancellation
suppresses output but does not abort external server inference; the pipeline can
stay busy until response/timeout. Setup checks also execute when Handy's model
unload setting is Immediately. No backend-process management is implemented.

## Reproduce focused tests

```sh
cargo test --manifest-path tests/phonon-adapter/Cargo.toml
cargo clippy --manifest-path tests/phonon-adapter/Cargo.toml --all-targets -- -D warnings
```

For a real local-server test, start the server using `PHONON_WINDOWS.md`, then:

```sh
cargo run --manifest-path tests/phonon-adapter/Cargo.toml --example smoke -- path/to/mono-16khz-pcm16.wav
```

That manual smoke command prints the transcription. Supply only audio you intend
to transcribe. No fixture, Python environment, model weights, or credentials are
included in this source snapshot.
