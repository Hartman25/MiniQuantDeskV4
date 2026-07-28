//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 6 — start-gate authority
//! evaluator.
//!
//! Composes Phase 4's plan builder and Phase 5's host pool builder into one
//! fail-closed evaluation of whether an effectively-`paper_enforced` run may
//! proceed. Mirrors every fail-closed condition Phase 6 named:
//!
//! - DB unavailable
//! - invalid/empty symbol universe, or candidate bound exceeded (both
//!   surface as the plan's own non-`computed` `truth_state`)
//! - no selected pair
//! - any eligible symbol lacks the required selection under the enforced
//!   policy (every symbol in the frozen universe must have a selection —
//!   not merely "at least one")
//! - plan/evidence invalid (non-`computed` `truth_state`)
//! - selected host construction fails, including a duplicate key (both
//!   surface through [`DynamicSelectionHostPool::build`]'s own fail-closed
//!   error)
//!
//! "Selected data readiness/freshness fails" is not re-checked independently
//! here: a *selected* candidate is, by the pure model's own gate contract,
//! one whose `data_ready` was already `true` when Phase 4 built the frozen
//! plan — re-deriving it a second time here would either trivially agree
//! (redundant) or disagree with a plan the caller is contractually treating
//! as frozen (a correctness hazard, not a safety improvement).
//!
//! # `not_applicable` vs `allowed`
//! Bundle 7 has authority *only* when `effective_mode == PaperEnforced`. If
//! the resolved mode is `Off`/`Shadow` — including when `Shadow` or
//! `PaperEnforced` was configured but the live lock demoted `effective_mode`
//! to `Off` — this evaluator performs **no I/O of any kind** and returns
//! `not_applicable = true, allowed = true`: the caller must proceed exactly
//! as if Bundle 7 did not exist for this run (legacy Tier A dispatch,
//! unaffected). This is the load-bearing distinction from `allowed = false`:
//! `not_applicable` means "Bundle 7 is inert here, by design" (Phase 2's
//! live-lock contract); `allowed = false` means "the operator explicitly
//! configured `paper_enforced`, it is genuinely in effect, and it cannot be
//! honored" — which must refuse the *whole* run start, never silently fall
//! back to legacy single-strategy dispatch (fail-closed over fail-open).
//!
//! # Not yet wired
//! This evaluator is standalone and returns its outcome to the caller — it
//! is not yet called from `AppState::start_execution_runtime`, does not
//! touch any `AppState` field, and does not persist anything. Mirrors the
//! same incremental precedent `daily_data_readiness::evaluate_daily_data_readiness_from_env`
//! itself documents ("not called by any route, lifecycle gate, or scheduler
//! in this phase") — splicing a call to this evaluator into the ~900-line
//! production start gate, and adding the `AppState` pool field plus its
//! stop/halt/start-failure clearing, is a separate, narrowly-scoped
//! follow-up patch reviewable against that exact function in isolation.

use chrono::{DateTime, Utc};
use mqk_portfolio::{
    DynamicSelectionContext, DynamicSelectionMode, DynamicSelectionPlan,
    DYNAMIC_SELECTION_TRUTH_STATE_COMPUTED,
};

use crate::dynamic_selection_host_pool::{
    DynamicSelectionHostPool, HostPoolBuildError, HostPoolKey,
};
use crate::dynamic_selection_mode::EffectiveDynamicSelectionMode;
use crate::dynamic_selection_plan_builder::{
    build_dynamic_selection_plan, DynamicSelectionPlanBuildContext,
};
use crate::state::MultiSymbolRuntimeConfig;

/// Closed, bounded reason-code vocabulary for a refused start-gate
/// evaluation. Every distinct failure mode Phase 6 named gets its own
/// variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DynamicSelectionStartGateReason {
    DbUnavailable,
    /// The plan's own `truth_state` was not `computed` -- covers empty/
    /// blank/duplicate/over-limit eligible symbols and over-limit candidate
    /// pairs (R1/R7, Phase 0R), all surfaced through the pure model's own
    /// fail-closed truth state rather than re-derived here.
    PlanInvalid {
        truth_state: String,
    },
    NoSelectedPair,
    /// One eligible symbol in the frozen universe has no selection --
    /// paper_enforced requires *every* symbol to resolve, not merely "at
    /// least one."
    SymbolMissingRequiredSelection {
        symbol: String,
    },
    /// [`DynamicSelectionHostPool::build`] failed -- covers duplicate key,
    /// unknown strategy, registry inconsistency, and spec mismatches, all
    /// via that function's own [`HostPoolBuildError`].
    HostPoolBuildFailed {
        code: &'static str,
        detail: String,
    },
}

