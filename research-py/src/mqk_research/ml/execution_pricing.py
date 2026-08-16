from __future__ import annotations

from dataclasses import dataclass, replace
from typing import Literal

# P7A (RESEARCH-EXECUTION-PRICING-PARITY-01)
#
# Pure, deterministic execution-pricing helpers reconciling Python Research's
# economic evaluator with the accepted Rust conservative fill model
# (core-rs/crates/mqk-backtest/src/engine.rs::conservative_fill_price /
# StressProfile). No IO, no wall-clock reads, no RNG.
#
# `conservative_fill_price_micros` mirrors the Rust function BIT-FOR-BIT:
# integer-micros arithmetic, same truncating-division order of operations,
# same saturation bounds. Research bars ultimately originate from md_bars'
# *_micros integer columns (see bars_postgres.py MICROS_SCALE), so
# round(price * 1_000_000) exactly recovers the source integer -- this is
# the same convention used throughout core-rs and mqk-*-bridge tooling.
#
# Two named, versioned pricing models:
#   - EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1: the official parity
#     model (BUY @ HIGH, SELL @ LOW, + slippage/volatility adjustment).
#     Requires high/low; see economic_walkforward.ExecutionPricingSpec.
#   - EXECUTION_PRICING_MODEL_ID_CLOSE_ONLY_DIAGNOSTIC_V1: the pre-P7A
#     legacy behavior (no adverse-price cost added at all; cost is entirely
#     CostModelSpec's flat symmetric bps). Diagnostic/non-promotion-grade --
#     see economic_registry_integration.require_official_execution_pricing_parity.

MICROS_SCALE = 1_000_000
I64_MAX = (1 << 63) - 1

EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1 = "rust_conservative_bar_range_v1"
EXECUTION_PRICING_MODEL_ID_CLOSE_ONLY_DIAGNOSTIC_V1 = "close_only_diagnostic_v1"

KNOWN_EXECUTION_PRICING_MODEL_IDS = frozenset(
    {
        EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
        EXECUTION_PRICING_MODEL_ID_CLOSE_ONLY_DIAGNOSTIC_V1,
    }
)

Side = Literal["buy", "sell"]


def to_micros(price: float) -> int:
    """Round-trip-exact dollars -> micros conversion (see module docstring)."""
    return int(round(float(price) * MICROS_SCALE))


def from_micros(price_micros: int) -> float:
    return float(price_micros) / MICROS_SCALE


def conservative_fill_price_micros(
    *,
    high_micros: int,
    low_micros: int,
    close_micros: int,
    side: Side,
    slippage_bps: int,
    volatility_mult_bps: int,
) -> int:
    """Bit-for-bit mirror of Rust's `BacktestEngine::conservative_fill_price`
    (core-rs/crates/mqk-backtest/src/engine.rs). BUY fills at HIGH (worst
    case for a buyer), SELL fills at LOW (worst case for a seller), then an
    adverse slippage adjustment on top:

        bar_spread_bps         = (high - low) * 10_000 / close
        vol_component           = bar_spread_bps * volatility_mult_bps / 10_000
        effective_slippage_bps  = slippage_bps + vol_component
        BUY:  min(base + base * effective_slippage_bps / 10_000, i64::MAX)
        SELL: max(base - base * effective_slippage_bps / 10_000, 0)

    All division here is Python integer floor-division (`//`), which is
    equivalent to Rust's truncating i64/i128 division for the non-negative
    operands this function requires (see validation below) -- truncation
    and floor division only diverge for negative operands.

    STRENGTHENING BEYOND RUST: Rust's `conservative_fill_price` does not
    itself validate that `high_micros >= low_micros` or that `close_micros`
    lies within `[low_micros, high_micros]` (confirmed by direct reading of
    engine.rs and `BacktestBar::new` -- no such check exists anywhere in the
    Rust backtest engine). Per CLAUDE.md's fail-closed philosophy, Python's
    OFFICIAL registered evaluation must not silently trust an impossible bar
    shape merely because the Rust reference implementation happens not to
    check it -- see bars_provenance.BarShapeInvalid / the mission's REQUIRED
    NEGATIVE TEST #11. This validation only ever REJECTS additional inputs
    Rust would have accepted; for every input both accept, the arithmetic
    below is identical.
    """
    if side not in ("buy", "sell"):
        raise ValueError(f"side must be 'buy' or 'sell', got {side!r}")
    if slippage_bps < 0:
        raise ValueError(
            f"negative slippage rejected: slippage_bps={slippage_bps} (must be >= 0; negative "
            "values produce favorable fills and are forbidden)"
        )
    if volatility_mult_bps < 0:
        raise ValueError(
            f"negative slippage rejected: volatility_mult_bps={volatility_mult_bps} (must be >= 0; "
            "negative values produce favorable fills and are forbidden)"
        )
    if close_micros <= 0:
        raise ValueError(f"close_micros must be > 0, got {close_micros}")
    if high_micros < low_micros:
        raise ValueError(
            f"impossible bar shape: high_micros={high_micros} < low_micros={low_micros}"
        )
    if not (low_micros <= close_micros <= high_micros):
        raise ValueError(
            f"impossible bar shape: close_micros={close_micros} outside "
            f"[low_micros={low_micros}, high_micros={high_micros}]"
        )

    base = high_micros if side == "buy" else low_micros

    bar_spread_bps = (high_micros - low_micros) * 10_000 // close_micros
    vol_component = bar_spread_bps * volatility_mult_bps // 10_000
    effective_slippage_bps = slippage_bps + vol_component
    if effective_slippage_bps == 0:
        return base

    adjustment = (base * effective_slippage_bps) // 10_000
    if side == "buy":
        return min(base + adjustment, I64_MAX)
    return max(base - adjustment, 0)


