"""
SYMBOL-INPUTS-PRODUCER-01 — Tests for the premarket symbol_inputs artifact producer.

Run:
    $env:PYTHONPATH="research-py;research-py\\src"
    python -m unittest research-py/tests/test_scanner_symbol_inputs.py

All tests are pure in-process, deterministic-fixture based. No DB, no broker,
no network, no daemon — file side effects only via tempfile.
"""
from __future__ import annotations

import copy
import json
import os
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from mqk_research.scanner.data_quality import (
    REASON_INSUFFICIENT_COMPLETED_BARS,
    REASON_LATEST_BAR_STALE,
    REASON_NO_BARS,
    REASON_ZERO_PRICE_BAR,
)
from mqk_research.scanner.liquidity import LiquidityMetrics
from mqk_research.scanner.regime import LABEL_TRENDING_UP, LABEL_UNCLASSIFIED, REASON_REGIME_UNCLASSIFIED
from mqk_research.scanner.symbol_inputs import (
    REASON_LIQUIDITY_METRICS_MISSING,
    REASON_MISSING_LATEST_PRICE,
    SCHEMA_VERSION,
    SymbolInputSpec,
    build_symbol_input_record,
    build_symbol_inputs,
    extract_symbol_inputs_map,
    load_symbol_inputs_artifact,
    write_symbol_inputs_artifact,
)
from mqk_research.scanner.premarket_revalidation import (
    REASON_PREMARKET_BAR_STALE,
    REASON_PREMARKET_LIQUIDITY_FAILED,
    REASON_PREMARKET_RVOL_TOO_LOW,
    REASON_PREMARKET_SLIPPAGE_TOO_HIGH,
    REASON_PREMARKET_SPREAD_TOO_WIDE,
    REASON_PREMARKET_SYMBOL_INPUT_MISSING,
    evaluate_premarket_watchlist,
)


# ---------------------------------------------------------------------------
# Bar fixtures — deterministic, no network/API access
# ---------------------------------------------------------------------------

_NOW = datetime(2026, 6, 5, 14, 0, 0, tzinfo=timezone.utc)


def _bar(ts: datetime, close: float, *, spread: float = 0.8, volume: int = 2_000_000) -> dict[str, Any]:
    return {
        "ts": ts.isoformat(),
        "open": close,
        "high": close + spread / 2.0,
        "low": close - spread / 2.0,
        "close": close,
        "volume": volume,
    }


def _trending_up_bars(
    flat_n: int = 10,
    trend_n: int = 20,
    *,
    flat_close: float = 99.0,
    trend_end_close: float = 100.0,
    spread: float = 0.8,
    interval_minutes: float = 5.0,
    latest_ts: datetime | None = None,
) -> list[dict[str, Any]]:
    """30 bars: 10 flat at 99.0, then 20 rising linearly to 100.0 -> trending_up.

    Mirrors the fixture pattern proven to classify as trending_up in
    test_scanner_regime.py (TestTrendingUp).
    """
    if latest_ts is None:
        latest_ts = _NOW - timedelta(minutes=interval_minutes)
    total = flat_n + trend_n
    closes = [flat_close] * flat_n + [
        flat_close + (trend_end_close - flat_close) * (i / max(trend_n - 1, 1))
        for i in range(trend_n)
    ]
    bars = []
    for idx, c in enumerate(closes):
        ts = latest_ts - timedelta(minutes=(total - 1 - idx) * interval_minutes)
        bars.append(_bar(ts, c, spread=spread))
    return bars


def _linear_bars(
    n: int,
    *,
    start: float = 100.0,
    end: float | None = None,
    spread: float = 0.8,
    interval_minutes: float = 5.0,
    latest_ts: datetime | None = None,
) -> list[dict[str, Any]]:
    if end is None:
        end = start
    if latest_ts is None:
        latest_ts = _NOW - timedelta(minutes=interval_minutes)
    bars = []
    for i in range(n):
        frac = i / max(n - 1, 1)
        c = start + (end - start) * frac
        ts = latest_ts - timedelta(minutes=(n - 1 - i) * interval_minutes)
        bars.append(_bar(ts, c, spread=spread))
    return bars


