"""
BACKTEST-END-TO-END-ARTIFACT-PROOF-01 — local/offline proof of the full
backtest artifact chain:

    backtest-queue-v1
      -> real local `mqk-cli backtest csv` subprocess
      -> metrics.json
      -> strategy-fit-v1 mapping (backtest_bridge)
      -> opt-in walk-forward validation merge (walkforward_runner)
      -> gate evaluation (backtest_gates)
      -> final strategy-fit-v1 artifact

This closes the remaining proof gap between two prior patches:
  - BACKTEST-REAL-MODE-LOCAL-PROOF-01 (4e1045f) proves
    queue -> real subprocess -> metrics.json -> strategy-fit-v1 -> gates,
    but with walk-forward validation disabled (the default).
  - BACKTEST-WALKFORWARD-VALIDATION-INTEGRATION-01 (d0dc547) proves the
    walk-forward merge is wired correctly, but with BOTH the backtest
    subprocess AND run_walkforward_entry mocked — it never drives a real
    subprocess (it explicitly asserts subprocess.run is never called).

Neither prior test proves "real backtest subprocess + walk-forward
validation enabled" in one chain. This file does, with the real subprocess
left genuinely real and only the walk-forward *result* injected.

Classification: CLOSED-PATCHED / REAL-WALKFORWARD-SUBPROCESS-PENDING
  Why the walk-forward subprocess is injected rather than real: the
  committed tests/fixtures/backtest/AAPL_5m.csv fixture spans under one
  calendar day (96 rows; min end_ts=1000, max end_ts=29500 -> 1970-01-01
  00:16-08:11 UTC). build_date_splits requires min_train_days=14 PLUS
  min_validation_days=7 (>=21 calendar days) to produce even a single
  fold — build_walkforward_plan would fail closed with
  REASON_NO_SPLITS before any subprocess could be invoked. Producing a
  real walk-forward subprocess proof would require a >=21-trading-day 5m
  fixture (well over 1,500 rows) plus one CLI invocation per split — out
  of proportion for a local artifact-chain proof. So: the MAIN backtest
  subprocess runs for real against the committed fixture; only
  run_walkforward_entry() is injected with a realistic completed
  aggregate (clearly named "REAL backtest + INJECTED walk-forward
  aggregate" below), and a separate always-on pure test proves the
  aggregate shape is genuinely accepted by the merge + gate pipeline.

Local/offline only: the only process invoked is the locally compiled
mqk-cli binary against the committed CSV fixture. No broker, OMS, daemon,
DB, or network contact occurs anywhere in this file.

Gated on MQK_BACKTEST_CLI — skips cleanly when unset or not pointing to an
existing binary (never required for CI):

    set MQK_BACKTEST_CLI=C:/Users/Zacha/Desktop/MiniQuantDeskV4/core-rs/target/debug/mqk-cli.exe
    cd research-py
    python -m pytest tests/test_scanner_backtest_end_to_end_artifact_proof.py -q

(build the binary first with `cargo build -p mqk-cli` if it does not exist)
"""
from __future__ import annotations

import json
import os
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
    QUEUE_SCHEMA_VERSION,
    SCHEMA_VERSION,
    STATUS_COMPLETE,
    BacktestRunnerConfig,
    StrategyFitResult,
    build_strategy_fit_artifact,
    run_backtest_queue,
)
from mqk_research.scanner.walkforward_runner import (
    REASON_WALKFORWARD_VALIDATION_BLOCKED,
    WalkForwardRunResult,
    merge_walkforward_validation_into_mapped,
)

FIXTURES_DIR = Path(__file__).parent / "fixtures" / "backtest"

QUEUE_ID = "bq-e2e-proof-001"
SYMBOL = "AAPL"
STRATEGY_ID = "intraday_scalper"
TIMEFRAME = "5m"


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

def _make_queue_entry() -> dict[str, Any]:
    return {
        "queue_id": QUEUE_ID,
        "symbol": SYMBOL,
        "strategy_id": STRATEGY_ID,
        "timeframe": TIMEFRAME,
        "regime_label": "trending_up",
        "source_rank": 1,
        "source_total_score": 0.75,
        "source_candidate_artifact": None,
        "source_ranked_export": None,
        "ambiguity_policy": "CONSERVATIVE_WORST_CASE",
        "stress_profile": "slippage_x2",
        "recommended_for_live": False,
        "notes": "BACKTEST-END-TO-END-ARTIFACT-PROOF-01 entry",
    }


