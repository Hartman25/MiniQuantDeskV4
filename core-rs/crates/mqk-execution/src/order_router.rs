//! Order Router: crate-private broker delegation layer.
//!
//! This module is intentionally NOT re-exported from `lib.rs`.
//! External crates must use [`crate::BrokerGateway`], which is the only
//! public path to broker operations and enforces all gate checks.
//!
//! `OrderRouter` and its methods are `pub(crate)` — they cannot be
//! constructed or called from outside `mqk-execution`.

use crate::broker_error::BrokerError;
pub use mqk_schemas::AssetClass;
pub use mqk_schemas::QtyMicros;

/// Convenience alias used throughout this module.
type Result<T> = std::result::Result<T, BrokerError>;

// ---------------------------------------------------------------------------
// BrokerEvent — canonical inbound broker event type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillIdentityStrength {
    /// Stable, broker-native economic fill identity.
    StrongBrokerNative,
    /// No broker-native fill identity was provided; only message identity exists.
    WeakMessageDerived,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BrokerEventIdentity {
    pub broker_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker_fill_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_identity_strength: Option<FillIdentityStrength>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker_sequence_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker_timestamp: Option<String>,
}

/// A broker-sourced lifecycle event for an in-flight order.
///
/// Produced by [`BrokerAdapter::fetch_events`] and persisted to `oms_inbox`
/// via JSON serialisation before being applied to the OMS state machine and
/// portfolio.  The `broker_message_id` is the deduplication key for inbox
/// insertion.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrokerEvent {
    Ack {
        broker_message_id: String,
        internal_order_id: String,
        /// RT-9: the broker/exchange-assigned order ID carried in the Ack.
        /// `None` for adapters that do not distinguish broker ID from internal ID
        /// (e.g. paper). `Some` for live adapters where the exchange assigns its
        /// own ID asynchronously. When `Some`, Phase 3b updates `BrokerOrderMap`
        /// with the authoritative ID.
        broker_order_id: Option<String>,
    },
    PartialFill {
        broker_message_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        broker_fill_id: Option<String>,
        internal_order_id: String,
        /// RT-9: broker-assigned order ID associated with this fill event.
        broker_order_id: Option<String>,
        symbol: String,
        side: crate::types::Side,
        /// CUTOVER-1B-OMS-QTY-MICROS-01: fractional-capable. Wire
        /// representation is schema-version-gated — see
        /// [`decode_broker_event`]. Never construct this
        /// variant directly from a raw JSON integer without routing through
        /// that function.
        delta_qty: QtyMicros,
        price_micros: i64,
        fee_micros: i64,
        /// PAPER-SOAK-PARTIAL-FILL-DEDUP-02/04: broker-authoritative
        /// cumulative filled quantity for this order immediately after this
        /// specific execution, when the adapter can establish it exactly.
        ///
        /// This is the one signal both the WS lane (Alpaca's live
        /// `order.filled_qty` push, atomic in the same trade-update message
        /// as the fill) and the REST lane (Alpaca's own `cum_qty` field,
        /// atomic in the same account-activity record as the fill) can
        /// produce with the SAME true value for the SAME physical execution
        /// — unlike `broker_message_id`/`broker_fill_id`, which differ by
        /// transport lane. Both lanes source this value directly from the
        /// broker; neither derives or reconstructs it from a separate,
        /// independently-fetched snapshot. The OMS apply layer
        /// (`OmsOrder::apply_with_watermark`) uses it as an exact economic
        /// identity: a candidate event whose `cum_qty_after` does not exceed
        /// the order's current `filled_qty` has already been reflected and
        /// is a no-op, regardless of which lane redelivered it.
        ///
        /// `None` when the adapter cannot establish this value at all (e.g.
        /// a paper/test broker that never supplies one, or the operator-gated
        /// halted-run REST repair path, which has its own separate,
        /// confirmation-required safety story — see
        /// `mqk-daemon::routes::repair`). PAPER-SOAK-PARTIAL-FILL-DEDUP-04:
        /// the live Alpaca REST lane (`mqk-broker-alpaca::fetch_events`)
        /// specifically does NOT use `None` for an ambiguous same-page
        /// PARTIAL_FILL — a REST partial whose broker-native `cum_qty` is
        /// missing/unparseable fails the whole page closed instead, because
        /// `event_id`-only fallback dedup cannot safely disambiguate a
        /// cross-lane duplicate. `None` from that adapter therefore only
        /// ever means "no PARTIAL_FILL activity was in play," never
        /// "identity could not be proven."
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cum_qty_after: Option<QtyMicros>,
    },
    Fill {
        broker_message_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        broker_fill_id: Option<String>,
        internal_order_id: String,
        /// RT-9: broker-assigned order ID associated with this fill event.
        broker_order_id: Option<String>,
        symbol: String,
        side: crate::types::Side,
        /// CUTOVER-1B-OMS-QTY-MICROS-01: see [`PartialFill::delta_qty`] doc.
        delta_qty: QtyMicros,
        price_micros: i64,
        fee_micros: i64,
    },
    CancelAck {
        broker_message_id: String,
        internal_order_id: String,
        /// RT-9: broker-assigned order ID for the cancelled order.
        broker_order_id: Option<String>,
    },
    CancelReject {
        broker_message_id: String,
        internal_order_id: String,
        /// RT-9: broker-assigned order ID for the rejected cancel.
        broker_order_id: Option<String>,
    },
    ReplaceAck {
        broker_message_id: String,
        internal_order_id: String,
        /// RT-9: broker-assigned order ID for the replaced order.
        broker_order_id: Option<String>,
        /// P1-03: authoritative post-replace total quantity.
        /// Equals filled_qty_at_replace + new_open_leaves.
        /// Used by the OMS to update `OmsOrder::total_qty` so subsequent fills
        /// validate against the amended order size rather than the original.
        /// CUTOVER-1B-OMS-QTY-MICROS-01: fractional-capable.
        new_total_qty: QtyMicros,
    },
    ReplaceReject {
        broker_message_id: String,
        internal_order_id: String,
        /// RT-9: broker-assigned order ID for the rejected replace.
        broker_order_id: Option<String>,
    },
    Reject {
        broker_message_id: String,
        internal_order_id: String,
        /// RT-9: broker-assigned order ID for the rejected order.
        broker_order_id: Option<String>,
    },
}

