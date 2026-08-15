"""
RESEARCH-ECONOMIC-WALKFORWARD-01 — economic walk-forward evaluation tests.

Two layers:
  * Direct-fixture tests (hand-crafted walk_forward_eval.json +
    walk_forward_oos_predictions.csv + bars.csv) for exact, hand-computable
    causal-execution / cost / metric proofs.
  * Full-pipeline tests (real run_walkforward_eval + run_economic_walkforward
    + registry) for OOS-only provenance, fwd_ret non-substitution, and trial
    identity behavior.

No subprocess, no broker, no OMS, no network, no DB other than local SQLite.
"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict, List

import numpy as np
import pandas as pd
import pytest

from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.ml import economics
from mqk_research.ml.economic_registry_integration import (
    build_economic_trial_identity,
    run_registered_economic_walkforward_eval,
)
from mqk_research.ml.economic_walkforward import (
    CAPACITY_POLICY_ID,
    PROTOCOL_ID,
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
    economic_protocol_identity,
    load_bars,
    load_oos_predictions,
    run_economic_walkforward,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec, run_walkforward_eval
from mqk_research.ml.schema import generate_feature_schema


def _ts(s: str) -> pd.Timestamp:
    return pd.Timestamp(s, tz="UTC")


DEFAULT_COST = CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=5.0)
DEFAULT_SIGNAL = SignalPolicySpec(entry_threshold=0.5)
DEFAULT_ANN = AnnualizationSpec()


def _spec(**overrides: Any) -> EconomicWalkForwardSpec:
    return EconomicWalkForwardSpec(
        signal_policy=overrides.get("signal_policy", DEFAULT_SIGNAL),
        cost_model=overrides.get("cost_model", DEFAULT_COST),
        annualization=overrides.get("annualization", DEFAULT_ANN),
    )


# ---------------------------------------------------------------------------
# Direct-fixture helpers: hand-craft walk_forward_eval.json + OOS predictions
# + bars.csv so economic results are exactly hand-computable, independent of
# the classification layer.
# ---------------------------------------------------------------------------

def _single_fold(test_start: str, test_end: str, fold: int = 1) -> Dict[str, Any]:
    return {
        "fold": fold,
        "skipped": False,
        "test_start_utc": _ts(test_start).isoformat(),
        "test_end_utc": _ts(test_end).isoformat(),
    }


def _oos_row(fold: int, symbol: str, decision_ts, score: float, target: int = 1, label_end_ts=None) -> Dict[str, Any]:
    return {
        "fold": fold,
        "symbol": symbol,
        "decision_ts": _ts(str(decision_ts)).isoformat(),
        "label_end_ts": _ts(str(label_end_ts or decision_ts)).isoformat(),
        "ml_score": score,
        "target": target,
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


# ---------------------------------------------------------------------------
# TESTS 19-23 — pure metric-formula unit tests (economics.py)
# ---------------------------------------------------------------------------

def test_compounded_total_return_is_not_simple_sum():
    """TEST 21"""
    returns = pd.Series([0.10, -0.10, 0.10])
    expected = (1.10 * 0.90 * 1.10) - 1.0
    assert economics.compute_compounded_total_return(returns) == pytest.approx(expected)
    assert economics.compute_compounded_total_return(returns) != pytest.approx(sum(returns))


def test_max_drawdown_known_series():
    """TEST 19"""
    returns = pd.Series([0.10, -0.20, 0.05, 0.30])
    equity = (1.0 + returns).cumprod()
    # equity: 1.10, 0.88, 0.924, 1.2012
    running_peak = equity.cummax()
    expected_dd = float((equity / running_peak - 1.0).min())
    assert expected_dd == pytest.approx(-0.20)
    assert economics.compute_max_drawdown(returns) == pytest.approx(expected_dd)


def test_sharpe_known_series_exact_formula():
    """TEST 20"""
    returns = pd.Series([0.01, 0.02, -0.01, 0.015, 0.0])
    rf_annual = 0.0
    ann_days = 252
    excess = returns - (rf_annual / ann_days)
    expected = np.sqrt(ann_days) * excess.mean() / excess.std(ddof=0)
    got = economics.compute_sharpe(returns, risk_free_rate_annual=rf_annual, annualization_days=ann_days)
    assert got == pytest.approx(expected)


def test_sharpe_zero_volatility_returns_none_not_zero_or_nan():
    returns = pd.Series([0.01, 0.01, 0.01, 0.01])
    assert economics.compute_sharpe(returns) is None


def test_annualized_return_uses_actual_trading_days_not_row_count():
    total_return = 0.10
    trading_days = 63  # ~ a quarter
    expected = (1.10) ** (252.0 / 63.0) - 1.0
    assert economics.compute_annualized_return(total_return, trading_days, 252) == pytest.approx(expected)


def test_annualized_return_fails_closed_on_total_wipeout():
    with pytest.raises(RuntimeError, match="[Ff]ail-closed"):
        economics.compute_annualized_return(-1.5, 10, 252)


def test_cost_drag_exact_definition():
    """TEST 22"""
    assert economics.compute_cost_drag(0.20, 0.15) == pytest.approx(0.05)


def test_transaction_cost_only_on_turnover():
    """TEST 9 (pure form)"""
    turnover = pd.Series([0.0, 0.0, 0.5, 0.0])
    cost = economics.compute_transaction_cost(turnover, one_way_cost_bps=100.0)  # 1%
    assert cost.tolist() == pytest.approx([0.0, 0.0, 0.005, 0.0])


def test_impossible_return_fails_closed():
    """Fail-closed condition #15"""
    with pytest.raises(RuntimeError, match="[Ff]ail-closed"):
        economics.validate_return_series(pd.Series([0.0, -1.5]))


# ---------------------------------------------------------------------------
# TEST 2 — SAME-BAR P&L FORBIDDEN (with an embedded negative control)
# ---------------------------------------------------------------------------

def test_same_bar_execution_forbidden(tmp_path):
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=7, freq="D", tz="UTC")
    closes = [100.0, 200.0, 200.0, 200.0, 200.0, 200.0, 200.0]  # jump is T0->T1
    bars_rows = [_bar_row("AAA", d, c) for d, c in zip(days, closes)]
    oos_rows = [_oos_row(1, "AAA", d, 1.0) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-08")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    out_path = run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    out = json.loads(out_path.read_text(encoding="utf-8"))
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")

    # Correct (2-bar-lag causal) model: the T0->T1 100% jump must contribute
    # ZERO gross return anywhere in the series.
    assert returns["gross_return"].abs().max() < 1e-9
    assert out["aggregate"]["gross_total_return"] == pytest.approx(0.0, abs=1e-9)

    # Negative control: an (incorrect) same-bar / 1-bar-lag model WOULD show
    # the jump. Recomputed independently here (not via the implementation)
    # to prove the fixture actually exercises the forbidden path.
    close_series = pd.Series(closes, index=days)
    naive_returns = close_series.pct_change().fillna(0.0)
    naive_weight_shift1 = pd.Series([1.0] * 7, index=days).shift(1).fillna(0.0)
    naive_gross = (naive_weight_shift1 * naive_returns)
    assert naive_gross.max() == pytest.approx(1.0)  # the forbidden 100% same-adjacent-bar earn


def test_target_symbol_execution_exactness(tmp_path):
    """TEST 4 — SPY's price magnitude must never affect AAPL's economics."""
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=10, freq="D", tz="UTC")
    aapl_closes = [100.0 + i for i in range(10)]
    spy_closes_baseline = [300.0] * 10
    aapl_scores = [1.0 if i % 3 == 0 else 0.0 for i in range(10)]
    spy_scores = [0.0] * 10  # SPY stays flat/undesired throughout in both runs

    def _run(spy_closes):
        bars_rows = [_bar_row("AAPL", d, c) for d, c in zip(days, aapl_closes)]
        bars_rows += [_bar_row("SPY", d, c) for d, c in zip(days, spy_closes)]
        oos_rows = [_oos_row(1, "AAPL", d, s) for d, s in zip(days, aapl_scores)]
        oos_rows += [_oos_row(1, "SPY", d, s) for d, s in zip(days, spy_scores)]
        folds = [_single_fold("2021-01-01", "2021-01-11")]
        bars_path = _write_direct_fixture(run_dir / str(spy_closes[0] == 300.0), folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)
        out_path = run_economic_walkforward(run_dir / str(spy_closes[0] == 300.0), bars_csv=bars_path, spec=_spec())
        returns = pd.read_csv((run_dir / str(spy_closes[0] == 300.0)) / "eval" / "economic_returns.csv")
        return returns

    baseline = _run(spy_closes_baseline)
    spy_shock = [300.0 * (1.0 + 5.0 * (i % 2)) for i in range(10)]  # huge SPY moves
    shocked = _run(spy_shock)

    aapl_baseline = baseline[baseline.index.isin(range(len(baseline)))]  # placeholder no-op
    # Compare only AAPL's own weight/return columns are unaffected: since our
    # schema is portfolio-level (not per-symbol columns), assert full gross/
    # net/turnover columns are identical because SPY never becomes active
    # (score always 0 => weight always 0 => never contributes and never
    # changes sizing denominator either since AAPL is always the sole active
    # name whenever active).
    pd.testing.assert_frame_equal(baseline, shocked)


# ---------------------------------------------------------------------------
# TEST 5 — SAME-TIMESTAMP ROW-ORDER INVARIANCE
# ---------------------------------------------------------------------------

def test_row_order_invariance(tmp_path):
    days = pd.date_range("2021-01-01", periods=10, freq="D", tz="UTC")
    bars_rows = []
    oos_rows = []
    for i, d in enumerate(days):
        bars_rows.append(_bar_row("AAA", d, 100.0 + i))
        bars_rows.append(_bar_row("BBB", d, 50.0 - 0.5 * i))
        bars_rows.append(_bar_row("CCC", d, 20.0 + 0.1 * i))
        oos_rows.append(_oos_row(1, "AAA", d, 1.0 if i % 2 == 0 else 0.0))
        oos_rows.append(_oos_row(1, "BBB", d, 1.0 if i % 3 == 0 else 0.0))
        oos_rows.append(_oos_row(1, "CCC", d, 0.6))
    folds = [_single_fold("2021-01-01", "2021-01-11")]

    run_a = tmp_path / "ordered"
    bars_path_a = _write_direct_fixture(run_a, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)
    out_a = run_economic_walkforward(run_a, bars_csv=bars_path_a, spec=_spec())

    rng = np.random.default_rng(7)
    shuffled_bars = list(bars_rows)
    rng.shuffle(shuffled_bars)
    shuffled_oos = list(oos_rows)
    rng.shuffle(shuffled_oos)

    run_b = tmp_path / "shuffled"
    bars_path_b = _write_direct_fixture(run_b, folds=folds, oos_rows=shuffled_oos, bars_rows=shuffled_bars)
    out_b = run_economic_walkforward(run_b, bars_csv=bars_path_b, spec=_spec())

    ret_a = pd.read_csv(run_a / "eval" / "economic_returns.csv")
    ret_b = pd.read_csv(run_b / "eval" / "economic_returns.csv")
    pd.testing.assert_frame_equal(ret_a, ret_b)

    obj_a = json.loads(out_a.read_text(encoding="utf-8"))
    obj_b = json.loads(out_b.read_text(encoding="utf-8"))
    assert obj_a["aggregate"] == obj_b["aggregate"]
    assert obj_a["folds"] == obj_b["folds"]


# ---------------------------------------------------------------------------
# TEST 6 — DUPLICATE BAR FAIL CLOSED
# ---------------------------------------------------------------------------

