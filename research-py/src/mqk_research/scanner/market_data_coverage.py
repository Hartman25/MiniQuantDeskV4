"""
MARKET-DATA-COVERAGE-01: read-only md_bars coverage report.

Reports row counts, time ranges, freshness, and basic gap counts for
symbol/timeframe combinations in Postgres md_bars, so operators can see
which combos are ready for backtest, paper-readiness, and paper-trading
proof runs before they are requested.

Hard invariants:
- approved_for_live is always False at config, entry, and result layers
- database access is read-only SELECT through SQLAlchemy parameter binding
- no daemon, runtime, OMS, broker adapter, or strategy-signal imports
- no subprocess or shell execution
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, Optional

from sqlalchemy import text
from sqlalchemy.sql.elements import TextClause

from mqk_research.io.pg import PgConfig, make_engine
from mqk_research.scanner.market_data_export import normalize_database_url


SCHEMA_VERSION = "market-data-coverage-v1"

STATUS_COMPLETE = "complete"
STATUS_PARTIAL = "partial"
STATUS_BLOCKED = "blocked"

REASON_MISSING_DATABASE_URL = "market_data_coverage_missing_database_url"
REASON_NO_SYMBOLS_REQUESTED = "market_data_coverage_no_symbols_requested"
REASON_NO_TIMEFRAMES_REQUESTED = "market_data_coverage_no_timeframes_requested"
REASON_INVALID_SYMBOL = "market_data_coverage_invalid_symbol"
REASON_INVALID_TIMEFRAME = "market_data_coverage_invalid_timeframe"
REASON_DATABASE_QUERY_FAILED = "market_data_coverage_database_query_failed"
REASON_NO_ROWS_FOR_SYMBOL = "market_data_coverage_no_rows_for_symbol"
REASON_NO_COVERAGE_DATA = "market_data_coverage_no_coverage_data"
REASON_STALE_DATA = "market_data_coverage_stale_data"

ALL_COVERAGE_REASONS = (
    REASON_MISSING_DATABASE_URL,
    REASON_NO_SYMBOLS_REQUESTED,
    REASON_NO_TIMEFRAMES_REQUESTED,
    REASON_INVALID_SYMBOL,
    REASON_INVALID_TIMEFRAME,
    REASON_DATABASE_QUERY_FAILED,
    REASON_NO_ROWS_FOR_SYMBOL,
    REASON_NO_COVERAGE_DATA,
    REASON_STALE_DATA,
)

_SAFE_FILE_COMPONENT_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")


@dataclass
class MarketDataCoverageConfig:
    database_url: str = ""
    symbols: list[str] = field(default_factory=list)
    timeframes: list[str] = field(default_factory=list)
    max_staleness_minutes_5m: float = 15.0
    max_staleness_minutes_1h: float = 90.0
    max_staleness_days_1d: float = 4.0
    max_expected_gap_days_1d: float = 4.0
    min_rows_for_backtest: int = 1
    assume_market_hours: Optional[bool] = None
    now_utc: Optional[datetime] = None
    approved_for_live: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(self, "approved_for_live", False)


@dataclass
class MarketDataCoverageEntry:
    symbol: str
    timeframe: str
    row_count: int = 0
    first_end_ts: Optional[int] = None
    latest_end_ts: Optional[int] = None
    latest_end_utc: Optional[str] = None
    staleness_minutes: Optional[float] = None
    is_fresh: bool = False
    gap_count: Optional[int] = None
    missing_reason: Optional[str] = None
    ready_for_backtest: bool = False
    ready_for_paper: bool = False
    failure_reasons: list[str] = field(default_factory=list)
    approved_for_live: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(self, "approved_for_live", False)

    def to_dict(self) -> dict[str, Any]:
        return {
            "symbol": self.symbol,
            "timeframe": self.timeframe,
            "row_count": self.row_count,
            "first_end_ts": self.first_end_ts,
            "latest_end_ts": self.latest_end_ts,
            "latest_end_utc": self.latest_end_utc,
            "staleness_minutes": self.staleness_minutes,
            "is_fresh": self.is_fresh,
            "gap_count": self.gap_count,
            "missing_reason": self.missing_reason,
            "ready_for_backtest": self.ready_for_backtest,
            "ready_for_paper": self.ready_for_paper,
            "failure_reasons": list(self.failure_reasons),
            "approved_for_live": False,
        }


@dataclass
class MarketDataCoverageResult:
    schema_version: str = SCHEMA_VERSION
    generated_at_utc: str = ""
    symbols_requested: list[str] = field(default_factory=list)
    timeframes_requested: list[str] = field(default_factory=list)
    symbols_ready_by_timeframe: dict[str, list[str]] = field(default_factory=dict)
    symbols_missing_by_timeframe: dict[str, list[str]] = field(default_factory=dict)
    entries: list[MarketDataCoverageEntry] = field(default_factory=list)
    approved_for_live: bool = False
    status: str = STATUS_BLOCKED
    failure_reasons: list[str] = field(default_factory=list)
    notes: str = ""

    def __post_init__(self) -> None:
        object.__setattr__(self, "approved_for_live", False)

    def to_dict(self) -> dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "generated_at_utc": self.generated_at_utc,
            "symbols_requested": list(self.symbols_requested),
            "timeframes_requested": list(self.timeframes_requested),
            "symbols_ready_by_timeframe": {
                timeframe: list(symbols) for timeframe, symbols in self.symbols_ready_by_timeframe.items()
            },
            "symbols_missing_by_timeframe": {
                timeframe: list(symbols) for timeframe, symbols in self.symbols_missing_by_timeframe.items()
            },
            "entries": [entry.to_dict() for entry in self.entries],
            "approved_for_live": False,
            "status": self.status,
            "failure_reasons": list(self.failure_reasons),
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


def _normalize_timeframes(timeframes: list[str]) -> tuple[list[str], list[str]]:
    normalized: list[str] = []
    failures: list[str] = []
    for raw in timeframes:
        if not isinstance(raw, str):
            failures.append(f"{REASON_INVALID_TIMEFRAME}:{raw!r}")
            continue
        timeframe = raw.strip()
        if not timeframe:
            continue
        if not _is_safe_file_component(timeframe):
            failures.append(f"{REASON_INVALID_TIMEFRAME}:{timeframe}")
            continue
        normalized.append(timeframe)
    return _dedupe_preserve_order(normalized), failures


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


def build_select_md_bars_coverage_statement() -> TextClause:
    return text(
        """
        select
          symbol,
          timeframe,
          end_ts
        from md_bars
        where symbol = any(:symbols)
          and timeframe = any(:timeframes)
          and is_complete = true
        order by symbol asc, timeframe asc, end_ts asc
        """
    )


def _fetch_coverage_rows(
    database_url: str,
    *,
    symbols: list[str],
    timeframes: list[str],
) -> list[Any]:
    engine = make_engine(PgConfig(normalize_database_url(database_url)))
    params = {"symbols": list(symbols), "timeframes": list(timeframes)}
    with engine.connect() as cxn:
        return list(cxn.execute(build_select_md_bars_coverage_statement(), params).fetchall())


def _is_regular_market_hours_utc(now_utc: datetime) -> bool:
    """Conservative heuristic for US equity regular trading hours.

    Treats Mon-Fri 13:00-21:00 UTC as market hours, which covers NYSE
    regular hours (9:30-16:00 ET) under both EST and EDT. This is a simple
    heuristic, not an authoritative trading calendar: it does not account
    for market holidays or early closes. Callers can bypass it entirely via
    MarketDataCoverageConfig.assume_market_hours.
    """
    if now_utc.weekday() >= 5:
        return False
    minutes_of_day = now_utc.hour * 60 + now_utc.minute
    return 13 * 60 <= minutes_of_day <= 21 * 60


def _compute_staleness_minutes(latest_end_ts: int, now_utc: datetime) -> float:
    latest_dt = datetime.fromtimestamp(latest_end_ts, tz=timezone.utc)
    return (now_utc - latest_dt).total_seconds() / 60.0


def _evaluate_freshness(
    timeframe: str,
    staleness_minutes: float,
    now_utc: datetime,
    config: MarketDataCoverageConfig,
) -> bool:
    if timeframe == "1D":
        return staleness_minutes <= (config.max_staleness_days_1d * 24 * 60)
    if timeframe == "5m":
        market_open = (
            config.assume_market_hours
            if config.assume_market_hours is not None
            else _is_regular_market_hours_utc(now_utc)
        )
        if not market_open:
            return True
        return staleness_minutes <= config.max_staleness_minutes_5m
    if timeframe == "1h":
        market_open = (
            config.assume_market_hours
            if config.assume_market_hours is not None
            else _is_regular_market_hours_utc(now_utc)
        )
        if not market_open:
            return True
        return staleness_minutes <= config.max_staleness_minutes_1h
    return staleness_minutes <= (config.max_staleness_days_1d * 24 * 60)


def _compute_gap_count_1d(sorted_end_ts: list[int], max_expected_gap_days: float) -> int:
    if len(sorted_end_ts) < 2:
        return 0
    max_gap_seconds = max_expected_gap_days * 86400.0
    gaps = 0
    for prev_ts, curr_ts in zip(sorted_end_ts, sorted_end_ts[1:]):
        if (curr_ts - prev_ts) > max_gap_seconds:
            gaps += 1
    return gaps


def _blocked(
    reasons: list[str],
    *,
    generated_at_utc: str,
    symbols_requested: Optional[list[str]] = None,
    timeframes_requested: Optional[list[str]] = None,
    notes: Optional[str] = None,
) -> MarketDataCoverageResult:
    unique_reasons = _dedupe_preserve_order(reasons)
    return MarketDataCoverageResult(
        generated_at_utc=generated_at_utc,
        symbols_requested=list(symbols_requested or []),
        timeframes_requested=list(timeframes_requested or []),
        status=STATUS_BLOCKED,
        failure_reasons=unique_reasons,
        notes=notes or f"blocked: {unique_reasons[0] if unique_reasons else STATUS_BLOCKED}",
    )


def run_market_data_coverage_report(config: MarketDataCoverageConfig) -> MarketDataCoverageResult:
    now_utc = config.now_utc or datetime.now(timezone.utc)
    generated_at_utc = now_utc.isoformat()

    reasons: list[str] = []

    symbols, symbol_failures = _normalize_symbols(config.symbols)
    reasons.extend(symbol_failures)

    timeframes, timeframe_failures = _normalize_timeframes(config.timeframes)
    reasons.extend(timeframe_failures)

    if not config.database_url or not str(config.database_url).strip():
        reasons.append(REASON_MISSING_DATABASE_URL)
    if not symbols:
        reasons.append(REASON_NO_SYMBOLS_REQUESTED)
    if not timeframes:
        reasons.append(REASON_NO_TIMEFRAMES_REQUESTED)

    if reasons:
        return _blocked(
            reasons,
            generated_at_utc=generated_at_utc,
            symbols_requested=symbols,
            timeframes_requested=timeframes,
        )

    try:
        rows = _fetch_coverage_rows(
            str(config.database_url).strip(),
            symbols=symbols,
            timeframes=timeframes,
        )
    except Exception:
        return _blocked(
            [REASON_DATABASE_QUERY_FAILED],
            generated_at_utc=generated_at_utc,
            symbols_requested=symbols,
            timeframes_requested=timeframes,
            notes=f"blocked: {REASON_DATABASE_QUERY_FAILED}",
        )

    end_ts_by_combo: dict[tuple[str, str], list[int]] = {}
    try:
        for row in rows:
            symbol = str(_row_value(row, "symbol")).strip().upper()
            timeframe = str(_row_value(row, "timeframe")).strip()
            end_ts = int(_row_value(row, "end_ts"))
            end_ts_by_combo.setdefault((symbol, timeframe), []).append(end_ts)
    except (KeyError, TypeError, ValueError):
        return _blocked(
            [REASON_DATABASE_QUERY_FAILED],
            generated_at_utc=generated_at_utc,
            symbols_requested=symbols,
            timeframes_requested=timeframes,
            notes=f"blocked: {REASON_DATABASE_QUERY_FAILED}",
        )

    entries: list[MarketDataCoverageEntry] = []
    symbols_ready_by_timeframe: dict[str, list[str]] = {timeframe: [] for timeframe in timeframes}
    symbols_missing_by_timeframe: dict[str, list[str]] = {timeframe: [] for timeframe in timeframes}
    overall_failure_reasons: list[str] = []

    for timeframe in timeframes:
        for symbol in symbols:
            combo_end_ts = sorted(end_ts_by_combo.get((symbol, timeframe), []))
            row_count = len(combo_end_ts)

            if row_count == 0:
                missing_reason = f"{REASON_NO_ROWS_FOR_SYMBOL}:{symbol}:{timeframe}"
                symbols_missing_by_timeframe[timeframe].append(symbol)
                overall_failure_reasons.append(missing_reason)
                entries.append(
                    MarketDataCoverageEntry(
                        symbol=symbol,
                        timeframe=timeframe,
                        row_count=0,
                        missing_reason=missing_reason,
                        failure_reasons=[missing_reason],
                    )
                )
                continue

            first_end_ts = combo_end_ts[0]
            latest_end_ts = combo_end_ts[-1]
            latest_end_dt = datetime.fromtimestamp(latest_end_ts, tz=timezone.utc)
            staleness_minutes = _compute_staleness_minutes(latest_end_ts, now_utc)
            is_fresh = _evaluate_freshness(timeframe, staleness_minutes, now_utc, config)

            # gap_count is only computed for 1D. For "5m" and "1h" it is reported
            # as None rather than fabricated: detecting intraday gaps correctly
            # requires a trading-session calendar (regular hours, half-days,
            # holidays), which this report does not implement.
            gap_count: Optional[int] = None
            if timeframe == "1D":
                gap_count = _compute_gap_count_1d(combo_end_ts, config.max_expected_gap_days_1d)

            ready_for_backtest = row_count >= config.min_rows_for_backtest
            ready_for_paper = ready_for_backtest and is_fresh

            entry_failure_reasons: list[str] = []
            if not is_fresh:
                stale_reason = f"{REASON_STALE_DATA}:{symbol}:{timeframe}"
                entry_failure_reasons.append(stale_reason)
                overall_failure_reasons.append(stale_reason)

            symbols_ready_by_timeframe[timeframe].append(symbol)

            entries.append(
                MarketDataCoverageEntry(
                    symbol=symbol,
                    timeframe=timeframe,
                    row_count=row_count,
                    first_end_ts=first_end_ts,
                    latest_end_ts=latest_end_ts,
                    latest_end_utc=latest_end_dt.isoformat(),
                    staleness_minutes=staleness_minutes,
                    is_fresh=is_fresh,
                    gap_count=gap_count,
                    ready_for_backtest=ready_for_backtest,
                    ready_for_paper=ready_for_paper,
                    failure_reasons=entry_failure_reasons,
                )
            )

    total_combos = len(symbols) * len(timeframes)
    missing_combos = sum(len(missing) for missing in symbols_missing_by_timeframe.values())

    if total_combos > 0 and missing_combos == total_combos:
        return _blocked(
            [REASON_NO_COVERAGE_DATA],
            generated_at_utc=generated_at_utc,
            symbols_requested=symbols,
            timeframes_requested=timeframes,
            notes=f"blocked: {REASON_NO_COVERAGE_DATA}",
        )

    status = STATUS_PARTIAL if missing_combos > 0 else STATUS_COMPLETE
    notes = (
        f"{status}: {total_combos - missing_combos}/{total_combos} symbol/timeframe combo(s) have data"
    )

    return MarketDataCoverageResult(
        generated_at_utc=generated_at_utc,
        symbols_requested=symbols,
        timeframes_requested=timeframes,
        symbols_ready_by_timeframe=symbols_ready_by_timeframe,
        symbols_missing_by_timeframe=symbols_missing_by_timeframe,
        entries=entries,
        status=status,
        failure_reasons=_dedupe_preserve_order(overall_failure_reasons),
        notes=notes,
    )


def _parse_reference_utc(raw: Optional[str]) -> Optional[datetime]:
    if not raw:
        return None
    try:
        parsed = datetime.fromisoformat(str(raw).strip().replace("Z", "+00:00"))
    except ValueError:
        return None
    if parsed.tzinfo is None:
        return None
    return parsed.astimezone(timezone.utc)


def _build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m mqk_research.scanner.market_data_coverage",
        description=(
            "Report md_bars coverage (row counts, freshness, gap counts) across symbols "
            "and timeframes. Read-only DB access; approved_for_live is always false."
        ),
    )
    parser.add_argument("--database-url", dest="database_url", default="")
    parser.add_argument("--symbols", dest="symbols", default="")
    parser.add_argument("--timeframes", dest="timeframes", default="5m,1D")
    parser.add_argument("--now-utc", dest="now_utc", default=None)
    parser.add_argument("--max-staleness-minutes-5m", dest="max_staleness_minutes_5m", type=float, default=15.0)
    parser.add_argument("--max-staleness-minutes-1h", dest="max_staleness_minutes_1h", type=float, default=90.0)
    parser.add_argument("--max-staleness-days-1d", dest="max_staleness_days_1d", type=float, default=4.0)
    parser.add_argument("--max-expected-gap-days-1d", dest="max_expected_gap_days_1d", type=float, default=4.0)
    parser.add_argument("--min-rows-for-backtest", dest="min_rows_for_backtest", type=int, default=1)
    parser.add_argument(
        "--assume-market-hours",
        dest="assume_market_hours",
        choices=["true", "false"],
        default=None,
        help="Override the regular-market-hours heuristic used for 5m and 1h freshness checks.",
    )
    return parser


def main(argv: Optional[list[str]] = None) -> int:
    parser = _build_arg_parser()
    args = parser.parse_args(argv)
    symbols = [item.strip() for item in args.symbols.split(",") if item.strip()]
    timeframes = [item.strip() for item in args.timeframes.split(",") if item.strip()]

    assume_market_hours: Optional[bool] = None
    if args.assume_market_hours is not None:
        assume_market_hours = args.assume_market_hours == "true"

    result = run_market_data_coverage_report(
        MarketDataCoverageConfig(
            database_url=args.database_url,
            symbols=symbols,
            timeframes=timeframes,
            max_staleness_minutes_5m=args.max_staleness_minutes_5m,
            max_staleness_minutes_1h=args.max_staleness_minutes_1h,
            max_staleness_days_1d=args.max_staleness_days_1d,
            max_expected_gap_days_1d=args.max_expected_gap_days_1d,
            min_rows_for_backtest=args.min_rows_for_backtest,
            assume_market_hours=assume_market_hours,
            now_utc=_parse_reference_utc(args.now_utc),
        )
    )
    print(json.dumps(result.to_dict(), indent=2, sort_keys=False))
    return 0 if result.status in (STATUS_COMPLETE, STATUS_PARTIAL) else 1


if __name__ == "__main__":
    sys.exit(main())
