"""
MARKET-DATA-EXPORT-01: read-only md_bars to scanner bars_root export.

Exports completed canonical Postgres md_bars rows into one local CSV per
symbol/timeframe for symbol_inputs_runner.normalize_bars_csv().

Hard invariants:
- approved_for_live is always False at config and result layers
- database access is read-only SELECT through SQLAlchemy parameter binding
- writes are limited to the caller-provided bars_root
- no daemon, runtime, OMS, broker adapter, or strategy-signal imports
- no subprocess or shell execution
"""
from __future__ import annotations

import argparse
import csv
import json
import re
import sys
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

from sqlalchemy import text
from sqlalchemy.sql.elements import TextClause

from mqk_research.io.pg import PgConfig, make_engine


STATUS_COMPLETE = "complete"
STATUS_PARTIAL = "partial"
STATUS_BLOCKED = "blocked"

REASON_MISSING_DATABASE_URL = "market_data_export_missing_database_url"
REASON_NO_SYMBOLS_REQUESTED = "market_data_export_no_symbols_requested"
REASON_INVALID_SYMBOL = "market_data_export_invalid_symbol"
REASON_INVALID_TIMEFRAME = "market_data_export_invalid_timeframe"
REASON_BARS_ROOT_MISSING = "market_data_export_bars_root_missing"
REASON_BARS_ROOT_INVALID = "market_data_export_bars_root_invalid"
REASON_OUTPUT_FORMAT_UNSUPPORTED = "market_data_export_output_format_unsupported"
REASON_INVALID_TIME_WINDOW = "market_data_export_invalid_time_window"
REASON_DATABASE_QUERY_FAILED = "market_data_export_database_query_failed"
REASON_OUTPUT_PATH_ESCAPE = "market_data_export_output_path_escape"
REASON_WRITE_FAILED = "market_data_export_write_failed"
REASON_NO_ROWS_FOR_SYMBOL = "market_data_export_no_rows_for_symbol"
REASON_NO_ROWS_EXPORTED = "market_data_export_no_rows_exported"

ALL_EXPORT_REASONS = (
    REASON_MISSING_DATABASE_URL,
    REASON_NO_SYMBOLS_REQUESTED,
    REASON_INVALID_SYMBOL,
    REASON_INVALID_TIMEFRAME,
    REASON_BARS_ROOT_MISSING,
    REASON_BARS_ROOT_INVALID,
    REASON_OUTPUT_FORMAT_UNSUPPORTED,
    REASON_INVALID_TIME_WINDOW,
    REASON_DATABASE_QUERY_FAILED,
    REASON_OUTPUT_PATH_ESCAPE,
    REASON_WRITE_FAILED,
    REASON_NO_ROWS_FOR_SYMBOL,
    REASON_NO_ROWS_EXPORTED,
)

_POSTGRES_BARE_SCHEMES = ("postgres://", "postgresql://")

_SAFE_FILE_COMPONENT_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
_BACKTEST_CSV_HEADER = (
    "symbol",
    "end_ts",
    "open_micros",
    "high_micros",
    "low_micros",
    "close_micros",
    "volume",
    "is_complete",
)


@dataclass
class MarketDataExportConfig:
    database_url: str = ""
    symbols: list[str] = field(default_factory=list)
    timeframe: str = "5m"
    start_utc: Optional[str] = None
    end_utc: Optional[str] = None
    bars_root: str = ""
    trade_date: Optional[str] = None
    output_format: str = "csv"
    approved_for_live: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(self, "approved_for_live", False)


@dataclass
class MarketDataExportResult:
    status: str = STATUS_BLOCKED
    symbols_requested: list[str] = field(default_factory=list)
    symbols_exported: list[str] = field(default_factory=list)
    symbols_missing: list[str] = field(default_factory=list)
    bars_written_by_symbol: dict[str, int] = field(default_factory=dict)
    output_paths: dict[str, str] = field(default_factory=dict)
    failure_reasons: list[str] = field(default_factory=list)
    approved_for_live: bool = False
    notes: str = ""

    def __post_init__(self) -> None:
        object.__setattr__(self, "approved_for_live", False)

    def to_dict(self) -> dict[str, Any]:
        return {
            "status": self.status,
            "symbols_requested": list(self.symbols_requested),
            "symbols_exported": list(self.symbols_exported),
            "symbols_missing": list(self.symbols_missing),
            "bars_written_by_symbol": dict(self.bars_written_by_symbol),
            "output_paths": dict(self.output_paths),
            "failure_reasons": list(self.failure_reasons),
            "approved_for_live": False,
            "notes": self.notes,
        }


