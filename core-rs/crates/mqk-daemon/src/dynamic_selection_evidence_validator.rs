//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 7C Part 3 — the one read-side
//! evidence validator/read model, shared by the pre-activation write-gate
//! (Part 2), the operator API (Part 4), the GUI (Part 5), guards, and soak
//! validation.
//!
//! Loads durable evidence, reconstructs the exact
//! `mqk_portfolio::dynamic_selection::DynamicSelectionPlan` the header and
//! candidate/symbol rows describe, and recomputes `plan_id` through the same
//! shared canonical helper the write path used
//! (`crate::dynamic_selection_dispatch_authority::derive_dynamic_selection_plan_id`)
//! -- never trusting the stored `plan_id` column on its own. Never
//! synthesizes evidence from the current environment and never repairs on
//! read: any incoherence is reported, never silently corrected.

use crate::dynamic_selection_dispatch_authority::{
    derive_dynamic_selection_plan_id, timeframe_secs_to_db_label,
};
use mqk_db::{BoundedDynamicSelectionPlanFetch, DynamicSelectionPlanCandidateRecord};
use mqk_portfolio::{
    DynamicSelectionContext, DynamicSelectionMode, DynamicSelectionPlan, ExactSelectionReason,
    SelectionCandidateDisposition, SelectionCandidateResult, SymbolSelectionResult,
};
use sqlx::PgPool;
use uuid::Uuid;

/// Single shared read-side candidate bound, mirroring
/// `mqk_db::DYNAMIC_SELECTION_CANDIDATE_READ_BOUND` so the SQL fetch bound
/// and this validator's own bound can never silently drift apart.
pub(crate) const DYNAMIC_SELECTION_EVIDENCE_READ_BOUND: usize =
    mqk_db::DYNAMIC_SELECTION_CANDIDATE_READ_BOUND;

/// Bounded typed outcome. `Valid` is the only state PaperEnforced soak
/// readiness may accept.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DynamicSelectionEvidenceValidationState {
    Valid,
    Missing,
    Incomplete {
        detail: String,
    },
    IdentityMismatch {
        stored_plan_id: Uuid,
        recomputed_plan_id: Uuid,
    },
    RunMismatch {
        expected_run_id: Uuid,
        stored_run_id: Uuid,
    },
    CandidateMismatch {
        detail: String,
    },
    RuntimeMismatch {
        detail: String,
    },
    LiveApprovalViolation,
}

impl DynamicSelectionEvidenceValidationState {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Missing => "missing",
            Self::Incomplete { .. } => "incomplete",
            Self::IdentityMismatch { .. } => "identity_mismatch",
            Self::RunMismatch { .. } => "run_mismatch",
            Self::CandidateMismatch { .. } => "candidate_mismatch",
            Self::RuntimeMismatch { .. } => "runtime_mismatch",
            Self::LiveApprovalViolation => "live_approval_violation",
        }
    }

    pub(crate) fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }
}

fn parse_candidate_disposition(raw: &str) -> Option<SelectionCandidateDisposition> {
    match raw {
        "selected" => Some(SelectionCandidateDisposition::Selected),
        "not_selected" => Some(SelectionCandidateDisposition::NotSelected),
        "refused" => Some(SelectionCandidateDisposition::Refused),
        _ => None,
    }
}

fn candidate_record_to_result(
    rec: &DynamicSelectionPlanCandidateRecord,
) -> Result<SelectionCandidateResult, String> {
    let disposition = parse_candidate_disposition(&rec.disposition)
        .ok_or_else(|| format!("unrecognized candidate disposition {:?}", rec.disposition))?;
    let exact_reason = ExactSelectionReason::parse_code(&rec.exact_reason_code)
        .ok_or_else(|| format!("unrecognized exact_reason_code {:?}", rec.exact_reason_code))?;
    let scanner_rank = rec
        .scanner_rank
        .map(u32::try_from)
        .transpose()
        .map_err(|_| "scanner_rank does not fit in u32".to_string())?;

    Ok(SelectionCandidateResult {
        symbol: rec.symbol.clone(),
        strategy_id: rec.strategy_id.clone(),
        timeframe_secs: rec.timeframe_secs,
        canonical_score_decimal: rec.canonical_score_decimal.clone(),
        canonical_score_micros: rec.canonical_score_micros,
        scanner_rank,
        watchlist_assigned: rec.watchlist_assigned,
        promotion_query_ok: rec.promotion_query_ok,
        promotion_state: rec.promotion_state.clone(),
        promotion_effective: rec.promotion_effective,
        promotion_expired: rec.promotion_expired,
        evidence_resolved: rec.evidence_resolved,
        review_state_is_paper_candidate: rec.review_state_is_paper_candidate,
        evidence_review_state: rec.evidence_review_state.clone(),
        durable_legacy_fingerprint: rec.durable_legacy_fingerprint.clone(),
        recomputed_legacy_fingerprint: rec.recomputed_legacy_fingerprint.clone(),
        legacy_fingerprint_matches: rec.legacy_fingerprint_matches,
        durable_exact_fingerprint_v2: rec.durable_exact_fingerprint_v2.clone(),
        recomputed_exact_fingerprint_v2: rec.recomputed_exact_fingerprint_v2.clone(),
        exact_fingerprint_v2_matches: rec.exact_fingerprint_v2_matches,
        registry_enabled: rec.registry_enabled,
        plugin_instantiable: rec.plugin_instantiable,
        timeframe_matches: rec.timeframe_matches,
        data_ready: rec.data_ready,
        evidence_review_id: rec.evidence_review_id.clone(),
        evidence_scanner_scan_id: rec.evidence_scanner_scan_id.clone(),
        evidence_artifact_path: rec.evidence_artifact_path.clone(),
        evidence_git_hash: rec.evidence_git_hash.clone(),
        promotion_transition_id: rec.promotion_transition_id.clone(),
        promotion_effective_at: rec.promotion_effective_at.clone(),
        promotion_expires_at: rec.promotion_expires_at.clone(),
        evidence_transition_id: rec.evidence_transition_id.clone(),
        exact_reason,
        selected: rec.selected,
        disposition,
        reason_code: rec.reason_code.clone(),
    })
}

