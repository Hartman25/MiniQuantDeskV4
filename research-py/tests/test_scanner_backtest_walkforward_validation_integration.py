"""
BACKTEST-WALKFORWARD-VALIDATION-INTEGRATION-01 — Tests proving walk-forward
aggregate validation metrics are wired into real-mode backtest strategy-fit
artifacts without weakening any gates.

Covers:
  A. Default behavior unchanged (flag off / dry-run unaffected).
  B. Positive integration: a successful walk-forward aggregate is merged and
     clears validation_metrics_missing, letting the out-of-sample gate (and
     recommended_for_paper) evaluate truthfully.
  C. Negative / fail-closed: blocked, partial, or low-quality walk-forward
     aggregates never fabricate metrics and never weaken gates.
  D. Safety: no broker/daemon/DB/network imports; subprocess remains
     double-gated (enable flag + walkforward_config.mode == "real").

No subprocess is ever actually executed in this file — run_backtest_for_entry
and run_walkforward_entry are mocked at their call sites, matching the
existing lazy-import-and-patch pattern used across the scanner test suite.
"""
from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from typing import Any, Optional
from unittest.mock import patch

from mqk_research.scanner.backtest_bridge import (
    REASON_VALIDATION_METRICS_MISSING,
    BacktestBridgeConfig,
)
from mqk_research.scanner.backtest_gates import apply_backtest_gates
from mqk_research.scanner.backtest_runner import (
    STATUS_COMPLETE,
    BacktestRunnerConfig,
    StrategyFitResult,
    _run_and_merge_walkforward_validation,
    run_backtest_queue,
)
from mqk_research.scanner.walkforward_runner import (
    REASON_WALKFORWARD_VALIDATION_BLOCKED,
    WalkForwardRunnerConfig,
    WalkForwardRunResult,
    merge_walkforward_validation_into_mapped,
)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

def _make_queue_entry(
    queue_id: str = "bq-wfv-001",
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
        "notes": "BACKTEST-WALKFORWARD-VALIDATION-INTEGRATION-01 entry",
    }


def _make_queue_artifact(entries: Optional[list[dict]] = None) -> dict[str, Any]:
    if entries is None:
        entries = [_make_queue_entry()]
    return {
        "schema_version": "backtest-queue-v1",
        "generated_at_utc": "2026-06-07T00:00:00+00:00",
        "source_ranked_export": None,
        "queue_count": len(entries),
        "entries": entries,
        "recommended_for_live": False,
        "notes": "BACKTEST-WALKFORWARD-VALIDATION-INTEGRATION-01 queue",
    }


def _make_metrics_dict() -> dict[str, Any]:
    """
    Minimal valid metrics.json dict (schema_version=1) shaped so every
    *required* gate passes under default BacktestGateConfig thresholds, with
    no validation_*/sample_quality/parameter_stability_score fields — this is
    what the Rust CLI actually emits today (BACKTEST-BRIDGE-BUNDLE-01 finding).
    """
    return {
        "schema_version": 1,
        "run_id": "550e8400-e29b-41d4-a716-446655440000",
        "strategy_name": "intraday_scalper",
        "halted": False,
        "halt_reason": None,
        "execution_blocked": False,
        "bars": 300,
        "orders": 60,
        "orders_filled": 55,
        "orders_rejected": 5,
        "fills": 110,
        "final_equity_micros": 101_000_000_000,
        "symbols": ["AAPL"],
        "last_prices_micros": {"AAPL": 180_000_000},
        "starting_equity_micros": 100_000_000_000,
        "ending_equity_micros": 101_000_000_000,
        "total_return_micros": 1_000_000_000,
        "total_return_pct": 1.0,
        "equity_high_water_mark_micros": 102_000_000_000,
        "max_drawdown_micros": 500_000_000,
        "max_drawdown_pct": 5.0,
        "total_commission_micros": 5_500_000,
        "trade_count": 55,
        "winning_trade_count": 33,
        "losing_trade_count": 22,
        "flat_trade_count": 0,
        "win_rate_pct": 60.0,
        "gross_profit_micros": 10_000_000_000,
        "gross_loss_micros": -8_000_000_000,
        "profit_factor": 1.5,
        "average_win_micros": 303_030_303,
        "average_loss_micros": -363_636_364,
        "expectancy_micros": 18_000_000,
        "best_trade_micros": 500_000_000,
        "worst_trade_micros": -400_000_000,
        "sharpe_ratio": 0.85,
        "sortino_ratio": 1.2,
        "exposure_bars": 150,
        "exposure_time_pct": 50.0,
        "strategy_sizing": {
            "target_qty": 1,
            "max_target_qty": None,
            "max_position_notional_usd": None,
        },
    }


