"""
BACKTEST-GATES-01 — Unit tests for MAIN scanner backtest gate evaluator.
"""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from mqk_research.scanner.backtest_gates import (
    SCHEMA_VERSION,
    STATUS_BLOCKED,
    REASON_BLOCKED_NO_INTERFACE,
    REASON_MISSING_METRIC,
    REASON_MIN_BARS,
    REASON_MIN_TRADES,
    REASON_MAX_DRAWDOWN,
    REASON_PROFIT_FACTOR,
    REASON_EXPECTANCY,
    REASON_COST_ADJUSTED_EDGE,
    REASON_OUT_OF_SAMPLE,
    REASON_SAMPLE_QUALITY,
    REASON_PARAMETER_STABILITY,
    REASON_SINGLE_TRADE_DEPENDENCY,
    BacktestGateConfig,
    BacktestGateResult,
    evaluate_strategy_fit,
    apply_backtest_gates,
    write_evaluated_strategy_fit_artifact,
)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

def _blocked_artifact() -> dict:
    """Typical artifact produced by blocked backtest runner."""
    return {
        "schema_version": "strategy-fit-v1",
        "generated_at_utc": "2026-06-05T18:00:00+00:00",
        "symbol": "AAPL",
        "strategy_id": "intraday_scalper",
        "status": "blocked_no_backtest_interface",
        "bars_used": None,
        "trades": None,
        "win_rate": None,
        "profit_factor": None,
        "expectancy_bps": None,
        "avg_trade_bps": None,
        "max_drawdown_bps": None,
        "sharpe": None,
        "sortino": None,
        "net_expectancy_after_cost_bps": None,
        "sample_quality": None,
        "parameter_stability_score": None,
        "passed_min_bars": False,
        "passed_min_trades": False,
        "passed_max_drawdown": False,
        "passed_profit_factor": False,
        "passed_expectancy": False,
        "passed_cost_adjusted_edge": False,
        "passed_out_of_sample_check": False,
        "recommended_for_paper": False,
        "recommended_for_live": False,
        "failure_reasons": ["backtest_interface_missing"],
        "notes": "dry-run blocked",
    }


def _passing_artifact() -> dict:
    """Artifact with all metrics present and all gates passing."""
    return {
        "schema_version": "strategy-fit-v1",
        "generated_at_utc": "2026-06-05T18:00:00+00:00",
        "symbol": "AAPL",
        "strategy_id": "intraday_scalper",
        "status": "complete",
        "bars_used": 300,
        "trades": 50,
        "win_rate": 0.60,
        "profit_factor": 1.5,
        "expectancy_bps": 10.0,
        "avg_trade_bps": 8.0,
        "max_drawdown_bps": 500.0,
        "sharpe": 1.2,
        "sortino": 1.5,
        "net_expectancy_after_cost_bps": 7.0,
        "sample_quality": 0.85,
        "parameter_stability_score": 0.80,
        "largest_trade_profit_fraction": 0.15,
        "validation_profit_factor": 1.2,
        "validation_trades": 15,
        "passed_min_bars": False,
        "passed_min_trades": False,
        "passed_max_drawdown": False,
        "passed_profit_factor": False,
        "passed_expectancy": False,
        "passed_cost_adjusted_edge": False,
        "passed_out_of_sample_check": False,
        "recommended_for_paper": False,
        "recommended_for_live": False,
        "failure_reasons": [],
        "notes": "complete backtest",
    }


# ---------------------------------------------------------------------------
# Test 1: Blocked artifact fails closed
# ---------------------------------------------------------------------------

