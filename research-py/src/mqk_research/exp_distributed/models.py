from __future__ import annotations

from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional


SCHEMA_VERSION = "exp-distributed-v1"
ENGINE_ID = "EXP"


@dataclass(frozen=True)
class WindowSpec:
    start_utc: str
    end_utc: str
    label: str = ""

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


@dataclass(frozen=True)
class DatasetFingerprint:
    dataset_path: str
    dataset_sha256: str
    selection_sha256: str
    timeframe: str
    symbols: List[str]
    start_utc: str
    end_utc: str
    filtered_row_count: Optional[int] = None
    filtered_sha256: Optional[str] = None

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)


@dataclass(frozen=True)
class BatchSpec:
    experiment_id: str
    dataset_path: str
    strategy_id: str
    windows: List[WindowSpec]
    symbol_groups: List[List[str]]
    base_params: Dict[str, Any] = field(default_factory=dict)
    parameter_grid: Dict[str, List[Any]] = field(default_factory=dict)
    timeframe: str = "1D"
    batch_label: str = ""
    max_workers: int = 1
    notes: List[str] = field(default_factory=list)
    schema_version: str = SCHEMA_VERSION
    engine_id: str = ENGINE_ID

    def validate(self) -> None:
        if self.schema_version != SCHEMA_VERSION:
            raise ValueError(f"unsupported batch schema_version: {self.schema_version}")
        if self.engine_id != ENGINE_ID:
            raise ValueError(f"batch engine_id must be {ENGINE_ID}")
        if not self.experiment_id.strip():
            raise ValueError("experiment_id is required")
        if not self.strategy_id.strip():
            raise ValueError("strategy_id is required")
        if self.max_workers < 1:
            raise ValueError("max_workers must be >= 1")
        if not self.windows:
            raise ValueError("at least one window is required")
        if not self.symbol_groups:
            raise ValueError("at least one symbol group is required")
        normalized_groups: List[List[str]] = []
        for group in self.symbol_groups:
            if not group:
                raise ValueError("symbol_groups cannot contain empty groups")
            normalized_groups.append(sorted(dict.fromkeys([symbol.strip().upper() for symbol in group if symbol.strip()])))
        if any(not group for group in normalized_groups):
            raise ValueError("symbol_groups must contain non-empty symbols")

    def to_dict(self) -> Dict[str, Any]:
        payload = asdict(self)
        payload["dataset_path"] = str(Path(self.dataset_path))
        return payload

    @classmethod
    def from_dict(cls, raw: Dict[str, Any]) -> "BatchSpec":
        windows = [
            window if isinstance(window, WindowSpec) else WindowSpec(**window)
            for window in raw.get("windows", [])
        ]
        return cls(
            experiment_id=raw["experiment_id"],
            dataset_path=str(raw["dataset_path"]),
            strategy_id=raw["strategy_id"],
            windows=windows,
            symbol_groups=[[str(symbol) for symbol in group] for group in raw.get("symbol_groups", [])],
            base_params=dict(raw.get("base_params", {})),
            parameter_grid=dict(raw.get("parameter_grid", {})),
            timeframe=str(raw.get("timeframe", "1D")),
            batch_label=str(raw.get("batch_label", "")),
            max_workers=int(raw.get("max_workers", 1)),
            notes=[str(note) for note in raw.get("notes", [])],
            schema_version=str(raw.get("schema_version", SCHEMA_VERSION)),
            engine_id=str(raw.get("engine_id", ENGINE_ID)),
        )


@dataclass(frozen=True)
class JobSpec:
    job_id: str
    batch_id: str
    job_index: int
    experiment_id: str
    strategy_id: str
    params: Dict[str, Any]
    symbols: List[str]
    window: WindowSpec
    dataset_fingerprint: DatasetFingerprint
    timeframe: str = "1D"
    status: str = "queued"
    schema_version: str = SCHEMA_VERSION
    engine_id: str = ENGINE_ID

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)

    @classmethod
    def from_dict(cls, raw: Dict[str, Any]) -> "JobSpec":
        return cls(
            job_id=raw["job_id"],
            batch_id=raw["batch_id"],
            job_index=int(raw["job_index"]),
            experiment_id=raw["experiment_id"],
            strategy_id=raw["strategy_id"],
            params=dict(raw.get("params", {})),
            symbols=[str(symbol) for symbol in raw.get("symbols", [])],
            window=raw["window"] if isinstance(raw["window"], WindowSpec) else WindowSpec(**raw["window"]),
            dataset_fingerprint=(
                raw["dataset_fingerprint"]
                if isinstance(raw["dataset_fingerprint"], DatasetFingerprint)
                else DatasetFingerprint(**raw["dataset_fingerprint"])
            ),
            timeframe=str(raw.get("timeframe", "1D")),
            status=str(raw.get("status", "queued")),
            schema_version=str(raw.get("schema_version", SCHEMA_VERSION)),
            engine_id=str(raw.get("engine_id", ENGINE_ID)),
        )


@dataclass(frozen=True)
class JobExecutionResult:
    job_id: str
    batch_id: str
    status: str
    metrics: Dict[str, Any]
    artifact_paths: Dict[str, str]
    failure_reason: Optional[str] = None
    runtime_seconds: Optional[float] = None

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)