def _make_wf_result(
    status: str = "complete",
    validation_profit_factor: Optional[float] = 1.4,
    validation_trades: Optional[int] = 45,
    sample_quality: Optional[float] = 0.9,
    parameter_stability_score: Optional[float] = 0.8,
    validation_metrics: Optional[dict[str, Any]] = "__default__",  # type: ignore[assignment]
) -> WalkForwardRunResult:
    """Construct a WalkForwardRunResult with controlled aggregate outputs."""
    if validation_metrics == "__default__":
        validation_metrics = {
            "validation_profit_factor": validation_profit_factor,
            "validation_trades": validation_trades,
            "validation_win_rate": 0.55,
            "sample_quality": sample_quality,
            "parameter_stability_score": parameter_stability_score,
            "split_count": 3,
            "passed_split_count": 3,
            "worst_split_profit_factor": validation_profit_factor,
            "median_split_profit_factor": validation_profit_factor,
            "all_splits_have_metrics": status == "complete",
            "recommended_for_live": False,
        }
    return WalkForwardRunResult(
        queue_id="bq-wfv-001",
        symbol="AAPL",
        strategy_id="intraday_scalper",
        mode="real",
        split_results=[],
        aggregate=None,
        validation_metrics=validation_metrics,
        failure_reasons=[] if status == "complete" else ["split_metrics_missing"],
        status=status,
        recommended_for_live=False,
    )


def _make_mapped(
    validation_profit_factor: Optional[float] = None,
    validation_trades: Optional[int] = None,
    sample_quality: Optional[float] = None,
    parameter_stability_score: Optional[float] = None,
    largest_trade_profit_fraction: Optional[float] = 0.05,
) -> dict[str, Any]:
    """A mapped strategy-fit dict shaped like map_metrics_to_strategy_fit output."""
    return {
        "bars_used": 300,
        "trades": 55,
        "win_rate": 0.60,
        "profit_factor": 1.5,
        "expectancy_bps": 1000.0,
        "avg_trade_bps": None,
        "max_drawdown_bps": 500.0,
        "sharpe": 0.85,
        "sortino": 1.2,
        "exposure_time_pct": 50.0,
        "net_expectancy_after_cost_bps": 994.4,
        "sample_quality": sample_quality,
        "parameter_stability_score": parameter_stability_score,
        "validation_profit_factor": validation_profit_factor,
        "validation_trades": validation_trades,
        "largest_trade_profit_fraction": largest_trade_profit_fraction,
    }


