"""W06-A-CAMPAIGN-PREDECLARATION-AUTHORITY-REPAIR-02 (Findings 5, 6, 8) --
the ONLY sanctioned computation of a Wave06 candidate's advancement verdict.

Prior defect (Finding 5): campaign_order_guard.write_closeout_status accepted
`verdict` as a caller-supplied string. load_verified_closeout re-verified
that cited trial_ids were genuinely registered/succeeded, but never checked
the verdict itself against any gate evidence -- a caller could write
REJECTED_NOT_ADVANCED (or any other terminal verdict) for a candidate with
succeeded trials and no gate evidence at all, and the guard would trust it.

This module fixes that by making the verdict a pure, deterministic function
of a caller-supplied evidence dict and the frozen advancement_policy in
PREDECLARED_CAMPAIGN.json -- classify_verdict() is the ONLY place a verdict
string is produced. Nothing here executes a trial, fetches data, or invents
a gate result: every evidence field must already have been computed by a
real, separate evaluation (DSR/PBO judge run, genuine_shuffled_placebo_cli,
p7a_p7b_economic_replay_stress_cli, the real canonical P9 robustness
gauntlet artifact) and handed in by the caller. This module's only job is to
apply the frozen policy to that evidence the same way every time.

Evidence schema (`evidence: dict`), one key per advancement_policy gate:

    "absolute_economic_requirement": {
        "long_only_failure_reason": Optional[str],
        "long_short_failure_reason": Optional[str],
    }
    "benchmark_relative_requirement": {"evaluable": bool, "excess": Optional[float]}
    "matched_diagnostic_placebo_requirement": {"evaluable": bool, "excess": Optional[float]}
    "primary_vs_control_requirement": {"evaluable": bool, "excess": Optional[float]}
    "dsr_requirement": {"evaluable": bool, "value": Optional[float]}
    "pbo_requirement": {"evaluable": bool, "value": Optional[float]}
    "genuine_shuffled_placebo_requirement": {"evaluable": bool, "passed": Optional[bool]}
    "dsr_pbo_block_count_sensitivity_requirement": {
        "evaluable": bool, "dsr_range": Optional[float], "pbo_range": Optional[float],
    }
    "canonical_p9_robustness_gauntlet_requirement": {
        "protocol_version": Optional[str], "is_complete": bool,
        "all_applicable_passed": bool, "scenario_names": list[str],
    }
    "p7a_p7b_economic_replay_stress_requirement": {"evaluable": bool, "passed": Optional[bool]}

A gate whose real attempt never ran because an earlier deterministic gate
already terminally rejected the candidate is recorded as
"NOT_RUN_AFTER_DETERMINISTIC_REJECTION" in the returned per-gate breakdown --
never silently omitted, never fabricated as passed.
"""
from __future__ import annotations

import hashlib
import json
from typing import Any, Dict, List, Optional

NOT_RUN = "NOT_RUN_AFTER_DETERMINISTIC_REJECTION"

REJECTED = "REJECTED_NOT_ADVANCED"
INCONCLUSIVE = "INCONCLUSIVE"
ADVANCED = "DEVELOPMENT_PROMISING_REQUIRES_FRESH_POINT_IN_TIME_CONFIRMATION"

_ALL_GATES = (
    "absolute_economic_requirement",
    "benchmark_relative_requirement",
    "matched_diagnostic_placebo_requirement",
    "primary_vs_control_requirement",
    "dsr_requirement",
    "pbo_requirement",
    "genuine_shuffled_placebo_requirement",
    "dsr_pbo_block_count_sensitivity_requirement",
    "canonical_p9_robustness_gauntlet_requirement",
    "p7a_p7b_economic_replay_stress_requirement",
)


class EvidenceRefusal(RuntimeError):
    """Raised when the supplied evidence is malformed or the classifier
    cannot compute a terminal verdict from it -- never silently defaulted to
    a terminal verdict."""


