//! D3 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION): the
//! fail-closed gate a future options execution/reconcile caller must
//! consult before treating broker truth as ground truth for an option
//! symbol whose lifecycle evidence is not yet complete.
//!
//! Invariant: a broker position change for an option/underlying observed
//! before its `OPEXC`/`OPASN`/`OPEXP` lifecycle evidence has been fully
//! applied (D2) must never be treated as a normal fill or used to
//! overwrite a local snapshot. The affected option becomes
//! lifecycle-pending; execution touching it fails closed. Unrelated
//! symbols/domains are unaffected -- this module never inspects or reports
//! on any symbol other than the one the caller asks about.
//!
//! Read-only by construction: [`evaluate_option_lifecycle_pending_gate`]
//! makes no write of any kind. Restart-safe by construction: the answer is
//! derived entirely from the two durable D1/D2 tables via
//! [`find_unresolved_option_lifecycle_activity`] -- there is no separate
//! mutable "pending" flag to lose across a restart; a fresh process reading
//! the same committed DB state gets the same answer. Complete evidence
//! (D2's `apply_option_lifecycle_activity` succeeding) is exactly what
//! clears it, since that is what removes the row this query looks for.
//!
//! No production caller wired in this patch -- Alpaca options capability
//! does not exist in this codebase yet, mirroring D1/D2's own scope.

use sqlx::PgPool;

use mqk_db::option_lifecycle_activity::{
    find_unresolved_option_lifecycle_activity, OptionLifecycleActivityType,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionLifecycleGateStatus {
    /// No unresolved `OPEXC`/`OPASN`/`OPEXP` evidence exists for this
    /// symbol -- broker truth may be trusted.
    Clear,
    /// At least one lifecycle activity for this symbol has not yet had its
    /// effect applied. The caller MUST fail closed: no synthetic fill, no
    /// snapshot overwrite, no execution that depends on this symbol's
    /// resolved state.
    Pending {
        blocking_activity_id: String,
        blocking_activity_type: OptionLifecycleActivityType,
    },
}

impl OptionLifecycleGateStatus {
    /// Fail-closed helper for a future execution-dispatch caller: true iff
    /// this symbol's lifecycle state is unresolved and execution touching
    /// it must therefore be refused.
    pub fn must_fail_closed(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }
}

/// Evaluate the D3 gate for one option symbol, scoped to
/// `broker_account_id` (D1 correction: a symbol's lifecycle-pending state
/// under one broker account must never be conflated with another
/// account's evidence). Read-only: makes no mutation of any kind, and
/// never inspects any symbol/account other than the ones given --
/// callers evaluating other symbols or accounts are structurally
/// unaffected by this symbol's state.
pub async fn evaluate_option_lifecycle_pending_gate(
    pool: &PgPool,
    broker_account_id: &str,
    option_symbol: &str,
) -> anyhow::Result<OptionLifecycleGateStatus> {
    match find_unresolved_option_lifecycle_activity(pool, broker_account_id, option_symbol).await? {
        Some((blocking_activity_id, blocking_activity_type)) => {
            Ok(OptionLifecycleGateStatus::Pending {
                blocking_activity_id,
                blocking_activity_type,
            })
        }
        None => Ok(OptionLifecycleGateStatus::Clear),
    }
}