class TestBlockedArtifactFailsClosed(unittest.TestCase):

    def test_blocked_all_required_gates_false(self):
        result = evaluate_strategy_fit(_blocked_artifact())
        self.assertFalse(result.passed_min_bars)
        self.assertFalse(result.passed_min_trades)
        self.assertFalse(result.passed_max_drawdown)
        self.assertFalse(result.passed_profit_factor)
        self.assertFalse(result.passed_expectancy)
        self.assertFalse(result.passed_cost_adjusted_edge)
        self.assertFalse(result.passed_out_of_sample_check)

    def test_blocked_recommended_for_paper_false(self):
        result = evaluate_strategy_fit(_blocked_artifact())
        self.assertFalse(result.recommended_for_paper)

    def test_blocked_recommended_for_live_false(self):
        result = evaluate_strategy_fit(_blocked_artifact())
        self.assertFalse(result.recommended_for_live)

    def test_blocked_has_failure_reason(self):
        result = evaluate_strategy_fit(_blocked_artifact())
        self.assertIn(REASON_BLOCKED_NO_INTERFACE, result.failure_reasons)

    def test_blocked_preserves_existing_failure_reasons(self):
        art = _blocked_artifact()
        art["failure_reasons"] = ["backtest_interface_missing", "some_prior_reason"]
        result = evaluate_strategy_fit(art)
        self.assertIn("some_prior_reason", result.failure_reasons)
        self.assertIn(REASON_BLOCKED_NO_INTERFACE, result.failure_reasons)


# ---------------------------------------------------------------------------
# Test 2: Missing required metrics fails closed
# ---------------------------------------------------------------------------

class TestMissingMetricsFailsClosed(unittest.TestCase):

    def _art_without(self, field_name: str) -> dict:
        art = _passing_artifact()
        art[field_name] = None
        art["status"] = "complete"
        return art

    def test_missing_bars_used(self):
        result = evaluate_strategy_fit(self._art_without("bars_used"))
        self.assertFalse(result.recommended_for_paper)
        self.assertIn(REASON_MISSING_METRIC, result.failure_reasons)

    def test_missing_trades(self):
        result = evaluate_strategy_fit(self._art_without("trades"))
        self.assertFalse(result.recommended_for_paper)
        self.assertIn(REASON_MISSING_METRIC, result.failure_reasons)

    def test_missing_profit_factor(self):
        result = evaluate_strategy_fit(self._art_without("profit_factor"))
        self.assertFalse(result.recommended_for_paper)
        self.assertIn(REASON_MISSING_METRIC, result.failure_reasons)

    def test_missing_expectancy_bps(self):
        result = evaluate_strategy_fit(self._art_without("expectancy_bps"))
        self.assertFalse(result.recommended_for_paper)
        self.assertIn(REASON_MISSING_METRIC, result.failure_reasons)

    def test_missing_max_drawdown_bps(self):
        result = evaluate_strategy_fit(self._art_without("max_drawdown_bps"))
        self.assertFalse(result.recommended_for_paper)
        self.assertIn(REASON_MISSING_METRIC, result.failure_reasons)

    def test_missing_net_expectancy(self):
        result = evaluate_strategy_fit(self._art_without("net_expectancy_after_cost_bps"))
        self.assertFalse(result.recommended_for_paper)
        self.assertIn(REASON_MISSING_METRIC, result.failure_reasons)


# ---------------------------------------------------------------------------
# Test 3: Passing metrics set all required passed_* fields true
# ---------------------------------------------------------------------------

class TestPassingMetricsSetGatesTrue(unittest.TestCase):

    def test_all_required_gates_true_on_passing_artifact(self):
        result = evaluate_strategy_fit(_passing_artifact())
        self.assertTrue(result.passed_min_bars)
        self.assertTrue(result.passed_min_trades)
        self.assertTrue(result.passed_max_drawdown)
        self.assertTrue(result.passed_profit_factor)
        self.assertTrue(result.passed_expectancy)
        self.assertTrue(result.passed_cost_adjusted_edge)
        self.assertTrue(result.passed_out_of_sample_check)


# ---------------------------------------------------------------------------
# Test 4: Passing metrics set recommended_for_paper=True
# ---------------------------------------------------------------------------

class TestPassingMetricsSetsRecommendedForPaper(unittest.TestCase):

    def test_recommended_for_paper_true_when_all_gates_pass(self):
        result = evaluate_strategy_fit(_passing_artifact())
        self.assertTrue(result.recommended_for_paper)

    def test_no_failure_reasons_on_fully_passing(self):
        result = evaluate_strategy_fit(_passing_artifact())
        self.assertEqual(result.failure_reasons, [])


