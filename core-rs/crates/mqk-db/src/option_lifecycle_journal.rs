//! D2: the canonical options-lifecycle adjustment journal (migration 0090) and
//! its one atomic apply transaction.
//!
//! [`apply_lifecycle_adjustment_tx`] is the ONLY writer. In one transaction it
//! (1) locks the lifecycle event's state row, (2) refuses unless the event is
//! `READY_TO_APPLY` under exactly the account/domain/contract/correlation the
//! caller derived (a crash before commit leaves zero economic mutation), (3)
//! inserts the immutable signed journal row, and (4) moves the state to
//! `APPLIED_AWAITING_BROKER` carrying the deterministic `economic_apply_id`. A
//! retry after commit finds the same row and mutates nothing more.
//!
//! The journal is consumed by the accepted canonical ledger
//! (`mqk_portfolio::LedgerEntry::LifecycleAdjustment`) at run recovery and,
//! between ticks, by the running orchestrator -- see the migration header for
//! the authority relationship.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::broker_account_authority::BrokerAccountAuthority;
use crate::option_lifecycle_activity::OptionLifecycleActivityType;
use crate::option_lifecycle_event_state::{CorrelationBasis, LifecycleEventState};

/// Deterministic identity of one lifecycle event's economic application.
pub fn economic_apply_id(
    broker_account_id: &str,
    execution_domain: &str,
    lifecycle_activity_id: &str,
    lifecycle_activity_type: OptionLifecycleActivityType,
) -> String {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!(
            "mqk.option-lifecycle-adjustment.v1|{broker_account_id}|{execution_domain}|\
             {lifecycle_activity_id}|{}",
            lifecycle_activity_type.as_str()
        )
        .as_bytes(),
    )
    .to_string()
}

/// The underlying-delivery half of an exercise/assignment adjustment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnderlyingDelivery {
    pub qty_delta_micros: i64,
    pub strike_micros: i64,
    /// Provider-signed `net_amount` of the settlement trade.
    pub cash_delta_micros: i64,
    pub optrd_activity_id: String,
}

