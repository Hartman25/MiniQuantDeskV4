"""
SCANNER-CANDIDATE-01 — MAIN scanner candidate artifact writer tests.

Run:
    $env:PYTHONPATH="research-py;research-py\\src"
    python -m unittest research-py/tests/test_scanner_candidate_artifacts.py
"""
from __future__ import annotations

import importlib
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

def _accepted_record(**overrides: Any) -> dict[str, Any]:
    base = {
        "scanner_id": "main-large-cap-01",
        "symbol": "AAPL",
        "exchange": "NASDAQ",
        "timeframe": "1D",
        "source": "alpaca_bars",
        "latest_bar_ts": "2026-06-03T20:00:00+00:00",
        "latest_close": 195.50,
        "lookback_close": 190.00,
        "move_bps": 289,
        "abs_move_bps": 289,
        "volume": 55_000_000,
        "avg_volume": 50_000_000.0,
        "relative_volume": 1.1,
        "atr": 2.80,
        "gap_pct": 0.0,
        "spread_estimate_bps": 2.0,
        "slippage_estimate_bps": 1.5,
        "round_trip_cost_bps": 7.0,
        "data_quality_score": 0.95,
        "liquidity_score": 0.90,
        "trend_score": 0.75,
        "momentum_score": 0.70,
        "mean_reversion_score": 0.30,
        "volatility_score": 0.60,
        "regime_label": "bull",
        "regime_score": 0.80,
        "risk_score": 0.25,
        "total_score": 0.72,
        "reason_tags": ["momentum_breakout"],
        "risk_tags": [],
        "eligible_for_scan": True,
        "eligible_for_backtest": True,
        "eligible_for_paper": True,
        "rejection_reason": None,
        "recommended_strategy": "intraday_scalper",
        "notes": None,
    }
    base.update(overrides)
    return base


def _rejected_record(**overrides: Any) -> dict[str, Any]:
    base = _accepted_record()
    base.update({
        "eligible_for_scan": False,
        "eligible_for_backtest": False,
        "eligible_for_paper": False,
        "rejection_reason": "liquidity_below_threshold",
        "reason_tags": [],
        "risk_tags": ["low_liquidity"],
    })
    base.update(overrides)
    return base


# ---------------------------------------------------------------------------
# SC-01: valid accepted candidate writes one JSONL row
# ---------------------------------------------------------------------------

class TestAcceptedCandidateWritesOneRow(unittest.TestCase):
    def test_write_one_row(self) -> None:
        from mqk_research.scanner.candidates import (
            ScannerCandidateWriter,
            build_scanner_candidate,
        )
        rec = build_scanner_candidate(**_accepted_record())
        with tempfile.TemporaryDirectory() as tmpdir:
            with ScannerCandidateWriter("main-large-cap-01", base_dir=tmpdir) as writer:
                path = writer.write_candidate(rec)
            lines = Path(path).read_text(encoding="utf-8").strip().splitlines()
        self.assertEqual(len(lines), 1)
        self.assertEqual(writer.accepted_count, 1)
        self.assertEqual(writer.rejected_count, 0)


# ---------------------------------------------------------------------------
# SC-02: required fields are present in written record
# ---------------------------------------------------------------------------

class TestRequiredFieldsPresent(unittest.TestCase):
    def test_required_fields(self) -> None:
        from mqk_research.scanner.candidates import (
            ScannerCandidateWriter,
            build_scanner_candidate,
            _REQUIRED_FIELDS,
        )
        rec = build_scanner_candidate(**_accepted_record())
        with tempfile.TemporaryDirectory() as tmpdir:
            with ScannerCandidateWriter("main-large-cap-01", base_dir=tmpdir) as writer:
                path = writer.write_candidate(rec)
            obj = json.loads(Path(path).read_text(encoding="utf-8").strip())
        for field in _REQUIRED_FIELDS:
            self.assertIn(field, obj, f"Missing required field: {field}")


# ---------------------------------------------------------------------------
# SC-03: schema_version is exactly "scanner-candidate-v1"
# ---------------------------------------------------------------------------

