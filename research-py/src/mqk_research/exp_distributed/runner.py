from __future__ import annotations

import json
from collections import OrderedDict
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path
from typing import Any, Dict, List, Sequence, Tuple

import yaml

from .aggregator import aggregate_results
from .artifacts import batch_root, capture_artifact_evidence, write_job_spec, write_json
from .artifacts import write_batch_artifacts as persist_batch_artifacts
from .models import BatchSpec, JobExecutionResult, JobSpec
from .registry_integration import build_candidate_trial_identity, PROTOCOL_ID as CANDIDATE_PROTOCOL_ID
from .scheduler import BatchPlan, build_batch_plan
from .storage import ResearchResultStore
from .worker import run_job_worker


def package_root() -> Path:
    return Path(__file__).resolve().parents[3]


def default_root() -> Path:
    return package_root() / "experiments" / "exp_distributed"


def default_db_path(root: Path) -> Path:
    return root / "state" / "exp_research.sqlite3"


def _resolve_spec_relative_paths(spec_path: Path, payload: Dict[str, Any]) -> Dict[str, Any]:
    resolved = dict(payload)
    dataset_path_raw = resolved.get("dataset_path")
    if dataset_path_raw is None:
        raise ValueError("batch spec missing dataset_path")
    dataset_path = Path(str(dataset_path_raw))
    if not dataset_path.is_absolute():
        dataset_path = (spec_path.parent / dataset_path).resolve()
    resolved["dataset_path"] = str(dataset_path)
    return resolved


def load_batch_spec(path: Path) -> BatchSpec:
    resolved_path = path.resolve()
    raw_text = resolved_path.read_text(encoding="utf-8")
    if resolved_path.suffix.lower() in {".yaml", ".yml"}:
        payload = yaml.safe_load(raw_text)
    else:
        payload = json.loads(raw_text)
    if not isinstance(payload, dict):
        raise ValueError("batch spec must decode to an object")
    return BatchSpec.from_dict(_resolve_spec_relative_paths(resolved_path, payload))


def _prepare_batch(plan: BatchPlan, spec: BatchSpec, root: Path, store: ResearchResultStore) -> Dict[str, Path]:
    root.mkdir(parents=True, exist_ok=True)
    store.upsert_batch(plan.batch_id, spec.to_dict(), root, len(plan.jobs), status="queued")
    spec_paths = {job.job_id: write_job_spec(root, job) for job in plan.jobs}
    store.upsert_jobs(plan.jobs, spec_paths, status="queued")
    write_json(batch_root(root, plan.batch_id) / "batch_manifest.json", plan.batch_manifest)
    return spec_paths


def create_batch(spec_path: Path, root: Path | None = None) -> Dict[str, Any]:
    actual_root = (root or default_root()).resolve()
    spec = load_batch_spec(spec_path)
    plan = build_batch_plan(spec)
    # RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-02: planning is not an attempt —
    # the durable batch_manifest.json must say so truthfully rather than
    # looking indistinguishable from an attempted (registered/diagnostic) run.
    plan.batch_manifest["registry_status"] = "planned_not_attempted"
    store = ResearchResultStore(default_db_path(actual_root))
    spec_paths = _prepare_batch(plan, spec, actual_root, store)
    return {
        "batch_id": plan.batch_id,
        "job_count": len(plan.jobs),
        "job_spec_paths": {job_id: str(path) for job_id, path in spec_paths.items()},
        "root": str(actual_root),
        "db_path": str(default_db_path(actual_root)),
    }


