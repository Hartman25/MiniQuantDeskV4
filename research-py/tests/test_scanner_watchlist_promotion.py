"""
WATCHLIST-PROMO-01 — Unit tests for MAIN scanner watchlist promotion gate.
"""
from __future__ import annotations

import json
import os
import tempfile
import unittest
from pathlib import Path

from mqk_research.scanner.watchlist_promotion import (
    SCHEMA_VERSION_WATCHLIST,
    SCHEMA_VERSION_WATCHLIST_V2,
    MULTI_SYMBOL_HARD_CEILING,
    REASON_WATCHLIST_SCHEMA_INVALID,
    REASON_WATCHLIST_MODE_NOT_PAPER,
    REASON_WATCHLIST_LIVE_NOT_LOCKED,
    REASON_NO_RANKED_CANDIDATES,
    REASON_STRATEGY_FIT_MISSING,
    REASON_STRATEGY_FIT_SYMBOL_MISMATCH,
    REASON_STRATEGY_FIT_STRATEGY_MISMATCH,
    REASON_STRATEGY_FIT_NOT_RECOMMENDED_FOR_PAPER,
    REASON_RISK_SIMULATION_REQUIRED,
    REASON_OPERATOR_REVIEW_REQUIRED,
    REASON_PREMARKET_REVALIDATION_REQUIRED,
    REASON_LIVE_APPROVAL_FORBIDDEN,
    REASON_WATCHLIST_V2_SYMBOL_ASSIGNMENT_MISSING,
    REASON_WATCHLIST_V2_MAX_SYMBOLS_INVALID,
    REASON_WATCHLIST_V2_CONCURRENT_LIMIT_INVALID,
    REASON_WATCHLIST_V2_HARD_CEILING_EXCEEDED,
    WatchlistPromotionConfig,
    PromotionInput,
    PromotionDecision,
    evaluate_watchlist_promotion,
    apply_watchlist_promotion,
    write_promoted_watchlist,
    _is_watchlist_v2,
    _selected_symbols_for_promotion,
)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

def _make_watchlist(
    symbols: list | None = None,
    mode: str = "paper",
    approved_for_live: bool = False,
    approved_for_autonomous_paper: bool = False,
    strategy_assignments: dict | None = None,
    schema_version: str = "watchlist-v1",
) -> dict:
    syms = symbols if symbols is not None else ["AAPL"]
    sa = strategy_assignments if strategy_assignments is not None else {
        s: "intraday_scalper" for s in syms
    }
    return {
        "schema_version": schema_version,
        "generated_at_utc": "2026-06-05T22:30:00+00:00",
        "mode": mode,
        "approved_for_autonomous_paper": approved_for_autonomous_paper,
        "approved_for_live": approved_for_live,
        "max_symbols_to_trade": 1,
        "max_concurrent_positions": 1,
        "symbols": list(syms),
        "strategy_assignments": sa,
        "paper_qty_limits": {s: 1 for s in syms},
        "notional_limits": {s: 1000.0 for s in syms},
        "ranked_candidates": [{"rank": i + 1, "symbol": s} for i, s in enumerate(syms)],
        "selection_reason": "selector_output_only",
    }


def _make_passing_fit(
    symbol: str = "AAPL",
    strategy_id: str = "intraday_scalper",
) -> dict:
    return {
        "schema_version": "strategy-fit-v1",
        "symbol": symbol,
        "strategy_id": strategy_id,
        "recommended_for_paper": True,
        "recommended_for_live": False,
        "failure_reasons": [],
    }


def _make_blocked_fit(symbol: str = "AAPL") -> dict:
    return {
        "schema_version": "strategy-fit-v1",
        "symbol": symbol,
        "strategy_id": "intraday_scalper",
        "status": "blocked_no_backtest_interface",
        "recommended_for_paper": False,
        "recommended_for_live": False,
        "failure_reasons": ["blocked_no_backtest_interface"],
    }


def _make_passing_config() -> WatchlistPromotionConfig:
    return WatchlistPromotionConfig(
        operator_review_approved=True,
        risk_simulation_passed=True,
        premarket_revalidation_required=False,
    )


# ---------------------------------------------------------------------------
# Test 1: Full approval path
# ---------------------------------------------------------------------------

class TestFullApproval(unittest.TestCase):

    def _setup(self):
        watchlist = _make_watchlist(symbols=["AAPL"])
        fit_artifacts = {"AAPL": _make_passing_fit("AAPL")}
        config = _make_passing_config()
        return watchlist, fit_artifacts, config

    def test_t01_all_gates_pass_approves_paper(self):
        """T01: valid watchlist + passing strategy-fit + risk + operator + no premarket → approved."""
        wl, fits, cfg = self._setup()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertTrue(decision.approved_for_autonomous_paper)
        self.assertTrue(decision.passed)
        self.assertEqual(decision.failure_reasons, [])
        self.assertIn("AAPL", decision.approved_symbols)

    def test_t02_approved_for_live_always_false(self):
        """T02: approved_for_live is always False even when all gates pass."""
        wl, fits, cfg = self._setup()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_live)

    def test_t14_strategy_assignment_preserved_for_approved_symbol(self):
        """T14: strategy assignment from fit artifact is preserved in decision."""
        wl, fits, cfg = self._setup()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertIn("AAPL", decision.strategy_assignments)
        self.assertEqual(decision.strategy_assignments["AAPL"], "intraday_scalper")

    def test_t15_only_top_ranked_symbol_approved_v1(self):
        """T15: even with two symbols, only the top-ranked one is approved in v1."""
        wl = _make_watchlist(symbols=["AAPL", "MSFT"])
        fits = {
            "AAPL": _make_passing_fit("AAPL"),
            "MSFT": _make_passing_fit("MSFT"),
        }
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertEqual(decision.approved_symbols, ["AAPL"])
        self.assertNotIn("MSFT", decision.approved_symbols)

    def test_approved_for_live_false_in_applied_artifact(self):
        """approved_for_live=False in the applied artifact."""
        wl, fits, cfg = self._setup()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        promoted = apply_watchlist_promotion(wl, decision, cfg)
        self.assertFalse(promoted["approved_for_live"])


