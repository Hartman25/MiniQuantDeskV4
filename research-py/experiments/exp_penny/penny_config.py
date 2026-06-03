"""
EXP-PENNY-SCANNER-CONFIG-VARS-01 — Environment-driven scanner configuration.

Reads MQK_EXP_PENNY_* vars and validates safety invariants.
Fails closed on any execution-enabling state.

Safety rules enforced here:
- MQK_EXP_PENNY_LIVE_ALLOWED must be absent or "false".
- MQK_EXP_PENNY_ORDER_MODE must be "scanner_only".
- MQK_EXP_PENNY_ALLOW_SHORTS must be absent or "false".
- Numeric bounds are validated on construction.

These vars tune scanner/reject logic only. They do not wire execution.
"""
from __future__ import annotations

import os
from dataclasses import dataclass, field
from typing import List


class PennyScannerConfigError(ValueError):
    """Raised when configuration is invalid or would enable unsafe state."""


def _bool_env(name: str, default: bool) -> bool:
    val = os.environ.get(name)
    if val is None:
        return default
    return val.strip().lower() in ("1", "true", "yes")


def _float_env(name: str, default: float) -> float:
    val = os.environ.get(name)
    if val is None:
        return default
    return float(val.strip())


def _int_env(name: str, default: int) -> int:
    val = os.environ.get(name)
    if val is None:
        return default
    return int(val.strip())


def _str_env(name: str, default: str) -> str:
    val = os.environ.get(name)
    return val.strip() if val is not None else default


def _list_env(name: str, default: List[str]) -> List[str]:
    val = os.environ.get(name)
    if val is None:
        return default
    return [x.strip() for x in val.split(",") if x.strip()]


