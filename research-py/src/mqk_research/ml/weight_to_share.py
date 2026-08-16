from __future__ import annotations

import math
from dataclasses import dataclass, replace
from typing import Any, Dict, List, Optional, Tuple

from mqk_research.ml.execution_pricing import to_micros

# P7B (RESEARCH-WEIGHT-TO-SHARE-PARITY-01)
#
# Pure, deterministic weight->share translation bridging Python Research's
# CONTINUOUS portfolio-weight economics (economic_walkforward.py) and Rust's
# discrete `TargetPosition { symbol, qty: i64 }` / `qty: i64` execution
# boundary (core-rs/crates/mqk-execution/src/types.rs,
# core-rs/crates/mqk-backtest/src/types.rs::StrategySizingConfig).
#
# THE GAP (see docs/research/Research_Backtest_V1_Closeout_Audit.md, Area
# K1/K7): Python emits continuous target weights (equal-weight-active across
# active symbols); Rust's native strategies decide an ABSOLUTE, PRICE-
# INDEPENDENT target_qty at signal time (StrategySizingConfig.target_qty is a
# fixed integer, e.g. 1 -- see mqk-strategy/src/engines/intraday_scalper.rs).
# There is no existing Rust seam that converts a *weight* into a *qty* at
# signal time using a price, because native Rust strategies never need to --
# their target_qty never depends on price at all.
#
# WHAT THIS MODULE REUSES FROM THE ACCEPTED RUST SEAMS (not invented):
#
#   1. Price basis + rounding formula: IntradayScalperStrategy::apply_caps's
#      MAX_POSITION_NOTIONAL_USD cap conversion --
#          max_qty_from_notional = (max_notional_usd * 1_000_000) / last_close_micros
#      (integer floor division against the LAST KNOWN bar's close in micros;
#      fails closed to qty=0 if that close is <= 0 -- "cannot compute
#      notional without a price reference"). This is the ONLY place in the
#      accepted Rust codebase that converts a USD notional into a discrete
#      share count from a price, so it is the authoritative formula this
#      module generalizes: `weight_to_target_qty` below computes
#      `notional_micros // price_micros` -- unit-identical to Rust's formula,
#      merely with `notional_micros` derived from `weight * equity_usd`
#      (via the same to_micros() round-trip convention execution_pricing.py
#      already established for bit-reproducible integer arithmetic) instead
#      of a fixed config constant.
#
#   2. Current-position -> order-delta convention:
#      mqk_execution::engine::targets_to_order_intents --
#          delta = target_qty - current_qty
#          delta == 0            -> no order
#          delta > 0             -> BUY delta
#          delta < 0             -> SELL -delta
#      (core-rs/crates/mqk-execution/src/engine.rs). Mirrored exactly by
#      `target_qty_to_order_delta` below.
#
# CAUSALITY (mission COST SCOPE / P7C hard-stop #2): the sizing PRICE is the
# executing row's OWN close -- the same close economic_walkforward.py already
# uses to compute that row's gross_return/turnover (see
# _row_execution_pricing_components's docstring: sizing happens at
# "signal-generation-adjacent" time, never at a later bar). This module never
# reads HIGH, LOW, or the P7A conservative fill price -- those remain P7A's
# execution-time-only authority for the ACTUAL FILL, entirely separate from
# the SIZING price used here to decide the target quantity. Mutating a bar
# strictly AFTER the row being sized cannot change that row's target_qty by
# construction (this module never looks ahead of the row it is given).
#
# EQUITY BASIS: economic_walkforward.py has no dollar equity/cash ledger --
# it works entirely in weight/return space (compounding is implicit via %
# returns, not tracked as micros). `WeightToShareSpec.equity_usd` is an
# explicit, FIXED (non-compounding) nominal basis -- a deliberate
# simplification, not a claim to model a real cash ledger (see CLAUDE.md
# Section 5D: "do not invent a fake broker cash model"). Its default,
# 100_000.0, mirrors BacktestConfig::test_defaults /
# BacktestConfig::conservative_defaults's own `initial_cash_micros =
# 100_000_000_000` (100k USD) -- the same repo-accepted default, not an
# invented number.
#
# WHAT THIS MODULE DOES NOT DO: it does not re-derive capacity/allocation
# decisions (max_gross_exposure, cohort deferral, reduce-first-defer-increase)
# -- those remain _simulate_fold_execution's frozen, accepted job (P7A/
# REPAIR-02/REPAIR-03). This module is a PURE, ADDITIVE, READ-ONLY
# translation applied AFTER that capacity-aware weight is already decided,
# turning each already-resolved `executed_weight` into the discrete share
# quantity a Rust TargetPosition carrying the same intent would need to
# reach an equivalent notional. Because whole-share floor rounding can only
# ever REDUCE magnitude relative to the continuous weight target (never
# increase it), the discrete series can never carry MORE gross notional than
# the continuous series already proved satisfies max_gross_exposure -- see
# GROSS EXPOSURE INVARIANT tests.
#
# OFFICIAL VS DIAGNOSTIC (mission Section 5F): there is exactly one protocol,
# WEIGHT_TO_SHARE_PROTOCOL_ID_V1. A candidate simply omitting
# `EconomicWalkForwardSpec.weight_to_share` (its default, None) is the
# diagnostic/legacy continuous-weight-only state and can never satisfy
# `economic_registry_integration.require_official_weight_to_share_parity`.
#
# P7B-REPAIR-01 (RESEARCH-WEIGHT-TO-SHARE-PARITY-01-REPAIR-01): two
# independent-review defects fixed against the SAME protocol_id (not a new
# protocol -- a repair to what weight_to_share_v1 was always documented to
# mean, mirroring how P7A-REPAIR-01 kept EXECUTION_PRICING_MODEL_ID_RUST_
# CONSERVATIVE_V1 unchanged):
#
#   1. SIGNAL-TIME SIZING: the sizing price is now genuinely
#      WEIGHT_TO_SHARE_PRICE_BASIS_V1 ("signal_row_close_v1") -- the close
#      at the moment the target WEIGHT was decided, not the close of a
#      later execution bar. This module never received a "later" price
#      itself (it only ever sees the single `price` its caller supplies);
#      the bug was entirely in economic_walkforward.py's caller passing the
#      execution row's close instead of the signal row's close (see that
#      module's docstring for the fix). This module now documents and
#      enforces the CONTRACT: `price` passed here must always be the
#      signal-time close, and target_qty computed here becomes permanently
#      fixed at that instant -- a caller must never re-derive target_qty
#      from a later bar for the same signal.
#
#   2. SIGNED TRANSLATION: `weight_to_target_qty` no longer rejects
#      negative weights. Rust's `TargetPosition { qty: i64 }` and
#      `targets_to_order_intents` were never long-only -- only
#      economic_walk_forward_v1's SignalPolicySpec(long_only=True) is (a
#      separate, still-frozen, still-long-only economic-signal-generation
#      layer -- see SignalPolicySpec). Restricting THIS translation layer
#      to reject negative weights removed capability Rust already has and
#      that RESEARCH-LONG-SHORT-ECONOMIC-POLICY-01 needs. Rounding is now
#      magnitude-floor-then-restore-sign (WEIGHT_TO_SHARE_ROUNDING_POLICY_V1
#      already documented this as "floor_toward_zero_magnitude_v1" --
#      the name anticipated signed magnitude semantics even before the
#      implementation supported them).

