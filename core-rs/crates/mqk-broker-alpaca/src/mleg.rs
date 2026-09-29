//! D5: multi-leg (mleg) vertical-spread order construction and response
//! authentication.
//!
//! One authority: the typed, parsed [`VerifiedVertical`] (two
//! [`OptionContractIdentity`]s proven to form a frozen call/put vertical) is
//! BOTH what is verified and what is sent. The wire symbols are derived from
//! the identities' canonical OCC form; there is no independent free-form symbol
//! path, so "verified strategy A, submitted contracts B" is unrepresentable.
//!
//! The parent action is typed ([`VerticalAction`]); each leg's `side` and
//! `position_intent` are DERIVED from it and never caller-supplied, so an
//! inconsistent side/intent pair cannot be constructed:
//!
//! | action | long leg             | short leg             |
//! |--------|----------------------|-----------------------|
//! | Open   | buy / buy_to_open    | sell / sell_to_open   |
//! | Close  | sell / sell_to_close | buy / buy_to_close    |
//!
//! [`build_mleg_submit_body`] emits exactly ONE `order_class=mleg` body with
//! both legs; [`normalize_mleg_submit_response`] authenticates the provider's
//! answer (parent id, echoed `client_order_id`, `order_class`, parent `qty`,
//! exactly two legs matching the submitted contracts and sides) before
//! returning anything. Any disagreement is a refusal the adapter surfaces as
//! `AmbiguousSubmit` (the order may be live in an unproven shape).
//!
//! No HTTP, no capability-flag check, no `BrokerError` here -- those live at
//! the `AlpacaBrokerAdapter` boundary in `lib.rs`.

use crate::types::{AlpacaMlegLegBody, AlpacaMlegSubmitBody, AlpacaMlegSubmitResponse};
use mqk_execution::option_strategy_permission::{
    classify_option_strategy_structure, OptionLegSide, OptionStrategyRefusal,
    OptionStrategyStructure, ProposedOptionLeg, ProposedOptionStrategy,
};
use mqk_execution::{OptionContractIdentity, OptionRight};
use mqk_schemas::QtyMicros;

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

/// Per Alpaca's POST /v2/orders reference, each mleg leg carries its own
/// `position_intent`, distinct from the parent order.
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

/// Typed parent action of a vertical spread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAction {
    /// Open both legs together.
    OpenVertical,
    /// Close both existing legs together.
    CloseVertical,
}

/// Wire side + position intent of one leg, derived from the action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegWire {
    pub side: MlegSide,
    pub position_intent: MlegPositionIntent,
}

impl VerticalAction {
    /// `(long leg, short leg)` wire semantics.
    pub fn leg_wire(self) -> (LegWire, LegWire) {
        match self {
            Self::OpenVertical => (
                LegWire {
                    side: MlegSide::Buy,
                    position_intent: MlegPositionIntent::BuyToOpen,
                },
                LegWire {
                    side: MlegSide::Sell,
                    position_intent: MlegPositionIntent::SellToOpen,
                },
            ),
            Self::CloseVertical => (
                LegWire {
                    side: MlegSide::Sell,
                    position_intent: MlegPositionIntent::SellToClose,
                },
                LegWire {
                    side: MlegSide::Buy,
                    position_intent: MlegPositionIntent::BuyToClose,
                },
            ),
        }
    }
}

/// Closed-vocabulary refusal for constructing a vertical / request. Every
/// non-frozen or malformed shape refuses here, before any request -- let alone
/// HTTP body -- exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerticalSpreadBuildRefusal {
    /// A supplied symbol is not a canonical standard OCC contract.
    UnparseableContract { symbol: String, reason: String },
    /// The two contracts do not share one underlying.
    MismatchedUnderlying { long: String, short: String },
    /// The frozen classifier refused the leg pair (mismatched expiry/right,
    /// equal strikes, ...).
    NotAFrozenStructure(OptionStrategyRefusal),
    /// The classifier accepted a non-vertical structure.
    NotAVerticalSpread { structure_code: &'static str },
    /// Quantity, price, time-in-force or client id is not acceptable.
    InvalidOrderField { field: &'static str, detail: String },
}

/// Two contracts proven to form one frozen call/put vertical: same underlying,
/// same expiration, same right, distinct strikes, one long + one short role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedVertical {
    long: OptionContractIdentity,
    short: OptionContractIdentity,
}