def _make_queue_artifact(entries: list[dict]) -> dict[str, Any]:
    return {
        "schema_version": QUEUE_SCHEMA_VERSION,
        "generated_at_utc": "2026-06-07T00:00:00+00:00",
        "source_ranked_export": None,
        "queue_count": len(entries),
        "entries": entries,
        "recommended_for_live": False,
        "notes": "BACKTEST-END-TO-END-ARTIFACT-PROOF-01 queue",
    }


def _resolve_local_cli_binary() -> Optional[str]:
    """Return MQK_BACKTEST_CLI path if set and pointing to an existing file, else None."""
    raw = os.environ.get("MQK_BACKTEST_CLI")
    if not raw:
        return None
    p = Path(raw)
    if not p.exists() or not p.is_file():
        return None
    return str(p)


def _make_realistic_completed_wf_result() -> WalkForwardRunResult:
    """
    A realistic *injected* completed walk-forward aggregate, shaped exactly
    like what run_walkforward_entry(mode="real") returns on success: three
    validation splits, all passing, with the conservative min-across-splits
    profit factor and summed trade count that aggregate_walkforward_results
    would produce. Stands in for a real subprocess run that the committed
    fixture is too short to produce (see module docstring).
    """
    validation_metrics = {
        "validation_profit_factor": 1.35,
        "validation_trades": 42,
        "validation_win_rate": 0.58,
        "sample_quality": 0.85,
        "parameter_stability_score": 0.75,
        "split_count": 3,
        "passed_split_count": 3,
        "worst_split_profit_factor": 1.2,
        "median_split_profit_factor": 1.35,
        "all_splits_have_metrics": True,
        "recommended_for_live": False,
    }
    return WalkForwardRunResult(
        queue_id=QUEUE_ID,
        symbol=SYMBOL,
        strategy_id=STRATEGY_ID,
        mode="real",
        split_results=[],
        aggregate=None,
        validation_metrics=validation_metrics,
        failure_reasons=[],
        status="complete",
        recommended_for_live=False,
    )


def _load_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


# ---------------------------------------------------------------------------
# A. Full chain proof: REAL backtest subprocess + INJECTED walk-forward result
# ---------------------------------------------------------------------------

