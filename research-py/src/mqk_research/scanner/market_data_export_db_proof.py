"""
MARKET-DATA-EXPORT-DB-PROOF-01: operator-run md_bars export proof.

This module composes the existing read-only md_bars exporter with local CSV
inspection and optional downstream artifact runners. It writes only to the
caller-provided bars root and proof report directory.
"""
from __future__ import annotations

import argparse
import csv
import json
import os
import sys
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

from mqk_research.scanner.market_data_export import (
    STATUS_BLOCKED as EXPORT_STATUS_BLOCKED,
    STATUS_COMPLETE as EXPORT_STATUS_COMPLETE,
    STATUS_PARTIAL as EXPORT_STATUS_PARTIAL,
    MarketDataExportConfig,
    MarketDataExportResult,
    export_md_bars_to_bars_root,
)
from mqk_research.scanner.paper_readiness_runner import (
    STATUS_READY_FOR_OPERATOR_REVIEW,
    STATUS_READY_FOR_PAPER_HANDOFF,
    PaperReadinessConfig,
    PaperReadinessResult,
    run_paper_readiness_pipeline,
)
from mqk_research.scanner.symbol_inputs_runner import (
    SymbolInputsRunnerConfig,
    SymbolInputsRunnerResult,
    normalize_bars_csv,
    run_symbol_inputs_producer,
)


SCHEMA_VERSION = "market-data-export-db-proof-v1"
REDACTED_DATABASE_URL = "<redacted>"

STATUS_COMPLETE = "complete"
STATUS_PARTIAL = "partial"
STATUS_BLOCKED = "blocked"

REASON_MISSING_DATABASE_URL = "market_data_export_db_proof_missing_database_url"
REASON_NO_SYMBOLS_REQUESTED = "market_data_export_db_proof_no_symbols_requested"
REASON_BARS_ROOT_MISSING = "market_data_export_db_proof_bars_root_missing"
REASON_PROOF_REPORT_PATH_MISSING = "market_data_export_db_proof_report_path_missing"
REASON_WATCHLIST_PATH_REQUIRED = "market_data_export_db_proof_watchlist_path_required"
REASON_STRATEGY_FIT_DIR_REQUIRED = "market_data_export_db_proof_strategy_fit_dir_required"
REASON_EXPORT_BLOCKED = "market_data_export_db_proof_export_blocked"
REASON_NO_FILES_EXPORTED = "market_data_export_db_proof_no_files_exported"
REASON_CSV_FILE_MISSING = "market_data_export_db_proof_csv_file_missing"
REASON_CSV_FILENAME_MISMATCH = "market_data_export_db_proof_csv_filename_mismatch"
REASON_CSV_HEADER_MISMATCH = "market_data_export_db_proof_csv_header_mismatch"
REASON_CSV_NO_DATA_ROWS = "market_data_export_db_proof_csv_no_data_rows"
REASON_CSV_ROW_MALFORMED = "market_data_export_db_proof_csv_row_malformed"
REASON_CSV_SYMBOL_MISMATCH = "market_data_export_db_proof_csv_symbol_mismatch"
REASON_CSV_TIMESTAMP_DESCENDING = "market_data_export_db_proof_csv_timestamp_descending"
REASON_CSV_PARSE_FAILED = "market_data_export_db_proof_csv_parse_failed"
REASON_SYMBOL_INPUTS_BLOCKED = "market_data_export_db_proof_symbol_inputs_blocked"
REASON_PAPER_READINESS_BLOCKED = "market_data_export_db_proof_paper_readiness_blocked"
REASON_PROOF_REPORT_WRITE_FAILED = "market_data_export_db_proof_report_write_failed"

