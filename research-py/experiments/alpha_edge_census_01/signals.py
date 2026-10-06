"""Causal desired-position / condition builders for census families S01-S20.

Every builder returns Sig(d, cond, s):
  d     bool[n]  desired long position decided AFTER completed bar t (filled strictly after t by simulate.py)
  cond  bool[n]  non-executable condition at bar t for ConditionalEdge forward-return statistics
  s     int      first bar at which all signal inputs are finite (None => never defined => NON_EVALUABLE)
Nothing here reads a bar after t when forming d[t] / cond[t] (calendar membership excepted: ex-ante dates)."""

from __future__ import annotations

import datetime as dt
import sys
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import pandas as pd
from numpy.lib.stride_tricks import sliding_window_view

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import calendar_authority as cal  # noqa: E402

MIN_CROSS_SECTION = 20
MIN_TRAIN_ROWS = 504
FEATURE_ROW_KEY = "features"
SESSIONS = cal.sessions_between(dt.date(2016, 1, 1), dt.date(2024, 12, 31))
SESSION_INDEX = {d: i for i, d in enumerate(SESSIONS)}
_MONTH_POS = cal.month_session_positions(dt.date(2016, 1, 1), dt.date(2024, 12, 31))
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
            raise RuntimeError(f"{symbol}: bar date {exc} is not a canonical session; fail closed") from None
        if np.any(np.diff(self.ord) <= 0):
            raise RuntimeError(f"{symbol}: bar dates are not strictly increasing")
        self.dates = np.array(days, dtype="datetime64[D]")
        self.years = np.array([d.year for d in days], dtype=np.int64)
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


def vol_ratio(sd: SymbolData) -> np.ndarray:
    def f():
        m = _roll(sd.v, 20, "mean")
        with np.errstate(divide="ignore", invalid="ignore"):
            return np.where(m > 0, sd.v / m, NAN)
    return sd.memo("vr", f)


def trend_ok(sd: SymbolData, trend) -> tuple[np.ndarray, np.ndarray]:
    """(ok, valid) for 'none'|'sma100'|'sma200'|int window (0 => none)."""
    if trend in ("none", 0):
        t = np.ones(sd.n, bool)
        return t, t
    w = int(trend[3:]) if isinstance(trend, str) else int(trend)
    m = sma(sd, w)
    return sd.c > m, np.isfinite(m)


def z_excursion(sd: SymbolData, L: int) -> np.ndarray:
    """log-return over L bars divided by sigma20 measured at t-L times sqrt(L)."""
    def f():
        sig = _shift(std1(sd, 20), L)
        with np.errstate(divide="ignore", invalid="ignore"):
            return np.where(sig > 0, logret(sd, L) / (sig * np.sqrt(L)), NAN)
    return sd.memo(("zex", L), f)


def first_valid(*masks) -> int | None:
    v = np.logical_and.reduce(masks)
    idx = np.flatnonzero(v)
    return int(idx[0]) if len(idx) else None


def hold_from_events(ev: np.ndarray, hold: int) -> np.ndarray:
    n = len(ev)
    cs = np.concatenate([[0], np.cumsum(ev, dtype=np.int64)])
    i = np.arange(n)
    return (cs[i + 1] - cs[np.maximum(i + 1 - hold, 0)]) > 0


def sample_hold(state: np.ndarray, ordinal: np.ndarray, k: int, s: int) -> tuple[np.ndarray, np.ndarray]:
    """State sampled at anchor bars (session ordinal % k == 0, bar >= s) and held until the next anchor."""
    n = len(state)
    i = np.arange(n)
    anchor = (ordinal % k == 0) & (i >= s)
    last = np.maximum.accumulate(np.where(anchor, i, -1))
    d = np.zeros(n, bool)
    ok = last >= 0
    d[ok] = state[last[ok]]
    return d, anchor & state


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


