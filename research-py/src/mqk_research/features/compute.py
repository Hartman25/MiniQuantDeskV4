from __future__ import annotations

from dataclasses import dataclass
from typing import Tuple

import pandas as pd


@dataclass(frozen=True)
class FeatureConfig:
    atr_window: int = 20
    adv_window: int = 20
    ret_windows: Tuple[int, ...] = (1, 5, 20, 60)
    ma_fast: int = 20
    ma_slow: int = 50


def _true_range(high: pd.Series, low: pd.Series, prev_close: pd.Series) -> pd.Series:
    a = (high - low).abs()
    b = (high - prev_close).abs()
    c = (low - prev_close).abs()
    return pd.concat([a, b, c], axis=1).max(axis=1)


def compute_daily_features(bars: pd.DataFrame, cfg: FeatureConfig) -> pd.DataFrame:
    """Compute reusable daily features (equities, 1D bars).

    Input bars columns:
      symbol, ts_utc, open, high, low, close, volume
    Output rows remain at bar granularity but include feature columns.
    Determinism:
      - explicit sorts
      - fixed rolling windows
      - no randomness
      - no implicit time
    """
    required = {"symbol", "ts_utc", "open", "high", "low", "close", "volume"}
    missing = required - set(bars.columns)
    if missing:
        raise ValueError(f"bars missing required columns: {sorted(missing)}")

    df = bars.copy()
    df["symbol"] = df["symbol"].astype(str).str.upper()
    df["ts_utc"] = pd.to_datetime(df["ts_utc"], utc=True)
    df = df.sort_values(["symbol", "ts_utc"], kind="mergesort").reset_index(drop=True)

    out_parts = []
    for sym, g in df.groupby("symbol", sort=True):
        g = g.sort_values("ts_utc", kind="mergesort").reset_index(drop=True)

        close = g["close"].astype(float)
        high = g["high"].astype(float)
        low = g["low"].astype(float)
        volume = g["volume"].astype(float)

        for w in cfg.ret_windows:
            g[f"ret_{w}d"] = close.pct_change(w)

        prev_close = close.shift(1)
        tr = _true_range(high, low, prev_close)
        atr = tr.rolling(cfg.atr_window, min_periods=cfg.atr_window).mean()
        g[f"atr_pct_{cfg.atr_window}"] = atr / close

        dollar_vol = close * volume
        g[f"adv_usd_{cfg.adv_window}"] = dollar_vol.rolling(cfg.adv_window, min_periods=cfg.adv_window).mean()

        ma_fast = close.rolling(cfg.ma_fast, min_periods=cfg.ma_fast).mean()
        ma_slow = close.rolling(cfg.ma_slow, min_periods=cfg.ma_slow).mean()
        g[f"ma_{cfg.ma_fast}"] = ma_fast
        g[f"ma_{cfg.ma_slow}"] = ma_slow
        g["trend_proxy"] = (ma_fast / ma_slow) - 1.0

        out_parts.append(g)

    out = pd.concat(out_parts, axis=0, ignore_index=True)

    # Keep only rows where core windows exist (prevents silent NaNs).
    core_cols = [
        "ret_1d",
        "ret_5d",
        "ret_20d",
        "ret_60d",
        f"atr_pct_{cfg.atr_window}",
        f"adv_usd_{cfg.adv_window}",
        "trend_proxy",
    ]
    out = out.dropna(subset=core_cols).reset_index(drop=True)

    # Normalize canonical names required downstream in Phase 1.
    out = out.rename(
        columns={
            f"atr_pct_{cfg.atr_window}": "atr_pct_20",
            f"adv_usd_{cfg.adv_window}": "adv_usd_20",
        }
    )

    return out