fn proposed_leg(side: OptionLegSide, c: &OptionContractIdentity) -> ProposedOptionLeg {
    let (y, m, d) = c.expiration();
    ProposedOptionLeg {
        side,
        right: c.right(),
        strike_micros: c.strike_micros(),
        expiry_yyyymmdd: format!("{y:04}{m:02}{d:02}"),
        qty: QtyMicros::new(1_000_000),
        multiplier: c.multiplier() as i32,
    }
}

impl VerifiedVertical {
    /// Prove `long`/`short` are a frozen vertical. Re-derives the structure
    /// through the frozen classifier; never trusts a pre-built structure.
    pub fn new(
        long: OptionContractIdentity,
        short: OptionContractIdentity,
    ) -> Result<Self, VerticalSpreadBuildRefusal> {
        if long.underlying() != short.underlying() {
            return Err(VerticalSpreadBuildRefusal::MismatchedUnderlying {
                long: long.underlying().to_string(),
                short: short.underlying().to_string(),
            });
        }
        let proposed = ProposedOptionStrategy {
            underlying: long.underlying().to_string(),
            legs: vec![
                proposed_leg(OptionLegSide::Long, &long),
                proposed_leg(OptionLegSide::Short, &short),
            ],
            covering_share_qty: None,
            cash_secured_collateral_micros: None,
        };
        let structure = classify_option_strategy_structure(&proposed)
            .map_err(VerticalSpreadBuildRefusal::NotAFrozenStructure)?;
        match structure {
            OptionStrategyStructure::CallVerticalSpread { .. }
            | OptionStrategyStructure::PutVerticalSpread { .. } => Ok(Self { long, short }),
            other => Err(VerticalSpreadBuildRefusal::NotAVerticalSpread {
                structure_code: other.structure_code(),
            }),
        }
    }

    /// Parse two canonical OCC symbols and prove them. The symbols must be
    /// EXACTLY the canonical form of the parsed identities (no case folding,
    /// padding or whitespace), so what is verified is what is sent.
    pub fn from_occ_symbols(
        long_symbol: &str,
        short_symbol: &str,
    ) -> Result<Self, VerticalSpreadBuildRefusal> {
        let parse = |s: &str| {
            OptionContractIdentity::parse(s).map_err(|e| {
                VerticalSpreadBuildRefusal::UnparseableContract {
                    symbol: s.to_string(),
                    reason: e.to_string(),
                }
            })
        };
        Self::new(parse(long_symbol)?, parse(short_symbol)?)
    }

    pub fn long(&self) -> &OptionContractIdentity {
        &self.long
    }

    pub fn short(&self) -> &OptionContractIdentity {
        &self.short
    }

    pub fn right(&self) -> OptionRight {
        self.long.right()
    }
}

/// A proposed vertical submission. Fields are public for inspection; a value
/// reaches HTTP only as a [`VerifiedVerticalSpreadSubmission`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitVerticalSpreadRequest {
    pub client_order_id: String,
    /// Units of the overall spread, a positive integer decimal string.
    pub qty: String,
    /// Net spread limit price, a non-zero canonical decimal string.
    pub limit_price: String,
    pub time_in_force: String,
    pub vertical: VerifiedVertical,
    pub action: VerticalAction,
}

/// A request proven by [`build_verified_vertical_spread_request`]. The inner
/// request is private, so `AlpacaBrokerAdapter::submit_vertical_spread`
/// accepting this type is a compile-time guarantee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedVerticalSpreadSubmission(SubmitVerticalSpreadRequest);

impl VerifiedVerticalSpreadSubmission {
    pub fn request(&self) -> &SubmitVerticalSpreadRequest {
        &self.0
    }
}

fn invalid(field: &'static str, detail: impl Into<String>) -> VerticalSpreadBuildRefusal {
    VerticalSpreadBuildRefusal::InvalidOrderField {
        field,
        detail: detail.into(),
    }
}

