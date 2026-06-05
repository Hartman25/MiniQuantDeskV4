"""
SCANNER-REGIME-01 — MAIN scanner regime classifier tests.

Run:
    $env:PYTHONPATH="research-py;research-py\\src"
    python -m unittest research-py/tests/test_scanner_regime.py

All tests are pure in-process. No DB, no broker, no network, no file side effects
except for tests that explicitly use tempfile.
"""
from __future__ import annotations

import json
import pathlib
import tempfile
import unittest
from typing import Any

from mqk_research.scanner.regime import (
    LABEL_GAPPING,
    LABEL_HIGH_VOLATILITY,
    LABEL_LOW_VOLATILITY,
    LABEL_RANGE_BOUND,
    LABEL_TRENDING_DOWN,
    LABEL_TRENDING_UP,
    LABEL_UNCLASSIFIED,
    REASON_REGIME_SCORE_BELOW_MIN,
    REASON_REGIME_UNCLASSIFIED,
    RegimeConfig,
    apply_regime_to_candidate_fields,
    build_regime_rejection_candidate,
    evaluate_regime,
)


# ---------------------------------------------------------------------------
# Bar factory helpers
# ---------------------------------------------------------------------------

def _make_bars(
    n: int,
    *,
    start: float = 100.0,
    end: float | None = None,
    spread: float = 0.8,
    volume: int = 1_000_000,
) -> list[dict[str, Any]]:
    """
    Create n bars with close linearly interpolated from start to end.
    open is set equal to close. high = close + spread/2. low = close - spread/2.
    """
    if end is None:
        end = start
    bars: list[dict[str, Any]] = []
    for i in range(n):
        frac = i / max(n - 1, 1)
        c = start + (end - start) * frac
        bars.append(
            {
                "ts": f"2026-06-03T{9 + i // 12:02d}:{(i % 12) * 5:02d}:00+00:00",
                "open": c,
                "high": c + spread / 2.0,
                "low": c - spread / 2.0,
                "close": c,
                "volume": volume,
            }
        )
    return bars


def _with_gap(bars: list[dict[str, Any]], gap_pct: float) -> list[dict[str, Any]]:
    """Return a copy of bars with the last bar's open shifted by gap_pct%."""
    import copy
    result = copy.deepcopy(bars)
    if len(result) >= 2:
        prior_close = result[-2]["close"]
        result[-1]["open"] = prior_close * (1.0 + gap_pct / 100.0)
    return result


# ---------------------------------------------------------------------------
# R01: trending_up
# ---------------------------------------------------------------------------

class TestTrendingUp(unittest.TestCase):
    def test_trending_up_label(self) -> None:
        # close goes from 99 to 100 over 30 bars (10 flat at 99, then 20 rising)
        flat = _make_bars(10, start=99.0, end=99.0, spread=0.8)
        trend = _make_bars(20, start=99.0, end=100.0, spread=0.8)
        bars = flat + trend
        result = evaluate_regime("TEST", bars)
        self.assertEqual(result.regime_label, LABEL_TRENDING_UP)
        self.assertTrue(result.passed)
        self.assertIsNone(result.rejection_reason)

    def test_trending_up_regime_score_gte_half(self) -> None:
        flat = _make_bars(10, start=99.0, end=99.0, spread=0.8)
        trend = _make_bars(20, start=99.0, end=100.0, spread=0.8)
        result = evaluate_regime("TEST", flat + trend)
        self.assertGreaterEqual(result.regime_score, 0.5)

    def test_trending_up_trend_score_positive(self) -> None:
        flat = _make_bars(10, start=99.0, end=99.0, spread=0.8)
        trend = _make_bars(20, start=99.0, end=100.0, spread=0.8)
        result = evaluate_regime("TEST", flat + trend)
        self.assertGreater(result.trend_score, 0.0)


# ---------------------------------------------------------------------------
# R02: trending_down
# ---------------------------------------------------------------------------

