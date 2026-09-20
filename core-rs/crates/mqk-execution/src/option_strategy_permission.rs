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
//! `max_loss_upper_bound_micros` on the two vertical-spread variants is a
//! **structural** bound: for any single-ratio (1 long leg vs. 1 short leg,
//! same underlying/expiry/right, distinct strikes) vertical, the maximum
//! possible loss can never exceed `|strike_a - strike_b| * multiplier`,
//! regardless of the net premium paid or received and regardless of
//! whether the spread is a net debit or net credit — this is a structural
//! options-theory fact, not a live-priced estimate, so it can be computed
//! here with zero market data.

use mqk_schemas::{OptionRight, QtyMicros};

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
        max_loss_upper_bound_micros: i128,
    },
    PutVerticalSpread {
        long_leg: ProposedOptionLeg,
        short_leg: ProposedOptionLeg,
        max_loss_upper_bound_micros: i128,
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
    /// A single short call with no proven share coverage. This is the
    /// specific, named refusal for a naked short call — never silently
    /// reclassified as anything else.
    NakedShortCall,
    /// A single short put with no proven cash collateral. The specific,
    /// named refusal for a naked short put.
    NakedShortPut,
    SpreadLegsMustShareRightAndExpiry,
    SpreadLegsMustBeOppositeDirection,
    SpreadLegsMustHaveEqualQuantity,
    SpreadLegsMustHaveEqualMultiplier,
    SpreadLegsMustHaveDistinctStrikes,
    /// Structural loss-bound arithmetic could not be represented even in
    /// `i128` (never silently saturated/truncated).
    SpreadLossBoundOverflow,
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
            Self::SpreadLegsMustShareRightAndExpiry => "spread_legs_must_share_right_and_expiry",
            Self::SpreadLegsMustBeOppositeDirection => "spread_legs_must_be_opposite_direction",
            Self::SpreadLegsMustHaveEqualQuantity => "spread_legs_must_have_equal_quantity",
            Self::SpreadLegsMustHaveEqualMultiplier => "spread_legs_must_have_equal_multiplier",
            Self::SpreadLegsMustHaveDistinctStrikes => "spread_legs_must_have_distinct_strikes",
            Self::SpreadLossBoundOverflow => "spread_loss_bound_overflow",
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
    if leg.expiry_yyyymmdd.trim().is_empty() {
        return Err(OptionStrategyRefusal::InvalidLeg {
            detail: "expiry_yyyymmdd must be non-empty".to_string(),
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

/// Structural worst-case loss bound for a single-ratio vertical spread:
/// `|strike_a - strike_b| * multiplier`, checked in `i128`. True regardless
/// of net debit/credit and regardless of call/put — see module docs.
fn vertical_spread_loss_bound_micros(
    long_leg: &ProposedOptionLeg,
    short_leg: &ProposedOptionLeg,
) -> Result<i128, OptionStrategyRefusal> {
    let width = (long_leg.strike_micros as i128 - short_leg.strike_micros as i128).abs();
    width
        .checked_mul(long_leg.multiplier as i128)
        .ok_or(OptionStrategyRefusal::SpreadLossBoundOverflow)
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

    match proposed.legs.as_slice() {
        [] => Err(OptionStrategyRefusal::NoLegs),
        [leg] => classify_single_leg(leg, proposed),
        [leg_a, leg_b] => classify_two_legs(leg_a, leg_b),
        legs => Err(OptionStrategyRefusal::TooManyLegs {
            leg_count: legs.len(),
        }),
    }
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
        (OptionLegSide::Short, OptionRight::Call) => match proposed.covering_share_qty {
            Some(covering_share_qty) if covering_share_qty.raw() > 0 => {
                Ok(OptionStrategyStructure::CoveredCall {
                    short_call: leg.clone(),
                    covering_share_qty,
                })
            }
            _ => Err(OptionStrategyRefusal::NakedShortCall),
        },
        (OptionLegSide::Short, OptionRight::Put) => match proposed.cash_secured_collateral_micros {
            Some(collateral_cash_micros) if collateral_cash_micros > 0 => {
                Ok(OptionStrategyStructure::CashSecuredPut {
                    short_put: leg.clone(),
                    collateral_cash_micros,
                })
            }
            _ => Err(OptionStrategyRefusal::NakedShortPut),
        },
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

    let max_loss_upper_bound_micros = vertical_spread_loss_bound_micros(long_leg, short_leg)?;

    match long_leg.right {
        OptionRight::Call => Ok(OptionStrategyStructure::CallVerticalSpread {
            long_leg: long_leg.clone(),
            short_leg: short_leg.clone(),
            max_loss_upper_bound_micros,
        }),
        OptionRight::Put => Ok(OptionStrategyStructure::PutVerticalSpread {
            long_leg: long_leg.clone(),
            short_leg: short_leg.clone(),
            max_loss_upper_bound_micros,
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
    fn call_vertical_spread_is_allowed_with_correct_loss_bound() {
        let s = strategy(vec![
            leg(OptionLegSide::Long, OptionRight::Call, 500_000_000),
            leg(OptionLegSide::Short, OptionRight::Call, 510_000_000),
        ]);
        match classify_option_strategy_structure(&s).unwrap() {
            OptionStrategyStructure::CallVerticalSpread {
                max_loss_upper_bound_micros,
                ..
            } => {
                // width = 10_000_000 micros ($10) * multiplier 100 = 1_000_000_000
                assert_eq!(max_loss_upper_bound_micros, 1_000_000_000);
            }
            other => panic!("expected CallVerticalSpread, got {other:?}"),
        }
    }

    #[test]
    fn put_vertical_spread_loss_bound_is_symmetric_regardless_of_strike_order() {
        let s = strategy(vec![
            leg(OptionLegSide::Long, OptionRight::Put, 510_000_000),
            leg(OptionLegSide::Short, OptionRight::Put, 500_000_000),
        ]);
        match classify_option_strategy_structure(&s).unwrap() {
            OptionStrategyStructure::PutVerticalSpread {
                max_loss_upper_bound_micros,
                ..
            } => {
                assert_eq!(max_loss_upper_bound_micros, 1_000_000_000);
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
}
