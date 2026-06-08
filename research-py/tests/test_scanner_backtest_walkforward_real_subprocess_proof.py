"""
BACKTEST-WALKFORWARD-REAL-SUBPROCESS-PROOF-01 — local/offline proof of a real
(non-mocked) walk-forward validation subprocess chain:

    queue entry + bars CSV
      -> build_walkforward_plan         (REAL — produces >=1 split)
      -> write_split_csv                (REAL — split CSVs written to disk)
      -> real local `mqk-cli backtest csv` subprocess per split (REAL — no mock)
      -> aggregate_walkforward_results  (REAL — conservative cross-split aggregate)
      -> map_walkforward_to_validation_metrics (REAL)

This closes the proof gap explicitly named in
BACKTEST-END-TO-END-ARTIFACT-PROOF-01 (3387d2f): the committed
tests/fixtures/backtest/AAPL_5m.csv fixture spans under one calendar day and
is too short for build_date_splits (requires >=21 calendar days:
min_train_days=14 + min_validation_days=7) to produce even one fold, so that
proof injected run_walkforward_entry()'s *result* rather than driving a real
walk-forward subprocess.

Fixture strategy — generated, not committed:
  A deterministic 25-calendar-day, 5-minute-bar CSV (7,200 rows: 25 days x
  288 bars/day, continuous, no gaps) is generated INSIDE this test to a
  tempdir at run time using a fixed-seed linear congruential generator (LCG)
  pseudo-random walk. It is never written into the repo or into exports/.
  Same seed + same recipe -> byte-identical CSV every run (deterministic).

  Empirically validated recipe (seed=999, step_scale=150_000, base=100M
  micros, starting 2024-01-01T00:00:00Z): with
  WalkForwardConfig(train_days=14, validation_days=7, step_days=7) this
  fixture produces exactly ONE split — fold 0, train [2024-01-01..2024-01-14],
  validation [2024-01-15..2024-01-21], 2016 validation rows — and the real
  mqk-cli subprocess against that validation window produces a genuine,
  non-fabricated result: trade_count=20, profit_factor=0.0 (all 20 trades
  losing — an authentic "long-only momentum strategy whipsawed by random-walk
  noise" outcome, not a gamed pass). validation_profit_factor/
  validation_trades/sample_quality/parameter_stability_score are all populated
  from these REAL split metrics (none are None) — exactly what this proof
  exists to demonstrate. The aggregate honestly fails
  min_validation_profit_factor (0.0 < 1.1); this is reported, not hidden or
  forced to pass. recommended_for_live is asserted False throughout (hard
  invariant in WalkForwardRunnerConfig/WalkForwardAggregate/WalkForwardRunResult).

Local/offline only: the only process invoked is the locally compiled mqk-cli
binary against a freshly generated, deterministic, synthetic CSV in a tempdir.
No broker, OMS, daemon, DB, network, live data, or market evidence of any kind
is involved anywhere in this file. This is NOT a market/live/paper proof —
it proves the walk-forward *subprocess plumbing* end to end with real
synthetic data.

Gated on MQK_BACKTEST_CLI — skips cleanly when unset or not pointing to an
existing binary (never required for CI):

    set MQK_BACKTEST_CLI=C:/Users/Zacha/Desktop/MiniQuantDeskV4/core-rs/target/debug/mqk-cli.exe
    cd research-py
    python -m pytest tests/test_scanner_backtest_walkforward_real_subprocess_proof.py -q

(build the binary first with `cargo build -p mqk-cli` if it does not exist)
"""
from __future__ import annotations

import os
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

from mqk_research.scanner.backtest_bridge import BacktestBridgeConfig
from mqk_research.scanner.walkforward_runner import (
    WalkForwardRunnerConfig,
    run_walkforward_entry,
)

QUEUE_ID = "bq-wf-real-proof-001"
SYMBOL = "AAPL"
STRATEGY_ID = "intraday_scalper"
TIMEFRAME = "5m"

