"""
RESEARCH-FACTOR-NULL-CONTROLS-01

Deterministic null/placebo controls for factor diagnostic evaluation:
  - cross-sectional permutation: shuffles factor values WITHIN each period,
    breaking the cross-sectional factor<->label relationship while
    preserving each period's marginal distributions.
  - temporal offset: shifts each symbol's factor series forward in time by a
    fixed number of periods, breaking genuine temporal alignment while
    preserving both the cross-sectional factor distribution and the label
    distribution.

A null control is an EVALUATION SLICE of the same factor candidate -- it is
registered under the SAME factor_id via the Patch A registry, never a new
factor identity, new hypothesis, or new trial. See
mqk_research.factors.registry.begin_factor_evaluation /
mqk_research.factors.contracts.FactorEvaluationSpec for that registry
contract; this module only adds a distinct, seed-bearing evaluation-slice
identity (FactorNullControlSpec) layered on top of it.

No threshold tuning: `compare_real_vs_null` reports numbers, never invents a
pass/fail significance claim.
"""
from __future__ import annotations

import copy
from dataclasses import dataclass
from typing import Any, Dict

import numpy as np
import pandas as pd

from mqk_research.exp_distributed.hashing import short_hash, stable_hash

from .contracts import EVAL_STATUS_SUCCEEDED, FactorEvaluationSpec
from .diagnostics import FACTOR_VALUE_COL, PERIOD_COL, SYMBOL_COL, FactorDiagnosticsReport

NULL_CONTROL_PROTOCOL_VERSION = "factor_null_control_v1"

CONTROL_KIND_CROSS_SECTIONAL_PERMUTATION = "cross_sectional_permutation"
CONTROL_KIND_TEMPORAL_OFFSET = "temporal_offset"
_VALID_CONTROL_KINDS = (CONTROL_KIND_CROSS_SECTIONAL_PERMUTATION, CONTROL_KIND_TEMPORAL_OFFSET)


@dataclass(frozen=True)
class FactorNullControlSpec:
    """Identity-bearing binding of a null/placebo control to a base
    evaluation. Wraps `FactorEvaluationSpec` rather than duplicating its
    fields -- `factor_id` and every evaluation-binding field come from
    `base_evaluation`, so this spec can never drift from that contract.

    For `control_kind == temporal_offset`, `seed` IS the offset (in periods)
    -- a single deterministic integer parameterizes both reproducibility and
    the transform itself, so there is no hidden, unseeded parameter.
    """

    base_evaluation: FactorEvaluationSpec
    control_kind: str
    seed: int
    control_protocol_version: str = NULL_CONTROL_PROTOCOL_VERSION

    @property
    def factor_id(self) -> str:
        return self.base_evaluation.factor_id

    def validate(self) -> None:
        self.base_evaluation.validate()
        if self.control_kind not in _VALID_CONTROL_KINDS:
            raise ValueError(f"control_kind must be one of {_VALID_CONTROL_KINDS}: got {self.control_kind!r}")
        if not isinstance(self.seed, int) or isinstance(self.seed, bool):
            raise ValueError("seed must be an int")
        if self.control_kind == CONTROL_KIND_TEMPORAL_OFFSET and self.seed < 1:
            raise ValueError("temporal_offset control requires seed (offset periods) >= 1")
        if not self.control_protocol_version.strip():
            raise ValueError("control_protocol_version is required")

    def identity_payload(self) -> Dict[str, Any]:
        return {
            "control_protocol_version": self.control_protocol_version,
            "base_evaluation_identity": copy.deepcopy(self.base_evaluation.identity_payload()),
            "control_kind": self.control_kind,
            "seed": self.seed,
        }

    def compute_control_evaluation_id(self) -> str:
        self.validate()
        return short_hash(self.identity_payload(), length=32)


def _derive_period_seed(seed: int, period_key: str) -> int:
    basis = {"schema": NULL_CONTROL_PROTOCOL_VERSION, "seed": seed, "period": period_key}
    digest = stable_hash(basis)
    return int(digest[:8], 16)


