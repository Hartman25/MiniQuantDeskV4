"""
EXP-ENGINE-CORE-01 — experimental scanner runner.

Entry point for Stage 1 scanner-only operation.

Usage (requires explicit --dry-run flag and MQK_EXPERIMENTAL_ENGINE_ENABLED=true):
    python -m experiments.exp_engine.scanner_runner --dry-run

The runner:
- Checks MQK_EXPERIMENTAL_ENGINE_ENABLED before doing anything.
- Requires --dry-run; refuses to run without it.
- Accepts a list of scanner instances and journals their candidates.
- Never submits orders, never calls broker endpoints.
- Never imports from OMS tables, broker gateway, or any execution adapter.
"""
from __future__ import annotations

import argparse
import sys
from typing import Sequence

from experiments.exp_engine.candidate_journal import CandidateJournalWriter
from experiments.exp_engine.config import load_config
from experiments.exp_engine.scanner_base import ScannerBase


def run(
    scanners: list[ScannerBase],
    argv: Sequence[str] | None = None,
) -> int:
    parser = argparse.ArgumentParser(
        description="EXP-ENGINE-CORE-01 scanner runner (scanner-only, Stage 1)"
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        required=True,
        help="Required flag. Scanner-only mode; no orders are placed.",
    )
    args = parser.parse_args(argv)

    if not args.dry_run:
        print("ERROR: --dry-run is required. This runner never places orders.", file=sys.stderr)
        return 1

    cfg = load_config()

    if not cfg.enabled:
        print(
            "Experimental engine is disabled. "
            "Set MQK_EXPERIMENTAL_ENGINE_ENABLED=true to run.",
            file=sys.stderr,
        )
        return 1

    total_candidates = 0
    for scanner in scanners:
        with CandidateJournalWriter(cfg.journal_dir, scanner.engine_id) as writer:
            candidates = scanner.scan()
            for record in candidates:
                writer.append(record)
                total_candidates += 1
            print(
                f"[{scanner.engine_id}/{scanner.lane_id}] "
                f"scanned {len(candidates)} candidates -> {cfg.journal_dir}"
            )

    print(f"Scanner run complete. Total candidates journaled: {total_candidates}")
    print("No orders were placed. paper_order_id and live_order_id are null for all records.")
    return 0


if __name__ == "__main__":
    raise SystemExit(
        run(
            scanners=[],  # wire scanner instances here when implementing EXP-PENNY-01A
            argv=sys.argv[1:],
        )
    )