def test_duplicate_bar_fails_closed(tmp_path):
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=5, freq="D", tz="UTC")
    bars_rows = [_bar_row("AAA", d, 100.0 + i) for i, d in enumerate(days)]
    bars_rows.append(_bar_row("AAA", days[2], 999.0))  # duplicate (symbol,end_ts), different price
    oos_rows = [_oos_row(1, "AAA", d, 1.0) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-06")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    with pytest.raises(RuntimeError, match="[Ff]ail-closed"):
        run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())


def test_duplicate_oos_prediction_fails_closed(tmp_path):
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=5, freq="D", tz="UTC")
    bars_rows = [_bar_row("AAA", d, 100.0 + i) for i, d in enumerate(days)]
    oos_rows = [_oos_row(1, "AAA", d, 1.0) for d in days]
    oos_rows.append(_oos_row(1, "AAA", days[1], 0.0))  # duplicate (fold,symbol,decision_ts)
    folds = [_single_fold("2021-01-01", "2021-01-06")]
    eval_dir = run_dir / "eval"
    eval_dir.mkdir(parents=True, exist_ok=True)
    (eval_dir / "walk_forward_eval.json").write_text(json.dumps({"folds": folds}), encoding="utf-8")
    pd.DataFrame(oos_rows).to_csv(eval_dir / "walk_forward_oos_predictions.csv", index=False)
    bars_path = run_dir / "bars.csv"
    pd.DataFrame(bars_rows).to_csv(bars_path, index=False)

    with pytest.raises(RuntimeError, match="[Ff]ail-closed"):
        run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())


# ---------------------------------------------------------------------------
# TEST 7 — THRESHOLD inclusion contract
# ---------------------------------------------------------------------------

def test_threshold_inclusion_contract(tmp_path):
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=6, freq="D", tz="UTC")
    bars_rows = [_bar_row("AAA", d, 100.0) for d in days]
    scores = [0.49, 0.5, 0.51, 0.5, 0.5, 0.5]
    oos_rows = [_oos_row(1, "AAA", d, s) for d, s in zip(days, scores)]
    folds = [_single_fold("2021-01-01", "2021-01-07")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    out_path = run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")
    # score idx0=0.49(<thr) -> active_state stays flat; idx1=0.5(>=thr) is the
    # first score to cross the threshold -> a pending long event is issued at
    # idx1 ("signal_ts"=idx1). Per the REPAIR-01 causal contract, that event
    # executes at THIS symbol's first later bar (idx2) -> active_positions
    # first shows a position at idx2, not idx3 (the old, buggy 2-bar-row
    # shift model delayed both execution AND cost to idx3). force_flat_last_bar
    # then flattens the position at idx5 (the fold's last bar), so idx5 is
    # active again False (post-trade) even though it still earns/holds
    # through the start of that same row.
    expected_active = [False, False, True, True, True, False]
    assert (returns["active_positions"] > 0).tolist() == expected_active


# ---------------------------------------------------------------------------
# TEST 8 — EXPOSURE CAP
# ---------------------------------------------------------------------------

def test_exposure_cap_respected(tmp_path):
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=6, freq="D", tz="UTC")
    symbols = ["AAA", "BBB", "CCC", "DDD"]
    bars_rows = []
    oos_rows = []
    for sym in symbols:
        for d in days:
            bars_rows.append(_bar_row(sym, d, 100.0))
            oos_rows.append(_oos_row(1, sym, d, 0.9))  # all always long
    folds = [_single_fold("2021-01-01", "2021-01-07")]
    max_gross = 0.6
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    spec = _spec(signal_policy=SignalPolicySpec(entry_threshold=0.5, max_gross_exposure=max_gross))
    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=spec)
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")
    assert returns["gross_exposure"].max() <= max_gross + 1e-9
    active_rows = returns[returns["active_positions"] > 0]
    assert (active_rows["gross_exposure"] - max_gross).abs().max() < 1e-9


# ---------------------------------------------------------------------------
# TESTS 9-12 — cost mechanics through the full pipeline
# ---------------------------------------------------------------------------

def _entry_exit_fixture(run_dir: Path) -> Path:
    days = pd.date_range("2021-01-01", periods=10, freq="D", tz="UTC")
    closes = [100.0] * 10  # flat prices: isolates cost effects from price P&L
    bars_rows = [_bar_row("AAA", d, c) for d, c in zip(days, closes)]
    # long from day0 through day4, flat afterward -> one entry, one exit
    scores = [1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    oos_rows = [_oos_row(1, "AAA", d, s) for d, s in zip(days, scores)]
    folds = [_single_fold("2021-01-01", "2021-01-11")]
    return _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)


def test_zero_cost_gross_equals_net(tmp_path):
    """TEST 10"""
    run_dir = tmp_path / "run"
    bars_path = _entry_exit_fixture(run_dir)
    zero_cost = CostModelSpec(commission_bps_per_side=0.0, slippage_bps_per_side=0.0, diagnostic_zero_cost=True)
    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec(cost_model=zero_cost))
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")
    assert (returns["gross_return"] - returns["net_return"]).abs().max() < 1e-12
    assert returns["transaction_cost"].abs().max() < 1e-12


def test_registered_zero_cost_without_flag_fails_closed():
    with pytest.raises(RuntimeError, match="[Ff]ail-closed"):
        CostModelSpec(commission_bps_per_side=0.0, slippage_bps_per_side=0.0).normalized()


def test_positive_cost_net_below_gross_exact_arithmetic(tmp_path):
    """TEST 11/12 — entry + exit both cost, exact turnover*rate arithmetic."""
    run_dir = tmp_path / "run"
    bars_path = _entry_exit_fixture(run_dir)
    cost = CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=5.0)  # 15bps one-way
    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec(cost_model=cost))
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")

    rate = 15.0 / 10_000.0
    expected_cost = returns["turnover"] * rate
    assert (returns["transaction_cost"] - expected_cost).abs().max() < 1e-12
    assert (returns["net_return"] <= returns["gross_return"] + 1e-12).all()

    # Two distinct one-way turnover events: entry (0->1.0) and exit (1.0->0).
    nonzero_turnover_rows = returns[returns["turnover"] > 1e-9]
    assert len(nonzero_turnover_rows) >= 2
    assert (nonzero_turnover_rows["turnover"] - 1.0).abs().max() < 1e-9


def test_no_trade_interval_has_zero_cost(tmp_path):
    """TEST 9"""
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=8, freq="D", tz="UTC")
    bars_rows = [_bar_row("AAA", d, 100.0) for d in days]
    scores = [1.0] * 8  # constant, never changes -> only entry + forced-exit cost
    oos_rows = [_oos_row(1, "AAA", d, s) for d, s in zip(days, scores)]
    folds = [_single_fold("2021-01-01", "2021-01-09")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")
    middle_rows = returns.iloc[3:-1]  # excludes the entry transition and the forced-exit last row
    assert (middle_rows["turnover"].abs() < 1e-12).all()
    assert (middle_rows["transaction_cost"].abs() < 1e-12).all()


# ---------------------------------------------------------------------------
# TESTS 13-14 — fold isolation (start flat, no future-data leak at fold end)
# ---------------------------------------------------------------------------

def test_fold_start_flat_no_cross_fold_leak(tmp_path):
    """TEST 13"""
    run_dir = tmp_path / "run"
    days1 = pd.date_range("2021-01-01", periods=6, freq="D", tz="UTC")
    days2 = pd.date_range("2021-02-01", periods=6, freq="D", tz="UTC")
    bars_rows = [_bar_row("AAA", d, 100.0) for d in list(days1) + list(days2)]
    oos_rows = [_oos_row(1, "AAA", d, 1.0) for d in days1]  # fold 1: always long
    oos_rows += [_oos_row(2, "AAA", d, 1.0) for d in days2]  # fold 2: also always long, independently
    folds = [
        _single_fold("2021-01-01", "2021-01-07", fold=1),
        _single_fold("2021-02-01", "2021-02-07", fold=2),
    ]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")
    fold1 = returns[returns["fold"] == 1].reset_index(drop=True)
    fold2 = returns[returns["fold"] == 2].reset_index(drop=True)
    # both folds start with realized_weight/turnover/active_positions identical
    # in shape (each fold re-establishes its own position from flat)
    assert fold1["active_positions"].tolist() == fold2["active_positions"].tolist()
    assert fold1["turnover"].tolist() == pytest.approx(fold2["turnover"].tolist())


def test_fold_end_cannot_use_future_data(tmp_path):
    """TEST 14"""
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=6, freq="D", tz="UTC")
    after_fold_days = pd.date_range("2021-01-10", periods=3, freq="D", tz="UTC")

    def _run(post_fold_close: float):
        bars_rows = [_bar_row("AAA", d, 100.0) for d in days]
        bars_rows += [_bar_row("AAA", d, post_fold_close) for d in after_fold_days]
        oos_rows = [_oos_row(1, "AAA", d, 1.0) for d in days]
        folds = [_single_fold("2021-01-01", "2021-01-07")]
        sub = run_dir / str(post_fold_close)
        bars_path = _write_direct_fixture(sub, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)
        out_path = run_economic_walkforward(sub, bars_csv=bars_path, spec=_spec())
        return pd.read_csv(sub / "eval" / "economic_returns.csv")

    baseline = _run(100.0)
    shocked = _run(999_999.0)  # dramatic move AFTER the fold's test window
    pd.testing.assert_frame_equal(baseline, shocked)


# ---------------------------------------------------------------------------
# TEST 18 — overlapping test folds fail closed
# ---------------------------------------------------------------------------

def test_overlapping_test_folds_fail_closed(tmp_path):
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=10, freq="D", tz="UTC")
    bars_rows = [_bar_row("AAA", d, 100.0) for d in days]
    oos_rows = [_oos_row(1, "AAA", d, 1.0) for d in days[:6]]
    oos_rows += [_oos_row(2, "AAA", d, 1.0) for d in days[4:10]]
    folds = [
        _single_fold("2021-01-01", "2021-01-07", fold=1),
        _single_fold("2021-01-05", "2021-01-11", fold=2),  # overlaps fold 1
    ]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    with pytest.raises(RuntimeError, match="[Oo]verlapping"):
        run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())


# ---------------------------------------------------------------------------
# TEST 17 — stitched OOS equals deterministic concatenation of fold series
# ---------------------------------------------------------------------------

def test_stitched_oos_equals_fold_concatenation(tmp_path):
    run_dir = tmp_path / "run"
    days1 = pd.date_range("2021-01-01", periods=6, freq="D", tz="UTC")
    days2 = pd.date_range("2021-02-01", periods=6, freq="D", tz="UTC")
    bars_rows = [_bar_row("AAA", d, 100.0 + i) for i, d in enumerate(list(days1) + list(days2))]
    oos_rows = [_oos_row(1, "AAA", d, 0.9) for d in days1]
    oos_rows += [_oos_row(2, "AAA", d, 0.1) for d in days2]
    folds = [
        _single_fold("2021-01-01", "2021-01-07", fold=1),
        _single_fold("2021-02-01", "2021-02-07", fold=2),
    ]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    out_path = run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    stitched = pd.read_csv(run_dir / "eval" / "economic_returns.csv")
    assert stitched["fold"].tolist() == sorted(stitched["fold"].tolist())
    assert list(stitched[stitched["fold"] == 1]["timestamp"]) == [d.isoformat() for d in days1]
    assert list(stitched[stitched["fold"] == 2]["timestamp"]) == [d.isoformat() for d in days2]

    out = json.loads(out_path.read_text(encoding="utf-8"))
    assert out["aggregate"]["stitched_oos_rows"] == len(stitched)
    assert out["aggregate"]["folds_used"] == 2


