"""
RESEARCH-FACTOR-IC-IR-QUANTILE-BENCH-01

Deterministic factor diagnostic evaluation: cross-sectional rank Information
Coefficient (IC), IC Information Ratio, and quantile spread diagnostics.

This module NEVER fetches market data -- it consumes a caller-supplied
canonical observations table. It NEVER promotes anything and NEVER touches
the final reserved holdout.

Degenerate/malformed inputs fail closed to a typed `not_evaluable` reason
(see NOT_EVALUABLE_* constants) -- they are never silently converted to
IC=0. Structural caller bugs (missing required columns) raise ValueError
instead, since those are not legitimate market conditions.

`label_fwd_ret` is a diagnostic LABEL, never executable P&L (see CLAUDE.md
section 19 and mqk_research.factors.contracts).
"""
from __future__ import annotations

import copy
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any, Dict, Optional

import numpy as np
import pandas as pd

from mqk_research.exp_distributed.hashing import stable_hash

from .contracts import (
    DIRECTION_HIGHER_IS_BETTER,
    DIRECTION_LOWER_IS_BETTER,
    EVAL_STATUS_NOT_EVALUABLE,
    EVAL_STATUS_SUCCEEDED,
)

FACTOR_IC_IR_PROTOCOL_VERSION = "factor_ic_ir_quantile_v1"
IC_METHOD_SPEARMAN = "spearman_rank"

SYMBOL_COL = "symbol"
PERIOD_COL = "period_ts_utc"
FACTOR_VALUE_COL = "factor_value"
LABEL_COL = "label_fwd_ret"
CUTOFF_COL = "information_cutoff_ts_utc"
LABEL_END_COL = "label_end_ts_utc"
_REQUIRED_COLUMNS = (SYMBOL_COL, PERIOD_COL, FACTOR_VALUE_COL, LABEL_COL)

NOT_EVALUABLE_ZERO_VARIANCE_FACTOR = "zero_variance_factor"
NOT_EVALUABLE_INSUFFICIENT_CROSS_SECTION = "insufficient_cross_section"
NOT_EVALUABLE_ALL_MISSING_FACTOR_VALUES = "all_missing_factor_values"
NOT_EVALUABLE_INSUFFICIENT_PERIODS = "insufficient_periods"
NOT_EVALUABLE_UNUSABLE_LABEL_POPULATION = "unusable_label_population"
NOT_EVALUABLE_NON_FINITE_INPUT = "non_finite_input"
NOT_EVALUABLE_DUPLICATE_OBSERVATION = "duplicate_symbol_period_observation"
NOT_EVALUABLE_TIME_ORDER_VIOLATION = "time_order_violation"


def _parse_ts(value: Any) -> datetime:
    text = str(value).strip().replace("Z", "+00:00")
    parsed = datetime.fromisoformat(text)
    if parsed.tzinfo is None:
        raise ValueError(f"timestamp must carry an explicit UTC offset/Z suffix: {value!r}")
    return parsed


@dataclass(frozen=True)
class FactorDiagnosticsReport:
    """Result of one deterministic IC/IR/quantile evaluation.

    `status` is one of contracts.EVAL_STATUS_SUCCEEDED /
    contracts.EVAL_STATUS_NOT_EVALUABLE. `reason` is required and typed
    whenever status is not succeeded. `metrics` is empty when not_evaluable.
    """

    status: str
    reason: Optional[str] = None
    metrics: Dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        return {"status": self.status, "reason": self.reason, "metrics": copy.deepcopy(self.metrics)}


def _not_evaluable(reason: str) -> FactorDiagnosticsReport:
    return FactorDiagnosticsReport(status=EVAL_STATUS_NOT_EVALUABLE, reason=reason, metrics={})


def _quantile_buckets(values: np.ndarray, n_quantiles: int) -> np.ndarray:
    n = len(values)
    ranks = pd.Series(values).rank(method="average").to_numpy()
    if n == 1:
        pct = np.array([0.5])
    else:
        pct = (ranks - 1.0) / (n - 1.0)
    buckets = np.floor(pct * n_quantiles).astype(int)
    return np.clip(buckets, 0, n_quantiles - 1)


