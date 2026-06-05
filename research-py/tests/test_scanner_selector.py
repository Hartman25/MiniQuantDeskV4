"""
SCANNER-SELECTOR-01 — Unit tests for MAIN scanner ranked candidate selector.
"""
from __future__ import annotations

import importlib
import json
import os
import tempfile
import unittest
from pathlib import Path

from mqk_research.scanner.selector import (
    SelectorConfig,
    RankedCandidate,
    RankedCandidateExport,
    WatchlistArtifact,
    select_ranked_candidates,
    build_ranked_candidate_export,
    build_watchlist_artifact,
    write_ranked_candidate_export,
    write_watchlist_artifact,
    SCHEMA_VERSION_RANKED,
    SCHEMA_VERSION_WATCHLIST,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_candidate(
    symbol: str = "AAPL",
    total_score: float = 0.7,
    liquidity_score: float = 0.8,
    regime_score: float = 0.6,
    regime_label: str = "trending_up",
    risk_score: float = 0.9,
    eligible_for_scan: bool = True,
    eligible_for_backtest: bool = True,
    eligible_for_live: bool = False,
    rejection_reason=None,
    recommended_strategy: str = "intraday_scalper",
    timeframe: str = "5m",
    schema_version: str = "scanner-candidate-v1",
) -> dict:
    return {
        "schema_version": schema_version,
        "symbol": symbol,
        "total_score": total_score,
        "liquidity_score": liquidity_score,
        "regime_score": regime_score,
        "regime_label": regime_label,
        "risk_score": risk_score,
        "eligible_for_scan": eligible_for_scan,
        "eligible_for_backtest": eligible_for_backtest,
        "eligible_for_live": eligible_for_live,
        "rejection_reason": rejection_reason,
        "recommended_strategy": recommended_strategy,
        "timeframe": timeframe,
    }


# ---------------------------------------------------------------------------
# Test cases
# ---------------------------------------------------------------------------

class TestSelectRankedCandidates(unittest.TestCase):

    def test_t01_selects_accepted_candidates(self):
        """T01: selects only accepted scanner candidates."""
        c = _make_candidate()
        result = select_ranked_candidates([c])
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0]["symbol"], "AAPL")

    def test_t02_excludes_rejected_candidates(self):
        """T02: excludes candidates with rejection_reason set."""
        rejected = _make_candidate(
            eligible_for_scan=False,
            rejection_reason="dq_fail",
        )
        result = select_ranked_candidates([rejected])
        self.assertEqual(result, [])

    def test_t03_excludes_eligible_for_live_true(self):
        """T03: excludes candidates where eligible_for_live=True."""
        live = _make_candidate(eligible_for_live=True)
        result = select_ranked_candidates([live])
        self.assertEqual(result, [])

    def test_t04_excludes_below_min_total_score(self):
        """T04: excludes candidates below min_total_score."""
        cfg = SelectorConfig(min_total_score=0.5)
        low = _make_candidate(total_score=0.3)
        result = select_ranked_candidates([low], cfg)
        self.assertEqual(result, [])

    def test_t04b_includes_at_min_total_score(self):
        """T04b: includes candidate exactly at min_total_score."""
        cfg = SelectorConfig(min_total_score=0.5)
        at_threshold = _make_candidate(total_score=0.5)
        result = select_ranked_candidates([at_threshold], cfg)
        self.assertEqual(len(result), 1)

    def test_t05_ranks_by_total_score_descending(self):
        """T05: ranks by total_score descending."""
        low = _make_candidate(symbol="LOW", total_score=0.3)
        high = _make_candidate(symbol="HIGH", total_score=0.9)
        result = select_ranked_candidates([low, high])
        self.assertEqual(result[0]["symbol"], "HIGH")
        self.assertEqual(result[1]["symbol"], "LOW")

    def test_t06_tiebreak_liquidity_then_regime_then_symbol(self):
        """T06: tie-breaks by liquidity_score desc, regime_score desc, symbol asc."""
        a = _make_candidate(symbol="CCC", total_score=0.7, liquidity_score=0.9, regime_score=0.5)
        b = _make_candidate(symbol="AAA", total_score=0.7, liquidity_score=0.8, regime_score=0.9)
        c = _make_candidate(symbol="BBB", total_score=0.7, liquidity_score=0.8, regime_score=0.9)
        result = select_ranked_candidates([a, b, c])
        # a wins on liquidity; between b and c (tied liq+regime), symbol asc → b before c
        self.assertEqual(result[0]["symbol"], "CCC")
        self.assertEqual(result[1]["symbol"], "AAA")
        self.assertEqual(result[2]["symbol"], "BBB")

    def test_t07_deduplicates_by_symbol_keeps_best(self):
        """T07: deduplicates by symbol and keeps highest-ranked record."""
        best = _make_candidate(symbol="AAPL", total_score=0.9)
        worse = _make_candidate(symbol="AAPL", total_score=0.4)
        result = select_ranked_candidates([worse, best])
        self.assertEqual(len(result), 1)
        self.assertAlmostEqual(result[0]["total_score"], 0.9)

    def test_t08_limits_to_max_candidates(self):
        """T08: limits output to max_candidates."""
        cfg = SelectorConfig(max_candidates=2)
        candidates = [_make_candidate(symbol=f"SYM{i}", total_score=float(i) / 10) for i in range(5)]
        result = select_ranked_candidates(candidates, cfg)
        self.assertEqual(len(result), 2)

    def test_t21_deterministic_output_for_same_inputs(self):
        """T21: same inputs produce same output across multiple calls."""
        candidates = [_make_candidate(symbol=f"SYM{i}", total_score=float(i) / 10) for i in range(5)]
        r1 = select_ranked_candidates(candidates)
        r2 = select_ranked_candidates(candidates)
        self.assertEqual([c["symbol"] for c in r1], [c["symbol"] for c in r2])

    def test_t22_empty_candidate_list(self):
        """T22: empty input produces valid empty result."""
        result = select_ranked_candidates([])
        self.assertEqual(result, [])

    def test_excludes_wrong_schema_version(self):
        """Candidates with wrong schema_version are excluded."""
        wrong = _make_candidate(schema_version="exp-candidate-v1")
        result = select_ranked_candidates([wrong])
        self.assertEqual(result, [])

    def test_excludes_eligible_for_backtest_false(self):
        """Candidates not eligible for backtest are excluded."""
        c = _make_candidate(eligible_for_backtest=False)
        result = select_ranked_candidates([c])
        self.assertEqual(result, [])