def _run_real_mode_with_mocks(
    runner_cfg: BacktestRunnerConfig,
    wf_result: Optional[WalkForwardRunResult],
    *,
    bars_root_dir: Optional[str] = None,
) -> tuple[Any, dict[str, Any], list]:
    """
    Drive run_backtest_queue(mode="real") with run_backtest_for_entry and
    run_walkforward_entry both mocked at their lazy-import call sites — proves
    the wiring without ever touching subprocess, broker, daemon, or DB.

    Returns (BacktestRunResult, written_artifact_dict, run_walkforward_entry_calls).
    """
    entry = _make_queue_entry()
    queue = _make_queue_artifact([entry])
    metrics = _make_metrics_dict()

    wf_calls: list = []

    def fake_run_walkforward_entry(qe, bars_csv, cfg=None, bridge_config=None):
        wf_calls.append((qe, bars_csv, cfg, bridge_config))
        assert wf_result is not None, "run_walkforward_entry should not be called"
        return wf_result

    with tempfile.TemporaryDirectory() as tmpdir:
        bars_root = bars_root_dir
        if bars_root is None:
            bars_root = str(Path(tmpdir) / "bars")
            Path(bars_root).mkdir(parents=True, exist_ok=True)
            (Path(bars_root) / "AAPL_5m.csv").write_text(
                "symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n"
                "AAPL,1000,99900000,100150000,99850000,100000000,600,1\n",
                encoding="utf-8",
            )

        bridge_cfg = BacktestBridgeConfig(
            mode="real",
            cli_binary=str(Path(tmpdir) / "fake-mqk-cli"),
            bars_root_dir=bars_root,
            out_dir=str(Path(tmpdir) / "out"),
        )
        Path(bridge_cfg.cli_binary).write_text("", encoding="utf-8")
        artifact_dir = str(Path(tmpdir) / "strategy_fit")

        with patch(
            "mqk_research.scanner.backtest_bridge.run_backtest_for_entry",
            return_value=(metrics, []),
        ), patch(
            "mqk_research.scanner.walkforward_runner.run_walkforward_entry",
            side_effect=fake_run_walkforward_entry,
        ), patch("subprocess.run") as mock_subproc_run:
            result = run_backtest_queue(
                queue=queue,
                config=runner_cfg,
                output_dir=artifact_dir,
                generated_at_utc="2026-06-07T00:00:00+00:00",
                bridge_config=bridge_cfg,
            )

        written = list(Path(artifact_dir).glob("*.json"))
        assert len(written) == 1
        artifact = _load_json(written[0])

    # subprocess.run must never be invoked — both the backtest CLI call and
    # the walk-forward split calls were mocked above the subprocess boundary.
    mock_subproc_run.assert_not_called()
    return result, artifact, wf_calls


def _load_json(path: Path) -> dict[str, Any]:
    import json
    return json.loads(path.read_text(encoding="utf-8"))


# ---------------------------------------------------------------------------
# A. Default behavior unchanged
# ---------------------------------------------------------------------------

class TestDefaultBehaviorUnchanged(unittest.TestCase):

    def test_config_defaults_disabled(self):
        cfg = BacktestRunnerConfig()
        self.assertFalse(cfg.enable_walkforward_validation)
        self.assertIsNone(cfg.walkforward_config)

    def test_real_mode_without_flag_still_appends_validation_metrics_missing(self):
        """Flag off (default) — behavior is identical to pre-patch real mode."""
        runner_cfg = BacktestRunnerConfig(mode="real")
        result, artifact, wf_calls = _run_real_mode_with_mocks(runner_cfg, wf_result=None)

        self.assertEqual(artifact["status"], STATUS_COMPLETE)
        self.assertIn(REASON_VALIDATION_METRICS_MISSING, artifact["failure_reasons"])
        self.assertNotIn(REASON_WALKFORWARD_VALIDATION_BLOCKED, artifact["failure_reasons"])
        self.assertIsNone(artifact["validation_profit_factor"])
        self.assertIsNone(artifact["validation_trades"])
        self.assertIsNone(artifact["sample_quality"])
        self.assertIsNone(artifact["parameter_stability_score"])
        self.assertEqual(wf_calls, [], "run_walkforward_entry must not be called when disabled")
        self.assertFalse(artifact["recommended_for_live"])

    def test_dry_run_mode_unaffected_even_with_flag_enabled(self):
        """enable_walkforward_validation=True must not change dry-run behavior."""
        from mqk_research.scanner.backtest_runner import (
            STATUS_BLOCKED,
            FAILURE_REASON_NO_INTERFACE,
        )

        runner_cfg = BacktestRunnerConfig(mode="dry_run", enable_walkforward_validation=True)
        queue = _make_queue_artifact()

        with patch(
            "mqk_research.scanner.walkforward_runner.run_walkforward_entry"
        ) as mock_wf, tempfile.TemporaryDirectory() as tmpdir:
            run_backtest_queue(
                queue=queue,
                config=runner_cfg,
                output_dir=tmpdir,
                generated_at_utc="2026-06-07T00:00:00+00:00",
            )
            written = list(Path(tmpdir).glob("*.json"))
            self.assertEqual(len(written), 1)
            artifact = _load_json(written[0])

        mock_wf.assert_not_called()
        self.assertEqual(artifact["status"], STATUS_BLOCKED)
        self.assertEqual(artifact["failure_reasons"], [FAILURE_REASON_NO_INTERFACE])


