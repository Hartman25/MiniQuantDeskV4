//! V4-BULK-CODE-COMPLETION-STAGE-B M8 (frozen V4 asset matrix): the frozen
//! options permission set, as a pure structural classifier.
//!
//! # Frozen scope (operator-approved, V4 asset matrix)
//!
//! ALLOWED:
//! - long call
//! - long put
//! - covered call
//! - cash-secured put
//! - defined-risk call vertical spread
//! - defined-risk put vertical spread
//!
//! NOT ALLOWED (never a variant of [`OptionStrategyStructure`], never
//! producible by [`classify_option_strategy_structure`]):
//! - naked short calls
//! - naked short puts
//! - unlimited-risk option structures
//! - arbitrary complex/multi-leg strategies outside defined-risk verticals
//!
//! This module is a **model-only structural classifier**, mirroring the
//! established precedent of `types::BracketLegs` / `asset_risk_policy.rs`:
//! it has zero production callers, does not touch `BrokerGateway`, does not
//! construct or submit any order, and does not read live market data.
//! `classify_option_strategy_structure` answers exactly one question —
//! "does this proposed leg set structurally match one of the six frozen
//! permitted shapes, and if so what is its structural worst-case loss
//! bound?" — and refuses (with a named, closed-vocabulary reason) anything
//! else, most importantly a short call/put with no proven coverage or
//! collateral. It never computes live P&L, Greeks, margin, or assignment
//! risk; those require real chain/price/account data and are out of scope
//! for a pure classifier.
//!
//! `spread_width_exercise_bound_micros` on the two vertical-spread variants
//! is a **structural exercise-width bound**, not a complete max-loss figure:
//! for any single-ratio (1 long leg vs. 1 short leg, same
//! underlying/expiry/right, distinct strikes) vertical,
//! `|strike_a - strike_b| * multiplier` is the maximum possible payoff swing
//! between the two strikes at expiry — a structural options-theory fact,
//! not a live-priced estimate, so it can be computed here with zero market
//! data. IR-B1-02: this is deliberately NOT named `max_loss_upper_bound`
//! because it does not net the premium paid or received for the spread; the
//! true economic max loss (or max gain, for a net-credit spread) requires
//! that premium/cost evidence, which belongs in a priced risk seam, not
//! this zero-market-data structural classifier.
//!
//! IR-B1-02: covered-call and cash-secured-put coverage/collateral adequacy
//! is checked exactly, not merely-positive:
//! - Covered call: `covering_share_qty` must be `>=` the short call's
//!   contract quantity times its multiplier (shares actually owed on
//!   assignment).
//! - Cash-secured put: `cash_secured_collateral_micros` must be `>=` the
//!   full conservative exercise obligation (`strike * multiplier *
//!   contracts`) — never inferred from a hoped-for future premium credit.

use mqk_schemas::{OptionRight, QtyMicros, QTY_MICROS_SCALE};

/// One proposed options leg, before permission classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionLegSide {
    Long,
    Short,
}

/// One proposed options leg. Pure data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedOptionLeg {
    pub side: OptionLegSide,
    pub right: OptionRight,
    pub strike_micros: i64,
    pub expiry_yyyymmdd: String,
    pub qty: QtyMicros,
    pub multiplier: i32,
}

/// A full proposed options strategy: one underlying, 1-2 legs, plus the
/// optional stock-coverage/cash-collateral context a covered call / cash-
/// secured put needs to be distinguished from a naked short.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedOptionStrategy {
    pub underlying: String,
    pub legs: Vec<ProposedOptionLeg>,
    /// Long shares of `underlying` proposed to cover a single short-call leg.
    /// Only read when `legs` is exactly one short call.
    pub covering_share_qty: Option<QtyMicros>,
    /// Cash collateral proposed to secure a single short-put leg. Only read
    /// when `legs` is exactly one short put.
    pub cash_secured_collateral_micros: Option<i64>,
}