@dataclass
class PennyScannerConfig:
    """
    Scanner gate parameters loaded from MQK_EXP_PENNY_* environment variables.

    All fields default to the values specified in .env.local.example.
    Construction fails closed on any execution-enabling configuration.
    """

    # --- Price / exchange ---
    min_price: float = 0.50
    max_price: float = 20.00
    exclude_otc: bool = True
    allowed_exchanges: List[str] = field(default_factory=lambda: ["NASDAQ", "NYSE", "AMEX"])

    # --- Liquidity / spread ---
    min_adv_usd: float = 500_000.0
    min_daily_volume_shares: int = 200_000
    min_current_volume_shares: int = 300_000
    max_spread_pct: float = 1.0

    # --- Momentum / breakout ---
    min_rvol: float = 2.0
    min_breakout_rvol: float = 2.0
    max_consolidation_range_pct: float = 15.0
    require_breakout_above_level: bool = True

    # --- Trend / support-resistance ---
    require_ma50_slope_positive: bool = True
    require_ma200_slope_positive: bool = True
    min_distance_above_support_pct: float = 0.0
    max_chase_above_breakout_pct: float = 8.0

    # --- Float / manipulation risk ---
    min_float_shares: int = 5_000_000
    max_float_shares: int = 100_000_000
    reject_recent_reverse_split: bool = True
    reject_active_offering: bool = True
    reject_recent_dilution: bool = True
    reject_promotion_risk: bool = True

    # --- Catalyst / news ---
    require_catalyst: bool = False
    allowed_catalyst_types: List[str] = field(
        default_factory=lambda: ["earnings", "contract", "fda", "merger", "sympathy", "sector_momentum"]
    )
    reject_unverified_social_hype: bool = True

    # --- Risk controls (scanner-only; do not create orders) ---
    max_position_notional_usd: float = 500.0
    max_daily_loss_usd: float = 100.0
    max_open_positions: int = 1
    max_trades_per_day: int = 3
    min_risk_reward: float = 2.0

    # --- Safety gates (must remain in locked state) ---
    allow_shorts: bool = False
    order_mode: str = "scanner_only"
    live_allowed: bool = False

    def __post_init__(self) -> None:
        self._validate()

    def _validate(self) -> None:
        if self.live_allowed:
            raise PennyScannerConfigError(
                "MQK_EXP_PENNY_LIVE_ALLOWED=true is not permitted. "
                "Live routing is not implemented for the penny scanner."
            )
        if self.order_mode != "scanner_only":
            raise PennyScannerConfigError(
                f"MQK_EXP_PENNY_ORDER_MODE={self.order_mode!r} is not permitted. "
                "Must be 'scanner_only'."
            )
        if self.allow_shorts:
            raise PennyScannerConfigError(
                "MQK_EXP_PENNY_ALLOW_SHORTS=true is not permitted. "
                "Short side is not implemented for the penny scanner."
            )
        if self.min_price <= 0:
            raise PennyScannerConfigError("min_price must be > 0")
        if self.max_price <= self.min_price:
            raise PennyScannerConfigError("max_price must be > min_price")
        if self.max_spread_pct <= 0:
            raise PennyScannerConfigError("max_spread_pct must be > 0")
        if self.min_adv_usd <= 0:
            raise PennyScannerConfigError("min_adv_usd must be > 0")
        if self.min_risk_reward <= 0:
            raise PennyScannerConfigError("min_risk_reward must be > 0")
        if self.max_position_notional_usd <= 0:
            raise PennyScannerConfigError("max_position_notional_usd must be > 0")
        if self.max_daily_loss_usd <= 0:
            raise PennyScannerConfigError("max_daily_loss_usd must be > 0")
        if self.min_breakout_rvol <= 0:
            raise PennyScannerConfigError("min_breakout_rvol must be > 0")
        if self.min_rvol <= 0:
            raise PennyScannerConfigError("min_rvol must be > 0")
        if self.min_current_volume_shares <= 0:
            raise PennyScannerConfigError("min_current_volume_shares must be > 0")
        if self.max_open_positions <= 0:
            raise PennyScannerConfigError("max_open_positions must be > 0")
        if self.max_trades_per_day <= 0:
            raise PennyScannerConfigError("max_trades_per_day must be > 0")
        if self.max_chase_above_breakout_pct < 0:
            raise PennyScannerConfigError("max_chase_above_breakout_pct must be >= 0")
        if self.max_float_shares <= self.min_float_shares:
            raise PennyScannerConfigError("max_float_shares must be > min_float_shares")
        if not self.allowed_exchanges:
            raise PennyScannerConfigError("allowed_exchanges must not be empty")

    @classmethod
    def from_env(cls) -> "PennyScannerConfig":
        """Load configuration from MQK_EXP_PENNY_* environment variables."""
        return cls(
            min_price=_float_env("MQK_EXP_PENNY_MIN_PRICE", 0.50),
            max_price=_float_env("MQK_EXP_PENNY_MAX_PRICE", 20.00),
            exclude_otc=_bool_env("MQK_EXP_PENNY_EXCLUDE_OTC", True),
            allowed_exchanges=_list_env(
                "MQK_EXP_PENNY_ALLOWED_EXCHANGES", ["NASDAQ", "NYSE", "AMEX"]
            ),
            min_adv_usd=_float_env("MQK_EXP_PENNY_MIN_ADV_USD", 500_000.0),
            min_daily_volume_shares=_int_env("MQK_EXP_PENNY_MIN_DAILY_VOLUME_SHARES", 200_000),
            min_current_volume_shares=_int_env("MQK_EXP_PENNY_MIN_CURRENT_VOLUME_SHARES", 300_000),
            max_spread_pct=_float_env("MQK_EXP_PENNY_MAX_SPREAD_PCT", 1.0),
            min_rvol=_float_env("MQK_EXP_PENNY_MIN_RVOL", 2.0),
            min_breakout_rvol=_float_env("MQK_EXP_PENNY_MIN_BREAKOUT_RVOL", 2.0),
            max_consolidation_range_pct=_float_env(
                "MQK_EXP_PENNY_MAX_CONSOLIDATION_RANGE_PCT", 15.0
            ),
            require_breakout_above_level=_bool_env(
                "MQK_EXP_PENNY_REQUIRE_BREAKOUT_ABOVE_LEVEL", True
            ),
            require_ma50_slope_positive=_bool_env(
                "MQK_EXP_PENNY_REQUIRE_MA50_SLOPE_POSITIVE", True
            ),
            require_ma200_slope_positive=_bool_env(
                "MQK_EXP_PENNY_REQUIRE_MA200_SLOPE_POSITIVE", True
            ),
            min_distance_above_support_pct=_float_env(
                "MQK_EXP_PENNY_MIN_DISTANCE_ABOVE_SUPPORT_PCT", 0.0
            ),
            max_chase_above_breakout_pct=_float_env(
                "MQK_EXP_PENNY_MAX_CHASE_ABOVE_BREAKOUT_PCT", 8.0
            ),
            min_float_shares=_int_env("MQK_EXP_PENNY_MIN_FLOAT_SHARES", 5_000_000),
            max_float_shares=_int_env("MQK_EXP_PENNY_MAX_FLOAT_SHARES", 100_000_000),
            reject_recent_reverse_split=_bool_env(
                "MQK_EXP_PENNY_REJECT_RECENT_REVERSE_SPLIT", True
            ),
            reject_active_offering=_bool_env("MQK_EXP_PENNY_REJECT_ACTIVE_OFFERING", True),
            reject_recent_dilution=_bool_env("MQK_EXP_PENNY_REJECT_RECENT_DILUTION", True),
            reject_promotion_risk=_bool_env("MQK_EXP_PENNY_REJECT_PROMOTION_RISK", True),
            require_catalyst=_bool_env("MQK_EXP_PENNY_REQUIRE_CATALYST", False),
            allowed_catalyst_types=_list_env(
                "MQK_EXP_PENNY_ALLOWED_CATALYST_TYPES",
                ["earnings", "contract", "fda", "merger", "sympathy", "sector_momentum"],
            ),
            reject_unverified_social_hype=_bool_env(
                "MQK_EXP_PENNY_REJECT_UNVERIFIED_SOCIAL_HYPE", True
            ),
            max_position_notional_usd=_float_env("MQK_EXP_PENNY_MAX_POSITION_NOTIONAL_USD", 500.0),
            max_daily_loss_usd=_float_env("MQK_EXP_PENNY_MAX_DAILY_LOSS_USD", 100.0),
            max_open_positions=_int_env("MQK_EXP_PENNY_MAX_OPEN_POSITIONS", 1),
            max_trades_per_day=_int_env("MQK_EXP_PENNY_MAX_TRADES_PER_DAY", 3),
            min_risk_reward=_float_env("MQK_EXP_PENNY_MIN_RISK_REWARD", 2.0),
            allow_shorts=_bool_env("MQK_EXP_PENNY_ALLOW_SHORTS", False),
            order_mode=_str_env("MQK_EXP_PENNY_ORDER_MODE", "scanner_only"),
            live_allowed=_bool_env("MQK_EXP_PENNY_LIVE_ALLOWED", False),
        )

    def to_breakout_config_kwargs(self) -> dict:
        """Return kwargs that map to PennyBreakoutConfig fields."""
        return {
            "min_price": self.min_price,
            "max_price": self.max_price,
            "min_adv_usd": self.min_adv_usd,
            "min_daily_vol_shares": self.min_daily_volume_shares,
            "min_current_vol_shares": self.min_current_volume_shares,
            "max_spread_pct": self.max_spread_pct,
            "ma200_slope_min": 0.0 if self.require_ma200_slope_positive else -1e9,
            "ma50_slope_min": 0.0 if self.require_ma50_slope_positive else -1e9,
            "max_consolidation_range_pct": self.max_consolidation_range_pct,
            "breakout_rvol_min": self.min_breakout_rvol,
            "rvol_min": self.min_rvol,
            "require_breakout_above_level": self.require_breakout_above_level,
            "max_chase_above_breakout_pct": self.max_chase_above_breakout_pct,
        }
