"""
WATCHLIST-PREMARKET-01 — Premarket revalidation for watchlist candidates.

Evaluates a watchlist-v1 artifact and per-symbol inputs against premarket
readiness gates. All inputs are artifact/dict based — no broker or API calls.

Checks (evaluated in order for the top symbol):
1.  watchlist_schema_valid     — schema_version == "watchlist-v1"
2.  mode_paper                 — mode must be "paper"
3.  live_lock                  — approved_for_live must not be True
4.  symbol_input_present       — symbol_inputs[symbol] must exist
5.  symbol_not_suppressed      — suppressed != True
6.  data_quality_passed        — data_quality_passed == True
7.  liquidity_passed           — liquidity_passed == True
8.  regime_compatible          — regime_label still compatible with assigned strategy
9.  bar_freshness              — latest_bar_ts within max_bar_age_minutes of reference_utc
10. spread_limit               — spread_estimate_bps <= max_spread_bps (if present)
11. slippage_limit             — slippage_estimate_bps <= max_slippage_bps (if present)
12. rvol_threshold             — relative_volume >= min_relative_volume (if present)
13. price_threshold            — latest_close >= min_price (if present)

Fail-closed rules:
- Missing symbol_input → fail.
- Missing latest_bar_ts → fail (bar freshness cannot be verified).
- Unknown strategy in compatibility map → fail regime check.
- approved_for_live=True in input is forced False; adds premarket_live_approval_forbidden.
- Any gate failure → passed=False.
- Spread/slippage/rvol/price checks only fire when the field is present in symbol_input.
- Does NOT submit orders, call broker endpoints, or mutate production DB.
- No network/DB imports. No subprocess.
- JSON artifact only.

Hard invariants:
- approved_for_live is ALWAYS False in output
- No broker/OMS/execution imports
- No network imports (requests, urllib, http.client, aiohttp, psycopg, sqlalchemy)
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

SCHEMA_VERSION = "premarket-revalidation-v1"
_WATCHLIST_SCHEMA = "watchlist-v1"

# Inline strategy-regime compatibility (mirrors backtest_queue._COMPATIBILITY).
# Duplicated here intentionally for self-containment — no cross-module dependency.
_STRATEGY_REGIME_COMPATIBILITY: dict[str, frozenset[str]] = {
    "intraday_scalper": frozenset({"trending_up", "trending_down", "high_volatility"}),
    "volatility_breakout": frozenset(
        {"trending_up", "trending_down", "high_volatility", "low_volatility", "gapping"}
    ),
    "mean_reversion": frozenset({"range_bound", "low_volatility"}),
    "swing_momentum": frozenset({"trending_up", "trending_down"}),
    "vwap_mean_reversion": frozenset({"range_bound", "low_volatility"}),
    "opening_range_fade": frozenset({"high_volatility", "gapping"}),
}

# Failure reason constants — stable strings used in failure_reasons lists.
REASON_PREMARKET_WATCHLIST_SCHEMA_INVALID = "premarket_watchlist_schema_invalid"
REASON_PREMARKET_MODE_NOT_PAPER = "premarket_mode_not_paper"
REASON_PREMARKET_LIVE_APPROVAL_FORBIDDEN = "premarket_live_approval_forbidden"
REASON_PREMARKET_SYMBOL_INPUT_MISSING = "premarket_symbol_input_missing"
REASON_PREMARKET_SYMBOL_SUPPRESSED = "premarket_symbol_suppressed"
REASON_PREMARKET_DATA_QUALITY_FAILED = "premarket_data_quality_failed"
REASON_PREMARKET_LIQUIDITY_FAILED = "premarket_liquidity_failed"
REASON_PREMARKET_REGIME_INCOMPATIBLE = "premarket_regime_incompatible"
REASON_PREMARKET_BAR_STALE = "premarket_bar_stale"
REASON_PREMARKET_SPREAD_TOO_WIDE = "premarket_spread_too_wide"
REASON_PREMARKET_SLIPPAGE_TOO_HIGH = "premarket_slippage_too_high"
REASON_PREMARKET_RVOL_TOO_LOW = "premarket_rvol_too_low"
REASON_PREMARKET_PRICE_TOO_LOW = "premarket_price_too_low"


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class PremarketRevalidationConfig:
    max_bar_age_minutes: float = 20.0
    max_spread_bps: float = 20.0
    max_slippage_bps: float = 15.0
    min_relative_volume: float = 0.5
    min_price: float = 5.0
    approved_for_live: bool = False

    def __post_init__(self) -> None:
        # Hard invariant: live approval is never permitted through config.
        object.__setattr__(self, "approved_for_live", False)


# ---------------------------------------------------------------------------
# Result
# ---------------------------------------------------------------------------

@dataclass
class PremarketRevalidationResult:
    """Result of evaluating a watchlist against premarket revalidation gates."""
    passed: bool
    failure_reasons: list[str] = field(default_factory=list)
    top_symbol: Optional[str] = None
    symbol_results: dict[str, dict[str, Any]] = field(default_factory=dict)
    notes: str = ""

    def to_dict(self) -> dict[str, Any]:
        return {
            "schema_version": SCHEMA_VERSION,
            "passed": self.passed,
            "failure_reasons": list(self.failure_reasons),
            "top_symbol": self.top_symbol,
            "symbol_results": dict(self.symbol_results),
            "notes": self.notes,
        }


# ---------------------------------------------------------------------------
# Pure helpers
# ---------------------------------------------------------------------------

def _deduplicate_reasons(reasons: list[str]) -> list[str]:
    seen: set[str] = set()
    result: list[str] = []
    for r in reasons:
        if r not in seen:
            seen.add(r)
            result.append(r)
    return result


def _parse_bar_ts(ts_raw: Any) -> Optional[datetime]:
    """Parse ISO-8601 bar timestamp to UTC-aware datetime. Returns None on failure."""
    if not isinstance(ts_raw, str):
        return None
    try:
        dt = datetime.fromisoformat(ts_raw.replace("Z", "+00:00"))
        if dt.tzinfo is None:
            dt = dt.replace(tzinfo=timezone.utc)
        return dt
    except (ValueError, TypeError):
        return None


def _regime_compatible(strategy_id: str, regime_label: str) -> bool:
    """Return True iff strategy_id is known and regime_label is in its compatible set."""
    compatible = _STRATEGY_REGIME_COMPATIBILITY.get(strategy_id)
    if compatible is None:
        return False
    return regime_label in compatible


# ---------------------------------------------------------------------------
# Gate evaluator
# ---------------------------------------------------------------------------

def evaluate_premarket_watchlist(
    watchlist: dict[str, Any],
    symbol_inputs: dict[str, dict[str, Any]],
    config: Optional[PremarketRevalidationConfig] = None,
    reference_utc: Optional[datetime] = None,
) -> PremarketRevalidationResult:
    """
    Evaluate a watchlist-v1 artifact and per-symbol inputs against premarket gates.

    symbol_inputs: dict keyed by symbol with per-symbol market state fields.
    reference_utc: fixed reference time for bar freshness (defaults to datetime.now(utc)).
    Fail-closed: any missing required input or gate failure → passed=False.
    approved_for_live is forced False on any output.
    Only the top-ranked symbol (symbols[0]) is evaluated in v1.
    """
    cfg = config or PremarketRevalidationConfig()
    ref_time = reference_utc or datetime.now(tz=timezone.utc)
    failure_reasons: list[str] = []

    # Check 1: watchlist schema version
    if watchlist.get("schema_version") != _WATCHLIST_SCHEMA:
        failure_reasons.append(REASON_PREMARKET_WATCHLIST_SCHEMA_INVALID)

    # Check 2: mode must be paper
    if watchlist.get("mode") != "paper":
        failure_reasons.append(REASON_PREMARKET_MODE_NOT_PAPER)

    # Check 3: approved_for_live must not be True
    if watchlist.get("approved_for_live") is True:
        failure_reasons.append(REASON_PREMARKET_LIVE_APPROVAL_FORBIDDEN)

    symbols: list[str] = watchlist.get("symbols") or []
    top_symbol: Optional[str] = symbols[0] if symbols else None
    symbol_results: dict[str, dict[str, Any]] = {}

    if top_symbol is not None:
        sym_input = symbol_inputs.get(top_symbol)
        sym_reasons: list[str] = []

        if sym_input is None:
            sym_reasons.append(REASON_PREMARKET_SYMBOL_INPUT_MISSING)
        else:
            # Check 5: symbol not suppressed
            if sym_input.get("suppressed") is True:
                sym_reasons.append(REASON_PREMARKET_SYMBOL_SUPPRESSED)

            # Check 6: data_quality_passed
            if sym_input.get("data_quality_passed") is not True:
                sym_reasons.append(REASON_PREMARKET_DATA_QUALITY_FAILED)

            # Check 7: liquidity_passed
            if sym_input.get("liquidity_passed") is not True:
                sym_reasons.append(REASON_PREMARKET_LIQUIDITY_FAILED)

            # Check 8: regime still compatible with assigned strategy
            regime_label = sym_input.get("regime_label")
            strategy_assignments: dict[str, str] = watchlist.get("strategy_assignments") or {}
            strategy_id = strategy_assignments.get(top_symbol)
            if strategy_id is None or regime_label is None:
                sym_reasons.append(REASON_PREMARKET_REGIME_INCOMPATIBLE)
            elif not _regime_compatible(strategy_id, regime_label):
                sym_reasons.append(REASON_PREMARKET_REGIME_INCOMPATIBLE)

            # Check 9: bar freshness — absent ts is fail-closed
            bar_ts_raw = sym_input.get("latest_bar_ts")
            if bar_ts_raw is None:
                sym_reasons.append(REASON_PREMARKET_BAR_STALE)
            else:
                bar_dt = _parse_bar_ts(bar_ts_raw)
                if bar_dt is None:
                    sym_reasons.append(REASON_PREMARKET_BAR_STALE)
                else:
                    age_minutes = (ref_time - bar_dt).total_seconds() / 60.0
                    if age_minutes > cfg.max_bar_age_minutes:
                        sym_reasons.append(REASON_PREMARKET_BAR_STALE)

            # Checks 10-13: only fire when field is present
            spread = sym_input.get("spread_estimate_bps")
            if spread is not None and float(spread) > cfg.max_spread_bps:
                sym_reasons.append(REASON_PREMARKET_SPREAD_TOO_WIDE)

            slippage = sym_input.get("slippage_estimate_bps")
            if slippage is not None and float(slippage) > cfg.max_slippage_bps:
                sym_reasons.append(REASON_PREMARKET_SLIPPAGE_TOO_HIGH)

            rvol = sym_input.get("relative_volume")
            if rvol is not None and float(rvol) < cfg.min_relative_volume:
                sym_reasons.append(REASON_PREMARKET_RVOL_TOO_LOW)

            price = sym_input.get("latest_close")
            if price is not None and float(price) < cfg.min_price:
                sym_reasons.append(REASON_PREMARKET_PRICE_TOO_LOW)

        sym_passed = len(sym_reasons) == 0
        symbol_results[top_symbol] = {
            "passed": sym_passed,
            "failure_reasons": list(sym_reasons),
        }
        failure_reasons.extend(sym_reasons)

    failure_reasons = _deduplicate_reasons(failure_reasons)
    passed = len(failure_reasons) == 0

    return PremarketRevalidationResult(
        passed=passed,
        failure_reasons=failure_reasons,
        top_symbol=top_symbol,
        symbol_results=symbol_results,
        notes=(
            "premarket_passed: all_checks_passed"
            if passed
            else "premarket_failed: " + "; ".join(failure_reasons)
        ),
    )


# ---------------------------------------------------------------------------
# Artifact updater
# ---------------------------------------------------------------------------

def apply_premarket_revalidation_to_watchlist(
    watchlist: dict[str, Any],
    result: PremarketRevalidationResult,
) -> dict[str, Any]:
    """
    Return a new watchlist artifact dict with premarket revalidation result applied.

    Does not modify the input watchlist dict.
    approved_for_live is always forced to False.
    """
    updated = dict(watchlist)
    updated["approved_for_live"] = False
    updated["premarket_revalidation"] = result.to_dict()
    return updated


# ---------------------------------------------------------------------------
# Writer
# ---------------------------------------------------------------------------

def write_premarket_revalidation_artifact(
    result: PremarketRevalidationResult, path: str
) -> Path:
    """Write a premarket revalidation result as JSON. Parent dirs created automatically."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result.to_dict(), indent=2, default=str), encoding="utf-8")
    return out
