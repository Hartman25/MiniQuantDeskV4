"""
WALKFORWARD-RUNNER-01 — Tests for walk-forward backtest runner.

20 required tests covering dry-run safety, CSV splitting, fail-closed behavior,
conservative aggregation, and gate integration.

No subprocess, no broker, no OMS, no network, no DB.
subprocess mocked in real-mode tests (never actually executed).
"""
from __future__ import annotations

import json
import tempfile
import unittest
from datetime import date, datetime, timezone, timedelta
from pathlib import Path
from typing import Any
from unittest.mock import MagicMock, patch

from mqk_research.scanner.walkforward import WalkForwardSplit
from mqk_research.scanner.walkforward_runner import (
    REASON_BARS_FILE_MISSING,
    REASON_NO_SPLITS,
    REASON_EMPTY_SPLIT_CSV,
    WalkForwardRunnerConfig,
    WalkForwardSplitResult,
    WalkForwardAggregate,
    build_walkforward_plan,
    write_split_csv,
    aggregate_walkforward_results,
    map_walkforward_to_validation_metrics,
    run_walkforward_entry,
    _date_to_epoch_secs,
    _split_csv_path,
    _run_dir_for_entry,
)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

def _make_queue_entry(
    queue_id: str = "wf-test-001",
    symbol: str = "AAPL",
    strategy_id: str = "intraday_scalper",
    timeframe: str = "5m",
    start_date: str | None = None,
    end_date: str | None = None,
) -> dict[str, Any]:
    entry: dict[str, Any] = {
        "queue_id": queue_id,
        "symbol": symbol,
        "strategy_id": strategy_id,
        "timeframe": timeframe,
        "regime_label": "trending_up",
    }
    if start_date:
        entry["start_date"] = start_date
    if end_date:
        entry["end_date"] = end_date
    return entry


# Jan 10, 2025 UTC midnight epoch seconds
_JAN_10_EPOCH = int(datetime(2025, 1, 10, tzinfo=timezone.utc).timestamp())
# Jan 11
_JAN_11_EPOCH = int(datetime(2025, 1, 11, tzinfo=timezone.utc).timestamp())
# Jan 19
_JAN_19_EPOCH = int(datetime(2025, 1, 19, tzinfo=timezone.utc).timestamp())
# Jan 20 (exclusive boundary)
_JAN_20_EPOCH = int(datetime(2025, 1, 20, tzinfo=timezone.utc).timestamp())
# Jan 1 (before the window)
_JAN_1_EPOCH = int(datetime(2025, 1, 1, tzinfo=timezone.utc).timestamp())

_CSV_HEADER = "symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete"


def _make_csv_content(rows: list[tuple]) -> str:
    """Build CSV string from (symbol, end_ts, ...) tuples with default OHLCV."""
    lines = [_CSV_HEADER]
    for sym, ts in rows:
        lines.append(f"{sym},{ts},150000000,155000000,148000000,152000000,1000,true")
    return "\n".join(lines) + "\n"


def _make_split(
    fold_index: int = 0,
    train_start: date = date(2025, 1, 1),
    train_end: date = date(2025, 1, 9),
    validation_start: date = date(2025, 1, 10),
    validation_end: date = date(2025, 1, 19),
) -> WalkForwardSplit:
    return WalkForwardSplit(
        fold_index=fold_index,
        train_start=train_start,
        train_end=train_end,
        validation_start=validation_start,
        validation_end=validation_end,
    )


def _make_split_result(
    fold_index: int = 0,
    status: str = "complete",
    profit_factor: float | None = 1.5,
    trade_count: int | None = 20,
    win_rate_pct: float | None = 60.0,
) -> WalkForwardSplitResult:
    metrics: dict[str, Any] | None = None
    if status == "complete":
        metrics = {
            "schema_version": 1,
            "profit_factor": profit_factor,
            "trade_count": trade_count,
            "win_rate_pct": win_rate_pct,
        }
    return WalkForwardSplitResult(
        fold_index=fold_index,
        split=_make_split(fold_index),
        split_csv_path=f"/tmp/split_{fold_index:03d}_validation.csv",
        status=status,
        metrics=metrics,
        failure_reasons=[] if status == "complete" else ["dry_run_no_subprocess"],
    )


