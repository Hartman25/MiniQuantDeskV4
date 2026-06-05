"""
SCANNER-DQ-01 — MAIN scanner data-quality gate.

Validates bar-history quality before a symbol can become an accepted MAIN scanner candidate.
Integrates with ScannerCandidateWriter via build_data_quality_rejection_candidate.

Hard invariants:
- No imports from broker adapters, OMS tables, execution orchestrator, or DB
- No network imports (requests, urllib, http.client, aiohttp, psycopg, sqlalchemy)
- No orders are placed at any stage
- eligible_for_live is always False
- Rejection records are written via scanner-candidate-v1 schema
"""
from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, Optional


# ---------------------------------------------------------------------------
# Stable rejection reason names
# ---------------------------------------------------------------------------

REASON_NO_BARS = "no_bars"
REASON_INSUFFICIENT_COMPLETED_BARS = "insufficient_completed_bars"
REASON_LATEST_BAR_STALE = "latest_bar_stale"
REASON_DUPLICATE_BAR_TIMESTAMP = "duplicate_bar_timestamp"
REASON_TOO_MANY_ZERO_VOLUME_BARS = "too_many_zero_volume_bars"
REASON_ZERO_PRICE_BAR = "zero_price_bar"
REASON_INVERTED_OHLC = "inverted_ohlc"
REASON_CLOSE_OUTSIDE_OHLC_RANGE = "close_outside_ohlc_range"
REASON_MISSING_BAR_GAP = "missing_bar_gap"
REASON_LATEST_BAR_INCOMPLETE = "latest_bar_incomplete"

ALL_REJECTION_REASONS = (
    REASON_NO_BARS,
    REASON_INSUFFICIENT_COMPLETED_BARS,
    REASON_LATEST_BAR_STALE,
    REASON_DUPLICATE_BAR_TIMESTAMP,
    REASON_TOO_MANY_ZERO_VOLUME_BARS,
    REASON_ZERO_PRICE_BAR,
    REASON_INVERTED_OHLC,
    REASON_CLOSE_OUTSIDE_OHLC_RANGE,
    REASON_MISSING_BAR_GAP,
    REASON_LATEST_BAR_INCOMPLETE,
)


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class DataQualityConfig:
    max_bar_age_minutes: float = 20.0
    min_bars_required: int = 30
    max_missing_gap_bars: int = 2
    max_zero_volume_fraction: float = 0.05
    expected_bar_interval_minutes: float = 5.0


# ---------------------------------------------------------------------------
# Result
# ---------------------------------------------------------------------------

@dataclass
class DataQualityResult:
    passed: bool
    score: float
    rejection_reason: Optional[str]
    reason_tags: list[str] = field(default_factory=list)
    risk_tags: list[str] = field(default_factory=list)
    latest_bar_ts: Optional[str] = None
    completed_bar_count: int = 0
    # diagnostics
    duplicate_count: int = 0
    zero_volume_count: int = 0
    missing_gap_count: int = 0
    bad_ohlc_count: int = 0


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _parse_ts(ts: Any) -> Optional[datetime]:
    """
    Parse bar ts to an aware datetime.
    Accepts: datetime (aware or naive→UTC), ISO str, int/float POSIX seconds.
    Returns None on failure.
    """
    if isinstance(ts, datetime):
        if ts.tzinfo is None:
            return ts.replace(tzinfo=timezone.utc)
        return ts
    if isinstance(ts, (int, float)):
        try:
            return datetime.fromtimestamp(ts, tz=timezone.utc)
        except (OSError, OverflowError, ValueError):
            return None
    if isinstance(ts, str):
        ts = ts.strip()
        for fmt in (
            "%Y-%m-%dT%H:%M:%S%z",
            "%Y-%m-%dT%H:%M:%S.%f%z",
            "%Y-%m-%dT%H:%M:%SZ",
            "%Y-%m-%dT%H:%M:%S.%fZ",
            "%Y-%m-%dT%H:%M:%S",
        ):
            try:
                dt = datetime.strptime(ts, fmt)
                if dt.tzinfo is None:
                    dt = dt.replace(tzinfo=timezone.utc)
                return dt
            except ValueError:
                continue
    return None


def _fail(reason: str, tags: list[str], risk_tags: list[str], **diag: int) -> DataQualityResult:
    return DataQualityResult(
        passed=False,
        score=0.0,
        rejection_reason=reason,
        reason_tags=["data_quality"] + tags,
        risk_tags=risk_tags,
        **diag,
    )


