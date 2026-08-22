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
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-FINAL-FOUNDATION-PROVENANCE-AND-
// ARTIFACT-HARDENING: evidence_fingerprint_v2 format hardening (migration
// 0058 is append-only and carries no CHECK constraint of its own -- format
// is enforced at the application boundary instead).
// ---------------------------------------------------------------------------

/// `true` iff `s` is exactly 64 lowercase hex characters -- the only shape
/// `mqk-daemon::promotion_evidence_validation::compute_evidence_fingerprint_v2`
/// (a SHA-256 hex digest via the `hex` crate, which always encodes
/// lowercase) or any other legitimate caller ever produces for
/// `evidence_fingerprint_v2`.
pub fn is_valid_evidence_fingerprint_v2_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}

/// Defect 3: `true` iff `args` carries any evidence content field at all, or
/// self-references as its own evidence-bearing transition (`evidence_transition_id
/// == transition_id`) -- either signal is independently sufficient to
/// classify this insert as "fresh evidence" for
/// [`validate_fresh_evidence_bundle`]. Mirrors [`resolve_evidence_lineage`]'s
/// own authority (`evidence_review_id.is_some()` is checked first there,
/// before falling back to `evidence_transition_id == transition_id`) -- this
/// validator and that resolver must never disagree about what counts as
/// "this row carries its own evidence."
fn is_fresh_evidence_insert(args: &InsertStrategyPromotionTransitionArgs) -> bool {
    args.evidence_transition_id == Some(args.transition_id)
        || args.evidence_review_id.is_some()
        || args.evidence_scanner_scan_id.is_some()
        || args.evidence_git_hash.is_some()
        || args.evidence_artifact_path.is_some()
        || args.evidence_fingerprint.is_some()
        || args.evidence_fingerprint_v2.is_some()
}

/// Defect 3: a fresh evidence-bearing insert (see [`is_fresh_evidence_insert`])
/// must carry a complete, valid evidence bundle -- every evidence content
/// field present (never a partial subset) and both the legacy and v2
/// fingerprints exactly 64 lowercase hexadecimal characters. A transition
/// that is not fresh evidence (including one that carries
/// `evidence_transition_id` lineage pointing to an *earlier* transition,
/// without duplicating that transition's evidence columns onto itself) is
/// untouched by this check -- absent evidence always round-trips as `NULL`.
///
/// This closes the gap where [`insert_strategy_promotion_transition`]/
/// [`insert_strategy_promotion_transition_serialized`] previously only
/// validated `evidence_fingerprint_v2`'s *shape when present*, never its
/// *presence* -- a caller could insert a brand-new evidence-bearing
/// transition with a full legacy bundle and no v2 fingerprint at all,
/// silently reproducing the exact legacy shape Bundle 7's read-side
/// validator refuses to rank. Never weakened for test fixtures -- there is
/// no production function that bypasses this check; a genuine
/// pre-migration-0058 legacy row must be seeded via direct SQL in the test
/// itself, never through a public library seam.
fn validate_fresh_evidence_bundle(args: &InsertStrategyPromotionTransitionArgs) -> Result<()> {
    if !is_fresh_evidence_insert(args) {
        return Ok(());
    }
    // Pattern-match all six evidence content fields at once -- if any single
    // one is `None`, the match itself fails and falls through to the
    // bounded "all-present or all-absent" error below, never a panic. `fp`
    // and `fp2` are extracted directly from this same match, so there is no
    // separate `.expect()` step that could observe a stale/inconsistent
    // view of `args`.
    let (
        Some(_review_id),
        Some(_scanner_scan_id),
        Some(_git_hash),
        Some(_artifact_path),
        Some(fp),
        Some(fp2),
    ) = (
        args.evidence_review_id.as_deref(),
        args.evidence_scanner_scan_id.as_deref(),
        args.evidence_git_hash.as_deref(),
        args.evidence_artifact_path.as_deref(),
        args.evidence_fingerprint.as_deref(),
        args.evidence_fingerprint_v2.as_deref(),
    )
    else {
        anyhow::bail!(
            "fresh evidence bundle must be all-present or all-absent -- this transition carries \
             at least one evidence content field, so every evidence content field (review_id, \
             scanner_scan_id, git_hash, artifact_path, fingerprint, fingerprint_v2) must be \
             present"
        );
    };
    if !is_valid_evidence_fingerprint_v2_hex(fp) {
        anyhow::bail!(
            "fresh evidence_fingerprint must be exactly 64 lowercase hex characters when this \
             transition carries fresh evidence (got {} chars): {fp:?}",
            fp.chars().count()
        );
    }
    if !is_valid_evidence_fingerprint_v2_hex(fp2) {
        anyhow::bail!(
            "fresh evidence_fingerprint_v2 must be exactly 64 lowercase hex characters when this \
             transition carries fresh evidence (got {} chars): {fp2:?}",
            fp2.chars().count()
        );
    }
    Ok(())
}