# ---------------------------------------------------------------------------
# Test 1: Dry-run default does not invoke subprocess
# ---------------------------------------------------------------------------

class TestDryRunNoSubprocess(unittest.TestCase):

    def test_dry_run_does_not_execute_subprocess(self):
        """mode=dry_run must never invoke subprocess, even if a bars CSV exists."""
        entry = _make_queue_entry(
            start_date="2025-01-01", end_date="2026-12-31"
        )
        cfg = WalkForwardRunnerConfig(mode="dry_run", train_days=70, validation_days=30, step_days=30)

        with tempfile.TemporaryDirectory() as tmp:
            csv_path = Path(tmp) / "AAPL_5m.csv"
            # Write a minimal CSV spanning the full date range
            rows = []
            base = _date_to_epoch_secs(date(2025, 1, 2))
            for i in range(400):
                rows.append(("AAPL", base + i * 86400))
            csv_path.write_text(_make_csv_content(rows), encoding="utf-8")

            cfg.output_root = tmp

            with patch("subprocess.run") as mock_run:
                result = run_walkforward_entry(entry, str(csv_path), cfg)
                mock_run.assert_not_called()

        self.assertEqual(result.status, "dry_run_planned")
        self.assertFalse(result.recommended_for_live)


# ---------------------------------------------------------------------------
# Test 2: Missing bars CSV fails closed
# ---------------------------------------------------------------------------

class TestMissingBarsCsvFailsClosed(unittest.TestCase):

    def test_missing_bars_csv_returns_failed(self):
        """Missing bars CSV must fail closed — never proceed to split planning."""
        entry = _make_queue_entry()
        cfg = WalkForwardRunnerConfig(mode="dry_run")

        result = run_walkforward_entry(entry, "/nonexistent/path/bars.csv", cfg)

        self.assertEqual(result.status, "failed")
        self.assertIn(REASON_BARS_FILE_MISSING, result.failure_reasons)
        self.assertFalse(result.recommended_for_live)


# ---------------------------------------------------------------------------
# Test 3: Split CSV writer filters rows by timestamp
# ---------------------------------------------------------------------------

class TestSplitCsvFiltersByTimestamp(unittest.TestCase):

    def test_write_split_csv_filters_by_validation_window(self):
        """Only rows within [validation_start, validation_end] appear in split CSV."""
        split = _make_split(
            validation_start=date(2025, 1, 10),
            validation_end=date(2025, 1, 19),
        )
        rows = [
            ("AAPL", _JAN_1_EPOCH),    # before window — excluded
            ("AAPL", _JAN_10_EPOCH),   # start of window — included
            ("AAPL", _JAN_11_EPOCH),   # inside window — included
            ("AAPL", _JAN_19_EPOCH),   # last day midnight — included
            ("AAPL", _JAN_20_EPOCH),   # exclusive boundary — excluded
        ]
        with tempfile.TemporaryDirectory() as tmp:
            in_csv = Path(tmp) / "bars.csv"
            out_csv = Path(tmp) / "split_val.csv"
            in_csv.write_text(_make_csv_content(rows), encoding="utf-8")

            count = write_split_csv(str(in_csv), split, str(out_csv))

        self.assertEqual(count, 3)


# ---------------------------------------------------------------------------
# Test 4: Split CSV writer preserves header
# ---------------------------------------------------------------------------

