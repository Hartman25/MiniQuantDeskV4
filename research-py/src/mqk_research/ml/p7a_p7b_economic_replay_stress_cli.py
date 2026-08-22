"""P7A-P7B-ECONOMIC-REPLAY-STRESS-01 -- thin cross-language orchestration
wrapper that re-evaluates an ALREADY-REGISTERED Research trial's FROZEN OOS
prediction stream under an explicit, conservative P7A/P7B economic stress
configuration, using the REAL, accepted `run_economic_walkforward` entry
point. Never re-implements P7A execution pricing or P7B weight-to-share
translation itself, and never re-trains a model -- mirrors
`dsr_pbo_sensitivity_cli.py` / `real_research_promotion_e2e_cli.py`'s own
established pattern of a thin wrapper around real, already-accepted Python
functions, called from Rust via subprocess.

REPLAY AUTHORITY (no new storage, no new trial): `economic_walk_forward.json`
(written by `run_registered_economic_walkforward_eval` for the trial's
successful attempt, located via `ResearchResultStore.list_attempts`'s
`artifact_paths_json["economic_walk_forward"]`) already durably records,
via `file_record()` (`{path, bytes, sha256}`), the EXACT `bars_csv`,
`oos_predictions_csv` (the trial's frozen model output), and
`walk_forward_eval` artifacts that originally produced it, plus the exact
`bars_provenance` manifest and economic-protocol identity
(`signal_policy`/`cost_model`/`execution_pricing`/`weight_to_share`/
`annualization`). This script:

  1. resolves the EXACT succeeded attempt where `trial_id == T` and the
     durable registry's own `result_id == economic_eval_id` (the
     P7C-authorized `E`, a REQUIRED caller-supplied argument) -- never "the
     latest successful attempt"; a trial with zero or more than one
     matching succeeded attempt fails closed;
  2. authenticates that attempt's `economic_walk_forward.json` against that
     SAME durable `result_id` by recomputing its content hash from what is
     on disk TODAY (never trusting the file's own self-declared
     `ids.economic_eval_id`) -- a mutated artifact can never pass merely
     because its referenced input files still hash correctly;
  3. re-verifies EVERY recorded input file still exists at its recorded
     path with the recorded byte count AND sha256 (fail closed on any
     missing/mutated input -- a path alone is never authority);
  4. reconstructs the EXACT baseline `EconomicWalkForwardSpec` from the
     recorded identity (never inferring an omitted field optimistically),
     requires its re-derived `economic_protocol_identity` round-trips
     byte-for-byte against what was recorded, and requires it used the
     OFFICIAL P7A (`rust_conservative_bar_range_v1`) and P7B
     (`weight_to_share_v1`) protocols -- fails closed otherwise;
  5. validates the caller-supplied P7A/P7B stress knobs are GENUINELY
     adverse relative to the verified baseline (P7A slippage/volatility
     never lower, at least one strictly worse; P7B capacity caps never
     loosened/removed, at least one strictly tighter) -- a misconfigured
     "stress" that is not actually worse than baseline is rejected before
     any re-run;
  6. builds a STRESSED spec that overrides ONLY those caller-supplied
     P7A/P7B stress knobs on top of the verified baseline, changing
     nothing else;
  7. re-runs `run_economic_walkforward` with the SAME verified
     `bars_csv`/`walk_forward_eval_path`/`oos_predictions_path` (the
     trial's model output is FROZEN -- never re-read from
     features.csv/targets.csv, never re-trained) under the stressed spec,
     into a FRESH output directory (never touching the original evidence);
  8. judges the stressed result against a caller-supplied, required (no
     default) conservative max-drawdown ceiling.

Never calls `ResearchResultStore.register_trial`/`register_hypothesis` --
this is an EVALUATION SLICE of trial T, never a new trial (`trial != attempt
!= evaluation slice`).

Output: exactly one JSON object on stdout.
- Exit 0 with `{"status": "evaluated", ...}` (a genuine pass/fail judgment)
  or `{"status": "not_evaluable", "reason": ...}` (baseline does not
  qualify for P7A/P7B stress evidence -- e.g. diagnostic pricing model).
- Exit 1 with `{"status": "error", "reason": ...}` for a genuine
  operational failure (bad trial_id, missing/mutated input, registry
  unavailable) -- never a raw Python traceback for the Rust caller to fail
  to parse.
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import replace
from pathlib import Path
from typing import Any, Dict, Optional

from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.ml.economic_walkforward import (
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
    economic_protocol_identity,
    run_economic_walkforward,
)
from mqk_research.ml.execution_pricing import (
    EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    ExecutionPricingSpec,
)
from mqk_research.ml.util_hash import file_record, sha256_file, sha256_json
from mqk_research.ml.weight_to_share import WEIGHT_TO_SHARE_PROTOCOL_ID_V1, WeightToShareSpec

STRESS_PROTOCOL_ID = "p7a_p7b_economic_replay_stress_v1"

# Keys `run_economic_walkforward` writes into `economic_walk_forward.json`
# AFTER computing `economic_eval_id = sha256_json(out)` (see
# economic_walkforward.py, `out["ids"] = ...`) or that a later caller
# (`economic_registry_integration.run_registered_economic_walkforward_eval`)
# appends to the file post-hoc (`out["registry"] = {...}`) -- neither was
# part of the original hash basis, so both must be excluded when
# recomputing that hash from the file as it exists on disk today.
_ECONOMIC_EVAL_ID_EXCLUDED_KEYS = frozenset({"ids", "registry"})


class ReplayAuthorityError(Exception):
    """A recorded input's path/bytes/sha256 could not be re-verified, the
    economic artifact's content hash disagrees with its durable registry
    authority, or the reconstructed baseline spec's protocol identity does
    not round-trip -- fail closed, never refetch-and-assume-identical or
    trust a self-declared id."""


def _recompute_economic_eval_id(econ: Dict[str, Any]) -> str:
    """Recompute `economic_eval_id` exactly as `run_economic_walkforward`
    originally did, from the artifact's CURRENT on-disk content -- never
    trusting the file's own self-declared `ids.economic_eval_id` field,
    which could be forged/mutated independently of the surrounding content
    it is supposed to attest to."""
    basis = {k: v for k, v in econ.items() if k not in _ECONOMIC_EVAL_ID_EXCLUDED_KEYS}
    return sha256_json(basis)


def _verify_recorded_input(name: str, record: Dict[str, Any]) -> Path:
    path_str = record.get("path")
    if not path_str:
        raise ReplayAuthorityError(f"{name}: no path recorded in economic_walk_forward.json")
    path = Path(path_str)
    if not path.exists():
        raise ReplayAuthorityError(
            f"{name}: recorded input no longer exists at {path} -- refusing to refetch or "
            "substitute; the exact original file is required"
        )
    actual_bytes = path.stat().st_size
    if record.get("bytes") != actual_bytes:
        raise ReplayAuthorityError(
            f"{name}: byte count changed since the original run (recorded {record.get('bytes')}, "
            f"actual {actual_bytes}) -- refusing to replay against a mutated input"
        )
    actual_sha256 = sha256_file(path)
    if record.get("sha256") != actual_sha256:
        raise ReplayAuthorityError(
            f"{name}: sha256 changed since the original run (recorded {record.get('sha256')!r}, "
            f"actual {actual_sha256!r}) -- refusing to replay against a mutated input"
        )
    return path


def _resolve_trial_economic_artifact(
    store: ResearchResultStore, trial_id: str, economic_eval_id: str
) -> Path:
    """Resolve the EXACT succeeded attempt where `trial_id == T` and the
    durable registry's own `result_id == economic_eval_id` (the
    P7C-authorized `E`) -- never "the latest successful attempt". `result_id`
    is written once, atomically, by `ResearchResultStore.finalize_attempt`
    at the time this attempt originally succeeded, and a terminal attempt can
    never be reopened or refinalized (see `finalize_attempt`'s own fail-closed
    contract) -- so `result_id` is durable, external authority, independent
    of whatever the `economic_walk_forward.json` file on disk says about
    itself today. A trial with zero or more-than-one succeeded attempt
    matching `economic_eval_id` fails closed (ambiguity is never resolved by
    guessing)."""
    trial = store.get_trial(trial_id)  # raises KeyError if unknown
    matching = [
        a
        for a in store.list_attempts(trial_id)
        if a["status"] == "succeeded" and a["result_id"] == economic_eval_id
    ]
    if not matching:
        raise ReplayAuthorityError(
            f"trial_id {trial_id!r} (strategy_id={trial['strategy_id']!r}) has no succeeded "
            f"attempt whose registered result_id equals economic_eval_id {economic_eval_id!r} "
            "-- refusing to guess which attempt to replay"
        )
    if len(matching) > 1:
        raise ReplayAuthorityError(
            f"trial_id {trial_id!r} has {len(matching)} succeeded attempts registered under "
            f"the SAME economic_eval_id {economic_eval_id!r} -- ambiguous, refusing to guess "
            "which one to replay"
        )
    attempt = matching[0]
    artifact_paths = json.loads(attempt["artifact_paths_json"] or "{}")
    economic_path_str = artifact_paths.get("economic_walk_forward")
    if not economic_path_str:
        raise ReplayAuthorityError(
            f"trial_id {trial_id!r}'s matching succeeded attempt has no recorded "
            "'economic_walk_forward' artifact path"
        )
    economic_path = Path(economic_path_str)
    if not economic_path.exists():
        raise ReplayAuthorityError(
            f"trial_id {trial_id!r}'s recorded economic_walk_forward.json no longer exists "
            f"at {economic_path}"
        )
    return economic_path


def _classify_cap_transition(baseline: Optional[float], stress: Optional[float]) -> str:
    """P7B stress-cap genuineness classification, per the mission's exact
    rules: `None` means "no cap" (looser than any finite cap)."""
    if baseline is None and stress is None:
        return "none_to_none"
    if baseline is None and stress is not None:
        return "tighter"  # a cap introduced where none existed before
    if baseline is not None and stress is None:
        return "forbidden_looser"  # an existing cap removed entirely
    if stress < baseline:  # type: ignore[operator]
        return "tighter"
    if stress > baseline:  # type: ignore[operator]
        return "forbidden_looser"
    return "unchanged"


def _validate_genuine_p7a_p7b_adversity(
    baseline_spec: EconomicWalkForwardSpec,
    *,
    stress_execution_slippage_bps: int,
    stress_execution_volatility_mult_bps: int,
    stress_max_target_qty: Optional[int],
    stress_max_position_notional_usd: Optional[float],
) -> Optional[str]:
    """Returns a fail-closed reason string if the caller-supplied stress
    configuration is not GENUINE adversity relative to the verified
    baseline, or `None` if it is. Never invents ADV/liquidity impact -- only
    validates the direction/magnitude of the caller's own P7A/P7B knobs."""
    baseline_slippage = baseline_spec.execution_pricing.slippage_bps
    baseline_volatility = baseline_spec.execution_pricing.volatility_mult_bps
    if (
        stress_execution_slippage_bps < baseline_slippage
        or stress_execution_volatility_mult_bps < baseline_volatility
    ):
        return (
            f"P7A stress is not adverse: stress execution_pricing "
            f"(slippage_bps={stress_execution_slippage_bps}, "
            f"volatility_mult_bps={stress_execution_volatility_mult_bps}) must be >= baseline "
            f"(slippage_bps={baseline_slippage}, volatility_mult_bps={baseline_volatility}) "
            "in both dimensions -- a stress replay can never be less adverse than the baseline "
            "it stresses"
        )
    if (
        stress_execution_slippage_bps <= baseline_slippage
        and stress_execution_volatility_mult_bps <= baseline_volatility
    ):
        return (
            "P7A stress is not genuinely adverse: stress execution_pricing "
            f"(slippage_bps={stress_execution_slippage_bps}, "
            f"volatility_mult_bps={stress_execution_volatility_mult_bps}) equals the baseline "
            f"(slippage_bps={baseline_slippage}, volatility_mult_bps={baseline_volatility}) in "
            "both dimensions -- at least one must be strictly worse"
        )

    baseline_wts = baseline_spec.weight_to_share
    baseline_max_qty = baseline_wts.max_target_qty if baseline_wts is not None else None
    baseline_max_notional = baseline_wts.max_position_notional_usd if baseline_wts is not None else None

    qty_class = _classify_cap_transition(baseline_max_qty, stress_max_target_qty)
    notional_class = _classify_cap_transition(baseline_max_notional, stress_max_position_notional_usd)

    if qty_class == "forbidden_looser":
        return (
            f"P7B stress is looser than baseline: max_target_qty baseline={baseline_max_qty!r} "
            f"stress={stress_max_target_qty!r} -- a capacity/sizing cap can never be removed or "
            "loosened by a stress replay"
        )
    if notional_class == "forbidden_looser":
        return (
            "P7B stress is looser than baseline: max_position_notional_usd "
            f"baseline={baseline_max_notional!r} stress={stress_max_position_notional_usd!r} -- "
            "a capacity/sizing cap can never be removed or loosened by a stress replay"
        )
    if qty_class != "tighter" and notional_class != "tighter":
        return (
            "P7B stress has no genuine tightening: neither max_target_qty "
            f"(baseline={baseline_max_qty!r} stress={stress_max_target_qty!r}) nor "
            f"max_position_notional_usd (baseline={baseline_max_notional!r} "
            f"stress={stress_max_position_notional_usd!r}) became strictly tighter -- at least "
            "one real existing capacity/sizing cap must become strictly tighter, and both caps "
            "remaining None is not valid P7B stress"
        )
    return None


