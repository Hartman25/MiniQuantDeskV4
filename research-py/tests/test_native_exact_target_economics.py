"""native_exact_target_qty_v1: the economic engine holds EXACTLY the native
absolute target quantity; it never re-sizes a direction from a weight."""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

from mqk_research.ml.economic_walkforward import (
    SIGNAL_DIRECTION_POLICY_NATIVE_EXACT_TARGET_QTY_V1,
    SIGNAL_SIZING_EXACT_NATIVE_TARGET_QTY_V1,
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
    economic_protocol_identity,
    load_oos_predictions,
    run_economic_walkforward,
)
from mqk_research.ml.execution_pricing import (
    EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    ExecutionPricingSpec,
)
from mqk_research.ml.genuine_shuffled_placebo_cli import _shuffle_oos_predictions
from mqk_research.ml.util_hash import file_record
from mqk_research.ml.weight_to_share import WeightToShareSpec

SYMBOL = "SPY"
START = pd.Timestamp("2020-01-02", tz="UTC")


def _bars_csv(path: Path, n: int = 120) -> pd.DataFrame:
    dates = pd.bdate_range(START, periods=n, tz="UTC")
    px = 100.0 + np.arange(n) * 0.1
    df = pd.DataFrame({"symbol": SYMBOL, "end_ts": [d.isoformat() for d in dates], "open": px,
                       "high": px * 1.002, "low": px * 0.998, "close": px, "volume": 1_000_000})
    df.to_csv(path, index=False)
    return df


def _spec(*, exact: bool, max_position_notional_usd=None) -> EconomicWalkForwardSpec:
    policy = (
        SignalPolicySpec(direction_policy=SIGNAL_DIRECTION_POLICY_NATIVE_EXACT_TARGET_QTY_V1,
                         sizing=SIGNAL_SIZING_EXACT_NATIVE_TARGET_QTY_V1)
        if exact else SignalPolicySpec()
    )
    return EconomicWalkForwardSpec(
        signal_policy=policy,
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        execution_pricing=ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1, slippage_bps=5, volatility_mult_bps=0),
        annualization=AnnualizationSpec(),
        weight_to_share=WeightToShareSpec(equity_usd=100_000.0, max_position_notional_usd=max_position_notional_usd),
    )


def _run(tmp: Path, oos_rows: pd.DataFrame, spec: EconomicWalkForwardSpec, n: int = 120) -> dict:
    bars = tmp / "bars.csv"
    _bars_csv(bars, n)
    eval_dir = tmp / "eval"
    eval_dir.mkdir()
    oos = eval_dir / "oos.csv"
    oos_rows.to_csv(oos, index=False)
    dates = pd.bdate_range(START, periods=n, tz="UTC")
    wf = eval_dir / "wf.json"
    wf.write_text(json.dumps({
        "schema_version": "walk_forward_eval_v2",
        "holdout": {"status": "reserved_not_evaluated"},
        "folds": [{"fold": 1, "test_start_utc": dates[0].isoformat(),
                   "test_end_utc": (dates[-1] + pd.Timedelta(days=1)).isoformat(), "skipped": False}],
    }), encoding="utf-8")
    out = run_economic_walkforward(tmp, bars_csv=bars, spec=spec, walk_forward_eval_path=wf, oos_predictions_path=oos)
    return json.loads(out.read_text(encoding="utf-8"))


def _oos(targets: list[int], n: int = 120) -> pd.DataFrame:
    dates = pd.bdate_range(START, periods=n, tz="UTC")
    qty = (targets + [0] * n)[:n]
    return pd.DataFrame({"fold": 1, "symbol": SYMBOL, "decision_ts": [d.isoformat() for d in dates],
                         "ml_score": [1.0 if q > 0 else 0.0 for q in qty], "target_qty": qty})


def _held(econ: dict) -> list[int]:
    return [e["target_qty"] for e in econ["folds"][0]["weight_to_share_evidence"][SYMBOL]]


def test_exact_target_holds_the_native_quantity_while_the_old_policy_resizes_it(tmp_path):
    targets = [0] * 10 + [1] * 30 + [0] * 20
    (tmp_path / "exact").mkdir()
    exact = _run(tmp_path / "exact", _oos(targets), _spec(exact=True))
    assert set(_held(exact)) == {0, 1}, "+1 share target -> +1 share"

    legacy_oos = _oos(targets).drop(columns=["target_qty"])
    (tmp_path / "old").mkdir()
    # The superseded campaigns' sizing: equity 100k with a 50k notional cap.
    old = _run(tmp_path / "old", legacy_oos, _spec(exact=False, max_position_notional_usd=50_000.0))
    assert max(_held(old)) > 400, "negative control: the binary-weight path held hundreds of shares"


def test_three_share_target_is_held_exactly(tmp_path):
    (tmp_path / "t").mkdir()
    econ = _run(tmp_path / "t", _oos([0] * 5 + [3] * 20 + [1] * 10), _spec(exact=True))
    assert set(_held(econ)) == {0, 1, 3}


