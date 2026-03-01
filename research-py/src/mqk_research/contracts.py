from __future__ import annotations

from dataclasses import asdict, dataclass, field
from typing import Any, Dict, List, Optional
import json


# Stable interface contract version for Research output artifacts.
# This is NOT the same as the "schema_version" field in manifest.json (kept for backward compatibility).
CONTRACT_VERSION: str = "1"


def _json_dumps_deterministic(obj: Any, *, indent: int = 2) -> str:
    # Deterministic JSON: stable keys + newline handled by caller.
    return json.dumps(obj, indent=indent, sort_keys=True, ensure_ascii=False)


@dataclass(frozen=True)
class ResearchUniverseRecord:
    """
    Stable typed record for universe.csv rows.
    Keep required fields minimal and stable; allow extensions via extras.
    """
    instrument_id: str
    symbol: str
    asset_class: str

    # Common optional fields (present in Phase 1 universe.csv today)
    rank: Optional[int] = None
    included: Optional[bool] = None

    # Forward-compatible extension bag (pure data; caller controls content)
    extras: Dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class ResearchTargetRecord:
    """
    Stable typed record for targets.csv rows.
    """
    instrument_id: str
    symbol: str
    asset_class: str
    side: str
    weight: float

    extras: Dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class ResearchIntent:
    """
    Stable typed schema for intent.json (non-equity placeholder artifact).
    """
    schema_version: str
    contract_version: str
    run_id: str
    asof_utc: str
    policy_name: str
    asset_class: str
    symbols: List[str]
    pipeline: str
    notes: List[str] = field(default_factory=list)

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)

    def to_json(self, *, indent: int = 2) -> str:
        return _json_dumps_deterministic(self.to_dict(), indent=indent)


@dataclass(frozen=True)
class ResearchManifest:
    """
    Stable typed schema for manifest.json.
    Pure data only — no I/O, no DB, no CLI logic.
    """
    schema_version: str
    contract_version: str
    run_id: str
    asof_utc: str
    policy_name: str
    policy_path: str
    policy_sha256: str
    params: Dict[str, Any]
    inputs: Dict[str, Any]
    outputs: Dict[str, Any]
    notes: List[str] = field(default_factory=list)

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)

    def to_json(self, *, indent: int = 2) -> str:
        return _json_dumps_deterministic(self.to_dict(), indent=indent)


def validate_contract_version(version: str) -> None:
    if version != CONTRACT_VERSION:
        raise ValueError(
            f"Research contract version mismatch: expected={CONTRACT_VERSION} got={version}. "
            "Refusing to write artifacts."
        )