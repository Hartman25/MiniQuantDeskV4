"""
P7A (RESEARCH-EXECUTION-PRICING-PARITY-01) — execution-price-model parity
between Python Research's economic evaluator and the accepted Rust
conservative fill model (core-rs/crates/mqk-backtest/src/engine.rs).

Covers the mission's REQUIRED NEGATIVE / REGRESSION TESTS list (items
referenced by number in each test's docstring). Chronology/cohort/holdout
invariants this patch does NOT touch (items 19, 20, 22, 24, 26) are not
re-proven here -- they're already covered by test_economic_walkforward.py's
existing suite, which this patch leaves passing unmodified; P7A's own
mechanism (_row_execution_price_cost) only ever ADDS a cost number to an
already-decided execution, it never changes which bar executes, so those
invariants cannot have been disturbed by construction.
"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict, List

import pandas as pd
import pytest

from mqk_research.data.bars_provenance import (
    CA_POLICY_FORBID_AFFECTED_PERIODS,
    PRICE_CONVENTION_RAW_UNADJUSTED,
    UNIVERSE_MODE_FIXED_EX_ANTE,
    BarShapeInvalid,
    BarsProvenanceContentMismatch,
    BarsProvenanceUnverifiable,
    build_bars_provenance_manifest,
    build_corporate_action_evidence,
    canonical_pricing_bars_hash,
    require_bars_pricing_provenance,
)
from mqk_research.ml.economic_registry_integration import (
    build_economic_trial_identity,
    require_official_execution_pricing_parity,
)
from mqk_research.ml.economic_walkforward import (
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
    economic_protocol_identity,
    load_bars,
    run_economic_walkforward,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec
from mqk_research.ml.execution_pricing import (
    EXECUTION_PRICING_MODEL_ID_CLOSE_ONLY_DIAGNOSTIC_V1,
    EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    ExecutionPricingSpec,
    conservative_fill_price_micros,
    to_micros,
)

GOLDEN_FIXTURE_PATH = (
    Path(__file__).resolve().parents[2] / "docs" / "research" / "fixtures" / "execution_pricing_golden_v1.json"
)


def _ts(s: str) -> pd.Timestamp:
    return pd.Timestamp(s, tz="UTC")


# ---------------------------------------------------------------------------
# Shared fixture helpers (direct-fixture style, mirrors test_economic_
# walkforward.py's _bar_row/_write_direct_fixture, extended with high/low)
# ---------------------------------------------------------------------------


def _single_fold(test_start: str, test_end: str, fold: int = 1) -> Dict[str, Any]:
    return {
        "fold": fold,
        "skipped": False,
        "test_start_utc": _ts(test_start).isoformat(),
        "test_end_utc": _ts(test_end).isoformat(),
    }


def _oos_row(fold: int, symbol: str, decision_ts, score: float) -> Dict[str, Any]:
    return {
        "fold": fold,
        "symbol": symbol,
        "decision_ts": _ts(str(decision_ts)).isoformat(),
        "label_end_ts": _ts(str(decision_ts)).isoformat(),
        "ml_score": score,
        "target": 1,
    }


def _bar_row_hl(symbol: str, end_ts, close: float, high: float, low: float) -> Dict[str, Any]:
    return {
        "symbol": symbol,
        "end_ts": _ts(str(end_ts)).isoformat(),
        "close": close,
        "high": high,
        "low": low,
    }


def _write_direct_fixture(
    run_dir: Path, *, folds: List[Dict[str, Any]], oos_rows: List[Dict[str, Any]], bars_rows: List[Dict[str, Any]]
) -> Path:
    eval_dir = run_dir / "eval"
    eval_dir.mkdir(parents=True, exist_ok=True)
    (eval_dir / "walk_forward_eval.json").write_text(json.dumps({"folds": folds}), encoding="utf-8")
    pd.DataFrame(oos_rows).to_csv(eval_dir / "walk_forward_oos_predictions.csv", index=False)
    bars_path = run_dir / "bars.csv"
    pd.DataFrame(bars_rows).to_csv(bars_path, index=False)
    return bars_path


OFFICIAL_PRICING = ExecutionPricingSpec(
    pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    slippage_bps=0,
    volatility_mult_bps=0,
)


def _spec(*, execution_pricing: ExecutionPricingSpec = ExecutionPricingSpec(), slippage_bps_per_side: float = 5.0) -> EconomicWalkForwardSpec:
    return EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=0.5),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=slippage_bps_per_side),
        annualization=AnnualizationSpec(),
        execution_pricing=execution_pricing,
    )


def _official_spec(*, slippage_bps: int = 0, volatility_mult_bps: int = 0) -> EconomicWalkForwardSpec:
    # Double-charging guard requires cost_model.slippage_bps_per_side == 0
    # whenever the official parity model is in play (see
    # EconomicWalkForwardSpec.normalized()).
    return _spec(
        execution_pricing=ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
            slippage_bps=slippage_bps,
            volatility_mult_bps=volatility_mult_bps,
        ),
        slippage_bps_per_side=0.0,
    )


# ---------------------------------------------------------------------------
# TESTS 2-6 — golden-vector cross-language parity (see also the Rust side:
# core-rs/crates/mqk-backtest/tests/scenario_research_execution_pricing_
# parity_p7a.rs, which runs the SAME fixture through the real BacktestEngine)
# ---------------------------------------------------------------------------


def _load_golden_cases() -> List[Dict[str, Any]]:
    fixture = json.loads(GOLDEN_FIXTURE_PATH.read_text(encoding="utf-8"))
    return fixture["cases"]


@pytest.mark.parametrize("case", _load_golden_cases(), ids=lambda c: c["name"])
def test_golden_vector_matches_rust_reference(case: Dict[str, Any]) -> None:
    """TESTS 2-6: BUY/SELL ordinary fills, zero slippage, nonzero flat
    slippage, nonzero volatility multiplier, wide/narrow bars, and an
    integer-truncation edge all resolve to the exact fill-price micros the
    accepted Rust conservative_fill_price produces for the identical input
    (proven independently on the Rust side against the same fixture file)."""
    got = conservative_fill_price_micros(
        high_micros=to_micros(case["high"]),
        low_micros=to_micros(case["low"]),
        close_micros=to_micros(case["close"]),
        side=case["side"],
        slippage_bps=case["slippage_bps"],
        volatility_mult_bps=case["volatility_mult_bps"],
    )
    assert got == case["expected_fill_price_micros"]


# ---------------------------------------------------------------------------
# TEST 7 — negative slippage fails closed
# ---------------------------------------------------------------------------


def test_negative_slippage_bps_rejected_by_pricing_function() -> None:
    with pytest.raises(ValueError, match="negative slippage rejected"):
        conservative_fill_price_micros(
            high_micros=101_000_000, low_micros=99_000_000, close_micros=100_000_000,
            side="buy", slippage_bps=-1, volatility_mult_bps=0,
        )


def test_negative_volatility_mult_bps_rejected_by_pricing_function() -> None:
    with pytest.raises(ValueError, match="negative slippage rejected"):
        conservative_fill_price_micros(
            high_micros=101_000_000, low_micros=99_000_000, close_micros=100_000_000,
            side="buy", slippage_bps=0, volatility_mult_bps=-1,
        )


def test_negative_slippage_rejected_at_spec_level() -> None:
    with pytest.raises(ValueError, match="negative slippage rejected"):
        ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1, slippage_bps=-5
        ).normalized()


def test_extreme_slippage_cannot_produce_negative_sell_price() -> None:
    """Saturation floor: SELL adjustment can never push the fill below 0,
    mirroring Rust's `.max(0)`."""
    got = conservative_fill_price_micros(
        high_micros=101_000_000, low_micros=99_000_000, close_micros=100_000_000,
        side="sell", slippage_bps=1_000_000, volatility_mult_bps=0,
    )
    assert got == 0


