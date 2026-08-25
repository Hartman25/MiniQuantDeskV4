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

from mqk_research.factors.contracts import (
    EVAL_STATUS_NOT_EVALUABLE,
    EVAL_STATUS_SUCCEEDED,
    FactorEvaluationSpec,
    FactorSpec,
    DIRECTION_HIGHER_IS_BETTER,
    NORMALIZATION_CROSS_SECTIONAL_RANK,
    TIMING_NEXT_BAR_TRADABLE,
)
from mqk_research.factors.exposure import (
    NOT_EVALUABLE_ILL_CONDITIONED_EXPOSURE_MATRIX,
    NOT_EVALUABLE_INSUFFICIENT_PERIODS_FOR_NEUTRALIZATION,
    NOT_EVALUABLE_SINGULAR_EXPOSURE_MATRIX,
    ExposureSchema,
    FactorExposureEvaluationSpec,
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


def _single_period_frame(factor_values: np.ndarray, exposure_values: np.ndarray) -> pd.DataFrame:
    n = len(factor_values)
    return pd.DataFrame(
        {
            "symbol": _symbols(n),
            "period_ts_utc": ["2024-01-01T00:00:00+00:00"] * n,
            "factor_value": factor_values,
            "size": exposure_values,
        }
    )


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


# -- a future-only categorical LEVEL cannot leak backward either -----------
# RESEARCH-FACTOR-EXPOSURE-POINT-IN-TIME-CATEGORY-01

def _independent_factor_dataset_with_sector(n_symbols=6, n_periods=8, seed=3) -> pd.DataFrame:
    df = _independent_factor_dataset(n_symbols=n_symbols, n_periods=n_periods, seed=seed)
    sectors = ["TECH", "FIN", "HEALTH"]
    df = df.copy()
    df["sector"] = [sectors[i % len(sectors)] for i in range(len(df))]
    return df


def test_future_categorical_level_does_not_leak_into_earlier_periods():
    df = _independent_factor_dataset_with_sector()
    schema = ExposureSchema(categorical_exposure_columns=["sector"])
    result_a = neutralize_factor(df, schema)

    mutated = df.copy()
    last_period = sorted(mutated["period_ts_utc"].unique())[-1]
    last_period_first_idx = mutated[mutated["period_ts_utc"] == last_period].index[0]
    # A brand-new category, never seen in any earlier period, appearing
    # ONLY in the final/future period.
    mutated.loc[last_period_first_idx, "sector"] = "ENERGY"
    result_b = neutralize_factor(mutated, schema)

    early_periods = sorted(df["period_ts_utc"].unique())[:-1]
    a_early = result_a["neutralized_observations"]
    a_early = a_early[a_early["period_ts_utc"].isin(early_periods)].sort_values(["period_ts_utc", "symbol"]).reset_index(drop=True)
    b_early = result_b["neutralized_observations"]
    b_early = b_early[b_early["period_ts_utc"].isin(early_periods)].sort_values(["period_ts_utc", "symbol"]).reset_index(drop=True)

    # Residuals for every earlier period are byte/numerically identical.
    pd.testing.assert_series_equal(a_early["factor_value"], b_early["factor_value"])

    # Evaluability decisions for earlier periods (excluded reason AND design
    # column count) are also unaffected -- the historical bug added an
    # all-zero "ENERGY" dummy column to every EARLIER period's design too,
    # which could rank-deficiency-exclude periods that were previously
    # well-posed.
    for period in early_periods:
        period_key = str(period)
        assert result_a["excluded_periods"].get(period_key) == result_b["excluded_periods"].get(period_key)
        assert (
            result_a["period_design_column_counts"].get(period_key)
            == result_b["period_design_column_counts"].get(period_key)
        )
        assert period_key not in result_a["period_design_column_counts"] or "ENERGY" not in result_a[
            "period_categorical_vocab"
        ][period_key].get("sector", [])


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


# -- residual numerical-dust tolerance is scale-of-the-regression-derived,
# never a fixed fraction of the original factor's business magnitude ------
# RESEARCH-FACTOR-RANK-AND-NEUTRALIZATION-NUMERICS-02

def test_exact_collinear_residual_becomes_numerical_dust_zero():
    rng = np.random.default_rng(5)
    exposure = rng.permutation(8).astype(float)
    factor = 3.0 * exposure  # exact linear function of the exposure -- nothing else
    schema = ExposureSchema(numeric_exposure_columns=["size"])
    result = neutralize_factor(_single_period_frame(factor, exposure), schema)
    residual = result["neutralized_observations"]["factor_value"].to_numpy()
    assert np.all(residual == 0.0)


def test_large_exposure_coefficient_does_not_erase_small_independent_signal():
    """factor = 1e9*exposure + 0.1*independent_signal: the real ~0.1 residual
    is NOT numerical dust and must survive neutralization even though the
    original factor's scale is dominated by a huge exposure coefficient."""
    rng = np.random.default_rng(6)
    exposure = rng.permutation(8).astype(float)
    independent = rng.permutation(8).astype(float) - 3.5
    factor = 1e9 * exposure + 0.1 * independent
    schema = ExposureSchema(numeric_exposure_columns=["size"])
    result = neutralize_factor(_single_period_frame(factor, exposure), schema)
    residual = result["neutralized_observations"]["factor_value"].to_numpy()
    assert not np.all(residual == 0.0)
    assert np.max(np.abs(residual)) > 0.01


def test_large_exposure_signal_conclusion_invariant_to_positive_rescaling():
    rng = np.random.default_rng(6)
    exposure = rng.permutation(8).astype(float)
    independent = rng.permutation(8).astype(float) - 3.5
    factor = 1e9 * exposure + 0.1 * independent
    schema = ExposureSchema(numeric_exposure_columns=["size"])

    result = neutralize_factor(_single_period_frame(factor, exposure), schema)
    residual = result["neutralized_observations"]["factor_value"].to_numpy()

    k = 7.0
    scaled_result = neutralize_factor(_single_period_frame(factor * k, exposure), schema)
    scaled_residual = scaled_result["neutralized_observations"]["factor_value"].to_numpy()

    assert not np.all(residual == 0.0)
    assert not np.all(scaled_residual == 0.0)
    # A generous tolerance: the factor's huge dynamic range (1e9 exposure
    # term vs 0.1 independent term) means the two independent OLS solves
    # (unscaled vs k-scaled) each lose a few digits of floating-point
    # precision on their own -- this checks the SAME conclusion (real signal
    # survives, same sign/order, scales with k), not bit-for-bit equality.
    np.testing.assert_allclose(scaled_residual, residual * k, rtol=1e-3)


def test_ill_conditioned_but_full_rank_design_fails_closed():
    """A design matrix that is full rank in floating point but materially
    ill-conditioned (near-duplicate, not exactly collinear, columns) must
    fail closed rather than return an unstable residual -- distinct from
    the exactly-singular case."""
    rng = np.random.default_rng(9)
    base = rng.permutation(8).astype(float)
    near_duplicate = base + 1e-10 * rng.standard_normal(8)
    df = _single_period_frame(base, base)
    df["size_near_dup"] = near_duplicate
    schema = ExposureSchema(numeric_exposure_columns=["size", "size_near_dup"])

    result = neutralize_factor(df, schema)
    assert len(result["neutralized_observations"]) == 0
    assert all(
        reason == NOT_EVALUABLE_ILL_CONDITIONED_EXPOSURE_MATRIX for reason in result["excluded_periods"].values()
    )


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


# -- exposure-evaluation identity is bound to the exposure schema, but
# never changes the base evaluation_id or the factor_id -------------------
# RESEARCH-FACTOR-EXPOSURE-POINT-IN-TIME-CATEGORY-01

def _base_eval_spec() -> FactorEvaluationSpec:
    return FactorEvaluationSpec(
        factor_id="f" * 32,
        universe_identity={"universe_id": "sp500_pit_v1"},
        evaluation_window_start_utc="2024-01-01T00:00:00+00:00",
        evaluation_window_end_utc="2024-06-01T00:00:00+00:00",
        label_protocol_version="fwd_ret_label_v1",
        evaluation_protocol_version="factor_ic_ir_quantile_v1",
    )


def test_exposure_schema_change_changes_exposure_evaluation_id_only():
    base_eval_spec = _base_eval_spec()
    schema_a = ExposureSchema(numeric_exposure_columns=["size"])
    schema_b = ExposureSchema(numeric_exposure_columns=["size", "beta"])

    spec_a = FactorExposureEvaluationSpec(base_evaluation=base_eval_spec, exposure_schema_id=schema_a.compute_schema_id())
    spec_b = FactorExposureEvaluationSpec(base_evaluation=base_eval_spec, exposure_schema_id=schema_b.compute_schema_id())

    assert spec_a.compute_exposure_evaluation_id() != spec_b.compute_exposure_evaluation_id()
    # The exposure schema is bound into the exposure-evaluation identity,
    # but never mutates the underlying FactorEvaluationSpec's own identity
    # or the factor_id.
    assert spec_a.base_evaluation.compute_evaluation_id() == spec_b.base_evaluation.compute_evaluation_id()
    assert spec_a.factor_id == spec_b.factor_id == "f" * 32


def test_exposure_evaluation_id_stable_for_same_schema():
    base_eval_spec = _base_eval_spec()
    schema = ExposureSchema(numeric_exposure_columns=["size"])
    spec_a = FactorExposureEvaluationSpec(base_evaluation=base_eval_spec, exposure_schema_id=schema.compute_schema_id())
    spec_b = FactorExposureEvaluationSpec(base_evaluation=base_eval_spec, exposure_schema_id=schema.compute_schema_id())
    assert spec_a.compute_exposure_evaluation_id() == spec_b.compute_exposure_evaluation_id()


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
