"""Batch 03 family ranking: a pure, mechanical function of the 60-row batch evidence table.

Ranks the ten predeclared families (never individual symbol trials) and advances at most the top
three to Confirmation as DISCOVERY_ADVANCED_TO_CONFIRMATION. That is NOT Promotion: no trial is
selected and no Promotion candidate is created from Discovery.

Fail-closed rules:
* the population must be exactly the declared 60 (family x symbol) slots, each once; a missing or
  duplicated slot is an unresolved population, never a silent drop;
* a non-evaluable slot stays in the population and counts as WORST on every key (never dropped,
  never imputed);
* a family needs at least `min_evaluable_trials_per_family` evaluable slots to be rankable;
* an evaluable slot without drawdown-improvement evidence is worst on key 5 only. The
  capital-fraction benchmark section carries no benchmark drawdown today, so no value is invented.
"""

from __future__ import annotations

import json
import math
import os
import sys
from pathlib import Path

NEG_INF = float("-inf")
ADVANCED = "DISCOVERY_ADVANCED_TO_CONFIRMATION"
NOT_ADVANCED = "DISCOVERY_NOT_ADVANCED"
INSUFFICIENT = "DISCOVERY_INSUFFICIENT_EVALUABLE_TRIALS"


class UnresolvedPopulation(Exception):
    """The population cannot be ranked (never reported as a rejection)."""


def median_worst(values: list[float]) -> float:
    """Median over every slot; a worst (-inf) slot in either middle position makes it worst."""
    ordered = sorted(values)
    n = len(ordered)
    lo, hi = ordered[(n - 1) // 2], ordered[n // 2]
    return NEG_INF if NEG_INF in (lo, hi) else (lo + hi) / 2


def slot_from_evidence(row: dict, family: str) -> dict:
    """Map a batch_results.json row onto the ranking slot (the only place evidence is interpreted)."""
    bench = row.get("benchmark_evidence") or {}
    alpha = bench.get("alpha_pct")
    evaluable = (not row.get("economic_failed") and str(row.get("judge_status")) == "included"
                 and row.get("dsr") is not None and alpha is not None)
    robust = bool(evaluable and not row.get("robustness_failed") and not row.get("robustness_missing"))
    return {"family": family, "symbol": row["symbol"], "trial_id": row["trial_id"], "evaluable": evaluable,
            "dsr": row.get("dsr") if evaluable else None, "alpha_pct": alpha if evaluable else None,
            "robustness_clear": robust,
            "drawdown_improvement": row.get("drawdown_improvement") if evaluable else None}


def _worst_if_none(slot: dict, field: str) -> float:
    value = slot[field] if slot["evaluable"] else None
    if value is None or isinstance(value, bool) or not isinstance(value, (int, float)) or math.isnan(value):
        return NEG_INF
    return float(value)


def family_metrics(slots: list[dict]) -> dict:
    alphas = [_worst_if_none(s, "alpha_pct") for s in slots]
    return {
        "evaluable_trials": sum(1 for s in slots if s["evaluable"]),
        "positive_alpha_symbols": sum(1 for a in alphas if a > 0),
        "median_dsr": median_worst([_worst_if_none(s, "dsr") for s in slots]),
        "median_net_alpha_pct": median_worst(alphas),
        "robustness_clear_trials": sum(1 for s in slots if s["evaluable"] and s["robustness_clear"] is True),
        "median_drawdown_improvement": median_worst([_worst_if_none(s, "drawdown_improvement") for s in slots]),
    }


def rank_key(family_id: str, m: dict) -> tuple:
    return (-m["positive_alpha_symbols"], -m["median_dsr"], -m["median_net_alpha_pct"],
            -m["robustness_clear_trials"], -m["median_drawdown_improvement"], family_id)


def rank_families(slots: list[dict], *, families: list[str], symbols: list[str], min_evaluable: int = 4,
                  advance: int = 3) -> dict:
    expected = {(f, s) for f in families for s in symbols}
    seen = [(x["family"], x["symbol"]) for x in slots]
    if len(set(seen)) != len(seen):
        raise UnresolvedPopulation("duplicate (family, symbol) slot in the evidence table")
    if set(seen) != expected:
        missing, extra = sorted(expected - set(seen)), sorted(set(seen) - expected)
        raise UnresolvedPopulation(f"population is not the declared {len(expected)} slots: missing={missing} extra={extra}")
    metrics = {f: family_metrics([x for x in slots if x["family"] == f]) for f in families}
    rankable = sorted((f for f in families if metrics[f]["evaluable_trials"] >= min_evaluable),
                      key=lambda f: rank_key(f, metrics[f]))
    advanced = rankable[:advance]
    outcomes = {f: (ADVANCED if f in advanced else NOT_ADVANCED if f in rankable else INSUFFICIENT) for f in families}
    return {"family_outcomes": outcomes, "advanced_families": advanced, "ranking": rankable,
            "metrics": {f: {k: (None if v == NEG_INF else v) for k, v in metrics[f].items()} for f in families},
            "selected_trial_ids": [], "promotion_candidates": [], "promotion_candidate_created": False}


def confirmation_template(advanced: list[str], universe: list[str]) -> dict:
    """Prepared Confirmation population. Registers nothing; history sufficiency is verified later."""
    return {"status": "PREPARED_NOT_REGISTERED", "registered": False, "history_verified": False,
            "future_trials": [{"family": f, "symbol": s} for f in advanced for s in universe],
            "substitution_allowed": False}


def main() -> None:
    here = Path(__file__).resolve().parent
    decl = json.loads((here / os.environ["MQK_M1_BATCH_DECLARATION"]).read_text(encoding="utf-8"))
    if decl.get("execution_gate", {}).get("executable") is not True:
        sys.exit(f"{decl['execution_gate']['status']}: family ranking has no evidence to read")
    run = here / decl["run_dir"]
    rows = json.loads((run / "batch_results.json").read_text(encoding="utf-8"))
    family_of = {h["strategy_id"]: h["hypothesis_label"] for h in decl["hypotheses"]}
    slots = [slot_from_evidence(r, family_of[r["strategy"]]) for r in rows]
    rule = decl["family_ranking"]
    result = rank_families(slots, families=[h["hypothesis_label"] for h in decl["hypotheses"]],
                           symbols=decl["universe"]["symbols"],
                           min_evaluable=rule["min_evaluable_trials_per_family"])
    result["confirmation"] = confirmation_template(result["advanced_families"],
                                                   decl["confirmation_preparation"]["universe"])
    (run / "family_ranking.json").write_text(json.dumps(result, indent=1, sort_keys=True), encoding="utf-8")
    print(json.dumps({k: result[k] for k in ("family_outcomes", "advanced_families")}, indent=1))


if __name__ == "__main__":
    main()