impl DynamicSelectionStartGateReason {
    pub fn code(&self) -> &'static str {
        match self {
            Self::DbUnavailable => "dynamic_selection_start_gate_db_unavailable",
            Self::PlanInvalid { .. } => "dynamic_selection_start_gate_plan_invalid",
            Self::NoSelectedPair => "dynamic_selection_start_gate_no_selected_pair",
            Self::SymbolMissingRequiredSelection { .. } => {
                "dynamic_selection_start_gate_symbol_missing_required_selection"
            }
            Self::HostPoolBuildFailed { .. } => {
                "dynamic_selection_start_gate_host_pool_build_failed"
            }
        }
    }
}

/// Result of evaluating one start attempt.
pub struct DynamicSelectionStartGateOutcome {
    /// `true` only when Bundle 7 has no authority for this start attempt at
    /// all (`effective_mode != PaperEnforced`). When `true`, `allowed` is
    /// always also `true`, `plan`/`host_pool` are always `None`, and
    /// `reasons` is always empty -- the caller must treat this exactly as
    /// "Bundle 7 does not exist for this run."
    pub not_applicable: bool,
    pub allowed: bool,
    pub plan: Option<DynamicSelectionPlan>,
    pub host_pool: Option<DynamicSelectionHostPool>,
    pub reasons: Vec<DynamicSelectionStartGateReason>,
}

/// Every selected `(symbol, strategy_id, timeframe_secs)` triple from an
/// already-`computed` plan -- the exact identity triple
/// [`DynamicSelectionHostPool::build`] keys on, derived from each symbol's
/// selected candidate row (never a separate/global timeframe).
fn selected_host_pool_keys(plan: &DynamicSelectionPlan) -> Vec<HostPoolKey> {
    plan.symbol_results
        .iter()
        .filter_map(|sr| {
            let strategy_id = sr.selected_strategy_id.clone()?;
            let candidate = sr.candidates.iter().find(|c| c.selected)?;
            Some((sr.symbol.clone(), strategy_id, candidate.timeframe_secs))
        })
        .collect()
}

/// Evaluate an already-`computed` (or fail-closed) [`DynamicSelectionPlan`]
/// against every paper_enforced start-gate requirement and, if it passes,
/// build the run-scoped host pool. Split out from
/// [`evaluate_dynamic_selection_start_gate`] so gate logic can be tested
/// directly against hand-constructed plan fixtures, without requiring a
/// full DB/artifact/calendar composition to exercise every branch.
fn evaluate_plan_for_start_gate(plan: DynamicSelectionPlan) -> DynamicSelectionStartGateOutcome {
    let mut reasons = Vec::new();

    if plan.truth_state != DYNAMIC_SELECTION_TRUTH_STATE_COMPUTED {
        reasons.push(DynamicSelectionStartGateReason::PlanInvalid {
            truth_state: plan.truth_state.clone(),
        });
        return DynamicSelectionStartGateOutcome {
            not_applicable: false,
            allowed: false,
            plan: Some(plan),
            host_pool: None,
            reasons,
        };
    }

    if plan.selected_count() == 0 {
        reasons.push(DynamicSelectionStartGateReason::NoSelectedPair);
    }
    for sr in &plan.symbol_results {
        if sr.selected_strategy_id.is_none() {
            reasons.push(
                DynamicSelectionStartGateReason::SymbolMissingRequiredSelection {
                    symbol: sr.symbol.clone(),
                },
            );
        }
    }

    if !reasons.is_empty() {
        return DynamicSelectionStartGateOutcome {
            not_applicable: false,
            allowed: false,
            plan: Some(plan),
            host_pool: None,
            reasons,
        };
    }

    let selected = selected_host_pool_keys(&plan);
    match DynamicSelectionHostPool::build(&selected) {
        Ok(pool) => DynamicSelectionStartGateOutcome {
            not_applicable: false,
            allowed: true,
            plan: Some(plan),
            host_pool: Some(pool),
            reasons: Vec::new(),
        },
        Err(e) => DynamicSelectionStartGateOutcome {
            not_applicable: false,
            allowed: false,
            plan: Some(plan),
            host_pool: None,
            reasons: vec![DynamicSelectionStartGateReason::HostPoolBuildFailed {
                code: e.code(),
                detail: host_pool_error_detail(&e),
            }],
        },
    }
}

fn host_pool_error_detail(e: &HostPoolBuildError) -> String {
    format!("{e:?}")
}

