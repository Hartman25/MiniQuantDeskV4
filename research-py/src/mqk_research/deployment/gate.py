"""
TV-02: Deployability gate evaluator.

A candidate artifact passes this gate iff all four explicit checks pass.
Passing is NOT proof of edge, profitability, or live trust.  It only confirms
minimum tradability and sample adequacy.

All checks read from the metrics dict produced by _metrics_from_returns in
exp_distributed/strategies.py.  No additional metric computation is required.
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Optional

from mqk_research.contracts import (
    DEPLOYABILITY_GATE_CONTRACT_VERSION,
    DeployabilityCheck,
    DeployabilityGateResult,
)


@dataclass(frozen=True)
class DeployabilityGateConfig:
    """
    Thresholds for each deployability check.

    Defaults represent the minimum bar for a signal_pack to be considered
    for downstream consideration.  Override only with explicit justification.

    min_trade_count:
        Minimum number of trade events in the backtest period.
        Below this the strategy has too little activity to be meaningful.

    min_trading_days:
        Minimum number of calendar trading days in the sample.
        Below this the backtest window is too short to be informative.

    max_daily_turnover:
        Maximum allowed total turnover divided by trading_days.
        Above this the strategy trades excessively (likely unrealisable live).

    min_active_day_fraction:
        Minimum fraction of trading days where the strategy held any position.
        Below this the strategy is effectively idle most of the time.
    """
    min_trade_count: int = 30
    min_trading_days: int = 60
    max_daily_turnover: float = 5.0
    min_active_day_fraction: float = 0.05


def evaluate_deployability_gate(
    artifact_id: str,
    metrics: Dict[str, Any],
    config: Optional[DeployabilityGateConfig] = None,
    *,
    evaluated_at_utc: Optional[str] = None,
) -> DeployabilityGateResult:
    """
    Evaluate a candidate artifact against the deployability gate.

    Parameters
    ----------
    artifact_id:
        Canonical artifact ID (TV-01).  Must be a non-empty hex string.
    metrics:
        Dict produced by _metrics_from_returns (exp_distributed/strategies.py).
        Required keys: trading_days, active_days, turnover, trade_event_count.
        Missing keys raise KeyError — callers must supply complete metrics.
    config:
        Gate thresholds.  Uses DeployabilityGateConfig defaults if None.
    evaluated_at_utc:
        ISO-8601 UTC string.  Defaults to now if None.  Inject for determinism.

    Returns
    -------
    DeployabilityGateResult with all four checks populated.
    passed=True iff all four checks pass.

    Raises
    ------
    KeyError: required metric key is absent from the metrics dict.
    ValueError: artifact_id is empty.
    """
    if not artifact_id:
        raise ValueError("artifact_id must be a non-empty string")

    cfg = config if config is not None else DeployabilityGateConfig()
    ts = evaluated_at_utc or datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

    trade_count = int(metrics["trade_event_count"])
    trading_days = int(metrics["trading_days"])
    turnover = float(metrics["turnover"])
    active_days = int(metrics["active_days"])

    daily_turnover = turnover / max(trading_days, 1)
    active_fraction = active_days / max(trading_days, 1)

    checks = [
        DeployabilityCheck(
            name="min_trade_count",
            passed=trade_count >= cfg.min_trade_count,
            value=float(trade_count),
            threshold=float(cfg.min_trade_count),
            note=(
                "Minimum number of trade events required.  "
                "Below this the strategy has insufficient activity to evaluate."
            ),
        ),
        DeployabilityCheck(
            name="min_trading_days",
            passed=trading_days >= cfg.min_trading_days,
            value=float(trading_days),
            threshold=float(cfg.min_trading_days),
            note=(
                "Minimum backtest window in trading days required.  "
                "Below this the sample is too short to be informative."
            ),
        ),
        DeployabilityCheck(
            name="max_daily_turnover",
            passed=daily_turnover <= cfg.max_daily_turnover,
            value=daily_turnover,
            threshold=cfg.max_daily_turnover,
            note=(
                "Maximum allowed average daily turnover (total_turnover / trading_days).  "
                "Above this the strategy trades excessively and may be unrealisable live."
            ),
        ),
        DeployabilityCheck(
            name="min_active_day_fraction",
            passed=active_fraction >= cfg.min_active_day_fraction,
            value=active_fraction,
            threshold=cfg.min_active_day_fraction,
            note=(
                "Minimum fraction of trading days with any held position.  "
                "Below this the strategy is effectively idle most of the time."
            ),
        ),
    ]

    all_passed = all(c.passed for c in checks)
    failed_names = [c.name for c in checks if not c.passed]

    if all_passed:
        overall_reason = (
            "All four deployability checks passed: trade count, sample window, "
            "daily turnover, and active day fraction are within bounds.  "
            "This artifact meets minimum tradability and sample adequacy criteria."
        )
    else:
        overall_reason = (
            f"Gate FAILED.  Failed checks: {', '.join(failed_names)}.  "
            "This artifact does not meet minimum tradability or sample adequacy criteria."
        )

    return DeployabilityGateResult(
        schema_version=DEPLOYABILITY_GATE_CONTRACT_VERSION,
        artifact_id=artifact_id,
        passed=all_passed,
        checks=checks,
        overall_reason=overall_reason,
        evaluated_at_utc=ts,
    )


def write_deployability_gate(artifact_dir: Path, result: DeployabilityGateResult) -> Path:
    """
    Persist the gate result to <artifact_dir>/deployability_gate.json.

    Standalone writer — not coupled to promote_signal_pack.
    Callers supply the artifact_dir (the promoted artifact directory).

    Returns the path written.

    Raises:
        ValueError: artifact_dir does not exist.
    """
    artifact_dir = Path(artifact_dir)
    if not artifact_dir.exists():
        raise ValueError(f"artifact_dir does not exist: {artifact_dir}")

    out = artifact_dir / "deployability_gate.json"
    out.write_text(result.to_json(), encoding="utf-8")
    return out
