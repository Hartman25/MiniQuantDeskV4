"""The KISS pre-run registration gate: exactly the 4 predeclared trial identities of the one hypothesis,
nothing else, and zero attempts before the first economic attempt. Synthetic provenance only: no provider,
no real registry, no economics.

The identity inputs below are TEST FIXTURES (synthetic bars provenance, synthetic fingerprints); they are
never written to the declaration, and the four real trial ids are not computable before the fetch.
"""

from __future__ import annotations

import copy
import importlib.util
import inspect
import json
import os
import sqlite3
import sys
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
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
import holdout_guard as hg  # noqa: E402

DECL_NAME = "PREDECLARED_KISS_EXT032_ETF_01.json"
os.environ["MQK_M1_BATCH_DECLARATION"] = DECL_NAME
try:
    SPEC = importlib.util.spec_from_file_location("run_batch_kiss_gate_under_test", HERE / "run_batch.py")
    rb = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(rb)
finally:
    del os.environ["MQK_M1_BATCH_DECLARATION"]

STRATEGY = "pre_holiday_two_session_long_v1"
SYMBOLS = ["SPY", "QQQ", "IWM", "DIA"]


def manifest(tmp: Path, scale: float = 1.0, timeframe: str = "1D", symbols=None) -> dict:
    rng = np.random.default_rng(5)
    dates = pd.bdate_range("2016-01-04", "2016-06-30", tz="UTC")
    syms = list(symbols or rb.DECL["universe"]["symbols"])
    rows = [{"symbol": s, "end_ts": d.isoformat(), "open": 100 * scale, "high": 101 * scale, "low": 99 * scale,
             "close": 100 * scale + float(rng.uniform(-0.9, 0.9)), "volume": 1000} for s in syms for d in dates]
    bars = pd.DataFrame(rows)
    path = tmp / f"bars_{scale}_{timeframe}_{'-'.join(syms)}.csv"
    bars.to_csv(path, index=False)
    ts = pd.to_datetime(bars["end_ts"], utc=True)
    start, end = ts.min().isoformat(), (ts.max() + pd.Timedelta(seconds=1)).isoformat()
    ev = build_corporate_action_evidence(source_provider_id="fixture", covered_symbol_universe=sorted(syms),
                                         coverage_start_utc=start, coverage_end_utc=end, corporate_action_entries=())
    return build_bars_provenance_manifest(
        price_provenance={"close_column": "close", "provider_ids_observed": ["fixture"],
                          "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED,
                          "provider_metadata_available": True, "convention_basis": "gate fixture"},
        corporate_action_policy=CA_POLICY_FORBID_AFFECTED_PERIODS, corporate_action_evidence_id=ev["evidence_id"],
        corporate_action_evidence=ev, forbidden_periods=(), timeframe=timeframe, start_utc=start, end_utc=end,
        symbol_universe=sorted(syms), universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE, bars=bars, artifact_path=path)


def fingerprints(override: dict | None = None) -> dict:
    out = {(s, y): (f"{i + 1:02x}" * 32, rb.HYP[s]["required_history_bars"]) for i, (s, y) in enumerate(rb.TRIALS)}
    out.update(override or {})
    return out


@pytest.fixture(autouse=True)
def restore_declaration():
    saved = copy.deepcopy(rb.DECL)
    yield
    rb.DECL.clear()
    rb.DECL.update(saved)


@pytest.fixture
def world(tmp_path, monkeypatch):
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


def test_the_declared_contract_yields_exactly_four_distinct_identities_for_one_hypothesis(world):
    _, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    assert [(s, y) for s, y, *_ in expected] == [(STRATEGY, y) for y in SYMBOLS] == rb.TRIALS
    assert len(expected) == 4 == len(ids(expected))
    assert {e[3]["hypothesis_id"] for e in expected} == {rb.HYP[STRATEGY]["hypothesis_id"]}
    assert {e[3]["signal_source"]["capital_sizing"]["allocation_fraction_bps"] for e in expected} == {1000}
    frozen = rb.research_stress_contract(rb.DECL)
    assert (frozen["scenario_id"], frozen["allocation_fraction_bps"], frozen["stress_execution_slippage_bps"],
            frozen["stress_execution_volatility_mult_bps"], frozen["max_drawdown_ceiling_bps"]) == (
        "half_exposure_capital_fraction_500bps_v1", 500, 15, 10, 4000)
    assert {e[3]["signal_source"]["stress_contract"]["scenario_id"] for e in expected} == {frozen["scenario_id"]}