/// Reconstruct the exact `DynamicSelectionPlan` a fetched header/symbol/
/// candidate row set describes. `Err` on any unrecognized closed-vocabulary
/// value -- never a guessed/defaulted reconstruction.
fn reconstruct_plan(
    header: &mqk_db::DynamicSelectionPlanRecord,
    symbols: &[mqk_db::DynamicSelectionPlanSymbolRecord],
    candidates: &[DynamicSelectionPlanCandidateRecord],
) -> Result<DynamicSelectionPlan, String> {
    let configured_mode = DynamicSelectionMode::parse(&header.configured_mode)
        .ok_or_else(|| format!("unrecognized configured_mode {:?}", header.configured_mode))?;
    let effective_mode = DynamicSelectionMode::parse(&header.effective_mode)
        .ok_or_else(|| format!("unrecognized effective_mode {:?}", header.effective_mode))?;

    let context = DynamicSelectionContext {
        run_id: header.run_id.to_string(),
        schema_version: header.context_schema_version.clone(),
        configured_mode,
        effective_mode,
        live_lock_applied: header.live_lock_applied,
        source_kind: header.source_kind.clone(),
        source_identity: header.source_identity.clone(),
        market_date: header.market_date.clone(),
    };

    let mut symbol_results = Vec::with_capacity(symbols.len());
    for s in symbols {
        let disposition = parse_candidate_disposition(&s.disposition)
            .ok_or_else(|| format!("unrecognized symbol disposition {:?}", s.disposition))?;
        let exact_reason =
            ExactSelectionReason::parse_code(&s.exact_reason_code).ok_or_else(|| {
                format!(
                    "unrecognized symbol exact_reason_code {:?}",
                    s.exact_reason_code
                )
            })?;

        let mut symbol_candidates: Vec<SelectionCandidateResult> = candidates
            .iter()
            .filter(|c| c.symbol == s.symbol)
            .map(candidate_record_to_result)
            .collect::<Result<Vec<_>, String>>()?;
        symbol_candidates.sort_by(|a, b| {
            (&a.strategy_id, a.timeframe_secs).cmp(&(&b.strategy_id, b.timeframe_secs))
        });

        symbol_results.push(SymbolSelectionResult {
            symbol: s.symbol.clone(),
            selected_strategy_id: s.selected_strategy_id.clone(),
            disposition,
            reason_code: s.reason_code.clone(),
            exact_reason,
            candidates: symbol_candidates,
        });
    }
    symbol_results.sort_by(|a, b| a.symbol.cmp(&b.symbol));

    let reconstructed_candidate_total: usize =
        symbol_results.iter().map(|s| s.candidates.len()).sum();
    if reconstructed_candidate_total != candidates.len() {
        return Err(format!(
            "candidate row referenced an unknown symbol: {} candidate rows fetched, only {} \
             attributable to a symbol row",
            candidates.len(),
            reconstructed_candidate_total
        ));
    }

    Ok(DynamicSelectionPlan {
        context,
        symbol_results,
        truth_state: header.truth_state.clone(),
        blockers: header.blockers.clone(),
    })
}

/// The one shared plan-ID recomputation authority for Phase 7C: reconstructs
/// the plan from durable rows and recomputes its identity through the exact
/// same helper the write path used. Exposed separately from the full
/// validator so guards/tests can exercise recomputation in isolation.
pub(crate) fn recompute_plan_id_from_reconstruction(
    header: &mqk_db::DynamicSelectionPlanRecord,
    symbols: &[mqk_db::DynamicSelectionPlanSymbolRecord],
    candidates: &[DynamicSelectionPlanCandidateRecord],
) -> Result<(DynamicSelectionPlan, Uuid), String> {
    let plan = reconstruct_plan(header, symbols, candidates)?;
    let recomputed = derive_dynamic_selection_plan_id(&plan);
    Ok((plan, recomputed))
}

/// Returns the first duplicate exact `(symbol, strategy_id, timeframe_secs)`
/// candidate key found, if any -- pure, no I/O, unit-testable independent of
/// a DB round trip. `None` means every candidate key is unique.
pub(crate) fn find_duplicate_candidate_key(
    candidates: &[DynamicSelectionPlanCandidateRecord],
) -> Option<(String, String, i64)> {
    let mut seen = std::collections::HashSet::with_capacity(candidates.len());
    for c in candidates {
        let key = (c.symbol.clone(), c.strategy_id.clone(), c.timeframe_secs);
        if !seen.insert(key.clone()) {
            return Some(key);
        }
    }
    None
}

