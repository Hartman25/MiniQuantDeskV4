"""
SCANNER-DQ-01 — Data-quality gate tests.

Run:
    $env:PYTHONPATH="research-py;research-py\\src"
    python -m unittest research-py/tests/test_scanner_data_quality.py
"""
from __future__ import annotations

import json
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _utc(offset_minutes: float = 0.0) -> datetime:
    return datetime(2026, 6, 4, 14, 0, 0, tzinfo=timezone.utc) + timedelta(minutes=offset_minutes)


def _bar(
    offset_minutes: float,
    *,
    open_: float = 100.0,
    high: float = 101.0,
    low: float = 99.0,
    close: float = 100.5,
    volume: int = 10_000,
    is_complete: bool | None = None,
) -> dict[str, Any]:
    ts = _utc(offset_minutes).isoformat()
    b: dict[str, Any] = {
        "ts": ts,
        "open": open_,
        "high": high,
        "low": low,
        "close": close,
        "volume": volume,
    }
    if is_complete is not None:
        b["is_complete"] = is_complete
    return b


def _clean_bars(
    count: int = 30,
    interval_minutes: float = 5.0,
    start_offset: float = -(30 * 5.0),
) -> list[dict[str, Any]]:
    """Generate count clean 5m completed bars ending near now."""
    bars = []
    for i in range(count):
        offset = start_offset + i * interval_minutes
        bars.append(_bar(offset))
    return bars


# Reference "now" for all freshness tests
_NOW = _utc(0.0)


# ---------------------------------------------------------------------------
# DQ-01: pass case — 30 clean completed 5m bars, fresh latest bar
# ---------------------------------------------------------------------------

class TestPassCase(unittest.TestCase):
    def test_clean_bars_pass(self) -> None:
        from mqk_research.scanner.data_quality import evaluate_data_quality, DataQualityConfig
        bars = _clean_bars(30)
        cfg = DataQualityConfig(max_bar_age_minutes=20.0)
        result = evaluate_data_quality("AAPL", "5Min", bars, cfg, now_utc=_NOW)
        self.assertTrue(result.passed)
        self.assertIsNone(result.rejection_reason)
        self.assertGreater(result.score, 0.0)

    def test_pass_score_in_bounds(self) -> None:
        from mqk_research.scanner.data_quality import evaluate_data_quality, DataQualityConfig
        bars = _clean_bars(30)
        result = evaluate_data_quality("AAPL", "5Min", bars, now_utc=_NOW)
        self.assertGreaterEqual(result.score, 0.0)
        self.assertLessEqual(result.score, 1.0)


# ---------------------------------------------------------------------------
# DQ-02: no bars → no_bars
# ---------------------------------------------------------------------------

class TestNoBars(unittest.TestCase):
    def test_empty_bars(self) -> None:
        from mqk_research.scanner.data_quality import evaluate_data_quality, REASON_NO_BARS
        result = evaluate_data_quality("AAPL", "5Min", [], now_utc=_NOW)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, REASON_NO_BARS)
        self.assertEqual(result.score, 0.0)


# ---------------------------------------------------------------------------
# DQ-03: insufficient completed bars → insufficient_completed_bars
# ---------------------------------------------------------------------------

class TestInsufficientBars(unittest.TestCase):
    def test_only_5_bars(self) -> None:
        from mqk_research.scanner.data_quality import (
            evaluate_data_quality, DataQualityConfig, REASON_INSUFFICIENT_COMPLETED_BARS,
        )
        bars = _clean_bars(5)
        cfg = DataQualityConfig(min_bars_required=30, max_bar_age_minutes=9999)
        result = evaluate_data_quality("AAPL", "5Min", bars, cfg, now_utc=_NOW)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, REASON_INSUFFICIENT_COMPLETED_BARS)


# ---------------------------------------------------------------------------
# DQ-04: latest bar stale → latest_bar_stale
# ---------------------------------------------------------------------------

