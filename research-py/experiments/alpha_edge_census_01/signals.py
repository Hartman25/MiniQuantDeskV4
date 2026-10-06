"""Causal desired-position / condition builders for census families S01-S14.

Every builder returns Sig(d, cond, s):
  d     bool[n]  desired long position decided AFTER completed bar t (filled strictly after t by simulate.py)
  cond  bool[n]  non-executable condition at bar t for ConditionalEdge forward-return observations
  s     int      first bar at which all signal inputs are finite (None => never defined => NON_EVALUABLE)
Nothing here reads a bar after t when forming d[t] / cond[t] (calendar membership excepted: ex-ante dates)."""

from __future__ import annotations

import datetime as dt
import sys
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import pandas as pd

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import calendar_authority as cal  # noqa: E402
from mqk_research.indicators.core import rsi as _rsi  # noqa: E402
from mqk_research.indicators.core import zscore as _zscore  # noqa: E402

LAST_DISCOVERY_DATE = dt.date(2023, 12, 31)
SESSIONS = cal.sessions_between(dt.date(2016, 1, 1), LAST_DISCOVERY_DATE)
SESSION_INDEX = {d: i for i, d in enumerate(SESSIONS)}
_MONTH_POS = cal.month_session_positions(dt.date(2016, 1, 1), LAST_DISCOVERY_DATE)
NAN = np.nan


@dataclass(frozen=True)
class Sig:
    d: np.ndarray
    cond: np.ndarray
    s: int


class SymbolData:
    """One symbol's discovery-fenced arrays plus memoised derived series."""

    def __init__(self, symbol: str, bars: pd.DataFrame):
        self.symbol = symbol
        self.n = len(bars)
        self.o = bars["open"].to_numpy(np.float64)
        self.h = bars["high"].to_numpy(np.float64)
        self.l = bars["low"].to_numpy(np.float64)
        self.c = bars["close"].to_numpy(np.float64)
        self.v = bars["volume"].to_numpy(np.float64)
        self.om, self.hm, self.lm, self.cm = (np.rint(a * 1_000_000).astype(np.int64) for a in (self.o, self.h, self.l, self.c))
        days = [t.date() for t in bars["end_ts"]]
        try:
            self.ord = np.array([SESSION_INDEX[d] for d in days], dtype=np.int64)
        except KeyError as exc:
            raise RuntimeError(f"{symbol}: bar date {exc} is not a canonical discovery session; fail closed") from None
        if np.any(np.diff(self.ord) <= 0):
            raise RuntimeError(f"{symbol}: bar dates are not strictly increasing")
        self.dates = np.array(days, dtype="datetime64[D]")
        self.years = np.array([d.year for d in days], dtype=np.int64)
        self.month_end = np.array([_MONTH_POS[d][1] == 1 for d in days], dtype=bool)
        self.regime = None
        self._memo: dict = {}
        if not (np.all(self.h >= self.l) and np.all(self.c >= self.l) and np.all(self.c <= self.h) and np.all(self.c > 0)):
            raise RuntimeError(f"{symbol}: impossible bar shape; fail closed")

    def memo(self, key, fn):
        if key not in self._memo:
            self._memo[key] = fn()
        return self._memo[key]


def _roll(x: np.ndarray, w: int, how: str) -> np.ndarray:
    r = pd.Series(x).rolling(w)
    return getattr(r, how)().to_numpy()


def _shift(x: np.ndarray, k: int) -> np.ndarray:
    out = np.full(len(x), NAN)
    if k < len(x):
        out[k:] = x[: len(x) - k]
    return out


def lr1(sd: SymbolData) -> np.ndarray:
    return sd.memo("lr1", lambda: np.concatenate([[NAN], np.log(sd.c[1:] / sd.c[:-1])]))


def logret(sd: SymbolData, L: int) -> np.ndarray:
    def f():
        out = np.full(sd.n, NAN)
        if L < sd.n:
            out[L:] = np.log(sd.c[L:] / sd.c[:-L])
        return out
    return sd.memo(("logret", L), f)