/// Checks the stored header `disposition` against `effective_mode` and, for
/// an Allowed disposition, against the exact facts
/// `evaluate_plan_for_start_gate`/`evaluate_shadow_plan_outcome`
/// (`dynamic_selection_start_gate.rs`) require before ever returning it:
/// `PaperEnforcedAllowed` and `ShadowAllowed` are only ever returned when
/// `selected_count() > 0`, and `ShadowAllowed` only when
/// `truth_state == "computed"`. A stored row whose disposition claims an
/// Allowed outcome the stored counts/truth_state could never have produced
/// is evidence tampering or a write-path defect, never a legitimate state.
/// `Some(detail)` on incoherence, `None` when coherent. Pure, no I/O.
pub(crate) fn disposition_coherence_violation(
    header: &mqk_db::DynamicSelectionPlanRecord,
) -> Option<String> {
    use crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition as StartGateDisposition;

    let Some(parsed_disposition) = StartGateDisposition::parse(&header.disposition) else {
        return Some(format!("unrecognized disposition {:?}", header.disposition));
    };

    let disposition_mode_coherent = matches!(
        (header.effective_mode.as_str(), parsed_disposition),
        (
            "shadow",
            StartGateDisposition::ShadowAllowed | StartGateDisposition::ShadowInvalid
        ) | (
            "paper_enforced",
            StartGateDisposition::PaperEnforcedAllowed | StartGateDisposition::PaperEnforcedRefused
        )
    );
    if !disposition_mode_coherent {
        return Some(format!(
            "disposition {:?} is incoherent with effective_mode {:?}",
            header.disposition, header.effective_mode
        ));
    }

    let requires_positive_selection = matches!(
        parsed_disposition,
        StartGateDisposition::ShadowAllowed | StartGateDisposition::PaperEnforcedAllowed
    );
    if requires_positive_selection && header.selected_count <= 0 {
        return Some(format!(
            "disposition {:?} requires selected_count > 0, but header.selected_count={}",
            header.disposition, header.selected_count
        ));
    }

    if matches!(parsed_disposition, StartGateDisposition::ShadowAllowed)
        && header.truth_state != "computed"
    {
        return Some(format!(
            "disposition shadow_allowed requires truth_state=\"computed\", but \
             header.truth_state={:?}",
            header.truth_state
        ));
    }

    None
}

/// Full read-side validation: load, reconstruct, recompute identity, and
/// check every coherence invariant. `expected_run_id` is the run this
/// evidence must belong to. `expected_bindings` (symbol, strategy_id,
/// timeframe_secs), when `Some`, is the committed runtime's own selected
/// bindings (from `RuntimeStrategyDispatchAuthority::DynamicPaperEnforced` or
/// `DynamicSelectionRuntimeState`) -- `None` when no runtime binding exists
/// yet to compare against (e.g. validating before activation, or a purely
/// historical read with no live run).
pub(crate) async fn validate_dynamic_selection_evidence(
    pool: &PgPool,
    plan_id: Uuid,
    expected_run_id: Uuid,
    expected_bindings: Option<&[(String, String, i64)]>,
) -> DynamicSelectionEvidenceValidationState {
    let fetch = match mqk_db::fetch_dynamic_selection_plan_for_read(
        pool,
        plan_id,
        DYNAMIC_SELECTION_EVIDENCE_READ_BOUND,
    )
    .await
    {
        Ok(f) => f,
        Err(e) => {
            return DynamicSelectionEvidenceValidationState::Incomplete {
                detail: format!("read failed: {e}"),
            }
        }
    };

    let (header, symbols, candidates) = match fetch {
        BoundedDynamicSelectionPlanFetch::NotFound => {
            return DynamicSelectionEvidenceValidationState::Missing;
        }
        BoundedDynamicSelectionPlanFetch::CandidateLimitExceeded {
            observed_at_least, ..
        } => {
            return DynamicSelectionEvidenceValidationState::Incomplete {
                detail: format!(
                    "candidate count exceeds the read bound ({DYNAMIC_SELECTION_EVIDENCE_READ_BOUND}); \
                     observed at least {observed_at_least} rows"
                ),
            };
        }
        BoundedDynamicSelectionPlanFetch::Complete(header, symbols, candidates) => {
            (header, symbols, candidates)
        }
    };

    if header.approved_for_live {
        return DynamicSelectionEvidenceValidationState::LiveApprovalViolation;
    }

    // Phase 7C Part 7: duplicate exact candidate key detection. The pure
    // model layer trusts its caller to have already deduplicated candidates
    // by (symbol, strategy_id, timeframe_secs) -- see
    // mqk_portfolio::dynamic_selection's own doc comment on
    // MAX_CANDIDATE_PAIRS -- but does not itself re-verify that. Migration
    // 0059's only uniqueness constraint was (plan_id, ordinal); migration
    // 0060 now also constrains this exact key at the DB level. This is the
    // read-side defense-in-depth check: never trust the DB constraint alone
    // to be the sole guard against a corrupted/pre-migration row.
    if let Some((symbol, strategy_id, timeframe_secs)) = find_duplicate_candidate_key(&candidates) {
        return DynamicSelectionEvidenceValidationState::CandidateMismatch {
            detail: format!(
                "duplicate candidate key (symbol={symbol}, strategy_id={strategy_id}, \
                 timeframe_secs={timeframe_secs}) present more than once for plan_id={plan_id}"
            ),
        };
    }

    // Phase 7C Part 7: header disposition must cohere with effective_mode,
    // and an Allowed disposition must cohere with the exact facts
    // `evaluate_plan_for_start_gate`/`evaluate_shadow_plan_outcome`
    // (dynamic_selection_start_gate.rs) require to ever return it.
    if let Some(detail) = disposition_coherence_violation(&header) {
        return DynamicSelectionEvidenceValidationState::CandidateMismatch { detail };
    }

    if header.run_id != expected_run_id {
        return DynamicSelectionEvidenceValidationState::RunMismatch {
            expected_run_id,
            stored_run_id: header.run_id,
        };
    }

    if symbols.len() != header.symbol_count as usize {
        return DynamicSelectionEvidenceValidationState::CandidateMismatch {
            detail: format!(
                "header.symbol_count={} but {} symbol rows were fetched",
                header.symbol_count,
                symbols.len()
            ),
        };
    }
    if candidates.len() != header.candidate_count as usize {
        return DynamicSelectionEvidenceValidationState::CandidateMismatch {
            detail: format!(
                "header.candidate_count={} but {} candidate rows were fetched",
                header.candidate_count,
                candidates.len()
            ),
        };
    }

    let (plan, recomputed_plan_id) =
        match recompute_plan_id_from_reconstruction(&header, &symbols, &candidates) {
            Ok(v) => v,
            Err(detail) => {
                return DynamicSelectionEvidenceValidationState::CandidateMismatch { detail }
            }
        };

    if plan.selected_count() != header.selected_count as usize {
        return DynamicSelectionEvidenceValidationState::CandidateMismatch {
            detail: format!(
                "header.selected_count={} but reconstruction computed {}",
                header.selected_count,
                plan.selected_count()
            ),
        };
    }
    if plan.refused_count() != header.refused_count as usize {
        return DynamicSelectionEvidenceValidationState::CandidateMismatch {
            detail: format!(
                "header.refused_count={} but reconstruction computed {}",
                header.refused_count,
                plan.refused_count()
            ),
        };
    }

    if recomputed_plan_id != plan_id {
        return DynamicSelectionEvidenceValidationState::IdentityMismatch {
            stored_plan_id: plan_id,
            recomputed_plan_id,
        };
    }

    if let Some(expected) = expected_bindings {
        let mut reconstructed: Vec<(String, String, i64)> = plan
            .symbol_results
            .iter()
            .filter_map(|sr| {
                let strategy_id = sr.selected_strategy_id.clone()?;
                let c = sr.candidates.iter().find(|c| c.selected)?;
                Some((sr.symbol.clone(), strategy_id, c.timeframe_secs))
            })
            .collect();
        reconstructed.sort();
        let mut expected_sorted = expected.to_vec();
        expected_sorted.sort();
        if reconstructed != expected_sorted {
            return DynamicSelectionEvidenceValidationState::RuntimeMismatch {
                detail: format!(
                    "reconstructed selected bindings {reconstructed:?} do not match the \
                     committed runtime bindings {expected_sorted:?}"
                ),
            };
        }
    }

    DynamicSelectionEvidenceValidationState::Valid
}

