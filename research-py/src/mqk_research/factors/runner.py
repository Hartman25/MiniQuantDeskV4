"""
RESEARCH-FACTOR-IC-IR-QUANTILE-BENCH-01

Canonical registered diagnostic-artifact orchestration runner: composes
register_factor -> begin_factor_evaluation -> evaluate_factor_ic_ir ->
build_factor_diagnostics_artifact -> finalize_factor_evaluation into one
production entry point, mirroring
`mqk_research.ml.registry_integration.run_registered_walkforward_eval`'s
register/attempt/evaluate/finalize seam (CLAUDE.md section 12: reuse
existing seams -- this is not a second registry or a parallel framework).

Every attempted factor evaluation exists in the registry (a durable
'started' attempt) BEFORE its result is known. A `succeeded` or
`not_evaluable` diagnostic outcome, or an unexpected exception (finalized
`failed` with the exception text), is always finalized and remains
durably visible via
`mqk_research.factors.registry.list_factor_evaluation_attempts` -- never
silently dropped. No hand-built winner-only registration: this is the
only sanctioned path for producing an authoritative registered
factor-diagnostics artifact.
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, Optional

import pandas as pd

from .contracts import (
    EVAL_STATUS_FAILED,
    FactorEvaluationResult,
    FactorEvaluationSpec,
    FactorSpec,
)
from .diagnostics import (
    FactorDiagnosticsProtocolSpec,
    assert_observations_within_evaluation_window,
    build_factor_diagnostics_artifact,
    evaluate_factor_ic_ir,
    observations_content_hash,
)
from .registry import begin_factor_evaluation, finalize_factor_evaluation, register_factor

__all__ = ["RegisteredFactorDiagnosticsResult", "run_registered_factor_diagnostics"]


@dataclass(frozen=True)
class RegisteredFactorDiagnosticsResult:
    """Everything a caller needs to locate the durable evidence this run
    produced -- never itself treated as identity (factor_id/evaluation_id
    were already fixed before this dataclass exists)."""

    factor_id: str
    evaluation_id: str
    attempt_id: str
    attempt_index: int
    artifact_path: Path
    status: str
    reason: Optional[str]


def run_registered_factor_diagnostics(
    registry_db: Path,
    out_dir: Path,
    *,
    factor_spec: FactorSpec,
    observations: pd.DataFrame,
    universe_identity: Dict[str, Any],
    evaluation_window_start_utc: str,
    evaluation_window_end_utc: str,
    label_protocol_version: str,
    n_quantiles: int = 5,
    min_cross_section: Optional[int] = None,
    min_periods: int = 2,
    holdout_status: str = "not_applicable",
    benchmark_comparison: Optional[Dict[str, Any]] = None,
    origin: Optional[str] = None,
    metadata: Optional[Dict[str, Any]] = None,
) -> RegisteredFactorDiagnosticsResult:
    """Register `factor_spec` (idempotent), open a durable evaluation
    attempt, run the real IC/IR/quantile diagnostic, write a deterministic
    artifact, and finalize the attempt with the genuine outcome -- success,
    not_evaluable, or (on an unexpected exception) failed.

    There is deliberately no free `direction` override parameter here:
    `factor_spec.direction` is already identity-bearing (part of
    `FactorSpec.identity_payload()`), so this runner always evaluates a
    factor under its own declared direction -- a caller can never run the
    same `factor_id` under a contradictory interpretation (identity says
    `higher_is_better` while the evaluation silently scores
    `lower_is_better`, or vice versa). Scoring against an inverted
    benchmark requires registering a distinct `FactorSpec` with the
    inverted `direction` (a genuinely different, honestly identified
    factor), never an ad-hoc override of an existing one.

    Every observation row's `period_ts_utc` is proven to fall within
    `[evaluation_window_start_utc, evaluation_window_end_utc)` (see
    `mqk_research.factors.diagnostics.assert_observations_within_evaluation_window`)
    before the diagnostic runs -- an evaluation identity that declares one
    time window must never silently consume rows outside it. This is
    checked in addition to, and independently of, any PIT universe
    membership check.

    There is deliberately no free-form `evaluation_protocol_version`
    parameter here: every knob capable of changing the diagnostic result
    (direction, n_quantiles, min_cross_section, min_periods) is bound into
    `FactorEvaluationSpec.evaluation_protocol_version` via
    `FactorDiagnosticsProtocolSpec.evaluation_protocol_version()` -- a caller
    can never run a semantically different diagnostic under the same
    evaluation_id (CLAUDE.md sections 6/20). The full protocol payload is
    also recorded in the durable attempt's `metadata["diagnostic_protocol"]`
    so an evaluator can reconstruct exactly what was run.

    The artifact filename embeds `attempt_index` so retries of the SAME
    evaluation (same factor_id + evaluation_id) never overwrite an earlier
    attempt's evidence -- but the filename/output directory are transport
    only and are never fed back into factor_id or evaluation_id.
    """
    registry_db = Path(registry_db)
    out_dir = Path(out_dir)

    factor_id = register_factor(registry_db, factor_spec)
    resolved_direction = factor_spec.direction

    protocol_spec = FactorDiagnosticsProtocolSpec(
        direction=resolved_direction,
        n_quantiles=n_quantiles,
        min_cross_section=min_cross_section,
        min_periods=min_periods,
    )

    eval_spec = FactorEvaluationSpec(
        factor_id=factor_id,
        universe_identity=universe_identity,
        evaluation_window_start_utc=evaluation_window_start_utc,
        evaluation_window_end_utc=evaluation_window_end_utc,
        label_protocol_version=label_protocol_version,
        evaluation_protocol_version=protocol_spec.evaluation_protocol_version(),
    )
    attempt_metadata = {**(metadata or {}), "diagnostic_protocol": protocol_spec.identity_payload()}
    attempt_id, evaluation_id, attempt_index = begin_factor_evaluation(
        registry_db, eval_spec, origin=origin, metadata=attempt_metadata
    )

    try:
        # The declared evaluation window is a component of evaluation_id
        # (FactorEvaluationSpec.identity_payload()) -- every row actually
        # consumed below must be proven to belong to it, or the identity is
        # a lie about what was evaluated.
        assert_observations_within_evaluation_window(
            observations,
            evaluation_window_start_utc=evaluation_window_start_utc,
            evaluation_window_end_utc=evaluation_window_end_utc,
        )
        report = evaluate_factor_ic_ir(
            observations,
            direction=resolved_direction,
            n_quantiles=n_quantiles,
            min_cross_section=min_cross_section,
            min_periods=min_periods,
        )
        input_content_sha256 = observations_content_hash(observations)
        artifact = build_factor_diagnostics_artifact(
            factor_identity_payload=factor_spec.identity_payload(),
            evaluation_identity_payload=eval_spec.identity_payload(),
            factor_id=factor_id,
            evaluation_id=evaluation_id,
            report=report,
            input_content_sha256=input_content_sha256,
            holdout_status=holdout_status,
            benchmark_comparison=benchmark_comparison,
        )
        out_dir.mkdir(parents=True, exist_ok=True)
        artifact_path = out_dir / f"factor_diagnostics_{factor_id}_{evaluation_id}_{attempt_index:04d}.json"
        artifact_path.write_text(
            json.dumps(artifact, sort_keys=True, separators=(",", ":")), encoding="utf-8"
        )
    except Exception as exc:
        # A crash mid-evaluation must still leave durable, honest evidence
        # that this attempt was made and did not silently vanish.
        finalize_factor_evaluation(
            registry_db,
            attempt_id,
            FactorEvaluationResult(
                eval_id=evaluation_id,
                factor_id=factor_id,
                status=EVAL_STATUS_FAILED,
                reason=f"{type(exc).__name__}: {exc}",
            ),
        )
        raise

    finalize_factor_evaluation(
        registry_db,
        attempt_id,
        FactorEvaluationResult(
            eval_id=evaluation_id,
            factor_id=factor_id,
            status=report.status,
            metrics=report.metrics,
            reason=report.reason,
            artifact_paths={"factor_diagnostics": str(artifact_path)},
        ),
    )

    return RegisteredFactorDiagnosticsResult(
        factor_id=factor_id,
        evaluation_id=evaluation_id,
        attempt_id=attempt_id,
        attempt_index=attempt_index,
        artifact_path=artifact_path,
        status=report.status,
        reason=report.reason,
    )
