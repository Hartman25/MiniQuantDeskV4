"""
RISK-SIM-01 — Artifact-level risk simulation for watchlist candidates.

Evaluates a watchlist-v1 artifact and strategy-fit-v1 artifacts against
risk simulation gates. Returns a RiskSimulationResult.

Risk checks (evaluated in order):
1. live_lock              — approved_for_live must be False in watchlist
2. has_candidates         — watchlist must have at least one symbol
3. strategy_fit_present   — strategy-fit artifact must exist for top symbol
4. concentration          — max_symbols_to_trade == 1 and max_concurrent_positions == 1
5. max_position_notional  — notional limit <= max_position_notional_usd
6. max_daily_loss         — max_drawdown_bps (proxy) <= max_daily_loss_bps
7. drawdown_stress        — max_drawdown_bps * slippage_stress_multiplier <= max_stressed_drawdown_bps
8. regime_consistency     — strategy_id compatible with candidate regime_label
9. cost_adjusted_edge     — net_expectancy_after_cost_bps >= min_net_expectancy_after_cost_bps

Fail-closed rules:
- No candidates → fail.
- No strategy-fit artifact → fail.
- approved_for_live=True in input is forced False; adds risk_live_approval_forbidden.
- Missing required metric in strategy-fit → fail.
- Unknown strategy or missing regime_label → fail regime check.
- All required metric reads fail closed on None.
- Does NOT submit orders, call broker endpoints, or mutate production DB.
- No network/DB imports. No subprocess.
- JSON artifact only.

Hard invariants:
- approved_for_live is ALWAYS False in output
- recommended_for_live is ALWAYS False (no such field; live_lock check enforces this)
- No broker/OMS/execution imports
- No network imports (requests, urllib, http.client, aiohttp, psycopg, sqlalchemy)
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional

SCHEMA_VERSION = "risk-simulation-v1"

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
REASON_RISK_NO_CANDIDATES = "risk_no_candidates"
REASON_RISK_STRATEGY_FIT_MISSING = "risk_strategy_fit_missing"
REASON_RISK_NOTIONAL_LIMIT_FAILED = "risk_notional_limit_failed"
REASON_RISK_DAILY_LOSS_FAILED = "risk_daily_loss_failed"
REASON_RISK_DRAWDOWN_STRESS_FAILED = "risk_drawdown_stress_failed"
REASON_RISK_CONCENTRATION_FAILED = "risk_concentration_failed"
REASON_RISK_REGIME_MISMATCH = "risk_regime_mismatch"
REASON_RISK_COST_ADJUSTED_EDGE_FAILED = "risk_cost_adjusted_edge_failed"
REASON_RISK_LIVE_APPROVAL_FORBIDDEN = "risk_live_approval_forbidden"
REASON_RISK_MISSING_METRIC = "risk_missing_required_metric"


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class RiskSimulationConfig:
    max_position_notional_usd: float = 5000.0
    max_daily_loss_bps: float = 200.0
    max_stressed_drawdown_bps: float = 1000.0
    slippage_stress_multiplier: float = 2.0
    min_net_expectancy_after_cost_bps: float = 5.0
    max_symbols_to_trade: int = 1
    max_concurrent_positions: int = 1
    approved_for_live: bool = False

    def __post_init__(self) -> None:
        # Hard invariant: live approval is never permitted through config.
        object.__setattr__(self, "approved_for_live", False)


# ---------------------------------------------------------------------------
# Result
# ---------------------------------------------------------------------------

@dataclass
class RiskSimulationResult:
    """Result of evaluating a watchlist against all risk simulation gates."""
    passed: bool
    failure_reasons: list[str] = field(default_factory=list)
    check_live_lock: bool = False
    check_has_candidates: bool = False
    check_strategy_fit_present: bool = False
    check_concentration: bool = False
    check_max_position_notional: bool = False
    check_max_daily_loss: bool = False
    check_drawdown_stress: bool = False
    check_regime_consistency: bool = False
    check_cost_adjusted_edge: bool = False
    top_symbol: Optional[str] = None
    notes: str = ""

    def to_dict(self) -> dict[str, Any]:
        return {
            "schema_version": SCHEMA_VERSION,
            "passed": self.passed,
            "failure_reasons": list(self.failure_reasons),
            "checks": {
                "live_lock": self.check_live_lock,
                "has_candidates": self.check_has_candidates,
                "strategy_fit_present": self.check_strategy_fit_present,
                "concentration": self.check_concentration,
                "max_position_notional": self.check_max_position_notional,
                "max_daily_loss": self.check_max_daily_loss,
                "drawdown_stress": self.check_drawdown_stress,
                "regime_consistency": self.check_regime_consistency,
                "cost_adjusted_edge": self.check_cost_adjusted_edge,
            },
            "top_symbol": self.top_symbol,
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


def _regime_compatible(strategy_id: str, regime_label: str) -> bool:
    """Return True iff strategy_id is known and regime_label is in its compatible set."""
    compatible = _STRATEGY_REGIME_COMPATIBILITY.get(strategy_id)
    if compatible is None:
        return False
    return regime_label in compatible


# ---------------------------------------------------------------------------
# Gate evaluator
# ---------------------------------------------------------------------------

def evaluate_watchlist_risk(
    watchlist: dict[str, Any],
    strategy_fit_artifacts: dict[str, dict[str, Any]],
    config: Optional[RiskSimulationConfig] = None,
) -> RiskSimulationResult:
    """
    Evaluate a watchlist-v1 artifact against all risk simulation gates.

    Returns a RiskSimulationResult; does not modify input artifacts.
    Fail-closed: missing required data → result.passed = False.
    approved_for_live is forced False on any output.
    Only the top-ranked symbol (symbols[0]) is evaluated in v1.
    """
    cfg = config or RiskSimulationConfig()
    failure_reasons: list[str] = []
    result = RiskSimulationResult(passed=False)

    # Check 1: live_lock
    if watchlist.get("approved_for_live") is True:
        failure_reasons.append(REASON_RISK_LIVE_APPROVAL_FORBIDDEN)
    else:
        result.check_live_lock = True

    # Check 2: has candidates
    symbols: list[str] = watchlist.get("symbols") or []
    if not symbols:
        failure_reasons.append(REASON_RISK_NO_CANDIDATES)
        result.failure_reasons = _deduplicate_reasons(failure_reasons)
        result.notes = "blocked: " + "; ".join(result.failure_reasons)
        return result
    result.check_has_candidates = True

    top_symbol: str = symbols[0]
    result.top_symbol = top_symbol

    # Check 3: strategy-fit artifact present
    fit = strategy_fit_artifacts.get(top_symbol)
    if fit is None:
        failure_reasons.append(REASON_RISK_STRATEGY_FIT_MISSING)
        result.failure_reasons = _deduplicate_reasons(failure_reasons)
        result.notes = "blocked: " + "; ".join(result.failure_reasons)
        return result
    result.check_strategy_fit_present = True

    # Check 4: concentration (v1: single-symbol, single-position only)
    max_symbols = watchlist.get("max_symbols_to_trade", 0)
    max_concurrent = watchlist.get("max_concurrent_positions", 0)
    if (max_symbols == cfg.max_symbols_to_trade
            and max_concurrent == cfg.max_concurrent_positions):
        result.check_concentration = True
    else:
        failure_reasons.append(REASON_RISK_CONCENTRATION_FAILED)

    # Check 5: max_position_notional
    notional_limits: dict[str, Any] = watchlist.get("notional_limits") or {}
    notional = notional_limits.get(top_symbol)
    if notional is None:
        failure_reasons.append(REASON_RISK_NOTIONAL_LIMIT_FAILED)
    elif float(notional) <= cfg.max_position_notional_usd:
        result.check_max_position_notional = True
    else:
        failure_reasons.append(REASON_RISK_NOTIONAL_LIMIT_FAILED)

    # Checks 6-9 require strategy-fit metrics; fail closed on missing
    max_drawdown_bps = fit.get("max_drawdown_bps")
    net_expectancy = fit.get("net_expectancy_after_cost_bps")

    if max_drawdown_bps is None or net_expectancy is None:
        failure_reasons.append(REASON_RISK_MISSING_METRIC)
        result.failure_reasons = _deduplicate_reasons(failure_reasons)
        result.passed = False
        result.notes = "blocked: " + "; ".join(result.failure_reasons)
        return result

    # Check 6: max_daily_loss (max_drawdown_bps used as daily loss proxy)
    if float(max_drawdown_bps) <= cfg.max_daily_loss_bps:
        result.check_max_daily_loss = True
    else:
        failure_reasons.append(REASON_RISK_DAILY_LOSS_FAILED)

    # Check 7: drawdown_stress
    stressed = float(max_drawdown_bps) * cfg.slippage_stress_multiplier
    if stressed <= cfg.max_stressed_drawdown_bps:
        result.check_drawdown_stress = True
    else:
        failure_reasons.append(REASON_RISK_DRAWDOWN_STRESS_FAILED)

    # Check 8: regime_consistency
    # Regime label comes from the watchlist's ranked_candidates for the top symbol.
    strategy_id: Optional[str] = fit.get("strategy_id")
    ranked_candidates: list[dict[str, Any]] = watchlist.get("ranked_candidates") or []
    top_candidate = next(
        (c for c in ranked_candidates if c.get("symbol") == top_symbol), None
    )
    regime_label: Optional[str] = top_candidate.get("regime_label") if top_candidate else None

    if strategy_id is None or regime_label is None:
        failure_reasons.append(REASON_RISK_REGIME_MISMATCH)
    elif _regime_compatible(strategy_id, regime_label):
        result.check_regime_consistency = True
    else:
        failure_reasons.append(REASON_RISK_REGIME_MISMATCH)

    # Check 9: cost_adjusted_edge
    if float(net_expectancy) >= cfg.min_net_expectancy_after_cost_bps:
        result.check_cost_adjusted_edge = True
    else:
        failure_reasons.append(REASON_RISK_COST_ADJUSTED_EDGE_FAILED)

    result.failure_reasons = _deduplicate_reasons(failure_reasons)
    result.passed = len(result.failure_reasons) == 0
    result.notes = (
        "risk_simulation_passed: all_checks_passed"
        if result.passed
        else "risk_simulation_failed: " + "; ".join(result.failure_reasons)
    )
    return result


# ---------------------------------------------------------------------------
# Artifact updater
# ---------------------------------------------------------------------------

def apply_risk_simulation_to_watchlist(
    watchlist: dict[str, Any],
    result: RiskSimulationResult,
) -> dict[str, Any]:
    """
    Return a new watchlist artifact dict with risk simulation result applied.

    Does not modify the input watchlist dict.
    approved_for_live is always forced to False.
    """
    updated = dict(watchlist)
    updated["approved_for_live"] = False
    updated["risk_simulation"] = result.to_dict()
    return updated


# ---------------------------------------------------------------------------
# Writer
# ---------------------------------------------------------------------------

def write_risk_simulation_artifact(result: RiskSimulationResult, path: str) -> Path:
    """Write a risk simulation result as JSON. Parent dirs created automatically."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result.to_dict(), indent=2, default=str), encoding="utf-8")
    return out