class TestSplitCsvPreservesHeader(unittest.TestCase):

    def test_header_line_identical_in_output(self):
        """Header in output CSV must be byte-identical to input header."""
        split = _make_split(
            validation_start=date(2025, 1, 10),
            validation_end=date(2025, 1, 19),
        )
        rows = [("AAPL", _JAN_10_EPOCH)]
        with tempfile.TemporaryDirectory() as tmp:
            in_csv = Path(tmp) / "bars.csv"
            out_csv = Path(tmp) / "split_val.csv"
            in_csv.write_text(_make_csv_content(rows), encoding="utf-8")

            write_split_csv(str(in_csv), split, str(out_csv))
            out_lines = out_csv.read_text(encoding="utf-8").splitlines()

        self.assertEqual(out_lines[0], _CSV_HEADER)

    def test_header_written_even_for_empty_result(self):
        """Header is written even when no rows match the window."""
        split = _make_split(
            validation_start=date(2025, 2, 1),   # no bars exist in Feb
            validation_end=date(2025, 2, 28),
        )
        rows = [("AAPL", _JAN_10_EPOCH)]
        with tempfile.TemporaryDirectory() as tmp:
            in_csv = Path(tmp) / "bars.csv"
            out_csv = Path(tmp) / "split_val.csv"
            in_csv.write_text(_make_csv_content(rows), encoding="utf-8")

            count = write_split_csv(str(in_csv), split, str(out_csv))
            out_lines = out_csv.read_text(encoding="utf-8").splitlines()

        self.assertEqual(count, 0)
        self.assertEqual(out_lines[0], _CSV_HEADER)


# ---------------------------------------------------------------------------
# Test 5: Empty split fails closed
# ---------------------------------------------------------------------------

class TestEmptySplitFailsClosed(unittest.TestCase):

    def test_empty_split_csv_produces_empty_split_status(self):
        """A split with zero rows must produce status=empty_split (fail-closed)."""
        entry = _make_queue_entry(
            start_date="2025-01-01", end_date="2026-12-31"
        )
        # Bars exist only in Jan 2025; splits will include later months that have no bars
        rows = [("AAPL", _JAN_10_EPOCH), ("AAPL", _JAN_11_EPOCH)]
        with tempfile.TemporaryDirectory() as tmp:
            csv_path = Path(tmp) / "AAPL_5m.csv"
            csv_path.write_text(_make_csv_content(rows), encoding="utf-8")
            cfg = WalkForwardRunnerConfig(
                mode="dry_run",
                train_days=70,
                validation_days=30,
                step_days=30,
                output_root=tmp,
            )
            result = run_walkforward_entry(entry, str(csv_path), cfg)

        # At least some splits should be empty_split
        statuses = {sr.status for sr in result.split_results}
        self.assertIn("empty_split", statuses)

        for sr in result.split_results:
            if sr.status == "empty_split":
                self.assertIn(REASON_EMPTY_SPLIT_CSV, sr.failure_reasons)


# ---------------------------------------------------------------------------
# Test 6: Multiple splits produce deterministic output paths
# ---------------------------------------------------------------------------

class TestDeterministicOutputPaths(unittest.TestCase):

    def test_split_paths_are_deterministic_and_unique(self):
        """Same queue entry + config must produce the same split CSV paths."""
        entry = _make_queue_entry(queue_id="det-test-007")
        cfg = WalkForwardRunnerConfig(output_root="exports/test_wf")

        run_dir = _run_dir_for_entry(entry, cfg.output_root)
        path_a = _split_csv_path(run_dir, 0)
        path_b = _split_csv_path(run_dir, 0)
        path_c = _split_csv_path(run_dir, 1)

        self.assertEqual(path_a, path_b)
        self.assertNotEqual(path_a, path_c)

    def test_different_queue_ids_produce_different_dirs(self):
        """Different queue_ids must not share run directories."""
        cfg = WalkForwardRunnerConfig(output_root="exports/test_wf")
        dir_a = _run_dir_for_entry({"queue_id": "qa-001"}, cfg.output_root)
        dir_b = _run_dir_for_entry({"queue_id": "qa-002"}, cfg.output_root)
        self.assertNotEqual(dir_a, dir_b)


# ---------------------------------------------------------------------------
# Test 7: Real mode uses mocked subprocess only
# ---------------------------------------------------------------------------