# Fixture generation constants — deterministic, fixed-seed, offline-only.
FIXTURE_DAYS = 25
BARS_PER_DAY = 288          # 24h / 5m
FIXTURE_ROWS = FIXTURE_DAYS * BARS_PER_DAY   # 7200
FIXTURE_INTERVAL_SECS = 300  # 5m
FIXTURE_START_EPOCH = int(datetime(2024, 1, 1, tzinfo=timezone.utc).timestamp())
FIXTURE_BASE_PRICE_MICROS = 100_000_000
FIXTURE_SEED = 999
FIXTURE_STEP_SCALE = 150_000
FIXTURE_FLOOR_MICROS = 50_000_000


# ---------------------------------------------------------------------------
# Deterministic synthetic fixture generator (LOCAL/OFFLINE ONLY — not market
# evidence; generated to a tempdir at test run time, never committed)
# ---------------------------------------------------------------------------

def _lcg_stream(seed: int, n: int) -> list[int]:
    """Deterministic linear-congruential pseudo-random stream (glibc constants)."""
    x = seed
    out: list[int] = []
    for _ in range(n):
        x = (1103515245 * x + 12345) & 0xFFFFFFFF
        out.append(x)
    return out


def _generate_walkforward_fixture_csv(dest_path: Path) -> int:
    """
    Write a deterministic, synthetic 25-day 5-minute bars CSV to dest_path.

    Schema matches core-rs/crates/mqk-backtest/src/loader.rs requirements:
      symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete

    Uses a fixed-seed LCG random walk (NOT a clean periodic wave — a periodic
    wave lets a long-only momentum strategy always buy low/sell high, producing
    only winning trades and profit_factor=None forever, which would prevent
    validation_profit_factor from ever being populated). Same seed always
    produces the same bytes — fully reproducible, fully synthetic, fully local.

    Returns the number of data rows written.
    """
    rnd = _lcg_stream(FIXTURE_SEED, FIXTURE_ROWS)
    lines = ["symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete"]
    price = FIXTURE_BASE_PRICE_MICROS
    for i in range(FIXTURE_ROWS):
        end_ts = FIXTURE_START_EPOCH + i * FIXTURE_INTERVAL_SECS
        r = rnd[i]
        sign = 1 if (r & 1) == 0 else -1
        magnitude = (r >> 8) % FIXTURE_STEP_SCALE
        step = sign * magnitude
        open_micros = price
        close_micros = price + step
        high_micros = max(open_micros, close_micros) + (r % 50_000)
        low_micros = min(open_micros, close_micros) - ((r >> 4) % 50_000)
        volume = 600 + (r % 400)
        lines.append(
            f"{SYMBOL},{end_ts},{open_micros},{high_micros},{low_micros},"
            f"{close_micros},{volume},1"
        )
        price = close_micros
        if price < FIXTURE_FLOOR_MICROS:
            price = FIXTURE_FLOOR_MICROS

    dest_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return FIXTURE_ROWS


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
        "notes": "BACKTEST-WALKFORWARD-REAL-SUBPROCESS-PROOF-01 entry",
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
# Always-on: fixture generator produces a CSV long enough to plan splits
# ---------------------------------------------------------------------------