CSV_HEADER = (
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
class MarketDataExportDbProofConfig:
    database_url: str = ""
    symbols: list[str] = field(default_factory=list)
    timeframe: str = "5m"
    start_utc: Optional[str] = None
    end_utc: Optional[str] = None
    bars_root: str = ""
    trade_date: str = ""
    proof_report_path: str = ""
    watchlist_path: Optional[str] = None
    strategy_fit_dir: Optional[str] = None
    liquidity_root: Optional[str] = None
    run_symbol_inputs: bool = False
    run_paper_readiness: bool = False
    operator_review_approved: bool = False
    approved_for_live: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(self, "approved_for_live", False)


@dataclass
class CsvInspectionResult:
    symbol: str
    path: str
    passed: bool = False
    row_count: int = 0
    first_end_ts: Optional[int] = None
    last_end_ts: Optional[int] = None
    failure_reasons: list[str] = field(default_factory=list)


@dataclass
class MarketDataExportDbProofResult:
    status: str = STATUS_BLOCKED
    symbols_requested: list[str] = field(default_factory=list)
    symbols_exported: list[str] = field(default_factory=list)
    bars_root: str = ""
    output_paths: dict[str, str] = field(default_factory=dict)
    row_counts: dict[str, int] = field(default_factory=dict)
    csv_parse_passed_by_symbol: dict[str, bool] = field(default_factory=dict)
    first_end_ts_by_symbol: dict[str, Optional[int]] = field(default_factory=dict)
    last_end_ts_by_symbol: dict[str, Optional[int]] = field(default_factory=dict)
    symbol_inputs_status: Optional[str] = None
    paper_readiness_status: Optional[str] = None
    proof_report_path: Optional[str] = None
    database_url: str = ""
    approved_for_live: bool = False
    daemon_enforcement_executed: bool = False
    broker_calls_executed: bool = False
    db_writes_executed: bool = False
    failure_reasons: list[str] = field(default_factory=list)
    notes: str = ""

    def __post_init__(self) -> None:
        object.__setattr__(self, "approved_for_live", False)
        object.__setattr__(self, "daemon_enforcement_executed", False)
        object.__setattr__(self, "broker_calls_executed", False)
        object.__setattr__(self, "db_writes_executed", False)

    def to_dict(self) -> dict[str, Any]:
        return {
            "schema_version": SCHEMA_VERSION,
            "status": self.status,
            "database_url": self.database_url,
            "symbols_requested": list(self.symbols_requested),
            "symbols_exported": list(self.symbols_exported),
            "bars_root": self.bars_root,
            "output_paths": dict(self.output_paths),
            "row_counts": dict(self.row_counts),
            "csv_parse_passed_by_symbol": dict(self.csv_parse_passed_by_symbol),
            "first_end_ts_by_symbol": dict(self.first_end_ts_by_symbol),
            "last_end_ts_by_symbol": dict(self.last_end_ts_by_symbol),
            "symbol_inputs_status": self.symbol_inputs_status,
            "paper_readiness_status": self.paper_readiness_status,
            "proof_report_path": self.proof_report_path,
            "approved_for_live": False,
            "daemon_enforcement_executed": False,
            "broker_calls_executed": False,
            "db_writes_executed": False,
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


def redact_database_url(database_url: str) -> str:
    if not str(database_url or "").strip():
        return ""
    return REDACTED_DATABASE_URL


def _normalize_symbols(symbols: list[str]) -> list[str]:
    return _dedupe_preserve_order([s.strip().upper() for s in symbols if isinstance(s, str) and s.strip()])


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


def _proof_output_path(config: MarketDataExportDbProofConfig, filename: str) -> str:
    return str((Path(config.proof_report_path).expanduser().resolve().parent / filename).resolve())


def _blocked_result(
    config: MarketDataExportDbProofConfig,
    reasons: list[str],
    *,
    notes: Optional[str] = None,
) -> MarketDataExportDbProofResult:
    unique_reasons = _dedupe_preserve_order(reasons)
    return MarketDataExportDbProofResult(
        status=STATUS_BLOCKED,
        symbols_requested=_normalize_symbols(config.symbols),
        bars_root=config.bars_root,
        proof_report_path=config.proof_report_path or None,
        database_url=redact_database_url(config.database_url),
        failure_reasons=unique_reasons,
        notes=notes or f"blocked: {unique_reasons[0] if unique_reasons else STATUS_BLOCKED}",
    )


def inspect_exported_csv(path: Path, *, symbol: str, timeframe: str) -> CsvInspectionResult:
    result = CsvInspectionResult(symbol=symbol, path=str(path))
    if path.name != f"{symbol}_{timeframe}.csv":
        result.failure_reasons.append(f"{REASON_CSV_FILENAME_MISMATCH}:{symbol}")
        return result
    if not path.is_file():
        result.failure_reasons.append(f"{REASON_CSV_FILE_MISSING}:{symbol}")
        return result

    try:
        text = path.read_text(encoding="utf-8")
        rows = list(csv.reader(text.splitlines()))
    except (OSError, csv.Error):
        result.failure_reasons.append(f"{REASON_CSV_ROW_MALFORMED}:{symbol}")
        return result

    if not rows or tuple(rows[0]) != CSV_HEADER:
        result.failure_reasons.append(f"{REASON_CSV_HEADER_MISMATCH}:{symbol}")
        return result
    if len(rows) <= 1:
        result.failure_reasons.append(f"{REASON_CSV_NO_DATA_ROWS}:{symbol}")
        return result

    previous_end_ts: Optional[int] = None
    for raw_row in rows[1:]:
        if len(raw_row) != len(CSV_HEADER):
            result.failure_reasons.append(f"{REASON_CSV_ROW_MALFORMED}:{symbol}")
            return result
        row = dict(zip(CSV_HEADER, raw_row))
        if row["symbol"].strip().upper() != symbol:
            result.failure_reasons.append(f"{REASON_CSV_SYMBOL_MISMATCH}:{symbol}")
            return result
        try:
            end_ts = int(row["end_ts"])
            int(row["open_micros"])
            int(row["high_micros"])
            int(row["low_micros"])
            int(row["close_micros"])
            float(row["volume"])
        except ValueError:
            result.failure_reasons.append(f"{REASON_CSV_ROW_MALFORMED}:{symbol}")
            return result
        if previous_end_ts is not None and end_ts < previous_end_ts:
            result.failure_reasons.append(f"{REASON_CSV_TIMESTAMP_DESCENDING}:{symbol}")
            return result
        previous_end_ts = end_ts
        result.row_count += 1
        if result.first_end_ts is None:
            result.first_end_ts = end_ts
        result.last_end_ts = end_ts

    parsed_bars, parse_reason = normalize_bars_csv(text)
    if parse_reason is not None or not parsed_bars:
        result.failure_reasons.append(f"{REASON_CSV_PARSE_FAILED}:{symbol}")
        return result

    required_bar_fields = ("ts", "open", "high", "low", "close", "volume", "is_complete")
    for bar in parsed_bars:
        if not isinstance(bar, dict) or any(field_name not in bar for field_name in required_bar_fields):
            result.failure_reasons.append(f"{REASON_CSV_PARSE_FAILED}:{symbol}")
            return result

    result.passed = True
    return result


def write_proof_report(result: MarketDataExportDbProofResult, proof_report_path: str) -> Path:
    out = Path(proof_report_path).expanduser().resolve()
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result.to_dict(), indent=2, sort_keys=False, default=str), encoding="utf-8")
    return out


