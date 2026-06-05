"""
BACKTEST-RUNNER-01 — MAIN scanner backtest runner.

Reads a backtest-queue-v1 artifact and writes strategy-fit-v1 artifacts.

Current mode: DRY-RUN BLOCKED.
The mqk-backtest CLI exists in core-rs/crates/mqk-backtest but requires a
compiled binary and outputs key=value metrics that do not include strategy-fit
fields (profit_factor, win_rate, expectancy_bps, etc.).  Until a proven
read-only invocation path exists, every queue entry produces a blocked artifact:
  status = "blocked_no_backtest_interface"

Hard invariants:
- recommended_for_live is ALWAYS False
- recommended_for_paper is ALWAYS False in this patch
- Does NOT invoke mqk-backtest or any external subprocess
- Does NOT submit orders or mutate DB
- Does NOT import broker adapters, OMS, execution orchestrator, or DB
- Does NOT import network libraries (requests, urllib, http.client, aiohttp,
  psycopg, sqlalchemy)
- JSON artifact only; deterministic paths based on queue_id
- EXP penny scanner (exp-candidate-v1) is not affected by this module
"""
from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

SCHEMA_VERSION = "strategy-fit-v1"

# Blocked status written when no safe backtest interface is available.
STATUS_BLOCKED = "blocked_no_backtest_interface"
FAILURE_REASON_NO_INTERFACE = "backtest_interface_missing"

# Expected schema version for input queue artifacts.
QUEUE_SCHEMA_VERSION = "backtest-queue-v1"


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class BacktestRunnerConfig:
    mode: str = "dry_run"
    strategy_fit_dir: str = "exports/strategy_fit"
    # Hard invariants — cannot be overridden by caller.
    recommended_for_live: bool = False
    recommended_for_paper: bool = False
    minimum_bars: int = 200
    minimum_trades: int = 30

    def __post_init__(self) -> None:
        # Enforce hard invariants regardless of what caller passes.
        object.__setattr__(self, "recommended_for_live", False)
        object.__setattr__(self, "recommended_for_paper", False)


# ---------------------------------------------------------------------------
# Result types
# ---------------------------------------------------------------------------

@dataclass
class StrategyFitResult:
    """Metrics from a backtest run, or None if not yet executed."""
    bars_used: Optional[int] = None
    trades: Optional[int] = None
    win_rate: Optional[float] = None
    profit_factor: Optional[float] = None
    expectancy_bps: Optional[float] = None
    avg_trade_bps: Optional[float] = None
    max_drawdown_bps: Optional[float] = None
    sharpe: Optional[float] = None
    sortino: Optional[float] = None
    exposure_time_pct: Optional[float] = None
    turnover: Optional[float] = None
    round_trip_cost_bps: Optional[float] = None
    net_expectancy_after_cost_bps: Optional[float] = None
    sample_quality: Optional[float] = None
    parameter_stability_score: Optional[float] = None


@dataclass
class BacktestRunResult:
    """Summary of a backtest runner pass over a queue."""
    queue_artifact_path: Optional[str]
    queue_id_processed: int
    artifacts_written: int
    status: str
    notes: str


# ---------------------------------------------------------------------------
# Pure helpers
# ---------------------------------------------------------------------------

def _utcnow_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _artifact_filename(queue_id: str) -> str:
    """Deterministic filename based on queue_id."""
    digest = hashlib.sha256(queue_id.encode("utf-8")).hexdigest()[:16]
    return f"strategy_fit_{queue_id[:20]}_{digest}.json"


# ---------------------------------------------------------------------------
# Queue loader
# ---------------------------------------------------------------------------

def load_backtest_queue(path: str) -> dict[str, Any]:
    """
    Load and validate a backtest-queue-v1 artifact from disk.

    Raises ValueError if the file is missing, unreadable, or has the wrong
    schema_version.
    """
    p = Path(path)
    if not p.exists():
        raise ValueError(f"backtest queue file not found: {path}")
    try:
        raw = json.loads(p.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError) as exc:
        raise ValueError(f"failed to read backtest queue: {exc}") from exc
    sv = raw.get("schema_version")
    if sv != QUEUE_SCHEMA_VERSION:
        raise ValueError(
            f"unexpected schema_version: {sv!r} (expected {QUEUE_SCHEMA_VERSION!r})"
        )
    return raw


# ---------------------------------------------------------------------------
# Artifact builder
# ---------------------------------------------------------------------------

