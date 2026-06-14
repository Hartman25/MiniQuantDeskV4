"""
WATCHLIST-PROMO-01 — MAIN scanner watchlist promotion gate.

Evaluates a watchlist-v1 (single-symbol) or watchlist-v2 (multi-symbol) artifact
and strategy-fit-v1 artifacts against all promotion requirements and produces a
PromotionDecision.

WATCHLIST-PROMO-V2-MULTI-SYMBOL-01 extends gate evaluation to watchlist-v2
artifacts without changing watchlist-v1 behavior.

Promotion gates (evaluated in order):
1. watchlist_schema_valid     — schema_version == "watchlist-v1" or "watchlist-v2"
2. watchlist_mode_paper       — mode == "paper"
3. watchlist_live_locked      — approved_for_live != True in input
4. has_ranked_candidates      — at least one symbol present
4b. watchlist_v2_limits_valid — (v2 only) max_symbols_to_trade and
                                 max_concurrent_positions are valid positive
                                 integers, and max_symbols_to_trade does not
                                 exceed MULTI_SYMBOL_HARD_CEILING
5. strategy_fit_present       — strategy-fit artifact exists for each selected
                                 symbol, and its symbol/strategy_id match the
                                 watchlist's assignment for that symbol
                                 (fail-closed on mismatch; v2 also requires an
                                 explicit strategy_assignments entry per symbol)
6. strategy_fit_recommended   — artifact has recommended_for_paper == True for
                                 every selected symbol
7. risk_simulation_passed     — config.risk_simulation_passed == True (or
                                 risk_simulation_result["passed"] == True)
8. operator_review_approved   — config.operator_review_approved == True
9. premarket_revalidation     — config.premarket_revalidation_required == False
                                 (v2: honored per-symbol via
                                 premarket_revalidation_result["symbol_results"]
                                 when present)

Selected symbols:
- v1: only symbols[0] (the top-ranked symbol) is ever considered.
- v2: symbols[:effective_limit], where
      effective_limit = min(max_symbols_to_trade, max_concurrent_positions,
      MULTI_SYMBOL_HARD_CEILING).

Fail-closed rules:
- Any gate failure → approved_for_autonomous_paper=False, approved_for_live=False.
- operator_review_approved defaults False → fail closed until explicit operator sign-off.
- risk_simulation_passed defaults False → fail closed until placeholder passes.
- premarket_revalidation_required defaults True → fail closed; cleared by WATCHLIST-PREMARKET-01.
- approved_for_live is ALWAYS False — not overrideable by caller or config.
- v1: max_symbols_to_trade=1 and max_concurrent_positions=1 forced.
- v2: max_symbols_to_trade and max_concurrent_positions carry through unchanged
  from the input (already validated by gate 4b); approval never exceeds
  MULTI_SYMBOL_HARD_CEILING and never falls back to symbols[0] silently.
- Input watchlist with approved_for_live already True is forced False; adds live_approval_forbidden reason.
- Does NOT write to operator config directories.
- Output goes to exports/watchlist/.

Hard invariants:
- approved_for_live is ALWAYS False
- approved_for_autonomous_paper is False unless ALL gates pass
- max_symbols_to_trade is forced to 1 in v1
- max_concurrent_positions is forced to 1 in v1
- v2 approval never exceeds MULTI_SYMBOL_HARD_CEILING (= 5) symbols
- No broker/OMS/execution imports; no network/DB imports
- No orders placed; no DB mutations; no subprocess/mqk-backtest calls
- JSON artifact only; EXP penny scanner (exp-candidate-v1) is not affected
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping, Optional

SCHEMA_VERSION_WATCHLIST = "watchlist-v1"
SCHEMA_VERSION_WATCHLIST_V2 = "watchlist-v2"

# Hard ceiling on the number of symbols a watchlist-v2 artifact may approve for
# autonomous paper trading. Mirrors core-rs/crates/mqk-daemon/src/watchlist_intake.rs
# MULTI_SYMBOL_HARD_CEILING. Exceeding it (max_symbols_to_trade > ceiling) fails closed.
MULTI_SYMBOL_HARD_CEILING = 5

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

# WATCHLIST-PROMO-V2-MULTI-SYMBOL-01 — watchlist-v2 multi-symbol reason constants.
REASON_WATCHLIST_V2_SYMBOL_ASSIGNMENT_MISSING = "watchlist_v2_symbol_assignment_missing"
REASON_WATCHLIST_V2_MAX_SYMBOLS_INVALID = "watchlist_v2_max_symbols_invalid"
REASON_WATCHLIST_V2_CONCURRENT_LIMIT_INVALID = "watchlist_v2_concurrent_limit_invalid"
REASON_WATCHLIST_V2_HARD_CEILING_EXCEEDED = "watchlist_v2_hard_ceiling_exceeded"


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
# v2 multi-symbol helpers (WATCHLIST-PROMO-V2-MULTI-SYMBOL-01)
# ---------------------------------------------------------------------------

def _watchlist_schema_version(watchlist: Mapping[str, Any]) -> str:
    """Return the watchlist's schema_version, or "" if absent."""
    return str(watchlist.get("schema_version") or "")


