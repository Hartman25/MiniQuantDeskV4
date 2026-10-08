"""Causal signed-position simulator for Census-02. d[t] is decided after completed bar t and fills on bar t+1 at the
rust_conservative_bar_range_v1 price (short entry = SELL at low-slip, cover = BUY at high+slip). qty = sign*floor(budget /
close of the signal bar), constant per run. Shorts accrue an explicit borrow fee; there is no default fee."""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import c2_protocol as pr  # noqa: E402,F401
import simulate as s1  # noqa: E402  (Census-01 constants, fill vector and metrics; its simulate() is bool-only and unused)
from c2_borrow import BorrowRefusal  # noqa: E402

SimOut = s1.SimOut


def _require_price_micros(hm, lm, cm) -> None:
    """Executable P&L only from integer-micro OHLC bars. A float label series (fwd_ret) is refused."""
    for a in (hm, lm, cm):
        if not isinstance(a, np.ndarray) or a.dtype != np.int64:
            raise TypeError("simulate_signed accepts only int64 micro-price bar arrays (labels are never executable P&L)")
    if len(cm) < 3 or not (np.all(lm > 0) and np.all(cm > 0) and np.all(hm >= cm) and np.all(cm >= lm)):
        raise TypeError("arrays are not a valid OHLC bar shape (high >= close >= low > 0): not executable price bars")


def _require_cost_overrides(commission_bps, slippage_bps) -> None:
    """A malformed override must never produce favorable economics: refuse instead of coercing."""
    if commission_bps is not None and (isinstance(commission_bps, bool) or not isinstance(commission_bps, (int, float, np.integer, np.floating))
                                       or not np.isfinite(commission_bps) or commission_bps < 0):
        raise ValueError("commission_bps must be a finite non-negative number")
    if slippage_bps is not None and (isinstance(slippage_bps, (bool, np.bool_)) or not isinstance(slippage_bps, (int, np.integer))
                                     or slippage_bps < 0):
        raise ValueError("slippage_bps must be a non-negative integer number of bps")