# ---------------------------------------------------------------------------
# TEST 25 — missing future execution bar: no fabricated fill
# ---------------------------------------------------------------------------

def test_signal_on_final_bar_produces_no_fabricated_fill(tmp_path):
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    bars_rows = [_bar_row("AAA", d, 100.0) for d in days]
    scores = [0.0, 0.0, 0.0, 1.0]  # signal only on the LAST bar of the fold
    oos_rows = [_oos_row(1, "AAA", d, s) for d, s in zip(days, scores)]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")
    # A signal on the final bar can never be executed within this fold (needs
    # 2 more bars that don't exist) -> no position ever becomes active.
    assert (returns["active_positions"] == 0).all()
    assert returns["gross_return"].abs().max() < 1e-12


# ---------------------------------------------------------------------------
# RESEARCH-ECONOMIC-WALKFORWARD-01-REPAIR-01
#
# BLOCKER 1 — execution turnover/cost must land on the EXECUTION bar (T+1),
# distinct from the return-interval bar (T+2) that first earns P&L from it,
# and net_return must use the exact (1-cost)*(1+gross)-1 wealth-compounding
# formula rather than the gross-cost approximation.
#
# BLOCKER 2 — the first LATER bar used for a symbol's execution must be that
# EXACT symbol's own next bar, never a sibling symbol's bar and never a
# fixed row-count offset over the union timeline.
# ---------------------------------------------------------------------------


def test_repair01_entry_cost_at_execution_bar_not_return_bar(tmp_path):
    """REPAIR-01 TEST 1 — entry cost lands on the EXECUTION bar (T1), not
    the bar where the resulting return is later earned (T2)."""
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=3, freq="D", tz="UTC")  # T0, T1, T2
    closes = [100.0, 100.0, 110.0]  # flat T0->T1 isolates entry cost; +10% T1->T2
    bars_rows = [_bar_row("AAA", d, c) for d, c in zip(days, closes)]
    oos_rows = [_oos_row(1, "AAA", d, 1.0) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-04")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")
    assert len(returns) == 3
    t0_row, t1_row, t2_row = (returns.iloc[i] for i in range(3))

    # T0: signal known, nothing executes, nothing earned.
    assert t0_row["gross_return"] == pytest.approx(0.0, abs=1e-12)
    assert t0_row["turnover"] == pytest.approx(0.0, abs=1e-12)
    # T1: first LATER bar -> entry executes HERE. Zero return earned (no
    # exposure existed over the T0->T1 interval); full entry turnover/cost.
    assert t1_row["gross_return"] == pytest.approx(0.0, abs=1e-12)
    assert t1_row["turnover"] == pytest.approx(1.0)
    assert t1_row["transaction_cost"] == pytest.approx(1.0 * DEFAULT_COST.one_way_cost_bps / 10_000.0)
    # T2: first bar to earn a return (T1->T2, +10%), plus the fold-end
    # forced-exit turnover charged on this same bar.
    assert t2_row["gross_return"] == pytest.approx(0.10)
    assert t2_row["turnover"] == pytest.approx(1.0)  # exit only — entry turnover was already spent at T1


def test_repair01_entry_cost_not_delayed_across_calendar_dates(tmp_path):
    """REPAIR-01 TEST 2 — with T1/T2 on different UTC calendar dates, entry
    cost is attributed to T1's own date, not delayed onto T2's date (which
    would distort daily Sharpe/drawdown by misdating the cost)."""
    run_dir = tmp_path / "run"
    t0 = _ts("2021-01-01T10:00:00")
    t1 = _ts("2021-01-01T20:00:00")  # same calendar date as t0
    t2 = _ts("2021-01-02T04:00:00")  # DIFFERENT calendar date from t1
    closes = {t0: 100.0, t1: 100.0, t2: 110.0}
    bars_rows = [_bar_row("AAA", ts, c) for ts, c in closes.items()]
    oos_rows = [_oos_row(1, "AAA", ts, 1.0) for ts in closes]
    folds = [_single_fold("2021-01-01", "2021-01-03")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    daily = pd.read_csv(run_dir / "eval" / "economic_daily_returns.csv")
    day1 = daily[daily["date"] == "2021-01-01"].iloc[0]
    day2 = daily[daily["date"] == "2021-01-02"].iloc[0]

    entry_cost = 1.0 * DEFAULT_COST.one_way_cost_bps / 10_000.0
    assert day1["transaction_cost"] == pytest.approx(entry_cost)
    assert day1["gross_daily_return"] == pytest.approx(0.0, abs=1e-12)
    assert day2["gross_daily_return"] == pytest.approx(0.10)


def test_repair01_exact_net_compounding_not_simple_subtraction(tmp_path):
    """REPAIR-01 TEST 3 — net_return uses the exact wealth-compounding
    formula (1-cost)*(1+gross)-1, not the gross-cost approximation, when a
    real return and a cost collide on the same row (here: the final in-fold
    interval return colliding with the force-flat exit cost — REPAIR-01
    TEST 8's exit chronology)."""
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=3, freq="D", tz="UTC")
    closes = [100.0, 100.0, 110.0]
    bars_rows = [_bar_row("AAA", d, c) for d, c in zip(days, closes)]
    oos_rows = [_oos_row(1, "AAA", d, 1.0) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-04")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")
    last = returns.iloc[-1]

    gross = float(last["gross_return"])
    cost = float(last["transaction_cost"])
    assert gross == pytest.approx(0.10)  # final legal T1->T2 return IS included
    assert cost > 0.0  # exit cost IS charged on this same (last) row

    exact_expected = (1.0 - cost) * (1.0 + gross) - 1.0
    approx_wrong = gross - cost
    assert abs(exact_expected - approx_wrong) > 1e-6  # fixture actually exercises the cross term
    assert float(last["net_return"]) == pytest.approx(exact_expected, abs=1e-12)
    assert float(last["net_return"]) != pytest.approx(approx_wrong, abs=1e-9)


def test_repair01_negative_control_a_old_shift_model_misattributes_cost():
    """Negative control A (BLOCKER 1) — recompute cost the OLD, buggy way
    (turnover derived from the SAME 2-bar-shifted realized/return-interval
    weight series used for gross_return, i.e. cost keyed to the return row)
    and confirm it puts ZERO cost at T1 and the true entry cost at T2
    instead — the exact defect REPAIR-01 fixes. Proves the TEST 1 fixture
    actually exercises the bug (not just a fixture that happens to pass
    either way)."""
    weights = pd.Series([1.0, 1.0, 1.0], index=[0, 1, 2])  # desired weight decided at row 0, held
    old_realized_weight = weights.shift(2).fillna(0.0)  # OLD model: same series for exposure AND cost basis
    old_turnover = (old_realized_weight - old_realized_weight.shift(1).fillna(0.0)).abs()
    assert old_turnover.tolist() == [0.0, 0.0, 1.0]
    assert old_turnover[1] == 0.0  # OLD model: no cost at T1 at all
    assert old_turnover[2] == 1.0  # OLD model: cost misattributed to T2, the SAME row as the earned return
    # The corrected implementation puts turnover at T1 instead — see
    # test_repair01_entry_cost_at_execution_bar_not_return_bar.


def _aapl_spy_missing_bar_fixture(run_dir: Path, spy_closes: Dict[pd.Timestamp, float] | None = None) -> Path:
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")  # T0, T1, T2, T3
    t0, t1, t2, t3 = days
    aapl_closes: Dict[pd.Timestamp, float] = {t0: 100.0, t2: 120.0, t3: 132.0}  # AAPL has NO bar at t1
    spy_closes = spy_closes or {t0: 300.0, t1: 300.0, t2: 300.0, t3: 300.0}
    bars_rows = [_bar_row("AAPL", ts, c) for ts, c in aapl_closes.items()]
    bars_rows += [_bar_row("SPY", ts, c) for ts, c in spy_closes.items()]
    oos_rows = [_oos_row(1, "AAPL", ts, 1.0) for ts in aapl_closes]  # AAPL signal wherever it has a bar
    oos_rows += [_oos_row(1, "SPY", ts, 0.0) for ts in spy_closes]  # SPY always inactive (isolates AAPL)
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    return _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)


def test_repair01_missing_intermediate_symbol_bar_defers_to_own_next_bar(tmp_path):
    """REPAIR-01 TEST 4 — AAPL signal at T0 must execute at AAPL's own first
    LATER bar (T2, since T1 has no AAPL bar), never at the global T1 row,
    and must earn ZERO return for the T0->T2 gap (no fabricated fill)."""
    run_dir = tmp_path / "run"
    bars_path = _aapl_spy_missing_bar_fixture(run_dir)
    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")
    assert len(returns) == 4
    t0_row, t1_row, t2_row, t3_row = (returns.iloc[i] for i in range(4))

    assert t0_row["gross_return"] == pytest.approx(0.0, abs=1e-12)
    assert t0_row["turnover"] == pytest.approx(0.0, abs=1e-12)
    # SPY has a bar at T1 but is never active -> the whole row is a no-op.
    assert t1_row["gross_return"] == pytest.approx(0.0, abs=1e-12)
    assert t1_row["turnover"] == pytest.approx(0.0, abs=1e-12)
    # AAPL executes at T2 (its own first later bar): entry cost here, and
    # ZERO return despite the 20% AAPL move from T0's close to T2's close.
    assert t2_row["turnover"] == pytest.approx(1.0)
    assert t2_row["gross_return"] == pytest.approx(0.0, abs=1e-12)
    # First AAPL P&L is T2->T3 (+10%), plus the fold-end forced exit.
    assert t3_row["gross_return"] == pytest.approx(132.0 / 120.0 - 1.0)
    assert t3_row["turnover"] == pytest.approx(1.0)


def test_repair01_sibling_bar_shock_does_not_move_target_symbol_execution(tmp_path):
    """REPAIR-01 TEST 5 — an enormous SPY move landing exactly on the bar
    AAPL is missing must not alter AAPL's economics at all."""
    run_dir_a = tmp_path / "baseline"
    run_dir_b = tmp_path / "shocked"
    bars_path_a = _aapl_spy_missing_bar_fixture(run_dir_a)
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    shocked_spy = {days[0]: 300.0, days[1]: 300_000.0, days[2]: 300.0, days[3]: 300.0}
    bars_path_b = _aapl_spy_missing_bar_fixture(run_dir_b, spy_closes=shocked_spy)

    run_economic_walkforward(run_dir_a, bars_csv=bars_path_a, spec=_spec())
    run_economic_walkforward(run_dir_b, bars_csv=bars_path_b, spec=_spec())
    returns_a = pd.read_csv(run_dir_a / "eval" / "economic_returns.csv")
    returns_b = pd.read_csv(run_dir_b / "eval" / "economic_returns.csv")
    pd.testing.assert_frame_equal(returns_a, returns_b)


