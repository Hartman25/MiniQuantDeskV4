"""The Batch 03 pre-run registration gate: exactly the 60 predeclared trial identities, nothing else,
and zero attempts before the first economic attempt.

These controls apply a TEST-FIXTURE stress contract (distinct from the operator-frozen one, so that
"a different contract changes every identity" is exercised) to a restored copy of the declaration.
The fixture values are not authority and are never written to the declaration.
"""

from __future__ import annotations

import copy
import importlib.util
import inspect
import os
import sys
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / "src"))

from mqk_research.data.bars_provenance import (  # noqa: E402
    CA_POLICY_FORBID_AFFECTED_PERIODS,
    PRICE_CONVENTION_RAW_UNADJUSTED,
    UNIVERSE_MODE_FIXED_EX_ANTE,
    build_bars_provenance_manifest,
    build_corporate_action_evidence,
)
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402
import mqk_research.ml.native_signal_registry_integration as bridge  # noqa: E402

os.environ["MQK_M1_BATCH_DECLARATION"] = "PREDECLARED_BATCH_03.json"
try:
    SPEC = importlib.util.spec_from_file_location("run_batch03_gate_under_test", HERE / "run_batch.py")
    rb = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(rb)
finally:
    del os.environ["MQK_M1_BATCH_DECLARATION"]

FIXTURE_STRESS = {
    "register_stress_contract": True,
    "stress_execution_slippage_bps": 7,
    "stress_execution_volatility_mult_bps": 3,
    "max_drawdown_ceiling": 0.3,
    "stress_sizing": {
        "scenario_id": "TEST_FIXTURE_NOT_AUTHORITY",
        "policy_id": "fixed_initial_capital_fraction_v1",
        "allocation_fraction_bps": 400,
        "initial_capital_micros": 100_000_000_000,
        "nominal_entry_budget_micros": 4_000_000_000,
        "quantity_rule": "fixture",
        "is_a_trial": False,
        "identity": "fixture",
    },
}


def manifest(tmp: Path, scale: float = 1.0, timeframe: str = "1D") -> dict:
    rng = np.random.default_rng(5)
    dates = pd.bdate_range("2016-01-04", "2016-06-30", tz="UTC")
    rows = [{"symbol": s, "end_ts": d.isoformat(), "open": 100 * scale, "high": 101 * scale, "low": 99 * scale,
             "close": 100 * scale + float(rng.uniform(-0.9, 0.9)), "volume": 1000}
            for s in rb.DECL["universe"]["symbols"] for d in dates]
    bars = pd.DataFrame(rows)
    bars.to_csv(tmp / f"bars_{scale}_{timeframe}.csv", index=False)
    ts = pd.to_datetime(bars["end_ts"], utc=True)
    symbols = sorted(bars["symbol"].unique())
    start, end = ts.min().isoformat(), (ts.max() + pd.Timedelta(seconds=1)).isoformat()
    ev = build_corporate_action_evidence(
        source_provider_id="fixture", covered_symbol_universe=symbols, coverage_start_utc=start,
        coverage_end_utc=end, corporate_action_entries=())
    return build_bars_provenance_manifest(
        price_provenance={"close_column": "close", "provider_ids_observed": ["fixture"],
                          "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED,
                          "provider_metadata_available": True, "convention_basis": "gate fixture"},
        corporate_action_policy=CA_POLICY_FORBID_AFFECTED_PERIODS, corporate_action_evidence_id=ev["evidence_id"],
        corporate_action_evidence=ev, forbidden_periods=(), timeframe=timeframe, start_utc=start, end_utc=end,
        symbol_universe=symbols, universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE, bars=bars,
        artifact_path=tmp / f"bars_{scale}_{timeframe}.csv")


def fingerprints(override: dict | None = None) -> dict:
    out = {}
    for i, (strategy, sym) in enumerate(rb.TRIALS):
        out[(strategy, sym)] = (f"{i + 1:02x}" * 32, rb.HYP[strategy]["required_history_bars"])
    out.update(override or {})
    return out


@pytest.fixture(autouse=True)
def restore_declaration():
    saved = copy.deepcopy(rb.DECL)
    yield
    rb.DECL.clear()
    rb.DECL.update(saved)


def with_fixture_stress():
    rb.DECL["robustness"]["p7a_p7b_stress"] = copy.deepcopy(FIXTURE_STRESS)


@pytest.fixture
def world(tmp_path, monkeypatch):
    with_fixture_stress()
    monkeypatch.setattr(rb, "REGISTRY", tmp_path / "research.sqlite3")
    return ResearchResultStore(tmp_path / "research.sqlite3"), manifest(tmp_path)


