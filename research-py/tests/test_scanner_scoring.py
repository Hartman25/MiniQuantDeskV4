"""
SCANNER-SCORE-01 — MAIN scanner candidate scoring tests.

Run:
    $env:PYTHONPATH="research-py;research-py\\src"
    python -m unittest research-py/tests/test_scanner_scoring.py

All tests are pure in-process. No DB, no broker, no network, no file I/O
except tests that explicitly use tempfile for writer integration.
"""
from __future__ import annotations

import ast
import importlib
import pathlib
import tempfile
import unittest

from mqk_research.scanner.scoring import (
    ScoringConfig,
    ScoringInputs,
    ScoringResult,
    build_scored_scanner_candidate,
    score_candidate,
)
from mqk_research.scanner.candidates import ScannerCandidateWriter, load_candidates


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _good_inputs(**overrides) -> ScoringInputs:
    base = dict(
        symbol="AAPL",
        exchange="NASDAQ",
        timeframe="5Min",
        source="alpaca",
        latest_bar_ts="2026-06-05T14:00:00+00:00",
        volume=500_000,
        avg_volume=400_000.0,
        relative_volume=1.25,
        data_quality_score=0.95,
        liquidity_score=0.80,
        trend_score=0.70,
        momentum_score=0.65,
        mean_reversion_score=0.30,
        volatility_score=0.55,
        regime_label="trending_up",
        regime_score=0.72,
        latest_close=175.50,
        lookback_close=170.00,
        move_bps=323.5,
        abs_move_bps=323.5,
        atr=1.75,
        gap_pct=0.3,
        spread_estimate_bps=5.0,
        slippage_estimate_bps=3.0,
        round_trip_cost_bps=10.0,
    )
    base.update(overrides)
    return ScoringInputs(**base)


def _build_accepted(**overrides) -> dict:
    return build_scored_scanner_candidate(
        scanner_id="main_scanner",
        inputs=_good_inputs(**overrides),
        dq_passed=True,
        liquidity_passed=True,
        regime_passed=True,
    )


# ---------------------------------------------------------------------------
# T01: Strong candidate produces accepted scanner-candidate-v1
# ---------------------------------------------------------------------------

class TestStrongCandidateAccepted(unittest.TestCase):
    def test_accepted_record_has_correct_schema(self):
        record = _build_accepted()
        self.assertEqual(record["schema_version"], "scanner-candidate-v1")
        self.assertEqual(record["eligible_for_scan"], True)


# ---------------------------------------------------------------------------
# T02: total_score bounded [0.0, 1.0]
# ---------------------------------------------------------------------------

class TestTotalScoreBounds(unittest.TestCase):
    def test_total_score_within_bounds(self):
        result = score_candidate(_good_inputs())
        self.assertGreaterEqual(result.total_score, 0.0)
        self.assertLessEqual(result.total_score, 1.0)

    def test_total_score_worst_inputs(self):
        inputs = _good_inputs(
            liquidity_score=0.0,
            volatility_score=0.0,
            regime_score=0.0,
            momentum_score=0.0,
            trend_score=0.0,
            round_trip_cost_bps=40.0,
            atr=100.0,
            abs_move_bps=500.0,
            gap_pct=5.0,
            spread_estimate_bps=20.0,
        )
        result = score_candidate(inputs)
        self.assertGreaterEqual(result.total_score, 0.0)
        self.assertLessEqual(result.total_score, 1.0)


# ---------------------------------------------------------------------------
# T03: risk_score bounded [0.0, 1.0]
# ---------------------------------------------------------------------------

class TestRiskScoreBounds(unittest.TestCase):
    def test_risk_score_within_bounds_good(self):
        result = score_candidate(_good_inputs())
        self.assertGreaterEqual(result.risk_score, 0.0)
        self.assertLessEqual(result.risk_score, 1.0)

    def test_risk_score_within_bounds_bad(self):
        inputs = _good_inputs(
            atr=10.0,
            gap_pct=10.0,
            round_trip_cost_bps=50.0,
            spread_estimate_bps=30.0,
            abs_move_bps=600.0,
        )
        result = score_candidate(inputs)
        self.assertGreaterEqual(result.risk_score, 0.0)
        self.assertLessEqual(result.risk_score, 1.0)


# ---------------------------------------------------------------------------
# T04: eligible_for_live is always False
# ---------------------------------------------------------------------------

class TestEligibleForLiveAlwaysFalse(unittest.TestCase):
    def test_eligible_for_live_false(self):
        record = _build_accepted()
        self.assertIs(record["eligible_for_live"], False)


# ---------------------------------------------------------------------------
# T05: eligible_for_scan is True
# ---------------------------------------------------------------------------