def _reconstruct_baseline_spec(econ: Dict[str, Any]) -> EconomicWalkForwardSpec:
    signal_policy = SignalPolicySpec(**econ["signal_policy"])
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


def _run_replay_stress(
    *,
    registry_db: Path,
    trial_id: str,
    economic_eval_id: str,
    stress_out_dir: Path,
    stress_execution_slippage_bps: int,
    stress_execution_volatility_mult_bps: int,
    stress_max_target_qty: Optional[int],
    stress_max_position_notional_usd: Optional[float],
    max_drawdown_ceiling: float,
) -> Dict[str, Any]:
    store = ResearchResultStore(registry_db)
    trial = store.get_trial(trial_id)
    strategy_id = trial["strategy_id"]

    # Section B: bind to the EXACT succeeded attempt whose durable registry
    # result_id == economic_eval_id (the P7C-authorized E) -- never "the
    # latest successful attempt".
    economic_path = _resolve_trial_economic_artifact(store, trial_id, economic_eval_id)
    econ = json.loads(economic_path.read_text(encoding="utf-8"))

    # Section C: authenticate the baseline artifact against DURABLE registry
    # authority (economic_eval_id, recorded at finalize_attempt time,
    # independent of the mutable file on disk) -- recompute the content hash
    # from what is on disk TODAY rather than trusting the file's own
    # self-declared `ids.economic_eval_id`. A mutated economic_walk_forward.json
    # (aggregate/fold/inputs/spec-identity content) can never pass this check
    # merely because its referenced input files still hash correctly.
    recomputed_economic_eval_id = _recompute_economic_eval_id(econ)
    if recomputed_economic_eval_id != economic_eval_id:
        raise ReplayAuthorityError(
            f"economic_walk_forward.json content hash disagrees with the durable registry "
            f"authority: recomputed economic_eval_id={recomputed_economic_eval_id!r} != "
            f"expected (registry result_id) {economic_eval_id!r} -- the artifact was mutated "
            "after the attempt was finalized; refusing to treat it as replay authority"
        )
    declared_economic_eval_id = (econ.get("ids") or {}).get("economic_eval_id")
    if declared_economic_eval_id != economic_eval_id:
        raise ReplayAuthorityError(
            f"economic_walk_forward.json's own declared ids.economic_eval_id "
            f"({declared_economic_eval_id!r}) disagrees with the durable registry authority "
            f"({economic_eval_id!r}) -- refusing to treat it as replay authority"
        )
    baseline_economic_eval_id = economic_eval_id

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

    baseline_spec = _reconstruct_baseline_spec(econ)
    if not baseline_spec.execution_pricing.is_official_parity_model:
        return {
            "status": "not_evaluable",
            "strategy_id": strategy_id,
            "reason": (
                f"baseline execution_pricing.pricing_model_id "
                f"{baseline_spec.execution_pricing.pricing_model_id!r} != required "
                f"{EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1!r} -- this trial's economic "
                "evidence was never produced under the official P7A parity model, so a stress "
                "replay of it can never count as P7A/P7B stress evidence"
            ),
        }
    if (
        baseline_spec.weight_to_share is None
        or baseline_spec.weight_to_share.protocol_id != WEIGHT_TO_SHARE_PROTOCOL_ID_V1
    ):
        return {
            "status": "not_evaluable",
            "strategy_id": strategy_id,
            "reason": (
                "baseline weight_to_share_protocol_id is not the official "
                f"{WEIGHT_TO_SHARE_PROTOCOL_ID_V1!r} -- this trial's economic evidence never "
                "engaged the official P7B translation, so a stress replay of it can never count "
                "as P7A/P7B stress evidence"
            ),
        }
    non_discrete_folds = [
        f["fold"]
        for f in econ.get("folds", [])
        if f.get("discrete_economics_protocol_id") != "discrete_share_economic_path_v1"
    ]
    if non_discrete_folds:
        return {
            "status": "not_evaluable",
            "strategy_id": strategy_id,
            "reason": (
                f"folds {non_discrete_folds} lack the discrete_share_economic_path_v1 marker -- "
                "an evidence-only weight_to_share translation is not sufficient"
            ),
        }

    # Section E: exact spec reconstruction -- the reconstructed baseline
    # spec's own canonical protocol identity must round-trip byte-for-byte
    # against what was actually recorded, never an optimistic assumption
    # that reconstruction preserved every field.
    baseline_protocol_identity = economic_protocol_identity(baseline_spec)
    recorded_identity = {
        "protocol_id": (econ.get("protocol") or {}).get("protocol_id"),
        "signal_policy": econ.get("signal_policy"),
        "cost_model": econ.get("cost_model"),
        "execution_pricing": econ.get("execution_pricing"),
        "weight_to_share": econ.get("weight_to_share"),
        "annualization": econ.get("annualization"),
    }
    if baseline_protocol_identity != recorded_identity:
        raise ReplayAuthorityError(
            "reconstructed baseline EconomicWalkForwardSpec's protocol identity does not match "
            "the recorded identity -- refusing to replay against a spec that does not exactly "
            f"reproduce the original: reconstructed={baseline_protocol_identity!r} "
            f"recorded={recorded_identity!r}"
        )

    # Section F: genuine P7A/P7B adversity -- a stress configuration that is
    # not strictly worse than the verified baseline is a caller
    # misconfiguration, never silently accepted.
    adversity_error = _validate_genuine_p7a_p7b_adversity(
        baseline_spec,
        stress_execution_slippage_bps=stress_execution_slippage_bps,
        stress_execution_volatility_mult_bps=stress_execution_volatility_mult_bps,
        stress_max_target_qty=stress_max_target_qty,
        stress_max_position_notional_usd=stress_max_position_notional_usd,
    )
    if adversity_error is not None:
        return {"status": "error", "strategy_id": strategy_id, "reason": adversity_error}

    stressed_spec = replace(
        baseline_spec,
        execution_pricing=replace(
            baseline_spec.execution_pricing,
            slippage_bps=stress_execution_slippage_bps,
            volatility_mult_bps=stress_execution_volatility_mult_bps,
        ),
        weight_to_share=replace(
            baseline_spec.weight_to_share,
            max_target_qty=stress_max_target_qty,
            max_position_notional_usd=stress_max_position_notional_usd,
        ),
    )

    stress_out_dir.mkdir(parents=True, exist_ok=True)
    stressed_path = run_economic_walkforward(
        stress_out_dir,
        bars_csv=bars_path,
        spec=stressed_spec,
        walk_forward_eval_path=wf_path,
        oos_predictions_path=oos_path,
        provenance_manifest=econ.get("bars_provenance"),
    )
    stressed = json.loads(stressed_path.read_text(encoding="utf-8"))
    stressed_max_drawdown = float(stressed["aggregate"]["max_drawdown"])
    passed = stressed_max_drawdown >= -abs(max_drawdown_ceiling)

    return {
        "status": "evaluated",
        "strategy_id": strategy_id,
        "trial_id": trial_id,
        "research_trial_id": trial_id,
        "passed": passed,
        "protocol_id": STRESS_PROTOCOL_ID,
        "baseline_economic_eval_id": baseline_economic_eval_id,
        "baseline_economic_artifact_sha256": sha256_file(economic_path),
        "baseline_protocol_identity": baseline_protocol_identity,
        "stressed_economic_eval_id": stressed["ids"]["economic_eval_id"],
        "stressed_artifact_path": str(stressed_path),
        "stressed_artifact_sha256": sha256_file(stressed_path),
        "bars_csv_sha256": bars_record["sha256"],
        "oos_predictions_csv_sha256": oos_record["sha256"],
        "walk_forward_eval_sha256": wf_record["sha256"],
        "bars_provenance_hash": (econ.get("bars_provenance") or {}).get(
            "canonical_semantic_bars_hash"
        ),
        "stress_spec": {
            "execution_pricing_slippage_bps": stress_execution_slippage_bps,
            "execution_pricing_volatility_mult_bps": stress_execution_volatility_mult_bps,
            "max_target_qty": stress_max_target_qty,
            "max_position_notional_usd": stress_max_position_notional_usd,
        },
        "max_drawdown_ceiling": max_drawdown_ceiling,
        "stressed_max_drawdown": stressed_max_drawdown,
    }


