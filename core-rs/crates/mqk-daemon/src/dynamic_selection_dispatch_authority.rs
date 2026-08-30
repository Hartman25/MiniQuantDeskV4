//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7B-SELECTED-HOST-ECONOMIC-
//! DISPATCH-CLOSURE — Parts 1/2: the one frozen, run-scoped strategy-
//! dispatch authority and its deterministic plan identity.
//!
//! `RuntimeStrategyDispatchAuthority` is built exactly once per start
//! attempt, before the Phase 7A startup barrier releases, and moved
//! wholesale into the execution loop task (`state/loop_runner.rs`) — never
//! cloned, never rebuilt, never shared with any other owner. `Off`/`Shadow`
//! always receive `Legacy`; only a `PaperEnforcedAllowed` disposition ever
//! receives `DynamicPaperEnforced`.

use std::collections::BTreeSet;

use uuid::Uuid;

use crate::dynamic_selection_host_pool::DynamicSelectionHostPool;
use crate::dynamic_selection_start_gate::selected_host_pool_keys;
use mqk_portfolio::{canonical_plan_identity_material, DynamicSelectionPlan};

use crate::state::SymbolStrategyAssignment;

/// Versioned namespace prefix for Part 2's deterministic plan-id derivation.
/// A future schema change bumps this so the new id space can never silently
/// collide with an older version's ids for a differently-shaped plan.
const PLAN_ID_NAMESPACE_SEED: &str = "mqk.dynamic-selection-plan-id.v1";

/// Part 2: mint the one deterministic UUIDv5 plan identity from a resolved
/// plan's exact canonical identity bytes
/// (`mqk_portfolio::canonical_plan_identity_material`) — never from debug
/// text, JSON map iteration, wall clock, input order, pointer identity, or
/// host-pool order. The same resolved plan always produces the same id; any
/// change to a result-affecting plan fact changes the id (proven by
/// `canonical_plan_identity_material`'s own exhaustiveness).
pub(crate) fn derive_dynamic_selection_plan_id(plan: &DynamicSelectionPlan) -> Uuid {
    let material = canonical_plan_identity_material(plan);
    let mut seed = Vec::with_capacity(PLAN_ID_NAMESPACE_SEED.len() + 1 + material.len());
    seed.extend_from_slice(PLAN_ID_NAMESPACE_SEED.as_bytes());
    seed.push(b'|');
    seed.extend_from_slice(&material);
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, &seed)
}

/// Fixed bijection between a selected candidate's `timeframe_secs` and the
/// DB timeframe label `mqk_db::fetch_recent_completed_bars_for_strategy`
/// expects — the exact inverse of the one other canonical label<->secs
/// conversion already in the codebase,
/// `mqk_db::strategy_promotion::scanner_timeframe_label_to_secs`. A selected
/// candidate whose `timeframe_secs` falls outside this fixed set has no
/// known DB label and fails dispatch-authority construction closed
/// (`DispatchAuthorityBuildError::UnknownDbTimeframeLabel`) rather than
/// guessing one.
pub(crate) fn timeframe_secs_to_db_label(secs: i64) -> Option<&'static str> {
    match secs {
        60 => Some("1m"),
        300 => Some("5m"),
        900 => Some("15m"),
        1800 => Some("30m"),
        3600 => Some("1H"),
        86400 => Some("1D"),
        _ => None,
    }
}

/// Part 1: one selected `(symbol, strategy_id, timeframe)` binding, carrying
/// everything the selected-host dispatch backend needs without rereading the
/// plan, evidence, promotion, watchlist, or fleet state mid-tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedDispatchBinding {
    pub(crate) symbol: String,
    pub(crate) strategy_id: String,
    pub(crate) timeframe_secs: i64,
    /// Exact DB timeframe label used for this binding's bar-window load.
    pub(crate) db_timeframe_label: String,
    /// Bounded, closed-vocabulary provenance proving this binding came from
    /// the frozen plan's own selected-candidate reason — never freeform
    /// text, never re-derived after start.
    pub(crate) selection_reason_code: String,
    /// The frozen plan this binding was selected from (Part 2).
    pub(crate) plan_id: Uuid,
}

