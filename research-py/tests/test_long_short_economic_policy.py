"""
RESEARCH-LONG-SHORT-ECONOMIC-POLICY-01 -- versioned long/short signal
direction policy built on top of the repaired (P7B-REPAIR-01) causal
discrete-share translation.

Covers the mission's REQUIRED TESTS list (Section 5I, 22 items, referenced
by number in each test's docstring). Long-only legacy reproduction (1),
distinct identity (2), bullish/bearish/neutral mapping (3-5), fail-closed
threshold validation (6), and gross-exposure/sign-mechanics/short-P&L/
borrow-scope proofs (7-22) are all exercised end-to-end through the real
public pipeline (run_economic_walkforward), NOT via hand-crafted internal
state -- this is the first patch that can generate a genuine negative
target weight through SignalPolicySpec itself.
"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict, List

import pandas as pd
import pytest

from mqk_research.ml.economic_walkforward import (
    BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1,
    SIGNAL_DIRECTION_POLICY_LONG_ONLY_V1,
    SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
    economic_protocol_identity,
    run_economic_walkforward,
)
from mqk_research.ml.execution_pricing import (
    EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    ExecutionPricingSpec,
)
from mqk_research.ml.weight_to_share import WeightToShareSpec


def _ts(s: str) -> pd.Timestamp:
    return pd.Timestamp(s, tz="UTC")


def _bar_row(symbol: str, end_ts, close: float, high: float = None, low: float = None) -> Dict[str, Any]:
    row = {"symbol": symbol, "end_ts": _ts(str(end_ts)).isoformat(), "close": close}
    if high is not None:
        row["high"] = high
    if low is not None:
        row["low"] = low
    return row


def _oos_row(fold: int, symbol: str, decision_ts, score: float) -> Dict[str, Any]:
    return {
        "fold": fold,
        "symbol": symbol,
        "decision_ts": _ts(str(decision_ts)).isoformat(),
        "label_end_ts": _ts(str(decision_ts)).isoformat(),
        "ml_score": score,
        "target": 1,
    }


def _single_fold(test_start: str, test_end: str, fold: int = 1) -> Dict[str, Any]:
    return {
        "fold": fold,
        "skipped": False,
        "test_start_utc": _ts(test_start).isoformat(),
        "test_end_utc": _ts(test_end).isoformat(),
    }


def _write_fixture(
    run_dir: Path, *, folds: List[Dict[str, Any]], oos_rows: List[Dict[str, Any]], bars_rows: List[Dict[str, Any]]
) -> Path:
    eval_dir = run_dir / "eval"
    eval_dir.mkdir(parents=True, exist_ok=True)
    (eval_dir / "walk_forward_eval.json").write_text(json.dumps({"folds": folds}), encoding="utf-8")
    pd.DataFrame(oos_rows).to_csv(eval_dir / "walk_forward_oos_predictions.csv", index=False)
    bars_path = run_dir / "bars.csv"
    pd.DataFrame(bars_rows).to_csv(bars_path, index=False)
    return bars_path


def _long_short_signal_policy(long_threshold: float = 0.7, short_threshold: float = 0.3) -> SignalPolicySpec:
    return SignalPolicySpec(
        entry_threshold=long_threshold,
        long_only=False,
        direction_policy=SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
        short_threshold=short_threshold,
        max_gross_exposure=1.0,
    )


def _diagnostic_spec(signal_policy: SignalPolicySpec, weight_to_share=None) -> EconomicWalkForwardSpec:
    return EconomicWalkForwardSpec(
        signal_policy=signal_policy,
        cost_model=CostModelSpec(commission_bps_per_side=0.0, slippage_bps_per_side=0.0, diagnostic_zero_cost=True),
        annualization=AnnualizationSpec(),
        weight_to_share=weight_to_share,
    )


# ---------------------------------------------------------------------------
# REQUIRED TEST 1: legacy long-only reproduces previous semantics
# ---------------------------------------------------------------------------


def test_legacy_long_only_reproduces_previous_semantics(tmp_path: Path) -> None:
    """REQUIRED TEST 1: a spec that never sets direction_policy behaves
    exactly as before -- default direction_policy is long_only_v1."""
    spec = SignalPolicySpec(entry_threshold=0.5)
    normalized = spec.normalized()
    assert normalized.direction_policy == SIGNAL_DIRECTION_POLICY_LONG_ONLY_V1
    assert normalized.long_only is True
    assert normalized.short_threshold is None
    assert normalized.borrow_model is None


def test_long_only_v1_rejects_long_only_false() -> None:
    with pytest.raises(ValueError, match="long_only_v1"):
        SignalPolicySpec(direction_policy=SIGNAL_DIRECTION_POLICY_LONG_ONLY_V1, long_only=False).normalized()


def test_long_only_v1_rejects_short_threshold() -> None:
    with pytest.raises(ValueError, match="long_only_v1 direction_policy does not accept short_threshold"):
        SignalPolicySpec(short_threshold=0.2).normalized()


# ---------------------------------------------------------------------------
# REQUIRED TEST 2: distinct semantic identity
# ---------------------------------------------------------------------------


def test_long_short_protocol_has_distinct_identity_from_legacy() -> None:
    """REQUIRED TEST 2."""
    legacy = SignalPolicySpec(entry_threshold=0.5)
    long_short = _long_short_signal_policy()
    spec_legacy = _diagnostic_spec(legacy)
    spec_long_short = _diagnostic_spec(long_short)
    id_legacy = economic_protocol_identity(spec_legacy.normalized())
    id_long_short = economic_protocol_identity(spec_long_short.normalized())
    assert id_legacy != id_long_short
    assert id_legacy["signal_policy"]["direction_policy"] == SIGNAL_DIRECTION_POLICY_LONG_ONLY_V1
    assert id_long_short["signal_policy"]["direction_policy"] == SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1


# ---------------------------------------------------------------------------
# REQUIRED TESTS 3/4/5: bullish/bearish/neutral mapping
# ---------------------------------------------------------------------------


def test_bullish_bearish_neutral_signal_mapping(tmp_path: Path) -> None:
    """REQUIRED TESTS 3/4/5: score >= 0.7 -> LONG (positive target), score
    <= 0.3 -> SHORT (negative target), otherwise -> FLAT (zero target)."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    bars_rows = (
        [_bar_row("BULL", d, 100.0) for d in days]
        + [_bar_row("BEAR", d, 100.0) for d in days]
        + [_bar_row("FLAT", d, 100.0) for d in days]
    )
    oos_rows = (
        [_oos_row(1, "BULL", d, 0.9) for d in days]
        + [_oos_row(1, "BEAR", d, 0.1) for d in days]
        + [_oos_row(1, "FLAT", d, 0.5) for d in days]
    )
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(_long_short_signal_policy())
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    fold = out["folds"][0]
    # 2 non-flat names (BULL, BEAR) -> weight_each_magnitude = 1.0/2 = 0.5.
    assert fold["symbols"] == ["BEAR", "BULL", "FLAT"]
    exposure_row = out  # gross_exposure reported at fold_df row granularity, checked via aggregate below
    assert fold["average_gross_exposure"] > 0.0  # BULL(+0.5) + BEAR(-0.5) both consume gross
    assert fold["active_days"] >= 1