# ---------------------------------------------------------------------------
# TESTS 8-11 — official evaluation fails closed on missing/invalid HIGH/LOW
# ---------------------------------------------------------------------------


def _bars_csv(tmp_path: Path, rows: List[Dict[str, Any]], name: str = "bars.csv") -> Path:
    path = tmp_path / name
    pd.DataFrame(rows).to_csv(path, index=False)
    return path


def test_missing_high_column_fails_closed_for_official_evaluation(tmp_path: Path) -> None:
    """TEST 8."""
    rows = [{"symbol": "AAA", "end_ts": _ts("2021-01-01").isoformat(), "close": 100.0, "low": 99.0}]
    path = _bars_csv(tmp_path, rows)
    with pytest.raises(RuntimeError, match="missing required columns"):
        load_bars(path, require_pricing_columns=True)


def test_missing_low_column_fails_closed_for_official_evaluation(tmp_path: Path) -> None:
    """TEST 9."""
    rows = [{"symbol": "AAA", "end_ts": _ts("2021-01-01").isoformat(), "close": 100.0, "high": 101.0}]
    path = _bars_csv(tmp_path, rows)
    with pytest.raises(RuntimeError, match="missing required columns"):
        load_bars(path, require_pricing_columns=True)


def test_missing_high_low_does_not_block_diagnostic_load(tmp_path: Path) -> None:
    """Negative control for TESTS 8/9: the SAME close-only bars are fine for
    the diagnostic (non-official) path -- proves the requirement is scoped
    to require_pricing_columns, not a blanket new restriction."""
    rows = [{"symbol": "AAA", "end_ts": _ts("2021-01-01").isoformat(), "close": 100.0}]
    path = _bars_csv(tmp_path, rows)
    load_bars(path, require_pricing_columns=False)  # must not raise