class TestRealModeUsesOnlyMockedSubprocess(unittest.TestCase):

    def test_real_mode_calls_subprocess_run_with_cli_binary(self):
        """Real mode must call subprocess.run with the cli_binary as first arg."""
        entry = _make_queue_entry(
            start_date="2025-01-01", end_date="2026-06-01"
        )
        rows = []
        base = _date_to_epoch_secs(date(2025, 1, 2))
        for i in range(600):
            rows.append(("AAPL", base + i * 86400))

        bridge_cfg = MagicMock()
        bridge_cfg.cli_binary = "/fake/mqk"
        bridge_cfg.initial_cash_micros = 100_000_000_000
        bridge_cfg.target_qty = 1
        bridge_cfg.integrity_stale_threshold_ticks = 172_800
        bridge_cfg.integrity_gap_tolerance_bars = 0

        fake_metrics = {
            "schema_version": 1,
            "profit_factor": 1.4,
            "trade_count": 15,
            "win_rate_pct": 55.0,
        }
        fake_metrics_json = json.dumps(fake_metrics)

        with tempfile.TemporaryDirectory() as tmp:
            csv_path = Path(tmp) / "AAPL_5m.csv"
            csv_path.write_text(_make_csv_content(rows), encoding="utf-8")

            cli_binary = Path(tmp) / "mqk"
            cli_binary.write_text("fake", encoding="utf-8")
            bridge_cfg.cli_binary = str(cli_binary)

            artifacts_dir = Path(tmp) / "fold_000_artifacts"
            artifacts_dir.mkdir(parents=True)
            (artifacts_dir / "metrics.json").write_text(fake_metrics_json, encoding="utf-8")

            fake_proc = MagicMock()
            fake_proc.returncode = 0
            fake_proc.stdout = f"artifacts_dir={artifacts_dir}\n"

            cfg = WalkForwardRunnerConfig(
                mode="real",
                train_days=70,
                validation_days=30,
                step_days=30,
                output_root=tmp,
            )

            with patch("subprocess.run", return_value=fake_proc) as mock_run:
                run_walkforward_entry(entry, str(csv_path), cfg, bridge_cfg)

            self.assertGreater(mock_run.call_count, 0)
            first_call_args = mock_run.call_args_list[0][0][0]
            self.assertEqual(first_call_args[0], str(cli_binary))


# ---------------------------------------------------------------------------
# Test 8: Conservative aggregation of multiple splits
# ---------------------------------------------------------------------------

class TestConservativeAggregation(unittest.TestCase):

    def test_validation_profit_factor_is_minimum(self):
        """validation_profit_factor must be the minimum PF, not average or best."""
        results = [
            _make_split_result(fold_index=0, profit_factor=2.0, trade_count=20),
            _make_split_result(fold_index=1, profit_factor=1.2, trade_count=15),
            _make_split_result(fold_index=2, profit_factor=1.8, trade_count=18),
        ]
        cfg = WalkForwardRunnerConfig(min_validation_profit_factor=1.1, min_validation_trades=10)
        agg = aggregate_walkforward_results(results, cfg)

        self.assertAlmostEqual(agg.validation_profit_factor, 1.2)


# ---------------------------------------------------------------------------
# Test 9: Worst split PF recorded
# ---------------------------------------------------------------------------

class TestWorstSplitPfRecorded(unittest.TestCase):

    def test_worst_split_profit_factor_equals_minimum(self):
        """worst_split_profit_factor must equal the minimum profit factor."""
        results = [
            _make_split_result(fold_index=0, profit_factor=1.9, trade_count=20),
            _make_split_result(fold_index=1, profit_factor=0.8, trade_count=12),   # worst
        ]
        cfg = WalkForwardRunnerConfig(min_validation_profit_factor=1.1, min_validation_trades=10)
        agg = aggregate_walkforward_results(results, cfg)

        self.assertAlmostEqual(agg.worst_split_profit_factor, 0.8)
        self.assertAlmostEqual(agg.validation_profit_factor, 0.8)


# ---------------------------------------------------------------------------
# Test 10: validation_trades sums across splits
# ---------------------------------------------------------------------------

