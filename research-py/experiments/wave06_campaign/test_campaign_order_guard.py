"""W06-A-CAMPAIGN-PREDECLARATION-AUTHORITY-REPAIR-01/02 (Findings 4, 5, 6) --
negative-control proofs for campaign_order_guard.py's deterministic
execution-order authority, using entirely disposable synthetic fixtures (a
throwaway campaign_root + registry_db + minimal advancement_policy). No real
candidate directory, no real registry, no real trial execution is touched by
these tests.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any, Dict

CAMPAIGN_ROOT = Path(__file__).resolve().parent
if str(CAMPAIGN_ROOT) not in sys.path:
    sys.path.insert(0, str(CAMPAIGN_ROOT))

import campaign_order_guard as cog  # noqa: E402
from campaign_advancement_authority import NOT_RUN, EvidenceRefusal  # noqa: E402
from campaign_identity import CAMPAIGN_REAL_EXPERIMENT_ID, resolve_local_src  # noqa: E402

_LOCAL_SRC = resolve_local_src(Path(__file__))
if str(_LOCAL_SRC) not in sys.path:
    sys.path.insert(0, str(_LOCAL_SRC))

from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402
from mqk_research.ml.economic_walkforward import PROTOCOL_ID as ECONOMIC_PROTOCOL_ID  # noqa: E402

_GROSS_WEALTH_INSOLVENCY_FAILURE_REASON = (
    "RuntimeError: Fail-closed: discrete gross wealth ledger equity is <= 0 -- "
    "cannot compute a further return fraction"
)

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


def _minimal_policy() -> Dict[str, Any]:
    """A disposable synthetic advancement_policy carrying only the fields
    campaign_advancement_authority.classify_verdict() actually reads --
    structurally identical in shape to PREDECLARED_CAMPAIGN.json's real
    advancement_policy, but with independent throwaway thresholds so these
    tests never depend on the real campaign's frozen numbers."""
    return {
        "benchmark_relative_requirement": {"min_excess": 0.0},
        "benchmark_relative_verdict_partition": {
            "rejected": "benchmark_excess <= 0.0",
            "inconclusive_band": "0.0 < benchmark_excess < 0.05",
            "gate_cleared": "benchmark_excess >= 0.05",
        },
        "matched_diagnostic_placebo_requirement": {"min_excess": 0.20},
        "primary_vs_control_requirement": {"min_excess": 0.0},
        "dsr_requirement": {"min_value": 0.5},
        "pbo_requirement": {"max_value": 0.5},
        "dsr_pbo_block_count_sensitivity_requirement": {
            "dsr_max_sensitivity_range": 0.15,
            "pbo_max_sensitivity_range": 0.15,
        },
        "canonical_p9_robustness_gauntlet_requirement": {
            "required_protocol_version": "bkt_robustness_gauntlet_v2",
            "required_scenario_names": _REQUIRED_P9_SCENARIO_NAMES,
        },
        "gross_wealth_insolvency_terminal_classification": {
            "recognized_failure_reason_strings": [_GROSS_WEALTH_INSOLVENCY_FAILURE_REASON],
        },
    }


def _fake_campaign_root(tmp_path: Path) -> Path:
    root = tmp_path / "fake_campaign"
    root.mkdir()
    (root / "cand_a").mkdir()
    (root / "cand_b").mkdir()
    campaign = {
        "campaign_id": "FAKE-CAMPAIGN-01",
        "campaign_order": ["A", "B"],
        "candidates": {
            "A": {"directory": "cand_a"},
            "B": {"directory": "cand_b"},
        },
        "advancement_policy": _minimal_policy(),
    }
    (root / "PREDECLARED_CAMPAIGN.json").write_text(json.dumps(campaign), encoding="utf-8")
    return root


def _register_succeeded_trial(store: ResearchResultStore, *, hypothesis_id: str, trial_id: str) -> None:
    store.register_hypothesis(hypothesis_id=hypothesis_id, experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID)
    store.register_trial(
        trial_id=trial_id, experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID, hypothesis_id=hypothesis_id,
        strategy_id="fake_strategy_v1", protocol_id=ECONOMIC_PROTOCOL_ID, identity={"hypothesis_id": hypothesis_id},
    )
    attempt_id, _ = store.begin_attempt(trial_id=trial_id)
    store.finalize_attempt(attempt_id, status="succeeded")


