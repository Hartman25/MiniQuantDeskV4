"""Guards the frozen dual_sma_50_200_trend campaign predeclaration."""

from __future__ import annotations

import json
import re
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
DUAL = json.loads((HERE / "PREDECLARED_CAMPAIGN_DUAL_SMA_01.json").read_text(encoding="utf-8"))
C3 = json.loads((HERE / "PREDECLARED_CAMPAIGN_03.json").read_text(encoding="utf-8"))
SRC = (REPO / "core-rs/crates/mqk-strategy/src/engines/dual_sma_50_200_trend.rs").read_text(encoding="utf-8")


def test_engine_constants_match_the_executable_source():
    eng = DUAL["native_engine"]
    assert eng["strategy_id"] == re.search(r'const NAME: &str = "([^"]+)"', SRC).group(1)
    assert eng["fast_lookback_closes"] == int(re.search(r"const FAST_LOOKBACK: usize = (\d+);", SRC).group(1))
    assert eng["slow_lookback_closes"] == int(re.search(r"const SLOW_LOOKBACK: usize = (\d+);", SRC).group(1))
    assert eng["timeframe_secs"] == int(re.search(r"const TIMEFRAME_SECS: i64 = ([\d_]+);", SRC).group(1).replace("_", ""))
    assert (eng["fast_lookback_closes"], eng["slow_lookback_closes"]) == (50, 200)


def test_everything_but_the_hypothesis_is_carried_forward_from_campaign_03():
    for key in ("universe", "partition", "promotion_policy", "scanner_review", "execution_fidelity", "rejection_gates"):
        a, b = C3[key], DUAL[key]
        if isinstance(a, dict):
            a, b = dict(a), dict(b)
        if key == "universe":
            b.pop("population_note")
        if key == "partition":
            a.pop("warmup_note")
            b.pop("warmup_note")
        assert a == b, key
    assert DUAL["economic_protocol"] == C3["economic_protocol"]
    assert DUAL["robustness"] == C3["robustness"]
    assert {k: v for k, v in C3["data"].items() if k != "reuse_verified_data_from"} == \
        {k: v for k, v in DUAL["data"].items() if k != "reuse_verified_data_from"}
    wts = DUAL["economic_protocol"]["weight_to_share"]
    assert wts == {"equity_usd": 100000.0, "max_position_notional_usd": 50000.0}
    assert DUAL["robustness"]["p7a_p7b_stress"]["stress_max_position_notional_usd"] < wts["max_position_notional_usd"]


def test_new_hypothesis_identity_and_finite_trial_bound():
    assert DUAL["hypothesis"]["hypothesis_id"] != C3["hypothesis"]["hypothesis_id"]
    assert DUAL["experiment"]["real_experiment_id"] != C3["experiment"]["real_experiment_id"]
    assert DUAL["native_engine"]["strategy_id"] != C3["native_engine"]["strategy_id"]
    assert len(DUAL["universe"]["symbols"]) == DUAL["universe"]["max_trials"] == 5
    assert DUAL["stopping_rule"]["no_additional_fast_slow_pairs"] is True
    assert DUAL["stopping_rule"]["no_holdout_consumption"] is True
    assert DUAL["turnover_measurement"]["gate"] is False


def test_no_result_values_in_the_predeclaration():
    text = json.dumps(DUAL).lower()
    for banned in ("net_total_return", "net_sharpe", "total_return_pct", "dsr_result", "\"run_id\"", "economic_eval_id"):
        assert banned not in text