class TestValidationTradesSumsAcrossSplits(unittest.TestCase):

    def test_validation_trades_is_sum(self):
        """validation_trades must be the sum of trade_count across splits."""
        results = [
            _make_split_result(fold_index=0, trade_count=12),
            _make_split_result(fold_index=1, trade_count=18),
            _make_split_result(fold_index=2, trade_count=5),
        ]
        agg = aggregate_walkforward_results(results)
        self.assertEqual(agg.validation_trades, 35)


# ---------------------------------------------------------------------------
# Test 11: sample_quality derived from validation trades
# ---------------------------------------------------------------------------

class TestSampleQualityFromValidationTrades(unittest.TestCase):

    def test_sample_quality_is_clamped_fraction(self):
        """sample_quality = clamp(validation_trades / (min_trades * splits), 0, 1)."""
        results = [
            _make_split_result(fold_index=0, trade_count=10),
            _make_split_result(fold_index=1, trade_count=10),
        ]
        cfg = WalkForwardRunnerConfig(min_validation_trades=10)
        agg = aggregate_walkforward_results(results, cfg)
        # 20 trades / (10 * 2) = 1.0
        self.assertAlmostEqual(agg.sample_quality, 1.0)

    def test_sample_quality_below_1_when_trades_low(self):
        """sample_quality < 1 when trades < min_required."""
        results = [
            _make_split_result(fold_index=0, trade_count=5),
            _make_split_result(fold_index=1, trade_count=5),
        ]
        cfg = WalkForwardRunnerConfig(min_validation_trades=10)
        agg = aggregate_walkforward_results(results, cfg)
        # 10 trades / (10 * 2) = 0.5
        self.assertAlmostEqual(agg.sample_quality, 0.5)

    def test_sample_quality_never_exceeds_1(self):
        """sample_quality must not exceed 1.0 even with many trades."""
        results = [
            _make_split_result(fold_index=0, trade_count=9999),
        ]
        agg = aggregate_walkforward_results(results)
        self.assertLessEqual(agg.sample_quality, 1.0)


# ---------------------------------------------------------------------------
# Test 12: parameter_stability_score derived from passed split ratio
# ---------------------------------------------------------------------------

class TestParameterStabilityScore(unittest.TestCase):

    def test_stability_score_is_fraction_of_passed_splits(self):
        """parameter_stability_score = passed / total."""
        cfg = WalkForwardRunnerConfig(min_validation_profit_factor=1.1, min_validation_trades=10)
        results = [
            _make_split_result(fold_index=0, profit_factor=1.5, trade_count=15),  # passes
            _make_split_result(fold_index=1, profit_factor=0.9, trade_count=15),  # fails PF
            _make_split_result(fold_index=2, profit_factor=1.2, trade_count=5),   # fails trades
            _make_split_result(fold_index=3, profit_factor=1.3, trade_count=12),  # passes
        ]
        agg = aggregate_walkforward_results(results, cfg)
        # 2 passed out of 4
        self.assertAlmostEqual(agg.parameter_stability_score, 0.5)

    def test_stability_zero_for_all_failed(self):
        """parameter_stability_score = 0.0 when no splits pass."""
        cfg = WalkForwardRunnerConfig(min_validation_profit_factor=2.0, min_validation_trades=100)
        results = [
            _make_split_result(fold_index=0, profit_factor=1.5, trade_count=15),
        ]
        agg = aggregate_walkforward_results(results, cfg)
        self.assertAlmostEqual(agg.parameter_stability_score, 0.0)


# ---------------------------------------------------------------------------
# Test 13: Missing split metrics fails closed
# ---------------------------------------------------------------------------