WEIGHT_TO_SHARE_PROTOCOL_ID_V1 = "weight_to_share_v1"
KNOWN_WEIGHT_TO_SHARE_PROTOCOL_IDS = frozenset({WEIGHT_TO_SHARE_PROTOCOL_ID_V1})

# P7B-REPAIR-01 (mission Section 4G/4H): explicit, versioned evidence that
# discrete signed target quantities actually drove the economic result for
# a given fold/run, distinct from the weight_to_share_protocol_id itself
# (which only asserts the TRANSLATION exists, not that it was economically
# load-bearing). Set by economic_walkforward.py's per-fold summary whenever
# spec.weight_to_share is engaged; a caller checking for "official parity"
# evidence must look for this marker, not merely for a non-None
# weight_to_share_protocol_id.
DISCRETE_ECONOMICS_PROTOCOL_ID_V1 = "discrete_share_economic_path_v1"

# Fixed-by-protocol-version assumptions: not independently configurable in
# V1 (baked into WEIGHT_TO_SHARE_PROTOCOL_ID_V1 itself, exactly as
# execution_pricing.py's pricing_model_id bakes in its own fixed formula --
# only equity_usd/max_target_qty/max_position_notional_usd genuinely vary
# here). Named as constants -- and echoed by
# `weight_to_share_protocol_identity` -- purely for audit/documentation
# clarity; a future V2 protocol_id would be required to change any of them.
WEIGHT_TO_SHARE_ROUNDING_POLICY_V1 = "floor_toward_zero_magnitude_v1"
WEIGHT_TO_SHARE_LOT_SIZE_V1 = 1
WEIGHT_TO_SHARE_PRICE_BASIS_V1 = "signal_row_close_v1"