def _dedupe_preserve_order(items: list[str]) -> list[str]:
    seen: set[str] = set()
    out: list[str] = []
    for item in items:
        if item not in seen:
            seen.add(item)
            out.append(item)
    return out


def _is_safe_file_component(value: str) -> bool:
    return bool(_SAFE_FILE_COMPONENT_RE.fullmatch(value))


def _normalize_symbols(symbols: list[str]) -> tuple[list[str], list[str]]:
    normalized: list[str] = []
    failures: list[str] = []
    for raw in symbols:
        if not isinstance(raw, str):
            failures.append(f"{REASON_INVALID_SYMBOL}:{raw!r}")
            continue
        symbol = raw.strip().upper()
        if not symbol:
            continue
        if not _is_safe_file_component(symbol):
            failures.append(f"{REASON_INVALID_SYMBOL}:{symbol}")
            continue
        normalized.append(symbol)
    return _dedupe_preserve_order(normalized), failures


def _parse_utc_epoch_seconds(raw: Optional[str], name: str) -> tuple[Optional[int], Optional[str]]:
    if raw is None or str(raw).strip() == "":
        return None, None
    value = str(raw).strip().replace("Z", "+00:00")
    try:
        parsed = datetime.fromisoformat(value)
    except ValueError:
        return None, f"{REASON_INVALID_TIME_WINDOW}:{name}"
    if parsed.tzinfo is None:
        return None, f"{REASON_INVALID_TIME_WINDOW}:{name}"
    return int(parsed.astimezone(timezone.utc).timestamp()), None


def normalize_database_url(database_url: str) -> str:
    """Rewrite a bare postgres(ql):// URL to the psycopg3 dialect driver.

    SQLAlchemy's default dialect driver for a driver-less postgres(ql)://
    URL is psycopg2, which is not installed in this project (only psycopg3
    is). URLs that already specify a driver (e.g. postgresql+psycopg://) or
    use a non-postgres scheme are returned unchanged.
    """
    url = str(database_url or "")
    for scheme in _POSTGRES_BARE_SCHEMES:
        if url.startswith(scheme):
            return "postgresql+psycopg://" + url[len(scheme):]
    return url


def build_select_md_bars_statement() -> TextClause:
    return text(
        """
        select
          symbol,
          timeframe,
          end_ts,
          open_micros,
          high_micros,
          low_micros,
          close_micros,
          volume,
          is_complete
        from md_bars
        where timeframe = :timeframe
          and symbol = any(:symbols)
          and (cast(:start_end_ts as bigint) is null or end_ts >= cast(:start_end_ts as bigint))
          and (cast(:end_end_ts as bigint) is null or end_ts <= cast(:end_end_ts as bigint))
          and is_complete = true
        order by symbol asc, end_ts asc
        """
    )


def _fetch_md_bars(
    database_url: str,
    *,
    symbols: list[str],
    timeframe: str,
    start_end_ts: Optional[int],
    end_end_ts: Optional[int],
) -> list[Any]:
    engine = make_engine(PgConfig(normalize_database_url(database_url)))
    params = {
        "symbols": list(symbols),
        "timeframe": timeframe,
        "start_end_ts": start_end_ts,
        "end_end_ts": end_end_ts,
    }
    with engine.connect() as cxn:
        return list(cxn.execute(build_select_md_bars_statement(), params).fetchall())


def _row_value(row: Any, key: str) -> Any:
    mapping = getattr(row, "_mapping", None)
    if mapping is not None:
        return mapping[key]
    if isinstance(row, dict):
        return row[key]
    try:
        return row[key]
    except (TypeError, KeyError, IndexError):
        return getattr(row, key)


def _csv_bool(value: Any) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    raw = str(value).strip().lower()
    if raw in ("1", "true", "t", "yes"):
        return "true"
    if raw in ("0", "false", "f", "no"):
        return "false"
    raise ValueError("is_complete is not parseable as boolean")


def normalize_db_row_to_csv_row(row: Any) -> dict[str, Any]:
    return {
        "symbol": str(_row_value(row, "symbol")).strip().upper(),
        "end_ts": int(_row_value(row, "end_ts")),
        "open_micros": int(_row_value(row, "open_micros")),
        "high_micros": int(_row_value(row, "high_micros")),
        "low_micros": int(_row_value(row, "low_micros")),
        "close_micros": int(_row_value(row, "close_micros")),
        "volume": int(_row_value(row, "volume")),
        "is_complete": _csv_bool(_row_value(row, "is_complete")),
    }


