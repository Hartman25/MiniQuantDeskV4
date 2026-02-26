from __future__ import annotations

from dataclasses import dataclass
from typing import Dict, Optional

import pandas as pd


@dataclass(frozen=True)
class UniverseResult:
    df: pd.DataFrame
    stubbed_earnings: bool


def build_universe_swing_v1(
    features: pd.DataFrame,
    policy: Dict,
    earnings_flags: Optional[pd.DataFrame],
) -> UniverseResult:
    """Build swing_v1 universe (ranked + filtered).

    features must include:
      symbol, ts_utc, close, adv_usd_20, atr_pct_20, ret_60d, trend_proxy

    earnings_flags (optional) schema:
      symbol, earnings_within_14d (bool)
    """
    p_filters = policy["filters"]
    rank_cfg = policy["rank"]

    req = {"symbol", "ts_utc", "close", "adv_usd_20", "atr_pct_20", "ret_60d", "trend_proxy"}
    missing = req - set(features.columns)
    if missing:
        raise ValueError(f"features missing required columns for universe: {sorted(missing)}")

    df = features.copy()
    df["symbol"] = df["symbol"].astype(str).str.upper()
    df["ts_utc"] = pd.to_datetime(df["ts_utc"], utc=True)
    df = df.sort_values(["symbol", "ts_utc"], kind="mergesort")

    # ASOF per symbol = last row in window.
    asof = df.groupby("symbol", sort=True).tail(1).reset_index(drop=True)

    # Earnings exclusion (stub allowed if missing).
    stubbed = False
    if earnings_flags is None:
        asof["earnings_within_14d"] = False
        stubbed = True
    else:
        ef = earnings_flags.copy()
        ef["symbol"] = ef["symbol"].astype(str).str.upper()
        ef = ef[["symbol", "earnings_within_14d"]].drop_duplicates(subset=["symbol"]).reset_index(drop=True)
        asof = asof.merge(ef, on="symbol", how="left")
        asof["earnings_within_14d"] = asof["earnings_within_14d"].fillna(False).astype(bool)

    # Deterministic filters.
    min_price = float(p_filters["min_price"])
    min_adv = float(p_filters["min_adv_usd_20"])

    included = pd.Series(True, index=asof.index)
    included &= asof["close"].astype(float) > min_price
    included &= asof["adv_usd_20"].astype(float) > min_adv
    included &= ~asof["earnings_within_14d"].astype(bool)
    asof["included"] = included

    # Score (Phase 1 fixed formula; policy carries string for audit only).
    asof["score"] = asof["ret_60d"].astype(float) + asof["trend_proxy"].astype(float)

    ranked = asof[asof["included"]].copy()
    ranked = ranked.sort_values(["score", "symbol"], ascending=[False, True], kind="mergesort").reset_index(drop=True)

    top_k = int(rank_cfg["top_k"])
    ranked = ranked.head(top_k).copy()
    ranked["rank"] = range(1, len(ranked) + 1)

    out = ranked[
        [
            "symbol",
            "rank",
            "included",
            "adv_usd_20",
            "atr_pct_20",
            "ret_60d",
            "trend_proxy",
            "earnings_within_14d",
            "score",
        ]
    ].copy()

    # Phase 1: synthetic instrument_id for equities.
    out["instrument_id"] = out["symbol"].map(lambda s: f"EQUITY::{s}")
    out["asset_class"] = policy.get("asset_class", "EQUITY")

    out = out[
        [
            "instrument_id",
            "symbol",
            "asset_class",
            "rank",
            "included",
            "adv_usd_20",
            "atr_pct_20",
            "ret_60d",
            "trend_proxy",
            "earnings_within_14d",
            "score",
        ]
    ].reset_index(drop=True)

    return UniverseResult(df=out, stubbed_earnings=stubbed)