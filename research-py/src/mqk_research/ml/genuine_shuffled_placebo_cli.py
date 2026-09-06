"""FINAL-P9-ROBUSTNESS-SEMANTICS-01 -- genuine shuffled/null-control placebo
for P9 `BKT-ROBUSTNESS-GAUNTLET-01`.

Mirrors `p7a_p7b_economic_replay_stress_cli.py`'s own replay-authority
pattern exactly (same exact-`economic_eval_id`-binding and durable-hash
authentication -- reused directly, not re-implemented): resolves an
ALREADY-REGISTERED Research trial's EXACT succeeded attempt, re-verifies its
recorded `bars_csv`/`oos_predictions_csv`/`walk_forward_eval` inputs, and
reconstructs the exact baseline `EconomicWalkForwardSpec` -- then, unlike the
P7A/P7B stress replay, does NOT alter the economic protocol at all. Instead
it replaces the FROZEN OOS `ml_score` signal stream with a deterministic
shuffled/null version of itself (same marginal distribution of scores, but
decoupled from `decision_ts`/`symbol` -- a genuine random-label/shuffled
placebo control) and re-runs the SAME, unmodified `run_economic_walkforward`
entry point against it.

This is a required P9 upgrade over the prior `placebo_temporal_offset`
scenario: a temporal-delay placebo is not a shuffled/random-label placebo
(delaying still preserves each decision's own score value, only shifting
which bar it lands on) -- per the mission's own instruction, temporal delay
alone does not satisfy this requirement.

Determinism (this project avoids RNG in the engine, but a placebo control
inherently needs a well-defined, reproducible permutation): the shuffle seed
is derived ENTIRELY from `trial_id` via
`int.from_bytes(sha256(f"genuine_shuffled_placebo_v1:{trial_id}"...)[:8])` --
the SAME trial always produces the SAME permutation, with no wall-clock or
process-state dependency. The permutation is applied independently within
each `fold` (grouping by the OOS predictions' own `fold` column), so a
fold's placebo scores are exactly a permutation of that fold's own real
scores -- the marginal score distribution is preserved exactly (it is the
same multiset of values), only the (symbol, decision_ts) <-> score
association is destroyed.

Never calls `ResearchResultStore.register_trial`/`register_hypothesis` --
this is an EVALUATION SLICE of trial T, never a new trial. Never touches
holdout data (the stressed re-run still reports `holdout:
reserved_not_evaluated`, exactly like the P7A/P7B replay).

Output: exactly one JSON object on stdout.
- Exit 0 with `{"status": "evaluated", ...}` (candidate's real signal beat
  the placebo -> `passed: true`; placebo performed as well or better ->
  `passed: false`, reported honestly, never tuned away) or
  `{"status": "not_evaluable", "reason": ...}` (e.g. too few OOS rows to
  shuffle meaningfully).
- Exit 1 with `{"status": "error", "reason": ...}` for a genuine
  operational failure.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from dataclasses import fields
from pathlib import Path
from typing import Any, Dict, Optional

import numpy as np
import pandas as pd

from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.ml.economic_walkforward import (
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
    economic_protocol_identity,
    run_economic_walkforward,
)
from mqk_research.ml.execution_pricing import ExecutionPricingSpec
from mqk_research.ml.p7a_p7b_economic_replay_stress_cli import (
    ReplayAuthorityError,
    _recompute_economic_eval_id,
    _resolve_trial_economic_artifact,
    _verify_recorded_input,
)
from mqk_research.ml.util_hash import sha256_file
from mqk_research.ml.weight_to_share import WeightToShareSpec

PLACEBO_PROTOCOL_ID = "genuine_shuffled_placebo_v1"
_MIN_OOS_ROWS_FOR_SHUFFLE = 20


def _reconstruct_baseline_spec(econ: Dict[str, Any]) -> EconomicWalkForwardSpec:
    # See identical fix/comment in p7a_p7b_economic_replay_stress_cli.py's
    # own `_reconstruct_baseline_spec`: `tie_policy` is persisted into
    # `signal_policy` for cross_sectional_rank_* direction policies but is
    # not a `SignalPolicySpec.__init__` parameter.
    signal_policy_fields = {f.name for f in fields(SignalPolicySpec)}
    signal_policy = SignalPolicySpec(
        **{k: v for k, v in econ["signal_policy"].items() if k in signal_policy_fields}
    )
    cost_model = CostModelSpec(**econ["cost_model"])
    execution_pricing = ExecutionPricingSpec(**econ["execution_pricing"])
    annualization = AnnualizationSpec(**econ["annualization"])

    wts_identity = econ["weight_to_share"]
    weight_to_share: Optional[WeightToShareSpec]
    if wts_identity.get("weight_to_share_protocol_id") is None:
        weight_to_share = None
    else:
        weight_to_share = WeightToShareSpec(
            equity_usd=wts_identity["equity_usd"],
            max_target_qty=wts_identity.get("max_target_qty"),
            max_position_notional_usd=wts_identity.get("max_position_notional_usd"),
        )

    return EconomicWalkForwardSpec(
        signal_policy=signal_policy,
        cost_model=cost_model,
        execution_pricing=execution_pricing,
        weight_to_share=weight_to_share,
        annualization=annualization,
    )


def _placebo_seed(trial_id: str) -> int:
    digest = hashlib.sha256(f"{PLACEBO_PROTOCOL_ID}:{trial_id}".encode("utf-8")).digest()
    return int.from_bytes(digest[:8], "big")


def _shuffle_oos_predictions(oos_path: Path, trial_id: str, out_path: Path) -> Dict[str, Any]:
    """Deterministically permute `ml_score` within each `fold` group -- the
    exact same multiset of scores, reassigned to different (symbol,
    decision_ts) rows within that fold. Never shuffles ACROSS folds (a fold
    boundary is itself part of the frozen walk-forward chronology, never
    something a placebo control should cross)."""
    df = pd.read_csv(oos_path)
    required = {"fold", "symbol", "decision_ts", "ml_score"}
    missing = required - set(df.columns)
    if missing:
        raise ReplayAuthorityError(f"oos_predictions_csv missing required columns: {sorted(missing)}")

    seed = _placebo_seed(trial_id)
    rng = np.random.default_rng(seed)
    shuffled = df.copy()
    for fold_value in sorted(df["fold"].unique()):
        mask = df["fold"] == fold_value
        idx = df.index[mask].to_numpy()
        permuted_idx = rng.permutation(idx)
        shuffled.loc[idx, "ml_score"] = df.loc[permuted_idx, "ml_score"].to_numpy()

    shuffled.to_csv(out_path, index=False)
    return {
        "seed": seed,
        "rows_shuffled": int(len(df)),
        "distinct_folds": int(df["fold"].nunique()),
    }


def _run_shuffled_placebo(
    *,
    registry_db: Path,
    trial_id: str,
    economic_eval_id: str,
    placebo_out_dir: Path,
) -> Dict[str, Any]:
    store = ResearchResultStore(registry_db)
    trial = store.get_trial(trial_id)
    strategy_id = trial["strategy_id"]

    economic_path = _resolve_trial_economic_artifact(store, trial_id, economic_eval_id)
    econ = json.loads(economic_path.read_text(encoding="utf-8"))

    recomputed_economic_eval_id = _recompute_economic_eval_id(econ)
    if recomputed_economic_eval_id != economic_eval_id:
        raise ReplayAuthorityError(
            f"economic_walk_forward.json content hash disagrees with the durable registry "
            f"authority: recomputed economic_eval_id={recomputed_economic_eval_id!r} != "
            f"expected (registry result_id) {economic_eval_id!r} -- the artifact was mutated "
            "after the attempt was finalized; refusing to treat it as placebo authority"
        )
    declared_economic_eval_id = (econ.get("ids") or {}).get("economic_eval_id")
    if declared_economic_eval_id != economic_eval_id:
        raise ReplayAuthorityError(
            f"economic_walk_forward.json's own declared ids.economic_eval_id "
            f"({declared_economic_eval_id!r}) disagrees with the durable registry authority "
            f"({economic_eval_id!r}) -- refusing to treat it as placebo authority"
        )

    inputs = econ.get("inputs") or {}
    bars_record = inputs.get("bars_csv")
    oos_record = inputs.get("oos_predictions_csv")
    wf_record = inputs.get("walk_forward_eval")
    if not (bars_record and oos_record and wf_record):
        return {
            "status": "not_evaluable",
            "strategy_id": strategy_id,
            "reason": (
                "economic_walk_forward.json has no recorded inputs.bars_csv / "
                "inputs.oos_predictions_csv / inputs.walk_forward_eval -- predates "
                "REAL-RESEARCH-PROMOTION-E2E-CLOSURE-01 or was produced by the "
                "unregistered/diagnostic entry point"
            ),
        }

    bars_path = _verify_recorded_input("inputs.bars_csv", bars_record)
    oos_path = _verify_recorded_input("inputs.oos_predictions_csv", oos_record)
    wf_path = _verify_recorded_input("inputs.walk_forward_eval", wf_record)

    oos_row_count = len(pd.read_csv(oos_path))
    if oos_row_count < _MIN_OOS_ROWS_FOR_SHUFFLE:
        return {
            "status": "not_evaluable",
            "strategy_id": strategy_id,
            "reason": (
                f"oos_predictions_csv has only {oos_row_count} rows (< "
                f"{_MIN_OOS_ROWS_FOR_SHUFFLE}) -- too few for a meaningful shuffle"
            ),
        }

    baseline_spec = _reconstruct_baseline_spec(econ)
    baseline_identity = economic_protocol_identity(baseline_spec)
    recorded_identity = {
        "protocol_id": (econ.get("protocol") or {}).get("protocol_id"),
        "signal_policy": econ.get("signal_policy"),
        "cost_model": econ.get("cost_model"),
        "execution_pricing": econ.get("execution_pricing"),
        "weight_to_share": econ.get("weight_to_share"),
        "annualization": econ.get("annualization"),
    }
    if baseline_identity != recorded_identity:
        raise ReplayAuthorityError(
            "reconstructed baseline EconomicWalkForwardSpec's protocol identity does not match "
            "the recorded identity -- refusing to build a placebo control against a spec that "
            "does not exactly reproduce the original"
        )

    baseline_net_total_return = float(econ["aggregate"]["net_total_return"])

    placebo_out_dir.mkdir(parents=True, exist_ok=True)
    shuffled_oos_path = placebo_out_dir / "shuffled_oos_predictions.csv"
    shuffle_info = _shuffle_oos_predictions(oos_path, trial_id, shuffled_oos_path)

    placebo_path = run_economic_walkforward(
        placebo_out_dir,
        bars_csv=bars_path,
        spec=baseline_spec,
        walk_forward_eval_path=wf_path,
        oos_predictions_path=shuffled_oos_path,
        provenance_manifest=econ.get("bars_provenance"),
    )
    placebo = json.loads(placebo_path.read_text(encoding="utf-8"))
    placebo_net_total_return = float(placebo["aggregate"]["net_total_return"])

    # Per the mission's explicit hard stop: if the placebo performs as well
    # as or better than the real signal, that is a genuine finding to report
    # -- never tuned away.
    passed = placebo_net_total_return < baseline_net_total_return

    return {
        "status": "evaluated",
        "strategy_id": strategy_id,
        "trial_id": trial_id,
        "research_trial_id": trial_id,
        "passed": passed,
        "protocol_id": PLACEBO_PROTOCOL_ID,
        "baseline_economic_eval_id": economic_eval_id,
        "baseline_economic_artifact_sha256": sha256_file(economic_path),
        "placebo_economic_eval_id": placebo["ids"]["economic_eval_id"],
        "placebo_artifact_path": str(placebo_path),
        "placebo_artifact_sha256": sha256_file(placebo_path),
        "bars_csv_sha256": bars_record["sha256"],
        "oos_predictions_csv_sha256": oos_record["sha256"],
        "walk_forward_eval_sha256": wf_record["sha256"],
        "bars_provenance_hash": (econ.get("bars_provenance") or {}).get(
            "canonical_semantic_bars_hash"
        ),
        "shuffle_seed": shuffle_info["seed"],
        "shuffle_rows": shuffle_info["rows_shuffled"],
        "shuffle_distinct_folds": shuffle_info["distinct_folds"],
        "baseline_net_total_return": baseline_net_total_return,
        "placebo_net_total_return": placebo_net_total_return,
    }


def main(argv: Optional[list] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--registry-db", required=True, type=Path)
    parser.add_argument("--trial-id", required=True)
    parser.add_argument(
        "--economic-eval-id",
        required=True,
        help=(
            "REQUIRED, no default: the P7C-authorized economic_eval_id E this placebo control "
            "must bind to. Resolved against the trial's durable registry result_id (never 'the "
            "latest successful attempt')."
        ),
    )
    parser.add_argument("--placebo-out-dir", required=True, type=Path)
    args = parser.parse_args(argv)

    try:
        result = _run_shuffled_placebo(
            registry_db=args.registry_db,
            trial_id=args.trial_id,
            economic_eval_id=args.economic_eval_id,
            placebo_out_dir=args.placebo_out_dir,
        )
    except Exception as exc:  # noqa: BLE001 -- deliberate catch-all: fail closed with
        # structured JSON, never a raw Python traceback for the Rust caller to fail to
        # parse (mirrors p7a_p7b_economic_replay_stress_cli.py's own contract).
        json.dump({"status": "error", "reason": str(exc)}, sys.stdout)
        return 1

    json.dump(result, sys.stdout)
    return 0  # "not_evaluable" is a legitimate structured outcome, not a CLI failure


if __name__ == "__main__":
    raise SystemExit(main())