def _safe_output_path(root: Path, symbol: str, timeframe: str) -> Path:
    if not _is_safe_file_component(symbol) or not _is_safe_file_component(timeframe):
        raise ValueError(REASON_OUTPUT_PATH_ESCAPE)
    path = (root / f"{symbol}_{timeframe}.csv").resolve()
    path.relative_to(root)
    return path


def write_symbol_timeframe_csv(
    *,
    bars_root: Path,
    symbol: str,
    timeframe: str,
    csv_rows: list[dict[str, Any]],
) -> Path:
    output_path = _safe_output_path(bars_root, symbol, timeframe)
    ordered_rows = sorted(csv_rows, key=lambda row: int(row["end_ts"]))
    with output_path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=_BACKTEST_CSV_HEADER, lineterminator="\n")
        writer.writeheader()
        for row in ordered_rows:
            writer.writerow({name: row[name] for name in _BACKTEST_CSV_HEADER})
    return output_path


def _blocked(
    reasons: list[str],
    *,
    symbols_requested: Optional[list[str]] = None,
    notes: Optional[str] = None,
) -> MarketDataExportResult:
    unique_reasons = _dedupe_preserve_order(reasons)
    return MarketDataExportResult(
        status=STATUS_BLOCKED,
        symbols_requested=list(symbols_requested or []),
        failure_reasons=unique_reasons,
        notes=notes or f"blocked: {unique_reasons[0] if unique_reasons else STATUS_BLOCKED}",
    )


def _prepare_bars_root(bars_root: str) -> tuple[Optional[Path], Optional[str]]:
    if not bars_root or not str(bars_root).strip():
        return None, REASON_BARS_ROOT_MISSING
    root = Path(str(bars_root).strip()).expanduser().resolve()
    if root.exists() and not root.is_dir():
        return None, REASON_BARS_ROOT_INVALID
    try:
        root.mkdir(parents=True, exist_ok=True)
    except OSError:
        return None, REASON_BARS_ROOT_INVALID
    return root, None


