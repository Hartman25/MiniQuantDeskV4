from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional, Tuple

import numpy as np
import pandas as pd


# -----------------------------
# Feature Set v1 (compact, high ROI, low leakage risk)
# Input bars contract:
# columns: symbol, end_ts, open, high, low, close, volume
# end_ts must be parseable to UTC timestamps (string ok).
# -----------------------------


@dataclass(frozen=True)
class FeatureSetV1Spec:
    # return horizons (bars) for log-returns
    ret_windows: Tuple[int, ...] = (1, 2, 5, 10, 20)

    # volatility windows (bars) for rolling std of returns
    vol_windows: Tuple[int, ...] = (10, 20, 60)

    # extrema windows
    extrema_windows: Tuple[int, ...] = (20, 60)

    # EMA windows
    ema_fast: int = 10
    ema_slow: int = 30

    # regression trend window
    trend_window: int = 20

    # ATR window (classic 14)
    atr_window: int = 14

    # cross-sectional ranks (computed per end_ts across universe)
    cross_section_windows: Tuple[int, ...] = (5, 20)

    # calendar flags
    add_calendar: bool = True

    # fail-closed minimum rows per symbol to compute rolling features
    min_rows_per_symbol: int = 80


def _to_utc_ts(s: pd.Series) -> pd.Series:
    return pd.to_datetime(s, utc=True)


def _log_return(close: pd.Series, n: int) -> pd.Series:
    return np.log(close / close.shift(n))


def _rolling_vol(ret: pd.Series, n: int) -> pd.Series:
    return ret.rolling(n, min_periods=n).std()


def _ema(x: pd.Series, span: int) -> pd.Series:
    # deterministic pandas ewm
    return x.ewm(span=span, adjust=False, min_periods=span).mean()


def _true_range(df_sym: pd.DataFrame) -> pd.Series:
    prev_close = df_sym["close"].shift(1)
    hl = df_sym["high"] - df_sym["low"]
    hc = (df_sym["high"] - prev_close).abs()
    lc = (df_sym["low"] - prev_close).abs()
    return pd.concat([hl, hc, lc], axis=1).max(axis=1)


def _atr(df_sym: pd.DataFrame, n: int) -> pd.Series:
    tr = _true_range(df_sym)
    return tr.rolling(n, min_periods=n).mean()


def _linreg_slope_r2(log_price: np.ndarray) -> Tuple[float, float]:
    # simple deterministic OLS of y ~ x
    y = log_price.astype(np.float64)
    n = len(y)
    if n < 2:
        return float("nan"), float("nan")
    x = np.arange(n, dtype=np.float64)
    x_mean = x.mean()
    y_mean = y.mean()
    ss_x = np.sum((x - x_mean) ** 2)
    if ss_x <= 0.0:
        return float("nan"), float("nan")
    cov_xy = np.sum((x - x_mean) * (y - y_mean))
    slope = cov_xy / ss_x
    y_hat = y_mean + slope * (x - x_mean)
    ss_tot = np.sum((y - y_mean) ** 2)
    ss_res = np.sum((y - y_hat) ** 2)
    r2 = float("nan") if ss_tot <= 0.0 else 1.0 - (ss_res / ss_tot)
    return float(slope), float(r2)


def _trend_features(df_sym: pd.DataFrame, window: int) -> pd.DataFrame:
    out = pd.DataFrame(index=df_sym.index)
    lp = np.log(df_sym["close"].astype(np.float64))
    slopes = np.full(len(df_sym), np.nan, dtype=np.float64)
    r2s = np.full(len(df_sym), np.nan, dtype=np.float64)
    for i in range(window - 1, len(df_sym)):
        seg = lp.iloc[i - window + 1 : i + 1].to_numpy()
        slope, r2 = _linreg_slope_r2(seg)
        slopes[i] = slope
        r2s[i] = r2
    out[f"slope_{window}"] = slopes
    out[f"r2_{window}"] = r2s
    return out


