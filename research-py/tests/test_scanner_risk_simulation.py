"""
RISK-SIM-01 — Unit tests for artifact-level watchlist risk simulation gate.
"""
from __future__ import annotations

import json
import os
import tempfile
import unittest
from pathlib import Path

from mqk_research.scanner.risk_simulation import (
    SCHEMA_VERSION,
    REASON_RISK_NO_CANDIDATES,
    REASON_RISK_STRATEGY_FIT_MISSING,
    REASON_RISK_NOTIONAL_LIMIT_FAILED,
    REASON_RISK_DAILY_LOSS_FAILED,
    REASON_RISK_DRAWDOWN_STRESS_FAILED,
    REASON_RISK_CONCENTRATION_FAILED,
    REASON_RISK_REGIME_MISMATCH,
    REASON_RISK_COST_ADJUSTED_EDGE_FAILED,
    REASON_RISK_LIVE_APPROVAL_FORBIDDEN,
    REASON_RISK_MISSING_METRIC,
    RiskSimulationConfig,
    RiskSimulationResult,
    evaluate_watchlist_risk,
    apply_risk_simulation_to_watchlist,
    write_risk_simulation_artifact,
)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

def _make_watchlist(
    symbol: str = "AAPL",
    notional: float = 1000.0,
    regime_label: str = "trending_up",
    strategy_id: str = "intraday_scalper",
    max_symbols_to_trade: int = 1,
    max_concurrent_positions: int = 1,
    approved_for_live: bool = False,
) -> dict:
    return {
        "schema_version": "watchlist-v1",
        "mode": "paper",
        "approved_for_live": approved_for_live,
        "max_symbols_to_trade": max_symbols_to_trade,
        "max_concurrent_positions": max_concurrent_positions,
        "symbols": [symbol],
        "strategy_assignments": {symbol: strategy_id},
        "notional_limits": {symbol: notional},
        "ranked_candidates": [
            {
                "rank": 1,
                "symbol": symbol,
                "strategy_id": strategy_id,
                "regime_label": regime_label,
                "regime_score": 0.75,
            }
        ],
    }


def _make_fit(
    symbol: str = "AAPL",
    strategy_id: str = "intraday_scalper",
    max_drawdown_bps: float = 150.0,
    net_expectancy_after_cost_bps: float = 10.0,
) -> dict:
    return {
        "schema_version": "strategy-fit-v1",
        "symbol": symbol,
        "strategy_id": strategy_id,
        "recommended_for_paper": True,
        "recommended_for_live": False,
        "max_drawdown_bps": max_drawdown_bps,
        "net_expectancy_after_cost_bps": net_expectancy_after_cost_bps,
        "failure_reasons": [],
    }


# ---------------------------------------------------------------------------
# RS01: Valid watchlist + strategy-fit passes
# ---------------------------------------------------------------------------

class TestPassingPath(unittest.TestCase):

    def test_rs01_all_checks_pass(self):
        """RS01: valid watchlist + strategy-fit with all metrics passes risk simulation."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertTrue(result.passed)
        self.assertEqual(result.failure_reasons, [])
        self.assertEqual(result.top_symbol, "AAPL")

    def test_rs01_all_checks_true(self):
        """RS01: all individual check flags are True on a full pass."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertTrue(result.check_live_lock)
        self.assertTrue(result.check_has_candidates)
        self.assertTrue(result.check_strategy_fit_present)
        self.assertTrue(result.check_concentration)
        self.assertTrue(result.check_max_position_notional)
        self.assertTrue(result.check_max_daily_loss)
        self.assertTrue(result.check_drawdown_stress)
        self.assertTrue(result.check_regime_consistency)
        self.assertTrue(result.check_cost_adjusted_edge)

    def test_rs01_approved_for_live_false_in_apply(self):
        """RS01: apply_risk_simulation_to_watchlist forces approved_for_live=False."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        updated = apply_risk_simulation_to_watchlist(wl, result)
        self.assertFalse(updated["approved_for_live"])
        self.assertIn("risk_simulation", updated)

    def test_rs01_input_not_mutated(self):
        """RS01: evaluate_watchlist_risk does not mutate input dicts."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_fit()}
        orig_live = wl["approved_for_live"]
        evaluate_watchlist_risk(wl, fits)
        self.assertEqual(wl["approved_for_live"], orig_live)


# ---------------------------------------------------------------------------
# RS02: Missing strategy-fit fails
# ---------------------------------------------------------------------------

