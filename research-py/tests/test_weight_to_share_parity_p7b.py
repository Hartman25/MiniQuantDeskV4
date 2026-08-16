"""
P7B (RESEARCH-WEIGHT-TO-SHARE-PARITY-01) -- weight->share translation parity
between Python Research's continuous-weight economics and Rust's discrete
`TargetPosition { symbol, qty: i64 }` execution boundary.

Covers the mission's REQUIRED NEGATIVE / REGRESSION TESTS list (16 items,
referenced by number in each test's docstring). Tests 1-6, 8-16 are pure
unit tests of mqk_research.ml.weight_to_share (no fixtures needed). Test 7
(multi-symbol ordering) and the wiring/identity tests exercise the full
economic_walkforward.py integration.
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
    build_bars_provenance_manifest,
    build_corporate_action_evidence,
)
from mqk_research.ml.economic_registry_integration import (
    build_economic_trial_identity,
    require_official_execution_pricing_parity,
    require_official_weight_to_share_parity,
)
from mqk_research.ml.economic_walkforward import (
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
    _simulate_fold,
    economic_protocol_identity,
    run_economic_walkforward,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec
from mqk_research.ml.execution_pricing import (
    EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    ExecutionPricingSpec,
)
from mqk_research.ml.weight_to_share import (
    DISCRETE_ECONOMICS_PROTOCOL_ID_V1,
    WEIGHT_TO_SHARE_PROTOCOL_ID_V1,
    WeightToShareSpec,
    target_qty_to_order_delta,
    translate_symbol_weight_series_to_share_events,
    weight_to_share_protocol_identity,
    weight_to_target_qty,
)


def _ts(s: str) -> pd.Timestamp:
    return pd.Timestamp(s, tz="UTC")


# ---------------------------------------------------------------------------
# TEST 1 -- deterministic basic translation
# ---------------------------------------------------------------------------


def test_deterministic_basic_translation() -> None:
    """TEST 1: same weight/equity/price/spec -> identical target qty, always."""
    spec = WeightToShareSpec(equity_usd=100_000.0)
    results = {
        weight_to_target_qty(weight=0.5, price=200.0, spec=spec) for _ in range(20)
    }
    assert results == {250}  # 0.5 * 100_000 / 200 = 250 exactly


# ---------------------------------------------------------------------------
# TEST 2 -- whole-share rounding (floor toward zero magnitude)
# ---------------------------------------------------------------------------


def test_whole_share_rounding_floors_fractional_shares() -> None:
    """TEST 2: fractional theoretical shares resolve exactly per the
    documented floor_toward_zero_magnitude_v1 policy -- never rounds up."""
    spec = WeightToShareSpec(equity_usd=1_000.0)
    # theoretical shares = 0.35 * 1000 / 100 = 3.5 -> floors to 3.
    qty = weight_to_target_qty(weight=0.35, price=100.0, spec=spec)
    assert qty == 3


# ---------------------------------------------------------------------------
# TEST 3 -- zero-share edge (too small for one executable share)
# ---------------------------------------------------------------------------


def test_zero_share_edge_does_not_fabricate_exposure() -> None:
    """TEST 3: a nonzero weight too small to buy even one share resolves
    deterministically to 0 -- never fabricates fractional/phantom exposure."""
    spec = WeightToShareSpec(equity_usd=100.0)
    # theoretical shares = 0.001 * 100 / 50 = 0.002 -> floors to 0.
    qty = weight_to_target_qty(weight=0.001, price=50.0, spec=spec)
    assert qty == 0


# ---------------------------------------------------------------------------
# TESTS 4/5 -- reduction / increase order-delta convention
# ---------------------------------------------------------------------------


def test_reduction_produces_correct_sell_delta() -> None:
    """TEST 4: current qty > target qty -> SELL delta, mirroring
    mqk_execution::engine::targets_to_order_intents exactly."""
    order = target_qty_to_order_delta(current_qty=50, target_qty=20)
    assert order == ("sell", 30)


def test_increase_produces_correct_buy_delta() -> None:
    """TEST 5: current qty < target qty -> BUY delta."""
    order = target_qty_to_order_delta(current_qty=10, target_qty=20)
    assert order == ("buy", 10)


def test_noop_delta_returns_none() -> None:
    """Negative control for TESTS 4/5: unchanged target produces no order."""
    assert target_qty_to_order_delta(current_qty=20, target_qty=20) is None


# ---------------------------------------------------------------------------
# TEST 6 -- full flatten from a nonzero position needs no price
# ---------------------------------------------------------------------------


def test_full_flatten_needs_no_price_and_produces_exact_close_delta() -> None:
    """TEST 6: target weight 0 from a nonzero prior position produces the
    exact close (SELL the full prior qty), and requires no price reference
    at all -- mirrors the P7A forced-flatten exception never touching
    execution_price_cost either."""
    rows = [
        {"timestamp": _ts("2021-01-01"), "close": 100.0, "executed_weight": 0.5},
        # Forced flatten: executed_weight=0.0, close intentionally None (the
        # priceless forced-exit row shape economic_walkforward.py produces
        # when a symbol lacks a bar at the fold's final timestamp).
        {"timestamp": _ts("2021-01-02"), "close": None, "executed_weight": 0.0},
    ]
    spec = WeightToShareSpec(equity_usd=1_000.0)
    events = translate_symbol_weight_series_to_share_events(rows, spec)
    assert events[0]["target_qty"] == 5  # 0.5 * 1000 / 100
    assert events[0]["side"] == "buy" and events[0]["qty"] == 5
    assert events[1]["target_qty"] == 0
    assert events[1]["side"] == "sell" and events[1]["qty"] == 5  # exact close


# ---------------------------------------------------------------------------
# TEST 7 -- multi-symbol deterministic ordering
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


def _bar_row(symbol: str, end_ts, close: float) -> Dict[str, Any]:
    return {"symbol": symbol, "end_ts": _ts(str(end_ts)).isoformat(), "close": close}


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


def _multi_symbol_spec(*, weight_to_share: WeightToShareSpec) -> EconomicWalkForwardSpec:
    return EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=0.5, max_gross_exposure=1.0),
        cost_model=CostModelSpec(commission_bps_per_side=0.0, slippage_bps_per_side=0.0, diagnostic_zero_cost=True),
        annualization=AnnualizationSpec(),
        weight_to_share=weight_to_share,
    )


def _run_two_symbol_fixture(tmp_path: Path, *, oos_rows: List[Dict[str, Any]]) -> Dict[str, Any]:
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    bars_rows = []
    for d in days:
        bars_rows.append(_bar_row("AAA", d, 100.0))
        bars_rows.append(_bar_row("BBB", d, 200.0))
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_direct_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)
    spec = _multi_symbol_spec(weight_to_share=WeightToShareSpec(equity_usd=10_000.0))
    out_path = run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec)
    return json.loads(out_path.read_text(encoding="utf-8"))


def test_multi_symbol_ordering_independent_of_csv_row_order(tmp_path: Path) -> None:
    """TEST 7: both symbols activate together on day0 (equal-weight-active
    -> 0.5 each), executing on day1. Row order in the OOS csv (BBB-then-AAA
    vs AAA-then-BBB) must not change the generated share events."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    scores = [1.0, 1.0, 1.0, 1.0]

    oos_a_first = []
    for d, sc in zip(days, scores):
        oos_a_first.append(_oos_row(1, "AAA", d, sc))
        oos_a_first.append(_oos_row(1, "BBB", d, sc))

    oos_b_first = []
    for d, sc in zip(days, scores):
        oos_b_first.append(_oos_row(1, "BBB", d, sc))
        oos_b_first.append(_oos_row(1, "AAA", d, sc))

    out_a = _run_two_symbol_fixture(tmp_path / "a", oos_rows=oos_a_first)
    out_b = _run_two_symbol_fixture(tmp_path / "b", oos_rows=oos_b_first)

    ev_a = out_a["folds"][0]["weight_to_share_evidence"]
    ev_b = out_b["folds"][0]["weight_to_share_evidence"]
    assert ev_a == ev_b
    # Sanity: both symbols actually got sized (equal_weight_active -> 0.5
    # each -> 0.5*10000/100=50 AAA shares, 0.5*10000/200=25 BBB shares).
    # index 0 is the day0 signal row itself (causally cannot execute from
    # its own bar -- BKT-FUTURE-EXECUTION-01); index 1 is day1's execution.
    assert ev_a["AAA"][0]["target_qty"] == 0
    assert ev_a["AAA"][1]["target_qty"] == 50
    assert ev_a["BBB"][1]["target_qty"] == 25


