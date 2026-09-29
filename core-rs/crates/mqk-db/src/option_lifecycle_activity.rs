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
//! effect are separate steps (`option_lifecycle_correlation` in the daemon,
//! then [`crate::option_lifecycle_journal::apply_lifecycle_adjustment_tx`]).
//!
//! `activity_id` (Alpaca's own id) is the natural idempotency key -- an
//! activity must never be applied to the ledger twice.
//!
//! # Account provenance and shape (migrations 0086, 0088, 0089)
//!
//! `activity_id` alone is unique only within one provider account and a
//! lifecycle row may coexist with an `OPTRD` row under the same id, so the
//! primary key is `(broker_account_id, activity_id, activity_type)` and
//! `broker_account_id` is the canonical provider-account authority key
//! (`alpaca:{provider_account_id}`, migration 0088 -- never the API key id).
//! `symbol` means different things per activity type (the OCC option contract
//! for `OPEXC`/`OPASN`/`OPEXP`; the underlying ticker for `OPTRD`), stored in
//! two CHECK-exclusive columns. Provider correlation/provenance fields are
//! kept verbatim (migration 0089). Pairing a lifecycle row with its `OPTRD` is
//! NOT a same-id lookup here: it is the daemon's evidence-based
//! `option_lifecycle_correlation`, and the economic application lives in
//! `option_lifecycle_journal`.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::option_lifecycle_event_state::{insert_pending_state_row_tx, LifecycleStateSeed};

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

    /// Parse the provider activity-type string (`OPEXC`/`OPASN`/`OPEXP`/`OPTRD`).
    pub fn parse_public(raw: &str) -> Result<Self> {
        Self::parse(raw)
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

/// Provider-supplied fields preserved verbatim next to the normalized
/// columns (migration 0089). Every field is optional: absence is stored as
/// NULL, never invented.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OptionLifecycleRawProvenance {
    pub group_id: Option<String>,
    pub ref_id: Option<String>,
    pub status: Option<String>,
    pub description: Option<String>,
    /// The exact parsed provider record.
    pub raw_json: Option<serde_json::Value>,
}

/// One durable row to insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOptionLifecycleActivity {
    pub activity_id: String,
    /// D1 correction: the authenticated Alpaca account's own
    /// `APCA-API-KEY-ID` (never the secret key). Part of the ledger's
    /// PRIMARY KEY as of migration 0086.
    pub broker_account_id: String,
    pub engine_id: String,
    pub mode: String,
    pub activity_type: OptionLifecycleActivityType,
    /// The OCC option-contract symbol (e.g. `"AAPL260619C00200000"`).
    /// `Some` for `Exercise`/`Assignment`/`Expiration`, `None` for
    /// `PairedTrade` (whose raw `symbol` field is the underlying, not an
    /// option contract -- see `underlying_symbol_raw`). DB-CHECK-enforced.
    pub option_symbol: Option<String>,
    /// The raw Alpaca `symbol` field for a `PairedTrade` row -- the
    /// underlying ticker (e.g. `"AAPL"`), never the option contract.
    /// `Some` only for `PairedTrade`. DB-CHECK-enforced.
    pub underlying_symbol_raw: Option<String>,
    /// Alpaca's own reported activity date, as an opaque string (e.g.
    /// `"2026-06-19"`) -- descriptive only; correlation uses the shared
    /// `activity_id`, not this field.
    pub activity_date: String,
    /// `OPEXC`/`OPASN`/`OPEXP`: contracts affected (signed, per Alpaca's
    /// own convention). `OPTRD`: underlying shares (signed).
    pub qty_raw: String,
    /// `OPTRD` only: strike price per share. `None` for
    /// `OPEXC`/`OPASN`/`OPEXP`.
    pub price_raw: Option<String>,
    /// Alpaca's own reported `net_amount` -- authoritative signed cash
    /// evidence. Always `"0"` for `OPEXC`/`OPASN`/`OPEXP`; the real signed
    /// strike consideration for `OPTRD`. Preserved exactly as reported so
    /// D2 never has to manufacture a cash effect.
    pub net_amount_raw: String,
    pub ingested_at_utc: DateTime<Utc>,
    /// Provider correlation/provenance fields (0089).
    pub provenance: OptionLifecycleRawProvenance,
    /// For `OPEXC`/`OPASN`/`OPEXP` rows: creates the lifecycle event's
    /// `PENDING_EVIDENCE` state row in the same transaction. `None` for
    /// `OPTRD` (which has no lifecycle state of its own).
    pub state_seed: Option<LifecycleStateSeed>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOptionLifecycleActivityOutcome {
    Inserted,
    AlreadyExists,
}