impl BrokerEvent {
    /// The deduplication key used for inbox insertion.
    pub fn broker_message_id(&self) -> &str {
        match self {
            Self::Ack {
                broker_message_id, ..
            }
            | Self::PartialFill {
                broker_message_id, ..
            }
            | Self::Fill {
                broker_message_id, ..
            }
            | Self::CancelAck {
                broker_message_id, ..
            }
            | Self::CancelReject {
                broker_message_id, ..
            }
            | Self::ReplaceAck {
                broker_message_id, ..
            }
            | Self::ReplaceReject {
                broker_message_id, ..
            }
            | Self::Reject {
                broker_message_id, ..
            } => broker_message_id.as_str(),
        }
    }

    /// PAPER-SOAK-PARTIAL-FILL-DEDUP-02: broker-authoritative cumulative
    /// filled quantity immediately after this event, when known exactly.
    ///
    /// Only ever `Some` for `PartialFill`. See the field doc on
    /// `PartialFill::cum_qty_after` for the full contract.
    pub fn cum_qty_after(&self) -> Option<QtyMicros> {
        match self {
            Self::PartialFill { cum_qty_after, .. } => *cum_qty_after,
            _ => None,
        }
    }

    /// Optional economic fill identity carried by this event.
    ///
    /// This value is distinct from `broker_message_id`: a broker can emit
    /// multiple transport messages that refer to the same underlying fill.
    /// Adapters should populate this only when the broker supplies a truthful
    /// fill identifier.
    pub fn broker_fill_id(&self) -> Option<&str> {
        match self {
            Self::PartialFill { broker_fill_id, .. } | Self::Fill { broker_fill_id, .. } => {
                broker_fill_id.as_deref()
            }
            _ => None,
        }
    }

    /// Strength classification of fill identity semantics for this event.
    ///
    /// - `Some(StrongBrokerNative)` for fill events that carry a stable
    ///   broker-native economic fill id.
    /// - `Some(WeakMessageDerived)` for fill events that do not carry a
    ///   broker-native fill id and would otherwise fall back to transport
    ///   message identity.
    /// - `None` for non-fill lifecycle events.
    pub fn fill_identity_strength(&self) -> Option<FillIdentityStrength> {
        match self {
            Self::PartialFill { broker_fill_id, .. } | Self::Fill { broker_fill_id, .. } => {
                Some(if broker_fill_id.is_some() {
                    FillIdentityStrength::StrongBrokerNative
                } else {
                    FillIdentityStrength::WeakMessageDerived
                })
            }
            _ => None,
        }
    }

    /// Canonical identity tuple for this broker event.
    pub fn identity(&self) -> BrokerEventIdentity {
        BrokerEventIdentity {
            broker_message_id: self.broker_message_id().to_string(),
            broker_fill_id: self.broker_fill_id().map(ToString::to_string),
            fill_identity_strength: self.fill_identity_strength(),
            broker_sequence_id: None,
            broker_timestamp: None,
        }
    }

    /// The broker/exchange-assigned order ID carried in this event, if any.
    ///
    /// `None` for adapters that do not distinguish broker ID from internal ID
    /// (e.g. paper).  `Some` for live adapters.  Phase 3b uses this value to
    /// update `BrokerOrderMap` when a real Ack arrives.
    pub fn broker_order_id(&self) -> Option<&str> {
        match self {
            Self::Ack {
                broker_order_id, ..
            }
            | Self::PartialFill {
                broker_order_id, ..
            }
            | Self::Fill {
                broker_order_id, ..
            }
            | Self::CancelAck {
                broker_order_id, ..
            }
            | Self::CancelReject {
                broker_order_id, ..
            }
            | Self::ReplaceAck {
                broker_order_id, ..
            }
            | Self::ReplaceReject {
                broker_order_id, ..
            }
            | Self::Reject {
                broker_order_id, ..
            } => broker_order_id.as_deref(),
        }
    }

