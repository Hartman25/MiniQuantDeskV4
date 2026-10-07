"""Confirmation evaluation core: scored-window observation frames on the 2024-01-01..2026-02-28 canonical grid, the accepted
Census estimator/null reused unchanged, complete-family BH, pure disposition logic, read-only ranking and diagnostics.
2024 is indicator warm-up only; every scored row, baseline row and label endpoint must lie in [2025-01-01, 2026-03-01)."""

from __future__ import annotations

import datetime as dt
import math

import numpy as np
import pandas as pd

import c1_cohort as cc  # noqa: F401  (puts the census dir and research-py/src on sys.path)
import c1_protocol as cp
import calendar_authority as cal  # noqa: E402
import conditional as cn  # noqa: E402
import signals as sg  # noqa: E402
from mqk_research.factors.contracts import EVAL_STATUS_SUCCEEDED  # noqa: E402
from mqk_research.factors.diagnostics import evaluate_factor_ic_ir  # noqa: E402
from mqk_research.factors.fdr import benjamini_hochberg  # noqa: E402
from partitions import PartitionBreach  # noqa: E402

FAMILIES = ("S05", "S06", "S13")
SCORE_START_TS = pd.Timestamp(cp.SCORE_START, tz="UTC")
FENCE_TS = pd.Timestamp(cp.SCORE_END_EXCLUSIVE, tz="UTC")
FENCE_DATE = np.datetime64(cp.SCORE_END_EXCLUSIVE, "D")
SCORE_START_DATE = np.datetime64(cp.SCORE_START, "D")
SESSIONS = cal.sessions_between(dt.date.fromisoformat(cp.WARMUP_START), dt.date(2026, 2, 28))
SESSION_INDEX = {d: i for i, d in enumerate(SESSIONS)}
_MONTH_POS = cal.month_session_positions(dt.date.fromisoformat(cp.WARMUP_START), dt.date(2026, 2, 28))


class LabelFenceBreach(PartitionBreach):
    """A bar, scored row or label endpoint at/after the final-holdout fence, or a scored row inside the warm-up."""


class ConfSymbolData(sg.SymbolData):
    """SymbolData on the Confirmation grid (warm-up + reserve). Same arrays and shape checks as the Census class."""

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
        if days and days[-1] >= dt.date.fromisoformat(cp.SCORE_END_EXCLUSIVE):
            raise LabelFenceBreach(f"{symbol}: bar {days[-1]} is not strictly before {cp.SCORE_END_EXCLUSIVE}")
        try:
            self.ord = np.array([SESSION_INDEX[d] for d in days], dtype=np.int64)
        except KeyError as exc:
            raise RuntimeError(f"{symbol}: bar date {exc} is not a canonical Confirmation-grid session; fail closed") from None
        if np.any(np.diff(self.ord) <= 0):
            raise RuntimeError(f"{symbol}: bar dates are not strictly increasing")
        self.end_iso = np.array([t.isoformat() for t in bars["end_ts"]])
        self.dates = np.array(days, dtype="datetime64[D]")
        self.years = np.array([d.year for d in days], dtype=np.int64)
        self.month_end = np.array([_MONTH_POS[d][1] == 1 for d in days], dtype=bool)
        self.regime = None
        self._memo = {}
        if not (np.all(self.h >= self.l) and np.all(self.c >= self.l) and np.all(self.c <= self.h) and np.all(self.c > 0)):
            raise RuntimeError(f"{symbol}: impossible bar shape; fail closed")


