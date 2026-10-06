$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$downloads = Join-Path $env:RUNNER_TEMP 'phonon-locked-downloads'
python scripts/phonon/build_runtime.py --downloads $downloads
if ($LASTEXITCODE -ne 0) { throw 'Locked Phonon runtime assembly failed' }
$runtime = (Resolve-Path 'src-tauri/resources/phonon-runtime').Path
$python = Join-Path $runtime 'python/python.exe'
$signature = Get-AuthenticodeSignature $python
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'Python Software Foundation') {
    throw 'Bundled Python executable has an unexpected signature'
}
& $python -I -B scripts/phonon/verify_runtime.py $runtime
if ($LASTEXITCODE -ne 0) { throw 'Bundled Python dependency/ABI/isolation checks failed' }
$provenancePath = Join-Path $env:RUNNER_TEMP 'handy-provenance.json'
$provenance = Get-Content $provenancePath -Raw | ConvertFrom-Json -AsHashtable
$lock = Get-Content scripts/phonon/runtime-lock.json -Raw | ConvertFrom-Json
$provenance['phonon_python'] = $lock.python.version
$provenance['phonon_runtime'] = 'fermion-research==0.2.7'
$provenance['phonon_runtime_lock_sha256'] = (Get-FileHash scripts/phonon/runtime-lock.json -Algorithm SHA256).Hash.ToLowerInvariant()
$provenance['contains_managed_phonon_runtime'] = $true
$provenance['contains_speech_model_weights'] = $false
$provenance['phonon_model_download_revision'] = $lock.model.revision
$provenance | ConvertTo-Json -Depth 5 | Set-Content $provenancePath -Encoding utf8NoBOM
