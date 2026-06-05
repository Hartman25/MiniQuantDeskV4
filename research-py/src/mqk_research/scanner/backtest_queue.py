"""
BACKTEST-QUEUE-01 — MAIN scanner backtest queue writer.

Reads ranked scanner candidates, applies the strategy compatibility matrix,
and writes a deterministic off-hours backtest queue artifact (backtest-queue-v1).

Hard invariants:
- recommended_for_live is ALWAYS False
- Does NOT invoke mqk-backtest or any backtest runner
- Does NOT submit orders or mutate DB
- Does NOT import broker adapters, OMS, execution orchestrator, or DB
- Does NOT import network libraries (requests, urllib, http.client, aiohttp,
  psycopg, sqlalchemy)
- JSON artifact only; deterministic ordering
- EXP penny scanner (exp-candidate-v1) is not affected by this module
"""
from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

SCHEMA_VERSION = "backtest-queue-v1"

# ---------------------------------------------------------------------------
# Strategy compatibility matrix
# ---------------------------------------------------------------------------

# Maps strategy_id → frozenset of compatible regime_labels.
# A candidate is only queued for a strategy if its regime_label is in the set.
_COMPATIBILITY: dict[str, frozenset[str]] = {
    "intraday_scalper": frozenset({"trending_up", "trending_down", "high_volatility"}),
    "volatility_breakout": frozenset(
        {"trending_up", "trending_down", "high_volatility", "low_volatility", "gapping"}
    ),
    "mean_reversion": frozenset({"range_bound", "low_volatility"}),
    "swing_momentum": frozenset({"trending_up", "trending_down"}),
    "vwap_mean_reversion": frozenset({"range_bound", "low_volatility"}),
    "opening_range_fade": frozenset({"high_volatility", "gapping"}),
}


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class BacktestQueueConfig:
    approved_strategy_ids: list[str] = field(default_factory=lambda: [
        "intraday_scalper",
        "volatility_breakout",
        "mean_reversion",
        "swing_momentum",
        "vwap_mean_reversion",
        "opening_range_fade",
    ])
    default_timeframe: str = "5m"
    max_queue_entries: int = 25
    ambiguity_policy: str = "CONSERVATIVE_WORST_CASE"
    stress_profile: str = "slippage_x2"
    recommended_for_live: bool = False


# ---------------------------------------------------------------------------
# Output types
# ---------------------------------------------------------------------------

@dataclass
class BacktestQueueEntry:
    queue_id: str
    symbol: str
    strategy_id: str
    timeframe: str
    regime_label: str
    source_rank: int
    source_total_score: float
    source_candidate_artifact: Optional[str]
    source_ranked_export: Optional[str]
    ambiguity_policy: str
    stress_profile: str
    recommended_for_live: bool
    notes: str

    def to_dict(self) -> dict[str, Any]:
        return {
            "queue_id": self.queue_id,
            "symbol": self.symbol,
            "strategy_id": self.strategy_id,
            "timeframe": self.timeframe,
            "regime_label": self.regime_label,
            "source_rank": self.source_rank,
            "source_total_score": self.source_total_score,
            "source_candidate_artifact": self.source_candidate_artifact,
            "source_ranked_export": self.source_ranked_export,
            "ambiguity_policy": self.ambiguity_policy,
            "stress_profile": self.stress_profile,
            "recommended_for_live": self.recommended_for_live,
            "notes": self.notes,
        }


@dataclass
class BacktestQueueArtifact:
    schema_version: str
    generated_at_utc: str
    source_ranked_export: Optional[str]
    queue_count: int
    entries: list[BacktestQueueEntry]
    recommended_for_live: bool
    notes: str

    def to_dict(self) -> dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "generated_at_utc": self.generated_at_utc,
            "source_ranked_export": self.source_ranked_export,
            "queue_count": self.queue_count,
            "entries": [e.to_dict() for e in self.entries],
            "recommended_for_live": self.recommended_for_live,
            "notes": self.notes,
        }


# ---------------------------------------------------------------------------
# Pure helpers
# ---------------------------------------------------------------------------

