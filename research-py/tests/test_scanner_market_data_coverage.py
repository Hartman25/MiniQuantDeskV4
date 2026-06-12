"""
MARKET-DATA-COVERAGE-01 tests.

Pure unit coverage for the read-only md_bars coverage report. Uses a fake
SQLAlchemy engine; no real DB, daemon, network, subprocess, or broker/API
calls.
"""
from __future__ import annotations

import json
import unittest
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from unittest.mock import patch

from mqk_research.scanner import market_data_coverage as mdc
from mqk_research.scanner.market_data_coverage import (
    REASON_DATABASE_QUERY_FAILED,
    REASON_MISSING_DATABASE_URL,
    REASON_NO_COVERAGE_DATA,
    REASON_NO_ROWS_FOR_SYMBOL,
    REASON_NO_SYMBOLS_REQUESTED,
    REASON_NO_TIMEFRAMES_REQUESTED,
    REASON_STALE_DATA,
    SCHEMA_VERSION,
    STATUS_BLOCKED,
    STATUS_COMPLETE,
    STATUS_PARTIAL,
    MarketDataCoverageConfig,
    MarketDataCoverageEntry,
    MarketDataCoverageResult,
    build_select_md_bars_coverage_statement,
    run_market_data_coverage_report,
)


def _cov_row(symbol: str, timeframe: str, end_ts: int) -> dict[str, Any]:
    return {"symbol": symbol, "timeframe": timeframe, "end_ts": end_ts}


class _FakeResult:
    def __init__(self, rows: list[dict[str, Any]]):
        self._rows = rows

    def fetchall(self) -> list[dict[str, Any]]:
        return list(self._rows)


class _FakeEngine:
    def __init__(self, rows: list[dict[str, Any]], *, fail: bool = False):
        self.rows = rows
        self.fail = fail
        self.statements: list[str] = []
        self.params: list[dict[str, Any]] = []

    def connect(self) -> "_FakeEngine":
        return self

    def __enter__(self) -> "_FakeEngine":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        return None

    def execute(self, statement: Any, params: dict[str, Any]) -> _FakeResult:
        self.statements.append(str(statement))
        self.params.append(dict(params))
        if self.fail:
            raise RuntimeError("fake query failed")
        return _FakeResult(self.rows)


def _run_with_fake_engine(config: MarketDataCoverageConfig, engine: _FakeEngine) -> MarketDataCoverageResult:
    with patch.object(mdc, "make_engine", return_value=engine):
        return run_market_data_coverage_report(config)


def _epoch(year: int, month: int, day: int, hour: int = 0, minute: int = 0) -> int:
    return int(datetime(year, month, day, hour, minute, tzinfo=timezone.utc).timestamp())


class TestCoverageHardInvariants(unittest.TestCase):
    def test_config_forces_approved_for_live_false(self) -> None:
        config = MarketDataCoverageConfig(
            database_url="postgresql://example",
            symbols=["AAPL"],
            timeframes=["5m"],
            **{"approved_for_live": bool(1)},
        )
        self.assertFalse(config.approved_for_live)

    def test_entry_forces_approved_for_live_false(self) -> None:
        entry = MarketDataCoverageEntry(symbol="AAPL", timeframe="5m", **{"approved_for_live": bool(1)})
        self.assertFalse(entry.approved_for_live)
        self.assertFalse(entry.to_dict()["approved_for_live"])

    def test_result_forces_approved_for_live_false(self) -> None:
        result = MarketDataCoverageResult(**{"approved_for_live": bool(1)})
        self.assertFalse(result.approved_for_live)
        self.assertFalse(result.to_dict()["approved_for_live"])


