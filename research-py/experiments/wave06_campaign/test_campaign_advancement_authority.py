"""W06-A-CAMPAIGN-PREDECLARATION-AUTHORITY-REPAIR-02 -- unit tests for
campaign_advancement_authority.classify_verdict() against the REAL frozen
advancement_policy in PREDECLARED_CAMPAIGN.json (not a synthetic policy --
these tests prove the classifier matches the actual policy this campaign
will use). No network, no trial execution, no registry -- pure function
tests over an in-memory evidence dict.
"""
from __future__ import annotations

import copy
import json
import sys
from pathlib import Path
from typing import Any, Dict

CAMPAIGN_ROOT = Path(__file__).resolve().parent
if str(CAMPAIGN_ROOT) not in sys.path:
    sys.path.insert(0, str(CAMPAIGN_ROOT))

from campaign_advancement_authority import (  # noqa: E402
    ADVANCED,
    INCONCLUSIVE,
    NOT_RUN,
    REJECTED,
    EvidenceRefusal,
    classify_verdict,
    evidence_hash,
)


def _policy() -> Dict[str, Any]:
    campaign = json.loads((CAMPAIGN_ROOT / "PREDECLARED_CAMPAIGN.json").read_text(encoding="utf-8"))
    return campaign["advancement_policy"]


_REQUIRED_P9_SCENARIO_NAMES = [
    "execution_delay_stress",
    "symbol_leave_one_out",
    "month_year_regime_concentration",
    "parameter_neighborhood_execution",
    "placebo_temporal_offset",
    "conservative_capacity_stress",
    "dsr_pbo_sensitivity",
    "p7a_p7b_economic_replay_stress",
    "genuine_shuffled_placebo",
]


def _clearing_evidence(benchmark_excess: float) -> Dict[str, Any]:
    return {
        "absolute_economic_requirement": {"long_only_failure_reason": None, "long_short_failure_reason": None},
        "benchmark_relative_requirement": {"evaluable": True, "excess": benchmark_excess},
        "matched_diagnostic_placebo_requirement": {"evaluable": True, "excess": 0.30},
        "primary_vs_control_requirement": {"evaluable": True, "excess": 0.05},
        "dsr_requirement": {"evaluable": True, "value": 0.75},
        "pbo_requirement": {"evaluable": True, "value": 0.2},
        "genuine_shuffled_placebo_requirement": {"evaluable": True, "passed": True},
        "dsr_pbo_block_count_sensitivity_requirement": {"evaluable": True, "dsr_range": 0.05, "pbo_range": 0.05},
        "canonical_p9_robustness_gauntlet_requirement": {
            "protocol_version": "bkt_robustness_gauntlet_v2",
            "is_complete": True,
            "all_applicable_passed": True,
            "scenario_names": _REQUIRED_P9_SCENARIO_NAMES,
        },
        "p7a_p7b_economic_replay_stress_requirement": {"evaluable": True, "passed": True},
    }


# ---------------------------------------------------------------------------
# Finding 8: exact, non-overlapping, gap-free benchmark-relative partition.
# ---------------------------------------------------------------------------


def test_benchmark_excess_negative_is_rejected() -> None:
    result = classify_verdict(_clearing_evidence(-0.5), _policy())
    assert result["verdict"] == REJECTED


def test_benchmark_excess_exactly_zero_is_rejected() -> None:
    result = classify_verdict(_clearing_evidence(0.0), _policy())
    assert result["verdict"] == REJECTED


def test_benchmark_excess_just_above_zero_is_inconclusive() -> None:
    result = classify_verdict(_clearing_evidence(1e-9), _policy())
    assert result["verdict"] == INCONCLUSIVE


def test_benchmark_excess_just_below_dead_zone_ceiling_is_inconclusive() -> None:
    result = classify_verdict(_clearing_evidence(0.05 - 1e-9), _policy())
    assert result["verdict"] == INCONCLUSIVE


def test_benchmark_excess_exactly_at_dead_zone_ceiling_clears_the_gate() -> None:
    result = classify_verdict(_clearing_evidence(0.05), _policy())
    assert result["verdict"] == ADVANCED


def test_benchmark_excess_above_dead_zone_clears_the_gate() -> None:
    result = classify_verdict(_clearing_evidence(0.5), _policy())
    assert result["verdict"] == ADVANCED


# ---------------------------------------------------------------------------
# Finding 6: exact, narrow gross-wealth-insolvency terminal classification.
# ---------------------------------------------------------------------------

_GROSS_WEALTH_INSOLVENCY_FAILURE_REASON = (
    "RuntimeError: Fail-closed: discrete gross wealth ledger equity is <= 0 -- "
    "cannot compute a further return fraction"
)


def test_exact_gross_wealth_insolvency_failure_reason_is_rejected() -> None:
    evidence = _clearing_evidence(0.5)
    evidence["absolute_economic_requirement"] = {
        "long_only_failure_reason": None,
        "long_short_failure_reason": _GROSS_WEALTH_INSOLVENCY_FAILURE_REASON,
    }
    result = classify_verdict(evidence, _policy())
    assert result["verdict"] == REJECTED
    assert result["gates"]["absolute_economic_requirement"] == REJECTED
    assert result["gates"]["benchmark_relative_requirement"] == NOT_RUN


