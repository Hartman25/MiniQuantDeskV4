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

RESEARCH-FACTOR-FDR-FULL-POPULATION-AUTHORITY-01: `build_fdr_population_
report` only ever emits an authoritative BH decision (`status == "complete"`,
carrying `rejected_factor_ids`/`q_values`) when EVERY registered factor in
the declared family population has been fully accounted for -- attempted,
and either excluded with an explicit typed reason or included via
`FactorPValueEvidence` bound to the exact successful evaluation_id that
produced it. A bare (factor_id -> float) p-value cannot prove it came from a
real registered evaluation, so `p_value_evidence` is a sequence of
`FactorPValueEvidence`, not a dict -- a caller cannot "sneak" an unbound
number past the correction. `status != "complete"` (incomplete /
not_evaluable) never carries a `rejected_factor_ids`/`q_values` decision.
"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Dict, Iterable, Optional

import pandas as pd

from .contracts import EVAL_STATUS_SUCCEEDED, FactorEvaluationSpec
from .diagnostics import FactorDiagnosticsReport, evaluate_factor_ic_ir
from .null_controls import CONTROL_KIND_CROSS_SECTIONAL_PERMUTATION, FactorNullControlSpec, run_null_control
from .registry import list_factor_evaluation_attempts, list_factors

FDR_PROTOCOL_VERSION = "factor_fdr_bh_v1"
EMPIRICAL_PVALUE_PROTOCOL_VERSION = "factor_empirical_permutation_pvalue_v1"

FDR_POPULATION_STATUS_COMPLETE = "complete"
FDR_POPULATION_STATUS_INCOMPLETE = "incomplete"
FDR_POPULATION_STATUS_NOT_EVALUABLE = "not_evaluable"
_VALID_FDR_POPULATION_STATUSES = (
    FDR_POPULATION_STATUS_COMPLETE,
    FDR_POPULATION_STATUS_INCOMPLETE,
    FDR_POPULATION_STATUS_NOT_EVALUABLE,
)


@dataclass(frozen=True)
class FactorPValueEvidence:
    """Binds one factor's empirical p-value to the EXACT successful
    evaluation attempt that produced it. A bare (factor_id -> float) mapping
    can never prove a number came from a real registered evaluation --
    this is the minimum identity `build_fdr_population_report` requires
    before accepting a p-value as authoritative registered evidence."""

    factor_id: str
    evaluation_id: str
    p_value: float

    def validate(self) -> None:
        if not self.factor_id.strip():
            raise ValueError("FactorPValueEvidence.factor_id is required")
        if not self.evaluation_id.strip():
            raise ValueError("FactorPValueEvidence.evaluation_id is required")
        if not (0.0 <= self.p_value <= 1.0):
            raise ValueError(f"FactorPValueEvidence.p_value out of [0,1]: {self.p_value!r}")


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
    p_value_evidence: Iterable[FactorPValueEvidence],
    alpha: float,
) -> Dict[str, Any]:
    """Run BH correction over the FULL declared population of registered
    factors for `family`, read from the Patch A durable registry -- never
    from `p_value_evidence`'s own factor_ids. Every registered factor is
    accounted for as exactly one of: included (succeeded evaluation, with
    p-value evidence bound to that exact succeeded evaluation_id), or
    excluded with an explicit typed reason (no attempt / no succeeded
    evaluation).

    Returns `status`:
      - "complete": every registered factor was accounted for and at least
        one was included -- `rejected_factor_ids`/`q_values` are a real BH
        decision.
      - "incomplete": a registered factor was never attempted, or succeeded
        but has no bound p-value evidence yet -- NOT an authoritative
        decision; `rejected_factor_ids`/`q_values` are None.
      - "not_evaluable": every registered factor was accounted for but none
        succeeded (nothing to correct) -- also not authoritative.

    Fails closed (ValueError, never silently accepted) if evidence is
    supplied for a factor_id outside the declared family population, or for
    a factor_id whose `evaluation_id` does not match any succeeded
    evaluation attempt of that factor -- a caller-supplied p-value must
    never be accepted for a factor with no succeeded evaluation, and a free
    floating number must never masquerade as bound registered evidence."""
    factors = list_factors(registry_db, family=family)
    if not factors:
        raise ValueError(f"no registered factors found for family={family!r}")
    declared_factor_ids = sorted(f["factor_id"] for f in factors)
    declared_set = set(declared_factor_ids)

    evidence_list = list(p_value_evidence)
    for evidence in evidence_list:
        evidence.validate()
    for evidence in evidence_list:
        if evidence.factor_id not in declared_set:
            raise ValueError(
                f"p-value evidence supplied for factor_id={evidence.factor_id!r}, which is outside the "
                f"declared family={family!r} population"
            )
    evidence_by_factor: Dict[str, FactorPValueEvidence] = {}
    for evidence in evidence_list:
        existing = evidence_by_factor.get(evidence.factor_id)
        if existing is not None and existing != evidence:
            raise ValueError(f"conflicting p-value evidence supplied for factor_id={evidence.factor_id!r}")
        evidence_by_factor[evidence.factor_id] = evidence

    included: Dict[str, float] = {}
    excluded: Dict[str, str] = {}
    incomplete = False

    for factor_id in declared_factor_ids:
        attempts = list_factor_evaluation_attempts(registry_db, factor_id)
        succeeded_attempts = [a for a in attempts if a["status"] == "succeeded"]
        succeeded_evaluation_ids = {a["evaluation_id"] for a in succeeded_attempts if a.get("evaluation_id")}
        evidence = evidence_by_factor.get(factor_id)

        if evidence is not None and evidence.evaluation_id not in succeeded_evaluation_ids:
            raise ValueError(
                f"p-value evidence for factor_id={factor_id!r} cites evaluation_id="
                f"{evidence.evaluation_id!r}, which is not a succeeded evaluation attempt of this factor; "
                "a caller-supplied p-value must not be accepted for a factor with no succeeded evaluation"
            )

        if not attempts:
            excluded[factor_id] = "no_evaluation_attempted"
            incomplete = True
        elif not succeeded_attempts:
            excluded[factor_id] = "no_succeeded_evaluation"
        elif evidence is None:
            excluded[factor_id] = "p_value_not_supplied"
            incomplete = True
        else:
            included[factor_id] = evidence.p_value

    base_report = {
        "protocol_version": FDR_PROTOCOL_VERSION,
        "family": family,
        "alpha": alpha,
        "declared_population_count": len(declared_factor_ids),
        "declared_factor_ids": declared_factor_ids,
        "included_factor_ids": sorted(included.keys()),
        "excluded_factor_ids_with_reasons": excluded,
        "raw_p_values": dict(included),
    }

    if incomplete:
        return {
            **base_report,
            "status": FDR_POPULATION_STATUS_INCOMPLETE,
            "q_values": None,
            "rejected_factor_ids": None,
            "hypothesis_count": None,
        }

    if not included:
        return {
            **base_report,
            "status": FDR_POPULATION_STATUS_NOT_EVALUABLE,
            "q_values": None,
            "rejected_factor_ids": None,
            "hypothesis_count": None,
        }

    bh = benjamini_hochberg(included, alpha=alpha)
    return {
        **base_report,
        "status": FDR_POPULATION_STATUS_COMPLETE,
        "q_values": bh["q_values"],
        "rejected_factor_ids": bh["rejected_factor_ids"],
        "hypothesis_count": bh["hypothesis_count"],
    }
