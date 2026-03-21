from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

from .runner import batch_summary, create_batch, failed_jobs, rerun_failed_jobs, run_batch, run_single_job


def _print(payload: Any) -> int:
    print(json.dumps(payload, indent=2, sort_keys=True))
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="EXP-only distributed research backtest engine")
    subparsers = parser.add_subparsers(dest="command", required=True)

    create_parser = subparsers.add_parser("create-batch", help="expand a batch spec and persist queued research jobs")
    create_parser.add_argument("--spec", required=True, type=Path)
    create_parser.add_argument("--root", type=Path, default=None)

    run_parser = subparsers.add_parser("run-batch", help="expand and run a batch spec")
    run_parser.add_argument("--spec", required=True, type=Path)
    run_parser.add_argument("--root", type=Path, default=None)
    run_parser.add_argument("--workers", type=int, default=None)

    job_parser = subparsers.add_parser("run-job", help="run a single persisted job spec")
    job_parser.add_argument("--job-spec", required=True, type=Path)
    job_parser.add_argument("--root", type=Path, default=None)

    summary_parser = subparsers.add_parser("batch-summary", help="inspect a batch summary from the research store")
    summary_parser.add_argument("--batch-id", required=True)
    summary_parser.add_argument("--root", type=Path, default=None)

    failed_parser = subparsers.add_parser("failed-jobs", help="inspect failed jobs for a batch")
    failed_parser.add_argument("--batch-id", required=True)
    failed_parser.add_argument("--root", type=Path, default=None)

    rerun_parser = subparsers.add_parser("rerun-failed", help="rerun failed jobs for a batch")
    rerun_parser.add_argument("--batch-id", required=True)
    rerun_parser.add_argument("--root", type=Path, default=None)
    rerun_parser.add_argument("--workers", type=int, default=1)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    if args.command == "create-batch":
        return _print(create_batch(spec_path=args.spec, root=args.root))
    if args.command == "run-batch":
        return _print(run_batch(spec_path=args.spec, root=args.root, max_workers=args.workers))
    if args.command == "run-job":
        return _print(run_single_job(job_spec_path=args.job_spec, root=args.root))
    if args.command == "batch-summary":
        return _print(batch_summary(batch_id=args.batch_id, root=args.root))
    if args.command == "failed-jobs":
        return _print(failed_jobs(batch_id=args.batch_id, root=args.root))
    if args.command == "rerun-failed":
        return _print(rerun_failed_jobs(batch_id=args.batch_id, root=args.root, max_workers=args.workers))
    parser.error(f"unsupported command: {args.command}")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
