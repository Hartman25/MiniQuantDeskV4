"""
TV-02: Deployability Gate proof tests.

Proves that:
1. A candidate artifact can be evaluated deterministically.
2. pass/fail/reason outputs are stable and reproducible.
3. All four checks are always present in the result.
4. Each individual check can independently cause a gate failure.
5. The gate result is persisted in a stable machine-readable form.
6. Producer/consumer agreement: write then read_deployability_gate returns
   an equivalent result.
7. Passing all checks produces passed=True with a canonical overall_reason.
8. Missing required metrics raise a clear error (KeyError).

These tests use no DB and no network.  They create minimal fixtures in a
temporary directory and exercise the full evaluate → write → read path.

This is gate-contract proof only.  It does NOT prove strategy viability,
economics, or live deployment readiness.
"""
from __future__ import annotations

import json
import shutil
import tempfile
import unittest
from pathlib import Path

from mqk_research.contracts import (
    DEPLOYABILITY_GATE_CONTRACT_VERSION,
    DeployabilityGateResult,
    read_deployability_gate,
)
from mqk_research.deployment.gate import (
    DeployabilityGateConfig,
    evaluate_deployability_gate,
    write_deployability_gate,
)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

_PASSING_METRICS = {
    "trade_event_count": 50,
    "trading_days": 120,
    "turnover": 240.0,    # daily_turnover = 240/120 = 2.0 (<= 5.0 threshold)
    "active_days": 80,    # active_fraction = 80/120 ≈ 0.667 (>= 0.05 threshold)
}

_FAILING_TRADE_COUNT = {
    "trade_event_count": 10,   # < 30: fails min_trade_count
    "trading_days": 120,
    "turnover": 240.0,
    "active_days": 80,
}

_FAILING_TRADING_DAYS = {
    "trade_event_count": 50,
    "trading_days": 30,        # < 60: fails min_trading_days
    "turnover": 60.0,
    "active_days": 20,
}

_FAILING_TURNOVER = {
    "trade_event_count": 50,
    "trading_days": 120,
    "turnover": 900.0,         # daily_turnover = 900/120 = 7.5 > 5.0: fails max_daily_turnover
    "active_days": 80,
}

_FAILING_ACTIVE_FRACTION = {
    "trade_event_count": 50,
    "trading_days": 120,
    "turnover": 240.0,
    "active_days": 2,          # active_fraction = 2/120 ≈ 0.017 < 0.05: fails min_active_day_fraction
}

_ARTIFACT_ID = "a" * 64  # fake 64-char hex artifact_id


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