# ---------------------------------------------------------------------------
# Gate
# ---------------------------------------------------------------------------

def evaluate_data_quality(
    symbol: str,
    timeframe: str,
    bars: list[dict[str, Any]],
    config: Optional[DataQualityConfig] = None,
    now_utc: Optional[datetime] = None,
) -> DataQualityResult:
    """
    Evaluate bar-history data quality for a candidate symbol.

    Parameters
    ----------
    symbol:
        Symbol being evaluated (informational only; not mutated).
    timeframe:
        Timeframe string (e.g. "5Min", "1D").
    bars:
        List of bar dicts.  Each must have: ts, open, high, low, close, volume.
        Optional: is_complete (bool).
    config:
        Quality thresholds.  Defaults to DataQualityConfig() if omitted.
    now_utc:
        Reference time for freshness check.  Defaults to datetime.now(UTC).

    Returns
    -------
    DataQualityResult
    """
    cfg = config or DataQualityConfig()
    now = now_utc or datetime.now(timezone.utc)

    # Check 1: no bars at all
    if not bars:
        return _fail(REASON_NO_BARS, [], ["data_quality_risk"])

    # Strip the latest bar if it is explicitly incomplete
    working_bars = list(bars)
    latest_raw = working_bars[-1]
    latest_incomplete = False
    if "is_complete" in latest_raw and latest_raw["is_complete"] is False:
        latest_incomplete = True
        working_bars = working_bars[:-1]

    # Check 2: duplicate timestamps (operate on all bars including possibly-incomplete)
    all_ts_raw = [b.get("ts") for b in bars]
    ts_strs = [str(t) for t in all_ts_raw]
    if len(ts_strs) != len(set(ts_strs)):
        return _fail(
            REASON_DUPLICATE_BAR_TIMESTAMP, [], ["data_quality_risk"],
            duplicate_count=len(ts_strs) - len(set(ts_strs)),
        )

    # Check 3: insufficient completed bars
    completed_count = len(working_bars)
    if completed_count < cfg.min_bars_required:
        if latest_incomplete:
            return _fail(
                REASON_LATEST_BAR_INCOMPLETE,
                [],
                ["data_quality_risk"],
                completed_bar_count=completed_count,
            )
        return _fail(
            REASON_INSUFFICIENT_COMPLETED_BARS, [], ["data_quality_risk"],
            completed_bar_count=completed_count,
        )

    # Use completed bars for remaining checks
    # Check 4: latest bar freshness (use last completed bar)
    latest_bar = working_bars[-1]
    latest_ts = _parse_ts(latest_bar.get("ts"))
    latest_bar_ts_str: Optional[str] = str(latest_bar.get("ts")) if latest_ts else None

    if latest_ts is None:
        return _fail(REASON_LATEST_BAR_STALE, [], ["data_quality_risk"])

    age_minutes = (now - latest_ts).total_seconds() / 60.0
    if age_minutes > cfg.max_bar_age_minutes:
        return _fail(
            REASON_LATEST_BAR_STALE, [], ["data_quality_risk"],
            latest_bar_ts=latest_bar_ts_str,
        )

    # Checks 5–8: per-bar OHLCV validation
    zero_volume_count = 0
    bad_ohlc_count = 0
    for bar in working_bars:
        o = bar.get("open", 0)
        h = bar.get("high", 0)
        lo = bar.get("low", 0)
        c = bar.get("close", 0)
        v = bar.get("volume", 0)

        if o <= 0 or h <= 0 or lo <= 0 or c <= 0:
            return _fail(
                REASON_ZERO_PRICE_BAR, [], ["data_quality_risk"],
                latest_bar_ts=latest_bar_ts_str,
                completed_bar_count=completed_count,
            )

        if h < lo:
            return _fail(
                REASON_INVERTED_OHLC, [], ["data_quality_risk"],
                latest_bar_ts=latest_bar_ts_str,
                completed_bar_count=completed_count,
                bad_ohlc_count=1,
            )

        if c < lo or c > h:
            return _fail(
                REASON_CLOSE_OUTSIDE_OHLC_RANGE, [], ["data_quality_risk"],
                latest_bar_ts=latest_bar_ts_str,
                completed_bar_count=completed_count,
                bad_ohlc_count=1,
            )

        if v == 0:
            zero_volume_count += 1

    # Check 5: zero-volume fraction
    zero_vol_fraction = zero_volume_count / completed_count
    if zero_vol_fraction > cfg.max_zero_volume_fraction:
        return _fail(
            REASON_TOO_MANY_ZERO_VOLUME_BARS, [], ["data_quality_risk"],
            latest_bar_ts=latest_bar_ts_str,
            completed_bar_count=completed_count,
            zero_volume_count=zero_volume_count,
        )

    # Check 9: missing bar gap (only for intraday timeframes where we can detect gaps)
    missing_gap_count = 0
    parsed_ts_list = [_parse_ts(b.get("ts")) for b in working_bars]
    valid_ts = [t for t in parsed_ts_list if t is not None]
    if len(valid_ts) > 1:
        interval_seconds = cfg.expected_bar_interval_minutes * 60.0
        # Only gap-check if interval is < 1 day (intraday)
        if cfg.expected_bar_interval_minutes < 1440:
            for i in range(1, len(valid_ts)):
                gap_seconds = (valid_ts[i] - valid_ts[i - 1]).total_seconds()
                gap_bars = gap_seconds / interval_seconds
                if gap_bars > cfg.max_missing_gap_bars + 1:
                    missing_gap_count += 1
            if missing_gap_count > 0:
                return _fail(
                    REASON_MISSING_BAR_GAP, [], ["data_quality_risk"],
                    latest_bar_ts=latest_bar_ts_str,
                    completed_bar_count=completed_count,
                    missing_gap_count=missing_gap_count,
                )

    # Check: latest bar explicitly incomplete and we only hit here if counts met
    # (covered above before min_bars check, but guard it here too)
    if latest_incomplete and completed_count < cfg.min_bars_required:
        return _fail(
            REASON_LATEST_BAR_INCOMPLETE, [], ["data_quality_risk"],
            latest_bar_ts=latest_bar_ts_str,
            completed_bar_count=completed_count,
        )

    # All checks passed — compute score
    # Score degrades linearly with zero-volume fraction
    score = round(max(0.0, min(1.0, 1.0 - zero_vol_fraction * 2.0)), 6)
    if score <= 0.0:
        score = 0.01  # never return exactly 0 on pass

    return DataQualityResult(
        passed=True,
        score=score,
        rejection_reason=None,
        reason_tags=[],
        risk_tags=[],
        latest_bar_ts=latest_bar_ts_str,
        completed_bar_count=completed_count,
        zero_volume_count=zero_volume_count,
        missing_gap_count=missing_gap_count,
    )


