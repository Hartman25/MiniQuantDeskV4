from __future__ import annotations

from typing import Any, Dict, Iterable, List, Tuple

import pandas as pd

from .hashing import short_hash
from .models import JobExecutionResult, JobSpec


def _rows_from_results(jobs: Iterable[JobSpec], results: Iterable[JobExecutionResult]) -> pd.DataFrame:
    result_by_id = {result.job_id: result for result in results}
    rows: List[Dict[str, Any]] = []
    for job in jobs:
        result = result_by_id[job.job_id]
        row: Dict[str, Any] = {
            "job_index": job.job_index,
            "job_id": job.job_id,
            "status": result.status,
            "strategy_id": job.strategy_id,
            "symbols": ",".join(job.symbols),
            "window_label": job.window.label,
            "window_start_utc": job.window.start_utc,
            "window_end_utc": job.window.end_utc,
            "runtime_seconds": result.runtime_seconds,
            "failure_reason": result.failure_reason or "",
            "metric_sharpe": None,
            "metric_total_return": None,
            "metric_max_drawdown": None,
            "metric_trade_event_count": None,
        }
        for key, value in sorted(job.params.items()):
            row[f"param_{key}"] = value
        for key, value in sorted(result.metrics.items()):
            row[f"metric_{key}"] = value
        rows.append(row)
    return pd.DataFrame(rows).sort_values(
        ["status", "metric_sharpe", "metric_total_return", "job_index"],
        ascending=[True, False, False, True],
        kind="mergesort",
        na_position="last",
    )


def aggregate_results(batch_manifest: Dict[str, Any], jobs: List[JobSpec], results: List[JobExecutionResult]) -> Tuple[pd.DataFrame, pd.DataFrame, Dict[str, Any], Dict[str, Any], Dict[str, Any]]:
    comparison = _rows_from_results(jobs, results)
    succeeded = comparison[comparison["status"] == "succeeded"].copy()
    failed = comparison[comparison["status"] == "failed"].copy()

    leaderboard_columns = [
        "job_index",
        "job_id",
        "symbols",
        "window_label",
        "metric_sharpe",
        "metric_total_return",
        "metric_max_drawdown",
        "metric_trade_event_count",
        "runtime_seconds",
    ]
    leaderboard = succeeded[leaderboard_columns].copy() if not succeeded.empty else pd.DataFrame(columns=leaderboard_columns)

    summary = {
        "batch_id": batch_manifest["batch_id"],
        "engine_id": batch_manifest["engine_id"],
        "job_count": int(len(jobs)),
        "succeeded": int(len(succeeded)),
        "failed": int(len(failed)),
        "best_job_id": None if succeeded.empty else str(leaderboard.iloc[0]["job_id"]),
        "best_sharpe": None if succeeded.empty else float(leaderboard.iloc[0]["metric_sharpe"]),
        "best_total_return": None if succeeded.empty else float(leaderboard.iloc[0]["metric_total_return"]),
    }
    summary["summary_hash"] = short_hash(summary, length=24)

    reproducibility_manifest = {
        "batch_id": batch_manifest["batch_id"],
        "engine_id": batch_manifest["engine_id"],
        "dataset_sha256": batch_manifest["dataset_sha256"],
        "strategy_id": batch_manifest["strategy_id"],
        "job_ids": [job.job_id for job in jobs],
        "job_count": len(jobs),
        "summary_hash": summary["summary_hash"],
    }

    failure_report = {
        "batch_id": batch_manifest["batch_id"],
        "failed_jobs": failed[["job_id", "failure_reason"]].to_dict(orient="records") if not failed.empty else [],
    }
    return leaderboard, comparison, summary, reproducibility_manifest, failure_report
