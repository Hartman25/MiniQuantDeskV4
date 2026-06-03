"""
EXP-PENNY-01A — scanner unit tests (Python stdlib unittest only).

Run:
    $env:PYTHONPATH="research-py"
    python -m unittest discover -s research-py/experiments/exp_penny/tests -p "test_*.py"
"""
from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

SAMPLE_UNIVERSE_JSON = Path(__file__).parent.parent / "sample_universe.json"
SAMPLE_UNIVERSE_CSV = Path(__file__).parent.parent / "sample_universe.csv"


def _passing_row(**overrides: Any) -> dict[str, Any]:
    """Return a universe row that passes all gates."""
    base = {
        "symbol": "PASS",
        "price": 4.85,
        "bid": 4.83,
        "ask": 4.87,
        "volume": 850_000,
        "adv_20d_shares": 620_000,
        "adv_20d_usd": 2_975_000.0,
        "rvol": 2.8,
        "ma200": 3.90,
        "ma200_slope_20d": 0.012,
        "ma50": 4.50,
        "ma50_slope_20d": 0.025,
        "consolidation_high": 4.80,
        "consolidation_low": 4.35,
        "consolidation_range_pct": 9.6,
        "breakout_level": 4.80,
        "breakout_rvol": 2.8,
        "gap_flag": False,
        "halt_flag": False,
        "news_flag": None,
    }
    base.update(overrides)
    return base


# ---------------------------------------------------------------------------
# T01 — passing candidate produces would_trade=True
# ---------------------------------------------------------------------------

class TestPassingCandidate(unittest.TestCase):
    def setUp(self) -> None:
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        self.scanner = PennyBreakoutScanner(universe=[_passing_row()])

    def test_would_trade_true(self) -> None:
        records = self.scanner.scan()
        self.assertEqual(len(records), 1)
        self.assertTrue(records[0]["would_trade"])

    def test_signal_direction_long(self) -> None:
        records = self.scanner.scan()
        self.assertEqual(records[0]["signal_direction"], "long")

    def test_rejection_reason_none(self) -> None:
        records = self.scanner.scan()
        self.assertIsNone(records[0]["rejection_reason"])

    def test_paper_order_id_null(self) -> None:
        records = self.scanner.scan()
        self.assertIsNone(records[0]["paper_order_id"])

    def test_live_order_id_null(self) -> None:
        records = self.scanner.scan()
        self.assertIsNone(records[0]["live_order_id"])


# ---------------------------------------------------------------------------
# T02 — rejection cases
# ---------------------------------------------------------------------------

class TestRejectionCases(unittest.TestCase):
    def _scan_one(self, **overrides: Any):
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        scanner = PennyBreakoutScanner(universe=[_passing_row(**overrides)])
        records = scanner.scan()
        self.assertEqual(len(records), 1)
        return records[0]

    def test_reject_price_too_low(self) -> None:
        rec = self._scan_one(symbol="SUBP", price=0.32)
        self.assertFalse(rec["would_trade"])
        self.assertIn("price_below_min", rec["rejection_reason"])

    def test_reject_price_too_high(self) -> None:
        rec = self._scan_one(symbol="HIGHP", price=25.00)
        self.assertFalse(rec["would_trade"])
        self.assertIn("price_above_max", rec["rejection_reason"])

    def test_reject_spread_too_wide(self) -> None:
        rec = self._scan_one(symbol="WDSP", bid=3.38, ask=3.62, price=3.50)
        self.assertFalse(rec["would_trade"])
        self.assertIn("spread_too_wide", rec["rejection_reason"])

    def test_reject_adv_usd_too_low(self) -> None:
        rec = self._scan_one(symbol="ILLQ", adv_20d_usd=300_000.0)
        self.assertFalse(rec["would_trade"])
        self.assertIn("adv_usd_below_min", rec["rejection_reason"])

    def test_reject_halt_flag(self) -> None:
        rec = self._scan_one(symbol="HLTD", halt_flag=True)
        self.assertFalse(rec["would_trade"])
        self.assertEqual(rec["rejection_reason"], "halt_flag_set")

    def test_reject_gap_flag(self) -> None:
        rec = self._scan_one(symbol="GAPR", gap_flag=True)
        self.assertFalse(rec["would_trade"])
        self.assertEqual(rec["rejection_reason"], "gap_flag_set")

    def test_reject_breakout_rvol_weak(self) -> None:
        rec = self._scan_one(symbol="WKBK", breakout_rvol=1.5)
        self.assertFalse(rec["would_trade"])
        self.assertIn("breakout_rvol_weak", rec["rejection_reason"])

    def test_reject_consolidation_too_wide(self) -> None:
        rec = self._scan_one(symbol="CNSL", consolidation_range_pct=20.0)
        self.assertFalse(rec["would_trade"])
        self.assertIn("consolidation_too_wide", rec["rejection_reason"])

    def test_reject_ma200_slope_flat(self) -> None:
        rec = self._scan_one(symbol="FLAT", ma200_slope_20d=-0.001)
        self.assertFalse(rec["would_trade"])
        self.assertIn("ma200_slope_not_rising", rec["rejection_reason"])

    def test_reject_ma50_slope_flat(self) -> None:
        rec = self._scan_one(symbol="FLAT50", ma50_slope_20d=-0.001)
        self.assertFalse(rec["would_trade"])
        self.assertIn("ma50_slope_not_rising", rec["rejection_reason"])

    def test_reject_volume_too_low(self) -> None:
        rec = self._scan_one(symbol="LOWVOL", volume=100_000)
        self.assertFalse(rec["would_trade"])
        self.assertIn("volume_below_min", rec["rejection_reason"])

    def test_reject_price_below_breakout_level(self) -> None:
        rec = self._scan_one(symbol="NBRK", price=4.50, breakout_level=4.80)
        self.assertFalse(rec["would_trade"])
        self.assertIn("price_below_breakout_level", rec["rejection_reason"])


# ---------------------------------------------------------------------------
# T03 — rejection reason always populated when would_trade=False
# ---------------------------------------------------------------------------

class TestRejectionReasonPresence(unittest.TestCase):
    def test_all_rejected_records_have_reason(self) -> None:
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        universe = [
            _passing_row(symbol="PASS"),
            _passing_row(symbol="SUBP", price=0.32),
            _passing_row(symbol="WDSP", bid=3.38, ask=3.62, price=3.50),
            _passing_row(symbol="ILLQ", adv_20d_usd=300_000.0),
            _passing_row(symbol="HLTD", halt_flag=True),
            _passing_row(symbol="WKBK", breakout_rvol=1.5, consolidation_range_pct=20.0),
        ]
        scanner = PennyBreakoutScanner(universe=universe)
        records = scanner.scan()
        for rec in records:
            if not rec["would_trade"]:
                self.assertIsNotNone(rec["rejection_reason"], f"Missing rejection_reason for {rec['symbol']}")
                self.assertGreater(len(rec["rejection_reason"]), 0)

    def test_signal_direction_neutral_on_reject(self) -> None:
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        scanner = PennyBreakoutScanner(universe=[_passing_row(halt_flag=True)])
        records = scanner.scan()
        self.assertEqual(records[0]["signal_direction"], "neutral")


# ---------------------------------------------------------------------------
# T04 — paper_order_id and live_order_id always null across all records
# ---------------------------------------------------------------------------

class TestOrderIdsAlwaysNull(unittest.TestCase):
    def test_all_records_null_order_ids(self) -> None:
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        universe = [
            _passing_row(symbol="PASS"),
            _passing_row(symbol="FAIL", halt_flag=True),
        ]
        scanner = PennyBreakoutScanner(universe=universe)
        for rec in scanner.scan():
            self.assertIsNone(rec["paper_order_id"], f"{rec['symbol']}: paper_order_id must be null")
            self.assertIsNone(rec["live_order_id"], f"{rec['symbol']}: live_order_id must be null")


# ---------------------------------------------------------------------------
# T05 — static import guard: scanner must not import forbidden modules
# ---------------------------------------------------------------------------

class TestForbiddenImports(unittest.TestCase):
    _FORBIDDEN = [
        "oms_outbox",
        "oms_inbox",
        "BrokerGateway",
        "broker_adapter",
        "alpaca",
        "Start-PaperTradingSmoke",
        "mqk_execution",
        "mqk_runtime",
        "ExecutionOrchestrator",
    ]
    _EXP_PENNY_DIR = Path(__file__).parent.parent

    def _source_files(self):
        return [
            p for p in self._EXP_PENNY_DIR.rglob("*.py")
            if p.name != "__pycache__" and "test_" not in p.name
        ]

    def test_no_forbidden_strings_in_source(self) -> None:
        for src in self._source_files():
            content = src.read_text(encoding="utf-8")
            for forbidden in self._FORBIDDEN:
                self.assertNotIn(
                    forbidden,
                    content,
                    f"{src.name} must not reference '{forbidden}'",
                )