class TestEligibleForScan(unittest.TestCase):
    def test_eligible_for_scan_true(self):
        record = _build_accepted()
        self.assertIs(record["eligible_for_scan"], True)


# ---------------------------------------------------------------------------
# T06: eligible_for_backtest is True
# ---------------------------------------------------------------------------

class TestEligibleForBacktest(unittest.TestCase):
    def test_eligible_for_backtest_true(self):
        record = _build_accepted()
        self.assertIs(record["eligible_for_backtest"], True)


# ---------------------------------------------------------------------------
# T07: eligible_for_paper is False
# ---------------------------------------------------------------------------

class TestEligibleForPaperFalse(unittest.TestCase):
    def test_eligible_for_paper_false(self):
        record = _build_accepted()
        self.assertIs(record["eligible_for_paper"], False)


# ---------------------------------------------------------------------------
# T08: rejection_reason is None for accepted candidate
# ---------------------------------------------------------------------------

class TestRejectionReasonNone(unittest.TestCase):
    def test_rejection_reason_none(self):
        record = _build_accepted()
        self.assertIsNone(record["rejection_reason"])


# ---------------------------------------------------------------------------
# T09: default recommended_strategy
# ---------------------------------------------------------------------------

class TestDefaultRecommendedStrategy(unittest.TestCase):
    def test_default_strategy(self):
        record = _build_accepted()
        self.assertEqual(record["recommended_strategy"], "intraday_scalper")

    def test_custom_config_strategy(self):
        cfg = ScoringConfig(default_recommended_strategy="swing_trend")
        result = score_candidate(_good_inputs(), cfg)
        self.assertEqual(result.recommended_strategy, "swing_trend")


# ---------------------------------------------------------------------------
# T10: High cost reduces score vs low cost
# ---------------------------------------------------------------------------

class TestHighCostReducesScore(unittest.TestCase):
    def test_high_cost_lower_than_low_cost(self):
        low = score_candidate(_good_inputs(round_trip_cost_bps=5.0))
        high = score_candidate(_good_inputs(round_trip_cost_bps=35.0))
        self.assertGreater(low.total_score, high.total_score)


# ---------------------------------------------------------------------------
# T11: Higher liquidity improves score
# ---------------------------------------------------------------------------

class TestLiquidityImprovesScore(unittest.TestCase):
    def test_higher_liquidity_higher_score(self):
        low = score_candidate(_good_inputs(liquidity_score=0.3))
        high = score_candidate(_good_inputs(liquidity_score=0.9))
        self.assertGreater(high.total_score, low.total_score)


# ---------------------------------------------------------------------------
# T12: Higher risk lowers score
# ---------------------------------------------------------------------------

class TestHigherRiskLowersScore(unittest.TestCase):
    def test_high_risk_lower_score(self):
        low_risk = score_candidate(_good_inputs(atr=0.5, gap_pct=0.1, abs_move_bps=50.0))
        high_risk = score_candidate(_good_inputs(atr=8.0, gap_pct=5.0, abs_move_bps=500.0))
        self.assertGreater(low_risk.total_score, high_risk.total_score)


# ---------------------------------------------------------------------------
# T13: Reason tags include required entries
# ---------------------------------------------------------------------------

class TestReasonTagsRequired(unittest.TestCase):
    def test_required_reason_tags_present(self):
        result = score_candidate(_good_inputs(regime_label="trending_up"))
        self.assertIn("scanner_scored", result.reason_tags)
        self.assertIn("dq_pass", result.reason_tags)
        self.assertIn("liquidity_pass", result.reason_tags)
        self.assertIn("regime_trending_up", result.reason_tags)


# ---------------------------------------------------------------------------
# T14: Risk tags for high gap / high cost / high spread
# ---------------------------------------------------------------------------

class TestRiskTagsForHighRisk(unittest.TestCase):
    def test_risk_large_gap_tag(self):
        result = score_candidate(_good_inputs(gap_pct=5.0))
        self.assertIn("risk_large_gap", result.risk_tags)

    def test_risk_high_cost_tag(self):
        result = score_candidate(_good_inputs(round_trip_cost_bps=35.0))
        self.assertIn("risk_high_cost", result.risk_tags)

    def test_risk_high_spread_tag(self):
        result = score_candidate(_good_inputs(spread_estimate_bps=20.0))
        self.assertIn("risk_high_spread", result.risk_tags)

    def test_risk_low_tag_for_clean_candidate(self):
        result = score_candidate(_good_inputs(
            atr=0.5, gap_pct=0.1, round_trip_cost_bps=5.0,
            spread_estimate_bps=2.0, abs_move_bps=30.0,
        ))
        self.assertIn("risk_low", result.risk_tags)