def _is_watchlist_v2(watchlist: Mapping[str, Any]) -> bool:
    """True iff the watchlist declares schema_version == "watchlist-v2"."""
    return _watchlist_schema_version(watchlist) == SCHEMA_VERSION_WATCHLIST_V2


def _v2_effective_symbol_limit(watchlist: Mapping[str, Any]) -> tuple[Optional[int], list[str]]:
    """
    Validate watchlist-v2 max_symbols_to_trade / max_concurrent_positions and
    compute the effective approval limit:
        effective_limit = min(max_symbols_to_trade, max_concurrent_positions,
                               MULTI_SYMBOL_HARD_CEILING)

    Returns (effective_limit, validation_failure_reasons). effective_limit is
    None when the artifact's limits are invalid or exceed
    MULTI_SYMBOL_HARD_CEILING — callers must then select zero symbols (fail closed).
    """
    reasons: list[str] = []
    max_symbols = watchlist.get("max_symbols_to_trade")
    max_concurrent = watchlist.get("max_concurrent_positions")

    valid_max_symbols = (
        isinstance(max_symbols, int) and not isinstance(max_symbols, bool) and max_symbols >= 1
    )
    valid_max_concurrent = (
        isinstance(max_concurrent, int) and not isinstance(max_concurrent, bool) and max_concurrent >= 1
    )

    if not valid_max_symbols:
        reasons.append(REASON_WATCHLIST_V2_MAX_SYMBOLS_INVALID)
    if not valid_max_concurrent:
        reasons.append(REASON_WATCHLIST_V2_CONCURRENT_LIMIT_INVALID)

    if not valid_max_symbols or not valid_max_concurrent:
        return None, reasons

    if max_symbols > MULTI_SYMBOL_HARD_CEILING:
        reasons.append(REASON_WATCHLIST_V2_HARD_CEILING_EXCEEDED)
        return None, reasons

    return min(max_symbols, max_concurrent, MULTI_SYMBOL_HARD_CEILING), reasons