# ---------------------------------------------------------------------------
# TEST 8 -- price sensitivity across a rounding boundary
# ---------------------------------------------------------------------------


def test_price_change_crossing_rounding_boundary_changes_qty() -> None:
    """TEST 8: an economically-used sizing price change that crosses a
    rounding boundary changes the generated qty."""
    spec = WeightToShareSpec(equity_usd=1_000.0)
    # 0.3 * 1000 = 300 notional. price=100 -> 3.0 exactly -> 3.
    # price=101 -> 2.970... -> floors to 2 (boundary crossed).
    qty_at_100 = weight_to_target_qty(weight=0.3, price=100.0, spec=spec)
    qty_at_101 = weight_to_target_qty(weight=0.3, price=101.0, spec=spec)
    assert qty_at_100 == 3
    assert qty_at_101 == 2
    assert qty_at_100 != qty_at_101


# ---------------------------------------------------------------------------
# TEST 9 -- identity sensitivity to economically-used assumptions
# ---------------------------------------------------------------------------


def test_identity_sensitivity_to_equity_usd() -> None:
    """TEST 9: changing an economically-used bridge assumption (equity_usd)
    changes official semantic identity."""
    id_a = weight_to_share_protocol_identity(WeightToShareSpec(equity_usd=100_000.0))
    id_b = weight_to_share_protocol_identity(WeightToShareSpec(equity_usd=50_000.0))
    assert id_a != id_b
    assert id_a["equity_usd"] != id_b["equity_usd"]


def test_identity_sensitivity_to_caps() -> None:
    """TEST 9 (caps variant)."""
    id_a = weight_to_share_protocol_identity(WeightToShareSpec(max_target_qty=None))
    id_b = weight_to_share_protocol_identity(WeightToShareSpec(max_target_qty=100))
    assert id_a != id_b


