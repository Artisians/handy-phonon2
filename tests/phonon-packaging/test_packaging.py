import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("build_runtime", ROOT / "scripts/phonon/build_runtime.py")
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)


class PackagingTests(unittest.TestCase):
    def test_archive_member_paths(self):
        for name in ("/escape", "../escape", "package/../../escape", "C:/escape", "a\\b"):
            with self.subTest(name=name), self.assertRaises(RuntimeError):
                BUILD.safe_parts(name)
        self.assertEqual(BUILD.safe_parts("package/data/file"), ("package", "data", "file"))

    def test_wheel_installation_layout_preserves_licenses(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            wheel = root / "test.whl"
            with zipfile.ZipFile(wheel, "w") as archive:
                archive.writestr("sample/__init__.py", "")
                archive.writestr("sample-1.dist-info/licenses/LICENSE", "original attribution")
                archive.writestr("sample-1.data/purelib/helper.py", "")
                archive.writestr("sample-1.data/scripts/user-command.exe", "not installed")
            BUILD.unpack_wheel(wheel, root / "python")
            site = root / "python/Lib/site-packages"
            self.assertEqual((site / "sample-1.dist-info/licenses/LICENSE").read_text(), "original attribution")
            self.assertTrue((site / "helper.py").exists())
            self.assertFalse(list((root / "python").rglob("*.exe")))

    def test_payload_inventory_detects_missing_changed_and_extra_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "payload").write_bytes(b"locked")
            (root / "payload-manifest.json").write_text(json.dumps({"files": BUILD.inventory(root)}))
            BUILD.verify_payload(root)
            (root / "extra").write_bytes(b"unreviewed")
            with self.assertRaises(RuntimeError):
                BUILD.verify_payload(root)
            (root / "extra").unlink()
            (root / "payload").write_bytes(b"changed")
            with self.assertRaises(RuntimeError):
                BUILD.verify_payload(root)
            (root / "payload").unlink()
            with self.assertRaises(RuntimeError):
                BUILD.verify_payload(root)

    def test_lock_is_exact_official_cpu_windows_dependencies(self):
        lock = json.loads((ROOT / "scripts/phonon/runtime-lock.json").read_text())
        self.assertEqual(lock["python"]["version"], "3.13.16")
        self.assertEqual(lock["model"]["revision"], "160671c34ffeae4d80d6f86896c68e40aad971a7")
        self.assertEqual(len(lock["wheels"]), 42)
        for item in lock["wheels"]:
            self.assertTrue(item["url"].startswith("https://files.pythonhosted.org/"))
            self.assertRegex(item["sha256"], r"^[a-f0-9]{64}$")
            self.assertNotIn("manylinux", item["filename"])
            self.assertFalse(item["name"].lower().startswith("nvidia-"))
        self.assertEqual(next(row["version"] for row in lock["wheels"] if row["name"] == "fermion-research"), "0.2.7")
        hashes = (ROOT / "scripts/phonon/requirements-win.lock").read_text()
        for row in lock["wheels"]:
            self.assertIn(f"{row['name']}=={row['version']} --hash=sha256:{row['sha256']}", hashes)


if __name__ == "__main__":
    unittest.main()
