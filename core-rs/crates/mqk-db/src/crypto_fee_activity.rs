//! B6 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION): durable,
//! restart-safe, deduplicated Crypto fee-activity ingestion (migration 0083).
//!
//! Pairs with `mqk_broker_alpaca::fee_attribution::normalize_fee_activity`
//! (asset_id -> [`FeeAttributionStatus`]-shaped record) and
//! `AlpacaBrokerAdapter::fetch_fee_activities_since` (the REST fetch), which
//! both existed with zero durable evidence or production caller before this
//! module. This module owns exactly the durable half: dedup, restart-safe
//! cursor, and the one atomic batch-ingest transaction a real caller would
//! drive. It does not fetch from Alpaca, does not normalize a raw payload,
//! and does not apply a `cash_fee` row's `fee_micros` to any live portfolio
//! balance -- that live-accounting wiring is explicit future work for
//! whichever caller eventually enables Crypto capability (still default off,
//! D2/B4).
//!
//! `activity_id` (Alpaca's own id) is the natural idempotency key
//! (`fee_attribution.rs`'s own doc: "an activity must never be applied to
//! the ledger twice") and is this table's PRIMARY KEY -- a duplicate insert
//! is a DB-level no-op, not merely caller discipline.
//!
//! # B6 correction: broker/account provenance (migration 0085)
//!
//! `activity_id` alone is unique only *within one Alpaca account*. Migration
//! 0085 adds `broker_account_id` (the authenticated account's own
//! `APCA-API-KEY-ID`, never the secret key) and widens the PRIMARY KEY on
//! both the ledger and the cursor to `(broker_account_id, ...)`, so two
//! distinct broker accounts can never collide on activity id or share a
//! cursor watermark, even if they happen to share an `engine_id`/`mode`
//! label. [`ingest_crypto_fee_activity_batch`] validates every row in a
//! batch against the requested `(broker_account_id, engine_id, mode,
//! activity_type)` scope *before* any insert or cursor advance -- a mixed
//! or mismatched batch refuses atomically, mutating nothing.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// The three [`FeeAttributionRecord`]-equivalent shapes this table can hold
/// (mirrors `mqk_broker_alpaca::fee_attribution::FeeAttributionRecord`
/// exactly; this crate has no dependency on that crate, so the shape is
/// duplicated as plain fields rather than imported).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoFeeAttributionStatus {
    CashFee,
    AssetDenominatedFeeUnsupported,
    ConfirmedZeroFee,
}

impl CryptoFeeAttributionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CashFee => "cash_fee",
            Self::AssetDenominatedFeeUnsupported => "asset_denominated_fee_unsupported",
            Self::ConfirmedZeroFee => "confirmed_zero_fee",
        }
    }

    fn parse(raw: &str) -> Result<Self> {
        match raw {
            "cash_fee" => Ok(Self::CashFee),
            "asset_denominated_fee_unsupported" => Ok(Self::AssetDenominatedFeeUnsupported),
            "confirmed_zero_fee" => Ok(Self::ConfirmedZeroFee),
            other => Err(anyhow::anyhow!(
                "crypto_fee_activity: unknown attribution_status '{other}'"
            )),
        }
    }
}