class TestTrendingDown(unittest.TestCase):
    def test_trending_down_label(self) -> None:
        flat = _make_bars(10, start=101.0, end=101.0, spread=0.8)
        trend = _make_bars(20, start=101.0, end=100.0, spread=0.8)
        bars = flat + trend
        result = evaluate_regime("TEST", bars)
        self.assertEqual(result.regime_label, LABEL_TRENDING_DOWN)
        self.assertTrue(result.passed)
        self.assertIsNone(result.rejection_reason)

    def test_trending_down_move_bps_negative(self) -> None:
        flat = _make_bars(10, start=101.0, end=101.0, spread=0.8)
        trend = _make_bars(20, start=101.0, end=100.0, spread=0.8)
        result = evaluate_regime("TEST", flat + trend)
        self.assertLess(result.move_bps, 0.0)


# ---------------------------------------------------------------------------
# R03: range_bound
# ---------------------------------------------------------------------------

class TestRangeBound(unittest.TestCase):
    def test_range_bound_label(self) -> None:
        # spread=1.0 → ATR ~1.0, atr_pct ~1% (strictly between 0.5% and 2.0%);
        # close flat → trend_move_bps ~0 ≤ 15 → range_bound
        bars = _make_bars(30, start=100.0, spread=1.0)
        result = evaluate_regime("TEST", bars)
        self.assertEqual(result.regime_label, LABEL_RANGE_BOUND)
        self.assertTrue(result.passed)

    def test_range_bound_trend_move_small(self) -> None:
        bars = _make_bars(30, start=100.0, spread=1.0)
        result = evaluate_regime("TEST", bars)
        self.assertLessEqual(abs(result.move_bps), 15.0 + 1e-6)


# ---------------------------------------------------------------------------
# R04: high_volatility
# ---------------------------------------------------------------------------

class TestHighVolatility(unittest.TestCase):
    def test_high_volatility_label(self) -> None:
        # spread=5.0 → TR ~5.0, ATR ~5.0, atr_pct ~5% >> 2.0 threshold
        bars = _make_bars(30, start=100.0, spread=5.0)
        result = evaluate_regime("TEST", bars)
        self.assertEqual(result.regime_label, LABEL_HIGH_VOLATILITY)
        self.assertTrue(result.passed)

    def test_high_volatility_atr_pct_above_threshold(self) -> None:
        bars = _make_bars(30, start=100.0, spread=5.0)
        result = evaluate_regime("TEST", bars)
        self.assertGreaterEqual(result.atr_pct, 2.0)


# ---------------------------------------------------------------------------
# R05: low_volatility
# ---------------------------------------------------------------------------

class TestLowVolatility(unittest.TestCase):
    def test_low_volatility_label(self) -> None:
        # spread=0.1 → ATR ~0.1, atr_pct ~0.1% < 0.5%; close flat → trend_move ~0 < 15
        bars = _make_bars(30, start=100.0, spread=0.1)
        result = evaluate_regime("TEST", bars)
        self.assertEqual(result.regime_label, LABEL_LOW_VOLATILITY)
        self.assertTrue(result.passed)

    def test_low_volatility_atr_pct_below_threshold(self) -> None:
        bars = _make_bars(30, start=100.0, spread=0.1)
        result = evaluate_regime("TEST", bars)
        self.assertLessEqual(result.atr_pct, 0.5)


# ---------------------------------------------------------------------------
# R06: gapping
# ---------------------------------------------------------------------------

class TestGapping(unittest.TestCase):
    def test_gapping_label(self) -> None:
        # Normal bars then latest bar opens 3% above prior close
        bars = _with_gap(_make_bars(30, start=100.0, spread=0.8), gap_pct=3.0)
        result = evaluate_regime("TEST", bars)
        self.assertEqual(result.regime_label, LABEL_GAPPING)
        self.assertTrue(result.passed)

    def test_gapping_gap_pct_above_threshold(self) -> None:
        bars = _with_gap(_make_bars(30, start=100.0, spread=0.8), gap_pct=3.0)
        result = evaluate_regime("TEST", bars)
        self.assertGreaterEqual(abs(result.gap_pct), 2.0)

    def test_negative_gap_also_classified_as_gapping(self) -> None:
        bars = _with_gap(_make_bars(30, start=100.0, spread=0.8), gap_pct=-3.0)
        result = evaluate_regime("TEST", bars)
        self.assertEqual(result.regime_label, LABEL_GAPPING)