def _unclassified_regime_bars() -> list[dict[str, Any]]:
    """30 bars with a moderate trend (~17 bps) and moderate ATR (~0.8%) —
    lands strictly between range_bound/trending thresholds -> unclassified.
    """
    return _linear_bars(30, start=100.0, end=100.25, spread=0.8)


_PASSING_LIQUIDITY_METRICS = LiquidityMetrics(
    latest_close=190.0,
    avg_daily_volume_shares=50_000_000.0,
    avg_daily_dollar_volume=9_500_000_000.0,
    relative_volume=1.5,
    spread_estimate_bps=8.0,
    slippage_estimate_bps=6.0,
    round_trip_cost_bps=14.0,
)

_FAILING_LIQUIDITY_METRICS = LiquidityMetrics(
    latest_close=190.0,
    avg_daily_volume_shares=100_000.0,
    avg_daily_dollar_volume=500_000.0,
    relative_volume=0.2,
    spread_estimate_bps=8.0,
    slippage_estimate_bps=6.0,
    round_trip_cost_bps=14.0,
)


def _make_watchlist(symbol: str = "AAPL", strategy_id: str = "intraday_scalper") -> dict[str, Any]:
    return {
        "schema_version": "watchlist-v1",
        "mode": "paper",
        "approved_for_live": False,
        "max_symbols_to_trade": 1,
        "max_concurrent_positions": 1,
        "symbols": [symbol],
        "strategy_assignments": {symbol: strategy_id},
    }


# ---------------------------------------------------------------------------
# SI01: valid bars produce expected passing record
# ---------------------------------------------------------------------------

class TestPassingRecord(unittest.TestCase):
    def test_si01_valid_bars_produce_expected_record(self) -> None:
        spec = SymbolInputSpec(
            symbol="AAPL",
            timeframe="5m",
            bars=_trending_up_bars(),
            liquidity_metrics=_PASSING_LIQUIDITY_METRICS,
        )
        record = build_symbol_input_record(spec, now_utc=_NOW)

        self.assertEqual(record["symbol"], "AAPL")
        self.assertEqual(record["timeframe"], "5m")
        self.assertTrue(record["data_quality_passed"])
        self.assertTrue(record["liquidity_passed"])
        self.assertEqual(record["regime_label"], LABEL_TRENDING_UP)
        self.assertIsNotNone(record["latest_bar_ts"])
        self.assertEqual(record["latest_close"], 100.0)
        self.assertEqual(record["spread_estimate_bps"], 8.0)
        self.assertEqual(record["slippage_estimate_bps"], 6.0)
        self.assertEqual(record["relative_volume"], 1.5)
        self.assertIsNone(record["rejection_reason"])
        self.assertFalse(record["suppressed"])

    def test_si01_artifact_wraps_record_under_symbol_key(self) -> None:
        spec = SymbolInputSpec(
            symbol="AAPL", timeframe="5m", bars=_trending_up_bars(),
            liquidity_metrics=_PASSING_LIQUIDITY_METRICS,
        )
        artifact = build_symbol_inputs([spec], trade_date="2026-06-08", source="test_fixture", now_utc=_NOW)
        self.assertEqual(artifact["schema_version"], SCHEMA_VERSION)
        self.assertEqual(artifact["trade_date"], "2026-06-08")
        self.assertEqual(artifact["source"], "test_fixture")
        self.assertIn("AAPL", artifact["symbols"])
        self.assertTrue(artifact["symbols"]["AAPL"]["data_quality_passed"])
        self.assertFalse(artifact["approved_for_live"])


# ---------------------------------------------------------------------------
# SI02: no bars fails closed
# ---------------------------------------------------------------------------

class TestNoBars(unittest.TestCase):
    def test_si02_no_bars_fails_closed(self) -> None:
        spec = SymbolInputSpec(symbol="AAPL", timeframe="5m", bars=[])
        record = build_symbol_input_record(spec, now_utc=_NOW)

        self.assertFalse(record["data_quality_passed"])
        self.assertEqual(record["rejection_reason"], REASON_NO_BARS)
        self.assertFalse(record["liquidity_passed"])
        self.assertIsNone(record["regime_label"])
        self.assertIsNone(record["latest_bar_ts"])
        self.assertIsNone(record["latest_close"])


