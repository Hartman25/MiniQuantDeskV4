"""
RESEARCH-FACTOR-FDR-01

Benjamini-Hochberg multiple-hypothesis correction for broad factor
screening. This is a Research-stage discovery diagnostic; it does NOT
replace the accepted DSR/PBO promotion judge
(mqk_research.ml.multiple_testing_judge).

P-VALUE SOURCE: no p-value is invented from IC/Sharpe magnitude. Each
factor's p-value is a deterministic, two-sided EMPIRICAL permutation
p-value derived from Patch C's null machinery (cross-sectional permutation)
-- the fraction of a deterministic null distribution whose |mean_ic| meets
or exceeds the real |mean_ic|.

POPULATION SOURCE: the correction population is read from the Patch A
durable registry's full set of REGISTERED factors for a family -- never
from a caller-assembled "winners" list. A registered factor with no
succeeded evaluation, or with a succeeded evaluation but no supplied
p-value, still appears in the report as an EXCLUDED candidate with an
explicit reason. Retries/evaluation slices never inflate hypothesis count:
population membership is per factor_id (the Patch A registry's own
uniqueness), not per attempt.
"""
from __future__ import annotations

from pathlib import Path
from typing import Any, Callable, Dict, Optional

import pandas as pd

from .contracts import EVAL_STATUS_SUCCEEDED, FactorEvaluationSpec
from .diagnostics import FactorDiagnosticsReport, evaluate_factor_ic_ir
from .null_controls import CONTROL_KIND_CROSS_SECTIONAL_PERMUTATION, FactorNullControlSpec, run_null_control
from .registry import list_factor_evaluation_attempts, list_factors

FDR_PROTOCOL_VERSION = "factor_fdr_bh_v1"
EMPIRICAL_PVALUE_PROTOCOL_VERSION = "factor_empirical_permutation_pvalue_v1"


def compute_empirical_pvalue(
    observations: pd.DataFrame,
    eval_spec: FactorEvaluationSpec,
    real_report: FactorDiagnosticsReport,
    *,
    n_permutations: int = 200,
    base_seed: int = 0,
    evaluate_fn: Callable[..., FactorDiagnosticsReport] = evaluate_factor_ic_ir,
    **evaluate_kwargs: Any,
) -> Dict[str, Any]:
    """Deterministic two-sided empirical permutation p-value.

    Draws `n_permutations` deterministic cross-sectional permutation nulls
    (seeds base_seed..base_seed+n_permutations-1, see
    mqk_research.factors.null_controls) and counts how many have
    |mean_ic| >= the real report's |mean_ic|. Uses the standard +1/+1
    continuity correction (Davison & Hinkley 1997) so the p-value is never
    exactly zero. Degenerate (not_evaluable) permutation draws are skipped
    entirely -- they are neither evidence for nor against the real factor."""
    if real_report.status != EVAL_STATUS_SUCCEEDED:
        raise ValueError("compute_empirical_pvalue requires a succeeded real report")
    if n_permutations < 1:
        raise ValueError("n_permutations must be >= 1")
    real_abs_ic = abs(real_report.metrics["mean_ic"])
    exceed_count = 0
    used = 0
    for i in range(n_permutations):
        seed = base_seed + i
        control_spec = FactorNullControlSpec(
            base_evaluation=eval_spec, control_kind=CONTROL_KIND_CROSS_SECTIONAL_PERMUTATION, seed=seed
        )
        null_report = run_null_control(observations, control_spec, evaluate_fn=evaluate_fn, **evaluate_kwargs)
        if null_report.status != EVAL_STATUS_SUCCEEDED:
            continue
        used += 1
        if abs(null_report.metrics["mean_ic"]) >= real_abs_ic:
            exceed_count += 1
    if used == 0:
        raise ValueError("no usable permutation draws; cannot compute empirical p-value")
    p_value = (exceed_count + 1) / (used + 1)
    return {
        "protocol_version": EMPIRICAL_PVALUE_PROTOCOL_VERSION,
        "n_permutations_requested": n_permutations,
        "n_permutations_used": used,
        "base_seed": base_seed,
        "exceed_count": exceed_count,
        "p_value": p_value,
        "real_abs_mean_ic": real_abs_ic,
    }


