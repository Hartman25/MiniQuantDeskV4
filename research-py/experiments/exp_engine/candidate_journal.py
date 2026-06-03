"""
EXP-ENGINE-CORE-01 — candidate journal writer (scanner-only, Stage 1).

Writes JSONL candidate records to exports/experimental/candidates/.
Schema version: exp-candidate-v1

Hard invariants:
- paper_order_id is always null in Stage 1 (no paper orders placed).
- live_order_id is always null; no live routing exists in any stage.
- No imports from broker adapters, OMS tables, or execution orchestrator.
"""
from __future__ import annotations

import json
import os
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional


SCHEMA_VERSION = "exp-candidate-v1"


def _utcnow_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def build_candidate_record(
    *,
    engine_id: str,
    lane_id: str,
    strategy_id: str,
    symbol: str,
    asset_class: str,
    candidate_type: str,
    signal_direction: str,
    scanned_at_utc: Optional[str] = None,
    confidence_score: Optional[float] = None,
    risk_notes: str = "",
    rejection_reason: Optional[str] = None,
    would_trade: bool = False,
    extra: Optional[dict[str, Any]] = None,
) -> dict[str, Any]:
    """
    Build a validated candidate journal record.

    paper_order_id and live_order_id are always null — orders are never
    placed in Stage 1. This is enforced here, not via a config flag.
    """
    record: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "engine_id": engine_id,
        "lane_id": lane_id,
        "strategy_id": strategy_id,
        "scanned_at_utc": scanned_at_utc or _utcnow_iso(),
        "symbol": symbol,
        "asset_class": asset_class,
        "candidate_type": candidate_type,
        "signal_direction": signal_direction,
        "confidence_score": confidence_score,
        "risk_notes": risk_notes,
        "rejection_reason": rejection_reason,
        "would_trade": would_trade,
        # Stage 1 hard invariants — not configurable.
        "paper_order_id": None,
        "live_order_id": None,
    }
    if extra:
        record["extra"] = extra
    return record


class CandidateJournalWriter:
    """
    Appends candidate records to a JSONL file under journal_dir.

    One file per session named: <engine_id>_<YYYYMMDD_HHMMSS>.jsonl
    File is created lazily on first write.
    """

    def __init__(self, journal_dir: str, engine_id: str) -> None:
        self._dir = Path(journal_dir)
        self._engine_id = engine_id
        self._path: Optional[Path] = None
        self._fh = None

    def _ensure_open(self) -> None:
        if self._fh is not None:
            return
        self._dir.mkdir(parents=True, exist_ok=True)
        ts = datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S")
        filename = f"{self._engine_id}_{ts}.jsonl"
        self._path = self._dir / filename
        self._fh = open(self._path, "a", encoding="utf-8")

    def append(self, record: dict[str, Any]) -> None:
        self._ensure_open()
        self._fh.write(json.dumps(record, default=str) + "\n")
        self._fh.flush()

    def close(self) -> None:
        if self._fh is not None:
            self._fh.close()
            self._fh = None

    def __enter__(self) -> "CandidateJournalWriter":
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()