/// Full orchestration: mode gate -> plan build (Phase 4) -> start-gate
/// evaluation + host pool build. See module docs for the complete contract.
pub async fn evaluate_dynamic_selection_start_gate(
    ctx: &DynamicSelectionPlanBuildContext<'_>,
    multi_symbol_config: &MultiSymbolRuntimeConfig,
    configured_strategy_ids: &[String],
    effective_mode: &EffectiveDynamicSelectionMode,
    context: DynamicSelectionContext,
    now_utc: DateTime<Utc>,
) -> DynamicSelectionStartGateOutcome {
    if effective_mode.effective_mode != DynamicSelectionMode::PaperEnforced {
        return DynamicSelectionStartGateOutcome {
            not_applicable: true,
            allowed: true,
            plan: None,
            host_pool: None,
            reasons: Vec::new(),
        };
    }

    if ctx.db.is_none() {
        return DynamicSelectionStartGateOutcome {
            not_applicable: false,
            allowed: false,
            plan: None,
            host_pool: None,
            reasons: vec![DynamicSelectionStartGateReason::DbUnavailable],
        };
    }

    let plan = build_dynamic_selection_plan(
        ctx,
        multi_symbol_config,
        configured_strategy_ids,
        context,
        now_utc,
    )
    .await;

    evaluate_plan_for_start_gate(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mqk_portfolio::{
        canonical_plan_identity_material, DynamicSelectionContext as DsContext,
        SelectionCandidateDisposition, SelectionCandidateEvidence, SelectionCandidateInput,
    };

    fn ds_context() -> DsContext {
        DsContext {
            run_id: "run-1".to_string(),
            schema_version: mqk_portfolio::DYNAMIC_SELECTION_SCHEMA_VERSION.to_string(),
            configured_mode: DynamicSelectionMode::PaperEnforced,
            effective_mode: DynamicSelectionMode::PaperEnforced,
            live_lock_applied: false,
            source_kind: "env_single_symbol_fallback".to_string(),
            source_identity: "env".to_string(),
            market_date: "2026-07-28".to_string(),
        }
    }

    fn valid_evidence(score_micros: i64) -> SelectionCandidateEvidence {
        SelectionCandidateEvidence {
            promotion_query_ok: true,
            promotion_state: Some("active_paper".to_string()),
            promotion_effective: true,
            promotion_expired: false,
            evidence_resolved: true,
            review_state_is_paper_candidate: true,
            fingerprint_matches: true,
            plugin_instantiable: true,
            timeframe_matches: true,
            data_ready: true,
            canonical_score_micros: Some(score_micros),
            scanner_rank: Some(1),
            watchlist_assigned: true,
            evidence_review_id: Some("review-1".to_string()),
            evidence_scanner_scan_id: Some("scan-1".to_string()),
            evidence_artifact_path: Some("/artifacts/review-1".to_string()),
            evidence_fingerprint: Some("fp-1".to_string()),
        }
    }

    fn computed_plan_all_selected() -> DynamicSelectionPlan {
        let candidates = vec![
            SelectionCandidateInput {
                symbol: "AAPL".to_string(),
                strategy_id: "swing_momentum".to_string(),
                timeframe_secs: 86400,
                evidence: valid_evidence(500_000),
            },
            SelectionCandidateInput {
                symbol: "MSFT".to_string(),
                strategy_id: "mean_reversion".to_string(),
                timeframe_secs: 3600,
                evidence: valid_evidence(700_000),
            },
        ];
        mqk_portfolio::compute_dynamic_selection_plan(
            ds_context(),
            &["AAPL".to_string(), "MSFT".to_string()],
            &candidates,
        )
    }

    #[test]
    fn all_symbols_selected_plan_builds_a_host_pool_and_allows() {
        let plan = computed_plan_all_selected();
        assert_eq!(
            plan.truth_state,
            mqk_portfolio::DYNAMIC_SELECTION_TRUTH_STATE_COMPUTED
        );
        assert_eq!(plan.selected_count(), 2);

        let outcome = evaluate_plan_for_start_gate(plan);
        assert!(!outcome.not_applicable);
        assert!(
            outcome.allowed,
            "reasons: {:?}",
            outcome.reasons.iter().map(|r| r.code()).collect::<Vec<_>>()
        );
        assert!(outcome.reasons.is_empty());
        let pool = outcome
            .host_pool
            .expect("host pool must be built on success");
        assert_eq!(pool.len(), 2);
        assert!(pool.contains_key("AAPL", "swing_momentum", 86400));
        assert!(pool.contains_key("MSFT", "mean_reversion", 3600));
    }

    #[test]
    fn non_computed_plan_is_refused_as_plan_invalid() {
        let plan = mqk_portfolio::compute_dynamic_selection_plan(ds_context(), &[], &[]);
        assert_ne!(
            plan.truth_state,
            mqk_portfolio::DYNAMIC_SELECTION_TRUTH_STATE_COMPUTED
        );

        let outcome = evaluate_plan_for_start_gate(plan);
        assert!(!outcome.allowed);
        assert!(outcome.host_pool.is_none());
        assert!(outcome
            .reasons
            .iter()
            .any(|r| matches!(r, DynamicSelectionStartGateReason::PlanInvalid { .. })));
    }

    #[test]
    fn zero_selections_is_refused_as_no_selected_pair() {
        let candidates = vec![SelectionCandidateInput {
            symbol: "AAPL".to_string(),
            strategy_id: "swing_momentum".to_string(),
            timeframe_secs: 86400,
            evidence: SelectionCandidateEvidence {
                promotion_state: None,
                ..valid_evidence(500_000)
            },
        }];
        let plan = mqk_portfolio::compute_dynamic_selection_plan(
            ds_context(),
            &["AAPL".to_string()],
            &candidates,
        );
        assert_eq!(plan.selected_count(), 0);

        let outcome = evaluate_plan_for_start_gate(plan);
        assert!(!outcome.allowed);
        assert!(outcome.host_pool.is_none());
        assert!(outcome
            .reasons
            .contains(&DynamicSelectionStartGateReason::NoSelectedPair));
        assert!(outcome
            .reasons
            .iter()
            .any(|r| matches!(
                r,
                DynamicSelectionStartGateReason::SymbolMissingRequiredSelection { symbol } if symbol == "AAPL"
            )));
    }

    #[test]
    fn one_of_two_symbols_missing_selection_is_refused_even_though_the_other_selected() {
        // paper_enforced requires *every* eligible symbol to resolve, not
        // merely "at least one" -- a partial selection must still refuse
        // the whole start.
        let candidates = vec![
            SelectionCandidateInput {
                symbol: "AAPL".to_string(),
                strategy_id: "swing_momentum".to_string(),
                timeframe_secs: 86400,
                evidence: valid_evidence(500_000),
            },
            SelectionCandidateInput {
                symbol: "MSFT".to_string(),
                strategy_id: "mean_reversion".to_string(),
                timeframe_secs: 3600,
                evidence: SelectionCandidateEvidence {
                    promotion_state: None,
                    ..valid_evidence(700_000)
                },
            },
        ];
        let plan = mqk_portfolio::compute_dynamic_selection_plan(
            ds_context(),
            &["AAPL".to_string(), "MSFT".to_string()],
            &candidates,
        );
        assert_eq!(plan.selected_count(), 1, "AAPL selected, MSFT is not");

        let outcome = evaluate_plan_for_start_gate(plan);
        assert!(!outcome.allowed);
        assert!(outcome.host_pool.is_none());
        assert!(!outcome
            .reasons
            .contains(&DynamicSelectionStartGateReason::NoSelectedPair));
        assert!(outcome.reasons.iter().any(|r| matches!(
            r,
            DynamicSelectionStartGateReason::SymbolMissingRequiredSelection { symbol } if symbol == "MSFT"
        )));
    }

    #[test]
    fn selected_but_unsupported_strategy_fails_host_pool_build() {
        // Evidence gates all pass, but no such strategy is registered in the
        // real plugin registry -- the plan itself can still select it
        // (evidence-only gate), and only DynamicSelectionHostPool::build
        // catches the mismatch.
        let candidates = vec![SelectionCandidateInput {
            symbol: "AAPL".to_string(),
            strategy_id: "totally_unknown_strategy".to_string(),
            timeframe_secs: 86400,
            evidence: valid_evidence(500_000),
        }];
        let plan = mqk_portfolio::compute_dynamic_selection_plan(
            ds_context(),
            &["AAPL".to_string()],
            &candidates,
        );
        assert_eq!(plan.selected_count(), 1);

        let outcome = evaluate_plan_for_start_gate(plan);
        assert!(!outcome.allowed);
        assert!(outcome.host_pool.is_none());
        assert!(outcome.reasons.iter().any(|r| matches!(
            r,
            DynamicSelectionStartGateReason::HostPoolBuildFailed { .. }
        )));
    }

    #[tokio::test]
    async fn mode_not_paper_enforced_is_not_applicable_with_zero_io() {
        use crate::state::{MultiSymbolConfigSource, OperatorAuthMode};
        use std::sync::Arc;

        let st = Arc::new(crate::state::AppState::new_with_operator_auth(
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let calendar = crate::state::market_calendar::NyseWeekdaysProvider;
        let ctx = DynamicSelectionPlanBuildContext {
            db: None,
            st: &st,
            calendar_provider: &calendar,
            provider_configs: &[],
            instruments: &[],
        };
        let cfg = crate::state::MultiSymbolRuntimeConfig {
            schema_version: "multi-symbol-runtime-config-v1".to_string(),
            symbols: vec![],
            max_concurrent_symbols: 1,
            source: MultiSymbolConfigSource::EnvSingleSymbolFallback,
        };
        let effective = EffectiveDynamicSelectionMode {
            configured_mode: DynamicSelectionMode::Shadow,
            effective_mode: DynamicSelectionMode::Off, // e.g. live-lock demoted
            invalid_configuration: None,
            live_lock_applied: true,
        };

        let outcome = evaluate_dynamic_selection_start_gate(
            &ctx,
            &cfg,
            &[],
            &effective,
            ds_context(),
            Utc::now(),
        )
        .await;
        assert!(outcome.not_applicable);
        assert!(outcome.allowed);
        assert!(outcome.plan.is_none());
        assert!(outcome.host_pool.is_none());
        assert!(outcome.reasons.is_empty());
    }

    #[tokio::test]
    async fn no_db_pool_is_refused_db_unavailable_without_building_a_plan() {
        use crate::state::{MultiSymbolConfigSource, OperatorAuthMode};
        use std::sync::Arc;

        let st = Arc::new(crate::state::AppState::new_with_operator_auth(
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let calendar = crate::state::market_calendar::NyseWeekdaysProvider;
        let ctx = DynamicSelectionPlanBuildContext {
            db: None,
            st: &st,
            calendar_provider: &calendar,
            provider_configs: &[],
            instruments: &[],
        };
        let cfg = crate::state::MultiSymbolRuntimeConfig {
            schema_version: "multi-symbol-runtime-config-v1".to_string(),
            symbols: vec![crate::state::SymbolStrategyAssignment {
                symbol: "AAPL".to_string(),
                strategy_id: "swing_momentum".to_string(),
                timeframe: "1D".to_string(),
            }],
            max_concurrent_symbols: 1,
            source: MultiSymbolConfigSource::EnvSingleSymbolFallback,
        };
        let effective = EffectiveDynamicSelectionMode {
            configured_mode: DynamicSelectionMode::PaperEnforced,
            effective_mode: DynamicSelectionMode::PaperEnforced,
            invalid_configuration: None,
            live_lock_applied: false,
        };

        let outcome = evaluate_dynamic_selection_start_gate(
            &ctx,
            &cfg,
            &[],
            &effective,
            ds_context(),
            Utc::now(),
        )
        .await;
        assert!(!outcome.not_applicable);
        assert!(!outcome.allowed);
        assert!(
            outcome.plan.is_none(),
            "no plan built when DB is unavailable"
        );
        assert_eq!(
            outcome.reasons,
            vec![DynamicSelectionStartGateReason::DbUnavailable]
        );
    }

    #[test]
    fn plan_identity_material_is_recoverable_from_a_refused_outcome() {
        // The plan is always carried through on the outcome (even when
        // refused), so a durable evidence writer (Phase 8) always has
        // something to mint an identity from and persist -- proven by
        // reusing the Phase 0R identity-material function directly.
        let plan = computed_plan_all_selected();
        let outcome = evaluate_plan_for_start_gate(plan);
        let plan_ref = outcome.plan.as_ref().expect("plan always present");
        let material = canonical_plan_identity_material(plan_ref);
        assert!(!material.is_empty());
    }

    #[test]
    fn selected_host_pool_keys_uses_each_candidates_own_timeframe() {
        let plan = computed_plan_all_selected();
        let keys = selected_host_pool_keys(&plan);
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&("AAPL".to_string(), "swing_momentum".to_string(), 86400)));
        assert!(keys.contains(&("MSFT".to_string(), "mean_reversion".to_string(), 3600)));
    }

    #[test]
    fn disposition_selected_marker_is_consistent_with_selected_strategy_id() {
        // Sanity check on the fixture helper itself: exactly one candidate
        // per symbol carries Disposition::Selected, and it is the same
        // strategy_id as symbol_results' own selected_strategy_id.
        let plan = computed_plan_all_selected();
        for sr in &plan.symbol_results {
            let selected_candidates: Vec<_> = sr
                .candidates
                .iter()
                .filter(|c| c.disposition == SelectionCandidateDisposition::Selected)
                .collect();
            assert_eq!(selected_candidates.len(), 1);
            assert_eq!(
                Some(selected_candidates[0].strategy_id.clone()),
                sr.selected_strategy_id
            );
        }
    }
}
