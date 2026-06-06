"""
WATCHLIST-PREMARKET-01 — Unit tests for premarket revalidation gate.
"""
from __future__ import annotations

import json
import os
import tempfile
import unittest
from datetime import datetime, timezone, timedelta
from pathlib import Path

from mqk_research.scanner.premarket_revalidation import (
    SCHEMA_VERSION,
    REASON_PREMARKET_WATCHLIST_SCHEMA_INVALID,
    REASON_PREMARKET_MODE_NOT_PAPER,
    REASON_PREMARKET_LIVE_APPROVAL_FORBIDDEN,
    REASON_PREMARKET_SYMBOL_INPUT_MISSING,
    REASON_PREMARKET_SYMBOL_SUPPRESSED,
    REASON_PREMARKET_DATA_QUALITY_FAILED,
    REASON_PREMARKET_LIQUIDITY_FAILED,
    REASON_PREMARKET_REGIME_INCOMPATIBLE,
    REASON_PREMARKET_BAR_STALE,
    REASON_PREMARKET_SPREAD_TOO_WIDE,
    REASON_PREMARKET_SLIPPAGE_TOO_HIGH,
    REASON_PREMARKET_RVOL_TOO_LOW,
    REASON_PREMARKET_PRICE_TOO_LOW,
    PremarketRevalidationConfig,
    PremarketRevalidationResult,
    evaluate_premarket_watchlist,
    apply_premarket_revalidation_to_watchlist,
    write_premarket_revalidation_artifact,
)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

_REFERENCE_UTC = datetime(2026, 6, 5, 9, 30, 0, tzinfo=timezone.utc)
_FRESH_BAR_TS = "2026-06-05T09:15:00+00:00"   # 15 min ago — within 20 min limit
_STALE_BAR_TS = "2026-06-05T09:00:00+00:00"   # 30 min ago — beyond 20 min limit


def _make_watchlist(
    symbol: str = "AAPL",
    strategy_id: str = "intraday_scalper",
    mode: str = "paper",
    approved_for_live: bool = False,
    schema_version: str = "watchlist-v1",
) -> dict:
    return {
        "schema_version": schema_version,
        "mode": mode,
        "approved_for_live": approved_for_live,
        "max_symbols_to_trade": 1,
        "max_concurrent_positions": 1,
        "symbols": [symbol],
        "strategy_assignments": {symbol: strategy_id},
        "notional_limits": {symbol: 1000.0},
        "ranked_candidates": [{"rank": 1, "symbol": symbol}],
    }


def _make_symbol_input(
    data_quality_passed: bool = True,
    liquidity_passed: bool = True,
    regime_label: str = "trending_up",
    latest_bar_ts: str = _FRESH_BAR_TS,
    suppressed: bool = False,
    spread_estimate_bps: float = 10.0,
    slippage_estimate_bps: float = 8.0,
    relative_volume: float = 1.2,
    latest_close: float = 185.0,
) -> dict:
    return {
        "data_quality_passed": data_quality_passed,
        "liquidity_passed": liquidity_passed,
        "regime_label": regime_label,
        "latest_bar_ts": latest_bar_ts,
        "suppressed": suppressed,
        "spread_estimate_bps": spread_estimate_bps,
        "slippage_estimate_bps": slippage_estimate_bps,
        "relative_volume": relative_volume,
        "latest_close": latest_close,
    }


# ---------------------------------------------------------------------------
# PM01: Valid symbol input passes
# ---------------------------------------------------------------------------

class TestPassingPath(unittest.TestCase):

    def test_pm01_all_checks_pass(self):
        """PM01: valid watchlist + symbol input with all fields in range passes."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input()}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertTrue(result.passed)
        self.assertEqual(result.failure_reasons, [])
        self.assertEqual(result.top_symbol, "AAPL")

    def test_pm01_symbol_results_populated(self):
        """PM01: symbol_results dict is populated for the top symbol."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input()}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertIn("AAPL", result.symbol_results)
        self.assertTrue(result.symbol_results["AAPL"]["passed"])

    def test_pm01_approved_for_live_false_in_apply(self):
        """PM01: apply_premarket_revalidation_to_watchlist forces approved_for_live=False."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input()}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        updated = apply_premarket_revalidation_to_watchlist(wl, result)
        self.assertFalse(updated["approved_for_live"])
        self.assertIn("premarket_revalidation", updated)

    def test_pm01_input_not_mutated(self):
        """PM01: evaluate does not mutate the input watchlist dict."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input()}
        orig_live = wl["approved_for_live"]
        evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertEqual(wl["approved_for_live"], orig_live)