def _fully_clearing_evidence() -> Dict[str, Any]:
    """Evidence that clears every required gate strictly -- classify_verdict()
    must compute ADVANCED (DEVELOPMENT_PROMISING_REQUIRES_FRESH_POINT_IN_TIME_CONFIRMATION)."""
    return {
        "absolute_economic_requirement": {"long_only_failure_reason": None, "long_short_failure_reason": None},
        "benchmark_relative_requirement": {"evaluable": True, "excess": 0.10},
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


def _benchmark_failing_evidence() -> Dict[str, Any]:
    """Evidence that fails ONLY the benchmark-relative gate (excess <= 0.0)
    -- classify_verdict() must compute REJECTED_NOT_ADVANCED with every
    later gate recorded NOT_RUN_AFTER_DETERMINISTIC_REJECTION."""
    evidence = _fully_clearing_evidence()
    evidence["benchmark_relative_requirement"] = {"evaluable": True, "excess": -0.1}
    return evidence


def _insolvency_evidence() -> Dict[str, Any]:
    return {
        "absolute_economic_requirement": {
            "long_only_failure_reason": None,
            "long_short_failure_reason": _GROSS_WEALTH_INSOLVENCY_FAILURE_REASON,
        },
        "benchmark_relative_requirement": {"evaluable": False, "excess": None},
        "matched_diagnostic_placebo_requirement": {"evaluable": False, "excess": None},
        "primary_vs_control_requirement": {"evaluable": False, "excess": None},
        "dsr_requirement": {"evaluable": False, "value": None},
        "pbo_requirement": {"evaluable": False, "value": None},
        "genuine_shuffled_placebo_requirement": {"evaluable": False, "passed": None},
        "dsr_pbo_block_count_sensitivity_requirement": {"evaluable": False, "dsr_range": None, "pbo_range": None},
        "canonical_p9_robustness_gauntlet_requirement": {
            "protocol_version": None, "is_complete": False, "all_applicable_passed": False, "scenario_names": [],
        },
        "p7a_p7b_economic_replay_stress_requirement": {"evaluable": False, "passed": None},
    }


def _operational_failure_evidence() -> Dict[str, Any]:
    """A generic, unrecognized RuntimeError -- must NEVER be classified as
    the recognized gross-wealth-insolvency terminal rejection (Finding 6)."""
    evidence = _insolvency_evidence()
    evidence["absolute_economic_requirement"] = {
        "long_only_failure_reason": None,
        "long_short_failure_reason": "RuntimeError: some unrelated operational failure",
    }
    return evidence


def test_first_candidate_in_campaign_order_is_always_authorized(tmp_path: Path) -> None:
    root = _fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    cog.require_authorized_to_execute("A", registry_db=registry_db, campaign_root=root)


def test_second_candidate_refused_with_no_closeout_at_all(tmp_path: Path) -> None:
    root = _fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    try:
        cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass


def test_second_candidate_refused_when_prior_attempt_failed_not_closed_out(tmp_path: Path) -> None:
    """A crashed/failed prior attempt with NO terminal closeout artifact
    must never silently authorize the next candidate."""
    root = _fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    store.register_hypothesis(hypothesis_id="hyp_a", experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID)
    store.register_trial(
        trial_id="trial_a_failed", experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID, hypothesis_id="hyp_a",
        strategy_id="fake_strategy_v1", protocol_id=ECONOMIC_PROTOCOL_ID, identity={"hypothesis_id": "hyp_a"},
    )
    attempt_id, _ = store.begin_attempt(trial_id="trial_a_failed")
    store.finalize_attempt(attempt_id, status="failed", failure_reason="synthetic")
    try:
        cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass


def test_second_candidate_refused_when_closeout_cites_fabricated_trial_id(tmp_path: Path) -> None:
    root = _fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    # No trial registered at all -- closeout cites a trial_id the registry has never heard of.
    cog.write_closeout_status(
        "A", evidence=_benchmark_failing_evidence(), hypothesis_ids=["hyp_a"],
        verified_trial_ids={"hyp_a": "never_registered_trial_id"}, campaign_root=root,
    )
    try:
        cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass


def test_second_candidate_authorized_after_verified_not_advanced_closeout(tmp_path: Path) -> None:
    root = _fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    _register_succeeded_trial(store, hypothesis_id="hyp_a", trial_id="trial_a")
    status_path = cog.write_closeout_status(
        "A", evidence=_benchmark_failing_evidence(), hypothesis_ids=["hyp_a"],
        verified_trial_ids={"hyp_a": "trial_a"}, campaign_root=root,
    )
    status = json.loads(status_path.read_text(encoding="utf-8"))
    assert status["verdict"] == "REJECTED_NOT_ADVANCED"
    cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)


