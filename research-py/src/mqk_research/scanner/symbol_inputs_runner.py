"""
SYMBOL-INPUTS-RUNNER-01 — Operational artifact runner for `symbol-inputs-v1`.

Reads a watchlist artifact or explicit symbol list, loads caller-local bar
files (and an optional liquidity sidecar) from disk, builds `symbol-inputs-v1`
records via the existing `symbol_inputs` producer, and writes one deterministic
artifact for `premarket_revalidation.evaluate_premarket_watchlist` to consume.

This module is an operational *runner* — it does not invent producer/gate
logic. It resolves local files into the in-memory shapes `symbol_inputs`
already accepts (`SymbolInputSpec`, `LiquidityMetrics`) and delegates assembly
to `build_symbol_inputs` / `write_symbol_inputs_artifact`.

Hard invariants:
- `approved_for_live` is ALWAYS False — at config, result, and artifact level
- A forged `approved_for_live=True` on an input watchlist never propagates;
  the runner reports `watchlist_live_approval_forbidden` instead
- No imports from broker adapters, OMS tables, execution orchestrator, or DB
- No network imports (requests, urllib, http.client, aiohttp, psycopg, sqlalchemy)
- No daemon/runtime imports
- No subprocess
- Local/offline only — caller supplies bars + optional liquidity sidecar; this
  module does not fetch market data or call any broker/API
- Missing/malformed inputs fail closed with explicit, stable reason strings —
  never silent omission, never a fabricated healthy record

Fail-closed cases represented (never disappear silently):
- no watchlist/symbol input            -> blocked, "symbol_inputs_runner_no_input_source"
- watchlist fails to load/parse        -> blocked, "symbol_inputs_runner_watchlist_load_failed"
- watchlist/symbol list resolves empty -> blocked, "symbol_inputs_runner_no_symbols_resolved"
- bars root missing/not a directory    -> blocked, "symbol_inputs_runner_bars_root_missing"
- no bars file found for a symbol      -> symbol still written with a fail-closed
                                           no-bars record; symbol listed in `missing_bars`
- malformed bars file for a symbol     -> symbol still written with a fail-closed
                                           no-bars record; symbol listed in `failed_symbols`
                                           with reason "symbol_inputs_runner_malformed_bars_file:<symbol>"
- liquidity sidecar absent             -> liquidity_metrics=None (producer fail-closes liquidity)
- liquidity sidecar malformed          -> liquidity_metrics=None; reason
                                           "symbol_inputs_runner_liquidity_sidecar_malformed:<symbol>"
- output artifact cannot be written    -> blocked, "symbol_inputs_runner_output_write_failed"
- forged `approved_for_live=True`      -> does not block; reported via
                                           `watchlist_live_approval_forbidden=True` and
                                           "symbol_inputs_runner_watchlist_live_approval_forbidden"

Notes:
- Bars file candidates under `bars_root` (first match wins, in this order):
  `<SYMBOL>_<TIMEFRAME>.json`, `<SYMBOL>_<TIMEFRAME>.csv`, `<SYMBOL>.json`, `<SYMBOL>.csv`
- JSON bars: a list of bar dicts, or a dict with a `bars` list — passed through
  as-is (already in the `symbol_inputs` scanner-style shape).
- CSV bars: supports both scanner-style (`ts,open,high,low,close,volume[,is_complete]`)
  and backtest-style (`symbol,end_ts,open_micros,high_micros,low_micros,close_micros,
  volume,is_complete`) headers, detected from the header row. Backtest-style micros
  are converted to decimal dollars (value / 1_000_000); `end_ts` (Unix epoch seconds,
  UTC) is converted to an ISO-8601 `ts` string; `is_complete` is passed through as a
  boolean when present and parseable.
- Liquidity sidecar candidates under `liquidity_root`: `<SYMBOL>.json`,
  `<SYMBOL>_liquidity.json` — a flat JSON object with the seven `LiquidityMetrics`
  fields (`latest_close`, `avg_daily_volume_shares`, `avg_daily_dollar_volume`,
  `relative_volume`, `spread_estimate_bps`, `slippage_estimate_bps`,
  `round_trip_cost_bps`). This module never derives liquidity from bars.
"""
from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