# ---------------------------------------------------------------------------
# Test 3: Operator review defaults false and blocks
# ---------------------------------------------------------------------------

class TestOperatorReview(unittest.TestCase):

    def test_t03_operator_review_default_false_blocks(self):
        """T03: operator_review_approved defaults False — blocks promotion."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = WatchlistPromotionConfig(
            risk_simulation_passed=True,
            premarket_revalidation_required=False,
            # operator_review_approved defaults to False
        )
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_OPERATOR_REVIEW_REQUIRED, decision.failure_reasons)

    def test_operator_review_false_explicit_blocks(self):
        """Explicitly False operator review blocks promotion."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = WatchlistPromotionConfig(
            operator_review_approved=False,
            risk_simulation_passed=True,
            premarket_revalidation_required=False,
        )
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_OPERATOR_REVIEW_REQUIRED, decision.failure_reasons)

    def test_operator_review_true_allows_gate(self):
        """operator_review_approved=True clears this gate."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertNotIn(REASON_OPERATOR_REVIEW_REQUIRED, decision.failure_reasons)


# ---------------------------------------------------------------------------
# Test 4: Risk simulation defaults false and blocks
# ---------------------------------------------------------------------------

class TestRiskSimulation(unittest.TestCase):

    def test_t04_risk_simulation_default_false_blocks(self):
        """T04: risk_simulation_passed defaults False — blocks promotion."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = WatchlistPromotionConfig(
            operator_review_approved=True,
            premarket_revalidation_required=False,
            # risk_simulation_passed defaults to False
        )
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_RISK_SIMULATION_REQUIRED, decision.failure_reasons)

    def test_risk_simulation_true_clears_gate(self):
        """risk_simulation_passed=True clears this gate."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertNotIn(REASON_RISK_SIMULATION_REQUIRED, decision.failure_reasons)


# ---------------------------------------------------------------------------
# Tests 5-6: Strategy-fit gates
# ---------------------------------------------------------------------------

class TestStrategyFit(unittest.TestCase):

    def test_t05_missing_strategy_fit_blocks(self):
        """T05: no strategy-fit artifact for top symbol blocks promotion."""
        wl = _make_watchlist(symbols=["AAPL"])
        fits: dict = {}  # no artifact for AAPL
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_STRATEGY_FIT_MISSING, decision.failure_reasons)

    def test_t06_strategy_fit_not_recommended_blocks(self):
        """T06: strategy-fit with recommended_for_paper=False blocks promotion."""
        wl = _make_watchlist(symbols=["AAPL"])
        fit = _make_passing_fit("AAPL")
        fit["recommended_for_paper"] = False
        fits = {"AAPL": fit}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_STRATEGY_FIT_NOT_RECOMMENDED_FOR_PAPER, decision.failure_reasons)

    def test_t23_blocked_strategy_fit_does_not_promote(self):
        """T23: backtest-blocked strategy-fit artifacts do not promote."""
        from mqk_research.scanner.backtest_gates import apply_backtest_gates
        blocked_runner_artifact = _make_blocked_fit("AAPL")
        evaluated = apply_backtest_gates(blocked_runner_artifact)
        self.assertFalse(evaluated["recommended_for_paper"])

        wl = _make_watchlist(symbols=["AAPL"])
        fits = {"AAPL": evaluated}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_STRATEGY_FIT_NOT_RECOMMENDED_FOR_PAPER, decision.failure_reasons)

    def test_strategy_fit_wrong_symbol_not_found(self):
        """Strategy-fit keyed to different symbol is not found for AAPL."""
        wl = _make_watchlist(symbols=["AAPL"])
        fits = {"MSFT": _make_passing_fit("MSFT")}  # AAPL not present
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_STRATEGY_FIT_MISSING, decision.failure_reasons)


# ---------------------------------------------------------------------------
# Tests 7-10: Watchlist schema gates
# ---------------------------------------------------------------------------

class TestWatchlistSchema(unittest.TestCase):

    def test_t07_wrong_schema_blocks(self):
        """T07: wrong schema_version blocks promotion."""
        wl = _make_watchlist(schema_version="ranked-candidates-v1")
        fits = {"AAPL": _make_passing_fit()}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_WATCHLIST_SCHEMA_INVALID, decision.failure_reasons)

    def test_t08_non_paper_mode_blocks(self):
        """T08: non-paper mode blocks promotion."""
        wl = _make_watchlist(mode="live")
        fits = {"AAPL": _make_passing_fit()}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_WATCHLIST_MODE_NOT_PAPER, decision.failure_reasons)

    def test_t09_approved_for_live_true_forced_false_with_reason(self):
        """T09: watchlist with approved_for_live=True is rejected; reason added."""
        wl = _make_watchlist()
        wl["approved_for_live"] = True
        fits = {"AAPL": _make_passing_fit()}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_live)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_LIVE_APPROVAL_FORBIDDEN, decision.failure_reasons)
        self.assertIn(REASON_WATCHLIST_LIVE_NOT_LOCKED, decision.failure_reasons)

    def test_t10_no_ranked_candidates_blocks(self):
        """T10: empty symbols list blocks promotion."""
        wl = _make_watchlist(symbols=[])
        fits: dict = {}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_NO_RANKED_CANDIDATES, decision.failure_reasons)
        # strategy_fit_missing must NOT appear — no top symbol to check
        self.assertNotIn(REASON_STRATEGY_FIT_MISSING, decision.failure_reasons)


# ---------------------------------------------------------------------------
# Test 11: Premarket revalidation deferred
# ---------------------------------------------------------------------------

class TestPremarketRevalidation(unittest.TestCase):

    def test_t11_premarket_required_blocks_in_this_patch(self):
        """T11: premarket_revalidation_required=True (default) blocks promotion."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = WatchlistPromotionConfig(
            operator_review_approved=True,
            risk_simulation_passed=True,
            premarket_revalidation_required=True,  # default; future WATCHLIST-PREMARKET-01
        )
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_PREMARKET_REVALIDATION_REQUIRED, decision.failure_reasons)

    def test_premarket_false_clears_gate(self):
        """premarket_revalidation_required=False clears this gate."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertNotIn(REASON_PREMARKET_REVALIDATION_REQUIRED, decision.failure_reasons)

    def test_default_config_premarket_required(self):
        """Default WatchlistPromotionConfig has premarket_revalidation_required=True."""
        cfg = WatchlistPromotionConfig()
        self.assertTrue(cfg.premarket_revalidation_required)


# ---------------------------------------------------------------------------
# Tests 12-13: Hard invariants on applied artifact
# ---------------------------------------------------------------------------

class TestHardInvariants(unittest.TestCase):

    def test_t12_max_symbols_to_trade_forced_1(self):
        """T12: apply_watchlist_promotion forces max_symbols_to_trade=1."""
        wl = _make_watchlist()
        wl["max_symbols_to_trade"] = 5
        fits = {"AAPL": _make_passing_fit()}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        promoted = apply_watchlist_promotion(wl, decision)
        self.assertEqual(promoted["max_symbols_to_trade"], 1)

    def test_t13_max_concurrent_positions_forced_1(self):
        """T13: apply_watchlist_promotion forces max_concurrent_positions=1."""
        wl = _make_watchlist()
        wl["max_concurrent_positions"] = 10
        fits = {"AAPL": _make_passing_fit()}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        promoted = apply_watchlist_promotion(wl, decision)
        self.assertEqual(promoted["max_concurrent_positions"], 1)

    def test_approved_for_live_false_in_config_not_overrideable(self):
        """WatchlistPromotionConfig approved_for_live cannot be set True."""
        cfg = WatchlistPromotionConfig(approved_for_live=True)
        self.assertFalse(cfg.approved_for_live)

    def test_decision_approved_for_live_always_false(self):
        """PromotionDecision.approved_for_live is always False."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_live)

    def test_applied_approved_for_live_always_false_on_blocked(self):
        """apply_watchlist_promotion forces approved_for_live=False on failed decision."""
        wl = _make_watchlist()
        fits: dict = {}
        cfg = WatchlistPromotionConfig()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        promoted = apply_watchlist_promotion(wl, decision)
        self.assertFalse(promoted["approved_for_live"])