class TestLatestBarStale(unittest.TestCase):
    def test_stale_latest_bar(self) -> None:
        from mqk_research.scanner.data_quality import (
            evaluate_data_quality, DataQualityConfig, REASON_LATEST_BAR_STALE,
        )
        # Latest bar is 60 minutes old; threshold is 20
        bars = _clean_bars(30, start_offset=-(30 * 5.0 + 40))
        cfg = DataQualityConfig(max_bar_age_minutes=20.0)
        result = evaluate_data_quality("AAPL", "5Min", bars, cfg, now_utc=_NOW)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, REASON_LATEST_BAR_STALE)


# ---------------------------------------------------------------------------
# DQ-05: duplicate timestamp → duplicate_bar_timestamp
# ---------------------------------------------------------------------------

class TestDuplicateTimestamp(unittest.TestCase):
    def test_duplicate_ts(self) -> None:
        from mqk_research.scanner.data_quality import (
            evaluate_data_quality, REASON_DUPLICATE_BAR_TIMESTAMP,
        )
        bars = _clean_bars(30)
        bars[-1]["ts"] = bars[-2]["ts"]  # introduce duplicate
        result = evaluate_data_quality("AAPL", "5Min", bars, now_utc=_NOW)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, REASON_DUPLICATE_BAR_TIMESTAMP)


# ---------------------------------------------------------------------------
# DQ-06: too many zero-volume bars → too_many_zero_volume_bars
# ---------------------------------------------------------------------------

class TestTooManyZeroVolume(unittest.TestCase):
    def test_high_zero_volume_fraction(self) -> None:
        from mqk_research.scanner.data_quality import (
            evaluate_data_quality, DataQualityConfig, REASON_TOO_MANY_ZERO_VOLUME_BARS,
        )
        bars = _clean_bars(30)
        # Zero out 5 of 30 bars → 16.7% > 5% threshold
        for i in range(5):
            bars[i]["volume"] = 0
        cfg = DataQualityConfig(max_zero_volume_fraction=0.05)
        result = evaluate_data_quality("AAPL", "5Min", bars, cfg, now_utc=_NOW)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, REASON_TOO_MANY_ZERO_VOLUME_BARS)
        self.assertGreaterEqual(result.zero_volume_count, 5)


# ---------------------------------------------------------------------------
# DQ-07: zero price bar → zero_price_bar
# ---------------------------------------------------------------------------

class TestZeroPrice(unittest.TestCase):
    def test_zero_close(self) -> None:
        from mqk_research.scanner.data_quality import evaluate_data_quality, REASON_ZERO_PRICE_BAR
        bars = _clean_bars(30)
        bars[10]["close"] = 0.0
        bars[10]["low"] = 0.0
        result = evaluate_data_quality("AAPL", "5Min", bars, now_utc=_NOW)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, REASON_ZERO_PRICE_BAR)

    def test_zero_open(self) -> None:
        from mqk_research.scanner.data_quality import evaluate_data_quality, REASON_ZERO_PRICE_BAR
        bars = _clean_bars(30)
        bars[5]["open"] = 0.0
        result = evaluate_data_quality("AAPL", "5Min", bars, now_utc=_NOW)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, REASON_ZERO_PRICE_BAR)


# ---------------------------------------------------------------------------
# DQ-08: high < low → inverted_ohlc
# ---------------------------------------------------------------------------

class TestInvertedOHLC(unittest.TestCase):
    def test_high_below_low(self) -> None:
        from mqk_research.scanner.data_quality import evaluate_data_quality, REASON_INVERTED_OHLC
        bars = _clean_bars(30)
        bars[15]["high"] = 98.0
        bars[15]["low"] = 102.0
        bars[15]["close"] = 99.0  # close outside range too, but inverted fires first
        result = evaluate_data_quality("AAPL", "5Min", bars, now_utc=_NOW)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, REASON_INVERTED_OHLC)


# ---------------------------------------------------------------------------
# DQ-09: close outside [low, high] → close_outside_ohlc_range
# ---------------------------------------------------------------------------

