"""
BACKTEST-RUNNER-01 — Unit tests for MAIN scanner backtest runner.
"""
from __future__ import annotations

import importlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

from mqk_research.scanner.backtest_runner import (
    SCHEMA_VERSION,
    QUEUE_SCHEMA_VERSION,
    STATUS_BLOCKED,
    FAILURE_REASON_NO_INTERFACE,
    BacktestRunnerConfig,
    StrategyFitResult,
    BacktestRunResult,
    load_backtest_queue,
    build_strategy_fit_artifact,
    write_strategy_fit_artifact,
    run_backtest_queue,
    _artifact_filename,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_queue_entry(
    queue_id: str = "bq-abc123",
    symbol: str = "AAPL",
    strategy_id: str = "intraday_scalper",
    timeframe: str = "5m",
    regime_label: str = "trending_up",
    source_rank: int = 1,
) -> dict:
    return {
        "queue_id": queue_id,
        "symbol": symbol,
        "strategy_id": strategy_id,
        "timeframe": timeframe,
        "regime_label": regime_label,
        "source_rank": source_rank,
        "source_total_score": 0.75,
        "source_candidate_artifact": None,
        "source_ranked_export": None,
        "ambiguity_policy": "CONSERVATIVE_WORST_CASE",
        "stress_profile": "slippage_x2",
        "recommended_for_live": False,
        "notes": "test entry",
    }


def _make_queue_artifact(entries: list[dict] = None) -> dict:
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


# ---------------------------------------------------------------------------
# Test 1: loads backtest-queue-v1
# ---------------------------------------------------------------------------

class TestLoadBacktestQueue(unittest.TestCase):

    def test_loads_valid_queue_file(self):
        artifact = _make_queue_artifact()
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".json", delete=False, encoding="utf-8"
        ) as f:
            json.dump(artifact, f)
            fpath = f.name
        loaded = load_backtest_queue(fpath)
        self.assertEqual(loaded["schema_version"], "backtest-queue-v1")

    def test_rejects_wrong_schema_version(self):
        artifact = _make_queue_artifact()
        artifact["schema_version"] = "ranked-candidates-v1"
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".json", delete=False, encoding="utf-8"
        ) as f:
            json.dump(artifact, f)
            fpath = f.name
        with self.assertRaises(ValueError) as ctx:
            load_backtest_queue(fpath)
        self.assertIn("schema_version", str(ctx.exception))

    def test_rejects_missing_file(self):
        with self.assertRaises(ValueError):
            load_backtest_queue("/nonexistent/path/queue.json")

    def test_rejects_invalid_json(self):
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".json", delete=False, encoding="utf-8"
        ) as f:
            f.write("not valid json{{{")
            fpath = f.name
        with self.assertRaises(ValueError):
            load_backtest_queue(fpath)


# ---------------------------------------------------------------------------
# Test 3: builds strategy-fit-v1 artifact
# ---------------------------------------------------------------------------

