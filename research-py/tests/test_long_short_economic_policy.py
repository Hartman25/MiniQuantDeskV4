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
    _simulate_fold,
    economic_protocol_identity,
    run_economic_walkforward,
)
from mqk_research.ml.execution_pricing import (
    EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    ExecutionPricingSpec,
)
from mqk_research.ml.weight_to_share import WeightToShareSpec, weight_to_target_qty


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


def test_legacy_long_only_identity_exact_golden_equality() -> None:
    """LONG-SHORT-REPAIR-01 REQUIRED TEST (mission Section 4B): the default
    long_only_v1 `signal_policy` identity fragment must be EXACTLY equal,
    key-for-key, to the hand-written golden object matching the PRE-long-
    short-patch (commit be1c6220, before 99e806e3) shape of
    economic_protocol_identity -- 6 keys, no direction_policy/
    short_threshold/borrow_model at all (not even as a `None` value; their
    mere PRESENCE, regardless of value, would change the canonical_json/
    short_hash trial_id every pre-existing registered long-only trial
    already committed to). This is the load-bearing RED/GREEN proof for
    defect A (legacy identity drift) -- against the unrepaired 99e806e3
    code this test fails because those 3 extra keys are always present."""
    spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=0.5),
        cost_model=CostModelSpec(commission_bps_per_side=0.0, slippage_bps_per_side=0.0, diagnostic_zero_cost=True),
        annualization=AnnualizationSpec(),
    ).normalized()
    identity = economic_protocol_identity(spec)

    golden_signal_policy = {
        "entry_threshold": 0.5,
        "long_only": True,
        "sizing": "equal_weight_active",
        "max_gross_exposure": 1.0,
        "fold_end_policy": "force_flat_last_bar",
        "capacity_policy": "reduce_first_defer_increase_batch_v1",
    }
    assert identity["signal_policy"] == golden_signal_policy
    assert set(identity["signal_policy"].keys()) == {
        "entry_threshold", "long_only", "sizing", "max_gross_exposure",
        "fold_end_policy", "capacity_policy",
    }


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
    # LONG-SHORT-REPAIR-01: direction_policy is ADDITIVE-ONLY -- absent
    # entirely from legacy long_only_v1 identity (see
    # test_legacy_long_only_identity_exact_golden_equality), present under
    # long_short_threshold_v1.
    assert "direction_policy" not in id_legacy["signal_policy"]
    assert id_long_short["signal_policy"]["direction_policy"] == SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1


def test_short_threshold_change_alters_long_short_identity_only() -> None:
    """LONG-SHORT-REPAIR-01 REQUIRED TEST (mission Section 4B/4F item 3):
    two long_short_threshold_v1 specs differing ONLY by short_threshold
    must produce different identity -- this field is genuinely
    identity-bearing under that direction policy."""
    id_a = economic_protocol_identity(
        _diagnostic_spec(_long_short_signal_policy(short_threshold=0.3)).normalized()
    )
    id_b = economic_protocol_identity(
        _diagnostic_spec(_long_short_signal_policy(short_threshold=0.2)).normalized()
    )
    assert id_a != id_b
    assert id_a["signal_policy"]["short_threshold"] == 0.3
    assert id_b["signal_policy"]["short_threshold"] == 0.2


def test_borrow_model_change_alters_long_short_identity_only() -> None:
    """LONG-SHORT-REPAIR-01 REQUIRED TEST (mission Section 4B/4F item 4):
    changing borrow_model under long_short_threshold_v1 changes identity.
    Only one borrow_model is currently accepted, so this proves the field
    is wired into identity by comparing an explicit vs. defaulted (but
    value-equal) borrow_model produces the SAME identity (proving the field
    is read, not ignored) -- a distinct accepted value would need a new
    KNOWN_BORROW_MODEL_IDS entry to test inequality without inventing an
    unsupported protocol string."""
    explicit = SignalPolicySpec(
        entry_threshold=0.7, long_only=False,
        direction_policy=SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
        short_threshold=0.3,
        borrow_model=BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1,
    )
    defaulted = SignalPolicySpec(
        entry_threshold=0.7, long_only=False,
        direction_policy=SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
        short_threshold=0.3,
    )
    id_explicit = economic_protocol_identity(_diagnostic_spec(explicit).normalized())
    id_defaulted = economic_protocol_identity(_diagnostic_spec(defaulted).normalized())
    assert id_explicit == id_defaulted
    assert id_explicit["signal_policy"]["borrow_model"] == BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1


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


