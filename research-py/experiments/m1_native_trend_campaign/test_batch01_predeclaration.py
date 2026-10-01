"""Guards the frozen m1_native_hypothesis_batch_01 predeclaration."""

from __future__ import annotations

import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
B = json.loads((HERE / "PREDECLARED_BATCH_01.json").read_text(encoding="utf-8"))
PB = json.loads((HERE / "PREDECLARED_CAMPAIGN_PULLBACK_01.json").read_text(encoding="utf-8"))

SYMBOLS = ["SPY", "EFA", "IEF", "VNQ", "GLD"]
STRATEGIES = ["absolute_momentum_252", "near_high_momentum_252_3pct", "trend_pullback_5d_4pct_hold5"]


def test_fifteen_trials_are_three_hypotheses_times_five_fixed_symbols():
    trials = B["universe"]["trials"]
    assert len(trials) == B["universe"]["max_trials"] == 15
    assert [(t["strategy_id"], t["symbol"]) for t in trials] == [(s, y) for s in STRATEGIES for y in SYMBOLS]
    assert [t["order"] for t in trials] == list(range(1, 16))
    assert sorted(B["universe"]["symbols"]) == sorted(SYMBOLS)
    assert [h["strategy_id"] for h in B["hypotheses"]] == STRATEGIES
    assert len({h["hypothesis_id"] for h in B["hypotheses"]}) == 3


def test_policy_sizing_data_and_review_are_carried_forward_unchanged():
    for key in ("promotion_policy", "scanner_review", "execution_fidelity", "rejection_gates", "economic_protocol", "robustness"):
        assert B[key] == PB[key], key
    assert B["economic_protocol"]["execution_pricing"]["pricing_model_id"] == "rust_conservative_bar_range_v1"
    assert B["economic_protocol"]["cost_model"]["slippage_bps_per_side"] == 0.0
    assert {k: v for k, v in B["data"].items() if k != "reuse_verified_data_from"} == \
        {k: v for k, v in PB["data"].items() if k != "reuse_verified_data_from"}
    p, b = dict(PB["partition"]), dict(B["partition"])
    p.pop("warmup_note"), b.pop("warmup_note")
    assert p == b
    wts = B["economic_protocol"]["weight_to_share"]
    assert wts == {"equity_usd": 100000.0, "max_position_notional_usd": 50000.0}
    assert B["robustness"]["p7a_p7b_stress"]["stress_max_position_notional_usd"] < wts["max_position_notional_usd"]
    th = B["promotion_policy"]
    assert th["MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO"] == 0.5
    assert th["MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING"] == 0.5
    assert B["execution_fidelity"]["floor"] == 0.95


def test_one_batch_wide_selection_population():
    e = B["experiment"]
    assert e["judge_scope"] == "whole experiment, never a subset"
    assert "hypothesis_id unset" in e["population_policy"]
    assert e["all_fifteen_trials_registered_before_first_economic_evaluation"] is True
    s = B["selection_authority"]
    assert s["max_promotions_in_this_batch"] == 1 and s["no_manual_selection"] is True
    assert s["selection_made_only_after_all_fifteen_trials_and_the_batch_judge_complete"] is True


def test_stopping_rule_is_finite_and_closes_the_batch():
    r = B["batch_stopping_rule"]
    assert r["trials"] == 15 and r["run_all_fifteen_regardless_of_outcome"] and r["no_early_selection"]
    assert r["no_h4_or_further_hypothesis"] and r["no_parameter_variations"] and r["no_holdout_consumption"]


def test_declared_history_requirements_and_no_state():
    req = {h["strategy_id"]: h["required_history_bars"] for h in B["hypotheses"]}
    assert req == {"absolute_momentum_252": 253, "near_high_momentum_252_3pct": 252, "trend_pullback_5d_4pct_hold5": 204}
    assert all(h["timeframe_secs"] == 86400 and h["direction"] == "long_only_flat" for h in B["hypotheses"])
    assert all(h["state"].startswith("stateless") for h in B["hypotheses"])


def test_no_result_values_in_the_predeclaration():
    text = json.dumps(B).lower()
    for banned in ("net_total_return", "net_sharpe", "total_return_pct", "dsr_result", '"run_id"', "economic_eval_id", "trial_id\":"):
        assert banned not in text