# ---------------------------------------------------------------------------
# Test 5: recommended_for_live=False always
# ---------------------------------------------------------------------------

class TestRecommendedForLiveAlwaysFalse(unittest.TestCase):

    def test_live_false_on_passing_artifact(self):
        result = evaluate_strategy_fit(_passing_artifact())
        self.assertFalse(result.recommended_for_live)

    def test_live_false_on_blocked_artifact(self):
        result = evaluate_strategy_fit(_blocked_artifact())
        self.assertFalse(result.recommended_for_live)

    def test_apply_gates_forces_live_false_even_if_input_has_true(self):
        art = _passing_artifact()
        art["recommended_for_live"] = True  # input has True
        updated = apply_backtest_gates(art)
        self.assertFalse(updated["recommended_for_live"])

    def test_config_recommended_for_live_cannot_be_set_true(self):
        cfg = BacktestGateConfig(recommended_for_live=True)
        self.assertFalse(cfg.recommended_for_live)

    def test_gate_result_recommended_for_live_false(self):
        result = BacktestGateResult(recommended_for_live=True)
        # BacktestGateResult does not enforce on its own; apply_backtest_gates enforces.
        # Verify apply_backtest_gates always writes False.
        art = _passing_artifact()
        updated = apply_backtest_gates(art)
        self.assertFalse(updated["recommended_for_live"])


# ---------------------------------------------------------------------------
# Test 6-12: Individual gate failures
# ---------------------------------------------------------------------------

class TestGateFailures(unittest.TestCase):

    def _fail_artifact(self, **overrides) -> dict:
        art = _passing_artifact()
        art.update(overrides)
        return art

    def test_min_bars_failure(self):
        """Test 6: min_bars_failed."""
        result = evaluate_strategy_fit(self._fail_artifact(bars_used=100))
        self.assertFalse(result.passed_min_bars)
        self.assertIn(REASON_MIN_BARS, result.failure_reasons)
        self.assertFalse(result.recommended_for_paper)

    def test_min_trades_failure(self):
        """Test 7: min_trades_failed."""
        result = evaluate_strategy_fit(self._fail_artifact(trades=10))
        self.assertFalse(result.passed_min_trades)
        self.assertIn(REASON_MIN_TRADES, result.failure_reasons)
        self.assertFalse(result.recommended_for_paper)

    def test_max_drawdown_failure(self):
        """Test 8: max_drawdown_failed."""
        result = evaluate_strategy_fit(self._fail_artifact(max_drawdown_bps=900.0))
        self.assertFalse(result.passed_max_drawdown)
        self.assertIn(REASON_MAX_DRAWDOWN, result.failure_reasons)
        self.assertFalse(result.recommended_for_paper)

    def test_profit_factor_failure(self):
        """Test 9: profit_factor_failed."""
        result = evaluate_strategy_fit(self._fail_artifact(profit_factor=1.0))
        self.assertFalse(result.passed_profit_factor)
        self.assertIn(REASON_PROFIT_FACTOR, result.failure_reasons)
        self.assertFalse(result.recommended_for_paper)

    def test_expectancy_failure_zero(self):
        """Test 10: expectancy_failed — expectancy must be strictly > 0."""
        result = evaluate_strategy_fit(self._fail_artifact(expectancy_bps=0.0))
        self.assertFalse(result.passed_expectancy)
        self.assertIn(REASON_EXPECTANCY, result.failure_reasons)
        self.assertFalse(result.recommended_for_paper)

    def test_expectancy_failure_negative(self):
        result = evaluate_strategy_fit(self._fail_artifact(expectancy_bps=-5.0))
        self.assertFalse(result.passed_expectancy)
        self.assertIn(REASON_EXPECTANCY, result.failure_reasons)

    def test_cost_adjusted_edge_failure(self):
        """Test 11: cost_adjusted_edge_failed."""
        result = evaluate_strategy_fit(self._fail_artifact(net_expectancy_after_cost_bps=2.0))
        self.assertFalse(result.passed_cost_adjusted_edge)
        self.assertIn(REASON_COST_ADJUSTED_EDGE, result.failure_reasons)
        self.assertFalse(result.recommended_for_paper)

    def test_out_of_sample_failure_low_pf(self):
        """Test 12: out_of_sample_failed — validation profit_factor below threshold."""
        result = evaluate_strategy_fit(
            self._fail_artifact(validation_profit_factor=1.0, validation_trades=20)
        )
        self.assertFalse(result.passed_out_of_sample_check)
        self.assertIn(REASON_OUT_OF_SAMPLE, result.failure_reasons)
        self.assertFalse(result.recommended_for_paper)

    def test_out_of_sample_failure_low_trades(self):
        """Test 12: out_of_sample_failed — validation trades below threshold."""
        result = evaluate_strategy_fit(
            self._fail_artifact(validation_profit_factor=1.3, validation_trades=5)
        )
        self.assertFalse(result.passed_out_of_sample_check)
        self.assertIn(REASON_OUT_OF_SAMPLE, result.failure_reasons)

    def test_out_of_sample_failure_missing_validation_fields(self):
        """Test 12: out_of_sample_failed — absent validation metrics → fail closed."""
        art = _passing_artifact()
        del art["validation_profit_factor"]
        del art["validation_trades"]
        result = evaluate_strategy_fit(art)
        self.assertFalse(result.passed_out_of_sample_check)
        self.assertIn(REASON_OUT_OF_SAMPLE, result.failure_reasons)
        self.assertFalse(result.recommended_for_paper)


