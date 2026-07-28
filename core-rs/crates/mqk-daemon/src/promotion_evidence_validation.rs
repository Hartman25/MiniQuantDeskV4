//! Shared promotion/review-artifact evidence validation.
//!
//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 3: this module is the single
//! implementation of "root-bound and content-validate a review artifact
//! directory against an exact `(strategy_id, symbol, timeframe_secs)`
//! identity, re-deriving the SHA-256 fingerprint rather than trusting a
//! caller's claim" — used by both:
//!
//! - `routes::strategy_promotions::strategy_promotion_transition` (the
//!   operator-facing POST route, via [`validate_paper_candidate_evidence`])
//!   — moved here verbatim from that module (STRATEGY-PROMOTION-REGISTRY-01C)
//!   with zero behavior change: same canonicalize+root-prefix pattern, same
//!   error message strings (the route surfaces them directly as
//!   `blockers`), same SHA-256 fingerprint derivation.
//! - Bundle 7's dynamic-selection plan construction/read validation, via
//!   [`validate_active_paper_candidate`] — a new read-side wrapper that
//!   additionally fetches the identity's current promotion state, checks
//!   `active_paper`/effectivity/expiry, resolves evidence lineage, and
//!   compares the freshly-recomputed fingerprint against the *durable*
//!   `evidence_fingerprint` column byte-for-byte (never trusting it).
//!
//! Both callers share the exact same artifact-reading/fingerprint logic —
//! there is no second, weaker reimplementation.

use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::state::AppState;
use mqk_db::{
    fetch_current_promotion_state, resolve_evidence_lineage, scanner_timeframe_label_to_secs,
    PROMOTION_STATE_ACTIVE_PAPER,
};

// ---------------------------------------------------------------------------
// Score/rank canonicalization
// ---------------------------------------------------------------------------

/// Scaled-integer representation for `scanner_score` -- deliberately its own
/// constant, independent of Bundle 5's `RUNTIME_OPPORTUNITY_SCALE`
/// (`mqk-db::runtime_opportunity_allocation::scale_to_micros`): Bundle 7
/// must not couple its score representation to Bundle 5's, even though both
/// happen to use 1e6 today. Matches `mqk_portfolio::MICROS_SCALE`.
pub const CANDIDATE_SCORE_SCALE: f64 = 1_000_000.0;

/// Convert a raw `scanner_score` to a decimal-exact scaled-integer (micros)
/// representation. Returns `None` for non-finite input (`NaN`/`±inf`) or a
/// magnitude that would overflow `i64` once scaled -- never silently
/// saturates or clamps, since that would fabricate a score the artifact
/// never actually contained.
pub fn canonical_score_to_micros(value: f64) -> Option<i64> {
    if !value.is_finite() {
        return None;
    }
    let scaled = value * CANDIDATE_SCORE_SCALE;
    if !scaled.is_finite() || scaled < i64::MIN as f64 || scaled > i64::MAX as f64 {
        return None;
    }
    Some(scaled.round() as i64)
}

// ---------------------------------------------------------------------------
// Write-path: root-bound, content-validated artifact read (moved verbatim
// from routes::strategy_promotions, STRATEGY-PROMOTION-REGISTRY-01C).
// ---------------------------------------------------------------------------

/// Evidence independently read and validated from a review artifact
/// directory, ready to attach to a transition insert (write path) or to
/// compare against durable evidence (read path).
#[derive(Debug, Clone)]
pub struct ValidatedEvidence {
    pub review_id: String,
    pub scanner_scan_id: String,
    pub git_hash: String,
    pub artifact_path: String,
    pub fingerprint: String,
    /// `None` when the matched row's raw `scanner_score` is absent from the
    /// artifact -- never defaulted to `0.0` or any other sentinel.
    pub scanner_score: Option<f64>,
    /// `None` when the matched row's raw `scanner_rank` is absent from the
    /// artifact -- never defaulted.
    pub scanner_rank: Option<usize>,
}

