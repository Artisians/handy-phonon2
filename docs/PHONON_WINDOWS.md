# Phonon-2 in Handy for Windows

Phonon-2 is a normal downloadable model in this fork. Install Handy Phonon (or
extract its portable ZIP), open **Models**, find **Phonon-2**, and click
**Download**. Handy shows download/setup progress. After setup, select it and use
your usual recording shortcut. Existing Handy engines remain available and you
can switch between them.

No separate Python installation, PowerShell command, terminal or manually
started server is needed. The installer includes a private Python interpreter
and all pinned CPU runtime dependencies. It does not change system Python,
PATH, firewall policy or your Python packages.

## Download, cancel, retry and remove

The first requested model download obtains the approximately 164 MB official
Phonon-2 archive over HTTPS, checks its pinned SHA-256 and reconstructs the model
in a temporary folder. Upstream per-file checksums and the final model-container
checksum are checked before installation is marked complete. Interrupted setup
cannot select a half-installed model. Use Handy's normal cancel/retry controls.
Deleting the model removes its managed weights; the internal runtime remains
available for a later download.

Model reconstruction needs temporary disk space in addition to the final
approximately 178 MB model. The engine may generate a roughly 304 MB local CPU
plane cache to speed up later starts. The installer is larger because it includes
Python, PyTorch and the other runtime dependencies; these are shared by the
Phonon model and are not downloaded separately on the user's machine.

## How recognition works

Handy owns the microphone, shortcuts, voice activity detection, history and
paste. When Phonon is selected, Handy starts its own hidden CPU backend on a
private loopback port. Each session uses a fresh in-memory token; Handy checks
that the responding backend is its own. The model stays resident while needed.
Handy terminates only its own process on unload/exit, never an unrelated Python
or Fermion process. It does not attach to a pre-existing service on port 8010.

Serving uses the installed local model path with network access disabled in the
Python process. After the requested download/setup, Phonon recognition works
offline. This does not make separately enabled cloud post-processing offline:
ChatGPT cleanup and other cloud options retain their own settings. Local history
may store audio and transcripts according to your Handy settings.

## Requirements and limitations

- Windows x64 and a compatible x86-64 CPU; no GPU is needed. The upstream native
  Phonon kernels require SSE4.1. The complete bundled PyTorch/native dependency
  stack is validated on the hosted Windows runner, not every older CPU.
- English transcription only. No translation or streaming-display promise.
- The adapter accepts recordings up to 120 seconds. Cancel suppresses output;
  an already running decode may drain before another recording starts.
- The backend uses at most four CPU threads by default. Performance depends on
  your hardware and other CPU workloads. There is no fixed latency guarantee.
- Download requires internet and enough free disk space. Normal recognition does
  not download code or models at startup.

## Troubleshooting

- **Download/setup failed:** Retry from the model card. Check connectivity, disk
  space and the displayed error. Do not install unofficial wheels or disable
  security controls. An integrity failure means the download is not accepted.
- **Runtime missing/damaged:** Reinstall the complete Handy Phonon package. Copying
  only handy.exe omits required resources. The portable ZIP must be fully extracted.
- **Model fails to start:** Retry selection. If it still fails, retain the error
  text and confirm you are running the x64 release on a compatible CPU.
- **Shortcut/paste fails while ASR works:** Check microphone permission, input
  device and ordinary Handy shortcut/paste settings; test a normal Notepad window.
  Elevated applications may have Windows permission boundaries.

## Verification and attribution

The public Windows workflow verifies all installed runtime files against a build
inventory, checks embedded Python isolation and every dependency version, invokes
the same model-setup code used by Handy, and runs actual speech recognition twice
through Handy's production managed-process path. Paths contain spaces. It verifies
that the backend stops afterward. These checks gate publication, and the build
provenance records their success. Physical microphone, hotkey, paste and real
ChatGPT sign-in still need destination-device validation.

The runtime is locked in `scripts/phonon/runtime-lock.json`: official Python
3.13.16 x64, Fermion Research 0.2.7 and exact PyPI wheels with SHA-256 checks.
The model is pinned to revision `160671c34ffeae4d80d6f86896c68e40aad971a7` of
[FermionResearch/Phonon-2](https://huggingface.co/FermionResearch/Phonon-2/tree/160671c34ffeae4d80d6f86896c68e40aad971a7).
Its weights are CC BY 4.0, derived from NVIDIA parakeet-tdt-0.6b-v3. The exact
upstream attribution, changes and license terms ship under
`resources/phonon-runtime/notices/`, alongside an index of every bundled Python
package's original license notices. Handy's wrapper does not modify model weights.
Fermion code is Apache-2.0; Handy retains its MIT license. This is an independent
integration, not an official Handy, Fermion, NVIDIA or OpenAI release.
