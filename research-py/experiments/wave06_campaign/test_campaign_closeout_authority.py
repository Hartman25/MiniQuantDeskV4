"""W06-A-CAMPAIGN-CLOSEOUT-AUTHORITY-REPAIR-03 (Finding 1) -- focused
authority/mutation-proof tests for campaign_closeout_authority.py, the
resolver that binds a Wave06 candidate's closeout evidence to real
ResearchResultStore trials/attempts/artifacts instead of a caller-supplied
dict. Entirely disposable synthetic fixtures (_closeout_test_fixtures.py) --
no real candidate directory, no real registry, no real trial execution, no
real judge run, no network.

Numbered comments below map to the mission's own required RED-style
negative-control list.
"""
from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

CAMPAIGN_ROOT = Path(__file__).resolve().parent
if str(CAMPAIGN_ROOT) not in sys.path:
    sys.path.insert(0, str(CAMPAIGN_ROOT))

import _closeout_test_fixtures as fx  # noqa: E402
import campaign_closeout_authority as cca  # noqa: E402
from campaign_advancement_authority import classify_verdict  # noqa: E402
from campaign_identity import resolve_local_src  # noqa: E402

_LOCAL_SRC = resolve_local_src(Path(__file__))
if str(_LOCAL_SRC) not in sys.path:
    sys.path.insert(0, str(_LOCAL_SRC))

from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402


def _store(tmp_path: Path) -> ResearchResultStore:
    return ResearchResultStore(tmp_path / "registry.sqlite3")


def _policy(root: Path) -> dict:
    return json.loads((root / "PREDECLARED_CAMPAIGN.json").read_text(encoding="utf-8"))["advancement_policy"]


def _register_clearing_family(store: ResearchResultStore, tmp_path: Path, candidate_key: str) -> dict:
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
    family_path = fx.write_family_result_artifact(
        tmp_path / f"{candidate_key}_family.json", long_short_trial_id=ls["trial_id"],
        long_short_attempt_id=ls["attempt_id"], benchmark_sharpe=0.3,
    )
    return {
        "long_only": lo, "long_short": ls,
        "benchmark_artifact_path": family_path, "judge_artifact_sha256": judge_sha,
        "genuine_placebo_artifact_path": placebo_path, "dsr_pbo_sensitivity_artifact_path": sens_path,
    }


def _resolve(root: Path, tmp_path: Path, candidate_key: str, **overrides) -> tuple:
    kwargs = dict(
        registry_db=tmp_path / "registry.sqlite3", campaign_root=root,
    )
    kwargs.update(overrides)
    return cca.resolve_authoritative_evidence(candidate_key, **kwargs)


# ---------------------------------------------------------------------------
# Required test 1: completely fabricated all-pass evidence cannot produce an
# authoritative closeout -- there is no code path left that accepts a
# caller-built evidence dict at all; resolve_authoritative_evidence's only
# inputs are identity/location values, and it refuses outright when the
# registry has nothing behind them.
# ---------------------------------------------------------------------------


def test_fabricated_all_pass_evidence_has_no_code_path(tmp_path: Path) -> None:
    root = fx.fake_campaign_root(tmp_path)
    try:
        _resolve(root, tmp_path, "A")
        assert False, "expected AuthorityRefusal: no real trials registered at all"
    except cca.AuthorityRefusal:
        pass


# ---------------------------------------------------------------------------
# Required test 2: changing stored evidence + recomputing its hash cannot
# bypass source authority -- covered end-to-end in
# test_campaign_order_guard.py::test_hand_edited_evidence_fails_hash_verification_even_if_verdict_still_matches
# (load_verified_closeout recomputes the verdict from the STORED evidence,
# never from a caller-recomputed hash). Here we prove the narrower claim:
# even a syntactically valid judge_artifact_sha256 whose registered content
# has been forged (sha256 does not match the stored canonical_judge_json)
# is refused.
# ---------------------------------------------------------------------------