/// Defect 3: `true` iff every field on `args` matches the corresponding
/// field on `existing` exactly -- the canonical payload comparison an exact
/// replay of `args.transition_id` must pass to be classified `Duplicate`
/// rather than `Conflict`. Any divergence (a different `evidence_fingerprint_v2`,
/// a different `new_state`, or any other field) means the same
/// `transition_id` was submitted for two different logical transitions --
/// never silently classified as an idempotent replay.
fn args_match_existing_record(
    args: &InsertStrategyPromotionTransitionArgs,
    existing: &StrategyPromotionTransitionRecord,
) -> bool {
    // Postgres `timestamptz` columns store microsecond precision -- an
    // in-memory `DateTime<Utc>` (e.g. from `Utc::now()`) can carry
    // nanosecond precision, so a raw `==` between the caller's pre-insert
    // value and the durable read-back value can spuriously disagree on an
    // exact replay. Compare at microsecond precision (the DB's own
    // precision) on both sides, mirroring the truncation Postgres itself
    // already performed on `existing`.
    args.strategy_id == existing.strategy_id
        && args.symbol == existing.symbol
        && args.timeframe_secs == existing.timeframe_secs
        && args.config_fingerprint == existing.config_fingerprint
        && args.config_identity_status == existing.config_identity_status
        && args.previous_state == existing.previous_state
        && args.new_state == existing.new_state
        && args.parent_transition_id == existing.parent_transition_id
        && args.evidence_transition_id == existing.evidence_transition_id
        && args.evidence_review_id == existing.evidence_review_id
        && args.evidence_scanner_scan_id == existing.evidence_scanner_scan_id
        && args.evidence_git_hash == existing.evidence_git_hash
        && args.evidence_artifact_path == existing.evidence_artifact_path
        && args.evidence_fingerprint == existing.evidence_fingerprint
        && args.evidence_fingerprint_v2 == existing.evidence_fingerprint_v2
        && args.effective_at_utc.timestamp_micros() == existing.effective_at_utc.timestamp_micros()
        && args.expires_at_utc.map(|t| t.timestamp_micros())
            == existing.expires_at_utc.map(|t| t.timestamp_micros())
        && args.initiated_by == existing.initiated_by
        && args.reason == existing.reason
        && args.created_at_utc.timestamp_micros() == existing.created_at_utc.timestamp_micros()
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
    /// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-FOUNDATION-AUTHORITY-CLOSURE-02
    /// Defect B: the versioned exact-score evidence fingerprint (see
    /// `mqk-daemon::promotion_evidence_validation::compute_evidence_fingerprint_v2`),
    /// hashed directly from the raw JSON score token -- never through the
    /// legacy `f64` round-trip. `None` for every row inserted before this
    /// column existed (legacy row) -- never backfilled from the mutable
    /// review-artifact filesystem at read time.
    pub evidence_fingerprint_v2: Option<String>,
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
    /// See [`StrategyPromotionTransitionRecord::evidence_fingerprint_v2`].
    pub evidence_fingerprint_v2: Option<String>,
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
        evidence_fingerprint_v2: r.try_get("evidence_fingerprint_v2")?,
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
    evidence_artifact_path, evidence_fingerprint, evidence_fingerprint_v2,
    effective_at_utc, expires_at_utc, initiated_by, reason, created_at_utc
"#;

// ---------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------

/// Insert a new promotion transition row.
///
/// Idempotent via `ON CONFLICT (transition_id) DO NOTHING`. Returns
/// `Ok(true)` when a new row was actually inserted, `Ok(false)` when the
/// `transition_id` already existed with an *exact*-matching payload
/// (verified via [`args_match_existing_record`] — a genuine replay, no-op,
/// not an error). Returns `Err` if `strategy_id`/`symbol` are blank or
/// `symbol == "*"`, if `initiated_by` is blank, if `timeframe_secs` is
/// not positive, or if `transition_id` already exists with a *divergent*
/// payload (a same-id collision — never silently collapsed into an
/// idempotent duplicate) — the first four are validated before any DB
/// contact, mirroring the DB `CHECK` constraints as defense in depth. Does
/// **not** validate transition legality — callers must call
/// [`is_legal_transition`] (using a `previous_state` freshly obtained from
/// [`fetch_current_promotion_state`]) before calling this function; the DB
/// `CHECK` constraint is a structural backstop against an application bug,
/// not the primary enforcement point.
///
/// Not concurrency-safe against another writer racing the same identity
/// (no advisory lock, no parent re-validation) — the daemon's
/// `POST /api/v1/strategy/promotions/transition` route always uses
/// [`insert_strategy_promotion_transition_serialized`] instead. This bare
/// form exists for direct, non-concurrent test-fixture seeding of a
/// *fresh-evidence-shaped* row only; a genuine pre-migration-0058 legacy
/// row (evidence-bearing, no `evidence_fingerprint_v2`) cannot be seeded
/// through any public function — construct it via direct SQL in the test
/// itself, clearly labeled as pre-0058 fixture seeding.
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
    validate_fresh_evidence_bundle(args).context("insert_strategy_promotion_transition")?;

    let result = sqlx::query(
        r#"
        insert into sys_strategy_promotion_transitions (
            transition_id, strategy_id, symbol, timeframe_secs,
            config_fingerprint, config_identity_status,
            previous_state, new_state,
            parent_transition_id, evidence_transition_id,
            evidence_review_id, evidence_scanner_scan_id, evidence_git_hash,
            evidence_artifact_path, evidence_fingerprint, evidence_fingerprint_v2,
            effective_at_utc, expires_at_utc, initiated_by, reason, created_at_utc
        )
        values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21)
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
    .bind(&args.evidence_fingerprint_v2)
    .bind(args.effective_at_utc)
    .bind(args.expires_at_utc)
    .bind(&args.initiated_by)
    .bind(&args.reason)
    .bind(args.created_at_utc)
    .execute(pool)
    .await
    .context("insert_strategy_promotion_transition failed")?;

    if result.rows_affected() > 0 {
        return Ok(true);
    }

    // `transition_id` already existed -- distinguish an exact-replay
    // duplicate (silently a no-op, matching this function's documented
    // idempotent contract) from a same-id divergent payload (never
    // silently collapsed into a duplicate -- refused as a collision).
    let existing_row = sqlx::query(&format!(
        "select {SELECT_COLUMNS} from sys_strategy_promotion_transitions where transition_id = $1"
    ))
    .bind(args.transition_id)
    .fetch_optional(pool)
    .await
    .context("insert_strategy_promotion_transition: post-conflict readback failed")?;
    let Some(row) = existing_row else {
        anyhow::bail!(
            "insert_strategy_promotion_transition: conflict reported but existing row not found \
             for transition_id {}",
            args.transition_id
        );
    };
    let record = row_to_record(row)?;
    if args_match_existing_record(args, &record) {
        Ok(false)
    } else {
        anyhow::bail!(
            "insert_strategy_promotion_transition: transition_id {} already exists with a \
             different payload -- refusing to silently collapse a same-id divergent payload \
             into an idempotent duplicate",
            args.transition_id
        );
    }
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
/// 6. on a match, inserts the new row (chained via `parent_transition_id`);
/// 7. if `evidence_lineage` is `Some`, writes it (Research/Backtest evidence
///    lineage, migration 0065) in the SAME transaction as step 6 -- see
///    [`PROMOTION-LINEAGE-ATOMICITY-01`](self) below;
/// 8. commits.
///
/// PROMOTION-LINEAGE-ATOMICITY-01: `evidence_lineage`, when `Some`, is
/// written durably atomically with the transition row itself -- either both
/// commit together or neither does (a lineage-write failure propagates as
/// `Err` before `tx.commit()` is ever reached, so the transaction rolls back
/// on drop and no transition is recorded). This replaced a prior best-effort
/// `UPDATE` run by the caller AFTER this function returned `Inserted`, which
/// could leave a successful shadow-approved transition with no durable
/// Research/Backtest evidence lineage if that follow-up write failed.
///
/// PROMOTION-EVIDENCE-LINEAGE-V3 LEGACY DUPLICATE RULE: a fresh insert (this
/// call created the row moments ago in this same transaction) always writes
/// the supplied lineage (`write_evidence_lineage_for_fresh_insert_in_tx`).
/// An idempotent replay (`Duplicate` -- the row already existed BEFORE this
/// call began) only ever VERIFIES lineage, never writes it
/// (`verify_evidence_lineage_for_duplicate_in_tx`): if that historical row
/// already carries lineage, the supplied `evidence_lineage` must match it
/// exactly (never silently replaced by a divergent value); if the
/// historical row's lineage is NULL (it predates complete lineage, or was
/// never evidence-bearing), the newly supplied lineage is REFUSED (`Err`)
/// -- a later duplicate request must never retroactively attach evidence to
/// a historical decision and claim that evidence authorized it. `Conflict`
/// never reaches lineage-writing code at all -- the transaction is rolled
/// back before any lineage write/verify is attempted.
pub async fn insert_strategy_promotion_transition_serialized(
    pool: &PgPool,
    args: &InsertStrategyPromotionTransitionArgs,
    evidence_lineage: Option<&PromotionEvidenceLineageV3>,
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
    validate_fresh_evidence_bundle(args)
        .context("insert_strategy_promotion_transition_serialized")?;

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
    // already exist? If so, compare the existing row against the submitted
    // canonical payload (Defect 3) -- an exact-replay is a `Duplicate`
    // (returned untouched, before any parent/current-state comparison runs
    // at all); a divergent payload sharing the same transition_id is a
    // `Conflict`, never silently classified as an idempotent replay. Either
    // way the original durable row is never modified here.
    let existing_row = sqlx::query(&format!(
        "select {SELECT_COLUMNS} from sys_strategy_promotion_transitions where transition_id = $1"
    ))
    .bind(args.transition_id)
    .fetch_optional(&mut *tx)
    .await
    .context("insert_strategy_promotion_transition_serialized: idempotency check failed")?;
    if let Some(row) = existing_row {
        let record = row_to_record(row)?;
        if !args_match_existing_record(args, &record) {
            tx.rollback().await.context(
                "insert_strategy_promotion_transition_serialized: rollback (collision) failed",
            )?;
            return Ok(TransitionInsertOutcome::Conflict {
                current: Some(record),
            });
        }
        if let Some(lineage) = evidence_lineage {
            verify_evidence_lineage_for_duplicate_in_tx(&mut tx, args.transition_id, lineage)
                .await?;
        }
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
            evidence_artifact_path, evidence_fingerprint, evidence_fingerprint_v2,
            effective_at_utc, expires_at_utc, initiated_by, reason, created_at_utc
        )
        values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21)
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
    .bind(&args.evidence_fingerprint_v2)
    .bind(args.effective_at_utc)
    .bind(args.expires_at_utc)
    .bind(&args.initiated_by)
    .bind(&args.reason)
    .bind(args.created_at_utc)
    .execute(&mut *tx)
    .await
    .context("insert_strategy_promotion_transition_serialized: insert failed")?;

    if let Some(lineage) = evidence_lineage {
        write_evidence_lineage_for_fresh_insert_in_tx(&mut tx, args.transition_id, lineage)
            .await?;
    }

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
            evidence_fingerprint_v2: args.evidence_fingerprint_v2.clone(),
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

// ---------------------------------------------------------------------------
// PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE / PROMOTION-LINEAGE-
// ATOMICITY-01: durable evidence lineage (migration 0065)
// ---------------------------------------------------------------------------
//
// A deliberately SEPARATE column group, not new fields on
// `InsertStrategyPromotionTransitionArgs`/`StrategyPromotionTransitionRecord`
// -- those two types are constructed by struct literal in over a dozen call
// sites across `mqk-daemon`'s test suite for purposes entirely unrelated to
// Research/Backtest evidence lineage (fleet/decision/outbox fixtures).
// Adding required fields there would force an unrelated, mechanical edit to
// every one of those call sites for a "narrow" migration. Instead,
// `insert_strategy_promotion_transition_serialized` takes evidence lineage
// as its own optional parameter and writes/verifies it (via
// `write_evidence_lineage_for_fresh_insert_in_tx`/
// `verify_evidence_lineage_for_duplicate_in_tx`, below) inside the SAME
// transaction as the transition row -- PROMOTION-LINEAGE-ATOMICITY-01 replaced an earlier
// best-effort post-commit `UPDATE` (which could leave a successful
// shadow-approved transition with no durable evidence lineage if that
// follow-up write failed) with this atomic write.

/// Durable Research/Backtest evidence lineage for one promotion transition.
/// `None` in every field for a transition that predates migration 0065/0066,
/// or one that was not evidence-bearing.
///
/// PROMOTION-EVIDENCE-LINEAGE-V3: extends the original five (migration
/// 0065) identity-pointer/judged-statistic fields with six more, each
/// reusing an EXISTING accepted audit hash already computed and verified
/// elsewhere in the evidence chain (see migration 0066's own docs) --
/// except `promotion_policy_fingerprint`, the one genuinely new fingerprint
/// (`PromotionConfig` was never durably hashed anywhere before).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PromotionEvidenceLineageV3 {
    pub research_trial_id: Option<String>,
    pub research_economic_eval_id: Option<String>,
    pub research_deflated_sharpe_ratio: Option<f64>,
    pub research_probability_backtest_overfitting: Option<f64>,
    pub backtest_run_id: Option<Uuid>,
    pub research_judge_artifact_sha256: Option<String>,
    pub stress_protocol_version: Option<String>,
    pub stress_artifact_sha256: Option<String>,
    pub robustness_protocol_version: Option<String>,
    pub finalized_robustness_artifact_sha256: Option<String>,
    pub promotion_policy_fingerprint: Option<String>,
}