/// A structurally classified, frozen-permission-set options strategy.
///
/// Every variant here is one of the six frozen-allowed V4 option
/// structures. There is no `Other`/`Unsupported` variant carrying a naked
/// short or unlimited-risk shape — those are refused, never classified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionStrategyStructure {
    LongCall(ProposedOptionLeg),
    LongPut(ProposedOptionLeg),
    CoveredCall {
        short_call: ProposedOptionLeg,
        covering_share_qty: QtyMicros,
    },
    CashSecuredPut {
        short_put: ProposedOptionLeg,
        collateral_cash_micros: i64,
    },
    CallVerticalSpread {
        long_leg: ProposedOptionLeg,
        short_leg: ProposedOptionLeg,
        /// Structural exercise-width bound, NOT a complete max-loss figure —
        /// see the module doc.
        spread_width_exercise_bound_micros: i128,
    },
    PutVerticalSpread {
        long_leg: ProposedOptionLeg,
        short_leg: ProposedOptionLeg,
        /// Structural exercise-width bound, NOT a complete max-loss figure —
        /// see the module doc.
        spread_width_exercise_bound_micros: i128,
    },
}

impl OptionStrategyStructure {
    /// Machine-readable structure name, matching the frozen vocabulary.
    pub fn structure_code(&self) -> &'static str {
        match self {
            Self::LongCall(_) => "long_call",
            Self::LongPut(_) => "long_put",
            Self::CoveredCall { .. } => "covered_call",
            Self::CashSecuredPut { .. } => "cash_secured_put",
            Self::CallVerticalSpread { .. } => "call_vertical_spread",
            Self::PutVerticalSpread { .. } => "put_vertical_spread",
        }
    }
}

/// Closed-vocabulary refusal reason. Every reachable non-frozen shape has a
/// named reason here — there is no catch-all "invalid" bucket that could
/// hide a genuinely new, unreviewed structure under a vague label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionStrategyRefusal {
    NoLegs,
    TooManyLegs {
        leg_count: usize,
    },
    EmptyUnderlying,
    InvalidLeg {
        detail: String,
    },
    /// A single short call with no proven share coverage, or with
    /// `covering_share_qty` strictly less than the shares owed on
    /// assignment (`short_call.qty * multiplier`). IR-B1-02: covers both
    /// "no coverage offered" and "coverage offered but insufficient" — a
    /// partially covered short call is still not a proven covered call, so
    /// it is never silently reclassified as anything else.
    NakedShortCall,
    /// A single short put with no proven cash collateral, or with
    /// `cash_secured_collateral_micros` strictly less than the conservative
    /// exercise obligation (`strike * multiplier * contracts`). IR-B1-02:
    /// covers both "no collateral offered" and "collateral offered but
    /// insufficient" — never inferred as adequate from a hoped-for future
    /// premium credit.
    NakedShortPut,
    /// IR-B1-02: the covered-call coverage-requirement arithmetic
    /// (`short_call.qty * multiplier`) overflowed `i64`. Fails closed rather
    /// than wrapping or silently truncating.
    CoveredCallCoverageOverflow,
    /// IR-B1-02: the cash-secured-put exercise-obligation arithmetic
    /// (`strike * multiplier * contracts`) overflowed `i128`, or the
    /// contract quantity was not a whole number of contracts (a fractional
    /// options contract has no well-defined cash obligation). Fails closed
    /// rather than wrapping, truncating, or rounding.
    CashSecuredPutObligationOverflow,
    SpreadLegsMustShareRightAndExpiry,
    SpreadLegsMustBeOppositeDirection,
    SpreadLegsMustHaveEqualQuantity,
    SpreadLegsMustHaveEqualMultiplier,
    SpreadLegsMustHaveDistinctStrikes,
    /// Structural width-bound arithmetic could not be represented even in
    /// `i128` (never silently saturated/truncated).
    SpreadWidthBoundOverflow,
    /// Any leg count/side/right combination outside the six frozen shapes
    /// (e.g. two long legs, three legs, a naked combination straddle).
    /// Never implemented, per the frozen "NOT ALLOWED" list.
    UnsupportedStructure,
}

