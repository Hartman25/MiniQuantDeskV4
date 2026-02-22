//! Internal → broker order-ID mapping — Patch L9
//!
//! # Problem
//!
//! After a successful broker submit, the broker assigns its own order identifier
//! (`broker_order_id` in `BrokerSubmitResponse`).  Cancel and replace operations
//! MUST target the **broker** ID — sending the internal ID to a live broker
//! will silently cancel the wrong order (or return a 404).
//!
//! # Solution
//!
//! `BrokerOrderMap` is the lightweight in-memory store that maps:
//!
//! ```text
//! internal_order_id  →  broker_order_id
//! ```
//!
//! Callers must:
//! 1. Call [`BrokerOrderMap::register`] immediately after every successful submit,
//!    passing the `order_id` from the request and the `broker_order_id` from the
//!    response.
//! 2. Call [`BrokerOrderMap::broker_id`] before every cancel/replace to obtain
//!    the correct broker target.  A `None` result means the mapping is missing
//!    and the operation MUST be aborted — do not fabricate or guess an ID.
//! 3. Call [`BrokerOrderMap::deregister`] when an order reaches a terminal state
//!    (filled, cancel-ack, rejected) to keep the map bounded.
//!
//! # Thread-safety
//! `BrokerOrderMap` is not `Sync`. If you need concurrent access, wrap it in
//! a `Mutex` or `RwLock`.  The intentional design keeps this struct simple and
//! pure; synchronization is the caller's responsibility.

use std::collections::HashMap;

/// Maps internal order IDs to broker-assigned order IDs.
///
/// See the [module documentation][self] for the usage contract.
#[derive(Clone, Debug, Default)]
pub struct BrokerOrderMap {
    /// internal_order_id → broker_order_id
    map: HashMap<String, String>,
}

impl BrokerOrderMap {
    /// Create an empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a mapping after a successful broker submit.
    ///
    /// `internal_id` must be the `order_id` from the `BrokerSubmitRequest`.
    /// `broker_id` must be the `broker_order_id` from `BrokerSubmitResponse`.
    ///
    /// If the same `internal_id` is registered twice (e.g. an idempotent retry
    /// that the broker accepted again), the mapping is overwritten with the new
    /// `broker_id`.
    pub fn register(&mut self, internal_id: impl Into<String>, broker_id: impl Into<String>) {
        self.map.insert(internal_id.into(), broker_id.into());
    }

    /// Look up the broker-assigned order ID for a given internal order ID.
    ///
    /// Returns `None` if the ID is unknown (never submitted successfully, or
    /// already deregistered).  Callers MUST treat `None` as an error and MUST
    /// NOT fabricate a broker ID.
    pub fn broker_id(&self, internal_id: &str) -> Option<&str> {
        self.map.get(internal_id).map(|s| s.as_str())
    }

    /// Remove a mapping when an order reaches a terminal state.
    ///
    /// Call this after a fill, cancel-ack, or reject to keep the map bounded.
    /// Silently ignores unknown `internal_id` values.
    pub fn deregister(&mut self, internal_id: &str) {
        self.map.remove(internal_id);
    }

    /// Number of live mappings currently tracked.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// `true` if no mappings are currently live.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}