# ---------------------------------------------------------------------------
# PM02: Missing symbol input fails
# ---------------------------------------------------------------------------

class TestMissingSymbolInput(unittest.TestCase):

    def test_pm02_missing_symbol_input_fails(self):
        """PM02: symbol_inputs dict has no entry for top symbol."""
        wl = _make_watchlist()
        sym_inputs: dict = {}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_SYMBOL_INPUT_MISSING, result.failure_reasons)

    def test_pm02_wrong_symbol_key_fails(self):
        """PM02: input keyed to wrong symbol is not found."""
        wl = _make_watchlist(symbol="AAPL")
        sym_inputs = {"MSFT": _make_symbol_input()}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_SYMBOL_INPUT_MISSING, result.failure_reasons)


# ---------------------------------------------------------------------------
# PM03: Suppressed symbol fails
# ---------------------------------------------------------------------------

class TestSuppressedSymbol(unittest.TestCase):

    def test_pm03_suppressed_true_fails(self):
        """PM03: suppressed=True in symbol_input fails."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input(suppressed=True)}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_SYMBOL_SUPPRESSED, result.failure_reasons)


# ---------------------------------------------------------------------------
# PM04: Data quality false fails
# ---------------------------------------------------------------------------

class TestDataQuality(unittest.TestCase):

    def test_pm04_data_quality_false_fails(self):
        """PM04: data_quality_passed=False fails."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input(data_quality_passed=False)}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_DATA_QUALITY_FAILED, result.failure_reasons)

    def test_pm04_data_quality_missing_fails(self):
        """PM04: absent data_quality_passed fails (not True)."""
        wl = _make_watchlist()
        sym_input = _make_symbol_input()
        sym_input.pop("data_quality_passed")
        sym_inputs = {"AAPL": sym_input}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_DATA_QUALITY_FAILED, result.failure_reasons)


# ---------------------------------------------------------------------------
# PM05: Liquidity false fails
# ---------------------------------------------------------------------------

