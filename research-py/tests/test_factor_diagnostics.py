"""
RESEARCH-FACTOR-IC-IR-QUANTILE-BENCH-01 -- IC/IR/quantile diagnostic tests.

Synthetic controls proving: perfect/reversed monotonic predictors produce
correctly-signed strong rank IC, weak/null orderings are materially weaker,
and every named degenerate case fails closed to a typed not_evaluable
reason rather than silently becoming IC=0.
"""
from __future__ import annotations

import numpy as np
import pandas as pd
import pytest

from mqk_research.factors.contracts import (
    DIRECTION_HIGHER_IS_BETTER,
    DIRECTION_LOWER_IS_BETTER,
    EVAL_STATUS_NOT_EVALUABLE,
    EVAL_STATUS_SUCCEEDED,
    FactorEvaluationSpec,
    FactorSpec,
    NORMALIZATION_CROSS_SECTIONAL_RANK,
    TIMING_NEXT_BAR_TRADABLE,
)
from mqk_research.factors.diagnostics import (
    NOT_EVALUABLE_ALL_MISSING_FACTOR_VALUES,
    NOT_EVALUABLE_DUPLICATE_OBSERVATION,
    NOT_EVALUABLE_EMPTY_QUANTILE_BUCKET,
    NOT_EVALUABLE_INSUFFICIENT_CROSS_SECTION,
    NOT_EVALUABLE_INSUFFICIENT_PERIODS,
    NOT_EVALUABLE_MISSING_CAUSAL_METADATA,
    NOT_EVALUABLE_NON_FINITE_INPUT,
    NOT_EVALUABLE_TIME_ORDER_VIOLATION,
    NOT_EVALUABLE_UNUSABLE_LABEL_POPULATION,
    NOT_EVALUABLE_ZERO_VARIANCE_FACTOR,
    build_factor_diagnostics_artifact,
    compare_to_benchmark,
    evaluate_factor_ic_ir,
    observations_content_hash,
)

N_SYMBOLS = 6
N_PERIODS = 8

# Fixed causal columns for fixtures that don't specifically exercise
# causality: cutoff == period_ts_utc (satisfies cutoff <= period) and a
# far-future label_end (satisfies period < label_end) for every row.
_FAR_FUTURE_LABEL_END = "2099-01-01T00:00:00+00:00"


def _periods():
    return [f"2024-01-{d:02d}T00:00:00+00:00" for d in range(1, N_PERIODS + 1)]


def _symbols():
    return [f"SYM{i}" for i in range(N_SYMBOLS)]


def _with_causal_columns(df: pd.DataFrame) -> pd.DataFrame:
    df = df.copy()
    df["information_cutoff_ts_utc"] = df["period_ts_utc"]
    df["label_end_ts_utc"] = _FAR_FUTURE_LABEL_END
    return df


def _monotonic_dataset(*, reversed_: bool = False) -> pd.DataFrame:
    rows = []
    for period in _periods():
        for i, sym in enumerate(_symbols()):
            factor_value = float(i)
            label = float(-i) if reversed_ else float(i)
            rows.append(
                {"symbol": sym, "period_ts_utc": period, "factor_value": factor_value, "label_fwd_ret": label}
            )
    return _with_causal_columns(pd.DataFrame(rows))


def _null_like_dataset(seed: int = 7) -> pd.DataFrame:
    rng = np.random.default_rng(seed)
    rows = []
    for period in _periods():
        factor_values = rng.permutation(N_SYMBOLS).astype(float)
        label_values = rng.permutation(N_SYMBOLS).astype(float)
        for sym, fv, lv in zip(_symbols(), factor_values, label_values):
            rows.append({"symbol": sym, "period_ts_utc": period, "factor_value": fv, "label_fwd_ret": lv})
    return _with_causal_columns(pd.DataFrame(rows))


def _constant_factor_dataset() -> pd.DataFrame:
    rows = []
    for period in _periods():
        for i, sym in enumerate(_symbols()):
            rows.append(
                {"symbol": sym, "period_ts_utc": period, "factor_value": 1.0, "label_fwd_ret": float(i)}
            )
    return _with_causal_columns(pd.DataFrame(rows))


# -- perfect / reversed / null synthetic controls -------------------------