def _resolve_registration_mode(hypothesis_id: str, allow_unregistered_diagnostic: bool) -> str:
    """Fail-closed gate for OFFICIAL distributed execution (run_batch /
    rerun_failed_jobs). Planning (create_batch/build_batch_plan) does not call
    this — planning != attempting. Every candidate actually evaluated by an
    official research workflow must leave durable registry evidence before its
    result is known, UNLESS the caller explicitly opts into an unregistered
    diagnostic run. There is no silent fallback
    (RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-01)."""
    if hypothesis_id:
        return "registered"
    if allow_unregistered_diagnostic:
        return "unregistered_diagnostic"
    raise ValueError(
        "run_batch/rerun_failed_jobs requires a non-empty hypothesis_id so the "
        "run is registered as research (see exp_distributed/registry_integration.py), "
        "or explicit allow_unregistered_diagnostic=True on the batch spec for a "
        "diagnostic run outside the registry. Refusing to silently run unregistered "
        "research."
    )


def _group_jobs_by_trial(
    jobs: Sequence[JobSpec], *, hypothesis_id: str, experiment_id: str
) -> Tuple["OrderedDict[str, List[JobSpec]]", Dict[str, Dict[str, Any]]]:
    """Group jobs by candidate trial identity (window-excluded). Preserves
    first-seen trial order. A window is an evaluation slice of a candidate,
    not a separate candidate — this is the grouping RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-01
    requires so one run_batch invocation opens exactly one attempt per unique
    candidate, not one attempt per window job."""
    groups: "OrderedDict[str, List[JobSpec]]" = OrderedDict()
    identities: Dict[str, Dict[str, Any]] = {}
    for job in jobs:
        trial_id, identity = build_candidate_trial_identity(
            job, hypothesis_id=hypothesis_id, experiment_id=experiment_id
        )
        groups.setdefault(trial_id, []).append(job)
        identities[trial_id] = identity
    return groups, identities


def _register_candidate_attempts(
    jobs: Sequence[JobSpec],
    store: ResearchResultStore,
    *,
    hypothesis_id: str,
    experiment_id: str,
    batch_id: str,
    scope: str,
) -> Tuple[Dict[str, str], Dict[str, str]]:
    """Register/reuse each unique candidate trial among `jobs` and open exactly
    ONE durable 'started' attempt per candidate BEFORE any of that candidate's
    window jobs execute, linking every window job for that candidate to the
    same attempt via research_attempt_jobs. No-op (returns ({}, {})) when
    hypothesis_id is empty — callers must already have fail-closed-gated this
    via _resolve_registration_mode (RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-01).

    Returns (attempt_by_job_id, trial_id_by_attempt_id)."""
    if not hypothesis_id:
        return {}, {}
    store.register_hypothesis(hypothesis_id=hypothesis_id, experiment_id=experiment_id)
    groups, identities = _group_jobs_by_trial(jobs, hypothesis_id=hypothesis_id, experiment_id=experiment_id)
    attempt_by_job_id: Dict[str, str] = {}
    trial_id_by_attempt_id: Dict[str, str] = {}
    for trial_id, trial_jobs in groups.items():
        store.register_trial(
            trial_id=trial_id,
            experiment_id=experiment_id,
            hypothesis_id=hypothesis_id,
            strategy_id=trial_jobs[0].strategy_id,
            protocol_id=CANDIDATE_PROTOCOL_ID,
            identity=identities[trial_id],
        )
        job_ids = [job.job_id for job in trial_jobs]
        attempt_id, _attempt_index = store.begin_attempt(
            trial_id=trial_id,
            origin="mqk-exp-dist",
            metadata={
                "batch_id": batch_id,
                "scope": scope,
                "job_ids": job_ids,
                "windows": [job.window.to_dict() for job in trial_jobs],
            },
        )
        store.link_attempt_jobs(attempt_id, job_ids)
        trial_id_by_attempt_id[attempt_id] = trial_id
        for job_id in job_ids:
            attempt_by_job_id[job_id] = attempt_id
    return attempt_by_job_id, trial_id_by_attempt_id


