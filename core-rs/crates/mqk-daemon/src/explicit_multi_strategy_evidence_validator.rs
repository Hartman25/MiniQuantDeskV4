//! MULTI-STRATEGY-RUNTIME-DISPATCH-01 Patch D3 (V4-STAGE-B-M2-C1-C3-REPAIR-04):
//! the one production read-side validator for a persisted explicit
//! multi-strategy authority, consulted before host-pool
//! construction/activation ever consumes it.
//!
//! Mirrors `dynamic_selection_evidence_validator.rs`'s own contract for
//! Bundle 7: a stored `authority_id` existing with the expected binding
//! count is not read-side validation. This module reads the persisted
//! header + bindings and fails closed unless the durable payload is exactly
//! coherent -- every header identity fact matches what this start attempt
//! actually resolved, the exact binding set matches (no missing child, no
//! extra child, no duplicate), and `authority_id` recomputed from the
//! DURABLE STORED facts (via the same deterministic
//! `derive_explicit_multi_strategy_authority_id` the write path used) equals
//! the stored id -- never trusting the stored column on its own. Never
//! synthesizes evidence from the current environment and never repairs on
//! read: any incoherence is reported, never silently corrected.

use std::collections::BTreeSet;

use uuid::Uuid;

use crate::multi_strategy_runtime_dispatch::{
    derive_explicit_multi_strategy_authority_id, ExplicitBindingEvaluation,
};

/// Every fact this start attempt's in-memory resolution expects the durable
/// authority to carry -- the "expected" side of the coherence check. Built
/// once by the caller from the same inputs
/// `build_new_explicit_multi_strategy_authority` was given, never re-derived
/// independently a second time.
pub(crate) struct ExpectedExplicitMultiStrategyAuthority<'a> {
    pub(crate) authority_id: Uuid,
    pub(crate) run_id: Uuid,
    pub(crate) source_kind: &'a str,
    pub(crate) source_identity: &'a str,
    pub(crate) source_artifact_hash: &'a str,
    pub(crate) config_fingerprint: &'a str,
    pub(crate) market_date: &'a str,
    pub(crate) writer_version: &'a str,
    pub(crate) evaluations: &'a [ExplicitBindingEvaluation],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExplicitMultiStrategyEvidenceValidationError {
    RunIdMismatch,
    SourceKindMismatch,
    SourceIdentityMismatch,
    SourceArtifactHashMismatch,
    ConfigFingerprintMismatch,
    MarketDateMismatch,
    ApprovedForLiveTrue,
    WriterVersionMismatch,
    BindingCountMismatch {
        expected: usize,
        stored: usize,
    },
    AuthorizedCountMismatch {
        expected: usize,
        stored: i32,
    },
    DuplicateStoredBinding {
        symbol: String,
        strategy_id: String,
        timeframe_secs: i64,
    },
    MissingBinding {
        symbol: String,
        strategy_id: String,
        timeframe_secs: i64,
    },
    ExtraBinding {
        symbol: String,
        strategy_id: String,
        timeframe_secs: i64,
    },
    UnparseableExactReason {
        symbol: String,
        strategy_id: String,
        timeframe_secs: i64,
    },
    NegativeScannerRank {
        symbol: String,
        strategy_id: String,
        timeframe_secs: i64,
    },
    /// `authority_id` recomputed from the durable stored header + bindings
    /// does not equal the stored `authority_id` column -- the durable row's
    /// own content and its claimed identity have diverged (corruption, a
    /// hand-edited row, or a writer-path defect).
    RecomputedAuthorityIdMismatch,
    /// The stored, self-consistent durable `authority_id` does not equal the
    /// `authority_id` this start attempt's in-memory resolution expected --
    /// the durable authority is coherent with itself but is not the
    /// authority this run actually resolved.
    ExpectedAuthorityIdMismatch,
}

