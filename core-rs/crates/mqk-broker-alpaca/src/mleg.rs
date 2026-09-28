//! D5 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION):
//! multi-leg (mleg) vertical-spread order construction and response
//! normalization.
//!
//! Pure domain <-> wire translation, mirroring `normalize.rs`'s role for
//! the inbound lane: [`build_mleg_submit_body`] constructs exactly ONE
//! Alpaca `order_class=mleg` request body carrying both legs of a
//! single-ratio vertical spread -- this crate never submits a leg as its
//! own independent order. [`normalize_mleg_submit_response`] verifies the
//! broker's response actually carries exactly two legs matching the two
//! legs requested (by option symbol) before returning normalized parent +
//! leg identity; any other shape is a refusal, never silently narrowed or
//! reordered by position.
//!
//! No HTTP, no capability-flag check, no `BrokerError` here -- those live
//! at the `AlpacaBrokerAdapter` boundary in `lib.rs`, exactly as
//! `normalize_trade_update` stays free of both concerns for the inbound
//! lane.
//!
//! # D5 correction: proving a frozen vertical before HTTP
//!
//! [`SubmitVerticalSpreadRequest`] alone -- two arbitrary
//! [`VerticalSpreadLeg`] symbols -- never proved same underlying/
//! expiration/right, distinct strikes, or one long+one short before this
//! crate's own review found `AlpacaBrokerAdapter::submit_vertical_spread`
//! serializing and POSTing it directly. [`build_verified_vertical_spread_request`]
//! is now the one sanctioned way to build a request for submission: it
//! re-derives the frozen structure itself via
//! `mqk_execution::option_strategy_permission::classify_option_strategy_structure`
//! (never trusts a pre-built [`mqk_execution::option_strategy_permission::OptionStrategyStructure`]
//! value, which a caller could otherwise hand-construct bypassing the
//! classifier's own checks, since its fields are public), accepts only the
//! two frozen vertical shapes (`CallVerticalSpread`/`PutVerticalSpread` --
//! every other classified/refused shape is refused here too), and checks
//! the two legs' position intents form one supported open/close
//! combination. Its `Result::Ok` is a [`VerifiedVerticalSpreadSubmission`]
//! -- a newtype whose inner request is only ever constructed through this
//! function -- and `AlpacaBrokerAdapter::submit_vertical_spread` now
//! accepts only that type, so the compiler (not caller discipline) rules
//! out ever reaching HTTP with an unverified request.

use crate::types::{AlpacaMlegLegBody, AlpacaMlegSubmitBody, AlpacaMlegSubmitResponse};
use mqk_execution::option_strategy_permission::{
    classify_option_strategy_structure, OptionStrategyStructure, ProposedOptionStrategy,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlegSide {
    Buy,
    Sell,
}

impl MlegSide {
    fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }
}

/// Per Alpaca's official POST /v2/orders reference (verified 2026-09-28):
/// each mleg leg carries its own `position_intent`, distinct from the
/// parent order's `side`/`qty` -- opening a long leg and closing an
/// existing short leg are structurally different intents even when both
/// happen to be `MlegSide::Buy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlegPositionIntent {
    BuyToOpen,
    BuyToClose,
    SellToOpen,
    SellToClose,
}

impl MlegPositionIntent {
    fn as_str(self) -> &'static str {
        match self {
            Self::BuyToOpen => "buy_to_open",
            Self::BuyToClose => "buy_to_close",
            Self::SellToOpen => "sell_to_open",
            Self::SellToClose => "sell_to_close",
        }
    }
}

/// One leg of a proposed vertical-spread submission. `option_symbol` is
/// already resolved to Alpaca's OCC-style contract symbol by the caller --
/// this crate does not parse or construct option symbols itself, mirroring
/// `option_lifecycle_apply`'s own documented boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerticalSpreadLeg {
    pub option_symbol: String,
    pub side: MlegSide,
    pub position_intent: MlegPositionIntent,
}