# ---------------------------------------------------------------------------
# SI03: insufficient bars fails closed
# ---------------------------------------------------------------------------

class TestInsufficientBars(unittest.TestCase):
    def test_si03_insufficient_bars_fails_closed(self) -> None:
        spec = SymbolInputSpec(
            symbol="AAPL", timeframe="5m",
            bars=_trending_up_bars(flat_n=5, trend_n=10),  # 15 < 30 required
        )
        record = build_symbol_input_record(spec, now_utc=_NOW)

        self.assertFalse(record["data_quality_passed"])
        self.assertEqual(record["rejection_reason"], REASON_INSUFFICIENT_COMPLETED_BARS)
        self.assertFalse(record["liquidity_passed"])
        self.assertIsNone(record["regime_label"])


# ---------------------------------------------------------------------------
# SI04: stale latest bar fails closed
# ---------------------------------------------------------------------------

class TestStaleBar(unittest.TestCase):
    def test_si04_stale_latest_bar_fails_closed(self) -> None:
        spec = SymbolInputSpec(
            symbol="AAPL", timeframe="5m",
            bars=_trending_up_bars(latest_ts=_NOW - timedelta(minutes=60)),
        )
        record = build_symbol_input_record(spec, now_utc=_NOW)

        self.assertFalse(record["data_quality_passed"])
        self.assertEqual(record["rejection_reason"], REASON_LATEST_BAR_STALE)
        # The stale ts is still reported honestly (not hidden) for diagnosis.
        self.assertIsNotNone(record["latest_bar_ts"])
        self.assertFalse(record["liquidity_passed"])
        self.assertIsNone(record["regime_label"])


# ---------------------------------------------------------------------------
# SI05: data quality failure (other than no-bars/insufficient/stale) is represented
# ---------------------------------------------------------------------------

class TestDataQualityFailureRepresented(unittest.TestCase):
    def test_si05_zero_price_bar_represented(self) -> None:
        bars = copy.deepcopy(_trending_up_bars())
        bars[-1]["close"] = 0.0
        spec = SymbolInputSpec(symbol="AAPL", timeframe="5m", bars=bars)
        record = build_symbol_input_record(spec, now_utc=_NOW)

        self.assertFalse(record["data_quality_passed"])
        self.assertEqual(record["rejection_reason"], REASON_ZERO_PRICE_BAR)
        self.assertFalse(record["liquidity_passed"])
        self.assertIsNone(record["regime_label"])
        self.assertIsNone(record["latest_close"])


# ---------------------------------------------------------------------------
# SI06: liquidity failure is represented
# ---------------------------------------------------------------------------

class TestLiquidityFailureRepresented(unittest.TestCase):
    def test_si06_liquidity_gate_failure_represented(self) -> None:
        spec = SymbolInputSpec(
            symbol="AAPL", timeframe="5m", bars=_trending_up_bars(),
            liquidity_metrics=_FAILING_LIQUIDITY_METRICS,
        )
        record = build_symbol_input_record(spec, now_utc=_NOW)

        self.assertTrue(record["data_quality_passed"])
        self.assertFalse(record["liquidity_passed"])
        self.assertIsNotNone(record["rejection_reason"])
        self.assertIn("liquidity", " ".join(record["reason_tags"]))

    def test_si06_liquidity_metrics_missing_fails_closed(self) -> None:
        """Absent liquidity_metrics must NOT be reported as passing/healthy."""
        spec = SymbolInputSpec(symbol="AAPL", timeframe="5m", bars=_trending_up_bars())
        record = build_symbol_input_record(spec, now_utc=_NOW)

        self.assertTrue(record["data_quality_passed"])
        self.assertFalse(record["liquidity_passed"])
        self.assertEqual(record["rejection_reason"], REASON_LIQUIDITY_METRICS_MISSING)
        self.assertIn(REASON_LIQUIDITY_METRICS_MISSING, record["reason_tags"])
        self.assertIsNone(record["spread_estimate_bps"])
        self.assertIsNone(record["slippage_estimate_bps"])
        self.assertIsNone(record["relative_volume"])