class TestMissingStrategyFit(unittest.TestCase):

    def test_rs02_missing_fit_fails(self):
        """RS02: no strategy-fit artifact for top symbol fails risk."""
        wl = _make_watchlist()
        fits: dict = {}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_STRATEGY_FIT_MISSING, result.failure_reasons)

    def test_rs02_wrong_symbol_key_fails(self):
        """RS02: fit keyed to wrong symbol is not found."""
        wl = _make_watchlist(symbol="AAPL")
        fits = {"MSFT": _make_fit("MSFT")}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_STRATEGY_FIT_MISSING, result.failure_reasons)


# ---------------------------------------------------------------------------
# RS03: No candidates fails
# ---------------------------------------------------------------------------

class TestNoCandidates(unittest.TestCase):

    def test_rs03_empty_symbols_fails(self):
        """RS03: watchlist with empty symbols list fails."""
        wl = _make_watchlist()
        wl["symbols"] = []
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_NO_CANDIDATES, result.failure_reasons)

    def test_rs03_none_symbols_fails(self):
        """RS03: watchlist with symbols=None fails."""
        wl = _make_watchlist()
        wl["symbols"] = None
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_NO_CANDIDATES, result.failure_reasons)


# ---------------------------------------------------------------------------
# RS04: Notional too high fails
# ---------------------------------------------------------------------------

class TestNotionalLimit(unittest.TestCase):

    def test_rs04_notional_too_high_fails(self):
        """RS04: notional limit exceeds max_position_notional_usd fails."""
        wl = _make_watchlist(notional=6000.0)  # > 5000 default
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_NOTIONAL_LIMIT_FAILED, result.failure_reasons)

    def test_rs04_notional_at_limit_passes(self):
        """RS04: notional == max_position_notional_usd passes."""
        wl = _make_watchlist(notional=5000.0)
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertTrue(result.check_max_position_notional)
        self.assertNotIn(REASON_RISK_NOTIONAL_LIMIT_FAILED, result.failure_reasons)

    def test_rs04_missing_notional_fails(self):
        """RS04: absent notional_limits entry for symbol fails."""
        wl = _make_watchlist()
        wl["notional_limits"] = {}  # no entry for AAPL
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_NOTIONAL_LIMIT_FAILED, result.failure_reasons)


# ---------------------------------------------------------------------------
# RS05: Daily loss too high fails
# ---------------------------------------------------------------------------

class TestDailyLoss(unittest.TestCase):

    def test_rs05_daily_loss_too_high_fails(self):
        """RS05: max_drawdown_bps > max_daily_loss_bps fails."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_fit(max_drawdown_bps=250.0)}  # > 200 default
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_DAILY_LOSS_FAILED, result.failure_reasons)

    def test_rs05_daily_loss_at_limit_passes(self):
        """RS05: max_drawdown_bps == max_daily_loss_bps passes."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_fit(max_drawdown_bps=200.0)}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertTrue(result.check_max_daily_loss)
        self.assertNotIn(REASON_RISK_DAILY_LOSS_FAILED, result.failure_reasons)


# ---------------------------------------------------------------------------
# RS06: Drawdown stress too high fails
# ---------------------------------------------------------------------------

class TestDrawdownStress(unittest.TestCase):

    def test_rs06_drawdown_stress_too_high_fails(self):
        """RS06: max_drawdown_bps * slippage_stress_multiplier > max_stressed_drawdown_bps fails."""
        # 550 * 2 = 1100 > 1000 default
        wl = _make_watchlist()
        fits = {"AAPL": _make_fit(max_drawdown_bps=550.0)}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_DRAWDOWN_STRESS_FAILED, result.failure_reasons)

    def test_rs06_drawdown_stress_at_limit_passes(self):
        """RS06: stressed == max_stressed_drawdown_bps passes."""
        # 500 * 2 = 1000 == 1000 default
        wl = _make_watchlist()
        fits = {"AAPL": _make_fit(max_drawdown_bps=500.0)}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertTrue(result.check_drawdown_stress)
        self.assertNotIn(REASON_RISK_DRAWDOWN_STRESS_FAILED, result.failure_reasons)


# ---------------------------------------------------------------------------
# RS07: Concentration fails in v1 (multi-symbol)
# ---------------------------------------------------------------------------

class TestConcentration(unittest.TestCase):

    def test_rs07_multi_symbol_fails(self):
        """RS07: max_symbols_to_trade > 1 fails concentration check."""
        wl = _make_watchlist(max_symbols_to_trade=2)
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_CONCENTRATION_FAILED, result.failure_reasons)

    def test_rs07_multi_concurrent_fails(self):
        """RS07: max_concurrent_positions > 1 fails concentration check."""
        wl = _make_watchlist(max_concurrent_positions=3)
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_CONCENTRATION_FAILED, result.failure_reasons)


