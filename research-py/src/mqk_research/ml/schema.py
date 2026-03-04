from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List

import pandas as pd

from .util_hash import sha256_file, sha256_json, file_record


@dataclass(frozen=True)
class FeatureSchema:
    schema_version: str
    id_columns: List[str]
    feature_columns: List[str]
    dtypes: Dict[str, str]
    features_csv_sha256: str

    def to_dict(self) -> Dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "id_columns": list(self.id_columns),
            "feature_columns": list(self.feature_columns),
            "dtypes": dict(self.dtypes),
            "features_csv_sha256": self.features_csv_sha256,
            "schema_hash": sha256_json({
                "schema_version": self.schema_version,
                "id_columns": list(self.id_columns),
                "feature_columns": list(self.feature_columns),
                "dtypes": dict(self.dtypes),
            }),
        }


def generate_feature_schema(run_dir: Path, id_columns: List[str] | None = None) -> Path:
    run_dir = Path(run_dir)
    id_columns = id_columns or ["symbol", "end_ts"]

    features_path = run_dir / "features.csv"
    if not features_path.exists():
        raise FileNotFoundError(f"Missing {features_path}")

    df = pd.read_csv(features_path, nrows=50)  # dtype sniff, deterministic subset
    cols = list(df.columns)

    for k in id_columns:
        if k not in cols:
            raise ValueError(f"features.csv missing id column '{k}' required for schema")

    feature_cols = [c for c in cols if c not in id_columns]
    feature_cols.sort()

    dtypes = {c: str(df[c].dtype) for c in cols}

    schema = FeatureSchema(
        schema_version="feature_schema_v1",
        id_columns=id_columns,
        feature_columns=feature_cols,
        dtypes=dtypes,
        features_csv_sha256=sha256_file(features_path),
    )

    out_path = run_dir / "feature_schema.json"
    out_path.write_text(json.dumps(schema.to_dict(), sort_keys=True, separators=(",", ":")), encoding="utf-8")
    return out_path


def validate_feature_schema(run_dir: Path, schema_path: Path) -> None:
    run_dir = Path(run_dir)
    schema_path = Path(schema_path)

    features_path = run_dir / "features.csv"
    if not features_path.exists():
        raise FileNotFoundError(f"Missing {features_path}")

    raw = json.loads(schema_path.read_text(encoding="utf-8"))
    expected_cols = list(raw["id_columns"]) + list(raw["feature_columns"])

    df = pd.read_csv(features_path, nrows=1)
    actual_cols = list(df.columns)

    if actual_cols != expected_cols:
        raise RuntimeError(f"Feature schema mismatch. expected={expected_cols[:12]}... actual={actual_cols[:12]}...")

    # hash check (full file)
    if sha256_file(features_path) != raw["features_csv_sha256"]:
        raise RuntimeError("features.csv sha256 mismatch vs schema (file changed)")