# ---------------------------------------------------------------------------
# R07: unclassified — insufficient bars
# ---------------------------------------------------------------------------

class TestUnclassifiedInsufficientBars(unittest.TestCase):
    def test_too_few_bars_returns_unclassified(self) -> None:
        # atr_bars default 14 → need ≥ 15 bars; supply 10
        bars = _make_bars(10, start=100.0)
        result = evaluate_regime("TEST", bars)
        self.assertEqual(result.regime_label, LABEL_UNCLASSIFIED)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, REASON_REGIME_UNCLASSIFIED)

    def test_empty_bars_returns_unclassified(self) -> None:
        result = evaluate_regime("TEST", [])
        self.assertEqual(result.regime_label, LABEL_UNCLASSIFIED)
        self.assertFalse(result.passed)

    def test_exactly_min_bars_does_not_raise(self) -> None:
        # 15 bars = atr_bars(14) + 1: should not crash
        bars = _make_bars(15, start=100.0, spread=0.1)
        result = evaluate_regime("TEST", bars)
        # Result may be any label; just must not raise
        self.assertIn(result.regime_label, (
            LABEL_TRENDING_UP, LABEL_TRENDING_DOWN, LABEL_RANGE_BOUND,
            LABEL_HIGH_VOLATILITY, LABEL_LOW_VOLATILITY, LABEL_GAPPING,
            LABEL_UNCLASSIFIED,
        ))


# ---------------------------------------------------------------------------
# R08: invalid / zero price
# ---------------------------------------------------------------------------

class TestInvalidZeroPrice(unittest.TestCase):
    def test_zero_latest_close_returns_unclassified(self) -> None:
        bars = _make_bars(30, start=100.0)
        bars[-1]["close"] = 0.0
        result = evaluate_regime("TEST", bars)
        self.assertEqual(result.regime_label, LABEL_UNCLASSIFIED)
        self.assertFalse(result.passed)

    def test_none_latest_close_returns_unclassified(self) -> None:
        bars = _make_bars(30, start=100.0)
        bars[-1]["close"] = None
        result = evaluate_regime("TEST", bars)
        self.assertEqual(result.regime_label, LABEL_UNCLASSIFIED)
        self.assertFalse(result.passed)

    def test_negative_close_returns_unclassified(self) -> None:
        bars = _make_bars(30, start=100.0)
        bars[-1]["close"] = -5.0
        result = evaluate_regime("TEST", bars)
        self.assertEqual(result.regime_label, LABEL_UNCLASSIFIED)
        self.assertFalse(result.passed)


# ---------------------------------------------------------------------------
# R09: ATR produces positive result for moving bars
# ---------------------------------------------------------------------------

class TestAtrPositive(unittest.TestCase):
    def test_atr_positive_for_bars_with_spread(self) -> None:
        bars = _make_bars(30, start=100.0, spread=1.5)
        result = evaluate_regime("TEST", bars)
        self.assertGreater(result.atr, 0.0)

    def test_atr_pct_positive(self) -> None:
        bars = _make_bars(30, start=100.0, spread=1.5)
        result = evaluate_regime("TEST", bars)
        self.assertGreater(result.atr_pct, 0.0)


# ---------------------------------------------------------------------------
# R10: move_bps and abs_move_bps
# ---------------------------------------------------------------------------