class TestBuildRankedCandidateExport(unittest.TestCase):

    def test_t09_ranked_export_schema_version(self):
        """T09: ranked export has schema_version ranked-candidates-v1."""
        export = build_ranked_candidate_export([_make_candidate()])
        self.assertEqual(export.schema_version, SCHEMA_VERSION_RANKED)

    def test_export_candidate_count_matches(self):
        """Export candidate_count matches number of ranked candidates."""
        candidates = [_make_candidate(symbol=f"S{i}") for i in range(3)]
        export = build_ranked_candidate_export(candidates)
        self.assertEqual(export.candidate_count, 3)
        self.assertEqual(len(export.ranked_candidates), 3)

    def test_export_ranks_are_sequential(self):
        """Ranks are 1-based and sequential."""
        candidates = [_make_candidate(symbol=f"S{i}") for i in range(3)]
        export = build_ranked_candidate_export(candidates)
        ranks = [c.rank for c in export.ranked_candidates]
        self.assertEqual(ranks, [1, 2, 3])

    def test_t18_json_export_writes_and_reads_back(self):
        """T18: JSON ranked export writes and reads back correctly."""
        export = build_ranked_candidate_export([_make_candidate()])
        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "ranked.json")
            write_ranked_candidate_export(export, path)
            data = json.loads(Path(path).read_text(encoding="utf-8"))
        self.assertEqual(data["schema_version"], SCHEMA_VERSION_RANKED)
        self.assertEqual(data["candidate_count"], 1)
        self.assertEqual(len(data["ranked_candidates"]), 1)
        self.assertEqual(data["ranked_candidates"][0]["rank"], 1)

    def test_t22_empty_export(self):
        """T22: empty candidate list produces valid empty export."""
        export = build_ranked_candidate_export([])
        self.assertEqual(export.candidate_count, 0)
        self.assertEqual(export.ranked_candidates, [])