def s01(sd, p):
    L, k = p["lookback"], p["rebalance"]
    lr = logret(sd, L)
    if p["norm"] == "raw":
        state, valid = lr > 0, np.isfinite(lr)
    else:
        with np.errstate(divide="ignore", invalid="ignore"):
            z = np.where(std1(sd, p["vol_window"]) > 0, lr / (std1(sd, p["vol_window"]) * np.sqrt(L)), NAN)
        state, valid = z > p["z_min"], np.isfinite(z)
    s = first_valid(valid)
    if s is None:
        return None
    d, cond = sample_hold(state & valid, sd.ord, k, s)
    return _finish(d, cond, s)


def s03(sd, p):
    z = z_excursion(sd, p["lookback"])
    ok, tv = trend_ok(sd, p["trend"])
    return _event_sig(sd, (z <= -p["z_thr"]) & ok, [np.isfinite(z), tv], p["hold"])


def s04(sd, p):
    w = p["window"]
    m, sdv = _roll(sd.c, w, "mean"), _roll(sd.c, w, "std")
    with np.errstate(divide="ignore", invalid="ignore"):
        z = np.where(sdv > 0, (sd.c - m) / sdv, NAN)
    ok, tv = trend_ok(sd, p["trend"])
    s = first_valid(np.isfinite(z), tv)
    if s is None:
        return None
    enter = (z <= p["entry_z"]) & ok
    exit_ = z >= p["exit_z"]
    enter[:s] = False
    return _finish(latch(enter, exit_, s), enter, s)


def s05(sd, p):
    pm, pn = prev_max_close(sd, p["entry"]), prev_min_close(sd, p["exit"])
    ok, tv = trend_ok(sd, p["trend"])
    s = first_valid(np.isfinite(pm), np.isfinite(pn), tv)
    if s is None:
        return None
    enter = (sd.c > pm) & ok
    enter[:s] = False
    return _finish(latch(enter, sd.c < pn, s), enter, s)


def s06(sd, p):
    f, sl = sma(sd, p["fast"]), sma(sd, p["slow"])
    s = first_valid(np.isfinite(f), np.isfinite(sl))
    if s is None:
        return None
    state = f > sl
    return _finish(state, state, s)


def rolling_ols_log(sd: SymbolData, w: int):
    def f():
        y = np.log(sd.c)
        slope = np.full(sd.n, NAN)
        r2 = np.full(sd.n, NAN)
        if sd.n >= w:
            win = sliding_window_view(y, w)
            x = np.arange(w) - (w - 1) / 2.0
            sxx = float((x * x).sum())
            sl = (win @ x) / sxx
            ssy = ((win - win.mean(axis=1, keepdims=True)) ** 2).sum(axis=1)
            with np.errstate(divide="ignore", invalid="ignore"):
                rr = np.where(ssy > 0, sl * sl * sxx / ssy, NAN)
            slope[w - 1:], r2[w - 1:] = sl, rr
        return slope, r2
    return sd.memo(("ols", w), f)


def s07(sd, p):
    slope, r2 = rolling_ols_log(sd, p["window"])
    valid = np.isfinite(slope) & np.isfinite(r2)
    s = first_valid(valid)
    if s is None:
        return None
    d, cond = sample_hold((slope > 0) & (r2 >= p["min_r2"]) & valid, sd.ord, p["rebalance"], s)
    return _finish(d, cond, s)


def s08(sd, p):
    hh = _roll(sd.c, p["high_window"], "max")
    valid = np.isfinite(hh)
    s = first_valid(valid)
    if s is None:
        return None
    d, cond = sample_hold((sd.c / hh >= p["min_ratio"]) & valid, sd.ord, p["rebalance"], s)
    return _finish(d, cond, s)


def s09(sd, p):
    L, a = p["drop_lookback"], atr(sd, p["atr_window"])
    pa = _shift(a, 1)
    drop = _shift(sd.c, L) - sd.c
    ok, tv = trend_ok(sd, p["trend"])
    valid = [np.isfinite(pa), np.isfinite(drop), tv]
    return _event_sig(sd, (drop >= p["mult"] * pa) & ok, valid, p["hold"])


def s10(sd, p):
    pa = _shift(atr(sd, p["atr_window"]), 1)
    gap = sd.o - _shift(sd.c, 1)
    ok, tv = trend_ok(sd, p["trend"])
    return _event_sig(sd, (gap <= -p["mult"] * pa) & ok, [np.isfinite(pa), np.isfinite(gap), tv], p["hold"])