class ConfUniverse(sg.Universe):
    """Eligible symbols on the Confirmation grid with the SPY regime (SPY close > SMA200 at the last SPY session strictly
    before the session; 2 = unknown). Only the supported Confirmation families may be built."""

    def __init__(self, symbol_bars: dict[str, pd.DataFrame]):
        self.symbols = sorted(symbol_bars)
        self.sd = {s: ConfSymbolData(s, b) for s, b in symbol_bars.items()}
        self.G = len(SESSIONS)
        self._assign_regime()

    def _assign_regime(self):
        spy = self.sd.get("SPY")
        if spy is None:
            raise RuntimeError("SPY missing: regime attribution unavailable; fail closed")
        m = sg.sma(spy, 200)
        on = np.where(np.isfinite(m), (spy.c > m).astype(np.int8), 2).astype(np.int8)
        last = np.full(self.G, -1, np.int64)
        last[spy.ord] = np.arange(spy.n)
        known = np.maximum.accumulate(last)
        at_or_before = np.where(known >= 0, on[np.maximum(known, 0)], 2).astype(np.int8)
        self.regime_g = np.concatenate([[2], at_or_before[:-1]]).astype(np.int8)
        for sd in self.sd.values():
            sd.regime = self.regime_g[sd.ord]

    def build(self, symbol: str, family: str, params: dict):
        if family not in FAMILIES:
            raise ValueError(f"family {family!r} is not part of the frozen Confirmation cohort")
        return super().build(symbol, family, params)


def assert_scored(frame: pd.DataFrame) -> dict:
    """Every observation lies in the reserve and every label endpoint is strictly before the fence. Empty frames pass
    vacuously and report no bounds."""
    if frame.empty:
        return {"rows": 0, "min_period": None, "max_period": None, "max_label_end": None}
    per = pd.to_datetime(frame["period_ts_utc"], utc=True)
    end = pd.to_datetime(frame["label_end_ts_utc"], utc=True)
    if per.min() < SCORE_START_TS:
        raise LabelFenceBreach(f"scored observation {per.min().isoformat()} precedes {cp.SCORE_START} (warm-up must not be scored)")
    if per.max() >= FENCE_TS or end.max() >= FENCE_TS:
        raise LabelFenceBreach(f"observation/label endpoint {max(per.max(), end.max()).isoformat()} reaches {cp.SCORE_END_EXCLUSIVE}")
    if not bool((end > per).all()):
        raise LabelFenceBreach("label endpoint is not strictly after the observation")
    return {"rows": int(len(frame)), "min_period": per.min().isoformat(), "max_period": per.max().isoformat(),
            "max_label_end": end.max().isoformat()}


def build_conf_frame(U: ConfUniverse, condition: dict, horizon: int) -> tuple[pd.DataFrame, pd.DataFrame]:
    """(observations, aux). One row per symbol bar t in the reserve with the condition defined and the t+horizon bar present
    and strictly before the fence. The unconditional baseline mean is over exactly these scored rows."""
    parts = []
    for sym in U.symbols:
        sd = U.sd[sym]
        sig = cn.condition_sig(U, sym, condition)
        if sig is None:
            continue
        first_scored = int(np.searchsorted(sd.dates, SCORE_START_DATE, side="left"))
        idx = np.arange(max(sig.s, first_scored), sd.n - horizon)
        if idx.size:
            idx = idx[sd.dates[idx + horizon] < FENCE_DATE]
        if idx.size == 0:
            continue
        ret = sd.c[idx + horizon] / sd.c[idx] - 1.0
        parts.append(pd.DataFrame({
            "symbol": sym, "period_ts_utc": sd.end_iso[idx], "factor_value": sig.cond[idx].astype(np.float64),
            "label_fwd_ret": ret - ret.mean(), "information_cutoff_ts_utc": sd.end_iso[idx],
            "label_end_ts_utc": sd.end_iso[idx + horizon], "year": sd.years[idx], "regime": sd.regime[idx],
            "month": [str(x)[:7] for x in sd.dates[idx]],
            "quarter": [f"{str(x)[:4]}Q{(int(str(x)[5:7]) - 1) // 3 + 1}" for x in sd.dates[idx]]}))
    if not parts:
        return (pd.DataFrame({c: [] for c in cn.FRAME_COLUMNS}), pd.DataFrame({"year": [], "regime": [], "month": [], "quarter": []}))
    df = pd.concat(parts, ignore_index=True).sort_values(["period_ts_utc", "symbol"], kind="mergesort").reset_index(drop=True)
    frame = df[cn.FRAME_COLUMNS].copy()
    assert_scored(frame)
    return frame, df[["year", "regime", "month", "quarter"]].copy()


