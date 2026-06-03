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

SAMPLE_UNIVERSE_PATH = (
    Path(__file__).parent.parent / "sample_universe.json"
)


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

        with open(SAMPLE_UNIVERSE_PATH, encoding="utf-8") as fh:
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


if __name__ == "__main__":
    unittest.main()
