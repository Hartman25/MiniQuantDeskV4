"""W06-A-CAMPAIGN-PREDECLARATION-AUTHORITY-REPAIR-01 (Finding 4) --
deterministic campaign-order execution authority.

Prior defect (Finding 4): each candidate's run_wave.py could execute its
own --execute stage independently -- nothing mechanically proved LIQ-01 had
been attempted, let alone that it had honestly failed the frozen
advancement policy, before VOL-01 could run. Order was documented in
PREDECLARED_CAMPAIGN.json's stopping_rule but not enforced.

This module is the ONLY sanctioned execution-order gate. It never infers
order from filesystem directory existence, never fabricates a result, and
never trusts a closeout claim without independently re-verifying it against
real, registered, succeeded trials in the shared campaign registry (see
campaign_identity.py). A missing, malformed, crashed, or incomplete prior
attempt refuses the next candidate -- it never silently authorizes it.

CANDIDATE_CLOSEOUT_STATUS.json is the only artifact this guard trusts, and
write_closeout_status() is the only sanctioned way to produce one -- no
other code path may hand-author this file. Writing one still requires the
caller to have independently applied PREDECLARED_CAMPAIGN.json's
advancement_policy to a real, already-registered, already-succeeded trial
pair; this module does not itself compute DSR/PBO/placebo/stress evidence
-- that is a separate, later mission's scope (see CLAUDE.md Section 30:
this predeclaration-authority repair only builds the gate, it does not run
any real evaluation).
"""
from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Optional

CAMPAIGN_ROOT = Path(__file__).resolve().parent
if str(CAMPAIGN_ROOT) not in sys.path:
    sys.path.insert(0, str(CAMPAIGN_ROOT))

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
    well-formed AND every trial_id it cites is independently confirmed, in
    the shared durable registry, to be registered under exactly the
    candidate's own declared hypothesis_ids under CAMPAIGN_REAL_EXPERIMENT_ID
    with at least one 'succeeded' attempt. Returns None (never a partial or
    guessed truth) if the file is missing, malformed, cites the wrong
    campaign/candidate, cites a non-terminal verdict, or cites a trial_id
    the registry does not independently confirm."""
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
    verdict: str,
    hypothesis_ids: list[str],
    verified_trial_ids: dict[str, str],
    campaign_root: Path = CAMPAIGN_ROOT,
) -> Path:
    """The ONLY sanctioned writer of CANDIDATE_CLOSEOUT_STATUS.json. Fails
    closed on a non-terminal verdict or a hypothesis/trial-id mismatch --
    never writes a claim this module cannot later re-verify against the
    registry."""
    if verdict not in TERMINAL_VERDICTS:
        raise ValueError(f"refusing to write a non-terminal verdict: {verdict!r}")
    if set(verified_trial_ids.keys()) != set(hypothesis_ids):
        raise ValueError("verified_trial_ids must have exactly one entry per declared hypothesis_id")
    campaign = load_campaign(campaign_root)
    status = {
        "campaign_id": campaign["campaign_id"],
        "candidate_key": candidate_key,
        "verdict": verdict,
        "hypothesis_ids": sorted(hypothesis_ids),
        "verified_trial_ids": dict(verified_trial_ids),
    }
    path = candidate_closeout_status_path(candidate_key, campaign, campaign_root)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(status, sort_keys=True, indent=2), encoding="utf-8")
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