class TestMoveBps(unittest.TestCase):
    def test_move_bps_positive_for_rising_close(self) -> None:
        bars = _make_bars(30, start=100.0, end=101.0, spread=0.8)
        result = evaluate_regime("TEST", bars)
        self.assertGreater(result.move_bps, 0.0)

    def test_move_bps_negative_for_falling_close(self) -> None:
        bars = _make_bars(30, start=101.0, end=100.0, spread=0.8)
        result = evaluate_regime("TEST", bars)
        self.assertLess(result.move_bps, 0.0)

    def test_abs_move_bps_equals_abs_of_move_bps(self) -> None:
        bars = _make_bars(30, start=101.0, end=100.0, spread=0.8)
        result = evaluate_regime("TEST", bars)
        self.assertAlmostEqual(result.abs_move_bps, abs(result.move_bps), places=4)

    def test_abs_move_bps_nonnegative(self) -> None:
        bars = _make_bars(30, start=101.0, end=100.0, spread=0.8)
        result = evaluate_regime("TEST", bars)
        self.assertGreaterEqual(result.abs_move_bps, 0.0)


# ---------------------------------------------------------------------------
# R11: all scores bounded [0.0, 1.0]
# ---------------------------------------------------------------------------

class TestScoreBounds(unittest.TestCase):
    def _assert_scores_bounded(self, bars: list[dict[str, Any]]) -> None:
        result = evaluate_regime("TEST", bars)
        for name, val in [
            ("volatility_score", result.volatility_score),
            ("trend_score", result.trend_score),
            ("momentum_score", result.momentum_score),
            ("mean_reversion_score", result.mean_reversion_score),
            ("regime_score", result.regime_score),
        ]:
            self.assertGreaterEqual(val, 0.0, f"{name} below 0.0")
            self.assertLessEqual(val, 1.0, f"{name} above 1.0")

    def test_scores_bounded_trending_up(self) -> None:
        flat = _make_bars(10, start=99.0, end=99.0, spread=0.8)
        trend = _make_bars(20, start=99.0, end=100.0, spread=0.8)
        self._assert_scores_bounded(flat + trend)

    def test_scores_bounded_high_volatility(self) -> None:
        self._assert_scores_bounded(_make_bars(30, start=100.0, spread=5.0))

    def test_scores_bounded_low_volatility(self) -> None:
        self._assert_scores_bounded(_make_bars(30, start=100.0, spread=0.1))

    def test_scores_bounded_gapping(self) -> None:
        self._assert_scores_bounded(_with_gap(_make_bars(30, start=100.0, spread=0.8), gap_pct=4.0))

    def test_scores_bounded_insufficient_bars(self) -> None:
        self._assert_scores_bounded(_make_bars(5, start=100.0))

    def test_scores_bounded_extreme_move(self) -> None:
        # Massive trend — scores must still cap at 1.0
        flat = _make_bars(10, start=50.0, end=50.0, spread=0.5)
        trend = _make_bars(20, start=50.0, end=200.0, spread=0.5)
        self._assert_scores_bounded(flat + trend)


# ---------------------------------------------------------------------------
# R12: regime_score_below_min becomes rejection
# ---------------------------------------------------------------------------

class TestRegimeScoreBelowMin(unittest.TestCase):
    def test_barely_trending_up_rejected_below_min_score(self) -> None:
        # trend_move_bps ≈ 22 → regime_score = 22/100 = 0.22 < 0.5
        flat = _make_bars(10, start=100.0, end=100.0, spread=0.8)
        trend = _make_bars(20, start=100.0, end=100.22, spread=0.8)
        result = evaluate_regime("TEST", flat + trend)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, REASON_REGIME_SCORE_BELOW_MIN)
        self.assertEqual(result.regime_label, LABEL_TRENDING_UP)
        self.assertLess(result.regime_score, 0.5)

    def test_custom_min_score_zero_accepts_everything(self) -> None:
        # With min_regime_score=0.0, any classified label should pass
        cfg = RegimeConfig(min_regime_score=0.0)
        flat = _make_bars(10, start=100.0, end=100.0, spread=0.8)
        trend = _make_bars(20, start=100.0, end=100.22, spread=0.8)
        result = evaluate_regime("TEST", flat + trend, cfg)
        self.assertTrue(result.passed)
        self.assertIsNone(result.rejection_reason)


# ---------------------------------------------------------------------------
# R13: rejection helper writes scanner-candidate-v1 with eligible_for_live=False
# ---------------------------------------------------------------------------