class TestBuildStrategyFitArtifact(unittest.TestCase):

    def _build(self, entry=None, result=None, config=None):
        return build_strategy_fit_artifact(
            entry=entry or _make_queue_entry(),
            result=result or StrategyFitResult(),
            config=config or BacktestRunnerConfig(),
            generated_at_utc="2026-06-05T17:00:00+00:00",
        )

    def test_schema_version(self):
        a = self._build()
        self.assertEqual(a["schema_version"], "strategy-fit-v1")

    def test_preserves_source_queue_id(self):
        entry = _make_queue_entry(queue_id="bq-testid123")
        a = self._build(entry=entry)
        self.assertEqual(a["source_queue_id"], "bq-testid123")

    def test_preserves_symbol(self):
        entry = _make_queue_entry(symbol="MSFT")
        a = self._build(entry=entry)
        self.assertEqual(a["symbol"], "MSFT")

    def test_preserves_strategy_id(self):
        entry = _make_queue_entry(strategy_id="mean_reversion")
        a = self._build(entry=entry)
        self.assertEqual(a["strategy_id"], "mean_reversion")

    def test_preserves_timeframe(self):
        entry = _make_queue_entry(timeframe="1m")
        a = self._build(entry=entry)
        self.assertEqual(a["timeframe"], "1m")

    def test_preserves_regime_label(self):
        entry = _make_queue_entry(regime_label="range_bound")
        a = self._build(entry=entry)
        self.assertEqual(a["regime_label"], "range_bound")

    def test_recommended_for_live_false(self):
        a = self._build()
        self.assertFalse(a["recommended_for_live"])

    def test_recommended_for_paper_false(self):
        a = self._build()
        self.assertFalse(a["recommended_for_paper"])

    def test_blocked_status(self):
        a = self._build()
        self.assertEqual(a["status"], STATUS_BLOCKED)

    def test_failure_reasons_include_no_interface(self):
        a = self._build()
        self.assertIn(FAILURE_REASON_NO_INTERFACE, a["failure_reasons"])

    def test_all_metrics_null_when_blocked(self):
        a = self._build()
        for key in ("bars_used", "trades", "win_rate", "profit_factor",
                    "expectancy_bps", "sharpe", "sortino", "max_drawdown_bps"):
            self.assertIsNone(a[key], msg=f"{key} should be None in blocked mode")

    def test_passed_gates_all_false_when_blocked(self):
        a = self._build()
        for key in ("passed_min_bars", "passed_min_trades", "passed_max_drawdown",
                    "passed_profit_factor", "passed_expectancy",
                    "passed_cost_adjusted_edge", "passed_out_of_sample_check"):
            self.assertFalse(a[key], msg=f"{key} should be False in blocked mode")

    def test_recommended_for_live_cannot_be_set_by_config(self):
        cfg = BacktestRunnerConfig()
        cfg.recommended_for_live = True  # attempt override
        a = build_strategy_fit_artifact(
            entry=_make_queue_entry(),
            result=StrategyFitResult(),
            config=cfg,
            generated_at_utc="2026-06-05T17:00:00+00:00",
        )
        self.assertFalse(a["recommended_for_live"])

    def test_source_queue_artifact_propagated(self):
        a = build_strategy_fit_artifact(
            entry=_make_queue_entry(),
            result=StrategyFitResult(),
            config=BacktestRunnerConfig(),
            source_queue_artifact="exports/queue/bq.json",
        )
        self.assertEqual(a["source_queue_artifact"], "exports/queue/bq.json")


# ---------------------------------------------------------------------------
# Test 10: strategy-fit JSON write/read
# ---------------------------------------------------------------------------

class TestWriteStrategyFitArtifact(unittest.TestCase):

    def test_write_and_read_round_trip(self):
        artifact = build_strategy_fit_artifact(
            entry=_make_queue_entry(),
            result=StrategyFitResult(),
            config=BacktestRunnerConfig(),
            generated_at_utc="2026-06-05T17:00:00+00:00",
        )
        with tempfile.TemporaryDirectory() as tmpdir:
            path = str(Path(tmpdir) / "fit.json")
            written = write_strategy_fit_artifact(artifact, path)
            self.assertTrue(written.exists())
            loaded = json.loads(written.read_text(encoding="utf-8"))
        self.assertEqual(loaded["schema_version"], "strategy-fit-v1")
        self.assertFalse(loaded["recommended_for_live"])

    def test_creates_parent_dirs(self):
        artifact = build_strategy_fit_artifact(
            entry=_make_queue_entry(),
            result=StrategyFitResult(),
            config=BacktestRunnerConfig(),
        )
        with tempfile.TemporaryDirectory() as tmpdir:
            path = str(Path(tmpdir) / "nested" / "deep" / "fit.json")
            written = write_strategy_fit_artifact(artifact, path)
            self.assertTrue(written.exists())


# ---------------------------------------------------------------------------
# Test 11: empty queue produces zero artifacts
# ---------------------------------------------------------------------------

