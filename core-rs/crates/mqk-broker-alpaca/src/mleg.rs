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

use crate::types::{AlpacaMlegLegBody, AlpacaMlegSubmitBody, AlpacaMlegSubmitResponse};

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
}
