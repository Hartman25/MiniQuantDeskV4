//! D3: the options-lifecycle pending gate over the persisted lifecycle state
//! machine.
//!
//! A lifecycle event fences BOTH its option contract and its underlying until
//! it is `RECONCILED` -- i.e. until the durable economic adjustment has been
//! applied (D2) AND a fresh authenticated broker snapshot agreed with the
//! resulting positions. `PENDING_EVIDENCE`, `PENDING_AMBIGUOUS`,
//! `READY_TO_APPLY` and `APPLIED_AWAITING_BROKER` all keep the gate closed; an
//! "applied marker exists" is never enough. Scope is exactly
//! `(provider account, execution domain)` x `{option symbol, underlying}`: an
//! event on a SPY option never fences AAPL, BTC/USD, another domain or another
//! account.
//!
//! Read-only: makes no write. Restart-safe: the answer is the durable state
//! row, there is no in-memory pending flag to lose.
//!
//! Every economic admission seam consults this gate through
//! [`check_symbol_fence`]: the internal strategy decision path, the manual
//! operator order route and the external strategy-signal route. Whole-account
//! baseline adoption (which overwrites local truth for every symbol at once)
//! uses [`check_account_fence`]. Risk-reducing flatten and order cancels are
//! deliberately NOT gated: they reduce exposure and must remain available while
//! evidence is unresolved.

use std::sync::Arc;

use sqlx::PgPool;

use mqk_db::{
    find_any_unreconciled_lifecycle_event, find_fencing_lifecycle_event, LifecycleEventState,
    OptionLifecycleEventStateRow,
};

use super::option_lifecycle_ingestion::OPTION_LIFECYCLE_EXECUTION_DOMAIN;
use super::OptionLifecycleActivityFetcher;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionLifecycleGateStatus {
    /// No unreconciled lifecycle event fences this scope.
    Clear,
    /// At least one lifecycle event has not reached `RECONCILED`. The caller
    /// MUST fail closed.
    Pending {
        blocking_activity_id: String,
        state: LifecycleEventState,
        option_symbol: String,
        underlying_symbol: Option<String>,
    },
}

impl OptionLifecycleGateStatus {
    /// True iff the scope is fenced and economic action must be refused.
    pub fn must_fail_closed(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }

    fn from_row(row: Option<OptionLifecycleEventStateRow>) -> Self {
        match row {
            None => Self::Clear,
            Some(r) => Self::Pending {
                blocking_activity_id: r.lifecycle_activity_id,
                state: r.state,
                option_symbol: r.option_symbol,
                underlying_symbol: r.underlying_symbol,
            },
        }
    }

    /// Operator-facing explanation of a `Pending` gate.
    pub fn explanation(&self) -> String {
        match self {
            Self::Clear => "clear".to_string(),
            Self::Pending {
                blocking_activity_id,
                state,
                option_symbol,
                underlying_symbol,
            } => format!(
                "unresolved options-lifecycle event {blocking_activity_id} on {option_symbol} \
                 (underlying {}) is {}; the gate clears only at RECONCILED",
                underlying_symbol.as_deref().unwrap_or("unproven"),
                state.as_str()
            ),
        }
    }
}

/// Evaluate the gate for one symbol under `broker_account_id` (the canonical
/// authority key). Fences the symbol when it is a pending event's option
/// contract OR its underlying.
pub async fn evaluate_option_lifecycle_pending_gate(
    pool: &PgPool,
    broker_account_id: &str,
    symbol: &str,
) -> anyhow::Result<OptionLifecycleGateStatus> {
    let row = find_fencing_lifecycle_event(
        pool,
        broker_account_id,
        OPTION_LIFECYCLE_EXECUTION_DOMAIN.as_str(),
        symbol.trim(),
    )
    .await?;
    Ok(OptionLifecycleGateStatus::from_row(row))
}

/// Failure to evaluate the gate at all. Callers refuse (fail closed) on both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateCheckError {
    /// The provider account could not be established (authenticated account
    /// endpoint unavailable): pending evidence for it cannot be looked up.
    AuthorityUnavailable(String),
    Database(String),
}

impl std::fmt::Display for GateCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AuthorityUnavailable(e) => write!(
                f,
                "provider account authority is not established, options-lifecycle gate cannot be \
                 evaluated: {e}"
            ),
            Self::Database(e) => write!(f, "options-lifecycle gate query failed: {e}"),
        }
    }
}

/// The shared per-symbol check used by every economic admission seam. A
/// deployment with no lifecycle fetcher has no Alpaca account and therefore no
/// lifecycle evidence: vacuously `Clear`.
pub async fn check_symbol_fence(
    pool: &PgPool,
    fetcher: Option<&Arc<dyn OptionLifecycleActivityFetcher>>,
    symbol: &str,
) -> Result<OptionLifecycleGateStatus, GateCheckError> {
    let Some(fetcher) = fetcher else {
        return Ok(OptionLifecycleGateStatus::Clear);
    };
    let authority = fetcher
        .broker_account_authority()
        .map_err(GateCheckError::AuthorityUnavailable)?;
    evaluate_option_lifecycle_pending_gate(pool, &authority.key(), symbol)
        .await
        .map_err(|e| GateCheckError::Database(e.to_string()))
}

/// Whole-account variant for operations that overwrite local truth for every
/// symbol at once (baseline adoption): fenced by ANY unreconciled event, so an
/// option that vanished from a broker snapshot can never bypass the gate.
pub async fn check_account_fence(
    pool: &PgPool,
    fetcher: Option<&Arc<dyn OptionLifecycleActivityFetcher>>,
) -> Result<OptionLifecycleGateStatus, GateCheckError> {
    let Some(fetcher) = fetcher else {
        return Ok(OptionLifecycleGateStatus::Clear);
    };
    let authority = fetcher
        .broker_account_authority()
        .map_err(GateCheckError::AuthorityUnavailable)?;
    let row = find_any_unreconciled_lifecycle_event(
        pool,
        &authority.key(),
        OPTION_LIFECYCLE_EXECUTION_DOMAIN.as_str(),
    )
    .await
    .map_err(|e| GateCheckError::Database(e.to_string()))?;
    Ok(OptionLifecycleGateStatus::from_row(row))
}
