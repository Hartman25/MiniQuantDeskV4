from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict

import pandas as pd

from .hashing import sha256_file
from .models import JobSpec


def _atomic_write_bytes(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temp_path = path.with_suffix(path.suffix + ".tmp")
    temp_path.write_bytes(payload)
    temp_path.replace(path)


def write_json(path: Path, payload: Any) -> None:
    rendered = json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False)
    _atomic_write_bytes(path, (rendered + "\n").encode("utf-8"))


def write_text(path: Path, text: str) -> None:
    _atomic_write_bytes(path, text.encode("utf-8"))


def write_csv(path: Path, frame: pd.DataFrame) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temp_path = path.with_suffix(path.suffix + ".tmp")
    frame.to_csv(temp_path, index=False)
    temp_path.replace(path)


def batch_root(root: Path, batch_id: str) -> Path:
    return root / "artifacts" / "exp_distributed" / "batches" / batch_id


def job_root(root: Path, batch_id: str, job_index: int, job_id: str) -> Path:
    return batch_root(root, batch_id) / "jobs" / f"{job_index:04d}_{job_id}"


def job_spec_root(root: Path, batch_id: str) -> Path:
    return batch_root(root, batch_id) / "job_specs"


def write_job_artifacts(
    root: Path,
    job: JobSpec,
    dataset_fingerprint: Dict[str, Any],
    metrics: Dict[str, Any],
    daily_returns: pd.DataFrame,
    positions: pd.DataFrame,
    trade_events: pd.DataFrame,
    failure_reason: str | None = None,
) -> Dict[str, str]:
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


def write_job_spec(root: Path, job: JobSpec) -> Path:
    spec_path = job_spec_root(root, job.batch_id) / f"{job.job_index:04d}_{job.job_id}.json"
    write_json(spec_path, job.to_dict())
    return spec_path


def write_batch_artifacts(
    root: Path,
    batch_id: str,
    manifest: Dict[str, Any],
    leaderboard: pd.DataFrame,
    comparison: pd.DataFrame,
    sweep_summary: Dict[str, Any],
    reproducibility_manifest: Dict[str, Any],
    failure_report: Dict[str, Any],
) -> Dict[str, str]:
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
