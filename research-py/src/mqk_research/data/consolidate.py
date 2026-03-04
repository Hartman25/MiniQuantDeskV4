from __future__ import annotations

import pandas as pd
from pathlib import Path
from typing import Literal, Optional

import numpy as np

from mqk_research.ml.util_hash import file_record, sha256_json
import json


Timeframe = Literal["1min","5min","15min","1h","1day"]


def _to_utc(s: pd.Series) -> pd.Series:
    return pd.to_datetime(s, utc=True)


def consolidate_bars_csv(
    *,
    bars_csv: Path,
    out_csv: Path,
    timeframe: Timeframe,
) -> Path:
    """Deterministic OHLCV consolidation by symbol."""
    bars_csv = Path(bars_csv)
    out_csv = Path(out_csv)

    df = pd.read_csv(bars_csv)
    for c in ["symbol","end_ts","open","high","low","close","volume"]:
        if c not in df.columns:
            raise ValueError(f"bars missing required column {c}")

    df["end_ts"] = _to_utc(df["end_ts"])
    df = df.sort_values(["symbol","end_ts"], kind="mergesort").reset_index(drop=True)

    rule = {
        "1min": "1min",
        "5min": "5min",
        "15min": "15min",
        "1h": "1h",
        "1day": "1D",
    }[timeframe]

    outs = []
    for sym, g in df.groupby("symbol", sort=True):
        g = g.set_index("end_ts")
        agg = pd.DataFrame({
            "open": g["open"].resample(rule, label="right", closed="right").first(),
            "high": g["high"].resample(rule, label="right", closed="right").max(),
            "low":  g["low"].resample(rule, label="right", closed="right").min(),
            "close":g["close"].resample(rule, label="right", closed="right").last(),
            "volume":g["volume"].resample(rule, label="right", closed="right").sum(),
        }).dropna()
        agg = agg.reset_index()
        agg.insert(0, "symbol", sym)
        outs.append(agg)

    out = pd.concat(outs, ignore_index=True) if outs else pd.DataFrame(columns=["symbol","end_ts","open","high","low","close","volume"])
    out = out.sort_values(["symbol","end_ts"], kind="mergesort").reset_index(drop=True)

    out_csv.parent.mkdir(parents=True, exist_ok=True)
    out.to_csv(out_csv, index=False)

    meta = {
        "schema_version": "consolidate_meta_v1",
        "timeframe": timeframe,
        "inputs": {"bars_csv": file_record(bars_csv)},
        "outputs": {"out_csv": file_record(out_csv)},
        "ids": {"consolidate_id": sha256_json({"timeframe": timeframe, "bars": file_record(bars_csv)})},
    }
    (out_csv.parent / "consolidate_meta.json").write_text(json.dumps(meta, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    return out_csv


def main_consolidate(argv: list[str] | None = None) -> int:
    import argparse
    ap = argparse.ArgumentParser(prog="mqk-consolidate", description="Consolidate bars.csv to a timeframe (scaffold)")
    ap.add_argument("--bars", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--timeframe", required=True, choices=["1min","5min","15min","1h","1day"])
    args = ap.parse_args(argv)
    out = consolidate_bars_csv(bars_csv=Path(args.bars), out_csv=Path(args.out), timeframe=args.timeframe)  # type: ignore[arg-type]
    print(f"OK consolidated={out}")
    return 0