/// Verify that every `strategy_signal_evaluations` row for `run_id` whose
/// `strategy_id` matches a selected binding's `strategy_id` carries exactly
/// one of that strategy_id's selected `(symbol, timeframe)` bindings.
///
/// Builds the exact allowed `(symbol, strategy_id, db_timeframe_label)`
/// tuple set up front and checks set membership -- never the prior
/// per-binding nested comparison, which required a row to match *every*
/// binding sharing its `strategy_id` rather than *one* of them. That
/// required-all-not-any shape falsely rejected a legitimate same-strategy
/// plan selected for more than one symbol or timeframe: a row correctly
/// attributed to binding A (e.g. `(AAPL, swing_momentum, 300)`) was also
/// checked against binding B (e.g. `(MSFT, swing_momentum, 300)`) merely
/// because A and B share `strategy_id = swing_momentum`, and rejected for
/// not being MSFT.
///
/// Rows whose `strategy_id` is not present in `selected_bindings` at all are
/// skipped -- this validator only ever proves attribution for the
/// selected-host bindings it was given, never asserts anything about a
/// strategy_id outside that set. (The schema does not currently carry a
/// `source`/`decision_stage` tag that distinguishes selected-host dispatch
/// from legacy single-strategy dispatch at the row level -- both write
/// through the same `strategy_signal_evaluations` insert path with
/// `source = "mqk-daemon.execution_loop"` regardless of which dispatch
/// authority drove the tick -- so strategy_id-set membership is the only
/// evidence-backed boundary this function can assert; it does not claim a
/// distinction the data does not carry.)
///
/// An empty result is authoritative: no contradicting row exists, not that
/// none was checked.
pub(crate) async fn verify_signal_journal_attribution(
    pool: &PgPool,
    run_id: Uuid,
    selected_bindings: &[(String, String, i64)],
    limit: i64,
) -> Result<(), String> {
    let rows = mqk_db::fetch_strategy_signal_evaluations_for_run(pool, run_id, limit)
        .await
        .map_err(|e| format!("signal journal fetch failed: {e}"))?;
    find_signal_journal_attribution_violation(&rows, selected_bindings)
}

