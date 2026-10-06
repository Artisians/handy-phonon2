"""CI-only import, ABI, dependency and isolation checks using bundled Python."""
from importlib import metadata
import json
from pathlib import Path
import sys

from packaging.requirements import Requirement
from packaging.utils import canonicalize_name

root = Path(sys.argv[1]).resolve()
lock = json.loads((root / "runtime-lock.json").read_text())
assert sys.version_info[:3] == tuple(map(int, lock["python"]["version"].split(".")))
assert Path(sys.prefix).resolve() == root / "python"
for path in sys.path:
    assert Path(path).resolve().is_relative_to(root / "python"), path
expected = {canonicalize_name(row["name"]): row["version"] for row in lock["wheels"]}
actual = {canonicalize_name(dist.metadata["Name"]): dist.version for dist in metadata.distributions()}
assert expected == actual, (expected, actual)
for dist in metadata.distributions():
    for raw in dist.requires or []:
        requirement = Requirement(raw)
        if requirement.marker and not requirement.marker.evaluate({"extra": ""}):
            continue
        version = actual[canonicalize_name(requirement.name)]
        assert requirement.specifier.contains(version, prereleases=True), raw
import torch
import numpy
import scipy.signal
import soundfile
import safetensors
import zstandard
import transformers
from fermion import server
from fermion._speech import backends
assert torch.version.cuda is None, "This runtime must be CPU-only"
assert torch.ones(3).sum().item() == 3
print(json.dumps({"status": "ok", "python": sys.version.split()[0], "packages": len(actual),
                  "torch": torch.__version__, "cpu_only": True, "isolated": True}))
