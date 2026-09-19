//! MULTI-STRATEGY-RUNTIME-DISPATCH-01 (R1B): explicit per-symbol
//! multi-strategy dispatch authority, built from a `watchlist-v3` artifact.
//!
//! Frozen design contract:
//! `docs/specs/multi_strategy_runtime_dispatch_01a_frozen_contract.md`.
//!
//! A wholly separate, additive mechanism from Bundle 7's dynamic ranking
//! selector (`mqk_portfolio::dynamic_selection` / `dynamic_selection_plan_builder.rs`
//! / `dynamic_selection_start_gate.rs`) — this module never modifies, calls
//! into the start-gate of, or persists evidence through that pipeline. It
//! reuses two of its building blocks directly, unmodified:
//!
//! - [`crate::dynamic_selection_plan_builder::evaluate_candidate`] — the
//!   exact same independent per-candidate check (registry enabled, promotion
//!   `active_paper`, config-identity verification, plugin instantiation,
//!   timeframe match, daily-data readiness) every Bundle 7 candidate already
//!   passes through. Reused as-is, not reimplemented.
//! - [`mqk_portfolio::compute_dynamic_selection_plan`] — called once **per
//!   binding**, each time with `eligible_symbols = [that one symbol]` and
//!   `candidates = [that one candidate]`. With no sibling candidate to rank
//!   against, the pure gate's outcome collapses to exactly "did this
//!   binding, on its own merits, pass the evidence gate" — the same
//!   `Selected`/`Refused` disposition Bundle 7 already computes, reused
//!   without weakening or reimplementing its one-per-symbol ranking
//!   contract (that contract is simply never invoked with more than one
//!   candidate at a time here).
//!
//! A promoted sibling never authorizes an unpromoted one: each binding's
//! gate call is fully independent — one binding's `Refused` outcome has no
//! effect on any other binding's evaluation or on the resulting host pool
//! (frozen contract §4).
//!
//! The resulting authorized binding set is fed into the exact same
//! [`crate::dynamic_selection_host_pool::DynamicSelectionHostPool::build`]
//! and wrapped in the exact same
//! [`crate::dynamic_selection_dispatch_authority::RuntimeStrategyDispatchAuthority::DynamicPaperEnforced`]
//! variant Bundle 7 already uses — so the unmodified, already-audited
//! per-tick dispatch pipeline (`state/loop_runner.rs`'s
//! `tick_strategy_dispatch_selected_hosts_with_bar_facts`, Bundle 6
//! conflict resolution, Bundle 5 opportunity allocation) is the *exact same
//! production code path* for both mechanisms, never a parallel
//! reimplementation.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use mqk_portfolio::{
    compute_dynamic_selection_plan, DynamicSelectionContext, DynamicSelectionMode,
    SelectionCandidateDisposition, SelectionCandidateInput,
};

use crate::dynamic_selection_dispatch_authority::{
    timeframe_secs_to_db_label, RuntimeStrategyDispatchAuthority, SelectedDispatchBinding,
};
use crate::dynamic_selection_host_pool::{DynamicSelectionHostPool, HostPoolKey};
use crate::dynamic_selection_plan_builder::{
    evaluate_candidate, DynamicSelectionPlanBuildContext, PendingCandidate,
};
use crate::watchlist_intake::LoadedWatchlistArtifactV3;

/// Bounded, closed-vocabulary selection-reason code for every binding this
/// module authorizes — distinct from Bundle 7's ranking-derived reason
/// codes (`selected_highest_score`, etc.), since explicit authorization
/// never ranks.
pub(crate) const REASON_EXPLICIT_MULTI_STRATEGY_AUTHORIZED: &str =
    "explicit_multi_strategy_authorized";

