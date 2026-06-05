"""
SCANNER-SELECTOR-01 — MAIN scanner ranked candidate selector and artifact exporter.

Takes accepted scanner-candidate-v1 records, ranks them deterministically,
writes a ranked candidate export (ranked-candidates-v1), and writes a
paper-mode watchlist artifact shell (watchlist-v1).

Hard invariants:
- approved_for_live is ALWAYS False
- approved_for_autonomous_paper is ALWAYS False in this patch
- max_symbols_to_trade is forced to 1 in v1
- max_concurrent_positions is forced to 1 in v1
- eligible_for_live=True candidates are always excluded
- No imports from broker adapters, OMS tables, execution orchestrator, or DB
- No network imports (requests, urllib, http.client, aiohttp, psycopg, sqlalchemy)
- No orders are placed at any stage
- JSON artifacts only; writes to exports/watchlist path only (never to operator config paths)
- EXP penny scanner (exp-candidate-v1) is not affected by this module
"""
from __future__ import annotations

import json
import os
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

SCHEMA_VERSION_RANKED = "ranked-candidates-v1"
SCHEMA_VERSION_WATCHLIST = "watchlist-v1"

_REQUIRED_CANDIDATE_SCHEMA = "scanner-candidate-v1"


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class SelectorConfig:
    max_candidates: int = 10
    max_symbols_to_trade: int = 1
    max_concurrent_positions: int = 1
    mode: str = "paper"
    min_total_score: float = 0.0
    default_notional_limit_usd: float = 1000.0
    default_paper_qty_limit: int = 1
    schema_version_ranked: str = SCHEMA_VERSION_RANKED
    schema_version_watchlist: str = SCHEMA_VERSION_WATCHLIST


# ---------------------------------------------------------------------------
# Output types
# ---------------------------------------------------------------------------

@dataclass
class RankedCandidate:
    rank: int
    symbol: str
    strategy_id: str
    timeframe: str
    total_score: float
    liquidity_score: float
    regime_label: str
    regime_score: float
    risk_score: float
    net_expectancy_after_cost_bps: Optional[float]
    paper_qty_limit: int
    notional_limit_usd: float
    selection_reason: str
    source_candidate_artifact: Optional[str]

    def to_dict(self) -> dict[str, Any]:
        return {
            "rank": self.rank,
            "symbol": self.symbol,
            "strategy_id": self.strategy_id,
            "timeframe": self.timeframe,
            "total_score": self.total_score,
            "liquidity_score": self.liquidity_score,
            "regime_label": self.regime_label,
            "regime_score": self.regime_score,
            "risk_score": self.risk_score,
            "net_expectancy_after_cost_bps": self.net_expectancy_after_cost_bps,
            "paper_qty_limit": self.paper_qty_limit,
            "notional_limit_usd": self.notional_limit_usd,
            "selection_reason": self.selection_reason,
            "source_candidate_artifact": self.source_candidate_artifact,
        }


@dataclass
class RankedCandidateExport:
    schema_version: str
    generated_at_utc: str
    candidate_count: int
    ranked_candidates: list[RankedCandidate] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "generated_at_utc": self.generated_at_utc,
            "candidate_count": self.candidate_count,
            "ranked_candidates": [c.to_dict() for c in self.ranked_candidates],
        }


@dataclass
class WatchlistArtifact:
    schema_version: str
    generated_at_utc: str
    mode: str
    approved_for_autonomous_paper: bool
    approved_for_live: bool
    max_symbols_to_trade: int
    max_concurrent_positions: int
    symbols: list[str]
    strategy_assignments: dict[str, str]
    paper_qty_limits: dict[str, int]
    notional_limits: dict[str, float]
    ranked_candidates: list[dict[str, Any]]
    selection_reason: str

    def to_dict(self) -> dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "generated_at_utc": self.generated_at_utc,
            "mode": self.mode,
            "approved_for_autonomous_paper": self.approved_for_autonomous_paper,
            "approved_for_live": self.approved_for_live,
            "max_symbols_to_trade": self.max_symbols_to_trade,
            "max_concurrent_positions": self.max_concurrent_positions,
            "symbols": self.symbols,
            "strategy_assignments": self.strategy_assignments,
            "paper_qty_limits": self.paper_qty_limits,
            "notional_limits": self.notional_limits,
            "ranked_candidates": self.ranked_candidates,
            "selection_reason": self.selection_reason,
        }


# ---------------------------------------------------------------------------
# Selection logic
# ---------------------------------------------------------------------------

def _utcnow_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _is_eligible(record: dict[str, Any], min_total_score: float) -> bool:
    """Return True iff record meets all selector admission criteria."""
    if record.get("schema_version") != _REQUIRED_CANDIDATE_SCHEMA:
        return False
    if record.get("eligible_for_scan") is not True:
        return False
    if record.get("eligible_for_backtest") is not True:
        return False
    if record.get("eligible_for_live") is not False:
        return False
    if record.get("rejection_reason") is not None:
        return False
    if (record.get("total_score") or 0.0) < min_total_score:
        return False
    return True


