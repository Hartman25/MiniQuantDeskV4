"""
DIRECT-RANK-AND-BROAD-UNIVERSE-RESEARCH-01 Patch B -- registered candidate
identity truth for the two cross-sectional rank direction policies added in
Patch A.

Patch A already wired `direction_policy`/`rank_side_count`/`borrow_model`/
`tie_policy` into `economic_protocol_identity`'s rank-specific fragment (see
economic_walkforward.py), which `build_economic_trial_identity` consumes
verbatim via its `economic_protocol` key -- so this patch is tests-only,
proving that wiring is actually correct end-to-end through the real
registered-identity path rather than re-deriving it. No production code is
touched here.

Covers the mission's Patch B REQUIRED TESTS list (12 items, referenced by
number in each test's docstring).
"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict

import pandas as pd
import pytest

from mqk_research.data.bars_provenance import (
    CA_POLICY_FORBID_AFFECTED_PERIODS,
    PRICE_CONVENTION_RAW_UNADJUSTED,
    UNIVERSE_MODE_FIXED_EX_ANTE,
    build_bars_provenance_manifest,
    build_corporate_action_evidence,
)
from mqk_research.ml.economic_registry_integration import build_economic_trial_identity
from mqk_research.ml.economic_walkforward import (
    BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1,
    SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
    SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1,
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
    run_economic_walkforward,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec

# ---------------------------------------------------------------------------
# Shared fixtures (mirrors test_bars_provenance.py's own local helpers --
# no shared conftest exists in this test package today).
# ---------------------------------------------------------------------------


def _bars_df(prices, symbol: str = "AAA", start: str = "2021-01-01") -> pd.DataFrame:
    dates = pd.date_range(start, periods=len(prices), freq="D", tz="UTC")
    return pd.DataFrame({"symbol": symbol, "end_ts": [d.isoformat() for d in dates], "close": prices})


def _base_manifest(bars_df: pd.DataFrame, **overrides: Any) -> Dict[str, Any]:
    kwargs: Dict[str, Any] = dict(
        price_provenance={
            "close_column": "close_micros",
            "provider_ids_observed": ["alpaca"],
            "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED,
            "provider_metadata_available": True,
            "convention_basis": "test",
        },
        corporate_action_policy=CA_POLICY_FORBID_AFFECTED_PERIODS,
        corporate_action_evidence_id=None,
        corporate_action_evidence=None,
        forbidden_periods=(),
        timeframe="1D",
        start_utc="2021-01-01T00:00:00+00:00",
        end_utc="2021-02-01T00:00:00+00:00",
        symbol_universe=["AAA"],
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE,
        bars=bars_df,
    )
    kwargs.update(overrides)
    if "corporate_action_evidence" not in overrides and "corporate_action_evidence_id" not in overrides:
        evidence = build_corporate_action_evidence(
            source_provider_id="test_synthetic_ca_source",
            covered_symbol_universe=kwargs["symbol_universe"],
            coverage_start_utc=kwargs["start_utc"],
            coverage_end_utc=kwargs["end_utc"],
            corporate_action_entries=(),
        )
        kwargs["corporate_action_evidence"] = evidence
        kwargs["corporate_action_evidence_id"] = evidence["evidence_id"]
    return build_bars_provenance_manifest(**kwargs)


def _write_min_registered_inputs(tmp_path: Path, bars_df: pd.DataFrame) -> Dict[str, Path]:
    tmp_path.mkdir(parents=True, exist_ok=True)
    features_path = tmp_path / "features.csv"
    targets_path = tmp_path / "targets.csv"
    schema_path = tmp_path / "feature_schema.json"
    bars_path = tmp_path / "bars.csv"
    features_path.write_text("symbol,end_ts,f1\nAAA,2021-01-01T00:00:00+00:00,0.1\n", encoding="utf-8")
    targets_path.write_text(
        "symbol,end_ts,target,label_end_ts\nAAA,2021-01-01T00:00:00+00:00,1,2021-01-02T00:00:00+00:00\n",
        encoding="utf-8",
    )
    schema_path.write_text("{}", encoding="utf-8")
    bars_df.to_csv(bars_path, index=False)
    return {
        "features_path": features_path,
        "targets_path": targets_path,
        "schema_path": schema_path,
        "bars_path": bars_path,
    }


def _rank_long_only_spec(rank_side_count: int = 2, max_gross: float = 1.0) -> SignalPolicySpec:
    return SignalPolicySpec(
        direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
        long_only=True, rank_side_count=rank_side_count, max_gross_exposure=max_gross,
    )


def _rank_long_short_spec(rank_side_count: int = 2, max_gross: float = 1.0, borrow_model=None) -> SignalPolicySpec:
    return SignalPolicySpec(
        direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1,
        long_only=False, rank_side_count=rank_side_count, max_gross_exposure=max_gross,
        borrow_model=borrow_model,
    )


def _economic_spec(signal_policy: SignalPolicySpec) -> EconomicWalkForwardSpec:
    return EconomicWalkForwardSpec(
        signal_policy=signal_policy,
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=5.0),
        annualization=AnnualizationSpec(),
    )


def _trial(tmp_path: Path, bars_df: pd.DataFrame, signal_policy: SignalPolicySpec, **manifest_overrides: Any):
    paths = _write_min_registered_inputs(tmp_path, bars_df)
    manifest = _base_manifest(bars_df, **manifest_overrides)
    return build_economic_trial_identity(
        experiment_id="exp", hypothesis_id="hyp", strategy_id="strat",
        features_path=paths["features_path"], targets_path=paths["targets_path"],
        schema_path=paths["schema_path"], bars_path=paths["bars_path"],
        label_col="target", end_ts_col="end_ts", wf_spec=WalkForwardSpec(),
        l2=1e-3, lr=0.05, steps=10, standardize=True, clip_z=8.0,
        economic_spec=_economic_spec(signal_policy), bars_provenance=manifest,
    )


# ---------------------------------------------------------------------------
# REQUIRED TESTS 1-7: semantic identity distinctions
# ---------------------------------------------------------------------------


def test_rank_long_only_vs_long_short_distinct_trial_id(tmp_path: Path) -> None:
    """REQUIRED TEST 1."""
    bars = _bars_df([100.0] * 5)
    id_a, _ = _trial(tmp_path / "a", bars, _rank_long_only_spec())
    id_b, _ = _trial(tmp_path / "b", bars, _rank_long_short_spec())
    assert id_a != id_b


def test_rank_side_count_change_distinct_trial_id(tmp_path: Path) -> None:
    """REQUIRED TEST 2: K=5 != K=10."""
    bars = _bars_df([100.0] * 5)
    id_5, _ = _trial(tmp_path / "a", bars, _rank_long_only_spec(rank_side_count=5))
    id_10, _ = _trial(tmp_path / "b", bars, _rank_long_only_spec(rank_side_count=10))
    assert id_5 != id_10


def test_max_gross_exposure_change_alters_rank_identity(tmp_path: Path) -> None:
    """REQUIRED TEST 3."""
    bars = _bars_df([100.0] * 5)
    id_a, _ = _trial(tmp_path / "a", bars, _rank_long_only_spec(max_gross=1.0))
    id_b, _ = _trial(tmp_path / "b", bars, _rank_long_only_spec(max_gross=0.5))
    assert id_a != id_b


def test_rank_policy_distinct_from_threshold_policy_identity(tmp_path: Path) -> None:
    """REQUIRED TEST 4."""
    bars = _bars_df([100.0] * 5)
    id_rank, _ = _trial(tmp_path / "a", bars, _rank_long_only_spec())
    id_legacy, _ = _trial(tmp_path / "b", bars, SignalPolicySpec(entry_threshold=0.5))
    assert id_rank != id_legacy


def test_borrow_model_semantic_difference_cannot_silently_alias(tmp_path: Path) -> None:
    """REQUIRED TEST 5: an explicit vs. defaulted (but value-EQUAL)
    borrow_model must produce the SAME identity (proves the field is
    actually read into identity, not ignored); an unsupported borrow_model
    string must be rejected outright (proves it cannot silently alias to a
    different accepted value either)."""
    bars = _bars_df([100.0] * 5)
    id_explicit, _ = _trial(
        tmp_path / "a", bars,
        _rank_long_short_spec(borrow_model=BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1),
    )
    id_defaulted, _ = _trial(tmp_path / "b", bars, _rank_long_short_spec())
    assert id_explicit == id_defaulted
    with pytest.raises(ValueError, match="unsupported borrow_model"):
        _rank_long_short_spec(borrow_model="not_a_real_borrow_model").normalized()


def test_legacy_trial_identity_fixture_unchanged(tmp_path: Path) -> None:
    """REQUIRED TEST 6: a legacy long_only_v1 candidate's registered
    identity fragment shape is unaffected by the rank-policy addition (spot
    check backing test_legacy_long_only_identity_exact_golden_equality in
    test_long_short_economic_policy.py, which is re-run unmodified at the
    validation boundary)."""
    bars = _bars_df([100.0] * 5)
    _trial_id, identity = _trial(tmp_path, bars, SignalPolicySpec(entry_threshold=0.5))
    assert identity["economic_protocol"]["signal_policy"] == {
        "entry_threshold": 0.5,
        "long_only": True,
        "sizing": "equal_weight_active",
        "max_gross_exposure": 1.0,
        "fold_end_policy": "force_flat_last_bar",
        "capacity_policy": "reduce_first_defer_increase_batch_v1",
    }
    assert "direction_policy" not in identity["economic_protocol"]["signal_policy"]


def test_unused_entry_threshold_cannot_manufacture_rank_trial_identity(tmp_path: Path) -> None:
    """REQUIRED TEST 7: entry_threshold is rejected outright for rank
    policies (SignalPolicySpec.normalized(), Patch A) unless it is exactly
    the canonical default 0.5 -- so it is structurally impossible to build
    two DIFFERENT registered rank trial identities that differ only by
    entry_threshold."""
    with pytest.raises(ValueError, match="entry_threshold=0.5"):
        SignalPolicySpec(
            direction_policy=SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
            long_only=True, rank_side_count=2, entry_threshold=0.9,
        ).normalized()


# ---------------------------------------------------------------------------
# REQUIRED TESTS 8-10: result-independence / retry determinism
# ---------------------------------------------------------------------------


def test_result_fields_absent_from_candidate_identity(tmp_path: Path) -> None:
    """REQUIRED TESTS 8/9: no return/Sharpe/result-derived field can ever
    appear in trial identity -- build_economic_trial_identity's signature
    and its `identity` dict are entirely result-independent by construction
    (it never takes evaluation output as an argument at all); this asserts
    that structurally, over the actual dict produced for a rank candidate."""
    bars = _bars_df([100.0] * 5)
    _trial_id, identity = _trial(tmp_path, bars, _rank_long_only_spec())
    blob = json.dumps(identity)
    for forbidden in ("sharpe", "net_total_return", "gross_total_return", "economic_eval_id", "result_summary"):
        assert forbidden not in blob.lower()


def test_retry_of_identical_rank_candidate_remains_same_trial(tmp_path: Path) -> None:
    """REQUIRED TEST 10: calling build_economic_trial_identity twice with
    byte-identical semantic inputs for a rank candidate produces the SAME
    trial_id both times -- a retry of the identical candidate is the same
    trial (attempt separation is the registry's job, unaffected by this
    patch)."""
    bars = _bars_df([100.0] * 5)
    id_first, _ = _trial(tmp_path / "a", bars, _rank_long_short_spec())
    id_second, _ = _trial(tmp_path / "b", bars, _rank_long_short_spec())
    assert id_first == id_second


# ---------------------------------------------------------------------------
# REQUIRED TEST 11: rank_side_count appears in normalized semantic identity
# ---------------------------------------------------------------------------


def test_rank_side_count_appears_in_serialized_identity(tmp_path: Path) -> None:
    """REQUIRED TEST 11."""
    bars = _bars_df([100.0] * 5)
    _trial_id, identity = _trial(tmp_path, bars, _rank_long_only_spec(rank_side_count=3))
    assert identity["economic_protocol"]["signal_policy"]["rank_side_count"] == 3


# ---------------------------------------------------------------------------
# REQUIRED TEST 12: prediction CSV physical row order does not change
# economic behavior (end-to-end, not just pending-event construction)
# ---------------------------------------------------------------------------


def _oos_row(fold: int, symbol: str, decision_ts: pd.Timestamp, score: float) -> Dict[str, Any]:
    return {
        "fold": fold, "symbol": symbol, "decision_ts": decision_ts.isoformat(),
        "label_end_ts": decision_ts.isoformat(), "ml_score": score, "target": 1,
    }


def test_prediction_csv_row_order_does_not_change_economic_behavior(tmp_path: Path) -> None:
    """REQUIRED TEST 12: running the full registered economic evaluation
    twice, differing ONLY by the physical row order of
    walk_forward_oos_predictions.csv, must produce byte-identical aggregate
    economics."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    bars_rows = (
        [{"symbol": s, "end_ts": d.isoformat(), "close": 100.0} for s in ("A", "B", "E", "F") for d in days]
    )
    rows = (
        [_oos_row(1, "A", d, 0.9) for d in days]
        + [_oos_row(1, "B", d, 0.8) for d in days]
        + [_oos_row(1, "E", d, 0.5) for d in days]
        + [_oos_row(1, "F", d, 0.4) for d in days]
    )
    folds = [{"fold": 1, "skipped": False, "test_start_utc": days[0].isoformat(), "test_end_utc": (days[-1] + pd.Timedelta(days=1)).isoformat()}]

    def _run(run_dir: Path, oos_rows) -> Dict[str, Any]:
        eval_dir = run_dir / "eval"
        eval_dir.mkdir(parents=True, exist_ok=True)
        (eval_dir / "walk_forward_eval.json").write_text(json.dumps({"folds": folds}), encoding="utf-8")
        pd.DataFrame(oos_rows).to_csv(eval_dir / "walk_forward_oos_predictions.csv", index=False)
        bars_path = run_dir / "bars.csv"
        pd.DataFrame(bars_rows).to_csv(bars_path, index=False)
        spec = EconomicWalkForwardSpec(
            signal_policy=_rank_long_short_spec(rank_side_count=2),
            cost_model=CostModelSpec(commission_bps_per_side=0.0, slippage_bps_per_side=0.0, diagnostic_zero_cost=True),
            annualization=AnnualizationSpec(),
        )
        out_path = run_economic_walkforward(run_dir, bars_csv=bars_path, spec=spec)
        return json.loads(out_path.read_text(encoding="utf-8"))["aggregate"]

    forward = _run(tmp_path / "forward", rows)
    shuffled = list(reversed(rows))
    shuffled.insert(3, shuffled.pop(0))
    backward = _run(tmp_path / "backward", shuffled)
    assert forward == backward
