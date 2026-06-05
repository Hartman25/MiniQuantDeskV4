"""
SCANNER-LIQUIDITY-01 — MAIN scanner liquidity / execution-cost gate.

Validates whether a symbol is liquid and cheap enough to trade before it can
progress toward candidate scoring or backtesting.

Hard invariants:
- No imports from broker adapters, OMS tables, execution orchestrator, or DB
- No network imports (requests, urllib, http.client, aiohttp, psycopg, sqlalchemy)
- No orders are placed at any stage
- eligible_for_live is always False
- Rejection records are written via scanner-candidate-v1 schema
- EXP penny scanner (exp-candidate-v1) is not affected by this module
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Optional

# ---------------------------------------------------------------------------
# Stable rejection reason names
# ---------------------------------------------------------------------------

REASON_PRICE_BELOW_MIN = "price_below_min"
REASON_ADV_USD_BELOW_MIN = "adv_usd_below_min"
REASON_RELATIVE_VOLUME_BELOW_MIN = "relative_volume_below_min"
REASON_SPREAD_TOO_WIDE = "spread_too_wide"
REASON_ROUND_TRIP_COST_TOO_HIGH = "round_trip_cost_too_high"
REASON_SLIPPAGE_TOO_HIGH = "slippage_too_high"
REASON_LIQUIDITY_SCORE_BELOW_MIN = "liquidity_score_below_min"

ALL_REJECTION_REASONS = (
    REASON_PRICE_BELOW_MIN,
    REASON_ADV_USD_BELOW_MIN,
    REASON_RELATIVE_VOLUME_BELOW_MIN,
    REASON_SPREAD_TOO_WIDE,
    REASON_ROUND_TRIP_COST_TOO_HIGH,
    REASON_SLIPPAGE_TOO_HIGH,
    REASON_LIQUIDITY_SCORE_BELOW_MIN,
)


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class LiquidityConfig:
    min_adv_usd: float = 10_000_000.0
    min_rvol: float = 1.0
    max_spread_bps: float = 20.0
    max_round_trip_cost_bps: float = 40.0
    max_slippage_bps: float = 15.0
    min_price: float = 5.0
    min_liquidity_score: float = 0.6


# ---------------------------------------------------------------------------
# Metrics
# ---------------------------------------------------------------------------

@dataclass
class LiquidityMetrics:
    latest_close: float
    avg_daily_volume_shares: float
    avg_daily_dollar_volume: float
    relative_volume: float
    spread_estimate_bps: float
    slippage_estimate_bps: float
    round_trip_cost_bps: float


# ---------------------------------------------------------------------------
# Result
# ---------------------------------------------------------------------------

@dataclass
class LiquidityResult:
    passed: bool
    score: float
    rejection_reason: Optional[str]
    reason_tags: list[str] = field(default_factory=list)
    risk_tags: list[str] = field(default_factory=list)
    # diagnostics
    latest_close: float = 0.0
    avg_daily_dollar_volume: float = 0.0
    relative_volume: float = 0.0
    spread_estimate_bps: float = 0.0
    slippage_estimate_bps: float = 0.0
    round_trip_cost_bps: float = 0.0


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------

def _clamp01(v: float) -> float:
    return max(0.0, min(1.0, v))


def _component_score(value: float, threshold: float, *, higher_is_better: bool) -> float:
    """
    Normalize a single metric to [0, 1] reflecting quality within the passing band.

    higher_is_better=True:
        0.0 at exactly threshold (barely passing), 1.0 at 2x threshold or above.
    higher_is_better=False:
        0.0 at exactly threshold (barely passing), 1.0 at 0 or at half threshold.
    """
    if threshold <= 0.0:
        return 1.0 if value >= 0.0 else 0.0
    if higher_is_better:
        # linear from 0 at threshold to 1 at 2x threshold
        return _clamp01((value - threshold) / threshold)
    else:
        # linear from 0 at threshold to 1 at 0
        return _clamp01(1.0 - value / threshold)


def _compute_score(metrics: LiquidityMetrics, cfg: LiquidityConfig) -> float:
    """
    Deterministic composite liquidity score in [0.0, 1.0].

    Equal-weighted average of 6 normalized components.
    Hard failure returns 0.0 (computed before calling this).
    """
    components = [
        _component_score(metrics.latest_close, cfg.min_price, higher_is_better=True),
        _component_score(metrics.avg_daily_dollar_volume, cfg.min_adv_usd, higher_is_better=True),
        _component_score(metrics.relative_volume, cfg.min_rvol, higher_is_better=True),
        _component_score(metrics.spread_estimate_bps, cfg.max_spread_bps, higher_is_better=False),
        _component_score(metrics.round_trip_cost_bps, cfg.max_round_trip_cost_bps, higher_is_better=False),
        _component_score(metrics.slippage_estimate_bps, cfg.max_slippage_bps, higher_is_better=False),
    ]
    raw = sum(components) / len(components)
    return round(_clamp01(raw), 6)


def _fail(
    reason: str,
    metrics: LiquidityMetrics,
    tags: list[str],
    risk_tags: list[str],
) -> LiquidityResult:
    return LiquidityResult(
        passed=False,
        score=0.0,
        rejection_reason=reason,
        reason_tags=["liquidity"] + tags,
        risk_tags=risk_tags,
        latest_close=metrics.latest_close,
        avg_daily_dollar_volume=metrics.avg_daily_dollar_volume,
        relative_volume=metrics.relative_volume,
        spread_estimate_bps=metrics.spread_estimate_bps,
        slippage_estimate_bps=metrics.slippage_estimate_bps,
        round_trip_cost_bps=metrics.round_trip_cost_bps,
    )


# ---------------------------------------------------------------------------
# Gate
# ---------------------------------------------------------------------------

def evaluate_liquidity(
    symbol: str,
    metrics: LiquidityMetrics,
    config: Optional[LiquidityConfig] = None,
) -> LiquidityResult:
    """
    Evaluate liquidity and execution-cost fitness for a candidate symbol.

    Checks are applied in order; the first failure is the rejection_reason.
    A composite score is computed for all candidates (0.0 on hard failure,
    [0.0, 1.0] otherwise). Candidates with score < min_liquidity_score are
    rejected with REASON_LIQUIDITY_SCORE_BELOW_MIN even if all hard checks pass.

    Parameters
    ----------
    symbol:
        Symbol being evaluated (informational only; not mutated).
    metrics:
        Liquidity and cost metrics for the symbol.
    config:
        Gate thresholds.  Defaults to LiquidityConfig() if omitted.

    Returns
    -------
    LiquidityResult
    """
    cfg = config or LiquidityConfig()

    # Check 1: price floor
    if metrics.latest_close < cfg.min_price:
        return _fail(REASON_PRICE_BELOW_MIN, metrics, [], ["liquidity_risk"])

    # Check 2: ADV
    if metrics.avg_daily_dollar_volume < cfg.min_adv_usd:
        return _fail(REASON_ADV_USD_BELOW_MIN, metrics, [], ["liquidity_risk"])

    # Check 3: relative volume
    if metrics.relative_volume < cfg.min_rvol:
        return _fail(REASON_RELATIVE_VOLUME_BELOW_MIN, metrics, [], ["liquidity_risk"])

    # Check 4: spread
    if metrics.spread_estimate_bps > cfg.max_spread_bps:
        return _fail(REASON_SPREAD_TOO_WIDE, metrics, [], ["execution_cost_risk"])

    # Check 5: round-trip cost
    if metrics.round_trip_cost_bps > cfg.max_round_trip_cost_bps:
        return _fail(REASON_ROUND_TRIP_COST_TOO_HIGH, metrics, [], ["execution_cost_risk"])

    # Check 6: slippage
    if metrics.slippage_estimate_bps > cfg.max_slippage_bps:
        return _fail(REASON_SLIPPAGE_TOO_HIGH, metrics, [], ["execution_cost_risk"])

    # Composite score
    score = _compute_score(metrics, cfg)

    # Check 7: composite score floor
    if score < cfg.min_liquidity_score:
        return LiquidityResult(
            passed=False,
            score=score,
            rejection_reason=REASON_LIQUIDITY_SCORE_BELOW_MIN,
            reason_tags=["liquidity"],
            risk_tags=["liquidity_risk"],
            latest_close=metrics.latest_close,
            avg_daily_dollar_volume=metrics.avg_daily_dollar_volume,
            relative_volume=metrics.relative_volume,
            spread_estimate_bps=metrics.spread_estimate_bps,
            slippage_estimate_bps=metrics.slippage_estimate_bps,
            round_trip_cost_bps=metrics.round_trip_cost_bps,
        )

    return LiquidityResult(
        passed=True,
        score=score,
        rejection_reason=None,
        reason_tags=[],
        risk_tags=[],
        latest_close=metrics.latest_close,
        avg_daily_dollar_volume=metrics.avg_daily_dollar_volume,
        relative_volume=metrics.relative_volume,
        spread_estimate_bps=metrics.spread_estimate_bps,
        slippage_estimate_bps=metrics.slippage_estimate_bps,
        round_trip_cost_bps=metrics.round_trip_cost_bps,
    )


# ---------------------------------------------------------------------------
# Rejection candidate integration helper
# ---------------------------------------------------------------------------

def build_liquidity_rejection_candidate(
    *,
    scanner_id: str,
    symbol: str,
    exchange: str,
    timeframe: str,
    source: str,
    latest_bar_ts: str,
    liq_result: LiquidityResult,
    lookback_close: float = 0.0,
    move_bps: float = 0.0,
    abs_move_bps: float = 0.0,
    volume: int = 0,
    avg_volume: float = 0.0,
    atr: float = 0.0,
    gap_pct: float = 0.0,
    data_quality_score: float = 0.0,
    generated_at_utc: Optional[str] = None,
) -> dict[str, Any]:
    """
    Build a rejected scanner-candidate-v1 record from a liquidity gate failure.

    All eligibility flags are False.  eligible_for_live is always False (schema invariant).
    This function must only be called with a failed LiquidityResult (passed=False).
    """
    if liq_result.passed:
        raise ValueError(
            "build_liquidity_rejection_candidate called with a passing liquidity result; "
            "only failing results may produce rejection records"
        )

    from mqk_research.scanner.candidates import build_scanner_candidate

    return build_scanner_candidate(
        scanner_id=scanner_id,
        symbol=symbol,
        exchange=exchange,
        timeframe=timeframe,
        source=source,
        latest_bar_ts=latest_bar_ts,
        latest_close=liq_result.latest_close,
        lookback_close=lookback_close,
        move_bps=move_bps,
        abs_move_bps=abs_move_bps,
        volume=volume,
        avg_volume=avg_volume,
        relative_volume=liq_result.relative_volume,
        atr=atr,
        gap_pct=gap_pct,
        spread_estimate_bps=liq_result.spread_estimate_bps,
        slippage_estimate_bps=liq_result.slippage_estimate_bps,
        round_trip_cost_bps=liq_result.round_trip_cost_bps,
        data_quality_score=data_quality_score,
        liquidity_score=liq_result.score,
        trend_score=0.0,
        momentum_score=0.0,
        mean_reversion_score=0.0,
        volatility_score=0.0,
        regime_label="unknown",
        regime_score=0.0,
        risk_score=0.0,
        total_score=0.0,
        reason_tags=list(liq_result.reason_tags),
        risk_tags=list(liq_result.risk_tags),
        eligible_for_scan=False,
        eligible_for_backtest=False,
        eligible_for_paper=False,
        rejection_reason=liq_result.rejection_reason,
        recommended_strategy=None,
        notes=None,
        generated_at_utc=generated_at_utc,
    )
