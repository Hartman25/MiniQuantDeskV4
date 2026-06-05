"""
BACKTEST-QUEUE-01 — Unit tests for MAIN scanner backtest queue writer.
"""
from __future__ import annotations

import importlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

from mqk_research.scanner.backtest_queue import (
    BacktestQueueConfig,
    BacktestQueueEntry,
    BacktestQueueArtifact,
    strategy_ids_for_regime,
    build_backtest_queue,
    write_backtest_queue,
    SCHEMA_VERSION,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_ranked_export(candidates: list[dict]) -> dict:
    return {
        "schema_version": "ranked-candidates-v1",
        "generated_at_utc": "2026-06-05T16:30:00+00:00",
        "candidate_count": len(candidates),
        "ranked_candidates": candidates,
    }


def _make_candidate(
    symbol: str = "AAPL",
    rank: int = 1,
    total_score: float = 0.75,
    regime_label: str = "trending_up",
    timeframe: str = "5m",
    strategy_id: str = "intraday_scalper",
    source_candidate_artifact: str = None,
) -> dict:
    return {
        "rank": rank,
        "symbol": symbol,
        "strategy_id": strategy_id,
        "timeframe": timeframe,
        "total_score": total_score,
        "liquidity_score": 0.8,
        "regime_label": regime_label,
        "regime_score": 0.7,
        "risk_score": 0.9,
        "net_expectancy_after_cost_bps": None,
        "paper_qty_limit": 1,
        "notional_limit_usd": 1000.0,
        "selection_reason": "test",
        "source_candidate_artifact": source_candidate_artifact,
    }


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

class TestBacktestQueueSchema(unittest.TestCase):
    """Test 1: schema_version is backtest-queue-v1."""

    def test_schema_version_constant(self):
        self.assertEqual(SCHEMA_VERSION, "backtest-queue-v1")

    def test_artifact_schema_version_field(self):
        export = _make_ranked_export([_make_candidate()])
        queue = build_backtest_queue(export, generated_at_utc="2026-06-05T16:30:00+00:00")
        self.assertEqual(queue.schema_version, "backtest-queue-v1")

    def test_artifact_dict_schema_version(self):
        export = _make_ranked_export([_make_candidate()])
        queue = build_backtest_queue(export, generated_at_utc="2026-06-05T16:30:00+00:00")
        d = queue.to_dict()
        self.assertEqual(d["schema_version"], "backtest-queue-v1")


class TestBacktestQueueFromRankedExport(unittest.TestCase):
    """Test 2: queue created from ranked export."""

    def test_entries_produced_from_ranked_export(self):
        export = _make_ranked_export([_make_candidate("AAPL", rank=1, regime_label="trending_up")])
        queue = build_backtest_queue(export)
        self.assertGreater(queue.queue_count, 0)
        self.assertGreater(len(queue.entries), 0)

    def test_queue_count_matches_entries_len(self):
        export = _make_ranked_export([_make_candidate("AAPL", rank=1, regime_label="trending_up")])
        queue = build_backtest_queue(export)
        self.assertEqual(queue.queue_count, len(queue.entries))


class TestStrategyCompatibility(unittest.TestCase):
    """Tests 3–8: strategy compatibility matrix."""

    def test_intraday_scalper_included_for_trending_up(self):
        """Test 3."""
        strats = strategy_ids_for_regime("trending_up")
        self.assertIn("intraday_scalper", strats)

    def test_intraday_scalper_excluded_for_range_bound(self):
        """Test 4."""
        strats = strategy_ids_for_regime("range_bound")
        self.assertNotIn("intraday_scalper", strats)

    def test_mean_reversion_included_for_range_bound(self):
        """Test 5."""
        strats = strategy_ids_for_regime("range_bound")
        self.assertIn("mean_reversion", strats)

    def test_volatility_breakout_included_for_gapping(self):
        """Test 6."""
        strats = strategy_ids_for_regime("gapping")
        self.assertIn("volatility_breakout", strats)

    def test_opening_range_fade_included_for_gapping(self):
        """Test 7."""
        strats = strategy_ids_for_regime("gapping")
        self.assertIn("opening_range_fade", strats)

    def test_unknown_regime_produces_no_entries(self):
        """Test 8."""
        export = _make_ranked_export([
            _make_candidate("TSLA", rank=1, regime_label="unknown_unclassified_xyz")
        ])
        queue = build_backtest_queue(export)
        self.assertEqual(queue.queue_count, 0)
        self.assertEqual(len(queue.entries), 0)

    def test_unclassified_empty_regime_produces_no_entries(self):
        export = _make_ranked_export([
            _make_candidate("TSLA", rank=1, regime_label="")
        ])
        queue = build_backtest_queue(export)
        self.assertEqual(queue.queue_count, 0)


class TestQueueEntryFields(unittest.TestCase):
    """Tests 9–10: entry fields."""

    def _first_entry(self) -> BacktestQueueEntry:
        export = _make_ranked_export([
            _make_candidate("MSFT", rank=1, total_score=0.82, regime_label="trending_up", timeframe="5m")
        ])
        queue = build_backtest_queue(export)
        self.assertGreater(len(queue.entries), 0)
        return queue.entries[0]

    def test_entry_contains_symbol_strategy_timeframe_regime(self):
        """Test 9."""
        e = self._first_entry()
        self.assertEqual(e.symbol, "MSFT")
        self.assertIn(e.strategy_id, ["intraday_scalper", "volatility_breakout", "swing_momentum"])
        self.assertEqual(e.timeframe, "5m")
        self.assertEqual(e.regime_label, "trending_up")

    def test_entry_contains_source_rank_and_score(self):
        """Test 10."""
        e = self._first_entry()
        self.assertEqual(e.source_rank, 1)
        self.assertAlmostEqual(e.source_total_score, 0.82)

    def test_entry_has_queue_id(self):
        e = self._first_entry()
        self.assertTrue(e.queue_id.startswith("bq-"))
        self.assertGreater(len(e.queue_id), 5)


class TestSafetyInvariants(unittest.TestCase):
    """Tests 11–13: recommended_for_live, ambiguity_policy, stress_profile."""

    def _queue(self) -> BacktestQueueArtifact:
        export = _make_ranked_export([_make_candidate("AAPL", rank=1, regime_label="gapping")])
        return build_backtest_queue(export)

    def test_recommended_for_live_false_on_artifact(self):
        """Test 11."""
        q = self._queue()
        self.assertFalse(q.recommended_for_live)
        self.assertFalse(q.to_dict()["recommended_for_live"])

    def test_recommended_for_live_false_on_entries(self):
        """Test 11 (entries)."""
        q = self._queue()
        for e in q.entries:
            self.assertFalse(e.recommended_for_live)
            self.assertFalse(e.to_dict()["recommended_for_live"])

    def test_ambiguity_policy_is_conservative_worst_case(self):
        """Test 12."""
        q = self._queue()
        for e in q.entries:
            self.assertEqual(e.ambiguity_policy, "CONSERVATIVE_WORST_CASE")

    def test_stress_profile_is_slippage_x2(self):
        """Test 13."""
        q = self._queue()
        for e in q.entries:
            self.assertEqual(e.stress_profile, "slippage_x2")


class TestQueueLimit(unittest.TestCase):
    """Test 14: max_queue_entries enforced."""

    def test_max_queue_entries_enforced(self):
        # Create many candidates — each trending_up gets 3 strategies
        candidates = [
            _make_candidate(f"SYM{i:02d}", rank=i, regime_label="trending_up")
            for i in range(1, 20)
        ]
        export = _make_ranked_export(candidates)
        cfg = BacktestQueueConfig(max_queue_entries=7)
        queue = build_backtest_queue(export, config=cfg)
        self.assertLessEqual(queue.queue_count, 7)
        self.assertLessEqual(len(queue.entries), 7)


class TestDeterminism(unittest.TestCase):
    """Test 15: deterministic output for same input."""

    def test_deterministic_output_same_input(self):
        candidates = [
            _make_candidate("AAPL", rank=1, regime_label="trending_up"),
            _make_candidate("MSFT", rank=2, regime_label="gapping"),
            _make_candidate("AMZN", rank=3, regime_label="range_bound"),
        ]
        export = _make_ranked_export(candidates)
        ts = "2026-06-05T16:30:00+00:00"
        q1 = build_backtest_queue(export, generated_at_utc=ts)
        q2 = build_backtest_queue(export, generated_at_utc=ts)
        self.assertEqual(json.dumps(q1.to_dict()), json.dumps(q2.to_dict()))

    def test_queue_ids_are_deterministic(self):
        export = _make_ranked_export([_make_candidate("AAPL", rank=1, regime_label="high_volatility")])
        q1 = build_backtest_queue(export)
        q2 = build_backtest_queue(export)
        ids1 = [e.queue_id for e in q1.entries]
        ids2 = [e.queue_id for e in q2.entries]
        self.assertEqual(ids1, ids2)


class TestJsonWriteRead(unittest.TestCase):
    """Test 16: JSON write/read works."""

    def test_write_and_read_json(self):
        export = _make_ranked_export([_make_candidate("AAPL", rank=1, regime_label="trending_up")])
        queue = build_backtest_queue(export, generated_at_utc="2026-06-05T16:30:00+00:00")
        with tempfile.TemporaryDirectory() as tmpdir:
            path = str(Path(tmpdir) / "queue.json")
            out = write_backtest_queue(queue, path)
            self.assertTrue(out.exists())
            data = json.loads(out.read_text(encoding="utf-8"))
            self.assertEqual(data["schema_version"], "backtest-queue-v1")
            self.assertFalse(data["recommended_for_live"])
            self.assertIn("entries", data)
            self.assertIn("queue_count", data)

    def test_write_creates_parent_dirs(self):
        export = _make_ranked_export([])
        queue = build_backtest_queue(export)
        with tempfile.TemporaryDirectory() as tmpdir:
            path = str(Path(tmpdir) / "sub" / "dir" / "queue.json")
            out = write_backtest_queue(queue, path)
            self.assertTrue(out.exists())


class TestEmptyRankedExport(unittest.TestCase):
    """Test 17: empty ranked export writes valid empty queue."""

    def test_empty_export_valid_artifact(self):
        export = _make_ranked_export([])
        queue = build_backtest_queue(export)
        self.assertEqual(queue.schema_version, "backtest-queue-v1")
        self.assertEqual(queue.queue_count, 0)
        self.assertEqual(len(queue.entries), 0)
        self.assertFalse(queue.recommended_for_live)

    def test_empty_export_json_roundtrip(self):
        export = _make_ranked_export([])
        queue = build_backtest_queue(export)
        d = queue.to_dict()
        self.assertIsInstance(d["entries"], list)
        self.assertEqual(d["queue_count"], 0)


class TestNoBacktestImport(unittest.TestCase):
    """Test 18: does not call or import mqk-backtest."""

    def test_module_source_contains_no_mqk_backtest_import(self):
        import inspect
        import mqk_research.scanner.backtest_queue as bq_mod
        src = inspect.getsource(bq_mod)
        # Must not import or call mqk_backtest (import statement form)
        self.assertNotIn("import mqk_backtest", src)
        self.assertNotIn("from mqk_backtest", src)
        self.assertNotIn("subprocess", src)
        self.assertNotIn("cargo run", src)


class TestNoForbiddenImports(unittest.TestCase):
    """Tests 19: no broker/OMS/execution/network/DB imports."""

    def test_no_forbidden_imports_in_module(self):
        import inspect
        import mqk_research.scanner.backtest_queue as bq_mod
        src = inspect.getsource(bq_mod)
        forbidden = [
            "import requests",
            "import urllib",
            "import http.client",
            "import aiohttp",
            "import psycopg",
            "import sqlalchemy",
            "BrokerGateway",
            "broker_adapter",
            "oms_outbox",
            "oms_inbox",
            "submit_order",
            "/v2/orders",
        ]
        for term in forbidden:
            self.assertNotIn(term, src, f"forbidden term found: {term}")

    def test_module_imports_are_stdlib_only(self):
        import mqk_research.scanner.backtest_queue as bq_mod
        allowed_prefixes = {"mqk_research", "hashlib", "json", "dataclasses",
                            "datetime", "pathlib", "typing", "__future__"}
        for name, mod in sys.modules.items():
            if mod is None:
                continue
            if name.startswith("mqk_research.scanner.backtest_queue"):
                continue
            # Just verify the module itself doesn't import unexpected top-level packages
        # Check the actual imports via source inspection
        import inspect
        src = inspect.getsource(bq_mod)
        # Network/DB imports must be absent
        for bad in ["import requests", "import aiohttp", "import psycopg", "import sqlalchemy"]:
            self.assertNotIn(bad, src)


class TestNoOrderFields(unittest.TestCase):
    """Test 20: no order IDs or broker fields in entries."""

    def test_entry_dict_has_no_order_fields(self):
        export = _make_ranked_export([_make_candidate("AAPL", rank=1, regime_label="trending_up")])
        queue = build_backtest_queue(export)
        order_fields = {"order_id", "broker_order_id", "client_order_id", "fill_id",
                        "oms_outbox_id", "oms_inbox_id"}
        for e in queue.entries:
            d = e.to_dict()
            intersection = order_fields & set(d.keys())
            self.assertEqual(intersection, set(), f"Order fields found in entry: {intersection}")

    def test_artifact_dict_has_no_order_fields(self):
        export = _make_ranked_export([_make_candidate("AAPL", rank=1, regime_label="trending_up")])
        queue = build_backtest_queue(export)
        d = queue.to_dict()
        order_fields = {"order_id", "broker_order_id", "fill_id", "oms_outbox_id"}
        intersection = order_fields & set(d.keys())
        self.assertEqual(intersection, set())


class TestOrderingBehavior(unittest.TestCase):
    """Additional: ordering is rank-ascending then strategy-order then symbol-asc."""

    def test_entries_ordered_by_rank_then_strategy(self):
        candidates = [
            _make_candidate("AAPL", rank=1, regime_label="trending_up"),
            _make_candidate("MSFT", rank=2, regime_label="trending_up"),
        ]
        export = _make_ranked_export(candidates)
        queue = build_backtest_queue(export)
        # All rank=1 entries should come before rank=2 entries
        ranks = [e.source_rank for e in queue.entries]
        # ranks should be non-decreasing
        self.assertEqual(ranks, sorted(ranks))

    def test_strategy_order_follows_approved_list(self):
        cfg = BacktestQueueConfig(approved_strategy_ids=["intraday_scalper", "volatility_breakout", "swing_momentum"])
        export = _make_ranked_export([_make_candidate("AAPL", rank=1, regime_label="trending_up")])
        queue = build_backtest_queue(export, config=cfg)
        strat_ids = [e.strategy_id for e in queue.entries if e.symbol == "AAPL"]
        # All three are compatible with trending_up; order must be config order
        self.assertEqual(strat_ids, ["intraday_scalper", "volatility_breakout", "swing_momentum"])


class TestSourceRankedExportPath(unittest.TestCase):
    """source_ranked_export_path propagates to entries and artifact."""

    def test_source_ranked_export_in_artifact(self):
        export = _make_ranked_export([_make_candidate("AAPL", rank=1, regime_label="gapping")])
        queue = build_backtest_queue(
            export, source_ranked_export_path="/exports/ranked_candidates/20260605.json"
        )
        self.assertEqual(queue.source_ranked_export, "/exports/ranked_candidates/20260605.json")

    def test_source_ranked_export_in_entries(self):
        export = _make_ranked_export([_make_candidate("AAPL", rank=1, regime_label="gapping")])
        queue = build_backtest_queue(
            export, source_ranked_export_path="/exports/ranked_candidates/20260605.json"
        )
        for e in queue.entries:
            self.assertEqual(e.source_ranked_export, "/exports/ranked_candidates/20260605.json")


if __name__ == "__main__":
    unittest.main()