@dataclass(frozen=True)
class WeightToShareSpec:
    """Versioned weight->share translation protocol choice (P7B). Whole-share,
    US equity/ETF V1. SIGNED (P7B-REPAIR-01): a positive weight produces a
    positive (long) target_qty, a negative weight produces a negative
    (short) target_qty, mirroring Rust's signed `TargetPosition.qty: i64`
    exactly -- this translator itself does not enforce long-only. Whether a
    caller ever actually generates a negative weight is a SEPARATE, higher
    layer decision (economic_walk_forward_v1's SignalPolicySpec is still
    long-only; RESEARCH-LONG-SHORT-ECONOMIC-POLICY-01 is the first caller
    that can produce negative weights).

    `equity_usd`: fixed (non-compounding) nominal portfolio-equity basis in
    USD used as the sizing denominator (`notional = weight * equity_usd`).
    Always identity-bearing -- changing it changes generated quantities.

    `max_target_qty` / `max_position_notional_usd`: optional caps, same
    field names and same semantics as Rust's
    `mqk_backtest::StrategySizingConfig` (`target_qty` cap) and
    `IntradayScalperStrategy`'s `MQK_STRATEGY_MAX_POSITION_NOTIONAL_USD`
    (notional cap) -- `None` (the default) means uncapped, exactly like
    those Rust defaults.
    """

    protocol_id: str = WEIGHT_TO_SHARE_PROTOCOL_ID_V1
    equity_usd: float = 100_000.0
    max_target_qty: Optional[int] = None
    max_position_notional_usd: Optional[float] = None
    schema_version: str = "weight_to_share_v1"

    def normalized(self) -> "WeightToShareSpec":
        if self.protocol_id not in KNOWN_WEIGHT_TO_SHARE_PROTOCOL_IDS:
            raise ValueError(f"unsupported weight_to_share protocol_id: {self.protocol_id!r}")

        equity_usd = float(self.equity_usd)
        if not math.isfinite(equity_usd) or equity_usd <= 0.0:
            raise ValueError(f"equity_usd must be a positive finite value, got {self.equity_usd!r}")

        max_target_qty = self.max_target_qty
        if max_target_qty is not None:
            max_target_qty = int(max_target_qty)
            if max_target_qty <= 0:
                raise ValueError(f"max_target_qty must be > 0 when set, got {max_target_qty}")

        max_position_notional_usd = self.max_position_notional_usd
        if max_position_notional_usd is not None:
            max_position_notional_usd = float(max_position_notional_usd)
            if not math.isfinite(max_position_notional_usd) or max_position_notional_usd <= 0.0:
                raise ValueError(
                    "max_position_notional_usd must be a positive finite value when set, "
                    f"got {self.max_position_notional_usd!r}"
                )

        return replace(
            self,
            equity_usd=equity_usd,
            max_target_qty=max_target_qty,
            max_position_notional_usd=max_position_notional_usd,
        )


def weight_to_share_protocol_identity(spec: Optional[WeightToShareSpec]) -> Dict[str, Any]:
    """Canonical, result-independent identity fragment. `spec=None` (the
    diagnostic/legacy continuous-weight-only state) always produces the same
    `{"weight_to_share_protocol_id": None}` fragment regardless of any other
    economic-spec field -- consumed by
    economic_walkforward.economic_protocol_identity /
    economic_registry_integration.build_economic_trial_identity."""
    if spec is None:
        return {"weight_to_share_protocol_id": None}
    normalized = spec.normalized()
    return {
        "weight_to_share_protocol_id": normalized.protocol_id,
        "rounding_policy": WEIGHT_TO_SHARE_ROUNDING_POLICY_V1,
        "lot_size": WEIGHT_TO_SHARE_LOT_SIZE_V1,
        "price_basis": WEIGHT_TO_SHARE_PRICE_BASIS_V1,
        "equity_usd": normalized.equity_usd,
        "max_target_qty": normalized.max_target_qty,
        "max_position_notional_usd": normalized.max_position_notional_usd,
    }