# ---------------------------------------------------------------------------
# T06 — runner refuses without MQK_EXPERIMENTAL_ENGINE_ENABLED
# ---------------------------------------------------------------------------

class TestRunnerRequiresEngineEnabled(unittest.TestCase):
    def test_runner_refuses_when_engine_disabled(self) -> None:
        from experiments.exp_engine.scanner_runner import run
        from experiments.exp_penny.scanner import PennyBreakoutScanner

        old = os.environ.pop("MQK_EXPERIMENTAL_ENGINE_ENABLED", None)
        try:
            os.environ["MQK_EXPERIMENTAL_ENGINE_ENABLED"] = "false"
            scanner = PennyBreakoutScanner(universe=[_passing_row()])
            exit_code = run(scanners=[scanner], argv=["--dry-run"])
            self.assertEqual(exit_code, 1)
        finally:
            if old is not None:
                os.environ["MQK_EXPERIMENTAL_ENGINE_ENABLED"] = old
            else:
                os.environ.pop("MQK_EXPERIMENTAL_ENGINE_ENABLED", None)


# ---------------------------------------------------------------------------
# T07 — runner requires --dry-run
# ---------------------------------------------------------------------------

class TestRunnerRequiresDryRun(unittest.TestCase):
    def test_runner_refuses_without_dry_run(self) -> None:
        from experiments.exp_engine.scanner_runner import run
        from experiments.exp_penny.scanner import PennyBreakoutScanner

        scanner = PennyBreakoutScanner(universe=[_passing_row()])
        # argparse calls sys.exit(2) when a required arg is missing
        with self.assertRaises(SystemExit) as ctx:
            run(scanners=[scanner], argv=[])
        self.assertNotEqual(ctx.exception.code, 0)


# ---------------------------------------------------------------------------
# T08 — JSONL output written to temp dir
# ---------------------------------------------------------------------------

class TestJsonlOutput(unittest.TestCase):
    def test_jsonl_written_to_temp_dir(self) -> None:
        from experiments.exp_engine.candidate_journal import CandidateJournalWriter
        from experiments.exp_penny.scanner import PennyBreakoutScanner

        scanner = PennyBreakoutScanner(universe=[_passing_row(), _passing_row(symbol="FAIL", halt_flag=True)])
        records = scanner.scan()

        with tempfile.TemporaryDirectory() as tmpdir:
            with CandidateJournalWriter(journal_dir=tmpdir, engine_id="exp-engine-core-01") as writer:
                for rec in records:
                    writer.append(rec)

            files = list(Path(tmpdir).glob("*.jsonl"))
            self.assertEqual(len(files), 1)

            lines = files[0].read_text(encoding="utf-8").strip().splitlines()
            self.assertEqual(len(lines), 2)

            for line in lines:
                obj = json.loads(line)
                self.assertIsNone(obj["paper_order_id"])
                self.assertIsNone(obj["live_order_id"])
                self.assertIn("would_trade", obj)


# ---------------------------------------------------------------------------
# T09 — no broker/OMS/outbox strings in exp_penny source tree
# ---------------------------------------------------------------------------

class TestNoOmsBrokerReferences(unittest.TestCase):
    _FORBIDDEN_STRINGS = [
        "oms_outbox",
        "oms_inbox",
        "BrokerGateway",
        "broker_adapter",
        "alpaca",
        "Start-PaperTradingSmoke",
    ]
    _EXP_PENNY_DIR = Path(__file__).parent.parent

    def test_no_forbidden_strings_anywhere_in_exp_penny(self) -> None:
        py_files = [p for p in self._EXP_PENNY_DIR.rglob("*.py") if "test_" not in p.name]
        self.assertGreater(len(py_files), 0, "Expected at least one non-test .py file in exp_penny")
        for src in py_files:
            content = src.read_text(encoding="utf-8")
            for forbidden in self._FORBIDDEN_STRINGS:
                self.assertNotIn(forbidden, content, f"{src.name} must not reference '{forbidden}'")


# ---------------------------------------------------------------------------
# T10 — sample universe file loads and produces expected mix of pass/reject
# ---------------------------------------------------------------------------

class TestSampleUniverseFile(unittest.TestCase):
    def test_sample_universe_produces_one_pass_and_multiple_rejects(self) -> None:
        from experiments.exp_penny.scanner import PennyBreakoutScanner

        with open(SAMPLE_UNIVERSE_JSON, encoding="utf-8") as fh:
            universe = json.load(fh)

        scanner = PennyBreakoutScanner(universe=universe)
        records = scanner.scan()
        self.assertEqual(len(records), len(universe))

        passing = [r for r in records if r["would_trade"]]
        rejected = [r for r in records if not r["would_trade"]]

        self.assertGreaterEqual(len(passing), 1, "Expected at least one passing candidate in sample universe")
        self.assertGreaterEqual(len(rejected), 1, "Expected at least one rejected candidate in sample universe")

        passing_symbols = [r["symbol"] for r in passing]
        self.assertIn("ACME", passing_symbols, "ACME should pass all gates")


# ---------------------------------------------------------------------------
# T11 — universe_loader: JSON sample loads 7 rows
# ---------------------------------------------------------------------------

class TestLoaderJsonLoads7Rows(unittest.TestCase):
    def test_json_sample_loads_7_rows(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_UNIVERSE_JSON)
        self.assertEqual(len(rows), 7)

    def test_json_symbols_present(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_UNIVERSE_JSON)
        symbols = {r["symbol"] for r in rows}
        self.assertIn("ACME", symbols)
        self.assertIn("HLTD", symbols)


# ---------------------------------------------------------------------------
# T12 — universe_loader: CSV sample loads 7 rows
# ---------------------------------------------------------------------------

class TestLoaderCsvLoads7Rows(unittest.TestCase):
    def test_csv_sample_loads_7_rows(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_UNIVERSE_CSV)
        self.assertEqual(len(rows), 7)

    def test_csv_symbols_present(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_UNIVERSE_CSV)
        symbols = {r["symbol"] for r in rows}
        self.assertIn("ACME", symbols)
        self.assertIn("HLTD", symbols)


# ---------------------------------------------------------------------------
# T13 — JSON and CSV produce the same pass/reject counts
# ---------------------------------------------------------------------------

class TestLoaderJsonCsvParity(unittest.TestCase):
    def test_json_csv_same_pass_reject_counts(self) -> None:
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        from experiments.exp_penny.universe_loader import load_universe

        json_rows = load_universe(SAMPLE_UNIVERSE_JSON)
        csv_rows = load_universe(SAMPLE_UNIVERSE_CSV)

        json_records = PennyBreakoutScanner(universe=json_rows).scan()
        csv_records = PennyBreakoutScanner(universe=csv_rows).scan()

        json_pass = sum(1 for r in json_records if r["would_trade"])
        csv_pass = sum(1 for r in csv_records if r["would_trade"])
        json_reject = sum(1 for r in json_records if not r["would_trade"])
        csv_reject = sum(1 for r in csv_records if not r["would_trade"])

        self.assertEqual(json_pass, csv_pass, "Pass count must match between JSON and CSV")
        self.assertEqual(json_reject, csv_reject, "Reject count must match between JSON and CSV")
        self.assertEqual(json_pass, 1)
        self.assertEqual(json_reject, 6)


# ---------------------------------------------------------------------------
# T14 — CSV booleans parse correctly
# ---------------------------------------------------------------------------

class TestLoaderCsvBooleans(unittest.TestCase):
    def _make_csv(self, halt_val: str, gap_val: str) -> str:
        header = (
            "symbol,price,bid,ask,volume,adv_20d_usd,ma200_slope_20d,"
            "ma50_slope_20d,consolidation_range_pct,breakout_level,"
            "breakout_rvol,gap_flag,halt_flag"
        )
        row = (
            f"TST,4.85,4.83,4.87,850000,2975000.0,0.012,"
            f"0.025,9.6,4.80,2.8,{gap_val},{halt_val}"
        )
        return f"{header}\n{row}\n"

    def _load_csv_text(self, text: str) -> list[dict]:
        import io
        import csv as csv_mod
        from experiments.exp_penny.universe_loader import _coerce_csv_row, _validate_row, REQUIRED_FIELDS
        reader = csv_mod.DictReader(io.StringIO(text))
        rows = []
        for i, raw in enumerate(reader, start=2):
            coerced = _coerce_csv_row(dict(raw), i)
            _validate_row(coerced, i)
            rows.append(coerced)
        return rows

    def test_true_values(self) -> None:
        for val in ("true", "1", "yes", "y"):
            rows = self._load_csv_text(self._make_csv(halt_val=val, gap_val="false"))
            self.assertIs(rows[0]["halt_flag"], True, f"halt_flag should be True for '{val}'")

    def test_false_values(self) -> None:
        for val in ("false", "0", "no", "n", ""):
            rows = self._load_csv_text(self._make_csv(halt_val=val, gap_val="false"))
            self.assertIs(rows[0]["halt_flag"], False, f"halt_flag should be False for '{val}'")

    def test_halt_flag_true_from_sample_csv(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_UNIVERSE_CSV)
        hltd = next(r for r in rows if r["symbol"] == "HLTD")
        self.assertIs(hltd["halt_flag"], True)

    def test_gap_flag_true_from_sample_csv(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_UNIVERSE_CSV)
        gapr = next(r for r in rows if r["symbol"] == "GAPR")
        self.assertIs(gapr["gap_flag"], True)