/// Pure matching core of [`verify_signal_journal_attribution`], separated
/// out so the exact-tuple-membership logic is unit-testable against
/// hand-built rows without a DB round trip.
pub(crate) fn find_signal_journal_attribution_violation(
    rows: &[mqk_db::StrategySignalEvaluationRecord],
    selected_bindings: &[(String, String, i64)],
) -> Result<(), String> {
    let mut allowed: std::collections::HashSet<(String, String, String)> =
        std::collections::HashSet::with_capacity(selected_bindings.len());
    let mut strategy_ids_selected: std::collections::HashSet<&str> =
        std::collections::HashSet::with_capacity(selected_bindings.len());
    for (symbol, strategy_id, timeframe_secs) in selected_bindings {
        let label = timeframe_secs_to_db_label(*timeframe_secs)
            .ok_or_else(|| format!("no db label for timeframe_secs={timeframe_secs}"))?;
        allowed.insert((symbol.clone(), strategy_id.clone(), label.to_string()));
        strategy_ids_selected.insert(strategy_id.as_str());
    }

    for row in rows {
        if !strategy_ids_selected.contains(row.strategy_id.as_str()) {
            continue;
        }
        let key = (
            row.symbol.clone(),
            row.strategy_id.clone(),
            row.timeframe.clone(),
        );
        if !allowed.contains(&key) {
            return Err(format!(
                "signal-evaluation row evaluation_id={} for run_id={:?} strategy_id={} \
                 carries symbol={:?} timeframe={:?}, which matches none of the selected \
                 bindings for that strategy_id",
                row.evaluation_id, row.run_id, row.strategy_id, row.symbol, row.timeframe
            ));
        }
    }

    Ok(())
}

/// Build a [`SelectionCandidateInput`]-shaped fixture is intentionally *not*
/// provided here -- this module only ever reconstructs from durable rows,
/// never builds a plan the way the selector itself does. (Kept as a doc
/// note, not code: a stray helper here would blur that boundary.)
#[cfg(test)]
mod tests {
    use super::*;
    use mqk_portfolio::SelectionCandidateEvidence;

    fn sample_evidence() -> SelectionCandidateEvidence {
        SelectionCandidateEvidence {
            promotion_query_ok: true,
            promotion_state: Some("active_paper".to_string()),
            promotion_effective: true,
            promotion_expired: false,
            evidence_resolved: true,
            review_state_is_paper_candidate: true,
            evidence_review_state: Some("paper_candidate".to_string()),
            durable_legacy_fingerprint: Some("a".repeat(64)),
            recomputed_legacy_fingerprint: Some("a".repeat(64)),
            legacy_fingerprint_matches: true,
            durable_exact_fingerprint_v2: Some("b".repeat(64)),
            recomputed_exact_fingerprint_v2: Some("b".repeat(64)),
            exact_fingerprint_v2_matches: true,
            registry_enabled: true,
            plugin_instantiable: true,
            timeframe_matches: true,
            data_ready: true,
            canonical_score_decimal: Some("1.000000".to_string()),
            canonical_score_micros: Some(1_000_000),
            scanner_rank: Some(1),
            watchlist_assigned: true,
            evidence_review_id: Some("review-1".to_string()),
            evidence_scanner_scan_id: Some("scan-1".to_string()),
            evidence_artifact_path: Some("/artifacts/review-1".to_string()),
            evidence_git_hash: Some("git-hash-1".to_string()),
            promotion_transition_id: Some("11111111-1111-1111-1111-111111111111".to_string()),
            promotion_effective_at: Some("2026-01-01T00:00:00Z".to_string()),
            promotion_expires_at: None,
            evidence_transition_id: Some("11111111-1111-1111-1111-111111111111".to_string()),
            exact_reason: None,
        }
    }

