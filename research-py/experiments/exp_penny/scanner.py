"""
EXP-PENNY-01A — Penny breakout/accumulation scanner (Stage 1, scanner-only).

Subclasses ScannerBase. Reads a static universe dict list. No market data
feed, no broker calls, no OMS writes. Candidate records are written by the
runner via CandidateJournalWriter.

Safety invariants (enforced here and by static guard):
- No imports from broker adapters, OMS tables, or execution orchestrator.
- paper_order_id and live_order_id are always null (set in build_candidate_record).
- would_trade=True only when all gates pass.
"""
from __future__ import annotations

from typing import Any, Optional

from experiments.exp_engine.candidate_journal import build_candidate_record
from experiments.exp_engine.scanner_base import ScannerBase
from experiments.exp_penny.penny_config import PennyScannerConfig

ENGINE_ID = "exp-engine-core-01"
LANE_ID = "exp-penny-01a"
STRATEGY_ID = "penny_breakout_accumulation_v1"
CANDIDATE_TYPE = "breakout_accumulation"
ASSET_CLASS = "us_equity"


class PennyBreakoutConfig:
    """Scanner gate parameters. All defaults are conservative."""

    def __init__(
        self,
        min_price: float = 0.50,
        max_price: float = 20.00,
        min_adv_usd: float = 500_000.0,
        min_daily_vol_shares: int = 200_000,
        max_spread_pct: float = 1.0,
        ma200_slope_min: float = 0.0,
        ma50_slope_min: float = 0.0,
        max_consolidation_range_pct: float = 15.0,
        breakout_rvol_min: float = 2.0,
    ) -> None:
        self.min_price = min_price
        self.max_price = max_price
        self.min_adv_usd = min_adv_usd
        self.min_daily_vol_shares = min_daily_vol_shares
        self.max_spread_pct = max_spread_pct
        self.ma200_slope_min = ma200_slope_min
        self.ma50_slope_min = ma50_slope_min
        self.max_consolidation_range_pct = max_consolidation_range_pct
        self.breakout_rvol_min = breakout_rvol_min


def _spread_pct(row: dict[str, Any]) -> Optional[float]:
    """Compute bid/ask spread as a percentage of mid price. None if data absent."""
    bid = row.get("bid")
    ask = row.get("ask")
    if bid is None or ask is None:
        return None
    mid = (bid + ask) / 2.0
    if mid <= 0:
        return None
    return ((ask - bid) / mid) * 100.0


def _evaluate_gates(
    row: dict[str, Any],
    cfg: PennyBreakoutConfig,
) -> Optional[str]:
    """
    Run all scanner gates for a single universe row.

    Returns None if all gates pass (candidate qualifies).
    Returns a rejection reason string on the first failing gate.
    """
    symbol = row.get("symbol", "?")

    # Gate: halt flag
    if row.get("halt_flag", False):
        return "halt_flag_set"

    # Gate: gap flag
    if row.get("gap_flag", False):
        return "gap_flag_set"

    # Gate: price range
    price = row.get("price")
    if price is None:
        return "missing_price"
    if price < cfg.min_price:
        return f"price_below_min ({price} < {cfg.min_price})"
    if price > cfg.max_price:
        return f"price_above_max ({price} > {cfg.max_price})"

    # Gate: ADV USD
    adv_usd = row.get("adv_20d_usd", 0.0)
    if adv_usd < cfg.min_adv_usd:
        return f"adv_usd_below_min ({adv_usd} < {cfg.min_adv_usd})"

    # Gate: daily volume shares
    volume = row.get("volume", 0)
    if volume < cfg.min_daily_vol_shares:
        return f"volume_below_min ({volume} < {cfg.min_daily_vol_shares})"

    # Gate: spread
    spread = _spread_pct(row)
    if spread is None:
        return "missing_bid_ask"
    if spread > cfg.max_spread_pct:
        return f"spread_too_wide ({spread:.2f}% > {cfg.max_spread_pct}%)"

    # Gate: MA200 slope (trend filter)
    ma200_slope = row.get("ma200_slope_20d")
    if ma200_slope is None:
        return "missing_ma200_slope"
    if ma200_slope <= cfg.ma200_slope_min:
        return f"ma200_slope_not_rising ({ma200_slope} <= {cfg.ma200_slope_min})"

    # Gate: MA50 slope (intermediate trend filter)
    ma50_slope = row.get("ma50_slope_20d")
    if ma50_slope is None:
        return "missing_ma50_slope"
    if ma50_slope <= cfg.ma50_slope_min:
        return f"ma50_slope_not_rising ({ma50_slope} <= {cfg.ma50_slope_min})"

    # Gate: consolidation range
    consol_range_pct = row.get("consolidation_range_pct")
    if consol_range_pct is None:
        return "missing_consolidation_range_pct"
    if consol_range_pct > cfg.max_consolidation_range_pct:
        return f"consolidation_too_wide ({consol_range_pct}% > {cfg.max_consolidation_range_pct}%)"

    # Gate: breakout RVOL
    breakout_rvol = row.get("breakout_rvol")
    if breakout_rvol is None:
        return "missing_breakout_rvol"
    if breakout_rvol < cfg.breakout_rvol_min:
        return f"breakout_rvol_weak ({breakout_rvol} < {cfg.breakout_rvol_min})"

    # Gate: price must be at or above breakout level
    breakout_level = row.get("breakout_level")
    if breakout_level is None:
        return "missing_breakout_level"
    if price < breakout_level:
        return f"price_below_breakout_level ({price} < {breakout_level})"

    return None  # all gates passed