# ---------------------------------------------------------------------------
# T15 — numeric strings parse correctly
# ---------------------------------------------------------------------------

class TestLoaderCsvNumericConversion(unittest.TestCase):
    def test_price_is_float_from_csv(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_UNIVERSE_CSV)
        acme = next(r for r in rows if r["symbol"] == "ACME")
        self.assertIsInstance(acme["price"], float)
        self.assertAlmostEqual(acme["price"], 4.85)

    def test_volume_is_int_from_csv(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_UNIVERSE_CSV)
        acme = next(r for r in rows if r["symbol"] == "ACME")
        self.assertIsInstance(acme["volume"], int)
        self.assertEqual(acme["volume"], 850000)

    def test_adv_20d_usd_is_float_from_csv(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_UNIVERSE_CSV)
        acme = next(r for r in rows if r["symbol"] == "ACME")
        self.assertIsInstance(acme["adv_20d_usd"], float)


# ---------------------------------------------------------------------------
# T16 — missing required CSV column raises UniverseLoadError
# ---------------------------------------------------------------------------

class TestLoaderMissingRequiredColumn(unittest.TestCase):
    def test_missing_column_raises(self) -> None:
        from experiments.exp_penny.universe_loader import UniverseLoadError, load_universe
        import io, csv as csv_mod, tempfile

        # CSV without 'halt_flag'
        text = (
            "symbol,price,bid,ask,volume,adv_20d_usd,ma200_slope_20d,"
            "ma50_slope_20d,consolidation_range_pct,breakout_level,breakout_rvol,gap_flag\n"
            "TST,4.85,4.83,4.87,850000,2975000.0,0.012,0.025,9.6,4.80,2.8,false\n"
        )
        with tempfile.NamedTemporaryFile(suffix=".csv", mode="w", delete=False, encoding="utf-8") as f:
            f.write(text)
            tmp_path = f.name

        try:
            with self.assertRaises(UniverseLoadError) as ctx:
                load_universe(tmp_path)
            self.assertIn("halt_flag", str(ctx.exception))
        finally:
            import os
            os.unlink(tmp_path)


# ---------------------------------------------------------------------------
# T17 — invalid numeric value raises UniverseLoadError
# ---------------------------------------------------------------------------

class TestLoaderInvalidNumeric(unittest.TestCase):
    def test_invalid_numeric_raises(self) -> None:
        from experiments.exp_penny.universe_loader import UniverseLoadError, load_universe
        import tempfile

        text = (
            "symbol,price,bid,ask,volume,adv_20d_usd,ma200_slope_20d,"
            "ma50_slope_20d,consolidation_range_pct,breakout_level,breakout_rvol,gap_flag,halt_flag\n"
            "TST,NOT_A_NUMBER,4.83,4.87,850000,2975000.0,0.012,0.025,9.6,4.80,2.8,false,false\n"
        )
        with tempfile.NamedTemporaryFile(suffix=".csv", mode="w", delete=False, encoding="utf-8") as f:
            f.write(text)
            tmp_path = f.name

        try:
            with self.assertRaises(UniverseLoadError) as ctx:
                load_universe(tmp_path)
            self.assertIn("price", str(ctx.exception))
        finally:
            import os
            os.unlink(tmp_path)


# ---------------------------------------------------------------------------
# T18 — invalid boolean value raises UniverseLoadError
# ---------------------------------------------------------------------------

class TestLoaderInvalidBoolean(unittest.TestCase):
    def test_invalid_bool_raises(self) -> None:
        from experiments.exp_penny.universe_loader import UniverseLoadError, load_universe
        import tempfile

        text = (
            "symbol,price,bid,ask,volume,adv_20d_usd,ma200_slope_20d,"
            "ma50_slope_20d,consolidation_range_pct,breakout_level,breakout_rvol,gap_flag,halt_flag\n"
            "TST,4.85,4.83,4.87,850000,2975000.0,0.012,0.025,9.6,4.80,2.8,MAYBE,false\n"
        )
        with tempfile.NamedTemporaryFile(suffix=".csv", mode="w", delete=False, encoding="utf-8") as f:
            f.write(text)
            tmp_path = f.name

        try:
            with self.assertRaises(UniverseLoadError) as ctx:
                load_universe(tmp_path)
            self.assertIn("gap_flag", str(ctx.exception))
        finally:
            import os
            os.unlink(tmp_path)


# ---------------------------------------------------------------------------
# T19 — unsupported extension raises UniverseLoadError
# ---------------------------------------------------------------------------

class TestLoaderUnsupportedExtension(unittest.TestCase):
    def test_unsupported_ext_raises(self) -> None:
        from experiments.exp_penny.universe_loader import UniverseLoadError, load_universe
        import tempfile

        with tempfile.NamedTemporaryFile(suffix=".xlsx", mode="w", delete=False, encoding="utf-8") as f:
            f.write("dummy")
            tmp_path = f.name

        try:
            with self.assertRaises(UniverseLoadError) as ctx:
                load_universe(tmp_path)
            self.assertIn(".xlsx", str(ctx.exception))
        finally:
            import os
            os.unlink(tmp_path)


# ---------------------------------------------------------------------------
# T20 — empty file / empty universe raises UniverseLoadError
# ---------------------------------------------------------------------------

class TestLoaderEmptyFile(unittest.TestCase):
    def test_empty_json_raises(self) -> None:
        from experiments.exp_penny.universe_loader import UniverseLoadError, load_universe
        import tempfile

        with tempfile.NamedTemporaryFile(suffix=".json", mode="w", delete=False, encoding="utf-8") as f:
            f.write("[]")
            tmp_path = f.name

        try:
            with self.assertRaises(UniverseLoadError):
                load_universe(tmp_path)
        finally:
            import os
            os.unlink(tmp_path)

    def test_empty_csv_raises(self) -> None:
        from experiments.exp_penny.universe_loader import UniverseLoadError, load_universe
        import tempfile

        with tempfile.NamedTemporaryFile(suffix=".csv", mode="w", delete=False, encoding="utf-8") as f:
            f.write("")
            tmp_path = f.name

        try:
            with self.assertRaises(UniverseLoadError):
                load_universe(tmp_path)
        finally:
            import os
            os.unlink(tmp_path)

    def test_csv_header_only_raises(self) -> None:
        from experiments.exp_penny.universe_loader import UniverseLoadError, load_universe
        import tempfile

        text = (
            "symbol,price,bid,ask,volume,adv_20d_usd,ma200_slope_20d,"
            "ma50_slope_20d,consolidation_range_pct,breakout_level,breakout_rvol,gap_flag,halt_flag\n"
        )
        with tempfile.NamedTemporaryFile(suffix=".csv", mode="w", delete=False, encoding="utf-8") as f:
            f.write(text)
            tmp_path = f.name

        try:
            with self.assertRaises(UniverseLoadError):
                load_universe(tmp_path)
        finally:
            import os
            os.unlink(tmp_path)


# ---------------------------------------------------------------------------
# T21 — runner works with CSV input under enabled dry-run config
# ---------------------------------------------------------------------------