def evidence_hash(evidence: Dict[str, Any]) -> str:
    """Deterministic content hash of an evidence dict -- canonical
    (sort_keys) JSON, sha256. Used by campaign_order_guard to detect a
    hand-edited evidence field even when the recomputed verdict happens to
    still match."""
    canonical = json.dumps(evidence, sort_keys=True, separators=(",", ":"), default=str)
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def _classify_gross_wealth_insolvency(
    policy: Dict[str, Any], gate: Dict[str, Any]
) -> Optional[str]:
    """Returns REJECTED if a recognized policy-terminal insolvency
    failure_reason is present, None if the gate shows no failure at all
    (both attempts succeeded, i.e. this gate is not itself the cause of
    refusal), or raises EvidenceRefusal for any unrecognized failure -- an
    unrecognized failure must never authorize a terminal verdict."""
    recognized = set(
        policy["gross_wealth_insolvency_terminal_classification"]["recognized_failure_reason_strings"]
    )
    long_only = gate.get("long_only_failure_reason")
    long_short = gate.get("long_short_failure_reason")
    failures = [f for f in (long_only, long_short) if f is not None]
    if not failures:
        return None
    for f in failures:
        if f not in recognized:
            raise EvidenceRefusal(
                f"absolute_economic_requirement failure_reason {f!r} is not a recognized "
                "policy-terminal gross-wealth-insolvency string -- refusing to classify an "
                "operational/unknown failure as a terminal rejection"
            )
    return REJECTED


def _benchmark_partition(policy: Dict[str, Any], excess: float) -> str:
    partition = policy["benchmark_relative_verdict_partition"]
    assert partition["rejected"] == "benchmark_excess <= 0.0"
    assert partition["inconclusive_band"] == "0.0 < benchmark_excess < 0.05"
    assert partition["gate_cleared"] == "benchmark_excess >= 0.05"
    if excess <= 0.0:
        return REJECTED
    if excess < 0.05:
        return INCONCLUSIVE
    return "CLEARED"


