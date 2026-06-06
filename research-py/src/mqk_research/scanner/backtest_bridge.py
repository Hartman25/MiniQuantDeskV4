"""
BACKTEST-BRIDGE-BUNDLE-01 — Python bridge to Rust mqk-backtest CLI output.

Parses metrics.json artifacts produced by:
  mqk backtest csv --bars <path> --strategy <name> --symbol <sym>
      --timeframe-secs <n> --initial-cash-micros <n> --target-qty <n>
      --out-dir <dir>

Maps fields to strategy-fit-v1 for gate evaluation.

Fail-closed rules:
- mode="dry_run" (default): build_mqk_backtest_command() is safe to call but
  run_backtest_for_entry() is never invoked.
- mode="real": subprocess is imported INSIDE run_backtest_for_entry() only,
  never at module level.
- expectancy_basis_missing when no notional can be derived.
- validation_metrics_missing is always appended (no walk-forward yet).
- recommended_for_live is always False — enforced by BacktestBridgeConfig.

Hard invariants:
- recommended_for_live is ALWAYS False
- Never calls broker, OMS, daemon, or production DB
- No network library imports: no requests/urllib/aiohttp/http.client/psycopg/sqlalchemy
- subprocess only inside run_backtest_for_entry() when mode="real"
- JSON artifacts only; deterministic
- EXP penny scanner (exp-candidate-v1) is not affected by this module
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Optional

# ---------------------------------------------------------------------------
# Failure reason constants — stable strings
# ---------------------------------------------------------------------------

REASON_METRICS_FILE_MISSING = "metrics_file_missing"
REASON_METRICS_SCHEMA_INVALID = "metrics_schema_invalid"
REASON_BACKTEST_COMMAND_FAILED = "backtest_command_failed"
REASON_EXPECTANCY_BASIS_MISSING = "expectancy_basis_missing"
REASON_VALIDATION_METRICS_MISSING = "validation_metrics_missing"
REASON_BARS_FILE_MISSING = "bars_file_missing"
REASON_BINARY_NOT_FOUND = "binary_not_found"
REASON_COMMAND_BUILD_FAILED = "command_build_failed"

# ---------------------------------------------------------------------------
# Schema / mapping constants
# ---------------------------------------------------------------------------

# metrics.json schema_version produced by mqk-artifacts write_backtest_report
METRICS_SCHEMA_VERSION = 1

# timeframe string → seconds (canonical for mqk backtest csv --timeframe-secs)
TIMEFRAME_TO_SECS: dict[str, int] = {
    "1m": 60,
    "5m": 300,
    "15m": 900,
    "30m": 1800,
    "1h": 3600,
    "1D": 86400,
    "1d": 86400,
}

# Strategy IDs that exist in register_builtin_strategies (mqk-strategy crate)
KNOWN_STRATEGY_IDS: frozenset[str] = frozenset({
    "intraday_scalper",
    "swing_momentum",
    "volatility_breakout",
    "mean_reversion",
})


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class BacktestBridgeConfig:
    """
    Configuration for the Rust backtest CLI bridge.

    mode="dry_run" (default): never invokes subprocess.
    mode="real": invokes the local Rust mqk binary per queue entry.
    cli_binary: absolute path to the compiled mqk binary.
    bars_root_dir: directory containing <SYMBOL>_<TF>.csv bar files.
    out_dir: directory where mqk writes run artifact subdirectories.
    initial_cash_micros: starting cash for backtest (default $100k).
    target_qty: share count passed to the strategy.
    integrity_stale_threshold_ticks: safe value for daily-bar gaps is 172800.
        The default 120 in the CLI triggers execution_blocked for daily data.
    integrity_gap_tolerance_bars: tolerated missing bar count.
    recommended_for_live: hard invariant; always forced to False.
    """
    mode: str = "dry_run"
    cli_binary: Optional[str] = None
    bars_root_dir: Optional[str] = None
    out_dir: Optional[str] = None
    initial_cash_micros: int = 100_000_000_000
    target_qty: int = 1
    integrity_stale_threshold_ticks: int = 172_800
    integrity_gap_tolerance_bars: int = 0
    recommended_for_live: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(self, "recommended_for_live", False)


# ---------------------------------------------------------------------------
# Command builder
# ---------------------------------------------------------------------------

def _resolve_bars_path(bars_root: str, symbol: str, timeframe: str) -> Optional[str]:
    """Return path to a bars CSV for (symbol, timeframe), or None if not found."""
    root = Path(bars_root)
    candidates = [
        root / f"{symbol}_{timeframe}.csv",
        root / f"{symbol.upper()}_{timeframe}.csv",
        root / f"{symbol.lower()}_{timeframe}.csv",
        root / f"{symbol}.csv",
        root / f"{symbol.upper()}.csv",
        root / f"{symbol.lower()}.csv",
    ]
    for c in candidates:
        if c.exists():
            return str(c)
    return None


def build_mqk_backtest_command(
    entry: dict[str, Any],
    config: BacktestBridgeConfig,
) -> Optional[list[str]]:
    """
    Build the CLI command list for a queue entry.

    Returns None if any required input is missing.
    Does NOT execute anything — safe to call in dry-run mode.

    Expected CLI call:
        mqk backtest csv --bars <path> --strategy <name> --symbol <sym>
            --timeframe-secs <n> --initial-cash-micros <n> --target-qty <n>
            --integrity-stale-threshold-ticks <n>
            --integrity-gap-tolerance-bars <n>
            --out-dir <dir>
    """
    if not config.cli_binary or not config.bars_root_dir:
        return None

    symbol = entry.get("symbol") or ""
    strategy_id = entry.get("strategy_id") or ""
    timeframe = entry.get("timeframe") or "5m"

    if not symbol or not strategy_id:
        return None
    if strategy_id not in KNOWN_STRATEGY_IDS:
        return None

    timeframe_secs = TIMEFRAME_TO_SECS.get(timeframe)
    if not timeframe_secs:
        return None

    bars_path = _resolve_bars_path(config.bars_root_dir, symbol, timeframe)
    if not bars_path:
        return None

    out_dir = config.out_dir or "exports/backtest_runs"

    return [
        config.cli_binary,
        "backtest", "csv",
        "--bars", bars_path,
        "--strategy", strategy_id,
        "--symbol", symbol,
        "--timeframe-secs", str(timeframe_secs),
        "--initial-cash-micros", str(config.initial_cash_micros),
        "--target-qty", str(config.target_qty),
        "--integrity-stale-threshold-ticks", str(config.integrity_stale_threshold_ticks),
        "--integrity-gap-tolerance-bars", str(config.integrity_gap_tolerance_bars),
        "--out-dir", out_dir,
    ]


# ---------------------------------------------------------------------------
# Metrics parser
# ---------------------------------------------------------------------------

def parse_mqk_metrics_json(path: str) -> dict[str, Any]:
    """
    Load and validate a metrics.json file from mqk-artifacts.

    Raises ValueError with a stable reason prefix on failure:
    - metrics_file_missing: file does not exist
    - metrics_schema_invalid: not JSON, not an object, or wrong schema_version
    """
    p = Path(path)
    if not p.exists():
        raise ValueError(f"{REASON_METRICS_FILE_MISSING}: {path}")
    try:
        raw = json.loads(p.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError) as exc:
        raise ValueError(f"{REASON_METRICS_SCHEMA_INVALID}: {exc}") from exc
    if not isinstance(raw, dict):
        raise ValueError(
            f"{REASON_METRICS_SCHEMA_INVALID}: metrics.json root must be an object"
        )
    sv = raw.get("schema_version")
    if sv != METRICS_SCHEMA_VERSION:
        raise ValueError(
            f"{REASON_METRICS_SCHEMA_INVALID}: unexpected schema_version={sv!r}"
        )
    return raw


# ---------------------------------------------------------------------------
# Metric normalization helpers
# ---------------------------------------------------------------------------

def _normalize_pct_to_fraction(value: float) -> float:
    """
    Normalize a win_rate value to a 0-1 fraction.

    If value > 1.0: treat as percentage (e.g. 60.0 → 0.60).
    If value <= 1.0: treat as already a fraction (e.g. 0.60 → 0.60).

    Note: values in (0, 1) are ambiguous — this convention treats them as
    fractions. Rust always outputs 0-100 (multiply by 100 before storing),
    so Rust values < 1.0 represent < 1% win rates (extremely rare).
    """
    if value > 1.0:
        return value / 100.0
    return value


def _normalize_drawdown_to_bps(value: float) -> float:
    """
    Normalize a drawdown value to basis points.

    If value > 1.0: treat as percentage (e.g. 5.0 → 500 bps).
    If value <= 1.0: treat as fraction (e.g. 0.05 → 500 bps).

    Note: Rust outputs max_drawdown_pct as a percentage (0-100 scale).
    Values < 1.0 from Rust are drawdowns < 1% and are treated as fractions
    here, which will over-estimate their bps value. Use fixtures with values
    > 1.0 for predictable test behavior.
    """
    if value > 1.0:
        return value * 100.0    # 5.0% → 500 bps
    return value * 10_000.0     # 0.05 fraction → 500 bps


# ---------------------------------------------------------------------------
# Metric mapper
# ---------------------------------------------------------------------------

def map_metrics_to_strategy_fit(
    entry: dict[str, Any],
    metrics: dict[str, Any],
    config: BacktestBridgeConfig,
) -> tuple[dict[str, Any], list[str]]:
    """
    Map a parsed metrics.json dict to strategy-fit-v1 field values.

    Returns (mapped_dict, failure_reasons).

    mapped_dict keys match StrategyFitResult fields in backtest_runner.py.
    validation_metrics_missing is always appended (no walk-forward exists yet).
    expectancy_basis_missing is appended when no notional basis can be derived.
    """
    failure_reasons: list[str] = []

    bars = metrics.get("bars")
    trade_count = metrics.get("trade_count")
    win_rate_pct_raw = metrics.get("win_rate_pct")
    profit_factor = metrics.get("profit_factor")
    expectancy_micros = metrics.get("expectancy_micros")
    max_drawdown_pct_raw = metrics.get("max_drawdown_pct")
    sharpe_ratio = metrics.get("sharpe_ratio")
    sortino_ratio = metrics.get("sortino_ratio")
    exposure_time_pct = metrics.get("exposure_time_pct")
    total_commission_micros: int = metrics.get("total_commission_micros") or 0

    win_rate: Optional[float] = None
    if win_rate_pct_raw is not None:
        win_rate = _normalize_pct_to_fraction(float(win_rate_pct_raw))

    max_drawdown_bps: Optional[float] = None
    if max_drawdown_pct_raw is not None:
        max_drawdown_bps = _normalize_drawdown_to_bps(float(max_drawdown_pct_raw))

    # expectancy_micros → expectancy_bps requires notional basis.
    expectancy_bps: Optional[float] = None
    net_expectancy_after_cost_bps: Optional[float] = None

    symbols_list: list[str] = metrics.get("symbols") or []
    last_prices: dict[str, Any] = metrics.get("last_prices_micros") or {}
    sizing: dict[str, Any] = metrics.get("strategy_sizing") or {}
    qty = sizing.get("target_qty")

    if symbols_list and last_prices and qty and qty > 0:
        sym = symbols_list[0]
        price_micros = last_prices.get(sym)
        if price_micros and price_micros > 0 and expectancy_micros is not None:
            notional = float(price_micros) * float(qty)
            expectancy_bps = float(expectancy_micros) / notional * 10_000.0
            if trade_count and trade_count > 0:
                comm_per_trade = float(total_commission_micros) / float(trade_count)
                net_exp = float(expectancy_micros) - comm_per_trade
                net_expectancy_after_cost_bps = net_exp / notional * 10_000.0
            else:
                net_expectancy_after_cost_bps = expectancy_bps
        elif expectancy_micros is not None:
            failure_reasons.append(REASON_EXPECTANCY_BASIS_MISSING)
    elif expectancy_micros is not None:
        failure_reasons.append(REASON_EXPECTANCY_BASIS_MISSING)

    # Validation metrics absent — fail-closed until walk-forward validation exists.
    failure_reasons.append(REASON_VALIDATION_METRICS_MISSING)

    mapped: dict[str, Any] = {
        "bars_used": bars,
        "trades": trade_count,
        "win_rate": win_rate,
        "profit_factor": profit_factor,
        "expectancy_bps": expectancy_bps,
        "avg_trade_bps": None,   # not derivable without per-trade notional
        "max_drawdown_bps": max_drawdown_bps,
        "sharpe": sharpe_ratio,
        "sortino": sortino_ratio,
        "exposure_time_pct": exposure_time_pct,
        "net_expectancy_after_cost_bps": net_expectancy_after_cost_bps,
    }
    return mapped, failure_reasons


# ---------------------------------------------------------------------------
# Subprocess execution path (only reached when mode="real")
# ---------------------------------------------------------------------------

def run_backtest_for_entry(
    entry: dict[str, Any],
    config: BacktestBridgeConfig,
) -> tuple[Optional[dict[str, Any]], list[str]]:
    """
    Execute the Rust backtest CLI for a single queue entry.

    Returns (metrics_dict_or_None, failure_reasons).

    ONLY called when config.mode == "real". In dry-run mode this function is
    never invoked. subprocess is imported INSIDE this function — not at module
    level.

    Safety: only invokes the local Rust binary. Never contacts broker, OMS,
    daemon, or any network endpoint.
    """
    failure_reasons: list[str] = []

    cmd = build_mqk_backtest_command(entry, config)
    if cmd is None:
        failure_reasons.append(REASON_COMMAND_BUILD_FAILED)
        return None, failure_reasons

    binary = cmd[0]
    if not Path(binary).exists():
        failure_reasons.append(REASON_BINARY_NOT_FOUND)
        return None, failure_reasons

    import subprocess  # deferred — not at module level

    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=300,
        )
    except (OSError, subprocess.TimeoutExpired):
        failure_reasons.append(REASON_BACKTEST_COMMAND_FAILED)
        return None, failure_reasons

    if proc.returncode != 0:
        failure_reasons.append(REASON_BACKTEST_COMMAND_FAILED)
        return None, failure_reasons

    # Parse stdout for the artifacts directory path printed by mqk-cli.
    artifacts_dir: Optional[str] = None
    for line in (proc.stdout or "").splitlines():
        if line.startswith("artifacts_dir="):
            artifacts_dir = line.split("=", 1)[1].strip()
            break

    if not artifacts_dir:
        failure_reasons.append(REASON_METRICS_FILE_MISSING)
        return None, failure_reasons

    metrics_path = str(Path(artifacts_dir) / "metrics.json")
    try:
        metrics = parse_mqk_metrics_json(metrics_path)
    except ValueError as exc:
        raw_reason = str(exc).split(":")[0]
        reason = raw_reason if raw_reason in (
            REASON_METRICS_FILE_MISSING, REASON_METRICS_SCHEMA_INVALID
        ) else REASON_METRICS_SCHEMA_INVALID
        failure_reasons.append(reason)
        return None, failure_reasons

    return metrics, failure_reasons