class TestMissingSplitMetricsFailsClosed(unittest.TestCase):

    def test_all_splits_have_metrics_false_when_any_missing(self):
        """all_splits_have_metrics=False when any split has status!=complete."""
        results = [
            _make_split_result(fold_index=0, status="complete"),
            _make_split_result(fold_index=1, status="failed"),
        ]
        agg = aggregate_walkforward_results(results)
        self.assertFalse(agg.all_splits_have_metrics)

    def test_run_result_fails_when_real_mode_has_missing_metrics(self):
        """run_walkforward_entry status=failed in real mode when metrics absent."""
        entry = _make_queue_entry(start_date="2025-01-01", end_date="2026-06-01")
        rows = []
        base = _date_to_epoch_secs(date(2025, 1, 2))
        for i in range(600):
            rows.append(("AAPL", base + i * 86400))

        bridge_cfg = MagicMock()
        bridge_cfg.initial_cash_micros = 100_000_000_000
        bridge_cfg.target_qty = 1
        bridge_cfg.integrity_stale_threshold_ticks = 172_800
        bridge_cfg.integrity_gap_tolerance_bars = 0

        with tempfile.TemporaryDirectory() as tmp:
            csv_path = Path(tmp) / "AAPL_5m.csv"
            csv_path.write_text(_make_csv_content(rows), encoding="utf-8")

            cli_binary = Path(tmp) / "mqk"
            cli_binary.write_text("fake", encoding="utf-8")
            bridge_cfg.cli_binary = str(cli_binary)

            # Subprocess fails → no metrics
            fake_proc = MagicMock()
            fake_proc.returncode = 1
            fake_proc.stdout = ""

            cfg = WalkForwardRunnerConfig(
                mode="real",
                train_days=70,
                validation_days=30,
                step_days=30,
                output_root=tmp,
            )
            with patch("subprocess.run", return_value=fake_proc):
                result = run_walkforward_entry(entry, str(csv_path), cfg, bridge_cfg)

        self.assertEqual(result.status, "failed")
        self.assertFalse(result.recommended_for_live)


# ---------------------------------------------------------------------------
# Test 14: Failed subprocess becomes failed result (not crash)
# ---------------------------------------------------------------------------

class TestFailedSubprocessNoException(unittest.TestCase):

    def test_subprocess_exception_does_not_crash_runner(self):
        """subprocess.run raising OSError must produce a failed split, not crash."""
        entry = _make_queue_entry(start_date="2025-01-01", end_date="2026-06-01")
        rows = [("AAPL", _JAN_10_EPOCH + i * 86400) for i in range(400)]

        bridge_cfg = MagicMock()
        bridge_cfg.initial_cash_micros = 100_000_000_000
        bridge_cfg.target_qty = 1
        bridge_cfg.integrity_stale_threshold_ticks = 172_800
        bridge_cfg.integrity_gap_tolerance_bars = 0

        with tempfile.TemporaryDirectory() as tmp:
            csv_path = Path(tmp) / "AAPL_5m.csv"
            csv_path.write_text(_make_csv_content(rows), encoding="utf-8")

            cli_binary = Path(tmp) / "mqk"
            cli_binary.write_text("fake", encoding="utf-8")
            bridge_cfg.cli_binary = str(cli_binary)

            cfg = WalkForwardRunnerConfig(
                mode="real", train_days=70, validation_days=30, step_days=30, output_root=tmp
            )
            with patch("subprocess.run", side_effect=OSError("no such file")):
                result = run_walkforward_entry(entry, str(csv_path), cfg, bridge_cfg)

        # Must not raise — result must reflect failure
        self.assertIsNotNone(result)
        failed_statuses = {sr.status for sr in result.split_results}
        self.assertTrue(failed_statuses.issubset({"failed", "empty_split", "blocked"}))
        self.assertFalse(result.recommended_for_live)


# ---------------------------------------------------------------------------
# Test 15: recommended_for_live always False
# ---------------------------------------------------------------------------