/// Read the durable evidence lineage columns for `transition_id` INSIDE
/// `tx`. `Ok(None)` iff the row itself does not exist.
async fn read_evidence_lineage_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    transition_id: Uuid,
) -> Result<Option<PromotionEvidenceLineageV3>> {
    let row = sqlx::query(
        r#"
        select research_trial_id, research_economic_eval_id,
               research_deflated_sharpe_ratio, research_probability_backtest_overfitting,
               backtest_run_id, research_judge_artifact_sha256, stress_protocol_version,
               stress_artifact_sha256, robustness_protocol_version,
               finalized_robustness_artifact_sha256, promotion_policy_fingerprint
        from sys_strategy_promotion_transitions
        where transition_id = $1
        "#,
    )
    .bind(transition_id)
    .fetch_optional(&mut **tx)
    .await
    .context("read_evidence_lineage_in_tx: read failed")?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(row_to_lineage(&row)?))
}

fn row_to_lineage(row: &sqlx::postgres::PgRow) -> Result<PromotionEvidenceLineageV3> {
    Ok(PromotionEvidenceLineageV3 {
        research_trial_id: row.try_get("research_trial_id")?,
        research_economic_eval_id: row.try_get("research_economic_eval_id")?,
        research_deflated_sharpe_ratio: row.try_get("research_deflated_sharpe_ratio")?,
        research_probability_backtest_overfitting: row
            .try_get("research_probability_backtest_overfitting")?,
        backtest_run_id: row.try_get("backtest_run_id")?,
        research_judge_artifact_sha256: row.try_get("research_judge_artifact_sha256")?,
        stress_protocol_version: row.try_get("stress_protocol_version")?,
        stress_artifact_sha256: row.try_get("stress_artifact_sha256")?,
        robustness_protocol_version: row.try_get("robustness_protocol_version")?,
        finalized_robustness_artifact_sha256: row
            .try_get("finalized_robustness_artifact_sha256")?,
        promotion_policy_fingerprint: row.try_get("promotion_policy_fingerprint")?,
    })
}