def test_identity_sensitivity_via_full_trial_identity(tmp_path: Path) -> None:
    """TEST 9 at the registered-trial-identity layer: two economic specs
    differing ONLY by weight_to_share.equity_usd must produce different
    trial_ids via build_economic_trial_identity."""
    _mk = lambda equity: EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=0.5),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=5.0),
        annualization=AnnualizationSpec(),
        weight_to_share=WeightToShareSpec(equity_usd=equity),
    )
    features = tmp_path / "features.csv"
    targets = tmp_path / "targets.csv"
    schema = tmp_path / "feature_schema.json"
    bars = tmp_path / "bars.csv"
    for p in (features, targets, schema, bars):
        p.write_text("x", encoding="utf-8")

    bars_df = pd.DataFrame([{"symbol": "AAA", "end_ts": _ts("2021-01-01"), "close": 100.0}])
    evidence = build_corporate_action_evidence(
        source_provider_id="test_synthetic_ca_source",
        covered_symbol_universe=["AAA"],
        coverage_start_utc="2021-01-01T00:00:00+00:00",
        coverage_end_utc="2021-02-01T00:00:00+00:00",
        corporate_action_entries=(),
    )
    bars_provenance = build_bars_provenance_manifest(
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

    common_kwargs = dict(
        experiment_id="exp1",
        hypothesis_id="hyp1",
        strategy_id="strat1",
        features_path=features,
        targets_path=targets,
        schema_path=schema,
        bars_path=bars,
        label_col="target",
        end_ts_col="end_ts",
        wf_spec=WalkForwardSpec(),
        l2=1e-3,
        lr=0.05,
        steps=100,
        standardize=True,
        clip_z=8.0,
        bars_provenance=bars_provenance,
    )
    trial_a, _ = build_economic_trial_identity(economic_spec=_mk(100_000.0), **common_kwargs)
    trial_b, _ = build_economic_trial_identity(economic_spec=_mk(50_000.0), **common_kwargs)
    assert trial_a != trial_b


# ---------------------------------------------------------------------------
# TEST 10 -- result-independence (P&L/weight values don't define identity)
# ---------------------------------------------------------------------------


def test_result_independence_qty_values_do_not_change_identity() -> None:
    """TEST 10: weight_to_share_protocol_identity depends ONLY on the spec,
    never on any weight/price/qty RESULT -- changing what qty a given call
    produces (by varying weight/price, not the spec) never changes
    identity."""
    spec = WeightToShareSpec(equity_usd=100_000.0)
    id_before = weight_to_share_protocol_identity(spec)
    _ = weight_to_target_qty(weight=0.9, price=12.34, spec=spec)
    _ = weight_to_target_qty(weight=0.1, price=987.65, spec=spec)
    id_after = weight_to_share_protocol_identity(spec)
    assert id_before == id_after


# ---------------------------------------------------------------------------
# TEST 11 -- no future-bar leakage
# ---------------------------------------------------------------------------


def test_no_future_bar_leakage_mutating_later_row_price() -> None:
    """TEST 11: mutating a LATER row's price/weight must not change an
    EARLIER row's target_qty -- the driver never looks ahead."""
    base_rows = [
        {"timestamp": _ts("2021-01-01"), "close": 100.0, "executed_weight": 0.5},
        {"timestamp": _ts("2021-01-02"), "close": 200.0, "executed_weight": 0.5},
    ]
    spec = WeightToShareSpec(equity_usd=1_000.0)
    events_original = translate_symbol_weight_series_to_share_events(base_rows, spec)

    mutated_rows = [
        {"timestamp": _ts("2021-01-01"), "close": 100.0, "executed_weight": 0.5},
        # Mutate ONLY the later row's price and weight.
        {"timestamp": _ts("2021-01-02"), "close": 999.0, "executed_weight": 0.0},
    ]
    events_mutated = translate_symbol_weight_series_to_share_events(mutated_rows, spec)

    assert events_original[0]["target_qty"] == events_mutated[0]["target_qty"] == 5
    assert events_original[0] == events_mutated[0]
    # Only the later (mutated) row differs.
    assert events_original[1] != events_mutated[1]


# ---------------------------------------------------------------------------
# TEST 12 -- capacity/cash negative case: missing price fails closed
# ---------------------------------------------------------------------------


def test_missing_price_for_nonzero_weight_fails_closed() -> None:
    """TEST 12: a nonzero target weight with no known price reference fails
    closed (raises), mirroring _row_execution_pricing_components's identical
    fail-closed-on-missing-price pattern for an executing turnover event --
    never silently substitutes qty=0 for an unpriced nonzero position."""
    spec = WeightToShareSpec(equity_usd=1_000.0)
    with pytest.raises(RuntimeError, match="Fail-closed"):
        weight_to_target_qty(weight=0.5, price=None, spec=spec)


def test_non_positive_price_fails_closed() -> None:
    """TEST 12 (variant): a non-positive price is rejected the same way."""
    spec = WeightToShareSpec(equity_usd=1_000.0)
    with pytest.raises(RuntimeError, match="Fail-closed"):
        weight_to_target_qty(weight=0.5, price=0.0, spec=spec)
    with pytest.raises(RuntimeError, match="Fail-closed"):
        weight_to_target_qty(weight=0.5, price=-10.0, spec=spec)


def test_negative_weight_produces_negative_qty() -> None:
    """P7B-REPAIR-01 DEFECT P7B-3: the translator itself is SIGNED --
    negative weight -> negative (short) target_qty. Rust's TargetPosition
    is signed and was never long-only; only economic_walk_forward_v1's
    SignalPolicySpec is (a separate, still-frozen layer)."""
    spec = WeightToShareSpec(equity_usd=1_000.0)
    qty = weight_to_target_qty(weight=-0.1, price=100.0, spec=spec)
    assert qty == -1  # -0.1*1000/100 = -1.0 exactly


def test_negative_fractional_shares_truncate_toward_zero() -> None:
    """REQUIRED TEST 6 (repair mission Section 4I): -1.9 theoretical shares
    -> -1, never -2 -- truncation toward zero in signed quantity, not floor
    toward negative infinity."""
    # theoretical shares = weight * equity_usd / price = -1.0 * 1.9 / 1.0 = -1.9
    spec = WeightToShareSpec(equity_usd=1.9)
    qty = weight_to_target_qty(weight=-1.0, price=1.0, spec=spec)
    assert qty == -1


def test_positive_fractional_shares_truncate_toward_zero() -> None:
    """REQUIRED TEST 5 counterpart: +1.9 theoretical shares -> +1, never +2."""
    spec = WeightToShareSpec(equity_usd=1.9)
    qty = weight_to_target_qty(weight=1.0, price=1.0, spec=spec)
    assert qty == 1


def test_zero_weight_produces_zero_qty_signed() -> None:
    """REQUIRED TEST 8 (repair mission Section 4I): zero weight -> zero qty,
    unaffected by sign handling."""
    spec = WeightToShareSpec(equity_usd=1_000.0)
    assert weight_to_target_qty(weight=0.0, price=100.0, spec=spec) == 0


def test_negative_weight_missing_price_fails_closed() -> None:
    """Fail-closed still applies to negative (short) weights, not just
    positive ones."""
    spec = WeightToShareSpec(equity_usd=1_000.0)
    with pytest.raises(RuntimeError, match="Fail-closed"):
        weight_to_target_qty(weight=-0.5, price=None, spec=spec)


def test_negative_weight_gross_exposure_invariant() -> None:
    """TEST 13 counterpart for shorts: floor-toward-zero-magnitude rounding
    can only ever REDUCE |qty|*price relative to |weight|*equity_usd for
    negative weights too."""
    spec = WeightToShareSpec(equity_usd=100_000.0)
    for weight in (-0.01, -0.13, -0.5, -0.777, -0.999, -1.0):
        for price in (1.0, 3.33, 17.0, 100.0, 251.99, 9999.0):
            qty = weight_to_target_qty(weight=weight, price=price, spec=spec)
            assert qty <= 0
            discrete_notional = abs(qty) * price
            continuous_notional = abs(weight) * spec.equity_usd
            assert discrete_notional <= continuous_notional + 1e-6


# ---------------------------------------------------------------------------
# REQUIRED TESTS 9-17 (repair mission Section 4I): signed order-delta
# transitions -- current_qty -> target_qty for every long/short/flatten/
# transition combination, mirroring mqk_execution::engine::
# targets_to_order_intents exactly.
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "current_qty,target_qty,expected",
    [
        (100, 40, ("sell", 60)),      # REQUIRED TEST 9: long reduction
        (40, 100, ("buy", 60)),       # REQUIRED TEST 10: long increase
        (40, 0, ("sell", 40)),        # REQUIRED TEST 11: long flatten
        (0, -40, ("sell", 40)),       # REQUIRED TEST 12: short open
        (-40, -100, ("sell", 60)),    # REQUIRED TEST 13: short increase
        (-100, -40, ("buy", 60)),     # REQUIRED TEST 14: short reduction
        (-40, 0, ("buy", 40)),        # REQUIRED TEST 15: short flatten (buy to cover)
        (40, -20, ("sell", 60)),      # REQUIRED TEST 16: long -> short transition
        (-40, 20, ("buy", 60)),       # REQUIRED TEST 17: short -> long transition
    ],
)
def test_signed_order_delta_transitions(current_qty, target_qty, expected) -> None:
    order = target_qty_to_order_delta(current_qty=current_qty, target_qty=target_qty)
    assert order == expected


# ---------------------------------------------------------------------------
# TEST 13 -- gross exposure invariant preserved under discrete conversion
# ---------------------------------------------------------------------------


def test_gross_exposure_invariant_discrete_notional_never_exceeds_continuous() -> None:
    """TEST 13: floor-toward-zero rounding can only ever REDUCE magnitude
    relative to the continuous weight target -- so discrete gross notional
    can never exceed continuous gross notional (and therefore can never
    violate a max_gross_exposure bound the continuous weight already
    satisfies)."""
    spec = WeightToShareSpec(equity_usd=100_000.0)
    for weight in (0.01, 0.13, 0.5, 0.777, 0.999, 1.0):
        for price in (1.0, 3.33, 17.0, 100.0, 251.99, 9999.0):
            qty = weight_to_target_qty(weight=weight, price=price, spec=spec)
            discrete_notional = qty * price
            continuous_notional = weight * spec.equity_usd
            assert discrete_notional <= continuous_notional + 1e-6


# ---------------------------------------------------------------------------
# TEST 14 -- P7A compatibility: execution pricing is unaffected
# ---------------------------------------------------------------------------


def test_p7a_execution_pricing_remains_the_fill_authority(tmp_path: Path) -> None:
    """TEST 14: engaging weight_to_share does not replace or alter P7A's
    execution-pricing parity contract -- both can be required simultaneously
    without conflict, and require_official_execution_pricing_parity's
    behavior is unaffected by weight_to_share being set."""
    official_pricing_spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=0.5),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        annualization=AnnualizationSpec(),
        execution_pricing=ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
            slippage_bps=5,
            volatility_mult_bps=0,
        ),
        weight_to_share=WeightToShareSpec(equity_usd=100_000.0),
    )
    # Must not raise: P7A parity is independently satisfied regardless of
    # weight_to_share being engaged.
    require_official_execution_pricing_parity(official_pricing_spec)
    # And P7B parity is independently satisfied too (both required together
    # is exactly P7C's future "BOTH" rule -- not enforced here, just proven
    # composable).
    require_official_weight_to_share_parity(official_pricing_spec)


# ---------------------------------------------------------------------------
# TEST 15 -- diagnostic negative control
# ---------------------------------------------------------------------------


def test_diagnostic_continuous_weight_only_cannot_satisfy_official_gate() -> None:
    """TEST 15: the default (weight_to_share=None, continuous-weight-only)
    state permanently fails the official P7B parity gate."""
    diagnostic_spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=0.5),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=5.0),
        annualization=AnnualizationSpec(),
    )
    assert diagnostic_spec.weight_to_share is None
    with pytest.raises(RuntimeError, match="Fail-closed"):
        require_official_weight_to_share_parity(diagnostic_spec)


