"""Guards the frozen pullback_mean_reversion_20_2 campaign predeclaration."""

from __future__ import annotations

import json
import re
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
PB = json.loads((HERE / "PREDECLARED_CAMPAIGN_PULLBACK_01.json").read_text(encoding="utf-8"))
DUAL = json.loads((HERE / "PREDECLARED_CAMPAIGN_DUAL_SMA_01.json").read_text(encoding="utf-8"))
SRC = (REPO / "core-rs/crates/mqk-strategy/src/engines/pullback_mean_reversion_20_2.rs").read_text(encoding="utf-8")


def test_engine_constants_match_the_executable_source():
    eng = PB["native_engine"]
    assert eng["strategy_id"] == re.search(r'const NAME: &str = "([^"]+)"', SRC).group(1)
    assert eng["lookback_closes"] == eng["required_history_bars"] == int(re.search(r"const LOOKBACK: usize = (\d+);", SRC).group(1))
    assert eng["sigma_multiplier"] == float(re.search(r"const SIGMA_MULTIPLIER: i128 = (\d+);", SRC).group(1)) == 2.0
    assert eng["timeframe_secs"] == int(re.search(r"const TIMEFRAME_SECS: i64 = ([\d_]+);", SRC).group(1).replace("_", ""))
    assert eng["states"] == ["FLAT", "LONG"] and "never short" in eng["rule"].lower()


def test_policy_sizing_data_and_review_are_carried_forward_unchanged():
    for key in ("promotion_policy", "scanner_review", "execution_fidelity", "rejection_gates", "economic_protocol", "robustness"):
        assert PB[key] == DUAL[key], key
    assert {k: v for k, v in PB["data"].items() if k != "reuse_verified_data_from"} ==         {k: v for k, v in DUAL["data"].items() if k != "reuse_verified_data_from"}
    p, d = dict(PB["partition"]), dict(DUAL["partition"])
    p.pop("warmup_note"), d.pop("warmup_note")
    assert p == d
    u, v = dict(PB["universe"]), dict(DUAL["universe"])
    u.pop("population_note"), v.pop("population_note")
    assert u == v
    wts = PB["economic_protocol"]["weight_to_share"]
    assert wts == {"equity_usd": 100000.0, "max_position_notional_usd": 50000.0}
    assert PB["robustness"]["p7a_p7b_stress"]["stress_max_position_notional_usd"] < wts["max_position_notional_usd"]
    thresholds = PB["promotion_policy"]
    assert thresholds["MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO"] == 0.5
    assert thresholds["MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING"] == 0.5
    assert PB["execution_fidelity"]["floor"] == 0.95


def test_new_identity_and_finite_bound():
    assert PB["hypothesis"]["hypothesis_id"] != DUAL["hypothesis"]["hypothesis_id"]
    assert PB["experiment"]["real_experiment_id"] != DUAL["experiment"]["real_experiment_id"]
    assert PB["native_engine"]["strategy_id"] not in ("trend_sma50", "dual_sma_50_200_trend", "intraday_scalper")
    assert len(PB["universe"]["symbols"]) == PB["universe"]["max_trials"] == 5
    s = PB["stopping_rule"]
    assert s["no_additional_sigma_or_lookback"] and s["no_holdout_consumption"] and s["run_once"]
    assert PB["activity_report"]["gate"] is False


def test_no_result_values_in_the_predeclaration():
    text = json.dumps(PB).lower()
    for banned in ("net_total_return", "net_sharpe", "total_return_pct", "dsr_result", '"run_id"', "economic_eval_id"):
        assert banned not in text