class TestRejectionHelperRecord(unittest.TestCase):
    def _failing_result(self) -> object:
        return evaluate_regime("TEST", _make_bars(5))  # too few bars

    def test_eligible_for_live_always_false(self) -> None:
        result = self._failing_result()
        record = build_regime_rejection_candidate(
            scanner_id="main-01",
            symbol="TEST",
            exchange="NASDAQ",
            timeframe="5Min",
            source="test",
            latest_bar_ts="2026-06-03T16:00:00+00:00",
            regime_result=result,
        )
        self.assertIs(record["eligible_for_live"], False)

    def test_all_eligibility_false(self) -> None:
        result = self._failing_result()
        record = build_regime_rejection_candidate(
            scanner_id="main-01",
            symbol="TEST",
            exchange="NASDAQ",
            timeframe="5Min",
            source="test",
            latest_bar_ts="2026-06-03T16:00:00+00:00",
            regime_result=result,
        )
        self.assertIs(record["eligible_for_scan"], False)
        self.assertIs(record["eligible_for_backtest"], False)
        self.assertIs(record["eligible_for_paper"], False)
        self.assertIs(record["eligible_for_live"], False)

    def test_schema_version_is_scanner_candidate_v1(self) -> None:
        result = self._failing_result()
        record = build_regime_rejection_candidate(
            scanner_id="main-01",
            symbol="TEST",
            exchange="NASDAQ",
            timeframe="5Min",
            source="test",
            latest_bar_ts="2026-06-03T16:00:00+00:00",
            regime_result=result,
        )
        self.assertEqual(record["schema_version"], "scanner-candidate-v1")

    def test_rejection_reason_from_result(self) -> None:
        result = self._failing_result()
        record = build_regime_rejection_candidate(
            scanner_id="main-01",
            symbol="TEST",
            exchange="NASDAQ",
            timeframe="5Min",
            source="test",
            latest_bar_ts="2026-06-03T16:00:00+00:00",
            regime_result=result,
        )
        self.assertEqual(record["rejection_reason"], result.rejection_reason)

    def test_regime_score_below_min_produces_correct_reason(self) -> None:
        flat = _make_bars(10, start=100.0, end=100.0, spread=0.8)
        trend = _make_bars(20, start=100.0, end=100.22, spread=0.8)
        result = evaluate_regime("TEST", flat + trend)
        self.assertFalse(result.passed)
        record = build_regime_rejection_candidate(
            scanner_id="main-01",
            symbol="AAPL",
            exchange="NASDAQ",
            timeframe="5Min",
            source="test",
            latest_bar_ts="2026-06-03T16:00:00+00:00",
            regime_result=result,
        )
        self.assertEqual(record["rejection_reason"], REASON_REGIME_SCORE_BELOW_MIN)


# ---------------------------------------------------------------------------
# R14: rejection helper writes JSONL through ScannerCandidateWriter
# ---------------------------------------------------------------------------