/// Root-bounded, content-validated read of a review artifact directory,
/// mirroring the exact canonicalize+root-prefix pattern used by
/// `GET /api/v1/strategy-scans/review-artifact`
/// (`routes/strategy_scans.rs`). Never trusts caller-supplied claims about
/// the evidence — always re-derives the fingerprint from the matched
/// decision row's own serialized content.
pub fn validate_paper_candidate_evidence(
    st: &AppState,
    review_dir: &str,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
) -> Result<ValidatedEvidence, String> {
    let requested = review_dir.trim();
    if requested.is_empty() {
        return Err("review_dir is required for this transition".to_string());
    }

    let candidate_path = Path::new(requested);
    let root = Path::new(&st.strategy_review_artifact_root);

    let candidate_canon = std::fs::canonicalize(candidate_path)
        .map_err(|_| format!("review_dir not found: {requested}"))?;
    let root_canon = std::fs::canonicalize(root).map_err(|_| {
        format!(
            "configured review artifact root is unavailable: {}",
            root.display()
        )
    })?;
    if !candidate_canon.starts_with(&root_canon) {
        return Err(
            "review_dir does not resolve inside the configured review artifact root".to_string(),
        );
    }

    let manifest_path = candidate_canon.join("manifest.json");
    let decisions_path = candidate_canon.join("review_decisions.json");
    for p in [&manifest_path, &decisions_path] {
        if !p.is_file() {
            return Err(format!("expected artifact file not found: {}", p.display()));
        }
    }

    let manifest: mqk_backtest::ReviewManifest = read_json(&manifest_path)?;
    let decisions: Vec<mqk_backtest::StrategyScanReviewDecision> = read_json(&decisions_path)?;

    let matches: Vec<&mqk_backtest::StrategyScanReviewDecision> = decisions
        .iter()
        .filter(|d| {
            d.strategy_id.trim() == strategy_id
                && d.symbol.trim().to_ascii_uppercase() == symbol
                && scanner_timeframe_label_to_secs(&d.timeframe) == Some(timeframe_secs)
        })
        .collect();

    if matches.is_empty() {
        return Err(format!(
            "no matching evidence row found for strategy_id='{strategy_id}' symbol='{symbol}' \
             timeframe_secs={timeframe_secs} in review artifact"
        ));
    }
    if matches.len() > 1 {
        return Err(format!(
            "review artifact contains {} ambiguous matching rows for this exact identity; \
             evidence must be unambiguous",
            matches.len()
        ));
    }
    let matched = matches[0];

    if matched.review_state != mqk_backtest::StrategyScanReviewState::PaperCandidate {
        return Err(format!(
            "matched evidence row has review_state='{}', not 'paper_candidate'",
            matched.review_state.code()
        ));
    }

    let canonical_json = serde_json::to_string(matched)
        .map_err(|e| format!("failed to serialize matched evidence row: {e}"))?;
    let mut hasher = Sha256::new();
    hasher.update(canonical_json.as_bytes());
    let fingerprint = hex::encode(hasher.finalize());

    Ok(ValidatedEvidence {
        review_id: manifest.review_id,
        scanner_scan_id: manifest.scanner_scan_id,
        git_hash: manifest.git_hash,
        artifact_path: candidate_canon.display().to_string(),
        fingerprint,
        scanner_score: matched.scanner_score,
        scanner_rank: matched.scanner_rank,
    })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<T, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("read failed: {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse failed: {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Read-path: durable-evidence-backed re-validation (Bundle 7, new).
// ---------------------------------------------------------------------------

/// Closed, bounded reason-code vocabulary for [`validate_active_paper_candidate`]
/// failures. Every distinct failure mode named by
/// docs/specs/dynamic_strategy_symbol_selection_01a (Phase 3) gets its own
/// variant -- never folded into a single generic "invalid" bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateEvidenceReason {
    PromotionQueryFailed,
    NoPromotionRecord,
    PromotionNotActivePaper,
    PromotionNotYetEffective,
    PromotionExpired,
    EvidenceLineageQueryFailed,
    EvidenceLineageBroken,
    ArtifactPathMissing,
    DurableFingerprintMissing,
    ArtifactRootUnavailable,
    ArtifactMissing,
    ArtifactRootEscape,
    ArtifactMalformed,
    ArtifactDuplicateIdentity,
    ArtifactNotPaperCandidate,
    ArtifactNoMatchingRow,
    FingerprintMismatch,
    ScoreMissing,
    ScoreNotFinite,
    RankOutOfRange,
    /// IR4: the durable `sys_strategy_registry` query itself failed (DB
    /// unavailable, query error) — distinct from a query that succeeded and
    /// found no row.
    RegistryQueryFailed,
    /// IR4: the durable strategy registry has no row at all for this
    /// `strategy_id` — plugin instantiability is not equivalent to durable
    /// registry admission; a candidate whose engine instantiates but whose
    /// identity was never registered must still be refused.
    RegistryRowMissing,
    /// IR4: the durable strategy registry row exists but `enabled = false`.
    RegistryDisabled,
    /// IR4: the plugin registry could not instantiate this `strategy_id` at
    /// all (unknown name or internal metadata/spec inconsistency) —
    /// distinct from `RegistryDisabled`/`RegistryRowMissing`, which are
    /// durable-registry-admission failures, not plugin-construction
    /// failures.
    UnsupportedStrategyPlugin,
    /// IR10: the ephemeral per-symbol [`mqk_strategy::PluginRegistry`]
    /// itself failed to construct (`register_builtin_strategies` returned
    /// `Err`, e.g. a duplicate-name programming error) — never silently
    /// discarded; this candidate is refused rather than proceeding against
    /// a registry that failed to build.
    RegistryConstructionFailed,
}

impl CandidateEvidenceReason {
    pub fn code(&self) -> &'static str {
        match self {
            Self::PromotionQueryFailed => "promotion_query_failed",
            Self::NoPromotionRecord => "no_promotion_record",
            Self::PromotionNotActivePaper => "promotion_not_active_paper",
            Self::PromotionNotYetEffective => "promotion_not_yet_effective",
            Self::PromotionExpired => "promotion_expired",
            Self::EvidenceLineageQueryFailed => "evidence_lineage_query_failed",
            Self::EvidenceLineageBroken => "evidence_lineage_broken",
            Self::ArtifactPathMissing => "artifact_path_missing",
            Self::DurableFingerprintMissing => "durable_fingerprint_missing",
            Self::ArtifactRootUnavailable => "artifact_root_unavailable",
            Self::ArtifactMissing => "artifact_missing",
            Self::ArtifactRootEscape => "artifact_root_escape",
            Self::ArtifactMalformed => "artifact_malformed",
            Self::ArtifactDuplicateIdentity => "artifact_duplicate_identity",
            Self::ArtifactNotPaperCandidate => "artifact_not_paper_candidate",
            Self::ArtifactNoMatchingRow => "artifact_no_matching_row",
            Self::FingerprintMismatch => "fingerprint_mismatch",
            Self::ScoreMissing => "score_missing",
            Self::ScoreNotFinite => "score_not_finite",
            Self::RankOutOfRange => "rank_out_of_range",
            Self::RegistryQueryFailed => "registry_query_failed",
            Self::RegistryRowMissing => "registry_row_missing",
            Self::RegistryDisabled => "registry_disabled",
            Self::UnsupportedStrategyPlugin => "unsupported_strategy_plugin",
            Self::RegistryConstructionFailed => "registry_construction_failed",
        }
    }
}

