"""Strategy robustness scenarios S1-S11 for one Pass-1 DISCOVERED_MODERATE StrategyEdge. A pure function of the frozen
Pass-1 candidate, the discovery-fenced bars and the Pass-1 classification index; it reuses the accepted evaluator
(census.evaluate_cell), signals, simulator and benchmark and creates no Strategy trial or attempt."""

from __future__ import annotations

import json
import math
from dataclasses import dataclass

import numpy as np

import p2_protocol as pp
import p2_pass1  # noqa: F401 - installs sys.path for the accepted Pass-1 modules
import census as ce  # noqa: E402
import edge_registry as er  # noqa: E402
import simulate as sm  # noqa: E402

PASS, FAIL, NA, BLOCKED = "PASS", "FAIL", "NOT_APPLICABLE", "BLOCKED"


class BaselineContradiction(RuntimeError):
    """S1: the accepted evaluator no longer reproduces accepted Pass-1 economics. A system contradiction, never an
    economic rejection: the attempt fails and the run hard-stops."""


@dataclass
class StrategyContext:
    U: object
    meta: dict
    configs: list
    neighbors: list            # Pass-1 adjacent-grid neighbour config indices per config
    moderate: set              # {(config_index, symbol)} Pass-1 DISCOVERED_MODERATE cells
    evaluable: dict            # config_index -> evaluable symbol count (Pass-1 ledger)
    moderate_count: dict       # config_index -> moderate symbol count


def build_context(U, meta: dict, cells, ledger) -> StrategyContext:
    configs = ce.cell_configs(cells)
    moderate, evaluable, mcount = set(), {}, {}
    for r in ledger:
        if r["d"] != "EVALUABLE":
            continue
        evaluable[r["c"]] = evaluable.get(r["c"], 0) + 1
        if r["class"] == pp.PASS1_STRATEGY_CLASS:
            moderate.add((r["c"], r["s"]))
            mcount[r["c"]] = mcount.get(r["c"], 0) + 1
        elif r["class"] not in (None, "DISCOVERED_WEAK"):
            raise ValueError(f"unexpected Pass-1 Strategy class {r['class']!r}")
    return StrategyContext(U, meta, configs, er.neighbor_map(configs), moderate, evaluable, mcount)


def _jsonable(x):
    return json.loads(json.dumps(x, default=lambda o: o.item()))


def _finite(*vals) -> bool:
    return all(isinstance(v, (int, float)) and math.isfinite(v) for v in vals)


def stressed_net_alpha(sd, d: np.ndarray, s: int, mult: int) -> tuple[float, float, float]:
    """(strategy net, matched-benchmark net, alpha) with EVERY commission and execution-slippage input scaled by mult;
    the matched passive benchmark pays the same stressed costs."""
    kw = {"commission_bps": sm.COMMISSION_BPS * mult, "slippage_bps": sm.SLIPPAGE_BPS * mult}
    w = slice(s + 1, None)
    so = sm.simulate(sd.hm, sd.lm, sd.cm, d, s, **kw)
    bo = sm.simulate(sd.hm, sd.lm, sd.cm, sm.benchmark_d(sd.n, s), s, **kw)
    net, bench = float(so.net[w].sum()), float(bo.net[w].sum())
    return net, bench, net - bench


def delayed_decisions(d: np.ndarray, sessions: int = pp.DELAY_SESSIONS) -> np.ndarray:
    """The candidate's own completed-bar decision stream shifted later by whole completed sessions (no same-bar fill)."""
    out = np.zeros_like(d)
    out[sessions:] = d[:-sessions]
    return out


def risk_bar(net: np.ndarray) -> dict:
    """MAIN risk bar on the window net P&L stream over the accepted initial capital."""
    cum = np.concatenate([[0.0], np.cumsum(net)])
    max_dd = float((np.maximum.accumulate(cum) - cum).max())
    k = pp.ROLLING_SESSIONS
    worst = float((cum[k:] - cum[:-k]).min()) if len(net) >= k else None
    return {"max_drawdown_usd": max_dd, "max_drawdown_frac_of_capital": max_dd / pp.INITIAL_CAPITAL_USD,
            "worst_rolling_net_usd": worst,
            "worst_rolling_return": None if worst is None else worst / pp.INITIAL_CAPITAL_USD}


