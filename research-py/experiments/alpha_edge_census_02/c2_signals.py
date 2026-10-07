"""Causal signed-position builders for Census-02. d[t] in {-1,0,+1} is decided AFTER completed bar t and filled strictly
after t by c2_simulate. Short mirrors reuse the accepted Census-01 helpers; no builder reads a bar after t."""

from __future__ import annotations

import sys
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import pandas as pd

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import c2_protocol as pr  # noqa: E402,F401  (sys.path for Census-01 + src)
import signals as s1  # noqa: E402  (Census-01 builders/helpers, read-only reuse)
from mqk_research.indicators.core import rsi as _rsi  # noqa: E402
from mqk_research.indicators.core import zscore as _zscore  # noqa: E402

SymbolData = s1.SymbolData
NAN = np.nan


@dataclass(frozen=True)
class SigS:
    d: np.ndarray      # int8[n] desired position after completed bar t
    cond: np.ndarray   # bool[n] non-executable condition at bar t (label/diagnostic use only)
    s: int


def build_symbol_data(symbol: str, bars: pd.DataFrame) -> SymbolData:
    """The only Census-02 entry from bars: the discovery fence runs BEFORE any array is built."""
    return SymbolData(symbol, pr.fence_bars(bars, what=f"{symbol} census-02 bars"))


def trend_below(sd, trend):
    """(ok, valid) for 'none' | 'below_sma200' (close below its SMA200 at the same completed bar)."""
    if trend == "none":
        t = np.ones(sd.n, bool)
        return t, t
    if trend != "below_sma200":
        raise ValueError(f"unsupported trend filter {trend!r}")
    m = s1.sma(sd, 200)
    return sd.c < m, np.isfinite(m)


def _short(sig) -> SigS | None:
    if sig is None:
        return None
    return SigS(-sig.d.astype(np.int8), sig.cond, sig.s)


def sh01(sd, p):
    lr = s1.logret(sd, p["lookback"])
    return _short(s1._state_sig(sd, lr < 0, np.isfinite(lr), p["cadence"]))


def sh02(sd, p):
    m = s1.sma(sd, p["sma"])
    return _short(s1._state_sig(sd, sd.c < m, np.isfinite(m)))


def sh03(sd, p):
    f, sl = s1.sma(sd, p["fast"]), s1.sma(sd, p["slow"])
    return _short(s1._state_sig(sd, f < sl, np.isfinite(f) & np.isfinite(sl)))


def sh04(sd, p):
    pn, pm = s1.prev_min_close(sd, p["entry"]), s1.prev_max_close(sd, p["exit"])
    return _short(s1._latched_sig(sd.c < pn, sd.c > pm, [np.isfinite(pn), np.isfinite(pm)]))


def sh05(sd, p):
    r = sd.memo(("rsi", p["period"]), lambda: _rsi(pd.Series(sd.c), p["period"]).to_numpy(np.float64))
    ok, tv = trend_below(sd, p["trend"])
    return _short(s1._latched_sig((r > p["entry_above"]) & ok, r < p["exit_below"], [np.isfinite(r), tv]))


def sh06(sd, p):
    z = sd.memo(("zscore", p["lookback"]), lambda: _zscore(pd.Series(sd.c), p["lookback"]).to_numpy(np.float64))
    ok, tv = trend_below(sd, p["trend"])
    return _short(s1._latched_sig((z >= p["entry_z"]) & ok, z <= p["exit_z"], [np.isfinite(z), tv]))


def sh07(sd, p):
    pa = s1._shift(s1.atr(sd, p["atr_window"]), 1)
    rise = sd.c - s1._shift(sd.c, p["rise_sessions"])
    ok, tv = trend_below(sd, p["trend"])
    return _short(s1._event_sig(sd, (rise >= p["mult"] * pa) & ok, [np.isfinite(pa), np.isfinite(rise), tv], p["hold"]))


def sh08(sd, p):
    pa = s1._shift(s1.atr(sd, p["atr_window"]), 1)
    gap = sd.o - s1._shift(sd.c, 1)
    ok, tv = trend_below(sd, p["trend"])
    return _short(s1._event_sig(sd, (gap >= p["mult"] * pa) & ok, [np.isfinite(pa), np.isfinite(gap), tv], p["hold"]))