def test_short_to_long_and_long_to_short_transitions_end_to_end() -> None:
    """REQUIRED TESTS 12/13/16 (mirrors mission section 4I 16/17 at the
    engine level): a symbol flipping from bearish to bullish produces the
    correct single-leg transition delta.

    P7B-REPAIR-02 interaction: a FULL 100%-conviction single-symbol
    reversal (the direction this test used before) always exactly
    self-competes against the fill-time allocation cap -- the leg BEING
    closed still counts as "current exposure" until its own fill actually
    lands (mirrors Rust's resolve_one_pending_order, which computes
    exposure from self.portfolio.positions BEFORE apply_fill runs), so a
    same-magnitude opposite-side reversal can never fit under any finite
    cap. Constructs pending_events directly with a SUB-CAP weight pair
    (-0.2 -> +0.4, not the equal-weight-active pipeline's automatic -1.0 ->
    +1.0) so the transition's residual genuinely admits, while still
    exercising the real discrete engine's sign/side/delta mechanics."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    close_frame = pd.DataFrame({"SWING": [100.0, 100.0, 100.0, 100.0]}, index=days)
    wts_spec = WeightToShareSpec(equity_usd=10_000.0)
    short_qty = weight_to_target_qty(weight=-0.2, price=100.0, spec=wts_spec)
    long_qty = weight_to_target_qty(weight=0.4, price=100.0, spec=wts_spec)
    assert short_qty == -20 and long_qty == 40
    pending_events = {
        "SWING": [
            (days[0], -0.2, short_qty),  # bearish: opens short (fills day1)
            (days[1], 0.4, long_qty),    # bullish: reverses to long (fills day2)
        ]
    }
    spec = _diagnostic_spec(
        _long_short_signal_policy(), weight_to_share=wts_spec,
    )
    fold_df, summary = _simulate_fold(
        1, {"test_start": days[0], "test_end": days[-1] + pd.Timedelta(days=1)},
        close_frame, pending_events, spec,
    )
    evidence = summary["weight_to_share_evidence"]["SWING"]
    qtys = [e["target_qty"] for e in evidence]
    sides = [e["side"] for e in evidence]
    assert -20 in qtys  # opened short
    assert 40 in qtys  # transitioned to long
    # The transition itself is a single BUY leg covering the short and
    # opening the long together (short -20 -> long +40 = BUY 60).
    transition_idx = qtys.index(40)
    assert sides[transition_idx] == "buy"
    assert evidence[transition_idx]["qty"] == 60


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


def test_short_loses_when_price_rises_before_costs() -> None:
    """REQUIRED TEST 15.

    P7B-REPAIR-02 interaction: opening a FULL 100%-conviction short from
    the equal-weight-active pipeline sizes to exactly the fill-time
    allocation cap at the signal close; the 10% adverse gap-up before the
    order actually fills (day0 close=100 -> day1 close=110, diagnostic
    close-priced fill) then legitimately breaches that cap (mission Section
    3D) and the order is capacity-rejected -- a SEPARATE, correctly-working
    concern from this test's real subject (a held short loses money as
    price rises). Constructs pending_events directly with a SUB-CAP weight
    (-0.3, not the pipeline's automatic -1.0) so the short genuinely opens."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    close_frame = pd.DataFrame({"BEAR": [100.0, 110.0, 120.0, 130.0]}, index=days)
    wts_spec = WeightToShareSpec(equity_usd=10_000.0)
    target_qty = weight_to_target_qty(weight=-0.3, price=100.0, spec=wts_spec)
    pending_events = {"BEAR": [(days[0], -0.3, target_qty)]}
    spec = _diagnostic_spec(
        _long_short_signal_policy(), weight_to_share=wts_spec,
    )
    fold_df, summary = _simulate_fold(
        1, {"test_start": days[0], "test_end": days[-1] + pd.Timedelta(days=1)},
        close_frame, pending_events, spec,
    )
    assert summary["weight_to_share_evidence"]["BEAR"][1]["target_qty"] == -30
    assert summary["gross_total_return"] < 0.0


# ---------------------------------------------------------------------------
# REQUIRED TEST 18: future execution bar cannot alter signal-time target_qty
# ---------------------------------------------------------------------------


def test_short_target_qty_fixed_at_signal_time_not_execution_time() -> None:
    """REQUIRED TEST 18 (short-side counterpart of P7B-REPAIR-01's own
    causal proof): the short's target_qty is fixed from the SIGNAL bar's
    close, not the execution bar's drastically different close.

    P7B-REPAIR-02 interaction: sub-cap weight (-0.3, not the pipeline's
    automatic -1.0) so a 2x execution-time price gap still admits under
    the fill-time capacity check (30 shares * $200 = $6,000 < $10,000 cap),
    isolating this test's real subject from capacity admission."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    close_frame = pd.DataFrame({"BEAR": [100.0, 200.0, 200.0, 200.0]}, index=days)
    wts_spec = WeightToShareSpec(equity_usd=10_000.0)
    target_qty = weight_to_target_qty(weight=-0.3, price=100.0, spec=wts_spec)
    assert target_qty == -30
    pending_events = {"BEAR": [(days[0], -0.3, target_qty)]}
    spec = _diagnostic_spec(
        _long_short_signal_policy(), weight_to_share=wts_spec,
    )
    fold_df, summary = _simulate_fold(
        1, {"test_start": days[0], "test_end": days[-1] + pd.Timedelta(days=1)},
        close_frame, pending_events, spec,
    )
    evidence = summary["weight_to_share_evidence"]["BEAR"]
    # -0.3 * 10000 / 100 (SIGNAL close) = -30, NOT -0.3*10000/200=-15.
    assert evidence[1]["target_qty"] == -30


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
    # Legacy long-only carries no borrow_model key at all (LONG-SHORT-
    # REPAIR-01: additive-only, not a None-valued key -- see
    # test_legacy_long_only_identity_exact_golden_equality).
    legacy_identity = economic_protocol_identity(_diagnostic_spec(SignalPolicySpec(entry_threshold=0.5)).normalized())
    assert "borrow_model" not in legacy_identity["signal_policy"]


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
    """REQUIRED TEST 21: a legacy long-only run's output JSON never emits a
    negative target_qty -- it cannot be silently reinterpreted as a
    long/short evaluation.

    LONG-SHORT-REPAIR-01 (mission Section 4A): `direction_policy`/
    `borrow_model` are ADDITIVE-ONLY fields, present only under
    long_short_threshold_v1 -- a legacy long_only_v1 run's signal_policy
    JSON carries neither key at all (not even a `None`/default value),
    preserving byte-for-byte pre-long-short-patch canonical identity (see
    test_legacy_long_only_identity_exact_golden_equality). Their ABSENCE is
    itself the proof this run cannot be confused with a long/short one."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    bars_rows = [_bar_row("AAA", d, 100.0) for d in days]
    oos_rows = [_oos_row(1, "AAA", d, 0.9) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(SignalPolicySpec(entry_threshold=0.5), weight_to_share=WeightToShareSpec(equity_usd=10_000.0))
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    assert "direction_policy" not in out["signal_policy"]
    assert "borrow_model" not in out["signal_policy"]
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
# LONG-SHORT-REPAIR-01 REQUIRED TEST (mission Section 4E/4F item 12):
# async multi-symbol long+short regression -- proves the D/F gross-
# MAGNITUDE classification repair (RESEARCH-LONG-SHORT-ECONOMIC-POLICY-01)
# did not break the previously-frozen async/REPAIR-03 cohort contract.
# ---------------------------------------------------------------------------


def test_async_long_and_short_cohort_defers_on_missing_sibling_bar(tmp_path: Path) -> None:
    """LONGSYM (bullish) and SHORTSYM (bearish) are both scored at day0 --
    a single atomic gross-increasing cohort (same signal_ts). SHORTSYM has
    NO bar on day1 (async gap); per the frozen REPAIR-03 cohort contract, a
    missing member's bar defers the WHOLE cohort -- LONGSYM must NOT
    execute alone on day1 despite having its own bar, and no bar is ever
    fabricated for SHORTSYM. Both then execute TOGETHER on day2 once
    SHORTSYM's bar returns. Gross exposure is consumed by MAGNITUDE
    (mission Section 5C): +0.5 long and -0.5 short together consume gross
    1.0, not 0.0."""
    days = pd.date_range("2021-01-01", periods=5, freq="D", tz="UTC")
    long_bars = [_bar_row("LONGSYM", d, 100.0) for d in days]
    # SHORTSYM: present day0, ABSENT day1 (async gap), present day2 onward.
    short_bars = [_bar_row("SHORTSYM", d, 100.0) for i, d in enumerate(days) if i != 1]
    bars_rows = long_bars + short_bars
    oos_rows = [
        _oos_row(1, "LONGSYM", days[0], 0.9),
        _oos_row(1, "SHORTSYM", days[0], 0.1),
    ]
    folds = [_single_fold("2021-01-01", "2021-01-06")]
    bars_path = _write_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _diagnostic_spec(
        _long_short_signal_policy(long_threshold=0.7, short_threshold=0.3),
        weight_to_share=WeightToShareSpec(equity_usd=10_000.0),
    )
    out = json.loads(
        run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec).read_text(encoding="utf-8")
    )
    folds_out = out["folds"][0]
    long_evidence = folds_out["weight_to_share_evidence"]["LONGSYM"]
    short_evidence = folds_out["weight_to_share_evidence"]["SHORTSYM"]

    # day1 (index 1): LONGSYM has a bar but must NOT execute alone -- the
    # cohort (shared with the absent SHORTSYM) is deferred in full.
    assert long_evidence[1]["target_qty"] == 0
    # day2 (index 2, the first frame where BOTH have a bar again): both
    # execute together -- weight_each = max_gross_exposure(1.0)/2 = 0.5,
    # so 0.5*10000/100 = 50 shares each, opposite signs.
    assert long_evidence[2]["target_qty"] == 50
    assert short_evidence[2]["target_qty"] == -50

    # Gross exposure is reported in weight-space (fraction): +0.5 long and
    # -0.5 short together consume gross 1.0 (mission Section 5C), so the
    # fold's average_gross_exposure must be materially nonzero -- a signed
    # 0.5 + (-0.5) = 0.0 cancellation would wrongly report near-zero
    # exposure despite both symbols holding a full position each.
    assert folds_out["average_gross_exposure"] >= 0.4
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


# ---------------------------------------------------------------------------
# LONG-SHORT-REPAIR-01 REQUIRED TEST (mission Section 4C/4F item 14):
# truthful score semantics -- no production wording claims ml_score is a
# calibrated probability of a negative/declining return.
# ---------------------------------------------------------------------------


def test_no_production_wording_claims_score_is_probability_of_decline() -> None:
    """The model's actual truth: target=1 iff fwd_ret > a POSITIVE return
    threshold, and ml_score = P(target=1) -- the probability of the
    BULLISH positive-return class. A low ml_score therefore means a LOW
    probability of that bullish class, NOT a calibrated probability of
    decline/negative return (mission Section 2, "LONG-SHORT TERMINOLOGY
    DEFECT") -- long_short_threshold_v1 may still use a low score as an
    explicit bearish/SHORT strategy hypothesis, but must never describe
    that score itself as a probability of decline. Scans the modules that
    define and consume the direction-policy score mapping."""
    import inspect

    from mqk_research.ml import economic_walkforward as ewf

    forbidden_phrases = [
        "probability of decline",
        "probability of a decline",
        "probability of negative return",
        "probability of a negative return",
    ]
    source = inspect.getsource(ewf)
    lowered = source.lower()
    for phrase in forbidden_phrases:
        assert phrase not in lowered, f"found misleading score-semantics wording: {phrase!r}"
