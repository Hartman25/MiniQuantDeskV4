"""
RESEARCH-FACTOR-EXPOSURE-ATTRIBUTION-01

Factor exposure and attribution diagnostics: detects whether an apparent
factor is merely a disguised common exposure (market/beta, size, sector).
Research diagnostics only -- no portfolio/risk/runtime integration, no
external factor dataset fetching, no Fama-French data invention. All
exposure values are caller-supplied columns on the same observations table
consumed by `mqk_research.factors.diagnostics`.

Neutralization is a deterministic per-PERIOD cross-sectional OLS
residualization: each period's regression uses ONLY that period's own rows,
so a later period's exposure values can never leak into an earlier period's
residual. A singular/rank-deficient design matrix for a period excludes that
period (typed reason) rather than fabricating a residual.

This module makes no causal claims -- "loses signal after neutralization"
is an observed association, not proof of mechanism.
"""
from __future__ import annotations

import copy
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

import numpy as np
import pandas as pd

from mqk_research.exp_distributed.hashing import short_hash

from .contracts import (
    DIRECTION_HIGHER_IS_BETTER,
    EVAL_STATUS_NOT_EVALUABLE,
    EVAL_STATUS_SUCCEEDED,
    FactorEvaluationSpec,
)
from .diagnostics import (
    FACTOR_VALUE_COL,
    PERIOD_COL,
    _quantile_buckets,
    _spearman_rank_ic,
    evaluate_factor_ic_ir,
)

EXPOSURE_SCHEMA_PROTOCOL_VERSION = "factor_exposure_schema_v1"
EXPOSURE_ATTRIBUTION_PROTOCOL_VERSION = "factor_exposure_attribution_v1"

NOT_EVALUABLE_SINGULAR_EXPOSURE_MATRIX = "singular_exposure_design_matrix"
NOT_EVALUABLE_INSUFFICIENT_PERIODS_FOR_NEUTRALIZATION = "insufficient_periods_for_neutralization"
NOT_EVALUABLE_BASELINE_NOT_EVALUABLE = "baseline_not_evaluable"

# RESEARCH-FACTOR-DIAGNOSTICS-CAUSALITY-AND-NUMERICS-01: an exactly (or
# near-exactly) collinear per-period fit leaves an OLS residual that is pure
# floating-point dust (~1e-16), never exact 0.0, but whose apparent "rank
# order" is numerical noise, not signal. Comparing that dust to ITS OWN
# scale cannot distinguish it from genuine tiny-scale signal (dust vs dust
# looks like real relative variation). The only sound comparison is against
# the ORIGINAL pre-residual regression scale -- this is the one place in the
# factor pipeline allowed to do that, per CLAUDE.md/mission: the generic
# rank evaluator (diagnostics._is_effectively_constant) stays purely
# relative-to-its-own-data and must never carry this absolute-feeling floor.
_RESIDUAL_DUST_REL_TOL = 1e-9


@dataclass(frozen=True)
class ExposureSchema:
    """Identity-bearing declaration of which observation columns are
    exposures and how they are typed. A schema change (added/removed
    column, numeric<->categorical retyping) changes `compute_schema_id()`."""

    numeric_exposure_columns: List[str] = field(default_factory=list)
    categorical_exposure_columns: List[str] = field(default_factory=list)
    protocol_version: str = EXPOSURE_SCHEMA_PROTOCOL_VERSION

    def validate(self) -> None:
        if not self.numeric_exposure_columns and not self.categorical_exposure_columns:
            raise ValueError("ExposureSchema requires at least one numeric or categorical exposure column")
        overlap = set(self.numeric_exposure_columns) & set(self.categorical_exposure_columns)
        if overlap:
            raise ValueError(f"ExposureSchema columns cannot be both numeric and categorical: {sorted(overlap)}")
        if not self.protocol_version.strip():
            raise ValueError("ExposureSchema.protocol_version is required")

    def identity_payload(self) -> Dict[str, Any]:
        return {
            "protocol_version": self.protocol_version,
            "numeric_exposure_columns": sorted(self.numeric_exposure_columns),
            "categorical_exposure_columns": sorted(self.categorical_exposure_columns),
        }

    def compute_schema_id(self) -> str:
        self.validate()
        return short_hash(self.identity_payload(), length=32)