def apply_cross_sectional_permutation(observations: pd.DataFrame, *, seed: int) -> pd.DataFrame:
    """Deterministically shuffle factor_value WITHIN each period. Symbol and
    label pairing is untouched; only which symbol received which factor
    value within a period is permuted. Reproducible: the same seed always
    produces the same permutation for the same period keys."""
    parts = []
    for period, group in observations.groupby(PERIOD_COL, sort=True):
        rng = np.random.default_rng(_derive_period_seed(seed, str(period)))
        permuted = group.copy()
        perm_idx = rng.permutation(len(group))
        permuted[FACTOR_VALUE_COL] = group[FACTOR_VALUE_COL].to_numpy()[perm_idx]
        parts.append(permuted)
    return pd.concat(parts, ignore_index=True)


def apply_temporal_offset(observations: pd.DataFrame, *, offset_periods: int) -> pd.DataFrame:
    """Deterministically shift each symbol's factor_value series forward by
    `offset_periods` periods (per-symbol chronological order), so period p's
    label is paired with an earlier period's factor value. Periods with no
    prior value to shift in (the first `offset_periods` per symbol) are
    dropped -- never filled with a fabricated value."""
    if offset_periods < 1:
        raise ValueError("offset_periods must be >= 1")
    ordered = observations.sort_values([SYMBOL_COL, PERIOD_COL]).reset_index(drop=True)
    shifted = ordered.copy()
    shifted[FACTOR_VALUE_COL] = ordered.groupby(SYMBOL_COL)[FACTOR_VALUE_COL].shift(offset_periods)
    return shifted.dropna(subset=[FACTOR_VALUE_COL]).reset_index(drop=True)


def run_null_control(
    observations: pd.DataFrame,
    control_spec: FactorNullControlSpec,
    *,
    evaluate_fn,
    **evaluate_kwargs: Any,
) -> FactorDiagnosticsReport:
    """Apply the deterministic transform named by `control_spec.control_kind`
    and run `evaluate_fn` (typically
    `mqk_research.factors.diagnostics.evaluate_factor_ic_ir`) on the
    transformed data. `evaluate_fn` is injected rather than imported
    directly so this module has no hard dependency on one specific
    diagnostic protocol."""
    control_spec.validate()
    if control_spec.control_kind == CONTROL_KIND_CROSS_SECTIONAL_PERMUTATION:
        transformed = apply_cross_sectional_permutation(observations, seed=control_spec.seed)
    else:
        transformed = apply_temporal_offset(observations, offset_periods=control_spec.seed)
    return evaluate_fn(transformed, **evaluate_kwargs)


def compare_real_vs_null(
    real_report: FactorDiagnosticsReport, null_report: FactorDiagnosticsReport
) -> Dict[str, Any]:
    """Truthful, threshold-free comparison of a real evaluation against its
    null control. Requires both to have succeeded -- comparing against a
    not_evaluable report would be meaningless. Never claims statistical
    significance; `null_as_strong_or_stronger` is a plain fact about the
    observed magnitudes, not a judgment."""
    if real_report.status != EVAL_STATUS_SUCCEEDED or null_report.status != EVAL_STATUS_SUCCEEDED:
        raise ValueError("compare_real_vs_null requires both reports to have status succeeded")
    real_abs_ic = abs(real_report.metrics["mean_ic"])
    null_abs_ic = abs(null_report.metrics["mean_ic"])
    return {
        "real_mean_ic": real_report.metrics["mean_ic"],
        "null_mean_ic": null_report.metrics["mean_ic"],
        "real_abs_mean_ic": real_abs_ic,
        "null_abs_mean_ic": null_abs_ic,
        "real_ic_information_ratio": real_report.metrics["ic_information_ratio"],
        "null_ic_information_ratio": null_report.metrics["ic_information_ratio"],
        "real_top_minus_bottom_spread": real_report.metrics["quantile"]["top_minus_bottom_spread"],
        "null_top_minus_bottom_spread": null_report.metrics["quantile"]["top_minus_bottom_spread"],
        "null_as_strong_or_stronger": bool(null_abs_ic >= real_abs_ic),
    }