def test_generic_runtime_error_is_refused_not_classified() -> None:
    evidence = _clearing_evidence(0.5)
    evidence["absolute_economic_requirement"] = {
        "long_only_failure_reason": None,
        "long_short_failure_reason": "RuntimeError: something else entirely went wrong",
    }
    try:
        classify_verdict(evidence, _policy())
        assert False, "expected EvidenceRefusal"
    except EvidenceRefusal:
        pass


def test_net_wealth_variant_message_is_also_refused_not_silently_matched() -> None:
    """The recognized string list is exact-match only -- a same-family but
    textually distinct message (the net, not gross, wealth ledger variant)
    must not be silently accepted."""
    evidence = _clearing_evidence(0.5)
    evidence["absolute_economic_requirement"] = {
        "long_only_failure_reason": None,
        "long_short_failure_reason": (
            "RuntimeError: Fail-closed: discrete net wealth ledger equity is <= 0 -- "
            "cannot compute a further return fraction"
        ),
    }
    try:
        classify_verdict(evidence, _policy())
        assert False, "expected EvidenceRefusal"
    except EvidenceRefusal:
        pass


def test_no_failure_at_all_does_not_trigger_insolvency_path() -> None:
    result = classify_verdict(_clearing_evidence(0.5), _policy())
    assert result["gates"]["absolute_economic_requirement"] == "PASSED"


# ---------------------------------------------------------------------------
# Finding 3: the complete canonical P9 gauntlet may never be satisfied by a
# partial artifact containing only the three Research-registry-anchored
# scenarios.
# ---------------------------------------------------------------------------


def test_partial_p9_artifact_with_only_registry_anchored_scenarios_is_rejected() -> None:
    evidence = _clearing_evidence(0.5)
    evidence["canonical_p9_robustness_gauntlet_requirement"] = {
        "protocol_version": "bkt_robustness_gauntlet_v2",
        "is_complete": True,
        "all_applicable_passed": True,
        "scenario_names": ["dsr_pbo_sensitivity", "p7a_p7b_economic_replay_stress", "genuine_shuffled_placebo"],
    }
    result = classify_verdict(evidence, _policy())
    assert result["verdict"] == REJECTED
    assert result["gates"]["canonical_p9_robustness_gauntlet_requirement"] == "NOT_EVALUABLE_OR_FAILED"


def test_wrong_protocol_version_is_rejected() -> None:
    evidence = _clearing_evidence(0.5)
    evidence["canonical_p9_robustness_gauntlet_requirement"]["protocol_version"] = "bkt_robustness_gauntlet_v1"
    result = classify_verdict(evidence, _policy())
    assert result["verdict"] == REJECTED


def test_incomplete_p9_artifact_is_rejected() -> None:
    evidence = _clearing_evidence(0.5)
    evidence["canonical_p9_robustness_gauntlet_requirement"]["is_complete"] = False
    result = classify_verdict(evidence, _policy())
    assert result["verdict"] == REJECTED


def test_not_all_applicable_passed_is_rejected() -> None:
    evidence = _clearing_evidence(0.5)
    evidence["canonical_p9_robustness_gauntlet_requirement"]["all_applicable_passed"] = False
    result = classify_verdict(evidence, _policy())
    assert result["verdict"] == REJECTED


def test_complete_real_p9_artifact_clears_the_gate() -> None:
    result = classify_verdict(_clearing_evidence(0.5), _policy())
    assert result["gates"]["canonical_p9_robustness_gauntlet_requirement"] == "PASSED"


# ---------------------------------------------------------------------------
# Structural: missing evidence keys refuse rather than silently pass/fail.
# ---------------------------------------------------------------------------


def test_missing_gate_key_refuses() -> None:
    evidence = _clearing_evidence(0.5)
    del evidence["p7a_p7b_economic_replay_stress_requirement"]
    try:
        classify_verdict(evidence, _policy())
        assert False, "expected EvidenceRefusal"
    except EvidenceRefusal:
        pass


def test_evidence_hash_is_deterministic_and_order_independent() -> None:
    evidence_a = _clearing_evidence(0.5)
    evidence_b = json.loads(json.dumps(evidence_a))  # round-trip, same content
    assert evidence_hash(evidence_a) == evidence_hash(evidence_b)


def test_evidence_hash_changes_on_any_mutation() -> None:
    evidence_a = _clearing_evidence(0.5)
    evidence_b = copy.deepcopy(evidence_a)
    evidence_b["dsr_requirement"]["value"] = 0.99
    assert evidence_hash(evidence_a) != evidence_hash(evidence_b)


# ---------------------------------------------------------------------------
# Finding 2: the dsr/pbo block-count sensitivity gate is applied, and its
# range ceilings are enforced.
# ---------------------------------------------------------------------------


def test_dsr_pbo_sensitivity_range_exceeded_is_rejected() -> None:
    evidence = _clearing_evidence(0.5)
    evidence["dsr_pbo_block_count_sensitivity_requirement"] = {
        "evaluable": True, "dsr_range": 0.99, "pbo_range": 0.05,
    }
    result = classify_verdict(evidence, _policy())
    assert result["verdict"] == REJECTED
    assert result["gates"]["dsr_pbo_block_count_sensitivity_requirement"] == "NOT_EVALUABLE_OR_FAILED"


def test_dsr_pbo_sensitivity_not_evaluable_is_rejected() -> None:
    evidence = _clearing_evidence(0.5)
    evidence["dsr_pbo_block_count_sensitivity_requirement"] = {
        "evaluable": False, "dsr_range": None, "pbo_range": None,
    }
    result = classify_verdict(evidence, _policy())
    assert result["verdict"] == REJECTED
