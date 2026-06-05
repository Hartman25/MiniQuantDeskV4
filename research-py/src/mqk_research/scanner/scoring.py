"""
SCANNER-SCORE-01 — MAIN scanner candidate scoring layer.

Combines DQ, liquidity, and regime gate results into a deterministic
scored scanner candidate record (scanner-candidate-v1).

Hard invariants:
- No imports from broker adapters, OMS tables, execution orchestrator, or DB
- No network imports (requests, urllib, http.client, aiohttp, psycopg, sqlalchemy)
- No orders are placed at any stage
- eligible_for_live is always False — enforced by build_scanner_candidate
- eligible_for_paper is always False until a watchlist/promotion gate exists
- Accepted candidates are only produced after DQ + liquidity + regime all pass
- EXP penny scanner (exp-candidate-v1) is not affected by this module
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Optional


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class ScoringConfig:
    w_liquidity: float = 0.25
    w_volatility: float = 0.20
    w_regime: float = 0.20
    w_momentum: float = 0.15
    w_trend: float = 0.10
    w_risk: float = 0.05
    w_cost: float = 0.05
    max_reason_tags: int = 20
    default_recommended_strategy: str = "intraday_scalper"

    # Risk thresholds (inputs above these raise the risk flag)
    risk_high_atr_pct: float = 3.0        # atr / latest_close * 100
    risk_large_gap_pct: float = 2.0       # abs(gap_pct)
    risk_high_cost_bps: float = 30.0      # round_trip_cost_bps
    risk_high_spread_bps: float = 15.0    # spread_estimate_bps
    risk_large_move_bps: float = 200.0    # abs_move_bps

    # Cost normalization ceiling (round_trip_cost_bps at which cost penalty = 1.0)
    cost_ceiling_bps: float = 40.0

    def validate(self) -> None:
        weights = [
            self.w_liquidity, self.w_volatility, self.w_regime,
            self.w_momentum, self.w_trend, self.w_risk, self.w_cost,
        ]
        for w in weights:
            if w < 0.0:
                raise ValueError(
                    f"ScoringConfig: all weights must be non-negative, got {w}"
                )


# ---------------------------------------------------------------------------
# Inputs
# ---------------------------------------------------------------------------

@dataclass
class ScoringInputs:
    symbol: str
    exchange: str
    timeframe: str
    source: str
    latest_bar_ts: str
    volume: int
    avg_volume: float
    relative_volume: float
    data_quality_score: float
    liquidity_score: float
    trend_score: float
    momentum_score: float
    mean_reversion_score: float
    volatility_score: float
    regime_label: str
    regime_score: float
    latest_close: float
    lookback_close: float
    move_bps: float
    abs_move_bps: float
    atr: float
    gap_pct: float
    spread_estimate_bps: float
    slippage_estimate_bps: float
    round_trip_cost_bps: float


# ---------------------------------------------------------------------------
# Result
# ---------------------------------------------------------------------------

@dataclass
class ScoringResult:
    total_score: float
    risk_score: float
    reason_tags: list[str] = field(default_factory=list)
    risk_tags: list[str] = field(default_factory=list)
    recommended_strategy: str = "intraday_scalper"


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------

def _clamp01(v: float) -> float:
    return max(0.0, min(1.0, v))


def _compute_risk_score(inputs: ScoringInputs, cfg: ScoringConfig) -> float:
    """
    Deterministic risk score in [0.0, 1.0].

    1.0 = low risk / clean candidate.
    Penalizes high ATR, large gap, high cost, wide spread, large move.
    """
    atr_pct = (inputs.atr / inputs.latest_close * 100.0) if inputs.latest_close > 0.0 else 0.0

    penalties = [
        _clamp01(atr_pct / cfg.risk_high_atr_pct) if cfg.risk_high_atr_pct > 0.0 else 0.0,
        _clamp01(abs(inputs.gap_pct) / cfg.risk_large_gap_pct) if cfg.risk_large_gap_pct > 0.0 else 0.0,
        _clamp01(inputs.round_trip_cost_bps / cfg.risk_high_cost_bps) if cfg.risk_high_cost_bps > 0.0 else 0.0,
        _clamp01(inputs.spread_estimate_bps / cfg.risk_high_spread_bps) if cfg.risk_high_spread_bps > 0.0 else 0.0,
        _clamp01(inputs.abs_move_bps / cfg.risk_large_move_bps) if cfg.risk_large_move_bps > 0.0 else 0.0,
    ]
    avg_penalty = sum(penalties) / len(penalties)
    return round(_clamp01(1.0 - avg_penalty), 6)


def _build_reason_tags(inputs: ScoringInputs, cfg: ScoringConfig) -> list[str]:
    tags: list[str] = []
    tags.append("scanner_scored")
    tags.append("dq_pass")
    tags.append("liquidity_pass")
    tags.append(f"regime_{inputs.regime_label}")
    if inputs.momentum_score >= 0.5:
        tags.append("momentum_positive")
    else:
        tags.append("momentum_negative")
    if inputs.volatility_score >= 0.5:
        tags.append("high_volatility")
    else:
        tags.append("low_volatility")
    if inputs.trend_score >= 0.5:
        tags.append("trend_positive")
    else:
        tags.append("trend_negative")
    cost_penalty = _clamp01(inputs.round_trip_cost_bps / cfg.cost_ceiling_bps) if cfg.cost_ceiling_bps > 0.0 else 0.0
    if cost_penalty <= 0.25:
        tags.append("low_cost")
    else:
        tags.append("high_cost_penalty")
    return tags[: cfg.max_reason_tags]


def _build_risk_tags(inputs: ScoringInputs, cfg: ScoringConfig) -> list[str]:
    tags: list[str] = []
    atr_pct = (inputs.atr / inputs.latest_close * 100.0) if inputs.latest_close > 0.0 else 0.0
    if atr_pct >= cfg.risk_high_atr_pct:
        tags.append("risk_high_atr")
    if abs(inputs.gap_pct) >= cfg.risk_large_gap_pct:
        tags.append("risk_large_gap")
    if inputs.round_trip_cost_bps >= cfg.risk_high_cost_bps:
        tags.append("risk_high_cost")
    if inputs.spread_estimate_bps >= cfg.risk_high_spread_bps:
        tags.append("risk_high_spread")
    if inputs.abs_move_bps >= cfg.risk_large_move_bps:
        tags.append("risk_large_move")
    if not tags:
        tags.append("risk_low")
    return tags


# ---------------------------------------------------------------------------
# Score function
# ---------------------------------------------------------------------------

def score_candidate(
    inputs: ScoringInputs,
    config: Optional[ScoringConfig] = None,
) -> ScoringResult:
    """
    Compute a deterministic ScoringResult from a set of ScoringInputs.

    All component scores are clamped to [0.0, 1.0].
    total_score is a weighted linear combination minus cost/risk penalties.
    """
    cfg = config or ScoringConfig()
    cfg.validate()

    risk_score = _compute_risk_score(inputs, cfg)

    cost_penalty = (
        _clamp01(inputs.round_trip_cost_bps / cfg.cost_ceiling_bps)
        if cfg.cost_ceiling_bps > 0.0
        else 0.0
    )

    liq = _clamp01(inputs.liquidity_score)
    vol = _clamp01(inputs.volatility_score)
    reg = _clamp01(inputs.regime_score)
    mom = _clamp01(inputs.momentum_score)
    trd = _clamp01(inputs.trend_score)
    risk_pen = _clamp01(1.0 - risk_score)   # higher risk → higher penalty
    cost_pen = cost_penalty

    raw = (
        cfg.w_liquidity * liq
        + cfg.w_volatility * vol
        + cfg.w_regime * reg
        + cfg.w_momentum * mom
        + cfg.w_trend * trd
        - cfg.w_risk * risk_pen
        - cfg.w_cost * cost_pen
    )
    total_score = round(_clamp01(raw), 6)

    reason_tags = _build_reason_tags(inputs, cfg)
    risk_tags = _build_risk_tags(inputs, cfg)

    return ScoringResult(
        total_score=total_score,
        risk_score=risk_score,
        reason_tags=reason_tags,
        risk_tags=risk_tags,
        recommended_strategy=cfg.default_recommended_strategy,
    )


# ---------------------------------------------------------------------------
# Accepted candidate builder
# ---------------------------------------------------------------------------

def build_scored_scanner_candidate(
    *,
    scanner_id: str,
    inputs: ScoringInputs,
    dq_passed: bool,
    liquidity_passed: bool,
    regime_passed: bool,
    config: Optional[ScoringConfig] = None,
    generated_at_utc: Optional[str] = None,
    notes: Optional[str] = None,
) -> dict[str, Any]:
    """
    Build an accepted scanner-candidate-v1 record after all upstream gates pass.

    Raises ValueError if any upstream gate did not pass.
    eligible_for_live is always False (enforced by build_scanner_candidate).
    eligible_for_paper is always False until a watchlist/promotion gate exists.
    """
    if not dq_passed:
        raise ValueError(
            f"build_scored_scanner_candidate: DQ gate did not pass for {inputs.symbol!r}; "
            "accepted candidates require dq_passed=True"
        )
    if not liquidity_passed:
        raise ValueError(
            f"build_scored_scanner_candidate: liquidity gate did not pass for {inputs.symbol!r}; "
            "accepted candidates require liquidity_passed=True"
        )
    if not regime_passed:
        raise ValueError(
            f"build_scored_scanner_candidate: regime gate did not pass for {inputs.symbol!r}; "
            "accepted candidates require regime_passed=True"
        )

    scoring = score_candidate(inputs, config)

    from mqk_research.scanner.candidates import build_scanner_candidate

    return build_scanner_candidate(
        scanner_id=scanner_id,
        symbol=inputs.symbol,
        exchange=inputs.exchange,
        timeframe=inputs.timeframe,
        source=inputs.source,
        latest_bar_ts=inputs.latest_bar_ts,
        latest_close=inputs.latest_close,
        lookback_close=inputs.lookback_close,
        move_bps=inputs.move_bps,
        abs_move_bps=inputs.abs_move_bps,
        volume=inputs.volume,
        avg_volume=inputs.avg_volume,
        relative_volume=inputs.relative_volume,
        atr=inputs.atr,
        gap_pct=inputs.gap_pct,
        spread_estimate_bps=inputs.spread_estimate_bps,
        slippage_estimate_bps=inputs.slippage_estimate_bps,
        round_trip_cost_bps=inputs.round_trip_cost_bps,
        data_quality_score=inputs.data_quality_score,
        liquidity_score=inputs.liquidity_score,
        trend_score=inputs.trend_score,
        momentum_score=inputs.momentum_score,
        mean_reversion_score=inputs.mean_reversion_score,
        volatility_score=inputs.volatility_score,
        regime_label=inputs.regime_label,
        regime_score=inputs.regime_score,
        risk_score=scoring.risk_score,
        total_score=scoring.total_score,
        reason_tags=scoring.reason_tags,
        risk_tags=scoring.risk_tags,
        eligible_for_scan=True,
        eligible_for_backtest=True,
        eligible_for_paper=False,
        rejection_reason=None,
        recommended_strategy=scoring.recommended_strategy,
        notes=notes,
        generated_at_utc=generated_at_utc,
    )
