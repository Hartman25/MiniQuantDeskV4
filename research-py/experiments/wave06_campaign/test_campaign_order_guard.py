"""W06-A-CAMPAIGN-PREDECLARATION-AUTHORITY-REPAIR-01 (Finding 4) --
negative-control proofs for campaign_order_guard.py's deterministic
execution-order authority, using entirely disposable synthetic fixtures (a
throwaway campaign_root + registry_db). No real candidate directory, no
real registry, no real trial execution is touched by these tests.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

CAMPAIGN_ROOT = Path(__file__).resolve().parent
if str(CAMPAIGN_ROOT) not in sys.path:
    sys.path.insert(0, str(CAMPAIGN_ROOT))

import campaign_order_guard as cog  # noqa: E402
from campaign_identity import CAMPAIGN_REAL_EXPERIMENT_ID, resolve_local_src  # noqa: E402

_LOCAL_SRC = resolve_local_src(Path(__file__))
if str(_LOCAL_SRC) not in sys.path:
    sys.path.insert(0, str(_LOCAL_SRC))

from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402
from mqk_research.ml.economic_walkforward import PROTOCOL_ID as ECONOMIC_PROTOCOL_ID  # noqa: E402


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
        "A", verdict="REJECTED_NOT_ADVANCED", hypothesis_ids=["hyp_a"],
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
    cog.write_closeout_status(
        "A", verdict="REJECTED_NOT_ADVANCED", hypothesis_ids=["hyp_a"],
        verified_trial_ids={"hyp_a": "trial_a"}, campaign_root=root,
    )
    cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)


def test_second_candidate_refused_after_advanced_closeout_campaign_stopped(tmp_path: Path) -> None:
    """A development-positive closeout STOPS the campaign -- it never
    authorizes the next candidate."""
    root = _fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    store = ResearchResultStore(registry_db)
    _register_succeeded_trial(store, hypothesis_id="hyp_a", trial_id="trial_a")
    cog.write_closeout_status(
        "A", verdict=cog.ADVANCED_VERDICT, hypothesis_ids=["hyp_a"],
        verified_trial_ids={"hyp_a": "trial_a"}, campaign_root=root,
    )
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
        "A", verdict="REJECTED_NOT_ADVANCED", hypothesis_ids=["hyp_a"],
        verified_trial_ids={"hyp_a": "trial_a_unproven"}, campaign_root=root,
    )
    try:
        cog.require_authorized_to_execute("B", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass


def test_closeout_writer_refuses_non_terminal_verdict(tmp_path: Path) -> None:
    root = _fake_campaign_root(tmp_path)
    try:
        cog.write_closeout_status(
            "A", verdict="SOME_NON_TERMINAL_VERDICT", hypothesis_ids=["hyp_a"],
            verified_trial_ids={"hyp_a": "trial_a"}, campaign_root=root,
        )
        assert False, "expected ValueError"
    except ValueError:
        pass


def test_unfrozen_candidate_key_is_refused(tmp_path: Path) -> None:
    root = _fake_campaign_root(tmp_path)
    registry_db = tmp_path / "registry.sqlite3"
    try:
        cog.require_authorized_to_execute("C", registry_db=registry_db, campaign_root=root)
        assert False, "expected CampaignOrderRefusal"
    except cog.CampaignOrderRefusal:
        pass