def _finalize_candidate_attempts(
    jobs: Sequence[JobSpec],
    results: Sequence[JobExecutionResult],
    store: ResearchResultStore,
    attempt_by_job_id: Dict[str, str],
    trial_id_by_attempt_id: Dict[str, str],
    attempt_scope: str,
) -> None:
    """Finalize each candidate attempt only after ALL of its linked slice jobs
    have terminal results. succeeded iff every linked slice succeeded;
    failed if one or more required slices failed.

    Before finalizing, persists an IMMUTABLE per-slice snapshot for every
    linked job into research_attempt_slices (RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-02).
    `exp_jobs` is reused/overwritten by later re-executions of the same
    deterministic job_id, so it cannot serve as durable per-attempt evidence —
    this snapshot can and does survive any later retry of the same job_id
    under a different attempt.

    RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-03: the job artifact directory
    itself (`result.artifact_paths`, under the deterministic
    artifacts/exp_distributed/batches/<batch_id>/jobs/<job_index>_<job_id>/
    path) is ALSO reused/overwritten by a later retry of the same job_id —
    a bare path is not durable evidence of what THIS attempt's files
    actually contained. capture_artifact_evidence() is called here,
    synchronously right after this attempt's own worker execution and before
    any later retry could run, to snapshot artifact_metadata.json's hashes
    into the immutable slice record."""
    job_by_id = {job.job_id: job for job in jobs}
    result_by_job_id = {result.job_id: result for result in results}
    jobs_by_attempt: "OrderedDict[str, List[str]]" = OrderedDict()
    for job_id, attempt_id in attempt_by_job_id.items():
        jobs_by_attempt.setdefault(attempt_id, []).append(job_id)

    for attempt_id, job_ids in jobs_by_attempt.items():
        slice_results = [result_by_job_id[job_id] for job_id in job_ids if job_id in result_by_job_id]
        succeeded = [r for r in slice_results if r.status == "succeeded"]
        failed = [r for r in slice_results if r.status != "succeeded"]
        status = "succeeded" if len(succeeded) == len(job_ids) else "failed"
        failure_reason = None
        if status == "failed":
            reasons = [f"{r.job_id}: {r.failure_reason}" for r in failed if r.failure_reason]
            failure_reason = "; ".join(reasons) if reasons else f"{len(failed)} of {len(job_ids)} evaluation slices failed"

        slice_snapshots = []
        for job_id in job_ids:
            job = job_by_id.get(job_id)
            result = result_by_job_id.get(job_id)
            status = result.status if result else "missing"
            failure_reason = result.failure_reason if result else None
            metrics = result.metrics if result else {}
            runtime_seconds = result.runtime_seconds if result else None
            artifact_paths = result.artifact_paths if result else {}
            # captured NOW, before any later retry of this same deterministic
            # job_id can overwrite the source artifact directory.
            file_evidence = capture_artifact_evidence(artifact_paths) if result else {
                "artifact_files": {},
                "artifact_metadata_sha256": None,
            }
            artifact_evidence = {
                "attempt_id": attempt_id,
                "job_id": job_id,
                "status": status,
                "failure_reason": failure_reason,
                "metrics": metrics,
                "runtime_seconds": runtime_seconds,
                "source_artifact_paths": artifact_paths,
                **file_evidence,
            }
            slice_snapshots.append(
                {
                    "job_id": job_id,
                    "window_start_utc": job.window.start_utc if job else None,
                    "window_end_utc": job.window.end_utc if job else None,
                    "status": status,
                    "failure_reason": failure_reason,
                    "artifact_paths": artifact_paths,
                    "metrics": metrics,
                    "runtime_seconds": runtime_seconds,
                    "attempt_scope": attempt_scope,
                    "artifact_evidence": artifact_evidence,
                }
            )
        store.record_attempt_slices(attempt_id, trial_id_by_attempt_id[attempt_id], slice_snapshots)

        store.finalize_attempt(
            attempt_id,
            status=status,
            result_summary={
                "total_slices": len(job_ids),
                "succeeded_slices": len(succeeded),
                "failed_slices": len(failed),
                "job_ids": job_ids,
            },
            failure_reason=failure_reason,
        )