def test_repair01_negative_control_b_global_row_shift_would_fabricate_fill():
    """Negative control B (BLOCKER 2) — recompute the OLD global-row-shift
    model (weights.shift(2) over the union bar index, with close ffilled
    across the gap) directly on TEST 4's raw AAPL prices, and confirm it
    fabricates a nonzero AAPL return at T2 from the T0->T2 gap that the
    corrected per-symbol model reports as zero. Proves the TEST 4 fixture
    actually exercises BLOCKER 2."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    t0, t1, t2, t3 = days
    aapl = pd.Series([100.0, np.nan, 120.0, 132.0], index=days)  # no AAPL bar at t1
    target_weight = pd.Series([1.0, 1.0, 1.0, 1.0], index=days)  # AAPL desired long from t0 onward

    naive_returns = aapl.ffill().pct_change().fillna(0.0)
    old_realized_weight = target_weight.shift(2).fillna(0.0)
    old_gross = old_realized_weight * naive_returns

    assert old_gross[t2] == pytest.approx(120.0 / 100.0 - 1.0)
    assert old_gross[t2] != 0.0  # violates "no exposure return before execution" (BLOCKER 2)


def test_repair01_independent_symbol_future_execution_timing(tmp_path):
    """REPAIR-01 TEST 6, corrected under REPAIR-03 — two symbols signalled
    TOGETHER at the same T0 form ONE atomic equal-weight increase cohort
    (see module docstring "same decision timestamp -> atomic increase
    cohort" / SignalPolicySpec's MULTI-SYMBOL EQUAL-WEIGHT SIZING RULE).
    SPY has a bar at T1; AAPL does not (its own first later bar is T2).
    Under REPAIR-01's fully-independent-per-symbol execution (and under
    REPAIR-02's buggy present-only cohort formation, which accidentally
    reproduced the same result for this fixture) SPY would have executed
    ALONE at T1. REPAIR-03 fixes exactly this: AAPL's absence at T1 defers
    the WHOLE T0 cohort, so neither executes until T2, where both AAPL and
    SPY have actual bars and execute together."""
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    t0, t1, t2, t3 = days
    aapl_closes = {t0: 100.0, t2: 200.0, t3: 220.0}  # +100% T0->T2 must never be earned
    spy_closes = {t0: 300.0, t1: 303.0, t2: 306.0, t3: 309.0}
    bars_rows = [_bar_row("AAPL", ts, c) for ts, c in aapl_closes.items()]
    bars_rows += [_bar_row("SPY", ts, c) for ts, c in spy_closes.items()]
    oos_rows = [_oos_row(1, "AAPL", ts, 1.0) for ts in aapl_closes]
    oos_rows += [_oos_row(1, "SPY", ts, 1.0) for ts in spy_closes]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")
    t0_row, t1_row, t2_row, t3_row = (returns.iloc[i] for i in range(4))

    # T1: SPY has a bar but AAPL (its T0 cohort sibling) does not -> the
    # WHOLE cohort is deferred; SPY must NOT execute alone.
    assert t1_row["turnover"] == pytest.approx(0.0, abs=1e-12)
    assert t1_row["active_positions"] == 0
    assert t1_row["gross_return"] == pytest.approx(0.0, abs=1e-12)

    # T2: AAPL's own first later bar -> both AAPL and SPY are now present ->
    # the whole cohort fits headroom (0.5+0.5=1.0<=max_gross=1.0) and
    # executes TOGETHER, atomically. Neither had any exposure established
    # before this row, so this row's gross_return is zero (AAPL's forbidden
    # 100% T0->T2 move is not earned, and SPY earns nothing either since it
    # was never actually holding a position before T2).
    assert t2_row["turnover"] == pytest.approx(1.0)  # 0.5 AAPL + 0.5 SPY
    assert t2_row["active_positions"] == 2
    assert t2_row["gross_return"] == pytest.approx(0.0, abs=1e-12)

    # T3: both earn their own established-since-T2 return, then both force-flat.
    expected_t3_gross = 0.5 * (aapl_closes[t3] / aapl_closes[t2] - 1.0) + 0.5 * (spy_closes[t3] / spy_closes[t2] - 1.0)
    assert t3_row["gross_return"] == pytest.approx(expected_t3_gross)
    assert t3_row["turnover"] == pytest.approx(1.0)  # both exit
    assert t3_row["active_positions"] == 0


def test_repair01_final_symbol_bar_signal_never_executes_even_with_later_sibling_bars(tmp_path):
    """REPAIR-01 TEST 7 — a signal on a symbol's OWN final bar in this fold
    can never execute, even though sibling bars continue afterward (which a
    buggy "any later global timestamp" model could mistake for the target
    symbol's own next bar). Stronger than merely having no global next
    timestamp."""
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    t0, t1, t2, t3 = days
    bars_rows = [_bar_row("AAPL", t0, 100.0), _bar_row("AAPL", t1, 100.0)]  # AAPL absent from t2,t3
    bars_rows += [_bar_row("SPY", ts, 300.0) for ts in days]  # SPY continues through t2,t3
    oos_rows = [_oos_row(1, "AAPL", t0, 0.0), _oos_row(1, "AAPL", t1, 1.0)]  # signal on AAPL's OWN LAST bar
    oos_rows += [_oos_row(1, "SPY", ts, 0.0) for ts in days]  # SPY always inactive
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")
    assert (returns["active_positions"] == 0).all()
    assert returns["turnover"].abs().max() < 1e-12
    assert returns["gross_return"].abs().max() < 1e-12


def test_repair01_row_order_invariance_with_missing_symbol(tmp_path):
    """REPAIR-01 TEST 9 — permuting physical CSV row order (bars AND OOS
    predictions) must not change execution timestamps, cost timestamps, or
    the resulting gross/net series, even when a symbol is absent from some
    frames.

    All three symbols activate together at day0 (one shared equal-weight
    rebalance) and then hold — this isolates row-order invariance under a
    missing bar from a SEPARATE, not-yet-solved question this patch does
    not attempt: transient max_gross_exposure headroom while independently
    lagged symbols are mid-rebalance across each other (see module docstring
    "MULTI-SYMBOL EQUAL-WEIGHT DETAIL" — a safe narrow rule, not a
    guarantee of zero transient overshoot under adversarial churn)."""
    days = pd.date_range("2021-01-01", periods=6, freq="D", tz="UTC")
    bars_rows = []
    oos_rows = []
    for i, d in enumerate(days):
        bars_rows.append(_bar_row("AAA", d, 100.0 + i))
        oos_rows.append(_oos_row(1, "AAA", d, 1.0))
        if i != 2:  # BBB has no bar/decision on day index 2 — an irregular gap
            bars_rows.append(_bar_row("BBB", d, 50.0 - 0.3 * i))
            oos_rows.append(_oos_row(1, "BBB", d, 1.0))
        bars_rows.append(_bar_row("CCC", d, 20.0 + 0.2 * i))
        oos_rows.append(_oos_row(1, "CCC", d, 0.7))
    folds = [_single_fold("2021-01-01", "2021-01-07")]

    run_a = tmp_path / "ordered"
    bars_path_a = _write_direct_fixture(run_a, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)
    out_a = run_economic_walkforward(run_a, bars_csv=bars_path_a, spec=_spec())

    rng = np.random.default_rng(11)
    shuffled_bars = list(bars_rows)
    rng.shuffle(shuffled_bars)
    shuffled_oos = list(oos_rows)
    rng.shuffle(shuffled_oos)

    run_b = tmp_path / "shuffled"
    bars_path_b = _write_direct_fixture(run_b, folds=folds, oos_rows=shuffled_oos, bars_rows=shuffled_bars)
    out_b = run_economic_walkforward(run_b, bars_csv=bars_path_b, spec=_spec())

    ret_a = pd.read_csv(run_a / "eval" / "economic_returns.csv")
    ret_b = pd.read_csv(run_b / "eval" / "economic_returns.csv")
    pd.testing.assert_frame_equal(ret_a, ret_b)

    obj_a = json.loads(out_a.read_text(encoding="utf-8"))
    obj_b = json.loads(out_b.read_text(encoding="utf-8"))
    assert obj_a["aggregate"] == obj_b["aggregate"]
    assert obj_a["folds"] == obj_b["folds"]


def test_repair01_regular_complete_panel_reproduces_decide_execute_earn_chain(tmp_path):
    """REPAIR-01 TEST 10 — for a complete, synchronized single-symbol panel,
    the new explicit state machine reproduces: decision at T, execution at
    T+1, first earned return T+1->T+2. Only the cost TIMING (T+1, not T+2)
    and the EXACT compounding formula differ from the prior approximation —
    the qualitative causal chain is unchanged."""
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=5, freq="D", tz="UTC")  # T, T+1, T+2, T+3, T+4
    closes = [100.0, 100.0, 105.0, 105.0, 105.0]
    bars_rows = [_bar_row("AAA", d, c) for d, c in zip(days, closes)]
    oos_rows = [_oos_row(1, "AAA", d, 1.0) for d in days]  # decision at T = day0
    folds = [_single_fold("2021-01-01", "2021-01-06")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")

    assert returns.loc[0, "turnover"] == pytest.approx(0.0, abs=1e-12)  # T: nothing executes
    assert returns.loc[1, "turnover"] == pytest.approx(1.0)  # T+1: execution
    assert returns.loc[1, "gross_return"] == pytest.approx(0.0, abs=1e-12)  # T->T+1 never earned
    assert returns.loc[2, "gross_return"] == pytest.approx(105.0 / 100.0 - 1.0)  # T+1->T+2 first earned


# ---------------------------------------------------------------------------
# Bars provenance / fail-closed data-quality
# ---------------------------------------------------------------------------

def test_missing_bars_file_fails_closed(tmp_path):
    run_dir = tmp_path / "run"
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    _write_direct_fixture(run_dir, folds=folds, oos_rows=[_oos_row(1, "AAA", "2021-01-01", 1.0)], bars_rows=[_bar_row("AAA", "2021-01-01", 100.0)])
    missing = run_dir / "does_not_exist.csv"
    with pytest.raises((FileNotFoundError, RuntimeError)):
        run_economic_walkforward(run_dir, bars_csv=missing, spec=_spec())


def test_non_positive_close_fails_closed(tmp_path):
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=3, freq="D", tz="UTC")
    bars_rows = [_bar_row("AAA", days[0], 100.0), _bar_row("AAA", days[1], 0.0), _bar_row("AAA", days[2], 100.0)]
    oos_rows = [_oos_row(1, "AAA", d, 1.0) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-04")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)
    with pytest.raises(RuntimeError, match="[Ff]ail-closed"):
        load_bars(bars_path)


def test_score_outside_unit_interval_fails_closed(tmp_path):
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=3, freq="D", tz="UTC")
    bars_rows = [_bar_row("AAA", d, 100.0) for d in days]
    oos_rows = [_oos_row(1, "AAA", days[0], 1.5), _oos_row(1, "AAA", days[1], 0.5), _oos_row(1, "AAA", days[2], 0.5)]
    folds = [_single_fold("2021-01-01", "2021-01-04")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)
    with pytest.raises(RuntimeError, match="[Ff]ail-closed"):
        run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())


# ---------------------------------------------------------------------------
# Full-pipeline fixtures (real classification layer)
# ---------------------------------------------------------------------------

BASE_SPEC_KW = dict(train_years=1, test_months=1, step_months=1, holdout_months=1, min_rows_per_fold=200)


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
                # extreme, deliberately unrelated to the close-price walk below
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


def _build_flat_bars(df: pd.DataFrame) -> pd.DataFrame:
    """Bars fully DECOUPLED from `fwd_ret`/`target` — flat closes so any
    resemblance between economic results and the (deliberately extreme)
    fwd_ret label would prove leakage, not coincidence."""
    rows = []
    for (sym, ts), _ in df.groupby(["symbol", "end_ts"]):
        rows.append({"symbol": sym, "end_ts": pd.Timestamp(ts).isoformat(), "close": 100.0})
    return pd.DataFrame(rows)


def test_oos_predictions_are_test_fold_only(tmp_path):
    """TEST 1"""
    run_dir = tmp_path / "run"
    df = _build_full_dataset()
    _write_full_run_dir(run_dir, df)
    spec = WalkForwardSpec(**BASE_SPEC_KW)
    wf_path = run_walkforward_eval(run_dir, spec=spec, steps=10)
    wf = json.loads(wf_path.read_text(encoding="utf-8"))

    oos = pd.read_csv(run_dir / "eval" / "walk_forward_oos_predictions.csv")
    expected_total_test_rows = sum(f["test_rows"] for f in wf["folds"] if not f["skipped"])
    assert len(oos) == expected_total_test_rows

    holdout_start = pd.Timestamp(wf["temporal_contract"]["holdout_start_utc"])
    assert (pd.to_datetime(oos["decision_ts"], utc=True) < holdout_start).all()

    # Every OOS row's fold must be one of the used (non-skipped) folds, and
    # its decision_ts must fall within that fold's own [test_start, test_end).
    used_by_fold = {f["fold"]: (pd.Timestamp(f["test_start_utc"]), pd.Timestamp(f["test_end_utc"])) for f in wf["folds"] if not f["skipped"]}
    for fold_no, group in oos.groupby("fold"):
        assert fold_no in used_by_fold
        ts_lo, ts_hi = used_by_fold[fold_no]
        decision_ts = pd.to_datetime(group["decision_ts"], utc=True)
        assert (decision_ts >= ts_lo).all() and (decision_ts < ts_hi).all()


def test_classification_metrics_still_present(tmp_path):
    """TEST 32"""
    run_dir = tmp_path / "run"
    df = _build_full_dataset()
    _write_full_run_dir(run_dir, df)
    spec = WalkForwardSpec(**BASE_SPEC_KW)
    wf_path = run_walkforward_eval(run_dir, spec=spec, steps=10)
    wf = json.loads(wf_path.read_text(encoding="utf-8"))
    assert "auc_mean" in wf["summary"]
    assert "logloss_mean" in wf["summary"]
    used = [f for f in wf["folds"] if not f["skipped"]]
    assert all("auc" in f["metrics"] and "logloss" in f["metrics"] for f in used)


def test_economic_result_follows_bars_not_fwd_ret_label(tmp_path):
    """TEST 3 — targets.csv carries an extreme, unrelated fwd_ret; bars are
    flat. Economic result must follow the (flat) bars, not the label."""
    run_dir = tmp_path / "run"
    df = _build_full_dataset(periods_days=560)
    _write_full_run_dir(run_dir, df)
    bars_df = _build_flat_bars(df)
    bars_path = run_dir / "bars.csv"
    bars_df.to_csv(bars_path, index=False)

    spec = WalkForwardSpec(**BASE_SPEC_KW)
    wf_path = run_walkforward_eval(run_dir, spec=spec, steps=10)
    economic_spec = _spec()
    out_path = run_economic_walkforward(run_dir, bars_csv=bars_path, spec=economic_spec, walk_forward_eval_path=wf_path)
    out = json.loads(out_path.read_text(encoding="utf-8"))

    # fwd_ret in targets.csv is +-999 (huge); flat bars mean gross P&L must
    # be ~0 regardless (only transaction costs can move net_return) — proving
    # the economic result tracks bars, not the label. The economic module is
    # never given targets.csv's path at all (only walk_forward_eval_path,
    # bars_csv, and the OOS predictions derived from it) — this is a
    # structural guarantee, not just a behavioral one.
    assert abs(out["aggregate"]["gross_total_return"]) < 1e-6


def test_holdout_never_produces_economic_output(tmp_path):
    """TEST 15/16 — holdout bar values cannot change discovery economics, and
    no holdout economic fields exist."""
    run_dir_a = tmp_path / "a"
    run_dir_b = tmp_path / "b"
    df = _build_full_dataset(periods_days=560)
    _write_full_run_dir(run_dir_a, df)
    _write_full_run_dir(run_dir_b, df)

    spec = WalkForwardSpec(**BASE_SPEC_KW)
    wf_a = json.loads(run_walkforward_eval(run_dir_a, spec=spec, steps=10).read_text(encoding="utf-8"))
    holdout_start = pd.Timestamp(wf_a["temporal_contract"]["holdout_start_utc"])

    bars_baseline = _build_flat_bars(df)
    bars_shocked = bars_baseline.copy()
    mask = pd.to_datetime(bars_shocked["end_ts"], utc=True) >= holdout_start
    bars_shocked.loc[mask, "close"] = 999_999.0

    bars_path_a = run_dir_a / "bars.csv"
    bars_baseline.to_csv(bars_path_a, index=False)
    bars_path_b = run_dir_b / "bars.csv"
    bars_shocked.to_csv(bars_path_b, index=False)

    wf_path_a = run_dir_a / "eval" / "walk_forward_eval.json"
    wf_path_b = run_walkforward_eval(run_dir_b, spec=spec, steps=10)

    out_a = json.loads(run_economic_walkforward(run_dir_a, bars_csv=bars_path_a, spec=_spec(), walk_forward_eval_path=wf_path_a).read_text(encoding="utf-8"))
    out_b = json.loads(run_economic_walkforward(run_dir_b, bars_csv=bars_path_b, spec=_spec(), walk_forward_eval_path=wf_path_b).read_text(encoding="utf-8"))

    assert out_a["aggregate"] == out_b["aggregate"]
    assert out_a["folds"] == out_b["folds"]
    assert out_a["holdout"] == {"status": "reserved_not_evaluated"}
    blob = json.dumps(out_a)
    for forbidden in ("holdout_pnl", "holdout_sharpe", "holdout_returns", "holdout_predictions"):
        assert forbidden not in blob


# ---------------------------------------------------------------------------
# TESTS 26-31 — registry integration
# ---------------------------------------------------------------------------

def _registered_economic_run(run_dir, df, bars_path, *, registry_db, economic_spec=None, wf_spec=None):
    _write_full_run_dir(run_dir, df)
    return run_registered_economic_walkforward_eval(
        run_dir,
        experiment_id="econ.discovery.test",
        hypothesis_id="econ.hyp1",
        strategy_id="research.econ_v1",
        bars_csv=bars_path,
        economic_spec=economic_spec or _spec(),
        registry_db=registry_db,
        wf_spec=wf_spec or WalkForwardSpec(**BASE_SPEC_KW),
        steps=10,
    )


def test_registry_cost_change_creates_distinct_trial(tmp_path):
    """TEST 26"""
    registry_db = tmp_path / "registry.sqlite3"
    df = _build_full_dataset(periods_days=560)
    bars_df = _build_flat_bars(df)

    run_a = tmp_path / "a"
    bars_a = run_a / "bars.csv"
    run_a.mkdir(parents=True, exist_ok=True)
    bars_df.to_csv(bars_a, index=False)
    out_a = json.loads(_registered_economic_run(run_a, df, bars_a, registry_db=registry_db).read_text(encoding="utf-8"))

    run_b = tmp_path / "b"
    bars_b = run_b / "bars.csv"
    run_b.mkdir(parents=True, exist_ok=True)
    bars_df.to_csv(bars_b, index=False)
    other_cost = _spec(cost_model=CostModelSpec(commission_bps_per_side=20.0, slippage_bps_per_side=5.0))
    out_b = json.loads(_registered_economic_run(run_b, df, bars_b, registry_db=registry_db, economic_spec=other_cost).read_text(encoding="utf-8"))

    assert out_a["registry"]["trial_id"] != out_b["registry"]["trial_id"]


def test_registry_threshold_change_creates_distinct_trial(tmp_path):
    """TEST 27"""
    registry_db = tmp_path / "registry.sqlite3"
    df = _build_full_dataset(periods_days=560)
    bars_df = _build_flat_bars(df)

    run_a = tmp_path / "a"
    run_a.mkdir(parents=True, exist_ok=True)
    bars_a = run_a / "bars.csv"
    bars_df.to_csv(bars_a, index=False)
    out_a = json.loads(_registered_economic_run(run_a, df, bars_a, registry_db=registry_db).read_text(encoding="utf-8"))

    run_b = tmp_path / "b"
    run_b.mkdir(parents=True, exist_ok=True)
    bars_b = run_b / "bars.csv"
    bars_df.to_csv(bars_b, index=False)
    other_threshold = _spec(signal_policy=SignalPolicySpec(entry_threshold=0.6))
    out_b = json.loads(_registered_economic_run(run_b, df, bars_b, registry_db=registry_db, economic_spec=other_threshold).read_text(encoding="utf-8"))

    assert out_a["registry"]["trial_id"] != out_b["registry"]["trial_id"]


def test_registry_bars_change_creates_distinct_trial(tmp_path):
    """TEST 28"""
    registry_db = tmp_path / "registry.sqlite3"
    df = _build_full_dataset(periods_days=560)

    run_a = tmp_path / "a"
    run_a.mkdir(parents=True, exist_ok=True)
    bars_a = run_a / "bars.csv"
    _build_flat_bars(df).to_csv(bars_a, index=False)
    out_a = json.loads(_registered_economic_run(run_a, df, bars_a, registry_db=registry_db).read_text(encoding="utf-8"))

    run_b = tmp_path / "b"
    run_b.mkdir(parents=True, exist_ok=True)
    bars_b = run_b / "bars.csv"
    other_bars = _build_flat_bars(df)
    other_bars["close"] = 200.0  # different bar content, same shape
    other_bars.to_csv(bars_b, index=False)
    out_b = json.loads(_registered_economic_run(run_b, df, bars_b, registry_db=registry_db).read_text(encoding="utf-8"))

    assert out_a["registry"]["trial_id"] != out_b["registry"]["trial_id"]


def test_rerun_same_candidate_same_trial_new_attempt(tmp_path):
    """TEST 29"""
    registry_db = tmp_path / "registry.sqlite3"
    df = _build_full_dataset(periods_days=560)
    bars_df = _build_flat_bars(df)

    run_a = tmp_path / "a"
    run_a.mkdir(parents=True, exist_ok=True)
    bars_a = run_a / "bars.csv"
    bars_df.to_csv(bars_a, index=False)
    out_a = json.loads(_registered_economic_run(run_a, df, bars_a, registry_db=registry_db).read_text(encoding="utf-8"))

    run_b = tmp_path / "b"
    run_b.mkdir(parents=True, exist_ok=True)
    bars_b = run_b / "bars.csv"
    bars_df.to_csv(bars_b, index=False)
    out_b = json.loads(_registered_economic_run(run_b, df, bars_b, registry_db=registry_db).read_text(encoding="utf-8"))

    assert out_a["registry"]["trial_id"] == out_b["registry"]["trial_id"]
    assert out_a["registry"]["attempt_id"] != out_b["registry"]["attempt_id"]
    assert out_b["registry"]["attempt_index"] == out_a["registry"]["attempt_index"] + 1


def test_registered_attempt_stores_economic_result_no_holdout(tmp_path):
    """TEST 30"""
    registry_db = tmp_path / "registry.sqlite3"
    df = _build_full_dataset(periods_days=560)
    run_dir = tmp_path / "run"
    run_dir.mkdir(parents=True, exist_ok=True)
    bars_path = run_dir / "bars.csv"
    _build_flat_bars(df).to_csv(bars_path, index=False)

    out = json.loads(_registered_economic_run(run_dir, df, bars_path, registry_db=registry_db).read_text(encoding="utf-8"))

    store = ResearchResultStore(registry_db)
    attempts = store.list_attempts(out["registry"]["trial_id"])
    assert len(attempts) == 1
    assert attempts[0]["status"] == "succeeded"
    assert attempts[0]["result_id"] == out["ids"]["economic_eval_id"]
    summary_blob = json.dumps(attempts[0])
    assert "holdout_pnl" not in summary_blob and "holdout_returns" not in summary_blob


def test_economic_replay_determinism(tmp_path):
    """TEST 31"""
    run_dir_a = tmp_path / "a"
    run_dir_b = tmp_path / "b"
    df = _build_full_dataset(periods_days=560)
    _write_full_run_dir(run_dir_a, df)
    _write_full_run_dir(run_dir_b, df)
    bars_df = _build_flat_bars(df)
    bars_a = run_dir_a / "bars.csv"
    bars_b = run_dir_b / "bars.csv"
    bars_df.to_csv(bars_a, index=False)
    bars_df.to_csv(bars_b, index=False)

    spec = WalkForwardSpec(**BASE_SPEC_KW)
    wf_a = run_walkforward_eval(run_dir_a, spec=spec, steps=10)
    wf_b = run_walkforward_eval(run_dir_b, spec=spec, steps=10)
    out_a = run_economic_walkforward(run_dir_a, bars_csv=bars_a, spec=_spec(), walk_forward_eval_path=wf_a)
    out_b = run_economic_walkforward(run_dir_b, bars_csv=bars_b, spec=_spec(), walk_forward_eval_path=wf_b)

    obj_a = json.loads(out_a.read_text(encoding="utf-8"))
    obj_b = json.loads(out_b.read_text(encoding="utf-8"))
    obj_a["inputs"] = None
    obj_b["inputs"] = None
    obj_a["outputs"] = None
    obj_b["outputs"] = None
    obj_a["ids"] = None
    obj_b["ids"] = None
    assert json.dumps(obj_a, sort_keys=True) == json.dumps(obj_b, sort_keys=True)


# ---------------------------------------------------------------------------
# Bars provenance mismatch fail-closed
# ---------------------------------------------------------------------------

def test_bars_provenance_hash_mismatch_fails_closed(tmp_path):
    from mqk_research.shadow.label_shadow_intents import LabelSpec, label_shadow_intents

    run_dir = tmp_path / "run"
    run_dir.mkdir(parents=True, exist_ok=True)
    bars = pd.DataFrame({
        "symbol": ["AAA"] * 10,
        "end_ts": pd.date_range("2021-01-01", periods=10, freq="D", tz="UTC"),
        "close": [100.0 + i for i in range(10)],
    })
    bars_csv = run_dir / "bars.csv"
    bars.to_csv(bars_csv, index=False)
    intents = pd.DataFrame({
        "run_id": ["r1"], "symbol": ["AAA"], "decision_ts": [bars["end_ts"].iloc[2]],
        "intent": ["long"], "horizon_bars": [3],
    })
    intents_csv = run_dir / "shadow_intents.csv"
    intents.to_csv(intents_csv, index=False)
    label_shadow_intents(
        shadow_intents_csv=intents_csv, bars_csv=bars_csv,
        out_targets_csv=run_dir / "targets.csv", spec=LabelSpec(),
    )

    tampered_bars_csv = run_dir / "bars_tampered.csv"
    tampered = bars.copy()
    tampered["close"] = tampered["close"] + 1.0
    tampered.to_csv(tampered_bars_csv, index=False)

    from mqk_research.ml.economic_walkforward import verify_bars_provenance
    with pytest.raises(RuntimeError, match="[Ff]ail-closed"):
        verify_bars_provenance(run_dir, tampered_bars_csv)

    # unmodified bars file matches recorded provenance
    verify_bars_provenance(run_dir, bars_csv)


# ---------------------------------------------------------------------------
# RESEARCH-ECONOMIC-WALKFORWARD-01-REPAIR-02
#
# REPAIR-01 executed each symbol's pending change fully independently of
# every other symbol, which let an ordinary ASYNCHRONOUS multi-symbol
# rebalance (a newly-active symbol's own bar arriving before the symbol it
# is replacing has had a bar to exit on) transiently push
# sum(abs(executed_weight)) above max_gross_exposure and fail closed on it —
# correct for a genuine bug, wrong for expected asynchronous behavior.
#
# REPAIR-02 replaces this with a joint TIMESTAMP-FRAME allocator
# (capacity_policy=CAPACITY_POLICY_ID, see economic_walkforward.
# _simulate_fold_execution) that shares one portfolio-level gross-exposure
# budget: at each frame, gross-reducing changes execute first; gross-
# increasing changes are grouped into atomic same-signal_ts cohorts and
# execute only if the WHOLE cohort fits the remaining headroom, else the
# whole cohort is deferred (never a partial/alphabetical/row-order subset)
# and retried at each member's own next bar.
# ---------------------------------------------------------------------------


def _zzz_amd_msft_deferred_cohort_fixture(
    run_dir: Path, *, bars_order: List[str] | None = None, oos_order: List[str] | None = None
) -> Path:
    """Direct fixture exercising REPAIR-02's full allocator: ZZZ activates
    alone first and holds the whole cap; AMD and MSFT later activate
    TOGETHER (one shared decision frame) while ZZZ has not yet had a bar to
    execute its own (equal-weight-triggered) reduction on — so the AMD/MSFT
    cohort must be deferred IN FULL, and ZZZ's already-held weight must NOT
    be rescaled to make room. Once ZZZ's own next bar executes its
    reduction, the freed headroom lets the (still-pending) AMD/MSFT cohort
    execute together, in the SAME frame, immediately after the reduction
    (reduce-before-increase). Flat closes throughout isolate pure
    allocation/turnover behavior from price P&L.

    `bars_order`/`oos_order` optionally permute the row CONSTRUCTION order
    (by symbol) to prove the result is independent of physical CSV row
    order — see test_repair02_row_order_invariance_with_deferred_cohort.
    """
    days = pd.date_range("2021-01-01", periods=5, freq="D", tz="UTC")
    t0, t1, t2, t3, t4 = days  # t2 has no bars from any symbol at all

    per_symbol_bars = {
        "ZZZ": [(t0, 100.0), (t1, 100.0), (t4, 100.0)],
        "AMD": [(t3, 100.0), (t4, 100.0)],
        "MSFT": [(t3, 100.0), (t4, 100.0)],
    }
    per_symbol_oos = {
        "ZZZ": [(t0, 1.0)],
        "AMD": [(t2, 1.0)],
        "MSFT": [(t2, 1.0)],
    }
    bars_order = bars_order or ["ZZZ", "AMD", "MSFT"]
    oos_order = oos_order or ["ZZZ", "AMD", "MSFT"]

    bars_rows = [
        _bar_row(sym, ts, c) for sym in bars_order for ts, c in per_symbol_bars[sym]
    ]
    oos_rows = [
        _oos_row(1, sym, ts, s) for sym in oos_order for ts, s in per_symbol_oos[sym]
    ]
    folds = [_single_fold("2021-01-01", "2021-01-06")]
    max_gross = 0.6
    spec = _spec(signal_policy=SignalPolicySpec(entry_threshold=0.5, max_gross_exposure=max_gross))
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)
    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=spec)
    return bars_path


def test_repair02_reduce_first_defer_cohort_atomic_no_rescale(tmp_path):
    """REPAIR-02 TESTS 1,3,4,5,6,7,9,10 in one hand-computable fixture (see
    _zzz_amd_msft_deferred_cohort_fixture for the full scenario). Timeline:
      T0: ZZZ signalled (alone) -> target 0.6 (= max_gross_exposure).
      T1: ZZZ's own next bar -> ZZZ ENTERS (turnover 0.6).
      T2: AMD+MSFT signalled together (ZZZ, still active, is also re-sized
          by the equal-weight-active rebalance to 0.6/3=0.2, but that is a
          NEW pending change for ZZZ, not an in-place rescale of its
          currently-EXECUTED weight).
      T3: AMD's and MSFT's own first bar after T2. ZZZ has NOT had a bar
          since T1, so ZZZ's executed weight is still 0.6 (unreduced) ->
          gross is already AT the cap -> the whole AMD+MSFT cohort (0.2 each,
          same signal_ts) is DEFERRED, atomically (neither executes even
          partially) -> zero turnover, zero cost, zero gross_return
          attributable to AMD/MSFT at this row, and ZZZ's own weight is
          UNCHANGED (0.6, matching T1's value exactly -- not silently
          shrunk to make room).
      T4 (fold end): ZZZ's own next bar -> ZZZ's REDUCTION (0.6->0.2)
          executes FIRST, freeing headroom; the still-pending AMD+MSFT
          cohort (now fitting: 0.2+0.2+0.2=0.6<=0.6) executes in the SAME
          frame, immediately after -- proving reductions run before
          increases within one frame. Fold-end force-flat then closes all
          three positions.
    """
    run_dir = tmp_path / "run"
    _zzz_amd_msft_deferred_cohort_fixture(run_dir)
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv").set_index("timestamp")
    days = pd.date_range("2021-01-01", periods=5, freq="D", tz="UTC")
    t0, t1, t2, t3, t4 = days
    max_gross = 0.6

    # T2 never appears: no symbol has an actual bar there (never fabricated).
    assert list(returns.index) == [d.isoformat() for d in (t0, t1, t3, t4)]

    t0_row, t1_row, t3_row, t4_row = (returns.loc[d.isoformat()] for d in (t0, t1, t3, t4))

    assert t0_row["turnover"] == pytest.approx(0.0, abs=1e-12)

    # T1: ZZZ enters alone, using the whole cap.
    assert t1_row["turnover"] == pytest.approx(max_gross)
    assert t1_row["gross_exposure"] == pytest.approx(max_gross)
    assert t1_row["active_positions"] == 1

    # T3: AMD+MSFT cohort deferred IN FULL -- zero turnover/cost/return, and
    # ZZZ's held weight is EXACTLY what it was at T1 (never rescaled).
    assert t3_row["turnover"] == pytest.approx(0.0, abs=1e-12)
    assert t3_row["transaction_cost"] == pytest.approx(0.0, abs=1e-12)
    assert t3_row["gross_return"] == pytest.approx(0.0, abs=1e-12)
    assert t3_row["gross_exposure"] == pytest.approx(max_gross)  # == T1's value, unchanged
    assert t3_row["active_positions"] == 1  # only ZZZ; AMD/MSFT never became active

    # T4: ZZZ's reduction (0.6->0.2, turnover 0.4) + its fold-end exit
    # (0.2->0, turnover 0.2) plus AMD's and MSFT's entries (0.2 each,
    # turnover 0.2) and their own immediate fold-end exits (turnover 0.2
    # each) all land on this one row: 0.4+0.2 + (0.2+0.2)*2 = 1.4.
    assert t4_row["turnover"] == pytest.approx(1.4)
    assert t4_row["gross_exposure"] == pytest.approx(0.0, abs=1e-12)  # all flattened
    assert t4_row["active_positions"] == 0

    # Internal invariant: gross exposure never exceeds max_gross_exposure at
    # any reported row.
    assert returns["gross_exposure"].max() <= max_gross + 1e-9


def test_repair02_negative_control_headroom_bypass_would_violate_cap(tmp_path):
    """Negative control (REQUIRED) — recompute what a BROKEN allocator that
    skips the headroom check for increases would have done at T3 in
    test_repair02_reduce_first_defer_cohort_atomic_no_rescale's fixture, and
    confirm it WOULD exceed max_gross_exposure. Proves that fixture actually
    exercises the cap constraint (not a fixture that happens to pass either
    way), and that the real implementation's headroom check is load-bearing:
    the real result (asserted below) never reaches this state."""
    max_gross = 0.6
    zzz_still_held = 0.6  # ZZZ has not yet had a bar to execute its reduction
    amd_candidate = 0.2
    msft_candidate = 0.2

    naive_gross_ignoring_headroom = zzz_still_held + amd_candidate + msft_candidate
    assert naive_gross_ignoring_headroom > max_gross + 1e-9  # the forbidden overshoot

    # The real implementation, run on the identical scenario, never reaches
    # this state -- see the T3 assertions above (gross_exposure == 0.6, not
    # 1.0).
    run_dir = tmp_path / "run"
    _zzz_amd_msft_deferred_cohort_fixture(run_dir)
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv").set_index("timestamp")
    t3 = pd.date_range("2021-01-01", periods=5, freq="D", tz="UTC")[3]
    assert returns.loc[t3.isoformat(), "gross_exposure"] == pytest.approx(max_gross)
    assert returns.loc[t3.isoformat(), "gross_exposure"] < naive_gross_ignoring_headroom - 1e-9


def test_repair02_row_order_invariance_with_deferred_cohort(tmp_path):
    """REPAIR-02 TEST 8 -- physical CSV/input row order (both bars.csv and
    the OOS predictions) cannot choose which cohort member "wins": permuting
    symbol construction order, including reversing AMD/MSFT (alphabetically
    adjacent, one of which a naive greedy-alphabetical policy would treat
    differently -- see the negative control below), must not change which
    symbols execute, when, or the resulting economic series at all."""
    run_a = tmp_path / "ordered"
    _zzz_amd_msft_deferred_cohort_fixture(
        run_a, bars_order=["ZZZ", "AMD", "MSFT"], oos_order=["ZZZ", "AMD", "MSFT"]
    )
    run_b = tmp_path / "reversed"
    _zzz_amd_msft_deferred_cohort_fixture(
        run_b, bars_order=["MSFT", "AMD", "ZZZ"], oos_order=["MSFT", "AMD", "ZZZ"]
    )

    ret_a = pd.read_csv(run_a / "eval" / "economic_returns.csv")
    ret_b = pd.read_csv(run_b / "eval" / "economic_returns.csv")
    pd.testing.assert_frame_equal(ret_a, ret_b)


def test_repair02_negative_control_alphabetical_greedy_would_pick_a_winner_by_name():
    """Negative control (optional, REQUIRED-if-practical) -- a naive "fill
    cohort members alphabetically until headroom is exhausted" policy is NOT
    order-independent: reversing physical row order flips which of two
    otherwise-identical requests "wins" headroom, purely by name. Pure
    computation, does not touch production code. The real
    reduce_first_defer_increase_batch_v1 policy never exhibits this --
    test_repair02_row_order_invariance_with_deferred_cohort proves the real
    implementation's result is identical in both orders (both members
    deferred, full cohort, regardless of name/order)."""
    headroom = 0.3
    each_wants = 0.3

    def naive_greedy_fill(order: List[str]) -> List[str]:
        remaining = headroom
        executed = []
        for name in order:
            if each_wants <= remaining + 1e-9:
                executed.append(name)
                remaining -= each_wants
        return executed

    assert naive_greedy_fill(["AMD", "MSFT"]) == ["AMD"]
    assert naive_greedy_fill(["MSFT", "AMD"]) == ["MSFT"]  # winner flips with row order


def test_repair02_entry_executes_strictly_later_after_headroom_frees(tmp_path):
    """REPAIR-02 TEST 2 -- a deferred entry executes at its OWN next bar
    once headroom exists, even when that headroom became available at an
    EARLIER frame the entrant symbol was not itself present at (never
    inferred from the reducing symbol's own bar -- exact-symbol execution,
    CLAUDE.md invariant #5)."""
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=6, freq="D", tz="UTC")
    t0, t1, t2, t3, t4, t5 = days
    max_gross = 0.6

    bars_rows = (
        [_bar_row("ZZZ", ts, 100.0) for ts in (t0, t1, t4)]
        + [_bar_row("AMD", ts, 100.0) for ts in (t3, t5)]
        + [_bar_row("MSFT", ts, 100.0) for ts in (t3, t5)]
    )
    oos_rows = (
        [_oos_row(1, "ZZZ", t0, 1.0)]
        + [_oos_row(1, "AMD", t2, 1.0)]
        + [_oos_row(1, "MSFT", t2, 1.0)]
    )
    folds = [_single_fold("2021-01-01", "2021-01-07")]
    spec = _spec(signal_policy=SignalPolicySpec(entry_threshold=0.5, max_gross_exposure=max_gross))
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=spec)
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv").set_index("timestamp")

    t3_row = returns.loc[t3.isoformat()]
    t4_row = returns.loc[t4.isoformat()]
    t5_row = returns.loc[t5.isoformat()]

    # T3: AMD+MSFT cohort deferred -- ZZZ still holds the whole cap.
    assert t3_row["turnover"] == pytest.approx(0.0, abs=1e-12)
    assert t3_row["transaction_cost"] == pytest.approx(0.0, abs=1e-12)
    assert t3_row["gross_return"] == pytest.approx(0.0, abs=1e-12)

    # T4: ZZZ ALONE has a bar here and reduces, freeing headroom -- but AMD
    # and MSFT are not present at T4 at all, so nothing is fabricated for
    # them on ZZZ's own bar.
    assert t4_row["turnover"] == pytest.approx(0.4)  # ZZZ's reduction only

    # T5: AMD's and MSFT's own next bar -- entry executes HERE, strictly
    # later than T4 where headroom first existed, plus their own immediate
    # fold-end exit on this same (their only remaining) row.
    assert t5_row["turnover"] == pytest.approx(1.0)  # ZZZ exit(0.2) + AMD(0.4) + MSFT(0.4)

    assert returns["gross_exposure"].max() <= max_gross + 1e-9