# ---------------------------------------------------------------------------
# Rejection candidate integration helper
# ---------------------------------------------------------------------------

def build_data_quality_rejection_candidate(
    *,
    scanner_id: str,
    symbol: str,
    exchange: str,
    timeframe: str,
    source: str,
    dq_result: DataQualityResult,
    generated_at_utc: Optional[str] = None,
) -> dict[str, Any]:
    """
    Build a rejected scanner-candidate-v1 record from a DQ failure.

    All eligibility flags are False.  eligible_for_live is always False (schema invariant).
    Unknown numeric scores use safe neutral defaults (0.0).
    This function must only be called with a failed DataQualityResult (passed=False).
    """
    if dq_result.passed:
        raise ValueError(
            "build_data_quality_rejection_candidate called with a passing DQ result; "
            "only failing results may produce rejection records"
        )

    from mqk_research.scanner.candidates import build_scanner_candidate

    return build_scanner_candidate(
        scanner_id=scanner_id,
        symbol=symbol,
        exchange=exchange,
        timeframe=timeframe,
        source=source,
        latest_bar_ts=dq_result.latest_bar_ts or "unknown",
        latest_close=0.0,
        lookback_close=0.0,
        move_bps=0.0,
        abs_move_bps=0.0,
        volume=0,
        avg_volume=0.0,
        relative_volume=0.0,
        atr=0.0,
        gap_pct=0.0,
        spread_estimate_bps=0.0,
        slippage_estimate_bps=0.0,
        round_trip_cost_bps=0.0,
        data_quality_score=dq_result.score,
        liquidity_score=0.0,
        trend_score=0.0,
        momentum_score=0.0,
        mean_reversion_score=0.0,
        volatility_score=0.0,
        regime_label="unknown",
        regime_score=0.0,
        risk_score=0.0,
        total_score=0.0,
        reason_tags=list(dq_result.reason_tags),
        risk_tags=list(dq_result.risk_tags),
        eligible_for_scan=False,
        eligible_for_backtest=False,
        eligible_for_paper=False,
        rejection_reason=dq_result.rejection_reason,
        recommended_strategy=None,
        notes=None,
        generated_at_utc=generated_at_utc,
    )