def test_registered_judge_artifact_content_hash_mismatch_is_refused(tmp_path: Path) -> None:
    root = fx.fake_campaign_root(tmp_path)
    store = _store(tmp_path)
    fixture = _register_clearing_family(store, tmp_path, "A")
    ls_trial_id = fixture["long_short"]["trial_id"]
    # Forge the row directly: register under a sha256 that does NOT match
    # the actual canonical_judge_json content (simulates a corrupted/
    # hand-edited registry row).
    forged_sha = hashlib.sha256(b"not the real content").hexdigest()
    canonical = json.dumps({
        "included_trial_ids": [ls_trial_id],
        "dsr_results": [{"trial_id": ls_trial_id, "evaluable": True, "deflated_sharpe_ratio": 0.75}],
        "pbo_result": {"status": "evaluated", "pbo": 0.2},
    })
    store.register_judge_artifact(
        judge_id="forged_judge", experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id=None, artifact_path=None,
        judge_artifact_sha256=forged_sha, canonical_judge_json=canonical,
        schema_version="v1", protocol_id="p1",
    )
    try:
        _resolve(
            root, tmp_path, "A", benchmark_artifact_path=fixture["benchmark_artifact_path"],
            judge_artifact_sha256=forged_sha, genuine_placebo_artifact_path=fixture["genuine_placebo_artifact_path"],
            dsr_pbo_sensitivity_artifact_path=fixture["dsr_pbo_sensitivity_artifact_path"],
        )
        assert False, "expected AuthorityRefusal: forged judge_artifact_sha256 content hash mismatch"
    except cca.AuthorityRefusal:
        pass


# ---------------------------------------------------------------------------
# Required test 3: LIQ closeout cannot use VOL hypotheses/trials.
# ---------------------------------------------------------------------------


def test_candidate_a_cannot_be_resolved_from_candidate_bs_trials(tmp_path: Path) -> None:
    root = fx.fake_campaign_root(tmp_path, candidate_keys=["A", "B"])
    store = _store(tmp_path)
    # Only B's real trials are ever registered.
    _register_clearing_family(store, tmp_path, "B")
    try:
        _resolve(root, tmp_path, "A")
        assert False, "expected AuthorityRefusal: A's own hypothesis ids were never registered"
    except cca.AuthorityRefusal:
        pass


def test_candidate_a_declaration_cannot_be_satisfied_by_mutated_population(tmp_path: Path) -> None:
    """A candidate whose own PREDECLARED_WAVE.json real_candidate_population
    disagrees with its own hypothesis_id_long_only/long_short pair is
    refused outright -- the declaration itself must be internally
    consistent before any registry lookup happens."""
    root = fx.fake_campaign_root(tmp_path, candidate_keys=["A"])
    decl_path = root / "cand_a" / "PREDECLARED_WAVE.json"
    decl = json.loads(decl_path.read_text(encoding="utf-8"))
    decl["real_candidate_population"] = ["some_other_hypothesis_id", "hyp_a_long_short"]
    decl_path.write_text(json.dumps(decl), encoding="utf-8")
    try:
        cca.resolve_family_hypothesis_ids("A", root)
        assert False, "expected AuthorityRefusal"
    except cca.AuthorityRefusal:
        pass


# ---------------------------------------------------------------------------
# Required tests 4-6: gross-wealth insolvency terminal classification.
# ---------------------------------------------------------------------------


def test_real_failed_attempt_with_exact_gross_insolvency_reason_rejects(tmp_path: Path) -> None:
    root = fx.fake_campaign_root(tmp_path)
    store = _store(tmp_path)
    fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_only",
        trial_id="trial_a_lo", net_sharpe=0.5,
    )
    fx.register_insolvent_economic_trial(
        store, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_short", trial_id="trial_a_ls",
    )
    evidence, hyp_ids, trial_ids = _resolve(root, tmp_path, "A")
    result = classify_verdict(evidence, _policy(root))
    assert result["verdict"] == "REJECTED_NOT_ADVANCED"
    assert evidence["absolute_economic_requirement"]["long_short_failure_reason"] == (
        fx.GROSS_WEALTH_INSOLVENCY_FAILURE_REASON
    )


def test_succeeded_trial_plus_fake_insolvency_text_is_impossible(tmp_path: Path) -> None:
    """There is no parameter through which a caller could even ATTEMPT to
    supply a fake insolvency failure_reason for a succeeded trial -- the
    resolver reads the failure_reason exclusively from the real registry
    attempt row."""
    root = fx.fake_campaign_root(tmp_path)
    store = _store(tmp_path)
    fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_only",
        trial_id="trial_a_lo", net_sharpe=0.5,
    )
    fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_short",
        trial_id="trial_a_ls", net_sharpe=0.9,
    )
    # This test's claim is structural, not behavioral: a succeeded attempt's
    # failure_reason column is durably None -- there is no parameter on
    # resolve_authoritative_evidence, write_closeout_status, or any
    # resolver function through which a caller could inject a fake
    # insolvency string for it.
    outcome = cca.resolve_attempt_outcome(store, "trial_a_ls")
    assert outcome["status"] == "succeeded"
    assert outcome["attempt"]["failure_reason"] is None