class TestRunnerWithCsvInput(unittest.TestCase):
    def test_runner_csv_produces_records(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        from experiments.exp_penny.scanner import PennyBreakoutScanner

        rows = load_universe(SAMPLE_UNIVERSE_CSV)
        scanner = PennyBreakoutScanner(universe=rows)
        records = scanner.scan()

        self.assertEqual(len(records), 7)
        passing = [r for r in records if r["would_trade"]]
        self.assertEqual(len(passing), 1)
        self.assertEqual(passing[0]["symbol"], "ACME")


# ---------------------------------------------------------------------------
# T22 — generated JSONL has null paper/live order IDs from CSV input
# ---------------------------------------------------------------------------

class TestJsonlNullOrderIdsFromCsv(unittest.TestCase):
    def test_csv_jsonl_null_order_ids(self) -> None:
        from experiments.exp_engine.candidate_journal import CandidateJournalWriter
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        from experiments.exp_penny.universe_loader import load_universe

        rows = load_universe(SAMPLE_UNIVERSE_CSV)
        scanner = PennyBreakoutScanner(universe=rows)
        records = scanner.scan()

        with tempfile.TemporaryDirectory() as tmpdir:
            with CandidateJournalWriter(journal_dir=tmpdir, engine_id="exp-engine-core-01") as writer:
                for rec in records:
                    writer.append(rec)

            files = list(Path(tmpdir).glob("*.jsonl"))
            self.assertEqual(len(files), 1)
            lines = files[0].read_text(encoding="utf-8").strip().splitlines()
            self.assertEqual(len(lines), 7)

            for line in lines:
                obj = json.loads(line)
                self.assertIsNone(obj["paper_order_id"])
                self.assertIsNone(obj["live_order_id"])


# ---------------------------------------------------------------------------
# T23 — no forbidden strings in exp_penny source including new loader file
# ---------------------------------------------------------------------------

class TestNoForbiddenStringsIncludingLoader(unittest.TestCase):
    _FORBIDDEN = [
        "oms_outbox",
        "oms_inbox",
        "BrokerGateway",
        "broker_adapter",
        "alpaca",
        "Start-PaperTradingSmoke",
    ]
    _EXP_PENNY_DIR = Path(__file__).parent.parent

    def test_universe_loader_has_no_forbidden_strings(self) -> None:
        loader = self._EXP_PENNY_DIR / "universe_loader.py"
        self.assertTrue(loader.exists(), "universe_loader.py must exist")
        content = loader.read_text(encoding="utf-8")
        for forbidden in self._FORBIDDEN:
            self.assertNotIn(forbidden, content, f"universe_loader.py must not reference '{forbidden}'")


# ---------------------------------------------------------------------------
# Sample file paths for screener profile tests
# ---------------------------------------------------------------------------

SAMPLES_DIR = Path(__file__).parent.parent / "samples"
SAMPLE_GENERIC = SAMPLES_DIR / "sample_generic_universe.csv"
SAMPLE_FINVIZ = SAMPLES_DIR / "sample_finviz_export.csv"
SAMPLE_TRADINGVIEW = SAMPLES_DIR / "sample_tradingview_export.csv"


# ---------------------------------------------------------------------------
# T24 — generic profile: canonical headers load correctly
# ---------------------------------------------------------------------------

class TestGenericProfileLoad(unittest.TestCase):
    def test_generic_sample_loads_2_rows(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_GENERIC, profile="generic")
        self.assertEqual(len(rows), 2)

    def test_generic_sample_symbols(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_GENERIC, profile="generic")
        symbols = {r["symbol"] for r in rows}
        self.assertIn("GPASS", symbols)
        self.assertIn("GFAIL", symbols)

    def test_generic_no_profile_arg_same_result(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows_default = load_universe(SAMPLE_GENERIC)
        rows_explicit = load_universe(SAMPLE_GENERIC, profile="generic")
        self.assertEqual(len(rows_default), len(rows_explicit))


# ---------------------------------------------------------------------------
# T25 — generic profile: produces 1 pass, 1 reject from sample
# ---------------------------------------------------------------------------

class TestGenericProfilePassReject(unittest.TestCase):
    def test_generic_sample_pass_reject_counts(self) -> None:
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        from experiments.exp_penny.universe_loader import load_universe

        rows = load_universe(SAMPLE_GENERIC, profile="generic")
        records = PennyBreakoutScanner(universe=rows).scan()

        passing = [r for r in records if r["would_trade"]]
        rejected = [r for r in records if not r["would_trade"]]

        self.assertEqual(len(passing), 1, "Expected exactly 1 passing candidate in generic sample")
        self.assertEqual(len(rejected), 1, "Expected exactly 1 rejected candidate in generic sample")
        self.assertEqual(passing[0]["symbol"], "GPASS")


# ---------------------------------------------------------------------------
# T26 — finviz profile: alias mapping produces canonical rows
# ---------------------------------------------------------------------------

class TestFinvizProfileAliasMapping(unittest.TestCase):
    def test_finviz_sample_loads_2_rows(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_FINVIZ, profile="finviz")
        self.assertEqual(len(rows), 2)

    def test_finviz_symbol_mapped_from_ticker(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_FINVIZ, profile="finviz")
        symbols = {r["symbol"] for r in rows}
        self.assertIn("FVPASS", symbols)
        self.assertIn("FVFAIL", symbols)

    def test_finviz_price_mapped_from_Price(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_FINVIZ, profile="finviz")
        fvpass = next(r for r in rows if r["symbol"] == "FVPASS")
        self.assertAlmostEqual(fvpass["price"], 4.85)

    def test_finviz_breakout_rvol_mapped_from_rel_volume(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_FINVIZ, profile="finviz")
        fvpass = next(r for r in rows if r["symbol"] == "FVPASS")
        self.assertAlmostEqual(fvpass["breakout_rvol"], 2.8)


# ---------------------------------------------------------------------------
# T27 — finviz profile: produces 1 pass, 1 reject from sample
# ---------------------------------------------------------------------------

class TestFinvizProfilePassReject(unittest.TestCase):
    def test_finviz_sample_pass_reject_counts(self) -> None:
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        from experiments.exp_penny.universe_loader import load_universe

        rows = load_universe(SAMPLE_FINVIZ, profile="finviz")
        records = PennyBreakoutScanner(universe=rows).scan()

        passing = [r for r in records if r["would_trade"]]
        rejected = [r for r in records if not r["would_trade"]]

        self.assertEqual(len(passing), 1, "Expected exactly 1 passing candidate in finviz sample")
        self.assertEqual(len(rejected), 1, "Expected exactly 1 rejected candidate in finviz sample")
        self.assertEqual(passing[0]["symbol"], "FVPASS")
        self.assertEqual(rejected[0]["symbol"], "FVFAIL")

    def test_finviz_rejected_reason_halt_flag(self) -> None:
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        from experiments.exp_penny.universe_loader import load_universe

        rows = load_universe(SAMPLE_FINVIZ, profile="finviz")
        records = PennyBreakoutScanner(universe=rows).scan()
        fvfail = next(r for r in records if r["symbol"] == "FVFAIL")
        self.assertEqual(fvfail["rejection_reason"], "halt_flag_set")


# ---------------------------------------------------------------------------
# T28 — tradingview profile: alias mapping produces canonical rows
# ---------------------------------------------------------------------------

class TestTradingviewProfileAliasMapping(unittest.TestCase):
    def test_tradingview_sample_loads_2_rows(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_TRADINGVIEW, profile="tradingview")
        self.assertEqual(len(rows), 2)

    def test_tradingview_symbol_mapped_from_Symbol(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_TRADINGVIEW, profile="tradingview")
        symbols = {r["symbol"] for r in rows}
        self.assertIn("TVPASS", symbols)
        self.assertIn("TVFAIL", symbols)

    def test_tradingview_price_mapped_from_Last(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_TRADINGVIEW, profile="tradingview")
        tvpass = next(r for r in rows if r["symbol"] == "TVPASS")
        self.assertAlmostEqual(tvpass["price"], 4.85)

    def test_tradingview_breakout_rvol_mapped_from_relative_volume(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe
        rows = load_universe(SAMPLE_TRADINGVIEW, profile="tradingview")
        tvpass = next(r for r in rows if r["symbol"] == "TVPASS")
        self.assertAlmostEqual(tvpass["breakout_rvol"], 2.8)


# ---------------------------------------------------------------------------
# T29 — tradingview profile: produces 1 pass, 1 reject from sample
# ---------------------------------------------------------------------------

class TestTradingviewProfilePassReject(unittest.TestCase):
    def test_tradingview_sample_pass_reject_counts(self) -> None:
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        from experiments.exp_penny.universe_loader import load_universe

        rows = load_universe(SAMPLE_TRADINGVIEW, profile="tradingview")
        records = PennyBreakoutScanner(universe=rows).scan()

        passing = [r for r in records if r["would_trade"]]
        rejected = [r for r in records if not r["would_trade"]]

        self.assertEqual(len(passing), 1, "Expected exactly 1 passing candidate in tradingview sample")
        self.assertEqual(len(rejected), 1, "Expected exactly 1 rejected candidate in tradingview sample")
        self.assertEqual(passing[0]["symbol"], "TVPASS")
        self.assertEqual(rejected[0]["symbol"], "TVFAIL")

    def test_tradingview_rejected_reason_price_below_min(self) -> None:
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        from experiments.exp_penny.universe_loader import load_universe

        rows = load_universe(SAMPLE_TRADINGVIEW, profile="tradingview")
        records = PennyBreakoutScanner(universe=rows).scan()
        tvfail = next(r for r in records if r["symbol"] == "TVFAIL")
        self.assertIn("price_below_min", tvfail["rejection_reason"])


# ---------------------------------------------------------------------------
# T30 — unsupported profile raises UniverseLoadError
# ---------------------------------------------------------------------------

class TestUnsupportedProfile(unittest.TestCase):
    def test_unknown_profile_raises(self) -> None:
        from experiments.exp_penny.universe_loader import UniverseLoadError, load_universe
        with self.assertRaises(UniverseLoadError) as ctx:
            load_universe(SAMPLE_GENERIC, profile="bloomberg")
        self.assertIn("bloomberg", str(ctx.exception))

    def test_error_mentions_supported_profiles(self) -> None:
        from experiments.exp_penny.universe_loader import UniverseLoadError, load_universe
        with self.assertRaises(UniverseLoadError) as ctx:
            load_universe(SAMPLE_GENERIC, profile="unknown_xyz")
        msg = str(ctx.exception)
        self.assertIn("generic", msg)


# ---------------------------------------------------------------------------
# T31 — missing required field after profile mapping raises UniverseLoadError
# ---------------------------------------------------------------------------

class TestMissingFieldAfterProfileMapping(unittest.TestCase):
    def test_finviz_profile_missing_halt_flag_raises(self) -> None:
        from experiments.exp_penny.universe_loader import UniverseLoadError, load_universe
        import tempfile

        # CSV using Finviz aliases but omitting halt_flag
        text = (
            "Ticker,Price,bid,ask,Volume,adv_20d_usd,Rel Volume,"
            "ma200_slope_20d,ma50_slope_20d,consolidation_range_pct,breakout_level,gap_flag\n"
            "TST,4.85,4.83,4.87,850000,2975000.0,2.8,0.012,0.025,9.6,4.80,false\n"
        )
        with tempfile.NamedTemporaryFile(suffix=".csv", mode="w", delete=False, encoding="utf-8") as f:
            f.write(text)
            tmp_path = f.name

        try:
            with self.assertRaises(UniverseLoadError) as ctx:
                load_universe(tmp_path, profile="finviz")
            self.assertIn("halt_flag", str(ctx.exception))
        finally:
            import os
            os.unlink(tmp_path)

    def test_generic_profile_missing_required_column_raises(self) -> None:
        from experiments.exp_penny.universe_loader import UniverseLoadError, load_universe
        import tempfile

        text = (
            "symbol,price,bid,ask,volume,adv_20d_usd,ma200_slope_20d,"
            "ma50_slope_20d,consolidation_range_pct,breakout_level,breakout_rvol,gap_flag\n"
            "TST,4.85,4.83,4.87,850000,2975000.0,0.012,0.025,9.6,4.80,2.8,false\n"
        )
        with tempfile.NamedTemporaryFile(suffix=".csv", mode="w", delete=False, encoding="utf-8") as f:
            f.write(text)
            tmp_path = f.name

        try:
            with self.assertRaises(UniverseLoadError) as ctx:
                load_universe(tmp_path, profile="generic")
            self.assertIn("halt_flag", str(ctx.exception))
        finally:
            import os
            os.unlink(tmp_path)


# ---------------------------------------------------------------------------
# T32 — runner accepts --profile finviz and --profile tradingview
# ---------------------------------------------------------------------------

class TestRunnerAcceptsProfileArg(unittest.TestCase):
    def _run_with_env(self, path: str, profile: str) -> int:
        from experiments.exp_engine.scanner_runner import run
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        from experiments.exp_penny.universe_loader import load_universe

        rows = load_universe(path, profile=profile)
        scanner = PennyBreakoutScanner(universe=rows)
        old = os.environ.pop("MQK_EXPERIMENTAL_ENGINE_ENABLED", None)
        try:
            os.environ["MQK_EXPERIMENTAL_ENGINE_ENABLED"] = "true"
            os.environ["MQK_EXPERIMENTAL_ENGINE_MODE"] = "scanner_only"
            os.environ["MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED"] = "false"
            with tempfile.TemporaryDirectory() as tmpdir:
                os.environ["MQK_EXPERIMENTAL_JOURNAL_DIR"] = tmpdir
                exit_code = run(scanners=[scanner], argv=["--dry-run"])
        finally:
            if old is not None:
                os.environ["MQK_EXPERIMENTAL_ENGINE_ENABLED"] = old
            else:
                os.environ.pop("MQK_EXPERIMENTAL_ENGINE_ENABLED", None)
        return exit_code

    def test_runner_finviz_profile_succeeds(self) -> None:
        exit_code = self._run_with_env(str(SAMPLE_FINVIZ), "finviz")
        self.assertEqual(exit_code, 0)

    def test_runner_tradingview_profile_succeeds(self) -> None:
        exit_code = self._run_with_env(str(SAMPLE_TRADINGVIEW), "tradingview")
        self.assertEqual(exit_code, 0)


# ---------------------------------------------------------------------------
# T33 — JSONL output from finviz profile has null paper/live order IDs
# ---------------------------------------------------------------------------

class TestJsonlNullOrderIdsFromFinvizProfile(unittest.TestCase):
    def test_finviz_jsonl_null_order_ids(self) -> None:
        from experiments.exp_engine.candidate_journal import CandidateJournalWriter
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        from experiments.exp_penny.universe_loader import load_universe

        rows = load_universe(SAMPLE_FINVIZ, profile="finviz")
        records = PennyBreakoutScanner(universe=rows).scan()

        with tempfile.TemporaryDirectory() as tmpdir:
            with CandidateJournalWriter(journal_dir=tmpdir, engine_id="exp-engine-core-01") as writer:
                for rec in records:
                    writer.append(rec)

            files = list(Path(tmpdir).glob("*.jsonl"))
            self.assertEqual(len(files), 1)
            lines = files[0].read_text(encoding="utf-8").strip().splitlines()
            self.assertEqual(len(lines), 2)

            for line in lines:
                obj = json.loads(line)
                self.assertIsNone(obj["paper_order_id"])
                self.assertIsNone(obj["live_order_id"])


# ---------------------------------------------------------------------------
# T34 — no network import modules in exp_penny source
# ---------------------------------------------------------------------------

class TestNoNetworkImports(unittest.TestCase):
    _NETWORK_MODULES = ["requests", "urllib", "http.client", "aiohttp"]
    _EXP_PENNY_DIR = Path(__file__).parent.parent

    def test_no_network_imports_in_exp_penny_source(self) -> None:
        py_files = [p for p in self._EXP_PENNY_DIR.rglob("*.py") if "test_" not in p.name]
        self.assertGreater(len(py_files), 0)
        for src in py_files:
            content = src.read_text(encoding="utf-8")
            for mod in self._NETWORK_MODULES:
                self.assertNotIn(
                    f"import {mod}",
                    content,
                    f"{src.name} must not import network module '{mod}'",
                )


# ---------------------------------------------------------------------------
# T35 — screener_profiles module: apply_profile is pure and correct
# ---------------------------------------------------------------------------

class TestScreenerProfilesModule(unittest.TestCase):
    def test_generic_no_remapping(self) -> None:
        from experiments.exp_penny.screener_profiles import apply_profile
        headers = ["symbol", "price", "bid", "ask"]
        self.assertEqual(apply_profile(headers, "generic"), headers)

    def test_finviz_ticker_becomes_symbol(self) -> None:
        from experiments.exp_penny.screener_profiles import apply_profile
        result = apply_profile(["Ticker", "Price", "bid"], "finviz")
        self.assertEqual(result, ["symbol", "price", "bid"])

    def test_tradingview_last_becomes_price(self) -> None:
        from experiments.exp_penny.screener_profiles import apply_profile
        result = apply_profile(["Symbol", "Last", "bid"], "tradingview")
        self.assertEqual(result, ["symbol", "price", "bid"])

    def test_tradingview_close_becomes_price(self) -> None:
        from experiments.exp_penny.screener_profiles import apply_profile
        result = apply_profile(["Symbol", "Close", "bid"], "tradingview")
        self.assertEqual(result, ["symbol", "price", "bid"])

    def test_unknown_profile_raises_key_error(self) -> None:
        from experiments.exp_penny.screener_profiles import apply_profile
        with self.assertRaises(KeyError):
            apply_profile(["symbol"], "bloomberg")

    def test_passthrough_unknown_headers(self) -> None:
        from experiments.exp_penny.screener_profiles import apply_profile
        result = apply_profile(["SomeUnknownCol", "Price"], "finviz")
        self.assertEqual(result[0], "SomeUnknownCol")
        self.assertEqual(result[1], "price")


# ---------------------------------------------------------------------------
# Helpers shared by enrichment tests
# ---------------------------------------------------------------------------

SAMPLES_OHLCV_DIR = Path(__file__).parent.parent / "samples" / "ohlcv"
SAMPLE_ENRICH_PASS_OHLCV = SAMPLES_OHLCV_DIR / "ENRICH_PASS.csv"
SAMPLE_ENRICHMENT_UNIVERSE = Path(__file__).parent.parent / "samples" / "sample_enrichment_universe.csv"


def _make_ohlcv_csv(tmpdir: str, symbol: str, n_bars: int = 220) -> str:
    """Generate a deterministic OHLCV CSV in tmpdir with n_bars rows."""
    from datetime import date, timedelta
    path = Path(tmpdir) / f"{symbol}.csv"
    lines = ["date,open,high,low,close,volume"]
    start = date(2024, 1, 2)
    for i in range(n_bars - 1):
        d = (start + timedelta(days=i)).isoformat()
        c = round(2.65 + i * 0.01, 2)
        lines.append(f"{d},{round(c - 0.01, 2)},{round(c + 0.05, 2)},{round(c - 0.05, 2)},{c},300000")
    # Final breakout bar
    lines.append("2024-08-08,4.90,5.15,4.95,5.10,700000")
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return str(path)


def _enrichment_passing_base_row(**overrides: Any) -> dict[str, Any]:
    """Return a partial universe row (no computed fields) that enriches to a passing candidate."""
    base = {
        "symbol": "ENRICH_PASS",
        "price": 5.10,
        "bid": 5.08,
        "ask": 5.12,
        "volume": 700_000,
        "gap_flag": False,
        "halt_flag": False,
    }
    base.update(overrides)
    return base


# ---------------------------------------------------------------------------
# T36 — load_ohlcv_csv parses a valid OHLCV CSV
# ---------------------------------------------------------------------------

class TestLoadOhlcvCsv(unittest.TestCase):
    def test_committed_fixture_loads_220_bars(self) -> None:
        from experiments.exp_penny.indicator_enrichment import load_ohlcv_csv
        bars = load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV)
        self.assertEqual(len(bars), 220)

    def test_bars_have_required_keys(self) -> None:
        from experiments.exp_penny.indicator_enrichment import load_ohlcv_csv
        bars = load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV)
        for key in ("date", "open", "high", "low", "close", "volume"):
            self.assertIn(key, bars[0], f"bar must have '{key}'")

    def test_close_is_float(self) -> None:
        from experiments.exp_penny.indicator_enrichment import load_ohlcv_csv
        bars = load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV)
        self.assertIsInstance(bars[0]["close"], float)

    def test_volume_is_int(self) -> None:
        from experiments.exp_penny.indicator_enrichment import load_ohlcv_csv
        bars = load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV)
        self.assertIsInstance(bars[0]["volume"], int)

    def test_latest_bar_is_breakout(self) -> None:
        from experiments.exp_penny.indicator_enrichment import load_ohlcv_csv
        bars = load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV)
        self.assertAlmostEqual(bars[-1]["close"], 5.10)
        self.assertEqual(bars[-1]["volume"], 700_000)


