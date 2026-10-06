"""ConditionalEdge robustness scenarios C1-C8 for one Pass-1 DISCOVERED_STRONG factor. Diagnostic relationship evidence
only: fwd_ret is a diagnostic label, never executable P&L. Reuses the accepted factor frame, diagnostics, registered
reconstruction and neighbour authority; creates no FactorSpec and no factor evaluation attempt."""

from __future__ import annotations

import json
import math
from dataclasses import dataclass

import numpy as np
import pandas as pd

import p2_protocol as pp
import p2_pass1  # noqa: F401 - installs sys.path for the accepted Pass-1 modules
import conditional as cd  # noqa: E402
import edge_registry as er  # noqa: E402

PASS, FAIL, NA, BLOCKED = "PASS", "FAIL", "NOT_APPLICABLE", "BLOCKED"


class FactorContradiction(RuntimeError):
    """C1: accepted Pass-1 factor evidence cannot be reproduced. A system contradiction, never an economic rejection."""


@dataclass
class ConditionalContext:
    U: object
    ctx: dict                  # accepted population context (universe + data provenance identities)
    conditions: dict           # condition_id -> condition
    cond_index: dict           # condition_id -> index in the 219-condition grammar
    neighbors: list            # Pass-1 conditional adjacency (semantic params only)
    strong: set                # {(condition_index, horizon)} Pass-1 DISCOVERED_STRONG
    factor_ledger: dict        # (condition_id, horizon) -> Pass-1 factor ledger row
    fdr: dict                  # accepted complete-family FDR report
    cache: object              # accepted permutation cache


def build_context(U, ctx: dict, conditions: list[dict], factor_ledger: list[dict], fdr: dict) -> ConditionalContext:
    idx = {c["condition_id"]: i for i, c in enumerate(conditions)}
    ledger = {(r["condition_id"], r["horizon"]): r for r in factor_ledger}
    strong = {(idx[r["condition_id"]], r["horizon"]) for r in factor_ledger if r["class"] == pp.PASS1_CONDITIONAL_CLASS}
    return ConditionalContext(U, ctx, {c["condition_id"]: c for c in conditions}, idx,
                              er.conditional_neighbor_map(conditions), strong, ledger, fdr, cd.PermutationCache())


class RawRows:
    """The accepted observation rows (same order as cd.build_frame) with raw forward returns for slice re-estimation."""

    def __init__(self, U, cond: dict, h: int):
        parts = []
        for sym in U.symbols:
            sd = U.sd[sym]
            sig = cd.condition_sig(U, sym, cond)
            if sig is None:
                continue
            idx = np.arange(sig.s, sd.n - h)
            if idx.size == 0:
                continue
            parts.append(pd.DataFrame({"symbol": sym, "period": sd.end_iso[idx], "ev": sig.cond[idx],
                                       "ret": sd.c[idx + h] / sd.c[idx] - 1.0, "year": sd.years[idx],
                                       "regime": sd.regime[idx]}))
        df = pd.concat(parts, ignore_index=True).sort_values(["period", "symbol"], kind="mergesort").reset_index(drop=True)
        self.symbol = df["symbol"].to_numpy()
        self.period = df["period"].to_numpy()
        self.ev = df["ev"].to_numpy(bool)
        self.ret = df["ret"].to_numpy(np.float64)
        self.year = df["year"].to_numpy(np.int64)
        self.regime = df["regime"].to_numpy()
        self.n = len(df)
        self.sym_rows = {s: np.flatnonzero(self.symbol == s) for s in sorted(set(self.symbol.tolist()))}
        self.label = self.effect_labels(np.ones(self.n, bool))

    def effect_labels(self, mask: np.ndarray) -> np.ndarray:
        """Per-row (return - same-symbol baseline over the slice's rows); NaN outside the slice."""
        lab = np.full(self.n, np.nan)
        for rows in self.sym_rows.values():
            r = rows[mask[rows]]
            if r.size:
                lab[r] = self.ret[r] - self.ret[r].mean()
        return lab

    def effect(self, mask: np.ndarray) -> tuple[float | None, int]:
        """E(S): mean over event rows of the slice of (return - symbol baseline over the slice); (None, 0) if no events."""
        ev = mask & self.ev
        n = int(ev.sum())
        return (float(self.effect_labels(mask)[ev].mean()), n) if n else (None, 0)


def _pos(v) -> bool:
    return v is not None and math.isfinite(v) and v > 0.0


def _canon(x) -> str:
    return json.dumps(x, sort_keys=True, separators=(",", ":"))


