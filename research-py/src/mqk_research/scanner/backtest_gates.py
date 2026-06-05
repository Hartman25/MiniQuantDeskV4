"""
BACKTEST-GATES-01 — MAIN scanner backtest gate evaluator.

Evaluates strategy-fit-v1 artifacts against promotion pass/fail gates.
Works for both blocked (dry-run) and real-metric artifacts.

Fail-closed rules:
- status == "blocked_no_backtest_interface" → all gates fail, recommended_for_paper=False.
- Any required metric null/missing → fail closed, missing_required_metric added.
- recommended_for_live is ALWAYS False.
- recommended_for_paper=True only if ALL required + additional gates pass.
- Existing failure_reasons are preserved and deduplicated; never removed.

Additional gates (sample_quality, parameter_stability, single_trade_dependency)
are tracked in failure_reasons only — no new passed_* schema fields added.

Hard invariants:
- recommended_for_live is ALWAYS False
- Does NOT invoke mqk-backtest or any external subprocess
- Does NOT submit orders or mutate DB
- Does NOT import broker adapters, OMS, execution orchestrator, or DB
- Does NOT import network libraries (requests, urllib, http.client, aiohttp,
  psycopg, sqlalchemy)
- JSON artifact only
- EXP penny scanner (exp-candidate-v1) is not affected by this module
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional

SCHEMA_VERSION = "strategy-fit-v1"

# Status string that identifies a blocked runner artifact.
STATUS_BLOCKED = "blocked_no_backtest_interface"

# Failure reason constants — stable strings used in failure_reasons lists.
REASON_BLOCKED_NO_INTERFACE = "blocked_no_backtest_interface"
REASON_MISSING_METRIC = "missing_required_metric"
REASON_MIN_BARS = "min_bars_failed"
REASON_MIN_TRADES = "min_trades_failed"
REASON_MAX_DRAWDOWN = "max_drawdown_failed"
REASON_PROFIT_FACTOR = "profit_factor_failed"
REASON_EXPECTANCY = "expectancy_failed"
REASON_COST_ADJUSTED_EDGE = "cost_adjusted_edge_failed"
REASON_OUT_OF_SAMPLE = "out_of_sample_failed"
REASON_SAMPLE_QUALITY = "sample_quality_failed"
REASON_PARAMETER_STABILITY = "parameter_stability_failed"
REASON_SINGLE_TRADE_DEPENDENCY = "single_trade_dependency_failed"


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class BacktestGateConfig:
    min_bars: int = 200
    min_trades: int = 30
    max_drawdown_bps: float = 800.0
    min_profit_factor: float = 1.3
    min_expectancy_bps: float = 0.0
    min_net_expectancy_after_cost_bps: float = 5.0
    min_validation_profit_factor: float = 1.1
    min_validation_trades: int = 10
    max_single_trade_profit_fraction: float = 0.30
    min_sample_quality: float = 0.5
    min_parameter_stability_score: float = 0.7
    recommended_for_live: bool = False

    def __post_init__(self) -> None:
        # Hard invariant: live recommendations are never permitted.
        object.__setattr__(self, "recommended_for_live", False)


# ---------------------------------------------------------------------------
# Gate result
# ---------------------------------------------------------------------------

@dataclass
class BacktestGateResult:
    """Result of evaluating a strategy-fit artifact against all gates."""
    passed_min_bars: bool = False
    passed_min_trades: bool = False
    passed_max_drawdown: bool = False
    passed_profit_factor: bool = False
    passed_expectancy: bool = False
    passed_cost_adjusted_edge: bool = False
    passed_out_of_sample_check: bool = False
    recommended_for_paper: bool = False
    recommended_for_live: bool = False
    failure_reasons: list[str] = field(default_factory=list)

    def all_required_passed(self) -> bool:
        return (
            self.passed_min_bars
            and self.passed_min_trades
            and self.passed_max_drawdown
            and self.passed_profit_factor
            and self.passed_expectancy
            and self.passed_cost_adjusted_edge
            and self.passed_out_of_sample_check
        )


# ---------------------------------------------------------------------------
# Pure helpers
# ---------------------------------------------------------------------------

def _is_blocked(artifact: dict[str, Any]) -> bool:
    return artifact.get("status") == STATUS_BLOCKED


def _merge_failure_reasons(existing: list[str], new_reasons: list[str]) -> list[str]:
    """Merge failure reasons: preserve order, deduplicate, never remove."""
    seen: set[str] = set(existing)
    result = list(existing)
    for r in new_reasons:
        if r not in seen:
            seen.add(r)
            result.append(r)
    return result


# ---------------------------------------------------------------------------
# Gate evaluator
# ---------------------------------------------------------------------------

def evaluate_strategy_fit(
    artifact: dict[str, Any],
    config: Optional[BacktestGateConfig] = None,
) -> BacktestGateResult:
    """
    Evaluate a strategy-fit-v1 artifact dict against all pass/fail gates.

    Returns a BacktestGateResult; does not modify the input artifact.

    Fail-closed: blocked status or any null required metric → all gates fail.
    Existing failure_reasons in the artifact are preserved and merged.
    recommended_for_live is always False.
    """
    cfg = config or BacktestGateConfig()
    result = BacktestGateResult(recommended_for_live=False)
    new_reasons: list[str] = []

    # --- Blocked path ---
    if _is_blocked(artifact):
        existing = list(artifact.get("failure_reasons") or [])
        if REASON_BLOCKED_NO_INTERFACE not in existing:
            new_reasons.append(REASON_BLOCKED_NO_INTERFACE)
        result.failure_reasons = _merge_failure_reasons(existing, new_reasons)
        result.recommended_for_paper = False
        result.recommended_for_live = False
        return result

    existing_reasons: list[str] = list(artifact.get("failure_reasons") or [])

    # --- Required metric presence check ---
    required_fields = [
        "bars_used", "trades", "profit_factor", "expectancy_bps",
        "max_drawdown_bps", "net_expectancy_after_cost_bps",
    ]
    for field_name in required_fields:
        if artifact.get(field_name) is None:
            new_reasons.append(REASON_MISSING_METRIC)
            result.failure_reasons = _merge_failure_reasons(existing_reasons, new_reasons)
            result.recommended_for_paper = False
            result.recommended_for_live = False
            return result

    bars_used: int = artifact["bars_used"]
    trades: int = artifact["trades"]
    profit_factor: float = artifact["profit_factor"]
    expectancy_bps: float = artifact["expectancy_bps"]
    max_drawdown_bps: float = artifact["max_drawdown_bps"]
    net_expectancy: float = artifact["net_expectancy_after_cost_bps"]

    # --- Required gates ---

    if bars_used >= cfg.min_bars:
        result.passed_min_bars = True
    else:
        new_reasons.append(REASON_MIN_BARS)

    if trades >= cfg.min_trades:
        result.passed_min_trades = True
    else:
        new_reasons.append(REASON_MIN_TRADES)

    if max_drawdown_bps <= cfg.max_drawdown_bps:
        result.passed_max_drawdown = True
    else:
        new_reasons.append(REASON_MAX_DRAWDOWN)

    if profit_factor >= cfg.min_profit_factor:
        result.passed_profit_factor = True
    else:
        new_reasons.append(REASON_PROFIT_FACTOR)

    if expectancy_bps > cfg.min_expectancy_bps:
        result.passed_expectancy = True
    else:
        new_reasons.append(REASON_EXPECTANCY)

    if net_expectancy >= cfg.min_net_expectancy_after_cost_bps:
        result.passed_cost_adjusted_edge = True
    else:
        new_reasons.append(REASON_COST_ADJUSTED_EDGE)

    # Out-of-sample: validation_profit_factor + validation_trades must both be present.
    validation_pf = artifact.get("validation_profit_factor")
    validation_trades = artifact.get("validation_trades")
    if validation_pf is not None and validation_trades is not None:
        if (validation_pf >= cfg.min_validation_profit_factor
                and validation_trades >= cfg.min_validation_trades):
            result.passed_out_of_sample_check = True
        else:
            new_reasons.append(REASON_OUT_OF_SAMPLE)
    else:
        # Absent validation metrics → fail closed
        new_reasons.append(REASON_OUT_OF_SAMPLE)

    # --- Additional gates (tracked in failure_reasons only; not separate passed_* fields) ---
    additional_pass = True

    sample_quality = artifact.get("sample_quality")
    if sample_quality is None or sample_quality < cfg.min_sample_quality:
        new_reasons.append(REASON_SAMPLE_QUALITY)
        additional_pass = False

    param_stability = artifact.get("parameter_stability_score")
    if param_stability is None or param_stability < cfg.min_parameter_stability_score:
        new_reasons.append(REASON_PARAMETER_STABILITY)
        additional_pass = False

    largest_fraction = artifact.get("largest_trade_profit_fraction")
    if largest_fraction is None:
        new_reasons.append(REASON_SINGLE_TRADE_DEPENDENCY)
        additional_pass = False
    elif largest_fraction > cfg.max_single_trade_profit_fraction:
        new_reasons.append(REASON_SINGLE_TRADE_DEPENDENCY)
        additional_pass = False

    result.failure_reasons = _merge_failure_reasons(existing_reasons, new_reasons)

    # recommended_for_paper: all required gates + all additional gates must pass.
    result.recommended_for_paper = result.all_required_passed() and additional_pass
    result.recommended_for_live = False

    return result


# ---------------------------------------------------------------------------
# Artifact updater
# ---------------------------------------------------------------------------

def apply_backtest_gates(
    artifact: dict[str, Any],
    config: Optional[BacktestGateConfig] = None,
) -> dict[str, Any]:
    """
    Return a new artifact dict with gate fields and recommended_for_paper updated.

    Does not modify the input artifact.
    recommended_for_live is always forced to False.
    """
    gate_result = evaluate_strategy_fit(artifact, config)
    updated = dict(artifact)
    updated["passed_min_bars"] = gate_result.passed_min_bars
    updated["passed_min_trades"] = gate_result.passed_min_trades
    updated["passed_max_drawdown"] = gate_result.passed_max_drawdown
    updated["passed_profit_factor"] = gate_result.passed_profit_factor
    updated["passed_expectancy"] = gate_result.passed_expectancy
    updated["passed_cost_adjusted_edge"] = gate_result.passed_cost_adjusted_edge
    updated["passed_out_of_sample_check"] = gate_result.passed_out_of_sample_check
    updated["recommended_for_paper"] = gate_result.recommended_for_paper
    updated["recommended_for_live"] = False  # hard invariant
    updated["failure_reasons"] = gate_result.failure_reasons
    return updated


# ---------------------------------------------------------------------------
# Artifact writer
# ---------------------------------------------------------------------------

def write_evaluated_strategy_fit_artifact(artifact: dict[str, Any], path: str) -> Path:
    """Write an evaluated strategy-fit-v1 artifact as JSON. Parent dirs created automatically."""
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(artifact, indent=2, default=str), encoding="utf-8")
    return out