def test_perfect_monotonic_predictor_strongly_positive_ic():
    report = evaluate_factor_ic_ir(_monotonic_dataset(), n_quantiles=3, min_cross_section=3)
    assert report.status == EVAL_STATUS_SUCCEEDED
    assert report.metrics["mean_ic"] > 0.95
    assert report.metrics["positive_ic_fraction"] == 1.0
    assert report.metrics["quantile"]["top_minus_bottom_spread"] > 0


def test_reversed_predictor_strongly_negative_ic():
    report = evaluate_factor_ic_ir(_monotonic_dataset(reversed_=True), n_quantiles=3, min_cross_section=3)
    assert report.status == EVAL_STATUS_SUCCEEDED
    assert report.metrics["mean_ic"] < -0.95
    assert report.metrics["quantile"]["top_minus_bottom_spread"] < 0


def test_null_like_ordering_materially_weaker_than_perfect():
    perfect = evaluate_factor_ic_ir(_monotonic_dataset(), n_quantiles=3, min_cross_section=3)
    null_like = evaluate_factor_ic_ir(_null_like_dataset(), n_quantiles=3, min_cross_section=3)
    assert null_like.status == EVAL_STATUS_SUCCEEDED
    assert abs(null_like.metrics["mean_ic"]) < 0.5
    assert abs(null_like.metrics["mean_ic"]) < abs(perfect.metrics["mean_ic"])


def test_direction_lower_is_better_flips_top_bottom():
    higher = evaluate_factor_ic_ir(
        _monotonic_dataset(), direction=DIRECTION_HIGHER_IS_BETTER, n_quantiles=3, min_cross_section=3
    )
    lower = evaluate_factor_ic_ir(
        _monotonic_dataset(), direction=DIRECTION_LOWER_IS_BETTER, n_quantiles=3, min_cross_section=3
    )
    assert higher.metrics["quantile"]["top_minus_bottom_spread"] > 0
    assert lower.metrics["quantile"]["top_minus_bottom_spread"] < 0


# -- degenerate cases fail closed, never IC=0 ------------------------------

def test_constant_factor_not_evaluable():
    report = evaluate_factor_ic_ir(_constant_factor_dataset(), n_quantiles=3, min_cross_section=3)
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    assert report.reason == NOT_EVALUABLE_ZERO_VARIANCE_FACTOR
    assert report.metrics == {}


def test_duplicate_observation_fails_closed():
    df = _monotonic_dataset()
    dup_row = df.iloc[[0]].copy()
    df = pd.concat([df, dup_row], ignore_index=True)
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    assert report.reason == NOT_EVALUABLE_DUPLICATE_OBSERVATION


def test_future_information_mutation_fails_closed():
    df = _monotonic_dataset()
    df["information_cutoff_ts_utc"] = df["period_ts_utc"]
    # Mutate one row's cutoff to AFTER its observation -- a lookahead leak.
    df.loc[0, "information_cutoff_ts_utc"] = "2099-01-01T00:00:00+00:00"
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    assert report.reason == NOT_EVALUABLE_TIME_ORDER_VIOLATION


def test_label_end_before_period_fails_closed():
    df = _monotonic_dataset()
    df["label_end_ts_utc"] = df["period_ts_utc"]  # label "ends" at the SAME instant -> not future
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    assert report.reason == NOT_EVALUABLE_TIME_ORDER_VIOLATION


def test_all_missing_factor_values_not_evaluable():
    df = _monotonic_dataset()
    df["factor_value"] = np.nan
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    assert report.reason == NOT_EVALUABLE_ALL_MISSING_FACTOR_VALUES


def test_unusable_label_population_not_evaluable():
    df = _monotonic_dataset()
    df["label_fwd_ret"] = np.nan
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    assert report.reason == NOT_EVALUABLE_UNUSABLE_LABEL_POPULATION


def test_non_finite_input_not_evaluable():
    df = _monotonic_dataset()
    df.loc[0, "factor_value"] = float("inf")
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    assert report.reason == NOT_EVALUABLE_NON_FINITE_INPUT


def test_insufficient_cross_section_not_evaluable():
    # Only 2 symbols per period but min_cross_section requires 3.
    rows = []
    for period in _periods():
        for i in range(2):
            rows.append(
                {"symbol": f"SYM{i}", "period_ts_utc": period, "factor_value": float(i), "label_fwd_ret": float(i)}
            )
    df = _with_causal_columns(pd.DataFrame(rows))
    report = evaluate_factor_ic_ir(df, n_quantiles=2, min_cross_section=3)
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    assert report.reason == NOT_EVALUABLE_INSUFFICIENT_CROSS_SECTION