impl ExplicitMultiStrategyEvidenceValidationError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::RunIdMismatch => "explicit_multi_strategy_evidence_run_id_mismatch",
            Self::SourceKindMismatch => "explicit_multi_strategy_evidence_source_kind_mismatch",
            Self::SourceIdentityMismatch => {
                "explicit_multi_strategy_evidence_source_identity_mismatch"
            }
            Self::SourceArtifactHashMismatch => {
                "explicit_multi_strategy_evidence_source_artifact_hash_mismatch"
            }
            Self::ConfigFingerprintMismatch => {
                "explicit_multi_strategy_evidence_config_fingerprint_mismatch"
            }
            Self::MarketDateMismatch => "explicit_multi_strategy_evidence_market_date_mismatch",
            Self::ApprovedForLiveTrue => "explicit_multi_strategy_evidence_approved_for_live_true",
            Self::WriterVersionMismatch => {
                "explicit_multi_strategy_evidence_writer_version_mismatch"
            }
            Self::BindingCountMismatch { .. } => {
                "explicit_multi_strategy_evidence_binding_count_mismatch"
            }
            Self::AuthorizedCountMismatch { .. } => {
                "explicit_multi_strategy_evidence_authorized_count_mismatch"
            }
            Self::DuplicateStoredBinding { .. } => {
                "explicit_multi_strategy_evidence_duplicate_stored_binding"
            }
            Self::MissingBinding { .. } => "explicit_multi_strategy_evidence_missing_binding",
            Self::ExtraBinding { .. } => "explicit_multi_strategy_evidence_extra_binding",
            Self::UnparseableExactReason { .. } => {
                "explicit_multi_strategy_evidence_unparseable_exact_reason"
            }
            Self::NegativeScannerRank { .. } => {
                "explicit_multi_strategy_evidence_negative_scanner_rank"
            }
            Self::RecomputedAuthorityIdMismatch => {
                "explicit_multi_strategy_evidence_recomputed_authority_id_mismatch"
            }
            Self::ExpectedAuthorityIdMismatch => {
                "explicit_multi_strategy_evidence_expected_authority_id_mismatch"
            }
        }
    }
}

type BindingKey = (String, String, i64);

fn binding_key(symbol: &str, strategy_id: &str, timeframe_secs: i64) -> BindingKey {
    (symbol.to_string(), strategy_id.to_string(), timeframe_secs)
}

/// Reconstruct the exact `ExplicitBindingEvaluation` a durable stored
/// binding row represents -- the inverse of
/// `build_new_explicit_multi_strategy_authority`'s own mapping. Fails closed
/// if a stored value cannot be losslessly reconstructed (an unparseable
/// `exact_reason` code, or a negative `scanner_rank`) rather than silently
/// coercing it to a default that would make a corrupted row look valid.
fn stored_binding_to_evaluation(
    b: &mqk_db::ExplicitMultiStrategyAuthorityBindingRecord,
) -> Result<ExplicitBindingEvaluation, ExplicitMultiStrategyEvidenceValidationError> {
    let exact_reason = match &b.exact_reason {
        Some(code) => Some(
            mqk_portfolio::ExactSelectionReason::parse_code(code).ok_or_else(|| {
                ExplicitMultiStrategyEvidenceValidationError::UnparseableExactReason {
                    symbol: b.symbol.clone(),
                    strategy_id: b.strategy_id.clone(),
                    timeframe_secs: b.timeframe_secs,
                }
            })?,
        ),
        None => None,
    };
    let scanner_rank = match b.scanner_rank {
        Some(r) if r >= 0 => Some(r as u32),
        Some(_) => {
            return Err(
                ExplicitMultiStrategyEvidenceValidationError::NegativeScannerRank {
                    symbol: b.symbol.clone(),
                    strategy_id: b.strategy_id.clone(),
                    timeframe_secs: b.timeframe_secs,
                },
            )
        }
        None => None,
    };

    let evidence = mqk_portfolio::SelectionCandidateEvidence {
        promotion_query_ok: b.promotion_query_ok,
        promotion_state: b.promotion_state.clone(),
        promotion_effective: b.promotion_effective,
        promotion_expired: b.promotion_expired,
        evidence_resolved: b.evidence_resolved,
        review_state_is_paper_candidate: b.review_state_is_paper_candidate,
        evidence_review_state: b.evidence_review_state.clone(),
        durable_legacy_fingerprint: b.durable_legacy_fingerprint.clone(),
        recomputed_legacy_fingerprint: b.recomputed_legacy_fingerprint.clone(),
        legacy_fingerprint_matches: b.legacy_fingerprint_matches,
        durable_exact_fingerprint_v2: b.durable_exact_fingerprint_v2.clone(),
        recomputed_exact_fingerprint_v2: b.recomputed_exact_fingerprint_v2.clone(),
        exact_fingerprint_v2_matches: b.exact_fingerprint_v2_matches,
        config_identity_verified: b.config_identity_verified,
        durable_config_fingerprint: b.durable_config_fingerprint.clone(),
        current_config_fingerprint: b.current_config_fingerprint.clone(),
        registry_enabled: b.registry_enabled,
        plugin_instantiable: b.plugin_instantiable,
        timeframe_matches: b.timeframe_matches,
        data_ready: b.data_ready,
        canonical_score_decimal: b.canonical_score_decimal.clone(),
        canonical_score_micros: b.canonical_score_micros,
        scanner_rank,
        watchlist_assigned: b.watchlist_assigned,
        evidence_review_id: b.evidence_review_id.clone(),
        evidence_scanner_scan_id: b.evidence_scanner_scan_id.clone(),
        evidence_artifact_path: b.evidence_artifact_path.clone(),
        evidence_git_hash: b.evidence_git_hash.clone(),
        promotion_transition_id: b.promotion_transition_id.clone(),
        promotion_effective_at: b.promotion_effective_at.clone(),
        promotion_expires_at: b.promotion_expires_at.clone(),
        evidence_transition_id: b.evidence_transition_id.clone(),
        exact_reason,
    };

    Ok(ExplicitBindingEvaluation {
        symbol: b.symbol.clone(),
        strategy_id: b.strategy_id.clone(),
        timeframe_secs: b.timeframe_secs,
        authorized: b.authorized,
        reason_code: b.reason_code.clone(),
        evidence,
    })
}

