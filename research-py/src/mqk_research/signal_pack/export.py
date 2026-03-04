from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict

import pandas as pd

from mqk_research.ml.util_hash import file_record, sha256_json
from mqk_research.registry.index import append_index, write_record


def export_signal_pack(run_dir: Path, *, output_signal_column: str = "signal") -> Path:
    run_dir = Path(run_dir)

    schema_path = run_dir / "feature_schema.json"
    scores_path = run_dir / "ml" / "scores.csv"
    model_path = run_dir / "ml" / "model_logreg_v1.json"
    train_meta_path = run_dir / "ml" / "ml_train_meta.json"
    scores_meta_path = run_dir / "ml" / "scores_meta.json"

    if not scores_path.exists():
        raise FileNotFoundError(f"Missing {scores_path} (run scoring first)")
    if not model_path.exists():
        raise FileNotFoundError(f"Missing {model_path} (run training first)")
    if not schema_path.exists():
        raise FileNotFoundError(f"Missing {schema_path}")
    if not train_meta_path.exists():
        raise FileNotFoundError(f"Missing {train_meta_path}")
    if not scores_meta_path.exists():
        raise FileNotFoundError(f"Missing {scores_meta_path}")

    df = pd.read_csv(scores_path)

    # Default: map ml_score -> signal in [0..1]
    if "ml_score" not in df.columns:
        raise RuntimeError("scores.csv missing ml_score column")
    out = df.copy()

    # Minimal contract for later Rust consumption:
    # symbol, end_ts, signal, confidence
    required = [c for c in ["symbol", "end_ts"] if c in out.columns]
    if "symbol" not in out.columns:
        raise RuntimeError("scores.csv missing symbol column (required for signal pack)")
    if "end_ts" not in out.columns:
        # allow fallback to asof_utc if present
        if "asof_utc" in out.columns:
            out["end_ts"] = out["asof_utc"]
        else:
            raise RuntimeError("scores.csv missing end_ts (or asof_utc) required for signal pack")

    out[output_signal_column] = out["ml_score"].astype(float)
    out["confidence"] = out["ml_score"].astype(float)
    keep_cols = ["symbol", "end_ts", output_signal_column, "confidence"]
    keep_cols = [c for c in keep_cols if c in out.columns]
    signals = out[keep_cols].copy()

    sp_dir = run_dir / "signal_pack"
    sp_dir.mkdir(parents=True, exist_ok=True)

    signals_path = sp_dir / "signals.csv"
    signals.to_csv(signals_path, index=False)

    train_meta = json.loads(train_meta_path.read_text(encoding="utf-8"))
    model = json.loads(model_path.read_text(encoding="utf-8"))
    schema = json.loads(schema_path.read_text(encoding="utf-8"))

    signal_pack = {
        "schema_version": "signal_pack_v1",
        "inputs": {
            "feature_schema": file_record(schema_path),
            "model": file_record(model_path),
            "scores_csv": file_record(scores_path),
        },
        "outputs": {
            "signals_csv": file_record(signals_path),
        },
        "ids": {
            "dataset_id": train_meta["ids"]["dataset_id"],
            "model_id": train_meta["ids"]["model_id"],
            "scores_id": json.loads(scores_meta_path.read_text(encoding="utf-8"))["ids"]["scores_id"],
        },
        "compat": {
            "signal_pack_format": "v1",
            "signal_column": output_signal_column,
        },
        "feature_schema_hash": schema.get("schema_hash"),
        "model_type": model.get("model_type"),
    }

    sp_path = sp_dir / "signal_pack.json"
    sp_path.write_text(json.dumps(signal_pack, sort_keys=True, separators=(",", ":")), encoding="utf-8")

    # register into project-level registry (project_root = research-py)
    project_root = run_dir.parents[1] if (run_dir.parents and len(run_dir.parents) >= 2) else run_dir
    # best-effort: if runs/<id>, then run_dir parent is runs, parent of that is project root.
    append_index(project_root, {"type": "signal_pack", "signal_pack_id": sha256_json(signal_pack), "path": str(sp_dir)})

    return sp_dir


def main_export(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(prog="mqk-signal-pack-export", description="Export a LEAN-like signal pack artifact (scaffold)")
    ap.add_argument("--run-dir", required=True)
    ap.add_argument("--signal-col", default="signal")
    args = ap.parse_args(argv)

    out = export_signal_pack(Path(args.run_dir), output_signal_column=args.signal_col)
    print(f"OK signal_pack_dir={out}")
    return 0