# ---------------------------------------------------------------------------
# T15: Passing candidate writes JSONL through ScannerCandidateWriter
# ---------------------------------------------------------------------------

class TestWriterIntegration(unittest.TestCase):
    def test_writer_produces_jsonl(self):
        record = _build_accepted()
        with tempfile.TemporaryDirectory() as tmpdir:
            with ScannerCandidateWriter("main_scanner", base_dir=tmpdir, date_str="20260605") as writer:
                path = writer.write_candidate(record)
            rows = load_candidates(str(path))
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["symbol"], "AAPL")
        self.assertEqual(rows[0]["eligible_for_live"], False)
        self.assertEqual(rows[0]["eligible_for_scan"], True)


# ---------------------------------------------------------------------------
# T16: Upstream DQ failure raises ValueError
# ---------------------------------------------------------------------------

class TestDQFailureRaisesError(unittest.TestCase):
    def test_dq_failed_raises(self):
        with self.assertRaises(ValueError) as ctx:
            build_scored_scanner_candidate(
                scanner_id="main_scanner",
                inputs=_good_inputs(),
                dq_passed=False,
                liquidity_passed=True,
                regime_passed=True,
            )
        self.assertIn("DQ gate did not pass", str(ctx.exception))


# ---------------------------------------------------------------------------
# T17: Upstream liquidity failure raises ValueError
# ---------------------------------------------------------------------------

class TestLiquidityFailureRaisesError(unittest.TestCase):
    def test_liquidity_failed_raises(self):
        with self.assertRaises(ValueError) as ctx:
            build_scored_scanner_candidate(
                scanner_id="main_scanner",
                inputs=_good_inputs(),
                dq_passed=True,
                liquidity_passed=False,
                regime_passed=True,
            )
        self.assertIn("liquidity gate did not pass", str(ctx.exception))


# ---------------------------------------------------------------------------
# T18: Upstream regime failure raises ValueError
# ---------------------------------------------------------------------------

class TestRegimeFailureRaisesError(unittest.TestCase):
    def test_regime_failed_raises(self):
        with self.assertRaises(ValueError) as ctx:
            build_scored_scanner_candidate(
                scanner_id="main_scanner",
                inputs=_good_inputs(),
                dq_passed=True,
                liquidity_passed=True,
                regime_passed=False,
            )
        self.assertIn("regime gate did not pass", str(ctx.exception))


# ---------------------------------------------------------------------------
# T19: No broker/OMS/execution/network/DB imports in scoring.py
# ---------------------------------------------------------------------------

class TestNoForbiddenImports(unittest.TestCase):
    def test_no_forbidden_imports_in_source(self):
        scoring_path = pathlib.Path(__file__).parent.parent / "src" / "mqk_research" / "scanner" / "scoring.py"
        source = scoring_path.read_text(encoding="utf-8")
        forbidden = [
            "import requests",
            "import urllib",
            "import http.client",
            "import aiohttp",
            "import psycopg",
            "import sqlalchemy",
            "BrokerGateway",
            "oms_outbox",
            "oms_inbox",
            "submit_order",
        ]
        for token in forbidden:
            self.assertNotIn(token, source, f"Forbidden token found in scoring.py: {token!r}")


# ---------------------------------------------------------------------------
# T20: Weight validation — invalid weights raise ValueError
# ---------------------------------------------------------------------------

class TestWeightValidation(unittest.TestCase):
    def test_negative_weight_raises(self):
        cfg = ScoringConfig(w_liquidity=-0.1)
        with self.assertRaises(ValueError):
            cfg.validate()

    def test_valid_zero_weight_ok(self):
        cfg = ScoringConfig(w_momentum=0.0)
        cfg.validate()  # should not raise


# ---------------------------------------------------------------------------
# T21: Score deterministic for same inputs
# ---------------------------------------------------------------------------

class TestScoreDeterminism(unittest.TestCase):
    def test_same_inputs_same_output(self):
        inputs = _good_inputs()
        r1 = score_candidate(inputs)
        r2 = score_candidate(inputs)
        self.assertEqual(r1.total_score, r2.total_score)
        self.assertEqual(r1.risk_score, r2.risk_score)
        self.assertEqual(r1.reason_tags, r2.reason_tags)
        self.assertEqual(r1.risk_tags, r2.risk_tags)


# ---------------------------------------------------------------------------
# T22: Accepted candidate does not contain paper_order_id or live_order_id
# ---------------------------------------------------------------------------

class TestNoOrderIds(unittest.TestCase):
    def test_no_order_ids_in_record(self):
        record = _build_accepted()
        self.assertNotIn("paper_order_id", record)
        self.assertNotIn("live_order_id", record)


if __name__ == "__main__":
    unittest.main()