def test_non_finite_high_fails_closed(tmp_path: Path) -> None:
    """TEST 10."""
    rows = [{"symbol": "AAA", "end_ts": _ts("2021-01-01").isoformat(), "close": 100.0, "high": float("inf"), "low": 99.0}]
    path = _bars_csv(tmp_path, rows)
    with pytest.raises(RuntimeError, match="non-finite"):
        load_bars(path, require_pricing_columns=True)


def test_non_finite_low_fails_closed(tmp_path: Path) -> None:
    """TEST 10."""
    rows = [{"symbol": "AAA", "end_ts": _ts("2021-01-01").isoformat(), "close": 100.0, "high": 101.0, "low": float("nan")}]
    path = _bars_csv(tmp_path, rows)
    with pytest.raises(RuntimeError, match="non-finite"):
        load_bars(path, require_pricing_columns=True)


def test_high_less_than_low_fails_closed(tmp_path: Path) -> None:
    """TEST 11: Rust does not itself validate this (confirmed by direct
    reading of engine.rs / BacktestBar::new) -- Python's official path fails
    closed anyway."""
    rows = [{"symbol": "AAA", "end_ts": _ts("2021-01-01").isoformat(), "close": 100.0, "high": 90.0, "low": 95.0}]
    path = _bars_csv(tmp_path, rows)
    with pytest.raises(RuntimeError, match="high < low"):
        load_bars(path, require_pricing_columns=True)


def test_close_outside_high_low_range_fails_closed(tmp_path: Path) -> None:
    """TEST 11 (impossible bar shape variant)."""
    rows = [{"symbol": "AAA", "end_ts": _ts("2021-01-01").isoformat(), "close": 110.0, "high": 105.0, "low": 95.0}]
    path = _bars_csv(tmp_path, rows)
    with pytest.raises(RuntimeError, match="outside \\[low, high\\]"):
        load_bars(path, require_pricing_columns=True)


def test_canonical_pricing_bars_hash_rejects_impossible_bar_shape() -> None:
    """TEST 11 at the provenance layer."""
    bars = pd.DataFrame([_bar_row_hl("AAA", "2021-01-01", 100.0, 90.0, 95.0)])
    with pytest.raises(BarShapeInvalid):
        canonical_pricing_bars_hash(bars)


# ---------------------------------------------------------------------------
# TESTS 12/13 — changed HIGH/LOW changes OFFICIAL economic bars identity
# (and, as a negative control, must NOT change identity for a diagnostic
# trial, since diagnostic pricing never consumes high/low)
# ---------------------------------------------------------------------------


def _ca_manifest(bars_df: pd.DataFrame) -> Dict[str, Any]:
    evidence = build_corporate_action_evidence(
        source_provider_id="test_synthetic_ca_source",
        covered_symbol_universe=["AAA"],
        coverage_start_utc="2021-01-01T00:00:00+00:00",
        coverage_end_utc="2021-02-01T00:00:00+00:00",
        corporate_action_entries=(),
    )
    return build_bars_provenance_manifest(
        price_provenance={
            "close_column": "close_micros",
            "provider_ids_observed": ["alpaca"],
            "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED,
            "provider_metadata_available": True,
            "convention_basis": "test",
        },
        corporate_action_policy=CA_POLICY_FORBID_AFFECTED_PERIODS,
        corporate_action_evidence_id=evidence["evidence_id"],
        corporate_action_evidence=evidence,
        forbidden_periods=(),
        timeframe="1D",
        start_utc="2021-01-01T00:00:00+00:00",
        end_utc="2021-02-01T00:00:00+00:00",
        symbol_universe=["AAA"],
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE,
        bars=bars_df,
    )


def _write_registered_inputs(tmp_path: Path, bars_df: pd.DataFrame) -> Dict[str, Path]:
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
    return {"features_path": features_path, "targets_path": targets_path, "schema_path": schema_path, "bars_path": bars_path}


