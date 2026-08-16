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
    economic_protocol_identity,
    run_economic_walkforward,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec
from mqk_research.ml.execution_pricing import (
    EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    ExecutionPricingSpec,
)
from mqk_research.ml.weight_to_share import (
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


def test_negative_weight_rejected() -> None:
    """Fail-closed guard: economic_walk_forward_v1 is long-only; a negative
    weight reaching this layer indicates an upstream invariant violation."""
    spec = WeightToShareSpec(equity_usd=1_000.0)
    with pytest.raises(ValueError, match="negative weight rejected"):
        weight_to_target_qty(weight=-0.1, price=100.0, spec=spec)


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
