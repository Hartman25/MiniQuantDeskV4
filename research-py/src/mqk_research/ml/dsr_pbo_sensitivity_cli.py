"""RESEARCH-MULTIPLE-TESTING-JUDGE-01 / P9 BKT-ROBUSTNESS-GAUNTLET-01 --
DSR/PBO sensitivity CLI.

Thin cross-language orchestration wrapper around the FROZEN, accepted
`build_multiple_testing_judge` (this module never redefines DSR/PBO
computation itself -- see `mqk_research.ml.multiple_testing_judge` and
`mqk_research.ml.multiple_testing_stats` for the actual statistics, both
out of scope here). Re-runs the judge for the SAME trial/experiment/
hypothesis population multiple times, varying ONLY `cscv_target_block_count`
-- an already-existing, caller-supplied `JudgeSpec` field.

FINAL-P9-AUTHORITY-BINDING-REPAIR-01 Section 1 ("DSR/PBO SENSITIVITY MUST
PRESERVE EXACT JUDGE SCOPE"): the caller-supplied `--judge-artifact-sha256`
is the EXACT, already-registered P7C-authorized `research_judge_artifacts`
row this sensitivity sweep must reuse the SCOPE of -- `experiment_id` and
`hypothesis_id`, INCLUDING `hypothesis_id = None` for a whole-experiment
scope. `build_multiple_testing_judge` legitimately accepts a `None`
`hypothesis_id` (whole-experiment comparison population); deriving scope
from `trial_id`'s own registered `hypothesis_id` instead (the prior
behavior) could silently narrow an authoritative whole-experiment comparison
population down to a single-hypothesis one while claiming to vary "only"
`cscv_target_block_count`. Every re-run in the grid therefore reuses the
EXACT `(experiment_id, hypothesis_id)` resolved from the registered judge
row -- never `trial_id`'s own hypothesis_id -- and every returned result
(evaluated, not_evaluable, or a resolved scope with no matching entry)
carries `authoritative_judge_artifact_sha256`/`authoritative_experiment_id`/
`authoritative_hypothesis_id` so the Rust caller can durably record exactly
which registered judge scope this sweep reused.

This is a safe, explicitly-anticipated use of the existing durable registry
write path: `ResearchResultStore.register_judge_artifact`'s own docstring
states "a re-run with an unchanged registry/population/protocol but a
genuinely different numeric result -- e.g. a different
cscv_target_block_count -- legitimately keeps the SAME judge_id while
producing a DIFFERENT canonical artifact and hash... Two rows may therefore
share a judge_id; this is expected, not a collision." Each re-run here is
therefore a normal, durable, individually-auditable judge build -- not a
throwaway/synthetic computation.

Called from Rust via subprocess (`mqk_backtest::dsr_pbo_sensitivity`) as
P9's DSR/PBO sensitivity scenario. Reports how much ONE candidate's own
Deflated Sharpe Ratio, and the comparison population's Probability of
Backtest Overfitting, move across a small grid of block counts.

Output: exactly one JSON object on stdout.
- Exit 0 with `{"status": "evaluated", ...}` or `{"status": "not_evaluable",
  "reason": ...}` -- both are legitimate, structured outcomes, not CLI
  failures (a `not_evaluable` outcome for a genuinely too-small comparison
  population is honest, not an error).
- Exit 1 with `{"status": "error", "reason": ...}` only for a genuine
  operational failure (bad arguments, unreadable registry, missing trial),
  so the Rust caller can fail closed rather than parse a Python traceback.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional

from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.ml.multiple_testing_judge import JudgeSpec, build_multiple_testing_judge


def _run_sensitivity(
    *, registry_db: Path, trial_id: str, judge_artifact_sha256: str, block_counts: List[int]
) -> Dict[str, Any]:
    store = ResearchResultStore(registry_db)
    trial = store.get_trial(trial_id)  # raises KeyError if unknown -- caller (main) reports as error
    strategy_id = trial["strategy_id"]

    # FINAL-P9-AUTHORITY-BINDING-REPAIR-01 Section 1: resolve the EXACT
    # registered judge scope from the caller-supplied, already-P7C-authorized
    # judge_artifact_sha256 -- raises KeyError (reported as a genuine error by
    # main()) if unknown, never silently falls back to trial_id's own scope.
    judge_row = store.get_judge_artifact(judge_artifact_sha256)
    experiment_id = judge_row["experiment_id"]
    hypothesis_id = judge_row["hypothesis_id"]

    if experiment_id != trial["experiment_id"]:
        raise ValueError(
            f"judge_artifact_sha256 {judge_artifact_sha256!r} is registered under "
            f"experiment_id {experiment_id!r}, but trial_id {trial_id!r} belongs to "
            f"experiment_id {trial['experiment_id']!r} -- refusing to rerun a judge scope "
            "that does not cover this trial's own experiment"
        )
    # hypothesis_id = None on the registered judge row means a whole-experiment
    # scope, which covers every hypothesis in this experiment including
    # trial_id's own -- only a NON-null judge hypothesis_id must match exactly.
    if hypothesis_id is not None and hypothesis_id != trial["hypothesis_id"]:
        raise ValueError(
            f"judge_artifact_sha256 {judge_artifact_sha256!r} is scoped to hypothesis_id "
            f"{hypothesis_id!r}, but trial_id {trial_id!r} belongs to hypothesis_id "
            f"{trial['hypothesis_id']!r} -- refusing to rerun a single-hypothesis judge scope "
            "that does not cover this trial"
        )

    authoritative_fields = {
        "authoritative_judge_artifact_sha256": judge_artifact_sha256,
        "authoritative_experiment_id": experiment_id,
        "authoritative_hypothesis_id": hypothesis_id,
    }

    per_block: List[Dict[str, Any]] = []
    dsr_values: List[float] = []
    pbo_values: List[float] = []

    for block_count in block_counts:
        spec = JudgeSpec(cscv_target_block_count=block_count)
        artifact = build_multiple_testing_judge(
            experiment_id=experiment_id,
            hypothesis_id=hypothesis_id,
            registry_db=registry_db,
            spec=spec,
        )
        dsr_entry = next(
            (r for r in artifact["dsr_results"] if r["trial_id"] == trial_id), None
        )
        if dsr_entry is None:
            return {
                "status": "not_evaluable",
                "strategy_id": strategy_id,
                "reason": (
                    f"trial_id {trial_id!r} has no dsr_results entry at "
                    f"block_count={block_count} (not part of the judged comparison scope)"
                ),
                "per_block": per_block,
                **authoritative_fields,
            }
        pbo_result = artifact["pbo_result"]
        entry = {
            "block_count": block_count,
            "judge_id": artifact["ids"]["judge_id"],
            "dsr_evaluable": bool(dsr_entry["evaluable"]),
            "deflated_sharpe_ratio": dsr_entry.get("deflated_sharpe_ratio"),
            "pbo_status": pbo_result["status"],
            "pbo": pbo_result.get("pbo"),
        }
        per_block.append(entry)

        if not dsr_entry["evaluable"] or entry["deflated_sharpe_ratio"] is None:
            return {
                "status": "not_evaluable",
                "strategy_id": strategy_id,
                "reason": (
                    f"DSR not evaluable for trial_id {trial_id!r} at block_count={block_count} "
                    f"(reason={dsr_entry.get('reason')!r})"
                ),
                "per_block": per_block,
                **authoritative_fields,
            }
        if pbo_result["status"] != "evaluated" or entry["pbo"] is None:
            return {
                "status": "not_evaluable",
                "strategy_id": strategy_id,
                "reason": f"PBO not evaluated at block_count={block_count} (status={pbo_result['status']!r})",
                "per_block": per_block,
                **authoritative_fields,
            }
        dsr_values.append(float(entry["deflated_sharpe_ratio"]))
        pbo_values.append(float(entry["pbo"]))

    return {
        "status": "evaluated",
        "trial_id": trial_id,
        "strategy_id": strategy_id,
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
        "block_counts": block_counts,
        "per_block": per_block,
        "dsr_min": min(dsr_values),
        "dsr_max": max(dsr_values),
        "dsr_range": max(dsr_values) - min(dsr_values),
        "pbo_min": min(pbo_values),
        "pbo_max": max(pbo_values),
        "pbo_range": max(pbo_values) - min(pbo_values),
        **authoritative_fields,
    }


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--registry-db", required=True, type=Path)
    parser.add_argument("--trial-id", required=True)
    parser.add_argument(
        "--judge-artifact-sha256",
        required=True,
        help=(
            "REQUIRED, no default: the P7C-authorized research_judge_artifacts."
            "judge_artifact_sha256 whose registered (experiment_id, hypothesis_id) scope this "
            "sensitivity sweep must reuse exactly -- never derived from trial_id's own "
            "hypothesis_id."
        ),
    )
    parser.add_argument(
        "--block-counts",
        required=True,
        help="comma-separated even integers >= 4, e.g. 8,10,12",
    )
    args = parser.parse_args(argv)

    try:
        block_counts = [int(x) for x in args.block_counts.split(",") if x.strip()]
        if not block_counts:
            raise ValueError("--block-counts must supply at least one value")
        result = _run_sensitivity(
            registry_db=args.registry_db,
            trial_id=args.trial_id,
            judge_artifact_sha256=args.judge_artifact_sha256,
            block_counts=block_counts,
        )
    except Exception as exc:  # noqa: BLE001 -- deliberate catch-all: fail closed with structured JSON,
        # never a raw Python traceback for the Rust caller to fail to parse.
        json.dump({"status": "error", "reason": str(exc)}, sys.stdout)
        return 1

    json.dump(result, sys.stdout)
    return 0  # "not_evaluable" is a legitimate structured outcome, not a CLI failure


if __name__ == "__main__":
    raise SystemExit(main())