# ---------------------------------------------------------------------------
# RS08: Regime mismatch fails
# ---------------------------------------------------------------------------

class TestRegimeMismatch(unittest.TestCase):

    def test_rs08_regime_mismatch_fails(self):
        """RS08: intraday_scalper with range_bound regime fails regime check."""
        wl = _make_watchlist(regime_label="range_bound")  # incompatible with intraday_scalper
        fits = {"AAPL": _make_fit(strategy_id="intraday_scalper")}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_REGIME_MISMATCH, result.failure_reasons)

    def test_rs08_unknown_strategy_fails(self):
        """RS08: unknown strategy_id fails regime check (fail closed)."""
        wl = _make_watchlist(regime_label="trending_up", strategy_id="unknown_strategy")
        fits = {"AAPL": _make_fit(strategy_id="unknown_strategy")}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_REGIME_MISMATCH, result.failure_reasons)

    def test_rs08_missing_regime_label_fails(self):
        """RS08: missing regime_label in ranked_candidates fails regime check."""
        wl = _make_watchlist()
        wl["ranked_candidates"][0].pop("regime_label", None)
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_REGIME_MISMATCH, result.failure_reasons)


# ---------------------------------------------------------------------------
# RS09: Cost-adjusted edge too low fails
# ---------------------------------------------------------------------------