/// One durable row to insert -- the caller's job (normalize the raw Alpaca
/// payload via `fee_attribution::normalize_fee_activity`, then translate the
/// resulting enum into this shape) is deliberately kept outside this crate.
#[derive(Debug, Clone)]
pub struct NewCryptoFeeActivity {
    pub activity_id: String,
    /// B6 correction: the authenticated Alpaca account's own
    /// `APCA-API-KEY-ID` (never the secret key) -- durable broker/account
    /// provenance, not merely `engine_id`/`mode`. Part of both tables'
    /// PRIMARY KEY as of migration 0085.
    pub broker_account_id: String,
    pub engine_id: String,
    pub mode: String,
    pub activity_type: String,
    pub symbol: Option<String>,
    pub attribution_status: CryptoFeeAttributionStatus,
    /// Populated only for `CashFee`.
    pub fee_micros: Option<i64>,
    /// Populated only for `AssetDenominatedFeeUnsupported`.
    pub qty_raw: Option<String>,
    pub ingested_at_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertCryptoFeeActivityOutcome {
    /// A genuinely new row -- this activity had never been ingested before.
    Inserted,
    /// `activity_id` already existed; zero mutation (`ON CONFLICT DO
    /// NOTHING`). The caller must not apply this activity's economic effect
    /// a second time.
    AlreadyExists,
}

/// Insert one fee-activity row, deduplicated at the DB level on
/// `activity_id`. Safe to call with the exact same `activity_id` any number
/// of times -- only the first call ever mutates the table.
pub async fn insert_crypto_fee_activity_if_new(
    pool: &PgPool,
    activity: &NewCryptoFeeActivity,
) -> Result<InsertCryptoFeeActivityOutcome> {
    let result = sqlx::query(
        r#"
        insert into sys_crypto_fee_activity_ledger (
            activity_id, broker_account_id, engine_id, mode, activity_type, symbol,
            attribution_status, fee_micros, qty_raw, ingested_at_utc
        ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        on conflict (broker_account_id, activity_id) do nothing
        "#,
    )
    .bind(&activity.activity_id)
    .bind(&activity.broker_account_id)
    .bind(&activity.engine_id)
    .bind(&activity.mode)
    .bind(&activity.activity_type)
    .bind(&activity.symbol)
    .bind(activity.attribution_status.as_str())
    .bind(activity.fee_micros)
    .bind(&activity.qty_raw)
    .bind(activity.ingested_at_utc)
    .execute(pool)
    .await
    .context("insert_crypto_fee_activity_if_new failed")?;

    Ok(if result.rows_affected() == 1 {
        InsertCryptoFeeActivityOutcome::Inserted
    } else {
        InsertCryptoFeeActivityOutcome::AlreadyExists
    })
}

/// Read the restart-safe ingestion cursor for `(engine_id, mode,
/// activity_type)`. `None` means no activity of this type has ever been
/// successfully ingested for this engine/mode -- the caller should fetch
/// from the beginning (`after_id: None`).
pub async fn fetch_crypto_fee_ingestion_cursor(
    pool: &PgPool,
    broker_account_id: &str,
    engine_id: &str,
    mode: &str,
    activity_type: &str,
) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        r#"
        select last_activity_id
          from sys_crypto_fee_ingestion_cursor
         where broker_account_id = $1 and engine_id = $2 and mode = $3 and activity_type = $4
        "#,
    )
    .bind(broker_account_id)
    .bind(engine_id)
    .bind(mode)
    .bind(activity_type)
    .fetch_optional(pool)
    .await
    .context("fetch_crypto_fee_ingestion_cursor failed")?;

    Ok(row.map(|(id,)| id))
}

/// Outcome of one atomic ingestion batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CryptoFeeIngestionBatchOutcome {
    pub newly_inserted: usize,
    pub already_existed: usize,
}

