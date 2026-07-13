//! STRATEGY-PROMOTION-REGISTRY-01B: durable, append-only strategy
//! paper-promotion registry.
//!
//! `sys_strategy_promotion_transitions` is the authoritative history of
//! every promotion state transition for an exact
//! `(strategy_id, symbol, timeframe_secs)` identity. There is no separate
//! mutable "current state" row: current state is always derived by
//! querying the latest transition per identity
//! ([`fetch_current_promotion_state`]).
//!
//! `registered + enabled` (`sys_strategy_registry.enabled`) is **not**
//! promotion approval. An empty transition history for an identity means
//! "no authorization inferred", never an implicit approval.
//!
//! See `docs/specs/strategy_promotion_registry_01a_current_truth_and_contract.md`
//! for the full design contract (states, legal transition graph, identity
//! boundary, evidence trust boundary).

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Promotion states
// ---------------------------------------------------------------------------

/// The six canonical promotion states, `snake_case` on the wire/DB.
pub const PROMOTION_STATE_SHADOW_APPROVED: &str = "shadow_approved";
pub const PROMOTION_STATE_PAPER_APPROVED: &str = "paper_approved";
pub const PROMOTION_STATE_ACTIVE_PAPER: &str = "active_paper";
pub const PROMOTION_STATE_DEMOTED: &str = "demoted";
pub const PROMOTION_STATE_RETIRED: &str = "retired";
pub const PROMOTION_STATE_REJECTED: &str = "rejected";

const ALL_STATES: [&str; 6] = [
    PROMOTION_STATE_SHADOW_APPROVED,
    PROMOTION_STATE_PAPER_APPROVED,
    PROMOTION_STATE_ACTIVE_PAPER,
    PROMOTION_STATE_DEMOTED,
    PROMOTION_STATE_RETIRED,
    PROMOTION_STATE_REJECTED,
];

/// `true` iff `state` is one of the six canonical promotion states.
pub fn is_known_promotion_state(state: &str) -> bool {
    ALL_STATES.contains(&state)
}

/// Pure mirror of the DB `CHECK` constraint's legal transition graph
/// (`sys_strategy_promotion_transitions_legal_graph`). Used by the daemon
/// route layer to reject an illegal transition *before* attempting an
/// insert, so a rejected transition never reaches the DB.
///
/// `previous` is `None` only for the very first transition of a new
/// identity (`no state -> shadow_approved`).
pub fn is_legal_transition(previous: Option<&str>, new_state: &str) -> bool {
    match previous {
        None => new_state == PROMOTION_STATE_SHADOW_APPROVED,
        Some(PROMOTION_STATE_SHADOW_APPROVED) => matches!(
            new_state,
            PROMOTION_STATE_PAPER_APPROVED | PROMOTION_STATE_REJECTED | PROMOTION_STATE_RETIRED
        ),
        Some(PROMOTION_STATE_PAPER_APPROVED) => matches!(
            new_state,
            PROMOTION_STATE_ACTIVE_PAPER | PROMOTION_STATE_DEMOTED | PROMOTION_STATE_RETIRED
        ),
        Some(PROMOTION_STATE_ACTIVE_PAPER) => {
            matches!(new_state, PROMOTION_STATE_DEMOTED | PROMOTION_STATE_RETIRED)
        }
        Some(PROMOTION_STATE_DEMOTED) => matches!(
            new_state,
            PROMOTION_STATE_SHADOW_APPROVED | PROMOTION_STATE_RETIRED
        ),
        // retired / rejected are terminal: no legal transition out of them.
        Some(PROMOTION_STATE_RETIRED) | Some(PROMOTION_STATE_REJECTED) => false,
        Some(_) => false,
    }
}

/// `true` iff `new_state` requires freshly validated `paper_candidate`
/// evidence before the transition may be inserted (`no state ->
/// shadow_approved`, and `demoted -> shadow_approved` re-approval).
pub fn transition_requires_evidence(previous: Option<&str>, new_state: &str) -> bool {
    new_state == PROMOTION_STATE_SHADOW_APPROVED
        && matches!(previous, None | Some(PROMOTION_STATE_DEMOTED))
}