# ---------------------------------------------------------------------------
# T37 — invalid OHLCV numeric raises IndicatorEnrichmentError
# ---------------------------------------------------------------------------

class TestOhlcvInvalidNumeric(unittest.TestCase):
    def test_non_numeric_close_raises(self) -> None:
        from experiments.exp_penny.indicator_enrichment import IndicatorEnrichmentError, load_ohlcv_csv
        text = "date,open,high,low,close,volume\n2024-01-02,2.64,2.70,2.60,NOT_A_NUMBER,300000\n"
        with tempfile.NamedTemporaryFile(suffix=".csv", mode="w", delete=False, encoding="utf-8") as f:
            f.write(text)
            tmp = f.name
        try:
            with self.assertRaises(IndicatorEnrichmentError) as ctx:
                load_ohlcv_csv(tmp)
            self.assertIn("close", str(ctx.exception))
        finally:
            os.unlink(tmp)

    def test_zero_close_raises(self) -> None:
        from experiments.exp_penny.indicator_enrichment import IndicatorEnrichmentError, load_ohlcv_csv
        text = "date,open,high,low,close,volume\n2024-01-02,2.64,2.70,2.60,0.00,300000\n"
        with tempfile.NamedTemporaryFile(suffix=".csv", mode="w", delete=False, encoding="utf-8") as f:
            f.write(text)
            tmp = f.name
        try:
            with self.assertRaises(IndicatorEnrichmentError):
                load_ohlcv_csv(tmp)
        finally:
            os.unlink(tmp)

    def test_missing_column_raises(self) -> None:
        from experiments.exp_penny.indicator_enrichment import IndicatorEnrichmentError, load_ohlcv_csv
        text = "date,open,high,low,volume\n2024-01-02,2.64,2.70,2.60,300000\n"
        with tempfile.NamedTemporaryFile(suffix=".csv", mode="w", delete=False, encoding="utf-8") as f:
            f.write(text)
            tmp = f.name
        try:
            with self.assertRaises(IndicatorEnrichmentError) as ctx:
                load_ohlcv_csv(tmp)
            self.assertIn("close", str(ctx.exception))
        finally:
            os.unlink(tmp)

    def test_empty_file_raises(self) -> None:
        from experiments.exp_penny.indicator_enrichment import IndicatorEnrichmentError, load_ohlcv_csv
        with tempfile.NamedTemporaryFile(suffix=".csv", mode="w", delete=False, encoding="utf-8") as f:
            f.write("")
            tmp = f.name
        try:
            with self.assertRaises(IndicatorEnrichmentError):
                load_ohlcv_csv(tmp)
        finally:
            os.unlink(tmp)

    def test_missing_file_raises(self) -> None:
        from experiments.exp_penny.indicator_enrichment import IndicatorEnrichmentError, load_ohlcv_csv
        with self.assertRaises(IndicatorEnrichmentError) as ctx:
            load_ohlcv_csv("/nonexistent/path/ZZZZ.csv")
        self.assertIn("not found", str(ctx.exception))


