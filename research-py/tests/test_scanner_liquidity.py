"""
SCANNER-LIQUIDITY-01 — Liquidity gate tests.

Run:
    $env:PYTHONPATH="research-py;research-py\\src"
    python -m unittest research-py/tests/test_scanner_liquidity.py
"""
from __future__ import annotations

import importlib
import json
import tempfile
import unittest
from pathlib import Path


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _strong_metrics(**overrides):
    """Return a LiquidityMetrics that passes all checks by default."""
    from mqk_research.scanner.liquidity import LiquidityMetrics
    defaults = dict(
        latest_close=50.0,
        avg_daily_volume_shares=1_000_000.0,
        avg_daily_dollar_volume=50_000_000.0,
        relative_volume=2.0,
        spread_estimate_bps=5.0,
        slippage_estimate_bps=5.0,
        round_trip_cost_bps=10.0,
    )
    defaults.update(overrides)
    return LiquidityMetrics(**defaults)


def _eval(metrics=None, config=None, **metric_overrides):
    from mqk_research.scanner.liquidity import evaluate_liquidity, LiquidityConfig
    if metrics is None:
        metrics = _strong_metrics(**metric_overrides)
    cfg = config or LiquidityConfig()
    return evaluate_liquidity("TEST", metrics, cfg)


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

class TestLiquidityGatePass(unittest.TestCase):
    def test_strong_metrics_pass(self):
        result = _eval()
        self.assertTrue(result.passed)
        self.assertIsNone(result.rejection_reason)
        self.assertGreater(result.score, 0.0)
        self.assertLessEqual(result.score, 1.0)

    def test_pass_score_above_zero(self):
        result = _eval()
        self.assertGreater(result.score, 0.0)

    def test_pass_reason_tags_empty(self):
        result = _eval()
        self.assertEqual(result.reason_tags, [])
        self.assertEqual(result.risk_tags, [])


class TestLiquidityGatePriceFloor(unittest.TestCase):
    def test_price_below_min(self):
        result = _eval(latest_close=4.99)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, "price_below_min")
        self.assertIn("liquidity", result.reason_tags)
        self.assertEqual(result.score, 0.0)

    def test_price_exactly_at_min_passes(self):
        result = _eval(latest_close=5.0)
        self.assertTrue(result.passed)

    def test_price_above_min_passes(self):
        result = _eval(latest_close=5.01)
        self.assertTrue(result.passed)


class TestLiquidityGateADV(unittest.TestCase):
    def test_adv_below_min(self):
        result = _eval(avg_daily_dollar_volume=9_999_999.0)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, "adv_usd_below_min")

    def test_adv_at_exactly_min_passes(self):
        result = _eval(avg_daily_dollar_volume=10_000_000.0)
        self.assertTrue(result.passed)


class TestLiquidityGateRelativeVolume(unittest.TestCase):
    def test_rvol_below_min(self):
        result = _eval(relative_volume=0.99)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, "relative_volume_below_min")

    def test_rvol_exactly_at_min_passes(self):
        result = _eval(relative_volume=1.0)
        self.assertTrue(result.passed)


class TestLiquidityGateSpread(unittest.TestCase):
    def test_spread_too_wide(self):
        result = _eval(spread_estimate_bps=20.01)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, "spread_too_wide")
        self.assertIn("execution_cost_risk", result.risk_tags)

    def test_spread_exactly_at_max_passes(self):
        result = _eval(spread_estimate_bps=20.0)
        self.assertTrue(result.passed)


class TestLiquidityGateRoundTripCost(unittest.TestCase):
    def test_round_trip_cost_too_high(self):
        result = _eval(round_trip_cost_bps=40.01)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, "round_trip_cost_too_high")

    def test_round_trip_cost_at_max_passes(self):
        result = _eval(round_trip_cost_bps=40.0)
        self.assertTrue(result.passed)


class TestLiquidityGateSlippage(unittest.TestCase):
    def test_slippage_too_high(self):
        result = _eval(slippage_estimate_bps=15.01)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, "slippage_too_high")

    def test_slippage_exactly_at_max_passes(self):
        result = _eval(slippage_estimate_bps=15.0)
        self.assertTrue(result.passed)


class TestLiquidityGateCompositeScore(unittest.TestCase):
    def test_score_below_min_liquidity_score(self):
        """
        Strong metrics pass all hard checks but score < 0.99.
        Score components normalize against threshold (0 at threshold, 1 at 2x).
        Strong metrics produce a composite score well under 1.0, so raising
        min_liquidity_score to 0.99 triggers the composite gate.
        """
        from mqk_research.scanner.liquidity import LiquidityConfig, LiquidityMetrics, evaluate_liquidity
        cfg = LiquidityConfig(
            min_price=5.0,
            min_adv_usd=10_000_000.0,
            min_rvol=1.0,
            max_spread_bps=20.0,
            max_round_trip_cost_bps=40.0,
            max_slippage_bps=15.0,
            min_liquidity_score=0.99,
        )
        # Strong metrics: all pass hard checks; composite score ~0.86 < 0.99
        metrics = _strong_metrics()
        result = evaluate_liquidity("TEST", metrics, cfg)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, "liquidity_score_below_min")
        self.assertGreater(result.score, 0.0)

    def test_score_bounds_on_pass(self):
        result = _eval()
        self.assertGreaterEqual(result.score, 0.0)
        self.assertLessEqual(result.score, 1.0)

    def test_score_zero_on_hard_failure(self):
        result = _eval(latest_close=1.0)  # price below min
        self.assertEqual(result.score, 0.0)

    def test_score_nonzero_on_composite_failure(self):
        from mqk_research.scanner.liquidity import LiquidityConfig, evaluate_liquidity
        # min_liquidity_score=0.99 forces composite rejection; score is nonzero
        cfg = LiquidityConfig(min_liquidity_score=0.99)
        result = evaluate_liquidity("TEST", _strong_metrics(), cfg)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, "liquidity_score_below_min")
        self.assertGreater(result.score, 0.0)


