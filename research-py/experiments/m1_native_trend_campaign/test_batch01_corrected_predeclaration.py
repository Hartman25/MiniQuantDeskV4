"""Guards the frozen corrected-protocol reevaluation of the three Batch 01 hypotheses.

The corrected declaration may differ from the historical one ONLY in the protocol,
capital, benchmark and run-scope fields; everything an observed result could tempt
someone to tune must be byte-identical.
"""

from __future__ import annotations

import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
OLD = json.loads((HERE / "PREDECLARED_BATCH_01.json").read_text(encoding="utf-8"))
NEW = json.loads((HERE / "PREDECLARED_BATCH_01_CORRECTED.json").read_text(encoding="utf-8"))
V2 = "capital_matched_exact_target_buy_hold_v1"


def test_hypotheses_universe_partition_robustness_and_thresholds_are_byte_identical():
    for key in ("hypotheses", "universe", "partition", "robustness", "promotion_policy", "rejection_gates",
                "selection_authority", "activity_report", "attempt_identity_rules"):
        assert NEW[key] == OLD[key], key
    assert NEW["scanner_review"]["policy"].startswith(OLD["scanner_review"]["policy"].split(" -- ")[0])
    assert {k: v for k, v in NEW["data"].items() if k != "reuse_verified_data_from"} == \
        {k: v for k, v in OLD["data"].items() if k != "reuse_verified_data_from"}
    keep = ("cost_model", "execution_pricing", "weight_to_share", "annualization", "protocol_id")
    for key in keep:
        assert NEW["economic_protocol"][key] == OLD["economic_protocol"][key], key


def test_the_only_economic_changes_are_exact_target_policy_and_the_capital_basis():
    sp = NEW["economic_protocol"]["signal_policy"]
    assert sp["direction_policy"] == "native_exact_target_qty_v1"
    assert sp["sizing"] == "exact_native_target_qty_v1"
    assert sp["entry_threshold"] == 0.5 and sp["long_only"] is True and sp["max_gross_exposure"] == 1.0
    assert NEW["economic_protocol"]["signal_stream_protocol"] == "native_strategy_signal_stream_v2"
    equity_micros = round(NEW["economic_protocol"]["weight_to_share"]["equity_usd"] * 1_000_000)
    assert NEW["native_backtest"]["initial_cash_micros"] == equity_micros == 100_000_000_000
    assert NEW["native_backtest"]["timeframe_secs"] == OLD["native_backtest"]["timeframe_secs"]
    assert NEW["native_backtest"]["integrity_calendar"] == OLD["native_backtest"]["integrity_calendar"]


def test_benchmark_v2_is_the_review_alpha_and_min_alpha_is_unchanged():
    b = NEW["benchmark"]
    assert b["policy_id"] == V2 and b["min_alpha_pct"] == 0.0
    assert "INFORMATIONAL ONLY" in b["legacy_fully_invested_benchmark"]
    assert NEW["scanner_review"]["benchmark_policy"] == V2
    assert "min_alpha_pct 0" in NEW["scanner_review"]["policy"]


def test_fifteen_evaluations_one_population_and_a_fresh_experiment_scope():
    assert len(NEW["universe"]["trials"]) == NEW["universe"]["max_trials"] == 15
    assert NEW["experiment"]["real_experiment_id"] != OLD["experiment"]["real_experiment_id"]
    assert NEW["experiment"]["registry_db_relative_path"] != OLD["experiment"]["registry_db_relative_path"]
    assert NEW["run_dir"] != OLD["run_dir"]
    assert NEW["experiment"]["judge_scope"] == "whole experiment, never a subset"
    assert NEW["comparison_scope"]["population"] == "all fifteen predeclared corrected trials"
    assert "hypothesis_id unset" in NEW["experiment"]["population_policy"]
    assert "not additional independent hypotheses" in NEW["experiment"]["population_policy"]


def test_old_evidence_is_declared_historical_and_untouched():
    s = NEW["supersedes"]
    assert s["declaration_file"] == "PREDECLARED_BATCH_01.json" and s["run_dir"] == OLD["run_dir"]
    assert "NOT_PROMOTION_AUTHORITY" in s["status"] and "SUPERSEDED_PROTOCOL" in s["status"]
    assert OLD["economic_protocol"]["signal_policy"]["direction_policy"] == "long_only_v1"


def test_holdout_reserved_data_identity_pinned_and_stopping_rule_finite():
    assert NEW["holdout"]["status"] == "RESERVED / UNCONSUMED"
    assert NEW["partition"]["holdout_months"] == 6
    pin = NEW["data"]["reuse_verified_data_from"]
    assert len(pin["expected_artifact_sha256"]) == 64 and pin["expected_row_count"] > 0
    r = NEW["batch_stopping_rule"]
    assert r["trials"] == 15 and r["run_all_fifteen_regardless_of_outcome"] and r["no_early_selection"]
    assert r["no_h4_or_further_hypothesis"] and r["no_parameter_variations"] and r["no_batch_02"]
    assert r["no_holdout_consumption"] is True


def test_no_result_values_in_the_predeclaration():
    text = json.dumps(NEW).lower()
    for banned in ("net_total_return", "net_sharpe", "total_return_pct", "dsr_result", '"run_id"', "economic_eval_id",
                   "trial_id\":", '"alpha_pct":'):
        assert banned not in text