async fn write_evidence_lineage_columns_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    transition_id: Uuid,
    lineage: &PromotionEvidenceLineageV3,
) -> Result<()> {
    sqlx::query(
        r#"
        update sys_strategy_promotion_transitions
        set research_trial_id = $2,
            research_economic_eval_id = $3,
            research_deflated_sharpe_ratio = $4,
            research_probability_backtest_overfitting = $5,
            backtest_run_id = $6,
            research_judge_artifact_sha256 = $7,
            stress_protocol_version = $8,
            stress_artifact_sha256 = $9,
            robustness_protocol_version = $10,
            finalized_robustness_artifact_sha256 = $11,
            promotion_policy_fingerprint = $12
        where transition_id = $1
        "#,
    )
    .bind(transition_id)
    .bind(lineage.research_trial_id.clone())
    .bind(lineage.research_economic_eval_id.clone())
    .bind(lineage.research_deflated_sharpe_ratio)
    .bind(lineage.research_probability_backtest_overfitting)
    .bind(lineage.backtest_run_id)
    .bind(lineage.research_judge_artifact_sha256.clone())
    .bind(lineage.stress_protocol_version.clone())
    .bind(lineage.stress_artifact_sha256.clone())
    .bind(lineage.robustness_protocol_version.clone())
    .bind(lineage.finalized_robustness_artifact_sha256.clone())
    .bind(lineage.promotion_policy_fingerprint.clone())
    .execute(&mut **tx)
    .await
    .context("write_evidence_lineage_columns_in_tx: write failed")?;
    Ok(())
}

