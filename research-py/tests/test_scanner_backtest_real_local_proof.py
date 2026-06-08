"""
BACKTEST-REAL-MODE-LOCAL-PROOF-01 — local/offline proof of real-mode subprocess
execution end to end (compiled mqk-cli binary -> metrics.json -> strategy-fit-v1).

BACKTEST-REAL-MODE-SAFE-INVOCATION-01 proved only the fail-closed command-build
path (bars_file_missing / command_build_failed / binary_not_found). It never
proved an actual non-mocked subprocess run. This file closes that proof gap
using a small committed deterministic fixture bars CSV
(tests/fixtures/backtest/AAPL_5m.csv) — NOT market/live paper proof.

Local/offline only: the only process invoked is the locally compiled mqk-cli
binary against a committed CSV fixture. No broker, OMS, daemon, DB, or network
contact occurs anywhere in this file.

The real-subprocess test is gated on the MQK_BACKTEST_CLI environment variable
and skips cleanly when it is unset or does not point to an existing binary —
it is never required for CI to pass. To run it locally:

    set MQK_BACKTEST_CLI=C:/Users/Zacha/Desktop/MiniQuantDeskV4/core-rs/target/debug/mqk-cli.exe
    cd research-py
    python -m pytest tests/test_scanner_backtest_real_local_proof.py -q

(build the binary first with `cargo build -p mqk-cli` if it does not exist)
"""
from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from typing import Any, Optional

from mqk_research.scanner.backtest_bridge import (
    REASON_BINARY_NOT_FOUND,
    BacktestBridgeConfig,
    run_backtest_for_entry,
)
from mqk_research.scanner.backtest_runner import (
    STATUS_COMPLETE,
    BacktestRunnerConfig,
    run_backtest_queue,
)

FIXTURES_DIR = Path(__file__).parent / "fixtures" / "backtest"


def _make_queue_entry(
    queue_id: str = "bq-real-proof-001",
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
        "notes": "BACKTEST-REAL-MODE-LOCAL-PROOF-01 entry",
    }


