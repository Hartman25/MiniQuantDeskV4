//! D2: derive and atomically apply options-lifecycle economics.
//!
//! The economic effect of an exercise/assignment/expiration is the
//! PROVIDER-SIGNED evidence, verified against the contract, never
//! reconstructed:
//!
//! - option position delta = the lifecycle row's signed `qty` (exercise
//!   removes long contracts = negative; assignment removes short = positive);
//! - underlying delta = the settlement trade's signed share `qty`, at the OCC
//!   strike as lot basis;
//! - cash delta = the settlement trade's signed `net_amount`, applied as
//!   reported (it is NOT recomputed from `qty x price`);
//! - an expiration removes the option position and nothing else -- no
//!   underlying delivery, no cash.
//!
//! [`derive_lifecycle_adjustment`] re-verifies every fact through
//! `option_lifecycle_correlation::verify_lifecycle_settlement` (the same
//! single source correlation uses) and every decimal conversion is exact and
//! checked: any malformed/inexact/overflowing value is a refusal, never a
//! truncated or defaulted number. [`apply_ready_lifecycle_events`] hands the
//! derived adjustment to `mqk_db::apply_lifecycle_adjustment_tx`, the one
//! atomic transaction (journal row + `READY_TO_APPLY ->
//! APPLIED_AWAITING_BROKER`), and demotes an event whose derivation fails
//! back to `PENDING_EVIDENCE`.
//!
//! Exercise, assignment and expiration are never fills: they enter the
//! canonical ledger only as `LedgerEntry::LifecycleAdjustment`.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use mqk_db::option_lifecycle_activity::{
    fetch_option_lifecycle_activity, NewOptionLifecycleActivity, OptionLifecycleActivityType,
};
use mqk_db::{
    apply_lifecycle_adjustment_tx, list_option_lifecycle_event_states, record_lifecycle_evaluation,
    ApplyAdjustmentOutcome, LifecycleEvaluation, LifecycleEventState, NewLifecycleAdjustment,
    OptionLifecycleEventStateRow, UnderlyingDelivery,
};

use super::option_lifecycle_correlation::verify_lifecycle_settlement;
use super::option_lifecycle_ingestion::OPTION_LIFECYCLE_EXECUTION_DOMAIN;

/// Why a READY event could not be turned into an adjustment. Every variant is
/// a fail-closed refusal; the event is demoted to `PENDING_EVIDENCE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeriveRefusal {
    /// The proven state row and the raw rows disagree (symbol/underlying/
    /// correlated trade).
    StateMismatch(String),
    /// A required settlement trade is missing or fails a fact.
    Evidence(String),
}

impl std::fmt::Display for DeriveRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StateMismatch(s) | Self::Evidence(s) => f.write_str(s),
        }
    }
}