def _run_jobs(
    jobs: Sequence[JobSpec],
    root: Path,
    store: ResearchResultStore,
    max_workers: int,
    *,
    hypothesis_id: str = "",
    experiment_id: str = "",
    batch_id: str = "",
    attempt_scope: str = "full_batch",
) -> List[JobExecutionResult]:
    attempt_by_job_id, trial_id_by_attempt_id = _register_candidate_attempts(
        jobs,
        store,
        hypothesis_id=hypothesis_id,
        experiment_id=experiment_id,
        batch_id=batch_id,
        scope=attempt_scope,
    )

    for job in jobs:
        store.set_job_status(job.job_id, "running")

    payloads = [job.to_dict() for job in jobs]
    if max_workers == 1:
        raw_results = [run_job_worker(payload, str(root)) for payload in payloads]
    else:
        with ProcessPoolExecutor(max_workers=max_workers) as executor:
            raw_results = list(executor.map(run_job_worker, payloads, [str(root)] * len(payloads)))

    results = [JobExecutionResult(**raw) for raw in raw_results]
    for result in results:
        store.persist_job_result(result)
    _finalize_candidate_attempts(jobs, results, store, attempt_by_job_id, trial_id_by_attempt_id, attempt_scope)
    return results


def _finalize_batch(plan: BatchPlan, results: List[JobExecutionResult], root: Path, store: ResearchResultStore) -> Dict[str, Any]:
    leaderboard, comparison, summary, reproducibility_manifest, failure_report = aggregate_results(plan.batch_manifest, plan.jobs, results)
    aggregate_paths = persist_batch_artifacts(
        root=root,
        batch_id=plan.batch_id,
        manifest=plan.batch_manifest,
        leaderboard=leaderboard,
        comparison=comparison,
        sweep_summary=summary,
        reproducibility_manifest=reproducibility_manifest,
        failure_report=failure_report,
    )
    status = "failed" if summary["failed"] > 0 else "succeeded"
    store.finalize_batch(plan.batch_id, status=status, aggregate_paths=aggregate_paths, summary=summary)
    return {
        "batch_id": plan.batch_id,
        "status": status,
        "summary": summary,
        "aggregate_paths": aggregate_paths,
    }


def run_batch(spec_path: Path, root: Path | None = None, max_workers: int | None = None) -> Dict[str, Any]:
    actual_root = (root or default_root()).resolve()
    spec = load_batch_spec(spec_path)
    registration_mode = _resolve_registration_mode(spec.hypothesis_id, spec.allow_unregistered_diagnostic)
    plan = build_batch_plan(spec)
    # RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-02: stamp the durable canonical
    # batch artifact with the same truth the in-memory result carries, so an
    # artifact directory inspected later (after the caller's return value is
    # gone) cannot be mistaken for registered research. Set before the first
    # (pre-execution) write so both the queued and finalized manifest agree.
    plan.batch_manifest["registry_status"] = registration_mode
    store = ResearchResultStore(default_db_path(actual_root))
    _prepare_batch(plan, spec, actual_root, store)
    results = _run_jobs(
        plan.jobs,
        actual_root,
        store,
        max_workers or spec.max_workers,
        hypothesis_id=spec.hypothesis_id,
        experiment_id=spec.experiment_id,
        batch_id=plan.batch_id,
        attempt_scope="full_batch",
    )
    finalized = _finalize_batch(plan, results, actual_root, store)
    finalized["registry_status"] = registration_mode
    return finalized


def run_single_job(job_spec_path: Path, root: Path | None = None) -> Dict[str, Any]:
    actual_root = (root or default_root()).resolve()
    job = JobSpec.from_dict(json.loads(job_spec_path.read_text(encoding="utf-8")))
    result = JobExecutionResult(**run_job_worker(job.to_dict(), str(actual_root)))
    return result.to_dict()


