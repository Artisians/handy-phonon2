param([Parameter(Mandatory = $true)][string]$ApplicationDirectory)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$runtime = Join-Path $ApplicationDirectory 'resources/phonon-runtime'
$python = Join-Path $runtime 'python/python.exe'
python scripts/phonon/build_runtime.py --downloads (Join-Path $env:RUNNER_TEMP 'phonon-locked-downloads') --output $runtime --verify
if ($LASTEXITCODE -ne 0) { throw 'Installed Phonon runtime inventory mismatch' }
& $python -I -B scripts/phonon/verify_runtime.py $runtime
if ($LASTEXITCODE -ne 0) { throw 'Installed runtime fails ABI/isolation checks' }
$testRoot = Join-Path $env:RUNNER_TEMP 'Handy Phonon ASR test'
New-Item $testRoot -ItemType Directory -Force | Out-Null
# Only CI fetches the fixture/model. Serving and setup both explicitly prohibit
# outgoing Python network connections; no pre-existing HF/model cache is used.
python -c "import sys; sys.path.insert(0,'scripts/phonon'); from build_runtime import fetch; import json; from pathlib import Path; lock=json.load(open('scripts/phonon/runtime-lock.json')); root=Path(sys.argv[1]); fetch(lock['model'],root); fetch(lock['test_audio'],root)" $testRoot
if ($LASTEXITCODE -ne 0) { throw 'Pinned ASR test inputs failed to download/verify' }
$model = Join-Path $testRoot 'downloaded model'
cargo run --locked --manifest-path tests/phonon-model-install/Cargo.toml --example install -- $runtime (Join-Path $testRoot 'phonon-2.bps.tar.zst') $model
if ($LASTEXITCODE -ne 0) { throw 'Installed automatic model setup failed' }
$wav = Join-Path $testRoot 'speech fixture.wav'
& $python -I -B -m unittest discover -s tests/phonon-audio -v
if ($LASTEXITCODE -ne 0) { throw 'ASR fixture conversion regression tests failed' }
& $python -I -B scripts/phonon/prepare_test_audio.py (Join-Path $testRoot 'jfk.flac') $wav
if ($LASTEXITCODE -ne 0) { throw 'ASR fixture conversion failed' }
$output = Join-Path $testRoot 'production-asr.json'
$exe = Join-Path $ApplicationDirectory 'handy.exe'
$arguments = @('--transcribe-file', "`"$wav`"", '--model', 'phonon-2-local', '--json', '--repeat', '2', '--phonon-test-model-dir', "`"$model`"", '--phonon-test-output', "`"$output`"")
$stdout = Join-Path $testRoot 'production-asr.stdout.txt'
$stderr = Join-Path $testRoot 'production-asr.stderr.txt'
function Write-AsrDiagnostics {
    # This is an ephemeral runner with only the pinned public speech fixture.
    # Bound diagnostic output and never include environment variables/credentials.
    foreach ($path in @($stderr, $stdout, $output)) {
        if (Test-Path $path) {
            $diagnostic = (Get-Content $path -Tail 60) -join "`n"
            if ($diagnostic.Length -gt 12000) { $diagnostic = $diagnostic.Substring($diagnostic.Length - 12000) }
            Write-Host "ASR diagnostic ($([System.IO.Path]::GetFileName($path))):"
            Write-Host $diagnostic
        }
    }
}
$process = Start-Process $exe -ArgumentList $arguments -WorkingDirectory $ApplicationDirectory -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
if (-not $process.WaitForExit(360000)) {
    $process.Kill()
    $process.WaitForExit(10000) | Out-Null
    Write-AsrDiagnostics
    throw 'Production managed Phonon ASR timed out'
}
if ($process.ExitCode -ne 0 -or -not (Test-Path $output)) {
    Write-AsrDiagnostics
    throw "Production managed Phonon ASR failed: $($process.ExitCode)"
}
$result = Get-Content $output -Raw | ConvertFrom-Json
if ($result.runtime_stopped -ne $true -or $result.texts.Count -ne 2) { throw 'Managed ASR repeat/shutdown proof missing' }
foreach ($text in $result.texts) {
    if ($text -notmatch '(?i)ask not' -or $text -notmatch '(?i)country') { throw 'Actual Phonon ASR did not recover the known speech fixture' }
}
# No text is copied into shipped payloads. Test audio is public JFK speech from
# the pinned OpenAI Whisper test fixture; no user microphone or account is used.
Write-Host 'Actual installed Phonon ASR passed using the production managed process path.'
$provenancePath = Join-Path $env:RUNNER_TEMP 'handy-provenance.json'
$provenance = Get-Content $provenancePath -Raw | ConvertFrom-Json -AsHashtable
$provenance['phonon_installed_runtime_asr_verified'] = $true
$provenance['phonon_model_setup_verified'] = $true
$provenance | ConvertTo-Json -Depth 5 | Set-Content $provenancePath -Encoding utf8NoBOM