def register(store, expected, skip=()):
    for strategy, sym, trial_id, identity in expected:
        if (strategy, sym) in skip:
            continue
        h = rb.HYP[strategy]
        store.register_hypothesis(hypothesis_id=h["hypothesis_id"], experiment_id=rb.EXPERIMENT,
                                  hypothesis_text=h["economic_rationale"])
        store.register_trial(trial_id=trial_id, experiment_id=rb.EXPERIMENT, hypothesis_id=h["hypothesis_id"],
                             strategy_id=strategy, protocol_id=bridge.ECONOMIC_PROTOCOL_ID, identity=identity)


def gate(store, expected, zero=True):
    return rb.registration_gate(store, rb.EXPERIMENT, expected, require_zero_attempts=zero)


def ids(expected):
    return {e[2] for e in expected}


def test_the_shipped_declaration_binds_the_frozen_stress_contract_into_all_sixty_identities(tmp_path):
    shipped = rb.expected_trial_ids(fingerprints(), manifest(tmp_path))
    assert len(shipped) == 60 and len(ids(shipped)) == 60
    frozen = rb.research_stress_contract(rb.DECL)
    assert frozen["scenario_id"] == "half_exposure_capital_fraction_500bps_v1"
    assert (frozen["allocation_fraction_bps"], frozen["stress_execution_slippage_bps"],
            frozen["stress_execution_volatility_mult_bps"], frozen["max_drawdown_ceiling_bps"]) == (500, 15, 10, 4000)
    for variant in ("stress_execution_slippage_bps", "stress_execution_volatility_mult_bps"):
        rb.DECL["robustness"]["p7a_p7b_stress"][variant] += 1
        other = rb.expected_trial_ids(fingerprints(), manifest(tmp_path))
        assert ids(shipped).isdisjoint(ids(other)), variant
        rb.DECL["robustness"]["p7a_p7b_stress"][variant] -= 1


