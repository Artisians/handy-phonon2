# Used only on an ephemeral, standard public GitHub Windows runner.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$config = Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json -AsHashtable
if ($config.productName -ne 'Handy Phonon' -or $config.identifier -ne 'io.github.artisians.handyphonon2') {
    throw 'Refusing to package a build with the upstream identity'
}
if ($config.bundle.createUpdaterArtifacts -or $config.plugins.ContainsKey('updater') -or $config.bundle.windows.ContainsKey('signCommand')) {
    throw 'Fork must have no upstream updater/signing configuration'
}
$activeWorkflows = @(Get-ChildItem .github/workflows -File | Where-Object { $_.Extension -in @('.yml', '.yaml') })
if ($activeWorkflows.Count -ne 1 -or $activeWorkflows[0].Name -ne 'windows-free-release.yml') {
    throw 'Unexpected active workflow: review cost/permissions before building'
}

# Main-branch push builds may use only the explicitly approved SDK and release
# series. A version/hash/terms change needs a new reviewed approval, not a silent
# upgrade. Manual dispatch still uses its explicit sdk_license_accepted input.
$policy = Get-Content scripts/ci/windows-build-policy.json -Raw | ConvertFrom-Json
$approvedSdkHash = '48b132169b64fe65cdb0f20970195335a65354e73f1ea5373032c2a8bbad4297'
$approvedSdkLicense = 'https://vulkan.lunarg.com/software/license/vulkan-1.4.309.0-windows-license-summary.txt'
if ($policy.repository -ne 'Artisians/handy-phonon2' -or
    $policy.applicationVersion -ne $config.version -or
    $policy.applicationVersion -ne '0.9.8' -or
    $policy.sdk.version -ne '1.4.309.0' -or
    $policy.sdk.sha256 -ne $approvedSdkHash -or
    $policy.sdk.licenseUrl -ne $approvedSdkLicense) {
    throw 'Build policy changed outside the approved release/SDK scope; review before running'
}
$sdkLicenseAccepted = $env:SDK_LICENSE_ACCEPTED -eq 'true'
if ($env:GITHUB_EVENT_NAME -eq 'push') {
    $sdkLicenseAccepted = $policy.sdk.approvedForTemporaryGitHubBuildMachine -eq $true
}

# No state is uploaded to Actions Cache; local package caches die with this VM.
npm install --global bun@1.3.11
if ($LASTEXITCODE -ne 0) { throw 'Bun installation failed' }
rustup toolchain install stable --profile minimal
if ($LASTEXITCODE -ne 0) { throw 'Rust installation failed' }
rustup default stable
if ($LASTEXITCODE -ne 0) { throw 'Rust selection failed' }
rustup target add x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw 'Rust MSVC target installation failed' }
foreach ($tool in @('bun', 'cargo', 'cmake', 'vcpkg', 'gh')) { Get-Command $tool -ErrorAction Stop | Out-Null }

# Same Vulkan SDK as the upstream v0.9.8 workflow. Pinned official checksum:
# https://vulkan.lunarg.com/sdk/files.json (Windows SDK 1.4.309.0).
$sdkVersion = '1.4.309.0'
$sdk = "C:\VulkanSDK\$sdkVersion"
if (-not (Test-Path "$sdk\Bin\glslc.exe")) {
    if (-not $sdkLicenseAccepted) {
        throw 'Vulkan SDK installation needs approval of https://vulkan.lunarg.com/software/license/vulkan-1.4.309.0-windows-license-summary.txt; rerun with sdk_license_accepted only after approval.'
    }
    $installer = Join-Path $env:RUNNER_TEMP 'VulkanSDK-Installer.exe'
    Invoke-WebRequest "https://sdk.lunarg.com/sdk/download/$sdkVersion/windows/VulkanSDK-$sdkVersion-Installer.exe" -OutFile $installer
    $expected = $approvedSdkHash
    if ((Get-FileHash $installer -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'Vulkan SDK checksum mismatch' }
    $signature = Get-AuthenticodeSignature $installer
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'LunarG') { throw 'Unexpected Vulkan SDK signer' }
    $process = Start-Process $installer -ArgumentList @('--root', $sdk, '--accept-licenses', '--default-answer', '--confirm-command', 'install') -PassThru
    if (-not $process.WaitForExit(900000)) { $process.Kill(); throw 'Vulkan SDK install timed out' }
    if ($process.ExitCode -ne 0) { throw "Vulkan SDK installation failed: $($process.ExitCode)" }
}
if (-not (Test-Path "$sdk\Bin\glslc.exe")) { throw 'Vulkan compiler is missing' }
"VULKAN_SDK=$sdk" >> $env:GITHUB_ENV
"$sdk\Bin" >> $env:GITHUB_PATH