class TestCloseOutsideRange(unittest.TestCase):
    def test_close_above_high(self) -> None:
        from mqk_research.scanner.data_quality import (
            evaluate_data_quality, REASON_CLOSE_OUTSIDE_OHLC_RANGE,
        )
        bars = _clean_bars(30)
        bars[20]["close"] = 110.0  # above high=101
        result = evaluate_data_quality("AAPL", "5Min", bars, now_utc=_NOW)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, REASON_CLOSE_OUTSIDE_OHLC_RANGE)

    def test_close_below_low(self) -> None:
        from mqk_research.scanner.data_quality import (
            evaluate_data_quality, REASON_CLOSE_OUTSIDE_OHLC_RANGE,
        )
        bars = _clean_bars(30)
        bars[20]["close"] = 80.0  # below low=99
        result = evaluate_data_quality("AAPL", "5Min", bars, now_utc=_NOW)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, REASON_CLOSE_OUTSIDE_OHLC_RANGE)


# ---------------------------------------------------------------------------
# DQ-10: missing 5m gap greater than allowed → missing_bar_gap
# ---------------------------------------------------------------------------

class TestMissingBarGap(unittest.TestCase):
    def test_large_gap_detected(self) -> None:
        from mqk_research.scanner.data_quality import (
            evaluate_data_quality, DataQualityConfig, REASON_MISSING_BAR_GAP,
        )
        bars = _clean_bars(30, interval_minutes=5.0)
        # Introduce a 30-minute gap between bar 15 and 16
        ts_15 = datetime.fromisoformat(bars[15]["ts"])
        for i in range(16, 30):
            delta = (i - 16) * 5.0 + 30.0  # shifted by 30 extra minutes
            bars[i]["ts"] = (ts_15 + timedelta(minutes=delta)).isoformat()
        cfg = DataQualityConfig(max_missing_gap_bars=2, expected_bar_interval_minutes=5.0)
        result = evaluate_data_quality("AAPL", "5Min", bars, cfg, now_utc=_NOW)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, REASON_MISSING_BAR_GAP)
        self.assertGreater(result.missing_gap_count, 0)


# ---------------------------------------------------------------------------
# DQ-11: latest incomplete bar behavior → latest_bar_incomplete
# ---------------------------------------------------------------------------

class TestLatestBarIncomplete(unittest.TestCase):
    def test_incomplete_flag_causes_rejection_when_remaining_insufficient(self) -> None:
        from mqk_research.scanner.data_quality import (
            evaluate_data_quality, DataQualityConfig, REASON_LATEST_BAR_INCOMPLETE,
        )
        # 30 bars but last one is incomplete — only 29 completed
        bars = _clean_bars(30)
        bars[-1]["is_complete"] = False
        cfg = DataQualityConfig(min_bars_required=30, max_bar_age_minutes=9999)
        result = evaluate_data_quality("AAPL", "5Min", bars, cfg, now_utc=_NOW)
        self.assertFalse(result.passed)
        self.assertEqual(result.rejection_reason, REASON_LATEST_BAR_INCOMPLETE)

    def test_incomplete_flag_passes_when_enough_remaining(self) -> None:
        from mqk_research.scanner.data_quality import evaluate_data_quality, DataQualityConfig
        # 31 bars, last incomplete → 30 completed → passes
        bars = _clean_bars(31)
        bars[-1]["is_complete"] = False
        cfg = DataQualityConfig(min_bars_required=30, max_bar_age_minutes=9999)
        result = evaluate_data_quality("AAPL", "5Min", bars, cfg, now_utc=_NOW)
        self.assertTrue(result.passed)


# ---------------------------------------------------------------------------
# DQ-12: score bounds [0.0, 1.0]
# ---------------------------------------------------------------------------

class TestScoreBounds(unittest.TestCase):
    def test_pass_score_between_0_and_1(self) -> None:
        from mqk_research.scanner.data_quality import evaluate_data_quality
        bars = _clean_bars(30)
        result = evaluate_data_quality("AAPL", "5Min", bars, now_utc=_NOW)
        self.assertGreaterEqual(result.score, 0.0)
        self.assertLessEqual(result.score, 1.0)

    def test_fail_score_is_0(self) -> None:
        from mqk_research.scanner.data_quality import evaluate_data_quality
        result = evaluate_data_quality("AAPL", "5Min", [], now_utc=_NOW)
        self.assertEqual(result.score, 0.0)


# ---------------------------------------------------------------------------
# DQ-13: rejection helper writes scanner-candidate-v1 with eligible_for_live=False
# ---------------------------------------------------------------------------

