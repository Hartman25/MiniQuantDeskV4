"""Native strategy signal stream -> registered Research trial (M1 bridge).

Signal content here is synthetic (the bridge's logic is independent of it);
the cross-language Rust-emitter -> Python path is exercised separately by
`test_real_rust_emitter_roundtrip`, which skips when the CLI binary is absent.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

from mqk_research.data.bars_provenance import (
    CA_POLICY_FORBID_AFFECTED_PERIODS,
    PRICE_CONVENTION_RAW_UNADJUSTED,
    UNIVERSE_MODE_FIXED_EX_ANTE,
    build_bars_provenance_manifest,
    build_corporate_action_evidence,
)
from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.ml.economic_walkforward import (
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
)
from mqk_research.ml.execution_pricing import (
    EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    ExecutionPricingSpec,
)
from mqk_research.ml.native_signal_registry_integration import (
    NATIVE_SIGNAL_STREAM_PROTOCOL_ID,
    NativeSignalError,
    build_native_signal_trial_identity,
    plan_native_folds,
    research_bars_to_backtest_csv,
    run_registered_native_signal_economic_eval,
)
from mqk_research.ml.util_hash import sha256_file
from mqk_research.ml.weight_to_share import WeightToShareSpec

SYMBOL = "SPY"
STRATEGY = "trend_sma50"
FP_X = "a" * 64
FP_Y = "b" * 64
EVAL_START = pd.Timestamp("2018-04-01", tz="UTC")
EXPERIMENT = "NATIVE-TEST-EXP"


def _bars(symbols=(SYMBOL, "EFA")) -> pd.DataFrame:
    rng = np.random.default_rng(7)
    dates = pd.bdate_range("2018-01-01", "2021-12-31", tz="UTC")
    rows = []
    for k, sym in enumerate(symbols):
        px = 100.0 + 10 * k
        for d in dates:
            px *= 1.0 + rng.normal(0.0004, 0.01)
            rows.append({"symbol": sym, "end_ts": d.isoformat(), "open": px, "high": px * 1.002,
                         "low": px * 0.998, "close": px, "volume": 1_000_000})
    return pd.DataFrame(rows)


def _spec(max_position_notional_usd: float | None = 90_000.0) -> EconomicWalkForwardSpec:
    return EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        execution_pricing=ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1, slippage_bps=5, volatility_mult_bps=0),
        annualization=AnnualizationSpec(),
        weight_to_share=WeightToShareSpec(equity_usd=100_000.0, max_position_notional_usd=max_position_notional_usd),
    )


def _manifest(bars: pd.DataFrame, bars_path: Path) -> dict:
    end_ts = pd.to_datetime(bars["end_ts"], utc=True)
    symbols = sorted(bars["symbol"].unique())
    start = end_ts.min().isoformat()
    end = (end_ts.max() + pd.Timedelta(seconds=1)).isoformat()
    evidence = build_corporate_action_evidence(
        source_provider_id="test_fixture_no_known_corporate_actions", covered_symbol_universe=symbols,
        coverage_start_utc=start, coverage_end_utc=end, corporate_action_entries=())
    return build_bars_provenance_manifest(
        price_provenance={"close_column": "close", "provider_ids_observed": ["test_fixture"],
                          "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED,
                          "provider_metadata_available": True, "convention_basis": "synthetic native-bridge fixture"},
        corporate_action_policy=CA_POLICY_FORBID_AFFECTED_PERIODS,
        corporate_action_evidence_id=evidence["evidence_id"], corporate_action_evidence=evidence,
        forbidden_periods=(), timeframe="1D", start_utc=start, end_utc=end, symbol_universe=symbols,
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE, bars=bars, artifact_path=bars_path)


def _write_stream(tmp: Path, bars_csv: Path, *, strategy=STRATEGY, fingerprint=FP_X, qty_one=1_000_000):
    bt = research_bars_to_backtest_csv(
        bars_csv, SYMBOL, tmp / "bt_bars.csv", end_exclusive_utc=pd.Timestamp("2021-07-01", tz="UTC"))
    spy = pd.read_csv(bt)
    stream = pd.DataFrame({
        "symbol": SYMBOL, "decision_ts": spy["end_ts"],
        "target_qty_micros": [qty_one if (i // 40) % 2 == 0 else 0 for i in range(len(spy))],
    })
    csv_path = tmp / "native_signals.csv"
    stream.to_csv(csv_path, index=False, lineterminator="\n")
    meta = {"protocol_id": NATIVE_SIGNAL_STREAM_PROTOCOL_ID, "strategy_name": strategy,
            "semantic_fingerprint": fingerprint, "symbol": SYMBOL, "timeframe_secs": 86400,
            "bar_history_len": 50, "bars_csv_sha256": sha256_file(bt), "signal_rows": len(stream),
            "native_signals_csv_sha256": sha256_file(csv_path)}
    meta_path = tmp / "native_signals_meta.json"
    meta_path.write_text(json.dumps(meta), encoding="utf-8")
    return bt, csv_path, meta_path


class Env:
    def __init__(self, tmp: Path, **stream_kw):
        self.tmp = tmp
        self.bars = _bars()
        self.bars_csv = tmp / "bars.csv"
        self.bars.to_csv(self.bars_csv, index=False)
        self.manifest = _manifest(pd.read_csv(self.bars_csv), self.bars_csv)
        self.bt, self.signals, self.meta = _write_stream(tmp, self.bars_csv, **stream_kw)
        self.db = tmp / "registry.sqlite3"

    def run(self, *, strategy_id=STRATEGY, hypothesis="H1", run_name="r1", **over):
        kw = dict(
            experiment_id=EXPERIMENT, hypothesis_id=hypothesis, strategy_id=strategy_id, symbol=SYMBOL,
            bars_csv=self.bars_csv, bars_provenance=self.manifest, backtest_bars_csv=self.bt,
            signals_csv=self.signals, signals_meta_json=self.meta, economic_spec=_spec(),
            evaluation_start_utc=EVAL_START, test_months=12, holdout_months=6, registry_db=self.db)
        kw.update(over)
        return run_registered_native_signal_economic_eval(self.tmp / run_name, **kw)


def test_registers_trial_attempt_and_holdout_and_never_scores_holdout(tmp_path):
    env = Env(tmp_path)
    out = env.run()
    econ = json.loads(out.read_text(encoding="utf-8"))
    assert econ["protocol"]["protocol_id"] == "economic_walk_forward_v1"
    assert econ["holdout"] == {"status": "reserved_not_evaluated"}
    reg = econ["registry"]
    store = ResearchResultStore(env.db)
    trial = store.get_trial(reg["trial_id"])
    assert trial["strategy_id"] == STRATEGY
    attempts = store.list_attempts(reg["trial_id"])
    assert [a["status"] for a in attempts] == ["succeeded"]
    assert attempts[0]["result_id"] == econ["ids"]["economic_eval_id"]
    assert store.get_holdout(reg["holdout_id"])["status"] == "reserved"

    oos = pd.read_csv(tmp_path / "r1" / "eval" / "walk_forward_oos_predictions.csv")
    wf = json.loads((tmp_path / "r1" / "eval" / "walk_forward_eval.json").read_text())
    holdout_start = pd.Timestamp(wf["holdout"]["start_utc"])
    assert pd.to_datetime(oos["decision_ts"], utc=True).max() < holdout_start
    assert holdout_start == pd.Timestamp("2021-07-01", tz="UTC")


def test_a_spec_that_cannot_implement_the_targets_is_refused_by_the_fidelity_gate(tmp_path):
    env = Env(tmp_path)
    # Sizing the position at the allocation cap rejects entries at the
    # conservative fill price: the evaluation would not measure the strategy.
    with pytest.raises(NativeSignalError, match="execution fidelity"):
        env.run(economic_spec=_spec(max_position_notional_usd=None))
    store = ResearchResultStore(env.db)
    (trial,) = store.list_trials(experiment_id=EXPERIMENT)
    (attempt,) = store.list_attempts(trial["trial_id"])
    assert attempt["status"] == "failed" and "execution fidelity" in attempt["failure_reason"]
    # The amended, implementable spec is a NEW trial identity, not a retry.
    ok = json.loads(env.run(run_name="r2").read_text())
    assert ok["registry"]["trial_id"] != trial["trial_id"]
    assert ok["registry"]["execution_fidelity"] >= 0.95


def test_retry_is_a_new_attempt_of_the_same_trial(tmp_path):
    env = Env(tmp_path)
    t1 = json.loads(env.run(run_name="r1").read_text())["registry"]
    t2 = json.loads(env.run(run_name="r2").read_text())["registry"]
    assert t1["trial_id"] == t2["trial_id"]
    assert (t1["attempt_index"], t2["attempt_index"]) == (1, 2)
    assert len(ResearchResultStore(env.db).list_trials(experiment_id=EXPERIMENT)) == 1


def test_identity_binds_strategy_and_fingerprint_and_ignores_results(tmp_path):
    env = Env(tmp_path)
    kw = dict(experiment_id=EXPERIMENT, hypothesis_id="H1", strategy_id=STRATEGY, symbol=SYMBOL,
              semantic_fingerprint=FP_X, bars_provenance=env.manifest, evaluation_start_utc=EVAL_START,
              test_months=12, holdout_months=6, economic_spec=_spec())
    base, _ = build_native_signal_trial_identity(**kw)
    assert base == build_native_signal_trial_identity(**kw)[0]
    assert build_native_signal_trial_identity(**{**kw, "semantic_fingerprint": FP_Y})[0] != base
    assert build_native_signal_trial_identity(**{**kw, "strategy_id": "other_engine"})[0] != base
    assert build_native_signal_trial_identity(**{**kw, "symbol": "EFA"})[0] != base
    assert build_native_signal_trial_identity(**{**kw, "test_months": 6})[0] != base


@pytest.mark.parametrize("mutate", ["strategy", "csv_bytes", "bars_mismatch", "qty", "fingerprint"])
def test_refuses_before_registering_anything(tmp_path, mutate):
    env = Env(tmp_path, qty_one=2_000_000 if mutate == "qty" else 1_000_000,
              fingerprint="zz" if mutate == "fingerprint" else FP_X)
    kw = {}
    if mutate == "strategy":
        kw["strategy_id"] = "some_other_engine"
    if mutate == "csv_bytes":
        env.signals.write_text(env.signals.read_text() + "\n", encoding="utf-8")
    if mutate == "bars_mismatch":
        env.bt.write_text(env.bt.read_text().replace("\n", "\n", 1) + "", encoding="utf-8")
        bad = pd.read_csv(env.bt)
        bad.loc[0, "close_micros"] += 1
        bad.to_csv(env.bt, index=False, lineterminator="\n")
    with pytest.raises((NativeSignalError, RuntimeError)):
        env.run(**kw)
    if env.db.exists():
        assert ResearchResultStore(env.db).list_trials(experiment_id=EXPERIMENT) == []


def test_wrong_timeframe_or_wrong_expected_fingerprint_is_refused_before_registering(tmp_path):
    env = Env(tmp_path)
    with pytest.raises(NativeSignalError, match="timeframe_secs"):
        env.run(expected_timeframe_secs=3_600)
    with pytest.raises(NativeSignalError, match="semantic_fingerprint"):
        env.run(expected_semantic_fingerprint=FP_Y)
    if env.db.exists():
        assert ResearchResultStore(env.db).list_trials(experiment_id=EXPERIMENT) == []
    # The matching expectations register normally.
    out = json.loads(env.run(expected_timeframe_secs=86_400, expected_semantic_fingerprint=FP_X).read_text())
    assert out["registry"]["semantic_fingerprint"] == FP_X


def test_failure_after_attempt_start_is_preserved_as_failed(tmp_path):
    env = Env(tmp_path)
    # A start date that leaves a fold without any signals fails after the attempt begins.
    stream = pd.read_csv(env.signals)
    stream = stream[pd.to_datetime(stream["decision_ts"], unit="s", utc=True) < pd.Timestamp("2019-01-01", tz="UTC")]
    stream.to_csv(env.signals, index=False, lineterminator="\n")
    meta = json.loads(env.meta.read_text())
    meta["signal_rows"] = len(stream)
    meta["native_signals_csv_sha256"] = sha256_file(env.signals)
    env.meta.write_text(json.dumps(meta), encoding="utf-8")
    with pytest.raises(NativeSignalError):
        env.run()
    store = ResearchResultStore(env.db)
    (trial,) = store.list_trials(experiment_id=EXPERIMENT)
    attempts = store.list_attempts(trial["trial_id"])
    assert [a["status"] for a in attempts] == ["failed"]
    assert "no native signals inside fold" in attempts[0]["failure_reason"]


def test_no_holdout_bar_reaches_the_emitter_input(tmp_path):
    env = Env(tmp_path)
    bt = pd.read_csv(env.bt)
    holdout_start = int(pd.Timestamp("2021-07-01", tz="UTC").timestamp())
    assert bt["end_ts"].max() < holdout_start
    # An untruncated conversion is refused as the backtest input.
    full = research_bars_to_backtest_csv(env.bars_csv, SYMBOL, tmp_path / "full.csv")
    with pytest.raises(NativeSignalError, match="holdout-truncated"):
        env.run(backtest_bars_csv=full)


def test_plan_native_folds_never_reaches_the_holdout():
    folds, holdout_start, dataset_end = plan_native_folds(
        t_min=pd.Timestamp("2018-01-02", tz="UTC"), t_max=pd.Timestamp("2021-12-31", tz="UTC"),
        evaluation_start_utc=EVAL_START, test_months=12, holdout_months=6)
    assert holdout_start == pd.Timestamp("2021-07-01", tz="UTC")
    assert dataset_end == pd.Timestamp("2022-01-01", tz="UTC")
    assert all(f.test_end <= holdout_start for f in folds)
    assert [f.test_start for f in folds[1:]] == [f.test_end for f in folds[:-1]]


def _cli() -> Path | None:
    root = Path(__file__).resolve().parents[2] / "core-rs" / "target" / "debug"
    for name in ("mqk-cli.exe", "mqk-cli"):
        if (root / name).exists():
            return root / name
    return None


@pytest.mark.skipif(_cli() is None, reason="mqk-cli binary not built")
def test_real_rust_emitter_roundtrip(tmp_path):
    env = Env(tmp_path)
    out_dir = tmp_path / "emit"
    subprocess.run([str(_cli()), "backtest", "native-signals", "--bars-path", str(env.bt), "--strategy", STRATEGY,
                    "--symbol", SYMBOL, "--timeframe-secs", "86400", "--out-dir", str(out_dir)],
                   check=True, capture_output=True, text=True)
    meta = json.loads((out_dir / "native_signals_meta.json").read_text())
    assert meta["strategy_name"] == STRATEGY and len(meta["semantic_fingerprint"]) == 64
    out = env.run(signals_csv=out_dir / "native_signals.csv", signals_meta_json=out_dir / "native_signals_meta.json")
    econ = json.loads(out.read_text())
    assert econ["registry"]["semantic_fingerprint"] == meta["semantic_fingerprint"]
