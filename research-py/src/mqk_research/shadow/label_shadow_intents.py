from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

import numpy as np
import pandas as pd

from mqk_research.ml.util_hash import file_record, sha256_json


@dataclass(frozen=True)
class LabelSpec:
    """
    Label spec for turning shadow intents into targets.

    Label definition (default):
      - fwd_ret = log(close[t+h] / close[t]) where h = horizon_bars
      - target = 1 if fwd_ret > ret_threshold else 0

    Deterministic: no randomness, stable sorts, strict joins.
    """
    ret_threshold: float = 0.005
    default_horizon_bars: int = 5
    require_exact_bar_match: bool = True

    def normalized(self) -> "LabelSpec":
        return LabelSpec(
            ret_threshold=float(self.ret_threshold),
            default_horizon_bars=int(self.default_horizon_bars),
            require_exact_bar_match=bool(self.require_exact_bar_match),
        )


def _to_utc(s: pd.Series) -> pd.Series:
    return pd.to_datetime(s, utc=True)


def label_shadow_intents(
    *,
    shadow_intents_csv: Path,
    bars_csv: Path,
    out_targets_csv: Path,
    spec: Optional[LabelSpec] = None,
) -> Path:
    spec = (spec or LabelSpec()).normalized()
    shadow_intents_csv = Path(shadow_intents_csv)
    bars_csv = Path(bars_csv)
    out_targets_csv = Path(out_targets_csv)

    intents = pd.read_csv(shadow_intents_csv)
    bars = pd.read_csv(bars_csv)

    req_i = ["run_id", "symbol", "decision_ts", "intent"]
    miss_i = [c for c in req_i if c not in intents.columns]
    if miss_i:
        raise ValueError(f"shadow_intents missing required columns: {miss_i}")

    req_b = ["symbol", "end_ts", "close"]
    miss_b = [c for c in req_b if c not in bars.columns]
    if miss_b:
        raise ValueError(f"bars missing required columns: {miss_b}")

    intents = intents.copy()
    bars = bars.copy()

    intents["decision_ts"] = _to_utc(intents["decision_ts"])
    bars["end_ts"] = _to_utc(bars["end_ts"])

    if "horizon_bars" not in intents.columns:
        intents["horizon_bars"] = spec.default_horizon_bars
    intents["horizon_bars"] = intents["horizon_bars"].astype(int)

    intents = intents.sort_values(["symbol", "decision_ts"], kind="mergesort").reset_index(drop=True)
    bars = bars.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)

    out_rows = []

    for sym, bi in bars.groupby("symbol", sort=True):
        bi = bi.reset_index(drop=True)
        if bi.empty:
            continue

        idx_map = {ts: i for i, ts in enumerate(bi["end_ts"].tolist())}
        ii = intents[intents["symbol"] == sym].copy()
        if ii.empty:
            continue

        closes = bi["close"].astype(np.float64).to_numpy()
        ts_list = bi["end_ts"].to_numpy()

        for _, row in ii.iterrows():
            ts = row["decision_ts"]
            h = int(row["horizon_bars"])

            if ts in idx_map:
                j = idx_map[ts]
            else:
                if spec.require_exact_bar_match:
                    continue
                j = int(np.searchsorted(ts_list, ts, side="right") - 1)
                if j < 0:
                    continue

            k = j + h
            if k >= len(closes):
                continue

            c0 = float(closes[j])
            c1 = float(closes[k])
            if c0 <= 0.0 or c1 <= 0.0:
                continue

            fwd_ret = float(np.log(c1 / c0))
            target = 1 if fwd_ret > spec.ret_threshold else 0

            out_rows.append({
                "run_id": row["run_id"],
                "symbol": sym,
                "end_ts": ts.isoformat(),
                "horizon_bars": h,
                "intent": row["intent"],
                "fwd_ret": fwd_ret,
                "target": int(target),
            })

    out = pd.DataFrame(out_rows)
    if out.empty:
        raise RuntimeError("No labeled rows produced (check timestamp alignment, horizons, data span).")

    out = out.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)

    out_targets_csv.parent.mkdir(parents=True, exist_ok=True)
    out.to_csv(out_targets_csv, index=False)

    meta = {
        "schema_version": "shadow_label_meta_v1",
        "spec": {
            "ret_threshold": spec.ret_threshold,
            "default_horizon_bars": spec.default_horizon_bars,
            "require_exact_bar_match": spec.require_exact_bar_match,
        },
        "inputs": {
            "shadow_intents_csv": file_record(shadow_intents_csv),
            "bars_csv": file_record(bars_csv),
        },
        "outputs": {
            "targets_csv": file_record(out_targets_csv),
        },
        "ids": {
            "label_run_id": sha256_json({
                "spec": {
                    "ret_threshold": spec.ret_threshold,
                    "default_horizon_bars": spec.default_horizon_bars,
                    "require_exact_bar_match": spec.require_exact_bar_match,
                },
                "inputs": {
                    "shadow_intents_csv": file_record(shadow_intents_csv),
                    "bars_csv": file_record(bars_csv),
                },
                "outputs": {
                    "targets_csv": file_record(out_targets_csv),
                },
            })
        }
    }
    meta_path = out_targets_csv.parent / "shadow_label_meta.json"
    meta_path.write_text(json.dumps(meta, sort_keys=True, separators=(",", ":")), encoding="utf-8")

    return out_targets_csv


def main_label(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(prog="mqk-shadow-label", description="Label shadow intents into targets.csv (scaffold)")
    ap.add_argument("--shadow-intents", required=True, help="Path to shadow_intents.csv")
    ap.add_argument("--bars-csv", required=True, help="Path to bars.csv containing symbol,end_ts,close (and more ok)")
    ap.add_argument("--out", required=True, help="Output targets.csv path")
    ap.add_argument("--ret-threshold", type=float, default=0.005)
    ap.add_argument("--default-horizon", type=int, default=5)
    ap.add_argument("--allow-asof", action="store_true", help="If set, allow decision_ts to align to nearest prior bar")
    args = ap.parse_args(argv)

    spec = LabelSpec(
        ret_threshold=args.ret_threshold,
        default_horizon_bars=args.default_horizon,
        require_exact_bar_match=(not args.allow_asof),
    )
    out = label_shadow_intents(
        shadow_intents_csv=Path(args.shadow_intents),
        bars_csv=Path(args.bars_csv),
        out_targets_csv=Path(args.out),
        spec=spec,
    )
    print(f"OK targets={out}")
    return 0