def build_feature_set_v1(bars: pd.DataFrame, spec: FeatureSetV1Spec | None = None) -> pd.DataFrame:
    spec = spec or FeatureSetV1Spec()

    required = ["symbol", "end_ts", "open", "high", "low", "close", "volume"]
    missing = [c for c in required if c not in bars.columns]
    if missing:
        raise ValueError(f"bars missing required columns: {missing}")

    df = bars.copy()
    df["end_ts"] = _to_utc_ts(df["end_ts"])
    df = df.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)

    # per-symbol features
    feats_all = []
    for sym, g in df.groupby("symbol", sort=True):
        g = g.reset_index(drop=True)
        if len(g) < spec.min_rows_per_symbol:
            # fail-closed: skip short history symbols deterministically
            continue

        out = pd.DataFrame({
            "symbol": g["symbol"],
            "end_ts": g["end_ts"].astype(str),
        })

        close = g["close"].astype(np.float64)
        open_ = g["open"].astype(np.float64)
        high = g["high"].astype(np.float64)
        low = g["low"].astype(np.float64)
        vol = g["volume"].astype(np.float64)

        # log returns
        for n in spec.ret_windows:
            out[f"ret_{n}"] = _log_return(close, n)

        # rolling vol on 1-bar returns
        r1 = _log_return(close, 1)
        for n in spec.vol_windows:
            out[f"vol_{n}"] = _rolling_vol(r1, n)

        # z-returns (return normalized by vol)
        for n in (5, 20):
            if f"ret_{n}" in out.columns and f"vol_{n}" in out.columns:
                out[f"zret_{n}"] = out[f"ret_{n}"] / out[f"vol_{n}"].replace(0.0, np.nan)

        # distance from highs/lows
        for n in spec.extrema_windows:
            hh = close.rolling(n, min_periods=n).max()
            ll = close.rolling(n, min_periods=n).min()
            out[f"hh_dist_{n}"] = (close / hh) - 1.0
            out[f"ll_dist_{n}"] = (close / ll) - 1.0

        # EMA spread normalized
        ema_f = _ema(close, spec.ema_fast)
        ema_s = _ema(close, spec.ema_slow)
        out["ema_fast_slow"] = (ema_f - ema_s) / close.replace(0.0, np.nan)

        # trend slope/r2 on log-price
        out = pd.concat([out, _trend_features(g.assign(close=close), spec.trend_window)], axis=1)

        # ATR features
        atr = _atr(g.assign(open=open_, high=high, low=low, close=close), spec.atr_window)
        out[f"atr_{spec.atr_window}"] = atr
        out[f"atr_pct_{spec.atr_window}"] = atr / close.replace(0.0, np.nan)

        # gaps and ranges
        prev_close = close.shift(1)
        out["gap_pct_1"] = (open_ / prev_close) - 1.0
        out["range_pct"] = (high - low) / close.replace(0.0, np.nan)

        # dollar volume + amihud illiquidity
        dolvol = close * vol
        out["dolvol_20"] = dolvol.rolling(20, min_periods=20).mean()
        out["vol_ratio"] = vol / vol.rolling(20, min_periods=20).mean()
        out["illiquidity_amihud"] = r1.abs() / dolvol.replace(0.0, np.nan)

        # stale bar proxy flags (data quality)
        out["stale_bar_flag"] = ((vol <= 0) | ((high == low) & (low == close) & (close == open_))).astype(int)

        feats_all.append(out)

    if not feats_all:
        raise RuntimeError("No symbols had enough rows to compute Feature Set v1 (min_rows_per_symbol gate)")

    feats = pd.concat(feats_all, axis=0, ignore_index=True)

    # cross-sectional ranks per timestamp (LEAN-style context)
    # Note: end_ts is string in out to preserve exact serialization; parse for grouping.
    feats["_end_ts_dt"] = pd.to_datetime(feats["end_ts"], utc=True)

    for w in spec.cross_section_windows:
        col = f"ret_{w}"
        if col in feats.columns:
            feats[f"ret_rank_{w}"] = feats.groupby("_end_ts_dt")[col].rank(pct=True, method="average")

    if f"vol_{20}" in feats.columns:
        feats["vol_rank_20"] = feats.groupby("_end_ts_dt")["vol_20"].rank(pct=True, method="average")
    if f"atr_pct_{spec.atr_window}" in feats.columns:
        feats["atr_rank_14"] = feats.groupby("_end_ts_dt")[f"atr_pct_{spec.atr_window}"].rank(pct=True, method="average")

    # composite momentum score (simple rank blend)
    if "ret_rank_20" in feats.columns and f"slope_{spec.trend_window}" in feats.columns:
        # slope rank per day
        feats["slope_rank_20"] = feats.groupby("_end_ts_dt")[f"slope_{spec.trend_window}"].rank(pct=True, method="average")
        feats["momentum_score"] = 0.5 * feats["ret_rank_20"] + 0.5 * feats["slope_rank_20"]

    # calendar
    if spec.add_calendar:
        dt = feats["_end_ts_dt"]
        feats["dow"] = dt.dt.dayofweek.astype(int)
        feats["month"] = dt.dt.month.astype(int)
        # month end flag
        next_day = dt + pd.Timedelta(days=1)
        feats["is_month_end"] = (dt.dt.month != next_day.dt.month).astype(int)

    feats = feats.drop(columns=["_end_ts_dt"], errors="ignore")

    # deterministic column order: ids first, then sorted rest
    id_cols = ["symbol", "end_ts"]
    other = [c for c in feats.columns if c not in id_cols]
    other.sort()
    feats = feats[id_cols + other]

    return feats


def write_features_csv_from_bars_csv(bars_csv: Path, out_features_csv: Path, *, spec: FeatureSetV1Spec | None = None) -> None:
    bars = pd.read_csv(bars_csv)
    feats = build_feature_set_v1(bars, spec=spec)
    out_features_csv.parent.mkdir(parents=True, exist_ok=True)
    feats.to_csv(out_features_csv, index=False)


def main_features_v1(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(prog="mqk-features-v1", description="Generate Feature Set v1 from bars CSV (scaffold)")
    ap.add_argument("--bars-csv", required=True, help="CSV with columns symbol,end_ts,open,high,low,close,volume")
    ap.add_argument("--out", required=True, help="Output features.csv path")
    args = ap.parse_args(argv)

    write_features_csv_from_bars_csv(Path(args.bars_csv), Path(args.out))
    print(f"OK wrote {args.out}")
    return 0