# ---------------------------------------------------------------------------
# Test 16: Determinism and deduplication
# ---------------------------------------------------------------------------

class TestDeterminism(unittest.TestCase):

    def test_t16_failure_reasons_deterministic(self):
        """T16: same inputs produce identical failure_reasons list across calls."""
        wl = _make_watchlist(mode="live", symbols=[])
        r1 = evaluate_watchlist_promotion(wl, {})
        r2 = evaluate_watchlist_promotion(wl, {})
        self.assertEqual(r1.failure_reasons, r2.failure_reasons)

    def test_failure_reasons_deduplicated_no_exact_duplicates(self):
        """Failure reasons list contains no exact duplicates."""
        wl = _make_watchlist()
        wl["approved_for_live"] = True
        decision = evaluate_watchlist_promotion(wl, {})
        self.assertEqual(len(decision.failure_reasons), len(set(decision.failure_reasons)))

    def test_live_lock_and_forbidden_both_appear_once(self):
        """REASON_WATCHLIST_LIVE_NOT_LOCKED and REASON_LIVE_APPROVAL_FORBIDDEN each appear once."""
        wl = _make_watchlist()
        wl["approved_for_live"] = True
        decision = evaluate_watchlist_promotion(wl, {})
        self.assertEqual(decision.failure_reasons.count(REASON_WATCHLIST_LIVE_NOT_LOCKED), 1)
        self.assertEqual(decision.failure_reasons.count(REASON_LIVE_APPROVAL_FORBIDDEN), 1)

    def test_full_pass_is_deterministic(self):
        """All-pass path produces same decision across two calls."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = _make_passing_config()
        d1 = evaluate_watchlist_promotion(wl, fits, cfg)
        d2 = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertEqual(d1.approved_for_autonomous_paper, d2.approved_for_autonomous_paper)
        self.assertEqual(d1.failure_reasons, d2.failure_reasons)
        self.assertEqual(d1.approved_symbols, d2.approved_symbols)
        self.assertEqual(d1.strategy_assignments, d2.strategy_assignments)
        self.assertEqual(d1.notes, d2.notes)


# ---------------------------------------------------------------------------
# Test 17: JSON write/read
# ---------------------------------------------------------------------------

class TestJsonWriteRead(unittest.TestCase):

    def test_t17_write_read_promoted_watchlist(self):
        """T17: write_promoted_watchlist writes a valid JSON file that reads back correctly."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        promoted = apply_watchlist_promotion(wl, decision, cfg)

        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "exports", "watchlist", "20260605", "promoted.json")
            out = write_promoted_watchlist(promoted, path)
            self.assertTrue(out.exists())
            loaded = json.loads(out.read_text(encoding="utf-8"))

        self.assertEqual(loaded["schema_version"], SCHEMA_VERSION_WATCHLIST)
        self.assertFalse(loaded["approved_for_live"])
        self.assertTrue(loaded["approved_for_autonomous_paper"])
        self.assertEqual(loaded["max_symbols_to_trade"], 1)
        self.assertEqual(loaded["max_concurrent_positions"], 1)
        self.assertIn("promotion_decision", loaded)
        self.assertTrue(loaded["promotion_decision"]["passed"])

    def test_write_creates_parent_dirs(self):
        """write_promoted_watchlist creates missing parent directories."""
        wl = _make_watchlist()
        fits: dict = {}
        cfg = WatchlistPromotionConfig()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        promoted = apply_watchlist_promotion(wl, decision)

        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "deeply", "nested", "dir", "watchlist.json")
            out = write_promoted_watchlist(promoted, path)
            self.assertTrue(out.exists())

    def test_write_failed_decision_produces_valid_json(self):
        """Failed promotion still produces a valid JSON artifact."""
        wl = _make_watchlist()
        fits: dict = {}
        cfg = WatchlistPromotionConfig()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.passed)
        promoted = apply_watchlist_promotion(wl, decision)

        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "watchlist_blocked.json")
            out = write_promoted_watchlist(promoted, path)
            loaded = json.loads(out.read_text(encoding="utf-8"))

        self.assertFalse(loaded["approved_for_live"])
        self.assertFalse(loaded["approved_for_autonomous_paper"])
        self.assertIn("promotion_decision", loaded)
        self.assertFalse(loaded["promotion_decision"]["passed"])


