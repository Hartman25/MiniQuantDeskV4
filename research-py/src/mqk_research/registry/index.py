from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict

from mqk_research.ml.util_hash import sha256_json


def registry_root(project_root: Path) -> Path:
    return Path(project_root) / "registry"


def append_index(project_root: Path, record: Dict[str, Any]) -> None:
    root = registry_root(project_root)
    root.mkdir(parents=True, exist_ok=True)
    idx = root / "index.jsonl"

    record = dict(record)
    record.setdefault("record_id", sha256_json(record))
    line = json.dumps(record, sort_keys=True, separators=(",", ":"))
    with open(idx, "a", encoding="utf-8") as f:
        f.write(line + "\n")


def write_record(path: Path, record: Dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(record, sort_keys=True, separators=(",", ":")), encoding="utf-8")