def _trial_id_for(tmp_path: Path, bars_df: pd.DataFrame, economic_spec: EconomicWalkForwardSpec) -> str:
    paths = _write_registered_inputs(tmp_path, bars_df)
    manifest = _ca_manifest(bars_df)
    trial_id, _identity = build_economic_trial_identity(
        experiment_id="exp", hypothesis_id="hyp", strategy_id="strat",
        features_path=paths["features_path"], targets_path=paths["targets_path"], schema_path=paths["schema_path"],
        bars_path=paths["bars_path"], label_col="target", end_ts_col="end_ts", wf_spec=WalkForwardSpec(),
        l2=1e-3, lr=0.05, steps=10, standardize=True, clip_z=8.0,
        economic_spec=economic_spec, bars_provenance=manifest,
    )
    return trial_id


def test_changed_high_changes_official_trial_identity(tmp_path: Path) -> None:
    """TEST 12."""
    bars_a = pd.DataFrame([_bar_row_hl("AAA", "2021-01-01", 100.0, 101.0, 99.0)])
    bars_b = pd.DataFrame([_bar_row_hl("AAA", "2021-01-01", 100.0, 103.0, 99.0)])  # HIGH changed only
    spec = _official_spec()
    id_a = _trial_id_for(tmp_path / "a", bars_a, spec)
    id_b = _trial_id_for(tmp_path / "b", bars_b, spec)
    assert id_a != id_b


def test_changed_low_changes_official_trial_identity(tmp_path: Path) -> None:
    """TEST 13."""
    bars_a = pd.DataFrame([_bar_row_hl("AAA", "2021-01-01", 100.0, 101.0, 99.0)])
    bars_b = pd.DataFrame([_bar_row_hl("AAA", "2021-01-01", 100.0, 101.0, 97.0)])  # LOW changed only
    spec = _official_spec()
    id_a = _trial_id_for(tmp_path / "a", bars_a, spec)
    id_b = _trial_id_for(tmp_path / "b", bars_b, spec)
    assert id_a != id_b


def test_changed_high_does_not_change_diagnostic_trial_identity(tmp_path: Path) -> None:
    """Negative control for TESTS 12/13 + the mission's "do not expand
    identity to unused data" instruction: under the DIAGNOSTIC model, which
    never consumes high/low, changing high must NOT change trial identity."""
    bars_a = pd.DataFrame([_bar_row_hl("AAA", "2021-01-01", 100.0, 101.0, 99.0)])
    bars_b = pd.DataFrame([_bar_row_hl("AAA", "2021-01-01", 100.0, 999.0, 99.0)])
    spec = _spec()  # default diagnostic execution_pricing
    id_a = _trial_id_for(tmp_path / "a", bars_a, spec)
    id_b = _trial_id_for(tmp_path / "b", bars_b, spec)
    assert id_a == id_b


# ---------------------------------------------------------------------------
# TEST 14 — row-order permutations do NOT change semantic bars identity
# ---------------------------------------------------------------------------


def test_canonical_pricing_bars_hash_is_row_order_invariant() -> None:
    """TEST 14."""
    rows = [
        _bar_row_hl("AAA", "2021-01-01", 100.0, 101.0, 99.0),
        _bar_row_hl("AAA", "2021-01-02", 102.0, 103.0, 101.0),
        _bar_row_hl("BBB", "2021-01-01", 50.0, 51.0, 49.0),
    ]
    forward = pd.DataFrame(rows)
    reversed_ = pd.DataFrame(list(reversed(rows)))
    assert canonical_pricing_bars_hash(forward) == canonical_pricing_bars_hash(reversed_)


# ---------------------------------------------------------------------------
# TEST 15 — physical CSV byte differences with same semantic OHLC do not
# create a new candidate (official-model trial identity)
# ---------------------------------------------------------------------------


def test_physical_row_reorder_does_not_change_official_trial_identity(tmp_path: Path) -> None:
    """TEST 15."""
    rows = [
        _bar_row_hl("AAA", "2021-01-01", 100.0, 101.0, 99.0),
        _bar_row_hl("AAA", "2021-01-02", 102.0, 103.0, 101.0),
    ]
    bars_forward = pd.DataFrame(rows)
    bars_reversed = pd.DataFrame(list(reversed(rows)))
    spec = _official_spec()
    id_forward = _trial_id_for(tmp_path / "fwd", bars_forward, spec)
    id_reversed = _trial_id_for(tmp_path / "rev", bars_reversed, spec)
    assert id_forward == id_reversed


# ---------------------------------------------------------------------------
# TESTS 16/17 — pricing-model-id and slippage-parameter changes change
# economic trial identity
# ---------------------------------------------------------------------------


def test_changing_pricing_model_id_changes_protocol_identity() -> None:
    """TEST 16."""
    diagnostic = economic_protocol_identity(_spec().normalized())
    official = economic_protocol_identity(_official_spec().normalized())
    assert diagnostic["execution_pricing"] != official["execution_pricing"]
    assert diagnostic["execution_pricing"]["pricing_model_id"] == EXECUTION_PRICING_MODEL_ID_CLOSE_ONLY_DIAGNOSTIC_V1
    assert official["execution_pricing"]["pricing_model_id"] == EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1


