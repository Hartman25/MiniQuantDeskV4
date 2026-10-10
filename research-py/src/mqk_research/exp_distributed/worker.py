from __future__ import annotations

import traceback
from pathlib import Path
from time import perf_counter
from typing import Any, Dict

import pandas as pd

from .artifacts import write_job_artifacts
from .dataset import finalize_dataset_fingerprint, load_job_slice
from .models import JobExecutionResult, JobSpec
from .storage import ResearchResultStore
from .strategies import run_strategy


def _assert_worker_authority(job: JobSpec, job_payload: Dict[str, Any], root_dir: str) -> tuple[str | None, str | None]:
    execution_claim = job_payload.get("_execution_claim")
    registry_db_path = job_payload.get("_registry_db_path")
    if execution_claim:
        if not registry_db_path:
            raise RuntimeError("execution claim is missing its registry database path")
        ResearchResultStore(Path(str(registry_db_path))).assert_execution_claim(
            job.batch_id, str(execution_claim)
        )
        return str(execution_claim), str(registry_db_path)
    if not job_payload.get("_unregistered_diagnostic"):
        raise RuntimeError(
            "worker execution requires a SQLite execution claim; "
            "standalone execution must be explicitly diagnostic"
        )
    return None, None


def run_job_worker(job_payload: Dict[str, Any], root_dir: str) -> Dict[str, Any]:
    job = JobSpec.from_dict(job_payload)
    execution_claim, registry_db_path = _assert_worker_authority(job, job_payload, root_dir)
    started = perf_counter()
    try:
        filtered = load_job_slice(job)
        realized_fingerprint = finalize_dataset_fingerprint(job.dataset_fingerprint, filtered)
        strategy_result = run_strategy(job.strategy_id, filtered, job.params)
        artifact_paths = write_job_artifacts(
            root=Path(root_dir),
            job=job,
            dataset_fingerprint=realized_fingerprint.to_dict(),
            metrics=strategy_result.metrics,
            daily_returns=strategy_result.daily_returns,
            positions=strategy_result.positions,
            trade_events=strategy_result.trade_events,
            execution_claim=execution_claim,
            registry_db_path=registry_db_path,
        )
        runtime_seconds = round(perf_counter() - started, 6)
        return JobExecutionResult(
            job_id=job.job_id,
            batch_id=job.batch_id,
            status="succeeded",
            metrics=strategy_result.metrics,
            artifact_paths=artifact_paths,
            runtime_seconds=runtime_seconds,
        ).to_dict()
    except Exception as exc:
        failure_reason = f"{type(exc).__name__}: {exc}"
        artifact_paths = write_job_artifacts(
            root=Path(root_dir),
            job=job,
            dataset_fingerprint=job.dataset_fingerprint.to_dict(),
            metrics={},
            daily_returns=pd.DataFrame(columns=["ts_utc", "portfolio_return"]),
            positions=pd.DataFrame(columns=["ts_utc"] + list(job.symbols)),
            trade_events=pd.DataFrame(columns=["ts_utc", "symbol", "event_type", "old_weight", "new_weight"]),
            failure_reason=failure_reason,
            execution_claim=execution_claim,
            registry_db_path=registry_db_path,
        )
        run_log = Path(artifact_paths["run_log"])
        if execution_claim and registry_db_path:
            ResearchResultStore(Path(registry_db_path)).assert_execution_claim(
                job.batch_id, execution_claim
            )
        from .artifacts import write_text

        write_text(
            run_log,
            run_log.read_text(encoding="utf-8") + "\n" + traceback.format_exc(),
        )
        runtime_seconds = round(perf_counter() - started, 6)
        return JobExecutionResult(
            job_id=job.job_id,
            batch_id=job.batch_id,
            status="failed",
            metrics={},
            artifact_paths=artifact_paths,
            failure_reason=failure_reason,
            runtime_seconds=runtime_seconds,
        ).to_dict()