/// Pure derivation of the adjustment for one `READY_TO_APPLY` event.
pub fn derive_lifecycle_adjustment(
    state: &OptionLifecycleEventStateRow,
    lifecycle: &NewOptionLifecycleActivity,
    optrd: Option<&NewOptionLifecycleActivity>,
    applied_at_utc: DateTime<Utc>,
) -> Result<NewLifecycleAdjustment, DeriveRefusal> {
    let verified =
        verify_lifecycle_settlement(lifecycle, optrd).map_err(DeriveRefusal::Evidence)?;

    if lifecycle.option_symbol.as_deref() != Some(state.option_symbol.as_str()) {
        return Err(DeriveRefusal::StateMismatch(
            "raw lifecycle option symbol differs from the state row".to_string(),
        ));
    }
    if state.underlying_symbol.as_deref() != Some(verified.contract.underlying()) {
        return Err(DeriveRefusal::StateMismatch(
            "state underlying differs from the OCC contract's underlying".to_string(),
        ));
    }
    let expected_optrd = optrd.map(|o| o.activity_id.as_str());
    if state.correlated_optrd_activity_id.as_deref() != expected_optrd {
        return Err(DeriveRefusal::StateMismatch(
            "state's correlated trade differs from the supplied settlement trade".to_string(),
        ));
    }
    let basis = state.correlation_basis.ok_or_else(|| {
        DeriveRefusal::StateMismatch("READY event has no correlation basis".to_string())
    })?;

    let underlying = match (&verified.settlement, optrd) {
        (Some(s), Some(o)) => Some(UnderlyingDelivery {
            qty_delta_micros: s.underlying_qty_delta_micros,
            strike_micros: verified.contract.strike_micros(),
            cash_delta_micros: s.cash_delta_micros,
            optrd_activity_id: o.activity_id.clone(),
        }),
        (None, None) => None,
        _ => {
            return Err(DeriveRefusal::Evidence(
                "settlement evidence does not match the lifecycle kind".to_string(),
            ))
        }
    };

    Ok(NewLifecycleAdjustment {
        broker_account_id: state.broker_account_id.clone(),
        execution_domain: state.execution_domain.clone(),
        lifecycle_activity_id: state.lifecycle_activity_id.clone(),
        lifecycle_activity_type: state.lifecycle_activity_type,
        option_symbol: state.option_symbol.clone(),
        underlying_symbol: verified.contract.underlying().to_string(),
        option_qty_delta_micros: verified.option_qty_delta_micros,
        underlying,
        correlation_basis: basis,
        applied_at_utc,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyCycleReport {
    pub applied: usize,
    pub already_applied: usize,
    pub demoted_to_pending: usize,
}

/// Apply every `READY_TO_APPLY` event of `account_key` (each in its own atomic
/// transaction). An event whose derivation refuses is demoted to
/// `PENDING_EVIDENCE` with the refusal reason; nothing is applied for it.
pub async fn apply_ready_lifecycle_events(
    pool: &PgPool,
    account_key: &str,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<ApplyCycleReport> {
    let domain = OPTION_LIFECYCLE_EXECUTION_DOMAIN.as_str();
    let mut report = ApplyCycleReport::default();
    for state in list_option_lifecycle_event_states(pool, account_key, domain).await? {
        if state.state != LifecycleEventState::ReadyToApply {
            continue;
        }
        let Some(lifecycle) = fetch_option_lifecycle_activity(
            pool,
            account_key,
            &state.lifecycle_activity_id,
            state.lifecycle_activity_type,
        )
        .await?
        else {
            anyhow::bail!(
                "apply_ready_lifecycle_events: no raw row for ready event {:?}",
                state.lifecycle_activity_id
            );
        };
        let optrd = match &state.correlated_optrd_activity_id {
            Some(id) => {
                fetch_option_lifecycle_activity(
                    pool,
                    account_key,
                    id,
                    OptionLifecycleActivityType::PairedTrade,
                )
                .await?
            }
            None => None,
        };

        match derive_lifecycle_adjustment(&state, &lifecycle, optrd.as_ref(), now_utc) {
            Ok(adjustment) => match apply_lifecycle_adjustment_tx(pool, &adjustment).await? {
                ApplyAdjustmentOutcome::Applied(_) => report.applied += 1,
                ApplyAdjustmentOutcome::AlreadyApplied(_) => report.already_applied += 1,
            },
            Err(refusal) => {
                record_lifecycle_evaluation(
                    pool,
                    account_key,
                    &state.lifecycle_activity_id,
                    state.lifecycle_activity_type,
                    &LifecycleEvaluation {
                        state: LifecycleEventState::PendingEvidence,
                        reason: format!("apply_refused:{refusal}"),
                        underlying_symbol: state.underlying_symbol.clone(),
                        correlated_optrd_activity_id: None,
                        correlation_basis: None,
                    },
                    now_utc,
                )
                .await?;
                report.demoted_to_pending += 1;
            }
        }
    }
    Ok(report)
}