# GGML's Vulkan backend needs a CMake-discoverable SPIRV-Headers package.
vcpkg install spirv-headers:x64-windows --disable-metrics
if ($LASTEXITCODE -ne 0) { throw 'SPIRV-Headers installation failed' }
$prefix = "$env:VCPKG_INSTALLATION_ROOT/installed/x64-windows" -replace '\\', '/'
"CMAKE_PREFIX_PATH=$prefix" >> $env:GITHUB_ENV
# Avoid compiling only for the runner's CPU: the destination PC may be older.
'TRANSCRIBE_CMAKE_ARGS=-DGGML_NATIVE=OFF' >> $env:GITHUB_ENV

# Stage Microsoft VC++ runtime DLLs beside the application via build.rs.
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vsRoot = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vsRoot) { throw 'MSVC build tools not found' }
$redistVersion = (Get-Content "$vsRoot\VC\Auxiliary\Build\Microsoft.VCRedistVersion.default.txt").Trim()
$archDir = "$vsRoot\VC\Redist\MSVC\$redistVersion\x64"
$crt = Get-ChildItem $archDir -Directory -Filter 'Microsoft.VC*.CRT' | Sort-Object Name | Select-Object -Last 1
$omp = Get-ChildItem $archDir -Directory -Filter 'Microsoft.VC*.OpenMP' | Sort-Object Name | Select-Object -Last 1
if (-not $crt -or -not $omp -or -not (Test-Path "$($crt.FullName)\msvcp140.dll") -or -not (Test-Path "$($omp.FullName)\vcomp140.dll")) {
    throw 'App-local Microsoft runtime files are missing'
}
"HANDY_VC_REDIST_DIRS=$($crt.FullName);$($omp.FullName)" >> $env:GITHUB_ENV

# Official baseline ONNX Runtime avoids startup crashes on non-AVX2 machines.
$ortVersion = '1.24.2'
$ortZip = Join-Path $env:RUNNER_TEMP 'onnxruntime.zip'
Invoke-WebRequest "https://github.com/microsoft/onnxruntime/releases/download/v$ortVersion/onnxruntime-win-x64-$ortVersion.zip" -OutFile $ortZip
$ortRoot = Join-Path $env:RUNNER_TEMP 'onnxruntime'
Expand-Archive $ortZip $ortRoot
$ortLib = Join-Path $ortRoot "onnxruntime-win-x64-$ortVersion\lib"
if (-not (Test-Path "$ortLib\onnxruntime.dll")) { throw 'ONNX Runtime DLL missing' }
"ORT_LIB_LOCATION=$ortLib" >> $env:GITHUB_ENV
'ORT_PREFER_DYNAMIC_LINK=1' >> $env:GITHUB_ENV

# The small VAD helper is already versioned upstream; no speech model download.
if (-not (Test-Path src-tauri/resources/models/silero_vad_v4.onnx)) { throw 'Versioned VAD resource is missing' }
$provenance = [ordered]@{
    source_repository = $env:GITHUB_REPOSITORY
    source_commit = $env:GITHUB_SHA
    upstream_base = 'a94b403e0610049fafa54b0a4077db2945084dd8'
    workflow_url = "https://github.com/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID"
    runner_image = $env:ImageOS
    runner_image_version = $env:ImageVersion
    node = (& node --version | Out-String).Trim()
    bun = (& bun --version | Out-String).Trim()
    rustc = (& rustc --version | Out-String).Trim()
    cargo = (& cargo --version | Out-String).Trim()
    cmake = (& cmake --version | Out-String).Trim()
    vulkan_sdk = $sdkVersion
    onnx_runtime = $ortVersion
    onnx_archive_sha256 = (Get-FileHash $ortZip -Algorithm SHA256).Hash.ToLowerInvariant()
    code_signed = $false
    contains_speech_model_weights = $false
    contains_user_audio_or_oauth_credentials = $false
}
$provenance | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $env:RUNNER_TEMP 'handy-provenance.json') -Encoding utf8NoBOM
