from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pandas as pd

from .schema import validate_feature_schema
from .util_hash import file_record, sha256_json


def _sigmoid(z: np.ndarray) -> np.ndarray:
    z = np.clip(z, -50.0, 50.0)
    return 1.0 / (1.0 + np.exp(-z))


def score_run(run_dir: Path, model_path: Path) -> Path:
    run_dir = Path(run_dir)
    model_path = Path(model_path)

    features_path = run_dir / "features.csv"
    schema_path = run_dir / "feature_schema.json"
    if not features_path.exists():
        raise FileNotFoundError(f"Missing {features_path}")
    if not schema_path.exists():
        raise FileNotFoundError(f"Missing {schema_path} (generate schema first)")

    validate_feature_schema(run_dir, schema_path)

    feats = pd.read_csv(features_path)
    model = json.loads(model_path.read_text(encoding="utf-8"))

    cols = list(model["feature_columns"])
    missing = [c for c in cols if c not in feats.columns]
    if missing:
        raise RuntimeError(f"Fail-closed: features missing model columns: {missing[:10]}")

    X = feats[cols].to_numpy(dtype=np.float64)

    if model.get("mean") is not None and model.get("std") is not None:
        mean = np.asarray(model["mean"], dtype=np.float64)
        std = np.asarray(model["std"], dtype=np.float64)
        std = np.where(std <= 1e-12, 1.0, std)
        X = (X - mean) / std

    coef = np.asarray(model["coef"], dtype=np.float64)
    z = X @ coef + float(model["intercept"])
    p = _sigmoid(z)

    out = feats.copy()
    out["ml_score"] = p.astype(float)

    out_dir = run_dir / "ml"
    out_dir.mkdir(parents=True, exist_ok=True)

    scores_path = out_dir / "scores.csv"
    out.to_csv(scores_path, index=False)

    meta_path = out_dir / "scores_meta.json"
    meta = {
        "schema_version": "ml_scores_meta_v1",
        "inputs": {
            "features_csv": file_record(features_path),
            "feature_schema": file_record(schema_path),
            "model": file_record(model_path),
        },
        "outputs": {
            "scores_csv": file_record(scores_path),
        },
        "ids": {
            "scores_id": sha256_json({"scores": file_record(scores_path), "model": file_record(model_path)}),
        },
    }
    meta_path.write_text(json.dumps(meta, sort_keys=True, separators=(",", ":")), encoding="utf-8")

    return scores_path


def main_score(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(prog="mqk-ml-score", description="Deterministic ML scoring (scaffold)")
    ap.add_argument("--run-dir", required=True)
    ap.add_argument("--model", required=True)
    args = ap.parse_args(argv)

    out = score_run(Path(args.run_dir), Path(args.model))
    print(f"OK scores={out}")
    return 0
