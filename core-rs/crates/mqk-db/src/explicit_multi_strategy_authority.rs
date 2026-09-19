//! MULTI-STRATEGY-RUNTIME-DISPATCH-01 Patch C2 (V4-STAGE-B-M2-REPAIR-03) --
//! durable evidence store for the explicit watchlist-v3 per-symbol
//! multi-strategy authorization mechanism.
//!
//! Deliberately separate from [`crate::dynamic_selection_evidence`]
//! (`sys_dynamic_selection_plans`/*), which represents one Bundle 7 ranking
//! plan -- this mechanism never fabricates a `DynamicSelectionPlan`, and its
//! own evidence is never stored as though it were one. See the frozen
//! contract: `docs/specs/multi_strategy_runtime_dispatch_01a_frozen_contract.md`.
//!
//! `approved_for_live` is constrained `false` at the DB level (migration
//! 0072) in addition to always being constructed `false` here.
//!
//! Idempotent by construction: `authority_id` is a caller-minted
//! deterministic identity binding every result-affecting input (see
//! `mqk-daemon::multi_strategy_runtime_dispatch::derive_explicit_multi_strategy_authority_id`).
//! When `authority_id` already exists, [`insert_explicit_multi_strategy_authority`]
//! fetches the stored header/bindings and compares them canonically
//! (independent of insertion order) against the incoming payload. An exact
//! match is the intended idempotent no-op (`AlreadyExists`); any divergence
//! is a [`InsertExplicitMultiStrategyAuthorityOutcome::PayloadCollision`] --
//! never silently accepted as a replay, never overwrites the original row.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Fixed source-kind label for every row this module writes -- constrained
/// at the DB level too (migration 0072).
pub const EXPLICIT_MULTI_STRATEGY_SOURCE_KIND: &str = "explicit_watchlist_v3";

#[derive(Debug, Clone)]
pub struct NewExplicitMultiStrategyAuthorityBinding {
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe_secs: i64,
    pub authorized: bool,
    pub reason_code: String,
    pub promotion_query_ok: bool,
    pub promotion_state: Option<String>,
    pub promotion_effective: bool,
    pub config_identity_verified: bool,
    pub durable_config_fingerprint: Option<String>,
    pub current_config_fingerprint: Option<String>,
    pub registry_enabled: bool,
    pub data_ready: bool,
    pub promotion_transition_id: Option<String>,
    pub evidence_transition_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewExplicitMultiStrategyAuthority {
    pub authority_id: Uuid,
    pub run_id: Uuid,
    /// Always [`EXPLICIT_MULTI_STRATEGY_SOURCE_KIND`].
    pub source_kind: String,
    /// The configured watchlist path (or other caller-supplied identity of
    /// the source artifact).
    pub source_identity: String,
    /// Exact source artifact identity/hash -- e.g. a content hash of the
    /// watchlist-v3 JSON file this authority was resolved from.
    pub source_artifact_hash: String,
    /// Semantic/config fingerprint truth for the authorized binding set as a
    /// whole (distinct from each binding's own
    /// `durable_config_fingerprint`/`current_config_fingerprint`, which are
    /// per-strategy).
    pub config_fingerprint: String,
    pub market_date: String,
    /// Always `false` -- constrained at the DB level too (migration 0072).
    pub approved_for_live: bool,
    pub writer_version: String,
    pub created_at_utc: DateTime<Utc>,
    pub bindings: Vec<NewExplicitMultiStrategyAuthorityBinding>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExplicitMultiStrategyAuthorityRecord {
    pub authority_id: Uuid,
    pub run_id: Uuid,
    pub source_kind: String,
    pub source_identity: String,
    pub source_artifact_hash: String,
    pub config_fingerprint: String,
    pub market_date: String,
    pub approved_for_live: bool,
    pub binding_count: i32,
    pub authorized_count: i32,
    pub writer_version: String,
    pub created_at_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExplicitMultiStrategyAuthorityBindingRecord {
    pub authority_id: Uuid,
    pub ordinal: i32,
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe_secs: i64,
    pub authorized: bool,
    pub reason_code: String,
    pub promotion_query_ok: bool,
    pub promotion_state: Option<String>,
    pub promotion_effective: bool,
    pub config_identity_verified: bool,
    pub durable_config_fingerprint: Option<String>,
    pub current_config_fingerprint: Option<String>,
    pub registry_enabled: bool,
    pub data_ready: bool,
    pub promotion_transition_id: Option<String>,
    pub evidence_transition_id: Option<String>,
}

/// Every way a [`NewExplicitMultiStrategyAuthority`] fails validation
/// *before* any DB I/O is attempted -- a caller-contract violation, not a
/// storage-layer concern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplicitMultiStrategyAuthorityValidationError {
    /// `approved_for_live` was `true` -- refused before the DB-level
    /// constraint would even be reached.
    ApprovedForLive,
    /// The exact same `(symbol, strategy_id, timeframe_secs)` binding
    /// appeared more than once in `bindings` -- never silently deduplicated.
    DuplicateBinding {
        symbol: String,
        strategy_id: String,
        timeframe_secs: i64,
    },
    /// `bindings` was empty -- an authority with zero bindings is never a
    /// valid durable authorization.
    NoBindings,
}

impl ExplicitMultiStrategyAuthorityValidationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ApprovedForLive => "explicit_multi_strategy_authority_approved_for_live",
            Self::DuplicateBinding { .. } => "explicit_multi_strategy_authority_duplicate_binding",
            Self::NoBindings => "explicit_multi_strategy_authority_no_bindings",
        }
    }
}