# ---------------------------------------------------------------------------
# SI07: unclassified regime is represented
# ---------------------------------------------------------------------------

class TestUnclassifiedRegimeRepresented(unittest.TestCase):
    def test_si07_unclassified_regime_represented(self) -> None:
        spec = SymbolInputSpec(
            symbol="AAPL", timeframe="5m", bars=_unclassified_regime_bars(),
            liquidity_metrics=_PASSING_LIQUIDITY_METRICS,
        )
        record = build_symbol_input_record(spec, now_utc=_NOW)

        self.assertTrue(record["data_quality_passed"])
        self.assertEqual(record["regime_label"], LABEL_UNCLASSIFIED)
        self.assertEqual(record["rejection_reason"], REASON_REGIME_UNCLASSIFIED)
        self.assertIn("regime", " ".join(record["reason_tags"]))


# ---------------------------------------------------------------------------
# SI08: missing latest price fails closed
# ---------------------------------------------------------------------------

class TestMissingLatestPrice(unittest.TestCase):
    def test_si08_nan_close_yields_missing_price(self) -> None:
        # NaN is the one numeric value that survives data_quality's `<= 0` /
        # range comparisons untouched (NaN comparisons are always False), so
        # DQ can pass while the close is still honestly unusable — a
        # non-numeric string would be rejected by DQ's own OHLC validation
        # before reaching the producer, so it cannot exercise this path.
        #
        # NaN also propagates into the regime score (it is computed from the
        # same trailing closes), so regime honestly reports unclassified for
        # this fixture too. Because each gate is evaluated independently and
        # `rejection_reason` is "first gate to fail wins", regime's reason may
        # claim the primary slot — but the producer's own missing-price
        # finding must still be represented in `reason_tags` and `latest_close`
        # must still be honestly None. That is the invariant this test proves:
        # the missing-price condition never disappears silently, even when
        # another gate's honest failure is reported first.
        bars = copy.deepcopy(_trending_up_bars())
        bars[-1]["close"] = float("nan")
        spec = SymbolInputSpec(
            symbol="AAPL", timeframe="5m", bars=bars,
            liquidity_metrics=_PASSING_LIQUIDITY_METRICS,
        )
        record = build_symbol_input_record(spec, now_utc=_NOW)

        self.assertTrue(record["data_quality_passed"])
        self.assertIsNone(record["latest_close"])
        self.assertIn(REASON_MISSING_LATEST_PRICE, record["reason_tags"])
        self.assertIsNotNone(record["rejection_reason"])


# ---------------------------------------------------------------------------
# SI09: approved_for_live is always false
# ---------------------------------------------------------------------------

class TestApprovedForLiveAlwaysFalse(unittest.TestCase):
    def test_si09_artifact_level_always_false(self) -> None:
        spec = SymbolInputSpec(
            symbol="AAPL", timeframe="5m", bars=_trending_up_bars(),
            liquidity_metrics=_PASSING_LIQUIDITY_METRICS,
        )
        artifact = build_symbol_inputs([spec], trade_date="2026-06-08", source="test", now_utc=_NOW)
        self.assertFalse(artifact["approved_for_live"])
        self.assertNotIn("approved_for_live", artifact["symbols"]["AAPL"])

    def test_si09_write_forces_false_on_forged_artifact(self) -> None:
        spec = SymbolInputSpec(symbol="AAPL", timeframe="5m", bars=_trending_up_bars())
        artifact = build_symbol_inputs([spec], trade_date="2026-06-08", source="test", now_utc=_NOW)
        forged = dict(artifact)
        forged["approved_for_live"] = True

        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "symbol_inputs.json")
            write_symbol_inputs_artifact(forged, path)
            on_disk = json.loads(Path(path).read_text(encoding="utf-8"))

        self.assertFalse(on_disk["approved_for_live"])

    def test_si09_load_forces_false_on_forged_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "forged_symbol_inputs.json")
            Path(path).write_text(
                json.dumps({
                    "schema_version": SCHEMA_VERSION,
                    "generated_at_utc": "2026-06-08T08:00:00+00:00",
                    "trade_date": "2026-06-08",
                    "source": "forged",
                    "symbols": {},
                    "approved_for_live": True,
                    "notes": "forged on disk",
                }),
                encoding="utf-8",
            )
            loaded = load_symbol_inputs_artifact(path)

        self.assertFalse(loaded["approved_for_live"])