class TestEndToEndArtifactChainRealBacktestInjectedWalkforward(unittest.TestCase):

    def test_queue_to_real_cli_to_strategy_fit_with_walkforward_merge_and_gates(self):
        """
        REAL backtest subprocess + INJECTED walk-forward aggregate.

        Drives the full chain with NO mocking of the main backtest subprocess
        or of subprocess.run itself:

            queue (backtest-queue-v1, schema asserted)
              -> run_backtest_queue(mode="real", enable_walkforward_validation=True)
              -> run_backtest_for_entry        (REAL subprocess.run of mqk-cli)
              -> parse_mqk_metrics_json        (REAL — reads metrics.json on disk)
              -> map_metrics_to_strategy_fit   (REAL)
              -> _run_and_merge_walkforward_validation
                   -> resolve_bars_csv_path    (REAL — resolves committed fixture)
                   -> run_walkforward_entry    ** INJECTED ** realistic completed
                                                  aggregate (see module docstring)
                   -> merge_walkforward_validation_into_mapped (REAL)
              -> apply_backtest_gates          (REAL)
              -> strategy-fit-v1 artifact written to disk

        Skips cleanly (does not fail CI) when MQK_BACKTEST_CLI is unset or does
        not point to an existing binary.
        """
        binary = _resolve_local_cli_binary()
        if binary is None:
            self.skipTest(
                "MQK_BACKTEST_CLI not set to an existing local binary path; "
                "skipping end-to-end artifact proof (offline/local-only — "
                "never required for CI). To run manually, build with "
                "`cargo build -p mqk-cli` and set MQK_BACKTEST_CLI to the "
                "resulting mqk-cli(.exe) path."
            )

        entry = _make_queue_entry()
        queue = _make_queue_artifact([entry])

        # --- Proof: input queue carries the canonical queue schema ---
        self.assertEqual(queue["schema_version"], QUEUE_SCHEMA_VERSION)
        self.assertEqual(queue["schema_version"], "backtest-queue-v1")

        wf_result = _make_realistic_completed_wf_result()

        with tempfile.TemporaryDirectory() as tmpdir:
            out_dir = str(Path(tmpdir) / "out")
            artifact_dir = str(Path(tmpdir) / "strategy_fit")

            bridge_cfg = BacktestBridgeConfig(
                mode="real",
                cli_binary=binary,
                bars_root_dir=str(FIXTURES_DIR),
                out_dir=out_dir,
            )
            runner_cfg = BacktestRunnerConfig(
                mode="real",
                enable_walkforward_validation=True,
            )

            with patch(
                "mqk_research.scanner.walkforward_runner.run_walkforward_entry",
                return_value=wf_result,
            ) as mock_wf_entry:
                result = run_backtest_queue(
                    queue=queue,
                    config=runner_cfg,
                    output_dir=artifact_dir,
                    generated_at_utc="2026-06-07T00:00:00+00:00",
                    bridge_config=bridge_cfg,
                )

            # --- Proof that a real, non-mocked main backtest subprocess ran ---
            written_metrics = list(Path(out_dir).rglob("metrics.json"))
            self.assertEqual(
                len(written_metrics), 1,
                f"expected exactly one metrics.json written by the real subprocess "
                f"under {out_dir}, found {written_metrics}",
            )

            # --- Proof the walk-forward seam was reached with the REAL,
            # resolved fixture bars CSV — only its *result* is injected ---
            mock_wf_entry.assert_called_once()
            call_entry, call_bars_csv = mock_wf_entry.call_args[0][0:2]
            self.assertEqual(call_entry.get("queue_id"), QUEUE_ID)
            self.assertEqual(
                Path(call_bars_csv).resolve(),
                (FIXTURES_DIR / "AAPL_5m.csv").resolve(),
            )

            # --- Proof of correct mapping into a strategy-fit-v1 artifact ---
            self.assertEqual(result.artifacts_written, 1)
            self.assertEqual(result.status, "real_complete")

            written_artifacts = list(Path(artifact_dir).glob("*.json"))
            self.assertEqual(len(written_artifacts), 1)
            artifact = _load_json(written_artifacts[0])

            self.assertEqual(artifact["schema_version"], SCHEMA_VERSION)
            self.assertEqual(artifact["schema_version"], "strategy-fit-v1")
            self.assertEqual(artifact["source_queue_id"], QUEUE_ID)
            self.assertEqual(artifact["symbol"], SYMBOL)
            self.assertEqual(artifact["strategy_id"], STRATEGY_ID)
            self.assertEqual(artifact["timeframe"], TIMEFRAME)
            self.assertEqual(artifact["status"], STATUS_COMPLETE)
            self.assertEqual(artifact["status"], "complete")

            # Real metrics were mapped through (non-trivial fixture: trades > 0)
            self.assertIsNotNone(artifact["bars_used"])
            self.assertGreater(artifact["bars_used"], 0)
            self.assertIsNotNone(artifact["trades"])
            self.assertGreater(artifact["trades"], 0)
            self.assertIsNotNone(artifact["profit_factor"])
            self.assertIsNotNone(artifact["expectancy_bps"])

            # --- Proof: walk-forward aggregate was merged into the artifact ---
            self.assertEqual(artifact["validation_profit_factor"], 1.35)
            self.assertEqual(artifact["validation_trades"], 42)
            self.assertEqual(artifact["sample_quality"], 0.85)
            self.assertEqual(artifact["parameter_stability_score"], 0.75)
            self.assertNotIn("validation_metrics_missing", artifact["failure_reasons"])
            self.assertNotIn(REASON_WALKFORWARD_VALIDATION_BLOCKED, artifact["failure_reasons"])

            # --- Proof: gates were applied (real evaluation, not skipped) ---
            for gate_field in (
                "passed_min_bars", "passed_min_trades", "passed_max_drawdown",
                "passed_profit_factor", "passed_expectancy",
                "passed_cost_adjusted_edge", "passed_out_of_sample_check",
            ):
                self.assertIn(gate_field, artifact)
                self.assertIsInstance(artifact[gate_field], bool)

            # Out-of-sample gate must evaluate truthfully now that real
            # validation_profit_factor/validation_trades are present
            # (1.35 >= 1.1 and 42 >= 10 — default BacktestGateConfig thresholds).
            self.assertTrue(artifact["passed_out_of_sample_check"])

            # --- Hard invariants must hold even on a successful real-mode run ---
            self.assertFalse(artifact["recommended_for_live"])
            self.assertIn("recommended_for_live=False", result.notes)