class TestLiquidityRejectionCandidate(unittest.TestCase):
    def test_rejection_candidate_eligible_for_live_false(self):
        from mqk_research.scanner.liquidity import (
            LiquidityResult, build_liquidity_rejection_candidate,
        )
        liq_result = LiquidityResult(
            passed=False,
            score=0.0,
            rejection_reason="price_below_min",
            reason_tags=["liquidity"],
            risk_tags=["liquidity_risk"],
            latest_close=2.0,
            avg_daily_dollar_volume=0.0,
            relative_volume=0.0,
            spread_estimate_bps=0.0,
            slippage_estimate_bps=0.0,
            round_trip_cost_bps=0.0,
        )
        record = build_liquidity_rejection_candidate(
            scanner_id="main_scanner",
            symbol="TINY",
            exchange="NASDAQ",
            timeframe="5Min",
            source="test",
            latest_bar_ts="2026-06-04T14:00:00Z",
            liq_result=liq_result,
        )
        self.assertFalse(record["eligible_for_live"])
        self.assertFalse(record["eligible_for_scan"])
        self.assertFalse(record["eligible_for_backtest"])
        self.assertFalse(record["eligible_for_paper"])
        self.assertEqual(record["schema_version"], "scanner-candidate-v1")
        self.assertEqual(record["rejection_reason"], "price_below_min")
        self.assertIn("liquidity", record["reason_tags"])

    def test_rejection_candidate_writes_jsonl(self):
        from mqk_research.scanner.liquidity import (
            LiquidityResult, build_liquidity_rejection_candidate,
        )
        from mqk_research.scanner.candidates import ScannerCandidateWriter

        liq_result = LiquidityResult(
            passed=False,
            score=0.0,
            rejection_reason="adv_usd_below_min",
            reason_tags=["liquidity"],
            risk_tags=["liquidity_risk"],
            latest_close=50.0,
            avg_daily_dollar_volume=1_000_000.0,
            relative_volume=0.5,
            spread_estimate_bps=5.0,
            slippage_estimate_bps=5.0,
            round_trip_cost_bps=10.0,
        )
        record = build_liquidity_rejection_candidate(
            scanner_id="main_scanner",
            symbol="LOWVOL",
            exchange="NYSE",
            timeframe="5Min",
            source="test",
            latest_bar_ts="2026-06-04T14:00:00Z",
            liq_result=liq_result,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            writer = ScannerCandidateWriter(
                scanner_id="main_scanner",
                base_dir=tmpdir,
                date_str="20260604",
            )
            path = writer.write(record)
            writer.close()

            self.assertTrue(path.exists())
            lines = path.read_text(encoding="utf-8").strip().splitlines()
            self.assertEqual(len(lines), 1)
            loaded = json.loads(lines[0])
            self.assertFalse(loaded["eligible_for_live"])
            self.assertEqual(loaded["rejection_reason"], "adv_usd_below_min")

    def test_passing_result_raises_valueerror(self):
        from mqk_research.scanner.liquidity import (
            LiquidityResult, build_liquidity_rejection_candidate,
        )
        liq_result = LiquidityResult(
            passed=True,
            score=0.9,
            rejection_reason=None,
            reason_tags=[],
            risk_tags=[],
            latest_close=50.0,
            avg_daily_dollar_volume=50_000_000.0,
            relative_volume=2.0,
            spread_estimate_bps=5.0,
            slippage_estimate_bps=5.0,
            round_trip_cost_bps=10.0,
        )
        with self.assertRaises(ValueError):
            build_liquidity_rejection_candidate(
                scanner_id="main_scanner",
                symbol="GOOD",
                exchange="NASDAQ",
                timeframe="5Min",
                source="test",
                latest_bar_ts="2026-06-04T14:00:00Z",
                liq_result=liq_result,
            )


class TestLiquidityNoBrokerImports(unittest.TestCase):
    def test_no_forbidden_imports_in_liquidity_module(self):
        """liquidity.py must not import broker/OMS/network/DB modules."""
        import_forbidden = [
            "import requests",
            "import urllib",
            "import http.client",
            "import aiohttp",
            "import psycopg",
            "import sqlalchemy",
        ]
        ref_forbidden = [
            "BrokerGateway",
            "broker_adapter",
            "oms_outbox",
            "oms_inbox",
            "submit_order",
            "/v2/orders",
        ]
        module_path = Path(__file__).parent.parent / "src" / "mqk_research" / "scanner" / "liquidity.py"
        content = module_path.read_text(encoding="utf-8")
        for term in import_forbidden:
            with self.subTest(term=term):
                self.assertNotIn(term, content, f"liquidity.py must not contain '{term}'")
        for term in ref_forbidden:
            with self.subTest(term=term):
                self.assertNotIn(term, content, f"liquidity.py must not reference '{term}'")


if __name__ == "__main__":
    unittest.main()