class TestRunBacktestQueue(unittest.TestCase):

    def test_empty_queue_produces_zero_artifacts(self):
        queue = _make_queue_artifact(entries=[])
        with tempfile.TemporaryDirectory() as tmpdir:
            result = run_backtest_queue(
                queue=queue,
                output_dir=tmpdir,
                generated_at_utc="2026-06-05T17:00:00+00:00",
            )
        self.assertEqual(result.artifacts_written, 0)
        self.assertEqual(result.queue_id_processed, 0)
        self.assertEqual(result.status, "dry_run_complete")

    def test_multiple_entries_produce_multiple_artifacts(self):
        entries = [
            _make_queue_entry(queue_id="bq-001", symbol="AAPL"),
            _make_queue_entry(queue_id="bq-002", symbol="MSFT"),
            _make_queue_entry(queue_id="bq-003", symbol="NVDA"),
        ]
        queue = _make_queue_artifact(entries=entries)
        with tempfile.TemporaryDirectory() as tmpdir:
            result = run_backtest_queue(
                queue=queue,
                output_dir=tmpdir,
                generated_at_utc="2026-06-05T17:00:00+00:00",
            )
            written_files = list(Path(tmpdir).glob("*.json"))
        self.assertEqual(result.artifacts_written, 3)
        self.assertEqual(len(written_files), 3)

    def test_artifact_paths_are_deterministic(self):
        """Same queue_id always produces the same filename."""
        name_a = _artifact_filename("bq-fixed-id-001")
        name_b = _artifact_filename("bq-fixed-id-001")
        self.assertEqual(name_a, name_b)

    def test_different_queue_ids_produce_different_paths(self):
        name_a = _artifact_filename("bq-001")
        name_b = _artifact_filename("bq-002")
        self.assertNotEqual(name_a, name_b)

    def test_runner_enforces_recommended_for_live_false(self):
        queue = _make_queue_artifact()
        with tempfile.TemporaryDirectory() as tmpdir:
            run_backtest_queue(queue=queue, output_dir=tmpdir,
                               generated_at_utc="2026-06-05T17:00:00+00:00")
            files = list(Path(tmpdir).glob("*.json"))
            self.assertEqual(len(files), 1)
            art = json.loads(files[0].read_text(encoding="utf-8"))
        self.assertFalse(art["recommended_for_live"])

    def test_runner_enforces_recommended_for_paper_false(self):
        queue = _make_queue_artifact()
        with tempfile.TemporaryDirectory() as tmpdir:
            run_backtest_queue(queue=queue, output_dir=tmpdir,
                               generated_at_utc="2026-06-05T17:00:00+00:00")
            files = list(Path(tmpdir).glob("*.json"))
            art = json.loads(files[0].read_text(encoding="utf-8"))
        self.assertFalse(art["recommended_for_paper"])

    def test_all_artifacts_are_blocked(self):
        entries = [_make_queue_entry(queue_id=f"bq-{i:03d}") for i in range(5)]
        queue = _make_queue_artifact(entries=entries)
        with tempfile.TemporaryDirectory() as tmpdir:
            run_backtest_queue(queue=queue, output_dir=tmpdir,
                               generated_at_utc="2026-06-05T17:00:00+00:00")
            for f in Path(tmpdir).glob("*.json"):
                art = json.loads(f.read_text(encoding="utf-8"))
                self.assertEqual(art["status"], STATUS_BLOCKED)
                self.assertIn(FAILURE_REASON_NO_INTERFACE, art["failure_reasons"])

    def test_source_queue_artifact_path_propagated(self):
        queue = _make_queue_artifact()
        with tempfile.TemporaryDirectory() as tmpdir:
            run_backtest_queue(
                queue=queue,
                output_dir=tmpdir,
                source_queue_artifact_path="exports/queue/bq.json",
                generated_at_utc="2026-06-05T17:00:00+00:00",
            )
            files = list(Path(tmpdir).glob("*.json"))
            art = json.loads(files[0].read_text(encoding="utf-8"))
        self.assertEqual(art["source_queue_artifact"], "exports/queue/bq.json")

    def test_run_result_has_correct_counts(self):
        entries = [_make_queue_entry(queue_id=f"bq-{i:03d}") for i in range(4)]
        queue = _make_queue_artifact(entries=entries)
        with tempfile.TemporaryDirectory() as tmpdir:
            result = run_backtest_queue(queue=queue, output_dir=tmpdir,
                                        generated_at_utc="2026-06-05T17:00:00+00:00")
        self.assertEqual(result.queue_id_processed, 4)
        self.assertEqual(result.artifacts_written, 4)