class TestSchemaVersion(unittest.TestCase):
    def test_schema_version_exact(self) -> None:
        from mqk_research.scanner.candidates import build_scanner_candidate
        rec = build_scanner_candidate(**_accepted_record())
        self.assertEqual(rec["schema_version"], "scanner-candidate-v1")


# ---------------------------------------------------------------------------
# SC-04: eligible_for_live forced False even if caller tries True
# ---------------------------------------------------------------------------

class TestEligibleForLiveForcedFalse(unittest.TestCase):
    def test_eligible_for_live_always_false(self) -> None:
        from mqk_research.scanner.candidates import build_scanner_candidate
        # build_scanner_candidate doesn't accept eligible_for_live as a param
        rec = build_scanner_candidate(**_accepted_record())
        self.assertIs(rec["eligible_for_live"], False)

    def test_validate_rejects_true(self) -> None:
        from mqk_research.scanner.candidates import (
            ScannerCandidateError,
            validate_scanner_candidate,
        )
        rec = build_from_dict_with_live_true()
        with self.assertRaises(ScannerCandidateError):
            validate_scanner_candidate(rec)


def build_from_dict_with_live_true() -> dict[str, Any]:
    """Construct a record bypassing build_scanner_candidate to test validator directly."""
    from mqk_research.scanner.candidates import SCHEMA_VERSION, ASSET_CLASS
    rec = _accepted_record()
    rec["schema_version"] = SCHEMA_VERSION
    rec["asset_class"] = ASSET_CLASS
    rec["eligible_for_live"] = True  # attempt to enable live
    return rec


# ---------------------------------------------------------------------------
# SC-05: rejected candidate writes rejection_reason
# ---------------------------------------------------------------------------

class TestRejectedCandidateWritesReason(unittest.TestCase):
    def test_rejection_reason_written(self) -> None:
        from mqk_research.scanner.candidates import (
            ScannerCandidateWriter,
            build_scanner_candidate,
        )
        rec = build_scanner_candidate(**_rejected_record())
        with tempfile.TemporaryDirectory() as tmpdir:
            with ScannerCandidateWriter("main-large-cap-01", base_dir=tmpdir) as writer:
                path = writer.write_rejection(rec)
            obj = json.loads(Path(path).read_text(encoding="utf-8").strip())
        self.assertEqual(obj["rejection_reason"], "liquidity_below_threshold")
        self.assertFalse(obj["eligible_for_scan"])

    def test_write_routes_to_rejection_file(self) -> None:
        from mqk_research.scanner.candidates import (
            ScannerCandidateWriter,
            build_scanner_candidate,
        )
        rec = build_scanner_candidate(**_rejected_record())
        with tempfile.TemporaryDirectory() as tmpdir:
            with ScannerCandidateWriter("main-large-cap-01", base_dir=tmpdir) as writer:
                writer.write(rec)
            self.assertEqual(writer.rejected_count, 1)
            self.assertEqual(writer.accepted_count, 0)


# ---------------------------------------------------------------------------
# SC-06: missing required field fails validation
# ---------------------------------------------------------------------------

class TestMissingFieldFails(unittest.TestCase):
    def test_missing_symbol_raises(self) -> None:
        from mqk_research.scanner.candidates import (
            ScannerCandidateError,
            validate_scanner_candidate,
            SCHEMA_VERSION,
            ASSET_CLASS,
        )
        rec = _accepted_record()
        rec["schema_version"] = SCHEMA_VERSION
        rec["asset_class"] = ASSET_CLASS
        rec["eligible_for_live"] = False
        del rec["symbol"]
        with self.assertRaises(ScannerCandidateError):
            validate_scanner_candidate(rec)

    def test_missing_total_score_raises(self) -> None:
        from mqk_research.scanner.candidates import (
            ScannerCandidateError,
            validate_scanner_candidate,
            SCHEMA_VERSION,
            ASSET_CLASS,
        )
        rec = _accepted_record()
        rec["schema_version"] = SCHEMA_VERSION
        rec["asset_class"] = ASSET_CLASS
        rec["eligible_for_live"] = False
        del rec["total_score"]
        with self.assertRaises(ScannerCandidateError):
            validate_scanner_candidate(rec)