def s11(sd, p):
    def rng(w):
        return _shift(_roll(sd.h, w, "max") - _roll(sd.l, w, "min"), 1)
    rs, rl = rng(p["short"]), rng(p["long"])
    with np.errstate(divide="ignore", invalid="ignore"):
        ratio = np.where(rl > 0, rs / rl, NAN)
    pm, pn = prev_max_close(sd, p["breakout"]), prev_min_close(sd, p["exit"])
    s = first_valid(np.isfinite(ratio), np.isfinite(pm), np.isfinite(pn))
    if s is None:
        return None
    enter = (ratio <= p["ratio"]) & (sd.c > pm)
    enter[:s] = False
    return _finish(latch(enter, sd.c < pn, s), enter, s)


def s12(sd, p):
    lr, vr = logret(sd, p["lookback"]), vol_ratio(sd)
    ok, tv = trend_ok(sd, p["trend"])
    return _event_sig(sd, (lr > 0) & (vr >= p["vol_ratio_min"]) & ok, [np.isfinite(lr), np.isfinite(vr), tv], p["hold"])


def s13(sd, p):
    pc = _shift(sd.c, 1)
    if p["norm"] == "raw_pct":
        thr = p["mult"] / 100.0 * pc
        valid = [np.isfinite(pc)]
    else:
        pa = _shift(atr(sd, 14), 1)
        thr = p["mult"] * pa
        valid = [np.isfinite(pa), np.isfinite(pc)]
    kind = p["kind"]
    if kind == "gap_down_reversal":
        ev = (sd.o - pc) <= -thr
    elif kind == "gap_up_continuation":
        ev = (sd.o - pc) >= thr
    elif kind == "intraday_up_continuation":
        ev = (sd.c - sd.o) >= thr
    elif kind == "intraday_down_reversal":
        ev = (sd.c - sd.o) <= -thr
    else:
        raise ValueError(kind)
    return _event_sig(sd, ev, valid, p["hold"])


def calendar_member(sd: SymbolData, p: dict) -> np.ndarray:
    """member[b]: bar b's session belongs to the calendar set (ex-ante date rule)."""
    days = [SESSIONS[i] for i in sd.ord]
    k = p["kind"]
    if k == "weekday":
        return np.array([d.weekday() == p["value"] for d in days])
    if k == "month":
        return np.array([d.month == p["value"] for d in days])
    if k == "season":
        nov_apr = {11, 12, 1, 2, 3, 4}
        return np.array([(d.month in nov_apr) == (p["value"] == "nov_apr") for d in days])
    if k == "tom":
        last, first = p["last"], p["first"]
        out = []
        for d in days:
            o, rem = _MONTH_POS[d]
            out.append((last > 0 and rem <= last) or (first > 0 and o <= first))
        return np.array(out)
    raise ValueError(k)


def s14(sd, p):
    m = calendar_member(sd, p)
    n = sd.n
    d = np.zeros(n, bool)
    cond = np.zeros(n, bool)
    d[: n - 2] = m[2:]
    cond[: n - 1] = m[1:]
    if n < 3:
        return None
    return Sig(d, cond, 0)


def s15(sd, p):
    w = p["vol_window"]

    def f():
        vol = std1(sd, w)
        pr = np.full(sd.n, NAN)
        v = vol[w:]
        if len(v) >= 252:
            win = sliding_window_view(v, 252)
            pr[w + 251:] = (win <= win[:, -1:]).mean(axis=1)
        return pr
    pr = sd.memo(("volpct", w), f)
    ok, tv = trend_ok(sd, p["trend_sma"])
    s = first_valid(np.isfinite(pr), tv)
    if s is None:
        return None
    d, cond = sample_hold((pr <= p["pctile"]) & ok & np.isfinite(pr), sd.ord, p["rebalance"], s)
    return _finish(d, cond, s)


def s16(sd, p):
    z = z_excursion(sd, p["pullback_lookback"])
    m = sma(sd, p["trend_lookback"])
    ev = (z <= -p["z_thr"]) & (sd.c > m)
    return _event_sig(sd, ev, [np.isfinite(z), np.isfinite(m)], p["hold"])


