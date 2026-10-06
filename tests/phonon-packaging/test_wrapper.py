"""The runtime's offline guard and abrupt-parent-exit behavior, without a model."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
WRAPPER = ROOT / "scripts/phonon/start_server.py"


class WrapperTests(unittest.TestCase):
    def test_offline_guard_rejects_outgoing_connections(self):
        spec = importlib.util.spec_from_file_location("wrapper", WRAPPER)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        for event in ("socket.connect", "socket.getaddrinfo"):
            with self.assertRaises(OSError):
                module.offline_only(event, ())
        module.offline_only("socket.bind", ())

    def test_child_exits_when_parent_dies(self):
        with tempfile.TemporaryDirectory() as directory:
            ready = Path(directory) / "ready"
            # The nested parent waits until its child owns a liveness handle,
            # then exits without terminating that child. stdout's EOF proves the
            # child exits itself; a missing watchdog fails with a 15s timeout.
            child = (
                "import os,runpy,time,pathlib; "
                f"m=runpy.run_path({str(WRAPPER)!r}); "
                "m['watch_parent'](os.getppid()); "
                f"pathlib.Path({str(ready)!r}).write_text('ready'); "
                "print('watching',flush=True); time.sleep(60)"
            )
            parent = (
                "import subprocess,sys,time,pathlib; "
                f"p=subprocess.Popen([sys.executable,'-I','-B','-c',{child!r}]); "
                f"ready=pathlib.Path({str(ready)!r}); "
                "deadline=time.monotonic()+10\n"
                "while not ready.exists() and time.monotonic()<deadline: time.sleep(.02)\n"
                "assert ready.exists()\n"
            )
            result = subprocess.run([sys.executable, "-I", "-B", "-c", parent],
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    timeout=15, check=True, text=True)
            self.assertEqual(result.stdout.strip(), "watching")


if __name__ == "__main__":
    unittest.main()
