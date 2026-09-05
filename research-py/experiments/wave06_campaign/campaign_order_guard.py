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

This module is the ONLY sanctioned execution-order gate. It never infers
order from filesystem directory existence, never fabricates a result, and
never trusts a closeout claim without independently re-verifying it against
real, registered, succeeded trials in the shared campaign registry (see
campaign_identity.py) AND recomputing its verdict from its own stored
evidence. A missing, malformed, crashed, or incomplete prior attempt
refuses the next candidate -- it never silently authorizes it.

CANDIDATE_CLOSEOUT_STATUS.json is the only artifact this guard trusts, and
write_closeout_status() is the only sanctioned way to produce one -- no
other code path may hand-author this file. Writing one still requires the
caller to have independently applied PREDECLARED_CAMPAIGN.json's
advancement_policy's gate evidence to a real, already-registered,
already-succeeded trial pair; this module does not itself compute
DSR/PBO/placebo/stress evidence -- that is a separate, later mission's
scope (see CLAUDE.md Section 30: this predeclaration-authority repair only
builds the gate, it does not run any real evaluation).
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
from campaign_identity import CAMPAIGN_REAL_EXPERIMENT_ID, load_campaign, resolve_local_src  # noqa: E402

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
        attempts = store.list_attempts(trial_id)
        if not any(a["status"] == "succeeded" for a in attempts):
            return None
    return status


def write_closeout_status(
    candidate_key: str,
    *,
    evidence: Dict[str, Any],
    hypothesis_ids: list[str],
    verified_trial_ids: dict[str, str],
    campaign_root: Path = CAMPAIGN_ROOT,
) -> Path:
    """The ONLY sanctioned writer of CANDIDATE_CLOSEOUT_STATUS.json. The
    verdict is COMPUTED here by classify_verdict() from the caller-supplied
    `evidence` and the frozen advancement_policy -- callers never supply a
    verdict string directly (Finding 5, item 1). Fails closed (raises,
    writes nothing) on a hypothesis/trial-id mismatch, on evidence
    classify_verdict() cannot evaluate, or if the computed verdict is
    somehow non-terminal (defensive; classify_verdict() only ever returns a
    TERMINAL_VERDICTS member)."""
    if set(verified_trial_ids.keys()) != set(hypothesis_ids):
        raise ValueError("verified_trial_ids must have exactly one entry per declared hypothesis_id")
    campaign = load_campaign(campaign_root)
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
    path.write_text(json.dumps(status, sort_keys=True, indent=2, default=str), encoding="utf-8")
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