/// Write `lineage` for a row THIS SAME call just inserted, INSIDE `tx` --
/// the caller (`insert_strategy_promotion_transition_serialized`'s
/// fresh-insert branch) guarantees the row was created moments ago in this
/// same transaction and therefore has NULL lineage. Defensively re-reads
/// and asserts that guarantee rather than trusting it blindly: if it does
/// not hold (a caller-contract violation), this fails closed (`Err`)
/// instead of silently overwriting existing lineage.
async fn write_evidence_lineage_for_fresh_insert_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    transition_id: Uuid,
    lineage: &PromotionEvidenceLineageV3,
) -> Result<()> {
    let existing = read_evidence_lineage_in_tx(tx, transition_id).await?.context(
        "write_evidence_lineage_for_fresh_insert_in_tx: row must already exist (just inserted)",
    )?;
    if existing.research_trial_id.is_some() {
        anyhow::bail!(
            "write_evidence_lineage_for_fresh_insert_in_tx: transition_id {transition_id} \
             already has recorded lineage -- this function must only be called immediately \
             after inserting a brand-new row"
        );
    }
    write_evidence_lineage_columns_in_tx(tx, transition_id, lineage).await
}

/// Verify (NEVER write) `lineage` for a `transition_id` that was found to
/// ALREADY EXIST before this call began -- i.e. a genuine duplicate/replay
/// of a previously-committed transition, INSIDE `tx`.
///
/// PROMOTION-EVIDENCE-LINEAGE-V3 LEGACY DUPLICATE RULE: if the historical
/// row's lineage is NULL (it predates complete lineage, or was never
/// evidence-bearing), this refuses to retroactively attach the newly
/// supplied lineage and claim that evidence authorized the historical
/// decision -- `Err`, never a silent backfill. If the historical row
/// already carries lineage, `lineage` must match it exactly (the original
/// idempotent-replay behavior, unchanged).
async fn verify_evidence_lineage_for_duplicate_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    transition_id: Uuid,
    lineage: &PromotionEvidenceLineageV3,
) -> Result<()> {
    let existing = read_evidence_lineage_in_tx(tx, transition_id)
        .await?
        .context("verify_evidence_lineage_for_duplicate_in_tx: row must already exist")?;
    if existing.research_trial_id.is_none() {
        anyhow::bail!(
            "verify_evidence_lineage_for_duplicate_in_tx: transition_id {transition_id} is a \
             duplicate of a historical transition with NO recorded evidence lineage -- refusing \
             to retroactively attach newly supplied lineage; the historical decision's \
             missing-lineage truth must be preserved (PROMOTION-EVIDENCE-LINEAGE-V3 legacy \
             duplicate rule)"
        );
    }
    if existing != *lineage {
        anyhow::bail!(
            "verify_evidence_lineage_for_duplicate_in_tx: transition_id {transition_id} already \
             has recorded evidence lineage that disagrees with the newly-supplied lineage -- \
             refusing to overwrite authoritative lineage"
        );
    }
    Ok(()) // idempotent replay -- already correct, nothing to write
}