def _validate_config(config: MarketDataExportDbProofConfig) -> list[str]:
    reasons: list[str] = []
    if not str(config.database_url or "").strip():
        reasons.append(REASON_MISSING_DATABASE_URL)
    if not _normalize_symbols(config.symbols):
        reasons.append(REASON_NO_SYMBOLS_REQUESTED)
    if not str(config.bars_root or "").strip():
        reasons.append(REASON_BARS_ROOT_MISSING)
    if not str(config.proof_report_path or "").strip():
        reasons.append(REASON_PROOF_REPORT_PATH_MISSING)
    if config.run_paper_readiness:
        if not str(config.watchlist_path or "").strip():
            reasons.append(REASON_WATCHLIST_PATH_REQUIRED)
        if not str(config.strategy_fit_dir or "").strip():
            reasons.append(REASON_STRATEGY_FIT_DIR_REQUIRED)
    return reasons


def _run_symbol_inputs_stage(
    config: MarketDataExportDbProofConfig,
    symbols_exported: list[str],
) -> SymbolInputsRunnerResult:
    return run_symbol_inputs_producer(
        SymbolInputsRunnerConfig(
            trade_date=config.trade_date,
            output_path=_proof_output_path(config, "symbol_inputs_from_db_export.json"),
            bars_root=config.bars_root,
            timeframe=config.timeframe,
            symbols=list(symbols_exported),
            liquidity_root=config.liquidity_root,
            source="market_data_export_db_proof",
            generated_at_utc=config.end_utc or config.start_utc,
            now_utc=_parse_reference_utc(config.end_utc),
        )
    )


