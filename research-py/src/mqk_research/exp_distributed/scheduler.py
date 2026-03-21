from __future__ import annotations

import itertools
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List

from .dataset import build_dataset_fingerprint
from .hashing import short_hash
from .models import BatchSpec, JobSpec


@dataclass(frozen=True)
class BatchPlan:
    batch_id: str
    jobs: List[JobSpec]
    batch_manifest: Dict[str, Any]


def _expand_parameter_grid(base_params: Dict[str, Any], parameter_grid: Dict[str, List[Any]]) -> List[Dict[str, Any]]:
    if not parameter_grid:
        return [dict(base_params)]
    keys = sorted(parameter_grid.keys())
    values = [parameter_grid[key] for key in keys]
    combinations: List[Dict[str, Any]] = []
    for raw_combo in itertools.product(*values):
        combo = dict(base_params)
        for key, value in zip(keys, raw_combo):
            combo[key] = value
        combinations.append(combo)
    return combinations


def build_batch_plan(spec: BatchSpec) -> BatchPlan:
    spec.validate()
    dataset_path = Path(spec.dataset_path)
    if not dataset_path.exists():
        raise FileNotFoundError(f"dataset path does not exist: {dataset_path}")

    batch_payload = spec.to_dict()
    batch_payload["dataset_path"] = str(dataset_path)
    batch_id = short_hash(batch_payload, length=24)
    dataset_sha256 = build_dataset_fingerprint(
        dataset_path=dataset_path,
        symbols=sorted({symbol for group in spec.symbol_groups for symbol in group}),
        start_utc=spec.windows[0].start_utc,
        end_utc=spec.windows[-1].end_utc,
        timeframe=spec.timeframe,
    ).dataset_sha256

    jobs: List[JobSpec] = []
    params_list = _expand_parameter_grid(spec.base_params, spec.parameter_grid)
    job_index = 0
    for window in spec.windows:
        for symbols in spec.symbol_groups:
            normalized_symbols = sorted(dict.fromkeys([symbol.upper() for symbol in symbols]))
            for params in params_list:
                fingerprint = build_dataset_fingerprint(
                    dataset_path=dataset_path,
                    symbols=normalized_symbols,
                    start_utc=window.start_utc,
                    end_utc=window.end_utc,
                    timeframe=spec.timeframe,
                    dataset_sha256=dataset_sha256,
                )
                job_payload = {
                    "batch_id": batch_id,
                    "experiment_id": spec.experiment_id,
                    "strategy_id": spec.strategy_id,
                    "symbols": normalized_symbols,
                    "window": window.to_dict(),
                    "params": params,
                    "dataset_fingerprint": fingerprint.to_dict(),
                }
                job_id = short_hash(job_payload, length=24)
                jobs.append(
                    JobSpec(
                        job_id=job_id,
                        batch_id=batch_id,
                        job_index=job_index,
                        experiment_id=spec.experiment_id,
                        strategy_id=spec.strategy_id,
                        params=params,
                        symbols=normalized_symbols,
                        window=window,
                        dataset_fingerprint=fingerprint,
                        timeframe=spec.timeframe,
                    )
                )
                job_index += 1

    manifest = {
        "schema_version": spec.schema_version,
        "engine_id": spec.engine_id,
        "batch_id": batch_id,
        "experiment_id": spec.experiment_id,
        "batch_label": spec.batch_label,
        "strategy_id": spec.strategy_id,
        "dataset_path": str(dataset_path),
        "dataset_sha256": dataset_sha256,
        "job_count": len(jobs),
        "max_workers": spec.max_workers,
        "notes": spec.notes,
    }
    return BatchPlan(batch_id=batch_id, jobs=jobs, batch_manifest=manifest)