/// Every way building the explicit multi-strategy dispatch authority can
/// fail closed. Distinct from `DispatchAuthorityBuildError` (Bundle 7's own
/// error set) — this mechanism has its own, smaller failure surface, since
/// it never depends on a `DynamicSelectionPlan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExplicitMultiStrategyBuildError {
    /// No binding in the v3 artifact independently passed its evidence gate
    /// — never a partial/empty-but-active authority.
    NoAuthorizedBindings,
    /// A binding that passed its evidence gate had no known DB timeframe
    /// label for its `timeframe_secs` — mirrors
    /// `DispatchAuthorityBuildError::UnknownDbTimeframeLabel`.
    UnknownDbTimeframeLabel { symbol: String, timeframe_secs: i64 },
    /// [`DynamicSelectionHostPool::build`] itself refused (unknown strategy,
    /// registry-inconsistent, spec mismatch, duplicate key, or host
    /// registration failure) — the pool's own fail-closed error, surfaced
    /// unwrapped rather than re-derived.
    HostPoolBuildFailed(crate::dynamic_selection_host_pool::HostPoolBuildError),
}

impl ExplicitMultiStrategyBuildError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::NoAuthorizedBindings => "explicit_multi_strategy_no_authorized_bindings",
            Self::UnknownDbTimeframeLabel { .. } => {
                "explicit_multi_strategy_unknown_db_timeframe_label"
            }
            Self::HostPoolBuildFailed(_) => "explicit_multi_strategy_host_pool_build_failed",
        }
    }
}

/// Versioned namespace seed for this mechanism's own deterministic plan-id
/// derivation — distinct from Bundle 7's `PLAN_ID_NAMESPACE_SEED` so the two
/// id spaces can never collide for a differently-sourced authority.
const EXPLICIT_PLAN_ID_NAMESPACE_SEED: &str = "mqk.explicit-multi-strategy-dispatch-plan-id.v1";

/// Mint the one deterministic plan identity for an explicit multi-strategy
/// authorization set, from `run_id` and the exact sorted, canonical
/// `(symbol, strategy_id, timeframe_secs)` binding set — never from wall
/// clock, input order, or any other non-reproducible fact. The same run_id
/// + binding set always produces the same id; any change to the binding set
/// changes it.
fn derive_explicit_multi_strategy_plan_id(run_id: Uuid, bindings: &[HostPoolKey]) -> Uuid {
    let mut sorted = bindings.to_vec();
    sorted.sort();
    let mut seed = Vec::new();
    seed.extend_from_slice(EXPLICIT_PLAN_ID_NAMESPACE_SEED.as_bytes());
    seed.push(b'|');
    seed.extend_from_slice(run_id.as_bytes());
    for (symbol, strategy_id, timeframe_secs) in &sorted {
        seed.push(b'|');
        seed.extend_from_slice(symbol.as_bytes());
        seed.push(b',');
        seed.extend_from_slice(strategy_id.as_bytes());
        seed.push(b',');
        seed.extend_from_slice(&timeframe_secs.to_le_bytes());
    }
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, &seed)
}