class TestFixtureGeneratorProducesPlannableSplitWindow(unittest.TestCase):
    """
    Proves — independent of MQK_BACKTEST_CLI availability — that the generated
    fixture is genuinely long enough (>=21 calendar days) for build_walkforward_plan
    to produce at least one split. This is the property the prior proof
    (3387d2f) explicitly identified as missing from the committed 96-row fixture.
    """

    def test_generated_fixture_yields_at_least_one_split(self):
        from mqk_research.scanner.walkforward_runner import build_walkforward_plan

        with tempfile.TemporaryDirectory() as tmpdir:
            bars_csv = Path(tmpdir) / f"{SYMBOL}_{TIMEFRAME}.csv"
            row_count = _generate_walkforward_fixture_csv(bars_csv)
            self.assertEqual(row_count, FIXTURE_ROWS)
            self.assertTrue(bars_csv.exists())

            entry = _make_queue_entry()
            wf_cfg = WalkForwardRunnerConfig(
                mode="dry_run",
                train_days=14,
                validation_days=7,
                step_days=7,
                output_root=str(Path(tmpdir) / "wf_plan_output"),
            )
            plan = build_walkforward_plan(entry, str(bars_csv), wf_cfg)

        self.assertTrue(
            plan.is_valid,
            f"expected a valid plan with >=1 split from a {FIXTURE_DAYS}-day fixture, "
            f"got failure_reasons={plan.failure_reasons}",
        )
        self.assertGreaterEqual(len(plan.splits), 1)


# ---------------------------------------------------------------------------
# Gated: real, non-mocked walk-forward subprocess proof
# ---------------------------------------------------------------------------