@dataclass(frozen=True)
class FactorExposureEvaluationSpec:
    """Identity-bearing binding of a base `FactorEvaluationSpec` to an
    exposure schema and attribution protocol (RESEARCH-FACTOR-EXPOSURE-
    POINT-IN-TIME-CATEGORY-01). Mirrors `null_controls.FactorNullControlSpec`
    -- wraps `base_evaluation` rather than duplicating its fields, so this
    spec can never drift from that contract.

    Changing the exposure schema (or attribution protocol) changes
    `compute_exposure_evaluation_id()` -- which exposure-evaluation artifact
    this analysis is filed under -- but NEVER `base_evaluation`'s own
    `compute_evaluation_id()`, and never the underlying `factor_id`."""

    base_evaluation: FactorEvaluationSpec
    exposure_schema_id: str
    exposure_attribution_protocol_version: str = EXPOSURE_ATTRIBUTION_PROTOCOL_VERSION

    @property
    def factor_id(self) -> str:
        return self.base_evaluation.factor_id

    def validate(self) -> None:
        self.base_evaluation.validate()
        if not self.exposure_schema_id.strip():
            raise ValueError("FactorExposureEvaluationSpec.exposure_schema_id is required")
        if not self.exposure_attribution_protocol_version.strip():
            raise ValueError("FactorExposureEvaluationSpec.exposure_attribution_protocol_version is required")

    def identity_payload(self) -> Dict[str, Any]:
        return {
            "exposure_attribution_protocol_version": self.exposure_attribution_protocol_version,
            "base_evaluation_identity": copy.deepcopy(self.base_evaluation.identity_payload()),
            "exposure_schema_id": self.exposure_schema_id,
        }

    def compute_exposure_evaluation_id(self) -> str:
        self.validate()
        return short_hash(self.identity_payload(), length=32)


@dataclass(frozen=True)
class FactorExposureReport:
    status: str
    reason: Optional[str] = None
    metrics: Dict[str, Any] = field(default_factory=dict)


def _categorical_vocab(observations: pd.DataFrame, schema: ExposureSchema) -> Dict[str, List[str]]:
    return {
        col: sorted(observations[col].astype(str).unique().tolist())
        for col in schema.categorical_exposure_columns
    }


def _design_column_count(schema: ExposureSchema, vocab: Dict[str, List[str]]) -> int:
    return 1 + len(schema.numeric_exposure_columns) + sum(max(len(v) - 1, 0) for v in vocab.values())


def _build_design_matrix(group: pd.DataFrame, schema: ExposureSchema, vocab: Dict[str, List[str]]) -> np.ndarray:
    n = len(group)
    columns = [np.ones(n)]
    for col in schema.numeric_exposure_columns:
        columns.append(group[col].to_numpy(dtype=np.float64))
    for col in schema.categorical_exposure_columns:
        categories = vocab[col]
        for category in categories[1:]:  # drop first category as reference
            columns.append((group[col].astype(str) == category).to_numpy(dtype=np.float64))
    return np.column_stack(columns)


