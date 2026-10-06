"""Offline reconstruction of the pinned official model. No downloads or pip.

Usage: python.exe -I -B prepare_model.py ARCHIVE STAGING [--parent-pid PID]
Success creates STAGING. A killed/failed run may leave STAGING.partial; the
owning model manager cleans both paths and publishes STAGING atomically.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import threading
import time


def watch_parent(pid):
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        kernel.WaitForSingleObject.restype = wintypes.DWORD
        handle = kernel.OpenProcess(0x00100000, False, pid)  # SYNCHRONIZE only.
        if not handle:
            raise RuntimeError("The setup owner has exited or is unavailable")
        def wait():
            kernel.WaitForSingleObject(handle, 0xFFFFFFFF)
            os._exit(99)
    else:
        def wait():
            while os.getppid() == pid:
                time.sleep(0.5)
            os._exit(99)
    threading.Thread(target=wait, daemon=True).start()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("archive", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--parent-pid", type=int)
    args = parser.parse_args()
    if args.parent_pid is not None:
        watch_parent(args.parent_pid)
    sys.dont_write_bytecode = True
    # Reconstruction never connects to any network or looks up another model.
    def offline(event, arguments):
        if event in {"socket.connect", "socket.getaddrinfo"}:
            raise OSError("Network connections are disabled during model setup")
    sys.addaudithook(offline)
    lock = json.loads((Path(__file__).parent / "runtime-lock.json").read_text())
    archive = args.archive.resolve()
    destination = args.destination.resolve()
    if destination.exists() or destination.with_name(destination.name + ".partial").exists():
        raise RuntimeError("Model setup needs a fresh staging directory")
    with archive.open("rb") as source:
        actual = hashlib.file_digest(source, "sha256").hexdigest()
    if actual != lock["model"]["sha256"]:
        raise RuntimeError("Downloaded Phonon archive failed its SHA-256 check")
    from fermion._speech.fetch import _unpack
    _unpack(archive, destination)
    manifest = json.loads((destination / "packed_manifest.json").read_text())
    with (destination / "model.fermion").open("rb") as source:
        actual = hashlib.file_digest(source, "sha256").hexdigest()
    if actual != lock["model"]["container_sha256"] or actual != manifest["container_sha256"]:
        raise RuntimeError("Reconstructed model digest mismatch")
    print(json.dumps({"status": "ok", "model": "FermionResearch/Phonon-2", "sha256": actual}))


if __name__ == "__main__":
    main()