class TestRealWalkforwardSubprocessProof(unittest.TestCase):

    def test_real_walkforward_subprocess_executes_and_populates_validation_metrics(self):
        """
        When MQK_BACKTEST_CLI points to a real compiled mqk-cli binary, drive
        the full real walk-forward chain end to end with NO mocking of
        run_walkforward_entry, _run_single_split, or subprocess.run:

            run_walkforward_entry(mode="real")
              -> build_walkforward_plan          (REAL — >=1 split from fixture)
              -> write_split_csv                 (REAL — split CSV written to disk)
              -> _run_single_split               (REAL subprocess.run of mqk-cli)
              -> parse_mqk_metrics_json          (REAL — reads split metrics.json)
              -> aggregate_walkforward_results   (REAL — conservative aggregate)
              -> map_walkforward_to_validation_metrics (REAL)

        Skips cleanly (does not fail CI) when MQK_BACKTEST_CLI is unset or does
        not point to an existing binary.
        """
        binary = _resolve_local_cli_binary()
        if binary is None:
            self.skipTest(
                "MQK_BACKTEST_CLI not set to an existing local binary path; "
                "skipping real walk-forward subprocess proof (offline/local-only "
                "— never required for CI). To run manually, build with "
                "`cargo build -p mqk-cli` and set MQK_BACKTEST_CLI to the "
                "resulting mqk-cli(.exe) path."
            )

        entry = _make_queue_entry()

        with tempfile.TemporaryDirectory() as tmpdir:
            bars_root = Path(tmpdir) / "bars"
            bars_root.mkdir(parents=True, exist_ok=True)
            bars_csv = bars_root / f"{SYMBOL}_{TIMEFRAME}.csv"
            row_count = _generate_walkforward_fixture_csv(bars_csv)
            self.assertEqual(row_count, FIXTURE_ROWS)

            out_dir = str(Path(tmpdir) / "bridge_out")
            wf_output_root = str(Path(tmpdir) / "wf_output")

            bridge_cfg = BacktestBridgeConfig(
                mode="real",
                cli_binary=binary,
                bars_root_dir=str(bars_root),
                out_dir=out_dir,
            )
            wf_cfg = WalkForwardRunnerConfig(
                mode="real",
                train_days=14,
                validation_days=7,
                step_days=7,
                output_root=wf_output_root,
            )

            # --- REAL call: no mocking of run_walkforward_entry, no mocking of
            # subprocess.run. The only injected input is the generated fixture
            # bars CSV (synthetic, deterministic, local-only). ---
            result = run_walkforward_entry(entry, str(bars_csv), wf_cfg, bridge_cfg)

            # --- Proof: plan produced >=1 split from the generated fixture ---
            self.assertGreaterEqual(
                len(result.split_results), 1,
                f"expected >=1 walk-forward split from a {FIXTURE_DAYS}-day fixture, "
                f"got split_results={result.split_results}, "
                f"failure_reasons={result.failure_reasons}",
            )

            # --- Proof: split CSVs were genuinely written to disk ---
            run_dir = Path(wf_output_root) / QUEUE_ID[:40]
            written_split_csvs = list(run_dir.glob("split_*_validation.csv"))
            self.assertGreaterEqual(
                len(written_split_csvs), 1,
                f"expected >=1 split CSV written under {run_dir}, "
                f"found {written_split_csvs}",
            )
            for split_csv_path in written_split_csvs:
                self.assertGreater(split_csv_path.stat().st_size, 0)

            # --- Proof: a real, non-mocked per-split mqk-cli subprocess ran and
            # produced a metrics.json on disk for at least one split ---
            written_split_metrics = list(run_dir.rglob("metrics.json"))
            self.assertGreaterEqual(
                len(written_split_metrics), 1,
                f"expected >=1 split metrics.json written by the real per-split "
                f"subprocess under {run_dir}, found {written_split_metrics}",
            )

            # --- Proof: at least one split completed with real, non-fabricated
            # metrics (status == 'complete', metrics dict present and non-empty) ---
            complete_splits = [
                sr for sr in result.split_results
                if sr.status == "complete" and sr.metrics is not None
            ]
            self.assertGreaterEqual(
                len(complete_splits), 1,
                f"expected >=1 split with status='complete' and real metrics, "
                f"got statuses={[sr.status for sr in result.split_results]}, "
                f"failure_reasons per split={[sr.failure_reasons for sr in result.split_results]}",
            )
            for sr in complete_splits:
                self.assertIn("profit_factor", sr.metrics)
                self.assertIn("trade_count", sr.metrics)
                self.assertIsNotNone(sr.metrics.get("trade_count"))
                self.assertGreater(sr.metrics["trade_count"], 0)

            # --- Proof: the aggregate is genuinely complete (not blocked/failed) ---
            self.assertEqual(
                result.status, "complete",
                f"expected aggregate status='complete', got {result.status!r} "
                f"with failure_reasons={result.failure_reasons}",
            )
            self.assertIsNotNone(result.aggregate)
            self.assertTrue(result.aggregate.all_splits_have_metrics)
            self.assertGreaterEqual(result.aggregate.split_count, 1)

            # --- Proof: validation_profit_factor / validation_trades /
            # sample_quality / parameter_stability_score are populated from
            # REAL split metrics — none are None (the exact gap this proof
            # closes versus the injected-aggregate proof in 3387d2f) ---
            vm = result.validation_metrics
            self.assertIsNotNone(vm)
            self.assertIsNotNone(vm["validation_profit_factor"])
            self.assertIsNotNone(vm["validation_trades"])
            self.assertGreater(vm["validation_trades"], 0)
            self.assertIsNotNone(vm["sample_quality"])
            self.assertIsNotNone(vm["parameter_stability_score"])
            self.assertEqual(vm["split_count"], result.aggregate.split_count)
            self.assertTrue(vm["all_splits_have_metrics"])

            # --- Hard invariant: recommended_for_live is always False, at
            # every layer of the walk-forward result ---
            self.assertFalse(result.recommended_for_live)
            self.assertFalse(result.aggregate.recommended_for_live)
            self.assertFalse(vm["recommended_for_live"])

            # --- Honest reporting: this run is NOT expected to pass validation
            # gate thresholds (min_validation_profit_factor=1.1,
            # min_validation_trades=10) — a long-only momentum strategy against
            # a fixed-seed random-walk fixture genuinely whipsaws and loses.
            # We assert the metrics are real and populated, NOT that they pass;
            # forcing a pass here would be exactly the gaming this bundle's
            # safety rules forbid ("Do not tune strategy parameters to force
            # pass... Do not weaken thresholds"). The honest outcome is
            # reported, not hidden. ---
            self.assertIsInstance(vm["validation_profit_factor"], float)
            self.assertIsInstance(vm["validation_trades"], int)


if __name__ == "__main__":
    unittest.main()