impl OptionStrategyRefusal {
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::NoLegs => "no_legs",
            Self::TooManyLegs { .. } => "too_many_legs",
            Self::EmptyUnderlying => "empty_underlying",
            Self::InvalidLeg { .. } => "invalid_leg",
            Self::NakedShortCall => "naked_short_call_refused",
            Self::NakedShortPut => "naked_short_put_refused",
            Self::CoveredCallCoverageOverflow => "covered_call_coverage_overflow",
            Self::CashSecuredPutObligationOverflow => "cash_secured_put_obligation_overflow",
            Self::SpreadLegsMustShareRightAndExpiry => "spread_legs_must_share_right_and_expiry",
            Self::SpreadLegsMustBeOppositeDirection => "spread_legs_must_be_opposite_direction",
            Self::SpreadLegsMustHaveEqualQuantity => "spread_legs_must_have_equal_quantity",
            Self::SpreadLegsMustHaveEqualMultiplier => "spread_legs_must_have_equal_multiplier",
            Self::SpreadLegsMustHaveDistinctStrikes => "spread_legs_must_have_distinct_strikes",
            Self::SpreadWidthBoundOverflow => "spread_width_bound_overflow",
            Self::UnsupportedStructure => "unsupported_structure_refused",
        }
    }
}

fn validate_leg(leg: &ProposedOptionLeg) -> Result<(), OptionStrategyRefusal> {
    if leg.strike_micros <= 0 {
        return Err(OptionStrategyRefusal::InvalidLeg {
            detail: "strike_micros must be positive".to_string(),
        });
    }
    if !mqk_schemas::is_canonical_yyyymmdd(&leg.expiry_yyyymmdd) {
        return Err(OptionStrategyRefusal::InvalidLeg {
            detail: "expiry_yyyymmdd must be a real YYYYMMDD calendar date".to_string(),
        });
    }
    if leg.qty.raw() <= 0 {
        return Err(OptionStrategyRefusal::InvalidLeg {
            detail: "qty must be positive".to_string(),
        });
    }
    if leg.multiplier <= 0 {
        return Err(OptionStrategyRefusal::InvalidLeg {
            detail: "multiplier must be positive".to_string(),
        });
    }
    Ok(())
}

/// Structural exercise-width bound for a single-ratio vertical spread:
/// `|strike_a - strike_b| * multiplier`, checked in `i128`. True regardless
/// of net debit/credit and regardless of call/put — see module docs. NOT a
/// complete max-loss figure (premium is not netted) — see module docs.
fn vertical_spread_width_bound_micros(
    long_leg: &ProposedOptionLeg,
    short_leg: &ProposedOptionLeg,
) -> Result<i128, OptionStrategyRefusal> {
    let width = (long_leg.strike_micros as i128 - short_leg.strike_micros as i128).abs();
    width
        .checked_mul(long_leg.multiplier as i128)
        .ok_or(OptionStrategyRefusal::SpreadWidthBoundOverflow)
}

/// IR-B1-02: shares owed on assignment of a single short-call leg —
/// `short_call.qty * multiplier`, expressed as a raw `QtyMicros` share
/// count (checked in `i64`; both operands are already at `QtyMicros`'s
/// 1e-6 scale, so `qty.raw() * multiplier` is directly the raw share-micros
/// value, no rescaling needed). `None` on overflow.
fn covered_call_required_coverage_raw(short_call: &ProposedOptionLeg) -> Option<i64> {
    short_call
        .qty
        .raw()
        .checked_mul(short_call.multiplier as i64)
}

/// IR-B1-02: the conservative cash exercise obligation of a single
/// short-put leg — `strike * multiplier * contracts`, in micro-dollars.
/// Checked in `i128` to absorb the intermediate product before dividing
/// back out `QtyMicros`'s 1e-6 scale. `None` on overflow, or if `qty` is
/// not a whole number of contracts (a fractional options contract has no
/// well-defined cash obligation — never silently rounded).
fn cash_secured_put_required_obligation_micros(short_put: &ProposedOptionLeg) -> Option<i128> {
    let strike = short_put.strike_micros as i128;
    let multiplier = short_put.multiplier as i128;
    let qty_raw = short_put.qty.raw() as i128;
    let scale = QTY_MICROS_SCALE as i128;
    let numerator = strike.checked_mul(multiplier)?.checked_mul(qty_raw)?;
    if numerator % scale != 0 {
        return None;
    }
    numerator.checked_div(scale)
}

