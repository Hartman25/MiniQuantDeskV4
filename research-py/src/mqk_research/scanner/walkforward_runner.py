"""
WALKFORWARD-RUNNER-01 — Walk-forward backtest runner with split CSV execution.

Executes or plans walk-forward train/validation split backtests by filtering
the bars CSV to per-split validation windows and optionally invoking the Rust CLI
on each window.

Fail-closed rules:
- mode="dry_run" (default): plan and split CSVs written; no subprocess invoked.
- mode="real": subprocess invoked per validation split (deferred import only).
- Missing bars CSV fails closed.
- No splits generated fails closed.
- Empty validation split CSV fails closed.
- Missing metrics from any split fails closed.
- recommended_for_live is ALWAYS False.

Conservative aggregation:
- validation_profit_factor = minimum profit_factor across splits.
- validation_trades = sum trade_count across splits.
- sample_quality = clamp(validation_trades / (min_validation_trades * splits), 0, 1).
- parameter_stability_score = passed_splits / total_splits.

Hard invariants:
- No broker, OMS, daemon, or production DB access.
- No network imports (requests, urllib, aiohttp, http.client).
- subprocess deferred inside _run_single_split() — never at module level.
- EXP penny scanner (exp-candidate-v1) is not affected by this module.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from datetime import date, datetime, timezone, timedelta
from pathlib import Path
from typing import Any, Optional

from mqk_research.scanner.walkforward import (
    WalkForwardConfig,
    WalkForwardSplit,
    build_date_splits,
)

# ---------------------------------------------------------------------------
# Failure reason constants
# ---------------------------------------------------------------------------

REASON_BARS_FILE_MISSING = "bars_file_missing"
REASON_NO_SPLITS = "no_splits_generated"
REASON_EMPTY_SPLIT_CSV = "empty_split_csv"
REASON_SPLIT_CSV_WRITE_FAILED = "split_csv_write_failed"
REASON_METRICS_MISSING = "split_metrics_missing"
REASON_BACKTEST_FAILED = "split_backtest_failed"
REASON_INVALID_BARS_CSV = "invalid_bars_csv"
REASON_MISSING_DATE_RANGE = "missing_date_range_in_entry"
REASON_COMMAND_BUILD_FAILED = "command_build_failed"
REASON_BINARY_NOT_FOUND = "binary_not_found"

# Appended to a strategy-fit artifact's failure_reasons when walk-forward
# validation was explicitly enabled but did not produce a usable aggregate
# (no splits, blocked splits, missing bars CSV, incomplete aggregate, etc).
REASON_WALKFORWARD_VALIDATION_BLOCKED = "walkforward_validation_blocked"


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class WalkForwardRunnerConfig:
    """
    Configuration for the walk-forward runner.

    mode="dry_run" (default): plan and split CSVs written; no subprocess invoked.
    mode="real": invokes Rust backtest CLI per validation split.
    train_days / validation_days / step_days: split window sizes.
    output_root: root dir for split CSVs and result artifacts.
    min_validation_trades: minimum trades for a passed split.
    min_validation_profit_factor: minimum PF for a passed split.
    metrics_pct_scale: "percent_0_100" (default, Rust-compatible).
    keep_split_files: retain split CSVs after run.
    recommended_for_live: hard invariant — always False.
    """
    mode: str = "dry_run"
    train_days: int = 70
    validation_days: int = 30
    step_days: int = 30
    output_root: str = "exports/backtests/walkforward"
    min_validation_trades: int = 10
    min_validation_profit_factor: float = 1.1
    metrics_pct_scale: str = "percent_0_100"
    keep_split_files: bool = True
    recommended_for_live: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(self, "recommended_for_live", False)


# ---------------------------------------------------------------------------
# Result types
# ---------------------------------------------------------------------------

@dataclass
class WalkForwardPlan:
    """Describes a set of splits that will or would be executed."""
    queue_id: str
    symbol: str
    strategy_id: str
    run_dir: str
    splits: list[WalkForwardSplit]
    failure_reasons: list[str]
    is_valid: bool


@dataclass
class WalkForwardSplitResult:
    """Result for a single validation split."""
    fold_index: int
    split: WalkForwardSplit
    split_csv_path: Optional[str]
    status: str   # "blocked" | "complete" | "failed" | "empty_split"
    metrics: Optional[dict[str, Any]]
    failure_reasons: list[str] = field(default_factory=list)


@dataclass
class WalkForwardAggregate:
    """Conservative aggregate across all validation splits."""
    split_count: int
    passed_split_count: int
    failed_split_count: int
    validation_profit_factor: Optional[float]    # min across splits (conservative)
    worst_split_profit_factor: Optional[float]   # same as validation_profit_factor
    median_split_profit_factor: Optional[float]
    validation_trades: int                        # sum across splits
    validation_win_rate: Optional[float]          # weighted avg (0-1 fraction)
    sample_quality: float                         # clamp(trades / min_required, 0, 1)
    parameter_stability_score: float              # passed / total splits
    all_splits_have_metrics: bool
    recommended_for_live: bool = False            # always False


@dataclass
class WalkForwardRunResult:
    """Top-level result for a run_walkforward_entry call."""
    queue_id: str
    symbol: str
    strategy_id: str
    mode: str
    split_results: list[WalkForwardSplitResult]
    aggregate: Optional[WalkForwardAggregate]
    validation_metrics: Optional[dict[str, Any]]
    failure_reasons: list[str]
    status: str   # "dry_run_planned" | "complete" | "failed"
    recommended_for_live: bool = False   # always False


# ---------------------------------------------------------------------------
# Pure helpers
# ---------------------------------------------------------------------------

def _date_to_epoch_secs(d: date) -> int:
    """Convert a calendar date to midnight UTC epoch seconds (inclusive start)."""
    return int(datetime(d.year, d.month, d.day, tzinfo=timezone.utc).timestamp())


def _run_dir_for_entry(entry: dict[str, Any], output_root: str) -> str:
    """Deterministic output directory path for a queue entry."""
    queue_id = str(entry.get("queue_id") or "unknown")
    safe_id = queue_id[:40].replace("/", "_").replace("\\", "_")
    return str(Path(output_root) / safe_id)


def _split_csv_path(run_dir: str, fold_index: int, window: str = "validation") -> str:
    """Deterministic path for a split CSV file."""
    return str(Path(run_dir) / f"split_{fold_index:03d}_{window}.csv")


# ---------------------------------------------------------------------------
# CSV split writer
# ---------------------------------------------------------------------------

def write_split_csv(
    input_csv: str,
    split: WalkForwardSplit,
    output_csv: str,
    window: str = "validation",
) -> int:
    """
    Write a filtered CSV containing only rows within the specified split window.

    window="validation" (default): filters to [validation_start, validation_end].
    window="train": filters to [train_start, train_end].

    Filters on the `end_ts` column (Unix epoch seconds, present in all mqk bar CSVs).
    Header is always written. Rows with unparsable end_ts are silently skipped.

    Returns the number of data rows written (excluding header).
    Raises ValueError if the input CSV is missing or lacks an end_ts column.
    """
    p = Path(input_csv)
    if not p.exists():
        raise ValueError(f"{REASON_BARS_FILE_MISSING}: {input_csv}")

    if window == "validation":
        win_start = split.validation_start
        win_end = split.validation_end
    elif window == "train":
        win_start = split.train_start
        win_end = split.train_end
    else:
        raise ValueError(f"invalid window={window!r}; expected 'validation' or 'train'")

    epoch_start = _date_to_epoch_secs(win_start)
    epoch_end_exclusive = _date_to_epoch_secs(win_end + timedelta(days=1))

    try:
        content = p.read_text(encoding="utf-8")
    except OSError as exc:
        raise ValueError(f"{REASON_SPLIT_CSV_WRITE_FAILED}: {exc}") from exc

    lines = content.splitlines()
    if not lines:
        raise ValueError(f"{REASON_INVALID_BARS_CSV}: empty file: {input_csv}")

    header_line = lines[0]
    header_cols = [c.strip().strip('"') for c in header_line.split(",")]

    try:
        end_ts_idx = header_cols.index("end_ts")
    except ValueError as exc:
        raise ValueError(
            f"{REASON_INVALID_BARS_CSV}: 'end_ts' column not found in {input_csv}"
        ) from exc

    out_path = Path(output_csv)
    out_path.parent.mkdir(parents=True, exist_ok=True)

    rows_written = 0
    out_lines = [header_line]

    for raw_line in lines[1:]:
        stripped = raw_line.strip()
        if not stripped:
            continue
        fields = stripped.split(",")
        if end_ts_idx >= len(fields):
            continue
        try:
            ts = int(fields[end_ts_idx].strip().strip('"'))
        except (ValueError, TypeError):
            continue
        if epoch_start <= ts < epoch_end_exclusive:
            out_lines.append(raw_line.rstrip("\r\n"))
            rows_written += 1

    out_path.write_text("\n".join(out_lines) + "\n", encoding="utf-8")
    return rows_written


# ---------------------------------------------------------------------------
# CSV date range deriver
# ---------------------------------------------------------------------------

def _derive_date_range_from_csv(bars_csv: str) -> tuple[date, date]:
    """
    Scan bars CSV for min/max end_ts and return as calendar dates.

    Raises ValueError on invalid or empty CSV.
    """
    p = Path(bars_csv)
    try:
        content = p.read_text(encoding="utf-8")
    except OSError as exc:
        raise ValueError(f"{REASON_INVALID_BARS_CSV}: {exc}") from exc

    lines = content.splitlines()
    if len(lines) < 2:
        raise ValueError(f"{REASON_INVALID_BARS_CSV}: no data rows in {bars_csv}")

    header_cols = [c.strip().strip('"') for c in lines[0].split(",")]
    try:
        end_ts_idx = header_cols.index("end_ts")
    except ValueError as exc:
        raise ValueError(f"{REASON_INVALID_BARS_CSV}: 'end_ts' column not found") from exc

    ts_values: list[int] = []
    for raw in lines[1:]:
        raw = raw.strip()
        if not raw:
            continue
        fields = raw.split(",")
        if end_ts_idx >= len(fields):
            continue
        try:
            ts_values.append(int(fields[end_ts_idx].strip().strip('"')))
        except (ValueError, TypeError):
            continue

    if not ts_values:
        raise ValueError(f"{REASON_INVALID_BARS_CSV}: no valid end_ts rows in {bars_csv}")

    min_ts = min(ts_values)
    max_ts = max(ts_values)
    start_date = datetime.fromtimestamp(min_ts, tz=timezone.utc).date()
    end_date = datetime.fromtimestamp(max_ts, tz=timezone.utc).date()
    return start_date, end_date


# ---------------------------------------------------------------------------
# Plan builder
# ---------------------------------------------------------------------------

def build_walkforward_plan(
    queue_entry: dict[str, Any],
    bars_csv: str,
    config: Optional[WalkForwardRunnerConfig] = None,
) -> WalkForwardPlan:
    """
    Build a walk-forward plan for a queue entry.

    Validates bars CSV exists, derives date range (from entry or CSV scan), and
    generates splits. Does NOT execute anything — safe in dry-run mode.

    Returns WalkForwardPlan; is_valid=False with failure_reasons on error.
    """
    cfg = config or WalkForwardRunnerConfig()
    queue_id = str(queue_entry.get("queue_id") or "unknown")
    symbol = str(queue_entry.get("symbol") or "")
    strategy_id = str(queue_entry.get("strategy_id") or "")
    run_dir = _run_dir_for_entry(queue_entry, cfg.output_root)
    failure_reasons: list[str] = []

    if not Path(bars_csv).exists():
        failure_reasons.append(REASON_BARS_FILE_MISSING)
        return WalkForwardPlan(
            queue_id=queue_id,
            symbol=symbol,
            strategy_id=strategy_id,
            run_dir=run_dir,
            splits=[],
            failure_reasons=failure_reasons,
            is_valid=False,
        )

    start_date_str = queue_entry.get("start_date")
    end_date_str = queue_entry.get("end_date")

    if start_date_str and end_date_str:
        try:
            start_date = date.fromisoformat(str(start_date_str))
            end_date = date.fromisoformat(str(end_date_str))
        except (ValueError, TypeError):
            failure_reasons.append(REASON_MISSING_DATE_RANGE)
            return WalkForwardPlan(
                queue_id=queue_id,
                symbol=symbol,
                strategy_id=strategy_id,
                run_dir=run_dir,
                splits=[],
                failure_reasons=failure_reasons,
                is_valid=False,
            )
    else:
        try:
            start_date, end_date = _derive_date_range_from_csv(bars_csv)
        except ValueError as exc:
            reason = str(exc).split(":")[0]
            failure_reasons.append(reason)
            return WalkForwardPlan(
                queue_id=queue_id,
                symbol=symbol,
                strategy_id=strategy_id,
                run_dir=run_dir,
                splits=[],
                failure_reasons=failure_reasons,
                is_valid=False,
            )

    wf_cfg = WalkForwardConfig(
        train_days=cfg.train_days,
        validation_days=cfg.validation_days,
        step_days=cfg.step_days,
    )
    splits = build_date_splits(start_date, end_date, wf_cfg)

    if not splits:
        failure_reasons.append(REASON_NO_SPLITS)
        return WalkForwardPlan(
            queue_id=queue_id,
            symbol=symbol,
            strategy_id=strategy_id,
            run_dir=run_dir,
            splits=[],
            failure_reasons=failure_reasons,
            is_valid=False,
        )

    return WalkForwardPlan(
        queue_id=queue_id,
        symbol=symbol,
        strategy_id=strategy_id,
        run_dir=run_dir,
        splits=splits,
        failure_reasons=[],
        is_valid=True,
    )


# ---------------------------------------------------------------------------
# Real-mode single split runner
# ---------------------------------------------------------------------------

def _run_single_split(
    split_csv: str,
    queue_entry: dict[str, Any],
    fold_index: int,
    bridge_config: Any,
) -> tuple[Optional[dict[str, Any]], list[str]]:
    """
    Invoke the Rust backtest CLI on a single split CSV.

    Returns (metrics_dict_or_None, failure_reasons).
    subprocess is imported INSIDE this function — never at module level.
    Only called when mode="real".
    """
    import subprocess  # deferred — not at module level

    from mqk_research.scanner.backtest_bridge import (  # lazy import
        parse_mqk_metrics_json,
        TIMEFRAME_TO_SECS,
        REASON_BACKTEST_COMMAND_FAILED as _BRK_CMD_FAILED,
        REASON_METRICS_FILE_MISSING as _BRK_METRICS_MISSING,
        REASON_METRICS_SCHEMA_INVALID as _BRK_SCHEMA_INVALID,
    )

    if not bridge_config or not getattr(bridge_config, "cli_binary", None):
        return None, [REASON_COMMAND_BUILD_FAILED]

    binary = bridge_config.cli_binary
    if not Path(binary).exists():
        return None, [REASON_BINARY_NOT_FOUND]

    symbol = str(queue_entry.get("symbol") or "")
    strategy_id = str(queue_entry.get("strategy_id") or "")
    timeframe = str(queue_entry.get("timeframe") or "5m")

    timeframe_secs = TIMEFRAME_TO_SECS.get(timeframe)
    if not timeframe_secs:
        return None, [REASON_COMMAND_BUILD_FAILED]

    out_dir = str(Path(split_csv).parent / f"fold_{fold_index:03d}_artifacts")

    cmd = [
        binary,
        "backtest", "csv",
        "--bars", split_csv,
        "--strategy", strategy_id,
        "--symbol", symbol,
        "--timeframe-secs", str(timeframe_secs),
        "--initial-cash-micros", str(getattr(bridge_config, "initial_cash_micros", 100_000_000_000)),
        "--target-qty", str(getattr(bridge_config, "target_qty", 1)),
        "--integrity-stale-threshold-ticks", str(getattr(bridge_config, "integrity_stale_threshold_ticks", 172_800)),
        "--integrity-gap-tolerance-bars", str(getattr(bridge_config, "integrity_gap_tolerance_bars", 0)),
        "--out-dir", out_dir,
    ]

    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=300)
    except (OSError, subprocess.TimeoutExpired):
        return None, [REASON_BACKTEST_FAILED]

    if proc.returncode != 0:
        return None, [REASON_BACKTEST_FAILED]

    artifacts_dir: Optional[str] = None
    for line in (proc.stdout or "").splitlines():
        if line.startswith("artifacts_dir="):
            artifacts_dir = line.split("=", 1)[1].strip()
            break

    if not artifacts_dir:
        return None, [_BRK_METRICS_MISSING]

    metrics_path = str(Path(artifacts_dir) / "metrics.json")
    try:
        metrics = parse_mqk_metrics_json(metrics_path)
    except ValueError as exc:
        raw_reason = str(exc).split(":")[0]
        if raw_reason not in (_BRK_METRICS_MISSING, _BRK_SCHEMA_INVALID):
            raw_reason = _BRK_SCHEMA_INVALID
        return None, [raw_reason]

    return metrics, []


# ---------------------------------------------------------------------------
# Aggregation
# ---------------------------------------------------------------------------

def aggregate_walkforward_results(
    split_results: list[WalkForwardSplitResult],
    config: Optional[WalkForwardRunnerConfig] = None,
) -> WalkForwardAggregate:
    """
    Aggregate split metrics conservatively across validation splits.

    Conservative rules:
    - validation_profit_factor = minimum profit_factor (not average, not best).
    - validation_trades = sum of trade_count.
    - validation_win_rate = weighted average by trades (percent_0_100 → fraction).
    - sample_quality = clamp(validation_trades / (min_trades * total_splits), 0, 1).
    - parameter_stability_score = passed_splits / total_splits.
    - all_splits_have_metrics=False if any split is missing profit_factor or trade_count.
    - recommended_for_live always False.
    """
    cfg = config or WalkForwardRunnerConfig()
    total_splits = len(split_results)

    if total_splits == 0:
        return WalkForwardAggregate(
            split_count=0,
            passed_split_count=0,
            failed_split_count=0,
            validation_profit_factor=None,
            worst_split_profit_factor=None,
            median_split_profit_factor=None,
            validation_trades=0,
            validation_win_rate=None,
            sample_quality=0.0,
            parameter_stability_score=0.0,
            all_splits_have_metrics=False,
            recommended_for_live=False,
        )

    profit_factors: list[float] = []
    all_trades: list[int] = []
    weighted_wr: list[tuple[float, int]] = []   # (win_rate_pct, trades)
    all_splits_have_metrics = True
    passed_splits = 0
    failed_splits = 0

    for sr in split_results:
        if sr.metrics is None or sr.status != "complete":
            all_splits_have_metrics = False
            failed_splits += 1
            continue

        pf = sr.metrics.get("profit_factor")
        tc = sr.metrics.get("trade_count")
        wr = sr.metrics.get("win_rate_pct")

        if pf is None or tc is None:
            all_splits_have_metrics = False
            failed_splits += 1
            continue

        pf_f = float(pf)
        tc_i = int(tc)
        profit_factors.append(pf_f)
        all_trades.append(tc_i)

        if wr is not None:
            weighted_wr.append((float(wr), tc_i))

        split_passed = (
            pf_f >= cfg.min_validation_profit_factor
            and tc_i >= cfg.min_validation_trades
        )
        if split_passed:
            passed_splits += 1
        else:
            failed_splits += 1

    validation_trades = sum(all_trades)

    validation_profit_factor: Optional[float] = None
    worst_split_pf: Optional[float] = None
    median_split_pf: Optional[float] = None

    if profit_factors:
        validation_profit_factor = min(profit_factors)
        worst_split_pf = min(profit_factors)
        sorted_pfs = sorted(profit_factors)
        n = len(sorted_pfs)
        if n % 2 == 1:
            median_split_pf = sorted_pfs[n // 2]
        else:
            median_split_pf = (sorted_pfs[n // 2 - 1] + sorted_pfs[n // 2]) / 2.0

    validation_win_rate: Optional[float] = None
    if weighted_wr:
        total_wt = sum(t for _, t in weighted_wr)
        if total_wt > 0:
            raw_wr = sum(wr * t for wr, t in weighted_wr) / total_wt
            validation_win_rate = raw_wr / 100.0   # percent_0_100 → fraction

    min_required = cfg.min_validation_trades * max(total_splits, 1)
    sample_quality = min(1.0, max(0.0, validation_trades / max(min_required, 1)))
    parameter_stability_score = (
        passed_splits / total_splits if total_splits > 0 else 0.0
    )

    return WalkForwardAggregate(
        split_count=total_splits,
        passed_split_count=passed_splits,
        failed_split_count=failed_splits,
        validation_profit_factor=validation_profit_factor,
        worst_split_profit_factor=worst_split_pf,
        median_split_profit_factor=median_split_pf,
        validation_trades=validation_trades,
        validation_win_rate=validation_win_rate,
        sample_quality=sample_quality,
        parameter_stability_score=parameter_stability_score,
        all_splits_have_metrics=all_splits_have_metrics,
        recommended_for_live=False,
    )


# ---------------------------------------------------------------------------
# Validation metric mapper
# ---------------------------------------------------------------------------

def map_walkforward_to_validation_metrics(
    aggregate: WalkForwardAggregate,
) -> dict[str, Any]:
    """
    Map a WalkForwardAggregate to strategy-fit validation metric fields.

    Keys match validation fields consumed by map_metrics_to_strategy_fit and
    evaluate_strategy_fit. recommended_for_live is always False.
    """
    return {
        "validation_profit_factor": aggregate.validation_profit_factor,
        "validation_trades": (
            aggregate.validation_trades if aggregate.validation_trades > 0 else None
        ),
        "validation_win_rate": aggregate.validation_win_rate,
        "sample_quality": aggregate.sample_quality,
        "parameter_stability_score": aggregate.parameter_stability_score,
        "split_count": aggregate.split_count,
        "passed_split_count": aggregate.passed_split_count,
        "worst_split_profit_factor": aggregate.worst_split_profit_factor,
        "median_split_profit_factor": aggregate.median_split_profit_factor,
        "all_splits_have_metrics": aggregate.all_splits_have_metrics,
        "recommended_for_live": False,
    }


# Strategy-fit fields that walk-forward aggregates may fill — never overrides
# a value already derived from the Rust metrics.json mapping.
_MERGEABLE_VALIDATION_FIELDS: tuple[str, ...] = (
    "validation_profit_factor",
    "validation_trades",
    "sample_quality",
    "parameter_stability_score",
)


def merge_walkforward_validation_into_mapped(
    mapped: dict[str, Any],
    extra_reasons: list[str],
    wf_result: WalkForwardRunResult,
) -> tuple[dict[str, Any], list[str]]:
    """
    Merge a completed walk-forward run's aggregate validation metrics into a
    mapped strategy-fit field dict (as produced by map_metrics_to_strategy_fit).

    Pure / no I/O — operates only on the already-computed WalkForwardRunResult.

    Merge rules (fail-closed):
    - Only merges when wf_result.status == "complete" AND its validation_metrics
      carries non-None validation_profit_factor AND validation_trades — a
      partial or blocked walk-forward run never contributes fabricated values.
    - Only fills fields that are currently None in `mapped`; real values already
      mapped from metrics.json (should the Rust CLI ever emit them) are never
      overridden.
    - "validation_metrics_missing" is removed from the reasons list only when
      both validation_profit_factor and validation_trades are present in the
      merged result — never unconditionally.
    - "walkforward_validation_blocked" is appended whenever the walk-forward
      run did not yield a usable aggregate — an explicit, honest record that
      validation was enabled but did not succeed.

    Returns (merged_mapped, merged_reasons); does not mutate inputs.
    """
    from mqk_research.scanner.backtest_bridge import REASON_VALIDATION_METRICS_MISSING

    merged = dict(mapped)
    reasons = list(extra_reasons)

    vm = wf_result.validation_metrics
    wf_succeeded = (
        wf_result.status == "complete"
        and vm is not None
        and vm.get("validation_profit_factor") is not None
        and vm.get("validation_trades") is not None
    )

    if wf_succeeded:
        for field_name in _MERGEABLE_VALIDATION_FIELDS:
            if merged.get(field_name) is None and vm.get(field_name) is not None:
                merged[field_name] = vm[field_name]
    elif REASON_WALKFORWARD_VALIDATION_BLOCKED not in reasons:
        reasons.append(REASON_WALKFORWARD_VALIDATION_BLOCKED)

    if (merged.get("validation_profit_factor") is not None
            and merged.get("validation_trades") is not None):
        reasons = [r for r in reasons if r != REASON_VALIDATION_METRICS_MISSING]

    return merged, reasons


# ---------------------------------------------------------------------------
# Entry runner
# ---------------------------------------------------------------------------

def run_walkforward_entry(
    queue_entry: dict[str, Any],
    bars_csv: str,
    config: Optional[WalkForwardRunnerConfig] = None,
    bridge_config: Optional[Any] = None,
) -> WalkForwardRunResult:
    """
    Execute or plan walk-forward validation for a single queue entry.

    mode="dry_run" (default): plan and split CSV files written; no subprocess.
    mode="real" + bridge_config: invokes Rust CLI on each validation split CSV.

    Returns WalkForwardRunResult with split results, aggregate, and validation metrics.
    recommended_for_live is always False.
    """
    cfg = config or WalkForwardRunnerConfig()
    queue_id = str(queue_entry.get("queue_id") or "unknown")
    symbol = str(queue_entry.get("symbol") or "")
    strategy_id = str(queue_entry.get("strategy_id") or "")

    plan = build_walkforward_plan(queue_entry, bars_csv, cfg)

    if not plan.is_valid:
        return WalkForwardRunResult(
            queue_id=queue_id,
            symbol=symbol,
            strategy_id=strategy_id,
            mode=cfg.mode,
            split_results=[],
            aggregate=None,
            validation_metrics=None,
            failure_reasons=list(plan.failure_reasons),
            status="failed",
            recommended_for_live=False,
        )

    real_mode = cfg.mode == "real" and bridge_config is not None
    split_results: list[WalkForwardSplitResult] = []

    for split in plan.splits:
        split_csv = _split_csv_path(plan.run_dir, split.fold_index, "validation")

        try:
            row_count = write_split_csv(bars_csv, split, split_csv, window="validation")
        except ValueError as exc:
            reason = str(exc).split(":")[0]
            split_results.append(WalkForwardSplitResult(
                fold_index=split.fold_index,
                split=split,
                split_csv_path=None,
                status="failed",
                metrics=None,
                failure_reasons=[reason],
            ))
            continue

        if row_count == 0:
            split_results.append(WalkForwardSplitResult(
                fold_index=split.fold_index,
                split=split,
                split_csv_path=split_csv,
                status="empty_split",
                metrics=None,
                failure_reasons=[REASON_EMPTY_SPLIT_CSV],
            ))
            continue

        if not real_mode:
            split_results.append(WalkForwardSplitResult(
                fold_index=split.fold_index,
                split=split,
                split_csv_path=split_csv,
                status="blocked",
                metrics=None,
                failure_reasons=["dry_run_no_subprocess"],
            ))
            continue

        metrics, reasons = _run_single_split(
            split_csv, queue_entry, split.fold_index, bridge_config
        )
        split_results.append(WalkForwardSplitResult(
            fold_index=split.fold_index,
            split=split,
            split_csv_path=split_csv,
            status="complete" if metrics is not None else "failed",
            metrics=metrics,
            failure_reasons=reasons,
        ))

    aggregate = aggregate_walkforward_results(split_results, cfg)
    validation_metrics = map_walkforward_to_validation_metrics(aggregate)

    overall_failure_reasons: list[str] = []
    if real_mode:
        if not aggregate.all_splits_have_metrics:
            overall_failure_reasons.append(REASON_METRICS_MISSING)
        status = "complete" if not overall_failure_reasons else "failed"
    else:
        status = "dry_run_planned"

    return WalkForwardRunResult(
        queue_id=queue_id,
        symbol=symbol,
        strategy_id=strategy_id,
        mode=cfg.mode,
        split_results=split_results,
        aggregate=aggregate,
        validation_metrics=validation_metrics,
        failure_reasons=overall_failure_reasons,
        status=status,
        recommended_for_live=False,
    )
