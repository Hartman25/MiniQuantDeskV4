from __future__ import annotations

import json
import shutil
from pathlib import Path
from typing import Any, Dict

from mqk_research.ml.util_hash import sha256_json, file_record
from mqk_research.registry.index import append_index
from mqk_research.signal_pack.gates import check_eval_gates


def promote_signal_pack(run_dir: Path, *, min_rows: int = 1, require_eval: bool = False, min_folds_used: int = 2, min_auc_mean: float = 0.52) -> Path:
    run_dir = Path(run_dir)
    sp_dir = run_dir / "signal_pack"
    sp_json = sp_dir / "signal_pack.json"
    signals_csv = sp_dir / "signals.csv"

    if not sp_json.exists() or not signals_csv.exists():
        raise FileNotFoundError("signal_pack missing; run mqk-signal-pack-export first")

    sp = json.loads(sp_json.read_text(encoding="utf-8"))
    sp_id = sha256_json(sp)

    if require_eval:
        ok, msg = check_eval_gates(run_dir, min_folds_used=min_folds_used, min_auc_mean=min_auc_mean)
        if not ok:
            raise RuntimeError(msg)

    # simple gate: require at least N signal rows
    import pandas as pd
    df = pd.read_csv(signals_csv)
    if len(df) < min_rows:
        raise RuntimeError(f"Fail-closed: too few signal rows ({len(df)} < {min_rows})")

    # promoted location: research-py/promoted/signal_packs/<id>/
    project_root = run_dir.parents[1] if len(run_dir.parents) >= 2 else run_dir
    dest = Path(project_root) / "promoted" / "signal_packs" / sp_id
    dest.mkdir(parents=True, exist_ok=True)

    # copy entire folder deterministically
    for name in ["signals.csv", "signal_pack.json"]:
        shutil.copy2(sp_dir / name, dest / name)

    # append index record
    append_index(project_root, {"type": "signal_pack_promoted", "signal_pack_id": sp_id, "path": str(dest)})

    return dest


def main_promote(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(prog="mqk-signal-pack-promote", description="Promote a signal pack (scaffold)")
    ap.add_argument("--run-dir", required=True)
    ap.add_argument("--min-rows", type=int, default=1)
    ap.add_argument("--require-eval", action="store_true")
    ap.add_argument("--min-folds-used", type=int, default=2)
    ap.add_argument("--min-auc-mean", type=float, default=0.52)
    args = ap.parse_args(argv)

    out = promote_signal_pack(Path(args.run_dir), min_rows=args.min_rows, require_eval=args.require_eval, min_folds_used=args.min_folds_used, min_auc_mean=args.min_auc_mean)
    print(f"OK promoted_dir={out}")
    return 0
