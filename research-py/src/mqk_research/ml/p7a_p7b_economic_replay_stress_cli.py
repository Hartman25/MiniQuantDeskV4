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
from dataclasses import fields, replace
from pathlib import Path
from typing import Any, Dict, Optional

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
from mqk_research.ml.execution_pricing import (
    EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    ExecutionPricingSpec,
)
from mqk_research.ml.replay_authority import (
    ReplayAuthorityError,
    recompute_economic_eval_id as _recompute_economic_eval_id,
    resolve_trial_economic_artifact as _resolve_trial_economic_artifact,
    verify_recorded_input as _verify_recorded_input,
)
from mqk_research.ml.native_signal_registry_integration import (
    NativeSignalError,
    _load_signals,
)
from mqk_research.ml.util_hash import file_record, sha256_file, sha256_json
from mqk_research.ml.weight_to_share import WEIGHT_TO_SHARE_PROTOCOL_ID_V1, WeightToShareSpec

STRESS_PROTOCOL_ID = "p7a_p7b_economic_replay_stress_v1"

# `ReplayAuthorityError`, `_recompute_economic_eval_id`,
# `_resolve_trial_economic_artifact`, `_verify_recorded_input` are re-exported
# here for backward compatibility (`genuine_shuffled_placebo_cli.py` imports
# them from THIS module). Canonical implementation now lives in
# `mqk_research.ml.replay_authority`
# (W06-A-P9-REPLAY-SOURCE-AUTHORITY-REPAIR-WAVE-02, Patch R1) -- behavior is
# byte-for-byte unchanged; see that module's docstring.


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
    capital_fraction_stress: bool = False,
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

    if capital_fraction_stress:
        # The P7B tightening is the recomputed (smaller) capital-fraction quantity, validated
        # separately; a post-sizing cap is never how a capital-fraction stress is expressed.
        if stress_max_target_qty is not None or stress_max_position_notional_usd is not None:
            return (
                "capital-fraction stress sizing forbids stress_max_target_qty / "
                "stress_max_position_notional_usd: a USD cap applied to a baseline-sized quantity "
                "is refused by the exact-target replay and is not a half-exposure stress; the "
                "independent baseline safety caps are carried unchanged"
            )
        return None

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
    # `run_economic_walkforward` persists `signal_policy` with an additional
    # `tie_policy` key for cross_sectional_rank_* direction policies
    # (identity-bearing there, but not a `SignalPolicySpec.__init__`
    # parameter -- see economic_walkforward.py's own comment at the
    # `tie_policy` assignment site). Filtering to the dataclass's own field
    # names reconstructs the exact same spec `SignalPolicySpec(**econ[...])`
    # would for every OTHER direction policy (which never has that extra
    # key) while no longer crashing on this one -- discovered while
    # constructing W06-A-P9-CANONICAL-CLI-AUTHORITY-REPAIR-01's synthetic
    # E2E fixture (Wave06 LIQ-01/VOL-01 are cross_sectional_rank_* trials).
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