# ---------------------------------------------------------------------------
# Test 13-15: Additional gate failures
# ---------------------------------------------------------------------------

class TestAdditionalGateFailures(unittest.TestCase):

    def _fail_artifact(self, **overrides) -> dict:
        art = _passing_artifact()
        art.update(overrides)
        return art

    def test_sample_quality_failure_below_min(self):
        """Test 13: sample_quality_failed."""
        result = evaluate_strategy_fit(self._fail_artifact(sample_quality=0.3))
        self.assertIn(REASON_SAMPLE_QUALITY, result.failure_reasons)
        self.assertFalse(result.recommended_for_paper)

    def test_sample_quality_failure_none(self):
        """Test 13: sample_quality_failed when null."""
        result = evaluate_strategy_fit(self._fail_artifact(sample_quality=None))
        self.assertIn(REASON_SAMPLE_QUALITY, result.failure_reasons)
        self.assertFalse(result.recommended_for_paper)

    def test_parameter_stability_failure_below_min(self):
        """Test 14: parameter_stability_failed."""
        result = evaluate_strategy_fit(self._fail_artifact(parameter_stability_score=0.5))
        self.assertIn(REASON_PARAMETER_STABILITY, result.failure_reasons)
        self.assertFalse(result.recommended_for_paper)

    def test_parameter_stability_failure_none(self):
        """Test 14: parameter_stability_failed when null."""
        result = evaluate_strategy_fit(self._fail_artifact(parameter_stability_score=None))
        self.assertIn(REASON_PARAMETER_STABILITY, result.failure_reasons)
        self.assertFalse(result.recommended_for_paper)

    def test_single_trade_dependency_failure_too_high(self):
        """Test 15: single_trade_dependency_failed — fraction above threshold."""
        result = evaluate_strategy_fit(self._fail_artifact(largest_trade_profit_fraction=0.40))
        self.assertIn(REASON_SINGLE_TRADE_DEPENDENCY, result.failure_reasons)
        self.assertFalse(result.recommended_for_paper)

    def test_single_trade_dependency_failure_none(self):
        """Test 15: single_trade_dependency_failed — absent field → fail closed."""
        art = _passing_artifact()
        del art["largest_trade_profit_fraction"]
        result = evaluate_strategy_fit(art)
        self.assertIn(REASON_SINGLE_TRADE_DEPENDENCY, result.failure_reasons)
        self.assertFalse(result.recommended_for_paper)

    def test_sample_quality_at_boundary_passes(self):
        """Exact boundary 0.5 passes (>= threshold)."""
        result = evaluate_strategy_fit(self._fail_artifact(sample_quality=0.5))
        self.assertNotIn(REASON_SAMPLE_QUALITY, result.failure_reasons)

    def test_parameter_stability_at_boundary_passes(self):
        """Exact boundary 0.7 passes (>= threshold)."""
        result = evaluate_strategy_fit(self._fail_artifact(parameter_stability_score=0.7))
        self.assertNotIn(REASON_PARAMETER_STABILITY, result.failure_reasons)

    def test_single_trade_fraction_at_boundary_passes(self):
        """Exactly 0.30 passes (<= threshold, spec says > 30% fails)."""
        result = evaluate_strategy_fit(self._fail_artifact(largest_trade_profit_fraction=0.30))
        self.assertNotIn(REASON_SINGLE_TRADE_DEPENDENCY, result.failure_reasons)


