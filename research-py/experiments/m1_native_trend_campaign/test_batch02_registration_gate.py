"""The Batch 02 pre-run registration gate: exactly the 15 predeclared trial identities, nothing
else, and zero attempts before the first economic attempt.

Every control below builds the registry through the real identity construction and then asserts
the gate refuses the specific defect; the gate never reads a result value.
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

os.environ["MQK_M1_BATCH_DECLARATION"] = "PREDECLARED_BATCH_02.json"
try:
    SPEC = importlib.util.spec_from_file_location("run_batch_gate_under_test", HERE / "run_batch.py")
    rb = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(rb)
finally:
    del os.environ["MQK_M1_BATCH_DECLARATION"]


def manifest(tmp: Path, scale: float = 1.0) -> dict:
    rng = np.random.default_rng(5)
    dates = pd.bdate_range("2016-01-04", "2016-06-30", tz="UTC")
    rows = [{"symbol": s, "end_ts": d.isoformat(), "open": 100 * scale, "high": 101 * scale, "low": 99 * scale,
             "close": 100 * scale + float(rng.uniform(-0.9, 0.9)), "volume": 1000} for s in rb.DECL["universe"]["symbols"]
            for d in dates]
    bars = pd.DataFrame(rows)
    bars.to_csv(tmp / f"bars_{scale}.csv", index=False)
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
        corporate_action_evidence=ev, forbidden_periods=(), timeframe="1D", start_utc=start, end_utc=end,
        symbol_universe=symbols, universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE, bars=bars,
        artifact_path=tmp / f"bars_{scale}.csv")


def fingerprints(override: dict | None = None) -> dict:
    out = {}
    for i, (strategy, sym) in enumerate(rb.TRIALS):
        required = rb.HYP[strategy]["required_history_bars"]
        out[(strategy, sym)] = (f"{i + 1:02x}" * 32, required)
    out.update(override or {})
    return out


@pytest.fixture
def world(tmp_path, monkeypatch):
    monkeypatch.setattr(rb, "REGISTRY", tmp_path / "research.sqlite3")
    man = manifest(tmp_path)
    store = ResearchResultStore(tmp_path / "research.sqlite3")
    return store, man


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


def test_exactly_the_fifteen_predeclared_identities_with_zero_attempts_pass(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    assert [(s, y) for s, y, *_ in expected] == rb.TRIALS and len(expected) == 15
    assert len({e[2] for e in expected}) == 15
    register(store, expected)
    assert gate(store, expected) == {"registered": 15, "attempts": 0}


def test_fourteen_registered_trials_block_the_campaign(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected, skip={rb.TRIALS[-1]})
    with pytest.raises(SystemExit, match="14 registered"):
        gate(store, expected)


def test_sixteen_registered_trials_block_the_campaign(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected)
    extra_fp = fingerprints({rb.TRIALS[0]: ("ee" * 32, 1)})
    extra = rb.expected_trial_ids(extra_fp, man)[0]
    register(store, [extra])
    with pytest.raises(SystemExit, match="16 registered"):
        gate(store, expected)


def test_a_contract_that_maps_two_slots_to_one_identity_is_refused(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected)
    dup = list(expected)
    dup[1] = dup[0]
    with pytest.raises(SystemExit, match="two slots to one trial identity"):
        gate(store, dup)


@pytest.mark.parametrize("label,swap", [
    ("wrong symbol", lambda fp: {("turn_of_month_last1_first3", "SPY"): ("11" * 32, 1)}),
    ("wrong inner strategy fingerprint", lambda fp: {("halloween_nov_apr", "EFA"): ("22" * 32, 1)}),
    ("wrong H3 history requirement", lambda fp: {("trading_range_breakout_50d_hold10", "GLD"): (fp[("trading_range_breakout_50d_hold10", "GLD")][0], 59)}),
])
def test_a_registered_trial_that_differs_from_the_contract_blocks(world, label, swap):
    store, man = world
    good = fingerprints()
    expected = rb.expected_trial_ids(good, man)
    bad = rb.expected_trial_ids({**good, **swap(good)}, man)
    register(store, bad)
    with pytest.raises(SystemExit, match="differ from the predeclared contract"):
        gate(store, expected)


def _expected_with(decl_patch, world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    saved = copy.deepcopy(rb.DECL)
    try:
        decl_patch(rb.DECL)
        other = rb.expected_trial_ids(fingerprints(), man)
    finally:
        rb.DECL.clear()
        rb.DECL.update(saved)
    return store, expected, other


@pytest.mark.parametrize("label,patch", [
    ("wrong bps", lambda d: d["capital_sizing"].update(allocation_fraction_bps=1001, nominal_entry_budget_micros=10_010_000_000)),
    ("wrong capital", lambda d: (d["native_backtest"].update(initial_cash_micros=90_000_000_000),
                                 d["economic_protocol"]["weight_to_share"].update(equity_usd=90000.0),
                                 d["capital_sizing"].update(initial_capital_micros=90_000_000_000,
                                                            nominal_entry_budget_micros=9_000_000_000))),
    ("wrong partition", lambda d: d["partition"].update(test_months=6)),
    ("wrong cost model", lambda d: d["economic_protocol"]["cost_model"].update(commission_bps_per_side=5.0)),
    ("wrong execution slippage", lambda d: d["economic_protocol"]["execution_pricing"].update(slippage_bps=1)),
    ("wrong cap", lambda d: d["capital_sizing"].update(max_position_notional_usd=60000)),
])
def test_wrong_sizing_capital_partition_or_cost_changes_every_trial_identity(world, label, patch):
    store, expected, other = _expected_with(patch, world)
    assert {e[2] for e in expected}.isdisjoint({e[2] for e in other}), label
    register(store, other)
    with pytest.raises(SystemExit, match="differ from the predeclared contract"):
        gate(store, expected)


def test_wrong_data_provenance_changes_every_trial_identity(world, tmp_path):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    other = rb.expected_trial_ids(fingerprints(), manifest(tmp_path, scale=2.0))
    assert {e[2] for e in expected}.isdisjoint({e[2] for e in other})
    register(store, other)
    with pytest.raises(SystemExit, match="differ from the predeclared contract"):
        gate(store, expected)


def test_an_attempt_before_the_gate_blocks_it_but_a_retry_never_creates_a_trial(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected)
    trial_id = expected[0][2]
    for _ in range(2):  # a first attempt and an infrastructure retry of the SAME trial
        store.begin_attempt(trial_id=trial_id, origin="test", metadata={})
    with pytest.raises(SystemExit, match="requires zero"):
        gate(store, expected, zero=True)
    assert gate(store, expected, zero=False) == {"registered": 15, "attempts": 2}


def test_trial_identity_has_no_result_input_and_is_reproducible(world):
    store, man = world
    a = [e[2] for e in rb.expected_trial_ids(fingerprints(), man)]
    b = [e[2] for e in rb.expected_trial_ids(fingerprints(), man)]
    assert a == b
    params = set(inspect.signature(bridge.build_native_signal_trial_identity).parameters)
    forbidden = {"sharpe", "dsr", "pbo", "alpha", "drawdown", "profit_factor", "trade_count", "scanner_result",
                 "review_state", "winner", "result", "return", "returns", "economic_eval_id"}
    assert params.isdisjoint(forbidden), params & forbidden


def test_the_check_stage_pins_trial_structure(monkeypatch):
    rb._require_frozen_trial_structure()
    bad = copy.deepcopy(rb.DECL)
    bad["universe"]["trials"][3]["order"] = 99
    monkeypatch.setattr(rb, "DECL", bad)
    with pytest.raises(SystemExit, match="trial order"):
        rb._require_frozen_trial_structure()


def test_a_calendar_identity_change_moves_exactly_the_calendar_dependent_trials(world):
    """H1/H2 fingerprints bind the calendar content hash (Rust: engines' fingerprint tests); a different
    calendar therefore yields different H1/H2 trial identities and cannot satisfy the contract, while the
    calendar-free H3 slots keep theirs."""
    store, man = world
    base = fingerprints()
    other_calendar = {k: ("ab" * 32, v[1]) for k, v in base.items() if k[0] != "trading_range_breakout_50d_hold10"}
    a = {(s, y): t for s, y, t, _ in rb.expected_trial_ids(base, man)}
    b = {(s, y): t for s, y, t, _ in rb.expected_trial_ids({**base, **other_calendar}, man)}
    for key in a:
        assert (a[key] != b[key]) == (key[0] != "trading_range_breakout_50d_hold10"), key
    register(store, rb.expected_trial_ids({**base, **other_calendar}, man))
    with pytest.raises(SystemExit, match="differ from the predeclared contract"):
        gate(store, rb.expected_trial_ids(base, man))
