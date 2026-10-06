# Phonon integration verification

Based on cjpais/Handy `a94b403e0610049fafa54b0a4077db2945084dd8`
(package version 0.9.8). This is the independent
[Artisians/handy-phonon2 fork](https://github.com/Artisians/handy-phonon2).
The source changes described below still require their exact-commit Windows CI
result before an installer is called verified.

## Current application workflow

Phonon-2 is a normal model in onboarding and Settings → Models. Download shows
byte progress, verifies the immutable official archive, prepares it automatically
and marks it installed only after validation. Cancel/retry also works during setup.
It can then be selected, switched away from, unloaded or deleted like other models.
A failed or deleted model remains visible for Download.

The installer contains the app-private interpreter and pinned dependencies. Handy
starts the hidden backend on a private loopback port, checks a fresh HMAC challenge
before sending the session secret or audio, and manages only its owned process.
Startup has cancellation, a readiness deadline and actionable retry errors. Normal
unload, switching and exit release the owned backend. No user-managed terminal,
Python installation or external service is required.

## Passed in the Linux cloud development environment

- Exact production adapter and process manager compiled through
  `tests/phonon-adapter`: 18 tests pass. One additional ignored test is only the
  child-process fixture invoked by the lifecycle tests.
- Adapter coverage includes PCM16 mono/16 kHz conversion, clipping, empty/nonfinite/
  oversized recording rejection, transcript validation, Content-Length multipart
  uploads, exact model identity, redirects, HTTP/malformed/oversized responses,
  bounded timeout and repeated requests.
- Authentication tests verify HMAC proof before bearer/audio transmission, reject
  a reflecting impostor, and reject an unauthenticated external Phonon server.
- Lifecycle coverage includes concurrent startup sharing one owned process, cancel
  before/during startup, late cancel after readiness, early process exit, timeout,
  wrong identity, bind collisions, missing resources, unload/restart, shutdown and
  leaving unrelated listeners untouched.
- Nine separate native model-install tests pass, including verified hashes,
  partial/corrupt rejection, transactional installation, staging cleanup, repeated
  requests, child cancellation/reaping and failure/retry.
- Frontend TypeScript, changed-file ESLint and a production Vite build pass.
- Server-rendered UI checks cover normal Download/select/delete cards, progress,
  cancel during verification/setup/startup, and actual Models/onboarding visibility
  for fresh and deleted/stale-selected Phonon entries.
- Adapter/runtime `cargo clippy --all-targets -- -D warnings` and
  `git diff --check` pass.

These isolated Rust tests import the production sources; their HTTP and process
fixtures are deliberately small mocks. They are not a substitute for Windows ASR
or microphone/paste validation.

## Pending or environment-blocked checks

- Full desktop Rust compilation is blocked in this Linux environment by missing
  GTK/GLib development dependencies. The Windows workflow compiles the full app.
- The interactive local browser harness cannot launch Chromium here because the
  execution environment disallows its process-singleton socket. It is wired into
  Windows CI and is not claimed as passed until that run succeeds.
- Repository-wide ESLint reports two existing untranslated strings in
  `src/components/shared/ProgressBar.tsx`; changed integration files pass lint.
- Installed Windows end-to-end ASR remains gated on the exact source commit's CI
  result. The workflow checks the installed runtime inventory, dependency versions,
  isolation, automatic model preparation, two actual speech transcriptions through
  Handy's production managed-runtime code, paths containing spaces and cleanup.
- Physical microphone capture, global shortcuts, focus/clipboard/paste, existing
  engines on the destination machine and actual ChatGPT sign-in still require
  device-level checks. The user's computer is not used by this cloud validation.

Dictation cancellation retains Handy's existing generation/output suppression.
An already-running inference can drain until its bounded request completes before
another recording starts; startup and model setup cancellation stop owned children.
ChatGPT text cleanup remains a separately enabled experimental feature.

## Reproduce focused developer checks

```sh
cargo test --locked --manifest-path tests/phonon-adapter/Cargo.toml
cargo test --locked --manifest-path tests/phonon-model-install/Cargo.toml
bun tests/phonon-ui.test.tsx
node scripts/test-phonon-browser.mjs
bun run build
```

The browser harness uses mocked IPC and never opens a sign-in page or external
service. The installer workflow's `scripts/ci/test-phonon-package.ps1` uses pinned
public JFK speech test audio and the production model setup/runtime. It requires
no microphone, user credentials or separately installed runtime. Test recordings,
transcripts and session credentials are not included in the release payload.