def test_missing_causal_cutoff_on_usable_row_fails_closed():
    # RESEARCH-FACTOR-DIAGNOSTICS-CAUSALITY-AND-NUMERICS-01: a usable row
    # (non-null factor/label) with a missing cutoff must fail closed, not be
    # silently treated as "no causal claim made".
    df = _monotonic_dataset()
    df.loc[0, "information_cutoff_ts_utc"] = None
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    assert report.reason == NOT_EVALUABLE_MISSING_CAUSAL_METADATA
    assert report.metrics == {}


def test_missing_causal_label_end_on_usable_row_fails_closed():
    df = _monotonic_dataset()
    df.loc[0, "label_end_ts_utc"] = None
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    assert report.reason == NOT_EVALUABLE_MISSING_CAUSAL_METADATA


def test_unparseable_causal_cutoff_fails_closed():
    df = _monotonic_dataset()
    df.loc[0, "information_cutoff_ts_utc"] = "not-a-timestamp"
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    assert report.reason == NOT_EVALUABLE_MISSING_CAUSAL_METADATA


# -- Spearman rank evaluability/IC are invariant to positive rescaling -----
# RESEARCH-FACTOR-DIAGNOSTICS-CAUSALITY-AND-NUMERICS-01

def test_rank_evaluability_and_ic_are_scale_invariant():
    df = _monotonic_dataset()
    scaled = df.copy()
    scaled["factor_value"] = scaled["factor_value"] * 1e-12
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)
    report_scaled = evaluate_factor_ic_ir(scaled, n_quantiles=3, min_cross_section=3)
    assert report.status == report_scaled.status == EVAL_STATUS_SUCCEEDED
    assert report.metrics["mean_ic"] == pytest.approx(report_scaled.metrics["mean_ic"])


def test_reversed_predictor_evaluability_and_ic_are_scale_invariant():
    df = _monotonic_dataset(reversed_=True)
    scaled = df.copy()
    scaled["factor_value"] = scaled["factor_value"] * 1e-12
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)
    report_scaled = evaluate_factor_ic_ir(scaled, n_quantiles=3, min_cross_section=3)
    assert report.status == report_scaled.status == EVAL_STATUS_SUCCEEDED
    assert report.metrics["mean_ic"] == pytest.approx(report_scaled.metrics["mean_ic"])


# -- an empty required tail quantile bucket under ties fails closed --------
# RESEARCH-FACTOR-DIAGNOSTICS-CAUSALITY-AND-NUMERICS-01

def _tied_quantile_dataset(n_periods: int = 4) -> pd.DataFrame:
    """factor_value = [1, 2, 2, 2, 2] every period: non-constant (real
    variance), but average-rank tie-breaking under n_quantiles=5 leaves the
    top bucket with no observation."""
    rows = []
    factor_values = [1.0, 2.0, 2.0, 2.0, 2.0]
    label_values = [0.0, 1.0, 2.0, 3.0, 4.0]
    for period in _periods()[:n_periods]:
        for i, (fv, lv) in enumerate(zip(factor_values, label_values)):
            rows.append({"symbol": f"SYM{i}", "period_ts_utc": period, "factor_value": fv, "label_fwd_ret": lv})
    return _with_causal_columns(pd.DataFrame(rows))


def test_tied_factor_values_empty_tail_bucket_fails_closed():
    df = _tied_quantile_dataset()
    report = evaluate_factor_ic_ir(df, n_quantiles=5)
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    assert report.reason == NOT_EVALUABLE_EMPTY_QUANTILE_BUCKET
    assert report.metrics == {}


def test_no_succeeded_report_ever_contains_nan_or_inf():
    import json
    import math

    df = _tied_quantile_dataset()
    report = evaluate_factor_ic_ir(df, n_quantiles=5)
    # not_evaluable here (empty tail bucket), so metrics must be empty --
    # this is the direct regression proof for the historical NaN leak.
    assert report.metrics == {}

    ok_report = evaluate_factor_ic_ir(_monotonic_dataset(), n_quantiles=3, min_cross_section=3)
    assert ok_report.status == EVAL_STATUS_SUCCEEDED
    flat = json.loads(json.dumps(ok_report.to_dict()))

    def _assert_finite(node):
        if isinstance(node, dict):
            for v in node.values():
                _assert_finite(v)
        elif isinstance(node, list):
            for v in node:
                _assert_finite(v)
        elif isinstance(node, float):
            assert not math.isnan(node) and not math.isinf(node)

    _assert_finite(flat)