    /// The system-assigned order ID this event pertains to.
    pub fn internal_order_id(&self) -> &str {
        match self {
            Self::Ack {
                internal_order_id, ..
            }
            | Self::PartialFill {
                internal_order_id, ..
            }
            | Self::Fill {
                internal_order_id, ..
            }
            | Self::CancelAck {
                internal_order_id, ..
            }
            | Self::CancelReject {
                internal_order_id, ..
            }
            | Self::ReplaceAck {
                internal_order_id, ..
            }
            | Self::ReplaceReject {
                internal_order_id, ..
            }
            | Self::Reject {
                internal_order_id, ..
            } => internal_order_id.as_str(),
        }
    }
}

// ---------------------------------------------------------------------------
// CUTOVER-1B-OMS-QTY-MICROS-01: schema-version-gated BrokerEvent decode
// ---------------------------------------------------------------------------

/// Legacy (pre-CUTOVER-1B) wire shape for [`BrokerEvent`], whose quantity
/// fields (`delta_qty`, `cum_qty_after`, `new_total_qty`) were raw
/// whole-unit `i64` integers rather than [`QtyMicros`].
///
/// Used ONLY by [`decode_broker_event`] to decode an `oms_inbox` row whose
/// `schema_version` classifies as
/// [`mqk_db::QuantityUnitEpoch::LegacyWholeUnits`] (missing, or the named
/// legacy version) — never for new writes, and never constructed directly
/// by production code outside this module.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LegacyBrokerEventWire {
    Ack {
        broker_message_id: String,
        internal_order_id: String,
        broker_order_id: Option<String>,
    },
    PartialFill {
        broker_message_id: String,
        #[serde(default)]
        broker_fill_id: Option<String>,
        internal_order_id: String,
        broker_order_id: Option<String>,
        symbol: String,
        side: crate::types::Side,
        delta_qty: i64,
        price_micros: i64,
        fee_micros: i64,
        #[serde(default)]
        cum_qty_after: Option<i64>,
    },
    Fill {
        broker_message_id: String,
        #[serde(default)]
        broker_fill_id: Option<String>,
        internal_order_id: String,
        broker_order_id: Option<String>,
        symbol: String,
        side: crate::types::Side,
        delta_qty: i64,
        price_micros: i64,
        fee_micros: i64,
    },
    CancelAck {
        broker_message_id: String,
        internal_order_id: String,
        broker_order_id: Option<String>,
    },
    CancelReject {
        broker_message_id: String,
        internal_order_id: String,
        broker_order_id: Option<String>,
    },
    ReplaceAck {
        broker_message_id: String,
        internal_order_id: String,
        broker_order_id: Option<String>,
        new_total_qty: i64,
    },
    ReplaceReject {
        broker_message_id: String,
        internal_order_id: String,
        broker_order_id: Option<String>,
    },
    Reject {
        broker_message_id: String,
        internal_order_id: String,
        broker_order_id: Option<String>,
    },
}

/// Upconvert a decoded legacy whole-unit event into the canonical
/// `QtyMicros`-based [`BrokerEvent`]. Fails closed on overflow rather than
/// wrapping or truncating (see `QtyMicros::from_whole_units`).
impl TryFrom<LegacyBrokerEventWire> for BrokerEvent {
    type Error = anyhow::Error;

    fn try_from(legacy: LegacyBrokerEventWire) -> std::result::Result<Self, Self::Error> {
        use anyhow::anyhow;
        let whole = |raw: i64| -> std::result::Result<QtyMicros, Self::Error> {
            QtyMicros::from_whole_units(raw)
                .ok_or_else(|| anyhow!("legacy whole-unit quantity {raw} overflows QtyMicros"))
        };
        Ok(match legacy {
            LegacyBrokerEventWire::Ack {
                broker_message_id,
                internal_order_id,
                broker_order_id,
            } => BrokerEvent::Ack {
                broker_message_id,
                internal_order_id,
                broker_order_id,
            },
            LegacyBrokerEventWire::PartialFill {
                broker_message_id,
                broker_fill_id,
                internal_order_id,
                broker_order_id,
                symbol,
                side,
                delta_qty,
                price_micros,
                fee_micros,
                cum_qty_after,
            } => BrokerEvent::PartialFill {
                broker_message_id,
                broker_fill_id,
                internal_order_id,
                broker_order_id,
                symbol,
                side,
                delta_qty: whole(delta_qty)?,
                price_micros,
                fee_micros,
                cum_qty_after: cum_qty_after.map(whole).transpose()?,
            },
            LegacyBrokerEventWire::Fill {
                broker_message_id,
                broker_fill_id,
                internal_order_id,
                broker_order_id,
                symbol,
                side,
                delta_qty,
                price_micros,
                fee_micros,
            } => BrokerEvent::Fill {
                broker_message_id,
                broker_fill_id,
                internal_order_id,
                broker_order_id,
                symbol,
                side,
                delta_qty: whole(delta_qty)?,
                price_micros,
                fee_micros,
            },
            LegacyBrokerEventWire::CancelAck {
                broker_message_id,
                internal_order_id,
                broker_order_id,
            } => BrokerEvent::CancelAck {
                broker_message_id,
                internal_order_id,
                broker_order_id,
            },
            LegacyBrokerEventWire::CancelReject {
                broker_message_id,
                internal_order_id,
                broker_order_id,
            } => BrokerEvent::CancelReject {
                broker_message_id,
                internal_order_id,
                broker_order_id,
            },
            LegacyBrokerEventWire::ReplaceAck {
                broker_message_id,
                internal_order_id,
                broker_order_id,
                new_total_qty,
            } => BrokerEvent::ReplaceAck {
                broker_message_id,
                internal_order_id,
                broker_order_id,
                new_total_qty: whole(new_total_qty)?,
            },
            LegacyBrokerEventWire::ReplaceReject {
                broker_message_id,
                internal_order_id,
                broker_order_id,
            } => BrokerEvent::ReplaceReject {
                broker_message_id,
                internal_order_id,
                broker_order_id,
            },
            LegacyBrokerEventWire::Reject {
                broker_message_id,
                internal_order_id,
                broker_order_id,
            } => BrokerEvent::Reject {
                broker_message_id,
                internal_order_id,
                broker_order_id,
            },
        })
    }
}