def test_changing_slippage_bps_changes_protocol_identity() -> None:
    """TEST 17."""
    a = economic_protocol_identity(_official_spec(slippage_bps=5).normalized())
    b = economic_protocol_identity(_official_spec(slippage_bps=10).normalized())
    assert a["execution_pricing"] != b["execution_pricing"]


def test_changing_volatility_mult_bps_changes_protocol_identity() -> None:
    """TEST 17."""
    a = economic_protocol_identity(_official_spec(volatility_mult_bps=1000).normalized())
    b = economic_protocol_identity(_official_spec(volatility_mult_bps=2000).normalized())
    assert a["execution_pricing"] != b["execution_pricing"]


# ---------------------------------------------------------------------------
# TESTS 1, 21, 23 — end-to-end wiring: RED negative control, cost charged on
# the execution row (not the signal row), and the forced-flatten exception
# ---------------------------------------------------------------------------

_HL_DAYS = pd.date_range("2021-01-01", periods=5, freq="D", tz="UTC")
_HL_CLOSES = [100.0, 100.0, 105.0, 105.0, 105.0]


def _hl_bars_rows() -> List[Dict[str, Any]]:
    # Uniform +/-2 spread on every bar; day index 1 (the BUY execution row)
    # and day index 4 (the fold-end forced-flatten row) are both wide enough
    # that conservative pricing would be clearly visible IF applied.
    return [_bar_row_hl("AAA", d, c, c + 2.0, c - 2.0) for d, c in zip(_HL_DAYS, _HL_CLOSES)]


def _run_hl_fixture(run_dir: Path, spec: EconomicWalkForwardSpec) -> pd.DataFrame:
    oos_rows = [_oos_row(1, "AAA", d, 1.0) for d in _HL_DAYS]
    folds = [_single_fold("2021-01-01", "2021-01-06")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=_hl_bars_rows())
    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=spec)
    return pd.read_csv(run_dir / "eval" / "economic_returns.csv")


def test_execution_price_cost_charged_on_execution_row_not_signal_row(tmp_path: Path) -> None:
    """TEST 21. Day0 is the decision/signal row (no execution yet, weight
    still 0) -- day1 is where the BUY actually executes (weight 0 -> 1),
    using day1's own HIGH (102.0) vs its close (100.0). With
    slippage_bps=volatility_mult_bps=0, execution_price_cost on day1 must be
    exactly |102-100|/100 * 1.0 = 0.02, and exactly 0.0 on day0."""
    returns = _run_hl_fixture(tmp_path, _official_spec())
    assert returns.loc[0, "execution_price_cost"] == pytest.approx(0.0, abs=1e-12)
    assert returns.loc[1, "execution_price_cost"] == pytest.approx(0.02, abs=1e-9)
    assert returns.loc[1, "turnover"] == pytest.approx(1.0)
    # Chronology unaffected: day1's gross_return still earns from the weight
    # held BEFORE day1 (0.0), never from the weight just executed at day1.
    assert returns.loc[1, "gross_return"] == pytest.approx(0.0, abs=1e-12)


def test_forced_flatten_exception_never_applies_conservative_pricing(tmp_path: Path) -> None:
    """TEST 23. Day4 (the fold's final bar) force-flattens the still-held
    position (force_flat_last_bar). Even though day4 has a wide (+/-2) high/
    low spread that WOULD produce a large execution_price_cost under
    ordinary conservative pricing, the forced-flatten exception must charge
    turnover with ZERO execution_price_cost -- mirroring Rust's flatten_all,
    which prices forced exits from `mark`/close, never HIGH/LOW+slippage."""
    returns = _run_hl_fixture(tmp_path, _official_spec())
    last_row = returns.iloc[-1]
    assert last_row["turnover"] > 0.0  # the exit did happen
    assert last_row["execution_price_cost"] == pytest.approx(0.0, abs=1e-12)


def test_diagnostic_and_official_pricing_diverge_on_adversarial_bar(tmp_path: Path) -> None:
    """TEST 1 (RED / negative control). Pre-repair-equivalent behavior
    (diagnostic close-only pricing with a small flat bps cost) must NOT
    equal post-repair official conservative pricing's economics on a bar
    with a materially wide high/low spread relative to close -- proving the
    fixture actually exercises the divergence the mission describes, not an
    accidentally-matching pair of specs."""
    diagnostic_returns = _run_hl_fixture(tmp_path / "diag", _spec(slippage_bps_per_side=5.0))
    official_returns = _run_hl_fixture(tmp_path / "official", _official_spec())

    assert diagnostic_returns.loc[1, "execution_price_cost"] == pytest.approx(0.0, abs=1e-12)
    assert official_returns.loc[1, "execution_price_cost"] == pytest.approx(0.02, abs=1e-9)
    # Official (conservative) pricing must be materially more expensive than
    # diagnostic flat-bps pricing on this adversarial bar.
    assert official_returns.loc[1, "transaction_cost"] > diagnostic_returns.loc[1, "transaction_cost"]