def std1(sd: SymbolData, w: int) -> np.ndarray:
    return sd.memo(("std1", w), lambda: _roll(lr1(sd), w, "std"))


def sma(sd: SymbolData, w: int) -> np.ndarray:
    return sd.memo(("sma", w), lambda: _roll(sd.c, w, "mean"))


def prev_max_close(sd: SymbolData, k: int) -> np.ndarray:
    return sd.memo(("pmax", k), lambda: _shift(_roll(sd.c, k, "max"), 1))


def prev_min_close(sd: SymbolData, k: int) -> np.ndarray:
    return sd.memo(("pmin", k), lambda: _shift(_roll(sd.c, k, "min"), 1))


def atr(sd: SymbolData, w: int) -> np.ndarray:
    def f():
        pc = _shift(sd.c, 1)
        tr = np.maximum(sd.h - sd.l, np.maximum(np.abs(sd.h - pc), np.abs(sd.l - pc)))
        tr[0] = NAN
        return _roll(tr, w, "mean")
    return sd.memo(("atr", w), f)


def trend_ok(sd: SymbolData, trend) -> tuple[np.ndarray, np.ndarray]:
    """(ok, valid) for 'none' | 'sma200': close above its simple moving average at the same completed bar."""
    if trend == "none":
        t = np.ones(sd.n, bool)
        return t, t
    if trend != "sma200":
        raise ValueError(f"unsupported trend filter {trend!r}")
    m = sma(sd, 200)
    return sd.c > m, np.isfinite(m)


def first_valid(*masks) -> int | None:
    v = np.logical_and.reduce(masks)
    idx = np.flatnonzero(v)
    return int(idx[0]) if len(idx) else None


def hold_from_events(ev: np.ndarray, hold: int) -> np.ndarray:
    n = len(ev)
    cs = np.concatenate([[0], np.cumsum(ev, dtype=np.int64)])
    i = np.arange(n)
    return (cs[i + 1] - cs[np.maximum(i + 1 - hold, 0)]) > 0


def cadence_anchor(sd: SymbolData, cadence: str) -> np.ndarray:
    if cadence == "daily":
        return np.ones(sd.n, bool)
    if cadence == "month_end":
        return sd.month_end
    raise ValueError(f"unsupported cadence {cadence!r}")


def sample_hold(state: np.ndarray, anchor: np.ndarray, s: int) -> tuple[np.ndarray, np.ndarray]:
    """State sampled at anchor bars (bar >= s) and held until the next anchor."""
    n = len(state)
    i = np.arange(n)
    a = anchor & (i >= s)
    last = np.maximum.accumulate(np.where(a, i, -1))
    d = np.zeros(n, bool)
    ok = last >= 0
    d[ok] = state[last[ok]]
    return d, a & state


def latch(enter: np.ndarray, exit_: np.ndarray, s: int) -> np.ndarray:
    n = len(enter)
    el, xl = enter.tolist(), exit_.tolist()
    out = [False] * n
    inpos = False
    for t in range(s, n):
        if inpos:
            if xl[t]:
                inpos = False
            else:
                out[t] = True
        elif el[t]:
            inpos = True
            out[t] = True
    return np.array(out, bool)


def _finish(d: np.ndarray, cond: np.ndarray, s: int | None) -> Sig | None:
    if s is None:
        return None
    d = d.copy()
    cond = cond.copy()
    d[:s] = False
    cond[:s] = False
    return Sig(d, cond, s)


def _event_sig(sd, ev, valid_masks, hold):
    s = first_valid(*valid_masks)
    if s is None:
        return None
    ev = ev & (np.arange(sd.n) >= s)
    return _finish(hold_from_events(ev, hold), ev, s)


