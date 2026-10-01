"""Guards the frozen M1 native trend campaign predeclaration."""

from __future__ import annotations

import json
import re
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
DECL = json.loads((HERE / "PREDECLARED_CAMPAIGN.json").read_text(encoding="utf-8"))
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


def test_no_result_values_in_the_predeclaration():
    text = json.dumps(DECL).lower()
    for banned in ("net_total_return", "net_sharpe", "total_return_pct", "dsr_result", "\"run_id\"", "trial_id", "economic_eval_id"):
        assert banned not in text