from mqk_research.scanner.data_quality import DataQualityConfig
from mqk_research.scanner.liquidity import LiquidityConfig, LiquidityMetrics
from mqk_research.scanner.regime import RegimeConfig
from mqk_research.scanner.symbol_inputs import (
    SymbolInputSpec,
    build_symbol_inputs,
    write_symbol_inputs_artifact,
)

# ---------------------------------------------------------------------------
# Stable reason constants
# ---------------------------------------------------------------------------

REASON_NO_INPUT_SOURCE = "symbol_inputs_runner_no_input_source"
REASON_WATCHLIST_LOAD_FAILED = "symbol_inputs_runner_watchlist_load_failed"
REASON_NO_SYMBOLS_RESOLVED = "symbol_inputs_runner_no_symbols_resolved"
REASON_BARS_ROOT_MISSING = "symbol_inputs_runner_bars_root_missing"
REASON_MALFORMED_BARS_FILE = "symbol_inputs_runner_malformed_bars_file"
REASON_LIQUIDITY_SIDECAR_MALFORMED = "symbol_inputs_runner_liquidity_sidecar_malformed"
REASON_OUTPUT_WRITE_FAILED = "symbol_inputs_runner_output_write_failed"
REASON_WATCHLIST_LIVE_APPROVAL_FORBIDDEN = "symbol_inputs_runner_watchlist_live_approval_forbidden"

ALL_RUNNER_REASONS = (
    REASON_NO_INPUT_SOURCE,
    REASON_WATCHLIST_LOAD_FAILED,
    REASON_NO_SYMBOLS_RESOLVED,
    REASON_BARS_ROOT_MISSING,
    REASON_MALFORMED_BARS_FILE,
    REASON_LIQUIDITY_SIDECAR_MALFORMED,
    REASON_OUTPUT_WRITE_FAILED,
    REASON_WATCHLIST_LIVE_APPROVAL_FORBIDDEN,
)

_SCANNER_CSV_REQUIRED_COLUMNS = ("ts", "open", "high", "low", "close", "volume")
_BACKTEST_CSV_REQUIRED_COLUMNS = (
    "symbol", "end_ts", "open_micros", "high_micros", "low_micros", "close_micros", "volume",
)
_LIQUIDITY_METRICS_FIELDS = (
    "latest_close",
    "avg_daily_volume_shares",
    "avg_daily_dollar_volume",
    "relative_volume",
    "spread_estimate_bps",
    "slippage_estimate_bps",
    "round_trip_cost_bps",
)


# ---------------------------------------------------------------------------
# Config / Result
# ---------------------------------------------------------------------------

@dataclass
class SymbolInputsRunnerConfig:
    """
    Configuration for the symbol_inputs operational runner.

    Exactly one of `watchlist_path` / `symbols` should be supplied as the
    symbol-list source; if both are present, `watchlist_path` takes precedence
    (it is the canonical premarket-handoff source). If neither is present, the
    run is blocked.

    `bars_root`: directory containing per-symbol bar files (see module docstring
    for the candidate filename conventions). Required — missing/absent blocks.

    `liquidity_root`: optional directory containing per-symbol liquidity sidecar
    JSON files. If omitted, every symbol is built with `liquidity_metrics=None`
    (the producer then fail-closes liquidity honestly).

    `approved_for_live` is a hard invariant — always forced to False.
    """
    trade_date: str
    output_path: str
    bars_root: Optional[str] = None
    timeframe: str = "5m"
    watchlist_path: Optional[str] = None
    symbols: Optional[list[str]] = None
    liquidity_root: Optional[str] = None
    source: str = "symbol_inputs_runner"
    generated_at_utc: Optional[str] = None
    dq_config: Optional[DataQualityConfig] = None
    liquidity_config: Optional[LiquidityConfig] = None
    regime_config: Optional[RegimeConfig] = None
    now_utc: Optional[datetime] = None
    approved_for_live: bool = False

    def __post_init__(self) -> None:
        # Hard invariant: live approval is never permitted through config.
        object.__setattr__(self, "approved_for_live", False)