def year_scenarios(year_alpha: dict) -> tuple[dict, dict]:
    """S6 (positive calendar years) and S7 (leave-best-year-out) from per-year alpha; absent years contribute 0."""
    per = {y: float(year_alpha.get(str(y), 0.0)) for y in pp.YEARS}
    positive = [y for y in pp.YEARS if per[y] > 0.0]
    s6 = {"status": PASS if len(positive) >= pp.MIN_POSITIVE_YEARS else FAIL, "positive_years": len(positive),
          "required": pp.MIN_POSITIVE_YEARS, "year_alpha_usd": {str(y): per[y] for y in pp.YEARS}}
    if not positive:
        return s6, {"status": BLOCKED, "reason": "no year with positive alpha contribution"}
    best = max(positive, key=lambda y: (per[y], -y))  # largest contribution; earliest year on ties
    remaining = sum(per[y] for y in pp.YEARS if y != best)
    return s6, {"status": PASS if remaining > 0.0 else FAIL, "best_year": best, "best_year_alpha_usd": per[best],
                "remaining_alpha_usd": remaining}


def trade_scenario(trade_count: int) -> dict:
    return {"status": PASS if trade_count >= pp.MIN_TRADES else FAIL, "trade_count": trade_count,
            "required": pp.MIN_TRADES}


def stress_scenario(net: float, bench: float, alpha: float, **extra) -> dict:
    """PASS needs BOTH stressed net P&L > 0 AND stressed matched-benchmark alpha > 0."""
    return {"status": PASS if (_finite(net, alpha) and net > 0.0 and alpha > 0.0) else FAIL, **extra,
            "net_pnl_usd": net, "benchmark_net_pnl_usd": bench, "alpha_usd": alpha}


def regime_scenario(share, regime_alpha: dict) -> dict:
    if share is None:
        return {"status": BLOCKED, "reason": "no positive regime bucket", "regime_alpha_usd": regime_alpha}
    return {"status": PASS if share <= pp.MAX_REGIME_CONCENTRATION else FAIL, "share": share,
            "max": pp.MAX_REGIME_CONCENTRATION, "regime_alpha_usd": regime_alpha}


def neighbor_scenario(neighbors: list, symbol: str, moderate: set) -> dict:
    """Support = the same-symbol neighbour cell is a Pass-1 DISCOVERED_MODERATE StrategyEdge; no neighbours => N/A."""
    if not neighbors:
        return {"status": NA, "n_neighbors": 0}
    n_mod = sum((j, symbol) in moderate for j in neighbors)
    return {"status": PASS if n_mod / len(neighbors) >= pp.MIN_NEIGHBOR_SHARE else FAIL, "n_neighbors": len(neighbors),
            "n_moderate_neighbors": n_mod, "share": n_mod / len(neighbors), "min": pp.MIN_NEIGHBOR_SHARE}


def risk_scenario(net: np.ndarray) -> dict:
    rb = risk_bar(net)
    ok = rb["max_drawdown_frac_of_capital"] <= pp.MAX_DRAWDOWN_FRAC and rb["worst_rolling_return"] is not None \
        and rb["worst_rolling_return"] >= pp.MIN_ROLLING_RETURN
    return {"status": PASS if ok else FAIL, **rb, "max_drawdown_frac_limit": pp.MAX_DRAWDOWN_FRAC,
            "rolling_return_limit": pp.MIN_ROLLING_RETURN, "initial_capital_usd": pp.INITIAL_CAPITAL_USD}


def replication_scenario(n_evaluable: int, n_moderate: int) -> dict:
    if n_moderate == 0:
        return {"status": BLOCKED, "class": "ZERO_REPLICATION_CONTRADICTION", "evaluable_symbols": n_evaluable}
    cls = ("SYMBOL_SPECIFIC" if n_moderate == 1 else
           "BROADLY_REPLICATED" if n_moderate / n_evaluable >= pp.BROADLY_SHARE else "CLUSTER_REPLICATED")
    return {"status": "CLASSIFIED", "class": cls, "moderate_symbols": n_moderate, "evaluable_symbols": n_evaluable}


def strategy_verdict(sc_: dict) -> tuple[list, list, str]:
    """(failed gates in S-order, blocked gates, verdict): any applicable FAIL is conclusive (REJECTED); BLOCKED only
    when nothing failed but required proof is unavailable; NOT_APPLICABLE scenarios are excluded."""
    failed = [g for g in pp.STRATEGY_HARD_GATES if sc_[g]["status"] == FAIL]
    blocked = [g for g in pp.STRATEGY_HARD_GATES if sc_[g]["status"] == BLOCKED]
    if sc_["S11"]["status"] == BLOCKED:
        blocked.append("S11")
    v = pp.VERDICTS_STRATEGY
    return failed, blocked, (v[1] if failed else v[2] if blocked else v[0])


