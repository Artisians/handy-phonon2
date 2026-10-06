"""Assemble the app-local Windows runtime from reviewed, SHA-256 locked inputs.

No system Python installation, pip, cache or user profile is shipped. Invoked on
an ephemeral Windows CI runner, never by the end user or installer.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
import email
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import sys
import urllib.request
import zipfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def digest(path):
    with path.open("rb") as source:
        checksum = hashlib.sha256()
        for block in iter(lambda: source.read(1024 * 1024), b""):
            checksum.update(block)
        return checksum.hexdigest()


def fetch(item, directory):
    path = directory / item["filename"]
    if path.exists() and digest(path) == item["sha256"]:
        return path
    temporary = path.with_suffix(path.suffix + ".partial")
    with urllib.request.urlopen(item["url"], timeout=120) as response, temporary.open("wb") as target:
        shutil.copyfileobj(response, target, 1024 * 1024)
    if digest(temporary) != item["sha256"]:
        temporary.unlink()
        raise RuntimeError(f"SHA-256 mismatch: {item['filename']}")
    temporary.replace(path)
    return path


def safe_parts(name):
    parts = PurePosixPath(name).parts
    if not parts or name.startswith("/") or "\\" in name or any(p in {"..", "."} or ":" in p for p in parts):
        raise RuntimeError(f"Unsafe archive member: {name}")
    return parts


def unpack_wheel(wheel, python_dir):
    site = python_dir / "Lib" / "site-packages"
    with zipfile.ZipFile(wheel) as archive:
        for item in archive.infolist():
            parts = safe_parts(item.filename)
            base = site
            if parts[0].endswith(".data"):
                if len(parts) < 3:
                    continue
                if parts[1] in {"purelib", "platlib"}:
                    parts = parts[2:]
                elif parts[1] in {"data", "headers"}:
                    base = python_dir
                    parts = parts[2:]
                elif parts[1] == "scripts":
                    continue  # No user-facing Python commands/entry points.
                else:
                    raise RuntimeError(f"Unknown wheel installation scheme: {item.filename}")
            target = base.joinpath(*parts)
            if item.is_dir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with archive.open(item) as source, target.open("wb") as output:
                shutil.copyfileobj(source, output)


def inventory(root):
    return {p.relative_to(root).as_posix(): {"sha256": digest(p), "size": p.stat().st_size}
            for p in sorted(root.rglob("*")) if p.is_file() and p.name != "payload-manifest.json"}


def verify_payload(root):
    manifest = json.loads((root / "payload-manifest.json").read_text())
    actual = inventory(root)
    if actual != manifest["files"]:
        raise RuntimeError("Packaged Phonon runtime differs from the built inventory")
    return manifest


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--downloads", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "src-tauri/resources/phonon-runtime")
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args()
    if args.verify:
        report = verify_payload(args.output)
        print(f"Verified {len(report['files'])} packaged runtime files")
        return
    lock = json.loads((HERE / "runtime-lock.json").read_text())
    args.downloads.mkdir(parents=True, exist_ok=True)
    assets = [lock["python"], *lock["wheels"], *lock["model_notices"], *lock.get("additional_notices", [])]
    with ThreadPoolExecutor(max_workers=6) as pool:
        list(pool.map(lambda item: fetch(item, args.downloads), assets))
    if args.output.exists():
        raise RuntimeError("Runtime output already exists; use a clean CI checkout")
    python_dir = args.output / "python"
    python_dir.mkdir(parents=True)
    with zipfile.ZipFile(args.downloads / lock["python"]["filename"]) as archive:
        for item in archive.infolist():
            safe_parts(item.filename)
        archive.extractall(python_dir)
    # An isolated embedded path: no registry, environment PYTHONPATH, current
    # working directory, user site or executable .pth startup hooks.
    (python_dir / "python313._pth").write_text("python313.zip\n.\nLib/site-packages\n", encoding="utf-8")
    for wheel in lock["wheels"]:
        unpack_wheel(args.downloads / wheel["filename"], python_dir)
    # The official embed ZIP carries Python's CRT; Torch additionally needs
    # the app-local Microsoft C++ runtime already staged by the Windows build.
    for directory in filter(None, os.environ.get("HANDY_VC_REDIST_DIRS", "").split(";")):
        for dll in Path(directory).glob("*.dll"):
            shutil.copy2(dll, python_dir / dll.name)
    for name in ("start_server.py", "prepare_model.py", "runtime-lock.json"):
        shutil.copy2(HERE / name, args.output / name)
    notices = args.output / "notices"
    notices.mkdir()
    shutil.copy2(ROOT / "LICENSE", notices / "LICENSE-Handy.txt")
    for notice in [*lock["model_notices"], *lock.get("additional_notices", [])]:
        shutil.copy2(args.downloads / notice["filename"], notices / notice["filename"])
    shutil.copy2(HERE / "THIRD_PARTY_NOTICES.md", notices / "THIRD_PARTY_NOTICES.md")
    # Every wheel's original dist-info/licenses/License/NOTICE is retained,
    # including Torch's bundled component licenses. This manifest indexes them.
    package_rows = []
    for dist in sorted((python_dir / "Lib/site-packages").glob("*.dist-info")):
        metadata = email.message_from_bytes((dist / "METADATA").read_bytes())
        supplemental = ["notices/TOKENIZERS-LICENSE"] if metadata["Name"] == "tokenizers" else []
        package_rows.append({"name": metadata["Name"], "version": metadata["Version"],
                             "metadata": (dist / "METADATA").relative_to(args.output).as_posix(),
                             "licenses": supplemental + [p.relative_to(args.output).as_posix() for p in dist.rglob("*")
                                          if p.is_file() and any(k in p.as_posix().lower() for k in ("license", "notice", "copying"))]})
    (notices / "python-packages.json").write_text(json.dumps(package_rows, indent=2) + "\n")
    manifest = {"schema": 1, "python": lock["python"]["version"], "fermion": "0.2.7",
                "model_weights_included": False, "files": inventory(args.output)}
    (args.output / "payload-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    verify_payload(args.output)
    print(f"Bundled runtime: {len(manifest['files'])} files, {sum(v['size'] for v in manifest['files'].values())} bytes")


if __name__ == "__main__":
    main()