# ---------------------------------------------------------------------------
# B. Always-on pure proof: injected aggregate shape is accepted end to end
#    by the merge + gate pipeline (no subprocess; never skipped in CI)
# ---------------------------------------------------------------------------

class TestInjectedWalkforwardAggregateShapeAcceptedByMergeAndGates(unittest.TestCase):
    """
    Proves — independent of MQK_BACKTEST_CLI availability — that a realistic
    completed walk-forward aggregate of the exact shape injected in the gated
    end-to-end test above is genuinely accepted by
    merge_walkforward_validation_into_mapped and flows through
    build_strategy_fit_artifact + apply_backtest_gates to a truthful,
    fail-closed recommendation. No subprocess, no mocking of production code —
    pure data-shape and gate-pipeline proof.
    """

    def test_realistic_aggregate_merges_and_gates_evaluate_truthfully(self):
        # A mapped dict shaped like map_metrics_to_strategy_fit's real-CLI
        # output, with validation_* fields still None (as the Rust CLI never
        # emits them — BACKTEST-BRIDGE-BUNDLE-01 finding).
        mapped: dict[str, Any] = {
            "bars_used": 96,
            "trades": 40,
            "win_rate": 0.55,
            "profit_factor": 1.4,
            "expectancy_bps": 12.0,
            "avg_trade_bps": None,
            "max_drawdown_bps": 300.0,
            "sharpe": 0.9,
            "sortino": 1.1,
            "exposure_time_pct": 45.0,
            "net_expectancy_after_cost_bps": 8.0,
            "sample_quality": None,
            "parameter_stability_score": None,
            "validation_profit_factor": None,
            "validation_trades": None,
            "largest_trade_profit_fraction": 0.05,
        }
        wf_result = _make_realistic_completed_wf_result()

        merged, reasons = merge_walkforward_validation_into_mapped(
            mapped, [REASON_VALIDATION_METRICS_MISSING], wf_result
        )

        # --- Proof: aggregate shape accepted — all four mergeable fields filled ---
        self.assertEqual(merged["validation_profit_factor"], 1.35)
        self.assertEqual(merged["validation_trades"], 42)
        self.assertEqual(merged["sample_quality"], 0.85)
        self.assertEqual(merged["parameter_stability_score"], 0.75)
        self.assertNotIn(REASON_VALIDATION_METRICS_MISSING, reasons)
        self.assertNotIn(REASON_WALKFORWARD_VALIDATION_BLOCKED, reasons)

        entry = _make_queue_entry()
        cfg = BacktestRunnerConfig(mode="real")
        artifact = build_strategy_fit_artifact(
            entry=entry,
            result=StrategyFitResult(**merged),
            config=cfg,
            generated_at_utc="2026-06-07T00:00:00+00:00",
            source_queue_artifact="in-memory-e2e-proof",
            extra_failure_reasons=reasons,
        )
        evaluated = apply_backtest_gates(artifact)

        # --- Proof: gates evaluated using the merged validation fields ---
        self.assertEqual(evaluated["status"], STATUS_COMPLETE)
        self.assertTrue(evaluated["passed_out_of_sample_check"])

        # min_bars (96 < 200) still fails on this fixture-scale sample —
        # proves walk-forward success does not bypass other required gates;
        # recommended_for_paper remains an honest ALL-gates-pass result.
        self.assertFalse(evaluated["passed_min_bars"])
        self.assertFalse(evaluated["recommended_for_paper"])
        self.assertFalse(evaluated["recommended_for_live"])


if __name__ == "__main__":
    unittest.main()