def evaluate_strategy(sc: StrategyContext, cand: dict, edge_metrics: dict) -> dict:
    """One robustness record. cand: cohort record (family, params, symbol, config_index). edge_metrics: Pass-1 registry
    metrics of the candidate. Raises BaselineContradiction when S1 cannot be reproduced."""
    U, sym = sc.U, cand["symbol"]
    got = ce.evaluate_cell(U, {"family": cand["family"], "params": cand["params"]}, sym, sc.meta)
    if got["d"] != "EVALUABLE" or json.dumps(_jsonable(got["m"]), sort_keys=True) != json.dumps(edge_metrics, sort_keys=True):
        raise BaselineContradiction(f"{cand['trial_id']}: accepted evaluator no longer reproduces the Pass-1 metrics")
    m = got["m"]
    sd = U.sd[sym]
    sig = U.build(sym, cand["family"], cand["params"])
    s = sig.s
    w = slice(s + 1, None)
    so = sm.simulate(sd.hm, sd.lm, sd.cm, sig.d, s)
    base_bench = float(ce.bench(sd, s).net[w].sum())
    net_w = so.net[w]
    if float(net_w.sum()) != m["net_pnl_usd"] or base_bench != m["benchmark_net_pnl_usd"] or \
            risk_bar(net_w)["max_drawdown_usd"] != m["max_drawdown_usd"]:
        raise BaselineContradiction(f"{cand['trial_id']}: net P&L stream does not reproduce the accepted metrics")

    sc_: dict = {"S1": {"status": PASS, "metrics_identical": True}, "S2": trade_scenario(m["trade_count"])}
    for key, mult in (("S3", pp.STRESS_2X), ("S4", pp.STRESS_3X)):
        sc_[key] = stress_scenario(*stressed_net_alpha(sd, sig.d, s, mult), multiplier=mult)
    net_d = float(sm.simulate(sd.hm, sd.lm, sd.cm, delayed_decisions(sig.d), s).net[w].sum())
    sc_["S5"] = stress_scenario(net_d, base_bench, net_d - base_bench, delay_sessions=pp.DELAY_SESSIONS)
    sc_["S6"], sc_["S7"] = year_scenarios(m["year_alpha_usd"])
    sc_["S8"] = regime_scenario(m["regime_concentration_share"], m["regime_alpha_usd"])
    ci = cand["config_index"]
    sc_["S9"] = neighbor_scenario(sc.neighbors[ci], sym, sc.moderate)
    sc_["S10"] = risk_scenario(net_w)
    sc_["S11"] = replication_scenario(sc.evaluable[ci], sc.moderate_count.get(ci, 0))
    failed, blocked, verdict = strategy_verdict(sc_)
    return {"kind": pp.KIND_STRATEGY, "candidate_id": cand["candidate_id"], "edge_id": cand["edge_id"],
            "trial_id": cand["trial_id"], "config_id": cand["config_id"], "family": cand["family"],
            "params": cand["params"], "symbol": sym, "scenarios": sc_, "failed_gates": failed,
            "blocked_gates": blocked, "not_applicable": [g for g in pp.STRATEGY_HARD_GATES if sc_[g]["status"] == NA],
            "verdict": verdict, "pass_3x_cost_diagnostic": sc_["S4"]["status"] == PASS,
            "replication_class": sc_["S11"]["class"],
            "rank_fields": {"passes_3x": sc_["S4"]["status"] == PASS, "alpha_2x": sc_["S3"]["alpha_usd"],
                            "alpha_delayed": sc_["S5"]["alpha_usd"],
                            "leave_best_year_out_alpha": sc_["S7"].get("remaining_alpha_usd"),
                            "positive_years": sc_["S6"]["positive_years"],
                            "max_drawdown_usd": sc_["S10"]["max_drawdown_usd"]},
            **pp.LABELS}


def rank_strategy_survivors(records: list[dict]) -> list[dict]:
    """READ-ONLY lexicographic ranking; never alters pass/fail or identity."""
    surv = [r for r in records if r["verdict"] == pp.VERDICTS_STRATEGY[0]]

    def key(r):
        f = r["rank_fields"]
        return (not f["passes_3x"], -f["alpha_2x"], -f["alpha_delayed"], -f["leave_best_year_out_alpha"],
                -f["positive_years"], f["max_drawdown_usd"], r["trial_id"])
    return sorted(surv, key=key)