def build_strategy_fit_artifact(
    entry: dict[str, Any],
    result: StrategyFitResult,
    config: BacktestRunnerConfig,
    generated_at_utc: Optional[str] = None,
    source_queue_artifact: Optional[str] = None,
) -> dict[str, Any]:
    """
    Build a strategy-fit-v1 artifact dict from a queue entry and run result.

    recommended_for_live is always False.
    recommended_for_paper is always False in this patch.
    """
    ts = generated_at_utc or _utcnow_iso()

    # Blocked pass — no metrics available.
    is_blocked = config.mode == "dry_run" or result.trades is None

    failure_reasons: list[str] = []
    status: str
    if is_blocked:
        status = STATUS_BLOCKED
        failure_reasons.append(FAILURE_REASON_NO_INTERFACE)
    else:
        # Future path when a real interface is wired.
        status = "complete"

    return {
        "schema_version": SCHEMA_VERSION,
        "generated_at_utc": ts,
        "source_queue_artifact": source_queue_artifact,
        "source_queue_id": entry.get("queue_id"),
        "symbol": entry.get("symbol"),
        "strategy_id": entry.get("strategy_id"),
        "timeframe": entry.get("timeframe"),
        "regime_label": entry.get("regime_label"),
        "training_window": None,
        "validation_window": None,
        "bars_used": result.bars_used,
        "trades": result.trades,
        "win_rate": result.win_rate,
        "profit_factor": result.profit_factor,
        "expectancy_bps": result.expectancy_bps,
        "avg_trade_bps": result.avg_trade_bps,
        "max_drawdown_bps": result.max_drawdown_bps,
        "sharpe": result.sharpe,
        "sortino": result.sortino,
        "exposure_time_pct": result.exposure_time_pct,
        "turnover": result.turnover,
        "round_trip_cost_bps": result.round_trip_cost_bps,
        "net_expectancy_after_cost_bps": result.net_expectancy_after_cost_bps,
        "sample_quality": result.sample_quality,
        "parameter_stability_score": result.parameter_stability_score,
        "passed_min_bars": False,
        "passed_min_trades": False,
        "passed_max_drawdown": False,
        "passed_profit_factor": False,
        "passed_expectancy": False,
        "passed_cost_adjusted_edge": False,
        "passed_out_of_sample_check": False,
        "recommended_for_paper": False,   # hard invariant
        "recommended_for_live": False,    # hard invariant
        "failure_reasons": failure_reasons,
        "status": status,
        "notes": (
            "strategy-fit-v1; dry-run blocked; backtest not yet executed; "
            "recommended_for_live=False always; BACKTEST-GATES-01 will evaluate metrics"
        ),
    }


# ---------------------------------------------------------------------------
# Artifact writer
# ---------------------------------------------------------------------------

def write_strategy_fit_artifact(artifact: dict[str, Any], path: str) -> Path:
    """Write strategy-fit-v1 artifact as JSON. Parent dirs created automatically."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(artifact, indent=2, default=str), encoding="utf-8")
    return out


# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

def run_backtest_queue(
    queue: dict[str, Any],
    config: Optional[BacktestRunnerConfig] = None,
    output_dir: Optional[str] = None,
    generated_at_utc: Optional[str] = None,
    source_queue_artifact_path: Optional[str] = None,
) -> BacktestRunResult:
    """
    Process every entry in a backtest-queue-v1 dict and write strategy-fit-v1
    artifacts.

    Current mode: dry_run — every entry is written as blocked.
    output_dir defaults to config.strategy_fit_dir.
    """
    cfg = config or BacktestRunnerConfig()
    # Enforce hard invariants even if caller mutated after construction.
    cfg.recommended_for_live = False
    cfg.recommended_for_paper = False

    ts = generated_at_utc or _utcnow_iso()
    out_root = Path(output_dir or cfg.strategy_fit_dir)

    entries: list[dict[str, Any]] = queue.get("entries", [])
    written = 0

    for entry in entries:
        result = StrategyFitResult()  # all None — blocked
        artifact = build_strategy_fit_artifact(
            entry=entry,
            result=result,
            config=cfg,
            generated_at_utc=ts,
            source_queue_artifact=source_queue_artifact_path,
        )
        queue_id = str(entry.get("queue_id") or "unknown")
        filename = _artifact_filename(queue_id)
        dest = out_root / filename
        write_strategy_fit_artifact(artifact, str(dest))
        written += 1

    return BacktestRunResult(
        queue_artifact_path=source_queue_artifact_path,
        queue_id_processed=len(entries),
        artifacts_written=written,
        status="dry_run_complete" if written > 0 or len(entries) == 0 else "error",
        notes=(
            f"dry-run blocked runner; {written} strategy-fit-v1 artifacts written; "
            "all status=blocked_no_backtest_interface; "
            "recommended_for_live=False always"
        ),
    )