def test_ml_score_is_ignored_and_target_qty_alone_drives_the_result(tmp_path):
    targets = [0] * 5 + [2] * 30
    (tmp_path / "a").mkdir(), (tmp_path / "b").mkdir(), (tmp_path / "c").mkdir()
    base = _run(tmp_path / "a", _oos(targets), _spec(exact=True))
    scrambled = _oos(targets)
    scrambled["ml_score"] = 0.123
    same = _run(tmp_path / "b", scrambled, _spec(exact=True))
    assert same["aggregate"] == base["aggregate"], "ml_score must not influence an exact-target evaluation"
    other = _run(tmp_path / "c", _oos([0] * 5 + [3] * 30), _spec(exact=True))
    assert other["aggregate"] != base["aggregate"], "the exact quantity does"


def test_identity_is_distinct_from_the_binary_weight_policy_and_legacy_identity_is_unchanged():
    exact = economic_protocol_identity(_spec(exact=True))
    legacy = economic_protocol_identity(_spec(exact=False))
    assert exact["signal_policy"]["direction_policy"] == SIGNAL_DIRECTION_POLICY_NATIVE_EXACT_TARGET_QTY_V1
    assert exact["signal_policy"]["sizing"] == SIGNAL_SIZING_EXACT_NATIVE_TARGET_QTY_V1
    assert "direction_policy" not in legacy["signal_policy"], "legacy identity stays byte-identical"
    assert exact != legacy


def test_exact_policy_fails_closed_without_the_target_qty_column(tmp_path):
    (tmp_path / "x").mkdir()
    with pytest.raises(RuntimeError, match="exact `target_qty` column"):
        _run(tmp_path / "x", _oos([0, 1, 1]).drop(columns=["target_qty"]), _spec(exact=True))


def test_other_policies_refuse_a_stray_target_qty_column(tmp_path):
    (tmp_path / "x").mkdir()
    with pytest.raises(RuntimeError, match="only valid under"):
        _run(tmp_path / "x", _oos([0, 1, 1]), _spec(exact=False))


@pytest.mark.parametrize("bad", [-1, 1.5])
def test_negative_or_fractional_targets_are_refused(tmp_path, bad):
    df = _oos([0, 1, 1]).astype({"target_qty": "float64"})
    df.loc[2, "target_qty"] = bad
    path = tmp_path / "oos.csv"
    df.to_csv(path, index=False)
    with pytest.raises(RuntimeError, match="target_qty must be"):
        load_oos_predictions(path)


def test_an_unfundable_target_is_refused_not_resized(tmp_path):
    (tmp_path / "x").mkdir()
    with pytest.raises(RuntimeError, match="allocation capacity"):
        _run(tmp_path / "x", _oos([0] * 5 + [5000] * 10), _spec(exact=True))
    (tmp_path / "y").mkdir()
    with pytest.raises(RuntimeError, match="max_position_notional_usd"):
        _run(tmp_path / "y", _oos([0] * 5 + [300] * 10), _spec(exact=True, max_position_notional_usd=20_000.0))


def test_exact_policy_validation():
    with pytest.raises(ValueError, match="long_only=True"):
        SignalPolicySpec(direction_policy=SIGNAL_DIRECTION_POLICY_NATIVE_EXACT_TARGET_QTY_V1,
                         sizing=SIGNAL_SIZING_EXACT_NATIVE_TARGET_QTY_V1, long_only=False).normalized()
    with pytest.raises(ValueError, match="requires sizing"):
        SignalPolicySpec(direction_policy=SIGNAL_DIRECTION_POLICY_NATIVE_EXACT_TARGET_QTY_V1).normalized()
    with pytest.raises(ValueError, match="entry_threshold=0.5"):
        SignalPolicySpec(direction_policy=SIGNAL_DIRECTION_POLICY_NATIVE_EXACT_TARGET_QTY_V1,
                         sizing=SIGNAL_SIZING_EXACT_NATIVE_TARGET_QTY_V1, entry_threshold=0.7).normalized()


def test_placebo_shuffle_permutes_target_qty_with_the_scores(tmp_path):
    df = _oos([0] * 10 + [1] * 10 + [2] * 10 + [0] * 90)
    src = tmp_path / "oos.csv"
    df.to_csv(src, index=False)
    out = tmp_path / "shuffled.csv"
    _shuffle_oos_predictions(src, "trial-x", out, is_rank=False)
    shuffled = pd.read_csv(out)
    assert sorted(shuffled["target_qty"]) == sorted(df["target_qty"]), "same multiset of targets"
    assert not np.array_equal(shuffled["target_qty"].to_numpy(), df["target_qty"].to_numpy()), "targets moved"
    assert ((shuffled["ml_score"] > 0) == (shuffled["target_qty"] > 0)).all(), "pairs stay together"