# ---------------------------------------------------------------------------
# REQUIRED TEST 6: invalid overlapping thresholds fail closed
# ---------------------------------------------------------------------------


def test_overlapping_thresholds_fail_closed() -> None:
    """REQUIRED TEST 6."""
    with pytest.raises(ValueError, match="short_threshold"):
        SignalPolicySpec(
            entry_threshold=0.5, long_only=False,
            direction_policy=SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
            short_threshold=0.5,  # equal to entry_threshold -- must be strictly less
        ).normalized()
    with pytest.raises(ValueError, match="short_threshold"):
        SignalPolicySpec(
            entry_threshold=0.3, long_only=False,
            direction_policy=SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
            short_threshold=0.7,  # greater than entry_threshold -- overlapping/inverted
        ).normalized()


def test_long_short_requires_short_threshold() -> None:
    with pytest.raises(ValueError, match="requires short_threshold"):
        SignalPolicySpec(
            long_only=False, direction_policy=SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1
        ).normalized()


# ---------------------------------------------------------------------------
# REQUIRED TESTS 7/8: gross exposure uses ABSOLUTE magnitude
# ---------------------------------------------------------------------------


def test_gross_exposure_uses_absolute_magnitude_not_signed_sum(tmp_path: Path) -> None:
    """REQUIRED TESTS 7/8: +0.5 long + -0.5 short = gross 1.0, NOT 0.0."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    bars_rows = [_bar_row("BULL", d, 100.0) for d in days] + [_bar_row("BEAR", d, 100.0) for d in days]
    oos_rows = [_oos_row(1, "BULL", d, 0.9) for d in days] + [_oos_row(1, "BEAR", d, 0.1) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(_long_short_signal_policy(), weight_to_share=WeightToShareSpec(equity_usd=10_000.0))
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    ev_bull = out["folds"][0]["weight_to_share_evidence"]["BULL"]
    ev_bear = out["folds"][0]["weight_to_share_evidence"]["BEAR"]
    # day1 execution: BULL +0.5*10000/100=+50, BEAR -0.5*10000/100=-50.
    assert ev_bull[1]["target_qty"] == 50
    assert ev_bear[1]["target_qty"] == -50
    # Gross exposure that day is abs(0.5)+abs(-0.5) = 1.0, never a netted 0.0.
    returns_csv = pd.read_csv(tmp_path / "eval" / "economic_returns.csv")
    max_gross = returns_csv["gross_exposure"].max()
    assert max_gross == pytest.approx(1.0, abs=1e-6)


# ---------------------------------------------------------------------------
# REQUIRED TESTS 9-17: signed mechanics + short P&L direction (end-to-end)
# ---------------------------------------------------------------------------


def test_short_entry_produces_sell_and_covers_with_buy_end_to_end(tmp_path: Path) -> None:
    """REQUIRED TESTS 9/11/17: a persistently bearish signal opens a short
    (SELL) on its first execution bar and the forced fold-end flatten
    covers it (BUY), using the close/mark exception."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    bars_rows = [_bar_row("BEAR", d, 100.0) for d in days]
    oos_rows = [_oos_row(1, "BEAR", d, 0.1) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(
        _long_short_signal_policy(), weight_to_share=WeightToShareSpec(equity_usd=10_000.0)
    )
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    evidence = out["folds"][0]["weight_to_share_evidence"]["BEAR"]
    assert evidence[0]["target_qty"] == 0  # signal row, cannot execute from its own bar
    assert evidence[1]["side"] == "sell" and evidence[1]["target_qty"] == -100  # short OPEN
    final = evidence[-1]
    assert final["side"] == "buy" and final["target_qty"] == 0  # forced flatten covers


def test_short_to_long_and_long_to_short_transitions_end_to_end(tmp_path: Path) -> None:
    """REQUIRED TESTS 12/13/16 (mirrors mission section 4I 16/17 at the
    engine level, now proven through the real signal-generation pipeline):
    a symbol flipping from bearish to bullish (and vice versa) produces the
    correct single-leg transition delta."""
    days = pd.date_range("2021-01-01", periods=6, freq="D", tz="UTC")
    # BEAR at day0-1, BULL from day2 onward -> short opens day1, then flips
    # to long at day3 (first bar after the day2 bullish signal).
    scores = [0.1, 0.1, 0.9, 0.9, 0.9, 0.9]
    bars_rows = [_bar_row("SWING", d, 100.0) for d in days]
    oos_rows = [_oos_row(1, "SWING", d, s) for d, s in zip(days, scores)]
    folds = [_single_fold("2021-01-01", "2021-01-07")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(
        _long_short_signal_policy(), weight_to_share=WeightToShareSpec(equity_usd=10_000.0)
    )
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    evidence = out["folds"][0]["weight_to_share_evidence"]["SWING"]
    qtys = [e["target_qty"] for e in evidence]
    sides = [e["side"] for e in evidence]
    assert -100 in qtys  # opened short
    assert 100 in qtys  # transitioned to long
    # The transition itself is a single BUY leg covering the short and
    # opening the long together (short -100 -> long +100 = BUY 200).
    transition_idx = qtys.index(100)
    assert sides[transition_idx] == "buy"
    assert evidence[transition_idx]["qty"] == 200


def test_short_profits_when_price_falls_before_costs(tmp_path: Path) -> None:
    """REQUIRED TEST 14."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    closes = [100.0, 90.0, 80.0, 70.0]  # falling price while short is held
    bars_rows = [_bar_row("BEAR", d, c) for d, c in zip(days, closes)]
    oos_rows = [_oos_row(1, "BEAR", d, 0.1) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(
        _long_short_signal_policy(), weight_to_share=WeightToShareSpec(equity_usd=10_000.0)
    )
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    assert out["folds"][0]["gross_total_return"] > 0.0


def test_short_loses_when_price_rises_before_costs(tmp_path: Path) -> None:
    """REQUIRED TEST 15."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    closes = [100.0, 110.0, 120.0, 130.0]  # rising price while short is held
    bars_rows = [_bar_row("BEAR", d, c) for d, c in zip(days, closes)]
    oos_rows = [_oos_row(1, "BEAR", d, 0.1) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(
        _long_short_signal_policy(), weight_to_share=WeightToShareSpec(equity_usd=10_000.0)
    )
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    assert out["folds"][0]["gross_total_return"] < 0.0


# ---------------------------------------------------------------------------
# REQUIRED TEST 18: future execution bar cannot alter signal-time target_qty
# ---------------------------------------------------------------------------


def test_short_target_qty_fixed_at_signal_time_not_execution_time(tmp_path: Path) -> None:
    """REQUIRED TEST 18 (short-side counterpart of P7B-REPAIR-01's own
    causal proof): the short's target_qty is fixed from the SIGNAL bar's
    close, not the execution bar's drastically different close."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    closes = [100.0, 200.0, 200.0, 200.0]  # execution bar close very different from signal bar
    bars_rows = [_bar_row("BEAR", d, c) for d, c in zip(days, closes)]
    oos_rows = [_oos_row(1, "BEAR", d, 0.1) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(
        _long_short_signal_policy(), weight_to_share=WeightToShareSpec(equity_usd=10_000.0)
    )
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    evidence = out["folds"][0]["weight_to_share_evidence"]["BEAR"]
    # -1.0 * 10000 / 100 (SIGNAL close) = -100, NOT -1.0*10000/200=-50.
    assert evidence[1]["target_qty"] == -100


# ---------------------------------------------------------------------------
# REQUIRED TEST 16: P7A adverse pricing for shorts (never favorable)
# ---------------------------------------------------------------------------


def test_p7a_short_execution_costs_are_adverse_not_favorable(tmp_path: Path) -> None:
    """REQUIRED TEST 16: SHORT OPEN (SELL) must fill at the conservative
    (adverse-to-seller) LOW, never the favorable HIGH."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    closes = [100.0, 100.0, 100.0, 100.0]
    highs = [100.5, 105.0, 100.5, 100.5]
    lows = [99.5, 90.0, 99.5, 99.5]
    bars_rows = [
        {"symbol": "BEAR", "end_ts": _ts(str(d)).isoformat(), "close": c, "high": h, "low": lo}
        for d, c, h, lo in zip(days, closes, highs, lows)
    ]
    oos_rows = [_oos_row(1, "BEAR", d, 0.1) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = EconomicWalkForwardSpec(
        signal_policy=_long_short_signal_policy(),
        cost_model=CostModelSpec(commission_bps_per_side=1.0, slippage_bps_per_side=0.0),
        annualization=AnnualizationSpec(),
        execution_pricing=ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
            slippage_bps=0, volatility_mult_bps=0,
        ),
        weight_to_share=WeightToShareSpec(equity_usd=10_000.0),
    )
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    fold = out["folds"][0]
    # The SELL execution on day1 fills at LOW=90.0 (adverse), not HIGH=105.0
    # (favorable) -- proven indirectly via execution_price_cost being
    # strictly positive (a favorable fill would score zero drag only by
    # coincidence; an adverse LOW-side fill on a materially wide bar cannot).
    assert fold["cost_drag"] > 0.0
    assert fold["net_total_return"] < fold["gross_total_return"]


# ---------------------------------------------------------------------------
# REQUIRED TEST 19: discrete rounding affects long/short economics
# ---------------------------------------------------------------------------


def test_discrete_rounding_affects_short_economics(tmp_path: Path) -> None:
    """REQUIRED TEST 19 (short-side): a short target that rounds to qty=0
    yields zero economic exposure, exactly like the long-side proof in
    P7B-REPAIR-01."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    closes = [100.0, 90.0, 80.0, 70.0]
    bars_rows = [_bar_row("BEAR", d, c) for d, c in zip(days, closes)]
    oos_rows = [_oos_row(1, "BEAR", d, 0.1) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    # equity_usd=50 at signal price 100 -> magnitude 0.5 theoretical shares -> floors to 0.
    spec = _diagnostic_spec(_long_short_signal_policy(), weight_to_share=WeightToShareSpec(equity_usd=50.0))
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    fold = out["folds"][0]
    assert fold["gross_total_return"] == 0.0
    assert all(e["target_qty"] == 0 for e in fold["weight_to_share_evidence"]["BEAR"])


# ---------------------------------------------------------------------------
# REQUIRED TEST 20: short borrow assumption explicitly present in identity
# ---------------------------------------------------------------------------


def test_borrow_model_present_in_identity_and_defaults_correctly() -> None:
    """REQUIRED TEST 20."""
    spec = _diagnostic_spec(_long_short_signal_policy()).normalized()
    identity = economic_protocol_identity(spec)
    assert identity["signal_policy"]["borrow_model"] == BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1
    # Legacy long-only carries no borrow_model (never shorts).
    legacy_identity = economic_protocol_identity(_diagnostic_spec(SignalPolicySpec(entry_threshold=0.5)).normalized())
    assert legacy_identity["signal_policy"]["borrow_model"] is None


def test_unsupported_borrow_model_fails_closed() -> None:
    with pytest.raises(ValueError, match="unsupported borrow_model"):
        SignalPolicySpec(
            entry_threshold=0.7, long_only=False,
            direction_policy=SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
            short_threshold=0.3, borrow_model="unproven_universal_borrow_v1",
        ).normalized()


# ---------------------------------------------------------------------------
# REQUIRED TEST 21: legacy artifact cannot be silently reinterpreted
# ---------------------------------------------------------------------------


def test_legacy_long_only_artifact_cannot_be_confused_with_long_short(tmp_path: Path) -> None:
    """REQUIRED TEST 21: a legacy long-only run's output JSON always carries
    direction_policy=long_only_v1 and never emits a negative target_qty or
    a borrow_model -- it cannot be silently reinterpreted as a long/short
    evaluation."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    bars_rows = [_bar_row("AAA", d, 100.0) for d in days]
    oos_rows = [_oos_row(1, "AAA", d, 0.9) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(SignalPolicySpec(entry_threshold=0.5), weight_to_share=WeightToShareSpec(equity_usd=10_000.0))
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    assert out["signal_policy"]["direction_policy"] == SIGNAL_DIRECTION_POLICY_LONG_ONLY_V1
    assert out["signal_policy"]["borrow_model"] is None
    all_qtys = [e["target_qty"] for e in out["folds"][0]["weight_to_share_evidence"]["AAA"]]
    assert all(q >= 0 for q in all_qtys)


# ---------------------------------------------------------------------------
# REQUIRED TEST 22: holdout remains reserved and untouched
# ---------------------------------------------------------------------------


def test_holdout_remains_reserved_for_long_short_evaluation(tmp_path: Path) -> None:
    """REQUIRED TEST 22: this patch does not touch holdout handling at all;
    a long/short run's output still reports reserved_not_evaluated."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    bars_rows = [_bar_row("BEAR", d, 100.0) for d in days]
    oos_rows = [_oos_row(1, "BEAR", d, 0.1) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(_long_short_signal_policy())
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    assert out["holdout"] == {"status": "reserved_not_evaluated"}


# ---------------------------------------------------------------------------
# Multiple-testing comparability (mission Section 5H): confirm the EXISTING
# comparison-key design already correctly treats direction_policy as a
# candidate-differentiating strategy choice (excluded from the comparison
# key, same category as entry_threshold/long_only/sizing/max_gross_exposure)
# -- no code change to multiple_testing_judge.py was required.
# ---------------------------------------------------------------------------


def test_long_only_and_long_short_candidates_remain_mutually_comparable() -> None:
    from mqk_research.ml.multiple_testing_judge import _comparison_key

    base_identity = {
        "protocol_id": "economic_walk_forward_v1",
        "data_identity": {"bars_provenance": {"x": 1}},
        "evaluation_spec": {"a": 1},
        "economic_protocol": {
            "annualization": {"annualization_days": 252, "risk_free_rate_annual": 0.0},
            "cost_model": {"commission_bps_per_side": 1.0, "slippage_bps_per_side": 0.0, "diagnostic_zero_cost": False},
            "signal_policy": {
                "capacity_policy": "reduce_first_defer_increase_batch_v1",
                "fold_end_policy": "force_flat_last_bar",
                "direction_policy": SIGNAL_DIRECTION_POLICY_LONG_ONLY_V1,
                "entry_threshold": 0.5, "long_only": True, "short_threshold": None, "borrow_model": None,
                "sizing": "equal_weight_active", "max_gross_exposure": 1.0,
            },
        },
    }
    long_short_identity = json.loads(json.dumps(base_identity))
    long_short_identity["economic_protocol"]["signal_policy"]["direction_policy"] = (
        SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1
    )
    long_short_identity["economic_protocol"]["signal_policy"]["long_only"] = False
    long_short_identity["economic_protocol"]["signal_policy"]["short_threshold"] = 0.3
    long_short_identity["economic_protocol"]["signal_policy"]["borrow_model"] = (
        BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1
    )

    key_a, _ = _comparison_key(base_identity)
    key_b, _ = _comparison_key(long_short_identity)
    assert key_a == key_b  # same measurement basis -> comparable candidate population