def _spearman_rank_ic(factor_values: np.ndarray, label_values: np.ndarray) -> Optional[float]:
    if np.std(factor_values) <= 0.0 or np.std(label_values) <= 0.0:
        return None
    rx = pd.Series(factor_values).rank(method="average").to_numpy()
    ry = pd.Series(label_values).rank(method="average").to_numpy()
    rx_c = rx - rx.mean()
    ry_c = ry - ry.mean()
    denom = float(np.sqrt(np.sum(rx_c**2) * np.sum(ry_c**2)))
    if denom <= 0.0:
        return None
    return float(np.sum(rx_c * ry_c) / denom)


def evaluate_factor_ic_ir(
    observations: pd.DataFrame,
    *,
    direction: str = DIRECTION_HIGHER_IS_BETTER,
    n_quantiles: int = 5,
    min_cross_section: Optional[int] = None,
    min_periods: int = 2,
) -> FactorDiagnosticsReport:
    """Deterministic cross-sectional Spearman rank IC + quantile diagnostics.

    `observations` must contain columns: symbol, period_ts_utc, factor_value,
    label_fwd_ret (the diagnostic forward LABEL, never executable P&L).
    Optional columns: information_cutoff_ts_utc, label_end_ts_utc -- when
    present, causality is checked (cutoff <= period_ts < label_end_ts).

    Missing columns are a caller/programmer error and raise ValueError.
    Everything else degenerate (zero variance, insufficient cross section,
    duplicate rows, non-finite values, time-order violations, ...) fails
    closed to a typed not_evaluable report -- never silently to IC=0.
    """
    if direction not in (DIRECTION_HIGHER_IS_BETTER, DIRECTION_LOWER_IS_BETTER):
        raise ValueError(f"unknown direction: {direction!r}")
    if n_quantiles < 2:
        raise ValueError("n_quantiles must be >= 2")
    if min_periods < 1:
        raise ValueError("min_periods must be >= 1")
    effective_min_cross_section = min_cross_section if min_cross_section is not None else n_quantiles
    if effective_min_cross_section < n_quantiles:
        raise ValueError("min_cross_section must be >= n_quantiles for quantile diagnostics to be well-defined")

    missing_columns = [c for c in _REQUIRED_COLUMNS if c not in observations.columns]
    if missing_columns:
        raise ValueError(f"observations is missing required columns: {missing_columns}")

    df = observations.copy()
    total_input_rows = len(df)

    dup_mask = df.duplicated(subset=[SYMBOL_COL, PERIOD_COL], keep=False)
    if bool(dup_mask.any()):
        return _not_evaluable(NOT_EVALUABLE_DUPLICATE_OBSERVATION)

    factor_raw = df[FACTOR_VALUE_COL].to_numpy(dtype=np.float64)
    label_raw = df[LABEL_COL].to_numpy(dtype=np.float64)
    non_finite_mask = np.isinf(factor_raw) | np.isinf(label_raw)
    if bool(non_finite_mask.any()):
        return _not_evaluable(NOT_EVALUABLE_NON_FINITE_INPUT)

    if CUTOFF_COL in df.columns or LABEL_END_COL in df.columns:
        for _, row in df.iterrows():
            period_ts = _parse_ts(row[PERIOD_COL])
            if CUTOFF_COL in df.columns and pd.notna(row[CUTOFF_COL]):
                if _parse_ts(row[CUTOFF_COL]) > period_ts:
                    return _not_evaluable(NOT_EVALUABLE_TIME_ORDER_VIOLATION)
            if LABEL_END_COL in df.columns and pd.notna(row[LABEL_END_COL]):
                if _parse_ts(row[LABEL_END_COL]) <= period_ts:
                    return _not_evaluable(NOT_EVALUABLE_TIME_ORDER_VIOLATION)

    if bool(df[FACTOR_VALUE_COL].isna().all()):
        return _not_evaluable(NOT_EVALUABLE_ALL_MISSING_FACTOR_VALUES)
    if bool(df[LABEL_COL].isna().all()):
        return _not_evaluable(NOT_EVALUABLE_UNUSABLE_LABEL_POPULATION)

    usable = df.dropna(subset=[FACTOR_VALUE_COL, LABEL_COL])

    top_bucket = n_quantiles - 1 if direction == DIRECTION_HIGHER_IS_BETTER else 0
    bottom_bucket = 0 if direction == DIRECTION_HIGHER_IS_BETTER else n_quantiles - 1

    per_period_ic: Dict[str, float] = {}
    per_period_top: Dict[str, float] = {}
    per_period_bottom: Dict[str, float] = {}
    excluded_periods: Dict[str, str] = {}
    observation_count = 0

    for period_ts, group in usable.groupby(PERIOD_COL, sort=True):
        period_key = str(period_ts)
        factor_values = group[FACTOR_VALUE_COL].to_numpy(dtype=np.float64)
        label_values = group[LABEL_COL].to_numpy(dtype=np.float64)
        n = len(factor_values)
        if n < effective_min_cross_section:
            excluded_periods[period_key] = NOT_EVALUABLE_INSUFFICIENT_CROSS_SECTION
            continue
        if np.std(factor_values) <= 0.0:
            excluded_periods[period_key] = NOT_EVALUABLE_ZERO_VARIANCE_FACTOR
            continue
        if np.std(label_values) <= 0.0:
            excluded_periods[period_key] = NOT_EVALUABLE_UNUSABLE_LABEL_POPULATION
            continue

        ic = _spearman_rank_ic(factor_values, label_values)
        if ic is None:
            excluded_periods[period_key] = NOT_EVALUABLE_ZERO_VARIANCE_FACTOR
            continue

        buckets = _quantile_buckets(factor_values, n_quantiles)
        top_ret = float(label_values[buckets == top_bucket].mean())
        bottom_ret = float(label_values[buckets == bottom_bucket].mean())

        per_period_ic[period_key] = ic
        per_period_top[period_key] = top_ret
        per_period_bottom[period_key] = bottom_ret
        observation_count += n

    period_count = len(per_period_ic)
    if period_count == 0:
        reasons = set(excluded_periods.values())
        if reasons == {NOT_EVALUABLE_ZERO_VARIANCE_FACTOR}:
            return _not_evaluable(NOT_EVALUABLE_ZERO_VARIANCE_FACTOR)
        if reasons == {NOT_EVALUABLE_UNUSABLE_LABEL_POPULATION}:
            return _not_evaluable(NOT_EVALUABLE_UNUSABLE_LABEL_POPULATION)
        if reasons == {NOT_EVALUABLE_INSUFFICIENT_CROSS_SECTION}:
            return _not_evaluable(NOT_EVALUABLE_INSUFFICIENT_CROSS_SECTION)
        return _not_evaluable(NOT_EVALUABLE_INSUFFICIENT_PERIODS)
    if period_count < min_periods:
        return _not_evaluable(NOT_EVALUABLE_INSUFFICIENT_PERIODS)

    ic_values = np.array(list(per_period_ic.values()), dtype=np.float64)
    mean_ic = float(np.mean(ic_values))
    ic_stdev = float(np.std(ic_values, ddof=1)) if period_count >= 2 else 0.0
    ic_information_ratio = mean_ic / ic_stdev if ic_stdev > 1e-12 else None
    positive_ic_fraction = float(np.mean(ic_values > 0.0))

    top_mean = float(np.mean(list(per_period_top.values())))
    bottom_mean = float(np.mean(list(per_period_bottom.values())))

    metrics = {
        "protocol_version": FACTOR_IC_IR_PROTOCOL_VERSION,
        "ic_method": IC_METHOD_SPEARMAN,
        "direction": direction,
        "period_count": period_count,
        "observation_count": observation_count,
        "coverage": float(observation_count) / float(total_input_rows) if total_input_rows > 0 else 0.0,
        "mean_ic": mean_ic,
        "ic_stdev": ic_stdev,
        "ic_information_ratio": ic_information_ratio,
        "positive_ic_fraction": positive_ic_fraction,
        "per_period_ic": per_period_ic,
        "quantile": {
            "n_quantiles": n_quantiles,
            "quantile_period_count": period_count,
            "top_quantile_label_return_mean": top_mean,
            "bottom_quantile_label_return_mean": bottom_mean,
            "top_minus_bottom_spread": top_mean - bottom_mean,
        },
        "excluded_periods": excluded_periods,
    }
    return FactorDiagnosticsReport(status=EVAL_STATUS_SUCCEEDED, reason=None, metrics=metrics)