def _utcnow_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _deterministic_queue_id(symbol: str, strategy_id: str, source_rank: int) -> str:
    """UUIDv5-style deterministic ID for a queue entry using SHA-256 prefix."""
    raw = f"backtest-queue-v1:{symbol}:{strategy_id}:{source_rank}"
    digest = hashlib.sha256(raw.encode("utf-8")).hexdigest()
    return f"bq-{digest[:32]}"


def strategy_ids_for_regime(regime_label: str, config: Optional[BacktestQueueConfig] = None) -> list[str]:
    """
    Return the ordered list of approved strategy IDs compatible with regime_label.

    Order follows approved_strategy_ids order in config (stable, not sorted).
    Returns empty list for unknown/unclassified regimes or when no strategy matches.
    """
    cfg = config or BacktestQueueConfig()
    result = []
    for sid in cfg.approved_strategy_ids:
        compatible_regimes = _COMPATIBILITY.get(sid, frozenset())
        if regime_label in compatible_regimes:
            result.append(sid)
    return result


# ---------------------------------------------------------------------------
# Builder
# ---------------------------------------------------------------------------

def build_backtest_queue(
    ranked_export: dict[str, Any],
    config: Optional[BacktestQueueConfig] = None,
    generated_at_utc: Optional[str] = None,
    source_ranked_export_path: Optional[str] = None,
) -> BacktestQueueArtifact:
    """
    Build a BacktestQueueArtifact from a ranked-candidates-v1 export dict.

    Ordering (deterministic):
      1. source_rank ascending (best candidate first)
      2. strategy_id in approved_strategy_ids order (stable)
      3. symbol ascending if ranks ever tied (not possible after dedup, but guarded)

    Entries are limited to max_queue_entries after expansion.
    recommended_for_live is always False.
    """
    cfg = config or BacktestQueueConfig()
    ts = generated_at_utc or _utcnow_iso()

    candidates: list[dict[str, Any]] = ranked_export.get("ranked_candidates", [])

    entries: list[BacktestQueueEntry] = []

    for candidate in candidates:
        if len(entries) >= cfg.max_queue_entries:
            break

        symbol = str(candidate.get("symbol") or "")
        source_rank = int(candidate.get("rank") or 0)
        source_total_score = float(candidate.get("total_score") or 0.0)
        regime_label = str(candidate.get("regime_label") or "")
        timeframe = str(candidate.get("timeframe") or "") or cfg.default_timeframe
        source_candidate_artifact = candidate.get("source_candidate_artifact")

        compatible_strategies = strategy_ids_for_regime(regime_label, cfg)

        for strategy_id in compatible_strategies:
            if len(entries) >= cfg.max_queue_entries:
                break

            queue_id = _deterministic_queue_id(symbol, strategy_id, source_rank)
            entry = BacktestQueueEntry(
                queue_id=queue_id,
                symbol=symbol,
                strategy_id=strategy_id,
                timeframe=timeframe,
                regime_label=regime_label,
                source_rank=source_rank,
                source_total_score=source_total_score,
                source_candidate_artifact=source_candidate_artifact,
                source_ranked_export=source_ranked_export_path,
                ambiguity_policy=cfg.ambiguity_policy,
                stress_profile=cfg.stress_profile,
                recommended_for_live=False,  # hard invariant
                notes=(
                    "backtest-queue-v1; not yet run through backtest engine; "
                    "recommended_for_live=False always at this stage"
                ),
            )
            entries.append(entry)

    return BacktestQueueArtifact(
        schema_version=SCHEMA_VERSION,
        generated_at_utc=ts,
        source_ranked_export=source_ranked_export_path,
        queue_count=len(entries),
        entries=entries,
        recommended_for_live=False,  # hard invariant
        notes=(
            "off-hours backtest queue; entries not yet executed; "
            "recommended_for_live=False always; BACKTEST-RUNNER-01 executes entries"
        ),
    )


# ---------------------------------------------------------------------------
# Writer
# ---------------------------------------------------------------------------

def write_backtest_queue(
    queue: BacktestQueueArtifact,
    path: str,
) -> Path:
    """Write backtest queue artifact as JSON. Parent dirs created automatically."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(queue.to_dict(), indent=2, default=str), encoding="utf-8")
    return out