def conservative_fill_price(
    *,
    high: float,
    low: float,
    close: float,
    side: Side,
    slippage_bps: int,
    volatility_mult_bps: int,
) -> float:
    """Float (dollars) convenience wrapper around
    `conservative_fill_price_micros` for callers holding decimal prices --
    round-trips through micros so the result is bit-for-bit consistent with
    the integer path (and therefore with Rust)."""
    fill_micros = conservative_fill_price_micros(
        high_micros=to_micros(high),
        low_micros=to_micros(low),
        close_micros=to_micros(close),
        side=side,
        slippage_bps=slippage_bps,
        volatility_mult_bps=volatility_mult_bps,
    )
    return from_micros(fill_micros)


@dataclass(frozen=True)
class ExecutionPricingSpec:
    """Versioned execution-pricing protocol choice for the economic
    evaluator (P7A). The default reproduces pre-P7A behavior exactly (no
    adverse-price cost is added anywhere; see
    EXECUTION_PRICING_MODEL_ID_CLOSE_ONLY_DIAGNOSTIC_V1) so existing
    diagnostic/test callers are unaffected unless they opt in.

    `slippage_bps`/`volatility_mult_bps` are unused (and therefore not
    identity-bearing beyond their own presence) under the diagnostic model;
    under the official model they must mirror the Rust `StressProfile`
    values a corresponding backtest run would use, and are always
    identity-bearing (see economic_walkforward.economic_protocol_identity).

    NOTE: when `pricing_model_id` is the official model, the caller's
    `CostModelSpec.slippage_bps_per_side` must be 0 -- slippage is then
    modeled entirely via directional bar-range pricing here, and charging
    both would double-count it (enforced fail-closed by
    EconomicWalkForwardSpec.normalized()).
    """

    pricing_model_id: str = EXECUTION_PRICING_MODEL_ID_CLOSE_ONLY_DIAGNOSTIC_V1
    slippage_bps: int = 0
    volatility_mult_bps: int = 0
    schema_version: str = "economic_execution_pricing_v1"

    def normalized(self) -> "ExecutionPricingSpec":
        if self.pricing_model_id not in KNOWN_EXECUTION_PRICING_MODEL_IDS:
            raise ValueError(f"unsupported execution pricing_model_id: {self.pricing_model_id!r}")
        slippage_bps = int(self.slippage_bps)
        volatility_mult_bps = int(self.volatility_mult_bps)
        if slippage_bps < 0:
            raise ValueError(
                f"negative slippage rejected: slippage_bps={slippage_bps} (must be >= 0; negative "
                "values produce favorable fills and are forbidden)"
            )
        if volatility_mult_bps < 0:
            raise ValueError(
                f"negative slippage rejected: volatility_mult_bps={volatility_mult_bps} (must be "
                ">= 0; negative values produce favorable fills and are forbidden)"
            )
        return replace(self, slippage_bps=slippage_bps, volatility_mult_bps=volatility_mult_bps)

    @property
    def is_official_parity_model(self) -> bool:
        return self.pricing_model_id == EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1