def weight_to_target_qty(*, weight: float, price: Optional[float], spec: WeightToShareSpec) -> int:
    """Deterministic SIGNED whole-share translation (P7B-REPAIR-01): a target
    WEIGHT (signed fraction of `spec.equity_usd`) becomes a signed discrete
    target share count -- positive weight -> positive (long) qty, negative
    weight -> negative (short) qty, zero -> zero.

    `price` MUST be the SIZING price basis at SIGNAL TIME -- the close of
    the bar in effect at the moment this target weight was decided, known
    when this target is generated (see module docstring CAUSALITY/SIGNAL-
    TIME SIZING). It is NEVER a later execution bar's
    close/HIGH/LOW/P7A-fill price; the qty this function returns must be
    treated as PERMANENTLY FIXED once computed -- a caller must never call
    this again for the same signal with a different (later) price.

    `weight == 0.0` always returns `0` WITHOUT requiring a valid price (a
    full flatten needs no price reference, mirroring how P7A's forced-
    flatten exception never touches execution_price_cost either). Any other
    `weight` requires `price` to be a positive finite value -- fails closed
    (raises) otherwise, mirroring `_row_execution_pricing_components`'s
    identical fail-closed-on-missing-price-for-an-executing-event pattern.

    Formula (see module docstring for the Rust precedent this generalizes;
    WEIGHT_TO_SHARE_ROUNDING_POLICY_V1 = "floor_toward_zero_magnitude_v1"):
        magnitude_notional_micros = to_micros(abs(weight) * spec.equity_usd)
        price_micros               = to_micros(price)
        qty_magnitude = magnitude_notional_micros // price_micros  # floor
        qty = sign(weight) * qty_magnitude
    then `qty_magnitude` is optionally capped by `max_target_qty` /
    `max_position_notional_usd` (same min()-style clamping Rust's
    `apply_caps` uses -- caps bound MAGNITUDE regardless of side) before the
    sign is restored. `+1.9` theoretical shares -> `+1` (never `+2`); `-1.9`
    theoretical shares -> `-1` (never `-2`) -- truncation toward zero in
    signed quantity, not toward negative infinity.
    """
    spec = spec.normalized()
    weight = float(weight)
    if not math.isfinite(weight):
        raise ValueError(f"weight_to_target_qty: weight must be finite, got {weight!r}")
    if weight == 0.0:
        return 0

    if price is None or not math.isfinite(price) or price <= 0.0:
        raise RuntimeError(
            f"Fail-closed: {WEIGHT_TO_SHARE_PROTOCOL_ID_V1} requires a positive finite sizing "
            f"price for a nonzero target weight (weight={weight!r}, price={price!r})"
        )

    sign = 1 if weight > 0.0 else -1
    price_micros = to_micros(price)
    magnitude_notional_micros = to_micros(abs(weight) * spec.equity_usd)
    qty_magnitude = magnitude_notional_micros // price_micros

    if spec.max_target_qty is not None:
        qty_magnitude = min(qty_magnitude, spec.max_target_qty)

    if spec.max_position_notional_usd is not None:
        cap_notional_micros = to_micros(spec.max_position_notional_usd)
        qty_magnitude = min(qty_magnitude, cap_notional_micros // price_micros)

    qty_magnitude = max(qty_magnitude, 0)
    return int(sign * qty_magnitude)


def target_qty_to_order_delta(*, current_qty: int, target_qty: int) -> Optional[Tuple[str, int]]:
    """Mirrors mqk_execution::engine::targets_to_order_intents's delta
    convention exactly (core-rs/crates/mqk-execution/src/engine.rs):
    `delta = target_qty - current_qty`; `None` (no order) if zero; otherwise
    `("buy", delta)` if positive or `("sell", -delta)` if negative."""
    delta = int(target_qty) - int(current_qty)
    if delta == 0:
        return None
    return ("buy", delta) if delta > 0 else ("sell", -delta)


def translate_symbol_weight_series_to_share_events(
    rows: List[Dict[str, Any]],
    spec: WeightToShareSpec,
) -> List[Dict[str, Any]]:
    """Per-symbol, chronologically-ordered driver: given `rows` (one dict per
    already-resolved execution row, each carrying `timestamp`,
    `executed_weight`, and an optional `close` sizing price -- e.g. the
    `rows_by_symbol[s]` shape economic_walkforward._simulate_fold_execution
    already produces), thread a `current_qty` state starting at 0 (matches
    Rust's own `current_qty.get(&sym).unwrap_or(&0)` bootstrapping) and emit
    one share-translation event per row.

    Pure and stateless across calls: given the same `rows` and `spec`, always
    produces the same output (REQUIRED TEST 1). Never inspects any row after
    the one currently being translated, so mutating a LATER row can never
    change an EARLIER row's `target_qty` (REQUIRED TEST 11 -- no future-bar
    leakage). Symbols are independent; multi-symbol ordering is the caller's
    responsibility (this function only ever sees one symbol's own
    chronological rows) -- see economic_walkforward._simulate_fold's driver,
    which iterates `symbols` in the fold's existing deterministic (sorted)
    order (REQUIRED TEST 7).
    """
    spec = spec.normalized()
    events: List[Dict[str, Any]] = []
    current_qty = 0
    for row in rows:
        weight = float(row["executed_weight"])
        close = row.get("close")
        price = None if close is None else float(close)
        target_qty = weight_to_target_qty(weight=weight, price=price, spec=spec)
        order = target_qty_to_order_delta(current_qty=current_qty, target_qty=target_qty)
        side, qty = (order[0], order[1]) if order is not None else (None, 0)
        events.append(
            {
                "timestamp": row["timestamp"],
                "close": price,
                "weight": weight,
                "target_qty": target_qty,
                "current_qty_before": current_qty,
                "side": side,
                "qty": qty,
            }
        )
        current_qty = target_qty
    return events
