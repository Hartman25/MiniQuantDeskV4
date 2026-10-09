"""Guards the batch runner's chronology: hypothesis -> ALL trials registered ->
attempt -> emitter/evaluation. A real native emitter or Backtest evaluation
must never run before every predeclared trial exists."""

from __future__ import annotations

import ast
import copy
import importlib.util
import inspect
import json
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
sys.path.insert(0, str(HERE))
import stage_auth_testkit  # noqa: E402

FP = "c" * 64
SPEC = importlib.util.spec_from_file_location("run_batch_under_test", HERE / "run_batch.py")
rb = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(rb)
ORIGINAL_DECL = copy.deepcopy(rb.DECL)


def _manifest(bars: pd.DataFrame, path: Path) -> dict:
    ts = pd.to_datetime(bars["end_ts"], utc=True)
    symbols = sorted(bars["symbol"].unique())
    start, end = ts.min().isoformat(), (ts.max() + pd.Timedelta(seconds=1)).isoformat()
    ev = build_corporate_action_evidence(
        source_provider_id="fixture", covered_symbol_universe=symbols, coverage_start_utc=start,
        coverage_end_utc=end, corporate_action_entries=())
    return build_bars_provenance_manifest(
        price_provenance={"close_column": "close", "provider_ids_observed": ["fixture"],
                          "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED,
                          "provider_metadata_available": True, "convention_basis": "chronology fixture"},
        corporate_action_policy=CA_POLICY_FORBID_AFFECTED_PERIODS, corporate_action_evidence_id=ev["evidence_id"],
        corporate_action_evidence=ev, forbidden_periods=(), timeframe="1D", start_utc=start, end_utc=end,
        symbol_universe=symbols, universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE, bars=bars, artifact_path=path)


@pytest.fixture
def runner(tmp_path, monkeypatch):
    """The runner pointed at a temp tree, two trials, exact-target economics, and a
    fake CLI that records every invocation and refuses to emit."""
    decl = copy.deepcopy(ORIGINAL_DECL)
    decl["economic_protocol"]["signal_policy"] = {
        "direction_policy": "native_exact_target_qty_v1", "sizing": "exact_native_target_qty_v1",
        "entry_threshold": 0.5, "long_only": True, "max_gross_exposure": 1.0}
    trials = [("absolute_momentum_252", "SPY"), ("absolute_momentum_252", "EFA")]
    rng = np.random.default_rng(3)
    dates = pd.bdate_range("2018-01-01", "2021-12-31", tz="UTC")
    rows = []
    for k, sym in enumerate(("SPY", "EFA")):
        px = 100.0 + 10 * k
        for d in dates:
            px *= 1.0 + rng.normal(0.0004, 0.01)
            rows.append({"symbol": sym, "end_ts": d.isoformat(), "open": px, "high": px * 1.002, "low": px * 0.998,
                         "close": px, "volume": 1_000_000})
    data = tmp_path / "run" / "data"
    data.mkdir(parents=True)
    bars_df = pd.DataFrame(rows)
    bars_df.to_csv(data / "research_bars.csv", index=False)
    (data / "research_bars_provenance.json").write_text(
        json.dumps(_manifest(pd.read_csv(data / "research_bars.csv"), data / "research_bars.csv")), encoding="utf-8")
    decl["partition"]["evaluation_start_utc"] = "2018-04-01T00:00:00Z"
    decl["native_backtest"]["initial_cash_micros"] = 100_000_000_000  # = equity_usd 100,000

    calls: list[tuple] = []

    def fake_cli(*argv):
        calls.append(argv)
        if argv[:2] == ("backtest", "native-fingerprint"):
            return f"strategy={argv[3]}\nsymbol={argv[5]}\ntimeframe_secs=86400\nsemantic_fingerprint={FP}\nrequired_history_bars=253\n"
        raise AssertionError(f"no emitter/Backtest/other CLI call may run here: {argv}")

    for name, value in {
        "DECL": decl, "TRIALS": trials, "RUN": tmp_path / "run", "BARS": data / "research_bars.csv",
        "MANIFEST": data / "research_bars_provenance.json", "REGISTRY": tmp_path / "reg" / "research.sqlite3",
        "INDEX": tmp_path / "run" / "trials_index.json", "_run_cli": fake_cli,
    }.items():
        monkeypatch.setattr(rb, name, value)
    rb.DECL["universe"]["max_trials"] = len(trials)
    stage_auth_testkit.grant_runner_stages(monkeypatch, rb)  # the mutated declaration is not a frozen historical one
    return rb, calls


