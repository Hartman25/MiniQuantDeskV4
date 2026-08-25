"""
RESEARCH-FACTOR-NULL-CONTROLS-01 -- deterministic null/placebo control tests.

Proves: null controls are evaluation slices of the SAME factor (never a new
factor_id), deterministic seeds reproduce byte-identical results, a changed
seed changes evaluation-slice identity, real-vs-null result never alters
factor identity, and a genuine synthetic signal loses its edge under a
destroyed-alignment control while a meaningless factor never distinguishes
itself from its own null.
"""
from __future__ import annotations

import json

import numpy as np
import pandas as pd
import pytest

from mqk_research.factors.contracts import (
    DIRECTION_HIGHER_IS_BETTER,
    EVAL_STATUS_SUCCEEDED,
    FactorEvaluationSpec,
    FactorSpec,
    NORMALIZATION_CROSS_SECTIONAL_RANK,
    TIMING_NEXT_BAR_TRADABLE,
)
from mqk_research.factors.diagnostics import evaluate_factor_ic_ir
from mqk_research.factors.null_controls import (
    CONTROL_KIND_CROSS_SECTIONAL_PERMUTATION,
    CONTROL_KIND_TEMPORAL_OFFSET,
    FactorNullControlSpec,
    compare_real_vs_null,
    run_null_control,
)
from mqk_research.factors.registry import begin_factor_evaluation, list_factors, register_factor


_FAR_FUTURE_LABEL_END = "2099-01-01T00:00:00+00:00"


def _symbols(n: int):
    return [f"SYM{i}" for i in range(n)]


def _periods(n: int):
    return [f"2024-01-{d:02d}T00:00:00+00:00" for d in range(1, n + 1)]


def _with_causal_columns(df: pd.DataFrame) -> pd.DataFrame:
    df = df.copy()
    df["information_cutoff_ts_utc"] = df["period_ts_utc"]
    df["label_end_ts_utc"] = _FAR_FUTURE_LABEL_END
    return df


def _monotonic_dataset(n_symbols=6, n_periods=8) -> pd.DataFrame:
    rows = []
    for period in _periods(n_periods):
        for i, sym in enumerate(_symbols(n_symbols)):
            rows.append({"symbol": sym, "period_ts_utc": period, "factor_value": float(i), "label_fwd_ret": float(i)})
    return _with_causal_columns(pd.DataFrame(rows))


def _null_like_dataset(n_symbols=6, n_periods=8, seed=7) -> pd.DataFrame:
    rng = np.random.default_rng(seed)
    rows = []
    for period in _periods(n_periods):
        fv = rng.permutation(n_symbols).astype(float)
        lv = rng.permutation(n_symbols).astype(float)
        for sym, f, l in zip(_symbols(n_symbols), fv, lv):
            rows.append({"symbol": sym, "period_ts_utc": period, "factor_value": f, "label_fwd_ret": l})
    return _with_causal_columns(pd.DataFrame(rows))


def _temporally_shuffled_signal_dataset(n_symbols=6, n_periods=12, seed=123) -> pd.DataFrame:
    """A genuine same-period signal: factor_value == label_fwd_ret exactly
    within each period, but the symbol<->rank assignment is independently
    re-randomized every period. Real (unshifted) evaluation must find a
    near-perfect same-period relationship; misaligning periods (temporal
    offset control) must destroy it, since knowing period p's assignment
    tells you nothing about period p-1's independent assignment."""
    rng = np.random.default_rng(seed)
    rows = []
    for period in _periods(n_periods):
        perm = rng.permutation(n_symbols).astype(float)
        for sym, val in zip(_symbols(n_symbols), perm):
            rows.append({"symbol": sym, "period_ts_utc": period, "factor_value": val, "label_fwd_ret": val})
    return _with_causal_columns(pd.DataFrame(rows))


def _spec() -> FactorSpec:
    return FactorSpec(
        family="momentum",
        name="12m_1m_lag",
        protocol_version="v1",
        params={"lookback_days": 252},
        required_input_fields=["close"],
        lookback_periods=252,
        horizon_periods=21,
        normalization=NORMALIZATION_CROSS_SECTIONAL_RANK,
        direction=DIRECTION_HIGHER_IS_BETTER,
        universe_identity={"universe_id": "sp500_pit_v1"},
        data_provenance_identity={"provider": "alpaca"},
        timing_convention=TIMING_NEXT_BAR_TRADABLE,
        information_lag_periods=1,
    )


