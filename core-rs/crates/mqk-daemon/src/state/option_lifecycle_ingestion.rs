//! D1: the production-safe options-lifecycle ingestion caller over the
//! dedicated provider type `AlpacaOptionLifecycleActivity`.
//!
//! Pipeline: establish the provider account authority (authenticated
//! `GET /v2/account`, never the credential) -> register it -> read the
//! restart-safe, account-scoped cursor -> fetch activities strictly after it
//! -> normalize every activity -> durably ingest the batch, create each
//! lifecycle event's `PENDING_EVIDENCE` state row and advance the cursor in
//! one transaction. A normalize failure on any activity fails the whole tick
//! closed (no partial ingestion, cursor never advances).
//!
//! Callers: `option_lifecycle_cycle::run_option_lifecycle_cycle`, driven by
//! the daemon's `option_lifecycle_poll` task.

use anyhow::Context;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use mqk_broker_alpaca::option_lifecycle_normalize::{
    normalize_option_lifecycle_activity, NormalizedOptionLifecycleActivity,
    OptionLifecycleProvenance,
};
use mqk_broker_alpaca::types::AlpacaOptionLifecycleActivity;
use mqk_db::option_lifecycle_activity::{
    NewOptionLifecycleActivity, OptionLifecycleActivityType, OptionLifecycleRawProvenance,
};
use mqk_db::{LifecycleStateSeed, OptionLifecycleIngestionBatchOutcome};
use mqk_execution::OptionContractIdentity;

use super::{ExecutionDomain, OptionLifecycleActivityFetcher};

/// US equity options settle into the equity account/domain.
pub const OPTION_LIFECYCLE_EXECUTION_DOMAIN: ExecutionDomain = ExecutionDomain::EquityNyse;

fn raw_provenance(
    provenance: &OptionLifecycleProvenance,
    raw: &AlpacaOptionLifecycleActivity,
) -> anyhow::Result<OptionLifecycleRawProvenance> {
    Ok(OptionLifecycleRawProvenance {
        group_id: provenance.group_id.clone(),
        ref_id: provenance.ref_id.clone(),
        status: provenance.status.clone(),
        description: provenance.description.clone(),
        raw_json: Some(
            serde_json::to_value(raw).context("serializing the raw provider record failed")?,
        ),
    })
}

/// Translate one normalized activity into the durable
/// [`NewOptionLifecycleActivity`] shape. Pure mapping -- every field comes
/// from the normalized record, the raw provider record, or the caller's
/// identity context, never invented.
fn new_option_lifecycle_activity_from_record(
    record: &NormalizedOptionLifecycleActivity,
    raw: &AlpacaOptionLifecycleActivity,
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
            provenance,
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
            // Normalization already proved the symbol parses; the underlying
            // is derived from the contract, never from a free-form field.
            let contract = OptionContractIdentity::parse(option_symbol)
                .context("lifecycle option symbol failed OCC parse after normalization")?;
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
                provenance: raw_provenance(provenance, raw)?,
                state_seed: Some(LifecycleStateSeed {
                    execution_domain: OPTION_LIFECYCLE_EXECUTION_DOMAIN.as_str().to_string(),
                    underlying_symbol: Some(contract.underlying().to_string()),
                }),
            })
        }
        NormalizedOptionLifecycleActivity::PairedTrade {
            activity_id,
            underlying_symbol_raw,
            activity_date,
            qty_raw,
            price_raw,
            net_amount_raw,
            provenance,
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
            provenance: raw_provenance(provenance, raw)?,
            state_seed: None,
        }),
    }
}

/// Run one restart-safe ingestion attempt for `(engine_id, mode,
/// activity_type)`, scoped to the fetcher's provider account.
///
/// The cursor is read fresh from `pool` on every call and passed as
/// `fetcher`'s `after_id`. An empty fetch is a pure no-op. A normalize
/// failure aborts the attempt before any DB write.
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
            "ingest_option_lifecycle_activities_once: refused -- requested mode {mode:?} does \
             not match the account's verified deployment mode {:?}",
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

    let raw: Vec<AlpacaOptionLifecycleActivity> = fetcher
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
        if activity.activity_type != activity_type {
            anyhow::bail!(
                "ingest_option_lifecycle_activities_once: fetched activity_id={:?} has \
                 activity_type {:?} but the request was for {activity_type:?}; refusing the \
                 whole batch",
                activity.id,
                activity.activity_type
            );
        }
        let record = normalize_option_lifecycle_activity(activity).map_err(|e| {
            anyhow::anyhow!(
                "ingest_option_lifecycle_activities_once: normalize failed for \
                 activity_id={:?}: {e}",
                activity.id
            )
        })?;
        batch.push(new_option_lifecycle_activity_from_record(
            &record,
            activity,
            &broker_account_id,
            engine_id,
            mode,
            now_utc,
        )?);
    }

    // Ascending order is `fetch_option_lifecycle_activities_since`'s
    // documented contract (`direction=asc`) -- the last element is the new
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