/// Read back the durable evidence lineage for one transition. `Ok(None)`
/// means `transition_id` does not exist at all -- distinct from
/// `Ok(Some(PromotionEvidenceLineageV3 { .. all None .. }))`, which means
/// the row exists but has no recorded lineage (pre-0065/0066 or
/// non-evidence-bearing transition).
pub async fn fetch_promotion_transition_lineage_v3(
    pool: &PgPool,
    transition_id: Uuid,
) -> Result<Option<PromotionEvidenceLineageV3>> {
    let row = sqlx::query(
        r#"
        select research_trial_id, research_economic_eval_id,
               research_deflated_sharpe_ratio, research_probability_backtest_overfitting,
               backtest_run_id, research_judge_artifact_sha256, stress_protocol_version,
               stress_artifact_sha256, robustness_protocol_version,
               finalized_robustness_artifact_sha256, promotion_policy_fingerprint
        from sys_strategy_promotion_transitions
        where transition_id = $1
        "#,
    )
    .bind(transition_id)
    .fetch_optional(pool)
    .await
    .context("fetch_promotion_transition_lineage_v3 failed")?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(row_to_lineage(&row)?))
}

#[cfg(test)]
mod fingerprint_v2_shape_tests {
    use super::*;

    #[test]
    fn sixty_four_lowercase_hex_chars_is_valid() {
        let v = "a".repeat(64);
        assert!(is_valid_evidence_fingerprint_v2_hex(&v));
        let mixed_digits = "0123456789abcdef".repeat(4);
        assert_eq!(mixed_digits.len(), 64);
        assert!(is_valid_evidence_fingerprint_v2_hex(&mixed_digits));
    }