/// Insert one activity row, deduplicated at the DB level on
/// `(broker_account_id, activity_id, activity_type)`.
pub async fn insert_option_lifecycle_activity_if_new(
    pool: &PgPool,
    activity: &NewOptionLifecycleActivity,
) -> Result<InsertOptionLifecycleActivityOutcome> {
    let mut tx = pool
        .begin()
        .await
        .context("insert_option_lifecycle_activity_if_new: begin tx failed")?;
    let inserted = insert_activity_and_seed_tx(&mut tx, activity).await?;
    tx.commit()
        .await
        .context("insert_option_lifecycle_activity_if_new: commit failed")?;
    Ok(if inserted {
        InsertOptionLifecycleActivityOutcome::Inserted
    } else {
        InsertOptionLifecycleActivityOutcome::AlreadyExists
    })
}

/// Insert the raw row and (for a lifecycle row with a state seed) its
/// `PENDING_EVIDENCE` state row, inside the caller's transaction. Returns
/// whether the raw row was new.
async fn insert_activity_and_seed_tx(
    conn: &mut sqlx::PgConnection,
    activity: &NewOptionLifecycleActivity,
) -> Result<bool> {
    let result = sqlx::query(
        r#"
        insert into sys_option_lifecycle_activity_ledger (
            activity_id, broker_account_id, engine_id, mode, activity_type,
            option_symbol, underlying_symbol_raw, activity_date, qty_raw,
            price_raw, net_amount_raw, ingested_at_utc,
            provider_group_id, provider_ref_id, provider_status, description_raw, raw_json
        ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
        on conflict (broker_account_id, activity_id, activity_type) do nothing
        "#,
    )
    .bind(&activity.activity_id)
    .bind(&activity.broker_account_id)
    .bind(&activity.engine_id)
    .bind(&activity.mode)
    .bind(activity.activity_type.as_str())
    .bind(&activity.option_symbol)
    .bind(&activity.underlying_symbol_raw)
    .bind(&activity.activity_date)
    .bind(&activity.qty_raw)
    .bind(&activity.price_raw)
    .bind(&activity.net_amount_raw)
    .bind(activity.ingested_at_utc)
    .bind(&activity.provenance.group_id)
    .bind(&activity.provenance.ref_id)
    .bind(&activity.provenance.status)
    .bind(&activity.provenance.description)
    .bind(&activity.provenance.raw_json)
    .execute(&mut *conn)
    .await
    .context("insert_activity_and_seed_tx: raw insert failed")?;

    if let (Some(seed), Some(option_symbol)) = (&activity.state_seed, &activity.option_symbol) {
        insert_pending_state_row_tx(
            &mut *conn,
            &activity.broker_account_id,
            &activity.activity_id,
            activity.activity_type,
            option_symbol,
            seed,
            activity.ingested_at_utc,
        )
        .await?;
    }
    Ok(result.rows_affected() == 1)
}

pub async fn fetch_option_lifecycle_ingestion_cursor(
    pool: &PgPool,
    broker_account_id: &str,
    engine_id: &str,
    mode: &str,
    activity_type: OptionLifecycleActivityType,
) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        r#"
        select last_activity_id
          from sys_option_lifecycle_ingestion_cursor
         where broker_account_id = $1 and engine_id = $2 and mode = $3 and activity_type = $4
        "#,
    )
    .bind(broker_account_id)
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
///
/// # D1 correction: scope validation before any mutation
///
/// Every row in `batch` must match the requested `(broker_account_id,
/// engine_id, mode, activity_type)` scope exactly, checked for the entire
/// batch before any row is inserted or the cursor is touched -- a
/// mismatched row refuses the whole call atomically.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_option_lifecycle_activity_batch(
    pool: &PgPool,
    broker_account_id: &str,
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

    for activity in batch {
        if activity.broker_account_id != broker_account_id
            || activity.engine_id != engine_id
            || activity.mode != mode
            || activity.activity_type.as_str() != activity_type.as_str()
        {
            return Err(anyhow::anyhow!(
                "ingest_option_lifecycle_activity_batch: refused -- activity_id={:?} carries \
                 scope (broker_account_id={:?}, engine_id={:?}, mode={:?}, activity_type={:?}) \
                 which does not match the requested batch scope \
                 (broker_account_id={broker_account_id:?}, engine_id={engine_id:?}, \
                 mode={mode:?}, activity_type={:?}); zero rows mutated",
                activity.activity_id,
                activity.broker_account_id,
                activity.engine_id,
                activity.mode,
                activity.activity_type.as_str(),
                activity_type.as_str(),
            ));
        }
    }

    let mut tx = pool
        .begin()
        .await
        .context("ingest_option_lifecycle_activity_batch: begin tx failed")?;

    let mut newly_inserted = 0usize;
    let mut already_existed = 0usize;

    for activity in batch {
        if insert_activity_and_seed_tx(&mut tx, activity).await? {
            newly_inserted += 1;
        } else {
            already_existed += 1;
        }
    }

    sqlx::query(
        r#"
        insert into sys_option_lifecycle_ingestion_cursor (
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

#[derive(sqlx::FromRow)]
struct OptionLifecycleActivityRow {
    activity_id: String,
    broker_account_id: String,
    engine_id: String,
    mode: String,
    activity_type: String,
    option_symbol: Option<String>,
    underlying_symbol_raw: Option<String>,
    activity_date: String,
    qty_raw: String,
    price_raw: Option<String>,
    net_amount_raw: String,
    ingested_at_utc: DateTime<Utc>,
    provider_group_id: Option<String>,
    provider_ref_id: Option<String>,
    provider_status: Option<String>,
    description_raw: Option<String>,
    raw_json: Option<serde_json::Value>,
}

fn row_to_activity(row: OptionLifecycleActivityRow) -> Result<NewOptionLifecycleActivity> {
    Ok(NewOptionLifecycleActivity {
        activity_id: row.activity_id,
        broker_account_id: row.broker_account_id,
        engine_id: row.engine_id,
        mode: row.mode,
        activity_type: OptionLifecycleActivityType::parse(&row.activity_type)?,
        option_symbol: row.option_symbol,
        underlying_symbol_raw: row.underlying_symbol_raw,
        activity_date: row.activity_date,
        qty_raw: row.qty_raw,
        price_raw: row.price_raw,
        net_amount_raw: row.net_amount_raw,
        ingested_at_utc: row.ingested_at_utc,
        provenance: OptionLifecycleRawProvenance {
            group_id: row.provider_group_id,
            ref_id: row.provider_ref_id,
            status: row.provider_status,
            description: row.description_raw,
            raw_json: row.raw_json,
        },
        state_seed: None,
    })
}

const OPTION_LIFECYCLE_ACTIVITY_COLUMNS: &str = "activity_id, broker_account_id, engine_id, \
     mode, activity_type, option_symbol, underlying_symbol_raw, activity_date, qty_raw, \
     price_raw, net_amount_raw, ingested_at_utc, provider_group_id, provider_ref_id, \
     provider_status, description_raw, raw_json";

/// Read back one durably-ingested activity row, for test/audit
/// verification and for D2's pairing lookup.
///
/// D1 correction: `activity_id` alone is ambiguous (a lifecycle activity
/// and its paired trade share it) -- `(broker_account_id, activity_id,
/// activity_type)` names an unambiguous row, exactly the table's own
/// PRIMARY KEY.
pub async fn fetch_option_lifecycle_activity(
    pool: &PgPool,
    broker_account_id: &str,
    activity_id: &str,
    activity_type: OptionLifecycleActivityType,
) -> Result<Option<NewOptionLifecycleActivity>> {
    let query = format!(
        "select {OPTION_LIFECYCLE_ACTIVITY_COLUMNS}
           from sys_option_lifecycle_activity_ledger
          where broker_account_id = $1 and activity_id = $2 and activity_type = $3"
    );
    let row: Option<OptionLifecycleActivityRow> = sqlx::query_as(&query)
        .bind(broker_account_id)
        .bind(activity_id)
        .bind(activity_type.as_str())
        .fetch_optional(pool)
        .await
        .context("fetch_option_lifecycle_activity failed")?;

    row.map(row_to_activity).transpose()
}

/// Every raw row of one activity type for one account, in a stable order
/// (`activity_date`, `activity_id`). Bounded by what a single account's
/// option events produce; used by correlation, which needs the whole
/// candidate set (never a `LIMIT 1` pick).
pub async fn list_option_lifecycle_activities(
    pool: &PgPool,
    broker_account_id: &str,
    activity_type: OptionLifecycleActivityType,
) -> Result<Vec<NewOptionLifecycleActivity>> {
    let query = format!(
        "select {OPTION_LIFECYCLE_ACTIVITY_COLUMNS}
           from sys_option_lifecycle_activity_ledger
          where broker_account_id = $1 and activity_type = $2
          order by activity_date asc, activity_id asc"
    );
    let rows: Vec<OptionLifecycleActivityRow> = sqlx::query_as(&query)
        .bind(broker_account_id)
        .bind(activity_type.as_str())
        .fetch_all(pool)
        .await
        .context("list_option_lifecycle_activities failed")?;
    rows.into_iter().map(row_to_activity).collect()
}