class TestLiquidity(unittest.TestCase):

    def test_pm05_liquidity_false_fails(self):
        """PM05: liquidity_passed=False fails."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input(liquidity_passed=False)}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_LIQUIDITY_FAILED, result.failure_reasons)


# ---------------------------------------------------------------------------
# PM06: Regime incompatible fails
# ---------------------------------------------------------------------------

class TestRegimeCompatibility(unittest.TestCase):

    def test_pm06_regime_incompatible_fails(self):
        """PM06: intraday_scalper with range_bound regime fails regime check."""
        wl = _make_watchlist(strategy_id="intraday_scalper")
        sym_inputs = {"AAPL": _make_symbol_input(regime_label="range_bound")}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_REGIME_INCOMPATIBLE, result.failure_reasons)

    def test_pm06_unknown_strategy_fails(self):
        """PM06: unknown strategy_id fails regime check."""
        wl = _make_watchlist(strategy_id="unknown_strategy")
        sym_inputs = {"AAPL": _make_symbol_input(regime_label="trending_up")}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_REGIME_INCOMPATIBLE, result.failure_reasons)

    def test_pm06_missing_regime_label_fails(self):
        """PM06: missing regime_label in symbol_input fails."""
        wl = _make_watchlist()
        sym_input = _make_symbol_input()
        sym_input.pop("regime_label")
        sym_inputs = {"AAPL": sym_input}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_REGIME_INCOMPATIBLE, result.failure_reasons)


# ---------------------------------------------------------------------------
# PM07: Stale bar fails
# ---------------------------------------------------------------------------

class TestBarFreshness(unittest.TestCase):

    def test_pm07_stale_bar_fails(self):
        """PM07: latest_bar_ts beyond max_bar_age_minutes fails."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input(latest_bar_ts=_STALE_BAR_TS)}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_BAR_STALE, result.failure_reasons)

    def test_pm07_missing_bar_ts_fails(self):
        """PM07: absent latest_bar_ts fails bar freshness check."""
        wl = _make_watchlist()
        sym_input = _make_symbol_input()
        sym_input.pop("latest_bar_ts")
        sym_inputs = {"AAPL": sym_input}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_BAR_STALE, result.failure_reasons)

    def test_pm07_none_bar_ts_fails(self):
        """PM07: None latest_bar_ts fails."""
        wl = _make_watchlist()
        sym_input = _make_symbol_input()
        sym_input["latest_bar_ts"] = None
        sym_inputs = {"AAPL": sym_input}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_BAR_STALE, result.failure_reasons)

    def test_pm07_fresh_bar_passes(self):
        """PM07: fresh bar ts within max_bar_age_minutes passes."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input(latest_bar_ts=_FRESH_BAR_TS)}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertNotIn(REASON_PREMARKET_BAR_STALE, result.failure_reasons)


# ---------------------------------------------------------------------------
# PM08: Spread too wide fails
# ---------------------------------------------------------------------------

class TestSpread(unittest.TestCase):

    def test_pm08_spread_too_wide_fails(self):
        """PM08: spread_estimate_bps > max_spread_bps fails."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input(spread_estimate_bps=25.0)}  # > 20 default
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_SPREAD_TOO_WIDE, result.failure_reasons)

    def test_pm08_spread_at_limit_passes(self):
        """PM08: spread_estimate_bps == max_spread_bps passes."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input(spread_estimate_bps=20.0)}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertNotIn(REASON_PREMARKET_SPREAD_TOO_WIDE, result.failure_reasons)

    def test_pm08_absent_spread_not_failed(self):
        """PM08: absent spread_estimate_bps does not trigger spread check."""
        wl = _make_watchlist()
        sym_input = _make_symbol_input()
        sym_input.pop("spread_estimate_bps")
        sym_inputs = {"AAPL": sym_input}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertNotIn(REASON_PREMARKET_SPREAD_TOO_WIDE, result.failure_reasons)


# ---------------------------------------------------------------------------
# PM09: Slippage too high fails
# ---------------------------------------------------------------------------

class TestSlippage(unittest.TestCase):

    def test_pm09_slippage_too_high_fails(self):
        """PM09: slippage_estimate_bps > max_slippage_bps fails."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input(slippage_estimate_bps=20.0)}  # > 15 default
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_SLIPPAGE_TOO_HIGH, result.failure_reasons)

    def test_pm09_slippage_at_limit_passes(self):
        """PM09: slippage_estimate_bps == max_slippage_bps passes."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input(slippage_estimate_bps=15.0)}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertNotIn(REASON_PREMARKET_SLIPPAGE_TOO_HIGH, result.failure_reasons)


# ---------------------------------------------------------------------------
# PM10: RVOL too low fails
# ---------------------------------------------------------------------------

class TestRelativeVolume(unittest.TestCase):

    def test_pm10_rvol_too_low_fails(self):
        """PM10: relative_volume < min_relative_volume fails."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input(relative_volume=0.3)}  # < 0.5 default
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_RVOL_TOO_LOW, result.failure_reasons)

    def test_pm10_rvol_at_limit_passes(self):
        """PM10: relative_volume == min_relative_volume passes."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input(relative_volume=0.5)}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertNotIn(REASON_PREMARKET_RVOL_TOO_LOW, result.failure_reasons)


# ---------------------------------------------------------------------------
# PM11: Price too low fails
# ---------------------------------------------------------------------------

class TestPrice(unittest.TestCase):

    def test_pm11_price_too_low_fails(self):
        """PM11: latest_close < min_price fails."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input(latest_close=3.0)}  # < 5.0 default
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_PRICE_TOO_LOW, result.failure_reasons)

    def test_pm11_price_at_limit_passes(self):
        """PM11: latest_close == min_price passes."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input(latest_close=5.0)}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertNotIn(REASON_PREMARKET_PRICE_TOO_LOW, result.failure_reasons)


# ---------------------------------------------------------------------------
# PM12: approved_for_live=True fails and is forced False
# ---------------------------------------------------------------------------

class TestLiveLock(unittest.TestCase):

    def test_pm12_approved_for_live_true_fails(self):
        """PM12: watchlist with approved_for_live=True fails."""
        wl = _make_watchlist(approved_for_live=True)
        sym_inputs = {"AAPL": _make_symbol_input()}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_LIVE_APPROVAL_FORBIDDEN, result.failure_reasons)

    def test_pm12_apply_forces_approved_for_live_false(self):
        """PM12: apply_premarket_revalidation_to_watchlist forces approved_for_live=False."""
        wl = _make_watchlist(approved_for_live=True)
        sym_inputs = {"AAPL": _make_symbol_input()}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        updated = apply_premarket_revalidation_to_watchlist(wl, result)
        self.assertFalse(updated["approved_for_live"])

    def test_pm12_config_approved_for_live_not_overrideable(self):
        """PM12: PremarketRevalidationConfig.approved_for_live cannot be set True."""
        cfg = PremarketRevalidationConfig(approved_for_live=True)
        self.assertFalse(cfg.approved_for_live)

    def test_pm12_invalid_schema_fails(self):
        """PM12: wrong schema_version fails."""
        wl = _make_watchlist(schema_version="ranked-candidates-v1")
        sym_inputs = {"AAPL": _make_symbol_input()}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_WATCHLIST_SCHEMA_INVALID, result.failure_reasons)

    def test_pm12_non_paper_mode_fails(self):
        """PM12: non-paper mode fails."""
        wl = _make_watchlist(mode="live")
        sym_inputs = {"AAPL": _make_symbol_input()}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)
        self.assertIn(REASON_PREMARKET_MODE_NOT_PAPER, result.failure_reasons)


# ---------------------------------------------------------------------------
# PM13: JSON write/read
# ---------------------------------------------------------------------------

class TestJsonWriteRead(unittest.TestCase):

    def test_pm13_write_read_passing_result(self):
        """PM13: write_premarket_revalidation_artifact writes valid JSON that reads back."""
        wl = _make_watchlist()
        sym_inputs = {"AAPL": _make_symbol_input()}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertTrue(result.passed)

        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "premarket", "AAPL_premarket.json")
            out = write_premarket_revalidation_artifact(result, path)
            self.assertTrue(out.exists())
            loaded = json.loads(out.read_text(encoding="utf-8"))

        self.assertEqual(loaded["schema_version"], SCHEMA_VERSION)
        self.assertTrue(loaded["passed"])
        self.assertEqual(loaded["failure_reasons"], [])
        self.assertEqual(loaded["top_symbol"], "AAPL")
        self.assertIn("symbol_results", loaded)

    def test_pm13_write_read_failed_result(self):
        """PM13: failed result is written as valid JSON."""
        wl = _make_watchlist()
        sym_inputs: dict = {}
        result = evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        self.assertFalse(result.passed)

        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "premarket_failed.json")
            out = write_premarket_revalidation_artifact(result, path)
            loaded = json.loads(out.read_text(encoding="utf-8"))

        self.assertFalse(loaded["passed"])
        self.assertTrue(len(loaded["failure_reasons"]) > 0)

    def test_pm13_to_dict_schema_version(self):
        """PM13: to_dict always includes schema_version."""
        result = PremarketRevalidationResult(passed=False)
        d = result.to_dict()
        self.assertEqual(d["schema_version"], SCHEMA_VERSION)

    def test_pm13_write_creates_parent_dirs(self):
        """PM13: write_premarket_revalidation_artifact creates missing parent dirs."""
        result = PremarketRevalidationResult(passed=False)
        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "deeply", "nested", "premarket.json")
            out = write_premarket_revalidation_artifact(result, path)
            self.assertTrue(out.exists())


# ---------------------------------------------------------------------------
# PM14: No broker/OMS/network/DB imports
# ---------------------------------------------------------------------------

class TestImportSafety(unittest.TestCase):

    def _get_source(self) -> str:
        import mqk_research.scanner.premarket_revalidation as mod
        return Path(mod.__file__).read_text(encoding="utf-8")

    def test_pm14_no_broker_oms_execution_references(self):
        """PM14: no broker/OMS/execution references in module source."""
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

    def test_pm14_no_network_db_imports(self):
        """PM14: no network/DB library imports."""
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

    def test_pm14_no_subprocess(self):
        """PM14: no subprocess import or calls."""
        import mqk_research.scanner.premarket_revalidation as mod
        self.assertFalse(
            hasattr(mod, "subprocess"),
            "subprocess must not be imported at module level",
        )
        src = self._get_source()
        self.assertNotIn("import subprocess", src)
        self.assertNotIn("os.system", src)

    def test_pm14_no_daemon_runtime_imports(self):
        """PM14: no daemon or runtime imports."""
        src = self._get_source()
        forbidden = [
            "from mqk_daemon",
            "import mqk_daemon",
            "from mqk_runtime",
            "import mqk_runtime",
        ]
        for token in forbidden:
            self.assertNotIn(token, src, f"forbidden import found: {token!r}")

    def test_pm14_evaluate_does_not_call_subprocess(self):
        """PM14: evaluate_premarket_watchlist must not call subprocess.run."""
        import subprocess
        original_run = subprocess.run
        called = []

        def spy_run(*args, **kwargs):
            called.append(args)
            return original_run(*args, **kwargs)

        subprocess.run = spy_run
        try:
            wl = _make_watchlist()
            sym_inputs = {"AAPL": _make_symbol_input()}
            evaluate_premarket_watchlist(wl, sym_inputs, reference_utc=_REFERENCE_UTC)
        finally:
            subprocess.run = original_run

        self.assertEqual(called, [], "subprocess.run must not be called by premarket evaluator")


if __name__ == "__main__":
    unittest.main()