    /// Reconstruction round-trips a hand-built plan through the write
    /// mapper (`crate::dynamic_selection_evidence_writer`) and back through
    /// this module's `reconstruct_plan`, without touching a database --
    /// proves the two mappers agree on every field in isolation from the DB
    /// layer's own round-trip proof (which lives in mqk-db's scenario test).
    #[test]
    fn reconstruction_recomputes_the_same_plan_id_as_the_original_build() {
        use mqk_portfolio::canonical_plan_identity_material;

        let context = DynamicSelectionContext {
            run_id: "11111111-1111-1111-1111-111111111111".to_string(),
            schema_version: mqk_portfolio::DYNAMIC_SELECTION_SCHEMA_VERSION.to_string(),
            configured_mode: DynamicSelectionMode::PaperEnforced,
            effective_mode: DynamicSelectionMode::PaperEnforced,
            live_lock_applied: false,
            source_kind: "watchlist_v2".to_string(),
            source_identity: "watchlist".to_string(),
            market_date: "2026-07-28".to_string(),
        };

        let evidence = sample_evidence();
        let candidate = SelectionCandidateResult {
            symbol: "AAPL".to_string(),
            strategy_id: "swing_momentum".to_string(),
            timeframe_secs: 300,
            canonical_score_decimal: evidence.canonical_score_decimal.clone(),
            canonical_score_micros: evidence.canonical_score_micros,
            scanner_rank: evidence.scanner_rank,
            watchlist_assigned: evidence.watchlist_assigned,
            promotion_query_ok: evidence.promotion_query_ok,
            promotion_state: evidence.promotion_state.clone(),
            promotion_effective: evidence.promotion_effective,
            promotion_expired: evidence.promotion_expired,
            evidence_resolved: evidence.evidence_resolved,
            review_state_is_paper_candidate: evidence.review_state_is_paper_candidate,
            evidence_review_state: evidence.evidence_review_state.clone(),
            durable_legacy_fingerprint: evidence.durable_legacy_fingerprint.clone(),
            recomputed_legacy_fingerprint: evidence.recomputed_legacy_fingerprint.clone(),
            legacy_fingerprint_matches: evidence.legacy_fingerprint_matches,
            durable_exact_fingerprint_v2: evidence.durable_exact_fingerprint_v2.clone(),
            recomputed_exact_fingerprint_v2: evidence.recomputed_exact_fingerprint_v2.clone(),
            exact_fingerprint_v2_matches: evidence.exact_fingerprint_v2_matches,
            registry_enabled: evidence.registry_enabled,
            plugin_instantiable: evidence.plugin_instantiable,
            timeframe_matches: evidence.timeframe_matches,
            data_ready: evidence.data_ready,
            evidence_review_id: evidence.evidence_review_id.clone(),
            evidence_scanner_scan_id: evidence.evidence_scanner_scan_id.clone(),
            evidence_artifact_path: evidence.evidence_artifact_path.clone(),
            evidence_git_hash: evidence.evidence_git_hash.clone(),
            promotion_transition_id: evidence.promotion_transition_id.clone(),
            promotion_effective_at: evidence.promotion_effective_at.clone(),
            promotion_expires_at: evidence.promotion_expires_at.clone(),
            evidence_transition_id: evidence.evidence_transition_id.clone(),
            exact_reason: ExactSelectionReason::SelectedHighestScore,
            selected: true,
            disposition: SelectionCandidateDisposition::Selected,
            reason_code: "selected_highest_score".to_string(),
        };
        let symbol_result = SymbolSelectionResult {
            symbol: "AAPL".to_string(),
            selected_strategy_id: Some("swing_momentum".to_string()),
            disposition: SelectionCandidateDisposition::Selected,
            reason_code: "selected_highest_score".to_string(),
            exact_reason: ExactSelectionReason::SelectedHighestScore,
            candidates: vec![candidate],
        };

        let plan = DynamicSelectionPlan {
            context,
            symbol_results: vec![symbol_result],
            truth_state: "computed".to_string(),
            blockers: vec![],
        };

        let original_plan_id = derive_dynamic_selection_plan_id(&plan);

        let run_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let new_plan = crate::dynamic_selection_evidence_writer::build_new_dynamic_selection_plan(
            &plan,
            original_plan_id,
            run_id,
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::PaperEnforcedAllowed,
            chrono::Utc::now(),
        )
        .expect("build should succeed");

        let header = mqk_db::DynamicSelectionPlanRecord {
            plan_id: new_plan.plan_id,
            run_id: new_plan.run_id,
            library_schema_version: new_plan.library_schema_version.clone(),
            context_schema_version: new_plan.context_schema_version.clone(),
            configured_mode: new_plan.configured_mode.clone(),
            effective_mode: new_plan.effective_mode.clone(),
            live_lock_applied: new_plan.live_lock_applied,
            approved_for_live: new_plan.approved_for_live,
            source_kind: new_plan.source_kind.clone(),
            source_identity: new_plan.source_identity.clone(),
            market_date: new_plan.market_date.clone(),
            disposition: new_plan.disposition.clone(),
            truth_state: new_plan.truth_state.clone(),
            blockers: new_plan.blockers.clone(),
            symbol_count: new_plan.symbol_count,
            candidate_count: new_plan.candidate_count,
            selected_count: new_plan.selected_count,
            refused_count: new_plan.refused_count,
            writer_version: new_plan.writer_version.clone(),
            created_at_utc: new_plan.created_at_utc,
        };
        let symbol_records: Vec<mqk_db::DynamicSelectionPlanSymbolRecord> = new_plan
            .symbols
            .iter()
            .map(|s| mqk_db::DynamicSelectionPlanSymbolRecord {
                plan_id: new_plan.plan_id,
                symbol: s.symbol.clone(),
                selected_strategy_id: s.selected_strategy_id.clone(),
                disposition: s.disposition.clone(),
                reason_code: s.reason_code.clone(),
                exact_reason_code: s.exact_reason_code.clone(),
            })
            .collect();
        let candidate_records: Vec<DynamicSelectionPlanCandidateRecord> = new_plan
            .candidates
            .iter()
            .enumerate()
            .map(|(i, c)| DynamicSelectionPlanCandidateRecord {
                plan_id: new_plan.plan_id,
                ordinal: i as i32,
                symbol: c.symbol.clone(),
                strategy_id: c.strategy_id.clone(),
                timeframe_secs: c.timeframe_secs,
                canonical_score_decimal: c.canonical_score_decimal.clone(),
                canonical_score_micros: c.canonical_score_micros,
                scanner_rank: c.scanner_rank,
                watchlist_assigned: c.watchlist_assigned,
                promotion_query_ok: c.promotion_query_ok,
                promotion_state: c.promotion_state.clone(),
                promotion_effective: c.promotion_effective,
                promotion_expired: c.promotion_expired,
                evidence_resolved: c.evidence_resolved,
                review_state_is_paper_candidate: c.review_state_is_paper_candidate,
                evidence_review_state: c.evidence_review_state.clone(),
                durable_legacy_fingerprint: c.durable_legacy_fingerprint.clone(),
                recomputed_legacy_fingerprint: c.recomputed_legacy_fingerprint.clone(),
                legacy_fingerprint_matches: c.legacy_fingerprint_matches,
                durable_exact_fingerprint_v2: c.durable_exact_fingerprint_v2.clone(),
                recomputed_exact_fingerprint_v2: c.recomputed_exact_fingerprint_v2.clone(),
                exact_fingerprint_v2_matches: c.exact_fingerprint_v2_matches,
                registry_enabled: c.registry_enabled,
                plugin_instantiable: c.plugin_instantiable,
                timeframe_matches: c.timeframe_matches,
                data_ready: c.data_ready,
                evidence_review_id: c.evidence_review_id.clone(),
                evidence_scanner_scan_id: c.evidence_scanner_scan_id.clone(),
                evidence_artifact_path: c.evidence_artifact_path.clone(),
                evidence_git_hash: c.evidence_git_hash.clone(),
                promotion_transition_id: c.promotion_transition_id.clone(),
                promotion_effective_at: c.promotion_effective_at.clone(),
                promotion_expires_at: c.promotion_expires_at.clone(),
                evidence_transition_id: c.evidence_transition_id.clone(),
                exact_reason_code: c.exact_reason_code.clone(),
                selected: c.selected,
                disposition: c.disposition.clone(),
                reason_code: c.reason_code.clone(),
            })
            .collect();

        let (reconstructed, recomputed_id) =
            recompute_plan_id_from_reconstruction(&header, &symbol_records, &candidate_records)
                .expect("reconstruction should succeed");

        assert_eq!(recomputed_id, original_plan_id);
        assert_eq!(
            canonical_plan_identity_material(&reconstructed),
            canonical_plan_identity_material(&plan)
        );
    }