class TestEvaluateDeployabilityGate(unittest.TestCase):
    def test_passing_metrics_returns_passed_true(self) -> None:
        """All four checks pass → result.passed=True."""
        result = evaluate_deployability_gate(_ARTIFACT_ID, _PASSING_METRICS)
        self.assertTrue(result.passed)

    def test_result_is_deployability_gate_result_type(self) -> None:
        result = evaluate_deployability_gate(_ARTIFACT_ID, _PASSING_METRICS)
        self.assertIsInstance(result, DeployabilityGateResult)

    def test_schema_version_is_canonical(self) -> None:
        result = evaluate_deployability_gate(_ARTIFACT_ID, _PASSING_METRICS)
        self.assertEqual(result.schema_version, DEPLOYABILITY_GATE_CONTRACT_VERSION)

    def test_artifact_id_propagated(self) -> None:
        result = evaluate_deployability_gate(_ARTIFACT_ID, _PASSING_METRICS)
        self.assertEqual(result.artifact_id, _ARTIFACT_ID)

    def test_all_four_checks_present(self) -> None:
        """All four check names must be present in every result."""
        expected = {
            "min_trade_count",
            "min_trading_days",
            "max_daily_turnover",
            "min_active_day_fraction",
        }
        result = evaluate_deployability_gate(_ARTIFACT_ID, _PASSING_METRICS)
        actual = {c.name for c in result.checks}
        self.assertEqual(actual, expected)

    def test_passing_result_overall_reason_contains_pass_language(self) -> None:
        result = evaluate_deployability_gate(_ARTIFACT_ID, _PASSING_METRICS)
        self.assertIn("passed", result.overall_reason.lower())

    def test_fail_min_trade_count(self) -> None:
        result = evaluate_deployability_gate(_ARTIFACT_ID, _FAILING_TRADE_COUNT)
        self.assertFalse(result.passed)
        check = next(c for c in result.checks if c.name == "min_trade_count")
        self.assertFalse(check.passed)
        self.assertIn("min_trade_count", result.overall_reason)

    def test_fail_min_trading_days(self) -> None:
        result = evaluate_deployability_gate(_ARTIFACT_ID, _FAILING_TRADING_DAYS)
        self.assertFalse(result.passed)
        check = next(c for c in result.checks if c.name == "min_trading_days")
        self.assertFalse(check.passed)

    def test_fail_max_daily_turnover(self) -> None:
        result = evaluate_deployability_gate(_ARTIFACT_ID, _FAILING_TURNOVER)
        self.assertFalse(result.passed)
        check = next(c for c in result.checks if c.name == "max_daily_turnover")
        self.assertFalse(check.passed)

    def test_fail_min_active_day_fraction(self) -> None:
        result = evaluate_deployability_gate(_ARTIFACT_ID, _FAILING_ACTIVE_FRACTION)
        self.assertFalse(result.passed)
        check = next(c for c in result.checks if c.name == "min_active_day_fraction")
        self.assertFalse(check.passed)

    def test_other_checks_pass_when_single_check_fails(self) -> None:
        """When only min_trade_count fails, the other three checks still pass."""
        result = evaluate_deployability_gate(_ARTIFACT_ID, _FAILING_TRADE_COUNT)
        passing_checks = {c.name for c in result.checks if c.passed}
        self.assertIn("min_trading_days", passing_checks)
        self.assertIn("max_daily_turnover", passing_checks)
        self.assertIn("min_active_day_fraction", passing_checks)

    def test_deterministic_for_same_inputs(self) -> None:
        """Same inputs, injected timestamp → identical JSON output."""
        ts = "2025-01-01T00:00:00Z"
        r1 = evaluate_deployability_gate(
            _ARTIFACT_ID, _PASSING_METRICS, evaluated_at_utc=ts
        )
        r2 = evaluate_deployability_gate(
            _ARTIFACT_ID, _PASSING_METRICS, evaluated_at_utc=ts
        )
        self.assertEqual(r1.to_json(), r2.to_json())

    def test_different_artifact_ids_produce_different_results(self) -> None:
        id_a = "a" * 64
        id_b = "b" * 64
        r_a = evaluate_deployability_gate(id_a, _PASSING_METRICS, evaluated_at_utc="2025-01-01T00:00:00Z")
        r_b = evaluate_deployability_gate(id_b, _PASSING_METRICS, evaluated_at_utc="2025-01-01T00:00:00Z")
        self.assertNotEqual(r_a.artifact_id, r_b.artifact_id)
        self.assertNotEqual(r_a.to_json(), r_b.to_json())

    def test_check_value_and_threshold_are_recorded(self) -> None:
        """Each check must record the observed value and the threshold applied."""
        result = evaluate_deployability_gate(_ARTIFACT_ID, _PASSING_METRICS)
        for check in result.checks:
            self.assertIsInstance(check.value, float)
            self.assertIsInstance(check.threshold, float)
            self.assertIsInstance(check.note, str)
            self.assertGreater(len(check.note), 0)

    def test_empty_artifact_id_raises_value_error(self) -> None:
        with self.assertRaises(ValueError):
            evaluate_deployability_gate("", _PASSING_METRICS)

    def test_missing_metric_key_raises_key_error(self) -> None:
        """Incomplete metrics dict must raise KeyError."""
        incomplete = {"trade_event_count": 50, "trading_days": 120}
        with self.assertRaises(KeyError):
            evaluate_deployability_gate(_ARTIFACT_ID, incomplete)

    def test_custom_config_overrides_thresholds(self) -> None:
        """A custom config with tighter thresholds should cause a pass to fail."""
        tight_cfg = DeployabilityGateConfig(
            min_trade_count=200,  # tighter than 50 trade events
            min_trading_days=60,
            max_daily_turnover=5.0,
            min_active_day_fraction=0.05,
        )
        result = evaluate_deployability_gate(_ARTIFACT_ID, _PASSING_METRICS, config=tight_cfg)
        self.assertFalse(result.passed)
        check = next(c for c in result.checks if c.name == "min_trade_count")
        self.assertFalse(check.passed)
        self.assertEqual(check.threshold, 200.0)

    def test_boundary_exactly_at_threshold_passes(self) -> None:
        """Exactly at threshold should pass (>=, <=)."""
        boundary_metrics = {
            "trade_event_count": 30,   # == min_trade_count threshold
            "trading_days": 60,         # == min_trading_days threshold
            "turnover": 300.0,          # daily_turnover = 300/60 = 5.0 == max_daily_turnover
            "active_days": 3,           # active_fraction = 3/60 = 0.05 == min_active_day_fraction
        }
        result = evaluate_deployability_gate(_ARTIFACT_ID, boundary_metrics)
        self.assertTrue(result.passed, f"All boundary values should pass: {result.overall_reason}")

    def test_zero_trading_days_does_not_divide_by_zero(self) -> None:
        """zero trading_days must not raise ZeroDivisionError (uses max(trading_days, 1))."""
        zero_day_metrics = {
            "trade_event_count": 50,
            "trading_days": 0,
            "turnover": 0.0,
            "active_days": 0,
        }
        try:
            result = evaluate_deployability_gate(_ARTIFACT_ID, zero_day_metrics)
            # trading_days=0 fails min_trading_days — just confirm no exception
            self.assertFalse(result.passed)
        except ZeroDivisionError:
            self.fail("ZeroDivisionError raised with trading_days=0")