def test_second_candidate_refused_after_advanced_closeout_campaign_stopped(tmp_path: Path) -> None:
    """A development-positive closeout STOPS the campaign -- it never
    authorizes the next candidate."""
    root = _fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    _register_succeeded_trial(store, hypothesis_id="hyp_a", trial_id="trial_a")
    status_path = cog.write_closeout_status(
        "A", evidence=_fully_clearing_evidence(), hypothesis_ids=["hyp_a"],
        verified_trial_ids={"hyp_a": "trial_a"}, campaign_root=root,
    )
    status = json.loads(status_path.read_text(encoding="utf-8"))
    assert status["verdict"] == cog.ADVANCED_VERDICT
    try:
        cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass


def test_second_candidate_refused_when_closeout_trial_attempt_never_succeeded(tmp_path: Path) -> None:
    """A closeout that cites a genuinely registered trial, but whose only
    attempt(s) never actually succeeded, must still be refused -- a
    fabricated/optimistic closeout claim is never trusted over the
    registry's own truth."""
    root = _fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    store.register_hypothesis(hypothesis_id="hyp_a", experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID)
    store.register_trial(
        trial_id="trial_a_unproven", experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID, hypothesis_id="hyp_a",
        strategy_id="fake_strategy_v1", protocol_id=ECONOMIC_PROTOCOL_ID, identity={"hypothesis_id": "hyp_a"},
    )
    attempt_id, _ = store.begin_attempt(trial_id="trial_a_unproven")
    store.finalize_attempt(attempt_id, status="failed", failure_reason="synthetic")
    cog.write_closeout_status(
        "A", evidence=_benchmark_failing_evidence(), hypothesis_ids=["hyp_a"],
        verified_trial_ids={"hyp_a": "trial_a_unproven"}, campaign_root=root,
    )
    try:
        cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass


def test_unfrozen_candidate_key_is_refused(tmp_path: Path) -> None:
    root = _fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    try:
        cog.require_authorized_to_execute("C", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass


# ---------------------------------------------------------------------------
# W06-A-CAMPAIGN-PREDECLARATION-AUTHORITY-REPAIR-02, Findings 5 + 6:
# verdict is COMPUTED from evidence, never caller-supplied; tampering with
# either the verdict or the evidence is refused; gross-wealth insolvency is
# narrowly, exactly recognized and every other failure is not.
# ---------------------------------------------------------------------------


def test_write_closeout_status_refuses_incomplete_evidence(tmp_path: Path) -> None:
    """A caller cannot force a terminal verdict by supplying succeeded
    trials and NO real gate evidence -- classify_verdict() refuses
    incomplete evidence outright, and write_closeout_status never falls
    back to trusting a caller-supplied verdict string (there is no such
    parameter any more)."""
    root = _fake_campaign_root(tmp_path)
    try:
        cog.write_closeout_status(
            "A", evidence={"absolute_economic_requirement": {}}, hypothesis_ids=["hyp_a"],
            verified_trial_ids={"hyp_a": "trial_a"}, campaign_root=root,
        )
        assert False, "expected EvidenceRefusal"
    except EvidenceRefusal:
        pass


def test_gross_wealth_insolvency_is_verified_rejected_and_authorizes_next_candidate(tmp_path: Path) -> None:
    """Finding 6, path A: the EXACT recognized gross-wealth-insolvency
    failure_reason is a legitimate terminal REJECTED_NOT_ADVANCED closeout,
    and does authorize the next candidate."""
    root = _fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    _register_succeeded_trial(store, hypothesis_id="hyp_a", trial_id="trial_a")
    status_path = cog.write_closeout_status(
        "A", evidence=_insolvency_evidence(), hypothesis_ids=["hyp_a"],
        verified_trial_ids={"hyp_a": "trial_a"}, campaign_root=root,
    )
    status = json.loads(status_path.read_text(encoding="utf-8"))
    assert status["verdict"] == "REJECTED_NOT_ADVANCED"
    assert status["gates"]["absolute_economic_requirement"] == "REJECTED_NOT_ADVANCED"
    assert status["gates"]["p7a_p7b_economic_replay_stress_requirement"] == NOT_RUN
    cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)