/// Everything the apply transaction needs, fully derived and verified by the
/// caller. `underlying` is `None` exactly for an expiration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLifecycleAdjustment {
    pub broker_account_id: String,
    pub execution_domain: String,
    pub lifecycle_activity_id: String,
    pub lifecycle_activity_type: OptionLifecycleActivityType,
    pub option_symbol: String,
    pub underlying_symbol: String,
    pub option_qty_delta_micros: i64,
    pub underlying: Option<UnderlyingDelivery>,
    pub correlation_basis: CorrelationBasis,
    pub applied_at_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleJournalEntry {
    pub journal_seq: i64,
    pub economic_apply_id: String,
    pub broker_account_id: String,
    pub execution_domain: String,
    pub lifecycle_activity_id: String,
    pub lifecycle_activity_type: OptionLifecycleActivityType,
    pub option_symbol: String,
    pub underlying_symbol: String,
    pub option_qty_delta_micros: i64,
    pub underlying_qty_delta_micros: Option<i64>,
    pub strike_micros: Option<i64>,
    pub cash_delta_micros: Option<i64>,
    pub optrd_activity_id: Option<String>,
    pub correlation_basis: CorrelationBasis,
    pub applied_at_utc: DateTime<Utc>,
    pub baseline_subsumed_at_utc: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyAdjustmentOutcome {
    /// This call durably applied the effect.
    Applied(LifecycleJournalEntry),
    /// The effect was already durably applied; zero additional mutation.
    AlreadyApplied(LifecycleJournalEntry),
}

#[derive(sqlx::FromRow)]
struct JournalRow {
    journal_seq: i64,
    economic_apply_id: String,
    broker_account_id: String,
    execution_domain: String,
    lifecycle_activity_id: String,
    lifecycle_activity_type: String,
    option_symbol: String,
    underlying_symbol: String,
    option_qty_delta_micros: i64,
    underlying_qty_delta_micros: Option<i64>,
    strike_micros: Option<i64>,
    cash_delta_micros: Option<i64>,
    optrd_activity_id: Option<String>,
    correlation_basis: String,
    applied_at_utc: DateTime<Utc>,
    baseline_subsumed_at_utc: Option<DateTime<Utc>>,
}

const JOURNAL_COLUMNS: &str = "journal_seq, economic_apply_id, broker_account_id, \
     execution_domain, lifecycle_activity_id, lifecycle_activity_type, option_symbol, \
     underlying_symbol, option_qty_delta_micros, underlying_qty_delta_micros, strike_micros, \
     cash_delta_micros, optrd_activity_id, correlation_basis, applied_at_utc, \
     baseline_subsumed_at_utc";

fn row_to_entry(r: JournalRow) -> Result<LifecycleJournalEntry> {
    Ok(LifecycleJournalEntry {
        journal_seq: r.journal_seq,
        economic_apply_id: r.economic_apply_id,
        broker_account_id: r.broker_account_id,
        execution_domain: r.execution_domain,
        lifecycle_activity_id: r.lifecycle_activity_id,
        lifecycle_activity_type: OptionLifecycleActivityType::parse_public(
            &r.lifecycle_activity_type,
        )?,
        option_symbol: r.option_symbol,
        underlying_symbol: r.underlying_symbol,
        option_qty_delta_micros: r.option_qty_delta_micros,
        underlying_qty_delta_micros: r.underlying_qty_delta_micros,
        strike_micros: r.strike_micros,
        cash_delta_micros: r.cash_delta_micros,
        optrd_activity_id: r.optrd_activity_id,
        correlation_basis: CorrelationBasis::parse(&r.correlation_basis)?,
        applied_at_utc: r.applied_at_utc,
        baseline_subsumed_at_utc: r.baseline_subsumed_at_utc,
    })
}

#[derive(sqlx::FromRow)]
struct LockedState {
    execution_domain: String,
    option_symbol: String,
    underlying_symbol: Option<String>,
    state: String,
    correlated_optrd_activity_id: Option<String>,
    correlation_basis: Option<String>,
}

/// The one atomic apply. See the module docs.
pub async fn apply_lifecycle_adjustment_tx(
    pool: &PgPool,
    adj: &NewLifecycleAdjustment,
) -> Result<ApplyAdjustmentOutcome> {
    // Shape is checked before any lock: an expiration carries no delivery and
    // an exercise/assignment always does.
    match (adj.lifecycle_activity_type, &adj.underlying) {
        (OptionLifecycleActivityType::Expiration, None) => {}
        (OptionLifecycleActivityType::Expiration, Some(_)) => {
            bail!("apply_lifecycle_adjustment_tx: an expiration must not carry an underlying delivery or cash")
        }
        (OptionLifecycleActivityType::PairedTrade, _) => {
            bail!("apply_lifecycle_adjustment_tx: OPTRD is not a lifecycle event")
        }
        (_, None) => {
            bail!("apply_lifecycle_adjustment_tx: an exercise/assignment requires its settlement evidence")
        }
        (_, Some(u)) => {
            if u.qty_delta_micros == 0 || u.strike_micros <= 0 {
                bail!("apply_lifecycle_adjustment_tx: settlement quantity/strike must be non-zero/positive");
            }
        }
    }
    if adj.option_qty_delta_micros == 0 {
        bail!("apply_lifecycle_adjustment_tx: option quantity delta must be non-zero");
    }

    let mut tx = pool
        .begin()
        .await
        .context("apply_lifecycle_adjustment_tx: begin failed")?;

    let locked: Option<LockedState> = sqlx::query_as(
        r#"
        select execution_domain, option_symbol, underlying_symbol, state,
               correlated_optrd_activity_id, correlation_basis
          from sys_option_lifecycle_event_state
         where broker_account_id = $1 and lifecycle_activity_id = $2
           and lifecycle_activity_type = $3
         for update
        "#,
    )
    .bind(&adj.broker_account_id)
    .bind(&adj.lifecycle_activity_id)
    .bind(adj.lifecycle_activity_type.as_str())
    .fetch_optional(&mut *tx)
    .await
    .context("apply_lifecycle_adjustment_tx: state lock failed")?;
    let Some(locked) = locked else {
        bail!(
            "apply_lifecycle_adjustment_tx: no lifecycle event state for account {:?} activity {:?}; \
             D1 ingestion must run first",
            adj.broker_account_id,
            adj.lifecycle_activity_id
        );
    };

    let apply_id = economic_apply_id(
        &adj.broker_account_id,
        &adj.execution_domain,
        &adj.lifecycle_activity_id,
        adj.lifecycle_activity_type,
    );

    if locked.execution_domain != adj.execution_domain {
        bail!(
            "apply_lifecycle_adjustment_tx: refused -- event domain {:?} does not match {:?}",
            locked.execution_domain,
            adj.execution_domain
        );
    }

    let state = LifecycleEventState::parse(&locked.state)?;
    match state {
        LifecycleEventState::AppliedAwaitingBroker | LifecycleEventState::Reconciled => {
            // Idempotent retry: the journal row is the one durable truth.
            let existing = fetch_entry_tx(&mut tx, &apply_id).await?.with_context(|| {
                format!("state {state:?} without a journal row for {apply_id}")
            })?;
            tx.rollback().await.ok();
            return Ok(ApplyAdjustmentOutcome::AlreadyApplied(existing));
        }
        LifecycleEventState::ReadyToApply => {}
        other => bail!(
            "apply_lifecycle_adjustment_tx: refused -- event is {} (only READY_TO_APPLY may be applied)",
            other.as_str()
        ),
    }

    // The caller's derivation must match the proven state exactly.
    if locked.option_symbol != adj.option_symbol
        || locked.underlying_symbol.as_deref() != Some(adj.underlying_symbol.as_str())
        || locked.correlation_basis.as_deref() != Some(adj.correlation_basis.as_str())
        || locked.correlated_optrd_activity_id.as_deref()
            != adj
                .underlying
                .as_ref()
                .map(|u| u.optrd_activity_id.as_str())
    {
        bail!(
            "apply_lifecycle_adjustment_tx: refused -- the derived adjustment does not match the \
             event's proven contract/correlation"
        );
    }

    let (u_qty, strike, cash, optrd) = match &adj.underlying {
        Some(u) => (
            Some(u.qty_delta_micros),
            Some(u.strike_micros),
            Some(u.cash_delta_micros),
            Some(u.optrd_activity_id.as_str()),
        ),
        None => (None, None, None, None),
    };

    sqlx::query(
        r#"
        insert into sys_option_lifecycle_adjustment_journal (
            economic_apply_id, broker_account_id, execution_domain, lifecycle_activity_id,
            lifecycle_activity_type, option_symbol, underlying_symbol, option_qty_delta_micros,
            underlying_qty_delta_micros, strike_micros, cash_delta_micros, optrd_activity_id,
            correlation_basis, applied_at_utc
        ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        "#,
    )
    .bind(&apply_id)
    .bind(&adj.broker_account_id)
    .bind(&adj.execution_domain)
    .bind(&adj.lifecycle_activity_id)
    .bind(adj.lifecycle_activity_type.as_str())
    .bind(&adj.option_symbol)
    .bind(&adj.underlying_symbol)
    .bind(adj.option_qty_delta_micros)
    .bind(u_qty)
    .bind(strike)
    .bind(cash)
    .bind(optrd)
    .bind(adj.correlation_basis.as_str())
    .bind(adj.applied_at_utc)
    .execute(&mut *tx)
    .await
    .context("apply_lifecycle_adjustment_tx: journal insert failed")?;

    sqlx::query(
        r#"
        update sys_option_lifecycle_event_state
           set state = 'APPLIED_AWAITING_BROKER',
               state_reason = 'applied_awaiting_broker_agreement',
               economic_apply_id = $4,
               updated_at_utc = $5
         where broker_account_id = $1 and lifecycle_activity_id = $2
           and lifecycle_activity_type = $3 and state = 'READY_TO_APPLY'
        "#,
    )
    .bind(&adj.broker_account_id)
    .bind(&adj.lifecycle_activity_id)
    .bind(adj.lifecycle_activity_type.as_str())
    .bind(&apply_id)
    .bind(adj.applied_at_utc)
    .execute(&mut *tx)
    .await
    .context("apply_lifecycle_adjustment_tx: state transition failed")
    .and_then(|r| {
        if r.rows_affected() == 1 {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "apply_lifecycle_adjustment_tx: state row was not READY_TO_APPLY at update time"
            ))
        }
    })?;

    let entry = fetch_entry_tx(&mut tx, &apply_id)
        .await?
        .context("journal row missing after insert")?;
    tx.commit()
        .await
        .context("apply_lifecycle_adjustment_tx: commit failed")?;
    Ok(ApplyAdjustmentOutcome::Applied(entry))
}

