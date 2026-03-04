from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

import numpy as np
import pandas as pd

from mqk_research.ml.util_hash import file_record, sha256_json
from mqk_research.signal_pack.contracts import SignalPackContractV1


@dataclass(frozen=True)
class BuildSpec:
    # Simple example: create a signal from fwd_return proxy, or any column in features.
    signal_column: str = "ret_1"
    horizon_bars: int = 5

    def normalized(self) -> "BuildSpec":
        return BuildSpec(signal_column=str(self.signal_column), horizon_bars=int(self.horizon_bars))


def build_signal_pack_from_features(
    *,
    features_csv: Path,
    out_signal_pack_csv: Path,
    run_id: str,
    policy_hash: str,
    spec: Optional[BuildSpec] = None,
) -> Path:
    spec = (spec or BuildSpec()).normalized()
    features_csv = Path(features_csv)
    out_signal_pack_csv = Path(out_signal_pack_csv)

    df = pd.read_csv(features_csv)
    for c in ["symbol", "end_ts"]:
        if c not in df.columns:
            raise ValueError(f"features missing {c} (expected columns include symbol,end_ts)")
    if spec.signal_column not in df.columns:
        raise ValueError(f"features missing signal_column={spec.signal_column}")

    out = pd.DataFrame({
        "ts": pd.to_datetime(df["end_ts"], utc=True).dt.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "symbol": df["symbol"].astype(str),
        "signal": df[spec.signal_column].astype(float),
        "horizon_bars": int(spec.horizon_bars),
        "policy_hash": policy_hash,
        "run_id": run_id,
    })

    # deterministic sort
    out = out.sort_values(["symbol","ts"], kind="mergesort").reset_index(drop=True)

    contract = SignalPackContractV1().normalized()
    missing = [c for c in contract.required_columns if c not in out.columns]
    if missing:
        raise RuntimeError(f"internal: signal pack missing {missing}")

    out_signal_pack_csv.parent.mkdir(parents=True, exist_ok=True)
    out.to_csv(out_signal_pack_csv, index=False)

    meta = {
        "schema_version": "signal_pack_meta_v1",
        "spec": {"signal_column": spec.signal_column, "horizon_bars": spec.horizon_bars},
        "inputs": {"features_csv": file_record(features_csv)},
        "outputs": {"signal_pack_csv": file_record(out_signal_pack_csv)},
        "ids": {"signal_pack_id": sha256_json({"inputs": file_record(features_csv), "spec": {"signal_column": spec.signal_column, "horizon_bars": spec.horizon_bars}})},
    }
    (out_signal_pack_csv.parent / "signal_pack_meta.json").write_text(json.dumps(meta, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    return out_signal_pack_csv


def main_build(argv: list[str] | None = None) -> int:
    import argparse
    ap = argparse.ArgumentParser(prog="mqk-signal-pack", description="Build signal_pack.csv from features.csv (scaffold)")
    ap.add_argument("--features", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--run-id", required=True)
    ap.add_argument("--policy-hash", required=True)
    ap.add_argument("--signal-col", default="ret_1")
    ap.add_argument("--horizon", type=int, default=5)
    args = ap.parse_args(argv)

    spec = BuildSpec(signal_column=args.signal_col, horizon_bars=args.horizon)
    out = build_signal_pack_from_features(
        features_csv=Path(args.features),
        out_signal_pack_csv=Path(args.out),
        run_id=args.run_id,
        policy_hash=args.policy_hash,
        spec=spec,
    )
    print(f"OK signal_pack={out}")
    return 0