def test_repair02_stale_deferred_target_superseded_by_newer_contrary_decision(tmp_path):
    """REPAIR-02 TEST 11 -- PENDING SUPERSESSION. AAA holds the whole cap
    first; BBB's entry (signalled at T2) is deferred at T3 (AAA hasn't
    exited yet); BEFORE capacity frees, a NEWER, CONTRARY decision for BBB
    (flat, signalled at T4) arrives. When capacity finally frees at T5, the
    STALE T2 buy must NOT execute -- only the newer T4 flat target (a
    no-op, since BBB never executed anything) is considered. If the stale
    buy had wrongly fired, T5's turnover would be 1.0 (AAA's 0.5 reduction
    + BBB's wrongly-executed 0.5 entry) instead of the correct 0.5."""
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=6, freq="D", tz="UTC")
    t0, t1, t2, t3, t4, t5 = days
    max_gross = 0.5

    bars_rows = (
        [_bar_row("AAA", ts, 100.0) for ts in (t0, t1, t5)]
        + [_bar_row("BBB", ts, 100.0) for ts in (t3, t5)]
    )
    oos_rows = [
        _oos_row(1, "AAA", t0, 1.0),
        _oos_row(1, "AAA", t2, 0.0),  # AAA flips inactive at T2
        _oos_row(1, "BBB", t2, 1.0),  # BBB flips active at T2 ("T0 buy" in module docstring terms)
        _oos_row(1, "BBB", t4, 0.0),  # BBB flips back to inactive at T4, before capacity frees
    ]
    folds = [_single_fold("2021-01-01", "2021-01-07")]
    spec = _spec(signal_policy=SignalPolicySpec(entry_threshold=0.5, max_gross_exposure=max_gross))
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=spec)
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv").set_index("timestamp")

    assert returns.loc[t1.isoformat(), "turnover"] == pytest.approx(0.5)  # AAA enters
    assert returns.loc[t3.isoformat(), "turnover"] == pytest.approx(0.0, abs=1e-12)  # BBB deferred
    # AAA's reduction only -- BBB's stale T2 buy never fires (would be 1.0
    # if it had).
    assert returns.loc[t5.isoformat(), "turnover"] == pytest.approx(0.5)
    assert returns["gross_exposure"].max() <= max_gross + 1e-9


