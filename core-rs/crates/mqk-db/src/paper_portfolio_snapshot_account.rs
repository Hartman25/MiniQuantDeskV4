//! 0092: durable provider-account binding for an accepted Paper portfolio
//! snapshot. See `migrations/0092_paper_portfolio_snapshot_account_binding.sql`.
//!
//! A snapshot is bound at most once, to a registered
//! [`BrokerAccountAuthority`]; a second bind to a different account is a
//! conflict, never an overwrite. A snapshot without a row is account-unverified.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::broker_account_authority::BrokerAccountAuthority;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindSnapshotAccountOutcome {
    Bound,
    AlreadyBound,
    /// The snapshot is already bound to a different account authority key.
    Conflict {
        existing_authority_key: String,
    },
}

/// Bind `snapshot_id` to `authority`. The authority must already be registered
/// (`verify_or_register_broker_account_authority`) and the snapshot must exist;
/// either missing is an error (foreign keys), never a silent no-op.
pub async fn bind_paper_portfolio_snapshot_account(
    pool: &PgPool,
    snapshot_id: Uuid,
    authority: &BrokerAccountAuthority,
    bound_at_utc: DateTime<Utc>,
) -> Result<BindSnapshotAccountOutcome> {
    let key = authority.key();
    let inserted = sqlx::query(
        r#"
        insert into sys_paper_portfolio_snapshot_account
            (snapshot_id, broker_account_id, bound_at_utc)
        values ($1, $2, $3)
        on conflict (snapshot_id) do nothing
        "#,
    )
    .bind(snapshot_id)
    .bind(&key)
    .bind(bound_at_utc)
    .execute(pool)
    .await
    .context("bind_paper_portfolio_snapshot_account: insert failed")?
    .rows_affected();

    let existing = fetch_paper_portfolio_snapshot_account(pool, snapshot_id)
        .await?
        .context("bind_paper_portfolio_snapshot_account: row vanished after insert")?;
    Ok(if existing != key {
        BindSnapshotAccountOutcome::Conflict {
            existing_authority_key: existing,
        }
    } else if inserted == 1 {
        BindSnapshotAccountOutcome::Bound
    } else {
        BindSnapshotAccountOutcome::AlreadyBound
    })
}

/// The authority key a snapshot is bound to; `None` = account-unverified.
pub async fn fetch_paper_portfolio_snapshot_account(
    pool: &PgPool,
    snapshot_id: Uuid,
) -> Result<Option<String>> {
    sqlx::query_scalar(
        r#"
        select broker_account_id
          from sys_paper_portfolio_snapshot_account
         where snapshot_id = $1
        "#,
    )
    .bind(snapshot_id)
    .fetch_optional(pool)
    .await
    .context("fetch_paper_portfolio_snapshot_account failed")
}
