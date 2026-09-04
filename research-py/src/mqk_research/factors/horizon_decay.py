"""
RESEARCH-FACTOR-IC-IR-QUANTILE-BENCH-01

Horizon/decay analysis: an AGGREGATE COMPARISON over the FULL, durable
REGISTERED horizon family of one factor, read from the registry -- never
an arbitrary caller-assembled Sequence[(factor_id, attempt_id)]. A caller
choosing which attempts to include could otherwise omit an unfavorable
horizon, or retry-shop for a favorable outcome; this module derives both
the family population and the comparison scope from registry truth, and
selects each family member's authoritative attempt with a deterministic,
result-independent rule.

`FactorSpec.horizon_periods` is part of `factor_id`'s identity payload
(see `mqk_research.factors.contracts`): a different horizon is therefore a
genuinely different semantic factor candidate, with its own registered
factor row. The HORIZON FAMILY is every registered factor sharing the
anchor's identity apart from `horizon_periods`. The COMPARISON SCOPE
(universe, evaluation window, label protocol, and P1's diagnostic
protocol identity -- everything in an evaluation identity except
`factor_id`, which legitimately differs per horizon) is read from one
caller-named anchor evaluation, never invented.

For each family member, the AUTHORITATIVE attempt is the one with the
highest `attempt_index` among that member's attempts whose evaluation
identity matches the comparison scope AND whose status is terminal
(never `started`) -- this is deterministic and result-independent: a
caller cannot select a favorable earlier attempt once a later retry of
the same scope exists, and two attempts of the same member can never
produce two horizon points. A family member with no terminal attempt
under the comparison scope is reported as incomplete, never silently
dropped from `family_identity`/`incomplete_factor_ids` accounting.
"""
from __future__ import annotations

from pathlib import Path
from typing import Any, Dict, List

from .registry import get_factor, list_factor_evaluation_attempts, list_factors

FACTOR_HORIZON_DECAY_REPORT_SCHEMA_VERSION = "factor_horizon_decay_report_v2"

HORIZON_STATUS_COMPLETE = "complete"
HORIZON_STATUS_INCOMPLETE = "incomplete"

# Durable attempt row status meaning "opened, not yet terminal" (see
# ResearchResultStore.begin_factor_evaluation_attempt) -- never eligible as
# an authoritative horizon point, since it carries no result evidence yet.
_ATTEMPT_STATUS_STARTED = "started"

__all__ = [
    "FACTOR_HORIZON_DECAY_REPORT_SCHEMA_VERSION",
    "HORIZON_STATUS_COMPLETE",
    "HORIZON_STATUS_INCOMPLETE",
    "build_factor_horizon_decay_report",
]


def _non_horizon_identity(identity: Dict[str, Any]) -> Dict[str, Any]:
    return {k: v for k, v in identity.items() if k != "horizon_periods"}


def _non_factor_evaluation_identity(evaluation_identity: Dict[str, Any]) -> Dict[str, Any]:
    return {k: v for k, v in evaluation_identity.items() if k != "factor_id"}