def _run_paper_readiness_stage(config: MarketDataExportDbProofConfig) -> PaperReadinessResult:
    return run_paper_readiness_pipeline(
        PaperReadinessConfig(
            trade_date=config.trade_date,
            paper_readiness_enabled=True,
            symbol_inputs_enabled=True,
            watchlist_promotion_enabled=True,
            operator_review_approved=bool(config.operator_review_approved),
            bars_root=config.bars_root,
            watchlist_path=config.watchlist_path,
            strategy_fit_dir=config.strategy_fit_dir,
            liquidity_root=config.liquidity_root,
            timeframe=config.timeframe,
            symbol_inputs_output_path=_proof_output_path(config, "paper_readiness_symbol_inputs.json"),
            premarket_output_path=_proof_output_path(config, "paper_readiness_premarket_revalidation.json"),
            promoted_watchlist_output_path=_proof_output_path(config, "paper_readiness_promoted_watchlist.json"),
            readiness_report_output_path=_proof_output_path(config, "paper_readiness_report.json"),
            generated_at_utc=config.end_utc or config.start_utc,
            now_utc=_parse_reference_utc(config.end_utc),
        )
    )


def _status_after_optional_stages(
    export_result: MarketDataExportResult,
    *,
    symbol_inputs_status: Optional[str],
) -> str:
    if export_result.status == EXPORT_STATUS_PARTIAL or symbol_inputs_status == STATUS_PARTIAL:
        return STATUS_PARTIAL
    return STATUS_COMPLETE