/// Ingest one batch of already-normalized activities and advance the cursor,
/// all inside one transaction -- the restart-safety property this module
/// exists for. If the process crashes at any point before this transaction
/// commits, the cursor stays at its prior value and every row in `batch`
/// remains un-ingested; the next attempt re-fetches the identical range and
/// re-dedupes cleanly (no gap, no double-apply). If it crashes after commit,
/// every row in `batch` is durably present and the cursor already reflects
/// it -- a retry that re-fetches the same range finds every activity_id
/// already ingested (`AlreadyExists`) and advances nothing further.
///
/// `batch` must be empty only when the caller has nothing new to ingest;
/// this function does not advance the cursor for an empty batch (there is
/// nothing whose `activity_id` the new cursor value could honestly name).
///
/// # B6 correction: scope validation before any mutation
///
/// Every row in `batch` must match the requested `(broker_account_id,
/// engine_id, mode, activity_type)` scope exactly. This is checked for the
/// *entire batch* before any row is inserted or the cursor is touched --
/// one mismatched row refuses the whole call atomically (`Err`, zero
/// mutation), never a partial ingest of the rows that did match.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_crypto_fee_activity_batch(
    pool: &PgPool,
    broker_account_id: &str,
    engine_id: &str,
    mode: &str,
    activity_type: &str,
    batch: &[NewCryptoFeeActivity],
    new_cursor_activity_id: &str,
    updated_at_utc: DateTime<Utc>,
) -> Result<CryptoFeeIngestionBatchOutcome> {
    if batch.is_empty() {
        return Ok(CryptoFeeIngestionBatchOutcome {
            newly_inserted: 0,
            already_existed: 0,
        });
    }

    for activity in batch {
        if activity.broker_account_id != broker_account_id
            || activity.engine_id != engine_id
            || activity.mode != mode
            || activity.activity_type != activity_type
        {
            return Err(anyhow::anyhow!(
                "ingest_crypto_fee_activity_batch: refused -- activity_id={:?} carries scope \
                 (broker_account_id={:?}, engine_id={:?}, mode={:?}, activity_type={:?}) which \
                 does not match the requested batch scope \
                 (broker_account_id={broker_account_id:?}, engine_id={engine_id:?}, \
                 mode={mode:?}, activity_type={activity_type:?}); zero rows mutated",
                activity.activity_id,
                activity.broker_account_id,
                activity.engine_id,
                activity.mode,
                activity.activity_type,
            ));
        }
    }

    let mut tx = pool
        .begin()
        .await
        .context("ingest_crypto_fee_activity_batch: begin tx failed")?;

    let mut newly_inserted = 0usize;
    let mut already_existed = 0usize;

    for activity in batch {
        let result = sqlx::query(
            r#"
            insert into sys_crypto_fee_activity_ledger (
                activity_id, broker_account_id, engine_id, mode, activity_type, symbol,
                attribution_status, fee_micros, qty_raw, ingested_at_utc
            ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            on conflict (broker_account_id, activity_id) do nothing
            "#,
        )
        .bind(&activity.activity_id)
        .bind(&activity.broker_account_id)
        .bind(&activity.engine_id)
        .bind(&activity.mode)
        .bind(&activity.activity_type)
        .bind(&activity.symbol)
        .bind(activity.attribution_status.as_str())
        .bind(activity.fee_micros)
        .bind(&activity.qty_raw)
        .bind(activity.ingested_at_utc)
        .execute(&mut *tx)
        .await
        .context("ingest_crypto_fee_activity_batch: activity insert failed")?;

        if result.rows_affected() == 1 {
            newly_inserted += 1;
        } else {
            already_existed += 1;
        }
    }

    sqlx::query(
        r#"
        insert into sys_crypto_fee_ingestion_cursor (
            broker_account_id, engine_id, mode, activity_type, last_activity_id, updated_at_utc
        ) values ($1, $2, $3, $4, $5, $6)
        on conflict (broker_account_id, engine_id, mode, activity_type)
        do update set last_activity_id = excluded.last_activity_id,
                      updated_at_utc = excluded.updated_at_utc
        "#,
    )
    .bind(broker_account_id)
    .bind(engine_id)
    .bind(mode)
    .bind(activity_type)
    .bind(new_cursor_activity_id)
    .bind(updated_at_utc)
    .execute(&mut *tx)
    .await
    .context("ingest_crypto_fee_activity_batch: cursor advance failed")?;

    tx.commit()
        .await
        .context("ingest_crypto_fee_activity_batch: commit failed")?;

    Ok(CryptoFeeIngestionBatchOutcome {
        newly_inserted,
        already_existed,
    })
}

/// Raw column tuple for [`fetch_crypto_fee_activity`]'s query, factored out
/// to keep the type below clippy's complexity threshold.
type CryptoFeeActivityRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<i64>,
    Option<String>,
    DateTime<Utc>,
);

/// Read back one durably-ingested activity row, for test/audit verification.
///
/// B6 correction: `activity_id` alone is unique only within one broker
/// account (migration 0085), so `broker_account_id` is required to name an
/// unambiguous row -- the same shape as the table's own PRIMARY KEY.
pub async fn fetch_crypto_fee_activity(
    pool: &PgPool,
    broker_account_id: &str,
    activity_id: &str,
) -> Result<Option<NewCryptoFeeActivity>> {
    let row: Option<CryptoFeeActivityRow> = sqlx::query_as(
        r#"
        select activity_id, broker_account_id, engine_id, mode, activity_type, symbol,
               attribution_status, fee_micros, qty_raw, ingested_at_utc
          from sys_crypto_fee_activity_ledger
         where broker_account_id = $1 and activity_id = $2
        "#,
    )
    .bind(broker_account_id)
    .bind(activity_id)
    .fetch_optional(pool)
    .await
    .context("fetch_crypto_fee_activity failed")?;

    row.map(
        |(
            activity_id,
            broker_account_id,
            engine_id,
            mode,
            activity_type,
            symbol,
            attribution_status,
            fee_micros,
            qty_raw,
            ingested_at_utc,
        )| {
            Ok(NewCryptoFeeActivity {
                activity_id,
                broker_account_id,
                engine_id,
                mode,
                activity_type,
                symbol,
                attribution_status: CryptoFeeAttributionStatus::parse(&attribution_status)?,
                fee_micros,
                qty_raw,
                ingested_at_utc,
            })
        },
    )
    .transpose()
}