def compare_to_benchmark(
    report: FactorDiagnosticsReport,
    *,
    benchmark_identity: Dict[str, Any],
    benchmark_metrics: Dict[str, Any],
) -> Dict[str, Any]:
    """Explicit, caller-supplied benchmark/baseline comparison. Never fetches
    data itself -- both sides are already-computed metrics dicts. Comparison
    is only meaningful when `report` succeeded; callers must check
    `report.status` before relying on this."""
    if report.status != EVAL_STATUS_SUCCEEDED:
        raise ValueError("cannot compare a non-succeeded report to a benchmark")
    comparison: Dict[str, Any] = {
        "benchmark_identity": copy.deepcopy(benchmark_identity),
        "benchmark_mean_ic": benchmark_metrics.get("mean_ic"),
        "mean_ic_delta": None,
        "ic_information_ratio_delta": None,
        "top_minus_bottom_spread_delta": None,
    }
    if isinstance(benchmark_metrics.get("mean_ic"), (int, float)):
        comparison["mean_ic_delta"] = report.metrics["mean_ic"] - float(benchmark_metrics["mean_ic"])
    bench_ir = benchmark_metrics.get("ic_information_ratio")
    if isinstance(bench_ir, (int, float)) and report.metrics.get("ic_information_ratio") is not None:
        comparison["ic_information_ratio_delta"] = report.metrics["ic_information_ratio"] - float(bench_ir)
    bench_spread = (benchmark_metrics.get("quantile") or {}).get("top_minus_bottom_spread")
    if isinstance(bench_spread, (int, float)):
        comparison["top_minus_bottom_spread_delta"] = (
            report.metrics["quantile"]["top_minus_bottom_spread"] - float(bench_spread)
        )
    return comparison