# ---------------------------------------------------------------------------
# SC-07: reason_tags and risk_tags must be lists
# ---------------------------------------------------------------------------

class TestTagsMustBeLists(unittest.TestCase):
    def test_reason_tags_not_list_raises(self) -> None:
        from mqk_research.scanner.candidates import (
            ScannerCandidateError,
            validate_scanner_candidate,
            SCHEMA_VERSION,
            ASSET_CLASS,
        )
        rec = _accepted_record()
        rec["schema_version"] = SCHEMA_VERSION
        rec["asset_class"] = ASSET_CLASS
        rec["eligible_for_live"] = False
        rec["reason_tags"] = "momentum_breakout"  # string, not list
        with self.assertRaises(ScannerCandidateError):
            validate_scanner_candidate(rec)

    def test_risk_tags_not_list_raises(self) -> None:
        from mqk_research.scanner.candidates import (
            ScannerCandidateError,
            validate_scanner_candidate,
            SCHEMA_VERSION,
            ASSET_CLASS,
        )
        rec = _accepted_record()
        rec["schema_version"] = SCHEMA_VERSION
        rec["asset_class"] = ASSET_CLASS
        rec["eligible_for_live"] = False
        rec["risk_tags"] = None
        with self.assertRaises(ScannerCandidateError):
            validate_scanner_candidate(rec)


# ---------------------------------------------------------------------------
# SC-08: artifact parent directory is created
# ---------------------------------------------------------------------------

class TestArtifactDirCreated(unittest.TestCase):
    def test_parent_dir_created(self) -> None:
        from mqk_research.scanner.candidates import (
            ScannerCandidateWriter,
            build_scanner_candidate,
        )
        rec = build_scanner_candidate(**_accepted_record())
        with tempfile.TemporaryDirectory() as tmpdir:
            nested = os.path.join(tmpdir, "deep", "nested")
            with ScannerCandidateWriter("main-large-cap-01", base_dir=nested) as writer:
                path = writer.write_candidate(rec)
            self.assertTrue(Path(path).parent.exists())
            self.assertTrue(Path(path).exists())


# ---------------------------------------------------------------------------
# SC-09: load/readback preserves symbol, strategy, and scores
# ---------------------------------------------------------------------------

class TestLoadReadback(unittest.TestCase):
    def test_readback_preserves_fields(self) -> None:
        from mqk_research.scanner.candidates import (
            ScannerCandidateWriter,
            build_scanner_candidate,
            load_candidates,
        )
        rec = build_scanner_candidate(**_accepted_record())
        with tempfile.TemporaryDirectory() as tmpdir:
            with ScannerCandidateWriter("main-large-cap-01", base_dir=tmpdir) as writer:
                path = writer.write_candidate(rec)
            loaded = load_candidates(str(path))
        self.assertEqual(len(loaded), 1)
        obj = loaded[0]
        self.assertEqual(obj["symbol"], "AAPL")
        self.assertEqual(obj["recommended_strategy"], "intraday_scalper")
        self.assertAlmostEqual(obj["total_score"], 0.72)
        self.assertEqual(obj["schema_version"], "scanner-candidate-v1")
        self.assertIs(obj["eligible_for_live"], False)


# ---------------------------------------------------------------------------
# SC-10: no broker/OMS/execution imports in candidate module
# ---------------------------------------------------------------------------

class TestNoForbiddenImports(unittest.TestCase):
    _FORBIDDEN = [
        "BrokerGateway",
        "broker_adapter",
        "alpaca",
        "oms_outbox",
        "oms_inbox",
        "submit_order",
        "/v2/orders",
        "live_routing_enabled",
        "import requests",
        "import urllib",
        "import http.client",
        "import aiohttp",
        "import psycopg",
        "import sqlalchemy",
    ]

    def test_no_forbidden_in_candidates_module(self) -> None:
        candidates_path = (
            Path(__file__).parent.parent / "src" / "mqk_research" / "scanner" / "candidates.py"
        )
        self.assertTrue(candidates_path.exists(), f"Module not found: {candidates_path}")
        content = candidates_path.read_text(encoding="utf-8")
        for forbidden in self._FORBIDDEN:
            self.assertNotIn(
                forbidden,
                content,
                f"candidates.py must not reference '{forbidden}'",
            )