def run_market_data_export_db_proof(
    config: MarketDataExportDbProofConfig,
) -> MarketDataExportDbProofResult:
    config_reasons = _validate_config(config)
    if config_reasons:
        result = _blocked_result(config, config_reasons)
        if config.proof_report_path:
            try:
                write_proof_report(result, config.proof_report_path)
            except OSError:
                pass
        return result

    export_result = export_md_bars_to_bars_root(
        MarketDataExportConfig(
            database_url=config.database_url,
            symbols=_normalize_symbols(config.symbols),
            timeframe=config.timeframe,
            start_utc=config.start_utc,
            end_utc=config.end_utc,
            bars_root=config.bars_root,
            trade_date=config.trade_date,
        )
    )

    base_result = MarketDataExportDbProofResult(
        status=STATUS_BLOCKED,
        database_url=redact_database_url(config.database_url),
        symbols_requested=list(export_result.symbols_requested),
        symbols_exported=list(export_result.symbols_exported),
        bars_root=config.bars_root,
        output_paths=dict(export_result.output_paths),
        row_counts=dict(export_result.bars_written_by_symbol),
        proof_report_path=config.proof_report_path,
        failure_reasons=list(export_result.failure_reasons),
        notes=export_result.notes,
    )

    if export_result.status == EXPORT_STATUS_BLOCKED:
        base_result.failure_reasons = _dedupe_preserve_order(
            [REASON_EXPORT_BLOCKED] + base_result.failure_reasons
        )
        base_result.notes = f"blocked: {REASON_EXPORT_BLOCKED}"
        try:
            write_proof_report(base_result, config.proof_report_path)
        except OSError:
            base_result.failure_reasons = _dedupe_preserve_order(
                base_result.failure_reasons + [REASON_PROOF_REPORT_WRITE_FAILED]
            )
        return base_result

    if not export_result.symbols_exported:
        base_result.failure_reasons = _dedupe_preserve_order(
            [REASON_NO_FILES_EXPORTED] + base_result.failure_reasons
        )
        base_result.notes = f"blocked: {REASON_NO_FILES_EXPORTED}"
        try:
            write_proof_report(base_result, config.proof_report_path)
        except OSError:
            base_result.failure_reasons = _dedupe_preserve_order(
                base_result.failure_reasons + [REASON_PROOF_REPORT_WRITE_FAILED]
            )
        return base_result

    csv_failures: list[str] = []
    for symbol in export_result.symbols_exported:
        output_path = export_result.output_paths.get(symbol)
        if not output_path:
            csv_failures.append(f"{REASON_CSV_FILE_MISSING}:{symbol}")
            base_result.csv_parse_passed_by_symbol[symbol] = False
            continue
        inspection = inspect_exported_csv(Path(output_path), symbol=symbol, timeframe=config.timeframe)
        base_result.csv_parse_passed_by_symbol[symbol] = inspection.passed
        base_result.row_counts[symbol] = inspection.row_count
        base_result.first_end_ts_by_symbol[symbol] = inspection.first_end_ts
        base_result.last_end_ts_by_symbol[symbol] = inspection.last_end_ts
        csv_failures.extend(inspection.failure_reasons)

    if csv_failures:
        base_result.failure_reasons = _dedupe_preserve_order(base_result.failure_reasons + csv_failures)
        base_result.notes = "blocked: exported CSV inspection failed"
        try:
            write_proof_report(base_result, config.proof_report_path)
        except OSError:
            base_result.failure_reasons = _dedupe_preserve_order(
                base_result.failure_reasons + [REASON_PROOF_REPORT_WRITE_FAILED]
            )
        return base_result

    symbol_inputs_status: Optional[str] = None
    if config.run_symbol_inputs:
        symbol_inputs_result = _run_symbol_inputs_stage(config, export_result.symbols_exported)
        symbol_inputs_status = symbol_inputs_result.status
        base_result.symbol_inputs_status = symbol_inputs_status
        if symbol_inputs_result.status == STATUS_BLOCKED:
            base_result.failure_reasons = _dedupe_preserve_order(
                base_result.failure_reasons
                + [REASON_SYMBOL_INPUTS_BLOCKED]
                + list(symbol_inputs_result.failure_reasons)
            )
            base_result.notes = f"blocked: {REASON_SYMBOL_INPUTS_BLOCKED}"
            try:
                write_proof_report(base_result, config.proof_report_path)
            except OSError:
                base_result.failure_reasons = _dedupe_preserve_order(
                    base_result.failure_reasons + [REASON_PROOF_REPORT_WRITE_FAILED]
                )
            return base_result

    if config.run_paper_readiness:
        paper_result = _run_paper_readiness_stage(config)
        base_result.paper_readiness_status = paper_result.status
        if paper_result.status not in (STATUS_READY_FOR_OPERATOR_REVIEW, STATUS_READY_FOR_PAPER_HANDOFF):
            base_result.failure_reasons = _dedupe_preserve_order(
                base_result.failure_reasons
                + [REASON_PAPER_READINESS_BLOCKED]
                + list(paper_result.reasons)
            )
            base_result.notes = f"blocked: {REASON_PAPER_READINESS_BLOCKED}"
            try:
                write_proof_report(base_result, config.proof_report_path)
            except OSError:
                base_result.failure_reasons = _dedupe_preserve_order(
                    base_result.failure_reasons + [REASON_PROOF_REPORT_WRITE_FAILED]
                )
            return base_result

    base_result.status = _status_after_optional_stages(
        export_result,
        symbol_inputs_status=symbol_inputs_status,
    )
    base_result.notes = (
        f"{base_result.status}: exported {len(export_result.symbols_exported)} symbol(s); "
        "CSV parse proof passed"
    )
    try:
        write_proof_report(base_result, config.proof_report_path)
    except OSError:
        base_result.status = STATUS_BLOCKED
        base_result.failure_reasons = _dedupe_preserve_order(
            base_result.failure_reasons + [REASON_PROOF_REPORT_WRITE_FAILED]
        )
        base_result.notes = f"blocked: {REASON_PROOF_REPORT_WRITE_FAILED}"
    return base_result


