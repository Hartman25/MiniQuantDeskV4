"""Mechanical selection, family and batch outcomes (select_batch.decide)."""

from __future__ import annotations

import copy
import json
import re
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import select_batch as sb  # noqa: E402

B2 = json.loads((HERE / "PREDECLARED_BATCH_02.json").read_text(encoding="utf-8"))
POLICY = B2["promotion_policy"]
STRATS = ["turn_of_month_last1_first3", "halloween_nov_apr", "trading_range_breakout_50d_hold10"]
SYMS = ["SPY", "EFA", "IEF", "VNQ", "GLD"]


def rows(**over):
    out = []
    for i, (st, sy) in enumerate((s, y) for s in STRATS for y in SYMS):
        out.append({"strategy": st, "symbol": sy, "trial_id": f"{i + 1:032x}", "dsr": 0.2, "judge_status": "included",
                    "economic_failed": None, "robustness_failed": [], "robustness_missing": [], "stress_failed": [],
                    "review_state": "rejected"})
    for tid, patch in over.items():
        next(r for r in out if r["trial_id"] == tid).update(patch)
    return out


def tid(n: int) -> str:
    return f"{n:032x}"


JUDGE = {"judge_status": "evaluated", "pbo_result": {"status": "evaluated", "pbo": 0.30}}
GOOD = {"review_state": "paper_candidate", "dsr": 0.7}


def verdicts(*passes):
    return {tid(n): {"passed": ok} for n, ok in passes}


def test_zero_eligible_is_batch_rejected_with_no_winner_and_every_family_rejected():
    out = sb.decide(rows(), JUDGE, POLICY)
    assert out["batch_outcome"] == "BATCH_REJECTED" and out["selected_trial_id"] is None
    assert set(out["family_outcomes"].values()) == {"FAMILY_REJECTED"}
    assert out["eligible_trial_ids"] == [] and out["population"]["trials"] == 15
    assert all(t["first_failed_gate"] == "dsr" for t in out["trials"])


def test_the_single_eligible_trial_is_selected_and_its_family_is_selected():
    r = rows(**{tid(7): GOOD})
    out = sb.decide(r, JUDGE, POLICY, verdicts((7, True)))
    assert out["batch_outcome"] == "BATCH_HAS_ELIGIBLE_CANDIDATE" and out["selected_trial_id"] == tid(7)
    assert out["selected_status"] == "PROMOTION_ELIGIBLE_PENDING_INDEPENDENT_REVIEW"
    assert out["family_outcomes"] == {STRATS[0]: "FAMILY_REJECTED", STRATS[1]: "FAMILY_SELECTED",
                                      STRATS[2]: "FAMILY_REJECTED"}


def test_greatest_dsr_wins_ties_go_to_the_smallest_trial_id_and_at_most_one_is_selected():
    r = rows(**{tid(3): {**GOOD, "dsr": 0.8}, tid(12): {**GOOD, "dsr": 0.9}, tid(9): {**GOOD, "dsr": 0.9}})
    out = sb.decide(r, JUDGE, POLICY, verdicts((3, True), (9, True), (12, True)))
    assert out["selected_trial_id"] == tid(9), "tie at 0.9 -> lexicographically smallest id"
    assert len(out["eligible_trial_ids"]) == 3 and out["selected_trial_id"] in out["eligible_trial_ids"]
    assert out["family_outcomes"] == {STRATS[0]: "FAMILY_ELIGIBLE_NOT_SELECTED", STRATS[1]: "FAMILY_SELECTED",
                                      STRATS[2]: "FAMILY_ELIGIBLE_NOT_SELECTED"}
    # a lower-DSR winner can never beat a higher one, regardless of id order
    out = sb.decide(rows(**{tid(1): {**GOOD, "dsr": 0.51}, tid(15): {**GOOD, "dsr": 0.52}}), JUDGE, POLICY,
                    verdicts((1, True), (15, True)))
    assert out["selected_trial_id"] == tid(15)


