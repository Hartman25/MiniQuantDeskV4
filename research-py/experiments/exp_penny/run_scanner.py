"""
EXP-PENNY-01A — penny scanner entry point.

Usage (requires explicit --dry-run and MQK_EXPERIMENTAL_ENGINE_ENABLED=true):

    $env:PYTHONPATH="research-py"
    $env:MQK_EXPERIMENTAL_ENGINE_ENABLED="true"
    $env:MQK_EXPERIMENTAL_ENGINE_MODE="scanner_only"
    $env:MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED="false"
    $env:MQK_EXPERIMENTAL_JOURNAL_DIR="exports/experimental/candidates"

    python -m experiments.exp_penny.run_scanner --dry-run --universe sample_universe.json
    python -m experiments.exp_penny.run_scanner --dry-run --universe sample_universe.csv
    python -m experiments.exp_penny.run_scanner --dry-run --universe samples/sample_finviz_export.csv --profile finviz
    python -m experiments.exp_penny.run_scanner --dry-run --universe samples/sample_tradingview_export.csv --profile tradingview

Scanner-only. No orders. No broker calls. No OMS writes.
Supports .json and .csv universe files via universe_loader.
Profile controls CSV column alias mapping (default: generic).
"""
from __future__ import annotations

import argparse
import sys

from experiments.exp_engine.scanner_runner import run
from experiments.exp_penny.scanner import PennyBreakoutScanner
from experiments.exp_penny.screener_profiles import SUPPORTED_PROFILES
from experiments.exp_penny.universe_loader import UniverseLoadError, load_universe


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
        help="Path to a .json or .csv file containing the universe.",
    )
    parser.add_argument(
        "--profile",
        default="generic",
        choices=list(SUPPORTED_PROFILES),
        help="CSV column alias profile. Default: generic (canonical headers).",
    )
    args, remaining = parser.parse_known_args(argv)

    try:
        universe = load_universe(args.universe, profile=args.profile)
    except UniverseLoadError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1

    scanner = PennyBreakoutScanner(universe=universe)

    # Pass --dry-run through to the base runner (required by scanner_runner.run).
    runner_argv = ["--dry-run"] + remaining
    return run(scanners=[scanner], argv=runner_argv)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
