"""Census-02 borrow-truth capability. The repo holds NO point-in-time borrow/locate/fee/recall data (Alpaca asset
`shortable`/`easy_to_borrow` are current-state snapshots only), so an individual-equity short can never be an executable
result. An ETF short is executable only under an operator-frozen, explicitly parameterised conservative assumption."""

from __future__ import annotations

import math

EVIDENCE_A = "A_INDIVIDUAL_EQUITY_SHORT_HYPOTHESIS_ONLY"
EVIDENCE_B = "B_EXECUTABLE_SHORT_WITH_POINT_IN_TIME_BORROW_TRUTH"
EVIDENCE_C = "C_LIQUID_ETF_SHORT_FROZEN_CONSERVATIVE_BORROW_ASSUMPTION"
EXECUTABLE_CLASSES = frozenset({EVIDENCE_B, EVIDENCE_C})

BORROW_TRUTH_UNAVAILABLE = "UNAVAILABLE_NO_POINT_IN_TIME_BORROW_DATA"
# Point-in-time borrow evidence (class B) needs a provenance-bound historical dataset that does not exist in the repo.
POINT_IN_TIME_BORROW_SUPPORTED = False

ASSUMPTION_KEYS = ("etf_short_scope", "annual_borrow_fee_bps", "availability", "recall", "short_rebate")
ASSUMPTION_FIXED = {"availability": "ALWAYS_AVAILABLE_FOR_SCOPE_ASSUMED", "recall": "NONE_ASSUMED",
                    "short_rebate": "ZERO"}


class BorrowRefusal(RuntimeError):
    pass


def validate_etf_assumption(a) -> dict:
    """Fail closed unless the frozen assumption is complete: explicit symbol list and a finite non-negative fee."""
    if not isinstance(a, dict) or sorted(a) != sorted(ASSUMPTION_KEYS):
        raise BorrowRefusal(f"borrow assumption must have exactly the keys {sorted(ASSUMPTION_KEYS)}")
    scope = a["etf_short_scope"]
    if not isinstance(scope, list) or not scope or scope != sorted(set(scope)) or not all(isinstance(s, str) and s for s in scope):
        raise BorrowRefusal("etf_short_scope must be a non-empty sorted list of unique symbols")
    fee = a["annual_borrow_fee_bps"]
    if isinstance(fee, bool) or not isinstance(fee, (int, float)) or not math.isfinite(fee) or fee < 0:
        raise BorrowRefusal("annual_borrow_fee_bps must be an explicit finite non-negative number (no default)")
    for k, v in ASSUMPTION_FIXED.items():
        if a[k] != v:
            raise BorrowRefusal(f"{k} must be {v!r}: the assumption is disclosed, never silently relaxed")
    return a


def classify_evidence(symbol: str, assumption: dict | None) -> str:
    """Evidence class of a short-bearing cell on `symbol`. No frozen assumption, or a symbol outside the frozen ETF scope,
    is hypothesis-only. Class B is unreachable until point-in-time borrow data exists."""
    if assumption is None:
        return EVIDENCE_A
    validate_etf_assumption(assumption)
    return EVIDENCE_C if symbol in assumption["etf_short_scope"] else EVIDENCE_A


def require_executable(evidence_class: str) -> None:
    if evidence_class not in EXECUTABLE_CLASSES:
        raise BorrowRefusal(f"{evidence_class}: no executable short evaluation without borrow truth or a frozen ETF assumption")
    if evidence_class == EVIDENCE_B and not POINT_IN_TIME_BORROW_SUPPORTED:
        raise BorrowRefusal("class B requires point-in-time borrow data, which is not available")
