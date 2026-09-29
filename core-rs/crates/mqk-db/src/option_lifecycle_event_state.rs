//! Persisted options-lifecycle event state machine (migration 0089).
//!
//! One row per `OPEXC`/`OPASN`/`OPEXP` raw lifecycle row, created in the same
//! transaction as the raw insert:
//!
//! ```text
//! PENDING_EVIDENCE <-> PENDING_AMBIGUOUS <-> READY_TO_APPLY
//!                                                 |  (apply tx only)
//!                                        APPLIED_AWAITING_BROKER
//!                                                 |  (broker agreement only)
//!                                             RECONCILED   (terminal)
//! ```
//!
//! Only `RECONCILED` clears the pending gate. The database trigger from 0089
//! enforces the monotonic machine and identity immutability; the functions
//! here additionally compare-and-set so a re-evaluation can never move a row
//! that has already been applied.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};

use crate::option_lifecycle_activity::OptionLifecycleActivityType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEventState {
    PendingEvidence,
    PendingAmbiguous,
    ReadyToApply,
    AppliedAwaitingBroker,
    Reconciled,
}

impl LifecycleEventState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingEvidence => "PENDING_EVIDENCE",
            Self::PendingAmbiguous => "PENDING_AMBIGUOUS",
            Self::ReadyToApply => "READY_TO_APPLY",
            Self::AppliedAwaitingBroker => "APPLIED_AWAITING_BROKER",
            Self::Reconciled => "RECONCILED",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        Ok(match raw {
            "PENDING_EVIDENCE" => Self::PendingEvidence,
            "PENDING_AMBIGUOUS" => Self::PendingAmbiguous,
            "READY_TO_APPLY" => Self::ReadyToApply,
            "APPLIED_AWAITING_BROKER" => Self::AppliedAwaitingBroker,
            "RECONCILED" => Self::Reconciled,
            other => bail!("option_lifecycle_event_state: unknown state '{other}'"),
        })
    }

    /// The gate is closed for every state except `RECONCILED`.
    pub fn fences_symbols(self) -> bool {
        !matches!(self, Self::Reconciled)
    }

    /// States a (re-)evaluation may write; applied/reconciled are reachable
    /// only through their dedicated transitions.
    fn is_evaluable(self) -> bool {
        matches!(
            self,
            Self::PendingEvidence | Self::PendingAmbiguous | Self::ReadyToApply
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorrelationBasis {
    NotRequired,
    ExplicitGroupId,
    SameActivityId,
    UniqueEvidence,
}

impl CorrelationBasis {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::ExplicitGroupId => "explicit_group_id",
            Self::SameActivityId => "same_activity_id",
            Self::UniqueEvidence => "unique_evidence",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        Ok(match raw {
            "not_required" => Self::NotRequired,
            "explicit_group_id" => Self::ExplicitGroupId,
            "same_activity_id" => Self::SameActivityId,
            "unique_evidence" => Self::UniqueEvidence,
            other => bail!("option_lifecycle_event_state: unknown correlation basis '{other}'"),
        })
    }
}

/// What the raw-insert transaction needs to create the initial
/// `PENDING_EVIDENCE` row for a lifecycle raw row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleStateSeed {
    pub execution_domain: String,
    /// The underlying, when the caller proved it from the OCC contract.
    pub underlying_symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionLifecycleEventStateRow {
    pub broker_account_id: String,
    pub lifecycle_activity_id: String,
    pub lifecycle_activity_type: OptionLifecycleActivityType,
    pub execution_domain: String,
    pub option_symbol: String,
    pub underlying_symbol: Option<String>,
    pub state: LifecycleEventState,
    pub state_reason: String,
    pub correlated_optrd_activity_id: Option<String>,
    pub correlation_basis: Option<CorrelationBasis>,
    pub economic_apply_id: Option<String>,
    pub created_at_utc: DateTime<Utc>,
    pub updated_at_utc: DateTime<Utc>,
}

/// The outcome of one correlation evaluation. `state` must be one of the
/// three evaluable states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleEvaluation {
    pub state: LifecycleEventState,
    pub reason: String,
    pub underlying_symbol: Option<String>,
    pub correlated_optrd_activity_id: Option<String>,
    pub correlation_basis: Option<CorrelationBasis>,
}

pub(crate) async fn insert_pending_state_row_tx(
    conn: &mut PgConnection,
    broker_account_id: &str,
    lifecycle_activity_id: &str,
    lifecycle_activity_type: OptionLifecycleActivityType,
    option_symbol: &str,
    seed: &LifecycleStateSeed,
    now_utc: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into sys_option_lifecycle_event_state (
            broker_account_id, lifecycle_activity_id, lifecycle_activity_type,
            execution_domain, option_symbol, underlying_symbol, state, state_reason,
            created_at_utc, updated_at_utc
        ) values ($1, $2, $3, $4, $5, $6, 'PENDING_EVIDENCE', 'ingested_awaiting_correlation', $7, $7)
        on conflict (broker_account_id, lifecycle_activity_id, lifecycle_activity_type) do nothing
        "#,
    )
    .bind(broker_account_id)
    .bind(lifecycle_activity_id)
    .bind(lifecycle_activity_type.as_str())
    .bind(&seed.execution_domain)
    .bind(option_symbol)
    .bind(&seed.underlying_symbol)
    .bind(now_utc)
    .execute(conn)
    .await
    .context("insert_pending_state_row_tx failed")?;
    Ok(())
}

type StateRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    DateTime<Utc>,
    DateTime<Utc>,
);

const STATE_COLUMNS: &str = "broker_account_id, lifecycle_activity_id, lifecycle_activity_type, \
     execution_domain, option_symbol, underlying_symbol, state, state_reason, \
     correlated_optrd_activity_id, correlation_basis, economic_apply_id, created_at_utc, \
     updated_at_utc";

fn row_to_state(r: StateRow) -> Result<OptionLifecycleEventStateRow> {
    Ok(OptionLifecycleEventStateRow {
        broker_account_id: r.0,
        lifecycle_activity_id: r.1,
        lifecycle_activity_type: OptionLifecycleActivityType::parse_public(&r.2)?,
        execution_domain: r.3,
        option_symbol: r.4,
        underlying_symbol: r.5,
        state: LifecycleEventState::parse(&r.6)?,
        state_reason: r.7,
        correlated_optrd_activity_id: r.8,
        correlation_basis: r.9.as_deref().map(CorrelationBasis::parse).transpose()?,
        economic_apply_id: r.10,
        created_at_utc: r.11,
        updated_at_utc: r.12,
    })
}

pub async fn fetch_option_lifecycle_event_state(
    pool: &PgPool,
    broker_account_id: &str,
    lifecycle_activity_id: &str,
    lifecycle_activity_type: OptionLifecycleActivityType,
) -> Result<Option<OptionLifecycleEventStateRow>> {
    let q = format!(
        "select {STATE_COLUMNS} from sys_option_lifecycle_event_state \
         where broker_account_id = $1 and lifecycle_activity_id = $2 \
           and lifecycle_activity_type = $3"
    );
    let row: Option<StateRow> = sqlx::query_as(&q)
        .bind(broker_account_id)
        .bind(lifecycle_activity_id)
        .bind(lifecycle_activity_type.as_str())
        .fetch_optional(pool)
        .await
        .context("fetch_option_lifecycle_event_state failed")?;
    row.map(row_to_state).transpose()
}

/// Every lifecycle event of one account/domain, in a stable order
/// (`created_at_utc`, id, type).
pub async fn list_option_lifecycle_event_states(
    pool: &PgPool,
    broker_account_id: &str,
    execution_domain: &str,
) -> Result<Vec<OptionLifecycleEventStateRow>> {
    let q = format!(
        "select {STATE_COLUMNS} from sys_option_lifecycle_event_state \
         where broker_account_id = $1 and execution_domain = $2 \
         order by created_at_utc asc, lifecycle_activity_id asc, lifecycle_activity_type asc"
    );
    let rows: Vec<StateRow> = sqlx::query_as(&q)
        .bind(broker_account_id)
        .bind(execution_domain)
        .fetch_all(pool)
        .await
        .context("list_option_lifecycle_event_states failed")?;
    rows.into_iter().map(row_to_state).collect()
}

/// Persist one correlation evaluation. Compare-and-set: succeeds only while
/// the row is still in an evaluable state (`PENDING_*`/`READY_TO_APPLY`);
/// returns `Ok(false)` (zero mutation) for a row that has already been
/// applied or reconciled -- a re-evaluation can never regress it.
pub async fn record_lifecycle_evaluation(
    pool: &PgPool,
    broker_account_id: &str,
    lifecycle_activity_id: &str,
    lifecycle_activity_type: OptionLifecycleActivityType,
    evaluation: &LifecycleEvaluation,
    now_utc: DateTime<Utc>,
) -> Result<bool> {
    if !evaluation.state.is_evaluable() {
        bail!(
            "record_lifecycle_evaluation: {} is not an evaluable state",
            evaluation.state.as_str()
        );
    }
    let result = sqlx::query(
        r#"
        update sys_option_lifecycle_event_state
           set state = $4,
               state_reason = $5,
               underlying_symbol = $6,
               correlated_optrd_activity_id = $7,
               correlation_basis = $8,
               updated_at_utc = $9
         where broker_account_id = $1
           and lifecycle_activity_id = $2
           and lifecycle_activity_type = $3
           and state in ('PENDING_EVIDENCE', 'PENDING_AMBIGUOUS', 'READY_TO_APPLY')
        "#,
    )
    .bind(broker_account_id)
    .bind(lifecycle_activity_id)
    .bind(lifecycle_activity_type.as_str())
    .bind(evaluation.state.as_str())
    .bind(&evaluation.reason)
    .bind(&evaluation.underlying_symbol)
    .bind(&evaluation.correlated_optrd_activity_id)
    .bind(evaluation.correlation_basis.map(CorrelationBasis::as_str))
    .bind(now_utc)
    .execute(pool)
    .await
    .context("record_lifecycle_evaluation failed")?;
    Ok(result.rows_affected() == 1)
}
