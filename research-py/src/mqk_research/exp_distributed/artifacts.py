from __future__ import annotations

import json
import io
import os
import threading
from pathlib import Path
from typing import Any, Dict

import pandas as pd

from .hashing import sha256_bytes, sha256_file
from .models import JobSpec


def _atomic_write_bytes(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    content_hash = sha256_bytes(payload)[:16]
    temp_path = path.with_name(
        f".{path.name}.{content_hash}.{os.getpid()}.{threading.get_ident()}.tmp"
    )
    try:
        temp_path.write_bytes(payload)
        temp_path.replace(path)
    finally:
        temp_path.unlink(missing_ok=True)


def write_json(path: Path, payload: Any) -> None:
    rendered = json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False)
    _atomic_write_bytes(path, (rendered + "\n").encode("utf-8"))


def write_text(path: Path, text: str) -> None:
    _atomic_write_bytes(path, text.encode("utf-8"))


def write_csv(path: Path, frame: pd.DataFrame) -> None:
    rendered = io.StringIO()
    frame.to_csv(rendered, index=False)
    _atomic_write_bytes(path, rendered.getvalue().encode("utf-8"))


def batch_root(root: Path, batch_id: str) -> Path:
    return root / "artifacts" / "exp_distributed" / "batches" / batch_id


def job_root(root: Path, batch_id: str, job_index: int, job_id: str) -> Path:
    return batch_root(root, batch_id) / "jobs" / f"{job_index:04d}_{job_id}"


def job_spec_root(root: Path, batch_id: str) -> Path:
    return batch_root(root, batch_id) / "job_specs"


def _assert_registered_artifact_authority(
    root: Path,
    batch_id: str,
    *,
    execution_claim: str | None,
    registry_db_path: str | None,
    allow_planning: bool = False,
) -> None:
    """Refuse direct writes into a known batch without its SQLite claim.

    Low-level artifact helpers remain usable for isolated fixture construction
    when no registry row exists.  Once a registry DB identifies the batch,
    every shared write must carry the current execution claim.
    """
    db_path = Path(registry_db_path) if registry_db_path else root / "state" / "exp_research.sqlite3"
    if not db_path.exists():
        return
    from .storage import ResearchResultStore

    store = ResearchResultStore(db_path)
    try:
        batch = store.get_batch(batch_id)
    except KeyError:
        return
    if not execution_claim:
        if allow_planning and batch["status"] != "running":
            return
        raise RuntimeError(
            f"execution claim required for shared artifact writes: batch_id={batch_id!r}"
        )
    store.assert_execution_claim(batch_id, execution_claim)


def write_job_artifacts(
    root: Path,
    job: JobSpec,
    dataset_fingerprint: Dict[str, Any],
    metrics: Dict[str, Any],
    daily_returns: pd.DataFrame,
    positions: pd.DataFrame,
    trade_events: pd.DataFrame,
    failure_reason: str | None = None,
    execution_claim: str | None = None,
    registry_db_path: str | None = None,
) -> Dict[str, str]:
    _assert_registered_artifact_authority(
        root,
        job.batch_id,
        execution_claim=execution_claim,
        registry_db_path=registry_db_path,
    )
    artifact_dir = job_root(root, job.batch_id, job.job_index, job.job_id)
    artifact_dir.mkdir(parents=True, exist_ok=True)

    manifest_path = artifact_dir / "manifest.json"
    params_path = artifact_dir / "params.json"
    dataset_path = artifact_dir / "dataset_fingerprint.json"
    metrics_path = artifact_dir / "summary_metrics.json"
    returns_path = artifact_dir / "daily_returns.csv"
    positions_path = artifact_dir / "positions.csv"
    trades_path = artifact_dir / "trade_events.csv"
    status_path = artifact_dir / "status.json"
    log_path = artifact_dir / "run.log"

    write_json(
        manifest_path,
        {
            "schema_version": job.schema_version,
            "engine_id": job.engine_id,
            "batch_id": job.batch_id,
            "job_id": job.job_id,
            "job_index": job.job_index,
            "experiment_id": job.experiment_id,
            "strategy_id": job.strategy_id,
            "symbols": job.symbols,
            "window": job.window.to_dict(),
            "dataset_fingerprint": dataset_fingerprint,
        },
    )
    write_json(params_path, job.params)
    write_json(dataset_path, dataset_fingerprint)
    write_json(metrics_path, metrics)
    write_csv(returns_path, daily_returns)
    write_csv(positions_path, positions)
    write_csv(trades_path, trade_events)
    write_json(
        status_path,
        {
            "job_id": job.job_id,
            "status": "failed" if failure_reason else "succeeded",
            "failure_reason": failure_reason,
        },
    )
    write_text(
        log_path,
        "\n".join(
            [
                f"engine_id={job.engine_id}",
                f"batch_id={job.batch_id}",
                f"job_id={job.job_id}",
                f"strategy_id={job.strategy_id}",
                f"status={'failed' if failure_reason else 'succeeded'}",
                f"failure_reason={failure_reason or ''}",
            ]
        )
        + "\n",
    )

    files = {
        "artifact_dir": str(artifact_dir),
        "manifest": str(manifest_path),
        "params": str(params_path),
        "dataset_fingerprint": str(dataset_path),
        "summary_metrics": str(metrics_path),
        "daily_returns": str(returns_path),
        "positions": str(positions_path),
        "trade_events": str(trades_path),
        "status": str(status_path),
        "run_log": str(log_path),
    }

    metadata_path = artifact_dir / "artifact_metadata.json"
    write_json(
        metadata_path,
        {
            "job_id": job.job_id,
            "engine_id": job.engine_id,
            "files": {
                name: {"path": path, "sha256": sha256_file(Path(path)), "bytes": Path(path).stat().st_size}
                for name, path in files.items()
                if name != "artifact_dir"
            },
        },
    )
    files["artifact_metadata"] = str(metadata_path)
    return files