# ---------------------------------------------------------------------------
# T38 — insufficient bars raises IndicatorEnrichmentError
# ---------------------------------------------------------------------------

class TestInsufficientBars(unittest.TestCase):
    def test_50_bars_raises(self) -> None:
        from experiments.exp_penny.indicator_enrichment import (
            IndicatorEnrichmentError,
            compute_symbol_features,
            load_ohlcv_csv,
        )
        with tempfile.TemporaryDirectory() as tmpdir:
            _make_ohlcv_csv(tmpdir, "WEAK", n_bars=50)
            bars = load_ohlcv_csv(Path(tmpdir) / "WEAK.csv")
        with self.assertRaises(IndicatorEnrichmentError) as ctx:
            compute_symbol_features(bars)
        self.assertIn("Insufficient bars", str(ctx.exception))

    def test_219_bars_raises(self) -> None:
        from experiments.exp_penny.indicator_enrichment import (
            IndicatorEnrichmentError,
            compute_symbol_features,
            load_ohlcv_csv,
        )
        with tempfile.TemporaryDirectory() as tmpdir:
            _make_ohlcv_csv(tmpdir, "NEAR", n_bars=219)
            bars = load_ohlcv_csv(Path(tmpdir) / "NEAR.csv")
        with self.assertRaises(IndicatorEnrichmentError) as ctx:
            compute_symbol_features(bars)
        self.assertIn("Insufficient bars", str(ctx.exception))

    def test_220_bars_succeeds(self) -> None:
        from experiments.exp_penny.indicator_enrichment import compute_symbol_features, load_ohlcv_csv
        bars = load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV)
        self.assertEqual(len(bars), 220)
        features = compute_symbol_features(bars)
        self.assertIn("ma50", features)


# ---------------------------------------------------------------------------
# T39 — MA50 and MA200 are deterministic
# ---------------------------------------------------------------------------