/// Classify a proposed options strategy against the frozen V4 permission
/// set. `Ok` only ever carries one of the six allowed structures; every
/// other shape — most importantly any naked short or any structure with
/// more than two legs — is refused with a specific, named reason.
pub fn classify_option_strategy_structure(
    proposed: &ProposedOptionStrategy,
) -> Result<OptionStrategyStructure, OptionStrategyRefusal> {
    if proposed.underlying.trim().is_empty() {
        return Err(OptionStrategyRefusal::EmptyUnderlying);
    }

    for leg in &proposed.legs {
        validate_leg(leg)?;
    }

    let structure = match proposed.legs.as_slice() {
        [] => Err(OptionStrategyRefusal::NoLegs),
        [leg] => classify_single_leg(leg, proposed),
        [leg_a, leg_b] => classify_two_legs(leg_a, leg_b),
        legs => Err(OptionStrategyRefusal::TooManyLegs {
            leg_count: legs.len(),
        }),
    }?;

    // Options trade in whole contracts. Shapes whose own arithmetic already
    // refuses a non-whole quantity keep their specific refusal above; every
    // shape that would otherwise be permitted is refused here instead of
    // being classified with a fractional contract count.
    if proposed.legs.iter().any(|leg| !leg.qty.is_whole()) {
        return Err(OptionStrategyRefusal::InvalidLeg {
            detail: "qty must be a whole number of contracts".to_string(),
        });
    }
    Ok(structure)
}

fn classify_single_leg(
    leg: &ProposedOptionLeg,
    proposed: &ProposedOptionStrategy,
) -> Result<OptionStrategyStructure, OptionStrategyRefusal> {
    match (leg.side, leg.right) {
        (OptionLegSide::Long, OptionRight::Call) => {
            Ok(OptionStrategyStructure::LongCall(leg.clone()))
        }
        (OptionLegSide::Long, OptionRight::Put) => {
            Ok(OptionStrategyStructure::LongPut(leg.clone()))
        }
        (OptionLegSide::Short, OptionRight::Call) => {
            let required_raw = covered_call_required_coverage_raw(leg)
                .ok_or(OptionStrategyRefusal::CoveredCallCoverageOverflow)?;
            match proposed.covering_share_qty {
                Some(covering_share_qty) if covering_share_qty.raw() >= required_raw => {
                    Ok(OptionStrategyStructure::CoveredCall {
                        short_call: leg.clone(),
                        covering_share_qty,
                    })
                }
                _ => Err(OptionStrategyRefusal::NakedShortCall),
            }
        }
        (OptionLegSide::Short, OptionRight::Put) => {
            let required_micros = cash_secured_put_required_obligation_micros(leg)
                .ok_or(OptionStrategyRefusal::CashSecuredPutObligationOverflow)?;
            match proposed.cash_secured_collateral_micros {
                Some(collateral_cash_micros)
                    if (collateral_cash_micros as i128) >= required_micros =>
                {
                    Ok(OptionStrategyStructure::CashSecuredPut {
                        short_put: leg.clone(),
                        collateral_cash_micros,
                    })
                }
                _ => Err(OptionStrategyRefusal::NakedShortPut),
            }
        }
    }
}