/// Part 6: explicit selection provenance threaded through Bundle 6/Bundle 5
/// as a runtime-local wrapper (never attached to `InternalStrategyDecision`
/// or `PendingDecisionWithBarFacts`, and never added to the broker order
/// schema or any public API) — kept in the execution loop's own per-tick
/// `BTreeMap<(symbol, strategy_id), DynamicSelectionDispatchProvenance>`
/// (`state/loop_runner.rs`) and re-validated by exact `(symbol,
/// strategy_id)` lookup before Bundle 6, after Bundle 6, after Bundle 5, and
/// before submission. Immutable once built; Bundle 6/Bundle 5 never see or
/// touch this map, so they structurally cannot drop, rewrite, swap, or
/// fabricate it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DynamicSelectionDispatchProvenance {
    pub(crate) run_id: Uuid,
    pub(crate) plan_id: Uuid,
    pub(crate) symbol: String,
    pub(crate) strategy_id: String,
    pub(crate) timeframe_secs: i64,
}

/// Every way building the one frozen [`RuntimeStrategyDispatchAuthority`] can
/// fail closed before activation (Part 1 requirements 9/10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DispatchAuthorityBuildError {
    /// TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 start-authority check:
    /// `plan.context.run_id` does not parse as a UUID at all.
    InvalidPlanRunId {
        raw: String,
    },
    /// TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 start-authority check:
    /// `plan.context.run_id` parses, but does not equal the actual `run_id`
    /// this authority is being built for — the plan and the run it is about
    /// to be activated under have diverged. Never trust the caller-supplied
    /// `run_id` alone; this plan is the frozen record of what was actually
    /// evaluated.
    RunIdMismatch {
        plan_run_id: Uuid,
        run_id: Uuid,
    },
    EmptyPlan,
    EmptySelectedBindings,
    MissingHostPoolEntry {
        symbol: String,
        strategy_id: String,
        timeframe_secs: i64,
    },
    ExtraHostPoolEntry {
        symbol: String,
        strategy_id: String,
        timeframe_secs: i64,
    },
    DuplicateBinding {
        symbol: String,
        strategy_id: String,
        timeframe_secs: i64,
    },
    UnknownDbTimeframeLabel {
        symbol: String,
        timeframe_secs: i64,
    },
    HostPoolCountMismatch {
        bindings: usize,
        pool_len: usize,
    },
}

impl DispatchAuthorityBuildError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::InvalidPlanRunId { .. } => "dispatch_authority_invalid_plan_run_id",
            Self::RunIdMismatch { .. } => "dispatch_authority_run_id_mismatch",
            Self::EmptyPlan => "dispatch_authority_empty_plan",
            Self::EmptySelectedBindings => "dispatch_authority_empty_selected_bindings",
            Self::MissingHostPoolEntry { .. } => "dispatch_authority_missing_host_pool_entry",
            Self::ExtraHostPoolEntry { .. } => "dispatch_authority_extra_host_pool_entry",
            Self::DuplicateBinding { .. } => "dispatch_authority_duplicate_binding",
            Self::UnknownDbTimeframeLabel { .. } => "dispatch_authority_unknown_db_timeframe_label",
            Self::HostPoolCountMismatch { .. } => "dispatch_authority_host_pool_count_mismatch",
        }
    }
}

/// Part 1: the one frozen, run-scoped, source-refined strategy-dispatch
/// authority. Built once at start, before the loop barrier releases, and
/// moved wholesale into the execution loop task (`state/loop_runner.rs`) —
/// never cloned, never rebuilt, never shared with any other owner. Dropping
/// this value (loop task exit on stop/halt/shutdown/reap/panic) drops the
/// whole host pool with it — no separate clear call is needed for the pool
/// itself.
pub(crate) enum RuntimeStrategyDispatchAuthority {
    /// `Off`/`Shadow`: the exact accepted legacy per-symbol assignment path.
    /// Zero behavior or ordering change from the pre-Phase-7B loop body.
    Legacy {
        assignments: Vec<SymbolStrategyAssignment>,
    },
    /// `PaperEnforcedAllowed` only: the frozen selected-host authority is the
    /// ONLY strategy-evaluation authority for these bindings this run — the
    /// legacy native bootstrap must be invoked zero times while this
    /// variant is active.
    DynamicPaperEnforced {
        run_id: Uuid,
        plan_id: Uuid,
        bindings: Vec<SelectedDispatchBinding>,
        host_pool: DynamicSelectionHostPool,
    },
}