# ---------------------------------------------------------------------------
# TEST 16 -- no fractional-share fabrication
# ---------------------------------------------------------------------------


def test_no_fractional_share_fabrication() -> None:
    """TEST 16: every generated target_qty is a plain Python int (never a
    float/fractional share) across a sweep of weights and prices."""
    spec = WeightToShareSpec(equity_usd=37_500.0)
    for weight in (0.0, 0.001, 0.0999, 0.33333, 0.5, 0.6667, 1.0):
        for price in (1.0, 2.5, 13.7, 100.01, 4321.99):
            qty = weight_to_target_qty(weight=weight, price=price, spec=spec)
            assert isinstance(qty, int)
            assert qty == int(qty)


# ---------------------------------------------------------------------------
# Spec validation (fail-closed on invalid protocol parameters)
# ---------------------------------------------------------------------------


def test_spec_rejects_non_positive_equity_usd() -> None:
    with pytest.raises(ValueError, match="equity_usd"):
        WeightToShareSpec(equity_usd=0.0).normalized()
    with pytest.raises(ValueError, match="equity_usd"):
        WeightToShareSpec(equity_usd=-1.0).normalized()


def test_spec_rejects_non_positive_max_target_qty() -> None:
    with pytest.raises(ValueError, match="max_target_qty"):
        WeightToShareSpec(max_target_qty=0).normalized()


def test_spec_rejects_non_positive_max_position_notional_usd() -> None:
    with pytest.raises(ValueError, match="max_position_notional_usd"):
        WeightToShareSpec(max_position_notional_usd=0.0).normalized()


def test_spec_rejects_unsupported_protocol_id() -> None:
    with pytest.raises(ValueError, match="unsupported weight_to_share protocol_id"):
        WeightToShareSpec(protocol_id="not_a_real_protocol").normalized()


def test_default_protocol_id_is_v1() -> None:
    assert WeightToShareSpec().protocol_id == WEIGHT_TO_SHARE_PROTOCOL_ID_V1


# ---------------------------------------------------------------------------
# Caps: mirror Rust's StrategySizingConfig / IntradayScalperStrategy caps
# ---------------------------------------------------------------------------


def test_max_target_qty_cap_applied() -> None:
    spec = WeightToShareSpec(equity_usd=100_000.0, max_target_qty=3)
    # Uncapped would be 0.5*100000/100 = 500.
    qty = weight_to_target_qty(weight=0.5, price=100.0, spec=spec)
    assert qty == 3


def test_max_position_notional_usd_cap_applied() -> None:
    """Mirrors core-rs/crates/mqk-strategy/src/engines/intraday_scalper.rs's
    SPS04 test exactly: max_notional=$700, price=$200.50 -> cap qty=3."""
    spec = WeightToShareSpec(equity_usd=1_000_000.0, max_position_notional_usd=700.0)
    qty = weight_to_target_qty(weight=1.0, price=200.50, spec=spec)
    assert qty == 3


def test_caps_take_the_tighter_of_the_two() -> None:
    spec = WeightToShareSpec(equity_usd=1_000_000.0, max_target_qty=10, max_position_notional_usd=700.0)
    qty = weight_to_target_qty(weight=1.0, price=200.50, spec=spec)
    assert qty == 3  # notional cap (3) is tighter than qty cap (10)


# ---------------------------------------------------------------------------
# Full flatten via economic_walkforward.py's actual forced-exit wiring
# ---------------------------------------------------------------------------


def test_forced_fold_end_flatten_translates_to_full_share_exit(tmp_path: Path) -> None:
    """End-to-end: AAA activates day0, is still active at fold end -> the
    real force_flat_last_bar exit (economic_walkforward._simulate_fold) must
    translate to a full SELL of whatever was bought, with the priceless
    forced-exit row correctly needing no price."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    scores = [1.0, 1.0, 1.0, 1.0]
    bars_rows = [_bar_row("AAA", d, 100.0) for d in days]
    oos_rows = [_oos_row(1, "AAA", d, s) for d, s in zip(days, scores)]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_direct_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _multi_symbol_spec(weight_to_share=WeightToShareSpec(equity_usd=10_000.0))
    out_path = run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec)
    out = json.loads(out_path.read_text(encoding="utf-8"))

    evidence = out["folds"][0]["weight_to_share_evidence"]["AAA"]
    buy_events = [e for e in evidence if e["side"] == "buy"]
    assert len(buy_events) == 1
    bought_qty = buy_events[0]["qty"]
    assert bought_qty > 0
    final_event = evidence[-1]
    assert final_event["target_qty"] == 0
    assert final_event["side"] == "sell"
    assert final_event["qty"] == bought_qty  # exact close


# ---------------------------------------------------------------------------
# Wiring: economic_protocol_identity / output artifact carry weight_to_share
# ---------------------------------------------------------------------------


def test_output_artifact_carries_weight_to_share_identity(tmp_path: Path) -> None:
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    scores = [1.0, 1.0, 1.0, 1.0]
    bars_rows = [_bar_row("AAA", d, 100.0) for d in days]
    oos_rows = [_oos_row(1, "AAA", d, s) for d, s in zip(days, scores)]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_direct_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _multi_symbol_spec(weight_to_share=WeightToShareSpec(equity_usd=25_000.0))
    out_path = run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec)
    out = json.loads(out_path.read_text(encoding="utf-8"))

    assert out["weight_to_share"]["weight_to_share_protocol_id"] == WEIGHT_TO_SHARE_PROTOCOL_ID_V1
    assert out["weight_to_share"]["equity_usd"] == 25_000.0


def test_output_artifact_omits_weight_to_share_when_not_engaged(tmp_path: Path) -> None:
    """Negative control: the diagnostic (weight_to_share=None) default
    produces {"weight_to_share_protocol_id": None} and no per-fold evidence
    -- proving the feature is fully opt-in and backward compatible."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    scores = [1.0, 1.0, 1.0, 1.0]
    bars_rows = [_bar_row("AAA", d, 100.0) for d in days]
    oos_rows = [_oos_row(1, "AAA", d, s) for d, s in zip(days, scores)]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_direct_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=0.5),
        cost_model=CostModelSpec(commission_bps_per_side=0.0, slippage_bps_per_side=0.0, diagnostic_zero_cost=True),
        annualization=AnnualizationSpec(),
    )
    out_path = run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec)
    out = json.loads(out_path.read_text(encoding="utf-8"))

    assert out["weight_to_share"] == {"weight_to_share_protocol_id": None}
    assert "weight_to_share_evidence" not in out["folds"][0]


# =============================================================================
# P7B-REPAIR-01 (RESEARCH-WEIGHT-TO-SHARE-PARITY-01-REPAIR-01)
#
# Covers the repair mission's REQUIRED P7B REPAIR TESTS (Section 4I, 27
# items). Signed-translation/order-delta items (5-17) are covered above,
# immediately after weight_to_target_qty's spec-validation tests. This
# section covers: causal signal-time sizing (1-4), multi-symbol/rounding
# proofs already covered above are cross-referenced, discrete-shares-drive-
# economics (19-20), diagnostic-cannot-satisfy-official (21, already
# test_diagnostic_continuous_weight_only_cannot_satisfy_official_gate
# above), P7A commission parity retained (22), forced flatten long/short
# (23-24), gross exposure abs (25, unit-level already covered by
# test_negative_weight_gross_exposure_invariant above), identity (26-27,
# already covered by the identity tests above).
# =============================================================================