def test_register_resolves_identity_without_any_emitter_or_data_evaluation(runner):
    r, calls = runner
    r.stage_register(None)
    store = ResearchResultStore(r.REGISTRY)
    registered = store.list_trials(experiment_id=r.EXPERIMENT)
    assert len(registered) == 2
    assert all(store.list_attempts(t["trial_id"]) == [] for t in registered)
    assert [c[:2] for c in calls] == [("backtest", "native-fingerprint")] * 2
    assert not any("native-signals" in c for c in calls)


def test_register_runs_once_and_never_into_a_populated_registry(runner):
    r, _ = runner
    r.stage_register(None)
    with pytest.raises(SystemExit, match="already holds trials"):
        r.stage_register(None)


def test_trials_refuse_to_start_until_every_trial_is_registered(runner):
    r, calls = runner
    r.stage_register(None)
    # Simulate an incomplete registration: drop the runner's view to three trials.
    r.TRIALS.append(("absolute_momentum_252", "GLD"))
    with pytest.raises(SystemExit, match="every predeclared trial must be registered"):
        r.stage_trials(None)
    assert not any("native-signals" in c for c in calls), "no emitter may run before all trials exist"


def test_the_emitter_is_handed_to_the_attempt_not_run_by_the_runner(runner, monkeypatch):
    r, calls = runner
    r.stage_register(None)
    seen = []

    def recording_bridge(run_dir, **kw):
        store = ResearchResultStore(r.REGISTRY)
        registered = store.list_trials(experiment_id=r.EXPERIMENT)
        seen.append({
            "strategy": kw["strategy_id"], "symbol": kw["symbol"], "callable": callable(kw["emit_signals"]),
            "trials_at_call": len(registered),
            "attempts_at_call": sum(len(store.list_attempts(t["trial_id"])) for t in registered),
            "emitter_calls_so_far": sum(1 for c in calls if "native-signals" in c),
        })
        raise bridge.NativeSignalError("stop after recording")

    monkeypatch.setattr(bridge, "run_registered_native_signal_economic_eval", recording_bridge)
    r.stage_trials(None)
    assert [(s["strategy"], s["symbol"]) for s in seen] == r.TRIALS, "frozen order"
    assert all(s["callable"] and s["trials_at_call"] == 2 and s["attempts_at_call"] == 0 for s in seen)
    assert all(s["emitter_calls_so_far"] == 0 for s in seen), "the runner itself never emits"


def test_the_superseded_batch01_predeclaration_is_refused(monkeypatch):
    monkeypatch.setattr(rb, "DECL", copy.deepcopy(ORIGINAL_DECL))
    assert rb.DECL["economic_protocol"]["signal_policy"]["direction_policy"] == "long_only_v1"
    for stage in (rb.stage_check, rb.stage_register, rb.stage_trials):
        with pytest.raises(SystemExit, match="superseded binary-weight"):
            stage(None)


def test_stage_register_has_no_emitter_invocation_in_its_source():
    tree = ast.parse(inspect.getsource(rb.stage_register))
    literals = {n.value for n in ast.walk(tree) if isinstance(n, ast.Constant) and isinstance(n.value, str)}
    assert "native-signals" not in literals and "native-fingerprint" in literals
    emit_stage = ast.parse(inspect.getsource(rb.stage_trials))
    funcs = [n for n in ast.walk(emit_stage) if isinstance(n, ast.FunctionDef) and n.name == "emit"]
    assert len(funcs) == 1, "the emitter is only ever wrapped in a closure passed into the attempt"


def test_a_capital_basis_mismatch_between_research_and_backtest_is_refused(runner):
    r, calls = runner
    r.DECL["native_backtest"]["initial_cash_micros"] = 1_000_000_000  # USD 1,000 vs equity 100,000
    for stage in (r.stage_check, r.stage_register, r.stage_trials):
        with pytest.raises(SystemExit, match="one capital basis"):
            stage(None)
    assert calls == []


def test_the_closed_campaign_runner_cannot_drive_the_superseded_bridge():
    spec = importlib.util.spec_from_file_location("run_campaign_under_test", HERE / "run_campaign.py")
    legacy = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(legacy)
    with pytest.raises(SystemExit, match="superseded native bridge v1"):
        legacy.main()


def test_the_closed_campaign_runner_refuses_any_declaration_that_is_not_pinned_history(monkeypatch):
    monkeypatch.setenv("M1_CAMPAIGN_FILE", "PREDECLARED_KISS_EXT032_ETF_01.json")
    spec = importlib.util.spec_from_file_location("run_campaign_kiss", HERE / "run_campaign.py")
    with pytest.raises(SystemExit, match="not a frozen historical declaration"):
        spec.loader.exec_module(importlib.util.module_from_spec(spec))