    #[test]
    fn unrecognized_disposition_fails_closed() {
        assert_eq!(parse_candidate_disposition("bogus"), None);
    }

    // -----------------------------------------------------------------
    // find_duplicate_candidate_key (Part 7)
    // -----------------------------------------------------------------

    fn sample_candidate_record(
        symbol: &str,
        strategy_id: &str,
        timeframe_secs: i64,
    ) -> DynamicSelectionPlanCandidateRecord {
        DynamicSelectionPlanCandidateRecord {
            plan_id: Uuid::nil(),
            ordinal: 0,
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe_secs,
            canonical_score_decimal: None,
            canonical_score_micros: None,
            scanner_rank: None,
            watchlist_assigned: true,
            promotion_query_ok: true,
            promotion_state: None,
            promotion_effective: true,
            promotion_expired: false,
            evidence_resolved: true,
            review_state_is_paper_candidate: true,
            evidence_review_state: None,
            durable_legacy_fingerprint: None,
            recomputed_legacy_fingerprint: None,
            legacy_fingerprint_matches: true,
            durable_exact_fingerprint_v2: None,
            recomputed_exact_fingerprint_v2: None,
            exact_fingerprint_v2_matches: true,
            registry_enabled: true,
            plugin_instantiable: true,
            timeframe_matches: true,
            data_ready: true,
            evidence_review_id: None,
            evidence_scanner_scan_id: None,
            evidence_artifact_path: None,
            evidence_git_hash: None,
            promotion_transition_id: None,
            promotion_effective_at: None,
            promotion_expires_at: None,
            evidence_transition_id: None,
            exact_reason_code: "selected_highest_score".to_string(),
            selected: true,
            disposition: "selected".to_string(),
            reason_code: "selected_highest_score".to_string(),
        }
    }

    #[test]
    fn find_duplicate_candidate_key_is_none_for_distinct_keys() {
        let candidates = vec![
            sample_candidate_record("AAPL", "swing_momentum", 300),
            sample_candidate_record("MSFT", "swing_momentum", 300),
            sample_candidate_record("AAPL", "swing_momentum", 900),
        ];
        assert_eq!(find_duplicate_candidate_key(&candidates), None);
    }

    #[test]
    fn find_duplicate_candidate_key_detects_an_exact_repeat_under_a_different_ordinal() {
        let mut second = sample_candidate_record("AAPL", "swing_momentum", 300);
        second.ordinal = 1;
        let candidates = vec![
            sample_candidate_record("AAPL", "swing_momentum", 300),
            second,
        ];
        assert_eq!(
            find_duplicate_candidate_key(&candidates),
            Some(("AAPL".to_string(), "swing_momentum".to_string(), 300))
        );
    }

    // -----------------------------------------------------------------
    // disposition_coherence_violation (Part 7)
    // -----------------------------------------------------------------

    fn sample_header(
        disposition: &str,
        effective_mode: &str,
        selected_count: i32,
        truth_state: &str,
    ) -> mqk_db::DynamicSelectionPlanRecord {
        mqk_db::DynamicSelectionPlanRecord {
            plan_id: Uuid::nil(),
            run_id: Uuid::nil(),
            library_schema_version: "dynamic-strategy-symbol-selection-v1".to_string(),
            context_schema_version: "dynamic-strategy-symbol-selection-v1".to_string(),
            configured_mode: effective_mode.to_string(),
            effective_mode: effective_mode.to_string(),
            live_lock_applied: false,
            approved_for_live: false,
            source_kind: "watchlist_v2".to_string(),
            source_identity: "watchlist".to_string(),
            market_date: "2026-08-01".to_string(),
            disposition: disposition.to_string(),
            truth_state: truth_state.to_string(),
            blockers: vec![],
            symbol_count: 1,
            candidate_count: 1,
            selected_count,
            refused_count: 0,
            writer_version: "test".to_string(),
            created_at_utc: chrono::Utc::now(),
        }
    }

    #[test]
    fn disposition_coherence_accepts_a_genuine_paper_enforced_allowed_header() {
        let header = sample_header("paper_enforced_allowed", "paper_enforced", 1, "computed");
        assert_eq!(disposition_coherence_violation(&header), None);
    }

    #[test]
    fn disposition_coherence_accepts_a_genuine_shadow_allowed_header() {
        let header = sample_header("shadow_allowed", "shadow", 1, "computed");
        assert_eq!(disposition_coherence_violation(&header), None);
    }

    #[test]
    fn disposition_coherence_accepts_paper_enforced_refused_with_zero_selected() {
        let header = sample_header("paper_enforced_refused", "paper_enforced", 0, "computed");
        assert_eq!(disposition_coherence_violation(&header), None);
    }