def _grouped(frame: pd.DataFrame, aux: pd.DataFrame, key: str, sign: float) -> dict:
    ev = frame["factor_value"].to_numpy() == 1.0
    if not ev.any():
        return {}
    lab, k = frame["label_fwd_ret"].to_numpy()[ev], aux[key].to_numpy()[ev]
    return {str(g): {"n": int((k == g).sum()), "effect": sign * float(lab[k == g].mean())} for g in sorted(set(k.tolist()))}


def evaluate_factor(U: ConfUniverse, entry: dict, cache: cn.PermutationCache | None = None) -> dict:
    """One factor's Confirmation evidence (no final disposition: q needs the whole family). Pure given the universe."""
    direction, horizon = entry["direction"], entry["horizon"]
    if direction != cp.DIRECTION or horizon not in cp.HORIZONS:
        raise ValueError(f"{entry['factor_id']}: direction/horizon outside the frozen contract")
    sign = 1.0 if direction == cp.DIRECTION else -1.0
    frame, aux = build_conf_frame(U, entry, horizon)
    bounds = assert_scored(frame)
    ev = cn.event_diagnostics(frame, aux, direction)
    rec = {"factor_id": entry["factor_id"], "evaluation_id": entry["evaluation_id"], "family": entry["family"],
           "condition_id": entry["condition_id"], "params": entry["params"], "horizon": horizon, "direction": direction,
           "events": ev["event_count"], "rows": ev["row_count"], "effect": ev["direction_adjusted_effect"],
           "symbols_represented": ev["symbols_represented"], "top_symbol_event_share": ev["top_symbol_event_share"],
           "per_symbol": ev["per_symbol"], "per_regime": ev["per_regime"], "per_year": ev["per_year"],
           "per_month": _grouped(frame, aux, "month", sign), "per_quarter": _grouped(frame, aux, "quarter", sign),
           "scored_bounds": bounds, "p_value": None, "mean_ic": None, "evaluable": False, "reason": None}
    if ev["event_count"] < cp.MIN_EVENTS:
        return {**rec, "reason": cp.REASON_INSUFFICIENT_EVENTS}
    report = evaluate_factor_ic_ir(frame, direction=direction, n_quantiles=cp.N_QUANTILES,
                                   min_cross_section=cp.MIN_CROSS_SECTION, min_periods=cp.MIN_PERIODS)
    if report.status != EVAL_STATUS_SUCCEEDED:
        return {**rec, "reason": f"native_not_evaluable:{report.reason}"}
    pv = cn.fast_empirical_pvalue(frame, report.metrics, n_permutations=cp.N_PERMUTATIONS, base_seed=cp.BASE_SEED, cache=cache)
    return {**rec, "evaluable": True, "p_value": float(pv["p_value"]), "mean_ic": float(report.metrics["mean_ic"]),
            "null": {"n_permutations": pv["n_permutations_used"], "base_seed": pv["base_seed"], "exceed_count": pv["exceed_count"]}}


def decide(events: int, effect, p, q) -> tuple[str, str | None]:
    """Pure disposition rule. Non-evaluable inputs are never combined with NOT_CONFIRMED."""
    if events < cp.MIN_EVENTS:
        return cp.NOT_EVALUABLE, cp.REASON_INSUFFICIENT_EVENTS
    if effect is None or not math.isfinite(float(effect)):
        return cp.NOT_EVALUABLE, "non_finite_effect"
    if p is None or q is None or not (math.isfinite(float(p)) and math.isfinite(float(q))):
        raise ValueError("an evaluable factor must carry a finite p-value and q-value")
    if float(effect) <= 0.0:
        return cp.NOT_CONFIRMED, "effect_not_positive"
    if float(p) <= cp.P_STRONG_MAX and float(q) <= cp.FDR_ALPHA:
        return cp.CONFIRMED_STRONG, None
    return cp.CONFIRMED_DIRECTIONAL_ONLY, "p_or_q_above_strong_threshold"


