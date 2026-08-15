from __future__ import annotations

import json
import sqlite3
from contextlib import closing
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional

from .models import JobExecutionResult, JobSpec

# RESEARCH-EXPERIMENT-REGISTRY-01
# Distinct contract from exp-distributed-v1 (BatchSpec/JobSpec) even though both
# live in the same SQLite authority. See CLAUDE.md: determinism first, fail-closed.
REGISTRY_SCHEMA_VERSION = "research-experiment-registry-v1"

_TERMINAL_ATTEMPT_STATUSES = ("succeeded", "failed", "blocked")

# RESEARCH-HOLDOUT-CONSUMPTION-LEDGER-01
# Distinct contract from the hypothesis/trial/attempt tables above, sharing
# the same SQLite authority. Purely additive: does not redefine what
# "reserved_not_evaluated" means anywhere else in the codebase (see
# eval_walkforward.py / economic_walkforward.py), and no code path in this
# patch ever reads or scores holdout-region data. See
# mqk_research.ml.holdout_ledger for the deterministic holdout_id and the
# public entry points that wrap the methods below.
HOLDOUT_LEDGER_SCHEMA_VERSION = "research_holdout_ledger_v1"
_HOLDOUT_STATUS_RESERVED = "reserved"
_HOLDOUT_STATUS_CONSUMED = "consumed"


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

                create table if not exists research_hypotheses (
                    hypothesis_id text primary key,
                    experiment_id text not null,
                    hypothesis_text text,
                    schema_version text not null
                );

                create table if not exists research_trials (
                    trial_id text primary key,
                    experiment_id text not null,
                    hypothesis_id text not null,
                    strategy_id text not null,
                    protocol_id text not null,
                    identity_json text not null,
                    schema_version text not null
                );

                create table if not exists research_attempts (
                    attempt_id text primary key,
                    trial_id text not null,
                    attempt_index integer not null,
                    status text not null,
                    origin text,
                    result_id text,
                    artifact_paths_json text,
                    result_summary_json text,
                    failure_reason text,
                    metadata_json text,
                    unique(trial_id, attempt_index)
                );

                create table if not exists research_attempt_jobs (
                    attempt_id text not null,
                    job_id text not null,
                    primary key (attempt_id, job_id)
                );

                create table if not exists research_attempt_slices (
                    attempt_id text not null,
                    job_id text not null,
                    trial_id text not null,
                    window_start_utc text,
                    window_end_utc text,
                    status text not null,
                    failure_reason text,
                    artifact_paths_json text,
                    artifact_evidence_json text,
                    metrics_json text,
                    runtime_seconds real,
                    attempt_scope text,
                    schema_version text not null,
                    primary key (attempt_id, job_id)
                );

                create index if not exists idx_research_trials_experiment
                    on research_trials(experiment_id);
                create index if not exists idx_research_trials_hypothesis
                    on research_trials(hypothesis_id);
                create index if not exists idx_research_attempts_trial
                    on research_attempts(trial_id);
                create index if not exists idx_research_attempt_jobs_job
                    on research_attempt_jobs(job_id);
                create index if not exists idx_research_attempt_slices_job
                    on research_attempt_slices(job_id);
                create index if not exists idx_research_attempt_slices_trial
                    on research_attempt_slices(trial_id);

                create table if not exists research_holdout_ledger (
                    holdout_id text primary key,
                    status text not null,
                    dataset_identity_json text not null,
                    holdout_start_utc text not null,
                    holdout_end_utc text not null,
                    protocol_version text not null,
                    consumer_identity_json text,
                    result_identity text,
                    consumed_at text,
                    schema_version text not null
                );
                """
            )
            connection.commit()
            # RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-03: a pre-existing DB file
            # created before this column existed has research_attempt_slices
            # without it — "create table if not exists" does not alter an
            # already-created table. Add it explicitly so reopening an older DB
            # doesn't silently lose immutable-evidence capture.
            existing_columns = {
                row[1] for row in connection.execute("PRAGMA table_info(research_attempt_slices)").fetchall()
            }
            if "artifact_evidence_json" not in existing_columns:
                connection.execute(
                    "alter table research_attempt_slices add column artifact_evidence_json text"
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

    # ------------------------------------------------------------------
    # RESEARCH-EXPERIMENT-REGISTRY-01
    #
    # hypothesis -> trial -> attempt. Trial identity is result-independent and
    # immutable once created; attempts are append-only (a rerun creates a new
    # attempt row, never overwrites a prior one). See docstrings on each method
    # for the exact fail-closed contract.
    # ------------------------------------------------------------------

    def register_hypothesis(
        self,
        *,
        hypothesis_id: str,
        experiment_id: str,
        hypothesis_text: Optional[str] = None,
    ) -> None:
        if not hypothesis_id.strip():
            raise ValueError("hypothesis_id is required")
        if not experiment_id.strip():
            raise ValueError("experiment_id is required")
        with closing(self._connect()) as connection:
            row = connection.execute(
                "select experiment_id, hypothesis_text from research_hypotheses where hypothesis_id=?",
                (hypothesis_id,),
            ).fetchone()
            if row is not None:
                existing_experiment_id, existing_text = row
                if existing_experiment_id != experiment_id:
                    raise RuntimeError(
                        f"hypothesis_id collision: {hypothesis_id!r} is already registered under "
                        f"experiment_id {existing_experiment_id!r}, not {experiment_id!r}"
                    )
                if hypothesis_text and existing_text and existing_text != hypothesis_text:
                    raise RuntimeError(
                        f"hypothesis_id collision: {hypothesis_id!r} is already registered with a "
                        "different hypothesis_text"
                    )
                if hypothesis_text and not existing_text:
                    connection.execute(
                        "update research_hypotheses set hypothesis_text=? where hypothesis_id=?",
                        (hypothesis_text, hypothesis_id),
                    )
                    connection.commit()
                return
            connection.execute(
                """
                insert into research_hypotheses (hypothesis_id, experiment_id, hypothesis_text, schema_version)
                values (?, ?, ?, ?)
                """,
                (hypothesis_id, experiment_id, hypothesis_text, REGISTRY_SCHEMA_VERSION),
            )
            connection.commit()

    def register_trial(
        self,
        *,
        trial_id: str,
        experiment_id: str,
        hypothesis_id: str,
        strategy_id: str,
        protocol_id: str,
        identity: Dict[str, Any],
    ) -> None:
        """Idempotently register a trial. Fails closed if `trial_id` is reused
        with conflicting identity content (hash collision / corruption / bug)."""
        identity_json = json.dumps(identity, sort_keys=True, separators=(",", ":"))
        with closing(self._connect()) as connection:
            row = connection.execute(
                """
                select experiment_id, hypothesis_id, strategy_id, protocol_id, identity_json
                from research_trials where trial_id=?
                """,
                (trial_id,),
            ).fetchone()
            if row is not None:
                if tuple(row) != (experiment_id, hypothesis_id, strategy_id, protocol_id, identity_json):
                    raise RuntimeError(
                        f"trial_id collision with conflicting canonical identity: {trial_id!r}"
                    )
                return
            connection.execute(
                """
                insert into research_trials (
                    trial_id, experiment_id, hypothesis_id, strategy_id, protocol_id,
                    identity_json, schema_version
                ) values (?, ?, ?, ?, ?, ?, ?)
                """,
                (trial_id, experiment_id, hypothesis_id, strategy_id, protocol_id, identity_json, REGISTRY_SCHEMA_VERSION),
            )
            connection.commit()

    def begin_attempt(
        self,
        *,
        trial_id: str,
        origin: Optional[str] = None,
        metadata: Optional[Dict[str, Any]] = None,
    ) -> tuple[str, int]:
        """Atomically allocate the next attempt_index for `trial_id` and insert a
        'started' attempt row. Must be called BEFORE the evaluation it represents
        runs, so a failure mid-evaluation still leaves durable evidence."""
        with closing(self._connect()) as connection:
            connection.isolation_level = None
            connection.execute("PRAGMA busy_timeout=5000")
            connection.execute("BEGIN IMMEDIATE")
            try:
                exists = connection.execute(
                    "select 1 from research_trials where trial_id=?", (trial_id,)
                ).fetchone()
                if exists is None:
                    raise KeyError(f"unknown trial_id: {trial_id}")
                row = connection.execute(
                    "select coalesce(max(attempt_index), 0) + 1 from research_attempts where trial_id=?",
                    (trial_id,),
                ).fetchone()
                attempt_index = int(row[0])
                attempt_id = f"{trial_id}:att{attempt_index:04d}"
                connection.execute(
                    """
                    insert into research_attempts (
                        attempt_id, trial_id, attempt_index, status, origin,
                        result_id, artifact_paths_json, result_summary_json,
                        failure_reason, metadata_json
                    ) values (?, ?, ?, 'started', ?, null, null, null, null, ?)
                    """,
                    (attempt_id, trial_id, attempt_index, origin, json.dumps(metadata or {}, sort_keys=True)),
                )
                connection.commit()
            except Exception:
                connection.rollback()
                raise
        return attempt_id, attempt_index

    def finalize_attempt(
        self,
        attempt_id: str,
        *,
        status: str,
        result_id: Optional[str] = None,
        artifact_paths: Optional[Dict[str, Any]] = None,
        result_summary: Optional[Dict[str, Any]] = None,
        failure_reason: Optional[str] = None,
    ) -> None:
        """Transition a 'started' attempt to a terminal status. Fails closed if the
        attempt is unknown or already terminal — terminal attempts are never
        reopened or overwritten (append-only attempt history)."""
        if status not in _TERMINAL_ATTEMPT_STATUSES:
            raise ValueError(f"invalid terminal attempt status: {status!r}")
        with closing(self._connect()) as connection:
            connection.isolation_level = None
            connection.execute("BEGIN IMMEDIATE")
            try:
                row = connection.execute(
                    "select status from research_attempts where attempt_id=?", (attempt_id,)
                ).fetchone()
                if row is None:
                    raise KeyError(f"unknown attempt_id: {attempt_id}")
                if row[0] != "started":
                    raise RuntimeError(
                        f"attempt {attempt_id!r} is already terminal (status={row[0]!r}); "
                        "terminal attempts cannot be reopened or refinalized"
                    )
                connection.execute(
                    """
                    update research_attempts
                    set status=?, result_id=?, artifact_paths_json=?, result_summary_json=?, failure_reason=?
                    where attempt_id=?
                    """,
                    (
                        status,
                        result_id,
                        json.dumps(artifact_paths, sort_keys=True) if artifact_paths is not None else None,
                        json.dumps(result_summary, sort_keys=True) if result_summary is not None else None,
                        failure_reason,
                        attempt_id,
                    ),
                )
                connection.commit()
            except Exception:
                connection.rollback()
                raise

    def link_attempt_jobs(self, attempt_id: str, job_ids: Iterable[str]) -> None:
        """Durably record that `attempt_id` covers each of `job_ids` (evaluation
        slices). One candidate attempt may own multiple window jobs
        (RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-01). Idempotent — relinking the
        same (attempt_id, job_id) pair is a no-op."""
        with closing(self._connect()) as connection:
            for job_id in job_ids:
                connection.execute(
                    "insert or ignore into research_attempt_jobs (attempt_id, job_id) values (?, ?)",
                    (attempt_id, job_id),
                )
            connection.commit()

    def list_attempt_jobs(self, attempt_id: str) -> List[str]:
        with closing(self._connect()) as connection:
            rows = connection.execute(
                "select job_id from research_attempt_jobs where attempt_id=? order by job_id asc",
                (attempt_id,),
            ).fetchall()
        return [row[0] for row in rows]

    def record_attempt_slices(
        self,
        attempt_id: str,
        trial_id: str,
        slices: Iterable[Dict[str, Any]],
    ) -> None:
        """Durably record an IMMUTABLE snapshot of what each linked evaluation
        slice (job) observed for THIS attempt (RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-02).

        `exp_jobs` is the current-state operational table for a deterministic
        job_id — it is legitimately overwritten every time that job_id is
        re-executed (e.g. by a later attempt's retry) and is therefore NOT
        durable per-attempt evidence. This table is: each row is scoped to
        (attempt_id, job_id), so the same deterministic job_id can appear
        under many attempts without any of them mutating another's history.

        Insert-only and fail-closed: recording a snapshot for an
        (attempt_id, job_id) pair that already exists raises instead of
        overwriting — terminal slice evidence is immutable through the public
        API, with no update path at all.

        Each item in `slices` is a dict with at least `job_id` and `status`;
        optional keys: `window_start_utc`, `window_end_utc`, `failure_reason`,
        `artifact_paths`, `metrics`, `runtime_seconds`, `attempt_scope`,
        `artifact_evidence`.

        RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-03 — `artifact_paths` and
        `artifact_evidence` are NOT the same thing and must not be conflated:

          * `artifact_paths` (-> artifact_paths_json) is the SOURCE/CURRENT
            operational job artifact location on disk at the deterministic
            path `artifacts/exp_distributed/batches/<batch_id>/jobs/<job_index>_<job_id>/`.
            That directory is reused and its files overwritten by a later
            retry of the same job_id — a bare path is NOT immutable evidence
            of what THIS attempt produced.
          * `artifact_evidence` (-> artifact_evidence_json) is the IMMUTABLE
            historical evidence captured for this attempt at finalization
            time, before any later retry could overwrite the source path —
            see exp_distributed/artifacts.capture_artifact_evidence, which
            snapshots artifact_metadata.json's own file/sha256/bytes records
            (reusing write_job_artifacts' hashes rather than a second,
            incompatible convention) plus this slice's terminal status/
            failure_reason/metrics/runtime, so it stands on its own without
            depending on the mutable source path ever being re-read.
        """
        slices = list(slices)
        with closing(self._connect()) as connection:
            connection.isolation_level = None
            connection.execute("PRAGMA busy_timeout=5000")
            connection.execute("BEGIN IMMEDIATE")
            try:
                for slc in slices:
                    job_id = slc["job_id"]
                    existing = connection.execute(
                        "select 1 from research_attempt_slices where attempt_id=? and job_id=?",
                        (attempt_id, job_id),
                    ).fetchone()
                    if existing is not None:
                        raise RuntimeError(
                            f"attempt slice snapshot is immutable and already recorded: "
                            f"attempt_id={attempt_id!r} job_id={job_id!r}"
                        )
                    connection.execute(
                        """
                        insert into research_attempt_slices (
                            attempt_id, job_id, trial_id, window_start_utc, window_end_utc,
                            status, failure_reason, artifact_paths_json, artifact_evidence_json,
                            metrics_json, runtime_seconds, attempt_scope, schema_version
                        ) values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                        """,
                        (
                            attempt_id,
                            job_id,
                            trial_id,
                            slc.get("window_start_utc"),
                            slc.get("window_end_utc"),
                            slc["status"],
                            slc.get("failure_reason"),
                            json.dumps(slc.get("artifact_paths") or {}, sort_keys=True),
                            json.dumps(slc.get("artifact_evidence") or {}, sort_keys=True),
                            json.dumps(slc.get("metrics") or {}, sort_keys=True),
                            slc.get("runtime_seconds"),
                            slc.get("attempt_scope"),
                            REGISTRY_SCHEMA_VERSION,
                        ),
                    )
                connection.commit()
            except Exception:
                connection.rollback()
                raise

    def list_attempt_slices(self, attempt_id: str) -> List[Dict[str, Any]]:
        """Immutable, attempt-specific slice evidence — the durable proof
        standard for RESEARCH-EXPERIMENT-REGISTRY-01. Reads ONLY from
        `research_attempt_slices`, never from `exp_jobs`' mutable current
        state, so a later attempt's retry of the same deterministic job_id
        cannot change what an earlier attempt observed.

        `artifact_paths` (source/current operational job artifact location —
        may since have been overwritten by a later retry of the same job_id)
        and `artifact_evidence` (immutable historical evidence captured for
        THIS attempt, safe to trust after such an overwrite — see
        RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-03 and
        record_attempt_slices' docstring) are deliberately distinct keys."""
        with closing(self._connect()) as connection:
            rows = connection.execute(
                """
                select attempt_id, job_id, trial_id, window_start_utc, window_end_utc,
                       status, failure_reason, artifact_paths_json, artifact_evidence_json,
                       metrics_json, runtime_seconds, attempt_scope, schema_version
                from research_attempt_slices where attempt_id=? order by job_id asc
                """,
                (attempt_id,),
            ).fetchall()
        slices: List[Dict[str, Any]] = []
        for row in rows:
            record = dict(row)
            record["artifact_paths"] = json.loads(record.pop("artifact_paths_json") or "{}")
            record["artifact_evidence"] = json.loads(record.pop("artifact_evidence_json") or "{}")
            record["metrics"] = json.loads(record.pop("metrics_json") or "{}")
            slices.append(record)
        return slices

    def get_trial(self, trial_id: str) -> Dict[str, Any]:
        with closing(self._connect()) as connection:
            row = connection.execute("select * from research_trials where trial_id=?", (trial_id,)).fetchone()
        if row is None:
            raise KeyError(f"unknown trial_id: {trial_id}")
        return dict(row)

    def list_trials(
        self,
        *,
        experiment_id: Optional[str] = None,
        hypothesis_id: Optional[str] = None,
        strategy_id: Optional[str] = None,
    ) -> List[Dict[str, Any]]:
        query = "select * from research_trials where 1=1"
        params: list[Any] = []
        if experiment_id is not None:
            query += " and experiment_id=?"
            params.append(experiment_id)
        if hypothesis_id is not None:
            query += " and hypothesis_id=?"
            params.append(hypothesis_id)
        if strategy_id is not None:
            query += " and strategy_id=?"
            params.append(strategy_id)
        query += " order by trial_id asc"
        with closing(self._connect()) as connection:
            rows = connection.execute(query, params).fetchall()
        return [dict(row) for row in rows]

    def list_attempts(self, trial_id: str) -> List[Dict[str, Any]]:
        with closing(self._connect()) as connection:
            rows = connection.execute(
                "select * from research_attempts where trial_id=? order by attempt_index asc",
                (trial_id,),
            ).fetchall()
        return [dict(row) for row in rows]

    def registry_summary(
        self,
        *,
        experiment_id: Optional[str] = None,
        hypothesis_id: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Raw durable facts only — not a multiple-testing judgment. Exposes
        unique trial count and attempt counts by status, kept strictly separate
        so callers cannot conflate a rerun with a new statistical trial."""
        trials = self.list_trials(experiment_id=experiment_id, hypothesis_id=hypothesis_id)
        trial_ids = [t["trial_id"] for t in trials]
        counts = {"started": 0, "succeeded": 0, "failed": 0, "blocked": 0}
        total_attempts = 0
        if trial_ids:
            placeholders = ",".join("?" for _ in trial_ids)
            with closing(self._connect()) as connection:
                rows = connection.execute(
                    f"select status, count(*) from research_attempts where trial_id in ({placeholders}) group by status",
                    trial_ids,
                ).fetchall()
            for status, n in rows:
                counts[status] = int(n)
                total_attempts += int(n)
        return {
            "unique_trials": len(trial_ids),
            "attempts": total_attempts,
            "started_attempts": counts["started"],
            "succeeded_attempts": counts["succeeded"],
            "failed_attempts": counts["failed"],
            "blocked_attempts": counts["blocked"],
        }

    # ------------------------------------------------------------------
    # RESEARCH-HOLDOUT-CONSUMPTION-LEDGER-01
    #
    # Durable record of a dataset's reserved final holdout region moving from
    # RESERVED to CONSUMED. `holdout_id` is a deterministic content hash the
    # caller computes from immutable facts only (see
    # mqk_research.ml.holdout_ledger.compute_holdout_id) -- this class never
    # computes it and never reads or scores any holdout-region data itself.
    # ------------------------------------------------------------------

    def reserve_holdout(
        self,
        *,
        holdout_id: str,
        dataset_identity: Dict[str, Any],
        holdout_start_utc: str,
        holdout_end_utc: str,
        protocol_version: str,
    ) -> None:
        """Idempotently record that `holdout_id` exists in the ledger. Safe to
        call repeatedly with the SAME facts (e.g. independent research runs
        against the same dataset/boundary reserving the same holdout) -- a
        no-op if the row already exists with identical facts, regardless of
        its current status (reserving an already-CONSUMED holdout must never
        downgrade it back to reserved). Fails closed if `holdout_id` already
        exists with CONFLICTING facts -- a genuine hash collision or a caller
        bug, either way not something to silently trust."""
        dataset_identity_json = json.dumps(dataset_identity, sort_keys=True, separators=(",", ":"))
        with closing(self._connect()) as connection:
            connection.execute(
                """
                insert or ignore into research_holdout_ledger (
                    holdout_id, status, dataset_identity_json, holdout_start_utc, holdout_end_utc,
                    protocol_version, consumer_identity_json, result_identity, consumed_at, schema_version
                ) values (?, ?, ?, ?, ?, ?, null, null, null, ?)
                """,
                (
                    holdout_id,
                    _HOLDOUT_STATUS_RESERVED,
                    dataset_identity_json,
                    holdout_start_utc,
                    holdout_end_utc,
                    protocol_version,
                    HOLDOUT_LEDGER_SCHEMA_VERSION,
                ),
            )
            connection.commit()
            row = connection.execute(
                """
                select dataset_identity_json, holdout_start_utc, holdout_end_utc, protocol_version
                from research_holdout_ledger where holdout_id=?
                """,
                (holdout_id,),
            ).fetchone()
        if row is None:
            raise RuntimeError(f"failed to reserve holdout_id={holdout_id!r}")
        if tuple(row) != (dataset_identity_json, holdout_start_utc, holdout_end_utc, protocol_version):
            raise RuntimeError(f"holdout_id collision with conflicting facts: {holdout_id!r}")

    def consume_holdout(
        self,
        *,
        holdout_id: str,
        consumer_identity: Dict[str, Any],
        consumed_at: str,
        result_identity: Optional[str] = None,
    ) -> None:
        """Atomically transition `holdout_id` from RESERVED to CONSUMED.
        Fails closed if the holdout was never reserved (unknown holdout_id --
        callers must reserve_holdout first) or is already CONSUMED (by this
        or any other caller; the original consumer's record is never
        overwritten). Uses the same BEGIN IMMEDIATE pattern as
        finalize_attempt: SQLite's write lock serializes concurrent callers
        racing to consume the SAME holdout_id, so exactly one transaction
        observes status='reserved' and wins; every other transaction (whether
        it started before or after the winner committed) observes
        status='consumed' and raises."""
        consumer_identity_json = json.dumps(consumer_identity, sort_keys=True, separators=(",", ":"))
        with closing(self._connect()) as connection:
            connection.isolation_level = None
            connection.execute("PRAGMA busy_timeout=5000")
            connection.execute("BEGIN IMMEDIATE")
            try:
                row = connection.execute(
                    "select status from research_holdout_ledger where holdout_id=?", (holdout_id,)
                ).fetchone()
                if row is None:
                    raise KeyError(f"unknown holdout_id (never reserved): {holdout_id!r}")
                if row[0] != _HOLDOUT_STATUS_RESERVED:
                    raise RuntimeError(
                        f"holdout {holdout_id!r} is already consumed; a reserved final holdout "
                        "can be consumed at most once and its original consumer is never overwritten"
                    )
                connection.execute(
                    """
                    update research_holdout_ledger
                    set status=?, consumer_identity_json=?, result_identity=?, consumed_at=?
                    where holdout_id=?
                    """,
                    (_HOLDOUT_STATUS_CONSUMED, consumer_identity_json, result_identity, consumed_at, holdout_id),
                )
                connection.commit()
            except Exception:
                connection.rollback()
                raise

    def get_holdout(self, holdout_id: str) -> Dict[str, Any]:
        with closing(self._connect()) as connection:
            row = connection.execute(
                "select * from research_holdout_ledger where holdout_id=?", (holdout_id,)
            ).fetchone()
        if row is None:
            raise KeyError(f"unknown holdout_id: {holdout_id}")
        record = dict(row)
        record["dataset_identity"] = json.loads(record.pop("dataset_identity_json"))
        consumer_json = record.pop("consumer_identity_json")
        record["consumer_identity"] = json.loads(consumer_json) if consumer_json else None
        return record
