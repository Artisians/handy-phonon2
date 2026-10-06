# Inherited workflows, deliberately inactive

These files are preserved from upstream Handy for reference. GitHub only reads
`.github/workflows/*.yml` and `.yaml`, so these inherited workflows cannot trigger
CI in this fork. In particular, their build matrices, caches, Actions artifacts,
signing, upstream release channels, and inherited automatic push/PR jobs are not enabled.

Do not move these files back into the active directory without reviewing cost,
permissions, fork identity, and publishing behavior. The sole supported workflow
is `../workflows/windows-free-release.yml`; see `docs/WINDOWS_RELEASE.md`.