/// The single production entry point for decoding a durable `oms_inbox`
/// `message_json` envelope into a [`BrokerEvent`].
///
/// Routes through `mqk_db`'s schema-version epoch classification
/// ([`mqk_db::quantity_unit_epoch`]) before touching the quantity fields:
/// - [`mqk_db::QuantityUnitEpoch::LegacyWholeUnits`]: decoded via
///   [`LegacyBrokerEventWire`], then upconverted to `QtyMicros` by
///   [`TryFrom`] above.
/// - [`mqk_db::QuantityUnitEpoch::QtyMicros`]: decoded directly — the
///   envelope's raw quantity integers already ARE `QtyMicros`.
///
/// Every caller that turns a durable `message_json` value into a
/// `BrokerEvent` (crash-recovery replay, operator repair tooling) MUST route
/// through this function rather than calling
/// `serde_json::from_value::<BrokerEvent>` directly — a direct call cannot
/// distinguish the two epochs and would silently misinterpret a historical
/// whole-unit row as raw micros (or vice versa).
pub fn decode_broker_event(message_json: &serde_json::Value) -> anyhow::Result<BrokerEvent> {
    match mqk_db::quantity_unit_epoch(message_json)? {
        mqk_db::QuantityUnitEpoch::LegacyWholeUnits => {
            let legacy: LegacyBrokerEventWire = serde_json::from_value(message_json.clone())?;
            BrokerEvent::try_from(legacy)
        }
        mqk_db::QuantityUnitEpoch::QtyMicros => {
            Ok(serde_json::from_value(message_json.clone())?)
        }
    }
}

// ---------------------------------------------------------------------------
// Public request / response types (external crates need these to build reqs)
// ---------------------------------------------------------------------------

/// Broker-agnostic order submission request.
///
/// `limit_price` is in **integer micros** (Patch L9). Use `crate::micros_to_price`
/// only when serialising to a broker REST payload.
#[derive(Debug, Clone)]
pub struct BrokerSubmitRequest {
    pub order_id: String,
    pub symbol: String,
    /// Direction of the order. Quantity is always positive; side carries direction.
    pub side: crate::types::Side,
    /// Fractional-capable (QTY-MICROS-PRODUCTION-CUTOVER-01): 1 unit = 1_000_000
    /// micros. Equity orders remain whole-unit by construction (validated
    /// upstream); Crypto orders may carry a fractional remainder.
    pub quantity: QtyMicros,
    pub order_type: String,
    /// Limit price in integer micros (1 unit = 1_000_000). `None` for market orders.
    pub limit_price: Option<i64>,
    pub time_in_force: String,
    /// Asset class for this order. `BrokerGateway::submit` rejects an asset
    /// class the configured broker adapter does not declare support for
    /// (`BrokerAdapter::supports_asset_class`) before any adapter is invoked
    /// (M5-BROKER-ASSET-CAPABILITY-AUTHORITY-01). The default adapter
    /// capability is `Equity` only; an adapter must explicitly override
    /// `supports_asset_class` to accept anything else.
    pub asset_class: AssetClass,
}

/// Broker-agnostic order submission response.
#[derive(Debug, Clone)]
pub struct BrokerSubmitResponse {
    pub broker_order_id: String,
    pub submitted_at: u64,
    pub status: String,
}

/// Broker-agnostic order cancellation response.
#[derive(Debug, Clone)]
pub struct BrokerCancelResponse {
    pub broker_order_id: String,
    pub cancelled_at: u64,
    pub status: String,
}

/// Broker-agnostic order replacement request.
///
/// `limit_price` is in **integer micros** (Patch L9).
#[derive(Debug, Clone)]
pub struct BrokerReplaceRequest {
    pub broker_order_id: String,
    pub quantity: QtyMicros,
    /// Limit price in integer micros (1 unit = 1_000_000). `None` for market orders.
    pub limit_price: Option<i64>,
    pub time_in_force: String,
}

/// Broker-agnostic order replacement response.
#[derive(Debug, Clone)]
pub struct BrokerReplaceResponse {
    pub broker_order_id: String,
    pub replaced_at: u64,
    pub status: String,
}

