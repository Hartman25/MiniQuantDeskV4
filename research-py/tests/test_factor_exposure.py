"""
RESEARCH-FACTOR-EXPOSURE-ATTRIBUTION-01 -- exposure/attribution negative controls.

Proves: a factor that is purely a size proxy loses signal after size
neutralization, an independent synthetic factor retains signal, a future
period's exposure values can never leak backward into an earlier period's
residual, a singular exposure design matrix fails closed per period, an
exposure schema change changes schema identity, and neutralization result
values never alter factor identity.
"""
from __future__ import annotations

import numpy as np
import pandas as pd
import pytest

from mqk_research.factors.contracts import EVAL_STATUS_NOT_EVALUABLE, EVAL_STATUS_SUCCEEDED, FactorSpec, DIRECTION_HIGHER_IS_BETTER, NORMALIZATION_CROSS_SECTIONAL_RANK, TIMING_NEXT_BAR_TRADABLE
from mqk_research.factors.exposure import (
    NOT_EVALUABLE_INSUFFICIENT_PERIODS_FOR_NEUTRALIZATION,
    NOT_EVALUABLE_SINGULAR_EXPOSURE_MATRIX,
    ExposureSchema,
    compute_exposure_association,
    evaluate_factor_exposure_diagnostics,
    neutralize_factor,
)


_FAR_FUTURE_LABEL_END = "2099-01-01T00:00:00+00:00"


def _periods(n):
    return [f"2024-01-{d:02d}T00:00:00+00:00" for d in range(1, n + 1)]


def _symbols(n):
    return [f"SYM{i}" for i in range(n)]


def _with_causal_columns(df: pd.DataFrame) -> pd.DataFrame:
    df = df.copy()
    df["information_cutoff_ts_utc"] = df["period_ts_utc"]
    df["label_end_ts_utc"] = _FAR_FUTURE_LABEL_END
    return df


def _size_proxy_dataset(n_symbols=6, n_periods=8) -> pd.DataFrame:
    """factor_value == size == label_fwd_ret exactly: a perfect signal that
    is ENTIRELY explained by the size exposure."""
    rows = []
    for period in _periods(n_periods):
        for i, sym in enumerate(_symbols(n_symbols)):
            size = float(i)
            rows.append(
                {"symbol": sym, "period_ts_utc": period, "factor_value": size, "label_fwd_ret": size, "size": size}
            )
    return _with_causal_columns(pd.DataFrame(rows))


def _independent_factor_dataset(n_symbols=6, n_periods=8, seed=3) -> pd.DataFrame:
    """factor_value == label_fwd_ret (real signal), but `size` is an
    INDEPENDENT random permutation each period, unrelated to the factor."""
    rng = np.random.default_rng(seed)
    rows = []
    for period in _periods(n_periods):
        factor_perm = rng.permutation(n_symbols).astype(float)
        size_perm = rng.permutation(n_symbols).astype(float)
        for sym, fv, sz in zip(_symbols(n_symbols), factor_perm, size_perm):
            rows.append({"symbol": sym, "period_ts_utc": period, "factor_value": fv, "label_fwd_ret": fv, "size": sz})
    return _with_causal_columns(pd.DataFrame(rows))


def _schema() -> ExposureSchema:
    return ExposureSchema(numeric_exposure_columns=["size"])


def _kwargs():
    return dict(n_quantiles=3, min_cross_section=3)


# -- factor that is purely a size proxy loses signal after neutralization -

def test_size_proxy_factor_loses_signal_after_neutralization():
    df = _size_proxy_dataset()
    report = evaluate_factor_exposure_diagnostics(df, _schema(), **_kwargs())
    assert report.metrics.get("baseline_mean_ic", None) == pytest.approx(1.0) or report.status == EVAL_STATUS_SUCCEEDED
    assert report.status == EVAL_STATUS_SUCCEEDED
    assert report.metrics["baseline_mean_ic"] > 0.95
    # After removing size, the residual factor is ~0 everywhere -> either
    # not_evaluable (zero variance) or a near-zero neutralized IC.
    if report.metrics["neutralized_status"] == EVAL_STATUS_SUCCEEDED:
        assert abs(report.metrics["neutralized_mean_ic"]) < 0.3
    else:
        assert report.metrics["neutralized_status"] == EVAL_STATUS_NOT_EVALUABLE


# -- independent synthetic factor retains signal ---------------------------

def test_independent_factor_retains_signal_after_neutralization():
    df = _independent_factor_dataset()
    report = evaluate_factor_exposure_diagnostics(df, _schema(), **_kwargs())
    assert report.status == EVAL_STATUS_SUCCEEDED
    assert report.metrics["baseline_mean_ic"] > 0.9
    assert report.metrics["neutralized_status"] == EVAL_STATUS_SUCCEEDED
    # Neutralizing against an unrelated exposure removes little signal.
    assert report.metrics["neutralized_mean_ic"] > 0.7


