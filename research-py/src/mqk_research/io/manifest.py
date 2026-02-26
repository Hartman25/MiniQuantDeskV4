from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Dict, List

from .hashing import sha256_bytes, sha256_file


@dataclass(frozen=True)
class Manifest:
    schema_version: str
    run_id: str
    asof_utc: str
    policy_name: str
    policy_path: str
    policy_sha256: str
    params: Dict[str, Any]
    inputs: Dict[str, Any]
    outputs: Dict[str, Any]
    notes: List[str] = field(default_factory=list)

    def to_json_bytes(self) -> bytes:
        obj = asdict(self)
        return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")

    def write(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(self.to_json_bytes())

    def sha256(self) -> str:
        return sha256_bytes(self.to_json_bytes())


def stable_run_id(policy_name: str, asof_utc: str, params: Dict[str, Any]) -> str:
    blob = json.dumps(
        {"policy_name": policy_name, "asof_utc": asof_utc, "params": params},
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")
    return sha256_bytes(blob)[:20]


def file_record(path: Path) -> Dict[str, Any]:
    return {
        "path": str(path),
        "sha256": sha256_file(path),
        "bytes": path.stat().st_size,
    }