# ---------------------------------------------------------------------------
# B. Positive validation integration
# ---------------------------------------------------------------------------

class TestPositiveValidationIntegration(unittest.TestCase):

    def test_merge_fills_all_four_fields_and_clears_missing_reason(self):
        mapped = _make_mapped()
        wf_result = _make_wf_result()

        merged, reasons = merge_walkforward_validation_into_mapped(
            mapped, [REASON_VALIDATION_METRICS_MISSING], wf_result
        )

        self.assertEqual(merged["validation_profit_factor"], 1.4)
        self.assertEqual(merged["validation_trades"], 45)
        self.assertEqual(merged["sample_quality"], 0.9)
        self.assertEqual(merged["parameter_stability_score"], 0.8)
        self.assertNotIn(REASON_VALIDATION_METRICS_MISSING, reasons)
        self.assertNotIn(REASON_WALKFORWARD_VALIDATION_BLOCKED, reasons)

    def test_merge_never_overrides_existing_real_metric_values(self):
        """Values already mapped from metrics.json win over walk-forward values."""
        mapped = _make_mapped(
            validation_profit_factor=2.0,
            validation_trades=100,
            sample_quality=0.99,
            parameter_stability_score=0.95,
        )
        wf_result = _make_wf_result(
            validation_profit_factor=1.4, validation_trades=45,
            sample_quality=0.5, parameter_stability_score=0.5,
        )

        merged, reasons = merge_walkforward_validation_into_mapped(mapped, [], wf_result)

        self.assertEqual(merged["validation_profit_factor"], 2.0)
        self.assertEqual(merged["validation_trades"], 100)
        self.assertEqual(merged["sample_quality"], 0.99)
        self.assertEqual(merged["parameter_stability_score"], 0.95)

    def test_does_not_modify_input_mapped_or_reasons(self):
        mapped = _make_mapped()
        reasons_in = [REASON_VALIDATION_METRICS_MISSING]
        wf_result = _make_wf_result()

        merge_walkforward_validation_into_mapped(mapped, reasons_in, wf_result)

        self.assertIsNone(mapped["validation_profit_factor"])
        self.assertEqual(reasons_in, [REASON_VALIDATION_METRICS_MISSING])

    def test_end_to_end_artifact_has_no_validation_metrics_missing_and_passes_oos_gate(self):
        runner_cfg = BacktestRunnerConfig(mode="real", enable_walkforward_validation=True)
        wf_result = _make_wf_result(
            validation_profit_factor=1.4, validation_trades=45,
            sample_quality=0.9, parameter_stability_score=0.8,
        )

        result, artifact, wf_calls = _run_real_mode_with_mocks(runner_cfg, wf_result)

        self.assertEqual(len(wf_calls), 1, "run_walkforward_entry should be invoked exactly once")
        self.assertNotIn(REASON_VALIDATION_METRICS_MISSING, artifact["failure_reasons"])
        self.assertNotIn(REASON_WALKFORWARD_VALIDATION_BLOCKED, artifact["failure_reasons"])
        self.assertEqual(artifact["validation_profit_factor"], 1.4)
        self.assertEqual(artifact["validation_trades"], 45)
        self.assertEqual(artifact["sample_quality"], 0.9)
        self.assertEqual(artifact["parameter_stability_score"], 0.8)
        self.assertTrue(artifact["passed_out_of_sample_check"])

    def test_end_to_end_recommended_for_paper_true_when_all_gates_pass(self):
        runner_cfg = BacktestRunnerConfig(mode="real", enable_walkforward_validation=True)
        wf_result = _make_wf_result(
            validation_profit_factor=1.4, validation_trades=45,
            sample_quality=0.9, parameter_stability_score=0.8,
        )

        _, artifact, _ = _run_real_mode_with_mocks(runner_cfg, wf_result)

        self.assertTrue(artifact["passed_min_bars"])
        self.assertTrue(artifact["passed_min_trades"])
        self.assertTrue(artifact["passed_max_drawdown"])
        self.assertTrue(artifact["passed_profit_factor"])
        self.assertTrue(artifact["passed_expectancy"])
        self.assertTrue(artifact["passed_cost_adjusted_edge"])
        self.assertTrue(artifact["passed_out_of_sample_check"])
        self.assertTrue(artifact["recommended_for_paper"])
        self.assertFalse(artifact["recommended_for_live"])