def neutralize_factor(
    observations: pd.DataFrame,
    schema: ExposureSchema,
    *,
    min_cross_section_margin: int = 1,
) -> Dict[str, Any]:
    """Deterministic per-period cross-sectional OLS residualization of
    `factor_value` against the supplied exposures. Fits each period using
    ONLY that period's own rows -- including the CATEGORICAL VOCABULARY
    itself (RESEARCH-FACTOR-EXPOSURE-POINT-IN-TIME-CATEGORY-01): a category
    level that first appears in a later period is never used to define an
    earlier period's design matrix, so its future existence can never change
    an earlier period's residual or evaluability decision. Returns a dict
    with:
      - 'neutralized_observations': a copy of `observations` (restricted to
        well-posed periods) with `factor_value` replaced by the OLS residual.
      - 'excluded_periods': period_key -> typed reason (never a fabricated
        residual for a singular/underdetermined period).
      - 'period_design_column_counts' / 'period_categorical_vocab': per-
        period design/schema evidence, so the point-in-time protocol is
        auditable rather than only asserted.
    """
    schema.validate()
    exposure_cols = schema.numeric_exposure_columns + schema.categorical_exposure_columns
    missing_cols = [c for c in exposure_cols if c not in observations.columns]
    if missing_cols:
        raise ValueError(f"observations is missing exposure columns: {missing_cols}")

    usable = observations.dropna(subset=[FACTOR_VALUE_COL] + exposure_cols)

    parts = []
    excluded: Dict[str, str] = {}
    period_design_column_counts: Dict[str, int] = {}
    period_categorical_vocab: Dict[str, Dict[str, List[str]]] = {}
    for period, group in usable.groupby(PERIOD_COL, sort=True):
        period_key = str(period)
        # Point-in-time-safe: vocabulary derived from THIS period's rows
        # only, never from the full (past+future) dataset.
        vocab = _categorical_vocab(group, schema)
        n_design_cols = _design_column_count(schema, vocab)
        if len(group) < n_design_cols + min_cross_section_margin:
            excluded[period_key] = "insufficient_cross_section_for_regression"
            continue
        design = _build_design_matrix(group, schema, vocab)
        if np.linalg.matrix_rank(design) < design.shape[1]:
            excluded[period_key] = NOT_EVALUABLE_SINGULAR_EXPOSURE_MATRIX
            continue
        y = group[FACTOR_VALUE_COL].to_numpy(dtype=np.float64)
        coef, _, _, _ = np.linalg.lstsq(design, y, rcond=None)
        residual = y - design @ coef
        # Exact-collinear numerical dust: the residual is dust RELATIVE TO
        # THE ORIGINAL FACTOR SCALE for this period, not relative to its own
        # (also-tiny) internal spread -- force it to exact zero so the
        # generic rank evaluator deterministically treats it as constant
        # rather than as accidental sub-machine-epsilon "signal".
        y_scale = float(np.max(np.abs(y))) if y.size else 0.0
        residual_scale = float(np.max(np.abs(residual))) if residual.size else 0.0
        if y_scale > 0.0 and residual_scale <= y_scale * _RESIDUAL_DUST_REL_TOL:
            residual = np.zeros_like(residual)
        neutralized_group = group.copy()
        neutralized_group[FACTOR_VALUE_COL] = residual
        parts.append(neutralized_group)
        period_design_column_counts[period_key] = n_design_cols
        period_categorical_vocab[period_key] = vocab

    neutralized_observations = pd.concat(parts, ignore_index=True) if parts else observations.iloc[0:0].copy()
    return {
        "neutralized_observations": neutralized_observations,
        "excluded_periods": excluded,
        "period_design_column_counts": period_design_column_counts,
        "period_categorical_vocab": period_categorical_vocab,
    }


def compute_exposure_association(
    observations: pd.DataFrame, schema: ExposureSchema, *, min_cross_section: int = 3
) -> Dict[str, Optional[float]]:
    """Mean per-period cross-sectional Spearman rank correlation between
    `factor_value` and each NUMERIC exposure column. `None` for a column
    with no computable period (e.g. always zero-variance)."""
    schema.validate()
    associations: Dict[str, Optional[float]] = {}
    for col in schema.numeric_exposure_columns:
        per_period_ic = []
        for _, group in observations.dropna(subset=[FACTOR_VALUE_COL, col]).groupby(PERIOD_COL, sort=True):
            if len(group) < min_cross_section:
                continue
            ic = _spearman_rank_ic(
                group[FACTOR_VALUE_COL].to_numpy(dtype=np.float64), group[col].to_numpy(dtype=np.float64)
            )
            if ic is not None:
                per_period_ic.append(ic)
        associations[col] = float(np.mean(per_period_ic)) if per_period_ic else None
    return associations


