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

# Status written when real-mode backtest completes with metrics.
STATUS_COMPLETE = "complete"

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
    # Walk-forward validation integration — disabled by default (fail-closed).
    # When False, real-mode behavior is byte-for-byte identical to before this
    # patch: validation_profit_factor/validation_trades/sample_quality/
    # parameter_stability_score remain whatever metrics.json provided (None
    # today) and validation_metrics_missing is appended as before.
    # When True (and mode == "real"), the runner resolves a bars CSV for the
    # entry and runs walk-forward validation (walkforward_config), merging the
    # resulting aggregate into the strategy-fit fields only when it actually
    # completed with real values. Subprocess invocation remains independently
    # gated by walkforward_config.mode == "real" — enabling this flag alone
    # does not authorize subprocess execution.
    enable_walkforward_validation: bool = False
    walkforward_config: Optional[Any] = None

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
    validation_profit_factor: Optional[float] = None
    validation_trades: Optional[int] = None
    largest_trade_profit_fraction: Optional[float] = None


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
    extra_failure_reasons: Optional[list[str]] = None,
) -> dict[str, Any]:
    """
    Build a strategy-fit-v1 artifact dict from a queue entry and run result.

    recommended_for_live is always False.
    recommended_for_paper is always False until apply_backtest_gates() is called.
    extra_failure_reasons: bridge-specific reasons injected into the artifact.
    """
    ts = generated_at_utc or _utcnow_iso()

    # Blocked when dry-run or no metrics produced.
    is_blocked = config.mode == "dry_run" or result.trades is None

    failure_reasons: list[str] = []
    status: str
    if is_blocked:
        status = STATUS_BLOCKED
        if config.mode == "dry_run":
            # Dry-run always uses the generic blocked reason.
            failure_reasons.append(FAILURE_REASON_NO_INTERFACE)
        # In real mode with no metrics, extra_failure_reasons carries the reason.
    else:
        status = STATUS_COMPLETE

    # Merge extra failure reasons (dedup, preserve order).
    for r in (extra_failure_reasons or []):
        if r not in failure_reasons:
            failure_reasons.append(r)

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
        "validation_profit_factor": result.validation_profit_factor,
        "validation_trades": result.validation_trades,
        "largest_trade_profit_fraction": result.largest_trade_profit_fraction,
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
# Walk-forward validation integration (real mode, explicitly opt-in)
# ---------------------------------------------------------------------------

def _run_and_merge_walkforward_validation(
    entry: dict[str, Any],
    mapped: dict[str, Any],
    extra_reasons: list[str],
    config: BacktestRunnerConfig,
    bridge_config: Any,
) -> tuple[dict[str, Any], list[str]]:
    """
    Resolve the bars CSV for `entry`, run walk-forward validation, and merge
    the resulting aggregate into the mapped strategy-fit fields.

    Only reached when config.enable_walkforward_validation is True and the
    runner is in real mode with metrics already produced. Lazily imports the
    walk-forward runner — never at module level, matching the existing
    bridge/gates lazy-import pattern in run_backtest_queue.

    Subprocess invocation is independently gated by walkforward_config.mode:
    passing the (default) dry-run WalkForwardRunnerConfig here will plan splits
    but never execute the CLI, leaving validation fields None and the artifact
    fail-closed via REASON_WALKFORWARD_VALIDATION_BLOCKED.

    When the bars CSV cannot be resolved, returns the inputs unchanged plus
    REASON_WALKFORWARD_VALIDATION_BLOCKED — fail-closed, never fabricated.
    """
    from mqk_research.scanner.backtest_bridge import resolve_bars_csv_path
    from mqk_research.scanner.walkforward_runner import (
        REASON_WALKFORWARD_VALIDATION_BLOCKED,
        WalkForwardRunnerConfig,
        merge_walkforward_validation_into_mapped,
        run_walkforward_entry,
    )

    bars_root = getattr(bridge_config, "bars_root_dir", None)
    symbol = str(entry.get("symbol") or "")
    timeframe = str(entry.get("timeframe") or "5m")

    bars_csv = (
        resolve_bars_csv_path(bars_root, symbol, timeframe)
        if bars_root and symbol else None
    )
    if bars_csv is None:
        reasons = list(extra_reasons)
        if REASON_WALKFORWARD_VALIDATION_BLOCKED not in reasons:
            reasons.append(REASON_WALKFORWARD_VALIDATION_BLOCKED)
        return mapped, reasons

    wf_cfg = config.walkforward_config or WalkForwardRunnerConfig()
    wf_result = run_walkforward_entry(entry, bars_csv, wf_cfg, bridge_config)
    return merge_walkforward_validation_into_mapped(mapped, extra_reasons, wf_result)


# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

def run_backtest_queue(
    queue: dict[str, Any],
    config: Optional[BacktestRunnerConfig] = None,
    output_dir: Optional[str] = None,
    generated_at_utc: Optional[str] = None,
    source_queue_artifact_path: Optional[str] = None,
    bridge_config: Optional[Any] = None,
) -> BacktestRunResult:
    """
    Process every entry in a backtest-queue-v1 dict and write strategy-fit-v1
    artifacts.

    mode="dry_run" (default): every entry is written as blocked.
    mode="real" + bridge_config: invokes the Rust backtest CLI per entry,
        parses metrics.json, maps to strategy-fit-v1, and applies gates.
        Subprocess is called only inside backtest_bridge.run_backtest_for_entry.

    output_dir defaults to config.strategy_fit_dir.
    bridge_config: BacktestBridgeConfig instance (from backtest_bridge module).
        Ignored when config.mode != "real".
    """
    cfg = config or BacktestRunnerConfig()
    # Enforce hard invariants even if caller mutated after construction.
    cfg.recommended_for_live = False
    cfg.recommended_for_paper = False

    ts = generated_at_utc or _utcnow_iso()
    out_root = Path(output_dir or cfg.strategy_fit_dir)

    entries: list[dict[str, Any]] = queue.get("entries", [])
    written = 0
    real_mode = cfg.mode == "real" and bridge_config is not None

    for entry in entries:
        if real_mode:
            # Real mode: invoke Rust CLI, parse metrics, apply gates.
            from mqk_research.scanner.backtest_bridge import (  # lazy import
                run_backtest_for_entry,
                map_metrics_to_strategy_fit,
            )
            from mqk_research.scanner.backtest_gates import (  # lazy import
                apply_backtest_gates,
            )

            metrics_dict, bridge_reasons = run_backtest_for_entry(entry, bridge_config)

            if metrics_dict is not None:
                mapped, map_reasons = map_metrics_to_strategy_fit(
                    entry, metrics_dict, bridge_config
                )
                extra_reasons = bridge_reasons + map_reasons

                if cfg.enable_walkforward_validation:
                    mapped, extra_reasons = _run_and_merge_walkforward_validation(
                        entry, mapped, extra_reasons, cfg, bridge_config
                    )

                result = StrategyFitResult(**mapped)
            else:
                result = StrategyFitResult()  # all None — command failed
                extra_reasons = bridge_reasons

            artifact = build_strategy_fit_artifact(
                entry=entry,
                result=result,
                config=cfg,
                generated_at_utc=ts,
                source_queue_artifact=source_queue_artifact_path,
                extra_failure_reasons=extra_reasons,
            )

            # Apply gates if we got real metrics (status will be "complete").
            if metrics_dict is not None:
                artifact = apply_backtest_gates(artifact)

        else:
            # Dry-run path (existing): produce a blocked artifact.
            result = StrategyFitResult()  # all None
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

    if real_mode:
        run_status = "real_complete" if written > 0 or len(entries) == 0 else "error"
        validation_note = (
            "walk-forward validation enabled; validation_metrics_missing removed "
            "only when walk-forward aggregate completes with real values"
            if cfg.enable_walkforward_validation
            else "validation_metrics_missing until walk-forward validation is enabled and succeeds"
        )
        notes = (
            f"real-mode runner; {written} strategy-fit-v1 artifacts written; "
            f"gates applied; {validation_note}; "
            "recommended_for_live=False always"
        )
    else:
        run_status = "dry_run_complete" if written > 0 or len(entries) == 0 else "error"
        notes = (
            f"dry-run blocked runner; {written} strategy-fit-v1 artifacts written; "
            "all status=blocked_no_backtest_interface; "
            "recommended_for_live=False always"
        )

    return BacktestRunResult(
        queue_artifact_path=source_queue_artifact_path,
        queue_id_processed=len(entries),
        artifacts_written=written,
        status=run_status,
        notes=notes,
    )
