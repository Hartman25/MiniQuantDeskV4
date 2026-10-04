"""Native strategy signal stream (v2, exact absolute targets) -> registered Research trial.

Signal content here is synthetic (the bridge's logic is independent of it); the
cross-language Rust-emitter -> Python path is exercised separately by
`test_real_rust_emitter_roundtrip`, which skips when the CLI binary is absent.
"""

from __future__ import annotations

import json
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
    SIGNAL_DIRECTION_POLICY_NATIVE_EXACT_TARGET_QTY_V1,
    SIGNAL_SIZING_EXACT_NATIVE_TARGET_QTY_V1,
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
    NATIVE_SIGNAL_SOURCE_KIND,
    NATIVE_SIGNAL_STREAM_PROTOCOL_ID,
    NativeSignalError,
    build_native_signal_trial_identity,
    native_exact_target_fidelity,
    plan_native_folds,
    register_native_signal_trial,
    require_native_exact_target_spec,
    research_bars_to_backtest_csv,
    run_registered_native_signal_economic_eval,
)
from mqk_research.ml.util_hash import sha256_file
from mqk_research.ml.weight_to_share import WeightToShareSpec

SYMBOL = "SPY"
STRATEGY = "trend_sma50"
FP_X = "a" * 64
FP_Y = "b" * 64
REQUIRED = 50
EVAL_START = pd.Timestamp("2018-04-01", tz="UTC")
EXPERIMENT = "NATIVE-TEST-EXP"
EQUITY = 100_000.0


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


def _spec(*, max_position_notional_usd: float | None = None, exact: bool = True) -> EconomicWalkForwardSpec:
    policy = (
        SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_NATIVE_EXACT_TARGET_QTY_V1,
            sizing=SIGNAL_SIZING_EXACT_NATIVE_TARGET_QTY_V1,
        )
        if exact
        else SignalPolicySpec()
    )
    return EconomicWalkForwardSpec(
        signal_policy=policy,
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        execution_pricing=ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1, slippage_bps=5, volatility_mult_bps=0),
        annualization=AnnualizationSpec(),
        weight_to_share=WeightToShareSpec(equity_usd=EQUITY, max_position_notional_usd=max_position_notional_usd),
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


