"""
MARKET-DATA-EXPORT-DB-PROOF-01 tests.

Default coverage uses fake exporter results and temp CSV fixtures. The optional
real DB proof is skipped unless MQK_RUN_DB_PROOF_TEST=1 and MQK_PAPER_DB_URL are
explicitly provided by the operator.
"""
from __future__ import annotations

import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from mqk_research.scanner import market_data_export_db_proof as dbp
from mqk_research.scanner.market_data_export import (
    STATUS_COMPLETE as EXPORT_STATUS_COMPLETE,
    STATUS_PARTIAL as EXPORT_STATUS_PARTIAL,
    MarketDataExportResult,
)
from mqk_research.scanner.symbol_inputs_runner import SymbolInputsRunnerResult


def _write_csv(path: Path, lines: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def _valid_csv(path: Path, *, symbol: str = "AAPL", end_ts: int = 1_749_130_200) -> None:
    _write_csv(
        path,
        [
            "symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete",
            f"{symbol},{end_ts},99000000,99400000,98600000,99200000,2000000,true",
        ],
    )


def _export_result(
    output_path: Path,
    *,
    status: str = EXPORT_STATUS_COMPLETE,
    requested: list[str] | None = None,
    exported: list[str] | None = None,
) -> MarketDataExportResult:
    requested = requested or ["AAPL"]
    exported = exported or ["AAPL"]
    missing = [symbol for symbol in requested if symbol not in exported]
    return MarketDataExportResult(
        status=status,
        symbols_requested=requested,
        symbols_exported=exported,
        symbols_missing=missing,
        bars_written_by_symbol={symbol: 1 for symbol in exported},
        output_paths={symbol: str(output_path) for symbol in exported},
        failure_reasons=[f"market_data_export_no_rows_for_symbol:{symbol}" for symbol in missing],
    )


def _config(root: Path, *, database_url: str = "postgresql://user:password@example/db", **kwargs) -> dbp.MarketDataExportDbProofConfig:
    values = {
        "database_url": database_url,
        "symbols": ["AAPL"],
        "timeframe": "5m",
        "start_utc": "2026-06-08T13:30:00Z",
        "end_utc": "2026-06-08T20:00:00Z",
        "bars_root": str(root / "bars"),
        "trade_date": "2026-06-08",
        "proof_report_path": str(root / "proofs" / "market_data_export_db_proof.json"),
    }
    values.update(kwargs)
    return dbp.MarketDataExportDbProofConfig(**values)


class TestProofHardInvariants(unittest.TestCase):
    def test_result_forces_live_and_execution_flags_false(self) -> None:
        result = dbp.MarketDataExportDbProofResult(
            approved_for_live=bool(1),
            daemon_enforcement_executed=bool(1),
            broker_calls_executed=bool(1),
            db_writes_executed=bool(1),
        )

        payload = result.to_dict()
        self.assertFalse(result.approved_for_live)
        self.assertFalse(result.daemon_enforcement_executed)
        self.assertFalse(result.broker_calls_executed)
        self.assertFalse(result.db_writes_executed)
        self.assertFalse(payload["approved_for_live"])
        self.assertFalse(payload["daemon_enforcement_executed"])
        self.assertFalse(payload["broker_calls_executed"])
        self.assertFalse(payload["db_writes_executed"])


class TestCliBlockingAndRedaction(unittest.TestCase):
    def test_missing_database_url_exits_nonzero_without_export_or_secret_print(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stream = io.StringIO()
            with patch.dict(os.environ, {}, clear=True), \
                    patch.object(dbp, "export_md_bars_to_bars_root") as fake_export, \
                    patch("sys.stdout", new=stream):
                exit_code = dbp.main([
                    "--symbols", "AAPL",
                    "--timeframe", "5m",
                    "--bars-root", str(root / "bars"),
                    "--trade-date", "2026-06-08",
                    "--proof-report-path", str(root / "proof.json"),
                ])

            self.assertEqual(exit_code, 1)
            fake_export.assert_not_called()
            printed = stream.getvalue()
            payload = json.loads(printed)
            self.assertEqual(payload["status"], dbp.STATUS_BLOCKED)
            self.assertIn(dbp.REASON_MISSING_DATABASE_URL, payload["failure_reasons"])
            self.assertNotIn("password", printed)
            self.assertNotIn("postgresql://", printed)

    def test_cli_redacts_database_url_in_printed_json(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            csv_path = root / "bars" / "AAPL_5m.csv"
            _valid_csv(csv_path)
            stream = io.StringIO()
            full_url = "postgresql://user:super-secret@example.invalid/mqk"
            with patch.object(dbp, "export_md_bars_to_bars_root", return_value=_export_result(csv_path)), \
                    patch("sys.stdout", new=stream):
                exit_code = dbp.main([
                    "--database-url", full_url,
                    "--symbols", "AAPL",
                    "--timeframe", "5m",
                    "--start-utc", "2026-06-08T13:30:00Z",
                    "--end-utc", "2026-06-08T20:00:00Z",
                    "--bars-root", str(root / "bars"),
                    "--trade-date", "2026-06-08",
                    "--proof-report-path", str(root / "proof.json"),
                ])

            printed = stream.getvalue()
            payload = json.loads(printed)
            self.assertEqual(exit_code, 0)
            self.assertEqual(payload["database_url"], dbp.REDACTED_DATABASE_URL)
            self.assertNotIn(full_url, printed)
            self.assertNotIn("super-secret", printed)


class TestCsvProofInspection(unittest.TestCase):
    def test_export_result_with_no_files_blocks_proof(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            missing_csv = root / "bars" / "AAPL_5m.csv"
            with patch.object(dbp, "export_md_bars_to_bars_root", return_value=_export_result(missing_csv)):
                result = dbp.run_market_data_export_db_proof(_config(root))

            self.assertEqual(result.status, dbp.STATUS_BLOCKED)
            self.assertFalse(result.csv_parse_passed_by_symbol["AAPL"])
            self.assertIn(f"{dbp.REASON_CSV_FILE_MISSING}:AAPL", result.failure_reasons)

    def test_exported_csv_with_correct_header_and_one_row_passes_parse_proof(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            csv_path = root / "bars" / "AAPL_5m.csv"
            _valid_csv(csv_path)
            with patch.object(dbp, "export_md_bars_to_bars_root", return_value=_export_result(csv_path)):
                result = dbp.run_market_data_export_db_proof(_config(root))

            self.assertEqual(result.status, dbp.STATUS_COMPLETE)
            self.assertTrue(result.csv_parse_passed_by_symbol["AAPL"])
            self.assertEqual(result.row_counts["AAPL"], 1)
            self.assertEqual(result.first_end_ts_by_symbol["AAPL"], 1_749_130_200)
            self.assertEqual(result.last_end_ts_by_symbol["AAPL"], 1_749_130_200)

    def test_exported_csv_with_wrong_header_fails_proof(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            csv_path = root / "bars" / "AAPL_5m.csv"
            _write_csv(csv_path, ["symbol,end_ts,open,high,low,close,volume", "AAPL,1,1,1,1,1,1"])
            with patch.object(dbp, "export_md_bars_to_bars_root", return_value=_export_result(csv_path)):
                result = dbp.run_market_data_export_db_proof(_config(root))

            self.assertEqual(result.status, dbp.STATUS_BLOCKED)
            self.assertIn(f"{dbp.REASON_CSV_HEADER_MISMATCH}:AAPL", result.failure_reasons)

    def test_exported_csv_with_no_data_rows_fails_proof(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            csv_path = root / "bars" / "AAPL_5m.csv"
            _write_csv(csv_path, ["symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete"])
            with patch.object(dbp, "export_md_bars_to_bars_root", return_value=_export_result(csv_path)):
                result = dbp.run_market_data_export_db_proof(_config(root))

            self.assertEqual(result.status, dbp.STATUS_BLOCKED)
            self.assertIn(f"{dbp.REASON_CSV_NO_DATA_ROWS}:AAPL", result.failure_reasons)

    def test_exported_csv_with_descending_timestamps_fails_proof(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            csv_path = root / "bars" / "AAPL_5m.csv"
            _write_csv(
                csv_path,
                [
                    "symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete",
                    "AAPL,200,99000000,99400000,98600000,99200000,2000000,true",
                    "AAPL,100,99000000,99400000,98600000,99200000,2000000,true",
                ],
            )
            with patch.object(dbp, "export_md_bars_to_bars_root", return_value=_export_result(csv_path)):
                result = dbp.run_market_data_export_db_proof(_config(root))

            self.assertEqual(result.status, dbp.STATUS_BLOCKED)
            self.assertIn(f"{dbp.REASON_CSV_TIMESTAMP_DESCENDING}:AAPL", result.failure_reasons)

    def test_exported_csv_with_symbol_mismatch_fails_proof(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            csv_path = root / "bars" / "AAPL_5m.csv"
            _valid_csv(csv_path, symbol="MSFT")
            with patch.object(dbp, "export_md_bars_to_bars_root", return_value=_export_result(csv_path)):
                result = dbp.run_market_data_export_db_proof(_config(root))

            self.assertEqual(result.status, dbp.STATUS_BLOCKED)
            self.assertIn(f"{dbp.REASON_CSV_SYMBOL_MISMATCH}:AAPL", result.failure_reasons)


class TestOptionalStages(unittest.TestCase):
    def test_symbol_inputs_stage_runs_only_when_requested(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            csv_path = root / "bars" / "AAPL_5m.csv"
            _valid_csv(csv_path)
            with patch.object(dbp, "export_md_bars_to_bars_root", return_value=_export_result(csv_path)), \
                    patch.object(dbp, "run_symbol_inputs_producer") as fake_symbol_inputs:
                result = dbp.run_market_data_export_db_proof(_config(root))

            self.assertEqual(result.status, dbp.STATUS_COMPLETE)
            self.assertIsNone(result.symbol_inputs_status)
            fake_symbol_inputs.assert_not_called()

            fake_result = SymbolInputsRunnerResult(
                symbols_requested=["AAPL"],
                symbols_written=["AAPL"],
                output_path=str(root / "proofs" / "symbol_inputs_from_db_export.json"),
                status="complete",
            )
            with patch.object(dbp, "export_md_bars_to_bars_root", return_value=_export_result(csv_path)), \
                    patch.object(dbp, "run_symbol_inputs_producer", return_value=fake_result) as fake_symbol_inputs:
                result = dbp.run_market_data_export_db_proof(_config(root, run_symbol_inputs=True))

            self.assertEqual(result.status, dbp.STATUS_COMPLETE)
            self.assertEqual(result.symbol_inputs_status, "complete")
            fake_symbol_inputs.assert_called_once()

    def test_paper_readiness_refuses_to_run_without_required_paths(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with patch.object(dbp, "export_md_bars_to_bars_root") as fake_export:
                result = dbp.run_market_data_export_db_proof(_config(root, run_paper_readiness=True))

            self.assertEqual(result.status, dbp.STATUS_BLOCKED)
            self.assertIn(dbp.REASON_WATCHLIST_PATH_REQUIRED, result.failure_reasons)
            self.assertIn(dbp.REASON_STRATEGY_FIT_DIR_REQUIRED, result.failure_reasons)
            fake_export.assert_not_called()


class TestWriteScope(unittest.TestCase):
    def test_no_writes_outside_caller_provided_bars_and_proof_paths(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            outside = root / "outside"
            outside.mkdir()
            csv_path = root / "bars" / "AAPL_5m.csv"
            _valid_csv(csv_path)

            with patch.object(dbp, "export_md_bars_to_bars_root", return_value=_export_result(csv_path)):
                result = dbp.run_market_data_export_db_proof(_config(root))

            self.assertEqual(result.status, dbp.STATUS_COMPLETE)
            self.assertEqual(list(outside.iterdir()), [])
            self.assertTrue((root / "proofs" / "market_data_export_db_proof.json").is_file())
            self.assertTrue(csv_path.is_file())


class TestSourceSafety(unittest.TestCase):
    def _proof_sources(self) -> str:
        module_source = Path(dbp.__file__).read_text(encoding="utf-8")
        script_source = (Path(__file__).resolve().parents[1] / "scripts" / "prove_market_data_export_db.py").read_text(
            encoding="utf-8"
        )
        return module_source + "\n" + script_source

    def test_no_broker_daemon_oms_runtime_or_network_order_imports(self) -> None:
        source = self._proof_sources()
        for forbidden in (
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
            "BrokerGateway",
            "broker_adapter",
            "oms_outbox",
            "oms_inbox",
            "/api/v1/strategy/signal",
            "strategy/signal",
            "/v2/orders",
            "submit_order",
            "place_order",
            "enqueue_outbox",
        ):
            self.assertNotIn(forbidden, source, forbidden)

    def test_no_subprocess(self) -> None:
        source = self._proof_sources()
        self.assertNotIn("import subprocess", source)
        self.assertNotIn("os.system", source)

    def test_no_live_recommendation_or_eligibility_true_literals(self) -> None:
        source = self._proof_sources()
        self.assertNotIn("approved_for_live=True", source)
        self.assertNotIn("recommended_for_live=True", source)
        self.assertNotIn("eligible_for_live=True", source)


class TestOptionalRealDbProof(unittest.TestCase):
    @unittest.skipUnless(
        os.environ.get("MQK_RUN_DB_PROOF_TEST") == "1" and os.environ.get("MQK_PAPER_DB_URL"),
        "requires MQK_RUN_DB_PROOF_TEST=1 and MQK_PAPER_DB_URL",
    )
    def test_real_db_export_proof_against_operator_provided_paper_db(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            symbols = [
                item.strip()
                for item in os.environ.get("MQK_DB_PROOF_SYMBOLS", "AAPL").split(",")
                if item.strip()
            ]
            result = dbp.run_market_data_export_db_proof(
                dbp.MarketDataExportDbProofConfig(
                    database_url=os.environ["MQK_PAPER_DB_URL"],
                    symbols=symbols,
                    timeframe=os.environ.get("MQK_DB_PROOF_TIMEFRAME", "5m"),
                    start_utc=os.environ.get("MQK_DB_PROOF_START_UTC"),
                    end_utc=os.environ.get("MQK_DB_PROOF_END_UTC"),
                    bars_root=str(root / "bars"),
                    trade_date=os.environ.get("MQK_DB_PROOF_TRADE_DATE", "2026-06-08"),
                    proof_report_path=str(root / "proof" / "market_data_export_db_proof.json"),
                )
            )

            self.assertIn(result.status, (dbp.STATUS_COMPLETE, dbp.STATUS_PARTIAL), result.to_dict())
            self.assertTrue(result.symbols_exported, result.to_dict())
            for symbol in result.symbols_exported:
                self.assertTrue(result.csv_parse_passed_by_symbol[symbol], result.to_dict())


if __name__ == "__main__":
    unittest.main()