def _eval_spec(factor_id: str) -> FactorEvaluationSpec:
    return FactorEvaluationSpec(
        factor_id=factor_id,
        universe_identity={"universe_id": "sp500_pit_v1"},
        evaluation_window_start_utc="2024-01-01T00:00:00+00:00",
        evaluation_window_end_utc="2024-06-01T00:00:00+00:00",
        label_protocol_version="fwd_ret_label_v1",
        evaluation_protocol_version="factor_ic_ir_quantile_v1",
    )


def _eval_kwargs():
    return dict(n_quantiles=3, min_cross_section=3)


# -- known synthetic signal beats its destroyed-alignment control --------

def test_known_signal_beats_temporal_offset_control():
    df = _temporally_shuffled_signal_dataset()
    real = evaluate_factor_ic_ir(df, **_eval_kwargs())
    assert real.status == EVAL_STATUS_SUCCEEDED
    assert real.metrics["mean_ic"] > 0.95

    eval_spec = _eval_spec("f" * 32)
    control_spec = FactorNullControlSpec(
        base_evaluation=eval_spec, control_kind=CONTROL_KIND_TEMPORAL_OFFSET, seed=1
    )
    null = run_null_control(df, control_spec, evaluate_fn=evaluate_factor_ic_ir, **_eval_kwargs())
    assert null.status == EVAL_STATUS_SUCCEEDED
    assert abs(null.metrics["mean_ic"]) < 0.5
    assert abs(null.metrics["mean_ic"]) < abs(real.metrics["mean_ic"])

    comparison = compare_real_vs_null(real, null)
    assert comparison["null_as_strong_or_stronger"] is False


# -- deliberately meaningless factor fails to distinguish itself from null

def test_meaningless_factor_does_not_beat_its_null():
    df = _null_like_dataset()
    real = evaluate_factor_ic_ir(df, **_eval_kwargs())
    eval_spec = _eval_spec("f" * 32)
    control_spec = FactorNullControlSpec(
        base_evaluation=eval_spec, control_kind=CONTROL_KIND_CROSS_SECTIONAL_PERMUTATION, seed=42
    )
    null = run_null_control(df, control_spec, evaluate_fn=evaluate_factor_ic_ir, **_eval_kwargs())
    assert real.status == EVAL_STATUS_SUCCEEDED
    assert null.status == EVAL_STATUS_SUCCEEDED
    # Both weak -- the meaningless factor never clears its own null baseline.
    assert abs(real.metrics["mean_ic"]) < 0.5
    assert abs(null.metrics["mean_ic"]) < 0.5


# -- deterministic repeated null run byte-identical -----------------------

def test_null_run_is_byte_identical_on_repeat():
    df = _monotonic_dataset()
    eval_spec = _eval_spec("f" * 32)
    control_spec = FactorNullControlSpec(
        base_evaluation=eval_spec, control_kind=CONTROL_KIND_CROSS_SECTIONAL_PERMUTATION, seed=99
    )
    a = run_null_control(df, control_spec, evaluate_fn=evaluate_factor_ic_ir, **_eval_kwargs())
    b = run_null_control(df, control_spec, evaluate_fn=evaluate_factor_ic_ir, **_eval_kwargs())
    assert json.dumps(a.to_dict(), sort_keys=True) == json.dumps(b.to_dict(), sort_keys=True)


# -- changed explicit seed changes evaluation-slice identity --------------

def test_changed_seed_changes_control_evaluation_id():
    eval_spec = _eval_spec("f" * 32)
    control_a = FactorNullControlSpec(
        base_evaluation=eval_spec, control_kind=CONTROL_KIND_CROSS_SECTIONAL_PERMUTATION, seed=1
    )
    control_b = FactorNullControlSpec(
        base_evaluation=eval_spec, control_kind=CONTROL_KIND_CROSS_SECTIONAL_PERMUTATION, seed=2
    )
    assert control_a.compute_control_evaluation_id() != control_b.compute_control_evaluation_id()


