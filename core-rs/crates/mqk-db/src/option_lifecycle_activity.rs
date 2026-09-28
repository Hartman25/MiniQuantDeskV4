//! D1 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION): durable,
//! restart-safe, deduplicated options lifecycle activity ingestion
//! (migration 0084).
//!
//! Mirrors `crypto_fee_activity.rs` (migration 0083, B6) exactly in shape
//! and idempotency contract. Per Alpaca's official "Non-Trade Activities
//! for Option Events" docs (verified 2026-09-27): exercise/assignment/
//! expiration are three distinct non-trade activities (`OPEXC`/`OPASN`/
//! `OPEXP`), each carrying no cash movement of its own; the underlying-
//! share delivery and strike cash consideration for `OPEXC`/`OPASN` is
//! reported separately via a same-day paired `OPTRD` trade activity. This
//! module stores each raw activity type as its own evidence row -- pairing
//! an `OPEXC`/`OPASN` row with its `OPTRD` row and applying the resulting
//! effect is D2's separate, idempotent accounting step
//! ([`insert_applied_option_lifecycle_effect_if_new`]).
//!
//! `activity_id` (Alpaca's own id) is the natural idempotency key -- an
//! activity must never be applied to the ledger twice.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// The four raw activity type this table can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionLifecycleActivityType {
    Exercise,
    Assignment,
    Expiration,
    /// The paired trade activity carrying the strike price and underlying
    /// share quantity for `Exercise`/`Assignment`.
    PairedTrade,
}

impl OptionLifecycleActivityType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exercise => "OPEXC",
            Self::Assignment => "OPASN",
            Self::Expiration => "OPEXP",
            Self::PairedTrade => "OPTRD",
        }
    }

    fn parse(raw: &str) -> Result<Self> {
        match raw {
            "OPEXC" => Ok(Self::Exercise),
            "OPASN" => Ok(Self::Assignment),
            "OPEXP" => Ok(Self::Expiration),
            "OPTRD" => Ok(Self::PairedTrade),
            other => Err(anyhow::anyhow!(
                "option_lifecycle_activity: unknown activity_type '{other}'"
            )),
        }
    }
}

/// One durable row to insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOptionLifecycleActivity {
    pub activity_id: String,
    pub engine_id: String,
    pub mode: String,
    pub activity_type: OptionLifecycleActivityType,
    pub option_symbol: String,
    /// Alpaca's own reported activity date, as an opaque string (e.g.
    /// `"2026-06-19"`) -- used only for `OPEXC`/`OPASN` <-> `OPTRD`
    /// correlation, never parsed into a chronology-bearing type here.
    pub activity_date: String,
    pub qty_raw: String,
    /// Populated only for `PairedTrade`.
    pub price_raw: Option<String>,
    pub ingested_at_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOptionLifecycleActivityOutcome {
    Inserted,
    AlreadyExists,
}

/// Insert one activity row, deduplicated at the DB level on `activity_id`.
pub async fn insert_option_lifecycle_activity_if_new(
    pool: &PgPool,
    activity: &NewOptionLifecycleActivity,
) -> Result<InsertOptionLifecycleActivityOutcome> {
    let result = sqlx::query(
        r#"
        insert into sys_option_lifecycle_activity_ledger (
            activity_id, engine_id, mode, activity_type, option_symbol,
            activity_date, qty_raw, price_raw, ingested_at_utc
        ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        on conflict (activity_id) do nothing
        "#,
    )
    .bind(&activity.activity_id)
    .bind(&activity.engine_id)
    .bind(&activity.mode)
    .bind(activity.activity_type.as_str())
    .bind(&activity.option_symbol)
    .bind(&activity.activity_date)
    .bind(&activity.qty_raw)
    .bind(&activity.price_raw)
    .bind(activity.ingested_at_utc)
    .execute(pool)
    .await
    .context("insert_option_lifecycle_activity_if_new failed")?;

    Ok(if result.rows_affected() == 1 {
        InsertOptionLifecycleActivityOutcome::Inserted
    } else {
        InsertOptionLifecycleActivityOutcome::AlreadyExists
    })
}

pub async fn fetch_option_lifecycle_ingestion_cursor(
    pool: &PgPool,
    engine_id: &str,
    mode: &str,
    activity_type: OptionLifecycleActivityType,
) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        r#"
        select last_activity_id
          from sys_option_lifecycle_ingestion_cursor
         where engine_id = $1 and mode = $2 and activity_type = $3
        "#,
    )
    .bind(engine_id)
    .bind(mode)
    .bind(activity_type.as_str())
    .fetch_optional(pool)
    .await
    .context("fetch_option_lifecycle_ingestion_cursor failed")?;

    Ok(row.map(|(id,)| id))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionLifecycleIngestionBatchOutcome {
    pub newly_inserted: usize,
    pub already_existed: usize,
}