class TestRejectionHelperJsonlWrite(unittest.TestCase):
    def test_writes_jsonl_via_scanner_candidate_writer(self) -> None:
        from mqk_research.scanner.candidates import ScannerCandidateWriter

        bars = _make_bars(5)  # too few → unclassified
        result = evaluate_regime("TEST", bars)
        self.assertFalse(result.passed)

        record = build_regime_rejection_candidate(
            scanner_id="main-regime-test",
            symbol="TSLA",
            exchange="NASDAQ",
            timeframe="5Min",
            source="test",
            latest_bar_ts="2026-06-03T16:00:00+00:00",
            regime_result=result,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            with ScannerCandidateWriter("main-regime-test", base_dir=tmpdir) as writer:
                path = writer.write_rejection(record)
            lines = pathlib.Path(path).read_text(encoding="utf-8").strip().splitlines()

        self.assertEqual(len(lines), 1)
        obj = json.loads(lines[0])
        self.assertEqual(obj["schema_version"], "scanner-candidate-v1")
        self.assertIs(obj["eligible_for_live"], False)
        self.assertIs(obj["eligible_for_scan"], False)

    def test_write_routes_rejection_automatically(self) -> None:
        from mqk_research.scanner.candidates import ScannerCandidateWriter

        bars = _make_bars(5)
        result = evaluate_regime("TEST", bars)
        record = build_regime_rejection_candidate(
            scanner_id="main-02",
            symbol="TEST",
            exchange="NYSE",
            timeframe="5Min",
            source="test",
            latest_bar_ts="2026-06-03T16:00:00+00:00",
            regime_result=result,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            with ScannerCandidateWriter("main-02", base_dir=tmpdir) as writer:
                writer.write(record)
            self.assertEqual(writer.rejected_count, 1)
            self.assertEqual(writer.accepted_count, 0)


# ---------------------------------------------------------------------------
# R15: passing regime result raises ValueError in rejection helper
# ---------------------------------------------------------------------------

class TestPassingResultRaisesOnRejectionHelper(unittest.TestCase):
    def test_passing_result_raises_value_error(self) -> None:
        flat = _make_bars(10, start=99.0, end=99.0, spread=0.8)
        trend = _make_bars(20, start=99.0, end=100.0, spread=0.8)
        result = evaluate_regime("TEST", flat + trend)
        self.assertTrue(result.passed, "Expected a passing result for this test")

        with self.assertRaises(ValueError):
            build_regime_rejection_candidate(
                scanner_id="main-01",
                symbol="TEST",
                exchange="NASDAQ",
                timeframe="5Min",
                source="test",
                latest_bar_ts="2026-06-03T16:00:00+00:00",
                regime_result=result,
            )


# ---------------------------------------------------------------------------
# R16: no forbidden imports in regime.py
# ---------------------------------------------------------------------------

class TestNoForbiddenImports(unittest.TestCase):
    def _regime_source(self) -> str:
        module_path = (
            pathlib.Path(__file__).parent.parent
            / "src"
            / "mqk_research"
            / "scanner"
            / "regime.py"
        )
        return module_path.read_text(encoding="utf-8")

    def test_no_broker_imports(self) -> None:
        src = self._regime_source()
        for forbidden in (
            "broker_adapter",
            "oms_outbox",
            "oms_inbox",
            "/v2/orders",
            "submit_order",
            "live_routing_enabled=True",
        ):
            self.assertNotIn(forbidden, src, f"regime.py must not contain '{forbidden}'")

    def test_no_network_db_imports(self) -> None:
        src = self._regime_source()
        for forbidden in (
            "import requests",
            "import urllib",
            "import http.client",
            "import aiohttp",
            "import psycopg",
            "import sqlalchemy",
        ):
            self.assertNotIn(forbidden, src, f"regime.py must not contain '{forbidden}'")


# ---------------------------------------------------------------------------
# R17: apply_regime_to_candidate_fields helper
# ---------------------------------------------------------------------------

class TestApplyRegimeToCandidateFields(unittest.TestCase):
    def test_returns_expected_keys(self) -> None:
        flat = _make_bars(10, start=99.0, end=99.0, spread=0.8)
        trend = _make_bars(20, start=99.0, end=100.0, spread=0.8)
        result = evaluate_regime("TEST", flat + trend)
        fields = apply_regime_to_candidate_fields(result)
        for key in (
            "atr",
            "gap_pct",
            "move_bps",
            "abs_move_bps",
            "trend_score",
            "momentum_score",
            "mean_reversion_score",
            "volatility_score",
            "regime_label",
            "regime_score",
            "latest_close",
            "lookback_close",
        ):
            self.assertIn(key, fields, f"apply_regime_to_candidate_fields missing '{key}'")

    def test_values_match_result_fields(self) -> None:
        flat = _make_bars(10, start=99.0, end=99.0, spread=0.8)
        trend = _make_bars(20, start=99.0, end=100.0, spread=0.8)
        result = evaluate_regime("TEST", flat + trend)
        fields = apply_regime_to_candidate_fields(result)
        self.assertEqual(fields["regime_label"], result.regime_label)
        self.assertEqual(fields["regime_score"], result.regime_score)
        self.assertEqual(fields["atr"], result.atr)


if __name__ == "__main__":
    unittest.main()