# ---------------------------------------------------------------------------
# SC-11: no order IDs required for MAIN candidates
# ---------------------------------------------------------------------------

class TestNoOrderIds(unittest.TestCase):
    def test_no_paper_order_id_field(self) -> None:
        from mqk_research.scanner.candidates import build_scanner_candidate
        rec = build_scanner_candidate(**_accepted_record())
        self.assertNotIn("paper_order_id", rec)

    def test_no_live_order_id_field(self) -> None:
        from mqk_research.scanner.candidates import build_scanner_candidate
        rec = build_scanner_candidate(**_accepted_record())
        self.assertNotIn("live_order_id", rec)


# ---------------------------------------------------------------------------
# SC-12: write routing for accepted and rejected via write()
# ---------------------------------------------------------------------------

class TestWriteRouting(unittest.TestCase):
    def test_write_accepted_goes_to_accepted_file(self) -> None:
        from mqk_research.scanner.candidates import (
            ScannerCandidateWriter,
            build_scanner_candidate,
        )
        accepted = build_scanner_candidate(**_accepted_record())
        rejected = build_scanner_candidate(**_rejected_record())
        with tempfile.TemporaryDirectory() as tmpdir:
            with ScannerCandidateWriter("main-large-cap-01", base_dir=tmpdir) as writer:
                writer.write(accepted)
                writer.write(rejected)
            self.assertEqual(writer.accepted_count, 1)
            self.assertEqual(writer.rejected_count, 1)


# ---------------------------------------------------------------------------
# SC-EXP: EXP exp-candidate-v1 writer still works (compatibility proof)
# ---------------------------------------------------------------------------

class TestExpCandidateCompatibility(unittest.TestCase):
    def test_exp_candidate_schema_version_unchanged(self) -> None:
        from experiments.exp_engine.candidate_journal import SCHEMA_VERSION as EXP_SCHEMA
        self.assertEqual(EXP_SCHEMA, "exp-candidate-v1")

    def test_exp_candidate_paper_order_id_null(self) -> None:
        from experiments.exp_engine.candidate_journal import build_candidate_record
        rec = build_candidate_record(
            engine_id="exp-engine-core-01",
            lane_id="penny-breakout",
            strategy_id="exp-penny-01",
            symbol="TEST",
            asset_class="us_equity",
            candidate_type="breakout",
            signal_direction="long",
        )
        self.assertIsNone(rec["paper_order_id"])
        self.assertIsNone(rec["live_order_id"])
        self.assertEqual(rec["schema_version"], "exp-candidate-v1")

    def test_exp_writer_appends_jsonl(self) -> None:
        from experiments.exp_engine.candidate_journal import (
            CandidateJournalWriter,
            build_candidate_record,
        )
        rec = build_candidate_record(
            engine_id="exp-engine-core-01",
            lane_id="penny-breakout",
            strategy_id="exp-penny-01",
            symbol="TEST",
            asset_class="us_equity",
            candidate_type="breakout",
            signal_direction="long",
        )
        with tempfile.TemporaryDirectory() as tmpdir:
            with CandidateJournalWriter(journal_dir=tmpdir, engine_id="exp-engine-core-01") as writer:
                writer.append(rec)
            files = list(Path(tmpdir).glob("*.jsonl"))
            self.assertEqual(len(files), 1)
            obj = json.loads(files[0].read_text(encoding="utf-8").strip())
        self.assertIsNone(obj["paper_order_id"])
        self.assertIsNone(obj["live_order_id"])
        self.assertEqual(obj["schema_version"], "exp-candidate-v1")


if __name__ == "__main__":
    unittest.main()
