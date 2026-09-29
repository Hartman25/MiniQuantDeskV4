//! D1 (V4-M5-M8-INDEPENDENT-REVIEW-CORRECTION-01): the actual
//! production-safe options-lifecycle ingestion caller -- mirrors
//! `state/crypto_fee_ingestion.rs`'s composition pattern exactly.
//!
//! Pipeline: read the restart-safe cursor -> fetch activities strictly
//! after it (`OptionLifecycleActivityFetcher::fetch_option_lifecycle_activities_since`)
//! -> normalize every activity
//! (`mqk_broker_alpaca::option_lifecycle_normalize::normalize_option_lifecycle_activity`)
//! -> durably ingest the batch and advance the cursor in one transaction
//! (`mqk_db::ingest_option_lifecycle_activity_batch`). A normalize failure
//! on any activity in the batch fails the whole tick closed (no partial
//! ingestion, cursor never advances).
//!
//! Deliberately NOT wired to a scheduled/spawned background loop and no
//! HTTP route is added by this patch -- nothing in the daemon calls this
//! automatically. Alpaca options trading capability does not exist in this
//! codebase yet; this is the durable evidence layer D2/D3 require.

use anyhow::Context;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use mqk_broker_alpaca::option_lifecycle_normalize::{
    normalize_option_lifecycle_activity, NormalizedOptionLifecycleActivity,
};
use mqk_broker_alpaca::types::AlpacaFeeActivity;
use mqk_db::option_lifecycle_activity::{NewOptionLifecycleActivity, OptionLifecycleActivityType};
use mqk_db::OptionLifecycleIngestionBatchOutcome;

use super::OptionLifecycleActivityFetcher;

/// Translate one normalized activity into the durable
/// [`NewOptionLifecycleActivity`] shape. Pure mapping -- every field comes
/// from the already-normalized record or the caller's own identity
/// context, never invented.
fn new_option_lifecycle_activity_from_record(
    record: &NormalizedOptionLifecycleActivity,
    broker_account_id: &str,
    engine_id: &str,
    mode: &str,
    ingested_at_utc: DateTime<Utc>,
) -> anyhow::Result<NewOptionLifecycleActivity> {
    match record {
        NormalizedOptionLifecycleActivity::Lifecycle {
            activity_id,
            activity_type,
            option_symbol,
            activity_date,
            qty_raw,
            net_amount_raw,
        } => {
            let parsed_type = match activity_type.as_str() {
                "OPEXC" => OptionLifecycleActivityType::Exercise,
                "OPASN" => OptionLifecycleActivityType::Assignment,
                "OPEXP" => OptionLifecycleActivityType::Expiration,
                other => anyhow::bail!(
                    "new_option_lifecycle_activity_from_record: unexpected lifecycle \
                     activity_type {other:?} for activity_id={activity_id:?}"
                ),
            };
            Ok(NewOptionLifecycleActivity {
                activity_id: activity_id.clone(),
                broker_account_id: broker_account_id.to_string(),
                engine_id: engine_id.to_string(),
                mode: mode.to_string(),
                activity_type: parsed_type,
                option_symbol: Some(option_symbol.clone()),
                underlying_symbol_raw: None,
                activity_date: activity_date.clone(),
                qty_raw: qty_raw.clone(),
                price_raw: None,
                net_amount_raw: net_amount_raw.clone(),
                ingested_at_utc,
            })
        }
        NormalizedOptionLifecycleActivity::PairedTrade {
            activity_id,
            underlying_symbol_raw,
            activity_date,
            qty_raw,
            price_raw,
            net_amount_raw,
        } => Ok(NewOptionLifecycleActivity {
            activity_id: activity_id.clone(),
            broker_account_id: broker_account_id.to_string(),
            engine_id: engine_id.to_string(),
            mode: mode.to_string(),
            activity_type: OptionLifecycleActivityType::PairedTrade,
            option_symbol: None,
            underlying_symbol_raw: Some(underlying_symbol_raw.clone()),
            activity_date: activity_date.clone(),
            qty_raw: qty_raw.clone(),
            price_raw: Some(price_raw.clone()),
            net_amount_raw: net_amount_raw.clone(),
            ingested_at_utc,
        }),
    }
}