# ---------------------------------------------------------------------------
# C. Negative / fail-closed cases
# ---------------------------------------------------------------------------

class TestFailClosedCases(unittest.TestCase):

    def test_blocked_walkforward_status_keeps_artifact_fail_closed(self):
        mapped = _make_mapped()
        wf_result = _make_wf_result(status="failed", validation_metrics=None)

        merged, reasons = merge_walkforward_validation_into_mapped(
            mapped, [REASON_VALIDATION_METRICS_MISSING], wf_result
        )

        self.assertIsNone(merged["validation_profit_factor"])
        self.assertIsNone(merged["validation_trades"])
        self.assertIn(REASON_VALIDATION_METRICS_MISSING, reasons)
        self.assertIn(REASON_WALKFORWARD_VALIDATION_BLOCKED, reasons)

    def test_missing_validation_profit_factor_keeps_missing_reason(self):
        mapped = _make_mapped()
        wf_result = _make_wf_result(validation_profit_factor=None, validation_trades=45)

        merged, reasons = merge_walkforward_validation_into_mapped(
            mapped, [REASON_VALIDATION_METRICS_MISSING], wf_result
        )

        self.assertIsNone(merged["validation_profit_factor"])
        self.assertIsNone(merged["validation_trades"], "trades must not be merged when pf is absent")
        self.assertIn(REASON_VALIDATION_METRICS_MISSING, reasons)
        self.assertIn(REASON_WALKFORWARD_VALIDATION_BLOCKED, reasons)

    def test_missing_validation_trades_keeps_missing_reason(self):
        mapped = _make_mapped()
        wf_result = _make_wf_result(validation_profit_factor=1.4, validation_trades=None)

        merged, reasons = merge_walkforward_validation_into_mapped(
            mapped, [REASON_VALIDATION_METRICS_MISSING], wf_result
        )

        self.assertIsNone(merged["validation_profit_factor"], "pf must not be merged when trades is absent")
        self.assertIsNone(merged["validation_trades"])
        self.assertIn(REASON_VALIDATION_METRICS_MISSING, reasons)
        self.assertIn(REASON_WALKFORWARD_VALIDATION_BLOCKED, reasons)

    def test_unresolvable_bars_csv_blocks_without_invoking_walkforward(self):
        """No bars CSV under bars_root_dir → blocked reason; run_walkforward_entry never called."""
        entry = _make_queue_entry()
        mapped = _make_mapped()

        with tempfile.TemporaryDirectory() as tmpdir:
            empty_bars_root = str(Path(tmpdir) / "empty_bars")
            Path(empty_bars_root).mkdir(parents=True, exist_ok=True)
            bridge_cfg = BacktestBridgeConfig(
                mode="real",
                cli_binary=str(Path(tmpdir) / "fake-mqk-cli"),
                bars_root_dir=empty_bars_root,
                out_dir=str(Path(tmpdir) / "out"),
            )
            Path(bridge_cfg.cli_binary).write_text("", encoding="utf-8")

            runner_cfg = BacktestRunnerConfig(mode="real", enable_walkforward_validation=True)

            with patch(
                "mqk_research.scanner.walkforward_runner.run_walkforward_entry"
            ) as mock_wf:
                merged, reasons = _run_and_merge_walkforward_validation(
                    entry, mapped, [REASON_VALIDATION_METRICS_MISSING], runner_cfg, bridge_cfg
                )

        mock_wf.assert_not_called()
        self.assertEqual(merged, mapped)
        self.assertIn(REASON_WALKFORWARD_VALIDATION_BLOCKED, reasons)
        self.assertIn(REASON_VALIDATION_METRICS_MISSING, reasons)

    def test_low_validation_profit_factor_fails_out_of_sample_gate(self):
        runner_cfg = BacktestRunnerConfig(mode="real", enable_walkforward_validation=True)
        wf_result = _make_wf_result(
            validation_profit_factor=0.8,  # below default min_validation_profit_factor=1.1
            validation_trades=45, sample_quality=0.9, parameter_stability_score=0.8,
        )

        _, artifact, _ = _run_real_mode_with_mocks(runner_cfg, wf_result)

        self.assertNotIn(REASON_VALIDATION_METRICS_MISSING, artifact["failure_reasons"],
                         "metrics were present — only their values were inadequate")
        self.assertFalse(artifact["passed_out_of_sample_check"])
        self.assertFalse(artifact["recommended_for_paper"])
        self.assertFalse(artifact["recommended_for_live"])

    def test_low_validation_trades_fails_out_of_sample_gate(self):
        runner_cfg = BacktestRunnerConfig(mode="real", enable_walkforward_validation=True)
        wf_result = _make_wf_result(
            validation_profit_factor=1.4,
            validation_trades=3,  # below default min_validation_trades=10
            sample_quality=0.9, parameter_stability_score=0.8,
        )

        _, artifact, _ = _run_real_mode_with_mocks(runner_cfg, wf_result)

        self.assertFalse(artifact["passed_out_of_sample_check"])
        self.assertFalse(artifact["recommended_for_paper"])

    def test_low_sample_quality_fails_gate_and_blocks_paper_recommendation(self):
        runner_cfg = BacktestRunnerConfig(mode="real", enable_walkforward_validation=True)
        wf_result = _make_wf_result(
            validation_profit_factor=1.4, validation_trades=45,
            sample_quality=0.2,  # below default min_sample_quality=0.5
            parameter_stability_score=0.8,
        )

        _, artifact, _ = _run_real_mode_with_mocks(runner_cfg, wf_result)

        self.assertEqual(artifact["sample_quality"], 0.2)
        self.assertTrue(artifact["passed_out_of_sample_check"])
        self.assertFalse(artifact["recommended_for_paper"],
                         "low sample_quality must block recommended_for_paper despite passing oos gate")
        self.assertFalse(artifact["recommended_for_live"])

    def test_low_parameter_stability_score_fails_gate_and_blocks_paper_recommendation(self):
        runner_cfg = BacktestRunnerConfig(mode="real", enable_walkforward_validation=True)
        wf_result = _make_wf_result(
            validation_profit_factor=1.4, validation_trades=45,
            sample_quality=0.9,
            parameter_stability_score=0.3,  # below default min_parameter_stability_score=0.7
        )

        _, artifact, _ = _run_real_mode_with_mocks(runner_cfg, wf_result)

        self.assertEqual(artifact["parameter_stability_score"], 0.3)
        self.assertTrue(artifact["passed_out_of_sample_check"])
        self.assertFalse(artifact["recommended_for_paper"],
                         "low parameter_stability_score must block recommended_for_paper")
        self.assertFalse(artifact["recommended_for_live"])

    def test_large_single_trade_dependency_still_fails_despite_successful_walkforward_merge(self):
        """
        largest_trade_profit_fraction comes from metrics.json (bridge), not
        walk-forward — proves the integration does not bypass that gate even
        when validation metrics are otherwise fully populated and passing.
        """
        gate_cfg_artifact = {
            "schema_version": "strategy-fit-v1",
            "status": STATUS_COMPLETE,
            "bars_used": 300,
            "trades": 55,
            "win_rate": 0.60,
            "profit_factor": 1.5,
            "expectancy_bps": 1000.0,
            "max_drawdown_bps": 500.0,
            "net_expectancy_after_cost_bps": 994.4,
            "validation_profit_factor": 1.4,
            "validation_trades": 45,
            "sample_quality": 0.9,
            "parameter_stability_score": 0.8,
            "largest_trade_profit_fraction": 0.45,  # above default max=0.30
            "failure_reasons": [],
            "recommended_for_live": False,
        }
        updated = apply_backtest_gates(gate_cfg_artifact)

        self.assertTrue(updated["passed_out_of_sample_check"])
        self.assertIn("single_trade_dependency_failed", updated["failure_reasons"])
        self.assertFalse(updated["recommended_for_paper"])
        self.assertFalse(updated["recommended_for_live"])