def _average_fraction_dicts(dicts: List[Dict[str, float]]) -> Dict[str, float]:
    if not dicts:
        return {}
    totals: Dict[str, float] = {}
    for d in dicts:
        for key, value in d.items():
            totals[key] = totals.get(key, 0.0) + value
    n = len(dicts)
    return {key: value / n for key, value in sorted(totals.items())}


def compute_quantile_exposure_profile(
    observations: pd.DataFrame,
    schema: ExposureSchema,
    *,
    direction: str = DIRECTION_HIGHER_IS_BETTER,
    n_quantiles: int = 5,
    min_cross_section: Optional[int] = None,
) -> Dict[str, Any]:
    """Cross-sectional exposure profile by factor quantile: for numeric
    exposures, the mean exposure value in the top vs bottom factor bucket
    (averaged across periods); for categorical exposures, the category
    composition fraction in the top vs bottom bucket (averaged across
    periods)."""
    schema.validate()
    effective_min_cross_section = min_cross_section if min_cross_section is not None else n_quantiles
    top_bucket = n_quantiles - 1 if direction == DIRECTION_HIGHER_IS_BETTER else 0
    bottom_bucket = 0 if direction == DIRECTION_HIGHER_IS_BETTER else n_quantiles - 1

    numeric_top: Dict[str, List[float]] = {col: [] for col in schema.numeric_exposure_columns}
    numeric_bottom: Dict[str, List[float]] = {col: [] for col in schema.numeric_exposure_columns}
    categorical_top: Dict[str, List[Dict[str, float]]] = {col: [] for col in schema.categorical_exposure_columns}
    categorical_bottom: Dict[str, List[Dict[str, float]]] = {col: [] for col in schema.categorical_exposure_columns}

    for _, group in observations.dropna(subset=[FACTOR_VALUE_COL]).groupby(PERIOD_COL, sort=True):
        if len(group) < effective_min_cross_section:
            continue
        buckets = _quantile_buckets(group[FACTOR_VALUE_COL].to_numpy(dtype=np.float64), n_quantiles)
        top_mask = buckets == top_bucket
        bottom_mask = buckets == bottom_bucket
        for col in schema.numeric_exposure_columns:
            top_vals = group.loc[top_mask, col].dropna()
            bottom_vals = group.loc[bottom_mask, col].dropna()
            if len(top_vals):
                numeric_top[col].append(float(top_vals.mean()))
            if len(bottom_vals):
                numeric_bottom[col].append(float(bottom_vals.mean()))
        for col in schema.categorical_exposure_columns:
            categorical_top[col].append(group.loc[top_mask, col].astype(str).value_counts(normalize=True).to_dict())
            categorical_bottom[col].append(
                group.loc[bottom_mask, col].astype(str).value_counts(normalize=True).to_dict()
            )

    numeric_profile = {
        col: {
            "top_quantile_mean": float(np.mean(numeric_top[col])) if numeric_top[col] else None,
            "bottom_quantile_mean": float(np.mean(numeric_bottom[col])) if numeric_bottom[col] else None,
        }
        for col in schema.numeric_exposure_columns
    }
    categorical_profile = {
        col: {
            "top_quantile_category_fraction": _average_fraction_dicts(categorical_top[col]),
            "bottom_quantile_category_fraction": _average_fraction_dicts(categorical_bottom[col]),
        }
        for col in schema.categorical_exposure_columns
    }
    return {"n_quantiles": n_quantiles, "numeric": numeric_profile, "categorical": categorical_profile}


