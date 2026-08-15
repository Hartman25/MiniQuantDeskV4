from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any, Dict, Optional

from mqk_research.ml.multiple_testing_judge import JudgeSpec, run_multiple_testing_judge

from .runner import batch_summary, create_batch, default_db_path, default_root, failed_jobs, rerun_failed_jobs, run_batch, run_single_job


def _print(payload: Any) -> int:
    print(json.dumps(payload, indent=2, sort_keys=True))
    return 0


def run_judge_command(
    *,
    experiment_id: str,
    hypothesis_id: Optional[str],
    root: Optional[Path],
    out: Path,
    cscv_target_block_count: int,
) -> Dict[str, Any]:
    """Thin CLI-facing wrapper: resolve the same registry DB path every other
    `mqk-exp-dist` command uses (`root` -> `default_db_path(root)`), then
    delegate to the judge's own orchestration function. No statistical or
    registry logic lives here."""
    actual_root = (root or default_root()).resolve()
    registry_db = default_db_path(actual_root)
    spec = JudgeSpec(cscv_target_block_count=cscv_target_block_count)
    artifact_path = run_multiple_testing_judge(
        out, experiment_id=experiment_id, hypothesis_id=hypothesis_id, registry_db=registry_db, spec=spec
    )
    return {
        "judge_artifact_path": str(artifact_path),
        "registry_db": str(registry_db),
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
    }


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

    judge_parser = subparsers.add_parser(
        "judge", help="build the DSR/PBO multiple-testing judge artifact for a registered experiment"
    )
    judge_parser.add_argument("--experiment-id", required=True)
    judge_parser.add_argument("--hypothesis-id", default=None)
    judge_parser.add_argument("--root", type=Path, default=None)
    judge_parser.add_argument("--out", required=True, type=Path, help="output path for the judge artifact JSON")
    judge_parser.add_argument("--cscv-target-block-count", type=int, default=10)

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
    if args.command == "judge":
        return _print(
            run_judge_command(
                experiment_id=args.experiment_id,
                hypothesis_id=args.hypothesis_id,
                root=args.root,
                out=args.out,
                cscv_target_block_count=args.cscv_target_block_count,
            )
        )
    parser.error(f"unsupported command: {args.command}")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
