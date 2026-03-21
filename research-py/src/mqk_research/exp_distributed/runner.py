from __future__ import annotations

import json
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path
from typing import Any, Dict, List, Sequence

import yaml

from .aggregator import aggregate_results
from .artifacts import batch_root, write_job_spec, write_json
from .artifacts import write_batch_artifacts as persist_batch_artifacts
from .models import BatchSpec, JobExecutionResult, JobSpec
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
    store = ResearchResultStore(default_db_path(actual_root))
    spec_paths = _prepare_batch(plan, spec, actual_root, store)
    return {
        "batch_id": plan.batch_id,
        "job_count": len(plan.jobs),
        "job_spec_paths": {job_id: str(path) for job_id, path in spec_paths.items()},
        "root": str(actual_root),
        "db_path": str(default_db_path(actual_root)),
    }


def _run_jobs(jobs: Sequence[JobSpec], root: Path, store: ResearchResultStore, max_workers: int) -> List[JobExecutionResult]:
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
    plan = build_batch_plan(spec)
    store = ResearchResultStore(default_db_path(actual_root))
    _prepare_batch(plan, spec, actual_root, store)
    results = _run_jobs(plan.jobs, actual_root, store, max_workers or spec.max_workers)
    return _finalize_batch(plan, results, actual_root, store)


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

    jobs = [JobSpec.from_dict(json.loads(Path(row["job_spec_path"]).read_text(encoding="utf-8"))) for row in failed_rows]
    batch = store.get_batch(batch_id)
    batch_spec = json.loads(batch["spec_json"])
    _run_jobs(jobs, actual_root, store, max_workers=max_workers)

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
        },
    )
    finalized = _finalize_batch(refreshed_plan, all_results, actual_root, store)
    finalized["rerun_count"] = len(jobs)
    return finalized