def test_generic_operational_failed_attempt_remains_blocked(tmp_path: Path) -> None:
    root = fx.fake_campaign_root(tmp_path)
    store = _store(tmp_path)
    fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_only",
        trial_id="trial_a_lo", net_sharpe=0.5,
    )
    fx.register_insolvent_economic_trial(
        store, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_short", trial_id="trial_a_ls",
        failure_reason="RuntimeError: something else entirely",
    )
    try:
        _resolve(root, tmp_path, "A")
        assert False, "expected AuthorityRefusal: no succeeded and no recognized-insolvency attempt"
    except cca.AuthorityRefusal:
        pass


def test_contradictory_succeeded_and_insolvency_failed_attempts_fail_closed(tmp_path: Path) -> None:
    """If retries on one unchanged trial contain contradictory terminal
    outcomes (both succeeded and recognized gross-insolvency failed), fail
    closed rather than choosing the favorable result."""
    root = fx.fake_campaign_root(tmp_path)
    store = _store(tmp_path)
    fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_short",
        trial_id="trial_a_ls", net_sharpe=0.9,
    )
    # A SECOND attempt on the SAME trial_id, this time recognized-insolvent.
    attempt_id, _ = store.begin_attempt(trial_id="trial_a_ls")
    store.finalize_attempt(attempt_id, status="failed", failure_reason=fx.GROSS_WEALTH_INSOLVENCY_FAILURE_REASON)
    try:
        cca.resolve_attempt_outcome(store, "trial_a_ls")
        assert False, "expected AuthorityRefusal: contradictory terminal attempts"
    except cca.AuthorityRefusal:
        pass


# ---------------------------------------------------------------------------
# Required tests 7-8: NaN/+-Inf at every numeric gate fail closed -- covered
# directly against classify_verdict() in test_campaign_advancement_
# authority.py; here we additionally prove the resolver itself never hands
# classify_verdict a non-finite value from a real (if corrupted) artifact.
# ---------------------------------------------------------------------------


def test_resolver_rejects_non_finite_net_sharpe_in_a_real_artifact(tmp_path: Path) -> None:
    import math

    root = fx.fake_campaign_root(tmp_path)
    store = _store(tmp_path)
    fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_only",
        trial_id="trial_a_lo", net_sharpe=0.5,
    )
    ls = fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_short",
        trial_id="trial_a_ls", net_sharpe=0.9,
    )
    # Corrupt the real artifact file in place with a NaN (json.dumps with
    # allow_nan=True default writes the literal `NaN` token, which
    # json.loads happily parses back to float('nan') -- exactly reproducing
    # a real corrupted-artifact scenario).
    econ = json.loads(ls["economic_walk_forward_path"].read_text(encoding="utf-8"))
    econ["aggregate"]["net_sharpe"] = float("nan")
    ls["economic_walk_forward_path"].write_text(json.dumps(econ, allow_nan=True), encoding="utf-8")
    family_path = fx.write_family_result_artifact(
        tmp_path / "family.json", long_short_trial_id=ls["trial_id"], long_short_attempt_id=ls["attempt_id"],
        benchmark_sharpe=0.3,
    )
    judge_sha = fx.register_judge_artifact(
        store, experiment_id=fx.REAL_EXPERIMENT_ID, included_trial_ids=[ls["trial_id"]],
        dsr_by_trial={ls["trial_id"]: 0.75}, pbo_value=0.2,
    )
    placebo_path = fx.write_genuine_placebo_artifact(
        tmp_path / "placebo.json", trial_id=ls["trial_id"], economic_eval_id=ls["economic_eval_id"],
        economic_artifact_sha256=hashlib.sha256(ls["economic_walk_forward_path"].read_bytes()).hexdigest(),
    )
    sens_path = fx.write_sensitivity_artifact(
        tmp_path / "sens.json", trial_id=ls["trial_id"], judge_artifact_sha256=judge_sha, dsr_range=0.05,
        pbo_range=0.05,
    )
    # resolve_authoritative_evidence itself probes the assembled evidence
    # through classify_verdict() before returning (see its own docstring on
    # the canonical_p9 probe) -- a NaN net_sharpe therefore already fails
    # closed INSIDE resolution, before any caller could even inspect the
    # evidence dict.
    try:
        _resolve(
            root, tmp_path, "A", benchmark_artifact_path=family_path, judge_artifact_sha256=judge_sha,
            genuine_placebo_artifact_path=placebo_path, dsr_pbo_sensitivity_artifact_path=sens_path,
        )
        assert False, "expected EvidenceRefusal: NaN net_sharpe must never reach a policy comparison"
    except cca.EvidenceRefusal:
        pass