def s17(sd, p):
    pm, pn, vr = prev_max_close(sd, p["breakout"]), prev_min_close(sd, p["exit"]), vol_ratio(sd)
    s = first_valid(np.isfinite(pm), np.isfinite(pn), np.isfinite(vr))
    if s is None:
        return None
    enter = (sd.c > pm) & (vr >= p["vol_ratio_min"])
    enter[:s] = False
    return _finish(latch(enter, sd.c < pn, s), enter, s)


def s_vol(sd, p):
    vr = vol_ratio(sd)
    return _event_sig(sd, vr >= p["vol_ratio_min"], [np.isfinite(vr)], p["hold"])


PER_SYMBOL = {"S01": s01, "S03": s03, "S04": s04, "S05": s05, "S06": s06, "S07": s07, "S08": s08, "S09": s09,
              "S10": s10, "S11": s11, "S12": s12, "S13": s13, "S14": s14, "S15": s15, "S16": s16, "S17": s17,
              "VOL": s_vol}


class Universe:
    """All evaluable symbols, FeatureSetV1 columns, the global session grid, SPY regime and S02 rank matrices."""

    def __init__(self, symbol_bars: dict[str, pd.DataFrame]):
        self.symbols = sorted(symbol_bars)
        self.sd = {s: SymbolData(s, b) for s, b in symbol_bars.items()}
        self.col = {s: i for i, s in enumerate(self.symbols)}
        self.G = len(SESSIONS)
        self._memo: dict = {}
        self._build_features(symbol_bars)
        self._assign_regime()

    def _build_features(self, symbol_bars):
        from mqk_research.features.feature_set_v1 import build_feature_set_v1
        frames = []
        for s in self.symbols:
            b = symbol_bars[s][["symbol", "end_ts", "open", "high", "low", "close", "volume"]].copy()
            b["end_ts"] = b["end_ts"].astype(str)
            frames.append(b)
        feats = build_feature_set_v1(pd.concat(frames, ignore_index=True))
        self.features: dict[str, dict[str, np.ndarray]] = {}
        for s, g in feats.groupby("symbol", sort=True):
            if len(g) != self.sd[s].n:
                raise RuntimeError(f"{s}: FeatureSetV1 row count differs from bars")
            self.features[s] = {c: g[c].to_numpy(np.float64) for c in g.columns if c not in ("symbol", "end_ts")}
        missing = sorted(set(self.symbols) - set(self.features))
        if missing:
            raise RuntimeError(f"FeatureSetV1 skipped symbols {missing}; fail closed")

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

    def feat(self, symbol: str, name: str) -> np.ndarray:
        return self.features[symbol][name]

    def _frame(self, key, make):
        if key not in self._memo:
            self._memo[key] = make()
        return self._memo[key]

    def _rank_state(self, basis: str, L: int | None, cutoff: float):
        """(state, valid) G x nsym boolean matrices for the S02 cross-sectional rank rule."""
        def base():
            mat = np.full((self.G, len(self.symbols)), NAN)
            for s in self.symbols:
                sd = self.sd[s]
                if basis == "ret":
                    v = np.full(sd.n, NAN)
                    v[L:] = sd.c[L:] / sd.c[:-L] - 1.0
                else:
                    v = self.features[s]["momentum_score"]
                mat[sd.ord, self.col[s]] = v
            df = pd.DataFrame(mat)
            if basis == "ret":
                rk = df.rank(axis=1, pct=True).to_numpy()
            else:
                rk = mat
            cnt = np.isfinite(mat).sum(axis=1)
            rk = np.where((cnt >= MIN_CROSS_SECTION)[:, None], rk, NAN)
            return rk
        rk = self._frame(("rank", basis, L), base)
        return rk >= cutoff, np.isfinite(rk)

    def s02(self, symbol: str, p: dict):
        state, valid = self._rank_state(p["rank_basis"], p.get("lookback"), p["cutoff"])
        sd = self.sd[symbol]
        st, va = state[sd.ord, self.col[symbol]], valid[sd.ord, self.col[symbol]]
        s = first_valid(va)
        if s is None:
            return None
        d, cond = sample_hold(st & va, sd.ord, p["rebalance"], s)
        return _finish(d, cond, s)

    # ---- S18 / S19 fold machinery -------------------------------------------------------------------------
    def _fold_starts(self, sd: SymbolData):
        yrs = sd.years
        return {int(y): int(np.argmax(yrs == y)) for y in np.unique(yrs)}

    def s18(self, symbol: str, p: dict):
        sd = self.sd[symbol]
        x = self.feat(symbol, p["feature"])
        ev = np.zeros(sd.n, bool)
        first = None
        for y, start in self._fold_starts(sd).items():
            def q_all(y=y, start=start):
                tr = x[:start]
                tr = tr[np.isfinite(tr)]
                if len(tr) < MIN_TRAIN_ROWS:
                    return None
                return np.quantile(tr, [0.1, 0.2, 0.3, 0.7, 0.8, 0.9])
            qs = sd.memo(("s18thr", p["feature"], y), q_all)
            if qs is None:
                continue
            thr = qs[[0.1, 0.2, 0.3, 0.7, 0.8, 0.9].index(p["q"])]
            end = int(np.argmax(sd.years > y)) if np.any(sd.years > y) else sd.n
            seg = x[start:end]
            ev[start:end] = (seg >= thr) if p["side"] == "ge" else (seg <= thr)
            first = start if first is None else first
        if first is None:
            return None
        return _finish(hold_from_events(ev, p["hold"]), ev, first)

    def s19(self, symbol: str, p: dict):
        from mqk_research.ml.model_logreg import fit_logreg_deterministic
        sd = self.sd[symbol]
        lh = p["label_horizon"]

        def probs():
            x = self.feat(symbol, p["feature"])
            pa = np.full(sd.n, NAN)
            y = np.full(sd.n, NAN)
            y[: sd.n - lh] = (sd.c[lh:] / sd.c[: sd.n - lh] - 1.0 > 0).astype(np.float64)
            first = None
            for yr, start in self._fold_starts(sd).items():
                tr_idx = np.arange(0, max(start - lh, 0))
                tr_idx = tr_idx[np.isfinite(x[tr_idx]) & np.isfinite(y[tr_idx])]
                if len(tr_idx) < MIN_TRAIN_ROWS:
                    continue
                model = fit_logreg_deterministic(x[tr_idx, None], y[tr_idx], feature_columns=[p["feature"]], l2=0.01,
                                                 lr=0.1, steps=300, fit_intercept=True, standardize=True, clip_z=5.0)
                end = int(np.argmax(sd.years > yr)) if np.any(sd.years > yr) else sd.n
                seg = x[start:end]
                ok = np.isfinite(seg)
                out = np.full(len(seg), NAN)
                if ok.any():
                    out[ok] = model.predict_proba(seg[ok, None])
                pa[start:end] = out
                first = start if first is None else first
            return pa, first
        pa, first = sd.memo(("s19p", p["feature"], lh), probs)
        if first is None:
            return None
        ev = pa >= p["p_entry"]
        return _finish(hold_from_events(ev, lh), ev, first)

    def s20(self, symbol: str, p: dict):
        sigs = [self.component(symbol, c) for c in (p["a"], p["b"])]
        if any(x is None for x in sigs):
            return None
        d = sigs[0].d & sigs[1].d
        s = max(sigs[0].s, sigs[1].s)
        return _finish(d, d, s)

    def component(self, symbol: str, c: dict):
        fam = c["family"]
        params = {k: v for k, v in c.items() if k != "family"}
        if fam == "S02":
            return self.s02(symbol, params)
        return PER_SYMBOL[fam](self.sd[symbol], params)

    def build(self, symbol: str, family: str, params: dict) -> Sig | None:
        """Single entry point: desired/condition arrays for one (family, params, symbol)."""
        if family == "S02":
            return self.s02(symbol, params)
        if family == "S18":
            return self.s18(symbol, params)
        if family == "S19":
            return self.s19(symbol, params)
        if family == "S20":
            return self.s20(symbol, params)
        return PER_SYMBOL[family](self.sd[symbol], params)