    #[test]
    fn wrong_length_is_invalid() {
        assert!(!is_valid_evidence_fingerprint_v2_hex(&"a".repeat(63)));
        assert!(!is_valid_evidence_fingerprint_v2_hex(&"a".repeat(65)));
        assert!(!is_valid_evidence_fingerprint_v2_hex(""));
    }

    #[test]
    fn uppercase_hex_is_invalid() {
        let v = "A".repeat(64);
        assert!(!is_valid_evidence_fingerprint_v2_hex(&v));
    }

    #[test]
    fn non_hex_characters_are_invalid() {
        let mut v = "a".repeat(63);
        v.push('g');
        assert!(!is_valid_evidence_fingerprint_v2_hex(&v));
    }

    fn no_evidence_args() -> InsertStrategyPromotionTransitionArgs {
        InsertStrategyPromotionTransitionArgs {
            transition_id: Uuid::nil(),
            strategy_id: "swing_momentum".to_string(),
            symbol: "AAPL".to_string(),
            timeframe_secs: 86400,
            config_fingerprint: None,
            config_identity_status: "unavailable_in_current_runtime".to_string(),
            previous_state: Some(PROMOTION_STATE_PAPER_APPROVED.to_string()),
            new_state: PROMOTION_STATE_ACTIVE_PAPER.to_string(),
            parent_transition_id: None,
            evidence_transition_id: None,
            evidence_review_id: None,
            evidence_scanner_scan_id: None,
            evidence_git_hash: None,
            evidence_artifact_path: None,
            evidence_fingerprint: None,
            evidence_fingerprint_v2: None,
            effective_at_utc: DateTime::<Utc>::MIN_UTC,
            expires_at_utc: None,
            initiated_by: "test-operator".to_string(),
            reason: "unit test".to_string(),
            created_at_utc: DateTime::<Utc>::MIN_UTC,
        }
    }

    fn fresh_evidence_args() -> InsertStrategyPromotionTransitionArgs {
        let mut a = no_evidence_args();
        a.evidence_transition_id = Some(a.transition_id);
        a.evidence_review_id = Some("review-1".to_string());
        a.evidence_scanner_scan_id = Some("scan-1".to_string());
        a.evidence_git_hash = Some("deadbeef".to_string());
        a.evidence_artifact_path = Some("exports/strategy_reviews/x".to_string());
        a.evidence_fingerprint = Some("a".repeat(64));
        a.evidence_fingerprint_v2 = Some("b".repeat(64));
        a
    }

    #[test]
    fn absent_v2_always_passes_shape_validation() {
        assert!(validate_fresh_evidence_bundle(&no_evidence_args()).is_ok());
    }

    #[test]
    fn present_valid_v2_passes_shape_validation() {
        assert!(validate_fresh_evidence_bundle(&fresh_evidence_args()).is_ok());
    }

    #[test]
    fn present_malformed_v2_fails_shape_validation() {
        let mut a = fresh_evidence_args();
        a.evidence_fingerprint_v2 = Some("not-a-hex-fingerprint".to_string());
        let err = validate_fresh_evidence_bundle(&a).expect_err("must be refused");
        assert!(err.to_string().contains("64 lowercase hex"), "got: {err}");
    }

    // ── Defect 3: fresh evidence bundle structural rules ──────────────────

    #[test]
    fn lineage_only_transition_with_no_evidence_content_passes() {
        // Non-evidence transition (e.g. paper_approved -> active_paper)
        // carrying no evidence_transition_id at all.
        assert!(validate_fresh_evidence_bundle(&no_evidence_args()).is_ok());
    }

    #[test]
    fn lineage_only_transition_pointing_elsewhere_passes_without_content() {
        // Carries evidence_transition_id lineage to an *earlier* transition
        // without duplicating that transition's evidence columns onto
        // itself -- explicitly allowed.
        let mut a = no_evidence_args();
        a.evidence_transition_id = Some(Uuid::new_v5(&Uuid::NAMESPACE_URL, b"earlier-transition"));
        assert!(validate_fresh_evidence_bundle(&a).is_ok());
    }

    #[test]
    fn legacy_fingerprint_without_v2_refuses() {
        // Defect 3, test 1.
        let mut a = fresh_evidence_args();
        a.evidence_fingerprint_v2 = None;
        let err = validate_fresh_evidence_bundle(&a).expect_err("must be refused");
        assert!(
            err.to_string().contains("all-present or all-absent"),
            "got: {err}"
        );
    }