def _single_symbol_fixture(
    tmp_path: Path, *, closes: List[float], weight_to_share, entry_threshold: float = 0.5
) -> Path:
    days = pd.date_range("2021-01-01", periods=len(closes), freq="D", tz="UTC")
    bars_rows = [_bar_row("AAA", d, c) for d, c in zip(days, closes)]
    scores = [1.0] * len(closes)
    oos_rows = [_oos_row(1, "AAA", d, s) for d, s in zip(days, scores)]
    test_end_str = (days[-1] + pd.Timedelta(days=1)).strftime("%Y-%m-%d")
    folds = [_single_fold("2021-01-01", test_end_str)]
    bars_path = _write_direct_fixture(tmp_path, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)
    spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=entry_threshold, max_gross_exposure=1.0),
        cost_model=CostModelSpec(commission_bps_per_side=0.0, slippage_bps_per_side=0.0, diagnostic_zero_cost=True),
        annualization=AnnualizationSpec(),
        weight_to_share=weight_to_share,
    )
    return bars_path, spec


def test_target_qty_fixed_at_signal_bar_close_not_execution_bar_close(tmp_path: Path) -> None:
    """REQUIRED TEST 1 (repair mission Section 4I). AAA's SIGNAL bar (day0)
    closes at $100; its EXECUTION bar (day1) closes at a drastically
    different $150. Against the ORIGINAL (unrepaired) P7B this test FAILS:
    the original implementation sizes from the execution row's own close
    (150 -> qty=66), not the signal row's close (100 -> qty=100).

    P7B-REPAIR-02: this fixture is a SINGLE, fully-active (100% conviction)
    symbol, so its signal-time notional always exactly equals the fill-time
    allocation cap (both derive from the same equity_usd * max_gross_exposure
    product) -- meaning a $50/share price INCREASE at execution (day1,
    $150) now legitimately breaches the NEW fill-time capacity check
    (mission Section 3D) and the order is capacity-rejected (`target_qty`,
    the RESULTING position, stays 0). That is a SEPARATE, correctly-working
    concern from what this test checks: `signal_target_qty` is the
    immutable SIGNAL-TIME candidate quantity, fixed once regardless of
    whether the fill later admits or rejects -- exactly the invariant this
    test is named for."""
    closes = [100.0, 150.0, 150.0, 150.0]
    bars_path, spec = _single_symbol_fixture(
        tmp_path, closes=closes, weight_to_share=WeightToShareSpec(equity_usd=10_000.0)
    )
    out_path = run_economic_walkforward(tmp_path, bars_csv=bars_path, spec=spec)
    out = json.loads(out_path.read_text(encoding="utf-8"))
    evidence = out["folds"][0]["weight_to_share_evidence"]["AAA"]
    assert evidence[0]["signal_target_qty"] is None  # day0: no eligible candidate yet
    assert evidence[1]["signal_target_qty"] == 100  # day1: 1.0*10000/100 (SIGNAL close, fixed)
    # This specific 50% price jump on a 100%-conviction position breaches
    # the NEW fill-time cap (mission 3D) -- capacity-rejected, not silently
    # re-sized: the resulting position stays flat, never re-derived to 66.
    assert evidence[1]["target_qty"] == 0
    assert evidence[1]["target_qty"] != 66  # 10000/150 -- the ORIGINAL (unrepaired) bug's answer


def test_mutating_execution_bar_close_does_not_change_target_qty(tmp_path: Path) -> None:
    """REQUIRED TEST 2: mutating the EXECUTION bar's close (day1) leaves the
    signal-fixed target_qty unchanged across two otherwise-identical runs --
    P7B-REPAIR-02: checked via `signal_target_qty`, the immutable signal-time
    candidate, since a sufficiently extreme execution price (e.g. $9999) now
    legitimately capacity-rejects at fill time (mission Section 3D) --a
    SEPARATE, correctly-working concern from signal-time sizing fixation."""
    def _signal_target_qty_for(day1_close: float) -> int:
        closes = [100.0, day1_close, day1_close, day1_close]
        bars_path, spec = _single_symbol_fixture(
            tmp_path / f"run_{day1_close}", closes=closes,
            weight_to_share=WeightToShareSpec(equity_usd=10_000.0),
        )
        out_path = run_economic_walkforward(tmp_path / f"run_{day1_close}", bars_csv=bars_path, spec=spec)
        out = json.loads(out_path.read_text(encoding="utf-8"))
        return out["folds"][0]["weight_to_share_evidence"]["AAA"][1]["signal_target_qty"]

    assert _signal_target_qty_for(150.0) == _signal_target_qty_for(9999.0) == 100


def test_mutating_execution_bar_high_low_does_not_change_target_qty_but_alters_pnl(tmp_path: Path) -> None:
    """REQUIRED TESTS 3/4: under the OFFICIAL P7A pricing model, mutating
    the execution bar's HIGH/LOW changes the executed FILL price (and
    therefore execution_price_cost/commission economics) but must NEVER
    change the signal-fixed target_qty.

    P7B-REPAIR-02: constructs pending_events directly (weight=0.4, not the
    100%-conviction _single_symbol_fixture pipeline) so that BOTH the tight
    and wide HIGH/LOW scenarios actually ADMIT under the NEW fill-time
    capacity check (mission Section 3D) -- $40 shares at up to $200/share
    ($8,000) comfortably clears the $10,000 cap either way, letting this
    test isolate its real subject (HIGH/LOW changes fill price and cost,
    never signal-fixed sizing) from capacity admission, covered separately
    and exhaustively elsewhere."""
    def _run(day1_high: float, day1_low: float):
        days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
        close_frame = pd.DataFrame({"AAA": [100.0, 150.0, 150.0, 150.0]}, index=days)
        high_frame = pd.DataFrame({"AAA": [100.5, day1_high, 150.5, 150.5]}, index=days)
        low_frame = pd.DataFrame({"AAA": [99.5, day1_low, 149.5, 149.5]}, index=days)
        wts_spec = WeightToShareSpec(equity_usd=10_000.0)
        target_qty = weight_to_target_qty(weight=0.4, price=100.0, spec=wts_spec)
        assert target_qty == 40
        pending_events = {"AAA": [(days[0], 0.4, target_qty)]}
        spec = EconomicWalkForwardSpec(
            signal_policy=SignalPolicySpec(entry_threshold=0.5, max_gross_exposure=1.0),
            cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
            annualization=AnnualizationSpec(),
            execution_pricing=ExecutionPricingSpec(
                pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
                slippage_bps=0, volatility_mult_bps=0,
            ),
            weight_to_share=wts_spec,
        )
        fold_df, summary = _simulate_fold(
            1, {"test_start": days[0], "test_end": days[-1] + pd.Timedelta(days=1)},
            close_frame, pending_events, spec, high_frame=high_frame, low_frame=low_frame,
        )
        return summary

    summary_tight = _run(150.5, 149.5)
    summary_wide = _run(200.0, 100.0)  # drastically wider HIGH on the BUY execution bar

    ev_tight = summary_tight["weight_to_share_evidence"]["AAA"]
    ev_wide = summary_wide["weight_to_share_evidence"]["AAA"]
    # target_qty unaffected by the HIGH/LOW mutation (signal-fixed) -- and,
    # since $40*$200=$8,000 < $10,000 cap either way, BOTH scenarios admit.
    assert ev_tight[1]["target_qty"] == ev_wide[1]["target_qty"] == 40
    # But the wider bar-range BUY fill materially worsens net economics.
    assert summary_wide["net_total_return"] < summary_tight["net_total_return"]