class TestCoverageBlocksFailClosed(unittest.TestCase):
    def test_missing_database_url_blocks(self) -> None:
        result = run_market_data_coverage_report(
            MarketDataCoverageConfig(symbols=["AAPL"], timeframes=["5m"])
        )

        self.assertEqual(result.status, STATUS_BLOCKED)
        self.assertIn(REASON_MISSING_DATABASE_URL, result.failure_reasons)

    def test_empty_symbols_blocks(self) -> None:
        result = run_market_data_coverage_report(
            MarketDataCoverageConfig(database_url="postgresql://example", symbols=[], timeframes=["5m"])
        )

        self.assertEqual(result.status, STATUS_BLOCKED)
        self.assertIn(REASON_NO_SYMBOLS_REQUESTED, result.failure_reasons)

    def test_empty_timeframes_blocks(self) -> None:
        result = run_market_data_coverage_report(
            MarketDataCoverageConfig(database_url="postgresql://example", symbols=["AAPL"], timeframes=[])
        )

        self.assertEqual(result.status, STATUS_BLOCKED)
        self.assertIn(REASON_NO_TIMEFRAMES_REQUESTED, result.failure_reasons)

    def test_database_query_failure_blocks(self) -> None:
        engine = _FakeEngine([], fail=True)
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(database_url="postgresql://example", symbols=["AAPL"], timeframes=["5m"]),
            engine,
        )

        self.assertEqual(result.status, STATUS_BLOCKED)
        self.assertEqual(result.failure_reasons, [REASON_DATABASE_QUERY_FAILED])

    def test_no_rows_for_any_requested_combo_blocks(self) -> None:
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(database_url="postgresql://example", symbols=["AAPL"], timeframes=["5m"]),
            _FakeEngine([]),
        )

        self.assertEqual(result.status, STATUS_BLOCKED)
        self.assertEqual(result.failure_reasons, [REASON_NO_COVERAGE_DATA])


class TestCoverageStatuses(unittest.TestCase):
    def test_all_combos_present_and_fresh_is_complete(self) -> None:
        now_utc = datetime(2026, 6, 12, 14, 0, tzinfo=timezone.utc)
        rows = [
            _cov_row("AAPL", "5m", _epoch(2026, 6, 12, 13, 55)),
            _cov_row("MSFT", "5m", _epoch(2026, 6, 12, 13, 55)),
            _cov_row("AAPL", "1D", _epoch(2026, 6, 11)),
            _cov_row("MSFT", "1D", _epoch(2026, 6, 11)),
        ]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL", "MSFT"],
                timeframes=["5m", "1D"],
                assume_market_hours=True,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )

        self.assertEqual(result.status, STATUS_COMPLETE)
        self.assertEqual(result.failure_reasons, [])
        self.assertEqual(sorted(result.symbols_ready_by_timeframe["5m"]), ["AAPL", "MSFT"])
        self.assertEqual(sorted(result.symbols_ready_by_timeframe["1D"]), ["AAPL", "MSFT"])
        self.assertEqual(result.symbols_missing_by_timeframe["5m"], [])
        self.assertEqual(result.symbols_missing_by_timeframe["1D"], [])
        for entry in result.entries:
            self.assertTrue(entry.is_fresh, msg=entry)
            self.assertTrue(entry.ready_for_backtest, msg=entry)
            self.assertTrue(entry.ready_for_paper, msg=entry)

    def test_one_missing_symbol_is_partial(self) -> None:
        now_utc = datetime(2026, 6, 12, 14, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "5m", _epoch(2026, 6, 12, 13, 55))]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL", "MSFT"],
                timeframes=["5m"],
                assume_market_hours=True,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )

        self.assertEqual(result.status, STATUS_PARTIAL)
        self.assertEqual(result.symbols_ready_by_timeframe["5m"], ["AAPL"])
        self.assertEqual(result.symbols_missing_by_timeframe["5m"], ["MSFT"])
        self.assertIn(f"{REASON_NO_ROWS_FOR_SYMBOL}:MSFT:5m", result.failure_reasons)

        missing_entry = next(e for e in result.entries if e.symbol == "MSFT")
        self.assertEqual(missing_entry.row_count, 0)
        self.assertEqual(missing_entry.missing_reason, f"{REASON_NO_ROWS_FOR_SYMBOL}:MSFT:5m")
        self.assertFalse(missing_entry.ready_for_backtest)
        self.assertFalse(missing_entry.ready_for_paper)
        self.assertIsNone(missing_entry.first_end_ts)
        self.assertIsNone(missing_entry.latest_end_ts)


