# Handy Phonon bundled speech runtime

This is an independent integration. No endorsement by Python, Fermion Research,
NVIDIA, PyTorch or the original Handy project is implied.

- Python 3.13.16, Python Software Foundation, PSF License Agreement and included
  third-party licenses: `../python/LICENSE.txt`.
  Source: https://www.python.org/downloads/release/python-31316/
- Fermion Research CLI/runtime 0.2.7, Apache License 2.0. Original LICENSE and
  NOTICE remain under `../python/Lib/site-packages/fermion_research-0.2.7.dist-info/licenses/`.
  Source: https://pypi.org/project/fermion-research/0.2.7/
- Every bundled Python dependency retains its original package license/notice
  files. `python-packages.json` indexes metadata and license paths, and
  `../runtime-lock.json` records exact wheel sources, versions and SHA-256 hashes.
  PyTorch's `torch-2.7.1.dist-info/LICENSE` includes its third-party component
  notices. Native DLLs included in those wheels retain the wheel's notices.
  The tokenizers wheel omits its license file; the exact release's Apache-2.0
  license is separately preserved as `TOKENIZERS-LICENSE` from
  https://github.com/huggingface/tokenizers/tree/v0.23.2.
- Phonon-2 is downloaded only when requested in Handy's model manager. Its weights
  are derived from NVIDIA parakeet-tdt-0.6b-v3 and distributed under CC BY 4.0.
  The complete upstream attribution, modification description and license terms
  are preserved here as `MODEL-NOTICE`, `MODEL-LICENSE-WEIGHTS-CC-BY-4.0.txt`,
  `MODEL-LICENSE-CODE-Apache-2.0.txt`, and `MODEL-README.md`.
  Source: https://huggingface.co/FermionResearch/Phonon-2/tree/160671c34ffeae4d80d6f86896c68e40aad971a7
  Handy reconstructs the exact published container, without changing weights.

Handy's wrapper adds authenticated process-owned loopback access and blocks
outgoing Python network connections during speech serving. These integration
scripts are distributed under this repository's MIT license (`LICENSE-Handy.txt`). The original
Fermion package files are not modified.