# ---------------------------------------------------------------------------
# Tests 18-21: Import safety and module source checks
# ---------------------------------------------------------------------------

class TestImportSafety(unittest.TestCase):

    def _get_source(self) -> str:
        import mqk_research.scanner.watchlist_promotion as mod
        return Path(mod.__file__).read_text(encoding="utf-8")

    def test_t18_no_config_watchlists_write(self):
        """T18: module never writes to config/watchlists."""
        src = self._get_source()
        self.assertNotIn("config/watchlist", src)

    def test_t19_no_broker_oms_execution_network_db_imports(self):
        """T19: no broker/OMS/execution/network/DB imports."""
        src = self._get_source()
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
            self.assertNotIn(token, src, f"forbidden token in source: {token!r}")

    def test_t20_no_subprocess_or_mqk_backtest(self):
        """T20: module does not invoke subprocess or mqk-backtest."""
        import mqk_research.scanner.watchlist_promotion as mod
        self.assertFalse(
            hasattr(mod, "subprocess"),
            "subprocess must not be imported at module level",
        )
        src = self._get_source()
        self.assertNotIn("import subprocess", src)
        self.assertNotIn("import mqk_backtest", src)
        self.assertNotIn("from mqk_backtest", src)
        self.assertNotIn("cargo run", src)
        self.assertNotIn("os.system", src)

    def test_t21_no_order_fields(self):
        """T21: module contains no order-submission fields or references."""
        src = self._get_source()
        order_tokens = [
            "submit_order",
            "place_order",
            "order_type",
            "order_side",
            "bracket_order",
            "/v2/orders",
        ]
        for token in order_tokens:
            self.assertNotIn(token, src, f"order token found in source: {token!r}")

    def test_no_db_mutations(self):
        """Module contains no direct DB mutation calls."""
        src = self._get_source()
        db_tokens = [
            "cursor.execute",
            "session.commit",
            "INSERT INTO",
            "UPDATE SET",
        ]
        for token in db_tokens:
            self.assertNotIn(token, src, f"DB mutation token found: {token!r}")


# ---------------------------------------------------------------------------
# Test 20b: No subprocess called during evaluation
# ---------------------------------------------------------------------------

class TestNoExternalExecution(unittest.TestCase):

    def test_evaluate_does_not_call_subprocess(self):
        """evaluate_watchlist_promotion must not call subprocess.run."""
        import subprocess
        original_run = subprocess.run
        called = []

        def spy_run(*args, **kwargs):
            called.append(args)
            return original_run(*args, **kwargs)

        subprocess.run = spy_run
        try:
            wl = _make_watchlist()
            fits = {"AAPL": _make_passing_fit()}
            cfg = _make_passing_config()
            evaluate_watchlist_promotion(wl, fits, cfg)
            evaluate_watchlist_promotion(_make_watchlist(symbols=[]), {})
        finally:
            subprocess.run = original_run

        self.assertEqual(called, [], "subprocess.run must not be called by promotion evaluator")

    def test_apply_does_not_call_subprocess(self):
        """apply_watchlist_promotion must not call subprocess.run."""
        import subprocess
        original_run = subprocess.run
        called = []

        def spy_run(*args, **kwargs):
            called.append(args)
            return original_run(*args, **kwargs)

        subprocess.run = spy_run
        try:
            wl = _make_watchlist()
            fits = {"AAPL": _make_passing_fit()}
            cfg = _make_passing_config()
            decision = evaluate_watchlist_promotion(wl, fits, cfg)
            apply_watchlist_promotion(wl, decision)
        finally:
            subprocess.run = original_run

        self.assertEqual(called, [])


# ---------------------------------------------------------------------------
# Test 22-23: Compatibility with existing pipeline artifacts
# ---------------------------------------------------------------------------

class TestCompatibility(unittest.TestCase):

    def test_t22_selector_watchlist_shell_compatible(self):
        """T22: selector-produced watchlist shell is valid input for promotion."""
        from mqk_research.scanner.selector import (
            build_ranked_candidate_export,
            build_watchlist_artifact,
        )

        def _make_candidate():
            return {
                "schema_version": "scanner-candidate-v1",
                "symbol": "AAPL",
                "total_score": 0.7,
                "liquidity_score": 0.8,
                "regime_score": 0.6,
                "regime_label": "trending_up",
                "risk_score": 0.9,
                "eligible_for_scan": True,
                "eligible_for_backtest": True,
                "eligible_for_live": False,
                "rejection_reason": None,
                "recommended_strategy": "intraday_scalper",
                "timeframe": "5m",
            }

        export = build_ranked_candidate_export([_make_candidate()])
        wl_artifact = build_watchlist_artifact(export)
        wl_dict = wl_artifact.to_dict()

        # Selector shell is fail-closed (no strategy-fit, no operator review)
        cfg = WatchlistPromotionConfig()
        decision = evaluate_watchlist_promotion(wl_dict, {}, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertFalse(decision.approved_for_live)
        # Schema and mode are valid from selector
        self.assertNotIn(REASON_WATCHLIST_SCHEMA_INVALID, decision.failure_reasons)
        self.assertNotIn(REASON_WATCHLIST_MODE_NOT_PAPER, decision.failure_reasons)

    def test_t22b_selector_shell_with_all_gates_promotes(self):
        """T22b: selector shell with all gates supplied can promote."""
        from mqk_research.scanner.selector import (
            build_ranked_candidate_export,
            build_watchlist_artifact,
        )

        def _make_candidate():
            return {
                "schema_version": "scanner-candidate-v1",
                "symbol": "AAPL",
                "total_score": 0.7,
                "liquidity_score": 0.8,
                "regime_score": 0.6,
                "regime_label": "trending_up",
                "risk_score": 0.9,
                "eligible_for_scan": True,
                "eligible_for_backtest": True,
                "eligible_for_live": False,
                "rejection_reason": None,
                "recommended_strategy": "intraday_scalper",
                "timeframe": "5m",
            }

        export = build_ranked_candidate_export([_make_candidate()])
        wl_artifact = build_watchlist_artifact(export)
        wl_dict = wl_artifact.to_dict()

        fits = {"AAPL": _make_passing_fit("AAPL")}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl_dict, fits, cfg)
        self.assertTrue(decision.approved_for_autonomous_paper)
        self.assertFalse(decision.approved_for_live)

    def test_t23_blocked_strategy_fit_from_runner_not_promoted(self):
        """T23: backtest-runner blocked artifacts (all metrics null) do not promote."""
        from mqk_research.scanner.backtest_runner import (
            build_strategy_fit_artifact,
            StrategyFitResult,
            BacktestRunnerConfig,
        )
        from mqk_research.scanner.backtest_gates import apply_backtest_gates

        entry = {
            "queue_id": "bq-promo-test-001",
            "symbol": "AAPL",
            "strategy_id": "intraday_scalper",
            "timeframe": "5m",
            "regime_label": "trending_up",
            "source_rank": 1,
        }
        runner_artifact = build_strategy_fit_artifact(
            entry=entry,
            result=StrategyFitResult(),
            config=BacktestRunnerConfig(),
            generated_at_utc="2026-06-05T18:00:00+00:00",
        )
        evaluated = apply_backtest_gates(runner_artifact)
        self.assertFalse(evaluated["recommended_for_paper"])

        wl = _make_watchlist(symbols=["AAPL"])
        fits = {"AAPL": evaluated}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_STRATEGY_FIT_NOT_RECOMMENDED_FOR_PAPER, decision.failure_reasons)