# ---------------------------------------------------------------------------
# Test 16: Existing failure reasons are preserved/deduplicated
# ---------------------------------------------------------------------------

class TestFailureReasonsMerge(unittest.TestCase):

    def test_existing_reasons_preserved_on_blocked(self):
        art = _blocked_artifact()
        art["failure_reasons"] = ["backtest_interface_missing", "prior_custom_reason"]
        result = evaluate_strategy_fit(art)
        self.assertIn("prior_custom_reason", result.failure_reasons)
        self.assertIn(REASON_BLOCKED_NO_INTERFACE, result.failure_reasons)

    def test_no_duplicate_blocked_reason(self):
        art = _blocked_artifact()
        art["failure_reasons"] = ["backtest_interface_missing", REASON_BLOCKED_NO_INTERFACE]
        result = evaluate_strategy_fit(art)
        count = result.failure_reasons.count(REASON_BLOCKED_NO_INTERFACE)
        self.assertEqual(count, 1, "REASON_BLOCKED_NO_INTERFACE should appear exactly once")

    def test_existing_reasons_preserved_on_metric_failure(self):
        art = _passing_artifact()
        art["failure_reasons"] = ["some_existing_reason"]
        art["bars_used"] = 50  # will fail min_bars
        result = evaluate_strategy_fit(art)
        self.assertIn("some_existing_reason", result.failure_reasons)
        self.assertIn(REASON_MIN_BARS, result.failure_reasons)

    def test_no_duplicate_min_bars_reason(self):
        art = _passing_artifact()
        art["failure_reasons"] = [REASON_MIN_BARS]
        art["bars_used"] = 50
        result = evaluate_strategy_fit(art)
        count = result.failure_reasons.count(REASON_MIN_BARS)
        self.assertEqual(count, 1)


# ---------------------------------------------------------------------------
# Test 17: Gate evaluation is deterministic
# ---------------------------------------------------------------------------

class TestDeterminism(unittest.TestCase):

    def test_same_input_same_output(self):
        art = _passing_artifact()
        r1 = evaluate_strategy_fit(art)
        r2 = evaluate_strategy_fit(art)
        self.assertEqual(r1.passed_min_bars, r2.passed_min_bars)
        self.assertEqual(r1.passed_min_trades, r2.passed_min_trades)
        self.assertEqual(r1.passed_max_drawdown, r2.passed_max_drawdown)
        self.assertEqual(r1.passed_profit_factor, r2.passed_profit_factor)
        self.assertEqual(r1.passed_expectancy, r2.passed_expectancy)
        self.assertEqual(r1.passed_cost_adjusted_edge, r2.passed_cost_adjusted_edge)
        self.assertEqual(r1.passed_out_of_sample_check, r2.passed_out_of_sample_check)
        self.assertEqual(r1.recommended_for_paper, r2.recommended_for_paper)
        self.assertEqual(r1.failure_reasons, r2.failure_reasons)

    def test_apply_gates_deterministic(self):
        art = _passing_artifact()
        u1 = apply_backtest_gates(art)
        u2 = apply_backtest_gates(art)
        self.assertEqual(json.dumps(u1, sort_keys=True), json.dumps(u2, sort_keys=True))

    def test_blocked_deterministic(self):
        art = _blocked_artifact()
        r1 = evaluate_strategy_fit(art)
        r2 = evaluate_strategy_fit(art)
        self.assertEqual(r1.failure_reasons, r2.failure_reasons)
        self.assertEqual(r1.recommended_for_paper, r2.recommended_for_paper)


