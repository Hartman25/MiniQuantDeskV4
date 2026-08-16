from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path


def _discover_repo_root() -> Path:
    start = Path(__file__).resolve()
    for parent in start.parents:
        if (parent / ".git").exists() or (parent / "core-rs").is_dir():
            return parent
    raise RuntimeError("could not discover MiniQuantDesk repository root; set MQK_MCP_REPO_ROOT")


@dataclass(frozen=True)
class Settings:
    repo_root: Path
    max_file_bytes: int = 1_000_000
    max_output_chars: int = 200_000
    max_search_results: int = 200

    @classmethod
    def from_env(cls) -> "Settings":
        raw_root = os.getenv("MQK_MCP_REPO_ROOT")
        root = Path(raw_root).expanduser() if raw_root else _discover_repo_root()
        root = root.resolve()
        if not root.is_dir():
            raise RuntimeError(f"MQK_MCP_REPO_ROOT is not a directory: {root}")
        return cls(repo_root=root)
