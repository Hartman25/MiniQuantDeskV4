//! C3 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION): deterministic
//! identity types for the IBKR adapter foundation.
//!
//! Three distinct identities, deliberately never conflated:
//! - [`IbkrDeploymentIdentity`]: which account/session this adapter instance
//!   targets (account/deployment identity).
//! - [`IbkrOrderIdentity`]: IBKR's own two-tier order identity — a
//!   session-scoped `order_id` (assigned by the client from TWS's
//!   `nextValidId`, meaningful only within one connected session) and a
//!   `perm_id` (IBKR's permanent, broker-native order identifier, stable
//!   across reconnects and order modifications for the life of the order).
//!   Never treated as interchangeable: a session-scoped `order_id` becomes
//!   stale across a reconnect; `perm_id` is the durable identity.
//! - [`IbkrExecutionIdentity`]: IBKR's `exec_id` — the natural,
//!   broker-native idempotency key for one execution/fill, analogous to
//!   Alpaca's `activity_id` (see `mqk-broker-alpaca::fee_attribution`'s own
//!   doc on this exact pattern).

use sha2::{Digest, Sha256};

/// Which IBKR account/session this adapter instance targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IbkrDeploymentIdentity {
    /// IBKR account code, e.g. `"DU1234567"` for a paper account.
    pub account_id: String,
    /// TWS API client id — identifies this API connection among possibly
    /// several simultaneous connections to the same TWS/Gateway instance.
    pub client_id: i32,
    pub host: String,
    pub port: u16,
}

impl IbkrDeploymentIdentity {
    /// Deterministic, versioned identity fingerprint — mirrors
    /// `crypto_execution_policy::crypto_execution_policy_fingerprint`'s
    /// exact shape (canonical pipe-delimited string, SHA-256). A changed
    /// account/client/host/port is a genuinely different deployment
    /// identity and must produce a different fingerprint.
    pub fn fingerprint(&self) -> String {
        let canonical = format!(
            "mqk.ibkr-deployment-identity.v1|account_id={}|client_id={}|host={}|port={}",
            self.account_id.trim(),
            self.client_id,
            self.host.trim(),
            self.port
        );
        let mut hasher = Sha256::new();
        hasher.update(canonical.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

/// IBKR's two-tier order identity. Never derive one from the other — they
/// are assigned by TWS independently and mean different things.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IbkrOrderIdentity {
    /// Session-scoped order id (TWS `orderId`). Stale across a reconnect —
    /// never used as a durable cross-session key.
    pub order_id: i64,
    /// IBKR's permanent order identifier (TWS `permId`). Stable for the
    /// life of the order across reconnects and modifications — the durable
    /// broker order identity.
    pub perm_id: i64,
}

/// IBKR's natural execution idempotency key (TWS `execId`). An execution
/// must never be applied to durable evidence twice — mirrors
/// `mqk-broker-alpaca::fee_attribution::FeeAttributionRecord`'s own
/// `activity_id` contract exactly.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IbkrExecutionIdentity(pub String);

impl IbkrExecutionIdentity {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(account_id: &str, client_id: i32, host: &str, port: u16) -> IbkrDeploymentIdentity {
        IbkrDeploymentIdentity {
            account_id: account_id.to_string(),
            client_id,
            host: host.to_string(),
            port,
        }
    }

    #[test]
    fn fingerprint_is_deterministic() {
        let a = identity("DU1234567", 1, "127.0.0.1", 7497);
        let b = identity("DU1234567", 1, "127.0.0.1", 7497);
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn different_account_id_changes_fingerprint() {
        let a = identity("DU1234567", 1, "127.0.0.1", 7497);
        let b = identity("DU7654321", 1, "127.0.0.1", 7497);
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn different_client_id_changes_fingerprint() {
        let a = identity("DU1234567", 1, "127.0.0.1", 7497);
        let b = identity("DU1234567", 2, "127.0.0.1", 7497);
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn different_port_changes_fingerprint_paper_vs_live_gateway() {
        // 7497 is TWS paper, 7496 is TWS live, 4002/4001 are IB Gateway
        // paper/live -- a port mismatch here is a genuinely different
        // deployment target and must never share an identity.
        let paper = identity("DU1234567", 1, "127.0.0.1", 7497);
        let live = identity("DU1234567", 1, "127.0.0.1", 7496);
        assert_ne!(paper.fingerprint(), live.fingerprint());
    }

    #[test]
    fn order_identity_distinguishes_order_id_from_perm_id() {
        let a = IbkrOrderIdentity {
            order_id: 5,
            perm_id: 100,
        };
        let b = IbkrOrderIdentity {
            order_id: 5,
            perm_id: 200,
        };
        assert_ne!(
            a, b,
            "two orders sharing a stale session order_id but different perm_id must be distinct"
        );
    }
}