def c1_baseline(cc: ConditionalContext, cand: dict, att: dict, rec1: dict, edge: dict, frame, aux, raw: RawRows,
                cond: dict, h: int) -> dict:
    """Reproduce the accepted factor evidence; any disagreement is a FactorContradiction."""
    spec = cd.factor_spec(cond, h, cc.ctx)
    fid = spec.compute_factor_id()
    eid = cd.expected_evaluation_id(cond, h, cc.ctx)
    if fid != cand["factor_id"] or att["factor_id"] != fid or att["evaluation_id"] != eid or rec1["evaluation_id"] != eid:
        raise FactorContradiction(f"{fid}: factor/evaluation identity changed")
    if not cd._record_binds(rec1, fid, att) or att["status"] != "succeeded":  # noqa: SLF001 - accepted binding check
        raise FactorContradiction(f"{fid}: persisted record does not bind the authoritative attempt")
    rec2 = cd.reconstruct_record(cc.U, cond, h, spec, att, cc.cache)
    if _canon(rec2) != _canon(rec1):
        raise FactorContradiction(f"{fid}: reconstructed evidence differs from the accepted persisted record")
    ev = rec1["events"]
    if ev != edge["events"] or rec1["pvalue"] != edge["pvalue"]:
        raise FactorContradiction(f"{fid}: accepted record differs from the Edge Registry V3 record")
    full_effect, full_n = raw.effect(np.ones(raw.n, bool))
    if full_n != ev["event_count"] or full_effect != ev["direction_adjusted_effect"] or \
            not np.array_equal(raw.label, frame["label_fwd_ret"].to_numpy()) or \
            not np.array_equal(raw.symbol, frame["symbol"].to_numpy()) or \
            not np.array_equal(raw.ev, frame["factor_value"].to_numpy() == 1.0) or \
            not np.array_equal(raw.year, aux["year"].to_numpy()):
        raise FactorContradiction(f"{fid}: slice estimator does not reproduce the accepted full-frame effect")
    fdr = cc.fdr
    q = (fdr.get("q_values") or {}).get(fid)
    p = rec1["pvalue"]["p_value"]
    if fdr.get("status") != "complete" or fid not in fdr["declared_factor_ids"] or q != edge["q_value"] or \
            not (isinstance(q, float) and q <= pp.FDR_ALPHA) or not (p <= pp.C_HORIZON_P_MAX):
        raise FactorContradiction(f"{fid}: accepted p/q/FDR provenance no longer binds")
    return {"status": PASS, "factor_id": fid, "evaluation_id": eid, "event_count": ev["event_count"],
            "direction_adjusted_effect": ev["direction_adjusted_effect"], "p_value": p, "q_value": q,
            "observations_content_sha256": rec1["observations_content_sha256"], "fdr_status": fdr["status"]}


