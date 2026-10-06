$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$releaseDir = Join-Path $env:CARGO_TARGET_DIR 'release'
$installers = @(Get-ChildItem (Join-Path $releaseDir 'bundle/nsis') -Filter '*.exe')
if ($installers.Count -ne 1) { throw "Expected exactly one NSIS installer, found $($installers.Count)" }
$installer = $installers[0]
$stagedDlls = @(Get-ChildItem src-tauri/transcribe-libs -Filter '*.dll')
foreach ($required in @('msvcp140.dll', 'vcruntime140.dll', 'vcomp140.dll', 'onnxruntime.dll')) {
    if ($required -notin $stagedDlls.Name) { throw "Staging missing $required" }
}
foreach ($pattern in @('*transcribe*', '*ggml*')) {
    if (-not ($stagedDlls | Where-Object Name -Like $pattern)) { throw "Staging missing $pattern runtime" }
}
if ((Get-AuthenticodeSignature $installer.FullName).Status -ne 'NotSigned') { throw 'Expected an explicitly unsigned fork installer' }

# Exercise the actual installer in its upstream-supported portable mode.
$portable = Join-Path $env:RUNNER_TEMP 'Handy-Phonon-Portable'
New-Item $portable -ItemType Directory -Force | Out-Null
$install = Start-Process $installer.FullName -ArgumentList @('/S', '/PORTABLE', "/D=$portable") -PassThru
if (-not $install.WaitForExit(120000)) { $install.Kill(); throw 'Portable installer timed out' }
if ($install.ExitCode -ne 0) { throw "Portable installer failed: $($install.ExitCode)" }
$exe = Join-Path $portable 'handy.exe'
foreach ($file in @($exe, (Join-Path $portable 'portable'), (Join-Path $portable 'resources/models/silero_vad_v4.onnx'), (Join-Path $portable 'resources/models/SILERO_LICENSE.txt'))) {
    if (-not (Test-Path $file)) { throw "Package missing $file" }
}
foreach ($dll in $stagedDlls) {
    if (-not (Test-Path (Join-Path $portable $dll.Name))) { throw "Package missing runtime DLL $($dll.Name)" }
}

# No microphone, login, transcription request or full speech model is needed.
foreach ($argument in @('--help', '--list-devices')) {
    $stdout = Join-Path $env:RUNNER_TEMP "smoke-$($argument.TrimStart('-')).txt"
    $stderr = "$stdout.err"
    $process = Start-Process $exe -ArgumentList $argument -WorkingDirectory $portable -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
    if (-not $process.WaitForExit(30000)) { $process.Kill(); throw "CLI smoke test timed out: $argument" }
    if ($process.ExitCode -ne 0) { Get-Content $stderr; throw "CLI smoke test failed: $argument, exit $($process.ExitCode)" }
    $output = Get-Content $stdout -Raw
    if ($argument -eq '--help' -and $output -notmatch '--list-devices') { throw 'CLI help output missing expected option' }
    if ($argument -eq '--list-devices' -and $output -notmatch 'kind=cpu') { throw 'CPU inference backend not listed' }
    Write-Host $output
}

# Remove the smoke run's app data before distributing the portable directory.
# Everything here is disposable CI-generated test output, never user data.
$data = Join-Path $portable 'Data'
if (Test-Path $data) { Remove-Item $data -Recurse -Force }
New-Item $data -ItemType Directory | Out-Null
Copy-Item LICENSE (Join-Path $portable 'LICENSE-Handy.txt')
Copy-Item docs/FORK_NOTICE.md (Join-Path $portable 'FORK_NOTICE.md')
$version = (Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json).version
$stem = "Handy-Phonon-$version-windows-x64"
$out = Join-Path $env:RUNNER_TEMP 'handy-delivery'
New-Item $out -ItemType Directory -Force | Out-Null
Copy-Item $installer.FullName (Join-Path $out "$stem-setup.exe")
Compress-Archive -Path "$portable/*" -DestinationPath (Join-Path $out "$stem-portable.zip") -CompressionLevel Optimal
Copy-Item (Join-Path $env:RUNNER_TEMP 'handy-provenance.json') (Join-Path $out 'BUILD_PROVENANCE.json')
Copy-Item docs/FORK_NOTICE.md $out
$hashes = foreach ($file in (Get-ChildItem $out -File | Sort-Object Name)) {
    if ($file.Length -ge 2GB) { throw "Release asset exceeds GitHub's per-file limit: $($file.Name)" }
    "{0}  {1}" -f (Get-FileHash $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $file.Name
}
$hashes | Set-Content (Join-Path $out 'SHA256SUMS.txt') -Encoding utf8NoBOM
"HANDY_DELIVERY_DIR=$out" >> $env:GITHUB_ENV
@"
## Windows package checks passed
- NSIS portable installation, packaged DLL/resource presence, CLI help and CPU device enumeration passed.
- Unsigned installer plus portable ZIP; no full speech models or authentication data.
- Microphone, hotkeys, GPU inference and real ChatGPT sign-in still need user-device validation.
"@ >> $env:GITHUB_STEP_SUMMARY
