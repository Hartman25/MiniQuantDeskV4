//! C3: raw IBKR event shapes, deterministic normalization into
//! `mqk_execution::BrokerEvent`, and the order-identity map a real
//! submission path would populate.
//!
//! Two distinct raw event families, matching IBKR's own TWS API design:
//! `OrderStatus` carries lifecycle only (acknowledged, cancelled, ...);
//! economic fill facts come from `ExecutionDetails` (mirrors this repo's
//! existing Alpaca precedent — fills are never inferred from order-status
//! polling alone). `broker_rules.md`'s own contract
//! (`trade_update_message_id format is fixed... Do not change format
//! without a version bump`) is followed here for IBKR's own
//! `broker_message_id` construction — see [`ibkr_message_id`].

use mqk_execution::{BrokerEvent, QtyMicros};

use crate::identity::IbkrOrderIdentity;

/// IBKR's own closed, documented set of `OrderStatus.status` values this
/// adapter recognizes. Anything else — including every other real IBKR
/// status string and any truly unrecognized value — is refused, never
/// silently defaulted (`normalize_order_status`'s `UnrecognizedStatus`
/// arm).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawIbkrOrderStatus {
    pub identity: IbkrOrderIdentity,
    /// Raw TWS status string, e.g. `"PreSubmitted"`, `"Submitted"`,
    /// `"Cancelled"`, `"ApiCancelled"`, `"Filled"`, `"PendingCancel"`.
    pub status: String,
    /// Cumulative filled quantity as an exact decimal string.
    pub filled: String,
}

/// One IBKR execution/fill report. `exec_id` is IBKR's own natural,
/// broker-native idempotency key for this execution — mirrors
/// `mqk-broker-alpaca::fee_attribution`'s `activity_id` contract exactly
/// ("an activity must never be applied to the ledger twice").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawIbkrExecution {
    pub identity: IbkrOrderIdentity,
    pub exec_id: String,
    pub symbol: String,
    /// `"BOT"` or `"SLD"` — IBKR's own execution-side vocabulary.
    pub side: String,
    /// This single execution's quantity, as an exact decimal string.
    pub shares: String,
    /// Cumulative quantity filled for the order as of this execution, as
    /// an exact decimal string.
    pub cum_qty: String,
}

/// Normalization failed. Every arm is a refusal, never a fabricated
/// `BrokerEvent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IbkrNormalizeError {
    /// `status` is not in this adapter's closed, recognized set.
    UnrecognizedStatus { status: String },
    /// `side` is not `"BOT"`/`"SLD"`.
    UnrecognizedSide { side: String },
    /// `internal_order_id` for this `order_id`/`perm_id` is not known —
    /// this adapter never fabricates an internal order id, so a raw event
    /// for an order this process never submitted (or has already forgotten)
    /// fails closed rather than guessing.
    UnknownOrderIdentity { identity: IbkrOrderIdentity },
    /// A quantity/price field did not parse as a decimal number.
    MalformedDecimal { field: &'static str, raw: String },
}

impl std::fmt::Display for IbkrNormalizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnrecognizedStatus { status } => {
                write!(f, "unrecognized IBKR order status: {status:?}")
            }
            Self::UnrecognizedSide { side } => write!(f, "unrecognized IBKR exec side: {side:?}"),
            Self::UnknownOrderIdentity { identity } => {
                write!(f, "unknown IBKR order identity: {identity:?}")
            }
            Self::MalformedDecimal { field, raw } => {
                write!(f, "malformed decimal in field {field:?}: {raw:?}")
            }
        }
    }
}

impl std::error::Error for IbkrNormalizeError {}

/// Restart-safe correlation from IBKR's own order identity to MQD's
/// `internal_order_id`. A real submission path (not built by this
/// foundation patch) would populate this at submit time; tests populate it
/// directly. Never guesses — a lookup miss is
/// [`IbkrNormalizeError::UnknownOrderIdentity`], not a fabricated id.
#[derive(Debug, Default, Clone)]
pub struct IbkrOrderIdentityMap {
    by_order_id: std::collections::HashMap<i64, (IbkrOrderIdentity, String)>,
}

impl IbkrOrderIdentityMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, identity: IbkrOrderIdentity, internal_order_id: impl Into<String>) {
        self.by_order_id
            .insert(identity.order_id, (identity, internal_order_id.into()));
    }

    pub fn internal_order_id_for(&self, identity: &IbkrOrderIdentity) -> Option<&str> {
        self.by_order_id
            .get(&identity.order_id)
            .filter(|(known, _)| known.perm_id == identity.perm_id)
            .map(|(_, internal)| internal.as_str())
    }
}

