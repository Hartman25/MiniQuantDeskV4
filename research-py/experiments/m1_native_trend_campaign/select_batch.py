"""Mechanical batch outcome: eligibility, max-one selection, family and batch outcomes.

A pure function of the batch evidence table (`batch_results.json` written by summarize_batch.py),
the batch-wide judge and the declaration's frozen policy. It reads no market data, tunes nothing and
cannot select from an unresolved population:

* a trial is ELIGIBLE iff it clears every canonical rejection gate (below);
* the selection is the eligible trial with the greatest judge DSR, ties broken by the
  lexicographically smallest trial_id; at most one;
* families and the batch outcome follow the predeclared definitions.

The Promotion thresholds (Sharpe/MDD/CAGR/profit factor/profitable months) are evaluated by the
canonical `evaluate_promotion`; this script only consumes that evaluator's verdict
(`promotion_eligibility.json`, keyed by trial_id) for trials that already cleared every earlier gate,
and refuses to decide while such a verdict is missing.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

# Mirrors mqk_backtest::REQUIRED_ROBUSTNESS_SCENARIO_NAMES (a test pins the equality).
REQUIRED_ROBUSTNESS_SCENARIOS = (
    "execution_delay_stress", "symbol_leave_one_out", "month_year_regime_concentration",
    "parameter_neighborhood_execution", "placebo_temporal_offset", "conservative_capacity_stress",
    "dsr_pbo_sensitivity", "p7a_p7b_economic_replay_stress", "genuine_shuffled_placebo",
)
GATES = ("economic_evaluation", "judge_evaluable", "dsr", "pbo", "robustness", "native_stress_suite",
         "scanner_review", "promotion_thresholds")


class UnresolvedPopulation(Exception):
    """The population cannot be decided yet (never reported as a rejection)."""


def gate_results(row: dict, judge: dict, policy: dict, promotion_eval: dict | None) -> dict:
    """gate -> True / False / None (None = not reached because an earlier gate failed)."""
    min_dsr = policy["MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO"]
    max_pbo = policy["MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING"]
    out: dict = {g: None for g in GATES}
    out["economic_evaluation"] = not row.get("economic_failed")
    pbo = judge.get("pbo_result") or {}
    out["judge_evaluable"] = (judge.get("judge_status") == "evaluated" and pbo.get("status") == "evaluated"
                              and str(row.get("judge_status")) == "included" and row.get("dsr") is not None)
    out["dsr"] = bool(out["judge_evaluable"]) and row["dsr"] >= min_dsr
    out["pbo"] = pbo.get("status") == "evaluated" and pbo.get("pbo") is not None and pbo["pbo"] <= max_pbo
    out["robustness"] = (out["economic_evaluation"] and not row.get("robustness_failed")
                         and not row.get("robustness_missing"))
    out["native_stress_suite"] = out["economic_evaluation"] and not row.get("stress_failed")
    out["scanner_review"] = row.get("review_state") == "paper_candidate"
    cleared_before = all(out[g] for g in GATES[:-1])
    if cleared_before:
        if promotion_eval is None or row["trial_id"] not in promotion_eval:
            raise UnresolvedPopulation(
                f"trial {row['trial_id']} cleared every gate before Promotion thresholds but has no canonical "
                "evaluate_promotion verdict")
        out["promotion_thresholds"] = bool(promotion_eval[row["trial_id"]]["passed"])
    return out


def decide(rows: list[dict], judge: dict, policy: dict, promotion_eval: dict | None = None,
           *, expected_trials: int = 15, family_of: dict | None = None) -> dict:
    if len(rows) != expected_trials:
        raise UnresolvedPopulation(f"{len(rows)} trials accounted for, {expected_trials} required")
    if len({r["trial_id"] for r in rows}) != len(rows):
        raise UnresolvedPopulation("duplicate trial in the evidence table")
    decided = []
    for r in rows:
        g = gate_results(r, judge, policy, promotion_eval)
        decided.append({"trial_id": r["trial_id"], "strategy": r["strategy"], "symbol": r["symbol"],
                        "dsr": r.get("dsr"), "gates": g,
                        "eligible": all(g[x] is True for x in GATES),
                        "first_failed_gate": next((x for x in GATES if g[x] is not True), None)})
    eligible = [d for d in decided if d["eligible"]]
    winner = None
    if eligible:
        winner = sorted(eligible, key=lambda d: (-d["dsr"], d["trial_id"]))[0]  # greatest DSR, then smallest id
    families = {}
    for strategy in dict.fromkeys(r["strategy"] for r in rows):
        members = [d for d in decided if d["strategy"] == strategy]
        if winner and winner["strategy"] == strategy:
            families[strategy] = "FAMILY_SELECTED"
        elif any(d["eligible"] for d in members):
            families[strategy] = "FAMILY_ELIGIBLE_NOT_SELECTED"
        else:
            families[strategy] = "FAMILY_REJECTED"
    return {
        "batch_outcome": "BATCH_HAS_ELIGIBLE_CANDIDATE" if winner else "BATCH_REJECTED",
        "selected_trial_id": winner["trial_id"] if winner else None,
        "selected_status": "PROMOTION_ELIGIBLE_PENDING_INDEPENDENT_REVIEW" if winner else None,
        "eligible_trial_ids": sorted(d["trial_id"] for d in eligible),
        "family_outcomes": families, "trials": decided,
        "population": {"trials": len(rows), "failed_attempts": sum(1 for r in rows if r.get("economic_failed")),
                       "judge_included": sum(1 for r in rows if str(r.get("judge_status")) == "included")},
    }


def main() -> None:
    here = Path(__file__).resolve().parent
    decl = json.loads((here / os.environ["MQK_M1_BATCH_DECLARATION"]).read_text(encoding="utf-8"))
    run = here / decl["run_dir"]
    rows = json.loads((run / "batch_results.json").read_text(encoding="utf-8"))
    judge = json.loads((run / "judge" / "judge.json").read_text(encoding="utf-8"))
    promo_path = run / "promotion_eligibility.json"
    promo_eval = json.loads(promo_path.read_text(encoding="utf-8")) if promo_path.exists() else None
    result = decide(rows, judge, decl["promotion_policy"], promo_eval, expected_trials=decl["universe"]["max_trials"])
    (run / "batch_outcome.json").write_text(json.dumps(result, indent=1, sort_keys=True), encoding="utf-8")
    print(json.dumps({k: result[k] for k in ("batch_outcome", "selected_trial_id", "selected_status",
                                            "family_outcomes", "population")}, indent=1))


if __name__ == "__main__":
    main()
