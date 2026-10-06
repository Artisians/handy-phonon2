$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Recheck live visibility, because the repository could change during the build.
$repoJson = & gh api "repos/$env:GITHUB_REPOSITORY"
if ($LASTEXITCODE -ne 0) { throw 'Unable to recheck repository visibility' }
$repo = $repoJson | ConvertFrom-Json
if ($repo.full_name -ne 'Artisians/handy-phonon2' -or $repo.private -or $repo.visibility -ne 'public') {
    throw 'Delivery is restricted to the approved public repository'
}
$version = (Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json).version
$tag = "phonon-v$version-$($env:GITHUB_SHA.Substring(0, 8))-run$env:GITHUB_RUN_ID-$env:GITHUB_RUN_ATTEMPT"
$out = $env:HANDY_DELIVERY_DIR
$files = @(Get-ChildItem $out -File | Sort-Object Name)
if ($files.Count -ne 5) { throw 'Unexpected delivery contents; refusing to publish' }
$notes = Join-Path $env:RUNNER_TEMP 'release-notes.md'
@"
# Handy Phonon $version (unofficial Windows x64 prerelease)

Based on Handy v0.9.8; this fork is not an official Handy, Fermion, or OpenAI release.

- Source commit: $env:GITHUB_SHA
- Build and smoke checks: https://github.com/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID
- Download the setup EXE, or extract the portable ZIP and run handy.exe.
- SHA256SUMS.txt verifies every payload; BUILD_PROVENANCE.json records toolchain and source.
- **Unsigned:** Windows may warn about an unknown publisher. No code-signing certificate or signed provenance is claimed. Respect device policy; do not disable Windows security.
- Full speech-model weights, Phonon/Fermion server, user recordings and login credentials are not included.
- Phonon-2 requires the separately configured local service; see docs/PHONON_WINDOWS.md.
- Self-updates are disabled. Install reviewed fork releases manually.
- CLI/payload tests do not validate the microphone, hotkey/paste flow, GPU performance, or real ChatGPT authentication on your PC. Those checks remain required.

This release is stored as GitHub Release assets, not Actions artifacts or caches.
"@ | Set-Content $notes -Encoding utf8NoBOM

# Create an unpublished draft, upload, download/read back every asset, then publish.
# Never overwrite another release; a failed/uncertain upload leaves a draft for review.
$paths = @($files | ForEach-Object FullName)
& gh release create $tag --repo $env:GITHUB_REPOSITORY --target $env:GITHUB_SHA --draft --prerelease --latest=false --title "Handy Phonon $version - Windows x64" --notes-file $notes @paths
if ($LASTEXITCODE -ne 0) { throw "Release staging failed; inspect draft $tag before any retry" }
$verify = Join-Path $env:RUNNER_TEMP 'handy-delivery-readback'
New-Item $verify -ItemType Directory | Out-Null
& gh release download $tag --repo $env:GITHUB_REPOSITORY --dir $verify
if ($LASTEXITCODE -ne 0) { throw "Release readback failed; $tag remains a draft" }
$remoteFiles = @(Get-ChildItem $verify -File)
if ($remoteFiles.Count -ne $files.Count) { throw 'Release file count mismatch' }
foreach ($file in $files) {
    $downloaded = Join-Path $verify $file.Name
    if (-not (Test-Path $downloaded) -or (Get-FileHash $downloaded -Algorithm SHA256).Hash -ne (Get-FileHash $file.FullName -Algorithm SHA256).Hash) {
        throw "Release readback checksum mismatch: $($file.Name)"
    }
}
& gh release edit $tag --repo $env:GITHUB_REPOSITORY --draft=false --prerelease --latest=false
if ($LASTEXITCODE -ne 0) { throw "Publish result uncertain; inspect $tag, do not blindly rerun" }
$verifiedJson = & gh release view $tag --repo $env:GITHUB_REPOSITORY --json isDraft,isPrerelease,url,assets
if ($LASTEXITCODE -ne 0) { throw 'Published release readback failed' }
$verified = $verifiedJson | ConvertFrom-Json
if ($verified.isDraft -or -not $verified.isPrerelease -or $verified.assets.Count -ne $files.Count) { throw 'Unexpected final release state' }
"Published and downloaded back with matching SHA-256: $($verified.url)" >> $env:GITHUB_STEP_SUMMARY
Write-Host $verified.url