def benjamini_hochberg(p_values_by_factor_id: Dict[str, float], *, alpha: float) -> Dict[str, Any]:
    """Standard Benjamini-Hochberg step-up procedure with the standard
    monotone adjusted-q-value formula. Order-independent: results depend
    only on the (factor_id, p_value) set, never on dict insertion order."""
    if not (0.0 < alpha < 1.0):
        raise ValueError("alpha must be strictly between 0 and 1")
    if not p_values_by_factor_id:
        raise ValueError("p_values_by_factor_id must be non-empty")
    for factor_id, p in p_values_by_factor_id.items():
        if not (0.0 <= p <= 1.0):
            raise ValueError(f"p-value for {factor_id!r} out of [0,1]: {p!r}")

    items = sorted(p_values_by_factor_id.items(), key=lambda kv: (kv[1], kv[0]))
    m = len(items)
    p_sorted = [p for _, p in items]

    critical_values = [(rank + 1) / m * alpha for rank in range(m)]
    reject_flags = [p <= crit for p, crit in zip(p_sorted, critical_values)]
    max_reject_idx = max((idx for idx, flag in enumerate(reject_flags) if flag), default=-1)
    rejected_ids = {items[idx][0] for idx in range(max_reject_idx + 1)}

    q_sorted = [0.0] * m
    q_sorted[-1] = p_sorted[-1]
    for i in range(m - 2, -1, -1):
        q_sorted[i] = min(q_sorted[i + 1], p_sorted[i] * m / (i + 1))
    q_by_id = {items[i][0]: q_sorted[i] for i in range(m)}

    return {
        "protocol_version": FDR_PROTOCOL_VERSION,
        "alpha": alpha,
        "hypothesis_count": m,
        "rejected_factor_ids": sorted(rejected_ids),
        "q_values": q_by_id,
    }


def build_fdr_population_report(
    registry_db: Path,
    *,
    family: str,
    p_values_by_factor_id: Dict[str, float],
    alpha: float,
) -> Dict[str, Any]:
    """Run BH correction over the FULL declared population of registered
    factors for `family`, read from the Patch A durable registry -- never
    from `p_values_by_factor_id`'s own keys. A registered factor absent
    from `p_values_by_factor_id` (or with no succeeded evaluation at all)
    is still surfaced as an EXCLUDED candidate with an explicit reason
    rather than silently vanishing -- so a winner-only p-value mapping is
    visibly incomplete in the output, never rejected as if it were the
    full population."""
    factors = list_factors(registry_db, family=family)
    if not factors:
        raise ValueError(f"no registered factors found for family={family!r}")

    declared_factor_ids = sorted(f["factor_id"] for f in factors)
    included: Dict[str, float] = {}
    excluded: Dict[str, str] = {}
    for factor_id in declared_factor_ids:
        supplied = p_values_by_factor_id.get(factor_id)
        if supplied is not None:
            included[factor_id] = supplied
            continue
        attempts = list_factor_evaluation_attempts(registry_db, factor_id)
        if not attempts:
            excluded[factor_id] = "no_evaluation_attempted"
        elif not any(a["status"] == "succeeded" for a in attempts):
            excluded[factor_id] = "no_succeeded_evaluation"
        else:
            excluded[factor_id] = "p_value_not_supplied"

    if not included:
        raise ValueError(f"no evaluable candidates with p-values for family={family!r}; cannot run FDR")

    bh = benjamini_hochberg(included, alpha=alpha)
    return {
        "protocol_version": FDR_PROTOCOL_VERSION,
        "family": family,
        "alpha": alpha,
        "declared_population_count": len(declared_factor_ids),
        "declared_factor_ids": declared_factor_ids,
        "included_factor_ids": sorted(included.keys()),
        "excluded_factor_ids_with_reasons": excluded,
        "raw_p_values": dict(included),
        "q_values": bh["q_values"],
        "rejected_factor_ids": bh["rejected_factor_ids"],
        "hypothesis_count": bh["hypothesis_count"],
    }
