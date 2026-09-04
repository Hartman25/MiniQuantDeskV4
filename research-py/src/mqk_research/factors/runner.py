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

RESEARCH-POINT-IN-TIME-UNIVERSE-01: callers pass exactly one of
`universe_identity` (a fixed, ex-ante declared universe label -- legal,
but never mislabeled point-in-time) or `pit_universe` (a real
`mqk_research.factors.universe.UniverseSpec`). Which mode was used is
NOT just informational: PIT and fixed-ex-ante execute genuinely
different membership rules (PIT proves every observation row's own
universe membership at its own period; fixed-ex-ante does not), so the
resolved mode is baked directly into the bound `universe_identity`
payload (`{"universe_mode": ..., **declared_identity}`, mode always
resolved server-side last) -- a fixed-ex-ante call and a PIT call can
therefore never collide under the same `evaluation_id` merely because
the caller passed the same underlying universe dictionary/coverage
data, and a caller can never spoof `metadata["universe_mode"]` into
claiming proven PIT authority for an unproven fixed-ex-ante run: that
metadata key is always overwritten from the actual resolved mode, never
caller-supplied.

`factor_spec.universe_identity` must equal this resolved (mode-tagged)
evaluation universe identity -- these are two already identity-bearing
authorities and this runner never leaves them silently contradictory.
Callers register a factor already declaring the universe (and mode) it
will be evaluated against; this is never silently rewritten here.
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from datetime import datetime
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
from .universe import UniverseSpec, assert_observations_within_universe, universe_identity_binding

# Authoritative, server-resolved universe-mode tag folded into the bound
# evaluation universe_identity -- never accepted verbatim from a caller
# (see module docstring). A fixed-ex-ante and a point-in-time evaluation of
# otherwise-identical universe content therefore always mint different
# evaluation identities.
UNIVERSE_MODE_FIXED_EX_ANTE = "fixed_ex_ante"
UNIVERSE_MODE_POINT_IN_TIME = "point_in_time"

__all__ = [
    "UNIVERSE_MODE_FIXED_EX_ANTE",
    "UNIVERSE_MODE_POINT_IN_TIME",
    "RegisteredFactorDiagnosticsResult",
    "run_registered_factor_diagnostics",
]


def _parse_ts(value: str) -> datetime:
    return datetime.fromisoformat(str(value).strip().replace("Z", "+00:00"))


def _assert_window_within_universe_coverage(
    pit_universe: UniverseSpec, evaluation_window_start_utc: str, evaluation_window_end_utc: str
) -> None:
    """Fail closed unless the DECLARED evaluation window lies entirely
    within `pit_universe`'s declared PIT coverage window -- membership is
    UNKNOWN outside that window, so a wider evaluation window is never
    proven PIT authority no matter which rows the caller happened to
    supply."""
    window_start = _parse_ts(evaluation_window_start_utc)
    window_end = _parse_ts(evaluation_window_end_utc)
    coverage_start = _parse_ts(pit_universe.coverage_start_utc)
    coverage_end = _parse_ts(pit_universe.coverage_end_utc)
    if window_start < coverage_start or window_end > coverage_end:
        raise ValueError(
            f"evaluation window [{evaluation_window_start_utc}, {evaluation_window_end_utc}] exceeds "
            f"universe {pit_universe.universe_name!r}'s declared PIT coverage "
            f"[{pit_universe.coverage_start_utc}, {pit_universe.coverage_end_utc}) -- membership is "
            "unproven outside declared coverage"
        )


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
    evaluation_window_start_utc: str,
    evaluation_window_end_utc: str,
    label_protocol_version: str,
    universe_identity: Optional[Dict[str, Any]] = None,
    pit_universe: Optional[UniverseSpec] = None,
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

    Exactly one of `universe_identity` or `pit_universe` is required. Pass
    `universe_identity` for a fixed, ex-ante declared universe (legal, but
    it is the caller's responsibility to label it truthfully). Pass
    `pit_universe` (a real `UniverseSpec`) for a proven point-in-time
    evaluation: its `universe_identity_binding` becomes part of this
    evaluation's bound identity, the declared evaluation window must lie
    entirely within its coverage window, and every observation row is
    proven an active member at its own period before the diagnostic runs.
    Either way, `factor_spec.universe_identity` must already equal the
    resolved (mode-tagged) evaluation universe identity -- this is never
    silently reconciled.

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

    if (universe_identity is None) == (pit_universe is None):
        raise ValueError(
            "run_registered_factor_diagnostics requires exactly one of universe_identity "
            "(a fixed, ex-ante declared universe) or pit_universe (a real point-in-time "
            "UniverseSpec) -- never both, never neither"
        )
    actual_universe_mode = UNIVERSE_MODE_POINT_IN_TIME if pit_universe is not None else UNIVERSE_MODE_FIXED_EX_ANTE
    declared_universe_identity = (
        universe_identity_binding(pit_universe) if pit_universe is not None else universe_identity
    )
    # The resolved mode is baked directly into the bound universe_identity
    # (last, so it always wins) -- PIT and fixed-ex-ante can never collide
    # under the same evaluation_id merely because the underlying universe
    # content happens to match.
    resolved_universe_identity = {**declared_universe_identity, "universe_mode": actual_universe_mode}
    if factor_spec.universe_identity != resolved_universe_identity:
        raise ValueError(
            "run_registered_factor_diagnostics requires factor_spec.universe_identity to already equal "
            f"the resolved (mode-tagged) evaluation universe identity; factor_spec.universe_identity="
            f"{factor_spec.universe_identity!r} but the resolved evaluation universe identity is "
            f"{resolved_universe_identity!r} -- these are never silently reconciled"
        )
    # universe_mode is authoritative, server-resolved diagnostic metadata --
    # never accepted from a caller-supplied value, so a fixed-ex-ante run can
    # never be labeled "point_in_time" in durable attempt metadata either.
    resolved_metadata = {**(metadata or {}), "universe_mode": actual_universe_mode}

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
        universe_identity=resolved_universe_identity,
        evaluation_window_start_utc=evaluation_window_start_utc,
        evaluation_window_end_utc=evaluation_window_end_utc,
        label_protocol_version=label_protocol_version,
        evaluation_protocol_version=protocol_spec.evaluation_protocol_version(),
    )
    attempt_metadata = {**resolved_metadata, "diagnostic_protocol": protocol_spec.identity_payload()}
    attempt_id, evaluation_id, attempt_index = begin_factor_evaluation(
        registry_db, eval_spec, origin=origin, metadata=attempt_metadata
    )

    try:
        if pit_universe is not None:
            # The DECLARED window must lie entirely within the universe's
            # proven PIT coverage -- checked before any row, since a window
            # that exceeds membership coverage is not proven PIT authority
            # regardless of which specific rows happen to fall inside it.
            _assert_window_within_universe_coverage(
                pit_universe, evaluation_window_start_utc, evaluation_window_end_utc
            )
            # Fail closed on any row whose symbol was not an active
            # universe member at its own period -- no survivorship
            # shortcut, no fallback to current/most-recent membership.
            assert_observations_within_universe(observations, pit_universe)
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