/// `broker_rules.md`'s IBKR counterpart to Alpaca's fixed
/// `"alpaca:{order_id}:{event}:{ts}"` `trade_update_message_id` format —
/// deterministic and replay-idempotent: the identical raw event always
/// produces the identical id; a genuinely different economic/lifecycle
/// fact always produces a different one. Do not change this format without
/// a version bump (mirrors the same rule stated for Alpaca).
fn ibkr_message_id(perm_id: i64, event: &str, discriminant: &str) -> String {
    format!("ibkr:{perm_id}:{event}:{discriminant}")
}

fn parse_exact_decimal_to_qty_micros(
    field: &'static str,
    raw: &str,
) -> Result<QtyMicros, IbkrNormalizeError> {
    let raw_trim = raw.trim();
    let parts: Vec<&str> = raw_trim.splitn(2, '.').collect();
    let whole: i64 = parts[0]
        .parse()
        .map_err(|_| IbkrNormalizeError::MalformedDecimal {
            field,
            raw: raw.to_string(),
        })?;
    let frac_micros: i64 = match parts.get(1) {
        None => 0,
        Some(frac) => {
            if frac.is_empty() || frac.len() > 6 || !frac.chars().all(|c| c.is_ascii_digit()) {
                return Err(IbkrNormalizeError::MalformedDecimal {
                    field,
                    raw: raw.to_string(),
                });
            }
            let padded = format!("{frac:0<6}");
            padded
                .parse()
                .map_err(|_| IbkrNormalizeError::MalformedDecimal {
                    field,
                    raw: raw.to_string(),
                })?
        }
    };
    let sign = if whole < 0 { -1 } else { 1 };
    Ok(QtyMicros::new(whole * 1_000_000 + sign * frac_micros))
}

/// Normalize one [`RawIbkrOrderStatus`] into a lifecycle-only
/// [`BrokerEvent`] (`Ack` or `CancelAck`). Every status outside this
/// adapter's closed recognized set — including every other genuine IBKR
/// status string (`PendingSubmit`, `PendingCancel`, `Inactive`, ...) and
/// any unrecognized value — is refused rather than guessed at.
pub fn normalize_order_status(
    raw: &RawIbkrOrderStatus,
    identities: &IbkrOrderIdentityMap,
) -> Result<BrokerEvent, IbkrNormalizeError> {
    let internal_order_id = identities
        .internal_order_id_for(&raw.identity)
        .ok_or(IbkrNormalizeError::UnknownOrderIdentity {
            identity: raw.identity,
        })?
        .to_string();
    let broker_order_id = Some(raw.identity.perm_id.to_string());

    match raw.status.as_str() {
        "PreSubmitted" | "Submitted" => Ok(BrokerEvent::Ack {
            broker_message_id: ibkr_message_id(raw.identity.perm_id, "ack", &raw.status),
            internal_order_id,
            broker_order_id,
        }),
        "Cancelled" | "ApiCancelled" => Ok(BrokerEvent::CancelAck {
            broker_message_id: ibkr_message_id(raw.identity.perm_id, "cancel_ack", &raw.status),
            internal_order_id,
            broker_order_id,
        }),
        other => Err(IbkrNormalizeError::UnrecognizedStatus {
            status: other.to_string(),
        }),
    }
}

