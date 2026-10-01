"""Guards the frozen M1 native trend campaign predeclaration."""

from __future__ import annotations

import json
import re
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
import pytest

CAMPAIGN_FILES = ["PREDECLARED_CAMPAIGN.json", "PREDECLARED_CAMPAIGN_02.json"]
DECL = json.loads((HERE / "PREDECLARED_CAMPAIGN_02.json").read_text(encoding="utf-8"))
V1 = json.loads((HERE / "PREDECLARED_CAMPAIGN.json").read_text(encoding="utf-8"))
ENGINE_SRC = (REPO / "core-rs/crates/mqk-strategy/src/engines/trend_sma50.rs").read_text(encoding="utf-8")


def test_frozen_marker_and_finite_trial_bound():
    assert DECL["frozen_before_any_result"] is True
    uni = DECL["universe"]
    assert len(uni["symbols"]) == len(set(uni["symbols"])) == uni["max_trials"] == 5
    assert uni["symbols"] == sorted(uni["symbols"])
    assert uni["additional_candidate_after_predeclaration"] == "forbidden"
    assert uni["universe_mode"] == "fixed_ex_ante"


def test_native_engine_constants_match_the_executable_source():
    eng = DECL["native_engine"]
    name = re.search(r'const NAME: &str = "([^"]+)"', ENGINE_SRC).group(1)
    lookback = int(re.search(r"const LOOKBACK: usize = (\d+);", ENGINE_SRC).group(1))
    timeframe = int(re.search(r"const TIMEFRAME_SECS: i64 = ([\d_]+);", ENGINE_SRC).group(1).replace("_", ""))
    assert (eng["strategy_id"], eng["lookback_closes"], eng["timeframe_secs"]) == (name, lookback, timeframe)


def test_symbols_are_enabled_registry_instruments():
    reg = json.loads((REPO / "config/instruments/equities.json").read_text(encoding="utf-8"))
    enabled = {i["symbol"] for i in reg if i.get("enabled")}
    assert set(DECL["universe"]["symbols"]) <= enabled


def test_partition_reserves_holdout_and_folds_are_consistent():
    part = DECL["partition"]
    assert part["holdout_months"] >= 6 and part["test_months"] == 12
    assert "never consumed" in part["holdout_rule"]
    assert DECL["stopping_rule"]["no_holdout_consumption"] is True


def test_thresholds_are_present_and_in_range():
    pol = DECL["promotion_policy"]
    for key in ("MQK_PROMOTION_MIN_SHARPE", "MQK_PROMOTION_MAX_MDD", "MQK_PROMOTION_MIN_CAGR",
                "MQK_PROMOTION_MIN_PROFIT_FACTOR", "MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT",
                "MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO", "MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING"):
        assert isinstance(pol[key], (int, float)), key
    assert 0 < pol["MQK_PROMOTION_MAX_MDD"] <= 1
    assert 0 <= pol["MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO"] <= 1
    assert 0 <= pol["MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING"] <= 1
    assert pol["MQK_RESEARCH_REQUIRE_NATIVE_SEMANTIC_BINDING"] == 1


@pytest.mark.parametrize("name", CAMPAIGN_FILES)
def test_no_result_values_in_the_predeclaration(name):
    text = json.dumps(json.loads((HERE / name).read_text(encoding="utf-8"))).lower()
    for banned in ("net_total_return", "net_sharpe", "total_return_pct", "dsr_result", "\"run_id\"", "economic_eval_id"):
        assert banned not in text


def test_campaign_02_amends_exactly_the_declared_fields_and_nothing_else():
    a, b = V1, DECL
    assert b["amends_campaign"] == a["campaign_id"]
    for key in ("hypothesis", "native_engine", "universe", "partition", "robustness", "promotion_policy",
                "scanner_review", "native_backtest", "rejection_gates"):
        assert a[key] == b[key], key
    assert a["data"] == {k: v for k, v in b["data"].items() if k != "reuse_verified_data_from"}
    assert b["economic_protocol"]["weight_to_share"]["max_position_notional_usd"] == 90000.0
    for key in ("signal_policy", "cost_model", "execution_pricing", "annualization"):
        assert a["economic_protocol"][key] == b["economic_protocol"][key], key
    assert b["experiment"]["real_experiment_id"] != a["experiment"]["real_experiment_id"]
    assert b["execution_fidelity"]["floor"] == 0.95
    assert b["stopping_rule"]["this_is_the_final_run_for_this_hypothesis_family"] is True


def test_voiding_evidence_contains_no_return_values():
    text = json.dumps(DECL["amendment"]).lower()
    for banned in ("net_total_return", "net_sharpe", "total_return", "sharpe", "dsr", "drawdown"):
        assert banned not in text