/// Ingest one batch of activities (all the same `activity_type`) and
/// advance that type's cursor, atomically -- restart-safety mirrors
/// `ingest_crypto_fee_activity_batch` exactly.
pub async fn ingest_option_lifecycle_activity_batch(
    pool: &PgPool,
    engine_id: &str,
    mode: &str,
    activity_type: OptionLifecycleActivityType,
    batch: &[NewOptionLifecycleActivity],
    new_cursor_activity_id: &str,
    updated_at_utc: DateTime<Utc>,
) -> Result<OptionLifecycleIngestionBatchOutcome> {
    if batch.is_empty() {
        return Ok(OptionLifecycleIngestionBatchOutcome {
            newly_inserted: 0,
            already_existed: 0,
        });
    }

    let mut tx = pool
        .begin()
        .await
        .context("ingest_option_lifecycle_activity_batch: begin tx failed")?;

    let mut newly_inserted = 0usize;
    let mut already_existed = 0usize;

    for activity in batch {
        let result = sqlx::query(
            r#"
            insert into sys_option_lifecycle_activity_ledger (
                activity_id, engine_id, mode, activity_type, option_symbol,
                activity_date, qty_raw, price_raw, ingested_at_utc
            ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            on conflict (activity_id) do nothing
            "#,
        )
        .bind(&activity.activity_id)
        .bind(&activity.engine_id)
        .bind(&activity.mode)
        .bind(activity.activity_type.as_str())
        .bind(&activity.option_symbol)
        .bind(&activity.activity_date)
        .bind(&activity.qty_raw)
        .bind(&activity.price_raw)
        .bind(activity.ingested_at_utc)
        .execute(&mut *tx)
        .await
        .context("ingest_option_lifecycle_activity_batch: activity insert failed")?;

        if result.rows_affected() == 1 {
            newly_inserted += 1;
        } else {
            already_existed += 1;
        }
    }

    sqlx::query(
        r#"
        insert into sys_option_lifecycle_ingestion_cursor (
            engine_id, mode, activity_type, last_activity_id, updated_at_utc
        ) values ($1, $2, $3, $4, $5)
        on conflict (engine_id, mode, activity_type)
        do update set last_activity_id = excluded.last_activity_id,
                      updated_at_utc = excluded.updated_at_utc
        "#,
    )
    .bind(engine_id)
    .bind(mode)
    .bind(activity_type.as_str())
    .bind(new_cursor_activity_id)
    .bind(updated_at_utc)
    .execute(&mut *tx)
    .await
    .context("ingest_option_lifecycle_activity_batch: cursor advance failed")?;

    tx.commit()
        .await
        .context("ingest_option_lifecycle_activity_batch: commit failed")?;

    Ok(OptionLifecycleIngestionBatchOutcome {
        newly_inserted,
        already_existed,
    })
}

type OptionLifecycleActivityRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    DateTime<Utc>,
);

fn row_to_activity(row: OptionLifecycleActivityRow) -> Result<NewOptionLifecycleActivity> {
    let (
        activity_id,
        engine_id,
        mode,
        activity_type,
        option_symbol,
        activity_date,
        qty_raw,
        price_raw,
        ingested_at_utc,
    ) = row;
    Ok(NewOptionLifecycleActivity {
        activity_id,
        engine_id,
        mode,
        activity_type: OptionLifecycleActivityType::parse(&activity_type)?,
        option_symbol,
        activity_date,
        qty_raw,
        price_raw,
        ingested_at_utc,
    })
}