def _selected_symbols_for_promotion(
    watchlist: Mapping[str, Any], config: WatchlistPromotionConfig
) -> list[str]:
    """
    Return the ranked symbols eligible for promotion evaluation.

    v1: only the top-ranked symbol (symbols[0]), if any.
    v2: symbols[:effective_limit], where effective_limit = min(
        max_symbols_to_trade, max_concurrent_positions, MULTI_SYMBOL_HARD_CEILING).
        Returns [] if the watchlist's limits are invalid or exceed
        MULTI_SYMBOL_HARD_CEILING (fail closed — never falls back to symbols[0]).
    """
    symbols: list[str] = list(watchlist.get("symbols") or [])
    if not symbols:
        return []
    if not _is_watchlist_v2(watchlist):
        return [symbols[0]]
    effective_limit, _ = _v2_effective_symbol_limit(watchlist)
    if effective_limit is None:
        return []
    return symbols[:effective_limit]


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
    Evaluate a watchlist-v1 (single-symbol) or watchlist-v2 (multi-symbol)
    artifact against all promotion gates.

    Returns a PromotionDecision; does not modify the input artifacts.

    risk_simulation_result: if provided, its "passed" bool overrides config.risk_simulation_passed.
    premarket_revalidation_result: if provided, its "passed" bool determines whether
      premarket revalidation is still required (not passed → required=True). For
      watchlist-v2, if premarket_revalidation_result["symbol_results"] is a dict,
      each selected symbol's own "passed" entry is honored instead of the overall
      "passed" flag (fail closed if any selected symbol is missing or not passed).

    Fail-closed: any gate failure → approved_for_autonomous_paper=False.
    approved_for_live is always False.
    v1: only the top-ranked symbol (symbols[0]) is considered for approval.
    v2: symbols[:effective_limit] are considered (see _selected_symbols_for_promotion).
    """
    cfg = config or WatchlistPromotionConfig()
    failure_reasons: list[str] = []
    is_v2 = _is_watchlist_v2(watchlist)

    # Resolve effective risk-simulation gate state from the result dict if provided,
    # else from config.
    effective_risk_passed = (
        bool(risk_simulation_result.get("passed", False))
        if risk_simulation_result is not None
        else cfg.risk_simulation_passed
    )

    # Gate 1: schema version must be watchlist-v1 or watchlist-v2
    if _watchlist_schema_version(watchlist) not in (SCHEMA_VERSION_WATCHLIST, SCHEMA_VERSION_WATCHLIST_V2):
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

    # Gate 4b (v2 only): max_symbols_to_trade / max_concurrent_positions must be
    # valid positive integers, and max_symbols_to_trade must not exceed
    # MULTI_SYMBOL_HARD_CEILING. Invalid/exceeded limits fail closed (zero
    # selected symbols), never silently clamped-and-approved.
    if is_v2:
        _, v2_limit_reasons = _v2_effective_symbol_limit(watchlist)
        failure_reasons.extend(v2_limit_reasons)

    selected_symbols = _selected_symbols_for_promotion(watchlist, cfg)

    # Resolve effective premarket-revalidation gate state.
    # v2: when premarket_revalidation_result["symbol_results"] is a dict, honor
    # each selected symbol's own "passed" entry (fail closed if any selected
    # symbol is missing or not passed) instead of the overall "passed" flag.
    if (
        is_v2
        and premarket_revalidation_result is not None
        and isinstance(premarket_revalidation_result.get("symbol_results"), dict)
        and selected_symbols
    ):
        symbol_results = premarket_revalidation_result["symbol_results"]
        effective_premarket_required = not all(
            bool((symbol_results.get(sym) or {}).get("passed", False))
            for sym in selected_symbols
        )
    elif premarket_revalidation_result is not None:
        effective_premarket_required = not bool(premarket_revalidation_result.get("passed", False))
    else:
        effective_premarket_required = cfg.premarket_revalidation_required

    # Gates 5-6: strategy-fit artifact(s) for the selected symbol(s) — presence,
    # identity binding (symbol/strategy_id must match the watchlist's assignment;
    # a mismatched or forged identity fails closed rather than silently adopting
    # a strategy/symbol the watchlist never assigned), and paper recommendation.
    validated_assignments: dict[str, str] = {}

    if not is_v2:
        # v1: only the top-ranked symbol (symbols[0]) is considered.
        top_symbol: Optional[str] = symbols[0] if symbols else None
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
                validated_strategy_id = fit_artifact.get("strategy_id") or assigned_strategy_id
                if validated_strategy_id:
                    validated_assignments[top_symbol] = validated_strategy_id
    else:
        # v2: every selected symbol must have an explicit strategy_assignments
        # entry AND a matching strategy-fit artifact AND pass the paper
        # recommendation gate. Fail closed on the first violation per symbol —
        # no partial approval of an otherwise-failing symbol.
        for sym in selected_symbols:
            assigned_strategy_id = (watchlist.get("strategy_assignments") or {}).get(sym)
            fit_artifact = strategy_fit_artifacts.get(sym)
            if assigned_strategy_id is None:
                failure_reasons.append(REASON_WATCHLIST_V2_SYMBOL_ASSIGNMENT_MISSING)
            elif fit_artifact is None:
                failure_reasons.append(REASON_STRATEGY_FIT_MISSING)
            elif fit_artifact.get("symbol") not in (None, sym):
                failure_reasons.append(REASON_STRATEGY_FIT_SYMBOL_MISMATCH)
            elif fit_artifact.get("strategy_id") != assigned_strategy_id:
                failure_reasons.append(REASON_STRATEGY_FIT_STRATEGY_MISMATCH)
            elif not fit_artifact.get("recommended_for_paper"):
                failure_reasons.append(REASON_STRATEGY_FIT_NOT_RECOMMENDED_FOR_PAPER)
            else:
                validated_assignments[sym] = fit_artifact.get("strategy_id") or assigned_strategy_id

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

    # Approved symbols: the selected symbol(s), only if all gates pass.
    # v1 selected_symbols is [symbols[0]] or []; v2 is symbols[:effective_limit] or [].
    if passed:
        approved_symbols = list(selected_symbols)
        assignments: dict[str, str] = dict(validated_assignments)
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
    v1: max_symbols_to_trade and max_concurrent_positions are forced to 1.
    v2: max_symbols_to_trade and max_concurrent_positions carry through
        unchanged from the input (already validated by
        evaluate_watchlist_promotion's gate 4b).
    symbols and strategy_assignments reflect only approved symbols.
    """
    updated = dict(watchlist)

    # Hard invariants
    updated["approved_for_autonomous_paper"] = decision.approved_for_autonomous_paper
    updated["approved_for_live"] = False
    if not _is_watchlist_v2(watchlist):
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
