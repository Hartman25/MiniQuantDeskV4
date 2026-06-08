"""
WATCHLIST-PROMO-01 — MAIN scanner watchlist promotion gate.

Evaluates a watchlist-v1 artifact and strategy-fit-v1 artifacts against all
promotion requirements and produces a PromotionDecision.

Promotion gates (evaluated in order):
1. watchlist_schema_valid     — schema_version == "watchlist-v1"
2. watchlist_mode_paper       — mode == "paper"
3. watchlist_live_locked      — approved_for_live != True in input
4. has_ranked_candidates      — at least one symbol present
5. strategy_fit_present       — strategy-fit artifact exists for top symbol, and its
                                 symbol/strategy_id match the watchlist's top-symbol
                                 identity and assignment (fail-closed on mismatch)
6. strategy_fit_recommended   — artifact has recommended_for_paper == True
7. risk_simulation_passed     — config.risk_simulation_passed == True
8. operator_review_approved   — config.operator_review_approved == True
9. premarket_revalidation     — config.premarket_revalidation_required == False

Fail-closed rules:
- Any gate failure → approved_for_autonomous_paper=False, approved_for_live=False.
- operator_review_approved defaults False → fail closed until explicit operator sign-off.
- risk_simulation_passed defaults False → fail closed until placeholder passes.
- premarket_revalidation_required defaults True → fail closed; cleared by WATCHLIST-PREMARKET-01.
- approved_for_live is ALWAYS False — not overrideable by caller or config.
- max_symbols_to_trade=1 and max_concurrent_positions=1 forced in v1.
- Input watchlist with approved_for_live already True is forced False; adds live_approval_forbidden reason.
- Does NOT write to operator config directories.
- Output goes to exports/watchlist/.

Hard invariants:
- approved_for_live is ALWAYS False
- approved_for_autonomous_paper is False unless ALL gates pass
- max_symbols_to_trade is forced to 1 in v1
- max_concurrent_positions is forced to 1 in v1
- No broker/OMS/execution imports; no network/DB imports
- No orders placed; no DB mutations; no subprocess/mqk-backtest calls
- JSON artifact only; EXP penny scanner (exp-candidate-v1) is not affected
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

SCHEMA_VERSION_WATCHLIST = "watchlist-v1"

# Failure reason constants — stable strings used in failure_reasons lists.
REASON_WATCHLIST_SCHEMA_INVALID = "watchlist_schema_invalid"
REASON_WATCHLIST_MODE_NOT_PAPER = "watchlist_mode_not_paper"
REASON_WATCHLIST_LIVE_NOT_LOCKED = "watchlist_live_not_locked"
REASON_NO_RANKED_CANDIDATES = "no_ranked_candidates"
REASON_STRATEGY_FIT_MISSING = "strategy_fit_missing"
REASON_STRATEGY_FIT_SYMBOL_MISMATCH = "strategy_fit_symbol_mismatch"
REASON_STRATEGY_FIT_STRATEGY_MISMATCH = "strategy_fit_strategy_mismatch"
REASON_STRATEGY_FIT_NOT_RECOMMENDED_FOR_PAPER = "strategy_fit_not_recommended_for_paper"
REASON_RISK_SIMULATION_REQUIRED = "risk_simulation_required"
REASON_OPERATOR_REVIEW_REQUIRED = "operator_review_required"
REASON_PREMARKET_REVALIDATION_REQUIRED = "premarket_revalidation_required"
REASON_LIVE_APPROVAL_FORBIDDEN = "live_approval_forbidden"


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class WatchlistPromotionConfig:
    operator_review_approved: bool = False
    risk_simulation_passed: bool = False
    premarket_revalidation_required: bool = True
    max_symbols_to_trade: int = 1
    max_concurrent_positions: int = 1
    approved_for_live: bool = False

    def __post_init__(self) -> None:
        # Hard invariant: live approval is never permitted through config.
        object.__setattr__(self, "approved_for_live", False)


# ---------------------------------------------------------------------------
# Input / output types
# ---------------------------------------------------------------------------

@dataclass
class PromotionInput:
    """Convenience bundle for evaluation inputs."""
    watchlist: dict[str, Any]
    strategy_fit_artifacts: dict[str, dict[str, Any]]  # symbol → strategy-fit-v1


@dataclass
class PromotionDecision:
    """Result of evaluating all promotion gates for a watchlist artifact."""
    approved_for_autonomous_paper: bool
    approved_for_live: bool  # always False
    passed: bool
    failure_reasons: list[str] = field(default_factory=list)
    approved_symbols: list[str] = field(default_factory=list)
    strategy_assignments: dict[str, str] = field(default_factory=dict)
    notes: str = ""

    def to_dict(self) -> dict[str, Any]:
        return {
            "approved_for_autonomous_paper": self.approved_for_autonomous_paper,
            "approved_for_live": self.approved_for_live,
            "passed": self.passed,
            "failure_reasons": list(self.failure_reasons),
            "approved_symbols": list(self.approved_symbols),
            "strategy_assignments": dict(self.strategy_assignments),
            "notes": self.notes,
        }


# ---------------------------------------------------------------------------
# Pure helpers
# ---------------------------------------------------------------------------

def _deduplicate_reasons(reasons: list[str]) -> list[str]:
    """Preserve insertion order; remove exact duplicates."""
    seen: set[str] = set()
    result: list[str] = []
    for r in reasons:
        if r not in seen:
            seen.add(r)
            result.append(r)
    return result


# ---------------------------------------------------------------------------
# Gate evaluator
# ---------------------------------------------------------------------------

def evaluate_watchlist_promotion(
    watchlist: dict[str, Any],
    strategy_fit_artifacts: dict[str, dict[str, Any]],
    config: Optional[WatchlistPromotionConfig] = None,
    risk_simulation_result: Optional[dict[str, Any]] = None,
    premarket_revalidation_result: Optional[dict[str, Any]] = None,
) -> PromotionDecision:
    """
    Evaluate a watchlist-v1 artifact against all promotion gates.

    Returns a PromotionDecision; does not modify the input artifacts.

    risk_simulation_result: if provided, its "passed" bool overrides config.risk_simulation_passed.
    premarket_revalidation_result: if provided, its "passed" bool determines whether
      premarket revalidation is still required (not passed → required=True).

    Fail-closed: any gate failure → approved_for_autonomous_paper=False.
    approved_for_live is always False.
    Only the top-ranked symbol (symbols[0]) is considered for approval in v1.
    """
    cfg = config or WatchlistPromotionConfig()
    failure_reasons: list[str] = []

    # Resolve effective gate states from result dicts (override config booleans when provided).
    effective_risk_passed = (
        bool(risk_simulation_result.get("passed", False))
        if risk_simulation_result is not None
        else cfg.risk_simulation_passed
    )
    effective_premarket_required = (
        not bool(premarket_revalidation_result.get("passed", False))
        if premarket_revalidation_result is not None
        else cfg.premarket_revalidation_required
    )

    # Gate 1: schema version must be watchlist-v1
    if watchlist.get("schema_version") != SCHEMA_VERSION_WATCHLIST:
        failure_reasons.append(REASON_WATCHLIST_SCHEMA_INVALID)

    # Gate 2: mode must be paper
    if watchlist.get("mode") != "paper":
        failure_reasons.append(REASON_WATCHLIST_MODE_NOT_PAPER)

    # Gate 3: approved_for_live must not be True in input
    if watchlist.get("approved_for_live") is True:
        failure_reasons.append(REASON_WATCHLIST_LIVE_NOT_LOCKED)
        failure_reasons.append(REASON_LIVE_APPROVAL_FORBIDDEN)

    # Gate 4: must have at least one candidate symbol
    symbols: list[str] = watchlist.get("symbols") or []
    if not symbols:
        failure_reasons.append(REASON_NO_RANKED_CANDIDATES)

    # Gates 5-6: strategy-fit artifact for top symbol — presence, identity binding
    # (symbol/strategy_id must match the watchlist's top-symbol assignment; a
    # mismatched or forged identity fails closed rather than silently adopting
    # a strategy/symbol the watchlist never assigned), and paper recommendation.
    top_symbol: Optional[str] = symbols[0] if symbols else None
    top_strategy_id: Optional[str] = None

    if top_symbol is not None:
        fit_artifact = strategy_fit_artifacts.get(top_symbol)
        assigned_strategy_id = (watchlist.get("strategy_assignments") or {}).get(top_symbol)
        if fit_artifact is None:
            failure_reasons.append(REASON_STRATEGY_FIT_MISSING)
        elif fit_artifact.get("symbol") not in (None, top_symbol):
            failure_reasons.append(REASON_STRATEGY_FIT_SYMBOL_MISMATCH)
        elif (
            assigned_strategy_id is not None
            and fit_artifact.get("strategy_id") is not None
            and fit_artifact.get("strategy_id") != assigned_strategy_id
        ):
            failure_reasons.append(REASON_STRATEGY_FIT_STRATEGY_MISMATCH)
        elif not fit_artifact.get("recommended_for_paper"):
            failure_reasons.append(REASON_STRATEGY_FIT_NOT_RECOMMENDED_FOR_PAPER)
        else:
            # Prefer strategy_id from the validated fit artifact; fall back to watchlist.
            top_strategy_id = fit_artifact.get("strategy_id") or assigned_strategy_id

    # Gate 7: risk simulation — driven by result dict if provided, else config boolean
    if not effective_risk_passed:
        failure_reasons.append(REASON_RISK_SIMULATION_REQUIRED)

    # Gate 8: operator review sign-off
    if not cfg.operator_review_approved:
        failure_reasons.append(REASON_OPERATOR_REVIEW_REQUIRED)

    # Gate 9: premarket revalidation — driven by result dict if provided, else config boolean
    if effective_premarket_required:
        failure_reasons.append(REASON_PREMARKET_REVALIDATION_REQUIRED)

    failure_reasons = _deduplicate_reasons(failure_reasons)
    passed = len(failure_reasons) == 0

    # Approved symbols: only the top-ranked symbol in v1, only if all gates pass.
    if passed and top_symbol is not None:
        approved_symbols = [top_symbol]
        assignments: dict[str, str] = {}
        if top_strategy_id:
            assignments[top_symbol] = top_strategy_id
    else:
        approved_symbols = []
        assignments = {}

    notes = (
        "promoted: all_gates_passed"
        if passed
        else "blocked: " + "; ".join(failure_reasons)
    )

    return PromotionDecision(
        approved_for_autonomous_paper=passed,
        approved_for_live=False,
        passed=passed,
        failure_reasons=failure_reasons,
        approved_symbols=approved_symbols,
        strategy_assignments=assignments,
        notes=notes,
    )


# ---------------------------------------------------------------------------
# Artifact updater
# ---------------------------------------------------------------------------

def apply_watchlist_promotion(
    watchlist: dict[str, Any],
    decision: PromotionDecision,
    config: Optional[WatchlistPromotionConfig] = None,
) -> dict[str, Any]:
    """
    Return a new watchlist artifact dict with the promotion decision applied.

    Does not modify the input watchlist dict.
    approved_for_live is always forced to False.
    max_symbols_to_trade and max_concurrent_positions are forced to 1 (v1).
    symbols and strategy_assignments reflect only approved symbols.
    """
    updated = dict(watchlist)

    # Hard invariants
    updated["approved_for_autonomous_paper"] = decision.approved_for_autonomous_paper
    updated["approved_for_live"] = False
    updated["max_symbols_to_trade"] = 1
    updated["max_concurrent_positions"] = 1

    # Only approved symbols are exposed for trading (empty if not passed)
    updated["symbols"] = list(decision.approved_symbols)
    updated["strategy_assignments"] = dict(decision.strategy_assignments)

    updated["selection_reason"] = decision.notes
    updated["promotion_decision"] = {
        "passed": decision.passed,
        "failure_reasons": list(decision.failure_reasons),
        "approved_symbols": list(decision.approved_symbols),
        "strategy_assignments": dict(decision.strategy_assignments),
        "notes": decision.notes,
    }

    return updated


# ---------------------------------------------------------------------------
# Writer
# ---------------------------------------------------------------------------

def write_promoted_watchlist(watchlist: dict[str, Any], path: str) -> Path:
    """Write a promoted watchlist artifact as JSON. Parent dirs created automatically."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(watchlist, indent=2, default=str), encoding="utf-8")
    return out