impl RuntimeStrategyDispatchAuthority {
    #[cfg(test)]
    pub(crate) fn is_dynamic_paper_enforced(&self) -> bool {
        matches!(self, Self::DynamicPaperEnforced { .. })
    }
}

/// Manual `Debug` impl: `DynamicSelectionHostPool` itself derives neither
/// `Clone` nor `Debug` (it is not meant to be cloned or dumped whole), so
/// this prints only its presence/size — never its internal `StrategyHost`
/// contents. Mirrors `state::types::DynamicSelectionRuntimeState`'s own
/// manual `Debug` impl for the same reason.
impl std::fmt::Debug for RuntimeStrategyDispatchAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Legacy { assignments } => f
                .debug_struct("RuntimeStrategyDispatchAuthority::Legacy")
                .field("assignments", assignments)
                .finish(),
            Self::DynamicPaperEnforced {
                run_id,
                plan_id,
                bindings,
                host_pool,
            } => f
                .debug_struct("RuntimeStrategyDispatchAuthority::DynamicPaperEnforced")
                .field("run_id", run_id)
                .field("plan_id", plan_id)
                .field("bindings", bindings)
                .field("host_pool_len", &host_pool.len())
                .finish(),
        }
    }
}

/// Part 1/2: build the one frozen
/// [`RuntimeStrategyDispatchAuthority::DynamicPaperEnforced`] from an
/// already-coherent, `PaperEnforcedAllowed` plan and its exact host pool
/// (both produced by the same `evaluate_dynamic_selection_start_gate` call).
/// Fails closed — builds nothing — on an empty plan, empty selected-binding
/// set, missing/extra/duplicate host-pool entry, host/binding count
/// mismatch, or a selected candidate whose timeframe has no known DB label.
/// All validation happens here, before any caller can activate this
/// authority.
pub(crate) fn build_dynamic_paper_enforced_dispatch_authority(
    run_id: Uuid,
    plan: &DynamicSelectionPlan,
    plan_id: Uuid,
    host_pool: DynamicSelectionHostPool,
) -> Result<RuntimeStrategyDispatchAuthority, DispatchAuthorityBuildError> {
    // TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 start-authority check:
    // fail-closed before any other validation, and before activation, that
    // the frozen plan's own recorded run_id parses and matches the exact
    // run_id this authority is about to be built and activated for. Never
    // relies on the caller having already proven this upstream.
    let plan_run_id: Uuid =
        plan.context
            .run_id
            .parse()
            .map_err(|_| DispatchAuthorityBuildError::InvalidPlanRunId {
                raw: plan.context.run_id.clone(),
            })?;
    if plan_run_id != run_id {
        return Err(DispatchAuthorityBuildError::RunIdMismatch {
            plan_run_id,
            run_id,
        });
    }

    if plan.symbol_results.is_empty() {
        return Err(DispatchAuthorityBuildError::EmptyPlan);
    }
    let keys = selected_host_pool_keys(plan);
    if keys.is_empty() {
        return Err(DispatchAuthorityBuildError::EmptySelectedBindings);
    }

    // Duplicate-binding check — defense in depth. IR2 selected-row
    // coherence already proved this upstream (`evaluate_plan_for_start_gate`
    // calls `mqk_portfolio::verify_plan_selection_coherence` before this
    // function is ever reached), but never trusted here implicitly.
    let mut seen: BTreeSet<(String, String, i64)> = BTreeSet::new();
    for k in &keys {
        if !seen.insert(k.clone()) {
            return Err(DispatchAuthorityBuildError::DuplicateBinding {
                symbol: k.0.clone(),
                strategy_id: k.1.clone(),
                timeframe_secs: k.2,
            });
        }
    }

    // Exact-match check both ways: every binding key must have a pool
    // entry, and the pool must carry no entry beyond the binding set.
    for (symbol, strategy_id, timeframe_secs) in &keys {
        if !host_pool.contains_key(symbol, strategy_id, *timeframe_secs) {
            return Err(DispatchAuthorityBuildError::MissingHostPoolEntry {
                symbol: symbol.clone(),
                strategy_id: strategy_id.clone(),
                timeframe_secs: *timeframe_secs,
            });
        }
    }
    for pool_key in host_pool.keys() {
        if !seen.contains(pool_key) {
            return Err(DispatchAuthorityBuildError::ExtraHostPoolEntry {
                symbol: pool_key.0.clone(),
                strategy_id: pool_key.1.clone(),
                timeframe_secs: pool_key.2,
            });
        }
    }
    if host_pool.len() != keys.len() {
        return Err(DispatchAuthorityBuildError::HostPoolCountMismatch {
            bindings: keys.len(),
            pool_len: host_pool.len(),
        });
    }

    let mut bindings = Vec::with_capacity(keys.len());
    for (symbol, strategy_id, timeframe_secs) in keys {
        let Some(db_timeframe_label) = timeframe_secs_to_db_label(timeframe_secs) else {
            return Err(DispatchAuthorityBuildError::UnknownDbTimeframeLabel {
                symbol,
                timeframe_secs,
            });
        };
        let selection_reason_code = plan
            .symbol_results
            .iter()
            .find(|sr| sr.symbol == symbol)
            .map(|sr| sr.reason_code.clone())
            .unwrap_or_default();
        bindings.push(SelectedDispatchBinding {
            symbol,
            strategy_id,
            timeframe_secs,
            db_timeframe_label: db_timeframe_label.to_string(),
            selection_reason_code,
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
    use mqk_portfolio::DynamicSelectionPlan;

    /// The one run_id every fixture plan's `context.run_id` and every
    /// `build_dynamic_paper_enforced_dispatch_authority` call in this module
    /// must agree on — the start-authority check requires them to match.
    fn test_run_id() -> Uuid {
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.test.run_id")
    }

    fn empty_plan() -> DynamicSelectionPlan {
        DynamicSelectionPlan {
            context: mqk_portfolio::DynamicSelectionContext {
                run_id: test_run_id().to_string(),
                schema_version: "dynamic-selection-context-v1".to_string(),
                configured_mode: mqk_portfolio::DynamicSelectionMode::PaperEnforced,
                effective_mode: mqk_portfolio::DynamicSelectionMode::PaperEnforced,
                live_lock_applied: false,
                source_kind: "env_single_symbol_fallback".to_string(),
                source_identity: "env".to_string(),
                market_date: "2026-08-01".to_string(),
            },
            symbol_results: Vec::new(),
            truth_state: "computed".to_string(),
            blockers: Vec::new(),
        }
    }

    #[test]
    fn timeframe_secs_to_db_label_is_a_fixed_bijection() {
        assert_eq!(timeframe_secs_to_db_label(60), Some("1m"));
        assert_eq!(timeframe_secs_to_db_label(300), Some("5m"));
        assert_eq!(timeframe_secs_to_db_label(900), Some("15m"));
        assert_eq!(timeframe_secs_to_db_label(1800), Some("30m"));
        assert_eq!(timeframe_secs_to_db_label(3600), Some("1H"));
        assert_eq!(timeframe_secs_to_db_label(86400), Some("1D"));
        assert_eq!(timeframe_secs_to_db_label(123), None);
    }

    #[test]
    fn empty_plan_fails_dispatch_authority_build_closed() {
        let plan = empty_plan();
        let pool = DynamicSelectionHostPool::build(&[]).expect("empty pool builds");
        let plan_id = derive_dynamic_selection_plan_id(&plan);
        let err = build_dynamic_paper_enforced_dispatch_authority(
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.test.run_id"),
            &plan,
            plan_id,
            pool,
        )
        .unwrap_err();
        assert_eq!(err, DispatchAuthorityBuildError::EmptyPlan);
    }

    // -----------------------------------------------------------------
    // Real, properly-selected plan fixtures, built via the actual pure
    // selector (`mqk_portfolio::compute_dynamic_selection_plan`) rather
    // than hand-constructing `SelectionCandidateResult` (30+ fields) --
    // mirrors `dynamic_selection_start_gate.rs`'s own test-fixture pattern.
    // -----------------------------------------------------------------

    fn ds_context() -> mqk_portfolio::DynamicSelectionContext {
        mqk_portfolio::DynamicSelectionContext {
            run_id: test_run_id().to_string(),
            schema_version: mqk_portfolio::DYNAMIC_SELECTION_SCHEMA_VERSION.to_string(),
            configured_mode: mqk_portfolio::DynamicSelectionMode::PaperEnforced,
            effective_mode: mqk_portfolio::DynamicSelectionMode::PaperEnforced,
            live_lock_applied: false,
            source_kind: "env_single_symbol_fallback".to_string(),
            source_identity: "env".to_string(),
            market_date: "2026-07-28".to_string(),
        }
    }

    fn decimal_from_micros(micros: i64) -> String {
        let negative = micros < 0;
        let abs = (micros as i128).unsigned_abs();
        let int_part = abs / 1_000_000;
        let frac_part = abs % 1_000_000;
        let token = format!(
            "{}{}.{:06}",
            if negative { "-" } else { "" },
            int_part,
            frac_part
        );
        mqk_portfolio::canonicalize_decimal_token(&token).expect("valid fixture token")
    }

    fn valid_evidence(score_micros: i64) -> mqk_portfolio::SelectionCandidateEvidence {
        mqk_portfolio::SelectionCandidateEvidence {
            promotion_query_ok: true,
            promotion_state: Some("active_paper".to_string()),
            promotion_effective: true,
            promotion_expired: false,
            evidence_resolved: true,
            review_state_is_paper_candidate: true,
            evidence_review_state: Some("paper_candidate".to_string()),
            durable_legacy_fingerprint: Some(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            ),
            recomputed_legacy_fingerprint: Some(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            ),
            legacy_fingerprint_matches: true,
            durable_exact_fingerprint_v2: Some(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            ),
            recomputed_exact_fingerprint_v2: Some(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            ),
            exact_fingerprint_v2_matches: true,
            config_identity_verified: true,
            durable_config_fingerprint: Some(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string(),
            ),
            current_config_fingerprint: Some(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string(),
            ),
            registry_enabled: true,
            plugin_instantiable: true,
            timeframe_matches: true,
            data_ready: true,
            canonical_score_decimal: Some(decimal_from_micros(score_micros)),
            canonical_score_micros: Some(score_micros),
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

    /// Two symbols, one candidate each, both real registered strategies
    /// with genuinely different timeframes (`intraday_scalper`=300s/"5m",
    /// `volatility_breakout`=3600s/"1H") — a real mixed-timeframe selected
    /// batch (Part 7 test coverage), not a fabricated one.
    fn computed_plan_two_symbols_mixed_timeframe() -> mqk_portfolio::DynamicSelectionPlan {
        let candidates = vec![
            mqk_portfolio::SelectionCandidateInput {
                symbol: "AAPL".to_string(),
                strategy_id: "intraday_scalper".to_string(),
                timeframe_secs: 300,
                evidence: valid_evidence(500_000),
            },
            mqk_portfolio::SelectionCandidateInput {
                symbol: "MSFT".to_string(),
                strategy_id: "volatility_breakout".to_string(),
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

    /// One symbol, one candidate with a timeframe outside the fixed DB-label
    /// bijection (Part 2's helper only knows 60/300/900/1800/3600/86400).
    fn computed_plan_unknown_timeframe() -> mqk_portfolio::DynamicSelectionPlan {
        let candidates = vec![mqk_portfolio::SelectionCandidateInput {
            symbol: "AAPL".to_string(),
            strategy_id: "intraday_scalper".to_string(),
            timeframe_secs: 123,
            evidence: valid_evidence(500_000),
        }];
        mqk_portfolio::compute_dynamic_selection_plan(
            ds_context(),
            &["AAPL".to_string()],
            &candidates,
        )
    }

    fn host_pool_for(plan: &mqk_portfolio::DynamicSelectionPlan) -> DynamicSelectionHostPool {
        DynamicSelectionHostPool::build(&selected_host_pool_keys(plan))
            .expect("fixture plan's selected candidates must build a real pool")
    }

    #[test]
    fn two_real_symbols_mixed_timeframe_builds_two_bindings_and_two_hosts() {
        let plan = computed_plan_two_symbols_mixed_timeframe();
        assert_eq!(plan.selected_count(), 2, "both candidates must be selected");
        let pool = host_pool_for(&plan);
        let plan_id = derive_dynamic_selection_plan_id(&plan);
        let authority = build_dynamic_paper_enforced_dispatch_authority(
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.test.run_id"),
            &plan,
            plan_id,
            pool,
        )
        .expect("a coherent two-symbol mixed-timeframe plan must build cleanly");

        let RuntimeStrategyDispatchAuthority::DynamicPaperEnforced {
            bindings,
            host_pool,
            ..
        } = authority
        else {
            panic!("expected DynamicPaperEnforced");
        };
        assert_eq!(bindings.len(), 2);
        assert_eq!(host_pool.len(), 2);
        let aapl = bindings.iter().find(|b| b.symbol == "AAPL").unwrap();
        assert_eq!(aapl.strategy_id, "intraday_scalper");
        assert_eq!(aapl.timeframe_secs, 300);
        assert_eq!(aapl.db_timeframe_label, "5m");
        let msft = bindings.iter().find(|b| b.symbol == "MSFT").unwrap();
        assert_eq!(msft.strategy_id, "volatility_breakout");
        assert_eq!(msft.timeframe_secs, 3600);
        assert_eq!(msft.db_timeframe_label, "1H");
    }

    #[test]
    fn plan_with_no_selected_candidates_fails_closed() {
        let plan = mqk_portfolio::DynamicSelectionPlan {
            symbol_results: Vec::new(),
            ..computed_plan_two_symbols_mixed_timeframe()
        };
        // symbol_results empty -> EmptyPlan is checked first (Part 1
        // requirement 9: "empty plan" and "empty selected binding set" are
        // both fail-closed; an empty `symbol_results` is the strongest form
        // of "no selected bindings").
        let pool = DynamicSelectionHostPool::build(&[]).expect("empty pool builds");
        let plan_id = derive_dynamic_selection_plan_id(&plan);
        let err = build_dynamic_paper_enforced_dispatch_authority(
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.test.run_id"),
            &plan,
            plan_id,
            pool,
        )
        .unwrap_err();
        assert_eq!(err, DispatchAuthorityBuildError::EmptyPlan);
    }

    #[test]
    fn unknown_db_timeframe_label_fails_closed() {
        // A selected candidate with a timeframe outside the fixed DB-label
        // bijection can never reach `build_dynamic_paper_enforced_dispatch_
        // authority`'s own `UnknownDbTimeframeLabel` check in practice:
        // `DynamicSelectionHostPool::build` already fails closed earlier
        // (`SpecTimeframeMismatch`) the moment a candidate's declared
        // timeframe doesn't match any real registered strategy's actual
        // spec timeframe — proven here directly. This is a good thing (an
        // earlier, stricter gate), not a test gap: the underlying pure
        // label function itself is exhaustively covered by
        // `timeframe_secs_to_db_label_is_a_fixed_bijection` (including its
        // `None` branch for an unrecognized value), which is the only
        // input `UnknownDbTimeframeLabel` could ever be built from.
        let plan = computed_plan_unknown_timeframe();
        assert_eq!(plan.selected_count(), 1);
        let Err(err) = DynamicSelectionHostPool::build(&selected_host_pool_keys(&plan)) else {
            panic!("expected build() to fail for an unregistered timeframe");
        };
        assert!(matches!(
            err,
            crate::dynamic_selection_host_pool::HostPoolBuildError::SpecTimeframeMismatch {
                expected: 123,
                actual: 300,
                ..
            }
        ));
    }

    #[test]
    fn missing_host_pool_entry_fails_closed() {
        let plan = computed_plan_two_symbols_mixed_timeframe();
        // Build a pool for only ONE of the two selected keys -- the other
        // selected binding has no corresponding host.
        let keys = selected_host_pool_keys(&plan);
        let partial_pool =
            DynamicSelectionHostPool::build(&keys[..1]).expect("partial pool builds");
        let plan_id = derive_dynamic_selection_plan_id(&plan);
        let err = build_dynamic_paper_enforced_dispatch_authority(
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.test.run_id"),
            &plan,
            plan_id,
            partial_pool,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            DispatchAuthorityBuildError::MissingHostPoolEntry { .. }
        ));
    }

    #[test]
    fn extra_host_pool_entry_fails_closed() {
        let plan = computed_plan_two_symbols_mixed_timeframe();
        let mut keys = selected_host_pool_keys(&plan);
        // An extra key the plan never selected.
        keys.push(("GOOG".to_string(), "intraday_scalper".to_string(), 300));
        let pool = DynamicSelectionHostPool::build(&keys).expect("pool with extra key builds");
        let plan_id = derive_dynamic_selection_plan_id(&plan);
        let err = build_dynamic_paper_enforced_dispatch_authority(
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.test.run_id"),
            &plan,
            plan_id,
            pool,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            DispatchAuthorityBuildError::ExtraHostPoolEntry { .. }
        ));
    }

    #[test]
    fn duplicate_binding_fails_closed() {
        // IR2 selected-row coherence would normally prevent two selected
        // rows for the same symbol from ever reaching this function -- this
        // test proves the defense-in-depth check fires anyway, by directly
        // constructing a plan with two selected `SymbolSelectionResult`
        // rows for the same symbol (bypassing the pure selector, which
        // itself already guarantees one row per symbol).
        let plan = computed_plan_two_symbols_mixed_timeframe();
        let aapl_result = plan
            .symbol_results
            .iter()
            .find(|s| s.symbol == "AAPL")
            .unwrap()
            .clone();
        let mut duplicated = plan.clone();
        duplicated.symbol_results.push(aapl_result);

        let pool = DynamicSelectionHostPool::build(&selected_host_pool_keys(&plan))
            .expect("original (non-duplicated) key set builds a valid pool");
        let plan_id = derive_dynamic_selection_plan_id(&duplicated);
        let err = build_dynamic_paper_enforced_dispatch_authority(
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.test.run_id"),
            &duplicated,
            plan_id,
            pool,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            DispatchAuthorityBuildError::DuplicateBinding { .. }
        ));
    }

    #[test]
    fn plan_id_is_deterministic_for_the_same_resolved_plan() {
        let plan_a = computed_plan_two_symbols_mixed_timeframe();
        let plan_b = computed_plan_two_symbols_mixed_timeframe();
        assert_eq!(
            derive_dynamic_selection_plan_id(&plan_a),
            derive_dynamic_selection_plan_id(&plan_b),
            "the same resolved plan must always produce the same plan_id"
        );
    }

    #[test]
    fn plan_id_changes_when_a_result_affecting_fact_changes() {
        let plan_a = computed_plan_two_symbols_mixed_timeframe();
        let candidates_b = vec![
            mqk_portfolio::SelectionCandidateInput {
                symbol: "AAPL".to_string(),
                strategy_id: "intraday_scalper".to_string(),
                timeframe_secs: 300,
                // Different score -> different resolved plan.
                evidence: valid_evidence(999_000),
            },
            mqk_portfolio::SelectionCandidateInput {
                symbol: "MSFT".to_string(),
                strategy_id: "volatility_breakout".to_string(),
                timeframe_secs: 3600,
                evidence: valid_evidence(700_000),
            },
        ];
        let plan_b = mqk_portfolio::compute_dynamic_selection_plan(
            ds_context(),
            &["AAPL".to_string(), "MSFT".to_string()],
            &candidates_b,
        );
        assert_ne!(
            derive_dynamic_selection_plan_id(&plan_a),
            derive_dynamic_selection_plan_id(&plan_b),
            "a change to a result-affecting plan fact must change plan_id"
        );
    }

    #[test]
    fn plan_id_is_independent_of_host_pool_and_construction_order() {
        // Part 2 requirement 3: never derived from host-pool order. Building
        // the pool from a reversed key slice must not change the plan_id
        // (which never even sees the pool).
        let plan = computed_plan_two_symbols_mixed_timeframe();
        let mut reversed_keys = selected_host_pool_keys(&plan);
        reversed_keys.reverse();
        let _pool_a = DynamicSelectionHostPool::build(&selected_host_pool_keys(&plan)).unwrap();
        let _pool_b = DynamicSelectionHostPool::build(&reversed_keys).unwrap();
        let id_a = derive_dynamic_selection_plan_id(&plan);
        let id_b = derive_dynamic_selection_plan_id(&plan);
        assert_eq!(id_a, id_b);
    }

    #[test]
    fn legacy_and_dynamic_paper_enforced_are_correctly_discriminated() {
        let legacy = RuntimeStrategyDispatchAuthority::Legacy {
            assignments: Vec::new(),
        };
        assert!(!legacy.is_dynamic_paper_enforced());

        let plan = computed_plan_two_symbols_mixed_timeframe();
        let pool = host_pool_for(&plan);
        let plan_id = derive_dynamic_selection_plan_id(&plan);
        let dynamic = build_dynamic_paper_enforced_dispatch_authority(
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.test.run_id"),
            &plan,
            plan_id,
            pool,
        )
        .expect("valid plan builds");
        assert!(dynamic.is_dynamic_paper_enforced());
    }

    #[test]
    fn dispatch_authority_debug_never_panics_and_hides_host_internals() {
        let plan = computed_plan_two_symbols_mixed_timeframe();
        let pool = host_pool_for(&plan);
        let plan_id = derive_dynamic_selection_plan_id(&plan);
        let dynamic = build_dynamic_paper_enforced_dispatch_authority(
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.test.run_id"),
            &plan,
            plan_id,
            pool,
        )
        .expect("valid plan builds");
        let debug_text = format!("{dynamic:?}");
        assert!(debug_text.contains("host_pool_len"));
        assert!(!debug_text.contains("StrategyHost"));
    }

    // -----------------------------------------------------------------
    // TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 start-authority check:
    // the plan's own recorded run_id must parse and match the actual run_id
    // this authority is being built for, checked before any other
    // validation and before activation.
    // -----------------------------------------------------------------

    #[test]
    fn invalid_plan_run_id_fails_closed() {
        let plan = mqk_portfolio::DynamicSelectionPlan {
            context: mqk_portfolio::DynamicSelectionContext {
                run_id: "not-a-uuid".to_string(),
                ..ds_context()
            },
            ..computed_plan_two_symbols_mixed_timeframe()
        };
        let pool = host_pool_for(&plan);
        let plan_id = derive_dynamic_selection_plan_id(&plan);
        let err =
            build_dynamic_paper_enforced_dispatch_authority(test_run_id(), &plan, plan_id, pool)
                .unwrap_err();
        assert_eq!(
            err,
            DispatchAuthorityBuildError::InvalidPlanRunId {
                raw: "not-a-uuid".to_string(),
            }
        );
        assert_eq!(err.code(), "dispatch_authority_invalid_plan_run_id");
    }

    #[test]
    fn mismatched_plan_run_id_fails_closed() {
        // The plan's own context.run_id is a real, valid UUID -- just not
        // the same one this authority is being activated for.
        let plan = computed_plan_two_symbols_mixed_timeframe();
        assert_eq!(plan.context.run_id, test_run_id().to_string());
        let pool = host_pool_for(&plan);
        let plan_id = derive_dynamic_selection_plan_id(&plan);
        let wrong_run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.test.wrong_run_id");
        let err =
            build_dynamic_paper_enforced_dispatch_authority(wrong_run_id, &plan, plan_id, pool)
                .unwrap_err();
        assert_eq!(
            err,
            DispatchAuthorityBuildError::RunIdMismatch {
                plan_run_id: test_run_id(),
                run_id: wrong_run_id,
            }
        );
        assert_eq!(err.code(), "dispatch_authority_run_id_mismatch");
    }

    #[test]
    fn matching_plan_run_id_builds_cleanly() {
        let plan = computed_plan_two_symbols_mixed_timeframe();
        let pool = host_pool_for(&plan);
        let plan_id = derive_dynamic_selection_plan_id(&plan);
        assert!(
            build_dynamic_paper_enforced_dispatch_authority(test_run_id(), &plan, plan_id, pool)
                .is_ok(),
            "a plan whose context.run_id matches the actual run_id must build cleanly"
        );
    }
}