// ---------------------------------------------------------------------------
// Timeframe normalization
// ---------------------------------------------------------------------------

/// Convert a scanner/review-artifact timeframe label (e.g. `"1D"`, `"1H"`,
/// `"5m"`) to canonical runtime `timeframe_secs`. Used **only** at
/// evidence-validation time (approval creation/re-approval) — runtime
/// decision paths already carry `timeframe_secs` natively and never need
/// this conversion. Unknown labels return `None`; callers must fail
/// closed rather than guess a default.
pub fn scanner_timeframe_label_to_secs(label: &str) -> Option<i64> {
    match label.trim() {
        "1m" => Some(60),
        "5m" => Some(300),
        "15m" => Some(900),
        "30m" => Some(1800),
        "1H" => Some(3600),
        "1D" => Some(86400),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// One row from `sys_strategy_promotion_transitions`.
#[derive(Debug, Clone)]
pub struct StrategyPromotionTransitionRecord {
    pub transition_id: Uuid,
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe_secs: i64,
    /// Identity-v1 bounded fallback: always `None` in this patch.
    pub config_fingerprint: Option<String>,
    /// Always `"unavailable_in_current_runtime"` in this patch.
    pub config_identity_status: String,
    pub previous_state: Option<String>,
    pub new_state: String,
    /// STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01 (Phase B): the exact
    /// transition this one was chained from; `None` only for the very
    /// first transition of an identity. Lineage bookkeeping, not itself
    /// the concurrency-safety mechanism -- see
    /// [`insert_strategy_promotion_transition_serialized`].
    pub parent_transition_id: Option<Uuid>,
    /// STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01 (Phase D): the exact
    /// transition that established the evidence currently authorizing
    /// this identity's state -- either this row's own `transition_id`
    /// (when this row is itself evidence-bearing) or carried forward
    /// unchanged from the previous current record. Resolve with
    /// [`resolve_evidence_lineage`].
    pub evidence_transition_id: Option<Uuid>,
    pub evidence_review_id: Option<String>,
    pub evidence_scanner_scan_id: Option<String>,
    pub evidence_git_hash: Option<String>,
    pub evidence_artifact_path: Option<String>,
    pub evidence_fingerprint: Option<String>,
    pub effective_at_utc: DateTime<Utc>,
    /// Only `active_paper` gives this field any tradability effect (see
    /// [`evaluate_promotion_tradability`]). Every other state may still
    /// carry a value here (e.g. a pre-set expiry queued for after a
    /// future re-promotion), but it is inert until/unless that identity
    /// reaches `active_paper` again.
    pub expires_at_utc: Option<DateTime<Utc>>,
    pub initiated_by: String,
    pub reason: String,
    pub created_at_utc: DateTime<Utc>,
}

/// Arguments for inserting a new promotion transition.
///
/// `transition_id` must be caller-derived (deterministic UUIDv5 from the
/// full transition request content is the daemon route layer's
/// convention) — never `Uuid::new_v4()` — so a replayed identical request
/// is idempotent. `previous_state` must be computed by the caller from
/// [`fetch_current_promotion_state`] immediately before constructing this
/// struct; this module does not trust or re-derive it.
#[derive(Debug, Clone)]
pub struct InsertStrategyPromotionTransitionArgs {
    pub transition_id: Uuid,
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe_secs: i64,
    pub config_fingerprint: Option<String>,
    pub config_identity_status: String,
    pub previous_state: Option<String>,
    pub new_state: String,
    /// See [`StrategyPromotionTransitionRecord::parent_transition_id`].
    /// The caller must set this to the identity's actual current
    /// transition id (or `None` for the first transition) immediately
    /// before calling [`insert_strategy_promotion_transition_serialized`]
    /// -- that function re-validates it against a freshly locked read and
    /// rejects a stale value rather than trusting this field blindly.
    pub parent_transition_id: Option<Uuid>,
    /// See [`StrategyPromotionTransitionRecord::evidence_transition_id`].
    pub evidence_transition_id: Option<Uuid>,
    pub evidence_review_id: Option<String>,
    pub evidence_scanner_scan_id: Option<String>,
    pub evidence_git_hash: Option<String>,
    pub evidence_artifact_path: Option<String>,
    pub evidence_fingerprint: Option<String>,
    pub effective_at_utc: DateTime<Utc>,
    pub expires_at_utc: Option<DateTime<Utc>>,
    pub initiated_by: String,
    pub reason: String,
    pub created_at_utc: DateTime<Utc>,
}

fn row_to_record(
    r: sqlx::postgres::PgRow,
) -> Result<StrategyPromotionTransitionRecord, sqlx::Error> {
    Ok(StrategyPromotionTransitionRecord {
        transition_id: r.try_get("transition_id")?,
        strategy_id: r.try_get("strategy_id")?,
        symbol: r.try_get("symbol")?,
        timeframe_secs: r.try_get("timeframe_secs")?,
        config_fingerprint: r.try_get("config_fingerprint")?,
        config_identity_status: r.try_get("config_identity_status")?,
        previous_state: r.try_get("previous_state")?,
        new_state: r.try_get("new_state")?,
        parent_transition_id: r.try_get("parent_transition_id")?,
        evidence_transition_id: r.try_get("evidence_transition_id")?,
        evidence_review_id: r.try_get("evidence_review_id")?,
        evidence_scanner_scan_id: r.try_get("evidence_scanner_scan_id")?,
        evidence_git_hash: r.try_get("evidence_git_hash")?,
        evidence_artifact_path: r.try_get("evidence_artifact_path")?,
        evidence_fingerprint: r.try_get("evidence_fingerprint")?,
        effective_at_utc: r.try_get("effective_at_utc")?,
        expires_at_utc: r.try_get("expires_at_utc")?,
        initiated_by: r.try_get("initiated_by")?,
        reason: r.try_get("reason")?,
        created_at_utc: r.try_get("created_at_utc")?,
    })
}

const SELECT_COLUMNS: &str = r#"
    transition_id, strategy_id, symbol, timeframe_secs,
    config_fingerprint, config_identity_status,
    previous_state, new_state,
    parent_transition_id, evidence_transition_id,
    evidence_review_id, evidence_scanner_scan_id, evidence_git_hash,
    evidence_artifact_path, evidence_fingerprint,
    effective_at_utc, expires_at_utc, initiated_by, reason, created_at_utc
"#;

// ---------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------

/// Insert a new promotion transition row.
///
/// Idempotent via `ON CONFLICT (transition_id) DO NOTHING`. Returns
/// `Ok(true)` when a new row was actually inserted, `Ok(false)` when the
/// `transition_id` already existed (duplicate/replay — no-op, not an
/// error). Returns `Err` if `strategy_id`/`symbol` are blank or
/// `symbol == "*"`, if `initiated_by` is blank, or if `timeframe_secs` is
/// not positive — validated before any DB contact, mirroring the DB
/// `CHECK` constraints as defense in depth. Does **not** validate
/// transition legality — callers must call [`is_legal_transition`] (using
/// a `previous_state` freshly obtained from
/// [`fetch_current_promotion_state`]) before calling this function; the DB
/// `CHECK` constraint is a structural backstop against an application bug,
/// not the primary enforcement point.
pub async fn insert_strategy_promotion_transition(
    pool: &PgPool,
    args: &InsertStrategyPromotionTransitionArgs,
) -> Result<bool> {
    if args.strategy_id.trim().is_empty() {
        anyhow::bail!("insert_strategy_promotion_transition: strategy_id must not be empty");
    }
    if args.symbol.trim().is_empty() {
        anyhow::bail!("insert_strategy_promotion_transition: symbol must not be empty");
    }
    if args.symbol.trim() == "*" {
        anyhow::bail!("insert_strategy_promotion_transition: wildcard symbol '*' is forbidden");
    }
    if args.timeframe_secs <= 0 {
        anyhow::bail!("insert_strategy_promotion_transition: timeframe_secs must be positive");
    }
    if args.initiated_by.trim().is_empty() {
        anyhow::bail!("insert_strategy_promotion_transition: initiated_by must not be empty");
    }
    if !is_known_promotion_state(&args.new_state) {
        anyhow::bail!(
            "insert_strategy_promotion_transition: unknown new_state '{}'",
            args.new_state
        );
    }

    let result = sqlx::query(
        r#"
        insert into sys_strategy_promotion_transitions (
            transition_id, strategy_id, symbol, timeframe_secs,
            config_fingerprint, config_identity_status,
            previous_state, new_state,
            parent_transition_id, evidence_transition_id,
            evidence_review_id, evidence_scanner_scan_id, evidence_git_hash,
            evidence_artifact_path, evidence_fingerprint,
            effective_at_utc, expires_at_utc, initiated_by, reason, created_at_utc
        )
        values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)
        on conflict (transition_id) do nothing
        "#,
    )
    .bind(args.transition_id)
    .bind(&args.strategy_id)
    .bind(&args.symbol)
    .bind(args.timeframe_secs)
    .bind(&args.config_fingerprint)
    .bind(&args.config_identity_status)
    .bind(&args.previous_state)
    .bind(&args.new_state)
    .bind(args.parent_transition_id)
    .bind(args.evidence_transition_id)
    .bind(&args.evidence_review_id)
    .bind(&args.evidence_scanner_scan_id)
    .bind(&args.evidence_git_hash)
    .bind(&args.evidence_artifact_path)
    .bind(&args.evidence_fingerprint)
    .bind(args.effective_at_utc)
    .bind(args.expires_at_utc)
    .bind(&args.initiated_by)
    .bind(&args.reason)
    .bind(args.created_at_utc)
    .execute(pool)
    .await
    .context("insert_strategy_promotion_transition failed")?;

    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Atomic, serialized transition insert (concurrency-safe)
// ---------------------------------------------------------------------------

/// Outcome of [`insert_strategy_promotion_transition_serialized`].
#[derive(Debug, Clone)]
pub enum TransitionInsertOutcome {
    /// A new row was inserted, chained from `parent_transition_id`.
    Inserted(StrategyPromotionTransitionRecord),
    /// `args.transition_id` already existed in history — idempotent
    /// replay, no new row created. Carries the existing row exactly as
    /// originally recorded.
    Duplicate(StrategyPromotionTransitionRecord),
    /// The identity's actual current transition (re-read inside the
    /// advisory lock) does not match `args.parent_transition_id` — a
    /// concurrent transition already advanced this identity. `current`
    /// is the actual current record (`None` only if the identity somehow
    /// has no history at all despite a non-`None` expected parent, which
    /// itself would also be a conflict).
    Conflict {
        current: Option<StrategyPromotionTransitionRecord>,
    },
}

/// Deterministic bigint lock key for `pg_advisory_xact_lock`, derived from
/// the exact `(strategy_id, symbol, timeframe_secs)` identity so
/// concurrent transition attempts for the *same* identity serialize on
/// the same lock, while attempts for different identities never contend.
fn identity_lock_key(strategy_id: &str, symbol: &str, timeframe_secs: i64) -> String {
    format!("mqk-strategy-promotion-transition-lock.v1|{strategy_id}|{symbol}|{timeframe_secs}")
}

/// Atomically validate-and-insert one promotion transition for an exact
/// identity, serializing concurrent attempts so history can never branch.
///
/// Unlike [`insert_strategy_promotion_transition`] (a bare insert, kept
/// unchanged for direct non-concurrent test-fixture seeding), this
/// function is the only insert path the daemon's
/// `POST /api/v1/strategy/promotions/transition` route uses. It:
///
/// 1. opens one DB transaction;
/// 2. acquires a transaction-scoped PostgreSQL advisory lock
///    (`pg_advisory_xact_lock`, automatically released at commit or
///    rollback — never held past this call) keyed deterministically on
///    the exact identity;
/// 3. re-reads, *inside* that lock, whether `args.transition_id` already
///    exists in history — if so, returns `Duplicate` without touching
///    parent validation at all, so an exact replay is never
///    misclassified as a conflict even if the chain has since advanced
///    for an unrelated reason;
/// 4. otherwise re-reads the identity's actual current transition inside
///    the same lock (never trusts the caller's pre-lock read) and
///    compares its id against `args.parent_transition_id`;
/// 5. on a mismatch, rolls back and returns `Conflict` with the actual
///    current record, so the caller can report a stable
///    `transition_conflict` reason instead of silently reordering,
///    merging, or overwriting;
/// 6. on a match, inserts the new row (chained via `parent_transition_id`)
///    and commits.
pub async fn insert_strategy_promotion_transition_serialized(
    pool: &PgPool,
    args: &InsertStrategyPromotionTransitionArgs,
) -> Result<TransitionInsertOutcome> {
    if args.strategy_id.trim().is_empty() {
        anyhow::bail!(
            "insert_strategy_promotion_transition_serialized: strategy_id must not be empty"
        );
    }
    if args.symbol.trim().is_empty() {
        anyhow::bail!("insert_strategy_promotion_transition_serialized: symbol must not be empty");
    }
    if args.symbol.trim() == "*" {
        anyhow::bail!(
            "insert_strategy_promotion_transition_serialized: wildcard symbol '*' is forbidden"
        );
    }
    if args.timeframe_secs <= 0 {
        anyhow::bail!(
            "insert_strategy_promotion_transition_serialized: timeframe_secs must be positive"
        );
    }
    if args.initiated_by.trim().is_empty() {
        anyhow::bail!(
            "insert_strategy_promotion_transition_serialized: initiated_by must not be empty"
        );
    }
    if !is_known_promotion_state(&args.new_state) {
        anyhow::bail!(
            "insert_strategy_promotion_transition_serialized: unknown new_state '{}'",
            args.new_state
        );
    }

    let mut tx = pool
        .begin()
        .await
        .context("insert_strategy_promotion_transition_serialized: begin transaction failed")?;

    let lock_key = identity_lock_key(&args.strategy_id, &args.symbol, args.timeframe_secs);
    sqlx::query("select pg_advisory_xact_lock(hashtext($1)::bigint)")
        .bind(&lock_key)
        .execute(&mut *tx)
        .await
        .context("insert_strategy_promotion_transition_serialized: advisory lock failed")?;

    // Idempotency check, inside the lock: does this exact transition_id
    // already exist? If so, this is a replay — return the existing row
    // untouched, before any parent/current-state comparison runs at all.
    let existing_row = sqlx::query(&format!(
        "select {SELECT_COLUMNS} from sys_strategy_promotion_transitions where transition_id = $1"
    ))
    .bind(args.transition_id)
    .fetch_optional(&mut *tx)
    .await
    .context("insert_strategy_promotion_transition_serialized: idempotency check failed")?;
    if let Some(row) = existing_row {
        let record = row_to_record(row)?;
        tx.commit().await.context(
            "insert_strategy_promotion_transition_serialized: commit (duplicate) failed",
        )?;
        return Ok(TransitionInsertOutcome::Duplicate(record));
    }

    // Re-read the identity's actual current transition inside the lock —
    // this is the authoritative check, never the caller's pre-lock read.
    let current_row = sqlx::query(&format!(
        r#"
        select {SELECT_COLUMNS}
        from sys_strategy_promotion_transitions
        where strategy_id = $1 and symbol = $2 and timeframe_secs = $3
        order by effective_at_utc desc, created_at_utc desc, transition_id desc
        limit 1
        "#
    ))
    .bind(&args.strategy_id)
    .bind(&args.symbol)
    .bind(args.timeframe_secs)
    .fetch_optional(&mut *tx)
    .await
    .context("insert_strategy_promotion_transition_serialized: current-state re-read failed")?;
    let current = current_row.map(row_to_record).transpose()?;
    let actual_parent = current.as_ref().map(|c| c.transition_id);

    if actual_parent != args.parent_transition_id {
        tx.rollback().await.context(
            "insert_strategy_promotion_transition_serialized: rollback (conflict) failed",
        )?;
        return Ok(TransitionInsertOutcome::Conflict { current });
    }

    sqlx::query(
        r#"
        insert into sys_strategy_promotion_transitions (
            transition_id, strategy_id, symbol, timeframe_secs,
            config_fingerprint, config_identity_status,
            previous_state, new_state,
            parent_transition_id, evidence_transition_id,
            evidence_review_id, evidence_scanner_scan_id, evidence_git_hash,
            evidence_artifact_path, evidence_fingerprint,
            effective_at_utc, expires_at_utc, initiated_by, reason, created_at_utc
        )
        values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)
        "#,
    )
    .bind(args.transition_id)
    .bind(&args.strategy_id)
    .bind(&args.symbol)
    .bind(args.timeframe_secs)
    .bind(&args.config_fingerprint)
    .bind(&args.config_identity_status)
    .bind(&args.previous_state)
    .bind(&args.new_state)
    .bind(args.parent_transition_id)
    .bind(args.evidence_transition_id)
    .bind(&args.evidence_review_id)
    .bind(&args.evidence_scanner_scan_id)
    .bind(&args.evidence_git_hash)
    .bind(&args.evidence_artifact_path)
    .bind(&args.evidence_fingerprint)
    .bind(args.effective_at_utc)
    .bind(args.expires_at_utc)
    .bind(&args.initiated_by)
    .bind(&args.reason)
    .bind(args.created_at_utc)
    .execute(&mut *tx)
    .await
    .context("insert_strategy_promotion_transition_serialized: insert failed")?;

    tx.commit()
        .await
        .context("insert_strategy_promotion_transition_serialized: commit (inserted) failed")?;

    Ok(TransitionInsertOutcome::Inserted(
        StrategyPromotionTransitionRecord {
            transition_id: args.transition_id,
            strategy_id: args.strategy_id.clone(),
            symbol: args.symbol.clone(),
            timeframe_secs: args.timeframe_secs,
            config_fingerprint: args.config_fingerprint.clone(),
            config_identity_status: args.config_identity_status.clone(),
            previous_state: args.previous_state.clone(),
            new_state: args.new_state.clone(),
            parent_transition_id: args.parent_transition_id,
            evidence_transition_id: args.evidence_transition_id,
            evidence_review_id: args.evidence_review_id.clone(),
            evidence_scanner_scan_id: args.evidence_scanner_scan_id.clone(),
            evidence_git_hash: args.evidence_git_hash.clone(),
            evidence_artifact_path: args.evidence_artifact_path.clone(),
            evidence_fingerprint: args.evidence_fingerprint.clone(),
            effective_at_utc: args.effective_at_utc,
            expires_at_utc: args.expires_at_utc,
            initiated_by: args.initiated_by.clone(),
            reason: args.reason.clone(),
            created_at_utc: args.created_at_utc,
        },
    ))
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

/// Fetch the current (latest) promotion transition for an exact identity.
///
/// Returns `Ok(None)` when no transition has ever been recorded for this
/// identity — authoritative "no record", never a synthesized/default row.
/// Ordering is deterministic: `effective_at_utc desc, created_at_utc desc,
/// transition_id desc`.
pub async fn fetch_current_promotion_state(
    pool: &PgPool,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
) -> Result<Option<StrategyPromotionTransitionRecord>> {
    let query = format!(
        r#"
        select {SELECT_COLUMNS}
        from sys_strategy_promotion_transitions
        where strategy_id = $1 and symbol = $2 and timeframe_secs = $3
        order by effective_at_utc desc, created_at_utc desc, transition_id desc
        limit 1
        "#
    );
    let row = sqlx::query(&query)
        .bind(strategy_id)
        .bind(symbol)
        .bind(timeframe_secs)
        .fetch_optional(pool)
        .await
        .context("fetch_current_promotion_state failed")?;

    row.map(row_to_record).transpose().map_err(Into::into)
}

/// Fetch the full transition history for an exact identity, newest first.
///
/// An empty `Vec` is authoritative: it means no transition has ever been
/// recorded for this identity, not that history is unavailable.
pub async fn fetch_promotion_history(
    pool: &PgPool,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    limit: i64,
) -> Result<Vec<StrategyPromotionTransitionRecord>> {
    let query = format!(
        r#"
        select {SELECT_COLUMNS}
        from sys_strategy_promotion_transitions
        where strategy_id = $1 and symbol = $2 and timeframe_secs = $3
        order by effective_at_utc desc, created_at_utc desc, transition_id desc
        limit $4
        "#
    );
    let rows = sqlx::query(&query)
        .bind(strategy_id)
        .bind(symbol)
        .bind(timeframe_secs)
        .bind(limit)
        .fetch_all(pool)
        .await
        .context("fetch_promotion_history failed")?;

    rows.into_iter()
        .map(row_to_record)
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// Fetch the current (latest) promotion transition for every known
/// identity, ordered by `(strategy_id, symbol, timeframe_secs)`.
///
/// An empty `Vec` is authoritative: it means no identity has ever had a
/// transition recorded, not that the registry is unavailable.
pub async fn fetch_all_current_promotions(
    pool: &PgPool,
) -> Result<Vec<StrategyPromotionTransitionRecord>> {
    let query = format!(
        r#"
        select distinct on (strategy_id, symbol, timeframe_secs)
            {SELECT_COLUMNS}
        from sys_strategy_promotion_transitions
        order by strategy_id, symbol, timeframe_secs,
                 effective_at_utc desc, created_at_utc desc, transition_id desc
        "#
    );
    let rows = sqlx::query(&query)
        .fetch_all(pool)
        .await
        .context("fetch_all_current_promotions failed")?;

    rows.into_iter()
        .map(row_to_record)
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// Tradability evaluation (pure)
// ---------------------------------------------------------------------------

/// Stable, machine-readable reason codes for a promotion tradability
/// decision. Every read/runtime surface that answers "is this identity
/// paper-tradable right now" uses one of these — see
/// `docs/specs/strategy_promotion_registry_01a_current_truth_and_contract.md`
/// section 13 for the exact `db_unavailable`/`query_failed` split, which
/// this pure evaluator does not produce itself (those require knowledge of
/// DB availability that only the caller has — see `mqk-daemon`'s
/// `promotion_gate` module).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromotionReasonCode {
    PromotionActive,
    PromotionMissing,
    PromotionShadowOnly,
    PromotionNotActive,
    PromotionDemoted,
    PromotionRetired,
    PromotionRejected,
    PromotionExpired,
    PromotionIdentityMismatch,
    PromotionTimeframeUnknown,
    PromotionConfigMismatch,
    PromotionDbUnavailable,
    PromotionQueryFailed,
    /// STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01 (Phase C): the
    /// current `active_paper` record's `effective_at_utc` is later than
    /// the evaluation time — the transition is recorded but has not yet
    /// taken effect.
    PromotionNotYetEffective,
    /// Defined for forward documentation/consistency only. No code path in
    /// this patch ever produces this variant — this patch adds no
    /// live-authorization check of any kind. A paper promotion state must
    /// never authorize a LIVE run or live-routing path.
    PromotionLiveNotAuthorized,
}

impl PromotionReasonCode {
    pub fn code(&self) -> &'static str {
        match self {
            Self::PromotionActive => "promotion_active",
            Self::PromotionMissing => "promotion_missing",
            Self::PromotionShadowOnly => "promotion_shadow_only",
            Self::PromotionNotActive => "promotion_not_active",
            Self::PromotionDemoted => "promotion_demoted",
            Self::PromotionRetired => "promotion_retired",
            Self::PromotionRejected => "promotion_rejected",
            Self::PromotionExpired => "promotion_expired",
            Self::PromotionIdentityMismatch => "promotion_identity_mismatch",
            Self::PromotionTimeframeUnknown => "promotion_timeframe_unknown",
            Self::PromotionConfigMismatch => "promotion_config_mismatch",
            Self::PromotionDbUnavailable => "promotion_db_unavailable",
            Self::PromotionQueryFailed => "promotion_query_failed",
            Self::PromotionNotYetEffective => "promotion_not_yet_effective",
            Self::PromotionLiveNotAuthorized => "promotion_live_not_authorized",
        }
    }
}

/// Pure evaluation of paper tradability from an already-fetched current
/// promotion record. No DB/IO — callers (route handlers, the runtime
/// promotion gate) are responsible for DB-availability/query-failure
/// handling before calling this, and for identity/timeframe validation
/// before fetching `record` in the first place.
///
/// Returns `(paper_tradable, reason_code)`. Only `active_paper`, not yet
/// expired, and already effective (`effective_at_utc <= now_utc`) as of
/// `now_utc`, is `paper_tradable = true`. STRATEGY-PROMOTION-REGISTRY-
/// CLOSURE-REPAIR-01 (Phase C): a future-dated `effective_at_utc` is
/// checked here explicitly — the daemon route additionally rejects any
/// `effective_at_utc` submitted materially ahead of the daemon's own
/// clock at transition-creation time (see
/// `routes::strategy_promotions::EFFECTIVE_AT_CLOCK_SKEW_TOLERANCE`), so
/// this check is defense-in-depth against clock skew / replication lag,
/// not a general "schedule a future promotion" feature. Expiry uses
/// `<=`, not `<`: a transition expiring exactly at `now_utc` is expired.
pub fn evaluate_promotion_tradability(
    record: Option<&StrategyPromotionTransitionRecord>,
    now_utc: DateTime<Utc>,
) -> (bool, PromotionReasonCode) {
    let Some(record) = record else {
        return (false, PromotionReasonCode::PromotionMissing);
    };

    match record.new_state.as_str() {
        PROMOTION_STATE_ACTIVE_PAPER => {
            if record.effective_at_utc > now_utc {
                return (false, PromotionReasonCode::PromotionNotYetEffective);
            }
            if let Some(expires_at) = record.expires_at_utc {
                if expires_at <= now_utc {
                    return (false, PromotionReasonCode::PromotionExpired);
                }
            }
            (true, PromotionReasonCode::PromotionActive)
        }
        PROMOTION_STATE_SHADOW_APPROVED => (false, PromotionReasonCode::PromotionShadowOnly),
        PROMOTION_STATE_PAPER_APPROVED => (false, PromotionReasonCode::PromotionNotActive),
        PROMOTION_STATE_DEMOTED => (false, PromotionReasonCode::PromotionDemoted),
        PROMOTION_STATE_RETIRED => (false, PromotionReasonCode::PromotionRetired),
        PROMOTION_STATE_REJECTED => (false, PromotionReasonCode::PromotionRejected),
        // Unreachable given the DB CHECK constraint on new_state, but fail
        // closed rather than panic if it ever happens (e.g. a future schema
        // change without a corresponding code update).
        _ => (false, PromotionReasonCode::PromotionMissing),
    }
}

// ---------------------------------------------------------------------------
// Evidence lineage resolution
// ---------------------------------------------------------------------------

/// Fetch one transition row by its exact `transition_id`, regardless of
/// whether it is any identity's current row. Used to resolve evidence
/// lineage (see [`resolve_evidence_lineage`]) when the evidence-bearing
/// ancestor is not the row currently in hand.
pub async fn fetch_promotion_transition_by_id(
    pool: &PgPool,
    transition_id: Uuid,
) -> Result<Option<StrategyPromotionTransitionRecord>> {
    let query = format!(
        "select {SELECT_COLUMNS} from sys_strategy_promotion_transitions where transition_id = $1"
    );
    let row = sqlx::query(&query)
        .bind(transition_id)
        .fetch_optional(pool)
        .await
        .context("fetch_promotion_transition_by_id failed")?;
    row.map(row_to_record).transpose().map_err(Into::into)
}

/// Durably resolve the exact evidence-bearing transition that authorizes
/// `record`'s current state — STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01
/// (Phase D). If `record` is itself evidence-bearing (its own
/// `evidence_review_id` is set), returns it unchanged. Otherwise follows
/// `record.evidence_transition_id` (carried forward from the most recent
/// evidence-bearing transition in this identity's chain) and fetches that
/// row. Returns `Ok(None)` only if `record.evidence_transition_id` is
/// itself `None` (should not happen for any transition inserted via the
/// daemon route, since every legal chain's root transition is always
/// evidence-bearing) or the referenced row cannot be found — both
/// reported honestly, never silently substituted.
pub async fn resolve_evidence_lineage(
    pool: &PgPool,
    record: &StrategyPromotionTransitionRecord,
) -> Result<Option<StrategyPromotionTransitionRecord>> {
    if record.evidence_review_id.is_some() {
        return Ok(Some(record.clone()));
    }
    let Some(evidence_transition_id) = record.evidence_transition_id else {
        return Ok(None);
    };
    if evidence_transition_id == record.transition_id {
        return Ok(Some(record.clone()));
    }
    fetch_promotion_transition_by_id(pool, evidence_transition_id).await
}
