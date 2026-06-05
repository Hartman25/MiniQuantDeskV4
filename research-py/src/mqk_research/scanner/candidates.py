"""
SCANNER-CANDIDATE-01 — MAIN scanner candidate artifact writer.

Schema version: scanner-candidate-v1
Artifacts: JSONL under exports/scanner/candidates/YYYYMMDD/

Hard invariants:
- schema_version is always "scanner-candidate-v1"
- asset_class is always "us_equity" for MAIN v1
- eligible_for_live is always False — enforced here, not via config
- No imports from broker adapters, OMS tables, execution orchestrator, or DB
- No network imports (requests, urllib, http.client, aiohttp, psycopg, sqlalchemy)
- No orders are placed at any stage
- Rejected candidates write rejection_reason; accepted candidates set it to None
"""
from __future__ import annotations

import json
import os
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

SCHEMA_VERSION = "scanner-candidate-v1"
ASSET_CLASS = "us_equity"

_REQUIRED_FIELDS = [
    "scanner_id",
    "symbol",
    "exchange",
    "timeframe",
    "source",
    "latest_bar_ts",
    "latest_close",
    "lookback_close",
    "move_bps",
    "abs_move_bps",
    "volume",
    "avg_volume",
    "relative_volume",
    "atr",
    "gap_pct",
    "spread_estimate_bps",
    "slippage_estimate_bps",
    "round_trip_cost_bps",
    "data_quality_score",
    "liquidity_score",
    "trend_score",
    "momentum_score",
    "mean_reversion_score",
    "volatility_score",
    "regime_label",
    "regime_score",
    "risk_score",
    "total_score",
    "reason_tags",
    "risk_tags",
    "eligible_for_scan",
    "eligible_for_backtest",
    "eligible_for_paper",
]

_LIST_FIELDS = ["reason_tags", "risk_tags"]
_BOOL_FIELDS = ["eligible_for_scan", "eligible_for_backtest", "eligible_for_paper", "eligible_for_live"]


class ScannerCandidateError(ValueError):
    """Raised when a candidate record fails validation."""


def _utcnow_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def validate_scanner_candidate(record: dict[str, Any]) -> None:
    """
    Validate a MAIN scanner candidate record.
    Raises ScannerCandidateError on any violation.
    """
    if record.get("schema_version") != SCHEMA_VERSION:
        raise ScannerCandidateError(
            f"schema_version must be '{SCHEMA_VERSION}', got {record.get('schema_version')!r}"
        )
    if record.get("asset_class") != ASSET_CLASS:
        raise ScannerCandidateError(
            f"asset_class must be '{ASSET_CLASS}', got {record.get('asset_class')!r}"
        )
    if record.get("eligible_for_live") is not False:
        raise ScannerCandidateError(
            "eligible_for_live must be False; live routing is not permitted in MAIN v1"
        )

    missing = [f for f in _REQUIRED_FIELDS if f not in record]
    if missing:
        raise ScannerCandidateError(f"Missing required fields: {missing}")

    for field in _LIST_FIELDS:
        if not isinstance(record.get(field), list):
            raise ScannerCandidateError(f"'{field}' must be a list, got {type(record.get(field)).__name__}")

    for field in _BOOL_FIELDS:
        val = record.get(field)
        if not isinstance(val, bool):
            raise ScannerCandidateError(f"'{field}' must be bool, got {type(val).__name__}")

    eligible_for_scan = record.get("eligible_for_scan")
    rejection_reason = record.get("rejection_reason")
    if eligible_for_scan is True and rejection_reason is not None:
        raise ScannerCandidateError(
            "accepted candidate (eligible_for_scan=True) must have rejection_reason=None"
        )
    if eligible_for_scan is False and not rejection_reason:
        raise ScannerCandidateError(
            "rejected candidate (eligible_for_scan=False) must have rejection_reason set"
        )


