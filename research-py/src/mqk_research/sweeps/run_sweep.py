from __future__ import annotations

import itertools
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional, Any

import pandas as pd

from mqk_research.ml.util_hash import sha256_json


def _grid_to_rows(grid: Dict[str, List[Any]]) -> List[Dict[str, Any]]:
    keys = list(grid.keys())
    vals = [grid[k] for k in keys]
    rows = []
    for combo in itertools.product(*vals):
        rows.append({k: v for k, v in zip(keys, combo)})
    return rows


def run_sweep_scaffold(*, grid_json: Path, out_csv: Path) -> Path:
    """Scaffold: expands a parameter grid and writes sweep_plan.csv. No execution."""
    grid = json.loads(Path(grid_json).read_text(encoding="utf-8"))
    rows = _grid_to_rows(grid.get("grid", {}))
    if not rows:
        raise ValueError("grid_json must include {"grid": {param: [values...]}}")
    df = pd.DataFrame(rows)
    df.insert(0, "sweep_id", [sha256_json(r) for r in rows])
    out_csv.parent.mkdir(parents=True, exist_ok=True)
    df.to_csv(out_csv, index=False)
    return out_csv


def main_sweep(argv: list[str] | None = None) -> int:
    import argparse
    ap = argparse.ArgumentParser(prog="mqk-sweep", description="Create a sweep plan from a param grid (scaffold)")
    ap.add_argument("--grid", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args(argv)
    out = run_sweep_scaffold(grid_json=Path(args.grid), out_csv=Path(args.out))
    print(f"OK sweep_plan={out}")
    return 0