class PennyBreakoutScanner(ScannerBase):
    """
    Stage 1 scanner-only implementation for EXP-PENNY-01A.

    Reads a static universe list (dicts). Evaluates scanner gates. Writes
    candidate records only. Never places orders, calls broker endpoints, or
    writes to OMS tables.
    """

    def __init__(
        self,
        universe: list[dict[str, Any]],
        config: Optional[PennyBreakoutConfig] = None,
        scanner_config: Optional[PennyScannerConfig] = None,
    ) -> None:
        self._universe = universe
        if config is not None:
            self._config = config
        elif scanner_config is not None:
            self._config = PennyBreakoutConfig(**scanner_config.to_breakout_config_kwargs())
        else:
            self._config = PennyBreakoutConfig()

    @property
    def engine_id(self) -> str:
        return ENGINE_ID

    @property
    def lane_id(self) -> str:
        return LANE_ID

    def scan(self) -> list[dict[str, Any]]:
        """
        Scan all universe rows and return candidate records.

        Each record has would_trade=True only when all gates pass.
        paper_order_id and live_order_id are always null (enforced by
        build_candidate_record).
        """
        records: list[dict[str, Any]] = []

        for row in self._universe:
            symbol = row.get("symbol", "UNKNOWN")
            price = row.get("price")
            rejection_reason = _evaluate_gates(row, self._config)
            would_trade = rejection_reason is None
            signal_direction = "long" if would_trade else "neutral"

            spread = _spread_pct(row)
            extra: dict[str, Any] = {
                "price_at_scan": price,
                "bid_at_scan": row.get("bid"),
                "ask_at_scan": row.get("ask"),
                "spread_pct_at_scan": round(spread, 4) if spread is not None else None,
                "volume_at_scan": row.get("volume"),
                "adv_20d_usd": row.get("adv_20d_usd"),
                "adv_20d_shares": row.get("adv_20d_shares"),
                "rvol": row.get("rvol"),
                "ma200_value": row.get("ma200"),
                "ma200_slope_20d": row.get("ma200_slope_20d"),
                "ma50_value": row.get("ma50"),
                "ma50_slope_20d": row.get("ma50_slope_20d"),
                "consolidation_high": row.get("consolidation_high"),
                "consolidation_low": row.get("consolidation_low"),
                "consolidation_range_pct": row.get("consolidation_range_pct"),
                "breakout_level": row.get("breakout_level"),
                "breakout_rvol": row.get("breakout_rvol"),
                "gap_flag": row.get("gap_flag"),
                "halt_flag": row.get("halt_flag"),
                "news_flag": row.get("news_flag"),
            }

            record = build_candidate_record(
                engine_id=ENGINE_ID,
                lane_id=LANE_ID,
                strategy_id=STRATEGY_ID,
                symbol=symbol,
                asset_class=ASSET_CLASS,
                candidate_type=CANDIDATE_TYPE,
                signal_direction=signal_direction,
                confidence_score=None,
                risk_notes="Stage 1 scanner-only. No orders placed.",
                rejection_reason=rejection_reason,
                would_trade=would_trade,
                extra=extra,
            )
            records.append(record)

        return records
