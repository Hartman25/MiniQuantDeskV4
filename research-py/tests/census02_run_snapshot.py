"""Content-strong snapshot of the real (gitignored) Census-02 run directory. Imports no Census-02 module, so it is safe to load before any."""

from __future__ import annotations

import hashlib
from pathlib import Path

REAL_RUN_DIR = Path(__file__).resolve().parents[1] / "runs" / "alpha_edge_census_02"


def tree_snapshot(root: Path) -> dict | None:
    """None when `root` is absent; otherwise {relative path: {size, sha256, mtime_ns}} for every file under `root` only.
    The SHA-256 is the content-change detector; size and mtime are supplemental (a same-size, mtime-restored rewrite still differs)."""
    if not root.exists():
        return None
    out = {}
    for p in sorted(root.rglob("*")):
        if p.is_file():
            with open(p, "rb") as f:
                digest = hashlib.file_digest(f, "sha256").hexdigest()
            st = p.stat()
            out[str(p.relative_to(root))] = {"size": st.st_size, "sha256": digest, "mtime_ns": st.st_mtime_ns}
    return out