class TestBuildWatchlistArtifact(unittest.TestCase):

    def _get_export(self, n: int = 3) -> RankedCandidateExport:
        candidates = [_make_candidate(symbol=f"SYM{i}", total_score=float(10 - i) / 10) for i in range(n)]
        return build_ranked_candidate_export(candidates)

    def test_t10_watchlist_schema_version(self):
        """T10: watchlist schema is watchlist-v1."""
        wl = build_watchlist_artifact(self._get_export())
        self.assertEqual(wl.schema_version, SCHEMA_VERSION_WATCHLIST)

    def test_t11_watchlist_mode_is_paper(self):
        """T11: watchlist mode is paper."""
        wl = build_watchlist_artifact(self._get_export())
        self.assertEqual(wl.mode, "paper")

    def test_t12_approved_for_live_false(self):
        """T12: approved_for_live=False always."""
        wl = build_watchlist_artifact(self._get_export())
        self.assertFalse(wl.approved_for_live)

    def test_t13_approved_for_autonomous_paper_false(self):
        """T13: approved_for_autonomous_paper=False."""
        wl = build_watchlist_artifact(self._get_export())
        self.assertFalse(wl.approved_for_autonomous_paper)

    def test_t14_max_symbols_to_trade_1(self):
        """T14: max_symbols_to_trade=1 always (v1)."""
        cfg = SelectorConfig(max_symbols_to_trade=5)
        wl = build_watchlist_artifact(self._get_export(), cfg)
        self.assertEqual(wl.max_symbols_to_trade, 1)

    def test_t15_max_concurrent_positions_1(self):
        """T15: max_concurrent_positions=1 always (v1)."""
        cfg = SelectorConfig(max_concurrent_positions=5)
        wl = build_watchlist_artifact(self._get_export(), cfg)
        self.assertEqual(wl.max_concurrent_positions, 1)

    def test_t16_strategy_assignments_correct(self):
        """T16: strategy_assignments maps symbol to strategy_id."""
        export = build_ranked_candidate_export([_make_candidate(symbol="AAPL", recommended_strategy="intraday_scalper")])
        wl = build_watchlist_artifact(export)
        self.assertEqual(wl.strategy_assignments.get("AAPL"), "intraday_scalper")

    def test_t17_paper_qty_limits_and_notional_limits_present(self):
        """T17: paper_qty_limits and notional_limits are present per symbol."""
        export = build_ranked_candidate_export([_make_candidate(symbol="AAPL")])
        wl = build_watchlist_artifact(export)
        self.assertIn("AAPL", wl.paper_qty_limits)
        self.assertIn("AAPL", wl.notional_limits)
        self.assertIsInstance(wl.paper_qty_limits["AAPL"], int)
        self.assertIsInstance(wl.notional_limits["AAPL"], float)

    def test_symbols_limited_to_1(self):
        """Watchlist symbols is capped at 1 even when export has more."""
        export = self._get_export(n=3)
        wl = build_watchlist_artifact(export)
        self.assertEqual(len(wl.symbols), 1)

    def test_ranked_candidates_in_watchlist_is_full_list(self):
        """Watchlist ranked_candidates carries the full ranked list."""
        export = self._get_export(n=3)
        wl = build_watchlist_artifact(export)
        self.assertEqual(len(wl.ranked_candidates), 3)

    def test_t19_json_watchlist_writes_and_reads_back(self):
        """T19: JSON watchlist writes and reads back correctly."""
        export = build_ranked_candidate_export([_make_candidate()])
        wl = build_watchlist_artifact(export)
        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "watchlist.json")
            write_watchlist_artifact(wl, path)
            data = json.loads(Path(path).read_text(encoding="utf-8"))
        self.assertEqual(data["schema_version"], SCHEMA_VERSION_WATCHLIST)
        self.assertFalse(data["approved_for_live"])
        self.assertFalse(data["approved_for_autonomous_paper"])
        self.assertEqual(data["max_symbols_to_trade"], 1)
        self.assertEqual(data["max_concurrent_positions"], 1)

    def test_t22_empty_watchlist(self):
        """T22: empty export produces valid watchlist with empty symbols."""
        export = build_ranked_candidate_export([])
        wl = build_watchlist_artifact(export)
        self.assertEqual(wl.symbols, [])
        self.assertEqual(wl.ranked_candidates, [])

    def test_watchlist_to_dict_has_all_keys(self):
        """Watchlist to_dict has all required keys."""
        export = build_ranked_candidate_export([_make_candidate()])
        wl = build_watchlist_artifact(export)
        d = wl.to_dict()
        for key in [
            "schema_version", "generated_at_utc", "mode",
            "approved_for_autonomous_paper", "approved_for_live",
            "max_symbols_to_trade", "max_concurrent_positions",
            "symbols", "strategy_assignments", "paper_qty_limits",
            "notional_limits", "ranked_candidates", "selection_reason",
        ]:
            self.assertIn(key, d, f"Missing key: {key}")


class TestSelectorImportSafety(unittest.TestCase):

    def test_t20_no_forbidden_imports_in_selector(self):
        """T20: selector.py has no broker/OMS/execution/network/DB imports."""
        import mqk_research.scanner.selector as sel_mod
        src = Path(sel_mod.__file__).read_text(encoding="utf-8")
        forbidden = [
            "import requests",
            "import urllib",
            "import http.client",
            "import aiohttp",
            "import psycopg",
            "import sqlalchemy",
            "/v2/orders",
            "submit_order",
            "live_routing_enabled=true",
            "oms_outbox",
            "oms_inbox",
            "BrokerGateway",
            "broker_adapter",
        ]
        for token in forbidden:
            self.assertNotIn(token, src, f"selector.py contains forbidden token: {token!r}")

    def test_t23_generated_exports_not_committed(self):
        """T23: no exports/ files are staged in git."""
        import subprocess
        result = subprocess.run(
            ["git", "diff", "--cached", "--name-only"],
            capture_output=True, text=True,
        )
        staged = result.stdout.splitlines()
        export_hits = [f for f in staged if f.startswith("exports/")]
        self.assertEqual(export_hits, [], f"Staged export files detected: {export_hits}")


if __name__ == "__main__":
    unittest.main()