def _latched_sig(enter, exit_, valid_masks):
    s = first_valid(*valid_masks)
    if s is None:
        return None
    enter = enter & (np.arange(len(enter)) >= s)
    return _finish(latch(enter, exit_, s), enter, s)


def _state_sig(sd, state, valid, cadence="daily"):
    s = first_valid(valid)
    if s is None:
        return None
    d, cond = sample_hold(state & valid, cadence_anchor(sd, cadence), s)
    return _finish(d, cond, s)


def s01(sd, p):
    lr = logret(sd, p["lookback"])
    return _state_sig(sd, lr > 0, np.isfinite(lr), p["cadence"])


def s02(sd, p):
    m = sma(sd, p["sma"])
    return _state_sig(sd, sd.c > m, np.isfinite(m))


def s03(sd, p):
    f, sl = sma(sd, p["fast"]), sma(sd, p["slow"])
    return _state_sig(sd, f > sl, np.isfinite(f) & np.isfinite(sl))


def s04(sd, p):
    pm, pn = prev_max_close(sd, p["entry"]), prev_min_close(sd, p["exit"])
    return _latched_sig(sd.c > pm, sd.c < pn, [np.isfinite(pm), np.isfinite(pn)])


def s05(sd, p):
    r = sd.memo(("rsi", p["period"]), lambda: _rsi(pd.Series(sd.c), p["period"]).to_numpy(np.float64))
    ok, tv = trend_ok(sd, p["trend"])
    return _latched_sig((r < p["entry_below"]) & ok, r > p["exit_above"], [np.isfinite(r), tv])


def s06(sd, p):
    z = sd.memo(("zscore", p["lookback"]), lambda: _zscore(pd.Series(sd.c), p["lookback"]).to_numpy(np.float64))
    ok, tv = trend_ok(sd, p["trend"])
    return _latched_sig((z <= p["entry_z"]) & ok, z >= p["exit_z"], [np.isfinite(z), tv])


def s07(sd, p):
    pa = _shift(atr(sd, p["atr_window"]), 1)
    drop = _shift(sd.c, p["decline_sessions"]) - sd.c
    ok, tv = trend_ok(sd, p["trend"])
    return _event_sig(sd, (drop >= p["mult"] * pa) & ok, [np.isfinite(pa), np.isfinite(drop), tv], p["hold"])


def s08(sd, p):
    pa = _shift(atr(sd, p["atr_window"]), 1)
    gap = sd.o - _shift(sd.c, 1)
    ok, tv = trend_ok(sd, p["trend"])
    return _event_sig(sd, (gap <= -p["mult"] * pa) & ok, [np.isfinite(pa), np.isfinite(gap), tv], p["hold"])


def s09(sd, p):
    def rng(w):
        return _shift(_roll(sd.h, w, "max") - _roll(sd.l, w, "min"), 1)
    rs, rl = rng(p["short"]), rng(p["long"])
    with np.errstate(divide="ignore", invalid="ignore"):
        ratio = np.where(rl > 0, rs / rl, NAN)
    pm, pn = prev_max_close(sd, p["breakout"]), prev_min_close(sd, p["exit"])
    ok, tv = trend_ok(sd, p["trend"])
    return _latched_sig((ratio <= p["ratio"]) & (sd.c > pm) & ok, sd.c < pn,
                        [np.isfinite(ratio), np.isfinite(pm), np.isfinite(pn), tv])


def s10(sd, p):
    hh = _roll(sd.c, p["high_lookback"], "max")
    return _state_sig(sd, sd.c >= (1.0 - p["distance"]) * hh, np.isfinite(hh), p["cadence"])


def s11(sd, p):
    n = p["down_sessions"]
    down = np.concatenate([[False], sd.c[1:] < sd.c[:-1]]).astype(np.int64)
    ev = _roll(down.astype(np.float64), n, "sum") >= n
    ok, tv = trend_ok(sd, p["trend"])
    valid = np.arange(sd.n) >= n
    return _event_sig(sd, ev & ok, [valid, tv], p["hold"])