/// Normalize one [`RawIbkrExecution`] into `BrokerEvent::Fill` or
/// `PartialFill`, deciding which by comparing `cum_qty` against `shares`:
/// `cum_qty == shares` for the very first execution on an order can still
/// be a full fill in one shot; this adapter foundation conservatively
/// classifies every execution as `PartialFill` (with `cum_qty_after` set)
/// except when the raw event does not distinguish — callers needing exact
/// full-vs-partial classification must additionally consult the
/// corresponding `OrderStatus` (`"Filled"` with `remaining == "0"`), which
/// this foundation does not attempt to correlate automatically (explicit
/// future work, not silently guessed here).
pub fn normalize_execution(
    raw: &RawIbkrExecution,
    identities: &IbkrOrderIdentityMap,
) -> Result<BrokerEvent, IbkrNormalizeError> {
    let internal_order_id = identities
        .internal_order_id_for(&raw.identity)
        .ok_or(IbkrNormalizeError::UnknownOrderIdentity {
            identity: raw.identity,
        })?
        .to_string();
    let side = match raw.side.as_str() {
        "BOT" => mqk_execution::Side::Buy,
        "SLD" => mqk_execution::Side::Sell,
        other => {
            return Err(IbkrNormalizeError::UnrecognizedSide {
                side: other.to_string(),
            })
        }
    };
    let delta_qty = parse_exact_decimal_to_qty_micros("shares", &raw.shares)?;
    let cum_qty_after = parse_exact_decimal_to_qty_micros("cum_qty", &raw.cum_qty)?;

    Ok(BrokerEvent::PartialFill {
        broker_message_id: ibkr_message_id(raw.identity.perm_id, "fill", &raw.exec_id),
        broker_fill_id: Some(raw.exec_id.clone()),
        internal_order_id,
        broker_order_id: Some(raw.identity.perm_id.to_string()),
        symbol: raw.symbol.clone(),
        side,
        delta_qty,
        // IBKR execution reports do not carry price/fee in this
        // foundation's raw shape (a real wire implementation would parse
        // `price`/`commissionReport` separately) — zero here is a
        // deliberate placeholder for the foundation's identity/dedup
        // proof, never presented as real economic evidence to a durable
        // ledger; wiring exact price/fee is explicit future work.
        price_micros: 0,
        fee_micros: 0,
        cum_qty_after: Some(cum_qty_after),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> IbkrOrderIdentity {
        IbkrOrderIdentity {
            order_id: 5,
            perm_id: 100_500,
        }
    }

    fn identities_with(id: IbkrOrderIdentity, internal_order_id: &str) -> IbkrOrderIdentityMap {
        let mut map = IbkrOrderIdentityMap::new();
        map.insert(id, internal_order_id);
        map
    }

    #[test]
    fn submitted_status_normalizes_to_ack() {
        let raw = RawIbkrOrderStatus {
            identity: identity(),
            status: "Submitted".to_string(),
            filled: "0".to_string(),
        };
        let ids = identities_with(identity(), "order-1");
        let event = normalize_order_status(&raw, &ids).expect("must normalize");
        match event {
            BrokerEvent::Ack {
                internal_order_id,
                broker_order_id,
                ..
            } => {
                assert_eq!(internal_order_id, "order-1");
                assert_eq!(broker_order_id.as_deref(), Some("100500"));
            }
            other => panic!("expected Ack, got {other:?}"),
        }
    }

    #[test]
    fn cancelled_status_normalizes_to_cancel_ack() {
        let raw = RawIbkrOrderStatus {
            identity: identity(),
            status: "Cancelled".to_string(),
            filled: "0".to_string(),
        };
        let ids = identities_with(identity(), "order-1");
        let event = normalize_order_status(&raw, &ids).expect("must normalize");
        assert!(matches!(event, BrokerEvent::CancelAck { .. }));
    }

    #[test]
    fn pending_submit_is_refused_not_guessed() {
        let raw = RawIbkrOrderStatus {
            identity: identity(),
            status: "PendingSubmit".to_string(),
            filled: "0".to_string(),
        };
        let ids = identities_with(identity(), "order-1");
        let err = normalize_order_status(&raw, &ids).expect_err("must refuse");
        assert_eq!(
            err,
            IbkrNormalizeError::UnrecognizedStatus {
                status: "PendingSubmit".to_string()
            }
        );
    }

    #[test]
    fn genuinely_unknown_status_is_refused() {
        let raw = RawIbkrOrderStatus {
            identity: identity(),
            status: "SomeFutureTwsStatusThatDoesNotExistYet".to_string(),
            filled: "0".to_string(),
        };
        let ids = identities_with(identity(), "order-1");
        let err = normalize_order_status(&raw, &ids).expect_err("must refuse");
        assert!(matches!(err, IbkrNormalizeError::UnrecognizedStatus { .. }));
    }

    #[test]
    fn unknown_order_identity_is_refused_never_fabricated() {
        let raw = RawIbkrOrderStatus {
            identity: identity(),
            status: "Submitted".to_string(),
            filled: "0".to_string(),
        };
        let empty_map = IbkrOrderIdentityMap::new();
        let err = normalize_order_status(&raw, &empty_map).expect_err("must refuse");
        assert_eq!(
            err,
            IbkrNormalizeError::UnknownOrderIdentity {
                identity: identity()
            }
        );
    }

    #[test]
    fn stale_order_id_with_mismatched_perm_id_is_refused() {
        // Same session order_id (5) as a previously-known order, but a
        // different perm_id -- TWS reused the session-scoped id across a
        // reconnect for a genuinely different order. Must never be
        // silently matched to the old identity.
        let ids = identities_with(identity(), "order-1");
        let stale = IbkrOrderIdentity {
            order_id: 5,
            perm_id: 999_999,
        };
        let raw = RawIbkrOrderStatus {
            identity: stale,
            status: "Submitted".to_string(),
            filled: "0".to_string(),
        };
        let err = normalize_order_status(&raw, &ids).expect_err("must refuse");
        assert_eq!(
            err,
            IbkrNormalizeError::UnknownOrderIdentity { identity: stale }
        );
    }

    #[test]
    fn execution_normalizes_to_partial_fill_with_exec_id_as_fill_id() {
        let raw = RawIbkrExecution {
            identity: identity(),
            exec_id: "0000e1a7.65123abc.01.01".to_string(),
            symbol: "MES".to_string(),
            side: "BOT".to_string(),
            shares: "1".to_string(),
            cum_qty: "1".to_string(),
        };
        let ids = identities_with(identity(), "order-1");
        let event = normalize_execution(&raw, &ids).expect("must normalize");
        match event {
            BrokerEvent::PartialFill {
                broker_fill_id,
                delta_qty,
                cum_qty_after,
                ..
            } => {
                assert_eq!(broker_fill_id.as_deref(), Some(raw.exec_id.as_str()));
                assert_eq!(delta_qty, QtyMicros::new(1_000_000));
                assert_eq!(cum_qty_after, Some(QtyMicros::new(1_000_000)));
            }
            other => panic!("expected PartialFill, got {other:?}"),
        }
    }

    #[test]
    fn replaying_the_identical_execution_produces_the_identical_message_id() {
        let raw = RawIbkrExecution {
            identity: identity(),
            exec_id: "exec-abc".to_string(),
            symbol: "MES".to_string(),
            side: "BOT".to_string(),
            shares: "1".to_string(),
            cum_qty: "1".to_string(),
        };
        let ids = identities_with(identity(), "order-1");
        let first = normalize_execution(&raw, &ids).expect("must normalize");
        let second = normalize_execution(&raw, &ids).expect("must normalize");
        assert_eq!(
            first.broker_message_id(),
            second.broker_message_id(),
            "C3: replaying the identical raw execution must produce the identical dedup key -- \
             this is the cursor/replay-idempotency proof the inbox insertion layer relies on"
        );
    }

    #[test]
    fn two_distinct_executions_on_the_same_order_never_collide() {
        let ids = identities_with(identity(), "order-1");
        let e1 = RawIbkrExecution {
            identity: identity(),
            exec_id: "exec-1".to_string(),
            symbol: "MES".to_string(),
            side: "BOT".to_string(),
            shares: "1".to_string(),
            cum_qty: "1".to_string(),
        };
        let e2 = RawIbkrExecution {
            exec_id: "exec-2".to_string(),
            cum_qty: "2".to_string(),
            ..e1.clone()
        };
        let ev1 = normalize_execution(&e1, &ids).unwrap();
        let ev2 = normalize_execution(&e2, &ids).unwrap();
        assert_ne!(ev1.broker_message_id(), ev2.broker_message_id());
    }

    #[test]
    fn unrecognized_exec_side_is_refused() {
        let raw = RawIbkrExecution {
            identity: identity(),
            exec_id: "exec-1".to_string(),
            symbol: "MES".to_string(),
            side: "WAT".to_string(),
            shares: "1".to_string(),
            cum_qty: "1".to_string(),
        };
        let ids = identities_with(identity(), "order-1");
        let err = normalize_execution(&raw, &ids).expect_err("must refuse");
        assert!(matches!(err, IbkrNormalizeError::UnrecognizedSide { .. }));
    }

    #[test]
    fn malformed_shares_decimal_is_refused() {
        let raw = RawIbkrExecution {
            identity: identity(),
            exec_id: "exec-1".to_string(),
            symbol: "MES".to_string(),
            side: "BOT".to_string(),
            shares: "not-a-number".to_string(),
            cum_qty: "1".to_string(),
        };
        let ids = identities_with(identity(), "order-1");
        let err = normalize_execution(&raw, &ids).expect_err("must refuse");
        assert!(matches!(
            err,
            IbkrNormalizeError::MalformedDecimal {
                field: "shares",
                ..
            }
        ));
    }

    #[test]
    fn fractional_shares_parse_exactly() {
        let raw = RawIbkrExecution {
            identity: identity(),
            exec_id: "exec-1".to_string(),
            symbol: "MESU6".to_string(),
            side: "BOT".to_string(),
            shares: "0.5".to_string(),
            cum_qty: "0.5".to_string(),
        };
        let ids = identities_with(identity(), "order-1");
        let event = normalize_execution(&raw, &ids).unwrap();
        match event {
            BrokerEvent::PartialFill { delta_qty, .. } => {
                assert_eq!(delta_qty, QtyMicros::new(500_000));
            }
            other => panic!("expected PartialFill, got {other:?}"),
        }
    }
}