class TestCoverageFreshness(unittest.TestCase):
    def test_1d_within_threshold_is_fresh(self) -> None:
        now_utc = datetime(2026, 6, 12, 12, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "1D", _epoch(2026, 6, 10))]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["1D"],
                max_staleness_days_1d=4.0,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )

        entry = result.entries[0]
        self.assertTrue(entry.is_fresh)
        self.assertTrue(entry.ready_for_paper)
        self.assertNotIn(f"{REASON_STALE_DATA}:AAPL:1D", result.failure_reasons)

    def test_1d_beyond_threshold_is_stale(self) -> None:
        now_utc = datetime(2026, 6, 12, 12, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "1D", _epoch(2026, 6, 1))]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["1D"],
                max_staleness_days_1d=4.0,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )

        entry = result.entries[0]
        self.assertFalse(entry.is_fresh)
        self.assertTrue(entry.ready_for_backtest)
        self.assertFalse(entry.ready_for_paper)
        self.assertIn(f"{REASON_STALE_DATA}:AAPL:1D", result.failure_reasons)

    def test_5m_during_market_hours_beyond_threshold_is_stale(self) -> None:
        now_utc = datetime(2026, 6, 12, 14, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "5m", _epoch(2026, 6, 12, 13, 30))]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["5m"],
                max_staleness_minutes_5m=15.0,
                assume_market_hours=True,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )

        entry = result.entries[0]
        self.assertFalse(entry.is_fresh)
        self.assertFalse(entry.ready_for_paper)

    def test_5m_outside_market_hours_is_fresh_regardless_of_staleness(self) -> None:
        now_utc = datetime(2026, 6, 12, 14, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "5m", _epoch(2026, 6, 11, 13, 30))]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["5m"],
                max_staleness_minutes_5m=15.0,
                assume_market_hours=False,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )

        entry = result.entries[0]
        self.assertTrue(entry.is_fresh)
        self.assertTrue(entry.ready_for_paper)

    def test_assume_market_hours_overrides_heuristic(self) -> None:
        # Saturday: the built-in heuristic would treat this as off-hours.
        now_utc = datetime(2026, 6, 13, 14, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "5m", _epoch(2026, 6, 13, 13, 30))]

        result_forced_open = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["5m"],
                max_staleness_minutes_5m=15.0,
                assume_market_hours=True,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )
        self.assertFalse(result_forced_open.entries[0].is_fresh)

        result_default = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["5m"],
                max_staleness_minutes_5m=15.0,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )
        self.assertTrue(result_default.entries[0].is_fresh)

    def test_1h_during_market_hours_beyond_threshold_is_stale(self) -> None:
        now_utc = datetime(2026, 6, 12, 14, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "1h", _epoch(2026, 6, 12, 12, 0))]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["1h"],
                max_staleness_minutes_1h=90.0,
                assume_market_hours=True,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )

        entry = result.entries[0]
        self.assertFalse(entry.is_fresh)
        self.assertFalse(entry.ready_for_paper)

    def test_1h_outside_market_hours_is_fresh_regardless_of_staleness(self) -> None:
        now_utc = datetime(2026, 6, 12, 14, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "1h", _epoch(2026, 6, 11, 13, 30))]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["1h"],
                max_staleness_minutes_1h=90.0,
                assume_market_hours=False,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )

        entry = result.entries[0]
        self.assertTrue(entry.is_fresh)
        self.assertTrue(entry.ready_for_paper)

    def test_1h_freshness_default_and_configurable_threshold(self) -> None:
        now_utc = datetime(2026, 6, 12, 14, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "1h", _epoch(2026, 6, 12, 12, 29))]  # 91 minutes stale

        result_default = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["1h"],
                assume_market_hours=True,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )
        self.assertFalse(result_default.entries[0].is_fresh)

        result_relaxed = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["1h"],
                max_staleness_minutes_1h=120.0,
                assume_market_hours=True,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )
        self.assertTrue(result_relaxed.entries[0].is_fresh)