def evaluate_factor_exposure_diagnostics(
    observations: pd.DataFrame,
    schema: ExposureSchema,
    *,
    direction: str = DIRECTION_HIGHER_IS_BETTER,
    n_quantiles: int = 5,
    min_cross_section: Optional[int] = None,
    min_periods: int = 2,
) -> FactorExposureReport:
    """Orchestrates the exposure/attribution diagnostics for one factor:
    baseline IC/IR, exposure association, quantile exposure profile, and a
    neutralized (residualized) re-evaluation. Never claims causal
    attribution -- only reports observed associations and the change in
    IC/spread after neutralization."""
    schema.validate()
    effective_min_cross_section = min_cross_section if min_cross_section is not None else n_quantiles

    baseline = evaluate_factor_ic_ir(
        observations,
        direction=direction,
        n_quantiles=n_quantiles,
        min_cross_section=effective_min_cross_section,
        min_periods=min_periods,
    )
    if baseline.status != EVAL_STATUS_SUCCEEDED:
        return FactorExposureReport(
            status=EVAL_STATUS_NOT_EVALUABLE,
            reason=f"{NOT_EVALUABLE_BASELINE_NOT_EVALUABLE}:{baseline.reason}",
            metrics={},
        )

    association = compute_exposure_association(observations, schema, min_cross_section=effective_min_cross_section)
    quantile_profile = compute_quantile_exposure_profile(
        observations, schema, direction=direction, n_quantiles=n_quantiles, min_cross_section=effective_min_cross_section
    )
    neutralization = neutralize_factor(observations, schema)
    neutralized_observations = neutralization["neutralized_observations"]

    if len(neutralized_observations) == 0:
        return FactorExposureReport(
            status=EVAL_STATUS_NOT_EVALUABLE,
            reason=NOT_EVALUABLE_INSUFFICIENT_PERIODS_FOR_NEUTRALIZATION,
            metrics={
                "baseline_mean_ic": baseline.metrics["mean_ic"],
                "association": association,
                "quantile_exposure_profile": quantile_profile,
                "excluded_neutralization_periods": neutralization["excluded_periods"],
            },
        )

    neutralized_report = evaluate_factor_ic_ir(
        neutralized_observations,
        direction=direction,
        n_quantiles=n_quantiles,
        min_cross_section=effective_min_cross_section,
        min_periods=min_periods,
    )
    neutralized_mean_ic = neutralized_report.metrics.get("mean_ic") if neutralized_report.status == EVAL_STATUS_SUCCEEDED else None
    neutralized_spread = (
        neutralized_report.metrics.get("quantile", {}).get("top_minus_bottom_spread")
        if neutralized_report.status == EVAL_STATUS_SUCCEEDED
        else None
    )

    metrics = {
        "protocol_version": EXPOSURE_ATTRIBUTION_PROTOCOL_VERSION,
        "exposure_schema_id": schema.compute_schema_id(),
        "baseline_mean_ic": baseline.metrics["mean_ic"],
        "baseline_top_minus_bottom_spread": baseline.metrics["quantile"]["top_minus_bottom_spread"],
        "association": association,
        "quantile_exposure_profile": quantile_profile,
        "excluded_neutralization_periods": neutralization["excluded_periods"],
        "neutralized_status": neutralized_report.status,
        "neutralized_reason": neutralized_report.reason,
        "neutralized_mean_ic": neutralized_mean_ic,
        "neutralized_top_minus_bottom_spread": neutralized_spread,
        "mean_ic_delta_after_neutralization": (
            neutralized_mean_ic - baseline.metrics["mean_ic"] if neutralized_mean_ic is not None else None
        ),
    }
    return FactorExposureReport(status=EVAL_STATUS_SUCCEEDED, reason=None, metrics=metrics)