# ---------------------------------------------------------------------------
# SI10: artifact write/read round trip preserves schema
# ---------------------------------------------------------------------------

class TestWriteReadRoundTrip(unittest.TestCase):
    def test_si10_round_trip_preserves_schema(self) -> None:
        spec = SymbolInputSpec(
            symbol="AAPL", timeframe="5m", bars=_trending_up_bars(),
            liquidity_metrics=_PASSING_LIQUIDITY_METRICS,
        )
        artifact = build_symbol_inputs(
            [spec], trade_date="2026-06-08", source="test_fixture",
            generated_at_utc="2026-06-08T08:00:00+00:00", now_utc=_NOW,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "premarket", "20260608", "symbol_inputs.json")
            out = write_symbol_inputs_artifact(artifact, path)
            self.assertTrue(out.exists())
            loaded = load_symbol_inputs_artifact(str(out))

        self.assertEqual(loaded["schema_version"], SCHEMA_VERSION)
        self.assertEqual(loaded["trade_date"], "2026-06-08")
        self.assertEqual(loaded["source"], "test_fixture")
        self.assertEqual(loaded["generated_at_utc"], "2026-06-08T08:00:00+00:00")
        self.assertEqual(set(loaded["symbols"].keys()), {"AAPL"})
        self.assertEqual(
            loaded["symbols"]["AAPL"]["regime_label"],
            artifact["symbols"]["AAPL"]["regime_label"],
        )
        self.assertFalse(loaded["approved_for_live"])

    def test_si10_load_rejects_wrong_schema_version(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "bad_schema.json")
            Path(path).write_text(
                json.dumps({"schema_version": "symbol-inputs-v2", "symbols": {}}),
                encoding="utf-8",
            )
            with self.assertRaises(ValueError):
                load_symbol_inputs_artifact(path)

    def test_si10_load_rejects_missing_symbols(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "no_symbols.json")
            Path(path).write_text(
                json.dumps({"schema_version": SCHEMA_VERSION}),
                encoding="utf-8",
            )
            with self.assertRaises(ValueError):
                load_symbol_inputs_artifact(path)


# ---------------------------------------------------------------------------
# SI11: no broker/daemon/DB/network imports
# ---------------------------------------------------------------------------

class TestImportSafety(unittest.TestCase):
    def _get_source(self) -> str:
        import mqk_research.scanner.symbol_inputs as mod
        return Path(mod.__file__).read_text(encoding="utf-8")

    def test_si11_no_broker_oms_execution_references(self) -> None:
        src = self._get_source()
        forbidden = [
            "/v2/orders",
            "submit_order",
            "live_routing_enabled=true",
            "oms_outbox",
            "oms_inbox",
            "BrokerGateway",
            "broker_adapter",
        ]
        for token in forbidden:
            self.assertNotIn(token, src, f"forbidden token found: {token!r}")

    def test_si11_no_network_db_imports(self) -> None:
        src = self._get_source()
        forbidden = [
            "import requests",
            "import urllib",
            "import http.client",
            "import aiohttp",
            "import psycopg",
            "import sqlalchemy",
        ]
        for token in forbidden:
            self.assertNotIn(token, src, f"forbidden import found: {token!r}")

    def test_si11_no_subprocess(self) -> None:
        import mqk_research.scanner.symbol_inputs as mod
        self.assertFalse(hasattr(mod, "subprocess"))
        src = self._get_source()
        self.assertNotIn("import subprocess", src)
        self.assertNotIn("os.system", src)

    def test_si11_no_daemon_runtime_imports(self) -> None:
        src = self._get_source()
        forbidden = [
            "from mqk_daemon", "import mqk_daemon",
            "from mqk_runtime", "import mqk_runtime",
        ]
        for token in forbidden:
            self.assertNotIn(token, src, f"forbidden import found: {token!r}")


# ---------------------------------------------------------------------------
# SI12: extract_symbol_inputs_map adapter
# ---------------------------------------------------------------------------