# ---------------------------------------------------------------------------
# Test 18: JSON write/read works
# ---------------------------------------------------------------------------

class TestJsonWriteRead(unittest.TestCase):

    def test_write_and_read_passing_artifact(self):
        art = _passing_artifact()
        updated = apply_backtest_gates(art)
        with tempfile.TemporaryDirectory() as tmpdir:
            path = str(Path(tmpdir) / "fit_evaluated.json")
            out = write_evaluated_strategy_fit_artifact(updated, path)
            self.assertTrue(out.exists())
            loaded = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(loaded["schema_version"], "strategy-fit-v1")
        self.assertFalse(loaded["recommended_for_live"])
        self.assertTrue(loaded["recommended_for_paper"])

    def test_write_and_read_blocked_artifact(self):
        art = _blocked_artifact()
        updated = apply_backtest_gates(art)
        with tempfile.TemporaryDirectory() as tmpdir:
            path = str(Path(tmpdir) / "fit_blocked.json")
            out = write_evaluated_strategy_fit_artifact(updated, path)
            self.assertTrue(out.exists())
            loaded = json.loads(out.read_text(encoding="utf-8"))
        self.assertFalse(loaded["recommended_for_paper"])
        self.assertFalse(loaded["recommended_for_live"])
        self.assertIn(REASON_BLOCKED_NO_INTERFACE, loaded["failure_reasons"])

    def test_write_creates_parent_dirs(self):
        art = apply_backtest_gates(_blocked_artifact())
        with tempfile.TemporaryDirectory() as tmpdir:
            path = str(Path(tmpdir) / "nested" / "deep" / "fit.json")
            out = write_evaluated_strategy_fit_artifact(art, path)
            self.assertTrue(out.exists())


# ---------------------------------------------------------------------------
# Test 19: No broker/OMS/execution/network/DB imports
# ---------------------------------------------------------------------------

class TestNoForbiddenImports(unittest.TestCase):

    def test_no_forbidden_imports_in_module_source(self):
        import mqk_research.scanner.backtest_gates as mod
        source = Path(mod.__file__).read_text(encoding="utf-8")
        forbidden_imports = [
            "import requests",
            "import urllib",
            "import http.client",
            "import aiohttp",
            "import psycopg",
            "import sqlalchemy",
        ]
        forbidden_refs = [
            "submit_order",
            "broker_adapter",
            "BrokerGateway",
            "oms_outbox",
            "oms_inbox",
            "/v2/orders",
        ]
        for token in forbidden_imports + forbidden_refs:
            self.assertNotIn(token, source, msg=f"forbidden token found: {token!r}")

    def test_no_subprocess_in_module(self):
        import mqk_research.scanner.backtest_gates as mod
        self.assertFalse(
            hasattr(mod, "subprocess"),
            "subprocess must not be imported at module level",
        )


# ---------------------------------------------------------------------------
# Test 20: Does not call mqk-backtest or subprocess
# ---------------------------------------------------------------------------

class TestNoExternalExecution(unittest.TestCase):

    def test_evaluate_does_not_call_subprocess(self):
        import subprocess
        original_run = subprocess.run
        called = []

        def spy_run(*args, **kwargs):
            called.append(args)
            return original_run(*args, **kwargs)

        subprocess.run = spy_run
        try:
            evaluate_strategy_fit(_passing_artifact())
            evaluate_strategy_fit(_blocked_artifact())
        finally:
            subprocess.run = original_run

        self.assertEqual(called, [], "subprocess.run must not be called by gate evaluator")

    def test_apply_gates_does_not_call_subprocess(self):
        import subprocess
        original_run = subprocess.run
        called = []

        def spy_run(*args, **kwargs):
            called.append(args)
            return original_run(*args, **kwargs)

        subprocess.run = spy_run
        try:
            apply_backtest_gates(_passing_artifact())
        finally:
            subprocess.run = original_run

        self.assertEqual(called, [])


