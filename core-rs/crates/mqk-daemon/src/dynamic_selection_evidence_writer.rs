//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 7C Part 2 — maps a resolved,
//! in-memory `mqk_portfolio::dynamic_selection::DynamicSelectionPlan` (plus
//! its start-gate disposition and minted `plan_id`) onto the durable
//! `mqk_db::NewDynamicSelectionPlan` DTO, and calls the one write authority.
//!
//! This module never re-evaluates, rescores, or re-reads the environment --
//! it only serializes the already-frozen plan the daemon start path built.
//! `Off` never reaches [`build_new_dynamic_selection_plan`]: the caller
//! never has a plan to serialize for that disposition.

use crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition;
use chrono::{DateTime, Utc};
use mqk_portfolio::{DynamicSelectionPlan, SelectionCandidateDisposition};
use uuid::Uuid;

/// Single, shared writer-version identity string recorded on every evidence
/// header row -- the schema/mapper version, distinct from the pure model's
/// own `DYNAMIC_SELECTION_SCHEMA_VERSION` (`library_schema_version`).
pub(crate) const DYNAMIC_SELECTION_EVIDENCE_WRITER_VERSION: &str =
    "mqk-daemon.dynamic-selection-evidence-writer.v1";

fn candidate_disposition_str(d: SelectionCandidateDisposition) -> &'static str {
    match d {
        SelectionCandidateDisposition::Selected => "selected",
        SelectionCandidateDisposition::NotSelected => "not_selected",
        SelectionCandidateDisposition::Refused => "refused",
    }
}

/// `Off` has no string here -- it is never passed to this function (see
/// module docs and the call site in `state/lifecycle.rs`).
fn start_gate_disposition_str(
    d: DynamicSelectionStartGateDisposition,
) -> Result<&'static str, String> {
    match d {
        DynamicSelectionStartGateDisposition::Off => {
            Err("Off must never be persisted as durable plan evidence".to_string())
        }
        DynamicSelectionStartGateDisposition::ShadowAllowed => Ok("shadow_allowed"),
        DynamicSelectionStartGateDisposition::ShadowInvalid => Ok("shadow_invalid"),
        DynamicSelectionStartGateDisposition::PaperEnforcedAllowed => {
            Ok("paper_enforced_allowed")
        }
        DynamicSelectionStartGateDisposition::PaperEnforcedRefused => {
            Ok("paper_enforced_refused")
        }
    }
}

/// Build the durable evidence DTO for one resolved plan. `run_id` is the
/// authoritative run identity (passed independently of `plan.context.run_id`
/// the same way the rest of Bundle 7 treats it -- the context string is
/// already IR3-coherence-checked against it upstream, never re-validated
/// here). Returns `Err` only for the structurally-impossible `Off` case.
pub(crate) fn build_new_dynamic_selection_plan(
    plan: &DynamicSelectionPlan,
    plan_id: Uuid,
    run_id: Uuid,
    disposition: DynamicSelectionStartGateDisposition,
    created_at_utc: DateTime<Utc>,
) -> Result<mqk_db::NewDynamicSelectionPlan, String> {
    let disposition_str = start_gate_disposition_str(disposition)?.to_string();
    let context = &plan.context;

    let mut symbols = Vec::with_capacity(plan.symbol_results.len());
    let mut candidates = Vec::new();
    let mut ordinal: i32 = 0;

    for sr in &plan.symbol_results {
        symbols.push(mqk_db::NewDynamicSelectionPlanSymbol {
            symbol: sr.symbol.clone(),
            selected_strategy_id: sr.selected_strategy_id.clone(),
            disposition: candidate_disposition_str(sr.disposition).to_string(),
            reason_code: sr.reason_code.clone(),
            exact_reason_code: sr.exact_reason.code(),
        });

        for c in &sr.candidates {
            candidates.push(mqk_db::NewDynamicSelectionPlanCandidate {
                ordinal,
                symbol: c.symbol.clone(),
                strategy_id: c.strategy_id.clone(),
                timeframe_secs: c.timeframe_secs,
                canonical_score_decimal: c.canonical_score_decimal.clone(),
                canonical_score_micros: c.canonical_score_micros,
                scanner_rank: c.scanner_rank.map(|v| v as i32),
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
                exact_reason_code: c.exact_reason.code(),
                selected: c.selected,
                disposition: candidate_disposition_str(c.disposition).to_string(),
                reason_code: c.reason_code.clone(),
            });
            ordinal += 1;
        }
    }

    Ok(mqk_db::NewDynamicSelectionPlan {
        plan_id,
        run_id,
        library_schema_version: mqk_portfolio::DYNAMIC_SELECTION_SCHEMA_VERSION.to_string(),
        context_schema_version: context.schema_version.clone(),
        configured_mode: context.configured_mode.as_str().to_string(),
        effective_mode: context.effective_mode.as_str().to_string(),
        live_lock_applied: context.live_lock_applied,
        approved_for_live: false,
        source_kind: context.source_kind.clone(),
        source_identity: context.source_identity.clone(),
        market_date: context.market_date.clone(),
        disposition: disposition_str,
        truth_state: plan.truth_state.clone(),
        blockers: plan.blockers.clone(),
        symbol_count: i32::try_from(plan.eligible_symbol_count())
            .map_err(|e| format!("symbol_count does not fit in i32: {e}"))?,
        candidate_count: i32::try_from(plan.candidate_count())
            .map_err(|e| format!("candidate_count does not fit in i32: {e}"))?,
        selected_count: i32::try_from(plan.selected_count())
            .map_err(|e| format!("selected_count does not fit in i32: {e}"))?,
        refused_count: i32::try_from(plan.refused_count())
            .map_err(|e| format!("refused_count does not fit in i32: {e}"))?,
        writer_version: DYNAMIC_SELECTION_EVIDENCE_WRITER_VERSION.to_string(),
        created_at_utc,
        symbols,
        candidates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_disposition_is_rejected() {
        let err = start_gate_disposition_str(DynamicSelectionStartGateDisposition::Off)
            .expect_err("Off must never map to a persistable disposition string");
        assert!(err.contains("Off"));
    }

    #[test]
    fn every_non_off_disposition_maps_to_the_closed_db_vocabulary() {
        let allowed = [
            "shadow_allowed",
            "shadow_invalid",
            "paper_enforced_allowed",
            "paper_enforced_refused",
        ];
        for d in [
            DynamicSelectionStartGateDisposition::ShadowAllowed,
            DynamicSelectionStartGateDisposition::ShadowInvalid,
            DynamicSelectionStartGateDisposition::PaperEnforcedAllowed,
            DynamicSelectionStartGateDisposition::PaperEnforcedRefused,
        ] {
            let s = start_gate_disposition_str(d).expect("non-Off disposition must map");
            assert!(allowed.contains(&s), "{s} not in closed vocabulary");
        }
    }
}