def test_official_pricing_double_charging_guard(tmp_path: Path) -> None:
    """The official model already prices slippage directionally; a spec
    that ALSO sets cost_model.slippage_bps_per_side != 0 would double-charge
    it and must fail closed at normalization time."""
    bad_spec = _spec(
        execution_pricing=ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1, slippage_bps=5,
        ),
        slippage_bps_per_side=5.0,
    )
    with pytest.raises(RuntimeError, match="double-charging"):
        bad_spec.normalized()


# ---------------------------------------------------------------------------
# Same-bar / cross-symbol isolation still hold under official pricing
# (TESTS 18/19 -- these mechanisms are unchanged by P7A, but the NEW
# high_frame/low_frame plumbing added by this patch is exactly the kind of
# change that could accidentally break per-symbol timestamp alignment, so
# they're worth reverifying directly against the new code path.)
# ---------------------------------------------------------------------------


def test_same_bar_fill_still_impossible_under_official_pricing(tmp_path: Path) -> None:
    """TEST 18."""
    days = pd.date_range("2021-01-01", periods=7, freq="D", tz="UTC")
    closes = [100.0, 200.0, 200.0, 200.0, 200.0, 200.0, 200.0]  # jump is T0->T1
    bars_rows = [_bar_row_hl("AAA", d, c, c + 1.0, c - 1.0) for d, c in zip(days, closes)]
    oos_rows = [_oos_row(1, "AAA", d, 1.0) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-08")]
    bars_path = _write_direct_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=_official_spec())
    returns = pd.read_csv(tmp_path / "eval" / "economic_returns.csv")

    # The T0->T1 100% jump must contribute ZERO gross return anywhere, same
    # as the diagnostic-model proof in test_economic_walkforward.py.
    assert returns["gross_return"].abs().max() < 1e-9


def test_cross_symbol_high_low_isolation(tmp_path: Path) -> None:
    """TEST 19 (adjacent): a sibling symbol's high/low must never leak into
    another symbol's execution_price_cost -- proves high_frame/low_frame's
    per-symbol column alignment survived the new pivot plumbing."""
    days = pd.date_range("2021-01-01", periods=3, freq="D", tz="UTC")
    # AAPL: tight spread (small adverse cost). SPY: enormous spread, priced
    # wildly differently -- must not affect AAPL's own execution_price_cost.
    aapl_rows = [_bar_row_hl("AAPL", d, 100.0, 100.5, 99.5) for d in days]
    spy_rows = [_bar_row_hl("SPY", d, 300.0, 900.0, 30.0) for d in days]
    oos_rows = [_oos_row(1, "AAPL", d, 1.0) for d in days] + [_oos_row(1, "SPY", d, 0.0) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-04")]
    bars_path = _write_direct_fixture(
        tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=aapl_rows + spy_rows
    )

    run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=_official_spec())
    returns = pd.read_csv(tmp_path / "eval" / "economic_returns.csv")

    # AAPL's day-1 execution cost reflects ONLY its own +/-0.5 spread, never
    # SPY's +/-huge spread (which never becomes active at all, score=0.0).
    day1 = returns.iloc[1]
    assert day1["execution_price_cost"] == pytest.approx(0.5 / 100.0, abs=1e-9)


# ---------------------------------------------------------------------------
# Regression: holdout status untouched (TEST 25) under official pricing
# ---------------------------------------------------------------------------


def test_holdout_status_still_reserved_not_evaluated_under_official_pricing(tmp_path: Path) -> None:
    """TEST 25."""
    oos_rows = [_oos_row(1, "AAA", d, 1.0) for d in _HL_DAYS]
    folds = [_single_fold("2021-01-01", "2021-01-06")]
    bars_path = _write_direct_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=_hl_bars_rows())
    out_path = run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=_official_spec())
    out = json.loads(out_path.read_text(encoding="utf-8"))
    assert out["holdout"]["status"] == "reserved_not_evaluated"
    assert out["execution_pricing"]["pricing_model_id"] == EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1


# ---------------------------------------------------------------------------
# require_bars_pricing_provenance content-binding (mirrors require_bars_
# match_manifest's own contract, for high/low)
# ---------------------------------------------------------------------------