    #[test]
    fn disposition_coherence_rejects_paper_enforced_allowed_with_zero_selected_count() {
        let header = sample_header("paper_enforced_allowed", "paper_enforced", 0, "computed");
        assert!(disposition_coherence_violation(&header).is_some());
    }

    #[test]
    fn disposition_coherence_rejects_shadow_allowed_with_non_computed_truth_state() {
        let header = sample_header(
            "shadow_allowed",
            "shadow",
            1,
            "fail_closed_no_eligible_symbols",
        );
        assert!(disposition_coherence_violation(&header).is_some());
    }

    #[test]
    fn disposition_coherence_rejects_a_mode_disposition_family_mismatch() {
        // shadow effective_mode paired with a paper_enforced_* disposition
        // can never come from a genuine write -- must be rejected.
        let header = sample_header("paper_enforced_allowed", "shadow", 1, "computed");
        assert!(disposition_coherence_violation(&header).is_some());
    }

    #[test]
    fn disposition_coherence_rejects_an_unrecognized_disposition_string() {
        let header = sample_header("bogus_disposition", "paper_enforced", 1, "computed");
        assert!(disposition_coherence_violation(&header).is_some());
    }

    // -----------------------------------------------------------------
    // find_signal_journal_attribution_violation (Part 6)
    // -----------------------------------------------------------------

    fn signal_row(
        strategy_id: &str,
        symbol: &str,
        timeframe: &str,
    ) -> mqk_db::StrategySignalEvaluationRecord {
        mqk_db::StrategySignalEvaluationRecord {
            evaluation_id: Uuid::new_v5(
                &Uuid::NAMESPACE_DNS,
                format!("test.dynamic_selection_evidence_validator.signal_row|{strategy_id}|{symbol}|{timeframe}")
                    .as_bytes(),
            ),
            ts_utc: chrono::Utc::now(),
            run_id: Some(Uuid::nil()),
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe: timeframe.to_string(),
            bar_context_source: "db_loaded".to_string(),
            bars_loaded: 10,
            latest_bar_ts_utc: Some(chrono::Utc::now()),
            signal_generated: true,
            signal_qty: Some(1),
            signal_side: Some("buy".to_string()),
            reason_code: "ok".to_string(),
            reason: "ok".to_string(),
            decision_stage: "strategy_evaluated".to_string(),
            source: "mqk-daemon.execution_loop".to_string(),
        }
    }

    #[test]
    fn attribution_accepts_a_same_strategy_multi_symbol_plan() {
        // The exact bug this repair closes: one strategy_id selected for
        // two different symbols must not reject a row correctly attributed
        // to either one.
        let bindings = vec![
            ("AAPL".to_string(), "swing_momentum".to_string(), 300i64),
            ("MSFT".to_string(), "swing_momentum".to_string(), 300i64),
        ];
        let rows = vec![
            signal_row("swing_momentum", "AAPL", "5m"),
            signal_row("swing_momentum", "MSFT", "5m"),
        ];
        assert_eq!(
            find_signal_journal_attribution_violation(&rows, &bindings),
            Ok(())
        );
    }

    #[test]
    fn attribution_accepts_a_same_strategy_multi_timeframe_plan() {
        let bindings = vec![
            ("AAPL".to_string(), "swing_momentum".to_string(), 300i64),
            ("AAPL".to_string(), "swing_momentum".to_string(), 3600i64),
        ];
        let rows = vec![
            signal_row("swing_momentum", "AAPL", "5m"),
            signal_row("swing_momentum", "AAPL", "1H"),
        ];
        assert_eq!(
            find_signal_journal_attribution_violation(&rows, &bindings),
            Ok(())
        );
    }

    #[test]
    fn attribution_rejects_a_wrong_symbol_under_a_selected_strategy() {
        let bindings = vec![("AAPL".to_string(), "swing_momentum".to_string(), 300i64)];
        let rows = vec![signal_row("swing_momentum", "TSLA", "5m")];
        assert!(find_signal_journal_attribution_violation(&rows, &bindings).is_err());
    }

    #[test]
    fn attribution_rejects_a_wrong_timeframe_under_a_selected_strategy() {
        let bindings = vec![("AAPL".to_string(), "swing_momentum".to_string(), 300i64)];
        let rows = vec![signal_row("swing_momentum", "AAPL", "1H")];
        assert!(find_signal_journal_attribution_violation(&rows, &bindings).is_err());
    }

    #[test]
    fn attribution_skips_a_row_for_an_unknown_selected_host_strategy() {
        let bindings = vec![("AAPL".to_string(), "swing_momentum".to_string(), 300i64)];
        // A row for a strategy that is not part of the selected-host set at
        // all (e.g. legacy single-strategy dispatch) is never asserted
        // against -- must not error.
        let rows = vec![signal_row("legacy_single_strategy", "GOOG", "1D")];
        assert_eq!(
            find_signal_journal_attribution_violation(&rows, &bindings),
            Ok(())
        );
    }

    #[test]
    fn attribution_accepts_an_empty_journal() {
        let bindings = vec![("AAPL".to_string(), "swing_momentum".to_string(), 300i64)];
        assert_eq!(
            find_signal_journal_attribution_violation(&[], &bindings),
            Ok(())
        );
    }

    #[test]
    fn attribution_fails_closed_on_an_unmapped_binding_timeframe() {
        let bindings = vec![("AAPL".to_string(), "swing_momentum".to_string(), 123i64)];
        let rows: Vec<mqk_db::StrategySignalEvaluationRecord> = vec![];
        assert!(find_signal_journal_attribution_violation(&rows, &bindings).is_err());
    }
}