@dataclass
class SymbolInputsRunnerResult:
    """
    Structured result of a `run_symbol_inputs_producer` call.

    `status`:
      "complete" — every requested symbol resolved bars and was written cleanly.
      "partial"  — an artifact was written, but one or more symbols had missing
                   or malformed bars (each still appears with a fail-closed
                   no-bars record; see `missing_bars` / `failed_symbols`).
      "blocked"  — no artifact was written (bad/absent input source, missing
                   bars root, or write failure).

    `approved_for_live` is a hard invariant — always False.
    """
    symbols_requested: list[str] = field(default_factory=list)
    symbols_written: list[str] = field(default_factory=list)
    output_path: Optional[str] = None
    missing_bars: list[str] = field(default_factory=list)
    failed_symbols: list[str] = field(default_factory=list)
    failure_reasons: list[str] = field(default_factory=list)
    watchlist_live_approval_forbidden: bool = False
    approved_for_live: bool = False
    status: str = "blocked"
    notes: str = ""

    def __post_init__(self) -> None:
        # Hard invariant: live approval is never permitted through the result.
        object.__setattr__(self, "approved_for_live", False)

    def to_dict(self) -> dict[str, Any]:
        return {
            "symbols_requested": list(self.symbols_requested),
            "symbols_written": list(self.symbols_written),
            "output_path": self.output_path,
            "missing_bars": list(self.missing_bars),
            "failed_symbols": list(self.failed_symbols),
            "failure_reasons": list(self.failure_reasons),
            "watchlist_live_approval_forbidden": self.watchlist_live_approval_forbidden,
            "approved_for_live": False,
            "status": self.status,
            "notes": self.notes,
        }


# ---------------------------------------------------------------------------
# Pure helpers — ordering / dedupe
# ---------------------------------------------------------------------------

def _dedupe_preserve_order(items: list[str]) -> list[str]:
    seen: set[str] = set()
    out: list[str] = []
    for item in items:
        if item not in seen:
            seen.add(item)
            out.append(item)
    return out


def _parse_bool(raw: str) -> Optional[bool]:
    value = raw.strip().lower()
    if value in ("1", "true", "t", "yes"):
        return True
    if value in ("0", "false", "f", "no"):
        return False
    return None


# ---------------------------------------------------------------------------
# Symbol-list loaders
# ---------------------------------------------------------------------------

def load_symbols_from_watchlist(path: str) -> tuple[Optional[list[str]], bool, Optional[str]]:
    """
    Load a symbol list from a watchlist-shaped JSON artifact.

    Returns `(symbols, forged_live_approval, error_reason)`:
    - On success: `(symbols, forged_live_approval, None)`.
    - On failure (missing file, invalid JSON, not an object, `symbols` not a
      list): `(None, False, REASON_WATCHLIST_LOAD_FAILED)`.

    `forged_live_approval` is True iff the artifact carries
    `approved_for_live: true` — the caller must report this and must never let
    it propagate. This loader does not raise; it fails closed via the reason.
    """
    try:
        text = Path(path).read_text(encoding="utf-8")
        data = json.loads(text)
    except (OSError, ValueError):
        return None, False, REASON_WATCHLIST_LOAD_FAILED

    if not isinstance(data, dict):
        return None, False, REASON_WATCHLIST_LOAD_FAILED

    symbols_raw = data.get("symbols")
    if not isinstance(symbols_raw, list):
        return None, False, REASON_WATCHLIST_LOAD_FAILED

    symbols = [s for s in symbols_raw if isinstance(s, str) and s]
    forged_live_approval = data.get("approved_for_live") is True
    return symbols, forged_live_approval, None


# ---------------------------------------------------------------------------
# Bars file resolution
# ---------------------------------------------------------------------------

