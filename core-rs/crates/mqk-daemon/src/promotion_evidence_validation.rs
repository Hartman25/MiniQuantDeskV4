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
use serde_json::value::RawValue;
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
//
// IR7: the old `raw_f64 * 1e6 -> round()` scheme is gone -- it is not an
// exact representation of the artifact's JSON numeric token (see
// `mqk_portfolio::canonicalize_decimal_token`'s module docs for why). Score
// canonicalization now runs entirely on the *raw JSON token text*, captured
// via `serde_json::value::RawValue` (the `raw_value` feature only adds this
// opt-in capability; it changes nothing about how any other type in this
// crate deserializes numbers) and handed to the pure, zero-float
// `mqk_portfolio::canonicalize_decimal_token` for parsing/canonicalization.

/// Shadow deserialization target used solely to recover the exact raw JSON
/// text of one row's `scanner_score` field, positionally aligned with the
/// already-matched-and-validated `Vec<StrategyScanReviewDecision>` parsed
/// from the same `review_decisions.json` text. Every other field is
/// intentionally omitted -- serde ignores unknown/absent fields by default,
/// and this type never becomes the source of truth for anything except the
/// one raw token.
#[derive(serde::Deserialize)]
struct RawScoreRow<'a> {
    #[serde(borrow)]
    scanner_score: Option<&'a RawValue>,
}

