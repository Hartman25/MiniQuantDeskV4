from __future__ import annotations

from typing import Dict

import pandas as pd


def build_targets_long_only_equal_weight(universe: pd.DataFrame, policy: Dict) -> pd.DataFrame:
    """Portfolio builder: long-only, equal weight top N.

    Input universe expects at least:
      included, rank, instrument_id, symbol, asset_class

    Output targets schema:
      instrument_id, symbol, asset_class, side, weight
    """
    port = policy["portfolio"]
    top_n = int(port["top_n"])
    max_pos = int(port["max_positions"])

    if top_n <= 0:
        raise ValueError("portfolio.top_n must be > 0")
    if max_pos <= 0:
        raise ValueError("portfolio.max_positions must be > 0")
    if top_n > max_pos:
        top_n = max_pos

    df = universe.copy()
    df = df[df["included"] == True].copy()
    df = df.sort_values(["rank", "symbol"], kind="mergesort").head(top_n).reset_index(drop=True)

    if df.empty:
        raise RuntimeError("Universe produced zero included instruments; cannot build targets")

    w = 1.0 / float(len(df))
    out = pd.DataFrame(
        {
            "instrument_id": df["instrument_id"].astype(str),
            "symbol": df["symbol"].astype(str),
            "asset_class": df["asset_class"].astype(str),
            "side": "LONG",
            "weight": w,
        }
    )

    out = out.sort_values(["symbol"], kind="mergesort").reset_index(drop=True)
    return out