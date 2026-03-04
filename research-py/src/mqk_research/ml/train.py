from __future__ import annotations

import json
from dataclasses import asdict
from pathlib import Path

import numpy as np
import pandas as pd

from .contracts import MLTrainConfig
from .model_logreg import fit_logreg_deterministic
from .schema import generate_feature_schema, validate_feature_schema
from .util_hash import file_record, sha256_json


def _stable_sort(df: pd.DataFrame) -> pd.DataFrame:
    sort_cols = [c for c in ["symbol", "end_ts"] if c in df.columns]
    if sort_cols:
        return df.sort_values(sort_cols, ascending=True, kind="mergesort").reset_index(drop=True)
    return df.reset_index(drop=True)


def _select_feature_columns(df: pd.DataFrame, cfg: MLTrainConfig) -> list[str]:
    excluded = set(cfg.id_columns + [cfg.label_column])
    cols = []
    for c in df.columns:
        if c in excluded:
            continue
        if any(c.startswith(pfx) for pfx in cfg.feature_exclude_prefixes):
            continue
        if pd.api.types.is_numeric_dtype(df[c]):
            cols.append(c)
    cols.sort()
    return cols


def train_model(run_dir: Path, cfg: MLTrainConfig) -> Path:
    cfg = cfg.normalized()
    run_dir = Path(run_dir)

    features_path = run_dir / "features.csv"
    targets_path = run_dir / "targets.csv"
    if not features_path.exists():
        raise FileNotFoundError(f"Missing {features_path}")
    if not targets_path.exists():
        raise FileNotFoundError(f"Missing {targets_path}")

    # create schema if absent
    schema_path = run_dir / "feature_schema.json"
    if not schema_path.exists():
        generate_feature_schema(run_dir, id_columns=cfg.id_columns)
    validate_feature_schema(run_dir, schema_path)

    feats = pd.read_csv(features_path)
    targs = pd.read_csv(targets_path)

    for k in cfg.id_columns:
        if k not in feats.columns or k not in targs.columns:
            raise ValueError(f"Missing join key '{k}' in features.csv or targets.csv")
    if cfg.label_column not in targs.columns:
        raise ValueError(f"targets.csv missing label column '{cfg.label_column}'")

    df = feats.merge(targs[cfg.id_columns + [cfg.label_column]], on=cfg.id_columns, how="inner", validate="one_to_one")
    df = _stable_sort(df)

    if len(df) < cfg.min_rows:
        raise RuntimeError(f"Fail-closed: too few rows ({len(df)} < {cfg.min_rows})")

    feature_cols = _select_feature_columns(df, cfg)
    if not feature_cols:
        raise RuntimeError("Fail-closed: no numeric features selected")

    X = df[feature_cols].to_numpy(dtype=np.float64)
    y = (df[cfg.label_column].astype(float) > 0.0).astype(int).to_numpy(dtype=np.float64)

    model = fit_logreg_deterministic(
        X, y,
        feature_columns=feature_cols,
        l2=cfg.l2,
        lr=cfg.lr,
        steps=cfg.steps,
        fit_intercept=cfg.fit_intercept,
        standardize=cfg.standardize,
        clip_z=cfg.clip_z,
    )

    out_dir = run_dir / "ml"
    out_dir.mkdir(parents=True, exist_ok=True)

    model_path = out_dir / "model_logreg_v1.json"
    artifact = {
        "model_type": "logreg_v1",
        "feature_columns": feature_cols,
        "coef": model.coef.astype(float).tolist(),
        "intercept": float(model.intercept),
        "mean": None if model.mean is None else model.mean.astype(float).tolist(),
        "std": None if model.std is None else model.std.astype(float).tolist(),
        "train_rows": int(len(df)),
        "train_pos_rate": float(np.mean(y)),
        "metrics": {},  # keep minimal; add eval module later
        "feature_schema_sha256": json.loads(schema_path.read_text(encoding="utf-8"))["schema_hash"],
    }
    model_path.write_text(json.dumps(artifact, sort_keys=True, separators=(",", ":")), encoding="utf-8")

    meta_path = out_dir / "ml_train_meta.json"
    meta = {
        "schema_version": "ml_train_meta_v1",
        "inputs": {
            "features_csv": file_record(features_path),
            "targets_csv": file_record(targets_path),
            "feature_schema": file_record(schema_path),
        },
        "config": asdict(cfg),
        "outputs": {
            "model": file_record(model_path),
        },
        "ids": {
            "dataset_id": sha256_json({"features": file_record(features_path), "targets": file_record(targets_path), "schema": file_record(schema_path)}),
            "model_id": sha256_json({"model": file_record(model_path), "schema_hash": artifact["feature_schema_sha256"]}),
        },
    }
    meta_path.write_text(json.dumps(meta, sort_keys=True, separators=(",", ":")), encoding="utf-8")

    return model_path


def main_train(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(prog="mqk-ml-train", description="Deterministic ML training (scaffold)")
    ap.add_argument("--run-dir", required=True)
    ap.add_argument("--label", default="target")
    ap.add_argument("--steps", type=int, default=500)
    ap.add_argument("--lr", type=float, default=0.05)
    ap.add_argument("--l2", type=float, default=1e-3)
    ap.add_argument("--min-rows", type=int, default=200)
    args = ap.parse_args(argv)

    cfg = MLTrainConfig(label_column=args.label, steps=args.steps, lr=args.lr, l2=args.l2, min_rows=args.min_rows)
    model_path = train_model(Path(args.run_dir), cfg)
    print(f"OK model={model_path}")
    return 0