def resolve_bars_path(bars_root: str, symbol: str, timeframe: str) -> Optional[Path]:
    """
    Return the first existing bars-file candidate for (symbol, timeframe), or
    None if none exist. Candidate order (first match wins):

        <SYMBOL>_<TIMEFRAME>.json
        <SYMBOL>_<TIMEFRAME>.csv
        <SYMBOL>.json
        <SYMBOL>.csv
    """
    root = Path(bars_root)
    candidates = (
        root / f"{symbol}_{timeframe}.json",
        root / f"{symbol}_{timeframe}.csv",
        root / f"{symbol}.json",
        root / f"{symbol}.csv",
    )
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    return None


# ---------------------------------------------------------------------------
# Bars normalization
# ---------------------------------------------------------------------------

def normalize_bars_json(text: str) -> tuple[Optional[list[dict[str, Any]]], Optional[str]]:
    """
    Normalize JSON bar content into the list-of-dict shape `symbol_inputs`
    already expects. Accepts either a top-level list of bar dicts, or a dict
    with a `bars` key holding such a list. Any other shape fails closed.
    """
    try:
        data = json.loads(text)
    except ValueError:
        return None, REASON_MALFORMED_BARS_FILE

    if isinstance(data, dict):
        data = data.get("bars")

    if not isinstance(data, list):
        return None, REASON_MALFORMED_BARS_FILE

    bars: list[dict[str, Any]] = []
    for entry in data:
        if not isinstance(entry, dict):
            return None, REASON_MALFORMED_BARS_FILE
        bars.append(dict(entry))
    return bars, None


def _split_csv_row(raw: str) -> list[str]:
    return [field_value.strip().strip('"') for field_value in raw.split(",")]


def _normalize_scanner_csv_rows(
    header: list[str], rows: list[str]
) -> tuple[Optional[list[dict[str, Any]]], Optional[str]]:
    try:
        idx = {name: header.index(name) for name in _SCANNER_CSV_REQUIRED_COLUMNS}
    except ValueError:
        return None, REASON_MALFORMED_BARS_FILE
    complete_idx = header.index("is_complete") if "is_complete" in header else None

    bars: list[dict[str, Any]] = []
    for raw in rows:
        fields = _split_csv_row(raw)
        if len(fields) < len(header):
            return None, REASON_MALFORMED_BARS_FILE
        try:
            bar: dict[str, Any] = {
                "ts": fields[idx["ts"]],
                "open": float(fields[idx["open"]]),
                "high": float(fields[idx["high"]]),
                "low": float(fields[idx["low"]]),
                "close": float(fields[idx["close"]]),
                "volume": float(fields[idx["volume"]]),
            }
        except (ValueError, IndexError):
            return None, REASON_MALFORMED_BARS_FILE
        if complete_idx is not None:
            parsed = _parse_bool(fields[complete_idx])
            if parsed is not None:
                bar["is_complete"] = parsed
        bars.append(bar)
    return bars, None


def _normalize_backtest_csv_rows(
    header: list[str], rows: list[str]
) -> tuple[Optional[list[dict[str, Any]]], Optional[str]]:
    try:
        idx = {name: header.index(name) for name in _BACKTEST_CSV_REQUIRED_COLUMNS}
    except ValueError:
        return None, REASON_MALFORMED_BARS_FILE
    complete_idx = header.index("is_complete") if "is_complete" in header else None

    bars: list[dict[str, Any]] = []
    for raw in rows:
        fields = _split_csv_row(raw)
        if len(fields) < len(header):
            return None, REASON_MALFORMED_BARS_FILE
        try:
            end_ts = int(fields[idx["end_ts"]])
            open_micros = int(fields[idx["open_micros"]])
            high_micros = int(fields[idx["high_micros"]])
            low_micros = int(fields[idx["low_micros"]])
            close_micros = int(fields[idx["close_micros"]])
            volume = float(fields[idx["volume"]])
        except (ValueError, IndexError):
            return None, REASON_MALFORMED_BARS_FILE
        bar: dict[str, Any] = {
            "ts": datetime.fromtimestamp(end_ts, tz=timezone.utc).isoformat(),
            "open": open_micros / 1_000_000,
            "high": high_micros / 1_000_000,
            "low": low_micros / 1_000_000,
            "close": close_micros / 1_000_000,
            "volume": volume,
        }
        if complete_idx is not None:
            parsed = _parse_bool(fields[complete_idx])
            if parsed is not None:
                bar["is_complete"] = parsed
        bars.append(bar)
    return bars, None


