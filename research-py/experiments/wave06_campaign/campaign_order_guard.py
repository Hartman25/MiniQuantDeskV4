"""W06-A-CAMPAIGN-PREDECLARATION-AUTHORITY-REPAIR-01/02 (Findings 4, 5, 6) --
deterministic campaign-order execution authority.

Prior defect (Finding 4): each candidate's run_wave.py could execute its
own --execute stage independently -- nothing mechanically proved LIQ-01 had
been attempted, let alone that it had honestly failed the frozen
advancement policy, before VOL-01 could run. Order was documented in
PREDECLARED_CAMPAIGN.json's stopping_rule but not enforced.

Prior defect (Finding 5, REPAIR-02): write_closeout_status originally
accepted `verdict` as a caller-supplied string, and load_verified_closeout
re-verified only that cited trial_ids were genuinely registered/succeeded --
never that the verdict itself followed from any gate evidence. A caller
could write REJECTED_NOT_ADVANCED for a candidate with succeeded trials and
zero gate evidence, and the guard would trust it. Fixed by routing the
verdict through campaign_advancement_authority.classify_verdict(): callers
supply `evidence`, never `verdict` directly, and load_verified_closeout
independently recomputes the verdict from the closeout's own stored
evidence, refusing on any mismatch (Finding 6's gross-wealth-insolvency
terminal classification lives in that module, not here).

W06-A-CAMPAIGN-CLOSEOUT-AUTHORITY-REPAIR-03 (Finding 1): REPAIR-02 closed
the verdict-computation gap but left the `evidence` dict itself entirely
caller-supplied -- a fabricated all-pass dict could still compute an
ADVANCED verdict with no real Research/P9 artifact ever having produced its
values, and hashing that dict only proved internal self-consistency, never
authority. write_closeout_status() no longer accepts `evidence`,
`hypothesis_ids`, or `verified_trial_ids` from the caller at all -- it
derives all three itself, via campaign_closeout_authority.
resolve_authoritative_evidence(), from real ResearchResultStore trials/
attempts, real registered judge artifacts, and real evaluator-CLI output
files (see that module's own docstring for exactly which authority each
gate is bound to, and its one explicit, reported gap:
canonical_p9_robustness_gauntlet_requirement). load_verified_closeout()
also now derives its expected hypothesis population from the candidate's
own frozen PREDECLARED_WAVE.json (Finding 3) rather than merely checking
that the closeout's cited hypothesis_ids exist somewhere in the registry.

This module is the ONLY sanctioned execution-order gate. It never infers
order from filesystem directory existence, never fabricates a result, and
never trusts a closeout claim without independently re-verifying it against
real, registered, succeeded trials in the shared campaign registry (see
campaign_identity.py) AND recomputing its verdict from its own stored
evidence. A missing, malformed, crashed, or incomplete prior attempt
refuses the next candidate -- it never silently authorizes it.

CANDIDATE_CLOSEOUT_STATUS.json is the only artifact this guard trusts, and
write_closeout_status() is the only sanctioned way to produce one -- no
other code path may hand-author this file.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any, Dict, Optional

CAMPAIGN_ROOT = Path(__file__).resolve().parent
if str(CAMPAIGN_ROOT) not in sys.path:
    sys.path.insert(0, str(CAMPAIGN_ROOT))

from campaign_advancement_authority import (  # noqa: E402
    EvidenceRefusal,
    classify_verdict,
    evidence_hash,
)
from campaign_closeout_authority import (  # noqa: E402
    AuthorityRefusal,
    resolve_attempt_outcome,
    resolve_authoritative_evidence,
)
from campaign_identity import (  # noqa: E402
    CAMPAIGN_REAL_EXPERIMENT_ID,
    load_campaign,
    load_candidate_declaration,
    resolve_local_src,
)

_LOCAL_SRC = resolve_local_src(Path(__file__))
if str(_LOCAL_SRC) not in sys.path:
    sys.path.insert(0, str(_LOCAL_SRC))

CLOSEOUT_STATUS_FILENAME = "CANDIDATE_CLOSEOUT_STATUS.json"

ADVANCED_VERDICT = "DEVELOPMENT_PROMISING_REQUIRES_FRESH_POINT_IN_TIME_CONFIRMATION"
NOT_ADVANCED_VERDICTS = frozenset({"REJECTED_NOT_ADVANCED", "INCONCLUSIVE"})
TERMINAL_VERDICTS = NOT_ADVANCED_VERDICTS | {ADVANCED_VERDICT}


class CampaignOrderRefusal(RuntimeError):
    """Raised when a candidate is not yet authorized to execute under the
    frozen campaign_order."""


def candidate_closeout_status_path(
    candidate_key: str, campaign: Optional[dict] = None, campaign_root: Path = CAMPAIGN_ROOT
) -> Path:
    campaign = campaign or load_campaign(campaign_root)
    directory = campaign["candidates"][candidate_key]["directory"]
    return (Path(campaign_root) / directory / CLOSEOUT_STATUS_FILENAME).resolve()


def load_verified_closeout(
    candidate_key: str,
    *,
    registry_db: Path,
    campaign: Optional[dict] = None,
    campaign_root: Path = CAMPAIGN_ROOT,
) -> Optional[dict]:
    """Returns the candidate's closeout status dict ONLY if it is internally
    well-formed, its stored `verdict` is the recomputation of its own stored
    `evidence` under the frozen advancement_policy (Finding 5), its stored
    `evidence_hash` matches a fresh hash of that same stored `evidence`
    (Finding 5, item 7 -- an evidence artifact mutation must fail
    verification even if someone also patched the verdict to match), AND
    every trial_id it cites is independently confirmed, in the shared
    durable registry, to be registered under exactly the candidate's own
    declared hypothesis_ids under CAMPAIGN_REAL_EXPERIMENT_ID with at least
    one 'succeeded' attempt. Returns None (never a partial or guessed truth)
    on ANY of these checks failing -- a hand-edited verdict or evidence
    field is never trusted over this recomputation."""
    campaign = campaign or load_campaign(campaign_root)
    path = candidate_closeout_status_path(candidate_key, campaign, campaign_root)
    if not path.is_file():
        return None
    try:
        status = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None

    if not isinstance(status, dict):
        return None
    if status.get("campaign_id") != campaign.get("campaign_id"):
        return None
    if status.get("candidate_key") != candidate_key:
        return None
    verdict = status.get("verdict")
    if verdict not in TERMINAL_VERDICTS:
        return None

    evidence = status.get("evidence")
    stored_hash = status.get("evidence_hash")
    if not isinstance(evidence, dict) or not isinstance(stored_hash, str):
        return None
    if evidence_hash(evidence) != stored_hash:
        return None
    try:
        recomputed = classify_verdict(evidence, campaign["advancement_policy"])
    except EvidenceRefusal:
        return None
    if recomputed["verdict"] != verdict:
        return None
    if status.get("gates") != recomputed["gates"]:
        return None

    hypothesis_ids = status.get("hypothesis_ids")
    trial_ids = status.get("verified_trial_ids")
    if not isinstance(hypothesis_ids, list) or not hypothesis_ids:
        return None
    if not isinstance(trial_ids, dict) or set(trial_ids.keys()) != set(hypothesis_ids):
        return None

    # Finding 3: the closeout's own cited hypothesis_ids must be EXACTLY
    # this candidate's own frozen real_candidate_population -- never merely
    # "some hypothesis ids that happen to be registered somewhere" (which
    # would let, e.g., a LIQ-01 closeout be satisfied by VOL-01's own
    # trials/hypothesis ids).
    try:
        decl = load_candidate_declaration(candidate_key, campaign_root)
    except (OSError, ValueError, KeyError):
        return None
    expected_hypothesis_ids = decl.get("real_candidate_population")
    if not isinstance(expected_hypothesis_ids, list) or set(expected_hypothesis_ids) != set(hypothesis_ids):
        return None

    from mqk_research.exp_distributed.storage import ResearchResultStore

    store = ResearchResultStore(Path(registry_db))
    for hyp_id, trial_id in trial_ids.items():
        matching = [
            t
            for t in store.list_trials(experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID, hypothesis_id=hyp_id)
            if t["trial_id"] == trial_id
        ]
        if len(matching) != 1:
            return None
        # Finding 2: a cited trial's real authority is either a genuinely
        # succeeded attempt OR a real registry attempt whose failure_reason
        # is EXACTLY the recognized gross-wealth-insolvency string -- never
        # only "succeeded" (that would make the frozen policy-terminal
        # economic-failure path unverifiable), and never a generic/
        # operational failure (resolve_attempt_outcome's own "incomplete"
        # classification, which never authorizes a terminal verdict).
        try:
            outcome = resolve_attempt_outcome(store, trial_id)
        except AuthorityRefusal:
            return None
        if outcome["status"] not in ("succeeded", "gross_insolvency_failed"):
            return None
    return status


def write_closeout_status(
    candidate_key: str,
    *,
    registry_db: Path,
    benchmark_artifact_path: Optional[Path] = None,
    judge_artifact_sha256: Optional[str] = None,
    genuine_placebo_artifact_path: Optional[Path] = None,
    dsr_pbo_sensitivity_artifact_path: Optional[Path] = None,
    campaign_root: Path = CAMPAIGN_ROOT,
) -> Path:
    """The ONLY sanctioned writer of CANDIDATE_CLOSEOUT_STATUS.json.

    Finding 1 (W06-A-CAMPAIGN-CLOSEOUT-AUTHORITY-REPAIR-03): the caller no
    longer supplies `evidence`, `hypothesis_ids`, or `verified_trial_ids` --
    every one of those is DERIVED by
    campaign_closeout_authority.resolve_authoritative_evidence() from real
    ResearchResultStore trials/attempts and real registered/artifact
    authority. The caller supplies only IDENTITY/LOCATION inputs (a
    registered judge's sha256, real evaluator-CLI output file paths) needed
    to resolve those real authorities -- never a gate result. The verdict
    is then COMPUTED by classify_verdict() from that resolved evidence and
    the frozen advancement_policy, exactly as before (Finding 5, item 1).
    Fails closed (raises, writes nothing) on any authority resolution
    failure (AuthorityRefusal, including its MissingAuthoritativeSeam
    subclass -- see that module's docstring for the one gate this repo
    cannot yet resolve at all), on evidence classify_verdict() cannot
    evaluate, or if the computed verdict is somehow non-terminal
    (defensive; classify_verdict() only ever returns a TERMINAL_VERDICTS
    member)."""
    campaign = load_campaign(campaign_root)
    evidence, hypothesis_ids, verified_trial_ids = resolve_authoritative_evidence(
        candidate_key,
        registry_db=registry_db,
        campaign_root=campaign_root,
        benchmark_artifact_path=benchmark_artifact_path,
        judge_artifact_sha256=judge_artifact_sha256,
        genuine_placebo_artifact_path=genuine_placebo_artifact_path,
        dsr_pbo_sensitivity_artifact_path=dsr_pbo_sensitivity_artifact_path,
    )
    if set(verified_trial_ids.keys()) != set(hypothesis_ids):
        raise ValueError("verified_trial_ids must have exactly one entry per declared hypothesis_id")
    result = classify_verdict(evidence, campaign["advancement_policy"])
    verdict = result["verdict"]
    if verdict not in TERMINAL_VERDICTS:
        raise ValueError(f"refusing to write a non-terminal verdict: {verdict!r}")
    status = {
        "campaign_id": campaign["campaign_id"],
        "candidate_key": candidate_key,
        "verdict": verdict,
        "gates": result["gates"],
        "evidence": evidence,
        "evidence_hash": evidence_hash(evidence),
        "hypothesis_ids": sorted(hypothesis_ids),
        "verified_trial_ids": dict(verified_trial_ids),
    }
    path = candidate_closeout_status_path(candidate_key, campaign, campaign_root)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(status, sort_keys=True, indent=2, default=str, allow_nan=False), encoding="utf-8")
    return path


def require_authorized_to_execute(
    candidate_key: str, *, registry_db: Path, campaign_root: Path = CAMPAIGN_ROOT
) -> None:
    """Fail-closed campaign-order gate. The first candidate in
    campaign_order is always authorized. Any later candidate is refused
    unless EVERY earlier candidate in campaign_order has a registry-
    verified closeout whose verdict is in NOT_ADVANCED_VERDICTS. A missing,
    malformed, crashed, or ADVANCED closeout for any earlier candidate
    refuses -- never silently authorizes -- this candidate. Raises
    CampaignOrderRefusal on refusal; returns None (silently) when
    authorized."""
    campaign = load_campaign(campaign_root)
    order = campaign["campaign_order"]
    if candidate_key not in order:
        raise CampaignOrderRefusal(f"{candidate_key!r} is not a frozen campaign candidate: {order!r}")
    position = order.index(candidate_key)
    for earlier_key in order[:position]:
        closeout = load_verified_closeout(
            earlier_key, registry_db=registry_db, campaign=campaign, campaign_root=campaign_root
        )
        if closeout is None:
            raise CampaignOrderRefusal(
                f"REFUSED: {candidate_key!r} requires a registry-verified closeout for {earlier_key!r} "
                "(prior candidate in the frozen campaign_order) before it may execute; none found or "
                "unverifiable -- a missing/crashed/incomplete attempt never authorizes the next candidate"
            )
        if closeout["verdict"] not in NOT_ADVANCED_VERDICTS:
            raise CampaignOrderRefusal(
                f"REFUSED: {earlier_key!r} closeout verdict {closeout['verdict']!r} satisfies the "
                "campaign stopping rule -- the remaining campaign is stopped; no later candidate may execute"
            )