# -- future exposure row cannot leak backward ------------------------------

def test_future_exposure_mutation_does_not_change_earlier_residuals():
    df = _independent_factor_dataset()
    schema = _schema()
    result_a = neutralize_factor(df, schema)

    mutated = df.copy()
    last_period = sorted(mutated["period_ts_utc"].unique())[-1]
    mutated.loc[mutated["period_ts_utc"] == last_period, "size"] = 999.0
    result_b = neutralize_factor(mutated, schema)

    early_periods = sorted(df["period_ts_utc"].unique())[:-1]
    a_early = result_a["neutralized_observations"]
    a_early = a_early[a_early["period_ts_utc"].isin(early_periods)].sort_values(["period_ts_utc", "symbol"]).reset_index(drop=True)
    b_early = result_b["neutralized_observations"]
    b_early = b_early[b_early["period_ts_utc"].isin(early_periods)].sort_values(["period_ts_utc", "symbol"]).reset_index(drop=True)

    pd.testing.assert_series_equal(a_early["factor_value"], b_early["factor_value"])


# -- singular exposure matrix -> typed not_evaluable -----------------------

def test_singular_exposure_matrix_not_evaluable():
    df = _independent_factor_dataset()
    df = df.copy()
    df["size_duplicate"] = df["size"] * 2.0  # perfectly collinear with `size`
    schema = ExposureSchema(numeric_exposure_columns=["size", "size_duplicate"])

    neutralization = neutralize_factor(df, schema)
    assert len(neutralization["neutralized_observations"]) == 0
    assert all(reason == NOT_EVALUABLE_SINGULAR_EXPOSURE_MATRIX for reason in neutralization["excluded_periods"].values())

    report = evaluate_factor_exposure_diagnostics(df, schema, **_kwargs())
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    assert report.reason == NOT_EVALUABLE_INSUFFICIENT_PERIODS_FOR_NEUTRALIZATION


# -- exposure column/schema change -> evaluation identity change ----------

def test_schema_change_changes_schema_id():
    base_id = ExposureSchema(numeric_exposure_columns=["size"]).compute_schema_id()
    changed_id = ExposureSchema(numeric_exposure_columns=["size", "beta"]).compute_schema_id()
    assert base_id != changed_id


def test_schema_retyping_changes_schema_id():
    numeric_id = ExposureSchema(numeric_exposure_columns=["sector"]).compute_schema_id()
    categorical_id = ExposureSchema(categorical_exposure_columns=["sector"]).compute_schema_id()
    assert numeric_id != categorical_id


def test_schema_rejects_overlap_and_empty():
    with pytest.raises(ValueError, match="requires at least one"):
        ExposureSchema().validate()
    with pytest.raises(ValueError, match="cannot be both"):
        ExposureSchema(numeric_exposure_columns=["size"], categorical_exposure_columns=["size"]).validate()


# -- numeric result change -> factor identity unchanged --------------------

def test_exposure_result_does_not_alter_factor_identity():
    spec = FactorSpec(
        family="momentum",
        name="size_check",
        protocol_version="v1",
        params={},
        required_input_fields=["close"],
        lookback_periods=10,
        horizon_periods=5,
        normalization=NORMALIZATION_CROSS_SECTIONAL_RANK,
        direction=DIRECTION_HIGHER_IS_BETTER,
        universe_identity={"universe_id": "sp500_pit_v1"},
        data_provenance_identity={"provider": "alpaca"},
        timing_convention=TIMING_NEXT_BAR_TRADABLE,
        information_lag_periods=1,
    )
    factor_id_before = spec.compute_factor_id()
    df = _size_proxy_dataset()
    evaluate_factor_exposure_diagnostics(df, _schema(), **_kwargs())
    assert spec.compute_factor_id() == factor_id_before


# -- categorical exposure profile ------------------------------------------

def test_categorical_exposure_quantile_profile():
    df = _independent_factor_dataset().copy()
    rng = np.random.default_rng(11)
    df["sector"] = rng.choice(["TECH", "FIN", "HEALTH"], size=len(df))
    schema = ExposureSchema(categorical_exposure_columns=["sector"])
    from mqk_research.factors.exposure import compute_quantile_exposure_profile

    profile = compute_quantile_exposure_profile(df, schema, **_kwargs())
    assert "sector" in profile["categorical"]
    top_frac = profile["categorical"]["sector"]["top_quantile_category_fraction"]
    assert isinstance(top_frac, dict)
    if top_frac:
        assert pytest.approx(sum(top_frac.values()), abs=1e-6) == 1.0


def test_association_reports_none_for_zero_variance_exposure():
    df = _independent_factor_dataset().copy()
    df["constant_exposure"] = 1.0
    schema = ExposureSchema(numeric_exposure_columns=["constant_exposure"])
    associations = compute_exposure_association(df, schema, min_cross_section=3)
    assert associations["constant_exposure"] is None