def batch_summary(batch_id: str, root: Path | None = None) -> Dict[str, Any]:
    actual_root = (root or default_root()).resolve()
    store = ResearchResultStore(default_db_path(actual_root))
    batch = store.get_batch(batch_id)
    return {
        "batch_id": batch["batch_id"],
        "status": batch["status"],
        "job_count": batch["job_count"],
        "summary": json.loads(batch["summary_json"]) if batch["summary_json"] else None,
        "aggregate_paths": json.loads(batch["aggregate_paths_json"]) if batch["aggregate_paths_json"] else None,
    }


def failed_jobs(batch_id: str, root: Path | None = None) -> Dict[str, Any]:
    actual_root = (root or default_root()).resolve()
    store = ResearchResultStore(default_db_path(actual_root))
    rows = store.list_jobs(batch_id, status="failed")
    return {
        "batch_id": batch_id,
        "failed_jobs": [
            {
                "job_id": row["job_id"],
                "job_index": row["job_index"],
                "failure_reason": row["failure_reason"],
                "job_spec_path": row["job_spec_path"],
            }
            for row in rows
        ],
    }


def rerun_failed_jobs(batch_id: str, root: Path | None = None, max_workers: int = 1) -> Dict[str, Any]:
    actual_root = (root or default_root()).resolve()
    store = ResearchResultStore(default_db_path(actual_root))
    failed_rows = store.list_jobs(batch_id, status="failed")
    if not failed_rows:
        return {"batch_id": batch_id, "rerun_count": 0, "status": "no_failed_jobs"}

    batch = store.get_batch(batch_id)
    batch_spec = json.loads(batch["spec_json"])
    hypothesis_id = str(batch_spec.get("hypothesis_id", ""))
    allow_unregistered_diagnostic = bool(batch_spec.get("allow_unregistered_diagnostic", False))
    registration_mode = _resolve_registration_mode(hypothesis_id, allow_unregistered_diagnostic)

    jobs = [JobSpec.from_dict(json.loads(Path(row["job_spec_path"]).read_text(encoding="utf-8"))) for row in failed_rows]
    _run_jobs(
        jobs,
        actual_root,
        store,
        max_workers=max_workers,
        hypothesis_id=hypothesis_id,
        experiment_id=str(batch_spec.get("experiment_id", "")),
        batch_id=batch_id,
        attempt_scope="failed_slices_only",
    )

    all_rows = store.list_jobs(batch_id)
    all_jobs = [JobSpec.from_dict(json.loads(Path(row["job_spec_path"]).read_text(encoding="utf-8"))) for row in all_rows]
    all_results = [
        JobExecutionResult(
            job_id=row["job_id"],
            batch_id=row["batch_id"],
            status=row["status"],
            metrics=json.loads(row["metrics_json"]) if row["metrics_json"] else {},
            artifact_paths=json.loads(row["artifact_paths_json"]) if row["artifact_paths_json"] else {},
            failure_reason=row["failure_reason"],
            runtime_seconds=row["runtime_seconds"],
        )
        for row in all_rows
    ]
    refreshed_plan = BatchPlan(
        batch_id=batch_id,
        jobs=all_jobs,
        batch_manifest={
            "schema_version": batch_spec.get("schema_version", "exp-distributed-v1"),
            "engine_id": batch_spec.get("engine_id", "EXP"),
            "batch_id": batch_id,
            "experiment_id": batch_spec["experiment_id"],
            "batch_label": batch_spec.get("batch_label", ""),
            "strategy_id": batch_spec["strategy_id"],
            "dataset_path": batch_spec["dataset_path"],
            "dataset_sha256": all_jobs[0].dataset_fingerprint.dataset_sha256,
            "job_count": len(all_jobs),
            "max_workers": batch_spec.get("max_workers", 1),
            "notes": batch_spec.get("notes", []),
            "registry_status": registration_mode,
        },
    )
    finalized = _finalize_batch(refreshed_plan, all_results, actual_root, store)
    finalized["rerun_count"] = len(jobs)
    finalized["registry_status"] = registration_mode
    return finalized