# ---------------------------------------------------------------------------
# Test 14: no broker/OMS/execution/network imports
# ---------------------------------------------------------------------------

class TestNoForbiddenImports(unittest.TestCase):

    def test_no_broker_imports(self):
        import mqk_research.scanner.backtest_runner as mod
        source_file = Path(mod.__file__).read_text(encoding="utf-8")
        # Check for actual import statements, not documentary mentions in comments.
        import_forbidden = [
            "import requests", "import urllib", "import http.client",
            "import aiohttp", "import psycopg", "import sqlalchemy",
        ]
        ref_forbidden = [
            "submit_order", "broker_adapter", "BrokerGateway",
            "oms_outbox", "oms_inbox", "/v2/orders",
        ]
        for token in import_forbidden + ref_forbidden:
            self.assertNotIn(token, source_file,
                             msg=f"forbidden token found in source: {token!r}")

    def test_no_subprocess_call_in_module(self):
        """Subprocess must not be imported at module level."""
        import mqk_research.scanner.backtest_runner as mod
        self.assertFalse(hasattr(mod, "subprocess"),
                         "subprocess must not be imported at module level")

    def test_no_order_fields_in_artifact(self):
        artifact = build_strategy_fit_artifact(
            entry=_make_queue_entry(),
            result=StrategyFitResult(),
            config=BacktestRunnerConfig(),
        )
        order_fields = ["order_id", "order_qty", "side", "limit_price",
                        "submit_order", "broker_event"]
        for f in order_fields:
            self.assertNotIn(f, artifact, msg=f"order field {f!r} must not appear")


# ---------------------------------------------------------------------------
# Test 16: does not run external command in unit tests
# ---------------------------------------------------------------------------

class TestNoExternalCommandExecution(unittest.TestCase):

    def test_runner_does_not_call_subprocess(self):
        """Verify run_backtest_queue works without subprocess."""
        import subprocess
        original_run = subprocess.run

        called = []

        def spy_run(*args, **kwargs):
            called.append(args)
            return original_run(*args, **kwargs)

        subprocess.run = spy_run
        try:
            queue = _make_queue_artifact()
            with tempfile.TemporaryDirectory() as tmpdir:
                run_backtest_queue(queue=queue, output_dir=tmpdir,
                                   generated_at_utc="2026-06-05T17:00:00+00:00")
        finally:
            subprocess.run = original_run

        self.assertEqual(called, [], "subprocess.run must not be called by runner")


# ---------------------------------------------------------------------------
# Test 19: recommended_for_live=True cannot be set
# ---------------------------------------------------------------------------

class TestRecommendedForLiveEnforcement(unittest.TestCase):

    def test_config_enforces_false_on_init(self):
        cfg = BacktestRunnerConfig(recommended_for_live=True)
        self.assertFalse(cfg.recommended_for_live)

    def test_run_result_never_sets_live(self):
        queue = _make_queue_artifact()
        with tempfile.TemporaryDirectory() as tmpdir:
            run_backtest_queue(queue=queue, output_dir=tmpdir,
                               generated_at_utc="2026-06-05T17:00:00+00:00")
            for f in Path(tmpdir).glob("*.json"):
                art = json.loads(f.read_text(encoding="utf-8"))
                self.assertFalse(art["recommended_for_live"])


if __name__ == "__main__":
    unittest.main()