def export_md_bars_to_bars_root(config: MarketDataExportConfig) -> MarketDataExportResult:
    reasons: list[str] = []

    symbols, symbol_failures = _normalize_symbols(config.symbols)
    reasons.extend(symbol_failures)

    if not config.database_url or not str(config.database_url).strip():
        reasons.append(REASON_MISSING_DATABASE_URL)
    if not symbols:
        reasons.append(REASON_NO_SYMBOLS_REQUESTED)
    if config.output_format != "csv":
        reasons.append(REASON_OUTPUT_FORMAT_UNSUPPORTED)

    timeframe = str(config.timeframe or "").strip()
    if not timeframe or not _is_safe_file_component(timeframe):
        reasons.append(REASON_INVALID_TIMEFRAME)

    start_end_ts, start_reason = _parse_utc_epoch_seconds(config.start_utc, "start_utc")
    end_end_ts, end_reason = _parse_utc_epoch_seconds(config.end_utc, "end_utc")
    if start_reason:
        reasons.append(start_reason)
    if end_reason:
        reasons.append(end_reason)
    if start_end_ts is not None and end_end_ts is not None and start_end_ts > end_end_ts:
        reasons.append(REASON_INVALID_TIME_WINDOW)

    if reasons:
        return _blocked(reasons, symbols_requested=symbols)

    bars_root, root_reason = _prepare_bars_root(config.bars_root)
    if root_reason is not None or bars_root is None:
        return _blocked([root_reason or REASON_BARS_ROOT_INVALID], symbols_requested=symbols)

    try:
        db_rows = _fetch_md_bars(
            str(config.database_url).strip(),
            symbols=symbols,
            timeframe=timeframe,
            start_end_ts=start_end_ts,
            end_end_ts=end_end_ts,
        )
    except Exception:
        return _blocked(
            [REASON_DATABASE_QUERY_FAILED],
            symbols_requested=symbols,
            notes=f"blocked: {REASON_DATABASE_QUERY_FAILED}",
        )

    rows_by_symbol: dict[str, list[dict[str, Any]]] = {symbol: [] for symbol in symbols}
    try:
        for row in db_rows:
            csv_row = normalize_db_row_to_csv_row(row)
            symbol = csv_row["symbol"]
            if symbol in rows_by_symbol:
                rows_by_symbol[symbol].append(csv_row)
    except (KeyError, TypeError, ValueError):
        return _blocked(
            [REASON_DATABASE_QUERY_FAILED],
            symbols_requested=symbols,
            notes=f"blocked: {REASON_DATABASE_QUERY_FAILED}",
        )

    symbols_exported: list[str] = []
    symbols_missing: list[str] = []
    bars_written_by_symbol: dict[str, int] = {}
    output_paths: dict[str, str] = {}
    write_failures: list[str] = []

    for symbol in symbols:
        rows = rows_by_symbol[symbol]
        if not rows:
            symbols_missing.append(symbol)
            continue
        try:
            out_path = write_symbol_timeframe_csv(
                bars_root=bars_root,
                symbol=symbol,
                timeframe=timeframe,
                csv_rows=rows,
            )
        except (OSError, ValueError):
            write_failures.append(f"{REASON_WRITE_FAILED}:{symbol}")
            continue
        symbols_exported.append(symbol)
        bars_written_by_symbol[symbol] = len(rows)
        output_paths[symbol] = str(out_path)

    if write_failures:
        return MarketDataExportResult(
            status=STATUS_BLOCKED,
            symbols_requested=symbols,
            symbols_exported=symbols_exported,
            symbols_missing=symbols_missing,
            bars_written_by_symbol=bars_written_by_symbol,
            output_paths=output_paths,
            failure_reasons=_dedupe_preserve_order(write_failures),
            notes=f"blocked: {REASON_WRITE_FAILED}",
        )

    if not symbols_exported:
        return MarketDataExportResult(
            status=STATUS_BLOCKED,
            symbols_requested=symbols,
            symbols_missing=symbols_missing,
            failure_reasons=[REASON_NO_ROWS_EXPORTED]
            + [f"{REASON_NO_ROWS_FOR_SYMBOL}:{symbol}" for symbol in symbols_missing],
            notes=f"blocked: {REASON_NO_ROWS_EXPORTED}",
        )

    if symbols_missing:
        return MarketDataExportResult(
            status=STATUS_PARTIAL,
            symbols_requested=symbols,
            symbols_exported=symbols_exported,
            symbols_missing=symbols_missing,
            bars_written_by_symbol=bars_written_by_symbol,
            output_paths=output_paths,
            failure_reasons=[f"{REASON_NO_ROWS_FOR_SYMBOL}:{symbol}" for symbol in symbols_missing],
            notes=(
                f"partial: exported {len(symbols_exported)} symbol(s); "
                f"missing {len(symbols_missing)} symbol(s)"
            ),
        )

    return MarketDataExportResult(
        status=STATUS_COMPLETE,
        symbols_requested=symbols,
        symbols_exported=symbols_exported,
        symbols_missing=[],
        bars_written_by_symbol=bars_written_by_symbol,
        output_paths=output_paths,
        failure_reasons=[],
        notes=f"complete: exported {len(symbols_exported)} symbol(s)",
    )


def _build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m mqk_research.scanner.market_data_export",
        description=(
            "Export completed md_bars rows to local bars_root CSV files for "
            "symbol_inputs_runner. Read-only DB access; approved_for_live is always false."
        ),
    )
    parser.add_argument("--database-url", dest="database_url", default="")
    parser.add_argument("--symbols", dest="symbols", default="")
    parser.add_argument("--timeframe", dest="timeframe", default="5m")
    parser.add_argument("--start-utc", dest="start_utc", default=None)
    parser.add_argument("--end-utc", dest="end_utc", default=None)
    parser.add_argument("--bars-root", dest="bars_root", default="")
    parser.add_argument("--trade-date", dest="trade_date", default=None)
    return parser


def main(argv: Optional[list[str]] = None) -> int:
    parser = _build_arg_parser()
    args = parser.parse_args(argv)
    symbols = [item.strip() for item in args.symbols.split(",") if item.strip()]
    result = export_md_bars_to_bars_root(
        MarketDataExportConfig(
            database_url=args.database_url,
            symbols=symbols,
            timeframe=args.timeframe,
            start_utc=args.start_utc,
            end_utc=args.end_utc,
            bars_root=args.bars_root,
            trade_date=args.trade_date,
        )
    )
    print(json.dumps(result.to_dict(), indent=2, sort_keys=False))
    return 0 if result.status in (STATUS_COMPLETE, STATUS_PARTIAL) else 1


if __name__ == "__main__":
    sys.exit(main())
