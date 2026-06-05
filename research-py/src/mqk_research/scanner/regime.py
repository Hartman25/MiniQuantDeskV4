"""
SCANNER-REGIME-01 — MAIN scanner volatility / regime classification.

Classifies a symbol's current market behavior after it passes DQ and liquidity gates.
Produces deterministic regime labels and scores for candidate scoring,
strategy compatibility, and backtest queue selection.

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

REASON_REGIME_UNCLASSIFIED = "regime_unclassified"
REASON_REGIME_SCORE_BELOW_MIN = "regime_score_below_min"

ALL_REJECTION_REASONS = (
    REASON_REGIME_UNCLASSIFIED,
    REASON_REGIME_SCORE_BELOW_MIN,
)

# ---------------------------------------------------------------------------
# Regime labels
# ---------------------------------------------------------------------------

LABEL_TRENDING_UP = "trending_up"
LABEL_TRENDING_DOWN = "trending_down"
LABEL_RANGE_BOUND = "range_bound"
LABEL_HIGH_VOLATILITY = "high_volatility"
LABEL_LOW_VOLATILITY = "low_volatility"
LABEL_GAPPING = "gapping"
LABEL_UNCLASSIFIED = "unclassified"

ALL_LABELS = (
    LABEL_TRENDING_UP,
    LABEL_TRENDING_DOWN,
    LABEL_RANGE_BOUND,
    LABEL_HIGH_VOLATILITY,
    LABEL_LOW_VOLATILITY,
    LABEL_GAPPING,
    LABEL_UNCLASSIFIED,
)


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class RegimeConfig:
    lookback_bars: int = 78
    short_move_bars: int = 5
    trend_move_bars: int = 20
    atr_bars: int = 14
    min_regime_score: float = 0.5
    trend_threshold_bps: float = 20.0
    high_volatility_atr_pct: float = 2.0
    low_volatility_atr_pct: float = 0.5
    gap_threshold_pct: float = 2.0
    range_threshold_bps: float = 15.0


# ---------------------------------------------------------------------------
# Result
# ---------------------------------------------------------------------------

@dataclass
class RegimeResult:
    passed: bool
    rejection_reason: Optional[str]
    regime_label: str
    regime_score: float
    atr: float
    atr_pct: float
    gap_pct: float
    move_bps: float
    abs_move_bps: float
    volatility_score: float
    trend_score: float
    momentum_score: float
    mean_reversion_score: float
    reason_tags: list[str] = field(default_factory=list)
    risk_tags: list[str] = field(default_factory=list)
    latest_close: float = 0.0
    lookback_close: float = 0.0


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------

def _clamp01(v: float) -> float:
    return max(0.0, min(1.0, v))


def _true_range(high: float, low: float, prev_close: float) -> float:
    return max(high - low, abs(high - prev_close), abs(low - prev_close))


def _compute_atr(bars: list[dict[str, Any]], atr_bars: int) -> float:
    """Average True Range over the last atr_bars periods (requires atr_bars+1 bars)."""
    if len(bars) < atr_bars + 1:
        return 0.0
    relevant = bars[-(atr_bars + 1):]
    trs = []
    for i in range(1, len(relevant)):
        h = float(relevant[i].get("high", 0.0) or 0.0)
        lo = float(relevant[i].get("low", 0.0) or 0.0)
        pc = float(relevant[i - 1].get("close", 0.0) or 0.0)
        if h > 0 and lo > 0 and pc > 0:
            trs.append(_true_range(h, lo, pc))
    if not trs:
        return 0.0
    return sum(trs) / len(trs)


def _safe_float(val: Any) -> float:
    try:
        v = float(val)
        return v if v == v else 0.0  # guard NaN
    except (TypeError, ValueError):
        return 0.0


# ---------------------------------------------------------------------------
# Gate
# ---------------------------------------------------------------------------

def evaluate_regime(
    symbol: str,
    bars: list[dict[str, Any]],
    config: Optional[RegimeConfig] = None,
) -> RegimeResult:
    """
    Classify the current regime of a symbol from its bar history.

    Parameters
    ----------
    symbol:
        Symbol being evaluated (informational only; not mutated).
    bars:
        List of bar dicts, each with: ts, open, high, low, close, volume.
        Bars must already have passed the DQ gate.
        Bars are assumed sorted ascending by time (oldest first).
    config:
        Classification thresholds. Defaults to RegimeConfig() if omitted.

    Returns
    -------
    RegimeResult
        passed=True if regime is classifiable and regime_score >= min_regime_score.
        passed=False with appropriate rejection_reason otherwise.
    """
    cfg = config or RegimeConfig()
    min_bars = cfg.atr_bars + 1

    def _make_unclassified() -> RegimeResult:
        return RegimeResult(
            passed=False,
            rejection_reason=REASON_REGIME_UNCLASSIFIED,
            regime_label=LABEL_UNCLASSIFIED,
            regime_score=0.0,
            atr=0.0,
            atr_pct=0.0,
            gap_pct=0.0,
            move_bps=0.0,
            abs_move_bps=0.0,
            volatility_score=0.0,
            trend_score=0.0,
            momentum_score=0.0,
            mean_reversion_score=0.0,
            reason_tags=["regime"],
            risk_tags=["regime_unclassified"],
        )

    if len(bars) < min_bars:
        return _make_unclassified()

    latest_close = _safe_float(bars[-1].get("close"))
    if latest_close <= 0.0:
        return _make_unclassified()

    # Lookback close: oldest bar within the lookback window
    lookback_idx = max(0, len(bars) - cfg.lookback_bars - 1)
    lookback_close = _safe_float(bars[lookback_idx].get("close"))
    if lookback_close <= 0.0:
        lookback_close = _safe_float(bars[0].get("close"))
    if lookback_close <= 0.0:
        return _make_unclassified()

    # Short-move close (for momentum_score)
    short_idx = max(0, len(bars) - cfg.short_move_bars - 1)
    short_close = _safe_float(bars[short_idx].get("close"))
    if short_close <= 0.0:
        short_close = lookback_close

    # Trend-move close (for trend classification)
    trend_idx = max(0, len(bars) - cfg.trend_move_bars - 1)
    trend_close = _safe_float(bars[trend_idx].get("close"))
    if trend_close <= 0.0:
        trend_close = lookback_close

    # Move calculations
    move_bps = (latest_close - lookback_close) / lookback_close * 10_000.0
    abs_move_bps = abs(move_bps)
    short_move_bps = (latest_close - short_close) / short_close * 10_000.0
    trend_move_bps = (latest_close - trend_close) / trend_close * 10_000.0

    # ATR
    atr = _compute_atr(bars, cfg.atr_bars)
    atr_pct = atr / latest_close * 100.0 if latest_close > 0.0 else 0.0

    # Gap pct: (latest open − prior close) / prior close × 100
    gap_pct = 0.0
    if len(bars) >= 2:
        prior_close = _safe_float(bars[-2].get("close"))
        latest_open = _safe_float(bars[-1].get("open"))
        if prior_close > 0.0 and latest_open > 0.0:
            gap_pct = (latest_open - prior_close) / prior_close * 100.0

    # Component scores [0, 1]
    hv_thresh = cfg.high_volatility_atr_pct if cfg.high_volatility_atr_pct > 0.0 else 1.0
    trend_thresh = cfg.trend_threshold_bps if cfg.trend_threshold_bps > 0.0 else 1.0
    range_thresh = cfg.range_threshold_bps if cfg.range_threshold_bps > 0.0 else 1.0

    volatility_score = _clamp01(atr_pct / hv_thresh)
    trend_score = _clamp01(abs(trend_move_bps) / (trend_thresh * 5.0))
    momentum_score = _clamp01(abs(short_move_bps) / (trend_thresh * 3.0))
    # Mean reversion: high when low-volatility and range-bound
    range_factor = _clamp01(1.0 - abs(trend_move_bps) / (range_thresh * 5.0))
    vol_factor = _clamp01(1.0 - atr_pct / hv_thresh)
    mean_reversion_score = _clamp01(range_factor * vol_factor)

    # Classification (strict priority order per spec)
    if abs(gap_pct) >= cfg.gap_threshold_pct:
        label = LABEL_GAPPING
        gap_thresh = cfg.gap_threshold_pct if cfg.gap_threshold_pct > 0.0 else 1.0
        regime_score = _clamp01(abs(gap_pct) / (gap_thresh * 2.0))
    elif atr_pct >= cfg.high_volatility_atr_pct:
        label = LABEL_HIGH_VOLATILITY
        excess = (atr_pct - cfg.high_volatility_atr_pct) / hv_thresh
        regime_score = _clamp01(0.5 + excess * 0.5)
    elif atr_pct <= cfg.low_volatility_atr_pct and abs(trend_move_bps) <= cfg.range_threshold_bps:
        label = LABEL_LOW_VOLATILITY
        lv_thresh = cfg.low_volatility_atr_pct if cfg.low_volatility_atr_pct > 0.0 else 1.0
        regime_score = _clamp01(1.0 - atr_pct / lv_thresh)
    elif trend_move_bps >= cfg.trend_threshold_bps:
        label = LABEL_TRENDING_UP
        regime_score = _clamp01(trend_move_bps / (trend_thresh * 5.0))
    elif trend_move_bps <= -cfg.trend_threshold_bps:
        label = LABEL_TRENDING_DOWN
        regime_score = _clamp01(abs(trend_move_bps) / (trend_thresh * 5.0))
    elif abs(trend_move_bps) <= cfg.range_threshold_bps:
        label = LABEL_RANGE_BOUND
        regime_score = _clamp01(1.0 - abs(trend_move_bps) / range_thresh)
    else:
        label = LABEL_UNCLASSIFIED
        regime_score = 0.0

    # Round scores for determinism
    atr_r = round(atr, 6)
    atr_pct_r = round(atr_pct, 6)
    gap_pct_r = round(gap_pct, 6)
    move_bps_r = round(move_bps, 4)
    abs_move_bps_r = round(abs_move_bps, 4)
    vol_score_r = round(volatility_score, 6)
    trend_score_r = round(trend_score, 6)
    mom_score_r = round(momentum_score, 6)
    mr_score_r = round(mean_reversion_score, 6)
    regime_score_r = round(regime_score, 6)

    if label == LABEL_UNCLASSIFIED:
        return RegimeResult(
            passed=False,
            rejection_reason=REASON_REGIME_UNCLASSIFIED,
            regime_label=label,
            regime_score=0.0,
            atr=atr_r,
            atr_pct=atr_pct_r,
            gap_pct=gap_pct_r,
            move_bps=move_bps_r,
            abs_move_bps=abs_move_bps_r,
            volatility_score=vol_score_r,
            trend_score=trend_score_r,
            momentum_score=mom_score_r,
            mean_reversion_score=mr_score_r,
            reason_tags=["regime"],
            risk_tags=["regime_unclassified"],
            latest_close=latest_close,
            lookback_close=lookback_close,
        )

    if regime_score_r < cfg.min_regime_score:
        return RegimeResult(
            passed=False,
            rejection_reason=REASON_REGIME_SCORE_BELOW_MIN,
            regime_label=label,
            regime_score=regime_score_r,
            atr=atr_r,
            atr_pct=atr_pct_r,
            gap_pct=gap_pct_r,
            move_bps=move_bps_r,
            abs_move_bps=abs_move_bps_r,
            volatility_score=vol_score_r,
            trend_score=trend_score_r,
            momentum_score=mom_score_r,
            mean_reversion_score=mr_score_r,
            reason_tags=["regime"],
            risk_tags=["regime_score_risk"],
            latest_close=latest_close,
            lookback_close=lookback_close,
        )

    return RegimeResult(
        passed=True,
        rejection_reason=None,
        regime_label=label,
        regime_score=regime_score_r,
        atr=atr_r,
        atr_pct=atr_pct_r,
        gap_pct=gap_pct_r,
        move_bps=move_bps_r,
        abs_move_bps=abs_move_bps_r,
        volatility_score=vol_score_r,
        trend_score=trend_score_r,
        momentum_score=mom_score_r,
        mean_reversion_score=mr_score_r,
        reason_tags=[],
        risk_tags=[],
        latest_close=latest_close,
        lookback_close=lookback_close,
    )


# ---------------------------------------------------------------------------
# Rejection candidate integration helper
# ---------------------------------------------------------------------------

def build_regime_rejection_candidate(
    *,
    scanner_id: str,
    symbol: str,
    exchange: str,
    timeframe: str,
    source: str,
    latest_bar_ts: str,
    regime_result: RegimeResult,
    liquidity_score: float = 0.0,
    data_quality_score: float = 0.0,
    spread_estimate_bps: float = 0.0,
    slippage_estimate_bps: float = 0.0,
    round_trip_cost_bps: float = 0.0,
    volume: int = 0,
    avg_volume: float = 0.0,
    generated_at_utc: Optional[str] = None,
) -> dict[str, Any]:
    """
    Build a rejected scanner-candidate-v1 record from a regime failure.

    All eligibility flags are False.  eligible_for_live is always False (schema invariant).
    This function must only be called with a failed RegimeResult (passed=False).
    """
    if regime_result.passed:
        raise ValueError(
            "build_regime_rejection_candidate called with a passing regime result; "
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
        latest_close=regime_result.latest_close,
        lookback_close=regime_result.lookback_close,
        move_bps=regime_result.move_bps,
        abs_move_bps=regime_result.abs_move_bps,
        volume=volume,
        avg_volume=avg_volume,
        relative_volume=0.0,
        atr=regime_result.atr,
        gap_pct=regime_result.gap_pct,
        spread_estimate_bps=spread_estimate_bps,
        slippage_estimate_bps=slippage_estimate_bps,
        round_trip_cost_bps=round_trip_cost_bps,
        data_quality_score=data_quality_score,
        liquidity_score=liquidity_score,
        trend_score=regime_result.trend_score,
        momentum_score=regime_result.momentum_score,
        mean_reversion_score=regime_result.mean_reversion_score,
        volatility_score=regime_result.volatility_score,
        regime_label=regime_result.regime_label,
        regime_score=regime_result.regime_score,
        risk_score=0.0,
        total_score=0.0,
        reason_tags=list(regime_result.reason_tags),
        risk_tags=list(regime_result.risk_tags),
        eligible_for_scan=False,
        eligible_for_backtest=False,
        eligible_for_paper=False,
        rejection_reason=regime_result.rejection_reason,
        recommended_strategy=None,
        notes=None,
        generated_at_utc=generated_at_utc,
    )


def apply_regime_to_candidate_fields(result: RegimeResult) -> dict[str, Any]:
    """
    Return candidate field values derived from a RegimeResult.

    No writes. Intended for use in SCANNER-SCORE-01 and later scoring patches.
    """
    return {
        "atr": result.atr,
        "gap_pct": result.gap_pct,
        "move_bps": result.move_bps,
        "abs_move_bps": result.abs_move_bps,
        "trend_score": result.trend_score,
        "momentum_score": result.momentum_score,
        "mean_reversion_score": result.mean_reversion_score,
        "volatility_score": result.volatility_score,
        "regime_label": result.regime_label,
        "regime_score": result.regime_score,
        "latest_close": result.latest_close,
        "lookback_close": result.lookback_close,
    }