def _sort_key(record: dict[str, Any]):
    """Deterministic sort key: total_score desc, liquidity_score desc, regime_score desc, symbol asc."""
    return (
        -(record.get("total_score") or 0.0),
        -(record.get("liquidity_score") or 0.0),
        -(record.get("regime_score") or 0.0),
        record.get("symbol") or "",
    )


def select_ranked_candidates(
    candidates: list[dict[str, Any]],
    config: Optional[SelectorConfig] = None,
) -> list[dict[str, Any]]:
    """
    Filter, deduplicate, rank, and limit candidates.

    Returns a list of raw candidate dicts in ranked order (best first),
    limited to config.max_candidates. Symbol deduplication keeps the
    highest-ranked record per symbol.
    """
    cfg = config or SelectorConfig()

    eligible = [r for r in candidates if _is_eligible(r, cfg.min_total_score)]
    eligible_sorted = sorted(eligible, key=_sort_key)

    seen_symbols: set[str] = set()
    deduped: list[dict[str, Any]] = []
    for record in eligible_sorted:
        sym = record.get("symbol") or ""
        if sym not in seen_symbols:
            seen_symbols.add(sym)
            deduped.append(record)

    return deduped[: cfg.max_candidates]


def build_ranked_candidate_export(
    candidates: list[dict[str, Any]],
    config: Optional[SelectorConfig] = None,
    generated_at_utc: Optional[str] = None,
) -> RankedCandidateExport:
    """Build a RankedCandidateExport from a list of raw candidate dicts."""
    cfg = config or SelectorConfig()
    ranked_raw = select_ranked_candidates(candidates, cfg)

    ranked: list[RankedCandidate] = []
    for i, record in enumerate(ranked_raw, start=1):
        strategy_id = record.get("recommended_strategy") or "intraday_scalper"
        ranked.append(RankedCandidate(
            rank=i,
            symbol=record.get("symbol") or "",
            strategy_id=strategy_id,
            timeframe=record.get("timeframe") or "",
            total_score=record.get("total_score") or 0.0,
            liquidity_score=record.get("liquidity_score") or 0.0,
            regime_label=record.get("regime_label") or "",
            regime_score=record.get("regime_score") or 0.0,
            risk_score=record.get("risk_score") or 0.0,
            net_expectancy_after_cost_bps=None,
            paper_qty_limit=cfg.default_paper_qty_limit,
            notional_limit_usd=cfg.default_notional_limit_usd,
            selection_reason="selector_output_only; requires future promotion gate and operator approval",
            source_candidate_artifact=None,
        ))

    return RankedCandidateExport(
        schema_version=cfg.schema_version_ranked,
        generated_at_utc=generated_at_utc or _utcnow_iso(),
        candidate_count=len(ranked),
        ranked_candidates=ranked,
    )


def build_watchlist_artifact(
    ranked_export: RankedCandidateExport,
    config: Optional[SelectorConfig] = None,
    generated_at_utc: Optional[str] = None,
) -> WatchlistArtifact:
    """
    Build a WatchlistArtifact shell from a RankedCandidateExport.

    Hard invariants enforced here (not via config):
    - approved_for_live = False
    - approved_for_autonomous_paper = False
    - max_symbols_to_trade = 1 (v1 is single-symbol only)
    - max_concurrent_positions = 1
    """
    cfg = config or SelectorConfig()

    top_candidates = ranked_export.ranked_candidates[: 1]

    symbols = [c.symbol for c in top_candidates]
    strategy_assignments = {c.symbol: c.strategy_id for c in top_candidates}
    paper_qty_limits = {c.symbol: c.paper_qty_limit for c in top_candidates}
    notional_limits = {c.symbol: c.notional_limit_usd for c in top_candidates}

    return WatchlistArtifact(
        schema_version=cfg.schema_version_watchlist,
        generated_at_utc=generated_at_utc or _utcnow_iso(),
        mode="paper",
        approved_for_autonomous_paper=False,
        approved_for_live=False,
        max_symbols_to_trade=1,
        max_concurrent_positions=1,
        symbols=symbols,
        strategy_assignments=strategy_assignments,
        paper_qty_limits=paper_qty_limits,
        notional_limits=notional_limits,
        ranked_candidates=[c.to_dict() for c in ranked_export.ranked_candidates],
        selection_reason=(
            "selector_output_only; approved_for_autonomous_paper=False until "
            "future promotion gate; approved_for_live=False always"
        ),
    )


# ---------------------------------------------------------------------------
# Writers
# ---------------------------------------------------------------------------

def write_ranked_candidate_export(
    export: RankedCandidateExport,
    path: str,
) -> Path:
    """Write ranked candidate export as JSON. Parent dirs created automatically."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(export.to_dict(), indent=2, default=str), encoding="utf-8")
    return out


def write_watchlist_artifact(
    watchlist: WatchlistArtifact,
    path: str,
) -> Path:
    """Write watchlist artifact as JSON. Parent dirs created automatically."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(watchlist.to_dict(), indent=2, default=str), encoding="utf-8")
    return out
