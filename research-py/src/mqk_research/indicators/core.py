from __future__ import annotations

import numpy as np
import pandas as pd


def sma(x: pd.Series, n: int) -> pd.Series:
    return x.rolling(n, min_periods=n).mean()


def ema(x: pd.Series, n: int) -> pd.Series:
    return x.ewm(span=n, adjust=False, min_periods=n).mean()


def rsi(close: pd.Series, n: int = 14) -> pd.Series:
    delta = close.diff()
    up = delta.clip(lower=0.0)
    down = (-delta).clip(lower=0.0)
    rs = up.rolling(n, min_periods=n).mean() / down.rolling(n, min_periods=n).mean()
    return 100.0 - (100.0 / (1.0 + rs))


def atr(high: pd.Series, low: pd.Series, close: pd.Series, n: int = 14) -> pd.Series:
    prev_close = close.shift(1)
    tr = pd.concat([(high-low).abs(), (high-prev_close).abs(), (low-prev_close).abs()], axis=1).max(axis=1)
    return tr.rolling(n, min_periods=n).mean()


def rolling_vol(ret: pd.Series, n: int = 20) -> pd.Series:
    return ret.rolling(n, min_periods=n).std(ddof=1)


def zscore(x: pd.Series, n: int = 20) -> pd.Series:
    mu = x.rolling(n, min_periods=n).mean()
    sd = x.rolling(n, min_periods=n).std(ddof=1)
    return (x - mu) / sd