async fn fetch_entry_tx(
    conn: &mut sqlx::PgConnection,
    economic_apply_id: &str,
) -> Result<Option<LifecycleJournalEntry>> {
    let q = format!(
        "select {JOURNAL_COLUMNS} from sys_option_lifecycle_adjustment_journal \
         where economic_apply_id = $1"
    );
    let row: Option<JournalRow> = sqlx::query_as(&q)
        .bind(economic_apply_id)
        .fetch_optional(conn)
        .await
        .context("fetch_entry_tx failed")?;
    row.map(row_to_entry).transpose()
}

pub async fn fetch_lifecycle_journal_entry(
    pool: &PgPool,
    economic_apply_id: &str,
) -> Result<Option<LifecycleJournalEntry>> {
    let mut conn = pool.acquire().await.context("acquire failed")?;
    fetch_entry_tx(&mut conn, economic_apply_id).await
}

/// Journal entries a run recovery must replay on top of its baseline + fills:
/// every not-yet-baseline-subsumed entry of exactly `account`, in `journal_seq`
/// order. Another provider account under the same deployment mode is never
/// visible.
pub async fn list_unsubsumed_lifecycle_journal(
    pool: &PgPool,
    execution_domain: &str,
    account: &BrokerAccountAuthority,
) -> Result<Vec<LifecycleJournalEntry>> {
    let q = format!(
        "select {} from sys_option_lifecycle_adjustment_journal j \
         join sys_broker_account_authority a on a.authority_key = j.broker_account_id \
         where j.execution_domain = $1 and j.broker_account_id = $2 \
           and a.deployment_mode = $3 and j.baseline_subsumed_at_utc is null \
         order by j.journal_seq asc",
        JOURNAL_COLUMNS
            .split(", ")
            .map(|c| format!("j.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let rows: Vec<JournalRow> = sqlx::query_as(&q)
        .bind(execution_domain)
        .bind(account.key())
        .bind(account.deployment_mode())
        .fetch_all(pool)
        .await
        .context("list_unsubsumed_lifecycle_journal failed")?;
    rows.into_iter().map(row_to_entry).collect()
}

/// Number of not-yet-subsumed journal entries held by ANY account registered
/// under `deployment_mode`. Presence probe only, for callers that could not
/// establish their own account: a non-zero count means account-scoped
/// consumption is impossible and they must fail closed. Never a source of
/// entries to apply.
pub async fn count_unsubsumed_lifecycle_journal_for_deployment_mode(
    pool: &PgPool,
    execution_domain: &str,
    deployment_mode: &str,
) -> Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "select count(*) from sys_option_lifecycle_adjustment_journal j          join sys_broker_account_authority a on a.authority_key = j.broker_account_id          where j.execution_domain = $1 and a.deployment_mode = $2            and j.baseline_subsumed_at_utc is null",
    )
    .bind(execution_domain)
    .bind(deployment_mode)
    .fetch_one(pool)
    .await
    .context("count_unsubsumed_lifecycle_journal_for_deployment_mode failed")?;
    Ok(n)
}

/// `APPLIED_AWAITING_BROKER -> RECONCILED`, only after the caller proved broker
/// agreement. Compare-and-set; `Ok(false)` when the event is not awaiting the
/// broker (zero mutation).
pub async fn mark_lifecycle_event_reconciled(
    pool: &PgPool,
    broker_account_id: &str,
    lifecycle_activity_id: &str,
    lifecycle_activity_type: OptionLifecycleActivityType,
    now_utc: DateTime<Utc>,
) -> Result<bool> {
    let r = sqlx::query(
        r#"
        update sys_option_lifecycle_event_state
           set state = 'RECONCILED',
               state_reason = 'broker_agreement_proven',
               updated_at_utc = $4
         where broker_account_id = $1 and lifecycle_activity_id = $2
           and lifecycle_activity_type = $3 and state = 'APPLIED_AWAITING_BROKER'
        "#,
    )
    .bind(broker_account_id)
    .bind(lifecycle_activity_id)
    .bind(lifecycle_activity_type.as_str())
    .bind(now_utc)
    .execute(pool)
    .await
    .context("mark_lifecycle_event_reconciled failed")?;
    Ok(r.rows_affected() == 1)
}

/// After an operator baseline adoption absorbed broker truth: stop replaying
/// every RECONCILED journal entry of exactly `account`. Entries still awaiting
/// the broker are never subsumed (the adoption route refuses while any exist),
/// and another account's entries are never touched. Returns the number of
/// entries newly subsumed.
pub async fn subsume_reconciled_lifecycle_journal(
    pool: &PgPool,
    execution_domain: &str,
    account: &BrokerAccountAuthority,
    now_utc: DateTime<Utc>,
) -> Result<u64> {
    let r = sqlx::query(
        r#"
        update sys_option_lifecycle_adjustment_journal j
           set baseline_subsumed_at_utc = $4
          from sys_option_lifecycle_event_state s, sys_broker_account_authority a
         where s.broker_account_id = j.broker_account_id
           and s.lifecycle_activity_id = j.lifecycle_activity_id
           and s.lifecycle_activity_type = j.lifecycle_activity_type
           and s.state = 'RECONCILED'
           and a.authority_key = j.broker_account_id
           and j.broker_account_id = $2
           and a.deployment_mode = $3
           and j.execution_domain = $1
           and j.baseline_subsumed_at_utc is null
        "#,
    )
    .bind(execution_domain)
    .bind(account.key())
    .bind(account.deployment_mode())
    .bind(now_utc)
    .execute(pool)
    .await
    .context("subsume_reconciled_lifecycle_journal failed")?;
    Ok(r.rows_affected())
}
