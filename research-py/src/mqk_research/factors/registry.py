"""
RESEARCH-FACTOR-CONTRACT-AND-REGISTRY-01

Public entry points wrapping `ResearchResultStore`'s factor tables. Thin,
functional wrappers only -- all identity computation lives in
`mqk_research.factors.contracts`; this module never computes identity itself,
it only persists what the contract already decided.

Dependency direction (mirrors mqk_research.ml.registry_integration):
    FactorSpec -> factor_id -> register_factor
    FactorEvaluationSpec -> evaluation_id -> begin_factor_evaluation (attempt)
    -> caller runs the actual evaluation -> finalize_factor_evaluation
evaluation_id/results never feed back into factor_id.
"""
from __future__ import annotations

from pathlib import Path
from typing import Any, Dict, List, Optional

from mqk_research.exp_distributed.storage import ResearchResultStore

from .contracts import FactorEvaluationResult, FactorEvaluationSpec, FactorSpec

__all__ = [
    "register_factor",
    "get_factor",
    "list_factors",
    "begin_factor_evaluation",
    "finalize_factor_evaluation",
    "list_factor_evaluation_attempts",
]


def register_factor(registry_db: Path, spec: FactorSpec) -> str:
    """Compute the deterministic factor_id and idempotently register it.
    Safe to call repeatedly for the same semantic FactorSpec (no-op after the
    first call). Registration never requires an evaluation result to exist --
    a factor is registerable regardless of what any later evaluation finds."""
    spec.validate()
    factor_id = spec.compute_factor_id()
    store = ResearchResultStore(Path(registry_db))
    store.register_factor(
        factor_id=factor_id,
        family=spec.family,
        name=spec.name,
        protocol_version=spec.protocol_version,
        identity=spec.identity_payload(),
    )
    return factor_id


def get_factor(registry_db: Path, factor_id: str) -> Dict[str, Any]:
    store = ResearchResultStore(Path(registry_db))
    return store.get_factor(factor_id)


def list_factors(registry_db: Path, *, family: Optional[str] = None) -> List[Dict[str, Any]]:
    store = ResearchResultStore(Path(registry_db))
    return store.list_factors(family=family)


def begin_factor_evaluation(
    registry_db: Path,
    eval_spec: FactorEvaluationSpec,
    *,
    origin: Optional[str] = None,
    metadata: Optional[Dict[str, Any]] = None,
) -> tuple[str, str, int]:
    """Compute the deterministic evaluation_id and open a durable 'started'
    attempt BEFORE the caller runs the actual evaluation, so a failure
    mid-evaluation still leaves durable evidence. Fails closed (KeyError) if
    `eval_spec.factor_id` was never registered. Returns
    (attempt_id, evaluation_id, attempt_index)."""
    eval_spec.validate()
    evaluation_id = eval_spec.compute_evaluation_id()
    store = ResearchResultStore(Path(registry_db))
    attempt_id, attempt_index = store.begin_factor_evaluation_attempt(
        factor_id=eval_spec.factor_id,
        evaluation_id=evaluation_id,
        evaluation_identity=eval_spec.identity_payload(),
        origin=origin,
        metadata=metadata,
    )
    return attempt_id, evaluation_id, attempt_index


def finalize_factor_evaluation(
    registry_db: Path,
    attempt_id: str,
    result: FactorEvaluationResult,
) -> None:
    """Transition a 'started' factor evaluation attempt to its terminal
    status. Fails closed if the attempt is unknown or already terminal."""
    result.validate()
    store = ResearchResultStore(Path(registry_db))
    store.finalize_factor_evaluation_attempt(
        attempt_id,
        status=result.status,
        result_summary={"metrics": result.metrics} if result.metrics else {},
        artifact_paths=result.artifact_paths,
        failure_reason=result.reason,
    )


def list_factor_evaluation_attempts(registry_db: Path, factor_id: str) -> List[Dict[str, Any]]:
    store = ResearchResultStore(Path(registry_db))
    return store.list_factor_evaluation_attempts(factor_id)