/// The one sanctioned way to build a submission from a proven vertical.
pub fn build_verified_vertical_spread_request(
    vertical: VerifiedVertical,
    action: VerticalAction,
    client_order_id: &str,
    qty: &str,
    limit_price: &str,
    time_in_force: &str,
) -> Result<VerifiedVerticalSpreadSubmission, VerticalSpreadBuildRefusal> {
    let client_order_id = client_order_id.trim();
    if client_order_id.is_empty() || client_order_id.len() > 128 {
        return Err(invalid("client_order_id", "must be 1-128 characters"));
    }
    let qty_ok = !qty.is_empty()
        && qty.chars().all(|c| c.is_ascii_digit())
        && qty.parse::<u32>().map(|n| n > 0).unwrap_or(false)
        && !qty.starts_with('0');
    if !qty_ok {
        return Err(invalid("qty", format!("{qty:?} is not a positive integer")));
    }
    let price_ok = is_nonzero_decimal(limit_price);
    if !price_ok {
        return Err(invalid(
            "limit_price",
            format!("{limit_price:?} is not a non-zero decimal"),
        ));
    }
    // Conservative MQD policy: only `day` is accepted for mleg until the
    // provider's other time-in-force behavior is proven for this path.
    if time_in_force != "day" {
        return Err(invalid(
            "time_in_force",
            format!("{time_in_force:?} is not accepted for mleg (only \"day\")"),
        ));
    }
    Ok(VerifiedVerticalSpreadSubmission(
        SubmitVerticalSpreadRequest {
            client_order_id: client_order_id.to_string(),
            qty: qty.to_string(),
            limit_price: limit_price.trim().to_string(),
            time_in_force: time_in_force.to_string(),
            vertical,
            action,
        },
    ))
}

/// `-?digits[.digits]` with a non-zero value and at most 9 fractional digits.
fn is_nonzero_decimal(raw: &str) -> bool {
    let t = raw.trim();
    let body = t.strip_prefix('-').unwrap_or(t);
    let mut parts = body.splitn(2, '.');
    let int = parts.next().unwrap_or("");
    let frac = parts.next();
    let frac_ok = match frac {
        None => true,
        Some(f) => !f.is_empty() && f.len() <= 9 && f.chars().all(|c| c.is_ascii_digit()),
    };
    let digits_ok = !int.is_empty() && int.chars().all(|c| c.is_ascii_digit()) && frac_ok;
    digits_ok && body.chars().any(|c| c.is_ascii_digit() && c != '0')
}

