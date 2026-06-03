"""
EXP-PENNY-01A — penny scanner entry point.

Usage (requires explicit --dry-run and MQK_EXPERIMENTAL_ENGINE_ENABLED=true):

    $env:PYTHONPATH="research-py"
    $env:MQK_EXPERIMENTAL_ENGINE_ENABLED="true"
    $env:MQK_EXPERIMENTAL_ENGINE_MODE="scanner_only"
    $env:MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED="false"
    $env:MQK_EXPERIMENTAL_JOURNAL_DIR="exports/experimental/candidates"

    python -m experiments.exp_penny.run_scanner --dry-run --universe research-py/experiments/exp_penny/sample_universe.json

Scanner-only. No orders. No broker calls. No OMS writes.
"""
from __future__ import annotations

import argparse
import json
import sys

from experiments.exp_engine.scanner_runner import run
from experiments.exp_penny.scanner import PennyBreakoutScanner


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="EXP-PENNY-01A penny breakout scanner (Stage 1, scanner-only)",
        add_help=True,
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        required=True,
        help="Required. Scanner-only; no orders are placed.",
    )
    parser.add_argument(
        "--universe",
        required=True,
        metavar="PATH",
        help="Path to a JSON file containing the universe array.",
    )
    args, remaining = parser.parse_known_args(argv)

    universe_path = args.universe
    try:
        with open(universe_path, encoding="utf-8") as fh:
            raw = json.load(fh)
    except FileNotFoundError:
        print(f"ERROR: universe file not found: {universe_path}", file=sys.stderr)
        return 1
    except json.JSONDecodeError as exc:
        print(f"ERROR: universe file is not valid JSON: {exc}", file=sys.stderr)
        return 1

    if not isinstance(raw, list):
        print("ERROR: universe file must be a JSON array of objects.", file=sys.stderr)
        return 1

    scanner = PennyBreakoutScanner(universe=raw)

    # Pass --dry-run through to the base runner (required by scanner_runner.run).
    runner_argv = ["--dry-run"] + remaining
    return run(scanners=[scanner], argv=runner_argv)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