def sh09(sd, p):
    def rng(w):
        return s1._shift(s1._roll(sd.h, w, "max") - s1._roll(sd.l, w, "min"), 1)
    rs, rl = rng(p["short"]), rng(p["long"])
    with np.errstate(divide="ignore", invalid="ignore"):
        ratio = np.where(rl > 0, rs / rl, NAN)
    pn, pm = s1.prev_min_close(sd, p["breakdown"]), s1.prev_max_close(sd, p["exit"])
    ok, tv = trend_below(sd, p["trend"])
    return _short(s1._latched_sig((ratio <= p["ratio"]) & (sd.c < pn) & ok, sd.c > pm,
                                  [np.isfinite(ratio), np.isfinite(pn), np.isfinite(pm), tv]))


def sh10(sd, p):
    ll = s1._roll(sd.c, p["low_lookback"], "min")
    return _short(s1._state_sig(sd, sd.c <= (1.0 + p["distance"]) * ll, np.isfinite(ll), p["cadence"]))


def sh11(sd, p):
    n = p["up_sessions"]
    up = np.concatenate([[False], sd.c[1:] > sd.c[:-1]]).astype(np.float64)
    ev = s1._roll(up, n, "sum") >= n
    ok, tv = trend_below(sd, p["trend"])
    return _short(s1._event_sig(sd, ev & ok, [np.arange(sd.n) >= n, tv], p["hold"]))


def sh12(sd, p):
    L = p["volume_lookback"]
    bm, bs = s1._shift(s1._roll(sd.v, L, "mean"), 1), s1._shift(s1._roll(sd.v, L, "std"), 1)
    with np.errstate(divide="ignore", invalid="ignore"):
        vz = np.where(bs > 0, (sd.v - bm) / bs, NAN)
    imp = s1.logret(sd, p["price_impulse"])
    sign = imp < 0 if p["mode"] == "continuation" else imp > 0     # down impulse continues; up impulse is faded
    return _short(s1._event_sig(sd, (vz >= p["volume_z"]) & sign, [np.isfinite(vz), np.isfinite(imp)], p["hold"]))


def sh13(sd, p):
    sv, lv = s1.std1(sd, p["short_vol"]), s1.std1(sd, p["long_vol"])
    with np.errstate(divide="ignore", invalid="ignore"):
        ratio = np.where(lv > 0, sv / lv, NAN)
    r1 = s1.lr1(sd)
    sign = r1 < 0 if p["mode"] == "breakdown" else r1 > 0           # reversal: upside expansion day is faded
    return _short(s1._event_sig(sd, (ratio >= p["expansion_ratio"]) & sign, [np.isfinite(ratio), np.isfinite(r1)], p["hold"]))


SHORT_BUILDERS = {"SH01": sh01, "SH02": sh02, "SH03": sh03, "SH04": sh04, "SH05": sh05, "SH06": sh06, "SH07": sh07,
                  "SH08": sh08, "SH09": sh09, "SH10": sh10, "SH11": sh11, "SH12": sh12, "SH13": sh13}


def _ls(sd, long_fn, long_params, short_fn, short_params) -> SigS | None:
    lg, sh = long_fn(sd, long_params), short_fn(sd, short_params)
    if lg is None or sh is None:
        return None
    s = max(lg.s, sh.s)
    d = lg.d.astype(np.int8) + sh.d              # +1 long, -1 short; both states at once cancel to flat, never an arbitrary side
    d[:s] = 0
    return SigS(d.astype(np.int8), lg.cond | sh.cond, s)


def ls01(sd, p):
    return _ls(sd, s1.s01, p, sh01, p)


def ls02(sd, p):
    return _ls(sd, s1.s02, p, sh02, p)


def ls03(sd, p):
    return _ls(sd, s1.s03, p, sh03, p)


def ls04(sd, p):
    return _ls(sd, s1.s10, {"high_lookback": p["lookback"], "distance": p["distance"], "cadence": p["cadence"]},
               sh10, {"low_lookback": p["lookback"], "distance": p["distance"], "cadence": p["cadence"]})


PER_SYMBOL = {**SHORT_BUILDERS, "LS01": ls01, "LS02": ls02, "LS03": ls03, "LS04": ls04}


def build(sd, family: str, params: dict) -> SigS | None:
    """Single entry point. A Census-01 family id is refused: Census-01 builders return long-only booleans."""
    if family not in PER_SYMBOL:
        raise KeyError(f"{family!r} is not a Census-02 family")
    return PER_SYMBOL[family](sd, params)