fn classify_two_legs(
    leg_a: &ProposedOptionLeg,
    leg_b: &ProposedOptionLeg,
) -> Result<OptionStrategyStructure, OptionStrategyRefusal> {
    if leg_a.right != leg_b.right || leg_a.expiry_yyyymmdd != leg_b.expiry_yyyymmdd {
        return Err(OptionStrategyRefusal::SpreadLegsMustShareRightAndExpiry);
    }
    if leg_a.side == leg_b.side {
        return Err(OptionStrategyRefusal::SpreadLegsMustBeOppositeDirection);
    }
    if leg_a.qty != leg_b.qty {
        return Err(OptionStrategyRefusal::SpreadLegsMustHaveEqualQuantity);
    }
    if leg_a.multiplier != leg_b.multiplier {
        return Err(OptionStrategyRefusal::SpreadLegsMustHaveEqualMultiplier);
    }
    if leg_a.strike_micros == leg_b.strike_micros {
        return Err(OptionStrategyRefusal::SpreadLegsMustHaveDistinctStrikes);
    }

    let (long_leg, short_leg) = match (leg_a.side, leg_b.side) {
        (OptionLegSide::Long, OptionLegSide::Short) => (leg_a, leg_b),
        (OptionLegSide::Short, OptionLegSide::Long) => (leg_b, leg_a),
        _ => unreachable!("opposite-direction check above already excludes same-side pairs"),
    };

    let spread_width_exercise_bound_micros =
        vertical_spread_width_bound_micros(long_leg, short_leg)?;

    match long_leg.right {
        OptionRight::Call => Ok(OptionStrategyStructure::CallVerticalSpread {
            long_leg: long_leg.clone(),
            short_leg: short_leg.clone(),
            spread_width_exercise_bound_micros,
        }),
        OptionRight::Put => Ok(OptionStrategyStructure::PutVerticalSpread {
            long_leg: long_leg.clone(),
            short_leg: short_leg.clone(),
            spread_width_exercise_bound_micros,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leg(side: OptionLegSide, right: OptionRight, strike_micros: i64) -> ProposedOptionLeg {
        ProposedOptionLeg {
            side,
            right,
            strike_micros,
            expiry_yyyymmdd: "20261218".to_string(),
            qty: QtyMicros::new(1_000_000),
            multiplier: 100,
        }
    }

    fn strategy(legs: Vec<ProposedOptionLeg>) -> ProposedOptionStrategy {
        ProposedOptionStrategy {
            underlying: "SPY".to_string(),
            legs,
            covering_share_qty: None,
            cash_secured_collateral_micros: None,
        }
    }

    #[test]
    fn long_call_is_allowed() {
        let s = strategy(vec![leg(
            OptionLegSide::Long,
            OptionRight::Call,
            500_000_000,
        )]);
        let result = classify_option_strategy_structure(&s).unwrap();
        assert_eq!(result.structure_code(), "long_call");
    }

    #[test]
    fn long_put_is_allowed() {
        let s = strategy(vec![leg(
            OptionLegSide::Long,
            OptionRight::Put,
            500_000_000,
        )]);
        let result = classify_option_strategy_structure(&s).unwrap();
        assert_eq!(result.structure_code(), "long_put");
    }

    #[test]
    fn naked_short_call_is_refused_never_classified() {
        let s = strategy(vec![leg(
            OptionLegSide::Short,
            OptionRight::Call,
            500_000_000,
        )]);
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::NakedShortCall
        );
    }

    #[test]
    fn naked_short_put_is_refused_never_classified() {
        let s = strategy(vec![leg(
            OptionLegSide::Short,
            OptionRight::Put,
            500_000_000,
        )]);
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::NakedShortPut
        );
    }

    #[test]
    fn short_call_with_share_coverage_is_covered_call() {
        let mut s = strategy(vec![leg(
            OptionLegSide::Short,
            OptionRight::Call,
            500_000_000,
        )]);
        s.covering_share_qty = Some(QtyMicros::new(100_000_000));
        let result = classify_option_strategy_structure(&s).unwrap();
        assert_eq!(result.structure_code(), "covered_call");
    }

    #[test]
    fn share_coverage_of_zero_still_refused_as_naked() {
        let mut s = strategy(vec![leg(
            OptionLegSide::Short,
            OptionRight::Call,
            500_000_000,
        )]);
        s.covering_share_qty = Some(QtyMicros::new(0));
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::NakedShortCall
        );
    }

    #[test]
    fn short_put_with_cash_collateral_is_cash_secured_put() {
        let mut s = strategy(vec![leg(
            OptionLegSide::Short,
            OptionRight::Put,
            500_000_000,
        )]);
        s.cash_secured_collateral_micros = Some(50_000_000_000);
        let result = classify_option_strategy_structure(&s).unwrap();
        assert_eq!(result.structure_code(), "cash_secured_put");
    }

    #[test]
    fn call_vertical_spread_is_allowed_with_correct_width_bound() {
        let s = strategy(vec![
            leg(OptionLegSide::Long, OptionRight::Call, 500_000_000),
            leg(OptionLegSide::Short, OptionRight::Call, 510_000_000),
        ]);
        match classify_option_strategy_structure(&s).unwrap() {
            OptionStrategyStructure::CallVerticalSpread {
                spread_width_exercise_bound_micros,
                ..
            } => {
                // width = 10_000_000 micros ($10) * multiplier 100 = 1_000_000_000
                assert_eq!(spread_width_exercise_bound_micros, 1_000_000_000);
            }
            other => panic!("expected CallVerticalSpread, got {other:?}"),
        }
    }

    #[test]
    fn put_vertical_spread_width_bound_is_symmetric_regardless_of_strike_order() {
        let s = strategy(vec![
            leg(OptionLegSide::Long, OptionRight::Put, 510_000_000),
            leg(OptionLegSide::Short, OptionRight::Put, 500_000_000),
        ]);
        match classify_option_strategy_structure(&s).unwrap() {
            OptionStrategyStructure::PutVerticalSpread {
                spread_width_exercise_bound_micros,
                ..
            } => {
                assert_eq!(spread_width_exercise_bound_micros, 1_000_000_000);
            }
            other => panic!("expected PutVerticalSpread, got {other:?}"),
        }
    }

    #[test]
    fn spread_across_call_and_put_is_refused() {
        let s = strategy(vec![
            leg(OptionLegSide::Long, OptionRight::Call, 500_000_000),
            leg(OptionLegSide::Short, OptionRight::Put, 510_000_000),
        ]);
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::SpreadLegsMustShareRightAndExpiry
        );
    }

    #[test]
    fn spread_with_both_legs_long_is_refused_not_silently_narrowed() {
        let s = strategy(vec![
            leg(OptionLegSide::Long, OptionRight::Call, 500_000_000),
            leg(OptionLegSide::Long, OptionRight::Call, 510_000_000),
        ]);
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::SpreadLegsMustBeOppositeDirection
        );
    }

    #[test]
    fn spread_with_identical_strikes_is_refused() {
        let s = strategy(vec![
            leg(OptionLegSide::Long, OptionRight::Call, 500_000_000),
            leg(OptionLegSide::Short, OptionRight::Call, 500_000_000),
        ]);
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::SpreadLegsMustHaveDistinctStrikes
        );
    }

    #[test]
    fn spread_with_mismatched_quantity_is_refused() {
        let mut short_leg = leg(OptionLegSide::Short, OptionRight::Call, 510_000_000);
        short_leg.qty = QtyMicros::new(2_000_000);
        let s = strategy(vec![
            leg(OptionLegSide::Long, OptionRight::Call, 500_000_000),
            short_leg,
        ]);
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::SpreadLegsMustHaveEqualQuantity
        );
    }

    #[test]
    fn three_legs_is_refused_as_too_many_never_partially_accepted() {
        let s = strategy(vec![
            leg(OptionLegSide::Long, OptionRight::Call, 500_000_000),
            leg(OptionLegSide::Short, OptionRight::Call, 510_000_000),
            leg(OptionLegSide::Long, OptionRight::Call, 520_000_000),
        ]);
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::TooManyLegs { leg_count: 3 }
        );
    }

    #[test]
    fn zero_legs_is_refused() {
        let s = strategy(vec![]);
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::NoLegs
        );
    }

    #[test]
    fn empty_underlying_is_refused_before_leg_inspection() {
        let mut s = strategy(vec![leg(
            OptionLegSide::Long,
            OptionRight::Call,
            500_000_000,
        )]);
        s.underlying = "  ".to_string();
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::EmptyUnderlying
        );
    }

    #[test]
    fn invalid_leg_strike_is_refused() {
        let s = strategy(vec![leg(OptionLegSide::Long, OptionRight::Call, 0)]);
        assert!(matches!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::InvalidLeg { .. }
        ));
    }

    // -----------------------------------------------------------------
    // IR-B1-02: covered-call / cash-secured-put adequacy negative controls
    // -----------------------------------------------------------------

    #[test]
    fn covered_call_insufficient_shares_refused_even_though_nonzero() {
        // 1 contract * multiplier 100 requires exactly 100 shares
        // (100_000_000 raw). One share short of that must still refuse —
        // proves the check is an exact `>=`, not merely-positive.
        let mut s = strategy(vec![leg(
            OptionLegSide::Short,
            OptionRight::Call,
            500_000_000,
        )]);
        s.covering_share_qty = Some(QtyMicros::new(99_999_999));
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::NakedShortCall
        );
    }

    #[test]
    fn covered_call_exact_adequate_coverage_succeeds() {
        let mut short_leg = leg(OptionLegSide::Short, OptionRight::Call, 500_000_000);
        short_leg.qty = QtyMicros::new(3_000_000); // 3 contracts
        let mut s = strategy(vec![short_leg]);
        // 3 contracts * multiplier 100 = 300 shares required, exactly.
        s.covering_share_qty = Some(QtyMicros::new(300_000_000));
        let result = classify_option_strategy_structure(&s).unwrap();
        assert_eq!(result.structure_code(), "covered_call");
    }

    #[test]
    fn covered_call_coverage_arithmetic_overflow_refuses_closed() {
        let mut short_leg = leg(OptionLegSide::Short, OptionRight::Call, 500_000_000);
        short_leg.qty = QtyMicros::new(i64::MAX);
        short_leg.multiplier = 2; // i64::MAX * 2 overflows i64
        let mut s = strategy(vec![short_leg]);
        s.covering_share_qty = Some(QtyMicros::new(i64::MAX));
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::CoveredCallCoverageOverflow
        );
    }

    #[test]
    fn cash_secured_put_insufficient_collateral_refused_even_though_nonzero() {
        // strike $500 * multiplier 100 * 1 contract = $50,000 obligation
        // (50_000_000_000 micros). One micro-dollar short must still
        // refuse — proves the check is an exact `>=` against the full
        // conservative exercise obligation, never inferred adequate from a
        // hoped-for future premium credit.
        let mut s = strategy(vec![leg(
            OptionLegSide::Short,
            OptionRight::Put,
            500_000_000,
        )]);
        s.cash_secured_collateral_micros = Some(49_999_999_999);
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::NakedShortPut
        );
    }

    #[test]
    fn cash_secured_put_exact_adequate_collateral_succeeds() {
        let mut short_leg = leg(OptionLegSide::Short, OptionRight::Put, 500_000_000);
        short_leg.qty = QtyMicros::new(2_000_000); // 2 contracts
        let mut s = strategy(vec![short_leg]);
        // strike $500 * multiplier 100 * 2 contracts = $100,000 exactly.
        s.cash_secured_collateral_micros = Some(100_000_000_000);
        let result = classify_option_strategy_structure(&s).unwrap();
        assert_eq!(result.structure_code(), "cash_secured_put");
    }

    #[test]
    fn cash_secured_put_obligation_arithmetic_overflow_refuses_closed() {
        let mut short_leg = leg(OptionLegSide::Short, OptionRight::Put, 500_000_000);
        short_leg.strike_micros = i64::MAX;
        short_leg.multiplier = i32::MAX;
        short_leg.qty = QtyMicros::new(i64::MAX);
        let mut s = strategy(vec![short_leg]);
        s.cash_secured_collateral_micros = Some(i64::MAX);
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::CashSecuredPutObligationOverflow
        );
    }

    #[test]
    fn cash_secured_put_refuses_closed_on_non_whole_contract_obligation() {
        // A genuinely fractional contract quantity (0.5 contracts) has no
        // well-defined whole-cent cash obligation here; the guard must
        // refuse rather than floor-round to an understated requirement
        // (which would fail OPEN, under-collateralizing a real short put).
        let mut short_leg = leg(OptionLegSide::Short, OptionRight::Put, 3);
        short_leg.multiplier = 1;
        short_leg.qty = QtyMicros::new(500_000); // 0.5 contracts
        let mut s = strategy(vec![short_leg]);
        s.cash_secured_collateral_micros = Some(i64::MAX);
        assert_eq!(
            classify_option_strategy_structure(&s).unwrap_err(),
            OptionStrategyRefusal::CashSecuredPutObligationOverflow
        );
    }

    #[test]
    fn naked_shorts_remain_refused_after_adequacy_tightening() {
        // Regression guard: the exact-adequacy rewrite must not weaken the
        // baseline zero-coverage/zero-collateral naked-short refusals.
        let call_s = strategy(vec![leg(
            OptionLegSide::Short,
            OptionRight::Call,
            500_000_000,
        )]);
        assert_eq!(
            classify_option_strategy_structure(&call_s).unwrap_err(),
            OptionStrategyRefusal::NakedShortCall
        );
        let put_s = strategy(vec![leg(
            OptionLegSide::Short,
            OptionRight::Put,
            500_000_000,
        )]);
        assert_eq!(
            classify_option_strategy_structure(&put_s).unwrap_err(),
            OptionStrategyRefusal::NakedShortPut
        );
    }
}