/// Run one restart-safe ingestion attempt for `(engine_id, mode,
/// activity_type)`, scoped to `fetcher`'s own authenticated broker
/// account: read the durable cursor, fetch strictly-after activities
/// through `fetcher`, normalize every one, and durably ingest the batch +
/// advance the cursor atomically.
///
/// Restart safety mirrors `ingest_crypto_fee_activities_once` exactly: the
/// cursor is read fresh from `pool` on every call (never cached across
/// calls) and passed as `fetcher`'s `after_id`. An empty fetch result is a
/// pure no-op. A normalize failure on any activity aborts the whole
/// attempt with `Err` before any DB write -- the cursor is never advanced
/// past evidence this caller could not durably record.
pub async fn ingest_option_lifecycle_activities_once(
    pool: &PgPool,
    fetcher: &dyn OptionLifecycleActivityFetcher,
    engine_id: &str,
    mode: &str,
    activity_type: &str,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<OptionLifecycleIngestionBatchOutcome> {
    let authority = fetcher.broker_account_authority().map_err(|e| {
        anyhow::anyhow!(
            "ingest_option_lifecycle_activities_once: account authority unavailable: {e}"
        )
    })?;
    if !mode.eq_ignore_ascii_case(authority.deployment_mode()) {
        anyhow::bail!(
            "ingest_option_lifecycle_activities_once: refused -- requested mode {mode:?} does              not match the account's verified deployment mode {:?}",
            authority.deployment_mode()
        );
    }
    mqk_db::verify_or_register_broker_account_authority(pool, &authority, now_utc)
        .await
        .context(
            "ingest_option_lifecycle_activities_once: account authority registration failed",
        )?;
    let broker_account_id = authority.key();
    let parsed_activity_type = match activity_type {
        "OPEXC" => OptionLifecycleActivityType::Exercise,
        "OPASN" => OptionLifecycleActivityType::Assignment,
        "OPEXP" => OptionLifecycleActivityType::Expiration,
        "OPTRD" => OptionLifecycleActivityType::PairedTrade,
        other => anyhow::bail!(
            "ingest_option_lifecycle_activities_once: activity_type must be one of \
             OPEXC/OPASN/OPEXP/OPTRD, got {other:?}"
        ),
    };

    let cursor = mqk_db::option_lifecycle_activity::fetch_option_lifecycle_ingestion_cursor(
        pool,
        &broker_account_id,
        engine_id,
        mode,
        parsed_activity_type,
    )
    .await
    .context(
        "ingest_option_lifecycle_activities_once: fetch_option_lifecycle_ingestion_cursor failed",
    )?;

    let raw: Vec<AlpacaFeeActivity> = fetcher
        .fetch_option_lifecycle_activities_since(activity_type, cursor.as_deref())
        .map_err(|e| {
            anyhow::anyhow!("ingest_option_lifecycle_activities_once: activity fetch failed: {e}")
        })?;

    if raw.is_empty() {
        return Ok(OptionLifecycleIngestionBatchOutcome {
            newly_inserted: 0,
            already_existed: 0,
        });
    }

    let mut batch = Vec::with_capacity(raw.len());
    for activity in &raw {
        let record = normalize_option_lifecycle_activity(activity).map_err(|e| {
            anyhow::anyhow!(
                "ingest_option_lifecycle_activities_once: normalize failed for \
                 activity_id={:?}: {e}",
                activity.id
            )
        })?;
        batch.push(new_option_lifecycle_activity_from_record(
            &record,
            &broker_account_id,
            engine_id,
            mode,
            now_utc,
        )?);
    }

    // Ascending order is `fetch_option_lifecycle_activities_since`'s
    // documented contract (`direction=asc`) — the last element is the new
    // watermark.
    let new_cursor_activity_id = &raw.last().expect("raw is non-empty; checked above").id;

    mqk_db::option_lifecycle_activity::ingest_option_lifecycle_activity_batch(
        pool,
        &broker_account_id,
        engine_id,
        mode,
        parsed_activity_type,
        &batch,
        new_cursor_activity_id,
        now_utc,
    )
    .await
    .context(
        "ingest_option_lifecycle_activities_once: ingest_option_lifecycle_activity_batch failed",
    )
}
