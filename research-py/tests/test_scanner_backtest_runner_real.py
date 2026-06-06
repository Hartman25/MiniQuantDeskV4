"""
BACKTEST-BRIDGE-BUNDLE-01 — Tests for Python bridge to Rust mqk-backtest output.

Tests the backtest_bridge module (parser, mapper, command builder) and the
real-mode path in backtest_runner, without executing any subprocess or
touching broker/OMS/daemon/production DB.
"""
from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any
from unittest.mock import MagicMock, patch

from mqk_research.scanner.backtest_bridge import (
    REASON_METRICS_FILE_MISSING,
    REASON_METRICS_SCHEMA_INVALID,
    REASON_BACKTEST_COMMAND_FAILED,
    REASON_EXPECTANCY_BASIS_MISSING,
    REASON_VALIDATION_METRICS_MISSING,
    REASON_COMMAND_BUILD_FAILED,
    REASON_BINARY_NOT_FOUND,
    PCT_SCALE_PERCENT_0_100,
    PCT_SCALE_AUTO,
    BacktestBridgeConfig,
    build_mqk_backtest_command,
    parse_mqk_metrics_json,
    map_metrics_to_strategy_fit,
    run_backtest_for_entry,
    _normalize_pct_to_fraction,
    _normalize_drawdown_to_bps,
)
from mqk_research.scanner.backtest_runner import (
    STATUS_BLOCKED,
    STATUS_COMPLETE,
    FAILURE_REASON_NO_INTERFACE,
    BacktestRunnerConfig,
    StrategyFitResult,
    run_backtest_queue,
)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

def _make_queue_entry(
    queue_id: str = "bq-abc123",
    symbol: str = "AAPL",
    strategy_id: str = "intraday_scalper",
    timeframe: str = "5m",
) -> dict[str, Any]:
    return {
        "queue_id": queue_id,
        "symbol": symbol,
        "strategy_id": strategy_id,
        "timeframe": timeframe,
        "regime_label": "trending_up",
        "source_rank": 1,
        "source_total_score": 0.75,
        "source_candidate_artifact": None,
        "source_ranked_export": None,
        "ambiguity_policy": "CONSERVATIVE_WORST_CASE",
        "stress_profile": "slippage_x2",
        "recommended_for_live": False,
        "notes": "test entry",
    }


def _make_queue_artifact(entries: list[dict] | None = None) -> dict[str, Any]:
    if entries is None:
        entries = [_make_queue_entry()]
    return {
        "schema_version": "backtest-queue-v1",
        "generated_at_utc": "2026-06-05T16:30:00+00:00",
        "source_ranked_export": None,
        "queue_count": len(entries),
        "entries": entries,
        "recommended_for_live": False,
        "notes": "test queue",
    }


def _make_metrics_dict(
    win_rate_pct: float = 60.0,
    profit_factor: float = 1.5,
    expectancy_micros: int = 18_000_000,
    max_drawdown_pct: float = 5.0,
    sharpe_ratio: float = 0.85,
    sortino_ratio: float = 1.2,
    bars: int = 300,
    trade_count: int = 55,
    price_micros: int = 180_000_000,
) -> dict[str, Any]:
    """Minimal valid metrics.json dict (schema_version=1)."""
    return {
        "schema_version": 1,
        "run_id": "550e8400-e29b-41d4-a716-446655440000",
        "strategy_name": "intraday_scalper",
        "halted": False,
        "halt_reason": None,
        "execution_blocked": False,
        "bars": bars,
        "orders": 60,
        "orders_filled": 55,
        "orders_rejected": 5,
        "fills": 110,
        "final_equity_micros": 101_000_000_000,
        "symbols": ["AAPL"],
        "last_prices_micros": {"AAPL": price_micros},
        "starting_equity_micros": 100_000_000_000,
        "ending_equity_micros": 101_000_000_000,
        "total_return_micros": 1_000_000_000,
        "total_return_pct": 1.0,
        "equity_high_water_mark_micros": 102_000_000_000,
        "max_drawdown_micros": 500_000_000,
        "max_drawdown_pct": max_drawdown_pct,
        "total_commission_micros": 5_500_000,
        "trade_count": trade_count,
        "winning_trade_count": 33,
        "losing_trade_count": 22,
        "flat_trade_count": 0,
        "win_rate_pct": win_rate_pct,
        "gross_profit_micros": 10_000_000_000,
        "gross_loss_micros": -8_000_000_000,
        "profit_factor": profit_factor,
        "average_win_micros": 303_030_303,
        "average_loss_micros": -363_636_364,
        "expectancy_micros": expectancy_micros,
        "best_trade_micros": 500_000_000,
        "worst_trade_micros": -400_000_000,
        "sharpe_ratio": sharpe_ratio,
        "sortino_ratio": sortino_ratio,
        "exposure_bars": 150,
        "exposure_time_pct": 50.0,
        "strategy_sizing": {
            "target_qty": 1,
            "max_target_qty": None,
            "max_position_notional_usd": None,
        },
    }


def _write_metrics_file(tmpdir: str, metrics: dict[str, Any]) -> str:
    """Write metrics dict to a temp file and return its path."""
    p = Path(tmpdir) / "metrics.json"
    p.write_text(json.dumps(metrics), encoding="utf-8")
    return str(p)


# ---------------------------------------------------------------------------
# Test 1: dry-run default does not execute subprocess
# ---------------------------------------------------------------------------

