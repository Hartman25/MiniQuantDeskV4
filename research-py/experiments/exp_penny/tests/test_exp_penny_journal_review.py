"""
EXP-PENNY-01A-JOURNAL-REVIEW — unit tests (Python stdlib unittest only).

Run:
    $env:PYTHONPATH="research-py"
    python -m unittest discover -s research-py/experiments/exp_penny/tests -p "test_*.py"
"""
from __future__ import annotations

import importlib
import inspect
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any

# Ensure research-py is on the path when run standalone
_THIS_DIR = Path(__file__).parent
_RESEARCH_PY = _THIS_DIR.parent.parent.parent
if str(_RESEARCH_PY) not in sys.path:
    sys.path.insert(0, str(_RESEARCH_PY))

from experiments.exp_penny.review_journal import (
    CLASS_CLOSED_NO_CANDIDATES,
    CLASS_CLOSED_SCANNER_ONLY_PASS,
    CLASS_OPEN_EMPTY_JOURNAL,
    CLASS_OPEN_SAFETY_FAIL,
    CLASS_PARTIAL,
    find_latest_journal,
    load_journal,
    review_journal,
    write_summaries,
    ReviewResult,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_record(**overrides: Any) -> dict[str, Any]:
    """Return a minimal valid passing candidate record."""
    base: dict[str, Any] = {
        "schema_version": "exp-candidate-v1",
        "engine_id": "exp-engine-core-01",
        "lane_id": "exp-penny-01a",
        "strategy_id": "penny_breakout_accumulation_v1",
        "scanned_at_utc": "2026-06-03T10:00:00+00:00",
        "symbol": "PASS",
        "asset_class": "us_equity",
        "candidate_type": "breakout_accumulation",
        "signal_direction": "long",
        "would_trade": True,
        "paper_order_id": None,
        "live_order_id": None,
        "confidence_score": None,
        "risk_notes": "Stage 1 scanner-only.",
        "rejection_reason": None,
    }
    base.update(overrides)
    return base


def _make_reject_record(symbol: str = "FAIL", reason: str = "price_below_min") -> dict[str, Any]:
    return _make_record(
        symbol=symbol,
        would_trade=False,
        signal_direction="neutral",
        rejection_reason=reason,
    )


def _write_jsonl(tmp_dir: Path, name: str, records: list[dict]) -> Path:
    p = tmp_dir / name
    with open(p, "w", encoding="utf-8") as fh:
        for r in records:
            fh.write(json.dumps(r) + "\n")
    return p


# ---------------------------------------------------------------------------
# Test cases
# ---------------------------------------------------------------------------


class TestValidJournalSummary(unittest.TestCase):
    """T1: Valid journal with pass and reject rows summarises correctly."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.journal_dir = Path(self.tmp.name)
        records = [
            _make_record(symbol="A1"),
            _make_record(symbol="A2"),
            _make_reject_record("B1", "price_below_min"),
            _make_reject_record("B2", "price_below_min"),
            _make_reject_record("B3", "spread_too_wide"),
        ]
        self.journal_path = _write_jsonl(self.journal_dir, "journal_01.jsonl", records)

    def tearDown(self):
        self.tmp.cleanup()

    def test_counts(self):
        result = review_journal(self.journal_path)
        self.assertEqual(result.total_rows, 5)
        self.assertEqual(result.valid_rows, 5)
        self.assertEqual(result.would_trade_true, 2)
        self.assertEqual(result.would_trade_false, 3)
        self.assertEqual(result.malformed_rows, 0)

    def test_rejection_reason_counts(self):
        result = review_journal(self.journal_path)
        self.assertEqual(result.rejection_reason_counts["price_below_min"], 2)
        self.assertEqual(result.rejection_reason_counts["spread_too_wide"], 1)

    def test_classification(self):
        result = review_journal(self.journal_path)
        self.assertEqual(result.classification, CLASS_CLOSED_SCANNER_ONLY_PASS)

    def test_no_safety_failures(self):
        result = review_journal(self.journal_path)
        self.assertEqual(result.safety_failures, [])


class TestLatestJournalSelection(unittest.TestCase):
    """T2: Latest journal selection returns the most recently modified file."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_latest_selected(self):
        import time
        old = _write_jsonl(self.d, "exp-engine-core-01_20260601_000000.jsonl", [_make_record()])
        time.sleep(0.05)
        new = _write_jsonl(self.d, "exp-engine-core-01_20260603_120000.jsonl", [_make_record()])
        found = find_latest_journal(self.d)
        self.assertEqual(found.name, new.name)

    def test_none_when_empty_dir(self):
        self.assertIsNone(find_latest_journal(self.d))


class TestRejectionReasonCounts(unittest.TestCase):
    """T3: Rejection reason counts are tallied per unique reason."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = Path(self.tmp.name)
        records = [
            _make_reject_record("R1", "volume_below_min"),
            _make_reject_record("R2", "volume_below_min"),
            _make_reject_record("R3", "volume_below_min"),
            _make_reject_record("R4", "spread_too_wide"),
        ]
        self.p = _write_jsonl(self.d, "j.jsonl", records)

    def tearDown(self):
        self.tmp.cleanup()

    def test_counts(self):
        result = review_journal(self.p)
        self.assertEqual(result.rejection_reason_counts["volume_below_min"], 3)
        self.assertEqual(result.rejection_reason_counts["spread_too_wide"], 1)


class TestTopCandidatesRanking(unittest.TestCase):
    """T4: Top candidates prioritise would_trade=True, then confidence_score desc."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = Path(self.tmp.name)
        records = [
            _make_record(symbol="HIGH", confidence_score=0.9),
            _make_record(symbol="MED", confidence_score=0.5),
            _make_record(symbol="LOW", confidence_score=0.1),
            _make_reject_record("REJ1"),
        ]
        self.p = _write_jsonl(self.d, "j.jsonl", records)

    def tearDown(self):
        self.tmp.cleanup()

    def test_top_order(self):
        result = review_journal(self.p, top_n=3)
        symbols = [c["symbol"] for c in result.top_candidates]
        self.assertEqual(symbols[0], "HIGH")
        self.assertEqual(symbols[1], "MED")
        self.assertEqual(symbols[2], "LOW")

    def test_top_n_limit(self):
        result = review_journal(self.p, top_n=2)
        self.assertLessEqual(len(result.top_candidates), 2)

    def test_rejects_excluded_from_top(self):
        result = review_journal(self.p, top_n=10)
        symbols = [c["symbol"] for c in result.top_candidates]
        self.assertNotIn("REJ1", symbols)


class TestPaperOrderIdSafetyFailure(unittest.TestCase):
    """T5: Non-null paper_order_id fails safety check."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = Path(self.tmp.name)
        records = [_make_record(symbol="BAD", paper_order_id="ORD-123")]
        self.p = _write_jsonl(self.d, "j.jsonl", records)

    def tearDown(self):
        self.tmp.cleanup()

    def test_safety_fail(self):
        result = review_journal(self.p)
        self.assertEqual(result.classification, CLASS_OPEN_SAFETY_FAIL)
        self.assertTrue(any("paper_order_id" in f for f in result.safety_failures))


class TestLiveOrderIdSafetyFailure(unittest.TestCase):
    """T6: Non-null live_order_id fails safety check."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = Path(self.tmp.name)
        records = [_make_record(symbol="BAD", live_order_id="LIVE-999")]
        self.p = _write_jsonl(self.d, "j.jsonl", records)

    def tearDown(self):
        self.tmp.cleanup()

    def test_safety_fail(self):
        result = review_journal(self.p)
        self.assertEqual(result.classification, CLASS_OPEN_SAFETY_FAIL)
        self.assertTrue(any("live_order_id" in f for f in result.safety_failures))


class TestMalformedJsonLine(unittest.TestCase):
    """T7: Malformed JSON line fails with OPEN-SAFETY-FAIL (or PARTIAL)."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = Path(self.tmp.name)
        p = self.d / "j.jsonl"
        with open(p, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(_make_record()) + "\n")
            fh.write("{this is not valid json\n")
        self.p = p

    def tearDown(self):
        self.tmp.cleanup()

    def test_malformed_detected(self):
        result = review_journal(self.p)
        self.assertGreater(result.malformed_rows, 0)
        self.assertIn(result.classification, (CLASS_OPEN_SAFETY_FAIL, CLASS_PARTIAL))

    def test_safety_failure_message(self):
        result = review_journal(self.p)
        self.assertTrue(any("Malformed" in f for f in result.safety_failures))


class TestMissingRequiredField(unittest.TestCase):
    """T8: Missing required field produces schema error → OPEN-SAFETY-FAIL."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = Path(self.tmp.name)
        record = _make_record()
        del record["would_trade"]
        self.p = _write_jsonl(self.d, "j.jsonl", [record])

    def tearDown(self):
        self.tmp.cleanup()

    def test_schema_error(self):
        result = review_journal(self.p)
        self.assertEqual(result.classification, CLASS_OPEN_SAFETY_FAIL)
        self.assertTrue(any("missing required fields" in f for f in result.safety_failures))


class TestEmptyJournal(unittest.TestCase):
    """T9: Empty journal returns OPEN-EMPTY-JOURNAL."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = Path(self.tmp.name)
        self.p = _write_jsonl(self.d, "j.jsonl", [])

    def tearDown(self):
        self.tmp.cleanup()

    def test_empty_classification(self):
        result = review_journal(self.p)
        self.assertEqual(result.classification, CLASS_OPEN_EMPTY_JOURNAL)

    def test_zero_counts(self):
        result = review_journal(self.p)
        self.assertEqual(result.total_rows, 0)
        self.assertEqual(result.would_trade_true, 0)


class TestSummaryWrites(unittest.TestCase):
    """T10: Summary markdown and JSON write successfully."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = Path(self.tmp.name)
        records = [_make_record(), _make_reject_record()]
        self.journal_path = _write_jsonl(self.d, "test_journal.jsonl", records)
        self.review_dir = self.d / "reviews"

    def tearDown(self):
        self.tmp.cleanup()

    def test_writes_files(self):
        result = review_journal(self.journal_path)
        write_summaries(result, self.review_dir)
        self.assertIsNotNone(result.summary_md_path)
        self.assertIsNotNone(result.summary_json_path)
        self.assertTrue(Path(result.summary_md_path).exists())
        self.assertTrue(Path(result.summary_json_path).exists())

    def test_json_schema_version(self):
        result = review_journal(self.journal_path)
        write_summaries(result, self.review_dir)
        data = json.loads(Path(result.summary_json_path).read_text(encoding="utf-8"))
        self.assertEqual(data["schema_version"], "review-v1")
        self.assertIn("classification", data)
        self.assertIn("counts", data)

    def test_md_contains_classification(self):
        result = review_journal(self.journal_path)
        write_summaries(result, self.review_dir)
        md = Path(result.summary_md_path).read_text(encoding="utf-8")
        self.assertIn(result.classification, md)


class TestNoNetworkImports(unittest.TestCase):
    """T11: review_journal module must not import network modules."""

    def _get_module_source(self) -> str:
        import experiments.exp_penny.review_journal as rj
        src_file = inspect.getfile(rj)
        return Path(src_file).read_text(encoding="utf-8")

    def test_no_requests(self):
        src = self._get_module_source()
        self.assertNotIn("import requests", src)

    def test_no_urllib(self):
        src = self._get_module_source()
        self.assertNotIn("import urllib", src)

    def test_no_http_client(self):
        src = self._get_module_source()
        self.assertNotIn("import http.client", src)

    def test_no_aiohttp(self):
        src = self._get_module_source()
        self.assertNotIn("import aiohttp", src)


class TestNoForbiddenImports(unittest.TestCase):
    """T12: review_journal module must not import broker/OMS/DB modules."""

    def _get_module_source(self) -> str:
        import experiments.exp_penny.review_journal as rj
        src_file = inspect.getfile(rj)
        return Path(src_file).read_text(encoding="utf-8")

    def test_no_broker_gateway(self):
        src = self._get_module_source()
        self.assertNotIn("BrokerGateway", src)

    def test_no_broker_adapter(self):
        src = self._get_module_source()
        self.assertNotIn("broker_adapter", src)

    def test_no_alpaca(self):
        src = self._get_module_source()
        self.assertNotIn("alpaca", src)

    def test_no_oms_outbox(self):
        src = self._get_module_source()
        self.assertNotIn("oms_outbox", src)

    def test_no_oms_inbox(self):
        src = self._get_module_source()
        self.assertNotIn("oms_inbox", src)

    def test_no_psycopg(self):
        src = self._get_module_source()
        self.assertNotIn("psycopg", src)

    def test_no_sqlalchemy(self):
        src = self._get_module_source()
        self.assertNotIn("sqlalchemy", src)


class TestAllRejectsNoCandidate(unittest.TestCase):
    """T13: Journal with only rejects → CLOSED-NO-CANDIDATES."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = Path(self.tmp.name)
        records = [
            _make_reject_record("R1", "price_below_min"),
            _make_reject_record("R2", "spread_too_wide"),
        ]
        self.p = _write_jsonl(self.d, "j.jsonl", records)

    def tearDown(self):
        self.tmp.cleanup()

    def test_no_candidates_classification(self):
        result = review_journal(self.p)
        self.assertEqual(result.classification, CLASS_CLOSED_NO_CANDIDATES)


if __name__ == "__main__":
    unittest.main()