# ---------------------------------------------------------------------------
# Additional: apply_watchlist_promotion immutability
# ---------------------------------------------------------------------------

class TestApplyImmutability(unittest.TestCase):

    def test_input_not_mutated_by_apply(self):
        """apply_watchlist_promotion does not modify the input watchlist dict."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)

        original_approved = wl["approved_for_autonomous_paper"]
        original_symbols = list(wl["symbols"])
        _ = apply_watchlist_promotion(wl, decision)
        self.assertEqual(wl["approved_for_autonomous_paper"], original_approved)
        self.assertEqual(wl["symbols"], original_symbols)

    def test_input_not_mutated_by_evaluate(self):
        """evaluate_watchlist_promotion does not modify the input watchlist."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = _make_passing_config()
        original_live = wl["approved_for_live"]
        _ = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertEqual(wl["approved_for_live"], original_live)


# ---------------------------------------------------------------------------
# Additional: PromotionInput dataclass exists and bundles inputs
# ---------------------------------------------------------------------------

class TestPromotionInput(unittest.TestCase):

    def test_promotion_input_bundles_correctly(self):
        """PromotionInput correctly holds watchlist and fit artifacts."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        inp = PromotionInput(watchlist=wl, strategy_fit_artifacts=fits)
        self.assertIs(inp.watchlist, wl)
        self.assertIs(inp.strategy_fit_artifacts, fits)


# ---------------------------------------------------------------------------
# Additional: Default config is fail-closed
# ---------------------------------------------------------------------------

class TestDefaultConfigFailClosed(unittest.TestCase):

    def test_default_config_all_gates_fail_closed(self):
        """Default WatchlistPromotionConfig blocks promotion on all three soft gates."""
        cfg = WatchlistPromotionConfig()
        self.assertFalse(cfg.operator_review_approved)
        self.assertFalse(cfg.risk_simulation_passed)
        self.assertTrue(cfg.premarket_revalidation_required)
        self.assertFalse(cfg.approved_for_live)

    def test_default_config_blocks_promotion(self):
        """Default config blocks promotion even with valid schema + fit artifact."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = WatchlistPromotionConfig()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_RISK_SIMULATION_REQUIRED, decision.failure_reasons)
        self.assertIn(REASON_OPERATOR_REVIEW_REQUIRED, decision.failure_reasons)
        self.assertIn(REASON_PREMARKET_REVALIDATION_REQUIRED, decision.failure_reasons)


# ---------------------------------------------------------------------------
# Integration tests: risk_simulation_result and premarket_revalidation_result
# ---------------------------------------------------------------------------

