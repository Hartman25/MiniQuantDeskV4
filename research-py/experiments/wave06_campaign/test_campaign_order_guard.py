"""W06-A-CAMPAIGN-PREDECLARATION-AUTHORITY-REPAIR-01/02/03 (Findings 1, 3, 4,
5, 6) -- negative-control proofs for campaign_order_guard.py's deterministic
execution-order authority, using entirely disposable synthetic fixtures (a
throwaway campaign_root + registry_db). No real candidate directory, no real
registry, no real trial execution is touched by these tests.

REPAIR-03 (Finding 1): write_closeout_status() no longer accepts a
caller-supplied `evidence`/`hypothesis_ids`/`verified_trial_ids` -- every
scenario below builds REAL ResearchResultStore trials/attempts/artifacts via
_closeout_test_fixtures and lets campaign_closeout_authority derive the
evidence itself. See test_campaign_closeout_authority.py for the resolver's
own focused authority/mutation-proof tests.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

CAMPAIGN_ROOT = Path(__file__).resolve().parent
if str(CAMPAIGN_ROOT) not in sys.path:
    sys.path.insert(0, str(CAMPAIGN_ROOT))

import campaign_order_guard as cog  # noqa: E402
import _closeout_test_fixtures as fx  # noqa: E402
from campaign_closeout_authority import AuthorityRefusal, MissingAuthoritativeSeam  # noqa: E402
from campaign_identity import resolve_local_src  # noqa: E402

_LOCAL_SRC = resolve_local_src(Path(__file__))
if str(_LOCAL_SRC) not in sys.path:
    sys.path.insert(0, str(_LOCAL_SRC))

from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402


def _register_full_clearing_family(store: ResearchResultStore, tmp_path: Path, candidate_key: str) -> dict:
    """Registers real long_only/long_short/placebo trials plus a real judge
    artifact/genuine-placebo artifact/sensitivity artifact/family_result
    that together clear every real, resolvable gate (i.e. would only ever
    be blocked by the documented canonical-P9 seam gap)."""
    hyps = fx.candidate_hypothesis_ids(candidate_key)
    lo = fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id=hyps["long_only"],
        trial_id=f"trial_{candidate_key}_lo", net_sharpe=0.5,
    )
    ls = fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id=hyps["long_short"],
        trial_id=f"trial_{candidate_key}_ls", net_sharpe=0.9,
    )
    fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.PLACEBO_EXPERIMENT_ID, hypothesis_id=hyps["placebo"],
        trial_id=f"trial_{candidate_key}_pb", net_sharpe=0.2,
    )
    judge_sha = fx.register_judge_artifact(
        store, experiment_id=fx.REAL_EXPERIMENT_ID, included_trial_ids=[ls["trial_id"]],
        dsr_by_trial={ls["trial_id"]: 0.75}, pbo_value=0.2,
    )
    placebo_path = fx.write_genuine_placebo_artifact(
        tmp_path / f"{candidate_key}_placebo.json", trial_id=ls["trial_id"],
        economic_eval_id=ls["economic_eval_id"], economic_artifact_sha256=ls["economic_walk_forward_sha256"],
    )
    sens_path = fx.write_sensitivity_artifact(
        tmp_path / f"{candidate_key}_sens.json", trial_id=ls["trial_id"], judge_artifact_sha256=judge_sha,
        dsr_range=0.05, pbo_range=0.05,
    )
    return {
        "benchmark_artifact_path": None,  # set per-scenario (drives benchmark_excess sign)
        "judge_artifact_sha256": judge_sha,
        "genuine_placebo_artifact_path": placebo_path,
        "dsr_pbo_sensitivity_artifact_path": sens_path,
        "long_short": ls,
    }


def _write_benchmark(tmp_path: Path, candidate_key: str, ls: dict, *, benchmark_sharpe: float) -> Path:
    return fx.write_family_result_artifact(
        tmp_path / f"{candidate_key}_family.json", long_short_trial_id=ls["trial_id"],
        long_short_attempt_id=ls["attempt_id"], benchmark_sharpe=benchmark_sharpe,
    )


def _write_rejecting_closeout(store: ResearchResultStore, tmp_path: Path, root: Path, candidate_key: str) -> Path:
    """A real closeout that rejects at the benchmark gate (benchmark_sharpe
    set higher than long_short's own net_sharpe) -- fully resolvable
    without ever touching the documented canonical-P9 gap."""
    fixture = _register_full_clearing_family(store, tmp_path, candidate_key)
    fixture["benchmark_artifact_path"] = _write_benchmark(
        tmp_path, candidate_key, fixture["long_short"], benchmark_sharpe=5.0
    )
    return cog.write_closeout_status(
        candidate_key,
        registry_db=tmp_path / "registry.sqlite3",
        benchmark_artifact_path=fixture["benchmark_artifact_path"],
        judge_artifact_sha256=fixture["judge_artifact_sha256"],
        genuine_placebo_artifact_path=fixture["genuine_placebo_artifact_path"],
        dsr_pbo_sensitivity_artifact_path=fixture["dsr_pbo_sensitivity_artifact_path"],
        campaign_root=root,
    )


def test_first_candidate_in_campaign_order_is_always_authorized(tmp_path: Path) -> None:
    root = fx.fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    cog.require_authorized_to_execute("A", registry_db=registry_db, campaign_root=root)


def test_second_candidate_refused_with_no_closeout_at_all(tmp_path: Path) -> None:
    root = fx.fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    try:
        cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass


def test_second_candidate_refused_when_prior_attempt_failed_not_closed_out(tmp_path: Path) -> None:
    """A crashed/failed prior attempt with NO terminal closeout artifact
    must never silently authorize the next candidate."""
    root = fx.fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    fx.register_insolvent_economic_trial(
        store, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_short",
        trial_id="trial_a_failed", failure_reason="synthetic operational failure",
    )
    try:
        cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass


def test_second_candidate_authorized_after_verified_not_advanced_closeout(tmp_path: Path) -> None:
    root = fx.fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    status_path = _write_rejecting_closeout(store, tmp_path, root, "A")
    status = json.loads(status_path.read_text(encoding="utf-8"))
    assert status["verdict"] == "REJECTED_NOT_ADVANCED"
    cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)


def test_second_candidate_refused_when_closeout_trial_attempt_never_succeeded(tmp_path: Path) -> None:
    """A candidate whose real trials never resolved to a terminal outcome
    (no succeeded attempt, no recognized insolvency) can never even produce
    a closeout -- write_closeout_status itself refuses."""
    root = fx.fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    fx.register_insolvent_economic_trial(
        store, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_only",
        trial_id="trial_a_lo_unproven", failure_reason="ConnectionError: some unrelated operational failure",
    )
    fx.register_insolvent_economic_trial(
        store, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_short",
        trial_id="trial_a_ls_unproven", failure_reason="ConnectionError: some unrelated operational failure",
    )
    try:
        cog.write_closeout_status("A", registry_db=registry_db, campaign_root=root)
        assert False, "expected AuthorityRefusal"
    except AuthorityRefusal:
        pass
    try:
        cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass


def test_unfrozen_candidate_key_is_refused(tmp_path: Path) -> None:
    root = fx.fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    try:
        cog.require_authorized_to_execute("C", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass


# ---------------------------------------------------------------------------
# W06-A-CAMPAIGN-CLOSEOUT-AUTHORITY-REPAIR-03, Finding 1: write_closeout_
# status can no longer be handed a fabricated evidence dict at all -- there
# is no such parameter any more. Finding 6: gross-wealth insolvency is
# narrowly, exactly recognized (via a REAL registry attempt) and every
# other failure is not.
# ---------------------------------------------------------------------------


def test_gross_wealth_insolvency_is_verified_rejected_and_authorizes_next_candidate(tmp_path: Path) -> None:
    """Finding 2/6, path A: a REAL registered attempt whose failure_reason
    is EXACTLY the recognized gross-wealth-insolvency string is a
    legitimate terminal REJECTED_NOT_ADVANCED closeout, and does authorize
    the next candidate -- no caller-supplied evidence dict is involved."""
    root = fx.fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_only",
        trial_id="trial_a_lo", net_sharpe=0.5,
    )
    fx.register_insolvent_economic_trial(
        store, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_short", trial_id="trial_a_ls",
    )
    status_path = cog.write_closeout_status("A", registry_db=registry_db, campaign_root=root)
    status = json.loads(status_path.read_text(encoding="utf-8"))
    assert status["verdict"] == "REJECTED_NOT_ADVANCED"
    assert status["gates"]["absolute_economic_requirement"] == "REJECTED_NOT_ADVANCED"
    from campaign_advancement_authority import NOT_RUN
    assert status["gates"]["p7a_p7b_economic_replay_stress_requirement"] == NOT_RUN
    cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)


def test_generic_operational_failure_is_never_classified_as_gross_wealth_insolvency(tmp_path: Path) -> None:
    """Finding 6, path B: a generic/unrecognized RuntimeError must refuse
    closeout entirely (write_closeout_status raises), so VOL-01 is never
    authorized on the strength of an unproven operational failure."""
    root = fx.fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_only",
        trial_id="trial_a_lo", net_sharpe=0.5,
    )
    fx.register_insolvent_economic_trial(
        store, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_short", trial_id="trial_a_ls",
        failure_reason="RuntimeError: some unrelated operational failure",
    )
    try:
        cog.write_closeout_status("A", registry_db=registry_db, campaign_root=root)
        assert False, "expected AuthorityRefusal"
    except AuthorityRefusal:
        pass
    try:
        cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass


def test_provider_network_style_failure_reason_refuses_closeout(tmp_path: Path) -> None:
    """Finding 6: a provider/network-style failure_reason string is exactly
    as unrecognized as any other operational failure -- never a terminal
    rejection."""
    root = fx.fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_only",
        trial_id="trial_a_lo", net_sharpe=0.5,
    )
    fx.register_insolvent_economic_trial(
        store, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_short", trial_id="trial_a_ls",
        failure_reason="ConnectionError: Alpaca API timed out after 30s",
    )
    try:
        cog.write_closeout_status("A", registry_db=registry_db, campaign_root=root)
        assert False, "expected AuthorityRefusal"
    except AuthorityRefusal:
        pass


def test_hand_edited_verdict_fails_verification(tmp_path: Path) -> None:
    """Finding 5, item 6: a hand-edited verdict string (even one that is
    itself a valid TERMINAL_VERDICTS member) must fail re-verification,
    because it no longer matches the recomputation from the file's own
    stored evidence."""
    root = fx.fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    status_path = _write_rejecting_closeout(store, tmp_path, root, "A")
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
    """Finding 5, item 7 / Finding 1 required test 2: mutating a stored
    evidence field breaks evidence_hash even when the attacker also
    (correctly or by luck) leaves the verdict string looking consistent
    with the mutated evidence -- the hash check must catch it
    independently, and this can never be bypassed by recomputing the hash
    over the mutated content, because load_verified_closeout recomputes the
    verdict from the STORED evidence, not from the attacker's hash."""
    root = fx.fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    status_path = _write_rejecting_closeout(store, tmp_path, root, "A")
    status = json.loads(status_path.read_text(encoding="utf-8"))
    # Mutate the stored evidence in place without touching evidence_hash or verdict.
    status["evidence"]["primary_vs_control_requirement"]["excess"] = 0.99
    status_path.write_text(json.dumps(status), encoding="utf-8")
    campaign = json.loads((root / "PREDECLARED_CAMPAIGN.json").read_text(encoding="utf-8"))
    assert cog.load_verified_closeout("A", registry_db=registry_db, campaign=campaign, campaign_root=root) is None


def test_p9_seam_gap_refuses_write_closeout_status_for_an_otherwise_advancing_candidate(tmp_path: Path) -> None:
    """Finding 1's own documented gap: a candidate that would clear every
    OTHER real gate cannot be closed out at all today -- write_closeout_
    status raises MissingAuthoritativeSeam rather than accepting a
    caller-supplied canonical_p9 boolean."""
    root = fx.fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    fixture = _register_full_clearing_family(store, tmp_path, "A")
    fixture["benchmark_artifact_path"] = _write_benchmark(tmp_path, "A", fixture["long_short"], benchmark_sharpe=0.3)
    try:
        cog.write_closeout_status(
            "A",
            registry_db=registry_db,
            benchmark_artifact_path=fixture["benchmark_artifact_path"],
            judge_artifact_sha256=fixture["judge_artifact_sha256"],
            genuine_placebo_artifact_path=fixture["genuine_placebo_artifact_path"],
            dsr_pbo_sensitivity_artifact_path=fixture["dsr_pbo_sensitivity_artifact_path"],
            campaign_root=root,
        )
        assert False, "expected MissingAuthoritativeSeam"
    except MissingAuthoritativeSeam:
        pass
