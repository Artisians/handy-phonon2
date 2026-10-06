"""Handy's offline, process-owned wrapper around the unmodified Fermion server.

Runtime contract: python.exe -I -B start_server.py --port PORT --model-dir PATH --parent-pid PID, with an ephemeral
HANDY_PHONON_TOKEN environment value. Never install packages or download models.
"""
from __future__ import annotations

import argparse
import errno
import hmac
import hashlib
import os
from pathlib import Path
import re
import sys
import threading
import time
import uuid


def offline_only(event, args):
    # Defense in depth alongside local model paths and the Hugging Face offline
    # switches. This affects only this Python process, not Windows/firewall policy.
    if event in {"socket.connect", "socket.getaddrinfo"}:
        raise OSError("Network connections are disabled in the bundled speech runtime")


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
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--model-dir", type=Path, required=True)
    parser.add_argument("--parent-pid", type=int, required=True)
    args = parser.parse_args()
    watch_parent(args.parent_pid)
    if not 1024 <= args.port <= 65535:
        parser.error("port must be between 1024 and 65535")
    token = os.environ.pop("HANDY_PHONON_TOKEN", "")
    if not re.fullmatch(r"[A-Za-z0-9_-]{32,128}", token):
        parser.error("a private session token is required")
    model = args.model_dir.resolve()
    for name in ("config.json", "packed_manifest.json", "model.fermion"):
        if not (model / name).is_file():
            raise RuntimeError("Bundled Phonon model is incomplete; reinstall Handy Phonon")
    sys.dont_write_bytecode = True
    os.environ.update({
        "FERMION_DEVICE": "cpu", "HF_HUB_OFFLINE": "1",
        "TRANSFORMERS_OFFLINE": "1", "HF_HUB_DISABLE_TELEMETRY": "1",
        "DO_NOT_TRACK": "1", "TOKENIZERS_PARALLELISM": "false",
    })
    # One resident backend should not saturate a user's mining/workload CPU.
    threads = max(1, min(4, os.cpu_count() or 1))
    sys.addaudithook(offline_only)
    from fermion import server
    from fermion.cli import main as fermion_main

    class PrivateHandler(server.Handler):
        def _private_authorised(self):
            value = self.headers.get("Authorization", "")
            if not hmac.compare_digest(value, "Bearer " + token):
                self.close_connection = True
                self._err(401, "Invalid local session", code="invalid_api_key")
                return False
            return True

        def do_GET(self):
            self._handy_proof = None
            if self.path == "/health":
                # Prove knowledge of our out-of-band launch secret BEFORE the
                # caller sends any credential or audio to this candidate port.
                challenge = self.headers.get("X-Handy-Phonon-Challenge", "")
                try:
                    if len(challenge) != 36 or str(uuid.UUID(challenge)) != challenge.lower():
                        raise ValueError("invalid challenge")
                except ValueError:
                    self.close_connection = True
                    return self._err(400, "A fresh local session challenge is required")
                self._handy_proof = hmac.new(token.encode("ascii"), challenge.encode("ascii"), hashlib.sha256).hexdigest()
                return self._send_json(200, self._health())
            if self._private_authorised():
                return super().do_GET()

        def do_POST(self):
            self._handy_proof = None
            if self._private_authorised():
                return super().do_POST()

        def do_OPTIONS(self):
            self.close_connection = True
            self._err(405, "Browser access is disabled")

        def end_headers(self):
            proof = getattr(self, "_handy_proof", None)
            if proof:
                self.send_header("X-Handy-Phonon-Proof", proof)
            super().end_headers()

        def log_message(self, format, *args):
            # No request URLs, headers, audio, or transcript in packaged logs.
            return

    server.Handler = PrivateHandler
    sys.argv = ["fermion", "serve", str(model), "--host", "127.0.0.1",
                "--port", str(args.port), "--api-key", token,
                "--threads", str(threads), "--served-model-name", "FermionResearch/Phonon-2"]
    try:
        fermion_main()
    except OSError as exc:
        if exc.errno in {errno.EADDRINUSE, 10048} or getattr(exc, "winerror", None) == 10048:
            return 98
        raise
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
