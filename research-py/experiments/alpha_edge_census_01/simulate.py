"""Vectorised causal long/flat simulator. d[t] is decided after completed bar t and fills on bar t+1 at the
rust_conservative_bar_range_v1 price (integer micros). qty = floor(budget / close of the signal bar), constant
per run, no compounding. Open runs are marked to the last close with no liquidation cost."""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np

BUDGET_MICROS = 10_000 * 1_000_000
CAPITAL_USD = 100_000.0
BUDGET_USD = 10_000.0
COMMISSION_BPS = 10.0
SLIPPAGE_BPS = 5
VOL_MULT_BPS = 0
ANNUALIZATION = 252


def conservative_fills(hm: np.ndarray, lm: np.ndarray, cm: np.ndarray, slippage_bps: int | None = None):
    """Vectorised int64 mirror of execution_pricing.conservative_fill_price_micros (BUY at high, SELL at low)."""
    spread = (hm - lm) * 10_000 // cm
    eff = (SLIPPAGE_BPS if slippage_bps is None else slippage_bps) + spread * VOL_MULT_BPS // 10_000
    buy = hm + hm * eff // 10_000
    sell = np.maximum(lm - lm * eff // 10_000, 0)
    return buy, sell


@dataclass
class SimOut:
    net: np.ndarray        # per-bar net pnl USD (len n)
    gross: np.ndarray
    cost: np.ndarray       # per-bar adverse-fill + commission USD
    held_before: np.ndarray
    entries: np.ndarray    # bar indices of executed entries
    exits: np.ndarray      # bar indices of executed exits (closed runs)
    run_pnl: np.ndarray    # closed-run net pnl USD
    notional_usd: float
    start: int             # first possible fill bar (s+1)


def simulate(hm, lm, cm, d: np.ndarray, s: int, *, commission_bps: float | None = None,
             slippage_bps: int | None = None) -> SimOut:
    """Run the position series. h[b] = d[b-1] is the position after the fill on bar b. The cost overrides default to
    the accepted census constants (None); a robustness stress passes explicit multiples."""
    commission = COMMISSION_BPS if commission_bps is None else commission_bps
    n = len(cm)
    h = np.zeros(n, bool)
    h[1:] = d[:-1]
    prev = np.zeros(n, bool)
    prev[1:] = h[:-1]
    entry = h & ~prev
    exit_ = ~h & prev
    run_id = np.cumsum(entry) - 1
    e_idx = np.flatnonzero(entry)
    qty_run = BUDGET_MICROS // cm[e_idx - 1] if len(e_idx) else np.zeros(0, np.int64)
    held_after = h & (qty_run[np.maximum(run_id, 0)] > 0) if len(e_idx) else np.zeros(n, bool)
    q_after = np.where(held_after, qty_run[np.maximum(run_id, 0)], 0).astype(np.int64) if len(e_idx) else np.zeros(n, np.int64)
    q_before = np.zeros(n, np.int64)
    q_before[1:] = q_after[:-1]
    buy, sell = conservative_fills(hm, lm, cm, slippage_bps)
    dc = np.zeros(n, np.int64)
    dc[1:] = cm[1:] - cm[:-1]
    gross_m = q_before * dc
    ent = entry & (q_after > 0)
    ext = exit_ & (q_before > 0)
    adverse_m = np.where(ent, q_after * (buy - cm), 0) + np.where(ext, q_before * (cm - sell), 0)
    traded_m = np.where(ent, q_after * buy, 0) + np.where(ext, q_before * sell, 0)
    comm = traded_m.astype(np.float64) * (commission / 10_000.0)
    gross = gross_m / 1e6
    cost = (adverse_m.astype(np.float64) + comm) / 1e6
    net = gross - cost
    entries = np.flatnonzero(ent)
    exits = np.flatnonzero(ext)
    if len(exits):
        run_pnl = np.empty(len(exits))
        csum = np.concatenate([[0.0], np.cumsum(net)])
        # closed run k pairs with the entry preceding its exit among executed entries
        ent_for_exit = entries[np.searchsorted(entries, exits) - 1]
        run_pnl = csum[exits + 1] - csum[ent_for_exit]
    else:
        run_pnl = np.zeros(0)
    return SimOut(net=net, gross=gross, cost=cost, held_before=q_before > 0, entries=entries, exits=exits,
                  run_pnl=run_pnl, notional_usd=float(traded_m.sum()) / 1e6, start=s + 1)


def benchmark_d(n: int, s: int) -> np.ndarray:
    d = np.zeros(n, bool)
    d[s:] = True
    return d


def _bucket_share(values: np.ndarray, keys: np.ndarray):
    out: dict = {}
    for k in np.unique(keys):
        out[int(k)] = float(values[keys == k].sum())
    pos = {k: v for k, v in out.items() if v > 0}
    share = (max(pos.values()) / sum(pos.values())) if pos else None
    return out, share


def _sharpe_stats(daily: np.ndarray) -> dict:
    n = len(daily)
    if n < 3:
        return {"observations": n, "sharpe_per_period": None, "sharpe_annualized": None, "skewness": None, "kurtosis_raw": None}
    m = float(daily.mean())
    sd = float(daily.std(ddof=1))
    if not (sd > 0.0) or not math.isfinite(sd):
        return {"observations": n, "sharpe_per_period": None, "sharpe_annualized": None, "skewness": None, "kurtosis_raw": None}
    z = (daily - m) / float(daily.std(ddof=0))
    sp = m / sd
    return {"observations": n, "sharpe_per_period": sp, "sharpe_annualized": sp * math.sqrt(ANNUALIZATION),
            "skewness": float((z ** 3).mean()), "kurtosis_raw": float((z ** 4).mean())}


def metrics(strat: SimOut, bench: SimOut, years: np.ndarray, regime: np.ndarray, *, capital_usd: float,
            budget_usd: float, window_start: int) -> dict:
    """Economics over bars [window_start, end). years/regime are aligned with the bar axis."""
    w = slice(window_start, None)
    net, bnet = strat.net[w], bench.net[w]
    alpha_bar = net - bnet
    nwin = len(net)
    net_total = float(net.sum())
    b_total = float(bnet.sum())
    alpha = net_total - b_total
    cum = np.concatenate([[0.0], np.cumsum(net)])
    max_dd = float((np.maximum.accumulate(cum) - cum).max())
    cost_s, cost_b = float(strat.cost[w].sum()), float(bench.cost[w].sum())
    year_pnl, year_share = _bucket_share(alpha_bar, years[w])
    reg_pnl, reg_share = _bucket_share(alpha_bar, regime[w].astype(np.int64))
    run = strat.run_pnl
    trades = int(len(strat.entries))
    n_years = nwin / ANNUALIZATION if nwin else 0.0
    sh = _sharpe_stats(net / capital_usd)
    return {
        "window_bars": nwin, "net_pnl_usd": net_total, "gross_pnl_usd": float(strat.gross[w].sum()), "cost_usd": cost_s,
        "benchmark_net_pnl_usd": b_total, "benchmark_cost_usd": cost_b, "net_alpha_usd": alpha,
        "net_return": net_total / capital_usd, "benchmark_return": b_total / capital_usd, "net_alpha_return": alpha / capital_usd,
        **sh, "max_drawdown_usd": max_dd, "max_drawdown_frac_of_budget": max_dd / budget_usd,
        "turnover": strat.notional_usd / budget_usd, "round_trips": int(len(run)), "trade_count": trades,
        "round_trips_per_year": (len(run) / n_years) if n_years > 0 else None,
        "exposure": float(strat.held_before[w].mean()) if nwin else None,
        "wins": int((run > 0).sum()), "losses": int((run <= 0).sum()),
        "win_loss_mean_win_usd": float(run[run > 0].mean()) if (run > 0).any() else None,
        "win_loss_mean_loss_usd": float(run[run <= 0].mean()) if (run <= 0).any() else None,
        "year_alpha_usd": {str(k): v for k, v in year_pnl.items()}, "year_concentration_share": year_share,
        "regime_alpha_usd": {{0: "RISK_OFF", 1: "RISK_ON", 2: "UNKNOWN"}[k]: v for k, v in reg_pnl.items()},
        "regime_concentration_share": reg_share,
        "cost_fragile": bool(alpha > 0 and (alpha - (cost_s - cost_b)) <= 0),
    }