def normalize_bars_csv(text: str) -> tuple[Optional[list[dict[str, Any]]], Optional[str]]:
    """
    Normalize CSV bar content, auto-detecting scanner-style
    (`ts,open,high,low,close,volume[,is_complete]`) vs backtest-style
    (`symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume[,is_complete]`)
    headers. Backtest-style micros are converted to decimal dollars and
    `end_ts` (Unix epoch seconds, UTC) to an ISO-8601 `ts` string. Any other
    header shape, or any row that fails to parse, fails closed.
    """
    lines = [ln for ln in text.splitlines() if ln.strip()]
    if not lines:
        return None, REASON_MALFORMED_BARS_FILE

    header = _split_csv_row(lines[0])
    rows = lines[1:]

    if set(_SCANNER_CSV_REQUIRED_COLUMNS).issubset(header):
        return _normalize_scanner_csv_rows(header, rows)
    if set(_BACKTEST_CSV_REQUIRED_COLUMNS).issubset(header):
        return _normalize_backtest_csv_rows(header, rows)
    return None, REASON_MALFORMED_BARS_FILE


def load_bars_for_symbol(path: Path) -> tuple[Optional[list[dict[str, Any]]], Optional[str]]:
    """
    Load and normalize a resolved bars file (`.json` or `.csv`) into the
    list-of-dict shape `symbol_inputs` expects. Returns `(bars, failure_reason)`;
    `failure_reason` is `REASON_MALFORMED_BARS_FILE` on any parse/shape failure
    (unreadable file, invalid JSON/CSV, unknown extension, unparseable rows).
    """
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return None, REASON_MALFORMED_BARS_FILE

    suffix = path.suffix.lower()
    if suffix == ".json":
        return normalize_bars_json(text)
    if suffix == ".csv":
        return normalize_bars_csv(text)
    return None, REASON_MALFORMED_BARS_FILE


# ---------------------------------------------------------------------------
# Liquidity sidecar
# ---------------------------------------------------------------------------

def load_liquidity_metrics_for_symbol(
    liquidity_root: str, symbol: str
) -> tuple[Optional[LiquidityMetrics], Optional[str]]:
    """
    Load an optional per-symbol liquidity sidecar JSON file into `LiquidityMetrics`.

    Returns `(metrics, failure_reason)`:
    - No candidate file present: `(None, None)` — absence is not an error; the
      caller passes `liquidity_metrics=None` and the producer fail-closes
      liquidity honestly.
    - Candidate present but unreadable / not a JSON object / missing or
      non-numeric required fields: `(None, REASON_LIQUIDITY_SIDECAR_MALFORMED)`
      — also fail-closed (never a fabricated/partial LiquidityMetrics).

    Never derives ADV/spread/slippage/rvol from bars — sidecar values only.
    """
    root = Path(liquidity_root)
    candidates = (root / f"{symbol}.json", root / f"{symbol}_liquidity.json")
    path = next((c for c in candidates if c.is_file()), None)
    if path is None:
        return None, None

    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None, REASON_LIQUIDITY_SIDECAR_MALFORMED

    if not isinstance(data, dict):
        return None, REASON_LIQUIDITY_SIDECAR_MALFORMED

    try:
        values = {name: float(data[name]) for name in _LIQUIDITY_METRICS_FIELDS}
    except (KeyError, TypeError, ValueError):
        return None, REASON_LIQUIDITY_SIDECAR_MALFORMED

    return LiquidityMetrics(**values), None


# ---------------------------------------------------------------------------
# Runner entry point
# ---------------------------------------------------------------------------

