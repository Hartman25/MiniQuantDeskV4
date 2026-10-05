"""Guards the Batch 03 prospective predeclaration (60 trials, ten families, six ETFs).

The declaration is deliberately NOT executable: the P7A/P7B stress contract needs explicit operator
authority and no stress value is invented. These tests pin the frozen structure and prove the runner
refuses every stage until the gate is lifted.
"""

from __future__ import annotations

import copy
import importlib.util
import json
import os
import re
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
ENGINES = HERE.parents[2] / "core-rs" / "crates" / "mqk-strategy" / "src" / "engines"
DECL_NAME = "PREDECLARED_BATCH_03.json"
DECL = json.loads((HERE / DECL_NAME).read_text(encoding="utf-8"))
BATCH02 = json.loads((HERE / "PREDECLARED_BATCH_02.json").read_text(encoding="utf-8"))

SYMBOLS = ["SPY", "QQQ", "IWM", "SMH", "XBI", "XLE"]
FAMILIES = [
    ("F01", "monthly_multihorizon_abs_momentum_consensus_v1", 275),
    ("F02", "trend_filtered_rsi5_reversion_v1", 200),
    ("F03", "trend_filtered_extreme_3d_atr_reversal_v1", 200),
    ("F04", "close_channel_100_50_trend_v1", 101),
    ("F05", "monthly_10month_trend_timing_v1", 253),
    ("F06", "trend_filtered_zscore20_reversion_v1", 200),
    ("F07", "volatility_contraction_breakout_v1", 61),
    ("F08", "monthly_12_minus_1_abs_momentum_v1", 275),
    ("F09", "delayed_overnight_gap_reversal_v1", 22),
    ("F10", "monthly_52week_high_proximity_v1", 274),
]


def load_runner(decl_name: str):
    saved = os.environ.get("MQK_M1_BATCH_DECLARATION")
    os.environ["MQK_M1_BATCH_DECLARATION"] = decl_name
    try:
        spec = importlib.util.spec_from_file_location(f"run_batch_{decl_name}", HERE / "run_batch.py")
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        return mod
    finally:
        if saved is None:
            os.environ.pop("MQK_M1_BATCH_DECLARATION")
        else:
            os.environ["MQK_M1_BATCH_DECLARATION"] = saved


@pytest.fixture(scope="module")
def rb():
    return load_runner(DECL_NAME)


def test_identity_scope_and_window():
    assert DECL["batch_id"] == "m1_native_hypothesis_batch_03"
    assert DECL["experiment"]["real_experiment_id"] == "M1-NATIVE-HYPOTHESIS-BATCH-03-DISCOVERY"
    assert DECL["run_dir"] == "runs/run_batch_03"
    assert DECL["experiment"]["real_experiment_id"] != BATCH02["experiment"]["real_experiment_id"]
    data = DECL["data"]
    assert (data["feed"], data["timeframe"], data["adjustment"]) == ("sip", "1Day", "all")
    assert data["timeframe_identity"] == "canonical_semantic_v1"
    assert (data["start_utc"], data["end_utc"]) == ("2016-01-01T00:00:00Z", "2026-09-01T00:00:00Z")
    assert data["completed_bars_only"] is True
    part = DECL["partition"]
    assert part["holdout_months"] == 6 and "2026-03-01" in part["holdout_rule"]
    assert DECL["holdout"]["status"] == "RESERVED / UNCONSUMED"


def test_sixty_trials_in_family_major_order_over_the_declared_universe(rb):
    assert DECL["universe"]["symbols"] == SYMBOLS
    assert DECL["universe"]["max_trials"] == 60
    expected = [(fid, strategy, sym) for fid, strategy, _ in FAMILIES for sym in SYMBOLS]
    got = [(t["hypothesis_label"], t["strategy_id"], t["symbol"]) for t in DECL["universe"]["trials"]]
    assert got == expected
    assert rb.TRIALS == [(s, y) for _, s, y in expected]
    rb._require_frozen_trial_structure()


def test_hypotheses_are_the_ten_declared_families_with_matching_history(rb):
    assert [(h["hypothesis_label"], h["strategy_id"], h["required_history_bars"])
            for h in DECL["hypotheses"]] == FAMILIES
    assert len({h["hypothesis_id"] for h in DECL["hypotheses"]}) == 10
    for h in DECL["hypotheses"]:
        assert h["direction"] == "long_only_flat" and h["timeframe_secs"] == 86400


@pytest.mark.parametrize("strategy", [s for _, s, _ in FAMILIES])
def test_every_declared_family_is_a_registered_native_engine(strategy):
    src = "".join(p.read_text(encoding="utf-8") for p in ENGINES.glob("*.rs"))
    assert re.search(rf'const NAME: &str = "{strategy}";', src)


def test_canonical_daily_identity_is_opted_in(rb):
    assert rb.canonical_timeframe_identity(DECL) is True