class TestCoverageGapCount(unittest.TestCase):
    def test_1d_gap_within_tolerance_is_zero(self) -> None:
        now_utc = datetime(2026, 6, 12, 12, 0, tzinfo=timezone.utc)
        rows = [
            _cov_row("AAPL", "1D", _epoch(2026, 6, 5)),
            _cov_row("AAPL", "1D", _epoch(2026, 6, 8)),  # weekend gap, 3 days
            _cov_row("AAPL", "1D", _epoch(2026, 6, 9)),
        ]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["1D"],
                max_expected_gap_days_1d=4.0,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )

        self.assertEqual(result.entries[0].gap_count, 0)

    def test_1d_gap_beyond_tolerance_is_counted(self) -> None:
        now_utc = datetime(2026, 6, 12, 12, 0, tzinfo=timezone.utc)
        rows = [
            _cov_row("AAPL", "1D", _epoch(2026, 5, 1)),
            _cov_row("AAPL", "1D", _epoch(2026, 5, 11)),  # 10-day gap
            _cov_row("AAPL", "1D", _epoch(2026, 5, 12)),
        ]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["1D"],
                max_expected_gap_days_1d=4.0,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )

        self.assertEqual(result.entries[0].gap_count, 1)

    def test_5m_gap_count_is_not_computable(self) -> None:
        now_utc = datetime(2026, 6, 12, 14, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "5m", _epoch(2026, 6, 12, 13, 55))]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["5m"],
                assume_market_hours=True,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )

        self.assertIsNone(result.entries[0].gap_count)

    def test_1h_gap_count_is_not_computable(self) -> None:
        now_utc = datetime(2026, 6, 12, 14, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "1h", _epoch(2026, 6, 12, 13, 0))]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["1h"],
                assume_market_hours=True,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )

        self.assertIsNone(result.entries[0].gap_count)


class TestCoverageMinRowsForBacktest(unittest.TestCase):
    def test_row_count_below_minimum_is_not_ready_for_backtest(self) -> None:
        now_utc = datetime(2026, 6, 12, 14, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "5m", _epoch(2026, 6, 12, 13, 55))]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["5m"],
                min_rows_for_backtest=5,
                assume_market_hours=True,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )

        entry = result.entries[0]
        self.assertEqual(entry.row_count, 1)
        self.assertFalse(entry.ready_for_backtest)
        self.assertFalse(entry.ready_for_paper)


class TestCoverageSchema(unittest.TestCase):
    def test_top_level_schema_fields_present(self) -> None:
        now_utc = datetime(2026, 6, 12, 14, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "5m", _epoch(2026, 6, 12, 13, 55))]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["5m"],
                assume_market_hours=True,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )
        payload = result.to_dict()

        self.assertEqual(payload["schema_version"], SCHEMA_VERSION)
        self.assertEqual(SCHEMA_VERSION, "market-data-coverage-v1")
        for key in (
            "generated_at_utc",
            "symbols_requested",
            "timeframes_requested",
            "symbols_ready_by_timeframe",
            "symbols_missing_by_timeframe",
            "approved_for_live",
            "status",
            "failure_reasons",
        ):
            self.assertIn(key, payload)
        self.assertFalse(payload["approved_for_live"])
        self.assertIn(payload["status"], (STATUS_COMPLETE, STATUS_PARTIAL, STATUS_BLOCKED))

    def test_per_entry_schema_fields_present(self) -> None:
        now_utc = datetime(2026, 6, 12, 14, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "5m", _epoch(2026, 6, 12, 13, 55))]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["5m"],
                assume_market_hours=True,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )
        entry_payload = result.entries[0].to_dict()

        for key in (
            "symbol",
            "timeframe",
            "row_count",
            "first_end_ts",
            "latest_end_ts",
            "latest_end_utc",
            "staleness_minutes",
            "is_fresh",
            "gap_count",
            "missing_reason",
            "ready_for_backtest",
            "ready_for_paper",
            "failure_reasons",
            "approved_for_live",
        ):
            self.assertIn(key, entry_payload)
        self.assertFalse(entry_payload["approved_for_live"])

    def test_result_is_json_serializable(self) -> None:
        now_utc = datetime(2026, 6, 12, 14, 0, tzinfo=timezone.utc)
        rows = [_cov_row("AAPL", "5m", _epoch(2026, 6, 12, 13, 55))]
        result = _run_with_fake_engine(
            MarketDataCoverageConfig(
                database_url="postgresql://example",
                symbols=["AAPL"],
                timeframes=["5m"],
                assume_market_hours=True,
                now_utc=now_utc,
            ),
            _FakeEngine(rows),
        )

        json.dumps(result.to_dict())