// ---------------------------------------------------------------------------
// PATCH A1 — Capability token: compile-time broker bypass prevention
// ---------------------------------------------------------------------------

/// Unforgeable capability token required by every [`BrokerAdapter`] method.
///
/// # Contract
/// - The type is `pub` so external crates can **name** it in trait
///   implementations (`fn submit_order(&self, req: …, _token: &BrokerInvokeToken)`).
/// - The inner field is `pub(crate)`, so external crates **cannot construct**
///   a `BrokerInvokeToken`. The only valid constructor is inside
///   `mqk-execution` itself.
/// - [`crate::BrokerGateway`] is the only internal site that manufactures the
///   token, making it the **single compile-time choke-point** for all broker
///   operations.
///
/// # What external crates can and cannot do
/// ```text
/// ✅  use mqk_execution::BrokerInvokeToken;               // naming: allowed
/// ✅  fn submit_order(…, _token: &BrokerInvokeToken) {…}  // impl trait: allowed
/// ❌  BrokerInvokeToken(())                               // construction: compile error
/// ❌  broker.submit_order(req, &BrokerInvokeToken(()))    // direct call: compile error
/// ```
pub struct BrokerInvokeToken(pub(crate) ());

#[cfg(any(test, feature = "testkit"))]
impl BrokerInvokeToken {
    /// Escape hatch for adapter unit tests outside `mqk-execution`.
    ///
    /// Only available under `#[cfg(test)]` or `feature = "testkit"`.
    /// Must not appear in production code paths.
    pub fn for_test() -> Self {
        Self(())
    }
}

// ---------------------------------------------------------------------------
// BrokerAdapter trait (public — external crates implement this)
// ---------------------------------------------------------------------------

/// Trait that all broker adapters must implement.
///
/// Declared `pub` so external crates can provide implementations (paper,
/// live, mock), but routing always flows through `BrokerGateway`.
///
/// # PATCH A1 — compile-time bypass prevention
/// Every method requires `_token: &BrokerInvokeToken`. External crates can
/// implement the trait (they can name the type) but cannot call the methods
/// (they cannot construct the token). Only `BrokerGateway` creates the token.
pub trait BrokerAdapter {
    /// Declares which asset classes this concrete adapter is capable of
    /// submitting/tracking orders for
    /// (M5-BROKER-ASSET-CAPABILITY-AUTHORITY-01, replacing the former
    /// hardcoded `AssetClass::Equity`-only `MULTI-ASSET-ROUTING-GUARD-01`
    /// check). `BrokerGateway::submit_with_context` consults this — via
    /// `OrderRouter::broker_supports_asset_class` — before evaluating any
    /// gate or invoking any adapter method, and refuses fail-closed with
    /// `GateRefusal::AssetClassDisabled` when it returns `false`.
    ///
    /// The default implementation preserves the exact pre-existing behavior
    /// for every adapter that does not override it: `Equity` only. An
    /// adapter must explicitly opt in to additional asset classes by
    /// overriding this method — there is no implicit or configuration-driven
    /// way to widen capability, keeping the check fail-closed by
    /// construction for any adapter/broker/asset-class combination this
    /// repository has not deliberately implemented.
    fn supports_asset_class(&self, asset_class: AssetClass) -> bool {
        matches!(asset_class, AssetClass::Equity)
    }

    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerSubmitResponse>;

    fn cancel_order(
        &self,
        order_id: &str,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerCancelResponse>;

    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerReplaceResponse>;

    /// Poll the broker for new lifecycle events since `cursor`.
    ///
    /// `cursor` is the last-consumed cursor value returned by a prior call, or
    /// `None` to start from the beginning.  The adapter returns all events that
    /// follow the cursor position together with the new cursor value to pass on
    /// the next call.  Returning `None` as the new cursor means no events were
    /// produced and no cursor advancement is needed.
    ///
    /// The orchestrator persists every event to `oms_inbox` with dedup on
    /// `broker_message_id` BEFORE advancing the cursor in DB, so a crash
    /// between the two steps is safe: on restart the orchestrator re-fetches
    /// from the old cursor and the inbox dedup prevents double-apply.
    fn fetch_events(
        &self,
        cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> Result<(Vec<BrokerEvent>, Option<String>)>;
}

// ---------------------------------------------------------------------------
// OrderRouter (crate-private)
// ---------------------------------------------------------------------------

/// Crate-private router that delegates directly to a broker adapter.
///
/// Cannot be constructed or called from outside `mqk-execution`.
/// All external broker operations must go through `BrokerGateway`.
pub(crate) struct OrderRouter<B: BrokerAdapter> {
    broker: B,
}

impl<B: BrokerAdapter> OrderRouter<B> {
    #[cfg(any(test, feature = "testkit", feature = "runtime-boundary"))]
    pub(crate) fn new(broker: B) -> Self {
        Self { broker }
    }

    /// Forwards to the wrapped adapter's `BrokerAdapter::supports_asset_class`
    /// (M5-BROKER-ASSET-CAPABILITY-AUTHORITY-01). Used by
    /// `BrokerGateway::submit_with_context` to enforce the broker×asset-class
    /// capability gate before any gate evaluation or adapter invocation.
    pub(crate) fn broker_supports_asset_class(&self, asset_class: AssetClass) -> bool {
        self.broker.supports_asset_class(asset_class)
    }

    pub(crate) fn route_submit(&self, req: BrokerSubmitRequest) -> Result<BrokerSubmitResponse> {
        self.broker.submit_order(req, &BrokerInvokeToken(()))
    }

    pub(crate) fn route_cancel(&self, order_id: &str) -> Result<BrokerCancelResponse> {
        self.broker.cancel_order(order_id, &BrokerInvokeToken(()))
    }

    pub(crate) fn route_replace(&self, req: BrokerReplaceRequest) -> Result<BrokerReplaceResponse> {
        self.broker.replace_order(req, &BrokerInvokeToken(()))
    }

    pub(crate) fn route_fetch_events(
        &self,
        cursor: Option<&str>,
    ) -> Result<(Vec<BrokerEvent>, Option<String>)> {
        self.broker.fetch_events(cursor, &BrokerInvokeToken(()))
    }
}

// ---------------------------------------------------------------------------
// Internal unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MockBroker {
        submitted: RefCell<HashMap<String, String>>,
    }