def s12(sd, p):
    L = p["volume_lookback"]
    base_m, base_s = _shift(_roll(sd.v, L, "mean"), 1), _shift(_roll(sd.v, L, "std"), 1)
    with np.errstate(divide="ignore", invalid="ignore"):
        vz = np.where(base_s > 0, (sd.v - base_m) / base_s, NAN)
    imp = logret(sd, p["price_impulse"])
    sign = imp > 0 if p["mode"] == "continuation" else imp < 0
    return _event_sig(sd, (vz >= p["volume_z"]) & sign, [np.isfinite(vz), np.isfinite(imp)], p["hold"])


def s13(sd, p):
    sv, lv = std1(sd, p["short_vol"]), std1(sd, p["long_vol"])
    with np.errstate(divide="ignore", invalid="ignore"):
        ratio = np.where(lv > 0, sv / lv, NAN)
    r1 = lr1(sd)
    sign = r1 > 0 if p["mode"] == "breakout" else r1 < 0
    return _event_sig(sd, (ratio >= p["expansion_ratio"]) & sign, [np.isfinite(ratio), np.isfinite(r1)], p["hold"])


def calendar_member(sd: SymbolData, p: dict) -> np.ndarray:
    """member[b]: bar b's session belongs to the calendar set (ex-ante date rule)."""
    days = [SESSIONS[i] for i in sd.ord]
    k = p["kind"]
    if k == "season":
        if p["value"] != "nov_apr":
            raise ValueError(p["value"])
        return np.array([d.month in (11, 12, 1, 2, 3, 4) for d in days])
    if k == "tom":
        last, first = p["last"], p["first"]
        out = []
        for d in days:
            o, rem = _MONTH_POS[d]
            out.append(rem <= last or o <= first)
        return np.array(out)
    raise ValueError(k)


def s14(sd, p):
    n = sd.n
    if n < 3:
        return None
    m = calendar_member(sd, p)
    d = np.zeros(n, bool)
    cond = np.zeros(n, bool)
    d[: n - 2] = m[2:]
    cond[: n - 1] = m[1:]
    return Sig(d, cond, 0)


PER_SYMBOL = {"S01": s01, "S02": s02, "S03": s03, "S04": s04, "S05": s05, "S06": s06, "S07": s07, "S08": s08,
              "S09": s09, "S10": s10, "S11": s11, "S12": s12, "S13": s13, "S14": s14}


class Universe:
    """All eligible symbols, the global session grid and the SPY regime."""

    def __init__(self, symbol_bars: dict[str, pd.DataFrame]):
        self.symbols = sorted(symbol_bars)
        self.sd = {s: SymbolData(s, b) for s, b in symbol_bars.items()}
        self.G = len(SESSIONS)
        self._assign_regime()

    def _assign_regime(self):
        """regime_g[g]: SPY close > SMA200 at the last SPY session strictly before session g (2 = unknown)."""
        spy = self.sd.get("SPY")
        if spy is None:
            raise RuntimeError("SPY missing: regime attribution unavailable; fail closed")
        m = sma(spy, 200)
        on = np.where(np.isfinite(m), (spy.c > m).astype(np.int8), 2).astype(np.int8)
        last = np.full(self.G, -1, np.int64)
        last[spy.ord] = np.arange(spy.n)
        known = np.maximum.accumulate(last)
        at_or_before = np.where(known >= 0, on[np.maximum(known, 0)], 2).astype(np.int8)
        self.regime_g = np.concatenate([[2], at_or_before[:-1]]).astype(np.int8)
        self.years_g = np.array([d.year for d in SESSIONS], dtype=np.int64)
        for sd in self.sd.values():
            sd.regime = self.regime_g[sd.ord]

    def build(self, symbol: str, family: str, params: dict) -> Sig | None:
        """Single entry point: desired/condition arrays for one (family, params, symbol)."""
        return PER_SYMBOL[family](self.sd[symbol], params)