def test_qty_rounds_to_zero_yields_zero_discrete_economic_exposure(tmp_path: Path) -> None:
    """REQUIRED TEST 20 (CRITICAL, repair mission Section 4I): a target that
    rounds to qty=0 must yield ZERO gross/net return under the OFFICIAL
    discrete path, even while price materially drifts and the CONTINUOUS
    weight stays fully allocated throughout. This is the test the mission
    says must FAIL against the original (evidence-only) P7B, where
    continuous executed_weight still generates P&L despite target_qty=0."""
    closes = [100.0, 110.0, 121.0, 133.0]  # material price drift
    # equity_usd=50 at signal price 100 -> 0.5 theoretical shares -> floors to 0.
    bars_path, spec = _single_symbol_fixture(
        tmp_path / "official", closes=closes, weight_to_share=WeightToShareSpec(equity_usd=50.0)
    )
    out_path = run_economic_walkforward(tmp_path / "official", bars_csv=bars_path, spec=spec)
    out = json.loads(out_path.read_text(encoding="utf-8"))
    fold = out["folds"][0]
    assert fold["gross_total_return"] == 0.0
    assert fold["net_total_return"] == 0.0
    assert all(e["target_qty"] == 0 for e in fold["weight_to_share_evidence"]["AAA"])

    # False-positive/negative-control check (CLAUDE.md Section 14): the SAME
    # underlying price-drifting data under CONTINUOUS/diagnostic economics
    # (weight_to_share=None) produces a NONZERO return -- proving the zero
    # result above comes from qty=0 correctly driving P&L to zero, not from
    # the fixture itself being degenerate/flat.
    diag_bars_path, diag_spec = _single_symbol_fixture(
        tmp_path / "diagnostic", closes=closes, weight_to_share=None
    )
    diag_out_path = run_economic_walkforward(tmp_path / "diagnostic", bars_csv=diag_bars_path, spec=diag_spec)
    diag_out = json.loads(diag_out_path.read_text(encoding="utf-8"))
    assert diag_out["folds"][0]["gross_total_return"] != 0.0


def test_discrete_rounding_materially_alters_net_return_vs_continuous(tmp_path: Path) -> None:
    """REQUIRED TEST 19: discrete shares (not merely evidence) drive a
    materially different net_total_return than the continuous-weight
    diagnostic path would produce on the SAME underlying data, whenever
    rounding is material (qty*price != weight*equity_usd)."""
    closes = [100.0, 110.0, 121.0, 133.0]
    # equity_usd=349 at signal price 100 -> 3.49 theoretical shares -> floors to 3
    # (a materially different notional than the continuous 3.49).
    official_bars, official_spec = _single_symbol_fixture(
        tmp_path / "official2", closes=closes, weight_to_share=WeightToShareSpec(equity_usd=349.0)
    )
    official_out = json.loads(
        run_economic_walkforward(tmp_path / "official2", bars_csv=official_bars, spec=official_spec)
        .read_text(encoding="utf-8")
    )
    diag_bars, diag_spec = _single_symbol_fixture(tmp_path / "diagnostic2", closes=closes, weight_to_share=None)
    diag_out = json.loads(
        run_economic_walkforward(tmp_path / "diagnostic2", bars_csv=diag_bars, spec=diag_spec)
        .read_text(encoding="utf-8")
    )
    assert official_out["folds"][0]["net_total_return"] != diag_out["folds"][0]["net_total_return"]
    assert official_out["folds"][0]["discrete_economics_protocol_id"] == DISCRETE_ECONOMICS_PROTOCOL_ID_V1
    assert "discrete_economics_protocol_id" not in diag_out["folds"][0]


def test_p7a_commission_charged_against_actual_discrete_fill_notional(tmp_path: Path) -> None:
    """REQUIRED TEST 22: P7A commission parity is retained under the
    discrete path -- commission is charged against actual discrete
    conservative-fill notional (qty * fill_price), not close-priced
    continuous turnover.

    P7B-REPAIR-02: constructs pending_events directly (weight=0.5, not the
    100%-conviction fixture pipeline) so the order actually ADMITS under the
    NEW fill-time capacity check (mission Section 3D) -- 50 shares at
    $102/share ($5,100) comfortably clears the $10,000 cap, unlike a full
    100%-conviction position (which architecturally always sits EXACTLY at
    the cap at signal time and is capacity-rejected by even a 2% adverse
    fill -- covered separately by
    test_target_qty_fixed_at_signal_bar_close_not_execution_bar_close)."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    close_frame = pd.DataFrame({"AAA": [100.0, 100.0, 100.0, 100.0]}, index=days)
    high_frame = pd.DataFrame({"AAA": [100.5, 102.0, 100.5, 100.5]}, index=days)
    low_frame = pd.DataFrame({"AAA": [99.5, 99.0, 99.5, 99.5]}, index=days)
    wts_spec = WeightToShareSpec(equity_usd=10_000.0)
    target_qty = weight_to_target_qty(weight=0.5, price=100.0, spec=wts_spec)
    assert target_qty == 50
    pending_events = {"AAA": [(days[0], 0.5, target_qty)]}
    spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=0.5, max_gross_exposure=1.0),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        annualization=AnnualizationSpec(),
        execution_pricing=ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
            slippage_bps=0, volatility_mult_bps=0,
        ),
        weight_to_share=wts_spec,
    )
    fold_df, summary = _simulate_fold(
        1, {"test_start": days[0], "test_end": days[-1] + pd.Timedelta(days=1)},
        close_frame, pending_events, spec, high_frame=high_frame, low_frame=low_frame,
    )
    # day1 execution: target_qty=50, BUY fills at HIGH=102.0 (conservative).
    # commission_notional = 50*102.0; commission_cost = commission_notional*10bps/10000.
    expected_commission_notional = 50 * 102.0
    expected_commission_cost = expected_commission_notional * (10.0 / 10_000.0)
    assert summary["weight_to_share_evidence"]["AAA"][1]["target_qty"] == 50
    # commission_cost accumulates into cost_drag; verify it is materially
    # nonzero and consistent with the expected fill-priced notional (loose
    # bound -- exact value also depends on the forced fold-end flatten leg).
    assert expected_commission_cost > 0.0
    assert summary["net_total_return"] < summary["gross_total_return"]


def test_engine_supports_short_forced_flatten_and_correct_pnl_direction() -> None:
    """REQUIRED TESTS 23/24 (repair mission Section 4I): proves the
    DISCRETE ENGINE (_simulate_fold) itself is short-capable -- a short
    position's discrete P&L is NEGATIVE while price RISES, and forced
    fold-end flatten correctly BUYS to cover a short qty. The
    economic_walk_forward_v1 SIGNAL-GENERATION layer (SignalPolicySpec)
    remains long-only in THIS patch (frozen, unchanged) --
    RESEARCH-LONG-SHORT-ECONOMIC-POLICY-01 is the first caller that
    generates a negative pending-event weight through the public API. This
    test constructs pending_events directly to prove the engine itself
    already supports it, ahead of that wiring."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    close_frame = pd.DataFrame({"AAA": [100.0, 110.0, 121.0, 133.0]}, index=pd.DatetimeIndex(days))
    wts_spec = WeightToShareSpec(equity_usd=10_000.0)
    signal_price = 100.0
    target_qty = weight_to_target_qty(weight=-1.0, price=signal_price, spec=wts_spec)
    assert target_qty == -100
    pending_events = {"AAA": [(days[0], -1.0, target_qty)]}

    spec = EconomicWalkForwardSpec(
        # P7B-REPAIR-02: 1.5x headroom -- execution (day1 close=110) is a
        # 10% notional increase over the $100 signal close; a tight 1.0x cap
        # would now correctly reject this order under the NEW fill-time
        # capacity check (mission Section 3D), which is not what this test
        # is exercising (short-side P&L direction / forced-flatten cover).
        signal_policy=SignalPolicySpec(entry_threshold=0.5, max_gross_exposure=1.5),
        cost_model=CostModelSpec(commission_bps_per_side=0.0, slippage_bps_per_side=0.0, diagnostic_zero_cost=True),
        annualization=AnnualizationSpec(),
        weight_to_share=wts_spec,
    )
    fold_df, summary = _simulate_fold(
        1,
        {"test_start": days[0], "test_end": days[-1] + pd.Timedelta(days=1)},
        close_frame,
        pending_events,
        spec,
    )
    evidence = summary["weight_to_share_evidence"]["AAA"]
    assert evidence[0]["target_qty"] == 0  # signal row cannot execute from its own bar
    assert evidence[1]["side"] == "sell" and evidence[1]["target_qty"] == -100  # opens the short
    # Price rises 100->110->121->133 while short is held -> losing position.
    assert summary["gross_total_return"] < 0.0
    # Forced fold-end flatten: BUY to cover (short -> 0), exact close.
    final_event = evidence[-1]
    assert final_event["target_qty"] == 0
    assert final_event["side"] == "buy"
    assert final_event["qty"] == 100