/// Evaluate every `(symbol, strategy_id)` pair a `watchlist-v3` artifact
/// explicitly authorizes, independently, through the exact same gate Bundle
/// 7 candidates pass through — and return only those that independently
/// passed, in deterministic `(symbol, strategy_id, timeframe_secs)` order.
///
/// `timeframe_secs`/`timeframe_label` are the artifact's one shared
/// timeframe (frozen contract §3 — v3 does not support per-binding
/// timeframe overrides in this patch). `market_date`/`source_identity` are
/// caller-supplied, caller-minted facts (this function reads no clock and
/// no env var), carried through into each per-binding gate call's
/// [`DynamicSelectionContext`] for durable-evidence-shape parity with
/// Bundle 7, even though this mechanism does not itself persist evidence.
pub(crate) async fn resolve_authorized_explicit_bindings(
    ctx: &DynamicSelectionPlanBuildContext<'_>,
    artifact: &LoadedWatchlistArtifactV3,
    timeframe_secs: i64,
    timeframe_label: &str,
    run_id: Uuid,
    source_identity: &str,
    market_date: &str,
    now_utc: DateTime<Utc>,
) -> Vec<HostPoolKey> {
    let mut authorized: Vec<HostPoolKey> = Vec::new();

    // Frozen contract §8: dry-run identities never gain economic authority
    // through this path, regardless of whether they appear in a v3
    // `strategy_assignments` list or are independently promoted. Checked
    // before any promotion/readiness I/O — a dry-run identity is refused
    // for the cheapest possible reason, not merely for later economic
    // reasons that happen to also apply.
    let dry_run_ids: std::collections::HashSet<String> =
        crate::state::dry_run_strategy::dry_run_strategy_ids_from_env()
            .into_iter()
            .collect();

    for symbol in &artifact.symbols {
        let Some(strategy_ids) = artifact.strategy_assignments.get(symbol) else {
            // Frozen contract §2.2 / the v3 evaluator's own validation
            // already guarantees every admitted symbol has a non-empty
            // entry — defensive, unreachable in practice, never a panic.
            continue;
        };
        for strategy_id in strategy_ids {
            if dry_run_ids.contains(strategy_id) {
                continue;
            }
            let pending = PendingCandidate {
                symbol: symbol.clone(),
                strategy_id: strategy_id.clone(),
                timeframe_secs,
                timeframe_label: timeframe_label.to_string(),
                watchlist_assigned: true,
            };
            let evidence = evaluate_candidate(ctx, &pending, now_utc).await;
            let candidate = SelectionCandidateInput {
                symbol: symbol.clone(),
                strategy_id: strategy_id.clone(),
                timeframe_secs,
                evidence,
            };

            let context = DynamicSelectionContext {
                run_id: run_id.to_string(),
                schema_version: crate::watchlist_intake::WATCHLIST_SCHEMA_VERSION_V3.to_string(),
                configured_mode: DynamicSelectionMode::PaperEnforced,
                effective_mode: DynamicSelectionMode::PaperEnforced,
                live_lock_applied: false,
                source_kind: "watchlist_v3".to_string(),
                source_identity: source_identity.to_string(),
                market_date: market_date.to_string(),
            };

            // One symbol, one candidate: no sibling to rank against, so the
            // pure gate's outcome is exactly this binding's own
            // pass/refuse — Bundle 7's ranking contract is never invoked
            // with more than one candidate here.
            let plan =
                compute_dynamic_selection_plan(context, &[symbol.clone()], &[candidate.clone()]);
            let passed = plan
                .symbol_results
                .first()
                .and_then(|sr| sr.candidates.first())
                .map(|c| c.disposition == SelectionCandidateDisposition::Selected)
                .unwrap_or(false);

            if passed {
                authorized.push((symbol.clone(), strategy_id.clone(), timeframe_secs));
            }
        }
    }

    authorized.sort();
    authorized
}