def observations_content_hash(observations: pd.DataFrame) -> str:
    """Deterministic content identity of the input observations table,
    independent of row order or index -- used as the artifact's input
    provenance hash."""
    records = observations.sort_values(by=[PERIOD_COL, SYMBOL_COL]).to_dict(orient="records")
    return stable_hash({"records": records})


def build_factor_diagnostics_artifact(
    *,
    factor_identity_payload: Dict[str, Any],
    evaluation_identity_payload: Dict[str, Any],
    factor_id: str,
    evaluation_id: str,
    report: FactorDiagnosticsReport,
    input_content_sha256: str,
    holdout_status: str = "not_applicable",
    benchmark_comparison: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    """Deterministic canonical evaluation artifact payload. Result values
    (metrics) never alter factor_id/evaluation_id -- both are passed in
    already-computed, never recomputed from this artifact's own contents."""
    return {
        "schema_version": "factor_diagnostics_artifact_v1",
        "protocol_version": FACTOR_IC_IR_PROTOCOL_VERSION,
        "factor_id": factor_id,
        "evaluation_id": evaluation_id,
        "factor_identity": copy.deepcopy(factor_identity_payload),
        "evaluation_identity": copy.deepcopy(evaluation_identity_payload),
        "input_provenance": {"content_sha256": input_content_sha256},
        "holdout_status": holdout_status,
        "status": report.status,
        "reason": report.reason,
        "metrics": copy.deepcopy(report.metrics),
        "benchmark_comparison": copy.deepcopy(benchmark_comparison) if benchmark_comparison else None,
    }