@pytest.mark.parametrize("patch,gate", [
    ({"economic_failed": "boom"}, "economic_evaluation"),
    ({"judge_status": "excluded:degenerate"}, "judge_evaluable"),
    ({"dsr": None}, "judge_evaluable"),
    ({"dsr": 0.4999}, "dsr"),
    ({"robustness_failed": ["genuine_shuffled_placebo"]}, "robustness"),
    ({"robustness_missing": ["p7a_p7b_economic_replay_stress"]}, "robustness"),
    ({"stress_failed": ["cost_stress_3x"]}, "native_stress_suite"),
    ({"review_state": "watchlist_candidate"}, "scanner_review"),
])
def test_every_canonical_gate_blocks_eligibility(patch, gate):
    out = sb.decide(rows(**{tid(4): {**GOOD, **patch}}), JUDGE, POLICY)
    t = next(x for x in out["trials"] if x["trial_id"] == tid(4))
    assert not t["eligible"] and t["first_failed_gate"] == gate
    assert out["batch_outcome"] == "BATCH_REJECTED"


def test_pbo_above_the_maximum_or_unevaluable_blocks_every_trial():
    good = rows(**{tid(2): GOOD})
    for pbo in ({"status": "evaluated", "pbo": 0.5001}, {"status": "not_evaluable", "pbo": None}):
        out = sb.decide(good, {"judge_status": "evaluated", "pbo_result": pbo}, POLICY, verdicts((2, True)))
        assert out["batch_outcome"] == "BATCH_REJECTED"
    assert sb.decide(good, {**JUDGE, "judge_status": "not_evaluable"}, POLICY)["batch_outcome"] == "BATCH_REJECTED"
    exactly = {"judge_status": "evaluated", "pbo_result": {"status": "evaluated", "pbo": 0.5}}
    assert sb.decide(good, exactly, POLICY, verdicts((2, True)))["selected_trial_id"] == tid(2)


def test_dsr_exactly_at_the_minimum_clears_and_promotion_thresholds_can_still_reject():
    r = rows(**{tid(5): {**GOOD, "dsr": 0.5}})
    assert sb.decide(r, JUDGE, POLICY, verdicts((5, True)))["selected_trial_id"] == tid(5)
    out = sb.decide(r, JUDGE, POLICY, verdicts((5, False)))
    assert out["batch_outcome"] == "BATCH_REJECTED"
    assert next(t for t in out["trials"] if t["trial_id"] == tid(5))["first_failed_gate"] == "promotion_thresholds"


def test_a_trial_that_cleared_every_earlier_gate_cannot_be_decided_without_the_canonical_verdict():
    r = rows(**{tid(6): GOOD})
    for pe in (None, {}, verdicts((7, True))):
        with pytest.raises(sb.UnresolvedPopulation, match="evaluate_promotion"):
            sb.decide(r, JUDGE, POLICY, pe)


@pytest.mark.parametrize("n", [14, 16])
def test_an_incomplete_or_oversized_population_is_never_decided(n):
    base = rows()
    r = base[:n] if n < 15 else base + [{**base[0], "trial_id": tid(99)}]
    with pytest.raises(sb.UnresolvedPopulation, match="required"):
        sb.decide(r, JUDGE, POLICY)
    dup = rows()
    dup[1]["trial_id"] = dup[0]["trial_id"]
    with pytest.raises(sb.UnresolvedPopulation, match="duplicate"):
        sb.decide(dup, JUDGE, POLICY)


def test_failed_trials_stay_in_their_family_and_the_population_accounts_for_them():
    r = rows(**{tid(1): {"economic_failed": "native stream halted", "dsr": None, "judge_status": "excluded:x"}})
    out = sb.decide(r, JUDGE, POLICY)
    assert out["population"] == {"trials": 15, "failed_attempts": 1, "judge_included": 14}
    assert out["family_outcomes"][STRATS[0]] == "FAMILY_REJECTED"


def test_required_scenarios_equal_the_rust_constant():
    src = (HERE.parents[2] / "core-rs/crates/mqk-backtest/src/robustness_gauntlet.rs").read_text(encoding="utf-8")
    body = src[src.index("pub const REQUIRED_ROBUSTNESS_SCENARIO_NAMES"):]
    body = body[: body.index("];")]
    literals = re.findall(r'^\s*"([a-z_0-9]+)",\s*$', body, re.M)
    consts = re.findall(r"crate::(dsr_pbo_sensitivity|p7a_p7b_economic_replay_stress|genuine_shuffled_placebo)::", body)
    assert len(literals) == 6 and len(consts) == 3
    names = {"dsr_pbo_sensitivity": "dsr_pbo_sensitivity", "p7a_p7b_economic_replay_stress":
             "p7a_p7b_economic_replay_stress", "genuine_shuffled_placebo": "genuine_shuffled_placebo"}
    assert tuple(literals + [names[c] for c in consts]) == sb.REQUIRED_ROBUSTNESS_SCENARIOS