def test_exactly_four_registered_identities_with_zero_attempts_pass(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected)
    assert gate(store, expected) == {"registered": 4, "attempts": 0}


def test_three_registered_trials_block_the_campaign(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected, skip={rb.TRIALS[-1]})
    with pytest.raises(SystemExit, match="3 registered"):
        gate(store, expected)


def test_five_registered_trials_block_the_campaign(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected)
    extra = rb.expected_trial_ids(fingerprints({rb.TRIALS[0]: ("ee" * 32, 2)}), man)[0]
    register(store, [extra])
    with pytest.raises(SystemExit, match="5 registered"):
        gate(store, expected)


def test_a_fifth_symbol_is_not_a_declared_slot(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    trial_id, identity = bridge.build_native_signal_trial_identity(
        experiment_id=rb.EXPERIMENT, hypothesis_id=rb.HYP[STRATEGY]["hypothesis_id"], strategy_id=STRATEGY, symbol="XLE",
        semantic_fingerprint="cc" * 32, required_history_bars=2, bars_provenance=man,
        evaluation_start_utc=pd.Timestamp(rb.DECL["partition"]["evaluation_start_utc"]), test_months=12,
        holdout_months=6, economic_spec=rb._economic_spec(), capital_sizing=rb.research_capital_sizing(rb.DECL),
        stress_contract=rb.research_stress_contract(rb.DECL), canonical_timeframe_identity=True)
    register(store, expected)
    store.register_trial(trial_id=trial_id, experiment_id=rb.EXPERIMENT, hypothesis_id=rb.HYP[STRATEGY]["hypothesis_id"],
                         strategy_id=STRATEGY, protocol_id=bridge.ECONOMIC_PROTOCOL_ID, identity=identity)
    with pytest.raises(SystemExit, match="5 registered"):
        gate(store, expected)


def test_a_contract_that_maps_two_slots_to_one_identity_is_refused(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected)
    dup = list(expected)
    dup[1] = dup[0]
    with pytest.raises(SystemExit, match="two slots to one trial identity"):
        gate(store, dup)


def test_a_row_carrying_the_right_id_but_a_foreign_strategy_or_identity_is_refused(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    h = rb.HYP[STRATEGY]
    store.register_hypothesis(hypothesis_id=h["hypothesis_id"], experiment_id=rb.EXPERIMENT,
                              hypothesis_text=h["economic_rationale"])
    for k, (strategy, sym, trial_id, identity) in enumerate(expected):
        tampered = copy.deepcopy(identity)
        if k == 0:
            row_strategy = "foreign_strategy"  # right id and identity, wrong strategy column
        else:
            row_strategy = strategy
            if k == 1:
                tampered["signal_source"]["symbol"] = "XLE"  # right id, identity text rewritten
        store.register_trial(trial_id=trial_id, experiment_id=rb.EXPERIMENT, hypothesis_id=h["hypothesis_id"],
                             strategy_id=row_strategy, protocol_id=bridge.ECONOMIC_PROTOCOL_ID, identity=tampered)
    with pytest.raises(SystemExit, match="does not carry the predeclared identity"):
        gate(store, expected)


def test_registering_the_same_trial_twice_does_not_create_a_fifth_trial(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected)
    try:
        register(store, expected[:1])
    except Exception:  # either idempotent or refused: never a new row
        pass
    assert len(store.list_trials(experiment_id=rb.EXPERIMENT)) == 4
    assert gate(store, expected) == {"registered": 4, "attempts": 0}


def test_an_attempt_before_the_gate_blocks_it_but_a_retry_never_creates_a_trial(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected)
    for _ in range(3):  # a first attempt and two infrastructure retries of the SAME trial
        store.begin_attempt(trial_id=expected[0][2], origin="test", metadata={})
    with pytest.raises(SystemExit, match="requires zero"):
        gate(store, expected, zero=True)
    assert gate(store, expected, zero=False) == {"registered": 4, "attempts": 3}
    assert len(store.list_trials(experiment_id=rb.EXPERIMENT)) == 4, "a retry is an attempt, never a trial"


def test_an_attempt_on_an_unregistered_trial_is_refused_by_the_store(world):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    with pytest.raises(Exception):
        store.begin_attempt(trial_id=expected[0][2], origin="test", metadata={})
    assert len(store.list_trials(experiment_id=rb.EXPERIMENT)) == 0


def test_the_trials_stage_refuses_before_any_attempt_unless_all_four_are_registered(world, tmp_path, monkeypatch):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    register(store, expected, skip={rb.TRIALS[-1]})
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(json.dumps(man), encoding="utf-8")
    monkeypatch.setattr(rb, "MANIFEST", manifest_path)
    with pytest.raises(SystemExit, match="every predeclared trial must be registered"):
        rb.stage_trials(None)
    con = sqlite3.connect(tmp_path / "research.sqlite3")
    assert con.execute("select count(*) from research_attempts").fetchone()[0] == 0
    con.close()


def test_each_symbol_fingerprint_moves_exactly_its_own_trial(world):
    _, man = world
    base = {(s, y): t for s, y, t, _ in rb.expected_trial_ids(fingerprints(), man)}
    for target in rb.TRIALS:
        other = {(s, y): t for s, y, t, _ in rb.expected_trial_ids(fingerprints({target: ("ab" * 32, 2)}), man)}
        for slot, trial_id in base.items():
            assert (other[slot] != trial_id) == (slot == target), (target, slot)


def test_a_drifted_fingerprint_is_not_the_predeclared_identity(world):
    store, man = world
    base = rb.expected_trial_ids(fingerprints(), man)
    register(store, rb.expected_trial_ids(fingerprints({rb.TRIALS[0]: ("ab" * 32, 2)}), man))
    with pytest.raises(SystemExit, match="differ from the predeclared contract"):
        gate(store, base)


def test_one_day_and_one_d_labels_make_one_identity(world, tmp_path):
    _, man_1d = world
    man_1day = manifest(tmp_path, timeframe="1Day")
    assert man_1d["timeframe"] == "1D" and man_1day["timeframe"] == "1Day"
    a, b = rb.expected_trial_ids(fingerprints(), man_1d), rb.expected_trial_ids(fingerprints(), man_1day)
    assert [x[2] for x in a] == [x[2] for x in b] and len(a) == 4
    del rb.DECL["data"]["timeframe_identity"]
    legacy = rb.expected_trial_ids(fingerprints(), man_1d)
    assert ids(a).isdisjoint(ids(legacy))


@pytest.mark.parametrize("label,patch", [
    ("wrong bps", lambda d: d["capital_sizing"].update(allocation_fraction_bps=1001, nominal_entry_budget_micros=10_010_000_000)),
    ("wrong cost model", lambda d: d["economic_protocol"]["cost_model"].update(commission_bps_per_side=5.0)),
    ("wrong execution slippage", lambda d: d["economic_protocol"]["execution_pricing"].update(slippage_bps=1)),
    ("wrong partition", lambda d: d["partition"].update(test_months=6)),
    ("wrong holdout months", lambda d: d["partition"].update(holdout_months=3)),
    ("fixed-quantity fallback", lambda d: d.pop("capital_sizing")),
    ("different stress", lambda d: d["robustness"]["p7a_p7b_stress"].update(stress_execution_slippage_bps=8)),
])
def test_wrong_sizing_cost_partition_stress_or_a_fixed_quantity_fallback_changes_every_identity(world, label, patch):
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


def test_a_wrong_benchmark_is_refused(world):
    rb.DECL["scanner_review"]["benchmark_policy"] = rb.BENCHMARK_V2
    with pytest.raises(SystemExit, match="benchmark_policy"):
        rb.sizing_args(rb.DECL)


def test_bars_provenance_changes_every_identity_including_a_different_symbol_universe(world, tmp_path):
    store, man = world
    expected = rb.expected_trial_ids(fingerprints(), man)
    other = rb.expected_trial_ids(fingerprints(), manifest(tmp_path, scale=2.0))
    assert ids(expected).isdisjoint(ids(other))
    swapped = manifest(tmp_path, symbols=["SPY", "QQQ", "IWM", "XLE"])
    assert ids(expected).isdisjoint(ids(rb.expected_trial_ids(fingerprints(), swapped)))
    register(store, other)
    with pytest.raises(SystemExit, match="differ from the predeclared contract"):
        gate(store, expected)


def test_trial_identity_has_no_result_input_and_is_reproducible(world):
    _, man = world
    assert [e[2] for e in rb.expected_trial_ids(fingerprints(), man)] == [e[2] for e in rb.expected_trial_ids(fingerprints(), man)]
    params = set(inspect.signature(bridge.build_native_signal_trial_identity).parameters)
    forbidden = {"sharpe", "dsr", "pbo", "alpha", "drawdown", "profit_factor", "trade_count", "scanner_result",
                 "review_state", "winner", "result", "return", "returns", "economic_eval_id"}
    assert params.isdisjoint(forbidden), params & forbidden
    blob = json.dumps(rb.expected_trial_ids(fingerprints(), man)[0][3])
    assert "pre_holiday" in blob and "sharpe" not in blob.lower()


def test_the_holdout_start_is_2026_03_01_and_a_holdout_row_never_reaches_discovery_bars(tmp_path):
    dates = pd.bdate_range("2016-01-04", "2026-08-31", tz="UTC")
    bars = pd.DataFrame({"symbol": "SPY", "end_ts": [d.isoformat() for d in dates], "open": 1.0, "high": 1.0,
                         "low": 1.0, "close": 1.0, "volume": 1})
    src = tmp_path / "bars.csv"
    bars.to_csv(src, index=False)
    hold = bridge.native_holdout_start(src, "SPY", rb.DECL["partition"]["holdout_months"])
    assert hold == pd.Timestamp("2026-03-01", tz="UTC")
    out = bridge.research_bars_to_backtest_csv(src, "SPY", tmp_path / "bt.csv", end_exclusive_utc=hold)
    assert pd.to_datetime(pd.read_csv(out)["end_ts"].astype("int64"), unit="s", utc=True).max() < hold


def _guard_world(tmp: Path, ledger_status="reserved"):
    run = tmp / "run"
    dates = pd.date_range("2025-01-02", "2026-08-31", freq="B", tz="UTC")
    pd.DataFrame([{"symbol": s, "end_ts": d.isoformat(), "close": 1.0} for s in SYMBOLS for d in dates]).to_csv(
        (run / "data").mkdir(parents=True) or run / "data" / "research_bars.csv", index=False)
    before = int(pd.Timestamp("2026-02-27T05:00:00", tz="UTC").timestamp())
    iso = [pd.Timestamp("2026-02-26", tz="UTC").isoformat(), pd.Timestamp("2026-02-27", tz="UTC").isoformat()]
    for name, pattern, column, kind in hg.CATEGORIES:
        path = run / pattern.replace("*", "x")
        path.parent.mkdir(parents=True, exist_ok=True)
        pd.DataFrame({column: [before - 86400, before] if kind == "epoch" else iso}).to_csv(path, index=False)
    registry = tmp / "research.sqlite3"
    con = sqlite3.connect(registry)
    con.execute("create table research_holdout_ledger (holdout_id text, status text, consumed_at text, consumer_identity_json text)")
    con.execute("insert into research_holdout_ledger values ('h', ?, ?, ?)",
                (ledger_status, None if ledger_status == "reserved" else "2026-10-09", None))
    con.commit()
    con.close()
    return run, registry


def test_the_holdout_guard_passes_a_clean_run_and_fails_on_a_consumed_ledger_or_a_holdout_row(tmp_path):
    decl = {"partition": rb.DECL["partition"], "universe": {"symbols": SYMBOLS}}
    run, registry = _guard_world(tmp_path / "a")
    assert hg.check(decl, run, registry, "post")["holdout_start_utc"] == pd.Timestamp("2026-03-01", tz="UTC").isoformat()
    run, registry = _guard_world(tmp_path / "b", ledger_status="consumed")
    with pytest.raises(hg.HoldoutBreach, match="not RESERVED"):
        hg.check(decl, run, registry, "post")
    for name, pattern, column, kind in hg.CATEGORIES:
        run, registry = _guard_world(tmp_path / f"c_{name}")
        path = run / pattern.replace("*", "x")
        late = int(pd.Timestamp("2026-03-02T05:00:00", tz="UTC").timestamp()) if kind == "epoch" else \
            pd.Timestamp("2026-03-02", tz="UTC").isoformat()
        pd.DataFrame({column: [late]}).to_csv(path, index=False)
        with pytest.raises(hg.HoldoutBreach, match=name):
            hg.check(decl, run, registry, "post")