def build_factor_horizon_decay_report(
    registry_db: Path,
    *,
    anchor_factor_id: str,
    anchor_evaluation_id: str,
) -> Dict[str, Any]:
    """Build a deterministic horizon/decay comparison over the full
    registered horizon family of `anchor_factor_id`, compared under the
    exact comparison scope named by `anchor_evaluation_id`.

    `anchor_factor_id`/`anchor_evaluation_id` name WHICH family and WHICH
    scope to report on -- they are identity, fixed before any result is
    known, never an outcome-based selection. Every other family member and
    every member's authoritative attempt are then derived entirely from
    registry truth:

      - HORIZON FAMILY: every registered factor (via `list_factors`) whose
        identity is identical to the anchor's apart from `horizon_periods`.
      - COMPARISON SCOPE: `anchor_evaluation_id`'s own evaluation identity
        (universe, evaluation window, label protocol, diagnostic protocol),
        apart from `factor_id`, read off one durable attempt that produced
        it.
      - AUTHORITATIVE ATTEMPT per family member: the highest
        `attempt_index` among that member's TERMINAL attempts (never
        `started`) whose evaluation identity matches the comparison scope.
        This is deterministic and result-independent -- a member with a
        succeeded attempt followed by a later failed/not_evaluable retry
        of the SAME scope resolves to that later attempt, never the older
        success; two attempts of the same member can never both appear.

    Fails closed (raises) if `anchor_factor_id` is unregistered or
    `anchor_evaluation_id` does not match any of its attempts -- the
    comparison scope cannot be derived from an evaluation that was never
    attempted. A family member with no terminal attempt under the
    comparison scope is never silently omitted: it is recorded in
    `incomplete_factor_ids` and the report's `status` is
    `HORIZON_STATUS_INCOMPLETE`, never a false `complete`.
    """
    anchor_factor = get_factor(registry_db, anchor_factor_id)
    family_identity = _non_horizon_identity(anchor_factor["identity"])

    anchor_attempts = list_factor_evaluation_attempts(registry_db, anchor_factor_id)
    anchor_matches = [a for a in anchor_attempts if a.get("evaluation_id") == anchor_evaluation_id]
    if not anchor_matches:
        raise ValueError(
            f"anchor_evaluation_id={anchor_evaluation_id!r} does not match any evaluation attempt of "
            f"anchor_factor_id={anchor_factor_id!r} -- the comparison scope cannot be derived from an "
            "evaluation that was never attempted"
        )
    reference_evaluation_identity = _non_factor_evaluation_identity(anchor_matches[0]["evaluation_identity"])

    family_members = [
        f
        for f in list_factors(registry_db, family=anchor_factor["family"])
        if _non_horizon_identity(f["identity"]) == family_identity
    ]

    horizons: List[Dict[str, Any]] = []
    incomplete_factor_ids: List[str] = []

    for member in sorted(family_members, key=lambda f: f["factor_id"]):
        factor_id = member["factor_id"]
        attempts = list_factor_evaluation_attempts(registry_db, factor_id)
        matching = [
            a
            for a in attempts
            if a["status"] != _ATTEMPT_STATUS_STARTED
            and a.get("evaluation_identity")
            and _non_factor_evaluation_identity(a["evaluation_identity"]) == reference_evaluation_identity
        ]
        if not matching:
            incomplete_factor_ids.append(factor_id)
            continue

        # Deterministic, result-independent authority: the latest
        # attempt_index among this member's terminal attempts under the
        # comparison scope -- never a caller-selected favorable attempt.
        authoritative = max(matching, key=lambda a: a["attempt_index"])

        metrics = (authoritative.get("result_summary") or {}).get("metrics") or {}
        quantile = metrics.get("quantile") or {}
        horizons.append(
            {
                "factor_id": factor_id,
                "evaluation_id": authoritative["evaluation_id"],
                "attempt_id": authoritative["attempt_id"],
                "horizon_periods": member["identity"].get("horizon_periods"),
                "status": authoritative["status"],
                "reason": authoritative.get("failure_reason"),
                "mean_ic": metrics.get("mean_ic"),
                "ic_information_ratio": metrics.get("ic_information_ratio"),
                "positive_ic_fraction": metrics.get("positive_ic_fraction"),
                "top_minus_bottom_spread": quantile.get("top_minus_bottom_spread"),
                "coverage": metrics.get("coverage"),
            }
        )

    horizons.sort(key=lambda row: row["horizon_periods"])

    return {
        "schema_version": FACTOR_HORIZON_DECAY_REPORT_SCHEMA_VERSION,
        "family_identity": family_identity,
        "evaluation_scope_identity": reference_evaluation_identity,
        "status": HORIZON_STATUS_INCOMPLETE if incomplete_factor_ids else HORIZON_STATUS_COMPLETE,
        "incomplete_factor_ids": sorted(incomplete_factor_ids),
        "horizons": horizons,
    }