def _blocked(
    reason: str,
    *,
    symbols_requested: Optional[list[str]] = None,
    extra_reasons: Optional[list[str]] = None,
    watchlist_live_approval_forbidden: bool = False,
) -> SymbolInputsRunnerResult:
    reasons = list(extra_reasons or [])
    reasons.append(reason)
    return SymbolInputsRunnerResult(
        symbols_requested=list(symbols_requested or []),
        failure_reasons=_dedupe_preserve_order(reasons),
        watchlist_live_approval_forbidden=watchlist_live_approval_forbidden,
        status="blocked",
        notes=f"blocked: {reason}",
    )


def run_symbol_inputs_producer(config: SymbolInputsRunnerConfig) -> SymbolInputsRunnerResult:
    """
    Resolve a symbol list + local bars (+ optional liquidity sidecars) into a
    written `symbol-inputs-v1` artifact.

    Pure-local, deterministic, fail-closed. Always returns a structured result;
    never raises on bad/missing input — every failure mode is represented as an
    explicit reason string and an honest `status`.
    """
    extra_reasons: list[str] = []
    watchlist_live_approval_forbidden = False

    # --- Step 1: resolve the symbol list -----------------------------------
    if config.watchlist_path:
        symbols, forged_live, load_error = load_symbols_from_watchlist(config.watchlist_path)
        if load_error is not None:
            return _blocked(load_error)
        watchlist_live_approval_forbidden = forged_live
        if forged_live:
            extra_reasons.append(REASON_WATCHLIST_LIVE_APPROVAL_FORBIDDEN)
        symbols_requested = list(symbols or [])
    elif config.symbols:
        symbols_requested = [s for s in config.symbols if isinstance(s, str) and s]
    else:
        return _blocked(REASON_NO_INPUT_SOURCE)

    symbols_requested = _dedupe_preserve_order(symbols_requested)
    if not symbols_requested:
        return _blocked(
            REASON_NO_SYMBOLS_RESOLVED,
            extra_reasons=extra_reasons,
            watchlist_live_approval_forbidden=watchlist_live_approval_forbidden,
        )

    # --- Step 2: validate bars root -----------------------------------------
    if not config.bars_root or not Path(config.bars_root).is_dir():
        return _blocked(
            REASON_BARS_ROOT_MISSING,
            symbols_requested=symbols_requested,
            extra_reasons=extra_reasons,
            watchlist_live_approval_forbidden=watchlist_live_approval_forbidden,
        )

    # --- Step 3: build one spec per symbol -----------------------------------
    missing_bars: list[str] = []
    failed_symbols: list[str] = []
    specs: list[SymbolInputSpec] = []

    for symbol in symbols_requested:
        bars: list[dict[str, Any]] = []
        bars_path = resolve_bars_path(config.bars_root, symbol, config.timeframe)
        if bars_path is None:
            missing_bars.append(symbol)
        else:
            loaded_bars, load_reason = load_bars_for_symbol(bars_path)
            if load_reason is not None:
                failed_symbols.append(symbol)
                extra_reasons.append(f"{load_reason}:{symbol}")
            else:
                bars = loaded_bars or []

        liquidity_metrics: Optional[LiquidityMetrics] = None
        if config.liquidity_root:
            metrics, liq_reason = load_liquidity_metrics_for_symbol(config.liquidity_root, symbol)
            if liq_reason is not None:
                extra_reasons.append(f"{liq_reason}:{symbol}")
            else:
                liquidity_metrics = metrics

        specs.append(SymbolInputSpec(
            symbol=symbol,
            timeframe=config.timeframe,
            bars=bars,
            liquidity_metrics=liquidity_metrics,
        ))

    # --- Step 4: assemble the artifact ---------------------------------------
    artifact = build_symbol_inputs(
        specs,
        trade_date=config.trade_date,
        source=config.source,
        generated_at_utc=config.generated_at_utc,
        dq_config=config.dq_config,
        liquidity_config=config.liquidity_config,
        regime_config=config.regime_config,
        now_utc=config.now_utc,
    )

    # --- Step 5: write it -----------------------------------------------------
    try:
        out_path = write_symbol_inputs_artifact(artifact, config.output_path)
    except OSError:
        return SymbolInputsRunnerResult(
            symbols_requested=symbols_requested,
            missing_bars=missing_bars,
            failed_symbols=failed_symbols,
            failure_reasons=_dedupe_preserve_order(extra_reasons + [REASON_OUTPUT_WRITE_FAILED]),
            watchlist_live_approval_forbidden=watchlist_live_approval_forbidden,
            status="blocked",
            notes=f"blocked: {REASON_OUTPUT_WRITE_FAILED}",
        )

    symbols_written = [spec.symbol for spec in specs]
    if missing_bars or failed_symbols:
        status = "partial"
        notes = (
            f"partial: wrote {len(symbols_written)} symbol(s) to {out_path}; "
            f"missing_bars={len(missing_bars)} failed_symbols={len(failed_symbols)} "
            "(each appears with a fail-closed no-bars record)"
        )
    else:
        status = "complete"
        notes = f"complete: wrote {len(symbols_written)} symbol(s) to {out_path}"

    return SymbolInputsRunnerResult(
        symbols_requested=symbols_requested,
        symbols_written=symbols_written,
        output_path=str(out_path),
        missing_bars=missing_bars,
        failed_symbols=failed_symbols,
        failure_reasons=_dedupe_preserve_order(extra_reasons),
        watchlist_live_approval_forbidden=watchlist_live_approval_forbidden,
        status=status,
        notes=notes,
    )


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------