class TestRecommendedForLiveAlwaysFalse(unittest.TestCase):

    def test_runner_config_recommended_for_live_false(self):
        """WalkForwardRunnerConfig.recommended_for_live must always be False."""
        cfg = WalkForwardRunnerConfig(recommended_for_live=True)
        self.assertFalse(cfg.recommended_for_live)

    def test_aggregate_recommended_for_live_false(self):
        """WalkForwardAggregate.recommended_for_live must always be False."""
        results = [_make_split_result(profit_factor=5.0, trade_count=100)]
        agg = aggregate_walkforward_results(results)
        self.assertFalse(agg.recommended_for_live)

    def test_validation_metrics_recommended_for_live_false(self):
        """map_walkforward_to_validation_metrics must always return recommended_for_live=False."""
        agg = WalkForwardAggregate(
            split_count=2,
            passed_split_count=2,
            failed_split_count=0,
            validation_profit_factor=1.5,
            worst_split_profit_factor=1.5,
            median_split_profit_factor=1.5,
            validation_trades=30,
            validation_win_rate=0.6,
            sample_quality=1.0,
            parameter_stability_score=1.0,
            all_splits_have_metrics=True,
            recommended_for_live=True,   # attempt to set True
        )
        vm = map_walkforward_to_validation_metrics(agg)
        self.assertFalse(vm["recommended_for_live"])

    def test_run_result_recommended_for_live_false(self):
        """WalkForwardRunResult.recommended_for_live must always be False."""
        entry = _make_queue_entry()
        with tempfile.TemporaryDirectory() as tmp:
            cfg = WalkForwardRunnerConfig(mode="dry_run", output_root=tmp)
            result = run_walkforward_entry(entry, "/nonexistent.csv", cfg)
        self.assertFalse(result.recommended_for_live)


# ---------------------------------------------------------------------------
# Test 16: No broker/OMS/runtime/DB imports
# ---------------------------------------------------------------------------

class TestNoBrokerOmsRuntimeDbImports(unittest.TestCase):

    def _source(self) -> str:
        import mqk_research.scanner.walkforward_runner as mod
        return Path(mod.__file__).read_text(encoding="utf-8")

    def test_no_broker_references(self):
        """Module must not reference broker routes or OMS objects."""
        source = self._source()
        for forbidden in ["/v2/orders", "submit_order", "BrokerGateway",
                          "oms_outbox", "mqk_broker", "broker_adapter"]:
            self.assertNotIn(forbidden, source, msg=f"forbidden: {forbidden!r}")

    def test_no_daemon_runtime_imports(self):
        """Module must not import daemon or runtime modules."""
        source = self._source()
        for forbidden in ["mqk_daemon", "mqk_runtime", "execution_orchestrator",
                          "from mqk_execution", "import mqk_execution"]:
            self.assertNotIn(forbidden, source, msg=f"forbidden: {forbidden!r}")

    def test_no_db_imports(self):
        """Module must not import DB or network modules at top level."""
        source = self._source()
        for forbidden in ["psycopg", "sqlalchemy", "import asyncpg",
                          "from mqk_db", "import mqk_db"]:
            self.assertNotIn(forbidden, source, msg=f"forbidden: {forbidden!r}")


# ---------------------------------------------------------------------------
# Test 17: No network imports
# ---------------------------------------------------------------------------

class TestNoNetworkImports(unittest.TestCase):

    def test_no_network_library_imports(self):
        """Module must not import network libraries at any level."""
        import mqk_research.scanner.walkforward_runner as mod
        source = Path(mod.__file__).read_text(encoding="utf-8")
        for forbidden in ["import requests", "import urllib", "import aiohttp",
                          "import http.client", "import httpx"]:
            self.assertNotIn(forbidden, source, msg=f"forbidden: {forbidden!r}")

    def test_subprocess_not_at_module_level(self):
        """subprocess must not appear as a module-level import."""
        import mqk_research.scanner.walkforward_runner as mod
        self.assertFalse(hasattr(mod, "subprocess"))


# ---------------------------------------------------------------------------
# Test 18: Existing walkforward split planner tests still pass
# ---------------------------------------------------------------------------

class TestExistingWalkforwardPlannerStillPasses(unittest.TestCase):

    def test_build_date_splits_still_works(self):
        """build_date_splits must still produce correct splits after this patch."""
        from mqk_research.scanner.walkforward import WalkForwardConfig, build_date_splits
        cfg = WalkForwardConfig(train_days=70, validation_days=30, step_days=30)
        splits = build_date_splits(date(2025, 1, 1), date(2026, 12, 31), cfg)
        self.assertGreater(len(splits), 0)
        for s in splits:
            expected_vs = s.train_end + timedelta(days=1)
            self.assertEqual(s.validation_start, expected_vs)