def test_insufficient_periods_not_evaluable():
    df = _monotonic_dataset()
    df = df[df["period_ts_utc"] == _periods()[0]]  # collapse to a single period
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3, min_periods=2)
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    assert report.reason == NOT_EVALUABLE_INSUFFICIENT_PERIODS


def test_missing_required_columns_raises():
    df = _monotonic_dataset().drop(columns=["label_fwd_ret"])
    with pytest.raises(ValueError, match="missing required columns"):
        evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)


# -- result changes do not change factor_id / evaluation_id ----------------

def test_result_changes_do_not_change_factor_or_evaluation_identity():
    spec = FactorSpec(
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
    eval_spec = FactorEvaluationSpec(
        factor_id=spec.compute_factor_id(),
        universe_identity={"universe_id": "sp500_pit_v1"},
        evaluation_window_start_utc="2024-01-01T00:00:00+00:00",
        evaluation_window_end_utc="2024-06-01T00:00:00+00:00",
        label_protocol_version="fwd_ret_label_v1",
        evaluation_protocol_version="factor_ic_ir_quantile_v1",
    )
    factor_id_before = spec.compute_factor_id()
    evaluation_id_before = eval_spec.compute_evaluation_id()

    report_perfect = evaluate_factor_ic_ir(_monotonic_dataset(), n_quantiles=3, min_cross_section=3)
    report_reversed = evaluate_factor_ic_ir(_monotonic_dataset(reversed_=True), n_quantiles=3, min_cross_section=3)
    assert report_perfect.metrics["mean_ic"] != report_reversed.metrics["mean_ic"]

    assert spec.compute_factor_id() == factor_id_before
    assert eval_spec.compute_evaluation_id() == evaluation_id_before


# -- artifact + benchmark comparison ---------------------------------------

def test_artifact_is_deterministic_and_json_serializable():
    import json

    df = _monotonic_dataset()
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)
    content_hash_a = observations_content_hash(df)
    content_hash_b = observations_content_hash(df.sample(frac=1, random_state=1))  # shuffled rows
    assert content_hash_a == content_hash_b  # order-independent

    artifact = build_factor_diagnostics_artifact(
        factor_identity_payload={"family": "momentum"},
        evaluation_identity_payload={"factor_id": "abc"},
        factor_id="abc",
        evaluation_id="def",
        report=report,
        input_content_sha256=content_hash_a,
    )
    text = json.dumps(artifact, sort_keys=True, separators=(",", ":"))
    assert json.loads(text) == artifact
    assert artifact["status"] == EVAL_STATUS_SUCCEEDED
    assert artifact["factor_id"] == "abc"


def test_benchmark_comparison_requires_succeeded_report():
    df = _constant_factor_dataset()
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)
    assert report.status == EVAL_STATUS_NOT_EVALUABLE
    with pytest.raises(ValueError):
        compare_to_benchmark(report, benchmark_identity={}, benchmark_metrics={"mean_ic": 0.1})


def test_benchmark_comparison_computes_deltas():
    # Null-like data varies IC period-to-period (non-zero stdev), so
    # ic_information_ratio is defined -- unlike the perfectly monotonic
    # fixture, whose identical per-period IC gives a zero stdev / None IR.
    df = _null_like_dataset()
    report = evaluate_factor_ic_ir(df, n_quantiles=3, min_cross_section=3)
    assert report.metrics["ic_information_ratio"] is not None
    comparison = compare_to_benchmark(
        report,
        benchmark_identity={"factor_id": "baseline123"},
        benchmark_metrics={"mean_ic": 0.1, "ic_information_ratio": 0.5},
    )
    assert comparison["mean_ic_delta"] == pytest.approx(report.metrics["mean_ic"] - 0.1)
    assert comparison["ic_information_ratio_delta"] == pytest.approx(
        report.metrics["ic_information_ratio"] - 0.5
    )
