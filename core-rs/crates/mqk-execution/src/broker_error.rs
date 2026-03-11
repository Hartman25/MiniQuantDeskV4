//! Typed error taxonomy for broker adapter failures (Patch A3).
//!
//! All [`crate::BrokerAdapter`] methods return [`BrokerError`] rather than
//! `Box<dyn std::error::Error>`, making the error class a first-class part
//! of the trait.  The orchestrator dispatches outbox row behaviour based on
//! the variant; adapters that cannot distinguish classes should prefer
//! [`BrokerError::Transient`] (conservative: mark FAILED, require operator).
//!
//! ## Outbox dispatch policy
//!
//! | Variant           | Outbox row fate               | Retry?           |
//! |-------------------|-------------------------------|------------------|
//! | `AmbiguousSubmit` | stays `DISPATCHING` (halt)    | Never (operator) |
//! | `Reject`          | → `FAILED`                    | Never            |
//! | `Transient`       | → `FAILED`                    | Never (operator) |
//! | `Transport`       | → `PENDING` (reset)           | Yes (bounded)    |
//! | `RateLimit`       | → `PENDING` (reset)           | Yes (bounded)    |
//! | `AuthSession`     | → `FAILED` (halt + disarm)    | Never (operator) |
/// Typed error class for all [`crate::BrokerAdapter`] method failures.
///
/// Every adapter implementation **must** map its failure modes to one of
/// these variants.  Generic strings or `Box<dyn Error>` are not acceptable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokerError {
    /// Submit reached the broker transport layer but the outcome is unknown.
    ///
    /// Causes: timeout *after* sending the request, partial ACK, connection
    /// drop between sending and receiving the response.
    ///
    /// The order **MAY** have been accepted.  NEVER silently retry.  The
    /// orchestrator transitions the outbox row to `AMBIGUOUS` (A4 explicit
    /// quarantine) and halts+disarms so the Phase-0b restart quarantine gate
    /// blocks further dispatch until an operator verifies whether the order is
    /// live at the broker and explicitly releases via
    /// `outbox_reset_ambiguous_to_pending`.
    AmbiguousSubmit { detail: String },
    /// Broker returned a hard business reject.
    ///
    /// Causes: invalid symbol, insufficient margin, quantity exceeds position
    /// limits, instrument not available for trading, unsupported order type.
    ///
    /// Do **not** retry.  The outbox row is marked `FAILED`.
    Reject { code: String, detail: String },
    /// Transient broker-side error.
    ///
    /// Causes: broker internal error (5xx), exchange connectivity issue,
    /// brief maintenance window.  The request may or may not have been queued
    /// at the exchange.  Treated conservatively: mark outbox row `FAILED`;
    /// requires operator review before re-dispatch.
    Transient { detail: String },
    /// Broker is throttling requests (HTTP 429 / equivalent).
    ///
    /// The request was **not** queued - the broker rejected it before
    /// processing.  Safe to retry after a delay.  The orchestrator resets the
    /// outbox row to `PENDING` for re-dispatch on the next tick.
    ///
    /// Note: per-row bounded retry (max dispatch attempts) will be enforced
    /// once `oms_outbox.dispatch_attempt_count` is added in a future patch.
    RateLimit {
        /// Suggested delay before retrying, if the broker supplied one.
        retry_after_ms: Option<u64>,
        detail: String,
    },
    /// Authentication or session credentials expired or revoked.
    ///
    /// Causes: API key invalid, OAuth token expired, session terminated.
    ///
    /// Do **not** retry without operator intervention.  The outbox row is
    /// marked `FAILED` and the run is halted+disarmed.
    AuthSession { detail: String },
    /// TCP/TLS-level transport failure before the request reached the broker.
    ///
    /// Causes: connection refused, DNS failure, TLS handshake error.  The
    /// request **never left the local host**.  Safe to retry.  The orchestrator
    /// resets the outbox row to `PENDING` for re-dispatch on the next tick.
    ///
    /// Note: per-row bounded retry (max dispatch attempts) will be enforced
    /// once `oms_outbox.dispatch_attempt_count` is added in a future patch.
    Transport { detail: String },
    /// Inbound lifecycle continuity could not be proven for broker events.
    ///
    /// Used by broker adapters whose websocket lifecycle coverage depends on a
    /// durable adapter-owned resume state. If continuity is cold-start
    /// unproven or a gap was detected, the adapter must fail closed and may
    /// supply an updated opaque cursor that the runtime should persist before
    /// returning the error.
    InboundContinuityUnproven {
        detail: String,
        persist_cursor: Option<String>,
    },
}
impl BrokerError {
    /// Whether this error class is safe to auto-retry without operator review.
    ///
    /// `true` ⟹ the request is guaranteed **not** to have reached the
    /// broker; the orchestrator resets the outbox row to `PENDING`.
    ///
    /// `false` ⟹ the row is marked `FAILED` (or left `DISPATCHING` for
    /// `AmbiguousSubmit`) and requires operator action.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            BrokerError::Transport { .. } | BrokerError::RateLimit { .. }
        )
    }
    /// Whether this error requires an immediate halt+disarm of the run.
    ///
    /// `true` ⟹ the orchestrator calls `persist_halt_and_disarm` before
    /// returning the error, preventing any further dispatch.
    pub fn requires_halt(&self) -> bool {
        matches!(
            self,
            BrokerError::AmbiguousSubmit { .. } | BrokerError::AuthSession { .. }
        )
    }
    /// Opaque cursor state that should be persisted even though the fetch
    /// failed closed.
    pub fn persist_cursor(&self) -> Option<&str> {
        match self {
            BrokerError::InboundContinuityUnproven { persist_cursor, .. } => {
                persist_cursor.as_deref()
            }
            _ => None,
        }
    }
}
impl std::fmt::Display for BrokerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrokerError::AmbiguousSubmit { detail } => {
                write!(f, "BROKER_ERROR[AmbiguousSubmit]: {detail}")
            }
            BrokerError::Reject { code, detail } => {
                write!(f, "BROKER_ERROR[Reject] code={code}: {detail}")
            }
            BrokerError::Transient { detail } => {
                write!(f, "BROKER_ERROR[Transient]: {detail}")
            }
            BrokerError::RateLimit {
                retry_after_ms,
                detail,
            } => match retry_after_ms {
                Some(ms) => write!(f, "BROKER_ERROR[RateLimit] retry_after={ms}ms: {detail}"),
                None => write!(f, "BROKER_ERROR[RateLimit]: {detail}"),
            },
            BrokerError::AuthSession { detail } => {
                write!(f, "BROKER_ERROR[AuthSession]: {detail}")
            }
            BrokerError::Transport { detail } => {
                write!(f, "BROKER_ERROR[Transport]: {detail}")
            }
            BrokerError::InboundContinuityUnproven {
                detail,
                persist_cursor,
            } => match persist_cursor {
                Some(cursor) => write!(
                    f,
                    "BROKER_ERROR[InboundContinuityUnproven] cursor={} detail={}",
                    cursor, detail
                ),
                None => write!(f, "BROKER_ERROR[InboundContinuityUnproven]: {detail}"),
            },
        }
    }
}
impl std::error::Error for BrokerError {}