class TestDryRunDoesNotExecuteSubprocess(unittest.TestCase):

    def test_dry_run_default_does_not_execute_subprocess(self):
        """Default config (mode=dry_run) must never invoke subprocess.run."""
        import subprocess
        original_run = subprocess.run
        calls = []

        def spy(*a, **kw):
            calls.append(a)
            return original_run(*a, **kw)

        subprocess.run = spy
        try:
            queue = _make_queue_artifact()
            with tempfile.TemporaryDirectory() as tmpdir:
                run_backtest_queue(
                    queue=queue,
                    output_dir=tmpdir,
                    generated_at_utc="2026-06-05T17:00:00+00:00",
                )
        finally:
            subprocess.run = original_run

        self.assertEqual(calls, [], "subprocess.run must not be called in dry-run mode")


# ---------------------------------------------------------------------------
# Test 2: command builder produces correct command structure
# ---------------------------------------------------------------------------

class TestCommandBuilder(unittest.TestCase):

    def test_command_builder_returns_list_with_binary_and_bars(self):
        """build_mqk_backtest_command returns a list starting with the binary."""
        with tempfile.TemporaryDirectory() as tmpdir:
            # Create a fake bars CSV
            bars_path = Path(tmpdir) / "AAPL_5m.csv"
            bars_path.write_text("ts,open,high,low,close,volume\n", encoding="utf-8")
            fake_binary = str(Path(tmpdir) / "mqk")
            Path(fake_binary).write_text("", encoding="utf-8")

            cfg = BacktestBridgeConfig(
                mode="real",
                cli_binary=fake_binary,
                bars_root_dir=tmpdir,
                out_dir=tmpdir,
            )
            entry = _make_queue_entry()
            cmd = build_mqk_backtest_command(entry, cfg)

        self.assertIsNotNone(cmd, "command must be built when all inputs present")
        self.assertIsInstance(cmd, list)
        self.assertEqual(cmd[0], fake_binary)
        self.assertIn("backtest", cmd)
        self.assertIn("csv", cmd)
        self.assertIn("--bars", cmd)
        self.assertIn("--strategy", cmd)
        self.assertIn("intraday_scalper", cmd)

    def test_command_builder_returns_none_without_binary(self):
        cfg = BacktestBridgeConfig(mode="real", bars_root_dir="/some/dir")
        cmd = build_mqk_backtest_command(_make_queue_entry(), cfg)
        self.assertIsNone(cmd)

    def test_command_builder_returns_none_without_bars_root(self):
        cfg = BacktestBridgeConfig(mode="real", cli_binary="/bin/mqk")
        cmd = build_mqk_backtest_command(_make_queue_entry(), cfg)
        self.assertIsNone(cmd)

    def test_command_builder_returns_none_for_unknown_strategy(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            cfg = BacktestBridgeConfig(
                mode="real",
                cli_binary=str(Path(tmpdir) / "mqk"),
                bars_root_dir=tmpdir,
            )
            entry = _make_queue_entry(strategy_id="vwap_mean_reversion")
            cmd = build_mqk_backtest_command(entry, cfg)
        self.assertIsNone(cmd, "unknown strategy_id must return None")

    def test_command_builder_returns_none_when_bars_file_absent(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            # bars file does NOT exist
            cfg = BacktestBridgeConfig(
                mode="real",
                cli_binary=str(Path(tmpdir) / "mqk"),
                bars_root_dir=tmpdir,
            )
            cmd = build_mqk_backtest_command(_make_queue_entry(), cfg)
        self.assertIsNone(cmd)

    def test_command_builder_includes_timeframe_secs(self):
        """5m timeframe → --timeframe-secs 300."""
        with tempfile.TemporaryDirectory() as tmpdir:
            bars_path = Path(tmpdir) / "AAPL_5m.csv"
            bars_path.write_text("ts\n", encoding="utf-8")
            cfg = BacktestBridgeConfig(
                mode="real",
                cli_binary="/fake/mqk",
                bars_root_dir=tmpdir,
                out_dir=tmpdir,
            )
            cmd = build_mqk_backtest_command(_make_queue_entry(), cfg)
        self.assertIsNotNone(cmd)
        idx = cmd.index("--timeframe-secs")
        self.assertEqual(cmd[idx + 1], "300")


# ---------------------------------------------------------------------------
# Test 3: metrics parser reads a valid fixture file
# ---------------------------------------------------------------------------

class TestMetricsParser(unittest.TestCase):

    def test_metrics_parser_reads_fixture_file(self):
        """parse_mqk_metrics_json successfully loads a valid metrics.json."""
        with tempfile.TemporaryDirectory() as tmpdir:
            path = _write_metrics_file(tmpdir, _make_metrics_dict())
            result = parse_mqk_metrics_json(path)
        self.assertEqual(result["schema_version"], 1)
        self.assertEqual(result["win_rate_pct"], 60.0)
        self.assertIn("trade_count", result)

    def test_missing_metrics_file_fails_closed(self):
        """Missing file raises ValueError with metrics_file_missing prefix."""
        with self.assertRaises(ValueError) as ctx:
            parse_mqk_metrics_json("/nonexistent/path/metrics.json")
        self.assertIn(REASON_METRICS_FILE_MISSING, str(ctx.exception))

    def test_invalid_json_fails_closed(self):
        """Non-JSON file raises ValueError with metrics_schema_invalid prefix."""
        with tempfile.TemporaryDirectory() as tmpdir:
            p = Path(tmpdir) / "metrics.json"
            p.write_text("not valid json {{{", encoding="utf-8")
            with self.assertRaises(ValueError) as ctx:
                parse_mqk_metrics_json(str(p))
        self.assertIn(REASON_METRICS_SCHEMA_INVALID, str(ctx.exception))

    def test_wrong_schema_version_fails_closed(self):
        """Wrong schema_version raises ValueError with metrics_schema_invalid prefix."""
        m = _make_metrics_dict()
        m["schema_version"] = 99
        with tempfile.TemporaryDirectory() as tmpdir:
            path = _write_metrics_file(tmpdir, m)
            with self.assertRaises(ValueError) as ctx:
                parse_mqk_metrics_json(path)
        self.assertIn(REASON_METRICS_SCHEMA_INVALID, str(ctx.exception))

    def test_non_object_root_fails_closed(self):
        """JSON array root raises ValueError with metrics_schema_invalid prefix."""
        with tempfile.TemporaryDirectory() as tmpdir:
            p = Path(tmpdir) / "metrics.json"
            p.write_text("[1,2,3]", encoding="utf-8")
            with self.assertRaises(ValueError) as ctx:
                parse_mqk_metrics_json(str(p))
        self.assertIn(REASON_METRICS_SCHEMA_INVALID, str(ctx.exception))


# ---------------------------------------------------------------------------
# Tests 6-11: metric mapping / normalization
# ---------------------------------------------------------------------------

class TestWinRateNormalization(unittest.TestCase):
    """Tests for _normalize_pct_to_fraction under percent_0_100 (default) and auto scales."""

    # --- percent_0_100 (default, Rust-compatible) ---

    def test_win_rate_pct_60_maps_to_0_60(self):
        """win_rate_pct=60.0 under percent_0_100 → 0.60."""
        self.assertAlmostEqual(_normalize_pct_to_fraction(60.0), 0.60)

    def test_win_rate_pct_0_60_maps_to_0_006_under_default_scale(self):
        """win_rate_pct=0.60 under percent_0_100 → 0.006 (0.6% win rate from Rust)."""
        self.assertAlmostEqual(_normalize_pct_to_fraction(0.60), 0.006)

    def test_win_rate_pct_100_maps_to_1_0(self):
        """win_rate_pct=100.0 → 1.0."""
        self.assertAlmostEqual(_normalize_pct_to_fraction(100.0), 1.0)

    def test_win_rate_pct_0_maps_to_0(self):
        """win_rate_pct=0.0 → 0.0."""
        self.assertAlmostEqual(_normalize_pct_to_fraction(0.0), 0.0)

    def test_win_rate_explicit_percent_0_100_scale(self):
        """Explicit percent_0_100 scale matches default."""
        self.assertAlmostEqual(
            _normalize_pct_to_fraction(60.0, PCT_SCALE_PERCENT_0_100), 0.60
        )

    # --- auto (legacy, not for production) ---

    def test_win_rate_auto_above_1_divides_by_100(self):
        """auto scale: 60.0 → 0.60 (>1.0 heuristic)."""
        self.assertAlmostEqual(_normalize_pct_to_fraction(60.0, PCT_SCALE_AUTO), 0.60)

    def test_win_rate_auto_at_or_below_1_preserved(self):
        """auto scale: 0.60 → 0.60 (≤1.0 treated as fraction)."""
        self.assertAlmostEqual(_normalize_pct_to_fraction(0.60, PCT_SCALE_AUTO), 0.60)


class TestDrawdownNormalization(unittest.TestCase):
    """Tests for _normalize_drawdown_to_bps under percent_0_100 (default) and auto scales."""

    # --- percent_0_100 (default, Rust-compatible) ---

    def test_max_drawdown_pct_5_maps_to_500_bps(self):
        """max_drawdown_pct=5.0 under percent_0_100 → 500 bps."""
        self.assertAlmostEqual(_normalize_drawdown_to_bps(5.0), 500.0)

    def test_max_drawdown_pct_0_5_maps_to_50_bps(self):
        """max_drawdown_pct=0.5 under percent_0_100 → 50 bps (0.5% drawdown)."""
        self.assertAlmostEqual(_normalize_drawdown_to_bps(0.5), 50.0)

    def test_max_drawdown_pct_0_05_maps_to_5_bps_under_default_scale(self):
        """max_drawdown_pct=0.05 under percent_0_100 → 5 bps (0.05% drawdown from Rust)."""
        self.assertAlmostEqual(_normalize_drawdown_to_bps(0.05), 5.0)

    def test_max_drawdown_pct_10_maps_to_1000_bps(self):
        """max_drawdown_pct=10.0 → 1000 bps."""
        self.assertAlmostEqual(_normalize_drawdown_to_bps(10.0), 1000.0)

    def test_max_drawdown_explicit_percent_0_100_scale(self):
        """Explicit percent_0_100 scale matches default."""
        self.assertAlmostEqual(
            _normalize_drawdown_to_bps(5.0, PCT_SCALE_PERCENT_0_100), 500.0
        )

    # --- auto (legacy, not for production) ---

    def test_max_drawdown_auto_above_1_multiplies_by_100(self):
        """auto scale: 5.0 → 500 bps (>1.0 heuristic)."""
        self.assertAlmostEqual(_normalize_drawdown_to_bps(5.0, PCT_SCALE_AUTO), 500.0)

    def test_max_drawdown_auto_at_or_below_1_multiplies_by_10000(self):
        """auto scale: 0.05 → 500 bps (≤1.0 treated as fraction * 10000)."""
        self.assertAlmostEqual(_normalize_drawdown_to_bps(0.05, PCT_SCALE_AUTO), 500.0)


class TestMetricMapping(unittest.TestCase):

    def _map(self, metrics: dict | None = None, entry: dict | None = None) -> tuple:
        m = metrics or _make_metrics_dict()
        e = entry or _make_queue_entry()
        cfg = BacktestBridgeConfig()
        return map_metrics_to_strategy_fit(e, m, cfg)

    def test_profit_factor_maps_correctly(self):
        mapped, _ = self._map(_make_metrics_dict(profit_factor=1.5))
        self.assertAlmostEqual(mapped["profit_factor"], 1.5)

    def test_sharpe_maps_correctly(self):
        mapped, _ = self._map(_make_metrics_dict(sharpe_ratio=0.85))
        self.assertAlmostEqual(mapped["sharpe"], 0.85)

    def test_sortino_maps_correctly(self):
        mapped, _ = self._map(_make_metrics_dict(sortino_ratio=1.2))
        self.assertAlmostEqual(mapped["sortino"], 1.2)

    def test_bars_used_maps_correctly(self):
        mapped, _ = self._map(_make_metrics_dict(bars=350))
        self.assertEqual(mapped["bars_used"], 350)

    def test_trade_count_maps_correctly(self):
        mapped, _ = self._map(_make_metrics_dict(trade_count=42))
        self.assertEqual(mapped["trades"], 42)

    def test_win_rate_normalized_from_pct(self):
        """win_rate_pct=60.0 → win_rate=0.60 via map_metrics_to_strategy_fit."""
        mapped, _ = self._map(_make_metrics_dict(win_rate_pct=60.0))
        self.assertAlmostEqual(mapped["win_rate"], 0.60)

    def test_max_drawdown_converted_to_bps(self):
        """max_drawdown_pct=5.0 → max_drawdown_bps=500."""
        mapped, _ = self._map(_make_metrics_dict(max_drawdown_pct=5.0))
        self.assertAlmostEqual(mapped["max_drawdown_bps"], 500.0)

    def test_expectancy_computed_when_notional_available(self):
        """expectancy_micros → expectancy_bps when symbol/price/qty are present."""
        # expectancy_micros=18_000_000, price=180_000_000, qty=1
        # expectancy_bps = 18_000_000 / 180_000_000 * 10000 = 1000.0
        mapped, reasons = self._map(
            _make_metrics_dict(expectancy_micros=18_000_000, price_micros=180_000_000)
        )
        self.assertIsNotNone(mapped["expectancy_bps"])
        self.assertAlmostEqual(mapped["expectancy_bps"], 1000.0, places=1)
        self.assertNotIn(REASON_EXPECTANCY_BASIS_MISSING, reasons)

    def test_expectancy_basis_missing_when_no_symbols(self):
        """expectancy_basis_missing when symbols list is empty."""
        m = _make_metrics_dict()
        m["symbols"] = []
        mapped, reasons = self._map(m)
        self.assertIsNone(mapped["expectancy_bps"])
        self.assertIn(REASON_EXPECTANCY_BASIS_MISSING, reasons)

    def test_avg_trade_bps_is_none(self):
        """avg_trade_bps is always None (not derivable without per-trade notional)."""
        mapped, _ = self._map()
        self.assertIsNone(mapped["avg_trade_bps"])

    def test_validation_metrics_missing_when_validation_fields_absent(self):
        """REASON_VALIDATION_METRICS_MISSING is appended when validation fields absent."""
        _, reasons = self._map()
        self.assertIn(REASON_VALIDATION_METRICS_MISSING, reasons)

    def test_net_expectancy_computed_when_commissions_available(self):
        """net_expectancy_after_cost_bps is set when notional and commissions present."""
        mapped, _ = self._map()
        self.assertIsNotNone(mapped["net_expectancy_after_cost_bps"])


# ---------------------------------------------------------------------------
# Tests: validation field mapping (BACKTEST-METRIC-NORMALIZATION-AUDIT-01 /
#        BACKTEST-VALIDATION-METRICS-01)
# ---------------------------------------------------------------------------

class TestValidationFieldMapping(unittest.TestCase):
    """Validation and additional gate field mapping from metrics.json."""

    def _map_with_metrics(self, extra: dict) -> tuple:
        m = _make_metrics_dict()
        m.update(extra)
        e = _make_queue_entry()
        cfg = BacktestBridgeConfig()
        return map_metrics_to_strategy_fit(e, m, cfg)

    def test_validation_metrics_missing_absent_when_validation_fields_present(self):
        """validation_metrics_missing NOT in reasons when both validation fields present."""
        mapped, reasons = self._map_with_metrics({
            "validation_profit_factor": 1.2,
            "validation_trades": 15,
        })
        self.assertNotIn(REASON_VALIDATION_METRICS_MISSING, reasons)

    def test_validation_metrics_missing_present_when_pf_absent(self):
        """validation_metrics_missing present when validation_profit_factor absent."""
        mapped, reasons = self._map_with_metrics({"validation_trades": 15})
        self.assertIn(REASON_VALIDATION_METRICS_MISSING, reasons)

    def test_validation_metrics_missing_present_when_trades_absent(self):
        """validation_metrics_missing present when validation_trades absent."""
        mapped, reasons = self._map_with_metrics({"validation_profit_factor": 1.2})
        self.assertIn(REASON_VALIDATION_METRICS_MISSING, reasons)

    def test_validation_profit_factor_maps_correctly(self):
        """validation_profit_factor from metrics.json maps to mapped dict."""
        mapped, _ = self._map_with_metrics({
            "validation_profit_factor": 1.35,
            "validation_trades": 12,
        })
        self.assertAlmostEqual(mapped["validation_profit_factor"], 1.35)

    def test_validation_trades_maps_correctly(self):
        """validation_trades from metrics.json maps to mapped dict."""
        mapped, _ = self._map_with_metrics({
            "validation_profit_factor": 1.2,
            "validation_trades": 18,
        })
        self.assertEqual(mapped["validation_trades"], 18)

    def test_largest_trade_profit_fraction_maps_directly_if_present(self):
        """largest_trade_profit_fraction passed directly from metrics if present."""
        mapped, _ = self._map_with_metrics({"largest_trade_profit_fraction": 0.12})
        self.assertAlmostEqual(mapped["largest_trade_profit_fraction"], 0.12)

    def test_largest_trade_profit_fraction_derived_from_best_and_gross(self):
        """largest_trade_profit_fraction derived from best_trade_micros / gross_profit_micros."""
        # Default fixture: best_trade_micros=500_000_000, gross_profit_micros=10_000_000_000
        # → 500_000_000 / 10_000_000_000 = 0.05
        mapped, _ = self._map_with_metrics({})
        self.assertIsNotNone(mapped["largest_trade_profit_fraction"])
        self.assertAlmostEqual(mapped["largest_trade_profit_fraction"], 0.05, places=5)

    def test_largest_trade_profit_fraction_none_when_gross_zero(self):
        """largest_trade_profit_fraction is None when gross_profit_micros is 0."""
        mapped, _ = self._map_with_metrics({"gross_profit_micros": 0})
        self.assertIsNone(mapped["largest_trade_profit_fraction"])

    def test_sample_quality_maps_correctly(self):
        """sample_quality from metrics.json maps to mapped dict."""
        mapped, _ = self._map_with_metrics({"sample_quality": 0.82})
        self.assertAlmostEqual(mapped["sample_quality"], 0.82)

    def test_sample_quality_none_when_absent(self):
        """sample_quality is None when not present in metrics.json."""
        mapped, _ = self._map_with_metrics({})
        self.assertIsNone(mapped["sample_quality"])

    def test_parameter_stability_score_maps_correctly(self):
        """parameter_stability_score from metrics.json maps to mapped dict."""
        mapped, _ = self._map_with_metrics({"parameter_stability_score": 0.75})
        self.assertAlmostEqual(mapped["parameter_stability_score"], 0.75)

    def test_parameter_stability_score_none_when_absent(self):
        """parameter_stability_score is None when not present in metrics.json."""
        mapped, _ = self._map_with_metrics({})
        self.assertIsNone(mapped["parameter_stability_score"])

    def test_artifact_with_all_validation_fields_can_pass_oos_gate(self):
        """Full validation fields + passing thresholds → out-of-sample gate passes."""
        from mqk_research.scanner.backtest_gates import apply_backtest_gates, REASON_OUT_OF_SAMPLE
        from mqk_research.scanner.backtest_runner import build_strategy_fit_artifact

        full_overrides = {
            "schema_version": "strategy-fit-v1",
            "status": "complete",
            "bars_used": 300,
            "trades": 50,
            "win_rate": 0.60,
            "profit_factor": 1.5,
            "expectancy_bps": 10.0,
            "max_drawdown_bps": 500.0,
            "net_expectancy_after_cost_bps": 8.0,
            "validation_profit_factor": 1.2,
            "validation_trades": 15,
            "sample_quality": 0.8,
            "parameter_stability_score": 0.9,
            "largest_trade_profit_fraction": 0.10,
            "failure_reasons": [],
        }
        artifact = apply_backtest_gates(full_overrides)
        self.assertNotIn(REASON_OUT_OF_SAMPLE, artifact["failure_reasons"])
        self.assertTrue(artifact["passed_out_of_sample_check"])
        self.assertTrue(artifact["recommended_for_paper"])
        self.assertFalse(artifact["recommended_for_live"])

    def test_recommended_for_live_remains_false_with_validation_fields(self):
        """recommended_for_live is always False even when all gates pass."""
        from mqk_research.scanner.backtest_gates import apply_backtest_gates
        full_overrides = {
            "status": "complete",
            "bars_used": 300,
            "trades": 50,
            "profit_factor": 1.5,
            "expectancy_bps": 10.0,
            "max_drawdown_bps": 500.0,
            "net_expectancy_after_cost_bps": 8.0,
            "validation_profit_factor": 1.2,
            "validation_trades": 15,
            "sample_quality": 0.8,
            "parameter_stability_score": 0.9,
            "largest_trade_profit_fraction": 0.10,
            "failure_reasons": [],
        }
        artifact = apply_backtest_gates(full_overrides)
        self.assertFalse(artifact["recommended_for_live"])

    def test_validation_fields_in_strategy_fit_result(self):
        """StrategyFitResult carries validation fields."""
        from mqk_research.scanner.backtest_runner import StrategyFitResult
        r = StrategyFitResult(
            validation_profit_factor=1.2,
            validation_trades=15,
            largest_trade_profit_fraction=0.10,
        )
        self.assertAlmostEqual(r.validation_profit_factor, 1.2)
        self.assertEqual(r.validation_trades, 15)
        self.assertAlmostEqual(r.largest_trade_profit_fraction, 0.10)

    def test_validation_fields_in_built_artifact(self):
        """build_strategy_fit_artifact includes validation fields in output dict."""
        from mqk_research.scanner.backtest_runner import (
            build_strategy_fit_artifact, StrategyFitResult, BacktestRunnerConfig,
        )
        entry = _make_queue_entry()
        result = StrategyFitResult(
            bars_used=300, trades=50,
            validation_profit_factor=1.3,
            validation_trades=12,
            largest_trade_profit_fraction=0.08,
        )
        artifact = build_strategy_fit_artifact(
            entry=entry,
            result=result,
            config=BacktestRunnerConfig(mode="real"),
            generated_at_utc="2026-06-05T19:00:00+00:00",
        )
        self.assertAlmostEqual(artifact["validation_profit_factor"], 1.3)
        self.assertEqual(artifact["validation_trades"], 12)
        self.assertAlmostEqual(artifact["largest_trade_profit_fraction"], 0.08)


# ---------------------------------------------------------------------------
# Test 12: missing validation metrics cause out-of-sample gate failure
# ---------------------------------------------------------------------------

class TestValidationMetricsAbsent(unittest.TestCase):

    def test_missing_validation_metrics_cause_oos_gate_failure(self):
        """After mapping + applying gates, out_of_sample_failed in failure_reasons."""
        from mqk_research.scanner.backtest_gates import apply_backtest_gates, REASON_OUT_OF_SAMPLE

        entry = _make_queue_entry()
        cfg = BacktestBridgeConfig()
        metrics = _make_metrics_dict()
        mapped, extra_reasons = map_metrics_to_strategy_fit(entry, metrics, cfg)

        # Build a "complete" strategy-fit artifact
        from mqk_research.scanner.backtest_runner import build_strategy_fit_artifact
        runner_cfg = BacktestRunnerConfig(mode="real")
        result = StrategyFitResult(**mapped)
        artifact = build_strategy_fit_artifact(
            entry=entry,
            result=result,
            config=runner_cfg,
            generated_at_utc="2026-06-05T17:00:00+00:00",
            extra_failure_reasons=extra_reasons,
        )
        artifact = apply_backtest_gates(artifact)

        self.assertIn(REASON_OUT_OF_SAMPLE, artifact["failure_reasons"])
        self.assertFalse(artifact["recommended_for_paper"])


# ---------------------------------------------------------------------------
# Tests 13, 14, 15: recommended_for_live / recommended_for_paper enforcement
# ---------------------------------------------------------------------------

class TestRecommendationEnforcement(unittest.TestCase):

    def _build_and_gate(self, artifact_extra: dict | None = None) -> dict:
        from mqk_research.scanner.backtest_gates import apply_backtest_gates
        entry = _make_queue_entry()
        metrics = _make_metrics_dict()
        cfg = BacktestBridgeConfig()
        mapped, extra_reasons = map_metrics_to_strategy_fit(entry, metrics, cfg)

        from mqk_research.scanner.backtest_runner import build_strategy_fit_artifact
        runner_cfg = BacktestRunnerConfig(mode="real")
        result = StrategyFitResult(**mapped)
        artifact = build_strategy_fit_artifact(
            entry=entry,
            result=result,
            config=runner_cfg,
            generated_at_utc="2026-06-05T17:00:00+00:00",
            extra_failure_reasons=extra_reasons,
        )
        if artifact_extra:
            artifact.update(artifact_extra)
        return apply_backtest_gates(artifact)

    def test_recommended_for_live_always_false(self):
        """recommended_for_live must always be False after gate application."""
        artifact = self._build_and_gate()
        self.assertFalse(artifact["recommended_for_live"])

    def test_recommended_for_paper_false_without_all_gates(self):
        """recommended_for_paper=False when validation + additional gate fields absent."""
        artifact = self._build_and_gate()
        # validation_metrics_missing, out_of_sample, sample_quality, parameter_stability
        # and single_trade_dependency are all absent → False
        self.assertFalse(artifact["recommended_for_paper"])

    def test_recommended_for_paper_true_when_all_gates_pass(self):
        """recommended_for_paper=True when ALL gate inputs satisfy all thresholds."""
        # Override with all required + additional gate inputs.
        full_overrides = {
            "status": STATUS_COMPLETE,
            "bars_used": 300,
            "trades": 50,
            "win_rate": 0.60,
            "profit_factor": 1.5,
            "expectancy_bps": 10.0,
            "max_drawdown_bps": 500.0,
            "net_expectancy_after_cost_bps": 8.0,
            "sharpe": 0.85,
            "sortino": 1.2,
            # out-of-sample gate inputs
            "validation_profit_factor": 1.2,
            "validation_trades": 15,
            # additional gate inputs
            "sample_quality": 0.8,
            "parameter_stability_score": 0.9,
            "largest_trade_profit_fraction": 0.10,
            # clear prior failure reasons
            "failure_reasons": [],
        }
        from mqk_research.scanner.backtest_gates import apply_backtest_gates
        artifact = apply_backtest_gates(full_overrides)
        self.assertTrue(artifact["recommended_for_paper"])
        self.assertFalse(artifact["recommended_for_live"])


# ---------------------------------------------------------------------------
# Test 16: dry-run behavior unchanged
# ---------------------------------------------------------------------------

class TestDryRunBehaviorUnchanged(unittest.TestCase):

    def test_dry_run_mode_still_produces_blocked_artifact(self):
        """mode="dry_run" must still produce status=blocked_no_backtest_interface."""
        queue = _make_queue_artifact()
        with tempfile.TemporaryDirectory() as tmpdir:
            run_backtest_queue(
                queue=queue,
                output_dir=tmpdir,
                generated_at_utc="2026-06-05T17:00:00+00:00",
            )
            files = list(Path(tmpdir).glob("*.json"))
            self.assertEqual(len(files), 1)
            art = json.loads(files[0].read_text(encoding="utf-8"))
        self.assertEqual(art["status"], STATUS_BLOCKED)
        self.assertIn(FAILURE_REASON_NO_INTERFACE, art["failure_reasons"])
        self.assertFalse(art["recommended_for_live"])

    def test_dry_run_existing_behavior_multi_entry(self):
        """Multiple entries all produce STATUS_BLOCKED in dry-run mode."""
        entries = [_make_queue_entry(queue_id=f"bq-{i:03d}") for i in range(3)]
        queue = _make_queue_artifact(entries=entries)
        with tempfile.TemporaryDirectory() as tmpdir:
            result = run_backtest_queue(
                queue=queue,
                output_dir=tmpdir,
                generated_at_utc="2026-06-05T17:00:00+00:00",
            )
            for f in Path(tmpdir).glob("*.json"):
                art = json.loads(f.read_text(encoding="utf-8"))
                self.assertEqual(art["status"], STATUS_BLOCKED)
        self.assertEqual(result.artifacts_written, 3)


# ---------------------------------------------------------------------------
# Test 17: failed subprocess → failed artifact, not crash
# ---------------------------------------------------------------------------

class TestFailedSubprocessConvertedToFailedArtifact(unittest.TestCase):

    def test_binary_not_found_yields_failure_reason(self):
        """Non-existent binary → binary_not_found in failure_reasons."""
        entry = _make_queue_entry()
        with tempfile.TemporaryDirectory() as tmpdir:
            # Create bars file so command can be built
            bars = Path(tmpdir) / "AAPL_5m.csv"
            bars.write_text("ts\n", encoding="utf-8")
            cfg = BacktestBridgeConfig(
                mode="real",
                cli_binary=str(Path(tmpdir) / "nonexistent_binary"),
                bars_root_dir=tmpdir,
                out_dir=tmpdir,
            )
            metrics_dict, reasons = run_backtest_for_entry(entry, cfg)
        self.assertIsNone(metrics_dict)
        self.assertIn(REASON_BINARY_NOT_FOUND, reasons)

    def test_command_build_failure_yields_reason(self):
        """No binary configured → command_build_failed in failure_reasons."""
        entry = _make_queue_entry()
        cfg = BacktestBridgeConfig(mode="real")  # no cli_binary
        metrics_dict, reasons = run_backtest_for_entry(entry, cfg)
        self.assertIsNone(metrics_dict)
        self.assertIn(REASON_COMMAND_BUILD_FAILED, reasons)

    def test_failed_subprocess_returns_failure_reason_not_crash(self):
        """subprocess.run non-zero returncode → backtest_command_failed, no exception."""
        entry = _make_queue_entry()
        with tempfile.TemporaryDirectory() as tmpdir:
            bars = Path(tmpdir) / "AAPL_5m.csv"
            bars.write_text("ts\n", encoding="utf-8")
            fake_binary = str(Path(tmpdir) / "fake_mqk")
            Path(fake_binary).write_text("", encoding="utf-8")
            cfg = BacktestBridgeConfig(
                mode="real",
                cli_binary=fake_binary,
                bars_root_dir=tmpdir,
                out_dir=tmpdir,
            )

            mock_result = MagicMock()
            mock_result.returncode = 1
            mock_result.stdout = ""
            mock_result.stderr = "error"

            import mqk_research.scanner.backtest_bridge as bridge_mod
            import subprocess
            original = subprocess.run

            def fake_run(*a, **kw):
                return mock_result

            # Patch subprocess.run inside the bridge module's namespace
            # by temporarily patching within the function call scope.
            # Since subprocess is imported deferred inside run_backtest_for_entry,
            # we patch subprocess.run at the subprocess module level.
            subprocess.run = fake_run
            try:
                # Restore Path to real before the call so command-building works
                metrics_dict, reasons = run_backtest_for_entry(entry, cfg)
            finally:
                subprocess.run = original

        self.assertIsNone(metrics_dict)
        self.assertIn(REASON_BACKTEST_COMMAND_FAILED, reasons)


# ---------------------------------------------------------------------------
# Test 18: subprocess mocked in unit tests
# ---------------------------------------------------------------------------

class TestSubprocessMockedInUnitTests(unittest.TestCase):

    def test_real_mode_calls_subprocess_when_binary_exists(self):
        """
        In mode="real" with valid binary and bars, run_backtest_for_entry calls subprocess.
        subprocess.run is mocked to avoid actual binary execution.
        """
        import subprocess

        entry = _make_queue_entry()
        with tempfile.TemporaryDirectory() as tmpdir:
            bars = Path(tmpdir) / "AAPL_5m.csv"
            bars.write_text("ts\n", encoding="utf-8")
            fake_binary = str(Path(tmpdir) / "fake_mqk")
            Path(fake_binary).write_text("", encoding="utf-8")
            cfg = BacktestBridgeConfig(
                mode="real",
                cli_binary=fake_binary,
                bars_root_dir=tmpdir,
                out_dir=tmpdir,
            )

            subprocess_calls = []
            original_run = subprocess.run

            def spy_run(cmd, **kw):
                subprocess_calls.append(cmd)
                mock = MagicMock()
                mock.returncode = 1
                mock.stdout = ""
                mock.stderr = "mocked failure"
                return mock

            subprocess.run = spy_run
            try:
                run_backtest_for_entry(entry, cfg)
            finally:
                subprocess.run = original_run

        # subprocess.run should have been called once
        self.assertEqual(len(subprocess_calls), 1)
        self.assertIn("backtest", subprocess_calls[0])


# ---------------------------------------------------------------------------
# Tests 19-21: no forbidden imports
# ---------------------------------------------------------------------------

class TestNoForbiddenImportsInBridge(unittest.TestCase):

    def _read_bridge_source(self) -> str:
        import mqk_research.scanner.backtest_bridge as mod
        return Path(mod.__file__).read_text(encoding="utf-8")

    def test_no_broker_oms_references(self):
        src = self._read_bridge_source()
        for token in ["/v2/orders", "submit_order", "BrokerGateway",
                      "oms_outbox", "oms_inbox", "live_routing_enabled=true"]:
            self.assertNotIn(token, src, msg=f"forbidden token in bridge: {token!r}")

    def test_no_network_imports(self):
        import re
        src = self._read_bridge_source()
        # Check for actual import statements at line start (not docstring mentions)
        for lib in ["requests", "urllib", "aiohttp", "http.client"]:
            pattern = rf"^import {re.escape(lib)}"
            self.assertIsNone(
                re.search(pattern, src, re.MULTILINE),
                msg=f"module-level 'import {lib}' found in backtest_bridge.py",
            )
            from_pattern = rf"^from {re.escape(lib)}"
            self.assertIsNone(
                re.search(from_pattern, src, re.MULTILINE),
                msg=f"module-level 'from {lib}' found in backtest_bridge.py",
            )

    def test_no_production_db_imports(self):
        src = self._read_bridge_source()
        for token in ["import psycopg", "import sqlalchemy",
                      "INSERT INTO", "UPDATE SET", "cursor.execute"]:
            self.assertNotIn(token, src, msg=f"DB import in bridge: {token!r}")

    def test_subprocess_not_at_module_level(self):
        import mqk_research.scanner.backtest_bridge as mod
        self.assertFalse(
            hasattr(mod, "subprocess"),
            "subprocess must not be a module-level attribute of backtest_bridge",
        )

    def test_recommended_for_live_false_in_bridge_config(self):
        cfg = BacktestBridgeConfig(recommended_for_live=True)
        self.assertFalse(cfg.recommended_for_live, "hard invariant must be False")


# ---------------------------------------------------------------------------
# Test 22: artifact path / write determinism
# ---------------------------------------------------------------------------

class TestArtifactDeterminism(unittest.TestCase):

    def test_artifact_path_is_deterministic(self):
        """Same queue_id always produces the same filename (round-trip)."""
        from mqk_research.scanner.backtest_runner import _artifact_filename
        name_a = _artifact_filename("bq-fixed-id-001")
        name_b = _artifact_filename("bq-fixed-id-001")
        self.assertEqual(name_a, name_b)

    def test_different_queue_ids_produce_different_paths(self):
        from mqk_research.scanner.backtest_runner import _artifact_filename
        self.assertNotEqual(
            _artifact_filename("bq-001"),
            _artifact_filename("bq-002"),
        )

    def test_real_mode_runner_writes_deterministic_artifacts(self):
        """Real-mode runner writes one file per queue entry with deterministic name."""
        entry = _make_queue_entry(queue_id="bq-det-001")
        queue = _make_queue_artifact(entries=[entry])

        with tempfile.TemporaryDirectory() as tmpdir:
            # Bridge config with no binary → command_build_failed, but still writes artifact
            from mqk_research.scanner.backtest_bridge import BacktestBridgeConfig
            bridge_cfg = BacktestBridgeConfig(mode="real")
            cfg = BacktestRunnerConfig(mode="real")

            run_backtest_queue(
                queue=queue,
                config=cfg,
                output_dir=tmpdir,
                generated_at_utc="2026-06-05T17:00:00+00:00",
                bridge_config=bridge_cfg,
            )
            files = list(Path(tmpdir).glob("*.json"))
            self.assertEqual(len(files), 1)
            art = json.loads(files[0].read_text(encoding="utf-8"))
            # Failed bridge run → blocked artifact
            self.assertFalse(art["recommended_for_live"])
            self.assertIn("command_build_failed", art["failure_reasons"])


# ---------------------------------------------------------------------------
# Extra: existing tests still pass — backtest_queue and backtest_gates smoke
# ---------------------------------------------------------------------------

class TestExistingModulesUnaffected(unittest.TestCase):

    def test_backtest_queue_import_unchanged(self):
        from mqk_research.scanner.backtest_queue import build_backtest_queue
        self.assertTrue(callable(build_backtest_queue))

    def test_backtest_gates_import_unchanged(self):
        from mqk_research.scanner.backtest_gates import apply_backtest_gates
        self.assertTrue(callable(apply_backtest_gates))

    def test_strategy_fit_result_fields_intact(self):
        r = StrategyFitResult()
        self.assertIsNone(r.win_rate)
        self.assertIsNone(r.profit_factor)
        self.assertIsNone(r.expectancy_bps)
        self.assertIsNone(r.max_drawdown_bps)
        self.assertIsNone(r.sharpe)
        self.assertIsNone(r.sortino)


if __name__ == "__main__":
    unittest.main()