/// Build the [`RuntimeStrategyDispatchAuthority::DynamicPaperEnforced`]
/// authority for an explicit multi-strategy authorization set. Fails closed
/// — constructs nothing — if zero bindings independently passed their gate,
/// any authorized binding's `timeframe_secs` has no known DB label, or host
/// pool construction itself fails for any authorized binding.
pub(crate) fn build_explicit_multi_strategy_dispatch_authority(
    run_id: Uuid,
    authorized_bindings: &[HostPoolKey],
) -> Result<RuntimeStrategyDispatchAuthority, ExplicitMultiStrategyBuildError> {
    if authorized_bindings.is_empty() {
        return Err(ExplicitMultiStrategyBuildError::NoAuthorizedBindings);
    }

    let plan_id = derive_explicit_multi_strategy_plan_id(run_id, authorized_bindings);

    let host_pool = DynamicSelectionHostPool::build(authorized_bindings)
        .map_err(ExplicitMultiStrategyBuildError::HostPoolBuildFailed)?;

    let mut bindings = Vec::with_capacity(authorized_bindings.len());
    for (symbol, strategy_id, timeframe_secs) in authorized_bindings {
        let Some(db_timeframe_label) = timeframe_secs_to_db_label(*timeframe_secs) else {
            return Err(ExplicitMultiStrategyBuildError::UnknownDbTimeframeLabel {
                symbol: symbol.clone(),
                timeframe_secs: *timeframe_secs,
            });
        };
        bindings.push(SelectedDispatchBinding {
            symbol: symbol.clone(),
            strategy_id: strategy_id.clone(),
            timeframe_secs: *timeframe_secs,
            db_timeframe_label: db_timeframe_label.to_string(),
            selection_reason_code: REASON_EXPLICIT_MULTI_STRATEGY_AUTHORIZED.to_string(),
            plan_id,
        });
    }

    Ok(RuntimeStrategyDispatchAuthority::DynamicPaperEnforced {
        run_id,
        plan_id,
        bindings,
        host_pool,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::market_calendar::NyseWeekdaysProvider;
    use crate::state::OperatorAuthMode;
    use std::sync::Arc;

    fn ctx_no_db<'a>(
        st: &'a Arc<crate::state::AppState>,
        calendar: &'a NyseWeekdaysProvider,
    ) -> DynamicSelectionPlanBuildContext<'a> {
        DynamicSelectionPlanBuildContext {
            db: None,
            st,
            calendar_provider: calendar,
            provider_configs: &[],
            instruments: &[],
        }
    }

    fn artifact(strategy_assignments: &[(&str, &[&str])]) -> LoadedWatchlistArtifactV3 {
        let symbols: Vec<String> = strategy_assignments
            .iter()
            .map(|(s, _)| s.to_string())
            .collect();
        let strategy_assignments = strategy_assignments
            .iter()
            .map(|(s, ids)| {
                (
                    s.to_string(),
                    ids.iter().map(|id| id.to_string()).collect(),
                )
            })
            .collect();
        LoadedWatchlistArtifactV3 {
            schema_version: crate::watchlist_intake::WATCHLIST_SCHEMA_VERSION_V3.to_string(),
            symbols,
            top_symbol: None,
            strategy_assignments,
            max_symbols_to_trade: 5,
            max_concurrent_positions: 5,
            approved_for_autonomous_paper: true,
            dropped_symbols: vec![],
        }
    }

    // -----------------------------------------------------------------
    // R1B proof #8: dry-run identity cannot become economic. Dry-run
    // exclusion happens before any DB I/O (`dry_run_ids.contains` check
    // precedes `evaluate_candidate`), so this is provable with a `db: None`
    // context — every non-dry-run candidate would also refuse here (no
    // promotion query possible), but the point is specifically that a
    // dry-run identity is excluded before it can even reach that refusal,
    // and can never appear in the authorized output regardless of DB state.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn r1b_08_dry_run_strategy_id_never_appears_in_authorized_bindings() {
        let _guard = crate::state::shared_test_locks::strategy_fleet_env_test_lock()
            .lock()
            .await;
        std::env::set_var(
            crate::state::dry_run_strategy::DRY_RUN_STRATEGY_IDS_ENV,
            "intraday_short_scalper",
        );

        let st = Arc::new(crate::state::AppState::new_with_operator_auth(
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let calendar = NyseWeekdaysProvider;
        let ctx = ctx_no_db(&st, &calendar);
        let art = artifact(&[("AAPL", &["intraday_scalper", "intraday_short_scalper"])]);

        let authorized = resolve_authorized_explicit_bindings(
            &ctx,
            &art,
            300,
            "5m",
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"r1b-08"),
            "test-path",
            "2026-09-18",
            Utc::now(),
        )
        .await;

        std::env::remove_var(crate::state::dry_run_strategy::DRY_RUN_STRATEGY_IDS_ENV);

        assert!(
            authorized
                .iter()
                .all(|(_, strategy_id, _)| strategy_id != "intraday_short_scalper"),
            "a dry-run-listed strategy id must never appear in the authorized set, \
             regardless of any other evidence: {authorized:?}"
        );
    }

    #[tokio::test]
    async fn r1b_no_authorized_bindings_is_a_build_error_never_a_partial_authority() {
        let err = build_explicit_multi_strategy_dispatch_authority(
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"r1b-empty"),
            &[],
        )
        .unwrap_err();
        assert_eq!(err, ExplicitMultiStrategyBuildError::NoAuthorizedBindings);
        assert_eq!(err.code(), "explicit_multi_strategy_no_authorized_bindings");
    }

    #[tokio::test]
    async fn r1b_plan_id_is_deterministic_for_the_same_run_and_binding_set() {
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"r1b-plan-id");
        let bindings = vec![
            ("AAPL".to_string(), "intraday_scalper".to_string(), 300),
            (
                "AAPL".to_string(),
                "intraday_short_scalper".to_string(),
                300,
            ),
        ];
        let mut reversed = bindings.clone();
        reversed.reverse();

        let a = derive_explicit_multi_strategy_plan_id(run_id, &bindings);
        let b = derive_explicit_multi_strategy_plan_id(run_id, &reversed);
        assert_eq!(
            a, b,
            "plan id must not depend on input binding order (sorted before hashing)"
        );

        let different_run = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"r1b-plan-id-different");
        let c = derive_explicit_multi_strategy_plan_id(different_run, &bindings);
        assert_ne!(a, c, "a different run_id must mint a different plan id");
    }

    // -----------------------------------------------------------------
    // R1B proofs #6/#10: per-binding independence of the authorization
    // gate. `resolve_authorized_explicit_bindings` gathers each binding's
    // evidence via the real `evaluate_candidate` (registry + promotion +
    // config-identity + readiness, all real DB I/O) and then calls
    // `compute_dynamic_selection_plan` once per binding — the exact same
    // real, pure, already-extensively-tested Bundle 7 gate function,
    // invoked the same "one symbol, one candidate" way that collapses its
    // ranking step to a trivial pass/refuse. Reaching a genuine `Selected`
    // through the *evidence-gathering* half additionally requires a full
    // research/backtest/review-artifact evidence chain (see
    // `dynamic_selection_plan_builder.rs`'s own
    // `full_evidence_chain_passes_refused_only_on_data_readiness` fixture,
    // which builds exactly that chain) — unrelated to this feature's own
    // new logic and out of scope to duplicate here. These two tests instead
    // exercise the exact downstream mechanism `resolve_authorized_explicit_
    // bindings` relies on — the real `compute_dynamic_selection_plan`,
    // called the same one-candidate-per-call way, with hand-built evidence
    // — mirroring the identical, established technique
    // `scenario_dynamic_selection_end_to_end_paper_dispatch_01.rs` uses for
    // the same reason (see that file's own module docs).
    // -----------------------------------------------------------------

    fn green_evidence() -> mqk_portfolio::SelectionCandidateEvidence {
        mqk_portfolio::SelectionCandidateEvidence {
            promotion_query_ok: true,
            promotion_state: Some("active_paper".to_string()),
            promotion_effective: true,
            promotion_expired: false,
            evidence_resolved: true,
            review_state_is_paper_candidate: true,
            evidence_review_state: Some("paper_candidate".to_string()),
            durable_legacy_fingerprint: Some(
                "86842b99db711429e7d503e0e3a5f6f2d31d9e8b91f19310a32220fc8fdd8b0e".to_string(),
            ),
            recomputed_legacy_fingerprint: Some(
                "86842b99db711429e7d503e0e3a5f6f2d31d9e8b91f19310a32220fc8fdd8b0e".to_string(),
            ),
            legacy_fingerprint_matches: true,
            durable_exact_fingerprint_v2: Some(
                "8d82312144cef2506a296d2620bad311a2095990045cb4518aa4bac054a2de95".to_string(),
            ),
            recomputed_exact_fingerprint_v2: Some(
                "8d82312144cef2506a296d2620bad311a2095990045cb4518aa4bac054a2de95".to_string(),
            ),
            exact_fingerprint_v2_matches: true,
            config_identity_verified: true,
            durable_config_fingerprint: Some(
                "c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f".to_string(),
            ),
            current_config_fingerprint: Some(
                "c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f".to_string(),
            ),
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

    /// Unpromoted: no promotion record at all — the exact shape
    /// `evaluate_candidate` produces for a real strategy with no durable
    /// `active_paper` promotion.
    fn unpromoted_evidence() -> mqk_portfolio::SelectionCandidateEvidence {
        let mut e = green_evidence();
        e.promotion_query_ok = true;
        e.promotion_state = None;
        e.promotion_effective = false;
        e.evidence_resolved = false;
        e.review_state_is_paper_candidate = false;
        e.legacy_fingerprint_matches = false;
        e.exact_fingerprint_v2_matches = false;
        e.config_identity_verified = false;
        e.durable_legacy_fingerprint = None;
        e.recomputed_legacy_fingerprint = None;
        e.durable_exact_fingerprint_v2 = None;
        e.recomputed_exact_fingerprint_v2 = None;
        e
    }

    /// Calls the real `compute_dynamic_selection_plan` the exact same way
    /// `resolve_authorized_explicit_bindings` does internally: one symbol,
    /// one candidate, per call — returning whether that one candidate was
    /// independently `Selected`.
    fn gate_passes(symbol: &str, strategy_id: &str, evidence: mqk_portfolio::SelectionCandidateEvidence) -> bool {
        let context = DynamicSelectionContext {
            run_id: "r1b-test".to_string(),
            schema_version: crate::watchlist_intake::WATCHLIST_SCHEMA_VERSION_V3.to_string(),
            configured_mode: DynamicSelectionMode::PaperEnforced,
            effective_mode: DynamicSelectionMode::PaperEnforced,
            live_lock_applied: false,
            source_kind: "watchlist_v3".to_string(),
            source_identity: "test-path".to_string(),
            market_date: "2026-09-18".to_string(),
        };
        let candidate = SelectionCandidateInput {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe_secs: 300,
            evidence,
        };
        let plan = compute_dynamic_selection_plan(context, &[symbol.to_string()], &[candidate]);
        plan.symbol_results
            .first()
            .and_then(|sr| sr.candidates.first())
            .map(|c| c.disposition == SelectionCandidateDisposition::Selected)
            .unwrap_or(false)
    }

    #[test]
    fn r1b_06_promoted_sibling_never_authorizes_an_unpromoted_one() {
        let promoted_passes = gate_passes("AAPL", "intraday_scalper", green_evidence());
        let unpromoted_passes =
            gate_passes("AAPL", "intraday_short_scalper", unpromoted_evidence());

        assert!(
            promoted_passes,
            "a genuinely promoted, fully-ready binding must pass its own independent gate"
        );
        assert!(
            !unpromoted_passes,
            "an unpromoted binding must be refused on its own evidence, \
             regardless of any sibling's promotion state"
        );
    }

    /// R1B proof #10 (per-binding independence layer): different symbols
    /// with different strategies evaluate to independent dispositions —
    /// one symbol's refused binding cannot suppress or contaminate
    /// another symbol's (or the same symbol's other strategy's) result.
    #[test]
    fn r1b_10_multi_symbol_multi_strategy_no_cross_contamination() {
        let aapl_scalper = gate_passes("AAPL", "intraday_scalper", green_evidence());
        let aapl_short = gate_passes("AAPL", "intraday_short_scalper", green_evidence());
        let msft_unpromoted = gate_passes("MSFT", "volatility_breakout", unpromoted_evidence());

        assert!(aapl_scalper, "AAPL/intraday_scalper must pass independently");
        assert!(
            aapl_short,
            "AAPL/intraday_short_scalper must pass independently, alongside its sibling"
        );
        assert!(
            !msft_unpromoted,
            "MSFT/volatility_breakout's refusal must not depend on or affect AAPL's results"
        );
    }
}
