"""
MARKET-DATA-EXPORT-01 tests.

Pure unit coverage for the read-only md_bars exporter. Uses a fake SQLAlchemy
engine; no real DB, daemon, network, subprocess, or broker/API calls.
"""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from typing import Any
from unittest.mock import patch

from mqk_research.scanner import market_data_export as mde
from mqk_research.scanner.market_data_export import (
    REASON_BARS_ROOT_INVALID,
    REASON_DATABASE_QUERY_FAILED,
    REASON_INVALID_SYMBOL,
    REASON_MISSING_DATABASE_URL,
    REASON_NO_ROWS_EXPORTED,
    REASON_NO_ROWS_FOR_SYMBOL,
    REASON_NO_SYMBOLS_REQUESTED,
    STATUS_BLOCKED,
    STATUS_COMPLETE,
    STATUS_PARTIAL,
    MarketDataExportConfig,
    MarketDataExportResult,
    build_select_md_bars_statement,
    export_md_bars_to_bars_root,
    normalize_database_url,
)
from mqk_research.scanner.symbol_inputs_runner import normalize_bars_csv


def _row(
    symbol: str,
    end_ts: int,
    *,
    open_micros: int = 100_000_000,
    high_micros: int = 101_000_000,
    low_micros: int = 99_000_000,
    close_micros: int = 100_500_000,
    volume: int = 1_000_000,
    is_complete: bool = True,
) -> dict[str, Any]:
    return {
        "symbol": symbol,
        "timeframe": "5m",
        "end_ts": end_ts,
        "open_micros": open_micros,
        "high_micros": high_micros,
        "low_micros": low_micros,
        "close_micros": close_micros,
        "volume": volume,
        "is_complete": is_complete,
    }


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


def _run_with_fake_engine(config: MarketDataExportConfig, engine: _FakeEngine) -> MarketDataExportResult:
    with patch.object(mde, "make_engine", return_value=engine):
        return export_md_bars_to_bars_root(config)


class TestExportHardInvariants(unittest.TestCase):
    def test_config_forces_approved_for_live_false(self) -> None:
        config = MarketDataExportConfig(
            database_url="postgresql://example",
            symbols=["AAPL"],
            bars_root="unused",
            **{"approved_for_live": bool(1)},
        )
        self.assertFalse(config.approved_for_live)

    def test_result_forces_approved_for_live_false(self) -> None:
        result = MarketDataExportResult(**{"approved_for_live": bool(1)})
        self.assertFalse(result.approved_for_live)
        self.assertFalse(result.to_dict()["approved_for_live"])


class TestDatabaseUrlNormalization(unittest.TestCase):
    def test_bare_postgres_scheme_is_rewritten_to_psycopg(self) -> None:
        self.assertEqual(
            normalize_database_url("postgres://user:pass@127.0.0.1:5440/miniquantdesk_paper"),
            "postgresql+psycopg://user:pass@127.0.0.1:5440/miniquantdesk_paper",
        )

    def test_bare_postgresql_scheme_is_rewritten_to_psycopg(self) -> None:
        self.assertEqual(
            normalize_database_url("postgresql://user:pass@127.0.0.1:5440/miniquantdesk_paper"),
            "postgresql+psycopg://user:pass@127.0.0.1:5440/miniquantdesk_paper",
        )

    def test_psycopg_driver_already_specified_is_unchanged(self) -> None:
        url = "postgresql+psycopg://user:pass@127.0.0.1:5440/miniquantdesk_paper"
        self.assertEqual(normalize_database_url(url), url)

    def test_psycopg2_driver_already_specified_is_unchanged(self) -> None:
        url = "postgresql+psycopg2://user:pass@127.0.0.1:5440/miniquantdesk_paper"
        self.assertEqual(normalize_database_url(url), url)

    def test_non_postgres_scheme_is_unchanged(self) -> None:
        url = "sqlite:///tmp/example.db"
        self.assertEqual(normalize_database_url(url), url)

    def test_empty_url_is_unchanged(self) -> None:
        self.assertEqual(normalize_database_url(""), "")


