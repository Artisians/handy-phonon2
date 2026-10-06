# Public Windows build with no paid CI storage

## Cost boundary

This repository deliberately has one Windows workflow:
`.github/workflows/windows-free-release.yml`. It runs one standard `windows-2022`
x64 GitHub-hosted job, with a 120-minute limit and concurrency cancellation.
It is restricted at the job level to the public `Artisians/handy-phonon2`
repository's default branch. Reviewed code/build changes pushed to `main` build
and publish the approved 0.9.8 release series; manual dispatch is also available.
Path filters avoid rebuilding for unrelated documentation-only changes.
Private repositories are skipped before runner
allocation; do not change visibility while a run is active.

As checked on 2026-10-06, GitHub documents standard hosted runner use in public
repositories as free. Larger runners are chargeable. Actions artifact storage
shares a metered allowance with GitHub Packages; caches have a separate allowance.
This workflow uses neither Actions artifact uploads nor Actions Cache, Packages,
LFS, paid runners, external CI, Azure signing, or custom credentials. Local build
and package-manager caches are ephemeral files on the runner, never uploaded.
No billing limit is raised or payment method configured.

Outputs go directly to GitHub Releases. GitHub documents no total release-size or
bandwidth limit; each release asset must be under 2 GiB, which the packaging script
checks. This uses the documented Release distribution facility rather than
assuming any remaining Actions-storage quota. The workflow cannot guarantee that
unrelated account activity is free or that GitHub's policies will never change.

Official references:

- [Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)
- [Release storage and bandwidth](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases#storage-and-bandwidth-quotas)
- [Windows 2022 runner software](https://github.com/actions/runner-images/blob/main/images/windows/Windows2022-Readme.md)

## Approved main-branch builds

A push to `main` containing a relevant code, test, asset, build-script or workflow
change starts the same bounded public build and prerelease delivery. The initial
reviewed source publication and subsequent fixes therefore require no separate
workflow-dispatch API. The repository's initial README alone does not trigger a
build, and older concurrent runs are cancelled.

`scripts/ci/windows-build-policy.json` records the approved application release
series and exact Vulkan SDK version, SHA-256 and license URL for installation on
the temporary GitHub build machine. `prepare-windows.ps1` checks those values
against independent pinned constants before using the recorded license approval.
A new SDK, changed license/hash or application version fails this check and needs
a new review; it cannot silently reuse the existing approval. No private approval
conversation or account information is committed.

## Run manually

1. Review the public source and commit. Never commit credentials, recordings,
   transcripts, local model caches, or machine-specific account files.
2. Keep the repository public throughout the run. Open **Actions → Build Handy
   Phonon Windows (public, no paid storage) → Run workflow** on the default branch.
3. The Windows build needs Vulkan SDK **1.4.309.0**, matching upstream. Before
   setting `sdk_license_accepted=true`, approve its
   [official Windows component license text](https://vulkan.lunarg.com/software/license/vulkan-1.4.309.0-windows-license-summary.txt).
   The installer uses an explicit license-acceptance flag on the temporary runner.
   Nothing is installed on the user's computer by this workflow. The official
   runner inventory does not list this SDK, so it cannot be assumed preinstalled.
4. Set `publish_release=true` only when public publication of the reviewed build
   is approved. Both manual inputs default to false. With manual publication false, the build
   can be checked but its binaries disappear with the runner. There is no hidden
   artifact upload fallback.
5. Wait for the package checks and release link in the job summary. A failed or
   cancelled publication can leave an unpublished draft; inspect it before retrying.
   Runs and attempts use unique tags and never overwrite previous assets.

There are no pull-request, tag, release, or scheduled triggers. Inherited
upstream workflows are archived outside `.github/workflows` and do not run.
A matching `main` source push is the only automatic trigger.

## Toolchain and verification

The scripts use official Node setup and checkout actions pinned to verified full
commit SHAs, Node 22, Bun 1.3.11 from npm, stable Rust MSVC, the runner's Visual
Studio/CMake/vcpkg tools, Vulkan SDK 1.4.309.0 with its published checksum and an
Authenticode signer check, and Microsoft's ONNX Runtime 1.24.2 baseline build.
`GGML_NATIVE=OFF` avoids tailoring native code only to the CI host CPU.
Cargo and Bun use their checked-in lockfiles. Toolchain versions and source SHA
are recorded in `BUILD_PROVENANCE.json`; this is build metadata, not a signed
attestation or a claim of bit-for-bit reproducibility.

The workflow checks frontend lint/build and model-free mock tests, compiles the
NSIS installer, installs it silently in portable mode on the runner, verifies
packaged runtime DLLs/resources, and runs `--help` and `--list-devices` with timeouts.
It creates a portable ZIP from the actual installer payload, clears only the
throwaway smoke-test data, computes SHA-256 hashes, stages a draft release,
downloads all its assets again to verify hashes, and publishes a prerelease only
after those checks pass. It never signs into ChatGPT or records microphone audio.
The pinned model and a public speech fixture are downloaded into disposable CI
storage to verify actual ASR twice through Handy's managed runtime, including
automatic model reconstruction and backend shutdown. Test weights/audio are
never shipped in the installer or portable ZIP. The embedded Python runtime, all
42 dependencies and installed payload files are hash/version/isolation checked.

Only the single release job has `contents: write`, needed to deliver files
straight to Releases without an Actions-artifact transfer between jobs. Checkout
uses `persist-credentials: false`; the ephemeral `GITHUB_TOKEN` is supplied to
`gh` only during the final publishing step. There are no PATs, repository secrets,
OIDC grants or signing certificates. Only trusted default-branch source may run.

Published payloads:

- `Handy-Phonon-0.9.8-windows-x64-setup.exe`: unsigned NSIS installer
- `Handy-Phonon-0.9.8-windows-x64-portable.zip`: extract, then launch `handy.exe`
- `SHA256SUMS.txt`, `BUILD_PROVENANCE.json`, and `FORK_NOTICE.md`

The Python/Fermion CPU runtime is included internally, with complete dependency
notices. Full Phonon weights download through Handy's regular model manager; no
separate service setup is needed. The small upstream VAD resource is included.
See [fork identity and update policy](FORK_NOTICE.md) and
[local Phonon setup](PHONON_WINDOWS.md). Confirm microphone/hotkeys/paste, local
GPU inference and real account sign-in on the destination Windows PC; hosted
CLI checks cannot validate them.
