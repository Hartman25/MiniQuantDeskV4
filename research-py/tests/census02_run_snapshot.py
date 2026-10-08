"""Stable snapshot of the real (gitignored) Census-02 run directory. Imports no Census-02 module, so it is safe to load before any."""

from __future__ import annotations

from pathlib import Path

REAL_RUN_DIR = Path(__file__).resolve().parents[1] / "runs" / "alpha_edge_census_02"


def tree_snapshot(root: Path) -> dict | None:
    """None when `root` is absent; otherwise {relative path: (size, mtime_ns)} for every file (stable, no content read)."""
    if not root.exists():
        return None
    return {str(p.relative_to(root)): (p.stat().st_size, p.stat().st_mtime_ns) for p in sorted(root.rglob("*")) if p.is_file()}
