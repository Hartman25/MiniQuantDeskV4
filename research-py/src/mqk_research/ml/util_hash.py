from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any, Dict


def sha256_bytes(b: bytes) -> str:
    h = hashlib.sha256()
    h.update(b)
    return h.hexdigest()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def sha256_json(obj: Any) -> str:
    # canonical json for deterministic hashing
    s = json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    return sha256_bytes(s)


def file_record(path: Path) -> Dict[str, Any]:
    path = Path(path)
    return {
        "path": str(path),
        "bytes": int(path.stat().st_size) if path.exists() else None,
        "sha256": sha256_file(path) if path.exists() else None,
    }
