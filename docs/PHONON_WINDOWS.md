# Phonon-2 in Handy: Windows local-service adapter

This experimental fork adds **Phonon-2 (local server)** as an optional recognition
engine. Handy still owns microphone capture, push-to-talk/hotkeys, silence
handling, transcription history, and paste. It sends a recorded WAV to the
separately started official Fermion server. Existing Handy engines remain
available. This is not a separate Python microphone application.

## Scope and prerequisites

- Windows **x64**, with an SSE4.1-capable CPU; Windows ARM speech is unsupported
  upstream. Use Python **3.12 x64** for the instructions below.
- CPU inference, English transcription only. No translation, streaming display,
  Apple Neural Engine path, or latency guarantee is claimed by this adapter.
- Recordings are limited to 120 seconds. Cancel suppresses output/paste, but an
  in-flight inference request may drain until its response or approximately 185-second combined request timeout
  before a new recording can start.
- The adapter is included in this fork. Python, Fermion, Torch and model weights
  are **not** bundled or automatically installed. No setup process is launched by
  clicking the model card. Python/Torch can require substantially more disk space
  than the approximately 164 MB model weights.
- The Windows installer, physical microphone/hotkeys, real model inference, and
  paste behavior have **not been validated end-to-end on Windows** for this fork.
  Follow the acceptance checks below before relying on it.

## 1. Install the external service

Install Python 3.12 x64 from [python.org](https://www.python.org/downloads/windows/)
if it is not already available. In PowerShell, choose a permanent folder for the
service and run:

```powershell
py -3.12 -m venv .venv-phonon
.\.venv-phonon\Scripts\python.exe -m pip install fermion-research==0.2.7 torch safetensors soundfile scipy zstandard
.\.venv-phonon\Scripts\python.exe -m pip check
```

Using the environment's executable directly avoids changing PowerShell execution
policy. `fermion-research==0.2.7` is the verified PyPI release used here. Its
[official installation instructions](https://pypi.org/project/fermion-research/0.2.7/)
use plain `torch` on Windows; the CPU wheel index instruction is Linux-specific.
Other dependency versions resolve at installation time. This is **not a fully
locked or Windows-tested dependency set**. Do not interpret the Fermion pin as a
reproducibility guarantee for every dependency. After successful Windows testing,
record the resolved versions for your machine with:

```powershell
.\.venv-phonon\Scripts\python.exe -m pip freeze > phonon-windows-resolved.txt
```

## 2. Start the local server

From that same folder:

```powershell
.\.venv-phonon\Scripts\fermion.exe serve phonon-2 --host 127.0.0.1 --port 8010
```

Leave this terminal open. First startup downloads the model from Hugging Face and
may take longer. Wait for loading to complete. Do not change the host to `0.0.0.0`
or a LAN address: this adapter expects the fixed loopback endpoint and the
upstream service has no authentication by default. Do not add an inbound firewall
exception to expose it to other computers. Ctrl+C stops the server.

In another PowerShell window, check:

```powershell
Invoke-RestMethod http://127.0.0.1:8010/health
```

The expected response identifies `status: ok`, model
`FermionResearch/Phonon-2`, and `kind: speech`. Another service answering on that
port does not establish Phonon readiness.

## 3. Select it in Handy

Open **Models → Phonon-2 (local server)** and click **Check and select**. The same
external-service card is available during model onboarding. The card explains
setup and displays errors if the check fails. “Selected” means Handy is configured
to use the adapter; it is not a live availability monitor. Keep the service running.
There are no Download/Delete buttons for this entry because Handy does not manage
its installation or external cache.

Use your usual Handy recording shortcut and speak a short English sentence. The
adapter posts a mono WAV to `http://127.0.0.1:8010/v1/audio/transcriptions`, with
`model=phonon-2`, and requests the transcript through the existing Handy pipeline.
Model switching, recording controls and paste settings otherwise stay in Handy.

## Privacy and ownership

Package installation and the first model download require internet access. The
adapter's audio request stays on loopback, and the model runs in the local server.
This is not a guarantee that every feature of Handy is offline: existing history
storage (including local audio and transcripts by default), update checks, and
any separately enabled cloud post-processing keep
their normal settings. Turn off cloud post-processing if you want local-only
recognition and output. The service's own dependencies and cache are managed
outside Handy. Closing Handy does not terminate that external process; unloading
the Handy adapter does not unload the server's model.

## Windows acceptance checks

1. Start the official server; inspect `/health` for the expected model and kind.
2. Select the Phonon card. It must never show a fake download, size, speed score,
   or “installed” confirmation for the service.
3. Dictate a short sentence into Notepad using Handy's configured hotkey. Verify
   the transcript, paste, and history entry. Repeat with push-to-talk if enabled.
4. Try silence, a canceled recording, and a second recording. Verify no previous
   transcript is accidentally pasted and the recording state returns to idle.
5. Stop the server and try selection/transcription. Verify an actionable error,
   no fabricated transcript, and that restarting the server allows a retry.
6. Switch to a previously working Handy engine and back. Confirm its behavior has
   not changed. Check the service remains external when Handy exits/restarts.
7. Measure cold-start and repeated recording latency on the actual target CPU.
   Upstream benchmark figures are not a guarantee for this integration.

## Troubleshooting

- **Connection refused / timeout:** Start the terminal command above and wait for
  loading. Ensure it uses port 8010 on 127.0.0.1. Check the terminal for download or
  dependency failures. The adapter does not start the process for you.
- **Wrong service / wrong model:** Inspect `/health`; free port 8010 by stopping
  only the conflicting process you recognize, then restart the intended server.
- **Python launcher cannot find 3.12:** Install Python 3.12 x64, reopen PowerShell,
  and confirm `py -3.12 --version`. Do not use an ARM Python environment.
- **Package or Torch installation fails:** Confirm x64 Python, sufficient space,
  network access, and `pip check`. Use the upstream installation instructions;
  do not disable TLS checks or install unofficial wheels to bypass an error.
- **Server works, shortcut or paste fails:** Verify microphone permission/input
  device and the ordinary Handy hotkey/paste settings. Test in a normal Notepad
  window. Elevated target apps may have Windows permission boundaries.
- **Slow CPU transcription:** Keep the server resident, try a short clip, and
  measure on your hardware. There is no GPU requirement or promised speed here.
- **Removing Phonon:** Stop its terminal. Its virtual environment and upstream
  model cache are separate from Handy's model directory; consult Fermion/Hugging
  Face documentation before deleting caches shared with other applications.

## Sources and licenses

- [Handy](https://github.com/cjpais/Handy): CJ Pais, MIT. The fork preserves the
  original `LICENSE`. Base commit: `a94b403e0610049fafa54b0a4077db2945084dd8`.
- [Fermion Phonon source](https://github.com/fermionresearch/phonon): Apache-2.0;
  see its [license](https://github.com/fermionresearch/phonon/blob/main/LICENSE)
  and [server API documentation](https://github.com/fermionresearch/phonon/blob/main/docs/server.md).
- [Phonon-2 model](https://huggingface.co/FermionResearch/Phonon-2): CC BY 4.0.
  Preserve the model's [NVIDIA/Fermion attribution NOTICE](https://huggingface.co/FermionResearch/Phonon-2/blob/main/NOTICE)
  and [license terms](https://creativecommons.org/licenses/by/4.0/) when redistributing.
  Model weights are downloaded separately, not redistributed in this source fork.

This adapter is an independent integration, not an official Fermion or Handy
release. Source/package facts were checked on 2026-10-06; upstream `main` may move.