/// Validate a [`NewExplicitMultiStrategyAuthority`] before any DB I/O.
/// `binding_count`/`authorized_count` are derived from `bindings` itself
/// (there is no separate caller-supplied count field to drift out of sync --
/// a caller that hands a wrong number of bindings gets exactly the bindings
/// it actually supplied, never a silently mismatched header).
pub fn validate_new_authority(
    new: &NewExplicitMultiStrategyAuthority,
) -> std::result::Result<(), ExplicitMultiStrategyAuthorityValidationError> {
    if new.approved_for_live {
        return Err(ExplicitMultiStrategyAuthorityValidationError::ApprovedForLive);
    }
    if new.bindings.is_empty() {
        return Err(ExplicitMultiStrategyAuthorityValidationError::NoBindings);
    }
    let mut seen: std::collections::HashSet<(&str, &str, i64)> = std::collections::HashSet::new();
    for b in &new.bindings {
        let key = (b.symbol.as_str(), b.strategy_id.as_str(), b.timeframe_secs);
        if !seen.insert(key) {
            return Err(
                ExplicitMultiStrategyAuthorityValidationError::DuplicateBinding {
                    symbol: b.symbol.clone(),
                    strategy_id: b.strategy_id.clone(),
                    timeframe_secs: b.timeframe_secs,
                },
            );
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub enum InsertExplicitMultiStrategyAuthorityOutcome {
    Inserted,
    /// `authority_id` already existed and the stored payload is canonically
    /// identical to this replay -- idempotent no-op.
    AlreadyExists,
    /// `authority_id` already existed but the stored payload diverges from
    /// this replay's payload. Never silently accepted as idempotent and
    /// never overwrites the original row.
    PayloadCollision {
        detail: String,
    },
}

/// Canonical, order-independent snapshot of the header's comparable fields
/// (excludes `created_at_utc`, which is evidence-only wall-clock capture
/// time, not part of the authority's economic identity).
#[derive(Debug, Clone, PartialEq)]
struct HeaderSnapshot {
    run_id: Uuid,
    source_kind: String,
    source_identity: String,
    source_artifact_hash: String,
    config_fingerprint: String,
    market_date: String,
    approved_for_live: bool,
    binding_count: i32,
    authorized_count: i32,
    writer_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct BindingSnapshot {
    symbol: String,
    strategy_id: String,
    timeframe_secs: i64,
    authorized: bool,
    reason_code: String,
    promotion_query_ok: bool,
    promotion_state: Option<String>,
    promotion_effective: bool,
    config_identity_verified: bool,
    durable_config_fingerprint: Option<String>,
    current_config_fingerprint: Option<String>,
    registry_enabled: bool,
    data_ready: bool,
    promotion_transition_id: Option<String>,
    evidence_transition_id: Option<String>,
}

fn new_header_snapshot(new: &NewExplicitMultiStrategyAuthority) -> HeaderSnapshot {
    HeaderSnapshot {
        run_id: new.run_id,
        source_kind: new.source_kind.clone(),
        source_identity: new.source_identity.clone(),
        source_artifact_hash: new.source_artifact_hash.clone(),
        config_fingerprint: new.config_fingerprint.clone(),
        market_date: new.market_date.clone(),
        approved_for_live: new.approved_for_live,
        binding_count: new.bindings.len() as i32,
        authorized_count: new.bindings.iter().filter(|b| b.authorized).count() as i32,
        writer_version: new.writer_version.clone(),
    }
}

fn stored_header_snapshot(rec: &ExplicitMultiStrategyAuthorityRecord) -> HeaderSnapshot {
    HeaderSnapshot {
        run_id: rec.run_id,
        source_kind: rec.source_kind.clone(),
        source_identity: rec.source_identity.clone(),
        source_artifact_hash: rec.source_artifact_hash.clone(),
        config_fingerprint: rec.config_fingerprint.clone(),
        market_date: rec.market_date.clone(),
        approved_for_live: rec.approved_for_live,
        binding_count: rec.binding_count,
        authorized_count: rec.authorized_count,
        writer_version: rec.writer_version.clone(),
    }
}

fn new_binding_snapshots(
    bindings: &[NewExplicitMultiStrategyAuthorityBinding],
) -> Vec<BindingSnapshot> {
    let mut v: Vec<BindingSnapshot> = bindings
        .iter()
        .map(|b| BindingSnapshot {
            symbol: b.symbol.clone(),
            strategy_id: b.strategy_id.clone(),
            timeframe_secs: b.timeframe_secs,
            authorized: b.authorized,
            reason_code: b.reason_code.clone(),
            promotion_query_ok: b.promotion_query_ok,
            promotion_state: b.promotion_state.clone(),
            promotion_effective: b.promotion_effective,
            config_identity_verified: b.config_identity_verified,
            durable_config_fingerprint: b.durable_config_fingerprint.clone(),
            current_config_fingerprint: b.current_config_fingerprint.clone(),
            registry_enabled: b.registry_enabled,
            data_ready: b.data_ready,
            promotion_transition_id: b.promotion_transition_id.clone(),
            evidence_transition_id: b.evidence_transition_id.clone(),
        })
        .collect();
    v.sort();
    v
}

fn stored_binding_snapshots(
    bindings: &[ExplicitMultiStrategyAuthorityBindingRecord],
) -> Vec<BindingSnapshot> {
    let mut v: Vec<BindingSnapshot> = bindings
        .iter()
        .map(|b| BindingSnapshot {
            symbol: b.symbol.clone(),
            strategy_id: b.strategy_id.clone(),
            timeframe_secs: b.timeframe_secs,
            authorized: b.authorized,
            reason_code: b.reason_code.clone(),
            promotion_query_ok: b.promotion_query_ok,
            promotion_state: b.promotion_state.clone(),
            promotion_effective: b.promotion_effective,
            config_identity_verified: b.config_identity_verified,
            durable_config_fingerprint: b.durable_config_fingerprint.clone(),
            current_config_fingerprint: b.current_config_fingerprint.clone(),
            registry_enabled: b.registry_enabled,
            data_ready: b.data_ready,
            promotion_transition_id: b.promotion_transition_id.clone(),
            evidence_transition_id: b.evidence_transition_id.clone(),
        })
        .collect();
    v.sort();
    v
}

const BINDING_COLUMNS: &str = "authority_id, ordinal, symbol, strategy_id, timeframe_secs, \
     authorized, reason_code, promotion_query_ok, promotion_state, promotion_effective, \
     config_identity_verified, durable_config_fingerprint, current_config_fingerprint, \
     registry_enabled, data_ready, promotion_transition_id, evidence_transition_id";

fn binding_select_sql(clause: &str) -> String {
    format!("select {BINDING_COLUMNS} from sys_explicit_multi_strategy_authority_bindings {clause}")
}

fn header_row_to_record(row: &sqlx::postgres::PgRow) -> ExplicitMultiStrategyAuthorityRecord {
    use sqlx::Row;
    ExplicitMultiStrategyAuthorityRecord {
        authority_id: row.get("authority_id"),
        run_id: row.get("run_id"),
        source_kind: row.get("source_kind"),
        source_identity: row.get("source_identity"),
        source_artifact_hash: row.get("source_artifact_hash"),
        config_fingerprint: row.get("config_fingerprint"),
        market_date: row.get("market_date"),
        approved_for_live: row.get("approved_for_live"),
        binding_count: row.get("binding_count"),
        authorized_count: row.get("authorized_count"),
        writer_version: row.get("writer_version"),
        created_at_utc: row.get("created_at_utc"),
    }
}

fn binding_row_to_record(
    row: &sqlx::postgres::PgRow,
) -> ExplicitMultiStrategyAuthorityBindingRecord {
    use sqlx::Row;
    ExplicitMultiStrategyAuthorityBindingRecord {
        authority_id: row.get("authority_id"),
        ordinal: row.get("ordinal"),
        symbol: row.get("symbol"),
        strategy_id: row.get("strategy_id"),
        timeframe_secs: row.get("timeframe_secs"),
        authorized: row.get("authorized"),
        reason_code: row.get("reason_code"),
        promotion_query_ok: row.get("promotion_query_ok"),
        promotion_state: row.get("promotion_state"),
        promotion_effective: row.get("promotion_effective"),
        config_identity_verified: row.get("config_identity_verified"),
        durable_config_fingerprint: row.get("durable_config_fingerprint"),
        current_config_fingerprint: row.get("current_config_fingerprint"),
        registry_enabled: row.get("registry_enabled"),
        data_ready: row.get("data_ready"),
        promotion_transition_id: row.get("promotion_transition_id"),
        evidence_transition_id: row.get("evidence_transition_id"),
    }
}

const HEADER_COLUMNS: &str = "authority_id, run_id, source_kind, source_identity, \
     source_artifact_hash, config_fingerprint, market_date, approved_for_live, \
     binding_count, authorized_count, writer_version, created_at_utc";

/// Insert a [`NewExplicitMultiStrategyAuthority`], validating it first (see
/// [`validate_new_authority`]) -- a validation failure never reaches the DB
/// at all. Mirrors [`crate::dynamic_selection_evidence::insert_dynamic_selection_plan`]'s
/// idempotent-insert-or-collision pattern exactly.
pub async fn insert_explicit_multi_strategy_authority(
    pool: &PgPool,
    new: NewExplicitMultiStrategyAuthority,
) -> Result<InsertExplicitMultiStrategyAuthorityOutcome> {
    if let Err(e) = validate_new_authority(&new) {
        anyhow::bail!(
            "insert_explicit_multi_strategy_authority: validation failed before any DB I/O: {} ({:?})",
            e.code(),
            e
        );
    }

    let mut tx = pool
        .begin()
        .await
        .context("insert_explicit_multi_strategy_authority: begin failed")?;

    let existing_header_row = sqlx::query(&format!(
        "select {HEADER_COLUMNS} from sys_explicit_multi_strategy_authority where authority_id = $1"
    ))
    .bind(new.authority_id)
    .fetch_optional(&mut *tx)
    .await
    .context("insert_explicit_multi_strategy_authority: existence check failed")?;

    if let Some(row) = existing_header_row {
        let existing_record = header_row_to_record(&row);

        let existing_binding_rows = sqlx::query(&binding_select_sql("where authority_id = $1"))
            .bind(new.authority_id)
            .fetch_all(&mut *tx)
            .await
            .context("insert_explicit_multi_strategy_authority: existing bindings query failed")?;
        let existing_bindings: Vec<ExplicitMultiStrategyAuthorityBindingRecord> =
            existing_binding_rows
                .iter()
                .map(binding_row_to_record)
                .collect();

        tx.rollback().await.context(
            "insert_explicit_multi_strategy_authority: rollback (read-only path) failed",
        )?;

        let headers_match = stored_header_snapshot(&existing_record) == new_header_snapshot(&new);
        let bindings_match =
            stored_binding_snapshots(&existing_bindings) == new_binding_snapshots(&new.bindings);

        if headers_match && bindings_match {
            return Ok(InsertExplicitMultiStrategyAuthorityOutcome::AlreadyExists);
        }
        return Ok(
            InsertExplicitMultiStrategyAuthorityOutcome::PayloadCollision {
                detail: format!(
                "authority_id {} already exists with a divergent payload (header fields match: \
                 {}, binding fields match: {}); never treated as idempotent replay",
                new.authority_id, headers_match, bindings_match
            ),
            },
        );
    }

    let binding_count = new.bindings.len() as i32;
    let authorized_count = new.bindings.iter().filter(|b| b.authorized).count() as i32;

    sqlx::query(
        r#"
        insert into sys_explicit_multi_strategy_authority
            (authority_id, run_id, source_kind, source_identity, source_artifact_hash,
             config_fingerprint, market_date, approved_for_live, binding_count,
             authorized_count, writer_version, created_at_utc)
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        "#,
    )
    .bind(new.authority_id)
    .bind(new.run_id)
    .bind(&new.source_kind)
    .bind(&new.source_identity)
    .bind(&new.source_artifact_hash)
    .bind(&new.config_fingerprint)
    .bind(&new.market_date)
    .bind(new.approved_for_live)
    .bind(binding_count)
    .bind(authorized_count)
    .bind(&new.writer_version)
    .bind(new.created_at_utc)
    .execute(&mut *tx)
    .await
    .context("insert_explicit_multi_strategy_authority: insert header row failed")?;

    for (ordinal, b) in new.bindings.iter().enumerate() {
        sqlx::query(
            r#"
            insert into sys_explicit_multi_strategy_authority_bindings
                (authority_id, ordinal, symbol, strategy_id, timeframe_secs, authorized,
                 reason_code, promotion_query_ok, promotion_state, promotion_effective,
                 config_identity_verified, durable_config_fingerprint,
                 current_config_fingerprint, registry_enabled, data_ready,
                 promotion_transition_id, evidence_transition_id)
            values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
            "#,
        )
        .bind(new.authority_id)
        .bind(ordinal as i32)
        .bind(&b.symbol)
        .bind(&b.strategy_id)
        .bind(b.timeframe_secs)
        .bind(b.authorized)
        .bind(&b.reason_code)
        .bind(b.promotion_query_ok)
        .bind(&b.promotion_state)
        .bind(b.promotion_effective)
        .bind(b.config_identity_verified)
        .bind(&b.durable_config_fingerprint)
        .bind(&b.current_config_fingerprint)
        .bind(b.registry_enabled)
        .bind(b.data_ready)
        .bind(&b.promotion_transition_id)
        .bind(&b.evidence_transition_id)
        .execute(&mut *tx)
        .await
        .context("insert_explicit_multi_strategy_authority: insert binding row failed")?;
    }

    tx.commit()
        .await
        .context("insert_explicit_multi_strategy_authority: commit failed")?;

    Ok(InsertExplicitMultiStrategyAuthorityOutcome::Inserted)
}

/// Read-validate seam: fetch the exact durable authority by id, for a caller
/// (Patch C3's daemon-start activation) to verify it exists and matches
/// expectations before releasing dispatch. Returns `Ok(None)` if no row
/// exists -- callers must fail closed on `None`, never treat it as an empty
/// authorization.
pub async fn fetch_explicit_multi_strategy_authority(
    pool: &PgPool,
    authority_id: Uuid,
) -> Result<
    Option<(
        ExplicitMultiStrategyAuthorityRecord,
        Vec<ExplicitMultiStrategyAuthorityBindingRecord>,
    )>,
> {
    let Some(header_row) = sqlx::query(&format!(
        "select {HEADER_COLUMNS} from sys_explicit_multi_strategy_authority where authority_id = $1"
    ))
    .bind(authority_id)
    .fetch_optional(pool)
    .await
    .context("fetch_explicit_multi_strategy_authority: header query failed")?
    else {
        return Ok(None);
    };

    let binding_rows = sqlx::query(&binding_select_sql(
        "where authority_id = $1 order by ordinal",
    ))
    .bind(authority_id)
    .fetch_all(pool)
    .await
    .context("fetch_explicit_multi_strategy_authority: bindings query failed")?;
    let bindings = binding_rows.iter().map(binding_row_to_record).collect();

    Ok(Some((header_row_to_record(&header_row), bindings)))
}

#[cfg(test)]
mod pure_tests {
    use super::*;

    fn binding(
        symbol: &str,
        strategy_id: &str,
        timeframe_secs: i64,
    ) -> NewExplicitMultiStrategyAuthorityBinding {
        NewExplicitMultiStrategyAuthorityBinding {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe_secs,
            authorized: true,
            reason_code: "explicit_multi_strategy_authorized".to_string(),
            promotion_query_ok: true,
            promotion_state: Some("active_paper".to_string()),
            promotion_effective: true,
            config_identity_verified: true,
            durable_config_fingerprint: Some("abc".to_string()),
            current_config_fingerprint: Some("abc".to_string()),
            registry_enabled: true,
            data_ready: true,
            promotion_transition_id: None,
            evidence_transition_id: None,
        }
    }

    fn header(
        bindings: Vec<NewExplicitMultiStrategyAuthorityBinding>,
    ) -> NewExplicitMultiStrategyAuthority {
        NewExplicitMultiStrategyAuthority {
            authority_id: Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test-authority"),
            run_id: Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test-run"),
            source_kind: EXPLICIT_MULTI_STRATEGY_SOURCE_KIND.to_string(),
            source_identity: "test-path".to_string(),
            source_artifact_hash: "hash-a".to_string(),
            config_fingerprint: "cfg-a".to_string(),
            market_date: "2026-09-18".to_string(),
            approved_for_live: false,
            writer_version: "test-writer-v1".to_string(),
            created_at_utc: Utc::now(),
            bindings,
        }
    }

    #[test]
    fn c2_approved_for_live_true_fails_validation() {
        let mut new = header(vec![binding("AAPL", "intraday_scalper", 300)]);
        new.approved_for_live = true;
        assert_eq!(
            validate_new_authority(&new),
            Err(ExplicitMultiStrategyAuthorityValidationError::ApprovedForLive)
        );
    }

    #[test]
    fn c2_empty_bindings_fails_validation() {
        let new = header(vec![]);
        assert_eq!(
            validate_new_authority(&new),
            Err(ExplicitMultiStrategyAuthorityValidationError::NoBindings)
        );
    }

    #[test]
    fn c2_duplicate_binding_fails_validation() {
        let new = header(vec![
            binding("AAPL", "intraday_scalper", 300),
            binding("AAPL", "intraday_scalper", 300),
        ]);
        assert_eq!(
            validate_new_authority(&new),
            Err(
                ExplicitMultiStrategyAuthorityValidationError::DuplicateBinding {
                    symbol: "AAPL".to_string(),
                    strategy_id: "intraday_scalper".to_string(),
                    timeframe_secs: 300,
                }
            )
        );
    }

    #[test]
    fn c2_valid_authority_passes_validation() {
        let new = header(vec![
            binding("AAPL", "intraday_scalper", 300),
            binding("MSFT", "swing_momentum", 86400),
        ]);
        assert_eq!(validate_new_authority(&new), Ok(()));
    }
}