/// IR4: validate that `strategy_id` is currently an enabled entry in the
/// durable strategy registry (`sys_strategy_registry`) — the same durable
/// authority internal admission already consults
/// (`mqk_db::fetch_strategy_registry_entry`). Plugin instantiability
/// ([`mqk_strategy::PluginRegistry::instantiate_verified`]) is a separate,
/// independent check: an engine can instantiate for a `strategy_id` that
/// was never durably registered (or was registered and then disabled), and
/// this function is what catches that gap. Distinctly refuses query
/// failure, missing row, and disabled — never folds them into one generic
/// "not usable" outcome.
pub async fn validate_strategy_registry_enabled(
    db: &PgPool,
    strategy_id: &str,
) -> Result<(), CandidateEvidenceReason> {
    let record = mqk_db::fetch_strategy_registry_entry(db, strategy_id)
        .await
        .map_err(|_| CandidateEvidenceReason::RegistryQueryFailed)?;
    match record {
        None => Err(CandidateEvidenceReason::RegistryRowMissing),
        Some(r) if !r.enabled => Err(CandidateEvidenceReason::RegistryDisabled),
        Some(_) => Ok(()),
    }
}

/// Classify one of [`validate_paper_candidate_evidence`]'s error strings
/// into a distinct [`CandidateEvidenceReason`]. This is the *only* place
/// that inspects those message strings -- the artifact-reading logic itself
/// is never duplicated. Falls back to [`CandidateEvidenceReason::ArtifactMalformed`]
/// for any message this function does not recognize, so an unrecognized
/// future message still fails closed rather than being silently swallowed.
fn classify_artifact_error(msg: &str) -> CandidateEvidenceReason {
    if msg.contains("does not resolve inside the configured review artifact root") {
        CandidateEvidenceReason::ArtifactRootEscape
    } else if msg.contains("configured review artifact root is unavailable") {
        CandidateEvidenceReason::ArtifactRootUnavailable
    } else if msg.starts_with("review_dir not found:")
        || msg.starts_with("expected artifact file not found:")
    {
        CandidateEvidenceReason::ArtifactMissing
    } else if msg.contains("ambiguous matching rows") {
        CandidateEvidenceReason::ArtifactDuplicateIdentity
    } else if msg.contains("no matching evidence row found") {
        CandidateEvidenceReason::ArtifactNoMatchingRow
    } else if msg.contains("not 'paper_candidate'") {
        CandidateEvidenceReason::ArtifactNotPaperCandidate
    } else {
        // "review_dir is required...", "read failed:...", "parse failed:...",
        // "failed to serialize matched evidence row:..." and anything else
        // this function has not been taught to distinguish -- all fail
        // closed as a malformed-artifact refusal, never as success.
        CandidateEvidenceReason::ArtifactMalformed
    }
}