/// Read back one durably-ingested activity row, for test/audit
/// verification and for D2's pairing lookup.
pub async fn fetch_option_lifecycle_activity(
    pool: &PgPool,
    activity_id: &str,
) -> Result<Option<NewOptionLifecycleActivity>> {
    let row: Option<OptionLifecycleActivityRow> = sqlx::query_as(
        r#"
        select activity_id, engine_id, mode, activity_type, option_symbol,
               activity_date, qty_raw, price_raw, ingested_at_utc
          from sys_option_lifecycle_activity_ledger
         where activity_id = $1
        "#,
    )
    .bind(activity_id)
    .fetch_optional(pool)
    .await
    .context("fetch_option_lifecycle_activity failed")?;

    row.map(row_to_activity).transpose()
}

/// D2's pairing lookup: find the `OPTRD` row (if any) sharing the same
/// `(option_symbol, activity_date)` as a given `OPEXC`/`OPASN` row. Returns
/// `Ok(None)` when no paired trade has been ingested yet -- the caller's
/// resolution must remain `Pending`, never assume one will arrive or
/// fabricate a value.
pub async fn find_paired_trade_activity(
    pool: &PgPool,
    option_symbol: &str,
    activity_date: &str,
) -> Result<Option<NewOptionLifecycleActivity>> {
    let row: Option<OptionLifecycleActivityRow> = sqlx::query_as(
        r#"
        select activity_id, engine_id, mode, activity_type, option_symbol,
               activity_date, qty_raw, price_raw, ingested_at_utc
          from sys_option_lifecycle_activity_ledger
         where option_symbol = $1 and activity_date = $2 and activity_type = 'OPTRD'
         limit 1
        "#,
    )
    .bind(option_symbol)
    .bind(activity_date)
    .fetch_optional(pool)
    .await
    .context("find_paired_trade_activity failed")?;

    row.map(row_to_activity).transpose()
}

// ---------------------------------------------------------------------------
// D2: idempotent-apply marker
// ---------------------------------------------------------------------------

/// One durably-applied `PairedLifecycleEffect` (mirrors
/// `mqk_portfolio::option_lifecycle::PairedLifecycleEffect`'s shape without
/// this crate depending on `mqk-portfolio` -- the caller translates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedOptionLifecycleEffect {
    pub lifecycle_activity_id: String,
    pub engine_id: String,
    pub mode: String,
    pub option_symbol: String,
    pub underlying_symbol: Option<String>,
    pub option_contracts_removed_raw: String,
    pub underlying_shares_delivered_raw: Option<String>,
    pub cash_effect_micros: Option<i64>,
    pub applied_at_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertAppliedOptionLifecycleEffectOutcome {
    /// Genuinely new -- this lifecycle_activity_id had never been applied.
    Applied,
    /// Already applied; zero mutation. The caller must not apply this
    /// effect's economic consequence a second time.
    AlreadyApplied,
}

/// Durably record that `effect` has been applied, deduplicated on
/// `lifecycle_activity_id`. Safe to call with the same id any number of
/// times -- only the first call ever mutates the table.
pub async fn insert_applied_option_lifecycle_effect_if_new(
    pool: &PgPool,
    effect: &AppliedOptionLifecycleEffect,
) -> Result<InsertAppliedOptionLifecycleEffectOutcome> {
    let result = sqlx::query(
        r#"
        insert into sys_option_lifecycle_applied (
            lifecycle_activity_id, engine_id, mode, option_symbol,
            underlying_symbol, option_contracts_removed_raw,
            underlying_shares_delivered_raw, cash_effect_micros, applied_at_utc
        ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        on conflict (lifecycle_activity_id) do nothing
        "#,
    )
    .bind(&effect.lifecycle_activity_id)
    .bind(&effect.engine_id)
    .bind(&effect.mode)
    .bind(&effect.option_symbol)
    .bind(&effect.underlying_symbol)
    .bind(&effect.option_contracts_removed_raw)
    .bind(&effect.underlying_shares_delivered_raw)
    .bind(effect.cash_effect_micros)
    .bind(effect.applied_at_utc)
    .execute(pool)
    .await
    .context("insert_applied_option_lifecycle_effect_if_new failed")?;

    Ok(if result.rows_affected() == 1 {
        InsertAppliedOptionLifecycleEffectOutcome::Applied
    } else {
        InsertAppliedOptionLifecycleEffectOutcome::AlreadyApplied
    })
}

type AppliedOptionLifecycleEffectRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<i64>,
    DateTime<Utc>,
);