def simulate_signed(hm, lm, cm, d: np.ndarray, s: int, *, borrow_fee_bps_annual: float | None = None,
                    commission_bps: float | None = None, slippage_bps: int | None = None) -> SimOut:
    _require_price_micros(hm, lm, cm)
    if d.dtype != np.int8 or np.any(np.abs(d) > 1):
        raise TypeError("signed desired-position series must be int8 in {-1,0,+1}")
    _require_cost_overrides(commission_bps, slippage_bps)
    fee = borrow_fee_bps_annual
    if (fee is not None and (isinstance(fee, bool) or not np.isfinite(fee) or fee < 0)) or (np.any(d < 0) and fee is None):
        raise BorrowRefusal("a short position needs an explicit finite non-negative borrow fee (no default, never zero by omission)")
    commission = s1.COMMISSION_BPS if commission_bps is None else commission_bps
    n = len(cm)
    h = np.zeros(n, np.int8)
    h[1:] = d[:-1]                                   # position after the fill on bar b is the decision after bar b-1
    prev = np.zeros(n, np.int8)
    prev[1:] = h[:-1]
    entry = (h != 0) & (h != prev)
    run_id = np.cumsum(entry) - 1
    e_idx = np.flatnonzero(entry)
    qty_run = h[e_idx].astype(np.int64) * (s1.BUDGET_MICROS // cm[e_idx - 1]) if len(e_idx) else np.zeros(0, np.int64)
    rid = np.maximum(run_id, 0)
    q_after = np.where(h != 0, qty_run[rid], 0).astype(np.int64) if len(e_idx) else np.zeros(n, np.int64)
    q_before = np.zeros(n, np.int64)
    q_before[1:] = q_after[:-1]
    buy, sell = s1.conservative_fills(hm, lm, cm, slippage_bps)
    dc = np.zeros(n, np.int64)
    dc[1:] = cm[1:] - cm[:-1]
    delta = q_after - q_before
    adv_ps = np.where(delta > 0, buy - cm, np.where(delta < 0, cm - sell, 0))
    px = np.where(delta > 0, buy, np.where(delta < 0, sell, 0))
    sb, sa = np.sign(q_before), np.sign(q_after)
    exit_sh = np.where((sb != 0) & (sa != sb), np.abs(q_before), 0)
    entry_sh = np.where((sa != 0) & (sa != sb), np.abs(q_after), 0)
    gross_m = q_before * dc
    borrow_m = np.zeros(n)
    if borrow_fee_bps_annual:
        pc = np.zeros(n, np.int64)
        pc[1:] = cm[:-1]
        borrow_m = np.where(q_before < 0, -q_before * pc, 0).astype(np.float64) * (borrow_fee_bps_annual / 10_000.0 / s1.ANNUALIZATION)
    cost_exit_m = exit_sh * adv_ps + exit_sh * px * (commission / 10_000.0)
    cost_entry_m = entry_sh * adv_ps + entry_sh * px * (commission / 10_000.0)
    cost_m = cost_exit_m + cost_entry_m + borrow_m
    gross = gross_m / 1e6
    cost = cost_m.astype(np.float64) / 1e6
    net = gross - cost
    entries = np.flatnonzero(entry & (q_after != 0))
    exits = np.flatnonzero(exit_sh > 0)
    if len(exits):
        g_bor = np.concatenate([[0.0], np.cumsum((gross_m - borrow_m) / 1e6)])
        ce, cx = cost_entry_m.astype(np.float64) / 1e6, cost_exit_m.astype(np.float64) / 1e6
        ent_for_exit = entries[np.searchsorted(entries, exits, side="left") - 1]   # the run's own entry is strictly earlier
        run_pnl = (g_bor[exits + 1] - g_bor[ent_for_exit + 1]) - ce[ent_for_exit] - cx[exits]
    else:
        run_pnl = np.zeros(0)
    traded = (entry_sh * px + exit_sh * px).astype(np.float64)
    return SimOut(net=net, gross=gross, cost=cost, held_before=q_before != 0, entries=entries, exits=exits,
                  run_pnl=run_pnl, notional_usd=float(traded.sum()) / 1e6, start=s + 1)


def benchmark_hold(n: int, s: int, direction: int) -> np.ndarray:
    """Passive constant-direction hold from the first defined bar (direction +1 or -1)."""
    if direction not in (-1, 1):
        raise ValueError("direction must be +1 or -1")
    d = np.zeros(n, np.int8)
    d[s:] = direction
    return d


BENCHMARK_ROLES, BENCHMARK_RULES = pr.BENCHMARK_ROLES, pr.BENCHMARK_RULES
_HOLD_DIRECTION = {"passive_long_hold": 1, "passive_short_hold": -1}


def benchmark_direction(side: str, d: np.ndarray) -> int:
    """Single-direction passive benchmark of a short-only strategy: a passive SHORT, never a long hold. A long/short
    strategy has no single direction and is refused here; its holds are diagnostics (see BENCHMARK_ROLES)."""
    if side == "short":
        return -1
    raise ValueError(f"{side!r} has no single-direction passive benchmark")


def qualifies(side: str, m: dict, rule: str) -> bool:
    """Proposed qualification over a cell's recorded benchmark metrics. Fails closed on an unknown side/rule or on a
    missing record the rule requires. A long/short cell never consults a passive hold."""
    if side not in BENCHMARK_ROLES:
        raise ValueError(f"unknown side {side!r}")
    if rule not in BENCHMARK_RULES:
        raise ValueError(f"unknown benchmark rule {rule!r}")
    net_positive = m["cash_zero"]["net_pnl_usd"] > 0
    if rule == "NET_POSITIVE_ONLY_ALL_SIDES" or side == "long_short":
        return bool(net_positive)
    return bool(net_positive and m["passive_short_hold"]["net_alpha_usd"] > 0)


def ssr_flag(lm, cm, fill_bar: int) -> bool:
    """Daily-bar Rule 201 hazard for a short fill on `fill_bar`: a >=10% decline from the prior close on the prior bar
    (restriction runs through this bar) or on this bar itself. Recorded only; whether to act on it is an operator decision."""
    def trig(b):
        return b >= 1 and lm[b] * 10 <= cm[b - 1] * 9
    return bool(trig(fill_bar - 1) or trig(fill_bar))


def _cash_out(so: SimOut) -> SimOut:
    z = np.zeros_like(so.net)
    return SimOut(net=z, gross=z, cost=z, held_before=np.zeros_like(so.held_before), entries=so.entries[:0],
                  exits=so.exits[:0], run_pnl=so.run_pnl[:0], notional_usd=0.0, start=so.start)


def evaluate_short_cell(sd, config: dict, evidence_class: str, *, borrow_fee_bps_annual: float | None) -> dict:
    """One short-bearing cell. A non-executable evidence class NEVER reaches the simulator: it returns a hypothesis-only
    disposition with no P&L fields. Benchmarks are recorded per BENCHMARK_ROLES; the qualification rule is an operator decision."""
    import c2_borrow as bw
    import c2_signals as sg
    if evidence_class not in bw.EXECUTABLE_CLASSES:
        return {"d": "HYPOTHESIS_ONLY_NO_EXECUTABLE_EVALUATION", "m": None, "evidence_class": evidence_class,
                "executable_pnl": False}
    bw.require_executable(evidence_class)
    if sd.regime is None:
        raise RuntimeError("regime attribution unavailable: fail closed")
    sig = sg.build(sd, config["family"], config["params"])
    if sig is None:
        return {"d": "NON_EVALUABLE_SIGNAL_UNDEFINED_INSUFFICIENT_HISTORY", "m": None, "evidence_class": evidence_class,
                "executable_pnl": False}
    so = simulate_signed(sd.hm, sd.lm, sd.cm, sig.d, sig.s, borrow_fee_bps_annual=borrow_fee_bps_annual)
    kw = dict(capital_usd=s1.CAPITAL_USD, budget_usd=s1.BUDGET_USD, window_start=sig.s + 1)
    recs = {"cash_zero": s1.metrics(so, _cash_out(so), sd.years, sd.regime, **kw)}
    for name in BENCHMARK_ROLES[config["side"]]:
        if name == "cash_zero":
            continue
        hold = simulate_signed(sd.hm, sd.lm, sd.cm, benchmark_hold(sd.n, sig.s, _HOLD_DIRECTION[name]), sig.s,
                               borrow_fee_bps_annual=borrow_fee_bps_annual)
        recs[name] = s1.metrics(so, hold, sd.years, sd.regime, **kw)
    # SSR is FLAG_ONLY in Discovery: a daily-bar hazard flag per short entry fill, never an input to the position or the fill.
    short_entries = [int(e) for e in so.entries if sig.d[e - 1] < 0]
    ssr = {"basis": "daily_bar_rule201_possible_hazard_flag_only_intraday_sequence_unknown", "short_entries": len(short_entries),
           "ssr_hazard_entries": sum(1 for e in short_entries if ssr_flag(sd.lm, sd.cm, e))}
    return {"d": "EVALUABLE", "m": recs, "evidence_class": evidence_class, "executable_pnl": True,
            "benchmark_roles": dict(BENCHMARK_ROLES[config["side"]]), "signal_start_bar": int(sig.s), "ssr": ssr}
