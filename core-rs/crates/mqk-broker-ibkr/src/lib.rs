//! `mqk-broker-ibkr`: C3 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION)
//! deterministic IBKR TWS-API adapter foundation.
//!
//! Architecture:
//!
//! ```text
//! MQD -> mqk-broker-ibkr -> selected TWS client abstraction (ibapi 4.2.0, C2)
//!     -> operator-managed IB Gateway (real wiring: explicit future work)
//! ```
//!
//! This crate makes **no real network connection anywhere**. [`connection::IbkrTransport`]
//! is an injectable abstraction; this crate defines no production
//! implementation backed by a real `ibapi::Client`, and every test here
//! drives a fake. No Crypto/Live capability is touched by this crate at
//! all — IBKR targets Futures/FX per the mission's canonical integration
//! instruments (MES / EUR-USD), which remain design-only until C4.
//!
//! # Foundation scope
//!
//! This is a **foundation**, not full `mqk_execution::BrokerAdapter`
//! conformance (order submit/cancel/replace wire calls and market-data
//! streaming are explicit future work, matching this repo's established
//! precedent of proving the identity/evidence layer before a live
//! consumer — see `ASSET-CORE-01..05`, and this mission's own B4/B5/B6).
//! What this crate proves now, with real (not stubbed-out) logic and
//! tests:
//!
//! - account/deployment identity ([`identity::IbkrDeploymentIdentity`])
//! - connection state machine + reconnect ([`connection::IbkrConnectionStateMachine`])
//! - order mapping ([`order_mapping::map_submit_request_to_ibkr_order`])
//! - broker order identity ([`identity::IbkrOrderIdentity`]: `perm_id` vs
//!   session-scoped `order_id`)
//! - execution identity ([`identity::IbkrExecutionIdentity`])
//! - event normalization ([`events::normalize_order_status`],
//!   [`events::normalize_execution`])
//! - cursor/replay idempotency (deterministic `broker_message_id`
//!   construction — replaying the identical raw event always produces the
//!   identical dedup key)
//! - unknown event refusal (an unrecognized status/side string fails
//!   closed, never silently defaults)

pub mod connection;
pub mod events;
pub mod identity;
pub mod order_mapping;