def test_exactly_the_sixty_predeclared_identities_with_zero_attempts_pass(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    assert [(s, y) for s, y, *_ in expected] == rb.TRIALS and len(expected) == 60
    assert len(ids(expected)) == 60
    register(store, expected)
    assert gate(store, expected) == {"registered": 60, "attempts": 0}


def test_fifty_nine_registered_trials_block_the_campaign(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected, skip={rb.TRIALS[-1]})
    with pytest.raises(SystemExit, match="59 registered"):
        gate(store, expected)


def test_sixty_one_registered_trials_block_the_campaign(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected)
    extra = rb.expected_trial_ids(fingerprints({rb.TRIALS[0]: ("ee" * 32, 1)}), man)[0]
    register(store, [extra])
    with pytest.raises(SystemExit, match="61 registered"):
        gate(store, expected)


def test_a_contract_that_maps_two_slots_to_one_identity_is_refused(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected)
    dup = list(expected)
    dup[1] = dup[0]
    with pytest.raises(SystemExit, match="two slots to one trial identity"):
        gate(store, dup)


def test_an_attempt_before_the_gate_blocks_it_but_a_retry_never_creates_a_trial(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected)
    for _ in range(2):  # a first attempt and an infrastructure retry of the SAME trial
        store.begin_attempt(trial_id=expected[0][2], origin="test", metadata={})
    with pytest.raises(SystemExit, match="requires zero"):
        gate(store, expected, zero=True)
    assert gate(store, expected, zero=False) == {"registered": 60, "attempts": 2}


def test_each_family_fingerprint_moves_exactly_its_own_six_trials(world):
    store, man = world
    base = rb.expected_trial_ids(fingerprints(), man)
    base_ids = {(s, y): t for s, y, t, _ in base}
    for _, strategy, _ in [(h["hypothesis_label"], h["strategy_id"], 0) for h in rb.DECL["hypotheses"]]:
        moved = {(s, y): ("ab" * 32, rb.HYP[s]["required_history_bars"]) for s, y in rb.TRIALS if s == strategy}
        other = {(s, y): t for s, y, t, _ in rb.expected_trial_ids(fingerprints(moved), man)}
        for slot, trial_id in base_ids.items():
            assert (other[slot] != trial_id) == (slot[0] == strategy), (strategy, slot)
    register(store, rb.expected_trial_ids(fingerprints({rb.TRIALS[0]: ("ab" * 32, 275)}), man))
    with pytest.raises(SystemExit, match="differ from the predeclared contract"):
        gate(store, base)


def test_one_day_and_one_d_labels_make_one_identity_under_the_canonical_opt_in(world, tmp_path):
    _, man_1d = world
    man_1day = manifest(tmp_path, timeframe="1Day")
    assert rb.canonical_timeframe_identity(rb.DECL) is True
    assert man_1d["timeframe"] == "1D" and man_1day["timeframe"] == "1Day"
    a = rb.expected_trial_ids(fingerprints(), man_1d)
    b = rb.expected_trial_ids(fingerprints(), man_1day)
    assert len(a) == 60
    assert [x[2] for x in a] == [x[2] for x in b]
    assert {x[3]["data_identity"]["bars_provenance"]["timeframe"] for x in a + b} == {"1D"}


def test_dropping_the_canonical_opt_in_makes_every_identity_disjoint(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    del rb.DECL["data"]["timeframe_identity"]
    legacy = rb.expected_trial_ids(fingerprints(), man)
    assert ids(expected).isdisjoint(ids(legacy))
    register(store, legacy)
    with pytest.raises(SystemExit, match="differ from the predeclared contract"):
        gate(store, expected)


@pytest.mark.parametrize("label,patch", [
    ("wrong bps", lambda d: d["capital_sizing"].update(allocation_fraction_bps=1001,
                                                       nominal_entry_budget_micros=10_010_000_000)),
    ("wrong cost model", lambda d: d["economic_protocol"]["cost_model"].update(commission_bps_per_side=5.0)),
    ("wrong execution slippage", lambda d: d["economic_protocol"]["execution_pricing"].update(slippage_bps=1)),
    ("wrong partition", lambda d: d["partition"].update(test_months=6)),
    ("fixed-quantity fallback", lambda d: d.pop("capital_sizing")),
])
def test_wrong_sizing_cost_partition_or_a_fixed_quantity_fallback_changes_every_identity(world, label, patch):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    patch(rb.DECL)
    try:
        other = rb.expected_trial_ids(fingerprints(), man)
    except SystemExit:
        return  # refused outright is also fail-closed
    assert ids(expected).isdisjoint(ids(other)), label
    register(store, other)
    with pytest.raises(SystemExit, match="differ from the predeclared contract"):
        gate(store, expected)


def test_a_different_stress_contract_changes_every_trial_identity(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    rb.DECL["robustness"]["p7a_p7b_stress"]["stress_execution_slippage_bps"] = 8
    other = rb.expected_trial_ids(fingerprints(), man)
    assert ids(expected).isdisjoint(ids(other))
    register(store, other)
    with pytest.raises(SystemExit, match="differ from the predeclared contract"):
        gate(store, expected)


def test_a_wrong_benchmark_is_refused(world):
    rb.DECL["scanner_review"]["benchmark_policy"] = rb.BENCHMARK_V2
    with pytest.raises(SystemExit, match="benchmark_policy"):
        rb.sizing_args(rb.DECL)


def test_wrong_data_provenance_changes_every_trial_identity(world, tmp_path):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    other = rb.expected_trial_ids(fingerprints(), manifest(tmp_path, scale=2.0))
    assert ids(expected).isdisjoint(ids(other))
    register(store, other)
    with pytest.raises(SystemExit, match="differ from the predeclared contract"):
        gate(store, expected)


def test_trial_identity_has_no_result_input_and_is_reproducible(world):
    _, man = world
    assert [e[2] for e in rb.expected_trial_ids(fingerprints(), man)] == \
        [e[2] for e in rb.expected_trial_ids(fingerprints(), man)]
    params = set(inspect.signature(bridge.build_native_signal_trial_identity).parameters)
    forbidden = {"sharpe", "dsr", "pbo", "alpha", "drawdown", "profit_factor", "trade_count", "scanner_result",
                 "review_state", "winner", "result", "return", "returns", "economic_eval_id"}
    assert params.isdisjoint(forbidden), params & forbidden


def test_a_holdout_period_bar_never_reaches_the_discovery_bars(tmp_path):
    dates = pd.bdate_range("2016-01-04", "2026-08-31", tz="UTC")
    bars = pd.DataFrame({"symbol": "SPY", "end_ts": [d.isoformat() for d in dates], "open": 1.0, "high": 1.0,
                         "low": 1.0, "close": 1.0, "volume": 1})
    src = tmp_path / "bars.csv"
    bars.to_csv(src, index=False)
    hold = bridge.native_holdout_start(src, "SPY", rb.DECL["partition"]["holdout_months"])
    assert hold == pd.Timestamp("2026-03-01", tz="UTC")
    out = bridge.research_bars_to_backtest_csv(src, "SPY", tmp_path / "bt.csv", end_exclusive_utc=hold)
    last = pd.to_datetime(pd.read_csv(out)["end_ts"].astype("int64"), unit="s", utc=True).max()
    assert last < hold