class TestMaDeterminism(unittest.TestCase):
    def test_ma50_ma200_deterministic(self) -> None:
        from experiments.exp_penny.indicator_enrichment import compute_symbol_features, load_ohlcv_csv
        bars = load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV)
        f1 = compute_symbol_features(bars)
        f2 = compute_symbol_features(bars)
        self.assertEqual(f1["ma50"], f2["ma50"])
        self.assertEqual(f1["ma200"], f2["ma200"])

    def test_ma50_is_float(self) -> None:
        from experiments.exp_penny.indicator_enrichment import compute_symbol_features, load_ohlcv_csv
        bars = load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV)
        f = compute_symbol_features(bars)
        self.assertIsInstance(f["ma50"], float)
        self.assertGreater(f["ma50"], 0.0)

    def test_ma200_is_float(self) -> None:
        from experiments.exp_penny.indicator_enrichment import compute_symbol_features, load_ohlcv_csv
        bars = load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV)
        f = compute_symbol_features(bars)
        self.assertIsInstance(f["ma200"], float)
        self.assertGreater(f["ma200"], 0.0)

    def test_ma50_greater_than_ma200_for_uptrend(self) -> None:
        from experiments.exp_penny.indicator_enrichment import compute_symbol_features, load_ohlcv_csv
        bars = load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV)
        f = compute_symbol_features(bars)
        self.assertGreater(f["ma50"], f["ma200"])


# ---------------------------------------------------------------------------
# T40 — slope fields are present and positive for rising data
# ---------------------------------------------------------------------------

class TestSlopeFields(unittest.TestCase):
    def test_ma50_slope_present(self) -> None:
        from experiments.exp_penny.indicator_enrichment import compute_symbol_features, load_ohlcv_csv
        f = compute_symbol_features(load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV))
        self.assertIn("ma50_slope_20d", f)

    def test_ma200_slope_present(self) -> None:
        from experiments.exp_penny.indicator_enrichment import compute_symbol_features, load_ohlcv_csv
        f = compute_symbol_features(load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV))
        self.assertIn("ma200_slope_20d", f)

    def test_slopes_positive_for_uptrend(self) -> None:
        from experiments.exp_penny.indicator_enrichment import compute_symbol_features, load_ohlcv_csv
        f = compute_symbol_features(load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV))
        self.assertGreater(f["ma50_slope_20d"], 0.0)
        self.assertGreater(f["ma200_slope_20d"], 0.0)

    def test_slopes_are_float(self) -> None:
        from experiments.exp_penny.indicator_enrichment import compute_symbol_features, load_ohlcv_csv
        f = compute_symbol_features(load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV))
        self.assertIsInstance(f["ma50_slope_20d"], float)
        self.assertIsInstance(f["ma200_slope_20d"], float)


# ---------------------------------------------------------------------------
# T41 — consolidation high/low/range are present and sane
# ---------------------------------------------------------------------------

class TestConsolidationFields(unittest.TestCase):
    def setUp(self) -> None:
        from experiments.exp_penny.indicator_enrichment import compute_symbol_features, load_ohlcv_csv
        self.f = compute_symbol_features(load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV))

    def test_consolidation_high_present(self) -> None:
        self.assertIn("consolidation_high", self.f)
        self.assertIsInstance(self.f["consolidation_high"], float)

    def test_consolidation_low_present(self) -> None:
        self.assertIn("consolidation_low", self.f)
        self.assertIsInstance(self.f["consolidation_low"], float)

    def test_consolidation_range_pct_present(self) -> None:
        self.assertIn("consolidation_range_pct", self.f)
        self.assertIsInstance(self.f["consolidation_range_pct"], float)

    def test_high_greater_than_low(self) -> None:
        self.assertGreater(self.f["consolidation_high"], self.f["consolidation_low"])

    def test_range_pct_positive(self) -> None:
        self.assertGreater(self.f["consolidation_range_pct"], 0.0)

    def test_breakout_level_equals_consolidation_high(self) -> None:
        self.assertAlmostEqual(self.f["breakout_level"], self.f["consolidation_high"])


# ---------------------------------------------------------------------------
# T42 — breakout_rvol present and > 0
# ---------------------------------------------------------------------------

class TestBreakoutRvol(unittest.TestCase):
    def test_breakout_rvol_present(self) -> None:
        from experiments.exp_penny.indicator_enrichment import compute_symbol_features, load_ohlcv_csv
        f = compute_symbol_features(load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV))
        self.assertIn("breakout_rvol", f)

    def test_breakout_rvol_positive(self) -> None:
        from experiments.exp_penny.indicator_enrichment import compute_symbol_features, load_ohlcv_csv
        f = compute_symbol_features(load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV))
        self.assertGreater(f["breakout_rvol"], 0.0)

    def test_breakout_rvol_exceeds_scanner_minimum(self) -> None:
        from experiments.exp_penny.indicator_enrichment import compute_symbol_features, load_ohlcv_csv
        f = compute_symbol_features(load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV))
        # Latest vol=700k / adv=300k ≈ 2.33 — must exceed scanner min of 2.0
        self.assertGreater(f["breakout_rvol"], 2.0)


# ---------------------------------------------------------------------------
# T43 — adv_20d_shares and adv_20d_usd present
# ---------------------------------------------------------------------------

class TestAdvFields(unittest.TestCase):
    def setUp(self) -> None:
        from experiments.exp_penny.indicator_enrichment import compute_symbol_features, load_ohlcv_csv
        self.f = compute_symbol_features(load_ohlcv_csv(SAMPLE_ENRICH_PASS_OHLCV))

    def test_adv_20d_shares_present(self) -> None:
        self.assertIn("adv_20d_shares", self.f)
        self.assertIsInstance(self.f["adv_20d_shares"], int)
        self.assertGreater(self.f["adv_20d_shares"], 0)

    def test_adv_20d_usd_present(self) -> None:
        self.assertIn("adv_20d_usd", self.f)
        self.assertIsInstance(self.f["adv_20d_usd"], float)
        self.assertGreater(self.f["adv_20d_usd"], 0.0)

    def test_adv_20d_usd_meets_scanner_minimum(self) -> None:
        # adv_20d_shares=300k * latest_close=5.10 = 1.53M ≥ 500k scanner minimum
        self.assertGreater(self.f["adv_20d_usd"], 500_000.0)


# ---------------------------------------------------------------------------
# T44 — enrich_universe_rows fills missing computed fields
# ---------------------------------------------------------------------------

class TestEnrichUniverseRows(unittest.TestCase):
    def test_enrich_fills_missing_fields(self) -> None:
        from experiments.exp_penny.indicator_enrichment import enrich_universe_rows
        rows = [_enrichment_passing_base_row()]
        with tempfile.TemporaryDirectory() as tmpdir:
            _make_ohlcv_csv(tmpdir, "ENRICH_PASS")
            enriched = enrich_universe_rows(rows, ohlcv_dir=tmpdir)
        self.assertEqual(len(enriched), 1)
        r = enriched[0]
        for field in ("ma50", "ma200", "ma50_slope_20d", "ma200_slope_20d",
                      "consolidation_high", "consolidation_low", "consolidation_range_pct",
                      "breakout_level", "breakout_rvol", "adv_20d_shares", "adv_20d_usd"):
            self.assertIn(field, r, f"Enriched row must have '{field}'")
            self.assertIsNotNone(r[field], f"Enriched row field '{field}' must not be None")

    def test_enrich_does_not_overwrite_existing_field(self) -> None:
        from experiments.exp_penny.indicator_enrichment import enrich_universe_rows
        row = _enrichment_passing_base_row()
        row["ma50"] = 99.0  # pre-set
        with tempfile.TemporaryDirectory() as tmpdir:
            _make_ohlcv_csv(tmpdir, "ENRICH_PASS")
            enriched = enrich_universe_rows([row], ohlcv_dir=tmpdir)
        # Pre-set value preserved
        self.assertAlmostEqual(enriched[0]["ma50"], 99.0)

    def test_base_fields_preserved_after_enrichment(self) -> None:
        from experiments.exp_penny.indicator_enrichment import enrich_universe_rows
        row = _enrichment_passing_base_row()
        with tempfile.TemporaryDirectory() as tmpdir:
            _make_ohlcv_csv(tmpdir, "ENRICH_PASS")
            enriched = enrich_universe_rows([row], ohlcv_dir=tmpdir)
        r = enriched[0]
        self.assertEqual(r["symbol"], "ENRICH_PASS")
        self.assertAlmostEqual(r["price"], 5.10)
        self.assertAlmostEqual(r["bid"], 5.08)
        self.assertAlmostEqual(r["ask"], 5.12)
        self.assertFalse(r["gap_flag"])
        self.assertFalse(r["halt_flag"])


# ---------------------------------------------------------------------------
# T45 — enriched row passes PennyBreakoutScanner
# ---------------------------------------------------------------------------