def test_changed_control_kind_changes_control_evaluation_id():
    eval_spec = _eval_spec("f" * 32)
    control_a = FactorNullControlSpec(
        base_evaluation=eval_spec, control_kind=CONTROL_KIND_CROSS_SECTIONAL_PERMUTATION, seed=1
    )
    control_b = FactorNullControlSpec(base_evaluation=eval_spec, control_kind=CONTROL_KIND_TEMPORAL_OFFSET, seed=1)
    assert control_a.compute_control_evaluation_id() != control_b.compute_control_evaluation_id()


def test_temporal_offset_requires_positive_seed():
    eval_spec = _eval_spec("f" * 32)
    with pytest.raises(ValueError, match="offset periods"):
        FactorNullControlSpec(
            base_evaluation=eval_spec, control_kind=CONTROL_KIND_TEMPORAL_OFFSET, seed=0
        ).validate()


def test_invalid_control_kind_rejected():
    eval_spec = _eval_spec("f" * 32)
    with pytest.raises(ValueError, match="control_kind"):
        FactorNullControlSpec(base_evaluation=eval_spec, control_kind="bogus", seed=1).validate()


# -- real vs null result does NOT alter factor identity -------------------

def test_real_vs_null_result_does_not_alter_factor_identity():
    spec = _spec()
    factor_id_before = spec.compute_factor_id()
    df = _monotonic_dataset()
    real = evaluate_factor_ic_ir(df, **_eval_kwargs())
    eval_spec = _eval_spec(factor_id_before)
    control_spec = FactorNullControlSpec(
        base_evaluation=eval_spec, control_kind=CONTROL_KIND_CROSS_SECTIONAL_PERMUTATION, seed=5
    )
    run_null_control(df, control_spec, evaluate_fn=evaluate_factor_ic_ir, **_eval_kwargs())
    assert spec.compute_factor_id() == factor_id_before


# -- registry: null run creates no new factor_id / retry does not either --

def test_null_run_registers_under_same_factor_id_no_new_factor(tmp_path):
    registry_db = tmp_path / "registry.sqlite3"
    factor_id = register_factor(registry_db, _spec())
    eval_spec = _eval_spec(factor_id)
    control_spec = FactorNullControlSpec(
        base_evaluation=eval_spec, control_kind=CONTROL_KIND_CROSS_SECTIONAL_PERMUTATION, seed=11
    )
    control_evaluation_id = control_spec.compute_control_evaluation_id()

    attempt_1, attempt_index_1 = _begin_control_attempt(registry_db, factor_id, control_spec, control_evaluation_id)
    attempt_2, attempt_index_2 = _begin_control_attempt(registry_db, factor_id, control_spec, control_evaluation_id)

    assert attempt_index_1 == 1 and attempt_index_2 == 2  # append-only retries
    assert len(list_factors(registry_db)) == 1  # no new factor identity created


def _begin_control_attempt(registry_db, factor_id, control_spec, control_evaluation_id):
    from mqk_research.exp_distributed.storage import ResearchResultStore

    store = ResearchResultStore(registry_db)
    return store.begin_factor_evaluation_attempt(
        factor_id=factor_id,
        evaluation_id=control_evaluation_id,
        evaluation_identity=control_spec.identity_payload(),
        origin="null_control_test",
    )


def test_compare_real_vs_null_requires_succeeded_reports():
    df = _monotonic_dataset()
    real = evaluate_factor_ic_ir(df, **_eval_kwargs())
    not_evaluable = evaluate_factor_ic_ir(
        _with_causal_columns(
            pd.DataFrame(
                {"symbol": ["A"], "period_ts_utc": ["2024-01-01T00:00:00+00:00"], "factor_value": [1.0], "label_fwd_ret": [1.0]}
            )
        ),
        n_quantiles=2,
        min_cross_section=2,
    )
    assert not_evaluable.status != EVAL_STATUS_SUCCEEDED
    with pytest.raises(ValueError):
        compare_real_vs_null(real, not_evaluable)