def classify_verdict(evidence: Dict[str, Any], policy: Dict[str, Any]) -> Dict[str, Any]:
    """Pure, deterministic function: (evidence, frozen advancement_policy)
    -> {"verdict": ..., "gates": {gate_name: outcome_string, ...}}.

    Short-circuits on the first failing/not-evaluable REQUIRED gate
    (early_rejection_semantics), recording every later gate as NOT_RUN --
    except the special gross-wealth-insolvency path, which recognizes only
    the exact frozen failure_reason strings and refuses (raises
    EvidenceRefusal) rather than silently rejecting on an unrecognized
    failure.
    """
    missing = [g for g in _ALL_GATES if g not in evidence]
    if missing:
        raise EvidenceRefusal(f"evidence is missing required gate(s): {missing!r}")

    gates: Dict[str, str] = {}

    econ = evidence["absolute_economic_requirement"]
    insolvency_verdict = _classify_gross_wealth_insolvency(policy, econ)
    if insolvency_verdict is not None:
        gates["absolute_economic_requirement"] = insolvency_verdict
        for g in _ALL_GATES[1:]:
            gates[g] = NOT_RUN
        return {"verdict": REJECTED, "gates": gates}
    gates["absolute_economic_requirement"] = "PASSED"

    bench = evidence["benchmark_relative_requirement"]
    if not bench.get("evaluable") or bench.get("excess") is None:
        gates["benchmark_relative_requirement"] = "NOT_EVALUABLE"
        for g in _ALL_GATES[2:]:
            gates[g] = NOT_RUN
        return {"verdict": REJECTED, "gates": gates}
    bench_classification = _benchmark_partition(policy, float(bench["excess"]))
    if bench_classification == REJECTED:
        gates["benchmark_relative_requirement"] = REJECTED
        for g in _ALL_GATES[2:]:
            gates[g] = NOT_RUN
        return {"verdict": REJECTED, "gates": gates}
    gates["benchmark_relative_requirement"] = bench_classification  # "INCONCLUSIVE" or "CLEARED"

    placebo = evidence["matched_diagnostic_placebo_requirement"]
    min_excess = policy["matched_diagnostic_placebo_requirement"]["min_excess"]
    if not placebo.get("evaluable") or placebo.get("excess") is None or float(placebo["excess"]) <= min_excess:
        gates["matched_diagnostic_placebo_requirement"] = "NOT_EVALUABLE_OR_FAILED"
        for g in _ALL_GATES[3:]:
            gates[g] = NOT_RUN
        return {"verdict": REJECTED, "gates": gates}
    gates["matched_diagnostic_placebo_requirement"] = "PASSED"

    control = evidence["primary_vs_control_requirement"]
    min_excess = policy["primary_vs_control_requirement"]["min_excess"]
    if not control.get("evaluable") or control.get("excess") is None or float(control["excess"]) <= min_excess:
        gates["primary_vs_control_requirement"] = "NOT_EVALUABLE_OR_FAILED"
        for g in _ALL_GATES[4:]:
            gates[g] = NOT_RUN
        return {"verdict": REJECTED, "gates": gates}
    gates["primary_vs_control_requirement"] = "PASSED"

    dsr = evidence["dsr_requirement"]
    min_value = policy["dsr_requirement"]["min_value"]
    if not dsr.get("evaluable") or dsr.get("value") is None or float(dsr["value"]) < min_value:
        gates["dsr_requirement"] = "NOT_EVALUABLE_OR_FAILED"
        for g in _ALL_GATES[5:]:
            gates[g] = NOT_RUN
        return {"verdict": REJECTED, "gates": gates}
    gates["dsr_requirement"] = "PASSED"

    pbo = evidence["pbo_requirement"]
    max_value = policy["pbo_requirement"]["max_value"]
    if not pbo.get("evaluable") or pbo.get("value") is None or float(pbo["value"]) > max_value:
        gates["pbo_requirement"] = "NOT_EVALUABLE_OR_FAILED"
        for g in _ALL_GATES[6:]:
            gates[g] = NOT_RUN
        return {"verdict": REJECTED, "gates": gates}
    gates["pbo_requirement"] = "PASSED"

    placebo_gate = evidence["genuine_shuffled_placebo_requirement"]
    if not placebo_gate.get("evaluable") or placebo_gate.get("passed") is not True:
        gates["genuine_shuffled_placebo_requirement"] = "NOT_EVALUABLE_OR_FAILED"
        for g in _ALL_GATES[7:]:
            gates[g] = NOT_RUN
        return {"verdict": REJECTED, "gates": gates}
    gates["genuine_shuffled_placebo_requirement"] = "PASSED"

    sens = evidence["dsr_pbo_block_count_sensitivity_requirement"]
    sens_policy = policy["dsr_pbo_block_count_sensitivity_requirement"]
    dsr_range = sens.get("dsr_range")
    pbo_range = sens.get("pbo_range")
    if (
        not sens.get("evaluable")
        or dsr_range is None
        or pbo_range is None
        or float(dsr_range) > sens_policy["dsr_max_sensitivity_range"]
        or float(pbo_range) > sens_policy["pbo_max_sensitivity_range"]
    ):
        gates["dsr_pbo_block_count_sensitivity_requirement"] = "NOT_EVALUABLE_OR_FAILED"
        for g in _ALL_GATES[8:]:
            gates[g] = NOT_RUN
        return {"verdict": REJECTED, "gates": gates}
    gates["dsr_pbo_block_count_sensitivity_requirement"] = "PASSED"

    gauntlet = evidence["canonical_p9_robustness_gauntlet_requirement"]
    gauntlet_policy = policy["canonical_p9_robustness_gauntlet_requirement"]
    required_names = set(gauntlet_policy["required_scenario_names"])
    present_names = set(gauntlet.get("scenario_names") or [])
    gauntlet_ok = (
        gauntlet.get("protocol_version") == gauntlet_policy["required_protocol_version"]
        and gauntlet.get("is_complete") is True
        and gauntlet.get("all_applicable_passed") is True
        and required_names.issubset(present_names)
    )
    if not gauntlet_ok:
        gates["canonical_p9_robustness_gauntlet_requirement"] = "NOT_EVALUABLE_OR_FAILED"
        gates["p7a_p7b_economic_replay_stress_requirement"] = NOT_RUN
        return {"verdict": REJECTED, "gates": gates}
    gates["canonical_p9_robustness_gauntlet_requirement"] = "PASSED"

    stress = evidence["p7a_p7b_economic_replay_stress_requirement"]
    if not stress.get("evaluable") or stress.get("passed") is not True:
        gates["p7a_p7b_economic_replay_stress_requirement"] = "NOT_EVALUABLE_OR_FAILED"
        return {"verdict": REJECTED, "gates": gates}
    gates["p7a_p7b_economic_replay_stress_requirement"] = "PASSED"

    if bench_classification == INCONCLUSIVE:
        return {"verdict": INCONCLUSIVE, "gates": gates}
    return {"verdict": ADVANCED, "gates": gates}