def _split_symbols(raw: str) -> list[str]:
    return [item.strip() for item in str(raw or "").split(",") if item.strip()]


def _resolve_database_url(arg_value: Optional[str]) -> str:
    if arg_value and str(arg_value).strip():
        return str(arg_value).strip()
    return os.environ.get("MQK_PAPER_DB_URL", "").strip()


def _build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python scripts/prove_market_data_export_db.py",
        description=(
            "Run a local proof for the read-only md_bars exporter and its "
            "scanner CSV consumers. Writes only caller-local proof artifacts."
        ),
    )
    parser.add_argument("--database-url", dest="database_url", default=None)
    parser.add_argument("--symbols", dest="symbols", required=True)
    parser.add_argument("--timeframe", dest="timeframe", required=True)
    parser.add_argument("--start-utc", dest="start_utc", default=None)
    parser.add_argument("--end-utc", dest="end_utc", default=None)
    parser.add_argument("--bars-root", dest="bars_root", required=True)
    parser.add_argument("--trade-date", dest="trade_date", required=True)
    parser.add_argument("--proof-report-path", dest="proof_report_path", required=True)
    parser.add_argument("--watchlist-path", dest="watchlist_path", default=None)
    parser.add_argument("--strategy-fit-dir", dest="strategy_fit_dir", default=None)
    parser.add_argument("--liquidity-root", dest="liquidity_root", default=None)
    parser.add_argument("--run-symbol-inputs", dest="run_symbol_inputs", action="store_true")
    parser.add_argument("--run-paper-readiness", dest="run_paper_readiness", action="store_true")
    parser.add_argument("--operator-review-approved", dest="operator_review_approved", action="store_true")
    return parser


def main(argv: Optional[list[str]] = None) -> int:
    parser = _build_arg_parser()
    args = parser.parse_args(argv)
    config = MarketDataExportDbProofConfig(
        database_url=_resolve_database_url(args.database_url),
        symbols=_split_symbols(args.symbols),
        timeframe=args.timeframe,
        start_utc=args.start_utc,
        end_utc=args.end_utc,
        bars_root=args.bars_root,
        trade_date=args.trade_date,
        proof_report_path=args.proof_report_path,
        watchlist_path=args.watchlist_path,
        strategy_fit_dir=args.strategy_fit_dir,
        liquidity_root=args.liquidity_root,
        run_symbol_inputs=args.run_symbol_inputs,
        run_paper_readiness=args.run_paper_readiness,
        operator_review_approved=args.operator_review_approved,
    )
    result = run_market_data_export_db_proof(config)
    print(json.dumps(result.to_dict(), indent=2, sort_keys=False, default=str))
    return 0 if result.status in (STATUS_COMPLETE, STATUS_PARTIAL) else 1


if __name__ == "__main__":
    sys.exit(main())