def _build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m mqk_research.scanner.symbol_inputs_runner",
        description=(
            "Build and write a symbol-inputs-v1 artifact from a local watchlist "
            "or symbol list plus local bar files. Artifact-only — no broker, "
            "daemon, DB, or network calls; approved_for_live is always false."
        ),
    )
    source_group = parser.add_mutually_exclusive_group(required=True)
    source_group.add_argument("--watchlist", dest="watchlist_path", default=None,
                              help="Path to a watchlist-v1 JSON artifact to read symbols from")
    source_group.add_argument("--symbols", dest="symbols", default=None,
                              help="Comma-separated explicit symbol list (e.g. AAPL,MSFT)")
    parser.add_argument("--bars-root", dest="bars_root", required=True,
                        help="Directory containing per-symbol bar files")
    parser.add_argument("--timeframe", dest="timeframe", default="5m",
                        help="Bar timeframe used for bars-file resolution (default: 5m)")
    parser.add_argument("--trade-date", dest="trade_date", required=True,
                        help="Trade date recorded in the artifact (e.g. 2026-06-08)")
    parser.add_argument("--output", dest="output_path", required=True,
                        help="Output path for the written symbol-inputs-v1 artifact JSON")
    parser.add_argument("--liquidity-root", dest="liquidity_root", default=None,
                        help="Optional directory containing per-symbol liquidity sidecar JSON files")
    parser.add_argument("--source", dest="source", default="symbol_inputs_runner",
                        help="Value recorded in the artifact's `source` field")
    return parser


def main(argv: Optional[list[str]] = None) -> int:
    """
    CLI entry point: `python -m mqk_research.scanner.symbol_inputs_runner ...`

    Prints the structured runner result as JSON to stdout. Exit code is 0 for
    "complete"/"partial" (an artifact was written — possibly with fail-closed
    per-symbol gaps that are explicitly reported) and 1 for "blocked" (nothing
    was written).
    """
    parser = _build_arg_parser()
    args = parser.parse_args(argv)

    symbols_list: Optional[list[str]] = None
    if args.symbols:
        symbols_list = [s.strip() for s in args.symbols.split(",") if s.strip()]

    config = SymbolInputsRunnerConfig(
        trade_date=args.trade_date,
        output_path=args.output_path,
        bars_root=args.bars_root,
        timeframe=args.timeframe,
        watchlist_path=args.watchlist_path,
        symbols=symbols_list,
        liquidity_root=args.liquidity_root,
        source=args.source,
    )
    result = run_symbol_inputs_producer(config)
    print(json.dumps(result.to_dict(), indent=2, sort_keys=False, default=str))
    return 0 if result.status in ("complete", "partial") else 1


if __name__ == "__main__":
    sys.exit(main())