class TestWriteAndReadDeployabilityGate(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _eval(self, metrics=None, ts="2025-01-01T00:00:00Z") -> DeployabilityGateResult:
        return evaluate_deployability_gate(
            _ARTIFACT_ID,
            metrics or _PASSING_METRICS,
            evaluated_at_utc=ts,
        )

    def test_write_creates_file(self) -> None:
        result = self._eval()
        path = write_deployability_gate(self.tmp, result)
        self.assertTrue(path.exists())
        self.assertEqual(path.name, "deployability_gate.json")

    def test_written_json_is_valid(self) -> None:
        result = self._eval()
        path = write_deployability_gate(self.tmp, result)
        raw = json.loads(path.read_text(encoding="utf-8"))
        self.assertIn("schema_version", raw)
        self.assertIn("artifact_id", raw)
        self.assertIn("passed", raw)
        self.assertIn("checks", raw)
        self.assertIn("overall_reason", raw)

    def test_write_then_read_returns_equivalent_result(self) -> None:
        """Producer writes → consumer reads → fields agree."""
        result = self._eval()
        path = write_deployability_gate(self.tmp, result)
        recovered = read_deployability_gate(path)

        self.assertEqual(recovered.schema_version, result.schema_version)
        self.assertEqual(recovered.artifact_id, result.artifact_id)
        self.assertEqual(recovered.passed, result.passed)
        self.assertEqual(recovered.overall_reason, result.overall_reason)
        self.assertEqual(len(recovered.checks), len(result.checks))

    def test_write_then_read_checks_are_faithful(self) -> None:
        """Each check's name, passed, value, threshold survive the round-trip."""
        result = self._eval()
        path = write_deployability_gate(self.tmp, result)
        recovered = read_deployability_gate(path)

        by_name_orig = {c.name: c for c in result.checks}
        by_name_read = {c.name: c for c in recovered.checks}
        self.assertEqual(set(by_name_orig.keys()), set(by_name_read.keys()))
        for name, orig_check in by_name_orig.items():
            read_check = by_name_read[name]
            self.assertEqual(orig_check.passed, read_check.passed)
            self.assertAlmostEqual(orig_check.value, read_check.value, places=10)
            self.assertAlmostEqual(orig_check.threshold, read_check.threshold, places=10)

    def test_schema_version_mismatch_raises_value_error(self) -> None:
        """Tampered schema_version must raise ValueError on read."""
        result = self._eval()
        path = write_deployability_gate(self.tmp, result)
        raw = json.loads(path.read_text(encoding="utf-8"))
        raw["schema_version"] = "gate-v0"
        path.write_text(json.dumps(raw), encoding="utf-8")

        with self.assertRaises(ValueError) as ctx:
            read_deployability_gate(path)
        self.assertIn("schema_version mismatch", str(ctx.exception))

    def test_read_missing_file_raises_file_not_found(self) -> None:
        with self.assertRaises(FileNotFoundError):
            read_deployability_gate(self.tmp / "no_such_dir" / "deployability_gate.json")

    def test_write_to_nonexistent_dir_raises_value_error(self) -> None:
        result = self._eval()
        with self.assertRaises(ValueError):
            write_deployability_gate(self.tmp / "does_not_exist", result)

    def test_write_is_deterministic_for_same_inputs(self) -> None:
        """Same inputs written twice → identical file content (deterministic JSON)."""
        result = self._eval()
        p1 = write_deployability_gate(self.tmp, result)
        content1 = p1.read_text(encoding="utf-8")
        p2 = write_deployability_gate(self.tmp, result)
        content2 = p2.read_text(encoding="utf-8")
        self.assertEqual(content1, content2)

    def test_failing_gate_round_trips_correctly(self) -> None:
        result = self._eval(metrics=_FAILING_TRADE_COUNT)
        path = write_deployability_gate(self.tmp, result)
        recovered = read_deployability_gate(path)
        self.assertFalse(recovered.passed)
        check = next(c for c in recovered.checks if c.name == "min_trade_count")
        self.assertFalse(check.passed)


if __name__ == "__main__":
    unittest.main()