    #[test]
    fn v2_without_legacy_refuses() {
        // Defect 3, test 2.
        let mut a = fresh_evidence_args();
        a.evidence_fingerprint = None;
        let err = validate_fresh_evidence_bundle(&a).expect_err("must be refused");
        assert!(
            err.to_string().contains("all-present or all-absent"),
            "got: {err}"
        );
    }

    #[test]
    fn partial_evidence_bundle_refuses() {
        // Defect 3, test 3: only review_id present, everything else absent.
        let mut a = no_evidence_args();
        a.evidence_review_id = Some("review-1".to_string());
        let err = validate_fresh_evidence_bundle(&a).expect_err("must be refused");
        assert!(
            err.to_string().contains("all-present or all-absent"),
            "got: {err}"
        );
    }

    #[test]
    fn lineage_only_non_evidence_transition_passes() {
        // Defect 3, test 4.
        let mut a = no_evidence_args();
        a.evidence_transition_id = Some(Uuid::new_v5(&Uuid::NAMESPACE_URL, b"root"));
        assert!(validate_fresh_evidence_bundle(&a).is_ok());
    }

    #[test]
    fn valid_fresh_evidence_bundle_passes() {
        // Defect 3, test 6 (structural half -- DB round-trip covered by the
        // integration scenario test).
        assert!(validate_fresh_evidence_bundle(&fresh_evidence_args()).is_ok());
    }

    #[test]
    fn args_match_existing_record_true_for_identical_payload() {
        let a = fresh_evidence_args();
        let record = StrategyPromotionTransitionRecord {
            transition_id: a.transition_id,
            strategy_id: a.strategy_id.clone(),
            symbol: a.symbol.clone(),
            timeframe_secs: a.timeframe_secs,
            config_fingerprint: a.config_fingerprint.clone(),
            config_identity_status: a.config_identity_status.clone(),
            previous_state: a.previous_state.clone(),
            new_state: a.new_state.clone(),
            parent_transition_id: a.parent_transition_id,
            evidence_transition_id: a.evidence_transition_id,
            evidence_review_id: a.evidence_review_id.clone(),
            evidence_scanner_scan_id: a.evidence_scanner_scan_id.clone(),
            evidence_git_hash: a.evidence_git_hash.clone(),
            evidence_artifact_path: a.evidence_artifact_path.clone(),
            evidence_fingerprint: a.evidence_fingerprint.clone(),
            evidence_fingerprint_v2: a.evidence_fingerprint_v2.clone(),
            effective_at_utc: a.effective_at_utc,
            expires_at_utc: a.expires_at_utc,
            initiated_by: a.initiated_by.clone(),
            reason: a.reason.clone(),
            created_at_utc: a.created_at_utc,
        };
        assert!(args_match_existing_record(&a, &record));
    }

    #[test]
    fn args_match_existing_record_false_when_v2_diverges() {
        // Defect 3, test 7: same transition_id, different v2 -- a
        // collision, never a Duplicate.
        let a = fresh_evidence_args();
        let mut record_args = a.clone();
        record_args.evidence_fingerprint_v2 = Some("c".repeat(64));
        let record = StrategyPromotionTransitionRecord {
            transition_id: record_args.transition_id,
            strategy_id: record_args.strategy_id.clone(),
            symbol: record_args.symbol.clone(),
            timeframe_secs: record_args.timeframe_secs,
            config_fingerprint: record_args.config_fingerprint.clone(),
            config_identity_status: record_args.config_identity_status.clone(),
            previous_state: record_args.previous_state.clone(),
            new_state: record_args.new_state.clone(),
            parent_transition_id: record_args.parent_transition_id,
            evidence_transition_id: record_args.evidence_transition_id,
            evidence_review_id: record_args.evidence_review_id.clone(),
            evidence_scanner_scan_id: record_args.evidence_scanner_scan_id.clone(),
            evidence_git_hash: record_args.evidence_git_hash.clone(),
            evidence_artifact_path: record_args.evidence_artifact_path.clone(),
            evidence_fingerprint: record_args.evidence_fingerprint.clone(),
            evidence_fingerprint_v2: record_args.evidence_fingerprint_v2.clone(),
            effective_at_utc: record_args.effective_at_utc,
            expires_at_utc: record_args.expires_at_utc,
            initiated_by: record_args.initiated_by.clone(),
            reason: record_args.reason.clone(),
            created_at_utc: record_args.created_at_utc,
        };
        assert!(!args_match_existing_record(&a, &record));
    }
}
