//! B6 correction (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION):
//! the actual production-safe ingestion caller B6's original commit left
//! unbuilt. `mqk_broker_alpaca::AlpacaBrokerAdapter::fetch_fee_activities_since`
//! (REST fetch), `mqk_broker_alpaca::fee_attribution::normalize_fee_activity`
//! (normalize), and `mqk_db::crypto_fee_activity::ingest_crypto_fee_activity_batch`
//! (durable dedup + cursor advance) each existed and were each independently
//! proven — but nothing composed them into a real caller. A DB repository
//! with zero production caller is not durable evidence; this module is that
//! caller.
//!
//! Pipeline: read the restart-safe cursor -> fetch activities strictly after
//! it (`CryptoFeeActivityFetcher::fetch_fee_activities_since`) -> normalize
//! every activity -> durably ingest the batch and advance the cursor in one
//! transaction (`mqk_db::ingest_crypto_fee_activity_batch`, already proven
//! restart-safe/idempotent by B6's own F01-F07 tests). A normalize failure
//! on any activity in the batch fails the whole tick closed (no partial
//! ingestion, cursor never advances) rather than silently dropping evidence
//! or guessing a value Alpaca did not confirm.
//!
//! Deliberately NOT wired to a scheduled/spawned background loop — nothing
//! in the daemon calls this automatically. Crypto capability remains
//! default off (D2/B4); no real broker activity is created by this patch.
//! No invented per-fill attribution, no synthetic `Fill` — this is fee
//! evidence only, exactly as B6's ledger schema enforces.

use anyhow::Context;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use mqk_broker_alpaca::fee_attribution::{normalize_fee_activity, FeeAttributionRecord};
use mqk_broker_alpaca::types::AlpacaFeeActivity;
use mqk_db::{CryptoFeeAttributionStatus, CryptoFeeIngestionBatchOutcome, NewCryptoFeeActivity};

use super::CryptoFeeActivityFetcher;

/// Translate one normalized [`FeeAttributionRecord`] into the durable
/// [`NewCryptoFeeActivity`] shape. Pure mapping — every field comes from the
/// already-normalized record or the caller's own identity context, never
/// invented.
fn new_crypto_fee_activity_from_record(
    record: &FeeAttributionRecord,
    broker_account_id: &str,
    engine_id: &str,
    mode: &str,
    ingested_at_utc: DateTime<Utc>,
) -> NewCryptoFeeActivity {
    match record {
        FeeAttributionRecord::CashFee {
            activity_id,
            activity_type,
            symbol,
            fee_micros,
        } => NewCryptoFeeActivity {
            activity_id: activity_id.clone(),
            broker_account_id: broker_account_id.to_string(),
            engine_id: engine_id.to_string(),
            mode: mode.to_string(),
            activity_type: activity_type.clone(),
            symbol: symbol.clone(),
            attribution_status: CryptoFeeAttributionStatus::CashFee,
            fee_micros: Some(*fee_micros),
            qty_raw: None,
            ingested_at_utc,
        },
        FeeAttributionRecord::AssetDenominatedFeeUnsupported {
            activity_id,
            activity_type,
            symbol,
            qty_raw,
        } => NewCryptoFeeActivity {
            activity_id: activity_id.clone(),
            broker_account_id: broker_account_id.to_string(),
            engine_id: engine_id.to_string(),
            mode: mode.to_string(),
            activity_type: activity_type.clone(),
            symbol: symbol.clone(),
            attribution_status: CryptoFeeAttributionStatus::AssetDenominatedFeeUnsupported,
            fee_micros: None,
            qty_raw: Some(qty_raw.clone()),
            ingested_at_utc,
        },
        FeeAttributionRecord::ConfirmedZeroFee {
            activity_id,
            activity_type,
            symbol,
        } => NewCryptoFeeActivity {
            activity_id: activity_id.clone(),
            broker_account_id: broker_account_id.to_string(),
            engine_id: engine_id.to_string(),
            mode: mode.to_string(),
            activity_type: activity_type.clone(),
            symbol: symbol.clone(),
            attribution_status: CryptoFeeAttributionStatus::ConfirmedZeroFee,
            fee_micros: None,
            qty_raw: None,
            ingested_at_utc,
        },
    }
}

/// Run one restart-safe ingestion attempt for `(engine_id, mode,
/// activity_type)`: read the durable cursor, fetch strictly-after activities
/// through `fetcher`, normalize every one, and durably ingest the batch +
/// advance the cursor atomically.
///
/// Restart safety: the cursor is read fresh from `pool` on every call (never
/// cached across calls) and passed as `fetcher`'s `after_id`, so a repeated
/// call — including one made by an entirely different process after a crash
/// — resumes from exactly where the last successfully committed transaction
/// left off. `ingest_crypto_fee_activity_batch`'s own dedup (`activity_id`
/// PRIMARY KEY) makes an overlapping re-fetch a safe no-op even if the
/// fetched range happens to include already-ingested activities.
///
/// An empty fetch result is a pure no-op: zero DB writes, cursor unchanged.
/// A normalize failure on any activity aborts the whole attempt with `Err`
/// before any DB write — the cursor is never advanced past evidence this
/// caller could not durably record.
pub async fn ingest_crypto_fee_activities_once(
    pool: &PgPool,
    fetcher: &dyn CryptoFeeActivityFetcher,
    engine_id: &str,
    mode: &str,
    activity_type: &str,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<CryptoFeeIngestionBatchOutcome> {
    let broker_account_id = fetcher.broker_account_id();

    let cursor = mqk_db::fetch_crypto_fee_ingestion_cursor(
        pool,
        &broker_account_id,
        engine_id,
        mode,
        activity_type,
    )
    .await
    .context("ingest_crypto_fee_activities_once: fetch_crypto_fee_ingestion_cursor failed")?;

    let raw: Vec<AlpacaFeeActivity> = fetcher
        .fetch_fee_activities_since(activity_type, cursor.as_deref())
        .map_err(|e| {
            anyhow::anyhow!("ingest_crypto_fee_activities_once: fee-activity fetch failed: {e}")
        })?;

    if raw.is_empty() {
        return Ok(CryptoFeeIngestionBatchOutcome {
            newly_inserted: 0,
            already_existed: 0,
        });
    }

    let mut batch = Vec::with_capacity(raw.len());
    for activity in &raw {
        let record = normalize_fee_activity(activity).map_err(|e| {
            anyhow::anyhow!(
                "ingest_crypto_fee_activities_once: normalize_fee_activity failed for \
                 activity_id={:?}: {e}",
                activity.id
            )
        })?;
        batch.push(new_crypto_fee_activity_from_record(
            &record,
            &broker_account_id,
            engine_id,
            mode,
            now_utc,
        ));
    }

    // Ascending order is `fetch_fee_activities_since`'s documented contract
    // (`direction=asc`) — the last element is the new watermark.
    let new_cursor_activity_id = &raw.last().expect("raw is non-empty; checked above").id;

    mqk_db::ingest_crypto_fee_activity_batch(
        pool,
        &broker_account_id,
        engine_id,
        mode,
        activity_type,
        &batch,
        new_cursor_activity_id,
        now_utc,
    )
    .await
    .context("ingest_crypto_fee_activities_once: ingest_crypto_fee_activity_batch failed")
}