/// A proposed two-leg vertical spread, ready to submit as one `mleg`
/// order. `long_leg`/`short_leg` are two named fields, never a `Vec` that
/// could hold a variable leg count -- mirrors
/// `option_strategy_permission::CallVerticalSpread`'s own structural
/// guarantee (D4, already correct/proven).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitVerticalSpreadRequest {
    pub client_order_id: String,
    /// Units of the overall spread strategy, as a decimal string (e.g.
    /// `"1"`).
    pub qty: String,
    /// Net spread price, as a decimal string.
    pub limit_price: String,
    pub time_in_force: String,
    pub long_leg: VerticalSpreadLeg,
    pub short_leg: VerticalSpreadLeg,
}

/// Construct the single request body for both legs. Always exactly two
/// entries in `legs`, each at `ratio_qty = "1"` (a single-ratio vertical:
/// one long contract paired with one short contract per spread unit) --
/// never split into two separate submissions.
pub fn build_mleg_submit_body(req: &SubmitVerticalSpreadRequest) -> AlpacaMlegSubmitBody {
    let leg_body = |leg: &VerticalSpreadLeg| AlpacaMlegLegBody {
        symbol: leg.option_symbol.clone(),
        side: leg.side.as_str().to_string(),
        position_intent: leg.position_intent.as_str().to_string(),
        ratio_qty: "1".to_string(),
    };
    AlpacaMlegSubmitBody {
        order_class: "mleg".to_string(),
        qty: req.qty.clone(),
        order_type: "limit".to_string(),
        time_in_force: req.time_in_force.clone(),
        limit_price: req.limit_price.clone(),
        legs: vec![leg_body(&req.long_leg), leg_body(&req.short_leg)],
        client_order_id: req.client_order_id.clone(),
    }
}

// ---------------------------------------------------------------------------
// D5 correction: prove a frozen vertical before HTTP
// ---------------------------------------------------------------------------

/// A [`SubmitVerticalSpreadRequest`] that has been proven, by
/// [`build_verified_vertical_spread_request`], to be one of the two frozen
/// vertical-spread shapes. The inner request is private -- constructible
/// only through that one function -- so `AlpacaBrokerAdapter::
/// submit_vertical_spread` accepting this type (rather than
/// [`SubmitVerticalSpreadRequest`] directly) is a compile-time guarantee,
/// not merely a caller-discipline convention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedVerticalSpreadSubmission(SubmitVerticalSpreadRequest);

impl VerifiedVerticalSpreadSubmission {
    pub fn request(&self) -> &SubmitVerticalSpreadRequest {
        &self.0
    }
}

/// Closed-vocabulary refusal for [`build_verified_vertical_spread_request`].
/// Every reachable non-frozen shape refuses here, before any
/// [`SubmitVerticalSpreadRequest`] -- let alone any HTTP body -- is ever
/// constructed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerticalSpreadBuildRefusal {
    /// `proposed` was refused by the frozen classifier itself -- e.g.
    /// mismatched underlying/expiry/right, equal strikes, same-direction
    /// legs, more than two legs, or any other non-frozen shape.
    NotAFrozenStructure(mqk_execution::option_strategy_permission::OptionStrategyRefusal),
    /// The classifier accepted `proposed`, but as a single-leg structure
    /// (`LongCall`/`LongPut`/`CoveredCall`/`CashSecuredPut`) -- those are
    /// single-order submissions, never mleg.
    NotAVerticalSpread { structure_code: &'static str },
    /// The two legs' position intents are not one supported open/close
    /// combination for a single atomic spread action (open both legs
    /// together, or close both legs together).
    UnsupportedPositionIntentCombination {
        long_intent: MlegPositionIntent,
        short_intent: MlegPositionIntent,
    },
}