class TestRejectionHelperSchema(unittest.TestCase):
    def _make_dq_fail(self) -> Any:
        from mqk_research.scanner.data_quality import evaluate_data_quality
        return evaluate_data_quality("AAPL", "5Min", [], now_utc=_NOW)

    def test_rejection_record_schema_version(self) -> None:
        from mqk_research.scanner.data_quality import build_data_quality_rejection_candidate
        dq = self._make_dq_fail()
        rec = build_data_quality_rejection_candidate(
            scanner_id="main-large-cap-01",
            symbol="AAPL",
            exchange="NASDAQ",
            timeframe="5Min",
            source="alpaca_bars",
            dq_result=dq,
        )
        self.assertEqual(rec["schema_version"], "scanner-candidate-v1")
        self.assertIs(rec["eligible_for_live"], False)
        self.assertIs(rec["eligible_for_scan"], False)
        self.assertIs(rec["eligible_for_backtest"], False)
        self.assertIs(rec["eligible_for_paper"], False)

    def test_rejection_reason_populated(self) -> None:
        from mqk_research.scanner.data_quality import (
            build_data_quality_rejection_candidate, REASON_NO_BARS,
        )
        dq = self._make_dq_fail()
        rec = build_data_quality_rejection_candidate(
            scanner_id="main-large-cap-01",
            symbol="AAPL",
            exchange="NASDAQ",
            timeframe="5Min",
            source="alpaca_bars",
            dq_result=dq,
        )
        self.assertEqual(rec["rejection_reason"], REASON_NO_BARS)

    def test_passing_dq_raises(self) -> None:
        from mqk_research.scanner.data_quality import (
            build_data_quality_rejection_candidate, evaluate_data_quality,
        )
        bars = _clean_bars(30)
        dq = evaluate_data_quality("AAPL", "5Min", bars, now_utc=_NOW)
        self.assertTrue(dq.passed)
        with self.assertRaises(ValueError):
            build_data_quality_rejection_candidate(
                scanner_id="main-large-cap-01",
                symbol="AAPL",
                exchange="NASDAQ",
                timeframe="5Min",
                source="alpaca_bars",
                dq_result=dq,
            )


# ---------------------------------------------------------------------------
# DQ-14: rejection helper writes JSONL through ScannerCandidateWriter
# ---------------------------------------------------------------------------

class TestRejectionHelperWritesJSONL(unittest.TestCase):
    def test_write_rejection_via_writer(self) -> None:
        from mqk_research.scanner.data_quality import (
            build_data_quality_rejection_candidate, evaluate_data_quality,
        )
        from mqk_research.scanner.candidates import ScannerCandidateWriter
        dq = evaluate_data_quality("AAPL", "5Min", [], now_utc=_NOW)
        rec = build_data_quality_rejection_candidate(
            scanner_id="main-large-cap-01",
            symbol="AAPL",
            exchange="NASDAQ",
            timeframe="5Min",
            source="alpaca_bars",
            dq_result=dq,
        )
        with tempfile.TemporaryDirectory() as tmpdir:
            with ScannerCandidateWriter("main-large-cap-01", base_dir=tmpdir) as writer:
                path = writer.write_rejection(rec)
            obj = json.loads(Path(path).read_text(encoding="utf-8").strip())
        self.assertEqual(obj["schema_version"], "scanner-candidate-v1")
        self.assertIs(obj["eligible_for_live"], False)
        self.assertFalse(obj["eligible_for_scan"])
        self.assertIsNotNone(obj["rejection_reason"])
        self.assertIn("data_quality", obj["reason_tags"])


# ---------------------------------------------------------------------------
# DQ-15: no broker/OMS/execution/network/DB imports in data_quality.py
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

    def test_no_forbidden_in_data_quality_module(self) -> None:
        dq_path = (
            Path(__file__).parent.parent / "src" / "mqk_research" / "scanner" / "data_quality.py"
        )
        self.assertTrue(dq_path.exists(), f"Module not found: {dq_path}")
        content = dq_path.read_text(encoding="utf-8")
        for forbidden in self._FORBIDDEN:
            self.assertNotIn(
                forbidden,
                content,
                f"data_quality.py must not reference '{forbidden}'",
            )


if __name__ == "__main__":
    unittest.main()