class TestRiskAndPremarketResultIntegration(unittest.TestCase):
    """
    WATCHLIST-PREMKT-RISK-BUNDLE-01 integration tests.
    Verifies that evaluate_watchlist_promotion accepts result dicts from
    risk_simulation and premarket_revalidation modules and applies them correctly.
    """

    def _make_passing_risk_result(self) -> dict:
        return {"schema_version": "risk-simulation-v1", "passed": True, "failure_reasons": []}

    def _make_failing_risk_result(self) -> dict:
        return {
            "schema_version": "risk-simulation-v1",
            "passed": False,
            "failure_reasons": ["risk_cost_adjusted_edge_failed"],
        }

    def _make_passing_premarket_result(self) -> dict:
        return {
            "schema_version": "premarket-revalidation-v1",
            "passed": True,
            "failure_reasons": [],
        }

    def _make_failing_premarket_result(self) -> dict:
        return {
            "schema_version": "premarket-revalidation-v1",
            "passed": False,
            "failure_reasons": ["premarket_bar_stale"],
        }

    def test_int01_risk_result_pass_and_premarket_pass_with_operator_review(self):
        """INT01: passing risk + premarket results + operator review → approved."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = WatchlistPromotionConfig(
            operator_review_approved=True,
            risk_simulation_passed=False,   # overridden by result dict
            premarket_revalidation_required=True,  # overridden by result dict
        )
        decision = evaluate_watchlist_promotion(
            wl, fits, cfg,
            risk_simulation_result=self._make_passing_risk_result(),
            premarket_revalidation_result=self._make_passing_premarket_result(),
        )
        self.assertTrue(decision.approved_for_autonomous_paper)
        self.assertTrue(decision.passed)
        self.assertEqual(decision.failure_reasons, [])
        self.assertFalse(decision.approved_for_live)

    def test_int02_failing_risk_result_blocks_promotion(self):
        """INT02: failing risk result dict blocks promotion even when config.risk_simulation_passed=True."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = WatchlistPromotionConfig(
            operator_review_approved=True,
            risk_simulation_passed=True,    # would pass without result dict
            premarket_revalidation_required=False,
        )
        decision = evaluate_watchlist_promotion(
            wl, fits, cfg,
            risk_simulation_result=self._make_failing_risk_result(),
        )
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_RISK_SIMULATION_REQUIRED, decision.failure_reasons)

    def test_int03_failing_premarket_result_blocks_promotion(self):
        """INT03: failing premarket result dict blocks promotion even when config says not required."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = WatchlistPromotionConfig(
            operator_review_approved=True,
            risk_simulation_passed=True,
            premarket_revalidation_required=False,  # would pass without result dict
        )
        decision = evaluate_watchlist_promotion(
            wl, fits, cfg,
            premarket_revalidation_result=self._make_failing_premarket_result(),
        )
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_PREMARKET_REVALIDATION_REQUIRED, decision.failure_reasons)

    def test_int04_existing_config_boolean_behavior_unchanged(self):
        """INT04: without result dicts, existing config boolean behavior is preserved."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        # Original API: no result dicts, config.risk_simulation_passed=True,
        # premarket_revalidation_required=False → should pass
        cfg = _make_passing_config()
        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertTrue(decision.approved_for_autonomous_paper)
        self.assertFalse(decision.approved_for_live)

    def test_int05_approved_for_live_remains_false_with_passing_results(self):
        """INT05: approved_for_live is always False even when both results pass."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = WatchlistPromotionConfig(operator_review_approved=True)
        decision = evaluate_watchlist_promotion(
            wl, fits, cfg,
            risk_simulation_result=self._make_passing_risk_result(),
            premarket_revalidation_result=self._make_passing_premarket_result(),
        )
        self.assertFalse(decision.approved_for_live)

    def test_int06_risk_result_passed_false_missing_key(self):
        """INT06: risk result dict with missing 'passed' key defaults to False (fail closed)."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_passing_fit()}
        cfg = WatchlistPromotionConfig(
            operator_review_approved=True,
            premarket_revalidation_required=False,
        )
        decision = evaluate_watchlist_promotion(
            wl, fits, cfg,
            risk_simulation_result={},   # no 'passed' key → defaults False → fail
        )
        self.assertFalse(decision.approved_for_autonomous_paper)
        self.assertIn(REASON_RISK_SIMULATION_REQUIRED, decision.failure_reasons)

    def test_int07_real_risk_simulation_module_integration(self):
        """INT07: real risk_simulation module result integrates end-to-end."""
        from mqk_research.scanner.risk_simulation import evaluate_watchlist_risk

        watchlist_with_regime = dict(_make_watchlist())
        watchlist_with_regime["notional_limits"] = {"AAPL": 1000.0}
        watchlist_with_regime["ranked_candidates"] = [
            {
                "rank": 1,
                "symbol": "AAPL",
                "strategy_id": "intraday_scalper",
                "regime_label": "trending_up",
                "regime_score": 0.75,
            }
        ]
        risk_fit = {
            "schema_version": "strategy-fit-v1",
            "symbol": "AAPL",
            "strategy_id": "intraday_scalper",
            "recommended_for_paper": True,
            "recommended_for_live": False,
            "max_drawdown_bps": 150.0,
            "net_expectancy_after_cost_bps": 10.0,
            "failure_reasons": [],
        }
        risk_result = evaluate_watchlist_risk(
            watchlist_with_regime, {"AAPL": risk_fit}
        )
        self.assertTrue(risk_result.passed)

        cfg = WatchlistPromotionConfig(
            operator_review_approved=True,
            premarket_revalidation_required=False,
        )
        promotion_fits = {"AAPL": _make_passing_fit()}
        decision = evaluate_watchlist_promotion(
            watchlist_with_regime, promotion_fits, cfg,
            risk_simulation_result=risk_result.to_dict(),
        )
        self.assertTrue(decision.approved_for_autonomous_paper)
        self.assertFalse(decision.approved_for_live)


# ---------------------------------------------------------------------------
# PV2-01..15: WATCHLIST-PROMO-V2-MULTI-SYMBOL-01 — watchlist-v2 multi-symbol
# promotion tests.
# ---------------------------------------------------------------------------