# ---------------------------------------------------------------------------
# D. Safety
# ---------------------------------------------------------------------------

class TestSafetyBoundaries(unittest.TestCase):

    def test_no_forbidden_imports_in_touched_modules(self):
        import mqk_research.scanner.backtest_runner as runner_mod
        import mqk_research.scanner.walkforward_runner as wf_mod
        import mqk_research.scanner.backtest_bridge as bridge_mod

        # NOTE: bare "daemon" is deliberately excluded — the modules' own
        # safety-describing docstrings legitimately say things like "No
        # broker, OMS, daemon, or production DB access", which would false-
        # positive a substring check. Matches the established convention in
        # test_scanner_backtest_runner.py::TestNoForbiddenImports.
        forbidden = [
            "import requests", "import urllib", "import http.client",
            "import aiohttp", "import psycopg", "import sqlalchemy",
            "submit_order", "broker_adapter", "BrokerGateway",
            "oms_outbox", "oms_inbox", "/v2/orders",
        ]
        for mod in (runner_mod, wf_mod, bridge_mod):
            source = Path(mod.__file__).read_text(encoding="utf-8")
            for token in forbidden:
                self.assertNotIn(token, source,
                                 msg=f"forbidden token {token!r} found in {mod.__name__}")

    def test_subprocess_not_at_module_level_in_touched_modules(self):
        import mqk_research.scanner.backtest_runner as runner_mod
        import mqk_research.scanner.walkforward_runner as wf_mod
        import mqk_research.scanner.backtest_bridge as bridge_mod

        for mod in (runner_mod, wf_mod, bridge_mod):
            self.assertFalse(hasattr(mod, "subprocess"),
                             f"subprocess must not be imported at module level in {mod.__name__}")

    def test_enabling_flag_with_dry_run_walkforward_config_never_invokes_subprocess(self):
        """
        enable_walkforward_validation=True alone must not authorize subprocess
        execution — walkforward_config.mode stays "dry_run" by default, so the
        walk-forward run plans splits but never shells out, and the merge sees
        no usable aggregate (fail-closed).
        """
        runner_cfg = BacktestRunnerConfig(
            mode="real",
            enable_walkforward_validation=True,
            walkforward_config=WalkForwardRunnerConfig(),  # default mode="dry_run"
        )
        entry = _make_queue_entry()
        queue = _make_queue_artifact([entry])
        metrics = _make_metrics_dict()

        with tempfile.TemporaryDirectory() as tmpdir:
            bars_root = str(Path(tmpdir) / "bars")
            Path(bars_root).mkdir(parents=True, exist_ok=True)
            bars_csv = Path(bars_root) / "AAPL_5m.csv"
            bars_csv.write_text(
                "symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n"
                "AAPL,1000,99900000,100150000,99850000,100000000,600,1\n"
                "AAPL,1300,100087500,100337500,100037500,100187500,605,1\n",
                encoding="utf-8",
            )
            bridge_cfg = BacktestBridgeConfig(
                mode="real",
                cli_binary=str(Path(tmpdir) / "fake-mqk-cli"),
                bars_root_dir=bars_root,
                out_dir=str(Path(tmpdir) / "out"),
            )
            Path(bridge_cfg.cli_binary).write_text("", encoding="utf-8")
            artifact_dir = str(Path(tmpdir) / "strategy_fit")

            with patch(
                "mqk_research.scanner.backtest_bridge.run_backtest_for_entry",
                return_value=(metrics, []),
            ), patch("subprocess.run") as mock_subproc_run:
                run_backtest_queue(
                    queue=queue,
                    config=runner_cfg,
                    output_dir=artifact_dir,
                    generated_at_utc="2026-06-07T00:00:00+00:00",
                    bridge_config=bridge_cfg,
                )

            written = list(Path(artifact_dir).glob("*.json"))
            self.assertEqual(len(written), 1)
            artifact = _load_json(written[0])

        mock_subproc_run.assert_not_called()
        self.assertIn(REASON_VALIDATION_METRICS_MISSING, artifact["failure_reasons"])
        self.assertIn(REASON_WALKFORWARD_VALIDATION_BLOCKED, artifact["failure_reasons"])
        self.assertIsNone(artifact["validation_profit_factor"])
        self.assertIsNone(artifact["validation_trades"])

    def test_dry_run_backtest_mode_never_calls_run_walkforward_entry(self):
        runner_cfg = BacktestRunnerConfig(mode="dry_run", enable_walkforward_validation=True)
        queue = _make_queue_artifact()

        with patch(
            "mqk_research.scanner.walkforward_runner.run_walkforward_entry"
        ) as mock_wf, patch("subprocess.run") as mock_subproc_run, tempfile.TemporaryDirectory() as tmpdir:
            run_backtest_queue(
                queue=queue,
                config=runner_cfg,
                output_dir=tmpdir,
                generated_at_utc="2026-06-07T00:00:00+00:00",
            )

        mock_wf.assert_not_called()
        mock_subproc_run.assert_not_called()

    def test_recommended_for_live_always_false_across_all_branches(self):
        """Sweep status/value combinations through the merge — recommended_for_live never True."""
        from mqk_research.scanner.backtest_gates import apply_backtest_gates

        combos = [
            _make_wf_result(),
            _make_wf_result(status="failed", validation_metrics=None),
            _make_wf_result(validation_profit_factor=None),
            _make_wf_result(validation_profit_factor=0.5, validation_trades=2),
            _make_wf_result(sample_quality=0.1, parameter_stability_score=0.1),
        ]
        for wf_result in combos:
            mapped, reasons = merge_walkforward_validation_into_mapped(
                _make_mapped(), [REASON_VALIDATION_METRICS_MISSING], wf_result
            )
            result = StrategyFitResult(**mapped)
            self.assertFalse(result is None)
            artifact = {
                "schema_version": "strategy-fit-v1",
                "status": STATUS_COMPLETE,
                "bars_used": mapped["bars_used"],
                "trades": mapped["trades"],
                "profit_factor": mapped["profit_factor"],
                "expectancy_bps": mapped["expectancy_bps"],
                "max_drawdown_bps": mapped["max_drawdown_bps"],
                "net_expectancy_after_cost_bps": mapped["net_expectancy_after_cost_bps"],
                "validation_profit_factor": mapped["validation_profit_factor"],
                "validation_trades": mapped["validation_trades"],
                "sample_quality": mapped["sample_quality"],
                "parameter_stability_score": mapped["parameter_stability_score"],
                "largest_trade_profit_fraction": mapped["largest_trade_profit_fraction"],
                "failure_reasons": reasons,
                "recommended_for_live": False,
            }
            updated = apply_backtest_gates(artifact)
            self.assertFalse(updated["recommended_for_live"])


if __name__ == "__main__":
    unittest.main()