class TestCostAdjustedEdge(unittest.TestCase):

    def test_rs09_edge_too_low_fails(self):
        """RS09: net_expectancy_after_cost_bps < threshold fails."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_fit(net_expectancy_after_cost_bps=3.0)}  # < 5.0 default
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_COST_ADJUSTED_EDGE_FAILED, result.failure_reasons)

    def test_rs09_edge_at_limit_passes(self):
        """RS09: net_expectancy_after_cost_bps == threshold passes."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_fit(net_expectancy_after_cost_bps=5.0)}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertTrue(result.check_cost_adjusted_edge)
        self.assertNotIn(REASON_RISK_COST_ADJUSTED_EDGE_FAILED, result.failure_reasons)

    def test_rs09_negative_edge_fails(self):
        """RS09: negative net_expectancy_after_cost_bps fails."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_fit(net_expectancy_after_cost_bps=-1.0)}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_COST_ADJUSTED_EDGE_FAILED, result.failure_reasons)


# ---------------------------------------------------------------------------
# RS10: approved_for_live=True fails and is forced False
# ---------------------------------------------------------------------------

class TestLiveLock(unittest.TestCase):

    def test_rs10_approved_for_live_true_fails(self):
        """RS10: watchlist with approved_for_live=True fails and reason is added."""
        wl = _make_watchlist(approved_for_live=True)
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_LIVE_APPROVAL_FORBIDDEN, result.failure_reasons)

    def test_rs10_apply_forces_approved_for_live_false(self):
        """RS10: apply_risk_simulation_to_watchlist forces approved_for_live=False."""
        wl = _make_watchlist(approved_for_live=True)
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        updated = apply_risk_simulation_to_watchlist(wl, result)
        self.assertFalse(updated["approved_for_live"])

    def test_rs10_config_approved_for_live_not_overrideable(self):
        """RS10: RiskSimulationConfig.approved_for_live cannot be set True."""
        cfg = RiskSimulationConfig(approved_for_live=True)
        self.assertFalse(cfg.approved_for_live)


# ---------------------------------------------------------------------------
# RS11: Missing required fields fail closed
# ---------------------------------------------------------------------------

class TestMissingFields(unittest.TestCase):

    def test_rs11_missing_max_drawdown_bps_fails(self):
        """RS11: strategy-fit missing max_drawdown_bps fails."""
        wl = _make_watchlist()
        fit = _make_fit()
        fit.pop("max_drawdown_bps")
        fits = {"AAPL": fit}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_MISSING_METRIC, result.failure_reasons)

    def test_rs11_missing_net_expectancy_fails(self):
        """RS11: strategy-fit missing net_expectancy_after_cost_bps fails."""
        wl = _make_watchlist()
        fit = _make_fit()
        fit.pop("net_expectancy_after_cost_bps")
        fits = {"AAPL": fit}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_MISSING_METRIC, result.failure_reasons)

    def test_rs11_none_metric_values_fail(self):
        """RS11: None metric values fail closed."""
        wl = _make_watchlist()
        fit = _make_fit()
        fit["max_drawdown_bps"] = None
        fits = {"AAPL": fit}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)
        self.assertIn(REASON_RISK_MISSING_METRIC, result.failure_reasons)


# ---------------------------------------------------------------------------
# RS12: JSON write/read
# ---------------------------------------------------------------------------

class TestJsonWriteRead(unittest.TestCase):

    def test_rs12_write_read_passing_result(self):
        """RS12: write_risk_simulation_artifact writes a valid JSON file that reads back."""
        wl = _make_watchlist()
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertTrue(result.passed)

        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "risk", "AAPL_risk.json")
            out = write_risk_simulation_artifact(result, path)
            self.assertTrue(out.exists())
            loaded = json.loads(out.read_text(encoding="utf-8"))

        self.assertEqual(loaded["schema_version"], SCHEMA_VERSION)
        self.assertTrue(loaded["passed"])
        self.assertEqual(loaded["failure_reasons"], [])
        self.assertIn("checks", loaded)
        self.assertEqual(loaded["top_symbol"], "AAPL")

    def test_rs12_write_read_failed_result(self):
        """RS12: failed result is written as valid JSON."""
        wl = _make_watchlist()
        fits: dict = {}
        result = evaluate_watchlist_risk(wl, fits)
        self.assertFalse(result.passed)

        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "risk_failed.json")
            out = write_risk_simulation_artifact(result, path)
            loaded = json.loads(out.read_text(encoding="utf-8"))

        self.assertFalse(loaded["passed"])
        self.assertTrue(len(loaded["failure_reasons"]) > 0)

    def test_rs12_to_dict_schema_version(self):
        """RS12: to_dict always includes schema_version field."""
        result = RiskSimulationResult(passed=False)
        d = result.to_dict()
        self.assertEqual(d["schema_version"], SCHEMA_VERSION)

    def test_rs12_write_creates_parent_dirs(self):
        """RS12: write_risk_simulation_artifact creates missing parent dirs."""
        result = RiskSimulationResult(passed=False)
        with tempfile.TemporaryDirectory() as tmpdir:
            path = os.path.join(tmpdir, "deeply", "nested", "risk.json")
            out = write_risk_simulation_artifact(result, path)
            self.assertTrue(out.exists())


# ---------------------------------------------------------------------------
# RS13: No broker/OMS/network/DB imports
# ---------------------------------------------------------------------------

class TestImportSafety(unittest.TestCase):

    def _get_source(self) -> str:
        import mqk_research.scanner.risk_simulation as mod
        return Path(mod.__file__).read_text(encoding="utf-8")

    def test_rs13_no_broker_oms_execution_references(self):
        """RS13: no broker/OMS/execution references in module source."""
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

    def test_rs13_no_network_db_imports(self):
        """RS13: no network/DB library imports."""
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

    def test_rs13_no_subprocess(self):
        """RS13: no subprocess import or calls."""
        import mqk_research.scanner.risk_simulation as mod
        self.assertFalse(
            hasattr(mod, "subprocess"),
            "subprocess must not be imported at module level",
        )
        src = self._get_source()
        self.assertNotIn("import subprocess", src)
        self.assertNotIn("os.system", src)

    def test_rs13_no_daemon_runtime_imports(self):
        """RS13: no daemon or runtime imports."""
        src = self._get_source()
        forbidden = [
            "from mqk_daemon",
            "import mqk_daemon",
            "from mqk_runtime",
            "import mqk_runtime",
        ]
        for token in forbidden:
            self.assertNotIn(token, src, f"forbidden import found: {token!r}")

    def test_rs13_approved_for_live_always_false_in_output(self):
        """RS13: approved_for_live is forced False in apply output."""
        wl = _make_watchlist(approved_for_live=True)
        fits = {"AAPL": _make_fit()}
        result = evaluate_watchlist_risk(wl, fits)
        updated = apply_risk_simulation_to_watchlist(wl, result)
        self.assertFalse(updated["approved_for_live"])

    def test_rs13_evaluate_does_not_call_subprocess(self):
        """RS13: evaluate_watchlist_risk must not call subprocess.run."""
        import subprocess
        original_run = subprocess.run
        called = []

        def spy_run(*args, **kwargs):
            called.append(args)
            return original_run(*args, **kwargs)

        subprocess.run = spy_run
        try:
            wl = _make_watchlist()
            fits = {"AAPL": _make_fit()}
            evaluate_watchlist_risk(wl, fits)
            evaluate_watchlist_risk({"symbols": []}, {})
        finally:
            subprocess.run = original_run

        self.assertEqual(called, [], "subprocess.run must not be called by risk evaluator")


if __name__ == "__main__":
    unittest.main()
