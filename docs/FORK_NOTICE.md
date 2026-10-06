# Handy Phonon: unofficial fork

Handy Phonon is an experimental fork of [Handy](https://github.com/cjpais/Handy),
based on version 0.9.8, commit `a94b403e0610049fafa54b0a4077db2945084dd8`.
Original copyright, MIT license, and contributor credits remain in [LICENSE](../LICENSE)
and the upstream project. This fork is not an official Handy, Fermion, or OpenAI
release and does not imply their endorsement.

The Windows application is identified as **Handy Phonon** with application ID
`io.github.artisians.handyphonon2`. It stores normal-install application data under
this separate identity. It does not migrate or overwrite an upstream Handy profile.
The executable retains its existing `handy.exe` filename; portable mode keeps its
settings in the adjacent `Data` directory. Extract each portable version into a
separate directory and do not share a portable directory with upstream Handy.

This fork has no signed self-update channel. Update checks are permanently
disabled, the upstream update endpoint is removed, and no upstream signing
credentials or public key are used. Review and install fork releases manually.

Windows builds are unsigned. An unknown-publisher/SmartScreen warning may appear.
SHA-256 hashes and a build-provenance JSON file identify published build outputs;
they are not a code-signing certificate or a cryptographically signed attestation.
Do not disable security controls to install this application.

The package includes the small upstream Silero VAD helper and runtime libraries,
but no full speech-recognition model weights, Phonon/Fermion server, user audio,
transcripts, account data, OAuth credentials or API keys. Runtime model/service
setup is described in [PHONON_WINDOWS.md](PHONON_WINDOWS.md).

Actual microphone capture, hotkeys, paste, GPU inference and ChatGPT plan eligibility
must be checked on the user's device. A passing hosted build does not establish
those end-to-end behaviors.

The unchanged Silero VAD v4 model bundled from upstream Handy is distributed under
the MIT license, copyright (c) 2020-present Silero Team. Its full notice is included
in `resources/models/SILERO_LICENSE.txt` in both Windows packages; the
[upstream Silero license](https://github.com/snakers4/silero-vad/blob/master/LICENSE)
provides the original attribution. The VAD helper detects speech activity; it is
not the Phonon-2 speech-recognition model.