/// The complete canonical candidate snapshot this validator is responsible
/// for, on success: every fact this module independently re-derived and
/// compared against durable evidence. Bundle 7's plan builder (Phase 4)
/// combines this with plugin/timeframe/data-readiness/watchlist facts it
/// gathers itself to populate `mqk_portfolio::SelectionCandidateEvidence`.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedCandidateEvidence {
    pub evidence_review_id: String,
    pub evidence_scanner_scan_id: String,
    pub evidence_artifact_path: String,
    pub evidence_fingerprint: String,
    pub canonical_score_micros: i64,
    pub scanner_rank: Option<u32>,
}

/// Validate that `(strategy_id, symbol, timeframe_secs)` is currently an
/// unexpired, effective `active_paper` promotion whose evidence-bearing
/// review artifact still fingerprint-matches durable evidence, exactly.
///
/// Runs, in order: promotion query -> active_paper/effective/expiry gate ->
/// evidence lineage resolution -> root-bound artifact read (via the exact
/// same [`validate_paper_candidate_evidence`] the write-path route calls)
/// -> byte-for-byte fingerprint comparison -> score finiteness -> rank
/// range. The first failing step returns its own distinct
/// [`CandidateEvidenceReason`] — never a generic catch-all, and never a
/// silent partial success.
pub async fn validate_active_paper_candidate(
    db: &PgPool,
    st: &Arc<AppState>,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    authority_ts: DateTime<Utc>,
) -> Result<ValidatedCandidateEvidence, CandidateEvidenceReason> {
    let record = fetch_current_promotion_state(db, strategy_id, symbol, timeframe_secs)
        .await
        .map_err(|_| CandidateEvidenceReason::PromotionQueryFailed)?;
    let Some(record) = record else {
        return Err(CandidateEvidenceReason::NoPromotionRecord);
    };

    if record.new_state != PROMOTION_STATE_ACTIVE_PAPER {
        return Err(CandidateEvidenceReason::PromotionNotActivePaper);
    }
    if record.effective_at_utc > authority_ts {
        return Err(CandidateEvidenceReason::PromotionNotYetEffective);
    }
    if let Some(expires_at) = record.expires_at_utc {
        if expires_at <= authority_ts {
            return Err(CandidateEvidenceReason::PromotionExpired);
        }
    }

    let evidence = resolve_evidence_lineage(db, &record)
        .await
        .map_err(|_| CandidateEvidenceReason::EvidenceLineageQueryFailed)?
        .ok_or(CandidateEvidenceReason::EvidenceLineageBroken)?;

    let artifact_path = evidence
        .evidence_artifact_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(CandidateEvidenceReason::ArtifactPathMissing)?;
    let durable_fingerprint = evidence
        .evidence_fingerprint
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(CandidateEvidenceReason::DurableFingerprintMissing)?;

    let fresh =
        validate_paper_candidate_evidence(st, artifact_path, strategy_id, symbol, timeframe_secs)
            .map_err(|msg| classify_artifact_error(&msg))?;

    if fresh.fingerprint != durable_fingerprint {
        return Err(CandidateEvidenceReason::FingerprintMismatch);
    }

    let canonical_score_micros = match fresh.scanner_score {
        None => return Err(CandidateEvidenceReason::ScoreMissing),
        Some(score) => {
            canonical_score_to_micros(score).ok_or(CandidateEvidenceReason::ScoreNotFinite)?
        }
    };
    let scanner_rank = match fresh.scanner_rank {
        None => None,
        Some(r) => Some(u32::try_from(r).map_err(|_| CandidateEvidenceReason::RankOutOfRange)?),
    };

    Ok(ValidatedCandidateEvidence {
        evidence_review_id: fresh.review_id,
        evidence_scanner_scan_id: fresh.scanner_scan_id,
        evidence_artifact_path: fresh.artifact_path,
        evidence_fingerprint: fresh.fingerprint,
        canonical_score_micros,
        scanner_rank,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── canonical_score_to_micros ────────────────────────────────────────

    #[test]
    fn finite_score_converts_exactly() {
        assert_eq!(canonical_score_to_micros(9.0), Some(9_000_000));
        assert_eq!(canonical_score_to_micros(0.5), Some(500_000));
        assert_eq!(canonical_score_to_micros(-1.25), Some(-1_250_000));
        assert_eq!(canonical_score_to_micros(0.0), Some(0));
    }

    #[test]
    fn nan_score_is_rejected() {
        assert_eq!(canonical_score_to_micros(f64::NAN), None);
    }

    #[test]
    fn infinite_score_is_rejected() {
        assert_eq!(canonical_score_to_micros(f64::INFINITY), None);
        assert_eq!(canonical_score_to_micros(f64::NEG_INFINITY), None);
    }

    #[test]
    fn overflowing_score_is_rejected_not_clamped() {
        assert_eq!(canonical_score_to_micros(f64::MAX), None);
        assert_eq!(canonical_score_to_micros(-f64::MAX), None);
    }

    // ── classify_artifact_error ─────────────────────────────────────────

    #[test]
    fn classifies_root_escape() {
        assert_eq!(
            classify_artifact_error(
                "review_dir does not resolve inside the configured review artifact root"
            ),
            CandidateEvidenceReason::ArtifactRootEscape
        );
    }

    #[test]
    fn classifies_root_unavailable() {
        assert_eq!(
            classify_artifact_error("configured review artifact root is unavailable: /x"),
            CandidateEvidenceReason::ArtifactRootUnavailable
        );
    }

    #[test]
    fn classifies_missing_artifact() {
        assert_eq!(
            classify_artifact_error("review_dir not found: /tmp/nope"),
            CandidateEvidenceReason::ArtifactMissing
        );
        assert_eq!(
            classify_artifact_error("expected artifact file not found: /tmp/x/manifest.json"),
            CandidateEvidenceReason::ArtifactMissing
        );
    }

    #[test]
    fn classifies_duplicate_identity() {
        assert_eq!(
            classify_artifact_error(
                "review artifact contains 2 ambiguous matching rows for this exact identity; \
                 evidence must be unambiguous"
            ),
            CandidateEvidenceReason::ArtifactDuplicateIdentity
        );
    }

    #[test]
    fn classifies_no_matching_row() {
        assert_eq!(
            classify_artifact_error(
                "no matching evidence row found for strategy_id='x' symbol='AAPL' \
                 timeframe_secs=300 in review artifact"
            ),
            CandidateEvidenceReason::ArtifactNoMatchingRow
        );
    }

    #[test]
    fn classifies_not_paper_candidate() {
        assert_eq!(
            classify_artifact_error(
                "matched evidence row has review_state='rejected', not 'paper_candidate'"
            ),
            CandidateEvidenceReason::ArtifactNotPaperCandidate
        );
    }

    #[test]
    fn classifies_unrecognized_message_as_malformed_not_success() {
        assert_eq!(
            classify_artifact_error("read failed: /tmp/x/manifest.json: permission denied"),
            CandidateEvidenceReason::ArtifactMalformed
        );
        assert_eq!(
            classify_artifact_error("parse failed: /tmp/x/manifest.json: expected value"),
            CandidateEvidenceReason::ArtifactMalformed
        );
        assert_eq!(
            classify_artifact_error("some completely unexpected future message"),
            CandidateEvidenceReason::ArtifactMalformed
        );
    }

    #[test]
    fn reason_code_is_stable_and_distinct() {
        let all = [
            CandidateEvidenceReason::PromotionQueryFailed,
            CandidateEvidenceReason::NoPromotionRecord,
            CandidateEvidenceReason::PromotionNotActivePaper,
            CandidateEvidenceReason::PromotionNotYetEffective,
            CandidateEvidenceReason::PromotionExpired,
            CandidateEvidenceReason::EvidenceLineageQueryFailed,
            CandidateEvidenceReason::EvidenceLineageBroken,
            CandidateEvidenceReason::ArtifactPathMissing,
            CandidateEvidenceReason::DurableFingerprintMissing,
            CandidateEvidenceReason::ArtifactRootUnavailable,
            CandidateEvidenceReason::ArtifactMissing,
            CandidateEvidenceReason::ArtifactRootEscape,
            CandidateEvidenceReason::ArtifactMalformed,
            CandidateEvidenceReason::ArtifactDuplicateIdentity,
            CandidateEvidenceReason::ArtifactNotPaperCandidate,
            CandidateEvidenceReason::ArtifactNoMatchingRow,
            CandidateEvidenceReason::FingerprintMismatch,
            CandidateEvidenceReason::ScoreMissing,
            CandidateEvidenceReason::ScoreNotFinite,
            CandidateEvidenceReason::RankOutOfRange,
            CandidateEvidenceReason::RegistryQueryFailed,
            CandidateEvidenceReason::RegistryRowMissing,
            CandidateEvidenceReason::RegistryDisabled,
            CandidateEvidenceReason::UnsupportedStrategyPlugin,
            CandidateEvidenceReason::RegistryConstructionFailed,
        ];
        let mut codes: Vec<&str> = all.iter().map(|r| r.code()).collect();
        let unique_count = {
            let mut c = codes.clone();
            c.sort_unstable();
            c.dedup();
            c.len()
        };
        assert_eq!(
            unique_count,
            codes.len(),
            "every reason code must be distinct"
        );
        codes.clear();
    }

    // ── IR4: validate_strategy_registry_enabled (DB-backed) ────────────────

    async fn make_db_pool_for_test() -> sqlx::PgPool {
        let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
            panic!(
                "DB tests require MQK_DATABASE_URL; run: \
                 MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
                 cargo test -p mqk-daemon --lib promotion_evidence_validation \
                 -- --include-ignored --test-threads=1"
            )
        });
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect to test DB");
        mqk_db::migrate(&pool).await.expect("run migrations");
        pool
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
    async fn missing_registry_row_is_refused() {
        let pool = make_db_pool_for_test().await;
        let strategy_id = format!("nonexistent_{}", uuid::Uuid::new_v4());
        let result = validate_strategy_registry_enabled(&pool, &strategy_id).await;
        assert_eq!(result, Err(CandidateEvidenceReason::RegistryRowMissing));
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
    async fn disabled_registry_row_is_refused() {
        let pool = make_db_pool_for_test().await;
        let strategy_id = format!("disabled_strat_{}", uuid::Uuid::new_v4());
        mqk_db::upsert_strategy_registry_entry(
            &pool,
            &mqk_db::UpsertStrategyRegistryArgs {
                strategy_id: strategy_id.clone(),
                display_name: "Test Disabled".to_string(),
                enabled: false,
                kind: "bar_driven".to_string(),
                registered_at_utc: Utc::now(),
                updated_at_utc: Utc::now(),
                note: String::new(),
            },
        )
        .await
        .expect("upsert must succeed");

        let result = validate_strategy_registry_enabled(&pool, &strategy_id).await;
        assert_eq!(result, Err(CandidateEvidenceReason::RegistryDisabled));
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
    async fn enabled_registry_row_passes() {
        let pool = make_db_pool_for_test().await;
        let strategy_id = format!("enabled_strat_{}", uuid::Uuid::new_v4());
        mqk_db::upsert_strategy_registry_entry(
            &pool,
            &mqk_db::UpsertStrategyRegistryArgs {
                strategy_id: strategy_id.clone(),
                display_name: "Test Enabled".to_string(),
                enabled: true,
                kind: "bar_driven".to_string(),
                registered_at_utc: Utc::now(),
                updated_at_utc: Utc::now(),
                note: String::new(),
            },
        )
        .await
        .expect("upsert must succeed");

        let result = validate_strategy_registry_enabled(&pool, &strategy_id).await;
        assert!(result.is_ok());
    }
}