def evaluate_conditional(cc: ConditionalContext, cand: dict, att: dict, rec1: dict, edge: dict) -> dict:
    cond = cc.conditions[cand["condition_id"]]
    h = cand["horizon"]
    frame, aux = cd.build_frame(cc.U, cond, h)
    raw = RawRows(cc.U, cond, h)
    sc: dict = {"C1": c1_baseline(cc, cand, att, rec1, edge, frame, aux, raw, cond, h)}
    ev = rec1["events"]

    per_year, positive_years = {}, 0
    for y in pp.YEARS:
        e, n = raw.effect(raw.year == y)
        per_year[str(y)] = {"effect": e, "events": n}
        positive_years += _pos(e)
    sc["C2"] = {"status": PASS if positive_years >= pp.MIN_POSITIVE_YEARS else FAIL, "positive_years": positive_years,
                "required": pp.MIN_POSITIVE_YEARS, "per_year": per_year}

    contrib = {y: float(raw.label[raw.ev & (raw.year == y)].sum()) for y in pp.YEARS}
    positive = [y for y in pp.YEARS if contrib[y] > 0.0]
    if not positive:
        sc["C3"] = {"status": BLOCKED, "reason": "no year with positive aggregate effect contribution"}
        lbyo_effect = None
    else:
        best = max(positive, key=lambda y: (contrib[y], -y))
        lbyo_effect, n_rem = raw.effect(raw.year != best)
        sc["C3"] = {"status": PASS if (_pos(lbyo_effect) and n_rem >= pp.C_MIN_EVENTS) else FAIL, "best_year": best,
                    "best_year_contribution": contrib[best], "remaining_effect": lbyo_effect, "remaining_events": n_rem,
                    "min_events": pp.C_MIN_EVENTS}

    top = ev["top_symbol_event_share"]
    sc["C4"] = {"status": PASS if (ev["symbols_represented"] > 1 and top <= pp.C_MAX_TOP_SYMBOL_SHARE) else FAIL,
                "top_symbol_event_share": top, "symbols_represented": ev["symbols_represented"],
                "max": pp.C_MAX_TOP_SYMBOL_SHARE}

    loo, ev_idx = {}, raw.ev
    for s in sorted(ev["per_symbol"]):
        keep = ev_idx & (raw.symbol != s)
        n = int(keep.sum())
        e = float(raw.label[keep].mean()) if n else None  # other symbols' baselines are unchanged by removing s
        loo[s] = {"effect": e, "remaining_events": n,
                  "status": PASS if (_pos(e) and n >= pp.C_MIN_EVENTS) else FAIL}
    defined = {s: v["effect"] for s, v in loo.items() if v["effect"] is not None}
    min_sym = min(defined, key=lambda s: (defined[s], s)) if defined else None
    sc["C5"] = {"status": PASS if (loo and all(v["status"] == PASS for v in loo.values())) else FAIL,
                "min_effect": None if min_sym is None else defined[min_sym], "min_effect_symbol": min_sym,
                "failed_symbols": sorted(s for s, v in loo.items() if v["status"] == FAIL), "per_symbol": loo}

    ci = cc.cond_index[cand["condition_id"]]
    nbrs = cc.neighbors[ci]
    if nbrs:
        n_strong = sum((j, h) in cc.strong for j in nbrs)
        sc["C6"] = {"status": PASS if n_strong / len(nbrs) >= pp.C_MIN_NEIGHBOR_SHARE else FAIL, "n_neighbors": len(nbrs),
                    "n_strong_neighbors": n_strong, "share": n_strong / len(nbrs), "min": pp.C_MIN_NEIGHBOR_SHARE}
    else:
        sc["C6"] = {"status": NA, "n_neighbors": 0}

    share = er._share({k: v["effect"] * v["n"] for k, v in ev["per_regime"].items()})  # noqa: SLF001
    sc["C7"] = {"status": "CLASSIFIED", "share": share, "max": pp.C_REGIME_CONCENTRATION,
                "class": ("REGIME_UNCLASSIFIED_NO_POSITIVE_BUCKET" if share is None else
                          "REGIME_CONCENTRATED" if share > pp.C_REGIME_CONCENTRATION else "GENERAL_REGIME")}

    k = pp.HORIZONS.index(h)
    adj, evaluable, supported = [x for x in (pp.HORIZONS[k - 1] if k else None,
                                             pp.HORIZONS[k + 1] if k + 1 < len(pp.HORIZONS) else None) if x], 0, 0
    detail = {}
    for ah in adj:
        row = cc.factor_ledger[(cand["condition_id"], ah)]
        ok = row["status"] == "succeeded"
        sup = ok and _pos(row["effect"]) and row["p_value"] is not None and row["p_value"] <= pp.C_HORIZON_P_MAX
        evaluable += ok
        supported += bool(sup)
        detail[str(ah)] = {"evaluable": ok, "effect": row["effect"], "p_value": row["p_value"], "supports": bool(sup)}
    sc["C8"] = {"status": "CLASSIFIED", "adjacent_horizons": detail,
                "class": "NOT_APPLICABLE" if not evaluable else "HORIZON_SUPPORTED" if supported else "HORIZON_ISOLATED"}

    failed = [g for g in pp.CONDITIONAL_HARD_GATES if sc[g]["status"] == FAIL]
    blocked = [g for g in pp.CONDITIONAL_HARD_GATES if sc[g]["status"] == BLOCKED]
    verdict = (pp.VERDICTS_CONDITIONAL[1] if failed else pp.VERDICTS_CONDITIONAL[2] if blocked
               else pp.VERDICTS_CONDITIONAL[0])
    return {"kind": pp.KIND_CONDITIONAL, "candidate_id": cand["candidate_id"], "edge_id": cand["edge_id"],
            "factor_id": cand["factor_id"], "condition_id": cand["condition_id"], "family": cand["family"],
            "params": cand["params"], "horizon": h, "scenarios": sc, "failed_gates": failed, "blocked_gates": blocked,
            "not_applicable": [g for g in pp.CONDITIONAL_HARD_GATES if sc[g]["status"] == NA], "verdict": verdict,
            "regime_class": sc["C7"]["class"], "horizon_class": sc["C8"]["class"],
            "rank_fields": {"min_loo_effect": sc["C5"]["min_effect"], "leave_best_year_out_effect": lbyo_effect,
                            "positive_years": positive_years, "q_value": sc["C1"]["q_value"]},
            "executable_pnl": False, **pp.LABELS}


def rank_conditional_survivors(records: list[dict]) -> list[dict]:
    """READ-ONLY lexicographic ranking; never alters pass/fail or identity."""
    surv = [r for r in records if r["verdict"] == pp.VERDICTS_CONDITIONAL[0]]

    def key(r):
        f = r["rank_fields"]
        return (-f["min_loo_effect"], -f["leave_best_year_out_effect"], -f["positive_years"], f["q_value"], r["factor_id"])
    return sorted(surv, key=key)