/// Read back whether `lifecycle_activity_id` has already been applied, for
/// test/audit verification and for D3's pending-lifecycle gate.
pub async fn fetch_applied_option_lifecycle_effect(
    pool: &PgPool,
    lifecycle_activity_id: &str,
) -> Result<Option<AppliedOptionLifecycleEffect>> {
    let row: Option<AppliedOptionLifecycleEffectRow> = sqlx::query_as(
        r#"
        select lifecycle_activity_id, engine_id, mode, option_symbol,
               underlying_symbol, option_contracts_removed_raw,
               underlying_shares_delivered_raw, cash_effect_micros, applied_at_utc
          from sys_option_lifecycle_applied
         where lifecycle_activity_id = $1
        "#,
    )
    .bind(lifecycle_activity_id)
    .fetch_optional(pool)
    .await
    .context("fetch_applied_option_lifecycle_effect failed")?;

    Ok(row.map(
        |(
            lifecycle_activity_id,
            engine_id,
            mode,
            option_symbol,
            underlying_symbol,
            option_contracts_removed_raw,
            underlying_shares_delivered_raw,
            cash_effect_micros,
            applied_at_utc,
        )| AppliedOptionLifecycleEffect {
            lifecycle_activity_id,
            engine_id,
            mode,
            option_symbol,
            underlying_symbol,
            option_contracts_removed_raw,
            underlying_shares_delivered_raw,
            cash_effect_micros,
            applied_at_utc,
        },
    ))
}

// ---------------------------------------------------------------------------
// D3: pending-lifecycle gate query
// ---------------------------------------------------------------------------

/// D3's pending-lifecycle gate: does `option_symbol` have any `OPEXC`/
/// `OPASN`/`OPEXP` activity durably ingested by D1 that has not yet had a
/// matching `PairedLifecycleEffect` applied by D2? Purely derived from the
/// two existing D1/D2 tables via a LEFT JOIN -- no separate mutable state,
/// so the answer is restart-safe by construction: it reflects only what is
/// already committed.
///
/// Returns the oldest such unresolved activity, if any. A caller with a
/// non-`None` result must treat `option_symbol` (and its underlying) as
/// lifecycle-pending: never synthesize a fill from a broker position change
/// for it, never overwrite its local snapshot from broker truth, and refuse
/// (fail closed) any execution that depends on its resolved state -- until
/// this returns `None` again, i.e. until D2 has genuinely applied the
/// matching effect.
pub async fn find_unresolved_option_lifecycle_activity(
    pool: &PgPool,
    option_symbol: &str,
) -> Result<Option<(String, OptionLifecycleActivityType)>> {
    let row: Option<(String, String)> = sqlx::query_as(
        r#"
        select l.activity_id, l.activity_type
          from sys_option_lifecycle_activity_ledger l
          left join sys_option_lifecycle_applied a
            on a.lifecycle_activity_id = l.activity_id
         where l.option_symbol = $1
           and l.activity_type in ('OPEXC', 'OPASN', 'OPEXP')
           and a.lifecycle_activity_id is null
         order by l.ingested_at_utc asc
         limit 1
        "#,
    )
    .bind(option_symbol)
    .fetch_optional(pool)
    .await
    .context("find_unresolved_option_lifecycle_activity failed")?;

    row.map(|(activity_id, activity_type_raw)| {
        OptionLifecycleActivityType::parse(&activity_type_raw).map(|t| (activity_id, t))
    })
    .transpose()
}