def finalize(entries: list[dict], evidence: dict[str, dict]) -> dict:
    """Complete-family BH over exactly the frozen entries (non-evaluables stay at p=1.0) and final dispositions."""
    ids = [e["factor_id"] for e in entries]
    if sorted(ids) != sorted(evidence) or len(ids) != len(set(ids)):
        raise ValueError("evidence does not cover exactly the frozen cohort")
    pmap = {i: (evidence[i]["p_value"] if evidence[i]["evaluable"] else 1.0) for i in ids}
    fdr = benjamini_hochberg(pmap, alpha=cp.FDR_ALPHA)
    if fdr["hypothesis_count"] != len(ids):
        raise ValueError("FDR family is not the complete frozen cohort")
    rows = []
    for e in sorted(entries, key=lambda x: x["factor_id"]):
        ev, q = evidence[e["factor_id"]], fdr["q_values"][e["factor_id"]]
        if ev["evaluable"]:
            status, reason = decide(ev["events"], ev["effect"], ev["p_value"], q)
        else:
            status, reason = cp.NOT_EVALUABLE, ev["reason"]
        disc = e["discovery_diagnostics"]
        ret = (ev["effect"] / disc["effect"]) if (ev["effect"] is not None and disc["effect"] and disc["effect"] > 0) else None
        rows.append({**ev, "q_value": q, "fdr_rejected": e["factor_id"] in set(fdr["rejected_factor_ids"]),
                     "status": status, "status_reason": reason,
                     "discovery": disc, "retention_ratio": ret, **cp.LABELS})
    return {"fdr": {"protocol": cp.FDR_PROTOCOL, "alpha": cp.FDR_ALPHA, "hypothesis_count": fdr["hypothesis_count"],
                    "denominator_includes_non_evaluable": True, "rejected_factor_ids": fdr["rejected_factor_ids"],
                    "p_values": pmap, "q_values": fdr["q_values"]}, "rows": rows}


def _rank_key(r: dict):
    return (r["q_value"], r["p_value"], -r["effect"], -r["events"], r["factor_id"])


def rankings(rows: list[dict]) -> dict:
    """Read-only presentation: never feeds back into status or identity."""
    def view(r):
        return {k: r[k] for k in ("factor_id", "family", "horizon", "status", "q_value", "p_value", "effect", "events")}
    strong = sorted((r for r in rows if r["status"] == cp.CONFIRMED_STRONG), key=_rank_key)
    directional = sorted((r for r in rows if r["status"] == cp.CONFIRMED_DIRECTIONAL_ONLY), key=_rank_key)
    return {"schema_version": "alpha_edge_confirmation_rankings_v1", "ordering": "q_asc,p_asc,effect_desc,events_desc,factor_id_asc",
            "read_only": True, "alters_status_or_identity": False, "confirmed_strong": [view(r) for r in strong],
            "confirmed_directional_only_separate_table": [view(r) for r in directional], **cp.LABELS}


def summarize(rows: list[dict]) -> dict:
    def tally(key):
        out: dict = {}
        for r in rows:
            k = str(r[key] if key != "regime_class" else r["discovery"]["regime_class"])
            out.setdefault(k, {s: 0 for s in cp.DISPOSITIONS})[r["status"]] += 1
        return dict(sorted(out.items()))
    by_status = {s: sum(1 for r in rows if r["status"] == s) for s in cp.DISPOSITIONS}
    return {"schema_version": "alpha_edge_confirmation_summary_v1", "denominator": len(rows), "by_status": by_status,
            "by_family": tally("family"), "by_horizon": tally("horizon"), "by_regime_class": tally("regime_class"),
            "advancing_toward_final_holdout": by_status[cp.CONFIRMED_STRONG], **cp.LABELS}


def assert_labels(doc: dict) -> None:
    """No document may claim validation, promotion authority or executable P&L."""
    for key, want in (("VALIDATION_STATUS", "NOT_VALIDATED"), ("PROMOTION_AUTHORITY", "NONE"), ("EXECUTABLE_PNL", False)):
        if key in doc and doc[key] != want:
            raise ValueError(f"{key}={doc[key]!r} violates the Confirmation label contract ({want!r})")
    if str(doc.get("promotion", "NOT_CLAIMED")) != "NOT_CLAIMED" or str(doc.get("final_holdout", "RESERVED_UNCONSUMED")) != "RESERVED_UNCONSUMED":
        raise ValueError("document claims Promotion or Final Holdout consumption")