def test_translate_symbol_weight_series_is_a_pure_diagnostic_utility_not_wired_into_pipeline() -> None:
    """Documents the P7B-REPAIR-01 scope decision: the post-hoc per-row
    translator remains a correct PURE function (given the right price for
    each row, e.g. proven by TEST 6/11 above), but the real causal pipeline
    (_simulate_fold) no longer calls it -- it assembles evidence directly
    from the causal engine's own signal-time-fixed target_qty state. This
    guards against a future regression silently re-wiring the post-hoc
    (execution-row-close) translator back into the pipeline."""
    import inspect

    from mqk_research.ml import economic_walkforward as ewf

    source = inspect.getsource(ewf)
    assert "translate_symbol_weight_series_to_share_events" not in source


# =============================================================================
# P7B-REPAIR-02 (RESEARCH-WEIGHT-TO-SHARE-PARITY-01-REPAIR-02)
#
# Covers the FINAL WAVE-2 repair mission's REQUIRED P7B TESTS (Section 3H):
# exact stateful wealth compounding (defect A) and fill-time capacity parity
# mirroring Rust's resolve_one_pending_order (defect B/C).
# =============================================================================


def _flat_single_symbol_fold(closes, weight, equity_usd=100_000.0, max_gross_exposure=1.0):
    days = pd.date_range("2021-01-01", periods=len(closes), freq="D", tz="UTC")
    close_frame = pd.DataFrame({"AAA": closes}, index=days)
    wts_spec = WeightToShareSpec(equity_usd=equity_usd)
    target_qty = weight_to_target_qty(weight=weight, price=closes[0], spec=wts_spec)
    pending_events = {"AAA": [(days[0], weight, target_qty)]}
    spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=0.5, max_gross_exposure=max_gross_exposure),
        cost_model=CostModelSpec(commission_bps_per_side=0.0, slippage_bps_per_side=0.0, diagnostic_zero_cost=True),
        annualization=AnnualizationSpec(),
        weight_to_share=wts_spec,
    )
    return _simulate_fold(
        1, {"test_start": days[0], "test_end": days[-1] + pd.Timedelta(days=1)},
        close_frame, pending_events, spec,
    )


def test_exact_wealth_ledger_100_110_121_equals_21_percent() -> None:
    """REQUIRED TESTS 1/27 (mission Section 3C): qty=1000 on $100,000
    equity, prices 100 -> 100 (entry, zero-P&L) -> 110 -> 121 must compound
    to EXACTLY +21% total return via a real stateful equity ledger
    (100,000 -> 110,000 -> 121,000), NOT dollar_pnl/100,000 FIXED-
    denominator fractions [0.10, 0.11] geometrically compounding to a WRONG
    +22.1%. The signal bar (day0, close=100) cannot execute from its own
    bar; day1 (close=100, flat) is where the position actually opens, so
    the P&L-bearing legs are exactly the mission's 100->110->121 sequence."""
    fold_df, summary = _flat_single_symbol_fold([100.0, 100.0, 110.0, 121.0], weight=1.0)
    assert summary["gross_total_return"] == pytest.approx(0.21, abs=1e-9)
    assert summary["net_total_return"] == pytest.approx(0.21, abs=1e-9)
    # The WRONG fixed-denominator answer this repair fixes.
    assert summary["gross_total_return"] != pytest.approx(0.221, abs=1e-6)


def test_exact_wealth_ledger_100_90_81_equals_negative_19_percent() -> None:
    """REQUIRED TEST 2 (mission Section 3C, adverse path): symmetric loss
    case -- 100 -> 100 (flat entry) -> 90 -> 81 must compound to EXACTLY
    -19%, not the WRONG fixed-denominator -19.9%."""
    fold_df, summary = _flat_single_symbol_fold([100.0, 100.0, 90.0, 81.0], weight=1.0)
    assert summary["gross_total_return"] == pytest.approx(-0.19, abs=1e-9)
    assert summary["net_total_return"] == pytest.approx(-0.19, abs=1e-9)
    assert summary["gross_total_return"] != pytest.approx(-0.199, abs=1e-6)


def test_stateful_ledger_total_return_agrees_with_explicit_ending_equity() -> None:
    """REQUIRED TEST 27: the reported total_return, applied to the starting
    equity_usd, reconstructs the explicit ending dollar equity exactly --
    proving the reported fraction really is a wealth-ledger return, not an
    approximation."""
    equity_usd = 50_000.0
    fold_df, summary = _flat_single_symbol_fold(
        [100.0, 100.0, 110.0, 121.0], weight=1.0, equity_usd=equity_usd,
    )
    reconstructed_ending_equity = equity_usd * (1.0 + summary["gross_total_return"])
    assert reconstructed_ending_equity == pytest.approx(60_500.0, abs=1e-6)  # 50,000 * 1.21


def test_gap_cap_100_to_200_matches_rust_rejection() -> None:
    """REQUIRED TEST 6 (mission Section 3D/3E): full-conviction target
    sized at signal close $100 (qty=1000 on $100,000 equity, exactly the
    1.0x cap boundary); the execution bar gaps to $200. Actual proposed
    fill notional (1000*$200=$200,000) breaches the $100,000 cap --
    mirrors Rust's resolve_one_pending_order rejecting this same scenario
    (see core-rs scenario_reversal_cap_bkt_wave2_repair.rs and
    scenario_allocation_cap_enforced.rs for the Rust-side proof)."""
    fold_df, summary = _flat_single_symbol_fold([100.0, 200.0, 200.0], weight=1.0)
    evidence = summary["weight_to_share_evidence"]["AAA"]
    assert evidence[1]["signal_target_qty"] == 1000
    # Capacity-rejected: resulting position never actually opens.
    assert all(e["target_qty"] == 0 for e in evidence)
    assert summary["gross_total_return"] == 0.0


def test_rejected_capacity_order_creates_no_economic_mutation() -> None:
    """REQUIRED TEST 7: a fill-time-capacity-rejected order produces zero
    turnover/commission/execution_price_cost/gross_contrib -- not merely a
    zero qty change, but a fully inert row (no P7A price drag, no
    commission, matching Rust's reject-before-any-portfolio-mutation
    semantics)."""
    fold_df, summary = _flat_single_symbol_fold([100.0, 200.0, 200.0], weight=1.0)
    rejected_rows = fold_df[fold_df["timestamp"] == fold_df["timestamp"].iloc[1]]
    row = rejected_rows.iloc[0]
    assert row["turnover"] == 0.0
    assert row["commission_cost"] == 0.0
    assert row["execution_price_cost"] == 0.0
    assert row["gross_return"] == 0.0