def _build_capital_fraction_stress_oos(
    *,
    stress_sizing: Dict[str, Any],
    wf_eval: Dict[str, Any],
    baseline_oos_path: Path,
    baseline_spec: EconomicWalkForwardSpec,
    strategy_id: str,
    stress_out_dir: Path,
) -> "tuple[Path, Dict[str, Any]]":
    """Half-exposure stress as a RECOMPUTED target quantity, never a cap on the baseline one.

    `stress_sizing` names an additional native signal stream of the SAME registered trial that the
    Rust emitter produced with `FixedInitialCapitalFractionV1` at a strictly smaller
    `allocation_fraction_bps` on the same immutable capital and bars. It is verified through the
    accepted stream loader (so every positive quantity is an engine-resolved entry), must carry
    the identical decisions as the authenticated baseline stream, and may never hold more than the
    baseline. Its quantities replace the baseline OOS `target_qty` row for row; nothing else changes.
    """
    required = {"scenario_id", "allocation_fraction_bps", "signals_csv", "signals_meta",
                "expected_semantic_fingerprint"}
    if set(stress_sizing) != required:
        raise ReplayAuthorityError(f"stress_sizing must have exactly the keys {sorted(required)}")
    stress_bps = stress_sizing["allocation_fraction_bps"]
    if type(stress_bps) is not int or not 1 <= stress_bps <= 10_000:
        raise ReplayAuthorityError("stress allocation_fraction_bps must be an explicit integer 1..=10000")

    wf_inputs = wf_eval.get("inputs") or {}
    base_csv_record = wf_inputs.get("native_signals_csv")
    base_meta_record = wf_inputs.get("native_signals_meta")
    if not (base_csv_record and base_meta_record):
        raise ReplayAuthorityError(
            "baseline walk-forward artifact records no native signal stream: a capital-fraction stress "
            "needs the authenticated baseline stream to prove the decisions are unchanged"
        )
    base_csv = _verify_recorded_input("inputs.native_signals_csv", base_csv_record)
    base_meta_path = _verify_recorded_input("inputs.native_signals_meta", base_meta_record)
    base_meta = json.loads(base_meta_path.read_text(encoding="utf-8"))
    base_block = base_meta.get("sizing")
    if not isinstance(base_block, dict) or base_block.get("policy_id") != "fixed_initial_capital_fraction_v1":
        raise ReplayAuthorityError("the baseline trial is not a capital-fraction trial; no capital-fraction stress exists")
    base_bps = int(base_block["allocation_fraction_bps"])
    if stress_bps >= base_bps:
        raise ReplayAuthorityError(
            f"capital-fraction stress is not adverse: stress {stress_bps} bps must be strictly below the "
            f"baseline {base_bps} bps on the same immutable capital"
        )

    equity_usd = baseline_spec.weight_to_share.equity_usd
    expected_sizing = {
        "policy_id": base_block["policy_id"],
        "allocation_fraction_bps": stress_bps,
        "initial_allocated_capital_micros": int(base_block["initial_allocated_capital_micros"]),
        "max_target_qty": base_block.get("max_target_qty"),
        "max_position_notional_usd": base_block.get("max_position_notional_usd"),
    }
    stress_csv = Path(stress_sizing["signals_csv"])
    stress_meta_path = Path(stress_sizing["signals_meta"])
    try:
        stress_signals, stress_meta = _load_signals(
            stress_csv, stress_meta_path, strategy_id=strategy_id, symbol=str(base_meta["symbol"]),
            backtest_bars_sha256=str(base_meta["bars_csv_sha256"]),
            expected_timeframe_secs=int(base_meta["timeframe_secs"]),
            expected_semantic_fingerprint=str(stress_sizing["expected_semantic_fingerprint"]),
            expected_required_history_bars=int(base_meta["required_history_bars"]),
            equity_usd=equity_usd, expected_capital_sizing=expected_sizing,
        )
    except NativeSignalError as exc:
        raise ReplayAuthorityError(f"stress signal stream rejected: {exc}") from exc
    if stress_meta["semantic_fingerprint"] == base_meta["semantic_fingerprint"]:
        raise ReplayAuthorityError("stress stream carries the baseline wrapper fingerprint: sizing was not re-resolved")

    base_signals = pd.read_csv(base_csv).sort_values("decision_ts", kind="mergesort").reset_index(drop=True)
    stress_frame = stress_signals[["decision_ts", "target_qty_micros"]].reset_index(drop=True)
    if list(base_signals["decision_ts"]) != list(stress_frame["decision_ts"]):
        raise ReplayAuthorityError("stress stream decision timestamps differ from the baseline stream")
    base_q = base_signals["target_qty_micros"].astype("int64")
    stress_q = stress_frame["target_qty_micros"].astype("int64")
    if ((base_q > 0) != (stress_q > 0)).any():
        raise ReplayAuthorityError("stress stream changed a long/flat decision: sizing stress must not change decisions")
    if (stress_q > base_q).any():
        raise ReplayAuthorityError("stress quantity exceeds the baseline quantity: not a reduced-exposure stress")

    by_ts = {int(ts): int(q) // 1_000_000 for ts, q in zip(stress_frame["decision_ts"], stress_q)}
    baseline_oos = pd.read_csv(baseline_oos_path)
    stress_rows = baseline_oos.copy()
    decision_epoch = (
        pd.to_datetime(stress_rows["decision_ts"], utc=True) - pd.Timestamp("1970-01-01", tz="UTC")
    ) // pd.Timedelta(seconds=1)
    try:
        stress_rows["target_qty"] = [by_ts[int(ts)] for ts in decision_epoch]
    except KeyError as exc:
        raise ReplayAuthorityError(f"baseline OOS decision {exc} has no stress quantity") from exc
    stress_oos_path = stress_out_dir / "stress_oos_predictions.csv"
    stress_rows.to_csv(stress_oos_path, index=False, lineterminator="\n")

    base_entries = base_block.get("entries") or []
    stress_entries = stress_meta["sizing"].get("entries") or []
    initial = int(base_block["initial_allocated_capital_micros"])
    evidence = {
        "scenario_id": stress_sizing["scenario_id"],
        "policy_id": base_block["policy_id"],
        "allocation_fraction_bps": stress_bps,
        "baseline_allocation_fraction_bps": base_bps,
        "initial_allocated_capital_micros": initial,
        "nominal_entry_budget_micros": initial * stress_bps // 10_000,
        "baseline_nominal_entry_budget_micros": initial * base_bps // 10_000,
        "quantity_rule": "stress Q re-resolved by the capital-fraction resolver; the baseline Q is never capped",
        "caps_unchanged_from_baseline": True,
        "baseline_caps": {"max_target_qty": expected_sizing["max_target_qty"],
                          "max_position_notional_usd": expected_sizing["max_position_notional_usd"]},
        "baseline_semantic_fingerprint": base_meta["semantic_fingerprint"],
        "stress_semantic_fingerprint": stress_meta["semantic_fingerprint"],
        "stress_native_signals_csv_sha256": sha256_file(stress_csv),
        "stress_native_signals_meta_sha256": sha256_file(stress_meta_path),
        "stress_oos_predictions_csv_sha256": sha256_file(stress_oos_path),
        "baseline_entries": len(base_entries),
        "stress_entries": len(stress_entries),
        "is_a_trial": False,
    }
    return stress_oos_path, evidence


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
    stress_sizing: Optional[Dict[str, Any]] = None,
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
        capital_fraction_stress=stress_sizing is not None,
    )
    if adversity_error is not None:
        return {"status": "error", "strategy_id": strategy_id, "reason": adversity_error}

    stress_out_dir.mkdir(parents=True, exist_ok=True)
    stress_oos_path = oos_path
    sizing_evidence: Optional[Dict[str, Any]] = None
    if stress_sizing is None:
        stressed_weight_to_share = replace(
            baseline_spec.weight_to_share,
            max_target_qty=stress_max_target_qty,
            max_position_notional_usd=stress_max_position_notional_usd,
        )
    else:
        # Half-exposure stress: the SAME decisions re-sized by the Rust capital-fraction resolver
        # at the stress fraction. The baseline safety caps are carried unchanged.
        stressed_weight_to_share = baseline_spec.weight_to_share
        stress_oos_path, sizing_evidence = _build_capital_fraction_stress_oos(
            stress_sizing=stress_sizing,
            wf_eval=json.loads(wf_path.read_text(encoding="utf-8")),
            baseline_oos_path=oos_path,
            baseline_spec=baseline_spec,
            strategy_id=strategy_id,
            stress_out_dir=stress_out_dir,
        )
    stressed_spec = replace(
        baseline_spec,
        execution_pricing=replace(
            baseline_spec.execution_pricing,
            slippage_bps=stress_execution_slippage_bps,
            volatility_mult_bps=stress_execution_volatility_mult_bps,
        ),
        weight_to_share=stressed_weight_to_share,
    )

    stressed_path = run_economic_walkforward(
        stress_out_dir,
        bars_csv=bars_path,
        spec=stressed_spec,
        walk_forward_eval_path=wf_path,
        oos_predictions_path=stress_oos_path,
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
            **({"stress_sizing": sizing_evidence} if sizing_evidence is not None else {}),
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
        "--stress-allocation-fraction-bps",
        type=int,
        default=None,
        help=(
            "Capital-fraction half-exposure stress: re-resolved target quantities at this strictly "
            "smaller fraction of the baseline's immutable capital. Requires the other --stress-sizing-* "
            "arguments and forbids the post-sizing cap arguments."
        ),
    )
    parser.add_argument("--stress-sizing-scenario-id", default=None)
    parser.add_argument("--stress-sizing-signals-csv", type=Path, default=None)
    parser.add_argument("--stress-sizing-signals-meta", type=Path, default=None)
    parser.add_argument("--stress-sizing-expected-semantic-fingerprint", default=None)
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

    sizing_args = (args.stress_allocation_fraction_bps, args.stress_sizing_scenario_id,
                   args.stress_sizing_signals_csv, args.stress_sizing_signals_meta,
                   args.stress_sizing_expected_semantic_fingerprint)
    if any(a is not None for a in sizing_args) and any(a is None for a in sizing_args):
        json.dump({"status": "error", "reason": "the capital-fraction stress arguments must be given together"},
                  sys.stdout)
        return 1
    stress_sizing = None
    if args.stress_allocation_fraction_bps is not None:
        stress_sizing = {
            "scenario_id": args.stress_sizing_scenario_id,
            "allocation_fraction_bps": args.stress_allocation_fraction_bps,
            "signals_csv": str(args.stress_sizing_signals_csv),
            "signals_meta": str(args.stress_sizing_signals_meta),
            "expected_semantic_fingerprint": args.stress_sizing_expected_semantic_fingerprint,
        }

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
            stress_sizing=stress_sizing,
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