class TestExportBlocksFailClosed(unittest.TestCase):
    def test_missing_database_url_blocks_without_creating_bars_root(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bars_root = Path(tmp) / "bars"
            result = export_md_bars_to_bars_root(
                MarketDataExportConfig(symbols=["AAPL"], bars_root=str(bars_root))
            )

            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertIn(REASON_MISSING_DATABASE_URL, result.failure_reasons)
            self.assertFalse(bars_root.exists())

    def test_empty_symbols_list_blocks(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            result = export_md_bars_to_bars_root(
                MarketDataExportConfig(database_url="postgresql://example", bars_root=str(Path(tmp) / "bars"))
            )

            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertIn(REASON_NO_SYMBOLS_REQUESTED, result.failure_reasons)

    def test_invalid_bars_root_file_blocks(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root_file = Path(tmp) / "not_a_dir"
            root_file.write_text("x", encoding="utf-8")
            engine = _FakeEngine([_row("AAPL", 100)])
            result = _run_with_fake_engine(
                MarketDataExportConfig(
                    database_url="postgresql://example",
                    symbols=["AAPL"],
                    bars_root=str(root_file),
                ),
                engine,
            )

            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertIn(REASON_BARS_ROOT_INVALID, result.failure_reasons)
            self.assertEqual(engine.statements, [])

    def test_database_query_failure_blocks(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            engine = _FakeEngine([], fail=True)
            result = _run_with_fake_engine(
                MarketDataExportConfig(
                    database_url="postgresql://example",
                    symbols=["AAPL"],
                    bars_root=str(Path(tmp) / "bars"),
                ),
                engine,
            )

            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertEqual(result.failure_reasons, [REASON_DATABASE_QUERY_FAILED])

    def test_all_symbols_without_rows_blocks_without_fabricating_files(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bars_root = Path(tmp) / "bars"
            result = _run_with_fake_engine(
                MarketDataExportConfig(
                    database_url="postgresql://example",
                    symbols=["AAPL"],
                    bars_root=str(bars_root),
                ),
                _FakeEngine([]),
            )

            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertIn(REASON_NO_ROWS_EXPORTED, result.failure_reasons)
            self.assertEqual(result.symbols_missing, ["AAPL"])
            self.assertEqual(list(bars_root.glob("*.csv")), [])


class TestExportWritesCsv(unittest.TestCase):
    def test_rows_for_one_symbol_write_symbol_timeframe_csv(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            result = _run_with_fake_engine(
                MarketDataExportConfig(
                    database_url="postgresql://example",
                    symbols=["AAPL"],
                    bars_root=str(Path(tmp) / "bars"),
                ),
                _FakeEngine([_row("AAPL", 200), _row("AAPL", 100)]),
            )

            self.assertEqual(result.status, STATUS_COMPLETE)
            self.assertEqual(result.symbols_exported, ["AAPL"])
            self.assertEqual(result.bars_written_by_symbol, {"AAPL": 2})
            output_path = Path(result.output_paths["AAPL"])
            self.assertEqual(output_path.name, "AAPL_5m.csv")
            self.assertTrue(output_path.is_file())

    def test_rows_for_multiple_symbols_write_one_file_per_symbol(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            result = _run_with_fake_engine(
                MarketDataExportConfig(
                    database_url="postgresql://example",
                    symbols=["MSFT", "AAPL"],
                    bars_root=str(Path(tmp) / "bars"),
                ),
                _FakeEngine([_row("AAPL", 100), _row("MSFT", 100)]),
            )

            self.assertEqual(result.status, STATUS_COMPLETE)
            self.assertEqual(result.symbols_exported, ["MSFT", "AAPL"])
            self.assertEqual(set(result.output_paths), {"AAPL", "MSFT"})
            self.assertTrue(Path(result.output_paths["AAPL"]).is_file())
            self.assertTrue(Path(result.output_paths["MSFT"]).is_file())

    def test_rows_are_written_timestamp_ascending(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            result = _run_with_fake_engine(
                MarketDataExportConfig(
                    database_url="postgresql://example",
                    symbols=["AAPL"],
                    bars_root=str(Path(tmp) / "bars"),
                ),
                _FakeEngine([_row("AAPL", 300), _row("AAPL", 100), _row("AAPL", 200)]),
            )

            lines = Path(result.output_paths["AAPL"]).read_text(encoding="utf-8").splitlines()
            self.assertEqual([line.split(",")[1] for line in lines[1:]], ["100", "200", "300"])

    def test_1d_timeframe_writes_symbol_1d_csv(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            result = _run_with_fake_engine(
                MarketDataExportConfig(
                    database_url="postgresql://example",
                    symbols=["AAPL"],
                    timeframe="1D",
                    bars_root=str(Path(tmp) / "bars"),
                ),
                _FakeEngine([_row("AAPL", 100), _row("AAPL", 200)]),
            )

            self.assertEqual(result.status, STATUS_COMPLETE)
            self.assertEqual(result.symbols_exported, ["AAPL"])
            self.assertEqual(result.bars_written_by_symbol, {"AAPL": 2})
            output_path = Path(result.output_paths["AAPL"])
            self.assertEqual(output_path.name, "AAPL_1D.csv")
            self.assertTrue(output_path.is_file())

    def test_missing_symbol_reports_partial(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            result = _run_with_fake_engine(
                MarketDataExportConfig(
                    database_url="postgresql://example",
                    symbols=["AAPL", "MSFT"],
                    bars_root=str(Path(tmp) / "bars"),
                ),
                _FakeEngine([_row("AAPL", 100)]),
            )

            self.assertEqual(result.status, STATUS_PARTIAL)
            self.assertEqual(result.symbols_exported, ["AAPL"])
            self.assertEqual(result.symbols_missing, ["MSFT"])
            self.assertIn(f"{REASON_NO_ROWS_FOR_SYMBOL}:MSFT", result.failure_reasons)


class TestNormalizeBarsCompatibility(unittest.TestCase):
    def test_exported_csv_is_parseable_by_symbol_inputs_runner(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            result = _run_with_fake_engine(
                MarketDataExportConfig(
                    database_url="postgresql://example",
                    symbols=["AAPL"],
                    bars_root=str(Path(tmp) / "bars"),
                ),
                _FakeEngine([
                    _row(
                        "AAPL",
                        1749130200,
                        open_micros=99_000_000,
                        high_micros=99_400_000,
                        low_micros=98_600_000,
                        close_micros=99_200_000,
                        volume=2_000_000,
                    )
                ]),
            )

            bars, reason = normalize_bars_csv(Path(result.output_paths["AAPL"]).read_text(encoding="utf-8"))
            self.assertIsNone(reason)
            self.assertEqual(len(bars), 1)
            self.assertEqual(bars[0]["open"], 99.0)
            self.assertEqual(bars[0]["high"], 99.4)
            self.assertEqual(bars[0]["low"], 98.6)
            self.assertEqual(bars[0]["close"], 99.2)
            self.assertEqual(bars[0]["volume"], 2_000_000.0)
            self.assertIs(bars[0]["is_complete"], True)

    def test_backtest_style_micros_rows_convert_through_normalize_bars_csv(self) -> None:
        text = (
            "symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n"
            "AAPL,1749130200,99000000,99400000,98600000,99200000,2000000,true\n"
        )
        bars, reason = normalize_bars_csv(text)

        self.assertIsNone(reason)
        self.assertEqual(bars[0]["open"], 99.0)
        self.assertEqual(bars[0]["close"], 99.2)

    def test_decimal_style_rows_remain_supported_by_normalize_bars_csv(self) -> None:
        text = (
            "ts,open,high,low,close,volume,is_complete\n"
            "2026-06-05T13:30:00+00:00,99.0,99.4,98.6,99.2,2000000,true\n"
        )
        bars, reason = normalize_bars_csv(text)

        self.assertIsNone(reason)
        self.assertEqual(bars[0]["open"], 99.0)
        self.assertEqual(bars[0]["close"], 99.2)


class TestPathAndSqlSafety(unittest.TestCase):
    def test_invalid_symbol_blocks_and_does_not_write_outside_bars_root(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp)
            bars_root = tmp_root / "bars"
            result = _run_with_fake_engine(
                MarketDataExportConfig(
                    database_url="postgresql://example",
                    symbols=["../AAPL"],
                    bars_root=str(bars_root),
                ),
                _FakeEngine([_row("AAPL", 100)]),
            )

            self.assertEqual(result.status, STATUS_BLOCKED)
            self.assertIn(f"{REASON_INVALID_SYMBOL}:../AAPL", result.failure_reasons)
            self.assertFalse((tmp_root / "AAPL_5m.csv").exists())
            self.assertFalse(bars_root.exists())

    def test_query_uses_parameterized_inputs(self) -> None:
        sql = str(build_select_md_bars_statement())

        self.assertIn(":timeframe", sql)
        self.assertIn(":symbols", sql)
        self.assertIn(":start_end_ts", sql)
        self.assertIn(":end_end_ts", sql)
        self.assertNotIn("{symbols}", sql)
        self.assertNotIn("{timeframe}", sql)

    def test_query_casts_nullable_time_window_params_to_bigint(self) -> None:
        sql = str(build_select_md_bars_statement())

        self.assertIn("cast(:start_end_ts as bigint)", sql)
        self.assertIn("cast(:end_end_ts as bigint)", sql)

    def test_query_contains_no_db_write_sql(self) -> None:
        sql_upper = str(build_select_md_bars_statement()).upper()
        for forbidden in ("INSERT ", "UPDATE ", "DELETE ", "DROP ", "ALTER ", "TRUNCATE ", "CREATE TABLE"):
            self.assertNotIn(forbidden, sql_upper)

    def test_module_has_no_forbidden_imports_or_references(self) -> None:
        source = Path(mde.__file__).read_text(encoding="utf-8")
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
            exit_code = mde.main(["--symbols", "AAPL", "--bars-root", "unused"])
        printed = "".join(call.args[0] for call in fake_stdout.write.call_args_list if call.args)
        payload = json.loads(printed)

        self.assertEqual(exit_code, 1)
        self.assertEqual(payload["status"], STATUS_BLOCKED)
        self.assertNotIn("postgres", printed)


if __name__ == "__main__":
    unittest.main()