class Env:
    """Synthetic emitter: `emit()` writes a v2 stream + meta. Every defect a test
    wants is applied by overriding `meta_overrides` / `qty_one` / `fingerprint`."""

    def __init__(self, tmp: Path, *, qty_one: int = 1_000_000, fingerprint: str = FP_X, meta_overrides=None):
        self.tmp = tmp
        self.bars_csv = tmp / "bars.csv"
        _bars().to_csv(self.bars_csv, index=False)
        self.manifest = _manifest(pd.read_csv(self.bars_csv), self.bars_csv)
        self.bt = research_bars_to_backtest_csv(
            self.bars_csv, SYMBOL, tmp / "bt_bars.csv", end_exclusive_utc=pd.Timestamp("2021-07-01", tz="UTC"))
        self.signals = tmp / "native_signals.csv"
        self.meta = tmp / "native_signals_meta.json"
        self.db = tmp / "registry.sqlite3"
        self.qty_one = qty_one
        self.fingerprint = fingerprint
        self.meta_overrides = dict(meta_overrides or {})
        self.emit_calls = 0
        self.on_emit = None

    def emit(self) -> None:
        self.emit_calls += 1
        if self.on_emit is not None:
            self.on_emit()
        spy = pd.read_csv(self.bt)
        stream = pd.DataFrame({
            "symbol": SYMBOL, "decision_ts": spy["end_ts"],
            "target_qty_micros": [self.qty_one if (i // 40) % 2 == 0 else 0 for i in range(len(spy))],
        })
        stream.to_csv(self.signals, index=False, lineterminator="\n")
        meta = {"protocol_id": NATIVE_SIGNAL_STREAM_PROTOCOL_ID, "quantity_semantics": "absolute_target_qty_micros_v1",
                "strategy_name": STRATEGY, "semantic_fingerprint": self.fingerprint, "symbol": SYMBOL,
                "timeframe_secs": 86400, "configured_bar_history_len": 50, "required_history_bars": REQUIRED,
                "effective_bar_history_len": max(50, REQUIRED), "observed_max_window_len": max(50, REQUIRED),
                "initial_cash_micros": int(EQUITY * 1_000_000), "bars_csv_sha256": sha256_file(self.bt),
                "signal_rows": len(stream), "native_signals_csv_sha256": sha256_file(self.signals)}
        meta.update(self.meta_overrides)
        self.meta.write_text(json.dumps(meta), encoding="utf-8")

    def reg_kwargs(self, hypothesis="H1", strategy_id=STRATEGY, **over):
        kw = dict(experiment_id=EXPERIMENT, hypothesis_id=hypothesis, strategy_id=strategy_id, symbol=SYMBOL,
                  semantic_fingerprint=FP_X, required_history_bars=REQUIRED, bars_provenance=self.manifest,
                  economic_spec=_spec(), evaluation_start_utc=EVAL_START, test_months=12, holdout_months=6,
                  registry_db=self.db)
        kw.update(over)
        return kw

    def register(self, **over) -> str:
        return register_native_signal_trial(**self.reg_kwargs(**over))

    def run(self, *, hypothesis="H1", strategy_id=STRATEGY, run_name="r1", register=True, **over):
        if register:
            self.register(hypothesis=hypothesis, strategy_id=strategy_id)
        kw = dict(
            experiment_id=EXPERIMENT, hypothesis_id=hypothesis, strategy_id=strategy_id, symbol=SYMBOL,
            bars_csv=self.bars_csv, bars_provenance=self.manifest, backtest_bars_csv=self.bt,
            emit_signals=self.emit, signals_csv=self.signals, signals_meta_json=self.meta,
            economic_spec=_spec(), evaluation_start_utc=EVAL_START, expected_semantic_fingerprint=FP_X,
            required_history_bars=REQUIRED, test_months=12, holdout_months=6, registry_db=self.db)
        kw.update(over)
        return run_registered_native_signal_economic_eval(self.tmp / run_name, **kw)

    def store(self) -> ResearchResultStore:
        return ResearchResultStore(self.db)


def test_registers_trial_attempt_and_holdout_and_never_scores_holdout(tmp_path):
    env = Env(tmp_path)
    out = env.run()
    econ = json.loads(out.read_text(encoding="utf-8"))
    assert econ["protocol"]["protocol_id"] == "economic_walk_forward_v1"
    assert econ["holdout"] == {"status": "reserved_not_evaluated"}
    assert econ["signal_policy"]["direction_policy"] == SIGNAL_DIRECTION_POLICY_NATIVE_EXACT_TARGET_QTY_V1
    reg = econ["registry"]
    store = env.store()
    trial = store.get_trial(reg["trial_id"])
    assert trial["strategy_id"] == STRATEGY
    attempts = store.list_attempts(reg["trial_id"])
    assert [a["status"] for a in attempts] == ["succeeded"]
    assert attempts[0]["result_id"] == econ["ids"]["economic_eval_id"]
    assert store.get_holdout(reg["holdout_id"])["status"] == "reserved"
    assert reg["effective_bar_history_len"] == 50 and reg["max_abs_target_qty_gap"] == 0

    oos = pd.read_csv(tmp_path / "r1" / "eval" / "walk_forward_oos_predictions.csv")
    wf = json.loads((tmp_path / "r1" / "eval" / "walk_forward_eval.json").read_text())
    holdout_start = pd.Timestamp(wf["holdout"]["start_utc"])
    assert pd.to_datetime(oos["decision_ts"], utc=True).max() < holdout_start
    assert holdout_start == pd.Timestamp("2021-07-01", tz="UTC")


@pytest.mark.parametrize("qty", [1, 3])
def test_the_exact_native_quantity_is_what_the_simulator_holds(tmp_path, qty):
    env = Env(tmp_path, qty_one=qty * 1_000_000)
    econ = json.loads(env.run().read_text(encoding="utf-8"))
    held = set()
    for fold in econ["folds"]:
        held |= {e["target_qty"] for e in fold["weight_to_share_evidence"][SYMBOL]}
    assert held == {0, qty}, "never a re-sized position (the superseded bridge held hundreds of shares)"
    assert econ["registry"]["execution_fidelity"] == 1.0


def test_a_target_the_allocation_cannot_fund_fails_the_attempt_instead_of_being_resized(tmp_path):
    env = Env(tmp_path, qty_one=5_000_000_000)  # 5,000 shares ~ $500k against $100k equity
    with pytest.raises(RuntimeError, match="allocation capacity"):
        env.run()
    (trial,) = env.store().list_trials(experiment_id=EXPERIMENT)
    assert [a["status"] for a in env.store().list_attempts(trial["trial_id"])] == ["failed"]


def _fidelity_fixture(held_qty: int, desired_qty: int):
    ts = pd.Timestamp("2020-01-01", tz="UTC")
    bars = [ts + pd.Timedelta(days=i) for i in range(6)]
    signals = pd.DataFrame({
        "decision_ts_utc": [bars[0]], "target_qty": [desired_qty],
    })
    events = [{"timestamp": b.isoformat(), "target_qty": (held_qty if i >= 1 else 0),
               "signal_target_qty": None, "side": None, "qty": 0} for i, b in enumerate(bars)]
    out = {"folds": [{"fold": 1, "test_start_utc": bars[0].isoformat(),
                      "test_end_utc": (bars[-1] + pd.Timedelta(days=1)).isoformat(),
                      "weight_to_share_evidence": {SYMBOL: events}}]}
    return out, signals


def _fidelity(held_qty: int, desired_qty: int):
    out, signals = _fidelity_fixture(held_qty, desired_qty)
    return native_exact_target_fidelity(out, SYMBOL, signals)


def test_fidelity_distinguishes_one_share_from_two_hundred_fifty():
    ok = _fidelity(1, 1)
    assert (ok.agreement, ok.max_abs_qty_gap, ok.evaluated_bars) == (1.0, 0, 5)
    bad = _fidelity(250, 1)
    assert bad.agreement == 1 / 5 and bad.max_abs_qty_gap == 249  # only the flat bar before the decision agrees
    off_by_one = _fidelity(2, 1)
    assert off_by_one.agreement == 1 / 5 and off_by_one.max_abs_qty_gap == 1


def test_the_old_binary_weight_spec_is_refused_and_nothing_is_registered(tmp_path):
    env = Env(tmp_path)
    with pytest.raises(NativeSignalError, match="re-size the target from a weight"):
        require_native_exact_target_spec(_spec(exact=False))
    with pytest.raises(NativeSignalError, match="re-size the target from a weight"):
        register_native_signal_trial(**env.reg_kwargs(economic_spec=_spec(exact=False)))
    with pytest.raises(NativeSignalError, match="re-size the target from a weight"):
        env.run(register=False, economic_spec=_spec(exact=False))
    assert env.emit_calls == 0
    if env.db.exists():
        assert env.store().list_trials(experiment_id=EXPERIMENT) == []


def test_old_bridge_v1_stream_is_not_accepted_as_evidence(tmp_path):
    env = Env(tmp_path, meta_overrides={"protocol_id": "native_strategy_signal_stream_v1"})
    with pytest.raises(NativeSignalError, match="superseded"):
        env.run()
    (trial,) = env.store().list_trials(experiment_id=EXPERIMENT)
    (attempt,) = env.store().list_attempts(trial["trial_id"])
    assert attempt["status"] == "failed" and "superseded" in attempt["failure_reason"]


def test_retry_is_a_new_attempt_of_the_same_trial(tmp_path):
    env = Env(tmp_path)
    t1 = json.loads(env.run(run_name="r1").read_text())["registry"]
    t2 = json.loads(env.run(run_name="r2", register=False).read_text())["registry"]
    assert t1["trial_id"] == t2["trial_id"]
    assert (t1["attempt_index"], t2["attempt_index"]) == (1, 2)
    assert len(env.store().list_trials(experiment_id=EXPERIMENT)) == 1


def test_identity_binds_strategy_fingerprint_history_and_quantity_contract(tmp_path):
    env = Env(tmp_path)
    kw = {k: v for k, v in env.reg_kwargs().items() if k not in ("hypothesis_text", "registry_db")}
    base, identity = build_native_signal_trial_identity(**kw)
    assert base == build_native_signal_trial_identity(**kw)[0]
    assert identity["signal_source"]["kind"] == NATIVE_SIGNAL_SOURCE_KIND == "native_strategy_signal_stream_v2"
    assert identity["signal_source"]["target_semantics"] == "absolute_whole_share_target_v1"
    assert identity["economic_protocol"]["signal_policy"]["direction_policy"] == SIGNAL_DIRECTION_POLICY_NATIVE_EXACT_TARGET_QTY_V1
    assert build_native_signal_trial_identity(**{**kw, "semantic_fingerprint": FP_Y})[0] != base
    assert build_native_signal_trial_identity(**{**kw, "strategy_id": "other_engine"})[0] != base
    assert build_native_signal_trial_identity(**{**kw, "symbol": "EFA"})[0] != base
    assert build_native_signal_trial_identity(**{**kw, "test_months": 6})[0] != base
    assert build_native_signal_trial_identity(**{**kw, "required_history_bars": REQUIRED + 1})[0] != base
    # The capital basis is part of the economic identity too.
    other = _spec()
    other = EconomicWalkForwardSpec(
        signal_policy=other.signal_policy, cost_model=other.cost_model, execution_pricing=other.execution_pricing,
        annualization=other.annualization, weight_to_share=WeightToShareSpec(equity_usd=50_000.0))
    assert build_native_signal_trial_identity(**{**kw, "economic_spec": other})[0] != base


# Defects detected after the attempt began: the trial stays registered, the attempt is failed.
@pytest.mark.parametrize("name,kw,msg", [
    ("fractional_qty", dict(qty_one=500_000), "whole-share"),
    ("fingerprint", dict(fingerprint="zz"), "64 lowercase hex"),
    ("other_fingerprint", dict(fingerprint=FP_Y), "expected native fingerprint"),
    ("semantics", dict(meta_overrides={"quantity_semantics": "direction_only"}), "quantity_semantics"),
    ("meta_requires_more_than_registered", dict(meta_overrides={"required_history_bars": 253}), "required_history_bars"),
    ("effective_below_required", dict(meta_overrides={"effective_bar_history_len": 40}), "effective_bar_history_len"),
    # Self-consistent lie (observed == effective) that only the max(configured, required) rule exposes.
    ("effective_not_the_engine_rule", dict(meta_overrides={"effective_bar_history_len": 60, "observed_max_window_len": 60}),
     "effective_bar_history_len 60 != max"),
    ("observed_window_lies", dict(meta_overrides={"observed_max_window_len": 49}), "observed_max_window_len"),
    ("history_meta_missing", dict(meta_overrides={"effective_bar_history_len": None}), "history/cash provenance"),
    ("capital_basis", dict(meta_overrides={"initial_cash_micros": 1_000_000_000}), "capital basis"),
    ("wrong_timeframe", dict(meta_overrides={"timeframe_secs": 3600}), "timeframe_secs"),
])
def test_stream_defects_fail_the_attempt_of_a_registered_trial(tmp_path, name, kw, msg):
    env = Env(tmp_path, **kw)
    with pytest.raises((NativeSignalError, RuntimeError), match=msg):
        env.run()
    (trial,) = env.store().list_trials(experiment_id=EXPERIMENT)
    attempts = env.store().list_attempts(trial["trial_id"])
    assert [a["status"] for a in attempts] == ["failed"], name


def test_csv_tampering_and_strategy_mismatch_fail_the_attempt(tmp_path):
    env = Env(tmp_path)
    env.on_emit = None
    original_emit = env.emit

    def tampered():
        original_emit()
        env.signals.write_text(env.signals.read_text() + "\n", encoding="utf-8")

    env.emit = tampered
    with pytest.raises(NativeSignalError, match="sha256"):
        env.run()
    env2 = Env(tmp_path / "b" if (tmp_path / "b").mkdir() is None else tmp_path / "b")
    with pytest.raises(NativeSignalError, match="strategy"):
        env2.run(strategy_id="some_other_engine")


def test_backtest_bars_mismatch_is_refused_before_any_attempt(tmp_path):
    env = Env(tmp_path)
    env.register()
    bad = pd.read_csv(env.bt)
    bad.loc[0, "close_micros"] += 1
    bad.to_csv(env.bt, index=False, lineterminator="\n")
    with pytest.raises(NativeSignalError, match="holdout-truncated"):
        env.run(register=False)
    (trial,) = env.store().list_trials(experiment_id=EXPERIMENT)
    assert env.store().list_attempts(trial["trial_id"]) == []
    assert env.emit_calls == 0


# --- chronology: trial -> attempt -> emission ---------------------------------

def test_an_unregistered_trial_is_refused_before_the_emitter_runs(tmp_path):
    env = Env(tmp_path)
    with pytest.raises(NativeSignalError, match="not registered"):
        env.run(register=False)
    assert env.emit_calls == 0, "no market data may be evaluated before the trial exists"
    if env.db.exists():
        assert env.store().list_trials(experiment_id=EXPERIMENT) == []


def test_the_emitter_runs_inside_the_attempt_of_an_already_registered_trial(tmp_path):
    env = Env(tmp_path)
    seen = {}

    def probe():
        trials = env.store().list_trials(experiment_id=EXPERIMENT)
        seen["trials"] = len(trials)
        seen["attempts"] = [a["status"] for a in env.store().list_attempts(trials[0]["trial_id"])]

    env.on_emit = probe
    env.run()
    assert seen == {"trials": 1, "attempts": ["started"]}


def test_an_emitter_failure_is_a_failed_attempt_never_a_new_trial(tmp_path):
    env = Env(tmp_path)

    def boom():
        raise RuntimeError("emitter crashed")

    env.on_emit = boom
    with pytest.raises(RuntimeError, match="emitter crashed"):
        env.run()
    store = env.store()
    (trial,) = store.list_trials(experiment_id=EXPERIMENT)
    (attempt,) = store.list_attempts(trial["trial_id"])
    assert attempt["status"] == "failed" and "emitter crashed" in attempt["failure_reason"]
    # Infrastructure retry: the SAME trial, the next attempt.
    env.on_emit = None
    out = json.loads(env.run(register=False, run_name="r2").read_text())["registry"]
    assert out["trial_id"] == trial["trial_id"] and out["attempt_index"] == 2
    assert len(store.list_trials(experiment_id=EXPERIMENT)) == 1


def test_failure_after_attempt_start_is_preserved_as_failed(tmp_path):
    env = Env(tmp_path)
    original_emit = env.emit

    def short_stream():
        original_emit()
        stream = pd.read_csv(env.signals)
        stream = stream[pd.to_datetime(stream["decision_ts"], unit="s", utc=True) < pd.Timestamp("2019-01-01", tz="UTC")]
        stream.to_csv(env.signals, index=False, lineterminator="\n")
        meta = json.loads(env.meta.read_text())
        meta["signal_rows"] = len(stream)
        meta["native_signals_csv_sha256"] = sha256_file(env.signals)
        env.meta.write_text(json.dumps(meta), encoding="utf-8")

    env.emit = short_stream
    with pytest.raises(NativeSignalError):
        env.run()
    store = env.store()
    (trial,) = store.list_trials(experiment_id=EXPERIMENT)
    attempts = store.list_attempts(trial["trial_id"])
    assert [a["status"] for a in attempts] == ["failed"]
    assert "no native signals inside fold" in attempts[0]["failure_reason"]


def test_no_holdout_bar_reaches_the_emitter_input(tmp_path):
    env = Env(tmp_path)
    bt = pd.read_csv(env.bt)
    holdout_start = int(pd.Timestamp("2021-07-01", tz="UTC").timestamp())
    assert bt["end_ts"].max() < holdout_start
    full = research_bars_to_backtest_csv(env.bars_csv, SYMBOL, tmp_path / "full.csv")
    with pytest.raises(NativeSignalError, match="holdout-truncated"):
        env.run(backtest_bars_csv=full)
    assert env.emit_calls == 0


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


def _cli_has_v2() -> bool:
    cli = _cli()
    return cli is not None and b"native_strategy_signal_stream_v2" in cli.read_bytes()


@pytest.mark.skipif(not _cli_has_v2(), reason="mqk-cli binary with the v2 emitter not built")
def test_real_rust_emitter_roundtrip_and_no_data_fingerprint(tmp_path):
    env = Env(tmp_path)
    # Fingerprint/history identity is resolved with NO market data, before any trial exists.
    resolved = subprocess.run([str(_cli()), "backtest", "native-fingerprint", "--strategy", STRATEGY,
                               "--symbol", SYMBOL], check=True, capture_output=True, text=True).stdout
    info = dict(line.split("=", 1) for line in resolved.splitlines() if "=" in line)
    fp, required = info["semantic_fingerprint"], int(info["required_history_bars"])
    assert len(fp) == 64 and int(info["timeframe_secs"]) == 86400
    assert int(info["initial_cash_micros"]) == int(EQUITY * 1_000_000)

    out_dir = tmp_path / "emit"

    def emit():
        subprocess.run([str(_cli()), "backtest", "native-signals", "--bars-path", str(env.bt), "--strategy", STRATEGY,
                        "--symbol", SYMBOL, "--timeframe-secs", "86400", "--out-dir", str(out_dir)],
                       check=True, capture_output=True, text=True)

    env.register(semantic_fingerprint=fp, required_history_bars=required)
    out = env.run(register=False, emit_signals=emit, signals_csv=out_dir / "native_signals.csv",
                  signals_meta_json=out_dir / "native_signals_meta.json",
                  expected_semantic_fingerprint=fp, required_history_bars=required)
    meta = json.loads((out_dir / "native_signals_meta.json").read_text())
    assert meta["protocol_id"] == NATIVE_SIGNAL_STREAM_PROTOCOL_ID
    assert meta["effective_bar_history_len"] == max(meta["configured_bar_history_len"], required)
    assert meta["observed_max_window_len"] == meta["effective_bar_history_len"]
    assert json.loads(out.read_text())["registry"]["semantic_fingerprint"] == fp


def _identity(env: Env, manifest: dict, **over):
    kw = dict(experiment_id=EXPERIMENT, hypothesis_id="H1", strategy_id=STRATEGY, symbol=SYMBOL,
              semantic_fingerprint=FP_X, required_history_bars=REQUIRED, bars_provenance=manifest,
              evaluation_start_utc=EVAL_START, test_months=12, holdout_months=6, economic_spec=_spec())
    kw.update(over)
    return build_native_signal_trial_identity(**kw)


def test_canonical_timeframe_identity_collapses_daily_aliases_and_leaves_v1_unchanged(tmp_path):
    env = Env(tmp_path)
    m_1d = dict(env.manifest, timeframe="1D")
    m_1day = dict(env.manifest, timeframe="1Day")
    # Historical default (v1): the raw label is identity-bearing; ids are unchanged by this seam.
    v1_d, v1_day = _identity(env, m_1d)[0], _identity(env, m_1day)[0]
    assert v1_d != v1_day
    assert _identity(env, m_1d, canonical_timeframe_identity=False)[0] == v1_d
    # Future (v2): one identity for the one economic timeframe, disjoint from v1.
    v2_d, id_d = _identity(env, m_1d, canonical_timeframe_identity=True)
    v2_day, id_day = _identity(env, m_1day, canonical_timeframe_identity=True)
    assert v2_d == v2_day and id_d == id_day and v2_d not in (v1_d, v1_day)
    assert id_d["data_identity"]["bars_provenance"]["timeframe_identity"] == "canonical_semantic_v1"
    for label in ("1H", "60Min", "5Min", "1Min", "garbage"):
        with pytest.raises(Exception, match="canonical semantic timeframe"):
            _identity(env, dict(env.manifest, timeframe=label), canonical_timeframe_identity=True)


def test_canonical_timeframe_registration_and_eval_agree_and_a_flag_mismatch_is_unregistered(tmp_path):
    env = Env(tmp_path)
    env.register(canonical_timeframe_identity=True)
    out = env.run(register=False, canonical_timeframe_identity=True)
    econ = json.loads(out.read_text(encoding="utf-8"))
    assert econ["registry"]["trial_id"] == _identity(env, env.manifest, canonical_timeframe_identity=True)[0]
    # The legacy (raw-label) identity of the same trial was never registered: the eval refuses it.
    with pytest.raises(NativeSignalError, match="not registered"):
        env.run(register=False, run_name="r2")
