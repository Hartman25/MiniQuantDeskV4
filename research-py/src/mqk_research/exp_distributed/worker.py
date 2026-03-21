from __future__ import annotations

import traceback
from pathlib import Path
from time import perf_counter
from typing import Any, Dict

import pandas as pd

from .artifacts import write_job_artifacts
from .dataset import finalize_dataset_fingerprint, load_job_slice
from .models import JobExecutionResult, JobSpec
from .strategies import run_strategy


def run_job_worker(job_payload: Dict[str, Any], root_dir: str) -> Dict[str, Any]:
    job = JobSpec.from_dict(job_payload)
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
        )
        run_log = Path(artifact_paths["run_log"])
        run_log.write_text(
            run_log.read_text(encoding="utf-8") + "\n" + traceback.format_exc(),
            encoding="utf-8",
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