def _make_queue_artifact(entries: list[dict]) -> dict[str, Any]:
    return {
        "schema_version": "backtest-queue-v1",
        "generated_at_utc": "2026-06-07T00:00:00+00:00",
        "source_ranked_export": None,
        "queue_count": len(entries),
        "entries": entries,
        "recommended_for_live": False,
        "notes": "BACKTEST-REAL-MODE-LOCAL-PROOF-01 queue",
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


# ---------------------------------------------------------------------------
# Always-on: fail-closed binary_not_found path (no env var required)
# ---------------------------------------------------------------------------

class TestRealModeFailsClosedWithoutBinary(unittest.TestCase):

    def test_missing_binary_path_yields_binary_not_found(self):
        """
        A cli_binary path that does not exist on disk must fail closed with
        REASON_BINARY_NOT_FOUND and never reach subprocess — proven against
        the real fixture directory so command-building succeeds up to the
        binary-existence check.
        """
        entry = _make_queue_entry()
        with tempfile.TemporaryDirectory() as tmpdir:
            cfg = BacktestBridgeConfig(
                mode="real",
                cli_binary=str(FIXTURES_DIR / "mqk-cli-does-not-exist.exe"),
                bars_root_dir=str(FIXTURES_DIR),
                out_dir=tmpdir,
            )
            metrics, reasons = run_backtest_for_entry(entry, cfg)

        self.assertIsNone(metrics)
        self.assertIn(REASON_BINARY_NOT_FOUND, reasons)


# ---------------------------------------------------------------------------
# Gated: actual non-mocked subprocess execution against the local compiled binary
# ---------------------------------------------------------------------------

class TestRealLocalSubprocessProof(unittest.TestCase):

    def test_real_subprocess_executes_and_maps_to_strategy_fit_artifact(self):
        """
        When MQK_BACKTEST_CLI points to a real compiled mqk-cli binary, drive
        the full real-mode path end to end with NO mocking of subprocess:

            run_backtest_queue (mode="real")
              -> run_backtest_for_entry (actual subprocess.run of mqk-cli)
              -> parse_mqk_metrics_json (reads metrics.json written by the CLI)
              -> map_metrics_to_strategy_fit
              -> apply_backtest_gates
              -> strategy-fit-v1 artifact written to disk

        Skips cleanly (does not fail CI) when MQK_BACKTEST_CLI is unset or does
        not point to an existing binary.
        """
        binary = _resolve_local_cli_binary()
        if binary is None:
            self.skipTest(
                "MQK_BACKTEST_CLI not set to an existing local binary path; "
                "skipping real-subprocess proof (offline/local-only — never "
                "required for CI). To run manually, build with "
                "`cargo build -p mqk-cli` and set MQK_BACKTEST_CLI to the "
                "resulting mqk-cli(.exe) path."
            )

        entry = _make_queue_entry()
        queue = _make_queue_artifact([entry])

        with tempfile.TemporaryDirectory() as tmpdir:
            out_dir = str(Path(tmpdir) / "out")
            artifact_dir = str(Path(tmpdir) / "strategy_fit")

            bridge_cfg = BacktestBridgeConfig(
                mode="real",
                cli_binary=binary,
                bars_root_dir=str(FIXTURES_DIR),
                out_dir=out_dir,
            )
            runner_cfg = BacktestRunnerConfig(mode="real")

            result = run_backtest_queue(
                queue=queue,
                config=runner_cfg,
                output_dir=artifact_dir,
                generated_at_utc="2026-06-07T00:00:00+00:00",
                bridge_config=bridge_cfg,
            )

            # --- Proof that a real, non-mocked subprocess actually ran ---
            # mqk-cli writes metrics.json under <out_dir>/<run_id>/ itself;
            # the bridge only reads it back. Its presence on disk under the
            # out_dir we supplied is direct evidence of real execution.
            written_metrics = list(Path(out_dir).rglob("metrics.json"))
            self.assertEqual(
                len(written_metrics), 1,
                f"expected exactly one metrics.json written by the real subprocess "
                f"under {out_dir}, found {written_metrics}",
            )

            # --- Proof of correct mapping into a strategy-fit-v1 artifact ---
            self.assertEqual(result.artifacts_written, 1)
            self.assertEqual(result.status, "real_complete")

            written_artifacts = list(Path(artifact_dir).glob("*.json"))
            self.assertEqual(len(written_artifacts), 1)
            artifact = _load_json(written_artifacts[0])

            self.assertEqual(artifact["schema_version"], "strategy-fit-v1")
            self.assertEqual(artifact["symbol"], "AAPL")
            self.assertEqual(artifact["strategy_id"], "intraday_scalper")
            self.assertEqual(artifact["timeframe"], "5m")
            self.assertEqual(artifact["status"], STATUS_COMPLETE)

            # Real metrics were mapped through (non-trivial fixture: trades > 0)
            self.assertIsNotNone(artifact["bars_used"])
            self.assertGreater(artifact["bars_used"], 0)
            self.assertIsNotNone(artifact["trades"])
            self.assertGreater(artifact["trades"], 0)
            self.assertIsNotNone(artifact["expectancy_bps"])
            self.assertIsNotNone(artifact["win_rate"])
            self.assertIsNotNone(artifact["profit_factor"])

            # validation_metrics_missing always appended for real CLI runs —
            # mqk-artifacts metrics.json never carries validation_* fields.
            self.assertIn("validation_metrics_missing", artifact["failure_reasons"])

            # Hard invariants must hold even on a successful real-mode run.
            self.assertFalse(artifact["recommended_for_live"])
            self.assertFalse(result.notes.find("recommended_for_live=False") == -1)


def _load_json(path: Path) -> dict[str, Any]:
    import json
    return json.loads(path.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