# RESEARCH-EXPERIMENT-REGISTRY-01-REPAIR-03
#
# job_root() is deterministic (batch_id, job_index, job_id) and write_job_artifacts
# overwrites in place on an exact retry, so `files` above (and artifact_metadata.json
# on disk) are current/source operational locations only — NOT durable per-attempt
# evidence. capture_artifact_evidence() snapshots artifact_metadata.json's own
# CONTENT (the file/sha256/bytes records write_job_artifacts already computed) so a
# caller can embed it into an immutable attempt-slice record BEFORE a later retry
# overwrites the same source path. Must be called synchronously right after the job
# that produced `artifact_paths` returns — not deferred, not re-derived later from
# the (by-then possibly-overwritten) source path.
def capture_artifact_evidence(artifact_paths: Dict[str, str]) -> Dict[str, Any]:
    metadata_path_raw = artifact_paths.get("artifact_metadata")
    if not metadata_path_raw:
        return {"artifact_files": {}, "artifact_metadata_sha256": None}
    metadata_bytes = Path(metadata_path_raw).read_bytes()
    metadata = json.loads(metadata_bytes.decode("utf-8"))
    return {
        "artifact_files": metadata.get("files", {}),
        "artifact_metadata_sha256": sha256_bytes(metadata_bytes),
    }


def write_job_spec(
    root: Path,
    job: JobSpec,
    *,
    execution_claim: str | None = None,
    registry_db_path: str | None = None,
    allow_planning: bool = False,
) -> Path:
    _assert_registered_artifact_authority(
        root,
        job.batch_id,
        execution_claim=execution_claim,
        registry_db_path=registry_db_path,
        allow_planning=allow_planning,
    )
    spec_path = job_spec_root(root, job.batch_id) / f"{job.job_index:04d}_{job.job_id}.json"
    write_json(spec_path, job.to_dict())
    return spec_path


def write_batch_manifest(
    root: Path,
    batch_id: str,
    manifest: Dict[str, Any],
    *,
    execution_claim: str | None = None,
    registry_db_path: str | None = None,
    allow_planning: bool = False,
) -> Path:
    _assert_registered_artifact_authority(
        root,
        batch_id,
        execution_claim=execution_claim,
        registry_db_path=registry_db_path,
        allow_planning=allow_planning,
    )
    manifest_path = batch_root(root, batch_id) / "batch_manifest.json"
    write_json(manifest_path, manifest)
    return manifest_path


def write_batch_artifacts(
    root: Path,
    batch_id: str,
    manifest: Dict[str, Any],
    leaderboard: pd.DataFrame,
    comparison: pd.DataFrame,
    sweep_summary: Dict[str, Any],
    reproducibility_manifest: Dict[str, Any],
    failure_report: Dict[str, Any],
    execution_claim: str | None = None,
    registry_db_path: str | None = None,
) -> Dict[str, str]:
    _assert_registered_artifact_authority(
        root,
        batch_id,
        execution_claim=execution_claim,
        registry_db_path=registry_db_path,
    )
    artifact_dir = batch_root(root, batch_id)
    manifest_path = artifact_dir / "batch_manifest.json"
    leaderboard_path = artifact_dir / "leaderboard.csv"
    comparison_path = artifact_dir / "comparison_table.csv"
    sweep_summary_path = artifact_dir / "sweep_summary.json"
    reproducibility_path = artifact_dir / "reproducibility_manifest.json"
    failure_report_path = artifact_dir / "aggregate_failure_report.json"

    write_json(manifest_path, manifest)
    write_csv(leaderboard_path, leaderboard)
    write_csv(comparison_path, comparison)
    write_json(sweep_summary_path, sweep_summary)
    write_json(reproducibility_path, reproducibility_manifest)
    write_json(failure_report_path, failure_report)

    return {
        "batch_manifest": str(manifest_path),
        "leaderboard": str(leaderboard_path),
        "comparison_table": str(comparison_path),
        "sweep_summary": str(sweep_summary_path),
        "reproducibility_manifest": str(reproducibility_path),
        "aggregate_failure_report": str(failure_report_path),
    }