class TestWatchlistPromoV2MultiSymbol(unittest.TestCase):

    def test_pv2_01_v1_multi_symbol_watchlist_still_approves_only_top_symbol(self):
        """PV2-01: v1 watchlist with 3 ranked symbols still approves only symbols[0]."""
        syms = ["AAPL", "MSFT", "NVDA"]
        wl = _make_watchlist(symbols=syms)  # schema_version defaults to watchlist-v1
        fits = {s: _make_passing_fit(s) for s in syms}
        cfg = _make_passing_config()

        self.assertFalse(_is_watchlist_v2(wl))
        self.assertEqual(_selected_symbols_for_promotion(wl, cfg), ["AAPL"])

        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertTrue(decision.passed)
        self.assertEqual(decision.approved_symbols, ["AAPL"])
        self.assertNotIn("MSFT", decision.approved_symbols)
        self.assertNotIn("NVDA", decision.approved_symbols)

    def test_pv2_02_v2_multi_symbol_all_pass_approves_all_selected(self):
        """PV2-02: v2 watchlist with 3 ranked symbols and matching limits approves all 3."""
        syms = ["AAPL", "MSFT", "NVDA"]
        wl = _make_watchlist(symbols=syms, schema_version=SCHEMA_VERSION_WATCHLIST_V2)
        wl["max_symbols_to_trade"] = 3
        wl["max_concurrent_positions"] = 3
        fits = {s: _make_passing_fit(s) for s in syms}
        cfg = _make_passing_config()

        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertTrue(decision.passed)
        self.assertEqual(decision.failure_reasons, [])
        self.assertEqual(decision.approved_symbols, syms)
        self.assertEqual(
            decision.strategy_assignments,
            {s: "intraday_scalper" for s in syms},
        )
        self.assertFalse(decision.approved_for_live)

    def test_pv2_03_v2_capped_by_max_symbols_to_trade(self):
        """PV2-03: max_symbols_to_trade caps approved symbols below max_concurrent_positions."""
        syms = ["AAPL", "MSFT", "NVDA"]
        wl = _make_watchlist(symbols=syms, schema_version=SCHEMA_VERSION_WATCHLIST_V2)
        wl["max_symbols_to_trade"] = 2
        wl["max_concurrent_positions"] = 3
        fits = {s: _make_passing_fit(s) for s in syms}
        cfg = _make_passing_config()

        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertTrue(decision.passed)
        self.assertEqual(decision.approved_symbols, ["AAPL", "MSFT"])
        self.assertNotIn("NVDA", decision.approved_symbols)

    def test_pv2_04_v2_capped_by_max_concurrent_positions(self):
        """PV2-04: max_concurrent_positions caps approved symbols below max_symbols_to_trade."""
        syms = ["AAPL", "MSFT", "NVDA"]
        wl = _make_watchlist(symbols=syms, schema_version=SCHEMA_VERSION_WATCHLIST_V2)
        wl["max_symbols_to_trade"] = 3
        wl["max_concurrent_positions"] = 1
        fits = {s: _make_passing_fit(s) for s in syms}
        cfg = _make_passing_config()

        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertTrue(decision.passed)
        self.assertEqual(decision.approved_symbols, ["AAPL"])

    def test_pv2_05_v2_hard_ceiling_caps_approval_at_five(self):
        """PV2-05: MULTI_SYMBOL_HARD_CEILING caps approval at 5 even with 6 ranked symbols."""
        syms = ["AAPL", "MSFT", "NVDA", "GOOG", "AMZN", "TSLA"]
        wl = _make_watchlist(symbols=syms, schema_version=SCHEMA_VERSION_WATCHLIST_V2)
        wl["max_symbols_to_trade"] = MULTI_SYMBOL_HARD_CEILING
        wl["max_concurrent_positions"] = MULTI_SYMBOL_HARD_CEILING
        fits = {s: _make_passing_fit(s) for s in syms}
        cfg = _make_passing_config()

        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertTrue(decision.passed)
        self.assertEqual(len(decision.approved_symbols), MULTI_SYMBOL_HARD_CEILING)
        self.assertEqual(decision.approved_symbols, syms[:MULTI_SYMBOL_HARD_CEILING])
        self.assertNotIn("TSLA", decision.approved_symbols)

    def test_pv2_06_v2_max_symbols_to_trade_over_hard_ceiling_fails_closed(self):
        """PV2-06: max_symbols_to_trade > MULTI_SYMBOL_HARD_CEILING fails closed (zero approved)."""
        syms = ["AAPL", "MSFT", "NVDA"]
        wl = _make_watchlist(symbols=syms, schema_version=SCHEMA_VERSION_WATCHLIST_V2)
        wl["max_symbols_to_trade"] = MULTI_SYMBOL_HARD_CEILING + 1
        wl["max_concurrent_positions"] = MULTI_SYMBOL_HARD_CEILING + 1
        fits = {s: _make_passing_fit(s) for s in syms}
        cfg = _make_passing_config()

        self.assertEqual(_selected_symbols_for_promotion(wl, cfg), [])

        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.passed)
        self.assertIn(REASON_WATCHLIST_V2_HARD_CEILING_EXCEEDED, decision.failure_reasons)
        self.assertEqual(decision.approved_symbols, [])

    def test_pv2_07_v2_missing_strategy_assignment_fails_closed(self):
        """PV2-07: a selected v2 symbol with no strategy_assignments entry fails closed."""
        syms = ["AAPL", "MSFT"]
        wl = _make_watchlist(symbols=syms, schema_version=SCHEMA_VERSION_WATCHLIST_V2)
        wl["max_symbols_to_trade"] = 2
        wl["max_concurrent_positions"] = 2
        wl["strategy_assignments"] = {"AAPL": "intraday_scalper"}  # MSFT missing
        fits = {s: _make_passing_fit(s) for s in syms}
        cfg = _make_passing_config()

        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.passed)
        self.assertIn(REASON_WATCHLIST_V2_SYMBOL_ASSIGNMENT_MISSING, decision.failure_reasons)
        self.assertEqual(decision.approved_symbols, [])

    def test_pv2_08_v2_missing_strategy_fit_artifact_fails_closed(self):
        """PV2-08: a selected v2 symbol with no strategy-fit artifact fails closed."""
        syms = ["AAPL", "MSFT"]
        wl = _make_watchlist(symbols=syms, schema_version=SCHEMA_VERSION_WATCHLIST_V2)
        wl["max_symbols_to_trade"] = 2
        wl["max_concurrent_positions"] = 2
        fits = {"AAPL": _make_passing_fit("AAPL")}  # MSFT artifact missing
        cfg = _make_passing_config()

        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.passed)
        self.assertIn(REASON_STRATEGY_FIT_MISSING, decision.failure_reasons)
        self.assertEqual(decision.approved_symbols, [])

    def test_pv2_09_v2_strategy_fit_symbol_mismatch_fails_closed(self):
        """PV2-09: a strategy-fit artifact with a forged/mismatched symbol fails closed."""
        syms = ["AAPL", "MSFT"]
        wl = _make_watchlist(symbols=syms, schema_version=SCHEMA_VERSION_WATCHLIST_V2)
        wl["max_symbols_to_trade"] = 2
        wl["max_concurrent_positions"] = 2
        forged_fit = _make_passing_fit("MSFT")
        forged_fit["symbol"] = "NVDA"  # forged identity — not in the watchlist at all
        fits = {"AAPL": _make_passing_fit("AAPL"), "MSFT": forged_fit}
        cfg = _make_passing_config()

        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.passed)
        self.assertIn(REASON_STRATEGY_FIT_SYMBOL_MISMATCH, decision.failure_reasons)
        self.assertEqual(decision.approved_symbols, [])

    def test_pv2_10_v2_strategy_fit_strategy_mismatch_fails_closed(self):
        """PV2-10: a strategy-fit artifact whose strategy_id differs from the watchlist's
        assignment for that symbol fails closed (v2 requires an exact match)."""
        syms = ["AAPL", "MSFT"]
        wl = _make_watchlist(symbols=syms, schema_version=SCHEMA_VERSION_WATCHLIST_V2)
        wl["max_symbols_to_trade"] = 2
        wl["max_concurrent_positions"] = 2
        mismatched_fit = _make_passing_fit("MSFT", strategy_id="swing_trader")
        fits = {"AAPL": _make_passing_fit("AAPL"), "MSFT": mismatched_fit}
        cfg = _make_passing_config()

        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.passed)
        self.assertIn(REASON_STRATEGY_FIT_STRATEGY_MISMATCH, decision.failure_reasons)
        self.assertEqual(decision.approved_symbols, [])

    def test_pv2_11_v2_one_symbol_not_recommended_fails_closed(self):
        """PV2-11: if any selected v2 symbol's strategy-fit is not recommended_for_paper,
        the whole decision fails closed (no partial approval)."""
        syms = ["AAPL", "MSFT"]
        wl = _make_watchlist(symbols=syms, schema_version=SCHEMA_VERSION_WATCHLIST_V2)
        wl["max_symbols_to_trade"] = 2
        wl["max_concurrent_positions"] = 2
        not_recommended_fit = _make_passing_fit("MSFT")
        not_recommended_fit["recommended_for_paper"] = False
        fits = {"AAPL": _make_passing_fit("AAPL"), "MSFT": not_recommended_fit}
        cfg = _make_passing_config()

        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.passed)
        self.assertIn(REASON_STRATEGY_FIT_NOT_RECOMMENDED_FOR_PAPER, decision.failure_reasons)
        self.assertEqual(decision.approved_symbols, [])

    def test_pv2_12_v2_approved_symbols_preserve_ranked_order(self):
        """PV2-12: approved_symbols preserves the watchlist's ranked (non-alphabetical) order."""
        syms = ["NVDA", "AAPL", "MSFT"]
        wl = _make_watchlist(symbols=syms, schema_version=SCHEMA_VERSION_WATCHLIST_V2)
        wl["max_symbols_to_trade"] = 3
        wl["max_concurrent_positions"] = 3
        fits = {s: _make_passing_fit(s) for s in syms}
        cfg = _make_passing_config()

        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertTrue(decision.passed)
        self.assertEqual(decision.approved_symbols, syms)

    def test_pv2_13_v2_approved_for_live_always_false_with_multiple_approved(self):
        """PV2-13: approved_for_live is always False, even with 3 approved v2 symbols."""
        syms = ["AAPL", "MSFT", "NVDA"]
        wl = _make_watchlist(symbols=syms, schema_version=SCHEMA_VERSION_WATCHLIST_V2)
        wl["max_symbols_to_trade"] = 3
        wl["max_concurrent_positions"] = 3
        fits = {s: _make_passing_fit(s) for s in syms}
        cfg = _make_passing_config()

        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertTrue(decision.passed)
        self.assertEqual(len(decision.approved_symbols), 3)
        self.assertFalse(decision.approved_for_live)

        promoted = apply_watchlist_promotion(wl, decision, cfg)
        self.assertFalse(promoted["approved_for_live"])
        self.assertEqual(promoted["symbols"], syms)
        self.assertEqual(promoted["max_symbols_to_trade"], 3)
        self.assertEqual(promoted["max_concurrent_positions"], 3)

    def test_pv2_14_v2_forged_approved_for_live_true_rejected(self):
        """PV2-14: a v2 watchlist forged with approved_for_live=True is rejected and
        approves zero symbols, even though per-symbol gates would otherwise pass."""
        syms = ["AAPL", "MSFT"]
        wl = _make_watchlist(symbols=syms, schema_version=SCHEMA_VERSION_WATCHLIST_V2)
        wl["max_symbols_to_trade"] = 2
        wl["max_concurrent_positions"] = 2
        wl["approved_for_live"] = True  # forged
        fits = {s: _make_passing_fit(s) for s in syms}
        cfg = _make_passing_config()

        decision = evaluate_watchlist_promotion(wl, fits, cfg)
        self.assertFalse(decision.passed)
        self.assertFalse(decision.approved_for_live)
        self.assertIn(REASON_LIVE_APPROVAL_FORBIDDEN, decision.failure_reasons)
        self.assertIn(REASON_WATCHLIST_LIVE_NOT_LOCKED, decision.failure_reasons)
        self.assertEqual(decision.approved_symbols, [])

        promoted = apply_watchlist_promotion(wl, decision, cfg)
        self.assertFalse(promoted["approved_for_live"])

    def test_pv2_15_v2_premarket_result_honored_per_symbol(self):
        """PV2-15: premarket_revalidation_result["symbol_results"] is honored per
        selected symbol for v2, overriding the overall "passed" flag in both
        directions (one failing symbol blocks; all-pass symbols clear the gate)."""
        syms = ["AAPL", "MSFT"]
        wl = _make_watchlist(symbols=syms, schema_version=SCHEMA_VERSION_WATCHLIST_V2)
        wl["max_symbols_to_trade"] = 2
        wl["max_concurrent_positions"] = 2
        fits = {s: _make_passing_fit(s) for s in syms}
        cfg = WatchlistPromotionConfig(
            operator_review_approved=True,
            risk_simulation_passed=True,
        )

        # Overall passed=True, but MSFT's per-symbol entry failed -> gate fires.
        premarket_one_fails = {
            "schema_version": "premarket-revalidation-v1",
            "passed": True,
            "failure_reasons": [],
            "symbol_results": {
                "AAPL": {"passed": True, "failure_reasons": []},
                "MSFT": {"passed": False, "failure_reasons": ["premarket_bar_stale"]},
            },
        }
        decision = evaluate_watchlist_promotion(
            wl, fits, cfg, premarket_revalidation_result=premarket_one_fails
        )
        self.assertFalse(decision.passed)
        self.assertIn(REASON_PREMARKET_REVALIDATION_REQUIRED, decision.failure_reasons)

        # Overall passed=False, but both selected symbols pass per-symbol -> gate clears.
        premarket_both_pass = {
            "schema_version": "premarket-revalidation-v1",
            "passed": False,
            "failure_reasons": ["premarket_bar_stale"],
            "symbol_results": {
                "AAPL": {"passed": True, "failure_reasons": []},
                "MSFT": {"passed": True, "failure_reasons": []},
            },
        }
        decision2 = evaluate_watchlist_promotion(
            wl, fits, cfg, premarket_revalidation_result=premarket_both_pass
        )
        self.assertNotIn(REASON_PREMARKET_REVALIDATION_REQUIRED, decision2.failure_reasons)
        self.assertTrue(decision2.passed)
        self.assertEqual(decision2.approved_symbols, syms)


if __name__ == "__main__":
    unittest.main()
