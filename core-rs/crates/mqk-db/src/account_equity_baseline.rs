//! Account equity baseline — durable, provenance-tagged equity snapshot per
//! trading day (PAPER-DAILY-PNL-BASELINE-01-COMBINED).
//!
//! Used by `/api/v1/portfolio/summary.daily_pnl` to compare today's account
//! equity against the previous trading day's captured close, once a real
//! capture has produced a row for the required date. This module only
//! provides upsert/fetch helpers — no capture mechanism (timing, trigger,
//! CLI, or route) is implemented here; that is deferred to a future patch.
//!
//! # Idempotency
//!
//! `trading_date` is the primary key. Upserting the same `trading_date`
//! again replaces that row's fields (last-write-wins for repeated captures
//! on the same day) rather than creating a duplicate — this is a deliberate
//! design choice distinct from `broker_baseline.rs`'s single-sentinel-row
//! upsert: here there is one row *per trading day*, each independently
//! idempotent by its own date key.

use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Row returned from `fetch_account_equity_baseline_for_date` /
/// `upsert_account_equity_baseline`.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountEquityBaselineRecord {
    pub trading_date: NaiveDate,
    pub equity_micros: i64,
    pub cash_micros: i64,
    pub currency: String,
    pub captured_at_utc: DateTime<Utc>,
    pub captured_by: String,
    pub broker_snapshot_source: String,
    pub audit_event_id: Uuid,
}

/// Arguments for `upsert_account_equity_baseline`. Caller supplies every
/// timestamp and the audit event ID — no `DEFAULT now()` / `DEFAULT
/// gen_random_uuid()` in the schema (DB rules).
#[derive(Debug, Clone)]
pub struct UpsertAccountEquityBaselineArgs {
    pub trading_date: NaiveDate,
    pub equity_micros: i64,
    pub cash_micros: i64,
    pub currency: String,
    pub captured_at_utc: DateTime<Utc>,
    pub captured_by: String,
    pub broker_snapshot_source: String,
    pub audit_event_id: Uuid,
}

/// Upsert an account equity baseline row (idempotent by `trading_date` —
/// a repeated capture for the same date replaces the row's fields rather
/// than duplicating it).
pub async fn upsert_account_equity_baseline(
    pool: &PgPool,
    args: UpsertAccountEquityBaselineArgs,
) -> Result<AccountEquityBaselineRecord> {
    sqlx::query(
        r#"
        insert into sys_account_equity_baseline
            (trading_date, equity_micros, cash_micros, currency,
             captured_at_utc, captured_by, broker_snapshot_source, audit_event_id)
        values ($1, $2, $3, $4, $5, $6, $7, $8)
        on conflict (trading_date) do update
            set equity_micros         = excluded.equity_micros,
                cash_micros           = excluded.cash_micros,
                currency              = excluded.currency,
                captured_at_utc       = excluded.captured_at_utc,
                captured_by           = excluded.captured_by,
                broker_snapshot_source = excluded.broker_snapshot_source,
                audit_event_id        = excluded.audit_event_id
        "#,
    )
    .bind(args.trading_date)
    .bind(args.equity_micros)
    .bind(args.cash_micros)
    .bind(&args.currency)
    .bind(args.captured_at_utc)
    .bind(&args.captured_by)
    .bind(&args.broker_snapshot_source)
    .bind(args.audit_event_id)
    .execute(pool)
    .await
    .map_err(|e| anyhow::anyhow!("upsert_account_equity_baseline failed: {e}"))?;

    Ok(AccountEquityBaselineRecord {
        trading_date: args.trading_date,
        equity_micros: args.equity_micros,
        cash_micros: args.cash_micros,
        currency: args.currency,
        captured_at_utc: args.captured_at_utc,
        captured_by: args.captured_by,
        broker_snapshot_source: args.broker_snapshot_source,
        audit_event_id: args.audit_event_id,
    })
}

/// Raw row shape for `fetch_account_equity_baseline_for_date`, matching
/// `sys_account_equity_baseline`'s column order.
type AccountEquityBaselineRow = (
    NaiveDate,
    i64,
    i64,
    String,
    DateTime<Utc>,
    String,
    String,
    Uuid,
);

/// Fetch the account equity baseline for a specific trading date. Returns
/// `None` when absent — never fabricates or substitutes a nearby date.
pub async fn fetch_account_equity_baseline_for_date(
    pool: &PgPool,
    trading_date: NaiveDate,
) -> Result<Option<AccountEquityBaselineRecord>> {
    let row: Option<AccountEquityBaselineRow> = sqlx::query_as(
        r#"
            select trading_date, equity_micros, cash_micros, currency,
                   captured_at_utc, captured_by, broker_snapshot_source, audit_event_id
            from sys_account_equity_baseline
            where trading_date = $1
            "#,
    )
    .bind(trading_date)
    .fetch_optional(pool)
    .await
    .map_err(|e| anyhow::anyhow!("fetch_account_equity_baseline_for_date failed: {e}"))?;

    Ok(row.map(
        |(
            trading_date,
            equity_micros,
            cash_micros,
            currency,
            captured_at_utc,
            captured_by,
            broker_snapshot_source,
            audit_event_id,
        )| AccountEquityBaselineRecord {
            trading_date,
            equity_micros,
            cash_micros,
            currency,
            captured_at_utc,
            captured_by,
            broker_snapshot_source,
            audit_event_id,
        },
    ))
}