def test_require_bars_pricing_provenance_fails_closed_on_missing_hash() -> None:
    bars = pd.DataFrame([_bar_row_hl("AAA", "2021-01-01", 100.0, 101.0, 99.0)])
    manifest: Dict[str, Any] = {"canonical_pricing_bars_hash": None}
    with pytest.raises(BarsProvenanceUnverifiable, match="canonical_pricing_bars_hash"):
        require_bars_pricing_provenance(bars, manifest)


def test_require_bars_pricing_provenance_fails_closed_on_mismatch() -> None:
    bars = pd.DataFrame([_bar_row_hl("AAA", "2021-01-01", 100.0, 101.0, 99.0)])
    manifest = {"canonical_pricing_bars_hash": "not-the-real-hash"}
    with pytest.raises(BarsProvenanceContentMismatch):
        require_bars_pricing_provenance(bars, manifest)


def test_require_bars_pricing_provenance_accepts_matching_content() -> None:
    bars = pd.DataFrame([_bar_row_hl("AAA", "2021-01-01", 100.0, 101.0, 99.0)])
    manifest = {"canonical_pricing_bars_hash": canonical_pricing_bars_hash(bars)}
    require_bars_pricing_provenance(bars, manifest)  # must not raise


def test_build_bars_provenance_manifest_omits_pricing_hash_without_high_low() -> None:
    bars = pd.DataFrame([{"symbol": "AAA", "end_ts": _ts("2021-01-01").isoformat(), "close": 100.0}])
    manifest = _ca_manifest(bars)
    assert manifest["canonical_pricing_bars_hash"] is None


# ---------------------------------------------------------------------------
# TEST 27 — legacy diagnostic close-only mode cannot satisfy the official
# registered parity gate
# ---------------------------------------------------------------------------


def test_diagnostic_default_fails_official_parity_gate() -> None:
    with pytest.raises(RuntimeError, match="official registered execution-pricing parity"):
        require_official_execution_pricing_parity(_spec())


def test_official_model_satisfies_official_parity_gate() -> None:
    require_official_execution_pricing_parity(_official_spec())  # must not raise


# ---------------------------------------------------------------------------
# RESEARCH-EXECUTION-PRICING-PARITY-01-REPAIR-01 — commission basis parity.
#
# P7A correctly reconciled the ADVERSE-PRICE (fill vs close) component but
# left commission_bps_per_side charged against close-priced turnover
# (economics.compute_transaction_cost(turnover, one_way_cost_bps)), whereas
# Rust's ordinary execution charges commission.compute_fee(qty, fill_price)
# -- i.e. against the SAME conservative fill notional used for the adverse-
# price drag. REPAIR-01 rescales commission by each execution's own
# fill/close notional ratio (_row_execution_pricing_components) under the
# official model only; the diagnostic model and the forced fold-end
# flatten exception are both unaffected (see their own tests below).
# ---------------------------------------------------------------------------


def test_buy_commission_reflects_actual_fill_notional_not_close_turnover(tmp_path: Path) -> None:
    """BUY ordinary fill: close=100, fill=high=102 (zero slippage/vol),
    turnover=1, commission_bps=10. Pre-repair (turnover * bps/10000) would
    charge 1.0 * 10/10000 = 0.001. Post-repair, commission must reflect the
    102/100 fill/close notional scaling: 1.0 * (102/100) * 10/10000 =
    0.00102 -- a value the currently-pushed P7A implementation cannot
    produce (it has no commission_cost column and its transaction_cost
    numerically equals the unrescaled 0.001 + 0.02 = 0.021, not 0.02102)."""
    returns = _run_hl_fixture(tmp_path, _official_spec())
    day1 = returns.iloc[1]
    assert day1["turnover"] == pytest.approx(1.0)
    assert day1["execution_price_cost"] == pytest.approx(0.02, abs=1e-9)
    expected_commission = 1.0 * (102.0 / 100.0) * (10.0 / 10_000.0)
    assert day1["commission_cost"] == pytest.approx(expected_commission, abs=1e-12)
    # RED under pre-repair P7A: transaction_cost there is 0.001 + 0.02 =
    # 0.021, not this repaired 0.02102 (proves the repair, not a tautology).
    assert day1["transaction_cost"] == pytest.approx(0.02102, abs=1e-9)
    unrescaled_commission = 1.0 * (10.0 / 10_000.0)
    assert day1["commission_cost"] != pytest.approx(unrescaled_commission, abs=1e-12)