# ---------------------------------------------------------------------------
# Test 21: Evaluating blocked runner artifact keeps recommended_for_paper=False
# ---------------------------------------------------------------------------

class TestBlockedRunnerArtifactRemainsUnpromoted(unittest.TestCase):

    def test_blocked_runner_artifact_via_apply_gates(self):
        """Simulate apply_backtest_gates on a real blocked runner artifact."""
        from mqk_research.scanner.backtest_runner import (
            build_strategy_fit_artifact,
            StrategyFitResult,
            BacktestRunnerConfig,
        )
        entry = {
            "queue_id": "bq-test-blocked-001",
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
        updated = apply_backtest_gates(runner_artifact)
        self.assertFalse(updated["recommended_for_paper"])
        self.assertFalse(updated["recommended_for_live"])
        self.assertIn(REASON_BLOCKED_NO_INTERFACE, updated["failure_reasons"])

    def test_blocked_runner_artifact_all_gates_false(self):
        from mqk_research.scanner.backtest_runner import (
            build_strategy_fit_artifact,
            StrategyFitResult,
            BacktestRunnerConfig,
        )
        entry = {
            "queue_id": "bq-test-blocked-002",
            "symbol": "MSFT",
            "strategy_id": "mean_reversion",
            "timeframe": "5m",
            "regime_label": "range_bound",
            "source_rank": 2,
        }
        runner_artifact = build_strategy_fit_artifact(
            entry=entry,
            result=StrategyFitResult(),
            config=BacktestRunnerConfig(),
        )
        updated = apply_backtest_gates(runner_artifact)
        for gate in (
            "passed_min_bars", "passed_min_trades", "passed_max_drawdown",
            "passed_profit_factor", "passed_expectancy",
            "passed_cost_adjusted_edge", "passed_out_of_sample_check",
        ):
            self.assertFalse(updated[gate], msg=f"{gate} should be False for blocked artifact")


# ---------------------------------------------------------------------------
# Additional: apply_backtest_gates does not mutate input
# ---------------------------------------------------------------------------

class TestApplyGatesImmutability(unittest.TestCase):

    def test_input_not_mutated(self):
        art = _passing_artifact()
        original_recommended = art["recommended_for_paper"]
        original_failure_reasons = list(art["failure_reasons"])
        _ = apply_backtest_gates(art)
        self.assertEqual(art["recommended_for_paper"], original_recommended)
        self.assertEqual(art["failure_reasons"], original_failure_reasons)


# ---------------------------------------------------------------------------
# Additional: custom config thresholds respected
# ---------------------------------------------------------------------------

class TestCustomConfig(unittest.TestCase):

    def test_custom_min_bars_threshold(self):
        cfg = BacktestGateConfig(min_bars=400)
        art = _passing_artifact()  # bars_used=300
        result = evaluate_strategy_fit(art, cfg)
        self.assertFalse(result.passed_min_bars)
        self.assertIn(REASON_MIN_BARS, result.failure_reasons)

    def test_custom_min_trades_threshold(self):
        cfg = BacktestGateConfig(min_trades=100)
        art = _passing_artifact()  # trades=50
        result = evaluate_strategy_fit(art, cfg)
        self.assertFalse(result.passed_min_trades)
        self.assertIn(REASON_MIN_TRADES, result.failure_reasons)

    def test_config_live_invariant_from_init(self):
        cfg = BacktestGateConfig(recommended_for_live=True)
        self.assertFalse(cfg.recommended_for_live)


# ---------------------------------------------------------------------------
# Additional: schema version constant
# ---------------------------------------------------------------------------

class TestSchemaVersion(unittest.TestCase):

    def test_schema_version_constant(self):
        self.assertEqual(SCHEMA_VERSION, "strategy-fit-v1")

    def test_status_blocked_constant(self):
        self.assertEqual(STATUS_BLOCKED, "blocked_no_backtest_interface")


if __name__ == "__main__":
    unittest.main()