    impl BrokerAdapter for MockBroker {
        fn submit_order(
            &self,
            req: BrokerSubmitRequest,
            _token: &BrokerInvokeToken,
        ) -> Result<BrokerSubmitResponse> {
            self.submitted
                .borrow_mut()
                .insert(req.order_id.clone(), req.symbol.clone());
            Ok(BrokerSubmitResponse {
                broker_order_id: format!("broker-{}", req.order_id),
                submitted_at: 1_000_000,
                status: "acknowledged".to_string(),
            })
        }

        fn cancel_order(
            &self,
            order_id: &str,
            _token: &BrokerInvokeToken,
        ) -> Result<BrokerCancelResponse> {
            Ok(BrokerCancelResponse {
                broker_order_id: format!("broker-{order_id}"),
                cancelled_at: 1_000_000,
                status: "cancelled".to_string(),
            })
        }

        fn replace_order(
            &self,
            req: BrokerReplaceRequest,
            _token: &BrokerInvokeToken,
        ) -> Result<BrokerReplaceResponse> {
            Ok(BrokerReplaceResponse {
                broker_order_id: req.broker_order_id,
                replaced_at: 1_000_000,
                status: "replaced".to_string(),
            })
        }

        fn fetch_events(
            &self,
            _cursor: Option<&str>,
            _token: &BrokerInvokeToken,
        ) -> Result<(Vec<BrokerEvent>, Option<String>)> {
            Ok((vec![], None))
        }
    }

    #[test]
    fn route_submit_delegates_to_broker() {
        let router = OrderRouter::new(MockBroker::default());
        let req = BrokerSubmitRequest {
            order_id: "ord-1".to_string(),
            symbol: "AAPL".to_string(),
            side: crate::types::Side::Buy,
            quantity: QtyMicros::from_whole_units(100).unwrap(),
            order_type: "limit".to_string(),
            limit_price: Some(150_000_000), // $150.00 in micros
            time_in_force: "day".to_string(),
            asset_class: AssetClass::Equity,
        };
        let resp = router.route_submit(req).unwrap();
        assert_eq!(resp.broker_order_id, "broker-ord-1");
        assert_eq!(resp.status, "acknowledged");
    }

    #[test]
    fn route_cancel_delegates_to_broker() {
        let router = OrderRouter::new(MockBroker::default());
        let resp = router.route_cancel("ord-1").unwrap();
        assert_eq!(resp.status, "cancelled");
    }

    #[test]
    fn route_replace_delegates_to_broker() {
        let router = OrderRouter::new(MockBroker::default());
        let req = BrokerReplaceRequest {
            broker_order_id: "broker-ord-1".to_string(),
            quantity: QtyMicros::from_whole_units(200).unwrap(),
            limit_price: Some(151_000_000), // $151.00 in micros
            time_in_force: "gtc".to_string(),
        };
        let resp = router.route_replace(req).unwrap();
        assert_eq!(resp.status, "replaced");
    }

    #[test]
    fn fill_identity_strength_is_strong_when_broker_fill_id_present() {
        let ev = BrokerEvent::Fill {
            broker_message_id: "msg-1".to_string(),
            broker_fill_id: Some("econ-1".to_string()),
            internal_order_id: "ord-1".to_string(),
            broker_order_id: Some("brk-1".to_string()),
            symbol: "AAPL".to_string(),
            side: crate::types::Side::Buy,
            delta_qty: QtyMicros::from_whole_units(1).unwrap(),
            price_micros: 100_000_000,
            fee_micros: 0,
        };
        assert_eq!(
            ev.fill_identity_strength(),
            Some(FillIdentityStrength::StrongBrokerNative)
        );
        assert_eq!(
            ev.identity().fill_identity_strength,
            ev.fill_identity_strength()
        );
    }