/// The one sanctioned way to build a vertical-spread submission. Re-derives
/// the frozen structure from `proposed` itself (see module doc for why this
/// never trusts a pre-built [`OptionStrategyStructure`]); `long_symbol`/
/// `short_symbol` are the caller-resolved Alpaca OCC-style contract symbols
/// for `proposed`'s long/short legs respectively -- this crate does not
/// parse or construct option symbols itself (same boundary
/// `build_mleg_submit_body` already documented).
#[allow(clippy::too_many_arguments)]
pub fn build_verified_vertical_spread_request(
    proposed: &ProposedOptionStrategy,
    long_symbol: &str,
    short_symbol: &str,
    long_intent: MlegPositionIntent,
    short_intent: MlegPositionIntent,
    client_order_id: String,
    qty: String,
    limit_price: String,
    time_in_force: String,
) -> Result<VerifiedVerticalSpreadSubmission, VerticalSpreadBuildRefusal> {
    let structure = classify_option_strategy_structure(proposed)
        .map_err(VerticalSpreadBuildRefusal::NotAFrozenStructure)?;

    match &structure {
        OptionStrategyStructure::CallVerticalSpread { .. }
        | OptionStrategyStructure::PutVerticalSpread { .. } => {}
        other => {
            return Err(VerticalSpreadBuildRefusal::NotAVerticalSpread {
                structure_code: other.structure_code(),
            })
        }
    }

    let supported_intents = matches!(
        (long_intent, short_intent),
        (
            MlegPositionIntent::BuyToOpen,
            MlegPositionIntent::SellToOpen
        ) | (
            MlegPositionIntent::BuyToClose,
            MlegPositionIntent::SellToClose
        )
    );
    if !supported_intents {
        return Err(
            VerticalSpreadBuildRefusal::UnsupportedPositionIntentCombination {
                long_intent,
                short_intent,
            },
        );
    }

    Ok(VerifiedVerticalSpreadSubmission(
        SubmitVerticalSpreadRequest {
            client_order_id,
            qty,
            limit_price,
            time_in_force,
            long_leg: VerticalSpreadLeg {
                option_symbol: long_symbol.to_string(),
                side: MlegSide::Buy,
                position_intent: long_intent,
            },
            short_leg: VerticalSpreadLeg {
                option_symbol: short_symbol.to_string(),
                side: MlegSide::Sell,
                position_intent: short_intent,
            },
        },
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedVerticalSpreadLeg {
    pub broker_leg_order_id: String,
    pub option_symbol: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedVerticalSpread {
    pub broker_parent_order_id: String,
    pub client_order_id: String,
    pub status: String,
    pub long_leg: SubmittedVerticalSpreadLeg,
    pub short_leg: SubmittedVerticalSpreadLeg,
}

/// Closed-vocabulary refusal for a broker response that does not match the
/// exactly-two-legs contract this crate requires. Never silently narrowed
/// (e.g. taking `legs[0]`/`legs[1]` by position without checking symbol
/// identity) -- broker truth disagreeing with what was requested is a
/// genuine anomaly, not a shape to paper over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MlegRefusal {
    LegCountMismatch { expected: usize, actual: usize },
    LegIdentityMismatch { detail: String },
}

/// Verify and normalize a broker response against the request that
/// produced it. Matches each response leg back to the request's long/short
/// leg by `option_symbol` -- never by array position, since Alpaca's
/// response leg order is not documented as stable.
pub fn normalize_mleg_submit_response(
    resp: AlpacaMlegSubmitResponse,
    req: &SubmitVerticalSpreadRequest,
) -> Result<SubmittedVerticalSpread, MlegRefusal> {
    if resp.legs.len() != 2 {
        return Err(MlegRefusal::LegCountMismatch {
            expected: 2,
            actual: resp.legs.len(),
        });
    }

    let find_leg = |option_symbol: &str| {
        resp.legs
            .iter()
            .find(|l| l.symbol == option_symbol)
            .cloned()
    };

    let long_resp =
        find_leg(&req.long_leg.option_symbol).ok_or_else(|| MlegRefusal::LegIdentityMismatch {
            detail: format!(
                "no response leg matched requested long leg symbol {:?}",
                req.long_leg.option_symbol
            ),
        })?;
    let short_resp =
        find_leg(&req.short_leg.option_symbol).ok_or_else(|| MlegRefusal::LegIdentityMismatch {
            detail: format!(
                "no response leg matched requested short leg symbol {:?}",
                req.short_leg.option_symbol
            ),
        })?;
    if long_resp.symbol == short_resp.symbol {
        return Err(MlegRefusal::LegIdentityMismatch {
            detail: format!(
                "long and short leg resolved to the same response leg (symbol {:?}) -- \
                 requested legs must be distinct contracts",
                long_resp.symbol
            ),
        });
    }

    Ok(SubmittedVerticalSpread {
        broker_parent_order_id: resp.id,
        client_order_id: resp.client_order_id,
        status: resp.status,
        long_leg: SubmittedVerticalSpreadLeg {
            broker_leg_order_id: long_resp.id,
            option_symbol: long_resp.symbol,
            status: long_resp.status,
        },
        short_leg: SubmittedVerticalSpreadLeg {
            broker_leg_order_id: short_resp.id,
            option_symbol: short_resp.symbol,
            status: short_resp.status,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AlpacaMlegLegResponse;

    fn leg(option_symbol: &str, side: MlegSide, intent: MlegPositionIntent) -> VerticalSpreadLeg {
        VerticalSpreadLeg {
            option_symbol: option_symbol.to_string(),
            side,
            position_intent: intent,
        }
    }

    fn request() -> SubmitVerticalSpreadRequest {
        SubmitVerticalSpreadRequest {
            client_order_id: "co-1".to_string(),
            qty: "1".to_string(),
            limit_price: "2.50".to_string(),
            time_in_force: "day".to_string(),
            long_leg: leg(
                "AAPL260619C00500000",
                MlegSide::Buy,
                MlegPositionIntent::BuyToOpen,
            ),
            short_leg: leg(
                "AAPL260619C00510000",
                MlegSide::Sell,
                MlegPositionIntent::SellToOpen,
            ),
        }
    }

    #[test]
    fn build_mleg_submit_body_carries_exactly_two_legs_never_split() {
        let body = build_mleg_submit_body(&request());
        assert_eq!(body.order_class, "mleg");
        assert_eq!(body.legs.len(), 2);
        assert_eq!(body.legs[0].symbol, "AAPL260619C00500000");
        assert_eq!(body.legs[0].side, "buy");
        assert_eq!(body.legs[0].position_intent, "buy_to_open");
        assert_eq!(body.legs[1].symbol, "AAPL260619C00510000");
        assert_eq!(body.legs[1].side, "sell");
        assert_eq!(body.legs[1].position_intent, "sell_to_open");
    }

    fn resp_leg(id: &str, symbol: &str, side: &str, status: &str) -> AlpacaMlegLegResponse {
        AlpacaMlegLegResponse {
            id: id.to_string(),
            symbol: symbol.to_string(),
            side: side.to_string(),
            status: status.to_string(),
        }
    }

    #[test]
    fn normalize_matches_legs_by_symbol_not_response_order() {
        let req = request();
        // Response lists the SHORT leg first -- normalization must not
        // assume request/response leg order agree.
        let resp = AlpacaMlegSubmitResponse {
            id: "parent-1".to_string(),
            client_order_id: "co-1".to_string(),
            order_class: "mleg".to_string(),
            qty: "1".to_string(),
            status: "accepted".to_string(),
            legs: vec![
                resp_leg("leg-short", "AAPL260619C00510000", "sell", "accepted"),
                resp_leg("leg-long", "AAPL260619C00500000", "buy", "accepted"),
            ],
        };

        let normalized = normalize_mleg_submit_response(resp, &req).expect("must normalize");
        assert_eq!(normalized.broker_parent_order_id, "parent-1");
        assert_eq!(normalized.long_leg.broker_leg_order_id, "leg-long");
        assert_eq!(normalized.long_leg.option_symbol, "AAPL260619C00500000");
        assert_eq!(normalized.short_leg.broker_leg_order_id, "leg-short");
        assert_eq!(normalized.short_leg.option_symbol, "AAPL260619C00510000");
    }

    #[test]
    fn normalize_refuses_when_broker_returns_wrong_leg_count() {
        let req = request();
        let resp = AlpacaMlegSubmitResponse {
            id: "parent-2".to_string(),
            client_order_id: "co-1".to_string(),
            order_class: "mleg".to_string(),
            qty: "1".to_string(),
            status: "accepted".to_string(),
            legs: vec![resp_leg(
                "leg-long",
                "AAPL260619C00500000",
                "buy",
                "accepted",
            )],
        };

        assert_eq!(
            normalize_mleg_submit_response(resp, &req).unwrap_err(),
            MlegRefusal::LegCountMismatch {
                expected: 2,
                actual: 1
            }
        );
    }

    #[test]
    fn normalize_refuses_when_a_requested_symbol_has_no_matching_response_leg() {
        let req = request();
        let resp = AlpacaMlegSubmitResponse {
            id: "parent-3".to_string(),
            client_order_id: "co-1".to_string(),
            order_class: "mleg".to_string(),
            qty: "1".to_string(),
            status: "accepted".to_string(),
            legs: vec![
                resp_leg("leg-long", "AAPL260619C00500000", "buy", "accepted"),
                // Wrong symbol -- broker returned something other than the
                // requested short leg.
                resp_leg("leg-other", "AAPL260619C00520000", "sell", "accepted"),
            ],
        };

        assert!(matches!(
            normalize_mleg_submit_response(resp, &req).unwrap_err(),
            MlegRefusal::LegIdentityMismatch { .. }
        ));
    }

    // -----------------------------------------------------------------
    // D5 correction: build_verified_vertical_spread_request proves a
    // frozen vertical before any SubmitVerticalSpreadRequest exists.
    // -----------------------------------------------------------------

    use mqk_execution::option_strategy_permission::{
        OptionLegSide, OptionStrategyRefusal, ProposedOptionLeg,
    };
    use mqk_schemas::{OptionRight, QtyMicros};

    fn proposed_leg(
        side: OptionLegSide,
        right: OptionRight,
        strike_micros: i64,
        expiry: &str,
    ) -> ProposedOptionLeg {
        ProposedOptionLeg {
            side,
            right,
            strike_micros,
            expiry_yyyymmdd: expiry.to_string(),
            qty: QtyMicros::new(1_000_000),
            multiplier: 100,
        }
    }

    fn valid_call_vertical() -> mqk_execution::option_strategy_permission::ProposedOptionStrategy {
        mqk_execution::option_strategy_permission::ProposedOptionStrategy {
            underlying: "AAPL".to_string(),
            legs: vec![
                proposed_leg(
                    OptionLegSide::Long,
                    OptionRight::Call,
                    500_000_000,
                    "20260619",
                ),
                proposed_leg(
                    OptionLegSide::Short,
                    OptionRight::Call,
                    510_000_000,
                    "20260619",
                ),
            ],
            covering_share_qty: None,
            cash_secured_collateral_micros: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn try_build(
        proposed: &mqk_execution::option_strategy_permission::ProposedOptionStrategy,
        long_intent: MlegPositionIntent,
        short_intent: MlegPositionIntent,
    ) -> Result<VerifiedVerticalSpreadSubmission, VerticalSpreadBuildRefusal> {
        build_verified_vertical_spread_request(
            proposed,
            "AAPL260619C00500000",
            "AAPL260619C00510000",
            long_intent,
            short_intent,
            "co-1".to_string(),
            "1".to_string(),
            "2.50".to_string(),
            "day".to_string(),
        )
    }

    #[test]
    fn valid_call_vertical_builds_a_verified_submission() {
        let verified = try_build(
            &valid_call_vertical(),
            MlegPositionIntent::BuyToOpen,
            MlegPositionIntent::SellToOpen,
        )
        .expect("a valid call vertical must build");
        let req = verified.request();
        assert_eq!(req.long_leg.option_symbol, "AAPL260619C00500000");
        assert_eq!(req.long_leg.side, MlegSide::Buy);
        assert_eq!(req.short_leg.option_symbol, "AAPL260619C00510000");
        assert_eq!(req.short_leg.side, MlegSide::Sell);
    }

    #[test]
    fn valid_put_vertical_builds_a_verified_submission() {
        let mut proposed = valid_call_vertical();
        proposed.legs[0].right = OptionRight::Put;
        proposed.legs[1].right = OptionRight::Put;
        let verified = try_build(
            &proposed,
            MlegPositionIntent::BuyToOpen,
            MlegPositionIntent::SellToOpen,
        )
        .expect("a valid put vertical must build");
        assert_eq!(
            verified.request().long_leg.option_symbol,
            "AAPL260619C00500000"
        );
    }

    #[test]
    fn mismatched_expiration_refuses_before_any_request_is_built() {
        let mut proposed = valid_call_vertical();
        proposed.legs[1].expiry_yyyymmdd = "20260717".to_string();
        let err = try_build(
            &proposed,
            MlegPositionIntent::BuyToOpen,
            MlegPositionIntent::SellToOpen,
        )
        .unwrap_err();
        assert_eq!(
            err,
            VerticalSpreadBuildRefusal::NotAFrozenStructure(
                OptionStrategyRefusal::SpreadLegsMustShareRightAndExpiry
            )
        );
    }

    #[test]
    fn call_plus_put_refuses_before_any_request_is_built() {
        let mut proposed = valid_call_vertical();
        proposed.legs[1].right = OptionRight::Put;
        let err = try_build(
            &proposed,
            MlegPositionIntent::BuyToOpen,
            MlegPositionIntent::SellToOpen,
        )
        .unwrap_err();
        assert_eq!(
            err,
            VerticalSpreadBuildRefusal::NotAFrozenStructure(
                OptionStrategyRefusal::SpreadLegsMustShareRightAndExpiry
            )
        );
    }

    #[test]
    fn equal_strikes_refuses_before_any_request_is_built() {
        let mut proposed = valid_call_vertical();
        proposed.legs[1].strike_micros = proposed.legs[0].strike_micros;
        let err = try_build(
            &proposed,
            MlegPositionIntent::BuyToOpen,
            MlegPositionIntent::SellToOpen,
        )
        .unwrap_err();
        assert_eq!(
            err,
            VerticalSpreadBuildRefusal::NotAFrozenStructure(
                OptionStrategyRefusal::SpreadLegsMustHaveDistinctStrikes
            )
        );
    }

    #[test]
    fn same_direction_legs_refuses_before_any_request_is_built() {
        let mut proposed = valid_call_vertical();
        proposed.legs[1].side = OptionLegSide::Long; // both long -- not a vertical
        let err = try_build(
            &proposed,
            MlegPositionIntent::BuyToOpen,
            MlegPositionIntent::SellToOpen,
        )
        .unwrap_err();
        assert_eq!(
            err,
            VerticalSpreadBuildRefusal::NotAFrozenStructure(
                OptionStrategyRefusal::SpreadLegsMustBeOppositeDirection
            )
        );
    }

    #[test]
    fn unsupported_multi_leg_structure_refuses_before_any_request_is_built() {
        // Three legs -- never a frozen shape at all.
        let mut proposed = valid_call_vertical();
        proposed.legs.push(proposed_leg(
            OptionLegSide::Long,
            OptionRight::Call,
            520_000_000,
            "20260619",
        ));
        let err = try_build(
            &proposed,
            MlegPositionIntent::BuyToOpen,
            MlegPositionIntent::SellToOpen,
        )
        .unwrap_err();
        assert_eq!(
            err,
            VerticalSpreadBuildRefusal::NotAFrozenStructure(OptionStrategyRefusal::TooManyLegs {
                leg_count: 3
            })
        );
    }

    #[test]
    fn single_leg_structure_is_refused_as_not_a_vertical_spread() {
        // A single long call is a real frozen shape (long_call) -- but it
        // is never an mleg submission.
        let proposed = mqk_execution::option_strategy_permission::ProposedOptionStrategy {
            underlying: "AAPL".to_string(),
            legs: vec![proposed_leg(
                OptionLegSide::Long,
                OptionRight::Call,
                500_000_000,
                "20260619",
            )],
            covering_share_qty: None,
            cash_secured_collateral_micros: None,
        };
        let err = try_build(
            &proposed,
            MlegPositionIntent::BuyToOpen,
            MlegPositionIntent::SellToOpen,
        )
        .unwrap_err();
        assert_eq!(
            err,
            VerticalSpreadBuildRefusal::NotAVerticalSpread {
                structure_code: "long_call"
            }
        );
    }

    #[test]
    fn malformed_long_short_intent_combination_refuses() {
        let err = try_build(
            &valid_call_vertical(),
            MlegPositionIntent::BuyToOpen,
            MlegPositionIntent::SellToClose, // opening one leg, closing the other -- invalid
        )
        .unwrap_err();
        assert_eq!(
            err,
            VerticalSpreadBuildRefusal::UnsupportedPositionIntentCombination {
                long_intent: MlegPositionIntent::BuyToOpen,
                short_intent: MlegPositionIntent::SellToClose,
            }
        );
    }

    #[test]
    fn closing_both_legs_is_a_supported_intent_combination() {
        try_build(
            &valid_call_vertical(),
            MlegPositionIntent::BuyToClose,
            MlegPositionIntent::SellToClose,
        )
        .expect("closing both legs together must build");
    }
}