class TestExtractSymbolInputsMap(unittest.TestCase):
    def test_si12_extracts_symbols_dict(self) -> None:
        spec = SymbolInputSpec(
            symbol="AAPL", timeframe="5m", bars=_trending_up_bars(),
            liquidity_metrics=_PASSING_LIQUIDITY_METRICS,
        )
        artifact = build_symbol_inputs([spec], trade_date="2026-06-08", source="test", now_utc=_NOW)
        sym_map = extract_symbol_inputs_map(artifact)
        self.assertIn("AAPL", sym_map)
        self.assertEqual(sym_map["AAPL"]["regime_label"], LABEL_TRENDING_UP)

    def test_si12_malformed_artifact_yields_empty_map(self) -> None:
        self.assertEqual(extract_symbol_inputs_map({}), {})
        self.assertEqual(extract_symbol_inputs_map({"schema_version": "wrong-v1"}), {})
        self.assertEqual(extract_symbol_inputs_map({"schema_version": SCHEMA_VERSION, "symbols": "not-a-dict"}), {})
        self.assertEqual(extract_symbol_inputs_map(None), {})  # type: ignore[arg-type]


# ---------------------------------------------------------------------------
# SI13: premarket integration — promoted watchlist + valid symbol_inputs passes
# ---------------------------------------------------------------------------

class TestPremarketIntegrationPassingPath(unittest.TestCase):
    def test_si13_valid_symbol_inputs_passes_premarket(self) -> None:
        spec = SymbolInputSpec(
            symbol="AAPL", timeframe="5m", bars=_trending_up_bars(),
            liquidity_metrics=_PASSING_LIQUIDITY_METRICS,
        )
        artifact = build_symbol_inputs([spec], trade_date="2026-06-08", source="test", now_utc=_NOW)
        sym_inputs = extract_symbol_inputs_map(artifact)

        wl = _make_watchlist(strategy_id="intraday_scalper")
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_NOW)

        self.assertTrue(result.passed, msg=f"unexpected failures: {result.failure_reasons}")
        self.assertEqual(result.failure_reasons, [])


# ---------------------------------------------------------------------------
# SI14: premarket integration — missing symbol input fails closed
# ---------------------------------------------------------------------------

class TestPremarketIntegrationMissingInput(unittest.TestCase):
    def test_si14_missing_symbol_input_fails_closed(self) -> None:
        spec = SymbolInputSpec(
            symbol="MSFT", timeframe="5m", bars=_trending_up_bars(),
            liquidity_metrics=_PASSING_LIQUIDITY_METRICS,
        )
        artifact = build_symbol_inputs([spec], trade_date="2026-06-08", source="test", now_utc=_NOW)
        sym_inputs = extract_symbol_inputs_map(artifact)  # has MSFT, not AAPL

        wl = _make_watchlist(symbol="AAPL")
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_NOW)

        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_SYMBOL_INPUT_MISSING, result.failure_reasons)

    def test_si14_malformed_artifact_fails_closed(self) -> None:
        sym_inputs = extract_symbol_inputs_map({"schema_version": "not-symbol-inputs"})
        self.assertEqual(sym_inputs, {})

        wl = _make_watchlist(symbol="AAPL")
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_NOW)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_SYMBOL_INPUT_MISSING, result.failure_reasons)


# ---------------------------------------------------------------------------
# SI15: premarket integration — stale symbol input fails closed
# ---------------------------------------------------------------------------

class TestPremarketIntegrationStaleInput(unittest.TestCase):
    def test_si15_symbol_input_stale_relative_to_later_reference_fails_closed(self) -> None:
        """Bar was fresh when the artifact was produced, but premarket evaluates
        later — bar_freshness must fail closed against the *evaluation* time."""
        spec = SymbolInputSpec(
            symbol="AAPL", timeframe="5m", bars=_trending_up_bars(),
            liquidity_metrics=_PASSING_LIQUIDITY_METRICS,
        )
        artifact = build_symbol_inputs([spec], trade_date="2026-06-08", source="test", now_utc=_NOW)
        sym_inputs = extract_symbol_inputs_map(artifact)
        # Sanity: was fresh and DQ-passing at production time.
        self.assertTrue(sym_inputs["AAPL"]["data_quality_passed"])

        wl = _make_watchlist()
        later_reference = _NOW + timedelta(minutes=45)
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=later_reference)

        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_BAR_STALE, result.failure_reasons)