def test_sell_commission_reflects_actual_fill_notional_not_close_turnover(tmp_path: Path) -> None:
    """SELL ordinary fill (an actual mid-fold liquidation, NOT the fold-end
    forced flatten): AAA goes active on day0 (executes BUY on day1 @
    high=102, close=100), then goes flat on day2's decision (executes SELL
    on day3 @ low=108, close=110). By day4 (fold end) the position is
    already flat, so this isolates the ordinary-SELL commission basis from
    the forced-flatten exception entirely. commission must scale by
    108/110, not close-priced turnover."""
    days = pd.date_range("2021-01-01", periods=5, freq="D", tz="UTC")
    closes = [100.0, 100.0, 110.0, 110.0, 110.0]
    scores = [1.0, 1.0, 0.0, 0.0, 0.0]
    bars_rows = [_bar_row_hl("AAA", d, c, c + 2.0, c - 2.0) for d, c in zip(days, closes)]
    oos_rows = [_oos_row(1, "AAA", d, s) for d, s in zip(days, scores)]
    folds = [_single_fold("2021-01-01", "2021-01-06")]
    bars_path = _write_direct_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)
    run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=_official_spec())
    returns = pd.read_csv(tmp_path / "eval" / "economic_returns.csv")

    day3 = returns.iloc[3]
    assert day3["turnover"] == pytest.approx(1.0)
    expected_adverse = 1.0 * abs(108.0 - 110.0) / 110.0
    assert day3["execution_price_cost"] == pytest.approx(expected_adverse, abs=1e-9)
    expected_commission = 1.0 * (108.0 / 110.0) * (10.0 / 10_000.0)
    assert day3["commission_cost"] == pytest.approx(expected_commission, abs=1e-12)
    # RED under pre-repair P7A: transaction_cost there is
    # 1.0*10/10000 + 0.018181... = 0.019181818..., not this repaired value.
    assert day3["transaction_cost"] == pytest.approx(expected_commission + expected_adverse, abs=1e-12)
    unrescaled_commission = 1.0 * (10.0 / 10_000.0)
    assert day3["commission_cost"] != pytest.approx(unrescaled_commission, abs=1e-12)
    # Day4 (fold end) is already flat -- no forced-flatten turnover at all,
    # confirming the SELL fully closed the position on day3.
    assert returns.iloc[4]["turnover"] == pytest.approx(0.0, abs=1e-12)


def test_official_transaction_cost_exactly_composes_commission_and_adverse_price(
    tmp_path: Path,
) -> None:
    """Official transaction_cost must exactly compose from the two
    auditable components: actual-fill commission_cost + adverse
    execution_price_cost, on EVERY row (not just the ones with turnover)."""
    returns = _run_hl_fixture(tmp_path, _official_spec())
    composed = returns["commission_cost"] + returns["execution_price_cost"]
    pd.testing.assert_series_equal(
        returns["transaction_cost"], composed, check_names=False, check_exact=True
    )


def test_forced_fold_end_flatten_commission_remains_close_priced(tmp_path: Path) -> None:
    """Forced fold-end flatten (day4 of the BUY fixture, force_flat_last_bar)
    must still have execution_price_cost == 0 (P7A, unchanged) AND its
    commission_cost must remain close/mark-priced -- i.e. an UNRESCALED
    turnover * commission_bps/10000, even though the official pricing model
    is active and this is the same wide (+/-2) bar that WOULD trigger
    conservative-fill rescaling for an ordinary execution."""
    returns = _run_hl_fixture(tmp_path, _official_spec())
    last_row = returns.iloc[-1]
    assert last_row["turnover"] > 0.0
    assert last_row["execution_price_cost"] == pytest.approx(0.0, abs=1e-12)
    expected_close_priced_commission = float(last_row["turnover"]) * (10.0 / 10_000.0)
    assert last_row["commission_cost"] == pytest.approx(expected_close_priced_commission, abs=1e-12)


def test_diagnostic_pricing_transaction_cost_numerically_unchanged_by_repair(
    tmp_path: Path,
) -> None:
    """Diagnostic close_only_diagnostic_v1 behavior must remain
    byte/numerically unchanged by REPAIR-01: transaction_cost must equal
    EXACTLY turnover * one_way_cost_bps / 10_000 (the original pre-repair,
    pre-P7A formula), not the new commission/adverse-price decomposition
    (which is scoped to the official model only)."""
    returns = _run_hl_fixture(tmp_path, _spec(slippage_bps_per_side=5.0))
    one_way_cost_bps = 10.0 + 5.0  # commission_bps_per_side + slippage_bps_per_side
    expected = returns["turnover"] * (one_way_cost_bps / 10_000.0)
    pd.testing.assert_series_equal(
        returns["transaction_cost"], expected, check_names=False, check_exact=True
    )
    assert (returns["execution_price_cost"] == 0.0).all()