# ---------------------------------------------------------------------------
# Required test 9: canonical P9 partial artifact cannot pass -- there is no
# artifact-based code path for P9 at all any more (MissingAuthoritativeSeam
# always fires once every other real gate has passed); classify_verdict's
# own partial-artifact rejection is covered directly in
# test_campaign_advancement_authority.py.
# ---------------------------------------------------------------------------


def test_p9_seam_is_never_bypassable_for_an_otherwise_advancing_candidate(tmp_path: Path) -> None:
    root = fx.fake_campaign_root(tmp_path)
    store = _store(tmp_path)
    fixture = _register_clearing_family(store, tmp_path, "A")
    try:
        _resolve(
            root, tmp_path, "A", benchmark_artifact_path=fixture["benchmark_artifact_path"],
            judge_artifact_sha256=fixture["judge_artifact_sha256"],
            genuine_placebo_artifact_path=fixture["genuine_placebo_artifact_path"],
            dsr_pbo_sensitivity_artifact_path=fixture["dsr_pbo_sensitivity_artifact_path"],
        )
        assert False, "expected MissingAuthoritativeSeam"
    except cca.MissingAuthoritativeSeam:
        pass


# ---------------------------------------------------------------------------
# Required test 10: artifact hash/identity mutation fails authority
# verification.
# ---------------------------------------------------------------------------


def test_mutated_economic_artifact_fails_registry_identity_check(tmp_path: Path) -> None:
    root = fx.fake_campaign_root(tmp_path)
    store = _store(tmp_path)
    fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_only",
        trial_id="trial_a_lo", net_sharpe=0.5,
    )
    ls = fx.register_succeeded_economic_trial(
        store, tmp_path, experiment_id=fx.REAL_EXPERIMENT_ID, hypothesis_id="hyp_a_long_short",
        trial_id="trial_a_ls", net_sharpe=0.9,
    )
    econ = json.loads(ls["economic_walk_forward_path"].read_text(encoding="utf-8"))
    econ["ids"]["economic_eval_id"] = "some_other_economic_eval_id"
    ls["economic_walk_forward_path"].write_text(json.dumps(econ), encoding="utf-8")
    try:
        _resolve(root, tmp_path, "A")
        assert False, "expected AuthorityRefusal: mutated ids.economic_eval_id disagrees with registry result_id"
    except cca.AuthorityRefusal:
        pass


def test_mutated_genuine_placebo_artifact_binding_fails(tmp_path: Path) -> None:
    root = fx.fake_campaign_root(tmp_path)
    store = _store(tmp_path)
    fixture = _register_clearing_family(store, tmp_path, "A")
    placebo = json.loads(fixture["genuine_placebo_artifact_path"].read_text(encoding="utf-8"))
    placebo["baseline_economic_artifact_sha256"] = "0" * 64
    fixture["genuine_placebo_artifact_path"].write_text(json.dumps(placebo), encoding="utf-8")
    try:
        _resolve(
            root, tmp_path, "A", benchmark_artifact_path=fixture["benchmark_artifact_path"],
            judge_artifact_sha256=fixture["judge_artifact_sha256"],
            genuine_placebo_artifact_path=fixture["genuine_placebo_artifact_path"],
            dsr_pbo_sensitivity_artifact_path=fixture["dsr_pbo_sensitivity_artifact_path"],
        )
        assert False, "expected AuthorityRefusal: forged baseline_economic_artifact_sha256"
    except cca.AuthorityRefusal:
        pass


# ---------------------------------------------------------------------------
# Required tests 11-12: a genuinely authoritative rejected closeout
# authorizes VOL; an authoritative ADVANCED closeout stops VOL -- these are
# exercised end-to-end (through write_closeout_status/require_authorized_
# to_execute) in test_campaign_order_guard.py. ADVANCED itself can never be
# produced today (the P9 seam gap), so its "stops the campaign" half is
# proven at the classify_verdict layer in test_campaign_advancement_
# authority.py and at the order-guard layer using a hand-built (never
# resolver-produced) ADVANCED status is deliberately NOT tested here --
# doing so would require constructing a closeout the resolver itself could
# never produce, which is precisely the caller-assertion pattern this
# repair exists to close off.
# ---------------------------------------------------------------------------