# ---------------------------------------------------------------------------
# Test 19: Backtest bridge tests still pass (import guard)
# ---------------------------------------------------------------------------

class TestBacktestBridgeStillImportable(unittest.TestCase):

    def test_backtest_bridge_imports_without_error(self):
        """backtest_bridge must still be importable with no regressions."""
        from mqk_research.scanner.backtest_bridge import (
            BacktestBridgeConfig,
            build_mqk_backtest_command,
            parse_mqk_metrics_json,
            map_metrics_to_strategy_fit,
        )
        cfg = BacktestBridgeConfig()
        self.assertFalse(cfg.recommended_for_live)


# ---------------------------------------------------------------------------
# Test 20: Gate can pass out-of-sample using aggregated validation metrics
# ---------------------------------------------------------------------------

class TestGatePassesWithWalkForwardValidationMetrics(unittest.TestCase):

    def test_out_of_sample_gate_passes_when_validation_metrics_meet_threshold(self):
        """apply_backtest_gates should pass out-of-sample check when walk-forward
        validation_profit_factor and validation_trades meet gate thresholds."""
        from mqk_research.scanner.backtest_gates import apply_backtest_gates, BacktestGateConfig

        # Build an aggregate that passes
        agg = WalkForwardAggregate(
            split_count=3,
            passed_split_count=3,
            failed_split_count=0,
            validation_profit_factor=1.4,
            worst_split_profit_factor=1.4,
            median_split_profit_factor=1.5,
            validation_trades=45,
            validation_win_rate=0.58,
            sample_quality=1.0,
            parameter_stability_score=1.0,
            all_splits_have_metrics=True,
        )
        vm = map_walkforward_to_validation_metrics(agg)

        # Build a strategy-fit artifact with all required fields + validation metrics
        artifact = {
            "schema_version": "strategy-fit-v1",
            "status": "complete",
            "bars_used": 500,
            "trades": 60,
            "win_rate": 0.58,
            "profit_factor": 1.6,
            "expectancy_bps": 10.0,
            "avg_trade_bps": None,
            "max_drawdown_bps": 300.0,
            "sharpe": 0.8,
            "sortino": 1.0,
            "exposure_time_pct": 0.15,
            "net_expectancy_after_cost_bps": 8.0,
            "sample_quality": vm["sample_quality"],
            "parameter_stability_score": vm["parameter_stability_score"],
            "validation_profit_factor": vm["validation_profit_factor"],
            "validation_trades": vm["validation_trades"],
            "largest_trade_profit_fraction": 0.10,
            "failure_reasons": [],
            "recommended_for_live": False,
        }

        gate_cfg = BacktestGateConfig(
            min_validation_profit_factor=1.1,
            min_validation_trades=10,
        )
        updated = apply_backtest_gates(artifact, gate_cfg)

        self.assertTrue(updated["passed_out_of_sample_check"])
        self.assertFalse(updated["recommended_for_live"])

    def test_out_of_sample_gate_fails_when_validation_profit_factor_absent(self):
        """Gate fails closed when validation_profit_factor is None."""
        from mqk_research.scanner.backtest_gates import apply_backtest_gates

        artifact = {
            "schema_version": "strategy-fit-v1",
            "status": "complete",
            "bars_used": 500,
            "trades": 60,
            "profit_factor": 1.6,
            "expectancy_bps": 10.0,
            "max_drawdown_bps": 300.0,
            "net_expectancy_after_cost_bps": 8.0,
            "validation_profit_factor": None,   # absent
            "validation_trades": 30,
            "sample_quality": 1.0,
            "parameter_stability_score": 1.0,
            "largest_trade_profit_fraction": 0.10,
            "failure_reasons": [],
            "recommended_for_live": False,
        }
        updated = apply_backtest_gates(artifact)
        self.assertFalse(updated["passed_out_of_sample_check"])
        self.assertFalse(updated["recommended_for_live"])


if __name__ == "__main__":
    unittest.main()