    #[test]
    fn fill_identity_strength_is_weak_without_broker_fill_id() {
        let ev = BrokerEvent::PartialFill {
            broker_message_id: "msg-2".to_string(),
            broker_fill_id: None,
            internal_order_id: "ord-1".to_string(),
            broker_order_id: Some("brk-1".to_string()),
            symbol: "AAPL".to_string(),
            side: crate::types::Side::Buy,
            delta_qty: QtyMicros::from_whole_units(1).unwrap(),
            price_micros: 100_000_000,
            fee_micros: 0,
            cum_qty_after: None,
        };
        assert_eq!(
            ev.fill_identity_strength(),
            Some(FillIdentityStrength::WeakMessageDerived)
        );
    }
}

// ---------------------------------------------------------------------------
// CUTOVER-1B-OMS-QTY-MICROS-01: decode_broker_event compatibility tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod decode_broker_event_tests {
    use super::*;
    use serde_json::json;
    use std::str::FromStr;

    /// A legacy whole-unit fill event (no schema_version, as every row
    /// persisted before CUTOVER-1A looked) decodes to the SAME economic
    /// quantity it always represented — 3 shares, expressed as
    /// QtyMicros::from_whole_units(3).
    #[test]
    fn db01_legacy_whole_unit_fill_decodes_identically_to_old_behavior() {
        let legacy = json!({
            "type": "fill",
            "broker_message_id": "m1",
            "broker_fill_id": null,
            "internal_order_id": "ord-1",
            "broker_order_id": "b-1",
            "symbol": "AAPL",
            "side": "Buy",
            "delta_qty": 3,
            "price_micros": 150_000_000,
            "fee_micros": 0
        });
        let event = decode_broker_event(&legacy).unwrap();
        match event {
            BrokerEvent::Fill { delta_qty, .. } => {
                assert_eq!(delta_qty, QtyMicros::from_whole_units(3).unwrap());
            }
            other => panic!("expected Fill, got {other:?}"),
        }
    }

    /// Explicit legacy schema_version=1 decodes identically to the missing
    /// case.
    #[test]
    fn db02_explicit_legacy_version_one_decodes_as_whole_units() {
        let legacy = json!({
            "type": "partial_fill",
            "broker_message_id": "m2",
            "internal_order_id": "ord-2",
            "broker_order_id": "b-2",
            "symbol": "AAPL",
            "side": "Sell",
            "delta_qty": 5,
            "price_micros": 100_000_000,
            "fee_micros": 0,
            "schema_version": mqk_db::LEGACY_WHOLE_UNIT_SCHEMA_VERSION
        });
        let event = decode_broker_event(&legacy).unwrap();
        match event {
            BrokerEvent::PartialFill { delta_qty, .. } => {
                assert_eq!(delta_qty, QtyMicros::from_whole_units(5).unwrap());
            }
            other => panic!("expected PartialFill, got {other:?}"),
        }
    }

    /// A new (current-schema) fractional Crypto fill round-trips exactly —
    /// the raw JSON integer already IS the QtyMicros value.
    #[test]
    fn db03_new_qty_micros_event_round_trips_exactly() {
        let half_btc = QtyMicros::from_str("0.5").unwrap();
        let current = json!({
            "type": "fill",
            "broker_message_id": "m3",
            "broker_fill_id": null,
            "internal_order_id": "ord-3",
            "broker_order_id": "b-3",
            "symbol": "BTC/USD",
            "side": "Buy",
            "delta_qty": half_btc.raw(),
            "price_micros": 60_000_000_000i64,
            "fee_micros": 0,
            "schema_version": mqk_db::MESSAGE_JSON_SCHEMA_VERSION
        });
        let event = decode_broker_event(&current).unwrap();
        match event {
            BrokerEvent::Fill { delta_qty, .. } => {
                assert_eq!(delta_qty, half_btc);
            }
            other => panic!("expected Fill, got {other:?}"),
        }
    }

    /// An unknown/future schema_version refuses fail-closed rather than
    /// guessing an interpretation.
    #[test]
    fn db04_unknown_schema_version_refuses() {
        let future = json!({
            "type": "ack",
            "broker_message_id": "m4",
            "internal_order_id": "ord-4",
            "broker_order_id": null,
            "schema_version": mqk_db::MESSAGE_JSON_SCHEMA_VERSION + 1
        });
        assert!(decode_broker_event(&future).is_err());
    }

    /// Central replay-safety proof: decoding the identical legacy durable row
    /// twice (simulating a crash-recovery replay happening at a later point
    /// in time, potentially in a future build) produces byte-identical
    /// economic quantity both times -- the interpretation is a pure function
    /// of the envelope, not of when/how many times it is decoded.
    #[test]
    fn db05_replay_of_old_durable_row_cannot_change_economic_quantity() {
        let old_row = json!({
            "type": "fill",
            "broker_message_id": "m5",
            "internal_order_id": "ord-5",
            "broker_order_id": "b-5",
            "symbol": "AAPL",
            "side": "Buy",
            "delta_qty": 7,
            "price_micros": 100_000_000,
            "fee_micros": 0
        });
        let first = decode_broker_event(&old_row).unwrap();
        let second = decode_broker_event(&old_row).unwrap();
        let extract = |ev: &BrokerEvent| match ev {
            BrokerEvent::Fill { delta_qty, .. } => *delta_qty,
            _ => panic!("expected Fill"),
        };
        assert_eq!(extract(&first), extract(&second));
        assert_eq!(extract(&first), QtyMicros::from_whole_units(7).unwrap());
    }

    /// cum_qty_after on a legacy PartialFill is also upconverted correctly.
    #[test]
    fn db06_legacy_cum_qty_after_upconverts() {
        let legacy = json!({
            "type": "partial_fill",
            "broker_message_id": "m6",
            "internal_order_id": "ord-6",
            "broker_order_id": "b-6",
            "symbol": "AAPL",
            "side": "Buy",
            "delta_qty": 2,
            "price_micros": 100_000_000,
            "fee_micros": 0,
            "cum_qty_after": 5
        });
        let event = decode_broker_event(&legacy).unwrap();
        assert_eq!(
            event.cum_qty_after(),
            Some(QtyMicros::from_whole_units(5).unwrap())
        );
    }

    /// ReplaceAck's new_total_qty is also upconverted on the legacy path.
    #[test]
    fn db07_legacy_replace_ack_new_total_qty_upconverts() {
        let legacy = json!({
            "type": "replace_ack",
            "broker_message_id": "m7",
            "internal_order_id": "ord-7",
            "broker_order_id": "b-7",
            "new_total_qty": 65
        });
        let event = decode_broker_event(&legacy).unwrap();
        match event {
            BrokerEvent::ReplaceAck { new_total_qty, .. } => {
                assert_eq!(new_total_qty, QtyMicros::from_whole_units(65).unwrap());
            }
            other => panic!("expected ReplaceAck, got {other:?}"),
        }
    }

    /// Non-quantity-bearing events (Ack) decode identically regardless of
    /// epoch, proving the legacy/current split does not disturb events with
    /// no quantity fields to reinterpret.
    #[test]
    fn db08_non_quantity_event_decodes_the_same_in_both_epochs() {
        let legacy = json!({
            "type": "ack",
            "broker_message_id": "m8",
            "internal_order_id": "ord-8",
            "broker_order_id": "b-8"
        });
        let current = json!({
            "type": "ack",
            "broker_message_id": "m8",
            "internal_order_id": "ord-8",
            "broker_order_id": "b-8",
            "schema_version": mqk_db::MESSAGE_JSON_SCHEMA_VERSION
        });
        let ev_legacy = decode_broker_event(&legacy).unwrap();
        let ev_current = decode_broker_event(&current).unwrap();
        assert_eq!(ev_legacy.internal_order_id(), ev_current.internal_order_id());
        assert_eq!(ev_legacy.broker_order_id(), ev_current.broker_order_id());
    }

    /// IR-B1-01: proves the ACTUAL production writer seam, not a hand-built
    /// JSON fixture. `serde_json::to_value(&BrokerEvent::Fill { .. })` is
    /// exactly what every production writer (alpaca_inbound, orchestrator
    /// Phase 3, repair, ws_gap_recovery — see `mqk_db::inbox_insert_deduped_with_identity`'s
    /// single-writer contract) calls before the row reaches
    /// `stamp_message_json_schema_version` and `oms_inbox`. `QtyMicros`
    /// derives `Serialize` as a plain tuple struct with one field, which
    /// serde_json renders as a bare JSON integer (newtype-transparent) — not
    /// an object, not a string. A current-schema writer therefore always
    /// serializes raw `QtyMicros` micro-unit semantics under `delta_qty`,
    /// never old whole-unit semantics, and `decode_broker_event` recovers
    /// the exact original value (including a genuinely fractional Crypto
    /// quantity) once the schema-version stamp is applied.
    #[test]
    fn db09_production_serialize_path_writes_raw_qty_micros_and_round_trips() {
        let qty = QtyMicros::from_str("0.5").unwrap(); // 0.5 BTC — genuinely fractional
        let event = BrokerEvent::Fill {
            broker_message_id: "m9".to_string(),
            broker_fill_id: None,
            internal_order_id: "ord-9".to_string(),
            broker_order_id: Some("b-9".to_string()),
            symbol: "BTC/USD".to_string(),
            side: crate::types::Side::Buy,
            delta_qty: qty,
            price_micros: 60_000_000_000,
            fee_micros: 0,
        };
        let wire = serde_json::to_value(&event).expect("production serialize must succeed");
        assert_eq!(
            wire.get("delta_qty").and_then(|v| v.as_i64()),
            Some(qty.raw()),
            "QtyMicros must serialize as a bare integer equal to its raw micro-unit value, \
             not an object or a whole-unit-rescaled number"
        );

        // Simulate the single writer seam: stamp the current schema version
        // exactly as `mqk_db::inbox_insert_transport_only_deduped` does
        // before the row is durably written.
        let mut stamped = wire.clone();
        stamped["schema_version"] = json!(mqk_db::MESSAGE_JSON_SCHEMA_VERSION);

        let decoded = decode_broker_event(&stamped).expect("current-schema decode must succeed");
        match decoded {
            BrokerEvent::Fill { delta_qty, .. } => {
                assert_eq!(
                    delta_qty, qty,
                    "current-schema writer round-trip must preserve the exact fractional quantity"
                );
            }
            other => panic!("expected Fill, got {other:?}"),
        }
    }
}