def main(argv: Optional[list] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--registry-db", required=True, type=Path)
    parser.add_argument("--trial-id", required=True)
    parser.add_argument(
        "--economic-eval-id",
        required=True,
        help=(
            "REQUIRED, no default: the P7C-authorized economic_eval_id E this replay must bind "
            "to. Resolved against the trial's durable registry result_id (never 'the latest "
            "successful attempt') -- a trial with no succeeded attempt registered under this "
            "exact economic_eval_id fails closed."
        ),
    )
    parser.add_argument("--stress-out-dir", required=True, type=Path)
    parser.add_argument("--stress-execution-slippage-bps", required=True, type=int)
    parser.add_argument("--stress-execution-volatility-mult-bps", required=True, type=int)
    parser.add_argument("--stress-max-target-qty", type=int, default=None)
    parser.add_argument("--stress-max-position-notional-usd", type=float, default=None)
    parser.add_argument(
        "--max-drawdown-ceiling",
        required=True,
        type=float,
        help=(
            "REQUIRED, no default: the conservative max-drawdown fraction (e.g. 0.30) the "
            "stressed replay must not breach to pass. No accepted policy value exists in this "
            "codebase -- the caller (production finalizer / CLI operator) must supply one "
            "explicitly every invocation."
        ),
    )
    args = parser.parse_args(argv)

    try:
        result = _run_replay_stress(
            registry_db=args.registry_db,
            trial_id=args.trial_id,
            economic_eval_id=args.economic_eval_id,
            stress_out_dir=args.stress_out_dir,
            stress_execution_slippage_bps=args.stress_execution_slippage_bps,
            stress_execution_volatility_mult_bps=args.stress_execution_volatility_mult_bps,
            stress_max_target_qty=args.stress_max_target_qty,
            stress_max_position_notional_usd=args.stress_max_position_notional_usd,
            max_drawdown_ceiling=args.max_drawdown_ceiling,
        )
    except Exception as exc:  # noqa: BLE001 -- deliberate catch-all: fail closed with
        # structured JSON, never a raw Python traceback for the Rust caller to fail to
        # parse (mirrors dsr_pbo_sensitivity_cli.py's own contract).
        json.dump({"status": "error", "reason": str(exc)}, sys.stdout)
        return 1

    json.dump(result, sys.stdout)
    return 0  # "not_evaluable" is a legitimate structured outcome, not a CLI failure


if __name__ == "__main__":
    raise SystemExit(main())