def build_scanner_candidate(
    *,
    scanner_id: str,
    symbol: str,
    exchange: str,
    timeframe: str,
    source: str,
    latest_bar_ts: str,
    latest_close: float,
    lookback_close: float,
    move_bps: float,
    abs_move_bps: float,
    volume: int,
    avg_volume: float,
    relative_volume: float,
    atr: float,
    gap_pct: float,
    spread_estimate_bps: float,
    slippage_estimate_bps: float,
    round_trip_cost_bps: float,
    data_quality_score: float,
    liquidity_score: float,
    trend_score: float,
    momentum_score: float,
    mean_reversion_score: float,
    volatility_score: float,
    regime_label: str,
    regime_score: float,
    risk_score: float,
    total_score: float,
    reason_tags: list[str],
    risk_tags: list[str],
    eligible_for_scan: bool,
    eligible_for_backtest: bool,
    eligible_for_paper: bool,
    rejection_reason: Optional[str] = None,
    recommended_strategy: Optional[str] = None,
    notes: Optional[str] = None,
    generated_at_utc: Optional[str] = None,
) -> dict[str, Any]:
    """
    Build a validated MAIN scanner candidate record.

    eligible_for_live is always False — not a parameter. Callers cannot override it.
    """
    record: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "generated_at_utc": generated_at_utc or _utcnow_iso(),
        "scanner_id": scanner_id,
        "symbol": symbol,
        "asset_class": ASSET_CLASS,
        "exchange": exchange,
        "timeframe": timeframe,
        "source": source,
        "latest_bar_ts": latest_bar_ts,
        "latest_close": latest_close,
        "lookback_close": lookback_close,
        "move_bps": move_bps,
        "abs_move_bps": abs_move_bps,
        "volume": volume,
        "avg_volume": avg_volume,
        "relative_volume": relative_volume,
        "atr": atr,
        "gap_pct": gap_pct,
        "spread_estimate_bps": spread_estimate_bps,
        "slippage_estimate_bps": slippage_estimate_bps,
        "round_trip_cost_bps": round_trip_cost_bps,
        "data_quality_score": data_quality_score,
        "liquidity_score": liquidity_score,
        "trend_score": trend_score,
        "momentum_score": momentum_score,
        "mean_reversion_score": mean_reversion_score,
        "volatility_score": volatility_score,
        "regime_label": regime_label,
        "regime_score": regime_score,
        "risk_score": risk_score,
        "total_score": total_score,
        "reason_tags": reason_tags,
        "risk_tags": risk_tags,
        "rejection_reason": rejection_reason,
        "eligible_for_scan": eligible_for_scan,
        "eligible_for_backtest": eligible_for_backtest,
        "eligible_for_paper": eligible_for_paper,
        # Hard invariant: eligible_for_live is NEVER True in MAIN v1
        "eligible_for_live": False,
        "recommended_strategy": recommended_strategy,
        "notes": notes,
    }
    validate_scanner_candidate(record)
    return record


def candidate_to_json_line(record: dict[str, Any]) -> str:
    return json.dumps(record, default=str)


def _artifact_path(scanner_id: str, date_str: str, suffix: str, base_dir: str) -> Path:
    ts = datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S")
    name = f"{scanner_id}{suffix}_{ts}.jsonl"
    return Path(base_dir) / "scanner" / "candidates" / date_str / name


class ScannerCandidateWriter:
    """
    Writes MAIN scanner candidate records (scanner-candidate-v1) to JSONL.

    accepted → exports/scanner/candidates/YYYYMMDD/<scanner_id>_<ts>.jsonl
    rejected → exports/scanner/candidates/YYYYMMDD/<scanner_id>_rejected_<ts>.jsonl

    File is created lazily on first write. Parent directory is created automatically.
    """

    def __init__(
        self,
        scanner_id: str,
        base_dir: Optional[str] = None,
        date_str: Optional[str] = None,
    ) -> None:
        self._scanner_id = scanner_id
        self._base_dir = base_dir or os.path.join("exports")
        self._date_str = date_str or datetime.now(timezone.utc).strftime("%Y%m%d")
        self._accepted_path: Optional[Path] = None
        self._rejected_path: Optional[Path] = None
        self._accepted_fh = None
        self._rejected_fh = None
        self._accepted_count = 0
        self._rejected_count = 0

    def _ensure_accepted(self) -> None:
        if self._accepted_fh is not None:
            return
        self._accepted_path = _artifact_path(
            self._scanner_id, self._date_str, "", self._base_dir
        )
        self._accepted_path.parent.mkdir(parents=True, exist_ok=True)
        self._accepted_fh = open(self._accepted_path, "a", encoding="utf-8")

    def _ensure_rejected(self) -> None:
        if self._rejected_fh is not None:
            return
        self._rejected_path = _artifact_path(
            self._scanner_id, self._date_str, "_rejected", self._base_dir
        )
        self._rejected_path.parent.mkdir(parents=True, exist_ok=True)
        self._rejected_fh = open(self._rejected_path, "a", encoding="utf-8")

    def write_candidate(self, record: dict[str, Any]) -> Path:
        """Write an accepted candidate. Returns the artifact path."""
        validate_scanner_candidate(record)
        self._ensure_accepted()
        self._accepted_fh.write(candidate_to_json_line(record) + "\n")
        self._accepted_fh.flush()
        self._accepted_count += 1
        return self._accepted_path

    def write_rejection(self, record: dict[str, Any]) -> Path:
        """Write a rejected candidate. Returns the artifact path."""
        validate_scanner_candidate(record)
        self._ensure_rejected()
        self._rejected_fh.write(candidate_to_json_line(record) + "\n")
        self._rejected_fh.flush()
        self._rejected_count += 1
        return self._rejected_path

    def write(self, record: dict[str, Any]) -> Path:
        """Route to write_candidate or write_rejection based on eligible_for_scan."""
        if record.get("eligible_for_scan"):
            return self.write_candidate(record)
        return self.write_rejection(record)

    @property
    def accepted_count(self) -> int:
        return self._accepted_count

    @property
    def rejected_count(self) -> int:
        return self._rejected_count

    def close(self) -> None:
        for fh in (self._accepted_fh, self._rejected_fh):
            if fh is not None:
                fh.close()
        self._accepted_fh = None
        self._rejected_fh = None

    def __enter__(self) -> "ScannerCandidateWriter":
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()


def load_candidates(path: str) -> list[dict[str, Any]]:
    """Load JSONL candidate records from a file (for tests/inspection only)."""
    records = []
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if line:
                records.append(json.loads(line))
    return records