/// Construct the single request body for both legs: exactly two entries, each
/// at `ratio_qty = "1"`, symbols from the verified identities, side/intent from
/// the typed action. Never split into separate submissions.
pub fn build_mleg_submit_body(req: &SubmitVerticalSpreadRequest) -> AlpacaMlegSubmitBody {
    let (long_wire, short_wire) = req.action.leg_wire();
    let leg = |c: &OptionContractIdentity, w: LegWire| AlpacaMlegLegBody {
        symbol: c.occ_symbol(),
        side: w.side.as_str().to_string(),
        position_intent: w.position_intent.as_str().to_string(),
        ratio_qty: "1".to_string(),
    };
    AlpacaMlegSubmitBody {
        order_class: "mleg".to_string(),
        qty: req.qty.clone(),
        order_type: "limit".to_string(),
        time_in_force: req.time_in_force.clone(),
        limit_price: req.limit_price.clone(),
        legs: vec![
            leg(req.vertical.long(), long_wire),
            leg(req.vertical.short(), short_wire),
        ],
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

/// Closed-vocabulary refusal for a provider response that does not authenticate
/// against the request that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MlegRefusal {
    ParentIdMissing,
    ClientOrderIdMismatch {
        expected: String,
        actual: String,
    },
    OrderClassMismatch {
        actual: String,
    },
    QtyMismatch {
        expected: String,
        actual: String,
    },
    LegCountMismatch {
        expected: usize,
        actual: usize,
    },
    LegIdentityMismatch {
        detail: String,
    },
    LegSideMismatch {
        symbol: String,
        expected: String,
        actual: String,
    },
}

/// Canonical `(integer digits, fractional digits)` of an unsigned decimal
/// (leading/trailing zeros stripped); `None` when not a plain decimal.
fn canonical_unsigned_decimal(raw: &str) -> Option<(String, String)> {
    let t = raw.trim();
    let mut parts = t.splitn(2, '.');
    let int = parts.next()?;
    let frac = parts.next().unwrap_or("");
    if int.is_empty()
        || !int.chars().all(|c| c.is_ascii_digit())
        || !frac.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let int = int.trim_start_matches('0');
    Some((
        if int.is_empty() { "0" } else { int }.to_string(),
        frac.trim_end_matches('0').to_string(),
    ))
}

fn same_decimal(a: &str, b: &str) -> bool {
    match (canonical_unsigned_decimal(a), canonical_unsigned_decimal(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Authenticate and normalize a provider response against its request. Legs
/// are matched by exact contract symbol, never by array position.
pub fn normalize_mleg_submit_response(
    resp: AlpacaMlegSubmitResponse,
    req: &SubmitVerticalSpreadRequest,
) -> Result<SubmittedVerticalSpread, MlegRefusal> {
    if resp.id.trim().is_empty() {
        return Err(MlegRefusal::ParentIdMissing);
    }
    if resp.client_order_id != req.client_order_id {
        return Err(MlegRefusal::ClientOrderIdMismatch {
            expected: req.client_order_id.clone(),
            actual: resp.client_order_id,
        });
    }
    if resp.order_class != "mleg" {
        return Err(MlegRefusal::OrderClassMismatch {
            actual: resp.order_class,
        });
    }
    if !same_decimal(&resp.qty, &req.qty) {
        return Err(MlegRefusal::QtyMismatch {
            expected: req.qty.clone(),
            actual: resp.qty,
        });
    }
    if resp.legs.len() != 2 {
        return Err(MlegRefusal::LegCountMismatch {
            expected: 2,
            actual: resp.legs.len(),
        });
    }

    let (long_wire, short_wire) = req.action.leg_wire();
    let expected = [
        (req.vertical.long().occ_symbol(), long_wire),
        (req.vertical.short().occ_symbol(), short_wire),
    ];
    let mut found = Vec::with_capacity(2);
    for (symbol, wire) in &expected {
        let matches: Vec<_> = resp.legs.iter().filter(|l| &l.symbol == symbol).collect();
        let leg = match matches.as_slice() {
            [one] => *one,
            [] => {
                return Err(MlegRefusal::LegIdentityMismatch {
                    detail: format!("no response leg matched submitted contract {symbol:?}"),
                })
            }
            _ => {
                return Err(MlegRefusal::LegIdentityMismatch {
                    detail: format!("contract {symbol:?} appears more than once in the response"),
                })
            }
        };
        if leg.side != wire.side.as_str() {
            return Err(MlegRefusal::LegSideMismatch {
                symbol: symbol.clone(),
                expected: wire.side.as_str().to_string(),
                actual: leg.side.clone(),
            });
        }
        if let Some(intent) = leg.position_intent.as_deref() {
            if intent != wire.position_intent.as_str() {
                return Err(MlegRefusal::LegIdentityMismatch {
                    detail: format!(
                        "leg {symbol:?} position_intent {intent:?} != submitted {:?}",
                        wire.position_intent.as_str()
                    ),
                });
            }
        }
        if leg.id.trim().is_empty() {
            return Err(MlegRefusal::LegIdentityMismatch {
                detail: format!("leg {symbol:?} has no broker id"),
            });
        }
        found.push(leg.clone());
    }
    // Two distinct submitted contracts each matched exactly one response leg
    // and the response has exactly two legs: no missing, extra or duplicate.
    let short_resp = found.pop().expect("two legs");
    let long_resp = found.pop().expect("two legs");
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

    const LONG_CALL: &str = "AAPL260619C00500000";
    const SHORT_CALL: &str = "AAPL260619C00510000";
    const LONG_PUT: &str = "AAPL260619P00510000";
    const SHORT_PUT: &str = "AAPL260619P00500000";

    fn submission(
        long: &str,
        short: &str,
        action: VerticalAction,
    ) -> VerifiedVerticalSpreadSubmission {
        build_verified_vertical_spread_request(
            VerifiedVertical::from_occ_symbols(long, short).unwrap(),
            action,
            "co-1",
            "1",
            "2.50",
            "day",
        )
        .unwrap()
    }

    #[test]
    fn open_and_close_call_and_put_verticals_derive_exact_wire_legs() {
        use VerticalAction::{CloseVertical, OpenVertical};
        // (long, short, action, long side/intent, short side/intent)
        let cases = [
            (
                LONG_CALL,
                SHORT_CALL,
                OpenVertical,
                ("buy", "buy_to_open"),
                ("sell", "sell_to_open"),
            ),
            (
                LONG_PUT,
                SHORT_PUT,
                OpenVertical,
                ("buy", "buy_to_open"),
                ("sell", "sell_to_open"),
            ),
            (
                LONG_CALL,
                SHORT_CALL,
                CloseVertical,
                ("sell", "sell_to_close"),
                ("buy", "buy_to_close"),
            ),
            (
                LONG_PUT,
                SHORT_PUT,
                CloseVertical,
                ("sell", "sell_to_close"),
                ("buy", "buy_to_close"),
            ),
        ];
        for (long, short, action, lw, sw) in cases {
            let sub = submission(long, short, action);
            let body = build_mleg_submit_body(sub.request());
            assert_eq!(body.order_class, "mleg");
            assert_eq!(body.legs.len(), 2, "one parent, exactly two legs");
            assert_eq!(body.legs[0].symbol, long);
            assert_eq!(body.legs[1].symbol, short);
            assert_eq!(
                (
                    body.legs[0].side.as_str(),
                    body.legs[0].position_intent.as_str()
                ),
                lw,
                "{long} {action:?}"
            );
            assert_eq!(
                (
                    body.legs[1].side.as_str(),
                    body.legs[1].position_intent.as_str()
                ),
                sw,
                "{short} {action:?}"
            );
            assert!(body.legs.iter().all(|l| l.ratio_qty == "1"));
        }
    }

    #[test]
    fn a_vertical_is_only_the_frozen_shape_and_only_from_canonical_occ_symbols() {
        let refuse = |l: &str, s: &str| VerifiedVertical::from_occ_symbols(l, s).unwrap_err();
        assert!(matches!(
            refuse(LONG_CALL, "MSFT260619C00510000"),
            VerticalSpreadBuildRefusal::MismatchedUnderlying { .. }
        ));
        assert!(
            matches!(
                refuse(LONG_CALL, "AAPL260717C00510000"),
                VerticalSpreadBuildRefusal::NotAFrozenStructure(_)
            ),
            "mismatched expiration"
        );
        assert!(
            matches!(
                refuse(LONG_CALL, LONG_PUT),
                VerticalSpreadBuildRefusal::NotAFrozenStructure(_)
            ),
            "call + put"
        );
        assert!(
            matches!(
                refuse(LONG_CALL, LONG_CALL),
                VerticalSpreadBuildRefusal::NotAFrozenStructure(_)
            ),
            "same strike / same contract"
        );
        // Non-canonical / wrong-shape strings cannot even name a contract.
        for bad in [
            "AAPL260619C0050000",
            "aapl260619C00510000",
            "AAPL260619X00510000",
            "AAPL260631C00510000",
            "AAPL",
        ] {
            assert!(
                matches!(
                    refuse(bad, SHORT_CALL),
                    VerticalSpreadBuildRefusal::UnparseableContract { .. }
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn order_fields_are_validated() {
        let v = || VerifiedVertical::from_occ_symbols(LONG_CALL, SHORT_CALL).unwrap();
        let build = |co: &str, qty: &str, px: &str, tif: &str| {
            build_verified_vertical_spread_request(
                v(),
                VerticalAction::OpenVertical,
                co,
                qty,
                px,
                tif,
            )
        };
        assert!(build("co", "1", "2.50", "day").is_ok());
        assert!(
            build("co", "3", "-0.25", "day").is_ok(),
            "credit spread price"
        );
        for (co, qty, px, tif) in [
            ("", "1", "2.50", "day"),
            ("co", "0", "2.50", "day"),
            ("co", "1.5", "2.50", "day"),
            ("co", "abc", "2.50", "day"),
            ("co", "1", "0", "day"),
            ("co", "1", "x", "day"),
            ("co", "1", "2.50", "gtc"),
        ] {
            assert!(
                build(co, qty, px, tif).is_err(),
                "{co:?} {qty:?} {px:?} {tif:?}"
            );
        }
    }

    fn resp_leg(id: &str, symbol: &str, side: &str) -> AlpacaMlegLegResponse {
        AlpacaMlegLegResponse {
            id: id.to_string(),
            symbol: symbol.to_string(),
            side: side.to_string(),
            status: "accepted".to_string(),
            position_intent: None,
        }
    }

    fn good_resp(action: VerticalAction) -> AlpacaMlegSubmitResponse {
        let (lw, sw) = action.leg_wire();
        AlpacaMlegSubmitResponse {
            id: "parent-1".to_string(),
            client_order_id: "co-1".to_string(),
            order_class: "mleg".to_string(),
            qty: "1".to_string(),
            status: "accepted".to_string(),
            legs: vec![
                resp_leg("leg-s", SHORT_CALL, sw.side.as_str()),
                resp_leg("leg-l", LONG_CALL, lw.side.as_str()),
            ],
        }
    }

    #[test]
    fn a_matching_response_is_authenticated_and_matched_by_symbol_not_position() {
        for action in [VerticalAction::OpenVertical, VerticalAction::CloseVertical] {
            let sub = submission(LONG_CALL, SHORT_CALL, action);
            let out = normalize_mleg_submit_response(good_resp(action), sub.request()).unwrap();
            assert_eq!(out.broker_parent_order_id, "parent-1");
            assert_eq!(out.long_leg.broker_leg_order_id, "leg-l");
            assert_eq!(out.short_leg.broker_leg_order_id, "leg-s");
        }
    }

    #[test]
    fn every_response_disagreement_is_refused() {
        let action = VerticalAction::OpenVertical;
        let sub = submission(LONG_CALL, SHORT_CALL, action);
        let req = sub.request();
        let refuse = |f: &dyn Fn(&mut AlpacaMlegSubmitResponse)| {
            let mut r = good_resp(action);
            f(&mut r);
            normalize_mleg_submit_response(r, req).unwrap_err()
        };
        assert!(matches!(
            refuse(&|r| r.id = " ".into()),
            MlegRefusal::ParentIdMissing
        ));
        assert!(matches!(
            refuse(&|r| r.client_order_id = "other".into()),
            MlegRefusal::ClientOrderIdMismatch { .. }
        ));
        assert!(matches!(
            refuse(&|r| r.order_class = "simple".into()),
            MlegRefusal::OrderClassMismatch { .. }
        ));
        assert!(matches!(
            refuse(&|r| r.qty = "2".into()),
            MlegRefusal::QtyMismatch { .. }
        ));
        assert!(
            matches!(
                refuse(&|r| {
                    r.legs.pop();
                }),
                MlegRefusal::LegCountMismatch { actual: 1, .. }
            ),
            "missing leg"
        );
        assert!(
            matches!(
                refuse(&|r| r.legs.push(resp_leg("x", "AAPL260619C00520000", "buy"))),
                MlegRefusal::LegCountMismatch { actual: 3, .. }
            ),
            "extra leg"
        );
        assert!(
            matches!(
                refuse(&|r| r.legs[1].symbol = "AAPL260619C00999000".into()),
                MlegRefusal::LegIdentityMismatch { .. }
            ),
            "wrong returned symbol"
        );
        assert!(
            matches!(
                refuse(&|r| r.legs[0].symbol = LONG_CALL.into()),
                MlegRefusal::LegIdentityMismatch { .. }
            ),
            "duplicate leg"
        );
        assert!(
            matches!(
                refuse(&|r| r.legs[1].side = "sell".into()),
                MlegRefusal::LegSideMismatch { .. }
            ),
            "wrong returned side"
        );
        assert!(
            matches!(
                refuse(&|r| r.legs[1].position_intent = Some("sell_to_close".into())),
                MlegRefusal::LegIdentityMismatch { .. }
            ),
            "wrong returned intent"
        );
    }

    #[test]
    fn a_close_response_with_open_sides_is_refused() {
        let sub = submission(LONG_CALL, SHORT_CALL, VerticalAction::CloseVertical);
        // Provider echoes OPEN sides for a CLOSE request.
        let err =
            normalize_mleg_submit_response(good_resp(VerticalAction::OpenVertical), sub.request())
                .unwrap_err();
        assert!(matches!(err, MlegRefusal::LegSideMismatch { .. }));
    }
}