def test_capital_fraction_sizing_is_the_frozen_1000_bps_on_100k(rb):
    block = DECL["capital_sizing"]
    assert block["policy_id"] == "fixed_initial_capital_fraction_v1"
    assert block["allocation_fraction_bps"] == 1000
    assert block["initial_capital_micros"] == 100_000_000_000
    assert block["nominal_entry_budget_micros"] == 10_000_000_000
    assert DECL["native_backtest"]["initial_cash_micros"] == 100_000_000_000
    assert DECL["benchmark"]["policy_id"] == "capital_fraction_matched_passive_buy_hold_v1"
    assert rb.sizing_args(DECL)  # the baseline block validates under the shared sizing authority


def test_stress_contract_is_unfinalized_and_no_batch02_value_is_borrowed(rb):
    stress = DECL["robustness"]["p7a_p7b_stress"]
    assert stress["status"] == "OPERATOR_DECISION_REQUIRED_BATCH03_STRESS_CONTRACT"
    assert "stress_sizing" not in stress
    assert stress["register_stress_contract"].startswith("deliberately absent")
    with pytest.raises(SystemExit):
        rb.stress_plan(DECL)
    with pytest.raises(SystemExit):
        rb.research_stress_contract(DECL)
    old = BATCH02["robustness"]["p7a_p7b_stress"]
    text = json.dumps(stress)
    assert old["stress_sizing"]["scenario_id"] not in text
    assert '"allocation_fraction_bps": 500' not in text


def test_the_gate_refuses_every_stage_and_is_a_noop_for_closed_declarations(rb):
    gate = DECL["execution_gate"]
    assert gate["status"] == "BATCH03_PREDECLARED_NOT_EXECUTED" and gate["executable"] is False
    assert gate["blocker"] == "OPERATOR_DECISION_REQUIRED_BATCH03_STRESS_CONTRACT"
    with pytest.raises(SystemExit, match="OPERATOR_DECISION_REQUIRED_BATCH03_STRESS_CONTRACT"):
        rb.require_executable_declaration(DECL)
    for bad in (False, None, "true", 1):
        mutated = copy.deepcopy(DECL)
        mutated["execution_gate"]["executable"] = bad
        with pytest.raises(SystemExit):
            rb.require_executable_declaration(mutated)
    rb.require_executable_declaration(BATCH02)
    ok = copy.deepcopy(DECL)
    ok["execution_gate"]["executable"] = True
    rb.require_executable_declaration(ok)


@pytest.mark.parametrize("stage", ["check", "register"])
def test_main_runs_no_stage_for_the_unfinalized_declaration(rb, stage, monkeypatch):
    called = []
    monkeypatch.setattr(rb, "STAGES", {stage: lambda _a: called.append(stage)})
    monkeypatch.setattr(sys, "argv", ["run_batch.py", stage])
    with pytest.raises(SystemExit, match="BATCH03_PREDECLARED_NOT_EXECUTED"):
        rb.main()
    assert called == []


def test_stopping_rule_variants_and_paper_live_posture():
    rule = DECL["batch_stopping_rule"]
    assert rule["trials"] == 60 and rule["economic_attempts"] == 0
    assert rule["stop_status"] == "BATCH03_PREDECLARED_NOT_EXECUTED"
    for banned in ("RSI2/3/7", "alternate thresholds", "Donchian variants", "hold lengths", "ATR multipliers",
                   "MA grids", "stops", "individual stocks"):
        assert banned in rule["no_additional_variants"]
    assert DECL["universe"]["additional_candidate_after_predeclaration"] is not True
    assert DECL["paper_live"]["paper"] == "NOT ACTIVATED"
    assert DECL["paper_live"]["live"] == "NOT TOUCHED"
    assert DECL["paper_live"]["daemon_capital_fraction_dispatch"] == "AUTHORIZED_NEXT / NOT YET COMPLETE"


def test_confirmation_universe_is_prepared_and_not_registered():
    conf = DECL["confirmation_preparation"]
    assert conf["status"] == "PREPARED_NOT_REGISTERED"
    assert conf["universe"] == ["DIA", "MDY", "XLF", "XLI", "XLV", "XLP"]
    assert conf["future_trials"] == 18
    assert not set(conf["universe"]) & set(SYMBOLS)
    assert all(t["symbol"] in SYMBOLS for t in DECL["universe"]["trials"])


def test_family_ranking_contract():
    rank = DECL["family_ranking"]
    assert rank["min_evaluable_trials_per_family"] == 4 and rank["trials_per_family"] == 6
    assert len(rank["rank_keys"]) == 6


def test_declared_restart_recovery_matches_the_engine_meta():
    stateless = {"F01", "F05", "F08", "F10"}
    for h in DECL["hypotheses"]:
        want = "BoundedHistoryReconstructible" if h["hypothesis_label"] in stateless else "DurableStateRequired"
        assert h["restart_recovery"] == want, h["hypothesis_label"]
        src = (ENGINES / (h["strategy_id"] + ".rs")).read_text(encoding="utf-8")
        assert ("RestartRecovery::DurableStateRequired" in src) == (want == "DurableStateRequired"), h["strategy_id"]