class TestEnrichedRowPassesScanner(unittest.TestCase):
    def test_enriched_row_would_trade_true(self) -> None:
        from experiments.exp_penny.indicator_enrichment import enrich_universe_rows
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        rows = [_enrichment_passing_base_row()]
        with tempfile.TemporaryDirectory() as tmpdir:
            _make_ohlcv_csv(tmpdir, "ENRICH_PASS")
            enriched = enrich_universe_rows(rows, ohlcv_dir=tmpdir)
        records = PennyBreakoutScanner(universe=enriched).scan()
        self.assertEqual(len(records), 1)
        self.assertTrue(records[0]["would_trade"], f"Rejection: {records[0].get('rejection_reason')}")

    def test_enriched_record_null_order_ids(self) -> None:
        from experiments.exp_penny.indicator_enrichment import enrich_universe_rows
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        rows = [_enrichment_passing_base_row()]
        with tempfile.TemporaryDirectory() as tmpdir:
            _make_ohlcv_csv(tmpdir, "ENRICH_PASS")
            enriched = enrich_universe_rows(rows, ohlcv_dir=tmpdir)
        records = PennyBreakoutScanner(universe=enriched).scan()
        self.assertIsNone(records[0]["paper_order_id"])
        self.assertIsNone(records[0]["live_order_id"])

    def test_committed_fixture_enrichment_passes_scanner(self) -> None:
        from experiments.exp_penny.indicator_enrichment import enrich_universe_rows
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        from experiments.exp_penny.universe_loader import load_universe_for_enrichment
        rows = load_universe_for_enrichment(SAMPLE_ENRICHMENT_UNIVERSE)
        enriched = enrich_universe_rows(rows, ohlcv_dir=SAMPLES_OHLCV_DIR)
        records = PennyBreakoutScanner(universe=enriched).scan()
        passing = [r for r in records if r["would_trade"]]
        self.assertGreaterEqual(len(passing), 1, "Expected at least one passing candidate from enrichment")
        self.assertIn("ENRICH_PASS", [r["symbol"] for r in passing])


# ---------------------------------------------------------------------------
# T46 — runner works with --ohlcv-dir
# ---------------------------------------------------------------------------

class TestRunnerWithOhlcvDir(unittest.TestCase):
    def test_runner_ohlcv_dir_succeeds(self) -> None:
        from experiments.exp_engine.scanner_runner import run
        from experiments.exp_penny.indicator_enrichment import enrich_universe_rows
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        from experiments.exp_penny.universe_loader import load_universe_for_enrichment

        rows = load_universe_for_enrichment(SAMPLE_ENRICHMENT_UNIVERSE)
        with tempfile.TemporaryDirectory() as tmpdir:
            _make_ohlcv_csv(tmpdir, "ENRICH_PASS")
            enriched = enrich_universe_rows(rows, ohlcv_dir=tmpdir)
            scanner = PennyBreakoutScanner(universe=enriched)

            old = os.environ.pop("MQK_EXPERIMENTAL_ENGINE_ENABLED", None)
            try:
                os.environ["MQK_EXPERIMENTAL_ENGINE_ENABLED"] = "true"
                os.environ["MQK_EXPERIMENTAL_ENGINE_MODE"] = "scanner_only"
                os.environ["MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED"] = "false"
                os.environ["MQK_EXPERIMENTAL_JOURNAL_DIR"] = tmpdir
                exit_code = run(scanners=[scanner], argv=["--dry-run"])
            finally:
                if old is not None:
                    os.environ["MQK_EXPERIMENTAL_ENGINE_ENABLED"] = old
                else:
                    os.environ.pop("MQK_EXPERIMENTAL_ENGINE_ENABLED", None)

        self.assertEqual(exit_code, 0)

    def test_runner_ohlcv_jsonl_null_order_ids(self) -> None:
        from experiments.exp_engine.candidate_journal import CandidateJournalWriter
        from experiments.exp_penny.indicator_enrichment import enrich_universe_rows
        from experiments.exp_penny.scanner import PennyBreakoutScanner
        from experiments.exp_penny.universe_loader import load_universe_for_enrichment

        rows = load_universe_for_enrichment(SAMPLE_ENRICHMENT_UNIVERSE)
        with tempfile.TemporaryDirectory() as tmpdir:
            _make_ohlcv_csv(tmpdir, "ENRICH_PASS")
            enriched = enrich_universe_rows(rows, ohlcv_dir=tmpdir)
            records = PennyBreakoutScanner(universe=enriched).scan()
            with CandidateJournalWriter(journal_dir=tmpdir, engine_id="exp-engine-core-01") as writer:
                for rec in records:
                    writer.append(rec)
            files = list(Path(tmpdir).glob("*.jsonl"))
            self.assertEqual(len(files), 1)
            for line in files[0].read_text(encoding="utf-8").strip().splitlines():
                obj = json.loads(line)
                self.assertIsNone(obj["paper_order_id"])
                self.assertIsNone(obj["live_order_id"])


# ---------------------------------------------------------------------------
# T47 — missing OHLCV file raises clear IndicatorEnrichmentError
# ---------------------------------------------------------------------------

class TestMissingOhlcvFile(unittest.TestCase):
    def test_missing_symbol_file_raises_with_symbol_name(self) -> None:
        from experiments.exp_penny.indicator_enrichment import IndicatorEnrichmentError, enrich_universe_rows
        rows = [_enrichment_passing_base_row(symbol="NOSUCHSYM")]
        with tempfile.TemporaryDirectory() as tmpdir:
            with self.assertRaises(IndicatorEnrichmentError) as ctx:
                enrich_universe_rows(rows, ohlcv_dir=tmpdir)
        self.assertIn("NOSUCHSYM", str(ctx.exception))

    def test_missing_ohlcv_dir_raises(self) -> None:
        from experiments.exp_penny.indicator_enrichment import IndicatorEnrichmentError, enrich_universe_rows
        rows = [_enrichment_passing_base_row()]
        with self.assertRaises(IndicatorEnrichmentError):
            enrich_universe_rows(rows, ohlcv_dir="/nonexistent/ohlcv/dir")


# ---------------------------------------------------------------------------
# T48 — load_universe_for_enrichment: base fields required, computed not
# ---------------------------------------------------------------------------

class TestLoadUniverseForEnrichment(unittest.TestCase):
    def test_enrichment_loader_accepts_partial_universe(self) -> None:
        from experiments.exp_penny.universe_loader import load_universe_for_enrichment
        rows = load_universe_for_enrichment(SAMPLE_ENRICHMENT_UNIVERSE)
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["symbol"], "ENRICH_PASS")

    def test_enrichment_loader_rejects_missing_base_field(self) -> None:
        from experiments.exp_penny.universe_loader import UniverseLoadError, load_universe_for_enrichment
        text = "symbol,price,bid,ask,volume,gap_flag\nTST,5.10,5.08,5.12,700000,false\n"
        with tempfile.NamedTemporaryFile(suffix=".csv", mode="w", delete=False, encoding="utf-8") as f:
            f.write(text)
            tmp = f.name
        try:
            with self.assertRaises(UniverseLoadError) as ctx:
                load_universe_for_enrichment(tmp)
            self.assertIn("halt_flag", str(ctx.exception))
        finally:
            os.unlink(tmp)

    def test_strict_load_universe_still_requires_computed_fields(self) -> None:
        from experiments.exp_penny.universe_loader import UniverseLoadError, load_universe
        # Partial file without ma200_slope_20d must fail strict load_universe
        with self.assertRaises(UniverseLoadError):
            load_universe(SAMPLE_ENRICHMENT_UNIVERSE)


# ---------------------------------------------------------------------------
# T49 — no forbidden strings in indicator_enrichment.py
# ---------------------------------------------------------------------------

class TestIndicatorEnrichmentNoForbiddenStrings(unittest.TestCase):
    _FORBIDDEN = [
        "oms_outbox", "oms_inbox", "BrokerGateway", "broker_adapter",
        "alpaca", "Start-PaperTradingSmoke",
    ]
    _NETWORK_MODS = ["import requests", "import urllib", "import http.client", "import aiohttp"]

    def test_no_forbidden_strings_in_indicator_enrichment(self) -> None:
        src = Path(__file__).parent.parent / "indicator_enrichment.py"
        self.assertTrue(src.exists(), "indicator_enrichment.py must exist")
        content = src.read_text(encoding="utf-8")
        for forbidden in self._FORBIDDEN:
            self.assertNotIn(forbidden, content,
                             f"indicator_enrichment.py must not reference '{forbidden}'")

    def test_no_network_imports_in_indicator_enrichment(self) -> None:
        src = Path(__file__).parent.parent / "indicator_enrichment.py"
        content = src.read_text(encoding="utf-8")
        for mod in self._NETWORK_MODS:
            self.assertNotIn(mod, content,
                             f"indicator_enrichment.py must not contain '{mod}'")


if __name__ == "__main__":
    unittest.main()
