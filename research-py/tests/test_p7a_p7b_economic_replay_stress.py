"""P7A-P7B-ECONOMIC-REPLAY-STRESS-01 — replay-authority and stress-evaluation
negative controls for `p7a_p7b_economic_replay_stress_cli._run_replay_stress`.

Mirrors `test_economic_walkforward.py`'s registry-integration fixtures
(`_registered_economic_run` / `_synthetic_bars_provenance`) but registers the
trial under the OFFICIAL P7A (`rust_conservative_bar_range_v1`) execution
pricing model and OFFICIAL P7B (`weight_to_share_v1`) weight-to-share
protocol, since only such a trial ever qualifies as real P7A/P7B stress
evidence. All controls call `_run_replay_stress` directly (in-process, no
subprocess) against a real temporary SQLite registry and real
`run_economic_walkforward` output -- no shortcut/fabricated fixtures.
"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict

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
from mqk_research.ml.economic_registry_integration import run_registered_economic_walkforward_eval
from mqk_research.ml.economic_walkforward import (
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec
from mqk_research.ml.execution_pricing import (
    EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    ExecutionPricingSpec,
)
from mqk_research.ml.p7a_p7b_economic_replay_stress_cli import ReplayAuthorityError, _run_replay_stress
from mqk_research.ml.schema import generate_feature_schema
from mqk_research.ml.util_hash import sha256_file
from mqk_research.ml.weight_to_share import WeightToShareSpec

BASE_SPEC_KW = dict(train_years=1, test_months=1, step_months=1, holdout_months=1, min_rows_per_fold=200)

OFFICIAL_SPEC = EconomicWalkForwardSpec(
    signal_policy=SignalPolicySpec(entry_threshold=0.5),
    cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
    execution_pricing=ExecutionPricingSpec(
        pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1, slippage_bps=5, volatility_mult_bps=0
    ),
    weight_to_share=WeightToShareSpec(equity_usd=100_000.0),
    annualization=AnnualizationSpec(),
)

NON_OFFICIAL_PRICING_SPEC = EconomicWalkForwardSpec(
    signal_policy=SignalPolicySpec(entry_threshold=0.5),
    cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=5.0),
    weight_to_share=WeightToShareSpec(equity_usd=100_000.0),
    annualization=AnnualizationSpec(),
)  # execution_pricing left at its close_only_diagnostic_v1 default -- fails control #7

OFFICIAL_PRICING_NO_WTS_SPEC = EconomicWalkForwardSpec(
    signal_policy=SignalPolicySpec(entry_threshold=0.5),
    cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
    execution_pricing=ExecutionPricingSpec(
        pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1, slippage_bps=5, volatility_mult_bps=0
    ),
    weight_to_share=None,
    annualization=AnnualizationSpec(),
)  # official P7A but no weight_to_share at all -- fails control #8


def _build_full_dataset(symbols=("AAA", "BBB"), periods_days=560, horizon_days=3, seed=0) -> pd.DataFrame:
    rng = np.random.default_rng(seed)
    dates = pd.date_range("2020-01-01", periods=periods_days, freq="D", tz="UTC")
    rows = []
    for sym in symbols:
        for i, d in enumerate(dates):
            f1 = float(rng.normal())
            target = 1 if f1 > 0.0 else 0
            rows.append({
                "symbol": sym, "end_ts": d, "f1": f1, "target": target,
                "label_end_ts": d + pd.Timedelta(days=horizon_days),
                "fwd_ret": 999.0 if target == 1 else -999.0,
            })
    return pd.DataFrame(rows)


def _write_full_run_dir(run_dir: Path, df: pd.DataFrame) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    feats = df[["symbol", "end_ts", "f1"]].copy()
    targs = df[["symbol", "end_ts", "target", "label_end_ts", "fwd_ret"]].copy()
    feats["end_ts"] = feats["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["end_ts"] = targs["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["label_end_ts"] = targs["label_end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    feats.to_csv(run_dir / "features.csv", index=False)
    targs.to_csv(run_dir / "targets.csv", index=False)
    generate_feature_schema(run_dir, id_columns=["symbol", "end_ts"])


def _build_flat_bars_with_high_low(df: pd.DataFrame) -> pd.DataFrame:
    """Flat closes (decoupled from the deliberately extreme fwd_ret label,
    same rationale as test_economic_walkforward.py's `_build_flat_bars`) plus
    a small, genuine high/low spread -- required by the official P7A
    `rust_conservative_bar_range_v1` execution pricing model."""
    rows = []
    for (sym, ts), _ in df.groupby(["symbol", "end_ts"]):
        rows.append({
            "symbol": sym, "end_ts": pd.Timestamp(ts).isoformat(),
            "close": 100.0, "high": 100.5, "low": 99.5,
        })
    return pd.DataFrame(rows)


def _synthetic_bars_provenance(bars_path: Path) -> Dict[str, Any]:
    bars = pd.read_csv(bars_path)
    end_ts = pd.to_datetime(bars["end_ts"], utc=True)
    symbol_universe = sorted(bars["symbol"].astype(str).unique().tolist())
    coverage_start = end_ts.min().isoformat()
    coverage_end = (end_ts.max() + pd.Timedelta(seconds=1)).isoformat()
    evidence = build_corporate_action_evidence(
        source_provider_id="test_fixture_no_known_corporate_actions",
        covered_symbol_universe=symbol_universe,
        coverage_start_utc=coverage_start,
        coverage_end_utc=coverage_end,
        corporate_action_entries=(),
    )
    return build_bars_provenance_manifest(
        price_provenance={
            "close_column": "close",
            "provider_ids_observed": ["test_fixture"],
            "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED,
            "provider_metadata_available": True,
            "convention_basis": "synthetic test fixture — no real provider involved",
        },
        corporate_action_policy=CA_POLICY_FORBID_AFFECTED_PERIODS,
        corporate_action_evidence_id=evidence["evidence_id"],
        corporate_action_evidence=evidence,
        forbidden_periods=(),
        timeframe="1D",
        start_utc=coverage_start,
        end_utc=coverage_end,
        symbol_universe=symbol_universe,
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE,
        bars=bars,
        artifact_path=bars_path,
    )


def _registered_run(tmp_path: Path, name: str, *, registry_db: Path, economic_spec: EconomicWalkForwardSpec, seed: int = 0):
    run_dir = tmp_path / name
    df = _build_full_dataset(periods_days=560, seed=seed)
    _write_full_run_dir(run_dir, df)
    bars_path = run_dir / "bars.csv"
    _build_flat_bars_with_high_low(df).to_csv(bars_path, index=False)
    out_path = run_registered_economic_walkforward_eval(
        run_dir,
        experiment_id=f"p7a_p7b_replay_stress.test.{name}",
        hypothesis_id=f"p7a_p7b_replay_stress.hyp.{name}",
        strategy_id=f"research.replay_stress_{name}",
        bars_csv=bars_path,
        economic_spec=economic_spec,
        bars_provenance=_synthetic_bars_provenance(bars_path),
        registry_db=registry_db,
        wf_spec=WalkForwardSpec(**BASE_SPEC_KW),
        steps=10,
    )
    out = json.loads(out_path.read_text(encoding="utf-8"))
    return out["registry"]["trial_id"], out_path


def _default_stress_kwargs(registry_db: Path, trial_id: str, stress_out_dir: Path) -> Dict[str, Any]:
    return dict(
        registry_db=registry_db,
        trial_id=trial_id,
        stress_out_dir=stress_out_dir,
        stress_execution_slippage_bps=20,
        stress_execution_volatility_mult_bps=50,
        stress_max_target_qty=None,
        stress_max_position_notional_usd=None,
        max_drawdown_ceiling=0.99,
    )


# ---------------------------------------------------------------------------
# Positive path (control #10) + no-new-trial (#11) + holdout reserved (#12)
# ---------------------------------------------------------------------------

def test_genuine_replay_through_real_p7a_p7b_evaluates_and_passes(tmp_path):
    """Control #10: the same OOS predictions genuinely replay through the
    real P7A/P7B machinery and produce a real pass/fail judgment."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, _ = _registered_run(tmp_path, "a", registry_db=registry_db, economic_spec=OFFICIAL_SPEC)

    result = _run_replay_stress(**_default_stress_kwargs(registry_db, trial_id, tmp_path / "stress"))

    assert result["status"] == "evaluated"
    assert result["trial_id"] == trial_id
    assert result["passed"] is True
    assert result["protocol_id"] == "p7a_p7b_economic_replay_stress_v1"
    assert isinstance(result["stressed_max_drawdown"], float)


def test_replay_never_registers_a_new_trial(tmp_path):
    """Control #11: a replay stress evaluation is an evaluation slice of the
    existing trial, never a new trial registration."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, _ = _registered_run(tmp_path, "a", registry_db=registry_db, economic_spec=OFFICIAL_SPEC)

    store = ResearchResultStore(registry_db)
    trials_before = store.list_trials()

    _run_replay_stress(**_default_stress_kwargs(registry_db, trial_id, tmp_path / "stress"))

    trials_after = store.list_trials()
    assert [t["trial_id"] for t in trials_after] == [t["trial_id"] for t in trials_before]
    assert len(trials_after) == 1


def test_replay_leaves_holdout_reserved(tmp_path):
    """Control #12: the stressed replay's own artifact still reports the
    final holdout as reserved, never evaluated -- replay stress never
    touches holdout data."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, _ = _registered_run(tmp_path, "a", registry_db=registry_db, economic_spec=OFFICIAL_SPEC)

    result = _run_replay_stress(**_default_stress_kwargs(registry_db, trial_id, tmp_path / "stress"))

    stressed = json.loads(Path(result["stressed_artifact_path"]).read_text(encoding="utf-8"))
    assert stressed["holdout"] == {"status": "reserved_not_evaluated"}


# ---------------------------------------------------------------------------
# Tamper / missing-input controls (#1-#4)
# ---------------------------------------------------------------------------

def test_bars_file_changed_after_original_run_fails_closed(tmp_path):
    """Control #1."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, econ_path = _registered_run(tmp_path, "a", registry_db=registry_db, economic_spec=OFFICIAL_SPEC)
    econ = json.loads(econ_path.read_text(encoding="utf-8"))
    bars_path = Path(econ["inputs"]["bars_csv"]["path"])

    original = bars_path.read_bytes()
    try:
        bars_path.write_bytes(original + b"\n999,MUTATED,999.0,999.0,999.0\n")
        with pytest.raises(ReplayAuthorityError, match="bars_csv"):
            _run_replay_stress(**_default_stress_kwargs(registry_db, trial_id, tmp_path / "stress"))
    finally:
        bars_path.write_bytes(original)

    # restored -- a clean replay now succeeds again (proves the tamper, not
    # something else, caused the failure)
    result = _run_replay_stress(**_default_stress_kwargs(registry_db, trial_id, tmp_path / "stress_clean"))
    assert result["status"] == "evaluated"


def test_oos_predictions_changed_after_original_run_fails_closed(tmp_path):
    """Control #2."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, econ_path = _registered_run(tmp_path, "a", registry_db=registry_db, economic_spec=OFFICIAL_SPEC)
    econ = json.loads(econ_path.read_text(encoding="utf-8"))
    oos_path = Path(econ["inputs"]["oos_predictions_csv"]["path"])

    original = oos_path.read_bytes()
    try:
        oos_path.write_bytes(original + b"\n")
        with pytest.raises(ReplayAuthorityError, match="oos_predictions_csv"):
            _run_replay_stress(**_default_stress_kwargs(registry_db, trial_id, tmp_path / "stress"))
    finally:
        oos_path.write_bytes(original)


def test_walk_forward_eval_changed_after_original_run_fails_closed(tmp_path):
    """Control #3."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, econ_path = _registered_run(tmp_path, "a", registry_db=registry_db, economic_spec=OFFICIAL_SPEC)
    econ = json.loads(econ_path.read_text(encoding="utf-8"))
    wf_path = Path(econ["inputs"]["walk_forward_eval"]["path"])

    original = wf_path.read_bytes()
    try:
        wf_path.write_bytes(original + b" ")
        with pytest.raises(ReplayAuthorityError, match="walk_forward_eval"):
            _run_replay_stress(**_default_stress_kwargs(registry_db, trial_id, tmp_path / "stress"))
    finally:
        wf_path.write_bytes(original)


def test_recorded_input_missing_fails_closed(tmp_path):
    """Control #4: a recorded input file has been deleted, not merely
    mutated -- distinct code path from the byte/hash mismatch checks."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, econ_path = _registered_run(tmp_path, "a", registry_db=registry_db, economic_spec=OFFICIAL_SPEC)
    econ = json.loads(econ_path.read_text(encoding="utf-8"))
    bars_path = Path(econ["inputs"]["bars_csv"]["path"])

    saved = bars_path.read_bytes()
    bars_path.unlink()
    try:
        with pytest.raises(ReplayAuthorityError, match="no longer exists"):
            _run_replay_stress(**_default_stress_kwargs(registry_db, trial_id, tmp_path / "stress"))
    finally:
        bars_path.write_bytes(saved)


def test_bars_provenance_mismatch_fails_closed(tmp_path):
    """Control #5: the bars.csv FILE itself is untouched (its own sha256
    still verifies), but the recorded `bars_provenance` block inside
    economic_walk_forward.json has been tampered with -- the downstream
    `run_economic_walkforward` -> `require_bars_match_manifest` content
    check must still fail closed, proving provenance identity is verified
    independently of the raw file hash."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, econ_path = _registered_run(tmp_path, "a", registry_db=registry_db, economic_spec=OFFICIAL_SPEC)
    econ = json.loads(econ_path.read_text(encoding="utf-8"))

    original_text = econ_path.read_text(encoding="utf-8")
    tampered = dict(econ)
    tampered["bars_provenance"] = dict(econ["bars_provenance"])
    tampered["bars_provenance"]["canonical_semantic_bars_hash"] = "0" * 64
    try:
        econ_path.write_text(json.dumps(tampered), encoding="utf-8")
        with pytest.raises(Exception):
            _run_replay_stress(**_default_stress_kwargs(registry_db, trial_id, tmp_path / "stress"))
    finally:
        econ_path.write_text(original_text, encoding="utf-8")


# ---------------------------------------------------------------------------
# Non-official baseline controls (#7, #8)
# ---------------------------------------------------------------------------

def test_baseline_not_using_official_p7a_is_not_evaluable(tmp_path):
    """Control #7."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, _ = _registered_run(tmp_path, "a", registry_db=registry_db, economic_spec=NON_OFFICIAL_PRICING_SPEC)

    result = _run_replay_stress(**_default_stress_kwargs(registry_db, trial_id, tmp_path / "stress"))

    assert result["status"] == "not_evaluable"
    assert "execution_pricing" in result["reason"]


def test_baseline_not_using_official_p7b_is_not_evaluable(tmp_path):
    """Control #8."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, _ = _registered_run(tmp_path, "a", registry_db=registry_db, economic_spec=OFFICIAL_PRICING_NO_WTS_SPEC)

    result = _run_replay_stress(**_default_stress_kwargs(registry_db, trial_id, tmp_path / "stress"))

    assert result["status"] == "not_evaluable"
    assert "weight_to_share" in result["reason"]


# ---------------------------------------------------------------------------
# research_trial_id controls (#6, #9)
# ---------------------------------------------------------------------------

def test_unknown_trial_id_fails_closed(tmp_path):
    """Control #6 (CLI layer): a research_trial_id that was never
    registered fails closed rather than silently substituting anything."""
    registry_db = tmp_path / "registry.sqlite3"
    _registered_run(tmp_path, "a", registry_db=registry_db, economic_spec=OFFICIAL_SPEC)

    with pytest.raises(KeyError, match="unknown trial_id"):
        _run_replay_stress(**_default_stress_kwargs(registry_db, "trial_that_was_never_registered", tmp_path / "stress"))


def test_stress_result_is_bound_to_the_trial_it_was_computed_from_not_a_different_one(tmp_path):
    """Control #9: two genuinely distinct, independently registered trials
    (A and B) each produce their OWN replay stress result -- trial A's
    result must never be usable as if it were trial B's. Proven here by
    replaying both and confirming the returned `trial_id` and
    `baseline_economic_eval_id` are trial-specific, never interchangeable;
    the Rust-level promotion gate (`evaluate_promotion` in
    mqk-promotion/src/evaluator.rs) is what actually refuses a mismatched
    binding at promotion time -- see
    `mqk-promotion/tests/scenario_promotion_requires_robustness_evidence_01.rs`."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_a, _ = _registered_run(tmp_path, "a", registry_db=registry_db, economic_spec=OFFICIAL_SPEC, seed=0)
    trial_b, _ = _registered_run(tmp_path, "b", registry_db=registry_db, economic_spec=OFFICIAL_SPEC, seed=1)
    assert trial_a != trial_b

    result_a = _run_replay_stress(**_default_stress_kwargs(registry_db, trial_a, tmp_path / "stress_a"))
    result_b = _run_replay_stress(**_default_stress_kwargs(registry_db, trial_b, tmp_path / "stress_b"))

    assert result_a["trial_id"] == trial_a
    assert result_b["trial_id"] == trial_b
    assert result_a["baseline_economic_eval_id"] != result_b["baseline_economic_eval_id"]
    # bars are intentionally decoupled from the (seed-varying) label/feature
    # data in this fixture (see `_build_flat_bars_with_high_low`), so bars
    # content is expected to be identical across A/B -- the OOS predictions
    # (derived from the seed-varying targets) are what actually differ.
    assert result_a["oos_predictions_csv_sha256"] != result_b["oos_predictions_csv_sha256"]


# ---------------------------------------------------------------------------
# Durable evidence content proof
# ---------------------------------------------------------------------------

def test_evaluated_result_carries_required_durable_evidence_fields(tmp_path):
    """The mission's DURABLE EVIDENCE list: trial_id, baseline economic
    protocol identity (via baseline_economic_eval_id), bars/OOS/walk-forward
    SHA-256, bars provenance hash, stress-spec identity, and the stressed
    result's own artifact SHA-256 must all be present and self-consistent."""
    registry_db = tmp_path / "registry.sqlite3"
    trial_id, econ_path = _registered_run(tmp_path, "a", registry_db=registry_db, economic_spec=OFFICIAL_SPEC)
    econ = json.loads(econ_path.read_text(encoding="utf-8"))

    result = _run_replay_stress(**_default_stress_kwargs(registry_db, trial_id, tmp_path / "stress"))

    assert result["trial_id"] == trial_id
    assert result["baseline_economic_eval_id"] == econ["ids"]["economic_eval_id"]
    assert result["bars_csv_sha256"] == econ["inputs"]["bars_csv"]["sha256"]
    assert result["oos_predictions_csv_sha256"] == econ["inputs"]["oos_predictions_csv"]["sha256"]
    assert result["walk_forward_eval_sha256"] == econ["inputs"]["walk_forward_eval"]["sha256"]
    assert result["bars_provenance_hash"] == econ["bars_provenance"]["canonical_semantic_bars_hash"]
    assert result["stress_spec"] == {
        "execution_pricing_slippage_bps": 20,
        "execution_pricing_volatility_mult_bps": 50,
        "max_target_qty": None,
        "max_position_notional_usd": None,
    }
    assert result["stressed_artifact_sha256"] == sha256_file(Path(result["stressed_artifact_path"]))