def test_repair02_older_eligible_pending_change_executes_before_new_decision(tmp_path):
    """REPAIR-02 TEST 12 -- an OLDER pending change that is ALREADY eligible
    (its signal_ts is strictly before the current bar) executes using its
    own (old) target even when a BRAND NEW decision is generated at that
    exact same bar timestamp -- a decision generated AT T has signal_ts==T,
    which can never be < T, so it cannot retroactively cancel or execute
    ahead of an older change already legally executable at T. Single symbol,
    max_gross_exposure=1.0 so headroom is never the constraint -- isolates
    pure chronology from capacity."""
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=3, freq="D", tz="UTC")
    t0, t1, t2 = days

    bars_rows = [_bar_row("AAA", ts, 100.0) for ts in days]
    oos_rows = [
        _oos_row(1, "AAA", t0, 1.0),  # T0: signal to enter
        _oos_row(1, "AAA", t1, 0.0),  # T1: a NEW decision, generated at AAA's own T1 bar
    ]
    folds = [_single_fold("2021-01-01", "2021-01-04")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv").set_index("timestamp")

    # T1: the OLD T0 buy executes HERE (using target 1.0), even though a new
    # decision was ALSO generated at this exact bar -- the new decision
    # (signal_ts == T1) cannot itself execute at T1 (needs T1's OWN target
    # to become eligible only strictly after T1).
    assert returns.loc[t1.isoformat(), "turnover"] == pytest.approx(1.0)
    assert returns.loc[t1.isoformat(), "gross_exposure"] == pytest.approx(1.0)

    # T2: the T1 flat decision -- generated at T1, eligible only strictly
    # after T1 -- executes here, one frame later.
    assert returns.loc[t2.isoformat(), "turnover"] == pytest.approx(1.0)
    assert returns.loc[t2.isoformat(), "gross_exposure"] == pytest.approx(0.0, abs=1e-12)


def test_repair02_max_gross_exposure_remains_part_of_trial_identity(tmp_path):
    """REPAIR-02 TEST 13 -- max_gross_exposure (the allocator's shared
    capacity budget) still changes trial identity, unchanged from
    REPAIR-01's already-existing behavior (this field's presence in
    economic_protocol_identity predates REPAIR-02; this test pins it
    explicitly under the new allocator)."""
    registry_db = tmp_path / "registry.sqlite3"
    df = _build_full_dataset(periods_days=560)
    bars_df = _build_flat_bars(df)

    run_a = tmp_path / "a"
    run_a.mkdir(parents=True, exist_ok=True)
    bars_a = run_a / "bars.csv"
    bars_df.to_csv(bars_a, index=False)
    out_a = json.loads(_registered_economic_run(run_a, df, bars_a, registry_db=registry_db).read_text(encoding="utf-8"))

    run_b = tmp_path / "b"
    run_b.mkdir(parents=True, exist_ok=True)
    bars_b = run_b / "bars.csv"
    bars_df.to_csv(bars_b, index=False)
    other_cap = _spec(signal_policy=SignalPolicySpec(max_gross_exposure=0.5))
    out_b = json.loads(_registered_economic_run(run_b, df, bars_b, registry_db=registry_db, economic_spec=other_cap).read_text(encoding="utf-8"))

    assert out_a["registry"]["trial_id"] != out_b["registry"]["trial_id"]


def test_repair02_capacity_policy_serialized_into_identity_and_fails_closed_on_unsupported():
    """REPAIR-02 TEST 14 -- the capacity_policy is part of the registered
    economic protocol identity (so a future different allocator policy would
    be a distinct trial, never silently reinterpreted as the same
    candidate), and an unsupported capacity_policy value fails closed
    (mirrors the existing fold_end_policy contract)."""
    identity = economic_protocol_identity(_spec().normalized())
    assert identity["signal_policy"]["capacity_policy"] == CAPACITY_POLICY_ID == "reduce_first_defer_increase_batch_v1"

    with pytest.raises(ValueError, match="capacity_policy"):
        SignalPolicySpec(capacity_policy="alphabetical_first_fit_v0").normalized()


def test_repair02_simple_complete_panel_unchanged_from_repair01(tmp_path):
    """REPAIR-02 TEST 15 -- regression pin. For a single-symbol, fully
    synchronized panel that never binds on max_gross_exposure, the REPAIR-02
    joint timestamp-frame allocator reproduces REPAIR-01's exact
    decide-at-T / execute-at-T+1 / earn-at-T+1-to-T+2 causal chain and cost
    timing bit-for-bit -- REPAIR-02 changes ONLY cross-symbol capacity
    behavior, never the single-symbol causal/cost contract. (Identical
    fixture to test_repair01_regular_complete_panel_reproduces_decide_
    execute_earn_chain -- duplicated deliberately so this specific
    non-regression claim has its own named, independently-runnable proof.)
    """
    run_dir = tmp_path / "run"
    days = pd.date_range("2021-01-01", periods=5, freq="D", tz="UTC")
    closes = [100.0, 100.0, 105.0, 105.0, 105.0]
    bars_rows = [_bar_row("AAA", d, c) for d, c in zip(days, closes)]
    oos_rows = [_oos_row(1, "AAA", d, 1.0) for d in days]
    folds = [_single_fold("2021-01-01", "2021-01-06")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)

    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv")

    assert returns.loc[0, "turnover"] == pytest.approx(0.0, abs=1e-12)
    assert returns.loc[1, "turnover"] == pytest.approx(1.0)
    assert returns.loc[1, "gross_return"] == pytest.approx(0.0, abs=1e-12)
    assert returns.loc[2, "gross_return"] == pytest.approx(105.0 / 100.0 - 1.0)


# ---------------------------------------------------------------------------
# RESEARCH-ECONOMIC-WALKFORWARD-01-REPAIR-03
#
# REPAIR-02's cohort formation grouped only symbols with an ACTUAL bar in
# the current frame (`present`) into same-signal_ts increase cohorts. This
# could split a same-decision atomic cohort when its members' execution bars
# arrive asynchronously: a symbol with a bar at T could execute ALONE even
# though a sibling signalled at the exact same instant has no bar at T yet
# (and is therefore not itself execution-eligible) — violating the frozen
# "same decision timestamp -> atomic increase cohort" policy.
#
# REPAIR-03 (see economic_walkforward._simulate_fold_execution) determines
# full cohort membership from every symbol's causally-effective unresolved
# pending increase as of T, including absent symbols (inspected read-only,
# never mutating their pending pointer, never fabricating an execution for
# them), and defers the ENTIRE cohort if any member lacks an actual bar at T.
# ---------------------------------------------------------------------------


def _amd_msft_missing_member_fixture(run_dir: Path) -> Path:
    """AMD and MSFT flip active TOGETHER at T0 (one shared decision frame,
    same signal_ts) -> both get an equal-weight (0.5 each, max_gross=1.0)
    pending increase event. AMD has a bar at T1; MSFT does not -> the T0
    cohort must be deferred IN FULL at T1 (neither executes), even though
    AMD alone would trivially fit headroom. At T2 both AMD and MSFT have
    bars -> the whole cohort executes together. T3 force-flats both."""
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    t0, t1, t2, t3 = days
    bars_rows = (
        [_bar_row("AMD", ts, 100.0) for ts in (t1, t2, t3)]
        + [_bar_row("MSFT", ts, 100.0) for ts in (t2, t3)]  # MSFT absent at t1
    )
    oos_rows = [
        _oos_row(1, "AMD", t0, 1.0),
        _oos_row(1, "MSFT", t0, 1.0),
    ]
    folds = [_single_fold("2021-01-01", "2021-01-05")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)
    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    return bars_path


def test_repair03_missing_cohort_member_defers_whole_cohort(tmp_path):
    """REPAIR-03 TEST 1 (fails against REPAIR-02 code) -- AMD must NOT
    execute alone at T1 merely because MSFT (its same-signal_ts cohort
    sibling) lacks an actual bar there. T1: zero turnover/cost/exposure for
    the whole cohort. T2: both AMD and MSFT have actual eligible bars ->
    execute together since the whole cohort fits headroom."""
    run_dir = tmp_path / "run"
    _amd_msft_missing_member_fixture(run_dir)
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv").set_index("timestamp")
    days = pd.date_range("2021-01-01", periods=4, freq="D", tz="UTC")
    t0, t1, t2, t3 = days

    # T0 never appears: no symbol has an actual bar there.
    assert list(returns.index) == [d.isoformat() for d in (t1, t2, t3)]

    t1_row = returns.loc[t1.isoformat()]
    t2_row = returns.loc[t2.isoformat()]

    # T1: AMD present, MSFT absent -- the T0 cohort must be deferred IN
    # FULL. AMD DOES NOT execute merely because it happens to have a bar.
    assert t1_row["turnover"] == pytest.approx(0.0, abs=1e-12)
    assert t1_row["transaction_cost"] == pytest.approx(0.0, abs=1e-12)
    assert t1_row["gross_exposure"] == pytest.approx(0.0, abs=1e-12)
    assert t1_row["active_positions"] == 0

    # T2: both AMD and MSFT now have actual eligible bars -> the whole
    # cohort fits headroom (0.5+0.5=1.0<=max_gross=1.0) -> executes together.
    assert t2_row["turnover"] == pytest.approx(1.0)  # 0.5 AMD + 0.5 MSFT
    assert t2_row["gross_exposure"] == pytest.approx(1.0)
    assert t2_row["active_positions"] == 2

    assert returns["gross_exposure"].max() <= 1.0 + 1e-9


_NEG_CONTROL_TOL = 1e-9


def test_repair03_negative_control_present_only_cohort_would_execute_amd_alone():
    """Negative control (REQUIRED) -- reproduce REPAIR-02's BROKEN cohort
    formation (group only `present` symbols by signal_ts) directly on TEST
    1's T1 state, and confirm it WOULD let AMD execute alone. Proves the
    fixture actually exercises the defect (not a fixture that happens to
    pass either way regardless of the fix). Pure computation; does not call
    production code."""
    max_gross_exposure = 1.0
    executed_weight = {"AMD": 0.0, "MSFT": 0.0}
    # Both AMD and MSFT have the same causally-effective pending increase
    # (signal_ts=T0, target=0.5) at T1 -- but only AMD has an actual bar.
    candidate = {"AMD": 0.5}  # MSFT is absent -> the OLD code never even
    # computes (let alone includes) a candidate for it, since it iterated
    # only `present = ["AMD"]`.
    present = ["AMD"]

    increase_symbols = [
        s for s in present
        if candidate.get(s) is not None and candidate[s] > executed_weight[s] + _NEG_CONTROL_TOL
    ]
    cohorts: Dict[str, List[str]] = {}
    for s in increase_symbols:
        cohorts.setdefault("T0", []).append(s)

    gross = sum(abs(w) for w in executed_weight.values())
    broken_executed = dict(executed_weight)
    for members in cohorts.values():
        delta = sum(abs(candidate[s]) - abs(broken_executed[s]) for s in members)
        if gross + delta <= max_gross_exposure + _NEG_CONTROL_TOL:
            for s in members:
                broken_executed[s] = candidate[s]
            gross += delta

    assert broken_executed["AMD"] == pytest.approx(0.5)  # the forbidden solo fill
    assert broken_executed["MSFT"] == pytest.approx(0.0)  # left untouched -> a split cohort

    # The real (fixed) implementation, run on the identical scenario, never
    # reaches this state -- see test_repair03_missing_cohort_member_defers_
    # whole_cohort's T1 assertions (turnover == 0.0, active_positions == 0).


def _amd_msft_supersession_unblock_fixture(run_dir: Path) -> Path:
    """AMD and MSFT flip active TOGETHER at T0 (shared signal_ts, equal-
    weight 0.5 each). AMD has a bar at T1; MSFT never has any bar in this
    fold -> the T0 cohort is deferred at T1 (REPAIR-03's fix). Before AMD's
    next bar, MSFT receives a NEWER, CONTRARY decision at T2 (flips back to
    inactive) -- this rebalances the equal-weight-active denominator for the
    still-active AMD (now the sole active name) to the full max_gross_
    exposure (1.0), issuing AMD a NEW pending event at signal_ts=T2. By T3
    (AMD's next bar), MSFT's causally-effective pending target is the newer
    T2 flat (0.0, a no-op since MSFT never executed anything) -- not the
    stale T0 increase -- so MSFT no longer blocks AMD's (now superseding)
    cohort of one. AMD executes ALONE at T3 using the T2 target (1.0, not
    the stale T0 target of 0.5)."""
    days = pd.date_range("2021-01-01", periods=5, freq="D", tz="UTC")
    t0, t1, t2, t3, t4 = days
    bars_rows = [_bar_row("AMD", ts, 100.0) for ts in (t1, t3, t4)]  # MSFT: no bars ever
    oos_rows = [
        _oos_row(1, "AMD", t0, 1.0),
        _oos_row(1, "MSFT", t0, 1.0),
        _oos_row(1, "MSFT", t2, 0.0),  # MSFT's newer, contrary decision
    ]
    folds = [_single_fold("2021-01-01", "2021-01-06")]
    bars_path = _write_direct_fixture(run_dir, folds=folds, oos_rows=oos_rows, bars_rows=bars_rows)
    run_economic_walkforward(run_dir, bars_csv=bars_path, spec=_spec())
    return bars_path


def test_repair03_supersession_unblocks_stale_cohort(tmp_path):
    """REPAIR-03 TEST 2 -- causal supersession must not deadlock an obsolete
    cohort. MSFT's stale T0 increase (superseded by its own newer T2 flat
    target before it ever had a chance to execute) must stop blocking AMD's
    T0 cohort membership once T2 becomes causally effective. AMD executes at
    T3 using the T2-rebalanced target (1.0 = the full cap, since MSFT is the
    only inactive name by then), not the stale T0 target (0.5)."""
    run_dir = tmp_path / "run"
    _amd_msft_supersession_unblock_fixture(run_dir)
    returns = pd.read_csv(run_dir / "eval" / "economic_returns.csv").set_index("timestamp")
    days = pd.date_range("2021-01-01", periods=5, freq="D", tz="UTC")
    t0, t1, t2, t3, t4 = days

    # T1: AMD present, MSFT absent -- T0 cohort deferred IN FULL.
    t1_row = returns.loc[t1.isoformat()]
    assert t1_row["turnover"] == pytest.approx(0.0, abs=1e-12)
    assert t1_row["active_positions"] == 0

    # T3: AMD's own next bar. MSFT's effective pending target as of T3 is
    # now the newer T2 flat (0.0), NOT the stale T0 increase (0.5) -- so
    # AMD's cohort is just [AMD], unblocked, and executes using the T2-
    # rebalanced target (1.0, the full cap -- AMD is the sole active name
    # once MSFT went flat), not the stale 0.5.
    t3_row = returns.loc[t3.isoformat()]
    assert t3_row["turnover"] == pytest.approx(1.0)
    assert t3_row["gross_exposure"] == pytest.approx(1.0)
    assert t3_row["active_positions"] == 1

    # T4 (fold end, AMD's own next bar): force-flat exit, MSFT contributes
    # nothing at any point (it never actually executed a single fill).
    t4_row = returns.loc[t4.isoformat()]
    assert t4_row["turnover"] == pytest.approx(1.0)  # AMD's exit only
    assert t4_row["active_positions"] == 0

    assert returns["gross_exposure"].max() <= 1.0 + 1e-9