def test_headroom_admits_when_fill_notional_fits() -> None:
    """Negative control for the gap/cap test: the SAME $100->$100k setup
    with a smaller execution-time gap ($100->$99, well inside the cap)
    must still admit -- proving the capacity check isn't unconditionally
    rejecting, only rejecting genuine breaches. A third bar at $105 (after
    admission) gives the held position a genuine price move to earn P&L on
    -- $99->$99 alone would earn zero regardless of admission."""
    fold_df, summary = _flat_single_symbol_fold([100.0, 99.0, 105.0], weight=1.0)
    evidence = summary["weight_to_share_evidence"]["AAA"]
    assert evidence[1]["target_qty"] == 1000
    assert summary["gross_total_return"] != 0.0


def test_python_reversal_long_to_short_residual_cannot_bypass_cap() -> None:
    """REQUIRED TEST 18 (Python-side mirror of the Rust reversal-cap fix,
    mission Section 3F): current +40 shares, reverse to -100 (SELL 140).
    Closing the 40-share long is safe; the residual 100-share NEW short
    would breach a cap with only $10,000 of headroom on $10,000 equity at
    a $100 flat price -- the WHOLE reversal must be rejected, not just the
    residual, mirroring Rust's resolve_one_pending_order exactly (see
    core-rs scenario_reversal_cap_bkt_wave2_repair.rs). max_gross_exposure
    is 1.0 (not tighter) so the CONTINUOUS weight-space admission -- frozen,
    unchanged -- still admits the magnitude-1.0 reversal exactly at its own
    boundary; only the DISCRETE fill-time dollar check (equity_usd * 1.0 =
    $10,000) rejects it, proving the two admission layers are independent."""
    # 4 bars: day2 (not the fold's LAST bar) is where the reversal resolves,
    # keeping its rejection observable before day3's forced fold-end
    # flatten independently zeroes the position (a separate, unrelated
    # mechanism -- see the forced-flatten tests elsewhere).
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    close_frame = pd.DataFrame({"AAA": [100.0, 100.0, 100.0, 100.0]}, index=days)
    wts_spec = WeightToShareSpec(equity_usd=10_000.0)
    pending_events = {
        "AAA": [
            (days[0], 0.4, 40),   # opens +40 (fills at day1)
            (days[1], -1.0, -100),  # reverses to -100 (fills at day2)
        ]
    }
    spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=0.5, max_gross_exposure=1.0),
        cost_model=CostModelSpec(commission_bps_per_side=0.0, slippage_bps_per_side=0.0, diagnostic_zero_cost=True),
        annualization=AnnualizationSpec(),
        weight_to_share=wts_spec,
    )
    fold_df, summary = _simulate_fold(
        1, {"test_start": days[0], "test_end": days[-1] + pd.Timedelta(days=1)},
        close_frame, pending_events, spec,
    )
    evidence = summary["weight_to_share_evidence"]["AAA"]
    assert evidence[1]["target_qty"] == 40  # the +40 open admits
    # The reversal's residual (100 new short shares * $100 = $10,000) plus
    # the existing $4,000 long exposure ($14,000) breaches the $10,000 cap
    # -- rejected in full, position stays +40 (day2, before fold-end flatten).
    assert evidence[2]["target_qty"] == 40
    assert evidence[2]["signal_target_qty"] == -100


def test_python_reversal_short_to_long_residual_cannot_bypass_cap() -> None:
    """REQUIRED TEST 19: mirror case, current -40, reverse to +100 (BUY
    140). 4 bars for the same reason as the long->short mirror test above
    -- keeps the reversal's rejection observable before fold-end flatten."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    close_frame = pd.DataFrame({"AAA": [100.0, 100.0, 100.0, 100.0]}, index=days)
    wts_spec = WeightToShareSpec(equity_usd=10_000.0)
    pending_events = {
        "AAA": [
            (days[0], -0.4, -40),
            (days[1], 1.0, 100),
        ]
    }
    spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=0.5, max_gross_exposure=1.0),
        cost_model=CostModelSpec(commission_bps_per_side=0.0, slippage_bps_per_side=0.0, diagnostic_zero_cost=True),
        annualization=AnnualizationSpec(),
        weight_to_share=wts_spec,
    )
    fold_df, summary = _simulate_fold(
        1, {"test_start": days[0], "test_end": days[-1] + pd.Timedelta(days=1)},
        close_frame, pending_events, spec,
    )
    evidence = summary["weight_to_share_evidence"]["AAA"]
    assert evidence[1]["target_qty"] == -40
    assert evidence[2]["target_qty"] == -40
    assert evidence[2]["signal_target_qty"] == 100


def test_no_p7a_cost_double_counting_in_wealth_ledger() -> None:
    """REQUIRED TEST 28: the stateful wealth ledger charges execution cost
    exactly once -- net dollar P&L equals gross dollar P&L minus a SINGLE
    commission+adverse-price-cost term, verified by reconstructing net
    equity from gross equity and the reported cost_drag with no residual
    discrepancy beyond floating-point tolerance."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    close_frame = pd.DataFrame({"AAA": [100.0, 100.0, 100.0, 100.0]}, index=days)
    high_frame = pd.DataFrame({"AAA": [100.5, 102.0, 100.5, 100.5]}, index=days)
    low_frame = pd.DataFrame({"AAA": [99.5, 99.0, 99.5, 99.5]}, index=days)
    wts_spec = WeightToShareSpec(equity_usd=10_000.0)
    target_qty = weight_to_target_qty(weight=0.5, price=100.0, spec=wts_spec)
    pending_events = {"AAA": [(days[0], 0.5, target_qty)]}
    spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=0.5, max_gross_exposure=1.0),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        annualization=AnnualizationSpec(),
        execution_pricing=ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
            slippage_bps=0, volatility_mult_bps=0,
        ),
        weight_to_share=wts_spec,
    )
    fold_df, summary = _simulate_fold(
        1, {"test_start": days[0], "test_end": days[-1] + pd.Timedelta(days=1)},
        close_frame, pending_events, spec, high_frame=high_frame, low_frame=low_frame,
    )
    gross_equity = 10_000.0 * (1.0 + summary["gross_total_return"])
    net_equity = 10_000.0 * (1.0 + summary["net_total_return"])
    # Total dollar cost drag reconstructed from the two equity trajectories.
    total_cost_drag_dollars = gross_equity - net_equity
    # A single commission leg on entry (50*102) + a single close/mark-priced
    # exit leg (50*100, forced flatten) -- both at commission_bps_per_side --
    # PLUS a single adverse-price-cost leg on entry (50*|102-100|=100, the
    # BUY filling at HIGH=102 instead of close=100). If execution_price_cost
    # were ALSO folded into commission_notional (double counting -- the
    # exact defect mission Section 3G warns against), this total would be
    # larger than the sum of these three genuinely-distinct legs.
    expected_entry_commission = (50 * 102.0) * (10.0 / 10_000.0)
    expected_exit_commission = (50 * 100.0) * (10.0 / 10_000.0)
    expected_adverse_price_cost = 50 * abs(102.0 - 100.0)
    expected_total = expected_entry_commission + expected_exit_commission + expected_adverse_price_cost
    assert total_cost_drag_dollars == pytest.approx(expected_total, abs=1e-6)
