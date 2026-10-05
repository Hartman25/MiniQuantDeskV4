"""Guards the Batch 03 prospective predeclaration (60 trials, ten families, six ETFs).

The operator-frozen P7A/P7B stress contract is pinned here. The execution gate is authorized for the
discovery population only; a closed gate must still refuse every runner stage.
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


def _bars(rb, per_symbol: dict[str, int]):
    import pandas as pd
    return pd.DataFrame([{"symbol": s, "end_ts": i} for s, n in per_symbol.items() for i in range(n)])


def test_data_declares_an_explicit_asof_and_no_fallback_window():
    data = DECL["data"]
    assert re.fullmatch(r"\d{4}-\d{2}-\d{2}", data["asof"])
    assert "fallback_if_corporate_action_gate_refuses" not in data


def test_fetch_verification_accepts_the_exact_universe_with_sufficient_history(rb):
    need = max(h["required_history_bars"] for h in DECL["hypotheses"])
    counts = rb.verify_fetched_bars(DECL, _bars(rb, {s: need for s in SYMBOLS}))
    assert counts == {s: need for s in SYMBOLS}


@pytest.mark.parametrize("mutate", ["missing", "extra", "short"])
def test_fetch_verification_fails_closed_without_substitution(rb, mutate):
    need = max(h["required_history_bars"] for h in DECL["hypotheses"])
    per = {s: need for s in SYMBOLS}
    if mutate == "missing":
        del per["XLE"]
    elif mutate == "extra":
        per["DIA"] = need
    else:
        per["XBI"] = need - 1
    with pytest.raises(SystemExit, match="fail-closed"):
        rb.verify_fetched_bars(DECL, _bars(rb, per))


def test_fetch_requires_execute_and_never_overwrites(rb, tmp_path, monkeypatch):
    class A:
        execute = False
    with pytest.raises(SystemExit, match="--execute"):
        rb.stage_fetch(A())
    monkeypatch.setattr(rb, "RUN", tmp_path)
    (tmp_path / "data").mkdir()
    (tmp_path / "data" / "research_bars.csv").write_text("x", encoding="utf-8")
    A.execute = True
    with pytest.raises(SystemExit, match="refusing to overwrite"):
        rb.stage_fetch(A())


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


def test_stress_contract_is_the_operator_frozen_half_exposure_contract(rb):
    stress = DECL["robustness"]["p7a_p7b_stress"]
    assert stress["status"] == "OPERATOR_FROZEN_BATCH03_STRESS_CONTRACT"
    assert stress["register_stress_contract"] is True
    sizing = stress["stress_sizing"]
    assert sizing["scenario_id"] == "half_exposure_capital_fraction_500bps_v1"
    assert sizing["allocation_fraction_bps"] == 500 and sizing["nominal_entry_budget_micros"] == 5_000_000_000
    assert sizing["is_a_trial"] is False
    assert not any(f in stress for f in rb.STRESS_FORBIDDEN_CAP_FIELDS)  # not a USD 5,000 cap
    assert rb.stress_plan(DECL) == {"mode": "capital_fraction", "scenario_id": sizing["scenario_id"],
                                    "allocation_fraction_bps": 500}
    contract = rb.research_stress_contract(DECL)
    assert contract["scenario_id"] == "half_exposure_capital_fraction_500bps_v1"
    assert (contract["allocation_fraction_bps"], contract["stress_execution_slippage_bps"],
            contract["stress_execution_volatility_mult_bps"], contract["max_drawdown_ceiling_bps"]) == (
        500, 15, 10, 4000)
    # the baseline sizing (and so every trial identity input) stays 1000 bps / USD 10,000
    assert DECL["capital_sizing"]["allocation_fraction_bps"] == 1000
    assert DECL["capital_sizing"]["nominal_entry_budget_micros"] == 10_000_000_000


def test_stress_is_at_least_as_adverse_as_the_canonical_baseline_execution_and_strictly_worse_in_one():
    base = DECL["economic_protocol"]["execution_pricing"]
    stress = DECL["robustness"]["p7a_p7b_stress"]
    s_slip, s_vol = stress["stress_execution_slippage_bps"], stress["stress_execution_volatility_mult_bps"]
    assert s_slip >= base["slippage_bps"] and s_vol >= base["volatility_mult_bps"]
    assert s_slip > base["slippage_bps"] or s_vol > base["volatility_mult_bps"]


CLOSED_STATUS = "BATCH03_PREDECLARED_NOT_EXECUTED"


def closed_copy():
    closed = copy.deepcopy(DECL)
    closed["execution_gate"].update({"status": CLOSED_STATUS, "executable": False,
                                     "blocker": "OPERATOR_EXECUTION_AUTHORIZATION_REQUIRED_BATCH03"})
    return closed


def test_the_authorized_gate_is_executable_and_scoped_to_discovery_only(rb):
    gate = DECL["execution_gate"]
    assert gate["status"] == "BATCH03_DISCOVERY_EXECUTION_AUTHORIZED" and gate["executable"] is True
    assert gate["blocker"] is None
    rule = gate["rule"]
    for forbidden in ("confirmation trial", "holdout consumption", "Promotion", "Paper deployment", "Live"):
        assert forbidden in rule
    rb.require_executable_declaration(DECL)


def test_a_non_literal_true_gate_refuses_and_closed_declarations_are_unaffected(rb):
    with pytest.raises(SystemExit, match="OPERATOR_EXECUTION_AUTHORIZATION_REQUIRED_BATCH03"):
        rb.require_executable_declaration(closed_copy())
    for bad in (False, None, "true", 1):
        mutated = copy.deepcopy(DECL)
        mutated["execution_gate"]["executable"] = bad
        with pytest.raises(SystemExit):
            rb.require_executable_declaration(mutated)
    rb.require_executable_declaration(BATCH02)


def test_a_closed_gate_refuses_every_runner_stage_including_fetch(rb, monkeypatch):
    called = []
    monkeypatch.setattr(rb, "DECL", closed_copy())
    monkeypatch.setattr(rb, "STAGES", {name: (lambda _a, n=name: called.append(n)) for name in rb.STAGES})
    assert {"check", "fetch", "register"} <= set(rb.STAGES)
    for name in sorted(rb.STAGES):
        monkeypatch.setattr(sys, "argv", ["run_batch.py", name])
        with pytest.raises(SystemExit, match=CLOSED_STATUS):
            rb.main()
    assert called == []


def test_the_open_gate_dispatches_the_requested_stage(rb, monkeypatch):
    called = []
    monkeypatch.setattr(rb, "STAGES", {"check": lambda _a: called.append("check")})
    monkeypatch.setattr(sys, "argv", ["run_batch.py", "check"])
    rb.main()
    assert called == ["check"]


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