/// True read-side validation (frozen contract; D3 independent-review
/// finding). Validates, in order:
///
/// 1. Every header identity fact (`run_id`, `source_kind`,
///    `source_identity`, `source_artifact_hash`, `config_fingerprint`,
///    `market_date`, `writer_version`) matches what this start attempt
///    expected -- never merely `authority_id` + a length check.
/// 2. `approved_for_live` is `false`.
/// 3. `binding_count`/`authorized_count` match both the stored bindings
///    themselves and the expected evaluation set.
/// 4. The exact `(symbol, strategy_id, timeframe_secs)` binding set matches
///    the expected evaluation set exactly -- no missing child, no extra
///    child, no duplicate stored binding.
/// 5. `authority_id` recomputed from the DURABLE STORED header + bindings
///    (via the same deterministic derivation the write path used) equals
///    the stored `authority_id` column -- the row has not diverged from its
///    own claimed identity.
/// 6. The stored `authority_id` equals `expected.authority_id` -- the
///    durable authority is not merely self-consistent, it is the exact
///    authority this run resolved.
pub(crate) fn validate_explicit_multi_strategy_authority(
    expected: &ExpectedExplicitMultiStrategyAuthority<'_>,
    header: &mqk_db::ExplicitMultiStrategyAuthorityRecord,
    bindings: &[mqk_db::ExplicitMultiStrategyAuthorityBindingRecord],
) -> Result<(), ExplicitMultiStrategyEvidenceValidationError> {
    use ExplicitMultiStrategyEvidenceValidationError as E;

    if header.run_id != expected.run_id {
        return Err(E::RunIdMismatch);
    }
    if header.source_kind != expected.source_kind {
        return Err(E::SourceKindMismatch);
    }
    if header.source_identity != expected.source_identity {
        return Err(E::SourceIdentityMismatch);
    }
    if header.source_artifact_hash != expected.source_artifact_hash {
        return Err(E::SourceArtifactHashMismatch);
    }
    if header.config_fingerprint != expected.config_fingerprint {
        return Err(E::ConfigFingerprintMismatch);
    }
    if header.market_date != expected.market_date {
        return Err(E::MarketDateMismatch);
    }
    if header.approved_for_live {
        return Err(E::ApprovedForLiveTrue);
    }
    if header.writer_version != expected.writer_version {
        return Err(E::WriterVersionMismatch);
    }

    if header.binding_count as usize != bindings.len() {
        return Err(E::BindingCountMismatch {
            expected: expected.evaluations.len(),
            stored: bindings.len(),
        });
    }
    if header.binding_count as usize != expected.evaluations.len() {
        return Err(E::BindingCountMismatch {
            expected: expected.evaluations.len(),
            stored: bindings.len(),
        });
    }
    let stored_authorized_count = bindings.iter().filter(|b| b.authorized).count() as i32;
    if header.authorized_count != stored_authorized_count {
        return Err(E::AuthorizedCountMismatch {
            expected: expected.evaluations.iter().filter(|e| e.authorized).count(),
            stored: header.authorized_count,
        });
    }
    let expected_authorized_count = expected.evaluations.iter().filter(|e| e.authorized).count();
    if header.authorized_count as usize != expected_authorized_count {
        return Err(E::AuthorizedCountMismatch {
            expected: expected_authorized_count,
            stored: header.authorized_count,
        });
    }

    // Exact binding set: no missing child, no extra child, no duplicate
    // stored binding.
    let mut stored_keys: BTreeSet<BindingKey> = BTreeSet::new();
    for b in bindings {
        let key = binding_key(&b.symbol, &b.strategy_id, b.timeframe_secs);
        if !stored_keys.insert(key) {
            return Err(E::DuplicateStoredBinding {
                symbol: b.symbol.clone(),
                strategy_id: b.strategy_id.clone(),
                timeframe_secs: b.timeframe_secs,
            });
        }
    }
    let expected_keys: BTreeSet<BindingKey> = expected
        .evaluations
        .iter()
        .map(|e| binding_key(&e.symbol, &e.strategy_id, e.timeframe_secs))
        .collect();
    if let Some(key) = expected_keys.difference(&stored_keys).next() {
        return Err(E::MissingBinding {
            symbol: key.0.clone(),
            strategy_id: key.1.clone(),
            timeframe_secs: key.2,
        });
    }
    if let Some(key) = stored_keys.difference(&expected_keys).next() {
        return Err(E::ExtraBinding {
            symbol: key.0.clone(),
            strategy_id: key.1.clone(),
            timeframe_secs: key.2,
        });
    }

    // Recompute authority_id from the DURABLE STORED facts -- never trust
    // the stored column on its own.
    let stored_evaluations: Vec<ExplicitBindingEvaluation> = bindings
        .iter()
        .map(stored_binding_to_evaluation)
        .collect::<Result<_, _>>()?;
    let recomputed_from_stored = derive_explicit_multi_strategy_authority_id(
        header.run_id,
        &header.source_identity,
        &header.source_artifact_hash,
        &header.config_fingerprint,
        &header.market_date,
        &stored_evaluations,
    );
    if recomputed_from_stored != header.authority_id {
        return Err(E::RecomputedAuthorityIdMismatch);
    }

    if header.authority_id != expected.authority_id {
        return Err(E::ExpectedAuthorityIdMismatch);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn evidence() -> mqk_portfolio::SelectionCandidateEvidence {
        mqk_portfolio::SelectionCandidateEvidence {
            promotion_query_ok: true,
            promotion_state: Some("active_paper".to_string()),
            promotion_effective: true,
            promotion_expired: false,
            evidence_resolved: true,
            review_state_is_paper_candidate: true,
            evidence_review_state: Some("paper_candidate".to_string()),
            durable_legacy_fingerprint: Some("legacy-a".to_string()),
            recomputed_legacy_fingerprint: Some("legacy-a".to_string()),
            legacy_fingerprint_matches: true,
            durable_exact_fingerprint_v2: Some("v2-a".to_string()),
            recomputed_exact_fingerprint_v2: Some("v2-a".to_string()),
            exact_fingerprint_v2_matches: true,
            config_identity_verified: true,
            durable_config_fingerprint: Some("cfg-a".to_string()),
            current_config_fingerprint: Some("cfg-a".to_string()),
            registry_enabled: true,
            plugin_instantiable: true,
            timeframe_matches: true,
            data_ready: true,
            canonical_score_decimal: Some("1".to_string()),
            canonical_score_micros: Some(1_000_000),
            scanner_rank: Some(1),
            watchlist_assigned: true,
            evidence_review_id: None,
            evidence_scanner_scan_id: None,
            evidence_artifact_path: None,
            evidence_git_hash: None,
            promotion_transition_id: None,
            promotion_effective_at: None,
            promotion_expires_at: None,
            evidence_transition_id: None,
            exact_reason: None,
        }
    }

    fn evaluation(symbol: &str, strategy_id: &str) -> ExplicitBindingEvaluation {
        ExplicitBindingEvaluation {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe_secs: 300,
            authorized: true,
            reason_code: "explicit_multi_strategy_authorized".to_string(),
            evidence: evidence(),
        }
    }

    struct Fixture {
        run_id: Uuid,
        source_identity: String,
        source_artifact_hash: String,
        config_fingerprint: String,
        market_date: String,
        evaluations: Vec<ExplicitBindingEvaluation>,
        header: mqk_db::ExplicitMultiStrategyAuthorityRecord,
        bindings: Vec<mqk_db::ExplicitMultiStrategyAuthorityBindingRecord>,
    }

    impl Fixture {
        fn expected(&self) -> ExpectedExplicitMultiStrategyAuthority<'_> {
            ExpectedExplicitMultiStrategyAuthority {
                authority_id: self.header.authority_id,
                run_id: self.run_id,
                source_kind: mqk_db::EXPLICIT_MULTI_STRATEGY_SOURCE_KIND,
                source_identity: &self.source_identity,
                source_artifact_hash: &self.source_artifact_hash,
                config_fingerprint: &self.config_fingerprint,
                market_date: &self.market_date,
                writer_version:
                    crate::multi_strategy_runtime_dispatch::EXPLICIT_MULTI_STRATEGY_AUTHORITY_WRITER_VERSION,
                evaluations: &self.evaluations,
            }
        }
    }

    /// Build a self-consistent (header, bindings) pair via the exact same
    /// write-path DTO construction (`build_new_explicit_multi_strategy_authority`)
    /// the real writer uses -- never a hand-rolled shape that could silently
    /// diverge from what production actually persists.
    fn build_fixture() -> Fixture {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"d3-validator-run");
        let source_identity = "watchlist-v3.json".to_string();
        let source_artifact_hash = "hash-a".to_string();
        let config_fingerprint = "cfg-a".to_string();
        let market_date = "2026-09-18".to_string();
        let evaluations = vec![
            evaluation("AAPL", "intraday_scalper"),
            evaluation("MSFT", "swing_momentum"),
        ];
        let created_at_utc = Utc::now();
        let new_authority =
            crate::multi_strategy_runtime_dispatch::build_new_explicit_multi_strategy_authority(
                &evaluations,
                run_id,
                &source_identity,
                &source_artifact_hash,
                &config_fingerprint,
                &market_date,
                created_at_utc,
            )
            .expect("fixture scanner_rank values are within i32::MAX");

        let header = mqk_db::ExplicitMultiStrategyAuthorityRecord {
            authority_id: new_authority.authority_id,
            run_id: new_authority.run_id,
            source_kind: new_authority.source_kind.clone(),
            source_identity: new_authority.source_identity.clone(),
            source_artifact_hash: new_authority.source_artifact_hash.clone(),
            config_fingerprint: new_authority.config_fingerprint.clone(),
            market_date: new_authority.market_date.clone(),
            approved_for_live: new_authority.approved_for_live,
            binding_count: new_authority.bindings.len() as i32,
            authorized_count: new_authority
                .bindings
                .iter()
                .filter(|b| b.authorized)
                .count() as i32,
            writer_version: new_authority.writer_version.clone(),
            created_at_utc,
        };
        let bindings: Vec<mqk_db::ExplicitMultiStrategyAuthorityBindingRecord> = new_authority
            .bindings
            .iter()
            .enumerate()
            .map(
                |(ordinal, b)| mqk_db::ExplicitMultiStrategyAuthorityBindingRecord {
                    authority_id: new_authority.authority_id,
                    ordinal: ordinal as i32,
                    symbol: b.symbol.clone(),
                    strategy_id: b.strategy_id.clone(),
                    timeframe_secs: b.timeframe_secs,
                    authorized: b.authorized,
                    reason_code: b.reason_code.clone(),
                    promotion_query_ok: b.promotion_query_ok,
                    promotion_state: b.promotion_state.clone(),
                    promotion_effective: b.promotion_effective,
                    promotion_expired: b.promotion_expired,
                    evidence_resolved: b.evidence_resolved,
                    review_state_is_paper_candidate: b.review_state_is_paper_candidate,
                    evidence_review_state: b.evidence_review_state.clone(),
                    durable_legacy_fingerprint: b.durable_legacy_fingerprint.clone(),
                    recomputed_legacy_fingerprint: b.recomputed_legacy_fingerprint.clone(),
                    legacy_fingerprint_matches: b.legacy_fingerprint_matches,
                    durable_exact_fingerprint_v2: b.durable_exact_fingerprint_v2.clone(),
                    recomputed_exact_fingerprint_v2: b.recomputed_exact_fingerprint_v2.clone(),
                    exact_fingerprint_v2_matches: b.exact_fingerprint_v2_matches,
                    config_identity_verified: b.config_identity_verified,
                    durable_config_fingerprint: b.durable_config_fingerprint.clone(),
                    current_config_fingerprint: b.current_config_fingerprint.clone(),
                    registry_enabled: b.registry_enabled,
                    plugin_instantiable: b.plugin_instantiable,
                    timeframe_matches: b.timeframe_matches,
                    data_ready: b.data_ready,
                    canonical_score_decimal: b.canonical_score_decimal.clone(),
                    canonical_score_micros: b.canonical_score_micros,
                    scanner_rank: b.scanner_rank,
                    watchlist_assigned: b.watchlist_assigned,
                    evidence_review_id: b.evidence_review_id.clone(),
                    evidence_scanner_scan_id: b.evidence_scanner_scan_id.clone(),
                    evidence_artifact_path: b.evidence_artifact_path.clone(),
                    evidence_git_hash: b.evidence_git_hash.clone(),
                    promotion_transition_id: b.promotion_transition_id.clone(),
                    promotion_effective_at: b.promotion_effective_at.clone(),
                    promotion_expires_at: b.promotion_expires_at.clone(),
                    evidence_transition_id: b.evidence_transition_id.clone(),
                    exact_reason: b.exact_reason.clone(),
                },
            )
            .collect();

        Fixture {
            run_id,
            source_identity,
            source_artifact_hash,
            config_fingerprint,
            market_date,
            evaluations,
            header,
            bindings,
        }
    }

    #[test]
    fn valid_fixture_passes() {
        let f = build_fixture();
        assert_eq!(
            validate_explicit_multi_strategy_authority(&f.expected(), &f.header, &f.bindings),
            Ok(())
        );
    }

    /// Same-count changed child evidence: one stored binding's evidence
    /// field is corrupted (row count is unchanged) -- the recomputed
    /// authority_id from stored facts must diverge from the stored column.
    #[test]
    fn same_count_changed_child_evidence_is_caught() {
        let f = build_fixture();
        let mut bindings = f.bindings.clone();
        bindings[0].promotion_effective = !bindings[0].promotion_effective;
        let err = validate_explicit_multi_strategy_authority(&f.expected(), &f.header, &bindings)
            .unwrap_err();
        assert_eq!(
            err,
            ExplicitMultiStrategyEvidenceValidationError::RecomputedAuthorityIdMismatch
        );
    }

    /// Changed source fact: `source_artifact_hash` on the stored header
    /// diverges from what this start attempt expected.
    #[test]
    fn changed_source_fact_is_caught() {
        let f = build_fixture();
        let mut header = f.header.clone();
        header.source_artifact_hash = "different-hash".to_string();
        let err = validate_explicit_multi_strategy_authority(&f.expected(), &header, &f.bindings)
            .unwrap_err();
        assert_eq!(
            err,
            ExplicitMultiStrategyEvidenceValidationError::SourceArtifactHashMismatch
        );
    }

    /// Changed config fact: `config_fingerprint` on the stored header
    /// diverges from what this start attempt expected.
    #[test]
    fn changed_config_fact_is_caught() {
        let f = build_fixture();
        let mut header = f.header.clone();
        header.config_fingerprint = "different-cfg".to_string();
        let err = validate_explicit_multi_strategy_authority(&f.expected(), &header, &f.bindings)
            .unwrap_err();
        assert_eq!(
            err,
            ExplicitMultiStrategyEvidenceValidationError::ConfigFingerprintMismatch
        );
    }

    /// Missing child / extra child: the exact binding set diverges at equal
    /// count (one stored binding's identity does not match any expected
    /// evaluation) -- neither a silently accepted substitution nor a false
    /// pass via a coincidentally-matching count.
    #[test]
    fn missing_and_extra_child_via_identity_swap_is_caught() {
        let f = build_fixture();
        let mut bindings = f.bindings.clone();
        bindings[0].symbol = "TSLA".to_string();
        let err = validate_explicit_multi_strategy_authority(&f.expected(), &f.header, &bindings)
            .unwrap_err();
        assert!(
            matches!(
                err,
                ExplicitMultiStrategyEvidenceValidationError::MissingBinding { .. }
                    | ExplicitMultiStrategyEvidenceValidationError::ExtraBinding { .. }
            ),
            "expected a missing/extra binding error, got {err:?}"
        );
    }

    /// A stored binding row count that disagrees with what this start
    /// attempt expected (a dropped or duplicated row) must fail closed.
    #[test]
    fn binding_count_mismatch_is_caught() {
        let f = build_fixture();
        let mut bindings = f.bindings.clone();
        bindings.pop();
        let err = validate_explicit_multi_strategy_authority(&f.expected(), &f.header, &bindings)
            .unwrap_err();
        assert!(matches!(
            err,
            ExplicitMultiStrategyEvidenceValidationError::BindingCountMismatch { .. }
        ));
    }

    /// Wrong run: the stored header's `run_id` does not match this start
    /// attempt's own `run_id` -- never activated as though it belonged to
    /// this run.
    #[test]
    fn wrong_run_is_caught() {
        let f = build_fixture();
        let mut header = f.header.clone();
        header.run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"a-different-run");
        let err = validate_explicit_multi_strategy_authority(&f.expected(), &header, &f.bindings)
            .unwrap_err();
        assert_eq!(
            err,
            ExplicitMultiStrategyEvidenceValidationError::RunIdMismatch
        );
    }

    /// Identity mismatch: the durable row is internally self-consistent
    /// (its own authority_id matches its own recomputed content) but is not
    /// the authority this start attempt actually resolved.
    #[test]
    fn identity_mismatch_is_caught() {
        let f = build_fixture();
        let mut expected = f.expected();
        expected.authority_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"a-different-authority");
        let err = validate_explicit_multi_strategy_authority(&expected, &f.header, &f.bindings)
            .unwrap_err();
        assert_eq!(
            err,
            ExplicitMultiStrategyEvidenceValidationError::ExpectedAuthorityIdMismatch
        );
    }

    /// IR-3 (independent review): the write path persists `scanner_rank`
    /// (`SelectionCandidateEvidence::scanner_rank: Option<u32>`) as
    /// `Option<i32>` via `.map(|r| r as i32)`
    /// (`multi_strategy_runtime_dispatch::build_new_explicit_multi_strategy_authority`).
    /// `u32 as i32` is a same-width bit-reinterpreting cast: every value in
    /// `0..=i32::MAX` round-trips exactly, and every value in
    /// `i32::MAX+1..=u32::MAX` has its top bit set, so it ALWAYS becomes
    /// negative under that cast -- a two's-complement identity, not a
    /// coincidence of the specific values tested here. This proves the
    /// write-side cast can never silently produce a wrong-but-plausible
    /// positive rank: any value it cannot represent is deterministically
    /// caught by this module's `r >= 0` read-side check and fails closed as
    /// `NegativeScannerRank`, never accepted as valid evidence.
    #[test]
    fn scanner_rank_overflow_is_always_caught_never_silently_accepted() {
        for overflowing in [
            1u32 << 31,
            (i32::MAX as u32) + 1,
            3_000_000_000u32,
            u32::MAX,
        ] {
            let stored_i32 = overflowing as i32;
            assert!(
                stored_i32 < 0,
                "u32 value {overflowing} did not become negative under the production cast; \
                 the write-side conversion could silently corrupt evidence"
            );

            let f = build_fixture();
            let mut binding = f.bindings[0].clone();
            binding.scanner_rank = Some(stored_i32);
            let err = stored_binding_to_evaluation(&binding).unwrap_err();
            assert!(
                matches!(
                    err,
                    ExplicitMultiStrategyEvidenceValidationError::NegativeScannerRank { .. }
                ),
                "expected NegativeScannerRank for overflowed value {overflowing}, got {err:?}"
            );
        }

        // The boundary itself: i32::MAX is the largest u32 value that round-
        // trips exactly through the production cast and must NOT be rejected.
        let f = build_fixture();
        let mut binding = f.bindings[0].clone();
        binding.scanner_rank = Some(i32::MAX);
        let evaluation = stored_binding_to_evaluation(&binding).expect("i32::MAX must round-trip");
        assert_eq!(evaluation.evidence.scanner_rank, Some(i32::MAX as u32));
    }
}