# ---------------------------------------------------------------------------
# SI16: premarket integration — approved_for_live=True in symbol_inputs forced/rejected
# ---------------------------------------------------------------------------

class TestPremarketIntegrationLiveLock(unittest.TestCase):
    def test_si16_top_level_live_flag_never_reaches_evaluation(self) -> None:
        spec = SymbolInputSpec(
            symbol="AAPL", timeframe="5m", bars=_trending_up_bars(),
            liquidity_metrics=_PASSING_LIQUIDITY_METRICS,
        )
        artifact = build_symbol_inputs([spec], trade_date="2026-06-08", source="test", now_utc=_NOW)
        forged = dict(artifact)
        forged["approved_for_live"] = True

        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "forged.json")
            write_symbol_inputs_artifact(forged, path)
            loaded = load_symbol_inputs_artifact(path)

        self.assertFalse(loaded["approved_for_live"])
        sym_inputs = extract_symbol_inputs_map(loaded)
        self.assertNotIn("approved_for_live", sym_inputs["AAPL"])

        wl = _make_watchlist()
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_NOW)
        self.assertTrue(result.passed)
        self.assertNotIn("approved_for_live", result.to_dict())


# ---------------------------------------------------------------------------
# SI17: absent optional fields stay "not checked" (not fabricated as healthy)
# ---------------------------------------------------------------------------

class TestPremarketIntegrationAbsentOptionalFields(unittest.TestCase):
    def test_si17_honestly_none_optional_fields_are_not_checked_not_fabricated(self) -> None:
        """When liquidity metrics are absent, the producer leaves spread/slippage/
        rvol honestly None rather than fabricating passing values. Premarket must
        treat None as 'not checked' (no spread/slippage/rvol-specific failure),
        while still failing closed overall via premarket_liquidity_failed."""
        spec = SymbolInputSpec(symbol="AAPL", timeframe="5m", bars=_trending_up_bars())
        artifact = build_symbol_inputs([spec], trade_date="2026-06-08", source="test", now_utc=_NOW)
        record = artifact["symbols"]["AAPL"]
        self.assertIsNone(record["spread_estimate_bps"])
        self.assertIsNone(record["slippage_estimate_bps"])
        self.assertIsNone(record["relative_volume"])

        sym_inputs = extract_symbol_inputs_map(artifact)
        wl = _make_watchlist()
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_NOW)

        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_LIQUIDITY_FAILED, result.failure_reasons)
        self.assertNotIn(REASON_PREMARKET_SPREAD_TOO_WIDE, result.failure_reasons)
        self.assertNotIn(REASON_PREMARKET_SLIPPAGE_TOO_HIGH, result.failure_reasons)
        self.assertNotIn(REASON_PREMARKET_RVOL_TOO_LOW, result.failure_reasons)


# ---------------------------------------------------------------------------
# SI18: no live approval can be produced anywhere in the chain
# ---------------------------------------------------------------------------

class TestNoLiveApprovalAnywhere(unittest.TestCase):
    def test_si18_no_live_approval_through_full_chain(self) -> None:
        spec = SymbolInputSpec(
            symbol="AAPL", timeframe="5m", bars=_trending_up_bars(),
            liquidity_metrics=_PASSING_LIQUIDITY_METRICS,
        )
        artifact = build_symbol_inputs([spec], trade_date="2026-06-08", source="test", now_utc=_NOW)
        self.assertFalse(artifact["approved_for_live"])

        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "symbol_inputs.json")
            write_symbol_inputs_artifact(artifact, path)
            loaded = load_symbol_inputs_artifact(path)
        self.assertFalse(loaded["approved_for_live"])

        sym_inputs = extract_symbol_inputs_map(loaded)
        wl = _make_watchlist()
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_NOW)

        self.assertTrue(result.passed)
        result_dict = result.to_dict()
        self.assertNotIn("approved_for_live", result_dict)
        self.assertNotIn("eligible_for_live", result_dict)


if __name__ == "__main__":
    unittest.main()