class TestCoveragePathAndSqlSafety(unittest.TestCase):
    def test_query_uses_parameterized_inputs(self) -> None:
        sql = str(build_select_md_bars_coverage_statement())

        self.assertIn(":symbols", sql)
        self.assertIn(":timeframes", sql)
        self.assertNotIn("{symbols}", sql)
        self.assertNotIn("{timeframes}", sql)

    def test_query_contains_no_db_write_sql(self) -> None:
        sql_upper = str(build_select_md_bars_coverage_statement()).upper()
        for forbidden in ("INSERT ", "UPDATE ", "DELETE ", "DROP ", "ALTER ", "TRUNCATE ", "CREATE TABLE"):
            self.assertNotIn(forbidden, sql_upper)

    def test_fetch_uses_normalized_database_url(self) -> None:
        engine = _FakeEngine([_cov_row("AAPL", "5m", _epoch(2026, 6, 12, 13, 55))])
        captured: dict[str, Any] = {}

        def _fake_make_engine(cfg: Any) -> _FakeEngine:
            captured["url"] = cfg.url
            return engine

        with patch.object(mdc, "make_engine", side_effect=_fake_make_engine):
            run_market_data_coverage_report(
                MarketDataCoverageConfig(
                    database_url="postgres://user:pass@127.0.0.1:5440/miniquantdesk_paper",
                    symbols=["AAPL"],
                    timeframes=["5m"],
                    assume_market_hours=True,
                    now_utc=datetime(2026, 6, 12, 14, 0, tzinfo=timezone.utc),
                )
            )

        self.assertEqual(captured["url"], "postgresql+psycopg://user:pass@127.0.0.1:5440/miniquantdesk_paper")

    def test_module_has_no_forbidden_imports_or_references(self) -> None:
        source = Path(mdc.__file__).read_text(encoding="utf-8")
        for forbidden in (
            "import subprocess",
            "os.system",
            "import requests",
            "import urllib",
            "import http.client",
            "import aiohttp",
            "import httpx",
            "from mqk_daemon",
            "import mqk_daemon",
            "from mqk_runtime",
            "import mqk_runtime",
            "from mqk_execution",
            "import mqk_execution",
            "oms_outbox",
            "oms_inbox",
            "/api/v1/strategy/signal",
            "strategy/signal",
            "/v2/orders",
            "submit_order",
            "place_order",
            "approved_for_live=True",
            "recommended_for_live=True",
            "eligible_for_live=True",
        ):
            self.assertNotIn(forbidden, source, msg=forbidden)

    def test_cli_prints_blocked_json_without_database_url(self) -> None:
        with patch("sys.stdout") as fake_stdout:
            exit_code = mdc.main(["--symbols", "AAPL", "--timeframes", "5m"])
        printed = "".join(call.args[0] for call in fake_stdout.write.call_args_list if call.args)
        payload = json.loads(printed)

        self.assertEqual(exit_code, 1)
        self.assertEqual(payload["status"], STATUS_BLOCKED)
        self.assertNotIn("postgres", printed)


if __name__ == "__main__":
    unittest.main()