/// Re-read `decisions_path`'s raw text and recover the exact JSON numeric
/// token backing `decisions[matched_index].scanner_score` -- `Ok(None)` when
/// the artifact's `scanner_score` is JSON `null` (absent), never a
/// fabricated `"0"`. The two parses (into `Vec<StrategyScanReviewDecision>`
/// and into `Vec<RawScoreRow>`) read the *same* file text and therefore
/// preserve the same array order, so positional alignment by
/// `matched_index` is exact.
fn extract_raw_scanner_score_token(
    decisions_path: &Path,
    matched_index: usize,
) -> Result<Option<String>, String> {
    let raw_text = std::fs::read_to_string(decisions_path)
        .map_err(|e| format!("read failed: {}: {e}", decisions_path.display()))?;
    let rows: Vec<RawScoreRow> = serde_json::from_str(&raw_text)
        .map_err(|e| format!("parse failed: {}: {e}", decisions_path.display()))?;
    let row = rows.get(matched_index).ok_or_else(|| {
        format!(
            "matched row index {matched_index} vanished on raw re-scan of {}",
            decisions_path.display()
        )
    })?;
    Ok(row.scanner_score.map(|rv| rv.get().to_string()))
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
    /// artifact -- never defaulted to `0.0` or any other sentinel. Retained
    /// for any existing caller that wants the lossy float; the read path
    /// (`validate_active_paper_candidate`) uses [`Self::scanner_score_token`]
    /// instead (IR7).
    pub scanner_score: Option<f64>,
    /// IR7: the exact raw JSON numeric token backing `scanner_score`, e.g.
    /// `"0.1234567"` -- never round-tripped through a float. `None` exactly
    /// when [`Self::scanner_score`] is `None`.
    pub scanner_score_token: Option<String>,
    /// `None` when the matched row's raw `scanner_rank` is absent from the
    /// artifact -- never defaulted.
    pub scanner_rank: Option<usize>,
    /// IR6: the matched row's exact `review_state` code (e.g.
    /// `"paper_candidate"`) -- carried through even though this function
    /// itself already enforces it must equal `"paper_candidate"` on
    /// success, so a caller never has to guess or re-derive it.
    pub review_state: String,
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
    let matched_index = decisions
        .iter()
        .position(|d| {
            d.strategy_id.trim() == strategy_id
                && d.symbol.trim().to_ascii_uppercase() == symbol
                && scanner_timeframe_label_to_secs(&d.timeframe) == Some(timeframe_secs)
        })
        .expect("matched above; index recomputation must agree");
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

    // IR7: recover the exact raw JSON token backing this row's
    // `scanner_score`, positionally, from the same file text already parsed
    // above -- never derived from `matched.scanner_score: Option<f64>`,
    // which has already lost any precision beyond an f64's.
    let scanner_score_token = extract_raw_scanner_score_token(&decisions_path, matched_index)?;

    Ok(ValidatedEvidence {
        review_id: manifest.review_id,
        scanner_scan_id: manifest.scanner_scan_id,
        git_hash: manifest.git_hash,
        artifact_path: candidate_canon.display().to_string(),
        fingerprint,
        scanner_score: matched.scanner_score,
        scanner_score_token,
        scanner_rank: matched.scanner_rank,
        review_state: matched.review_state.code().to_string(),
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
    /// IR6: the review artifact manifest's `git_hash`.
    pub evidence_git_hash: String,
    /// IR6: the matched review row's exact `review_state` code.
    pub evidence_review_state: String,
    /// IR7: canonical, decimal-exact string form of the durable
    /// `scanner_score` -- the authoritative score; never derived from a
    /// float.
    pub canonical_score_decimal: String,
    /// IR7: convenience bridge to the legacy scaled-integer (micros)
    /// representation, `Some` only when exactly representable at that
    /// scale. See `mqk_portfolio::canonical_decimal_to_micros_if_exact`.
    pub canonical_score_micros: Option<i64>,
    pub scanner_rank: Option<u32>,
    /// IR6: the durable promotion transition id currently authorizing this
    /// identity's `active_paper` state.
    pub promotion_transition_id: uuid::Uuid,
    pub promotion_effective_at: DateTime<Utc>,
    pub promotion_expires_at: Option<DateTime<Utc>>,
    /// IR6: the exact transition id that established the evidence this
    /// validation compared against (see
    /// `mqk_db::StrategyPromotionTransitionRecord::evidence_transition_id`
    /// / `resolve_evidence_lineage`).
    pub evidence_transition_id: uuid::Uuid,
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

    // IR7: canonicalize from the exact raw JSON token -- never from
    // `fresh.scanner_score: Option<f64>`, which has already lost any
    // precision beyond an f64's.
    let canonical_score_decimal = match fresh.scanner_score_token.as_deref() {
        None => return Err(CandidateEvidenceReason::ScoreMissing),
        Some(token) => mqk_portfolio::canonicalize_decimal_token(token)
            .ok_or(CandidateEvidenceReason::ScoreNotFinite)?,
    };
    let canonical_score_micros =
        mqk_portfolio::canonical_decimal_to_micros_if_exact(&canonical_score_decimal);
    let scanner_rank = match fresh.scanner_rank {
        None => None,
        Some(r) => Some(u32::try_from(r).map_err(|_| CandidateEvidenceReason::RankOutOfRange)?),
    };

    Ok(ValidatedCandidateEvidence {
        evidence_review_id: fresh.review_id,
        evidence_scanner_scan_id: fresh.scanner_scan_id,
        evidence_artifact_path: fresh.artifact_path,
        evidence_fingerprint: fresh.fingerprint,
        evidence_git_hash: fresh.git_hash,
        evidence_review_state: fresh.review_state,
        canonical_score_decimal,
        canonical_score_micros,
        scanner_rank,
        promotion_transition_id: record.transition_id,
        promotion_effective_at: record.effective_at_utc,
        promotion_expires_at: record.expires_at_utc,
        evidence_transition_id: evidence.transition_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── IR7: exact raw-token extraction, filesystem-to-RawValue end-to-end ──
    // (pure-decimal canonicalization math itself is proven in
    // mqk_portfolio::canonical_decimal's own unit tests; this test proves
    // this crate's artifact-reading layer hands that math the untouched
    // original token, not an f64 round-trip.)

    #[test]
    fn scanner_score_token_preserves_precision_an_f64_roundtrip_would_lose() {
        let root = std::env::temp_dir().join(format!(
            "mqk_daemon_scanner_score_token_test_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create temp root");
        let review_dir = root.join("review-1");
        std::fs::create_dir_all(&review_dir).expect("create review dir");

        std::fs::write(
            review_dir.join("manifest.json"),
            r#"{
                "schema_version": 1,
                "review_id": "review-1",
                "scanner_scan_id": "scan-1",
                "source_artifact_dir": "fixture",
                "created_at_utc": "2026-07-01T00:00:00Z",
                "git_hash": "test-git-hash",
                "policy_min_bars_used": 252,
                "policy_min_trade_count": 5,
                "policy_min_total_return_pct": 0.0,
                "policy_min_alpha_pct": 0.0,
                "policy_max_drawdown_pct": 25.0,
                "policy_min_profit_factor": 1.05,
                "candidate_count": 1,
                "blocked_count": 0,
                "needs_review_count": 0,
                "watchlist_candidate_count": 0,
                "paper_candidate_count": 1,
                "rejected_count": 0,
                "blockers": [],
                "warnings": []
            }"#,
        )
        .expect("write manifest");

        // 15 fractional digits -- an f64 round-trip through
        // `raw * 1e6 -> round()` cannot be trusted to preserve this exactly;
        // the raw-token path must hand back this literal string untouched.
        std::fs::write(
            review_dir.join("review_decisions.json"),
            r#"[{
                "symbol": "AAPL",
                "timeframe": "1D",
                "strategy_id": "swing_momentum",
                "scanner_rank": 1,
                "scanner_score": 0.123456789012345,
                "review_state": "paper_candidate",
                "reason_codes": ["eligible_paper_candidate"],
                "blockers": [],
                "warnings": []
            }]"#,
        )
        .expect("write decisions");

        std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", &root);
        let st = crate::state::AppState::new_with_operator_auth(
            crate::state::OperatorAuthMode::ExplicitDevNoToken,
        );

        let evidence = validate_paper_candidate_evidence(
            &st,
            review_dir.to_str().unwrap(),
            "swing_momentum",
            "AAPL",
            86400,
        )
        .expect("evidence must validate");

        assert_eq!(
            evidence.scanner_score_token.as_deref(),
            Some("0.123456789012345"),
            "must be the exact original token, not an f64-rounded approximation"
        );
        assert_eq!(evidence.review_state, "paper_candidate");

        std::fs::remove_dir_all(&root).ok();
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