def test_generic_operational_failure_is_never_classified_as_gross_wealth_insolvency(tmp_path: Path) -> None:
    """Finding 6, path B: a generic/unrecognized RuntimeError must refuse
    closeout entirely (write_closeout_status raises), so VOL-01 is never
    authorized on the strength of an unproven operational failure."""
    root = _fake_campaign_root(tmp_path)
    try:
        cog.write_closeout_status(
            "A", evidence=_operational_failure_evidence(), hypothesis_ids=["hyp_a"],
            verified_trial_ids={"hyp_a": "trial_a"}, campaign_root=root,
        )
        assert False, "expected EvidenceRefusal"
    except EvidenceRefusal:
        pass
    registry_db = tmp_path / "registry.sqlite3"
    try:
        cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass


def test_provider_network_style_failure_reason_refuses_closeout(tmp_path: Path) -> None:
    """Finding 6: a provider/network-style failure_reason string is exactly
    as unrecognized as any other operational failure -- never a terminal
    rejection."""
    root = _fake_campaign_root(tmp_path)
    evidence = _insolvency_evidence()
    evidence["absolute_economic_requirement"] = {
        "long_only_failure_reason": None,
        "long_short_failure_reason": "ConnectionError: Alpaca API timed out after 30s",
    }
    try:
        cog.write_closeout_status(
            "A", evidence=evidence, hypothesis_ids=["hyp_a"],
            verified_trial_ids={"hyp_a": "trial_a"}, campaign_root=root,
        )
        assert False, "expected EvidenceRefusal"
    except EvidenceRefusal:
        pass


def test_hand_edited_verdict_fails_verification(tmp_path: Path) -> None:
    """Finding 5, item 6: a hand-edited verdict string (even one that is
    itself a valid TERMINAL_VERDICTS member) must fail re-verification,
    because it no longer matches the recomputation from the file's own
    stored evidence."""
    root = _fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    _register_succeeded_trial(store, hypothesis_id="hyp_a", trial_id="trial_a")
    status_path = cog.write_closeout_status(
        "A", evidence=_benchmark_failing_evidence(), hypothesis_ids=["hyp_a"],
        verified_trial_ids={"hyp_a": "trial_a"}, campaign_root=root,
    )
    status = json.loads(status_path.read_text(encoding="utf-8"))
    assert status["verdict"] == "REJECTED_NOT_ADVANCED"
    status["verdict"] = cog.ADVANCED_VERDICT  # tamper: claim a positive result the evidence never proved
    status_path.write_text(json.dumps(status), encoding="utf-8")
    campaign = json.loads((root / "PREDECLARED_CAMPAIGN.json").read_text(encoding="utf-8"))
    assert cog.load_verified_closeout("A", registry_db=registry_db, campaign=campaign, campaign_root=root) is None
    try:
        cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass


def test_hand_edited_evidence_fails_hash_verification_even_if_verdict_still_matches(tmp_path: Path) -> None:
    """Finding 5, item 7: mutating a stored evidence field breaks the
    evidence_hash even when the attacker also (correctly or by luck)
    leaves the verdict string looking consistent with the mutated
    evidence -- the hash check must catch it independently."""
    root = _fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    _register_succeeded_trial(store, hypothesis_id="hyp_a", trial_id="trial_a")
    status_path = cog.write_closeout_status(
        "A", evidence=_benchmark_failing_evidence(), hypothesis_ids=["hyp_a"],
        verified_trial_ids={"hyp_a": "trial_a"}, campaign_root=root,
    )
    status = json.loads(status_path.read_text(encoding="utf-8"))
    # Mutate the stored evidence in place without touching evidence_hash or verdict.
    status["evidence"]["dsr_requirement"]["value"] = 0.99
    status_path.write_text(json.dumps(status), encoding="utf-8")
    campaign = json.loads((root / "PREDECLARED_CAMPAIGN.json").read_text(encoding="utf-8"))
    assert cog.load_verified_closeout("A", registry_db=registry_db, campaign=campaign, campaign_root=root) is None
