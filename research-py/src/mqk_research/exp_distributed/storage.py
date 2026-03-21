from __future__ import annotations

import json
import sqlite3
from contextlib import closing
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional

from .models import JobExecutionResult, JobSpec


class ResearchResultStore:
    def __init__(self, db_path: Path) -> None:
        self.db_path = db_path
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        self._initialize()

    def _connect(self) -> sqlite3.Connection:
        connection = sqlite3.connect(self.db_path)
        connection.row_factory = sqlite3.Row
        return connection

    def _initialize(self) -> None:
        with closing(self._connect()) as connection:
            connection.executescript(
                """
                create table if not exists exp_batches (
                    batch_id text primary key,
                    engine_id text not null,
                    experiment_id text not null,
                    strategy_id text not null,
                    status text not null,
                    spec_json text not null,
                    batch_label text not null,
                    root_dir text not null,
                    job_count integer not null,
                    aggregate_paths_json text,
                    summary_json text
                );

                create table if not exists exp_jobs (
                    job_id text primary key,
                    batch_id text not null,
                    job_index integer not null,
                    engine_id text not null,
                    experiment_id text not null,
                    strategy_id text not null,
                    status text not null,
                    params_json text not null,
                    symbols_json text not null,
                    window_start_utc text not null,
                    window_end_utc text not null,
                    dataset_fingerprint_json text not null,
                    job_spec_path text not null,
                    artifact_dir text,
                    artifact_paths_json text,
                    metrics_json text,
                    failure_reason text,
                    runtime_seconds real,
                    unique(batch_id, job_index)
                );
                """
            )
            connection.commit()

    def upsert_batch(self, batch_id: str, spec: Dict[str, Any], root_dir: Path, job_count: int, status: str = "queued") -> None:
        with closing(self._connect()) as connection:
            connection.execute(
                """
                insert into exp_batches (
                    batch_id, engine_id, experiment_id, strategy_id, status, spec_json,
                    batch_label, root_dir, job_count, aggregate_paths_json, summary_json
                ) values (?, ?, ?, ?, ?, ?, ?, ?, ?, null, null)
                on conflict(batch_id) do update set
                    status=excluded.status,
                    spec_json=excluded.spec_json,
                    batch_label=excluded.batch_label,
                    root_dir=excluded.root_dir,
                    job_count=excluded.job_count
                """,
                (
                    batch_id,
                    spec["engine_id"],
                    spec["experiment_id"],
                    spec["strategy_id"],
                    status,
                    json.dumps(spec, sort_keys=True),
                    spec.get("batch_label", ""),
                    str(root_dir),
                    int(job_count),
                ),
            )
            connection.commit()

    def upsert_jobs(self, jobs: Iterable[JobSpec], spec_paths: Dict[str, Path], status: str = "queued") -> None:
        with closing(self._connect()) as connection:
            for job in jobs:
                connection.execute(
                    """
                    insert into exp_jobs (
                        job_id, batch_id, job_index, engine_id, experiment_id, strategy_id,
                        status, params_json, symbols_json, window_start_utc, window_end_utc,
                        dataset_fingerprint_json, job_spec_path, artifact_dir, artifact_paths_json,
                        metrics_json, failure_reason, runtime_seconds
                    ) values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, null, null, null, null, null)
                    on conflict(job_id) do update set
                        status=excluded.status,
                        params_json=excluded.params_json,
                        symbols_json=excluded.symbols_json,
                        window_start_utc=excluded.window_start_utc,
                        window_end_utc=excluded.window_end_utc,
                        dataset_fingerprint_json=excluded.dataset_fingerprint_json,
                        job_spec_path=excluded.job_spec_path,
                        failure_reason=null,
                        runtime_seconds=null
                    """,
                    (
                        job.job_id,
                        job.batch_id,
                        int(job.job_index),
                        job.engine_id,
                        job.experiment_id,
                        job.strategy_id,
                        status,
                        json.dumps(job.params, sort_keys=True),
                        json.dumps(job.symbols, sort_keys=True),
                        job.window.start_utc,
                        job.window.end_utc,
                        json.dumps(job.dataset_fingerprint.to_dict(), sort_keys=True),
                        str(spec_paths[job.job_id]),
                    ),
                )
            connection.commit()

    def set_job_status(self, job_id: str, status: str) -> None:
        with closing(self._connect()) as connection:
            connection.execute("update exp_jobs set status=? where job_id=?", (status, job_id))
            connection.commit()

    def persist_job_result(self, result: JobExecutionResult) -> None:
        with closing(self._connect()) as connection:
            connection.execute(
                """
                update exp_jobs
                set status=?,
                    artifact_dir=?,
                    artifact_paths_json=?,
                    metrics_json=?,
                    failure_reason=?,
                    runtime_seconds=?
                where job_id=?
                """,
                (
                    result.status,
                    result.artifact_paths.get("artifact_dir"),
                    json.dumps(result.artifact_paths, sort_keys=True),
                    json.dumps(result.metrics, sort_keys=True),
                    result.failure_reason,
                    result.runtime_seconds,
                    result.job_id,
                ),
            )
            connection.commit()

    def finalize_batch(
        self,
        batch_id: str,
        status: str,
        aggregate_paths: Dict[str, str],
        summary: Dict[str, Any],
    ) -> None:
        with closing(self._connect()) as connection:
            connection.execute(
                """
                update exp_batches
                set status=?, aggregate_paths_json=?, summary_json=?
                where batch_id=?
                """,
                (
                    status,
                    json.dumps(aggregate_paths, sort_keys=True),
                    json.dumps(summary, sort_keys=True),
                    batch_id,
                ),
            )
            connection.commit()

    def get_batch(self, batch_id: str) -> Dict[str, Any]:
        with closing(self._connect()) as connection:
            row = connection.execute("select * from exp_batches where batch_id=?", (batch_id,)).fetchone()
        if row is None:
            raise KeyError(f"unknown batch_id: {batch_id}")
        return dict(row)

    def list_jobs(self, batch_id: str, status: Optional[str] = None) -> List[Dict[str, Any]]:
        query = "select * from exp_jobs where batch_id=?"
        params: list[Any] = [batch_id]
        if status is not None:
            query += " and status=?"
            params.append(status)
        query += " order by job_index asc"
        with closing(self._connect()) as connection:
            rows = connection.execute(query, params).fetchall()
        return [dict(row) for row in rows]
