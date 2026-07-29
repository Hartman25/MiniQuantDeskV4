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
//! Bundle 7 has authority *only* when `effective_mode == PaperEnforced`.
//!
//! **`Off`** (configured `Off`, or `Shadow`/`PaperEnforced` demoted there by
//! the live lock) is the *only* mode that performs **no I/O of any kind** —
//! it returns `not_applicable = true, allowed = true` and the caller must
//! proceed exactly as if Bundle 7 did not exist for this run (legacy Tier A
//! dispatch, unaffected).
//!
//! **`Shadow`** is genuinely effective and is *not* `not_applicable`: it
//! performs real I/O (builds and identifies a real
//! [`DynamicSelectionPlan`] via [`build_dynamic_selection_plan`]), but is
//! never permitted to affect economic dispatch or hold a host pool — see
//! Defect C below and [`DynamicSelectionStartGateDisposition::ShadowAllowed`]/
//! [`DynamicSelectionStartGateDisposition::ShadowInvalid`]. Conflating Shadow
//! with Off's "no I/O" contract was a stale-documentation defect; the two are
//! not the same disposition and must never be described as if they were.
//!
//! `allowed = false` (only reachable via `PaperEnforcedRefused`) means "the
//! operator explicitly configured `paper_enforced`, it is genuinely in
//! effect, and it cannot be honored" — which must refuse the *whole* run
//! start, never silently fall back to legacy single-strategy dispatch
//! (fail-closed over fail-open). Shadow, by construction, can never produce
//! `allowed = false` — see Defect C.
//!
//! # Defect C — shadow failure truth
//! Shadow must never be indistinguishable from a successful, computed plan
//! when it is not one. `effective_mode == Shadow` resolves to exactly one of:
//! - [`DynamicSelectionStartGateDisposition::ShadowAllowed`] — a `computed`
//!   plan with at least one selection: Shadow's own explicitly *positive*
//!   outcome.
//! - [`DynamicSelectionStartGateDisposition::ShadowInvalid`] — context
//!   incoherence, DB unavailable, a non-`computed` plan `truth_state`, or a
//!   `computed` plan with zero selections across the whole universe: a
//!   *bounded, distinct* refused-shaped outcome (see
//!   [`DynamicSelectionStartGateReason`]), carried on `reasons`.
//!
//! In both cases `allowed` is always `true` (Shadow failures never block
//! legacy economic operation) and `host_pool` is always `None` (Shadow never
//! constructs or retains a host pool) — only the disposition and `reasons`
//! distinguish a genuine positive outcome from an invalid one. A Shadow
//! failure is never labeled `ShadowAllowed`.
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
use crate::state::market_calendar::{resolve_market_session_schedule, MarketCalendarProvider};
use crate::state::{MultiSymbolConfigSource, MultiSymbolRuntimeConfig};

/// Defect D: `"YYYY-MM-DD"` formatting for a market-date tuple, mirroring
/// the identical helper already used by
/// `routes::market_data_readiness::format_market_date` and
/// `state::autonomous_daily_operation::format_market_date` (duplicated
/// rather than imported across modules for a one-line helper, matching this
/// codebase's existing convention for this exact function).
fn format_market_date(date: (i64, i64, i64)) -> String {
    format!("{:04}-{:02}-{:02}", date.0, date.1, date.2)
}

/// Defect D: derive the *authoritative* `(source_kind, source_identity)`
/// pair from the actual [`MultiSymbolConfigSource`] the run is using --
/// `"env_single_symbol_fallback"`/`"env"` for the legacy fallback,
/// `"watchlist_v2"`/the artifact's own path for an approved watchlist-v2
/// artifact. A caller-supplied context can never claim one kind while
/// carrying the other kind's identity (or vice versa): both fields are
/// always derived together, from the same real source, never independently.
fn source_kind_and_identity(source: &MultiSymbolConfigSource) -> (String, String) {
    match source {
        MultiSymbolConfigSource::EnvSingleSymbolFallback => {
            ("env_single_symbol_fallback".to_string(), "env".to_string())
        }
        MultiSymbolConfigSource::WatchlistArtifactV2 { path } => {
            ("watchlist_v2".to_string(), path.clone())
        }
    }
}

/// Defect D: build the authoritative [`DynamicSelectionContext`] this start
/// gate evaluation actually corresponds to -- every field derived from a real
/// authority source, never a caller-supplied opaque string:
///
/// - `run_id`: the actual run id this evaluation was invoked for.
/// - `schema_version`: the current pure-model schema version constant.
/// - `configured_mode`/`effective_mode`/`live_lock_applied`: the resolver's
///   own [`EffectiveDynamicSelectionMode`] output.
/// - `source_kind`/`source_identity`: derived together from the actual
///   [`MultiSymbolRuntimeConfig::source`] (see [`source_kind_and_identity`])
///   -- a watchlist-v2 run and an env-fallback run can never impersonate one
///   another here.
/// - `market_date`: the accepted market-calendar/session schedule's own
///   `market_date`, resolved from `calendar_provider` at `now_utc` --
///   never a caller-supplied date string.
///
/// This is the value [`evaluate_dynamic_selection_start_gate`] treats as
/// ground truth and compares the caller-supplied context against
/// ([`validate_context_coherence`]) -- it is never itself trusted from a
/// caller.
pub fn build_dynamic_selection_context(
    run_id: &str,
    effective_mode: &EffectiveDynamicSelectionMode,
    multi_symbol_config: &MultiSymbolRuntimeConfig,
    calendar_provider: &dyn MarketCalendarProvider,
    now_utc: DateTime<Utc>,
) -> DynamicSelectionContext {
    let (source_kind, source_identity) = source_kind_and_identity(&multi_symbol_config.source);
    let schedule = resolve_market_session_schedule(calendar_provider, now_utc);

    DynamicSelectionContext {
        run_id: run_id.to_string(),
        schema_version: mqk_portfolio::DYNAMIC_SELECTION_SCHEMA_VERSION.to_string(),
        configured_mode: effective_mode.configured_mode,
        effective_mode: effective_mode.effective_mode,
        live_lock_applied: effective_mode.live_lock_applied,
        source_kind,
        source_identity,
        market_date: format_market_date(schedule.market_date),
    }
}

/// Closed, bounded reason-code vocabulary for a refused start-gate
/// evaluation. Every distinct failure mode Phase 6 named gets its own
/// variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DynamicSelectionStartGateReason {
    DbUnavailable,
    /// IR3: the caller-supplied [`DynamicSelectionContext`] does not agree
    /// with the resolver's own [`EffectiveDynamicSelectionMode`] output
    /// and/or the run/source facts this evaluation was actually invoked
    /// with. Never trust a context object's self-reported mode/source --
    /// always prove it against the authoritative inputs first.
    ContextIncoherent {
        detail: &'static str,
    },
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
    /// IR2: a symbol's selected-row shape fails
    /// [`mqk_portfolio::verify_symbol_selection_coherence`] -- e.g. two
    /// candidates marked selected, a selected id with no matching selected
    /// candidate, or a selected candidate whose disposition/timeframe is
    /// invalid. Never silently pick the first `selected=true` row; refuse
    /// the whole start instead.
    SelectedRowCoherenceViolation {
        symbol: String,
        violation: &'static str,
    },
    /// [`DynamicSelectionHostPool::build`] failed -- covers duplicate key,
    /// unknown strategy, registry inconsistency, and spec mismatches, all
    /// via that function's own [`HostPoolBuildError`].
    HostPoolBuildFailed {
        code: &'static str,
        detail: String,
    },
    /// IR2 defense-in-depth: the host pool that was successfully built does
    /// not contain exactly one host per selected symbol. Unreachable given
    /// the coherence check above and `DynamicSelectionHostPool::build`'s own
    /// fail-closed duplicate-key handling, but checked explicitly rather
    /// than assumed.
    HostPoolCountMismatch {
        selected_count: usize,
        pool_count: usize,
    },
}

impl DynamicSelectionStartGateReason {
    pub fn code(&self) -> &'static str {
        match self {
            Self::DbUnavailable => "dynamic_selection_start_gate_db_unavailable",
            Self::ContextIncoherent { .. } => "dynamic_selection_start_gate_context_incoherent",
            Self::PlanInvalid { .. } => "dynamic_selection_start_gate_plan_invalid",
            Self::NoSelectedPair => "dynamic_selection_start_gate_no_selected_pair",
            Self::SymbolMissingRequiredSelection { .. } => {
                "dynamic_selection_start_gate_symbol_missing_required_selection"
            }
            Self::SelectedRowCoherenceViolation { .. } => {
                "dynamic_selection_start_gate_selected_row_coherence_violation"
            }
            Self::HostPoolBuildFailed { .. } => {
                "dynamic_selection_start_gate_host_pool_build_failed"
            }
            Self::HostPoolCountMismatch { .. } => {
                "dynamic_selection_start_gate_host_pool_count_mismatch"
            }
        }
    }
}

/// IR1: the exact mode-driven path this start-gate evaluation took. Distinct
/// from `allowed`/`not_applicable` alone -- gives every caller (lifecycle
/// wiring, durable evidence writer, operator surface) an unambiguous,
/// closed-vocabulary answer to "which of the four contractually distinct
/// outcomes happened here", never inferred from a combination of booleans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicSelectionStartGateDisposition {
    /// `effective_mode == Off` (whether configured `Off` or demoted there by
    /// the live lock). Zero selection DB/filesystem/host/persistence work;
    /// the caller proceeds exactly as if Bundle 7 did not exist.
    Off,
    /// `effective_mode == Shadow`: a `computed` plan with at least one
    /// selection was built, validated, and identified for evidence purposes
    /// -- Shadow's own explicitly *positive* outcome (Defect C). Never blocks
    /// run start (`allowed` is always `true`), never retains a host pool, and
    /// has zero economic effect -- legacy dispatch is untouched.
    ShadowAllowed,
    /// `effective_mode == Shadow`, but the outcome is not a genuine positive
    /// one (Defect C) -- covers context/mode incoherence (IR3), DB
    /// unavailable, a non-`computed` plan `truth_state`, or a `computed` plan
    /// with zero selections. `reasons` always carries a bounded explanation.
    /// Still never blocks run start (`allowed` is always `true`) and never
    /// retains a host pool -- Shadow has no economic authority to withhold,
    /// and a Shadow failure is never labeled `ShadowAllowed`.
    ShadowInvalid,
    /// `effective_mode == PaperEnforced` and every requirement passed: a
    /// validated plan and a constructed host pool are both present.
    PaperEnforcedAllowed,
    /// `effective_mode == PaperEnforced` and at least one requirement
    /// failed. Blocks the whole run start (`allowed = false`) -- never
    /// falls back to legacy single-strategy dispatch.
    PaperEnforcedRefused,
}

/// Result of evaluating one start attempt.
pub struct DynamicSelectionStartGateOutcome {
    /// IR1: see [`DynamicSelectionStartGateDisposition`].
    pub disposition: DynamicSelectionStartGateDisposition,
    /// `true` only for [`DynamicSelectionStartGateDisposition::Off`] --
    /// Bundle 7 has no authority for this start attempt at all. When `true`,
    /// `allowed` is always also `true`, `plan`/`host_pool` are always
    /// `None`, and `reasons` is always empty -- the caller must treat this
    /// exactly as "Bundle 7 does not exist for this run."
    pub not_applicable: bool,
    pub allowed: bool,
    pub plan: Option<DynamicSelectionPlan>,
    pub host_pool: Option<DynamicSelectionHostPool>,
    pub reasons: Vec<DynamicSelectionStartGateReason>,
}

/// Every selected `(symbol, strategy_id, timeframe_secs)` triple from an
/// already-coherent (see [`mqk_portfolio::verify_plan_selection_coherence`])
/// `computed` plan -- the exact identity triple
/// [`DynamicSelectionHostPool::build`] keys on, derived from each symbol's
/// selected candidate row (never a separate/global timeframe). Callers must
/// verify plan coherence before calling this; it trusts the "exactly zero or
/// one selected candidate per symbol" invariant coherence proves rather than
/// re-deriving it.
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
/// full DB/artifact/calendar composition to exercise every branch. Always
/// produces a `PaperEnforced*` disposition -- this function is only ever
/// reached on the `paper_enforced` path (see module docs, "`not_applicable`
/// vs `allowed`").
fn evaluate_plan_for_start_gate(plan: DynamicSelectionPlan) -> DynamicSelectionStartGateOutcome {
    let mut reasons = Vec::new();

    let refused = |plan: DynamicSelectionPlan, reasons: Vec<DynamicSelectionStartGateReason>| {
        DynamicSelectionStartGateOutcome {
            disposition: DynamicSelectionStartGateDisposition::PaperEnforcedRefused,
            not_applicable: false,
            allowed: false,
            plan: Some(plan),
            host_pool: None,
            reasons,
        }
    };

    if plan.truth_state != DYNAMIC_SELECTION_TRUTH_STATE_COMPUTED {
        reasons.push(DynamicSelectionStartGateReason::PlanInvalid {
            truth_state: plan.truth_state.clone(),
        });
        return refused(plan, reasons);
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
        return refused(plan, reasons);
    }

    // IR2: prove selected-row coherence before deriving any host-pool key
    // from it -- never trust "first candidate with selected=true" implicitly.
    let violations = mqk_portfolio::verify_plan_selection_coherence(&plan);
    if !violations.is_empty() {
        let reasons = violations
            .into_iter()
            .map(
                |(symbol, v)| DynamicSelectionStartGateReason::SelectedRowCoherenceViolation {
                    symbol,
                    violation: v.code(),
                },
            )
            .collect();
        return refused(plan, reasons);
    }

    let selected = selected_host_pool_keys(&plan);
    let selected_count = plan.selected_count();
    match DynamicSelectionHostPool::build(&selected) {
        Ok(pool) => {
            // IR2 defense-in-depth: host-key count and pool count must equal
            // selected count. `DynamicSelectionHostPool::build` already
            // fails closed on any duplicate key, so this is unreachable
            // given the coherence check above -- checked explicitly anyway,
            // never assumed.
            if pool.len() != selected_count || selected.len() != selected_count {
                return refused(
                    plan,
                    vec![DynamicSelectionStartGateReason::HostPoolCountMismatch {
                        selected_count,
                        pool_count: pool.len(),
                    }],
                );
            }
            DynamicSelectionStartGateOutcome {
                disposition: DynamicSelectionStartGateDisposition::PaperEnforcedAllowed,
                not_applicable: false,
                allowed: true,
                plan: Some(plan),
                host_pool: Some(pool),
                reasons: Vec::new(),
            }
        }
        Err(e) => refused(
            plan,
            vec![DynamicSelectionStartGateReason::HostPoolBuildFailed {
                code: e.code(),
                detail: host_pool_error_detail(&e),
            }],
        ),
    }
}

fn host_pool_error_detail(e: &HostPoolBuildError) -> String {
    format!("{e:?}")
}

/// Defect C: evaluate an already-built Shadow-mode
/// [`DynamicSelectionPlan`] for its shadow-truth outcome -- split out from
/// [`evaluate_dynamic_selection_start_gate`] so it can be tested directly
/// against hand-constructed plan fixtures, mirroring
/// [`evaluate_plan_for_start_gate`]'s split for the `PaperEnforced` path.
///
/// Always produces a `Shadow*` disposition, always `allowed = true` (Shadow
/// never blocks legacy economic operation), and always `host_pool = None`
/// (Shadow never constructs or retains a host pool) -- only the disposition
/// and `reasons` distinguish a genuine positive outcome
/// ([`DynamicSelectionStartGateDisposition::ShadowAllowed`]: `computed` truth
/// state, at least one selection) from an invalid one
/// ([`DynamicSelectionStartGateDisposition::ShadowInvalid`]: non-`computed`
/// truth state, or `computed` with zero selections).
fn evaluate_shadow_plan_outcome(plan: DynamicSelectionPlan) -> DynamicSelectionStartGateOutcome {
    if plan.truth_state != DYNAMIC_SELECTION_TRUTH_STATE_COMPUTED {
        let truth_state = plan.truth_state.clone();
        return DynamicSelectionStartGateOutcome {
            disposition: DynamicSelectionStartGateDisposition::ShadowInvalid,
            not_applicable: false,
            allowed: true,
            plan: Some(plan),
            host_pool: None,
            reasons: vec![DynamicSelectionStartGateReason::PlanInvalid { truth_state }],
        };
    }

    if plan.selected_count() == 0 {
        return DynamicSelectionStartGateOutcome {
            disposition: DynamicSelectionStartGateDisposition::ShadowInvalid,
            not_applicable: false,
            allowed: true,
            plan: Some(plan),
            host_pool: None,
            reasons: vec![DynamicSelectionStartGateReason::NoSelectedPair],
        };
    }

    DynamicSelectionStartGateOutcome {
        disposition: DynamicSelectionStartGateDisposition::ShadowAllowed,
        not_applicable: false,
        allowed: true,
        plan: Some(plan),
        host_pool: None,
        reasons: Vec::new(),
    }
}

/// Defect D: prove the caller-supplied [`DynamicSelectionContext`] agrees,
/// field-for-field, with `expected` -- a context built entirely from real
/// authority sources by [`build_dynamic_selection_context`] (the resolver's
/// own mode/live-lock output, the run actually being started, the actual
/// [`MultiSymbolRuntimeConfig::source`], and the accepted market-calendar
/// schedule). A caller-supplied nonblank string is *never* accepted as
/// authority on its own -- `source_kind`/`source_identity`/`market_date` are
/// no longer merely checked for blankness, they are checked for exact
/// equality with what the real sources actually produced, so a
/// watchlist-v2 run can never impersonate an env-fallback run's context (or
/// vice versa) by supplying a nonblank-but-wrong pair.
///
/// Returns the first incoherence found (fixed check order, never a random
/// pick). This is the same rule the read-side historical validator (Phase 9)
/// must apply to a durably-persisted context.
fn validate_context_coherence(
    context: &DynamicSelectionContext,
    expected: &DynamicSelectionContext,
) -> Result<(), &'static str> {
    if context.schema_version != expected.schema_version {
        return Err("context.schema_version does not match the current pure-model schema version");
    }
    if context.configured_mode != expected.configured_mode {
        return Err("context.configured_mode does not match the resolver's configured_mode");
    }
    if context.effective_mode != expected.effective_mode {
        return Err("context.effective_mode does not match the resolver's effective_mode");
    }
    if context.live_lock_applied != expected.live_lock_applied {
        return Err("context.live_lock_applied does not match the resolver's live_lock_applied");
    }
    if context.run_id.trim().is_empty() {
        return Err("context.run_id is blank");
    }
    if context.run_id != expected.run_id {
        return Err("context.run_id does not match the run being started");
    }
    if context.source_kind.trim().is_empty() {
        return Err("context.source_kind is blank");
    }
    if context.source_kind != expected.source_kind {
        return Err(
            "context.source_kind does not match the actual MultiSymbolRuntimeConfig source",
        );
    }
    if context.source_identity.trim().is_empty() {
        return Err("context.source_identity is blank");
    }
    if context.source_identity != expected.source_identity {
        return Err(
            "context.source_identity does not match the actual MultiSymbolRuntimeConfig source identity",
        );
    }
    if context.market_date.trim().is_empty() {
        return Err("context.market_date is blank");
    }
    if context.market_date != expected.market_date {
        return Err("context.market_date does not match the accepted market-calendar session date");
    }
    Ok(())
}

/// Full orchestration: mode gate -> context coherence (IR3) -> plan build
/// (Phase 4) -> start-gate evaluation + host pool build. See module docs and
/// [`DynamicSelectionStartGateDisposition`] for the complete contract.
///
/// `run_id` is the authoritative identity of the run actually being
/// started — supplied independently of `context.run_id` so IR3's coherence
/// check has something outside `context` itself to prove `context` against
/// (a context can never authenticate itself).
///
/// # IR1 — mode-driven behavior
/// - `Off` (configured or live-lock-demoted): [`DynamicSelectionStartGateDisposition::Off`],
///   zero I/O, exactly the legacy path.
/// - `Shadow`: builds, validates, and identifies the same plan
///   `PaperEnforced` would build (context coherence permitting) —
///   [`DynamicSelectionStartGateDisposition::ShadowAllowed`]/`ShadowInvalid`
///   (Defect C). Never retains a host pool, never blocks run start (`allowed`
///   is always `true`), and has zero economic effect.
/// - `PaperEnforced`: builds/validates/persists* the plan and constructs the
///   host pool — [`DynamicSelectionStartGateDisposition::PaperEnforcedAllowed`]/`PaperEnforcedRefused`.
///   Refusal blocks the whole run start. (*Durable persistence is Phase 8;
///   this evaluator always returns the resolved plan to the caller so a
///   durable writer always has something to persist — see
///   `plan_identity_material_is_recoverable_from_a_refused_outcome`.)
pub async fn evaluate_dynamic_selection_start_gate(
    ctx: &DynamicSelectionPlanBuildContext<'_>,
    multi_symbol_config: &MultiSymbolRuntimeConfig,
    configured_strategy_ids: &[String],
    effective_mode: &EffectiveDynamicSelectionMode,
    context: DynamicSelectionContext,
    run_id: &str,
    now_utc: DateTime<Utc>,
) -> DynamicSelectionStartGateOutcome {
    if effective_mode.effective_mode == DynamicSelectionMode::Off {
        return DynamicSelectionStartGateOutcome {
            disposition: DynamicSelectionStartGateDisposition::Off,
            not_applicable: true,
            allowed: true,
            plan: None,
            host_pool: None,
            reasons: Vec::new(),
        };
    }

    // Defect D: the authoritative context this evaluation actually
    // corresponds to, derived entirely from real sources -- never trusted
    // from `context` itself.
    let expected_context = build_dynamic_selection_context(
        run_id,
        effective_mode,
        multi_symbol_config,
        ctx.calendar_provider,
        now_utc,
    );

    if let Err(detail) = validate_context_coherence(&context, &expected_context) {
        let disposition = if effective_mode.effective_mode == DynamicSelectionMode::Shadow {
            DynamicSelectionStartGateDisposition::ShadowInvalid
        } else {
            DynamicSelectionStartGateDisposition::PaperEnforcedRefused
        };
        // Shadow never blocks run start even when its own context is
        // incoherent -- it has no economic authority to withhold.
        let allowed = disposition != DynamicSelectionStartGateDisposition::PaperEnforcedRefused;
        return DynamicSelectionStartGateOutcome {
            disposition,
            not_applicable: false,
            allowed,
            plan: None,
            host_pool: None,
            reasons: vec![DynamicSelectionStartGateReason::ContextIncoherent { detail }],
        };
    }

    if effective_mode.effective_mode == DynamicSelectionMode::Shadow {
        // Defect C: Shadow failures must be distinguishable from a genuine
        // positive outcome -- DB unavailable is checked *before* attempting
        // to build a plan (a plan built with no DB pool would just collapse
        // every candidate to `promotion_query_failed`, silently masking the
        // real cause).
        if ctx.db.is_none() {
            return DynamicSelectionStartGateOutcome {
                disposition: DynamicSelectionStartGateDisposition::ShadowInvalid,
                not_applicable: false,
                allowed: true,
                plan: None,
                host_pool: None,
                reasons: vec![DynamicSelectionStartGateReason::DbUnavailable],
            };
        }

        // Shadow builds/validates/identifies the same plan PaperEnforced
        // would build -- but never retains a host pool and never blocks run
        // start, regardless of the plan's own outcome.
        let plan = build_dynamic_selection_plan(
            ctx,
            multi_symbol_config,
            configured_strategy_ids,
            context,
            now_utc,
        )
        .await;

        return evaluate_shadow_plan_outcome(plan);
    }

    // effective_mode.effective_mode == PaperEnforced from here.
    if ctx.db.is_none() {
        return DynamicSelectionStartGateOutcome {
            disposition: DynamicSelectionStartGateDisposition::PaperEnforcedRefused,
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

    /// Mirrors `mqk_portfolio::dynamic_selection`'s own test helper of the
    /// same name -- builds the canonical decimal string exactly
    /// corresponding to a micros value via the real canonicalizer.
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

    fn valid_evidence(score_micros: i64) -> SelectionCandidateEvidence {
        SelectionCandidateEvidence {
            promotion_query_ok: true,
            promotion_state: Some("active_paper".to_string()),
            promotion_effective: true,
            promotion_expired: false,
            evidence_resolved: true,
            review_state_is_paper_candidate: true,
            evidence_review_state: Some("paper_candidate".to_string()),
            durable_legacy_fingerprint: Some("fp-1".to_string()),
            recomputed_legacy_fingerprint: Some("fp-1".to_string()),
            legacy_fingerprint_matches: true,
            durable_exact_fingerprint_v2: Some("fp-v2-1".to_string()),
            recomputed_exact_fingerprint_v2: Some("fp-v2-1".to_string()),
            exact_fingerprint_v2_matches: true,
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
    async fn mode_off_is_not_applicable_with_zero_io() {
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
        // IR1: live-lock-demoted Shadow -> Off is still exactly the Off
        // path, zero I/O, regardless of what was originally configured.
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
            "run-1",
            Utc::now(),
        )
        .await;
        assert_eq!(
            outcome.disposition,
            DynamicSelectionStartGateDisposition::Off
        );
        assert!(outcome.not_applicable);
        assert!(outcome.allowed);
        assert!(outcome.plan.is_none());
        assert!(outcome.host_pool.is_none());
        assert!(outcome.reasons.is_empty());
    }

    /// Defect C: Shadow with no DB pool must be distinguishable from a
    /// genuine positive outcome -- `ShadowInvalid` with a `DbUnavailable`
    /// reason, never `ShadowAllowed` with an empty `reasons` list. Still
    /// never blocks run start and never retains a host pool.
    #[tokio::test]
    async fn effective_shadow_mode_with_no_db_is_shadow_invalid_not_shadow_allowed() {
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
            configured_mode: DynamicSelectionMode::Shadow,
            effective_mode: DynamicSelectionMode::Shadow,
            invalid_configuration: None,
            live_lock_applied: false,
        };
        let now_utc = Utc::now();
        // Defect D: build a genuinely coherent context via the same
        // authority-derivation function the evaluator itself uses -- never
        // a hand-typed literal date, which would silently drift out of sync
        // with `now_utc` and turn this test's fixture into a hidden
        // ContextIncoherent failure on any day other than the one it was
        // written on.
        let shadow_ds_context =
            build_dynamic_selection_context("run-1", &effective, &cfg, &calendar, now_utc);

        let outcome = evaluate_dynamic_selection_start_gate(
            &ctx,
            &cfg,
            &[],
            &effective,
            shadow_ds_context,
            "run-1",
            now_utc,
        )
        .await;
        assert_eq!(
            outcome.disposition,
            DynamicSelectionStartGateDisposition::ShadowInvalid,
            "no DB must never be labeled ShadowAllowed"
        );
        assert!(!outcome.not_applicable, "shadow performs real I/O");
        assert!(outcome.allowed, "shadow never blocks run start");
        assert!(
            outcome.plan.is_none(),
            "no plan is built when DB is unavailable"
        );
        assert!(
            outcome.host_pool.is_none(),
            "shadow must never retain a host pool"
        );
        assert_eq!(
            outcome.reasons,
            vec![DynamicSelectionStartGateReason::DbUnavailable],
            "shadow invalid must carry a bounded reason, never an empty list"
        );
    }

    /// Defect C: a `computed` Shadow plan with zero selections across the
    /// whole universe must be `ShadowInvalid` (`NoSelectedPair`), never
    /// `ShadowAllowed` -- a Shadow "success" with nothing selected is not a
    /// genuine positive outcome.
    #[test]
    fn shadow_plan_with_zero_selections_is_shadow_invalid() {
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

        let outcome = evaluate_shadow_plan_outcome(plan);
        assert_eq!(
            outcome.disposition,
            DynamicSelectionStartGateDisposition::ShadowInvalid
        );
        assert!(outcome.allowed, "shadow never blocks run start");
        assert!(outcome.host_pool.is_none());
        assert_eq!(
            outcome.reasons,
            vec![DynamicSelectionStartGateReason::NoSelectedPair]
        );
    }

    /// Defect C: a non-`computed` plan `truth_state` in Shadow mode must be
    /// `ShadowInvalid` (`PlanInvalid`), never `ShadowAllowed`.
    #[test]
    fn shadow_non_computed_plan_is_shadow_invalid() {
        let plan = mqk_portfolio::compute_dynamic_selection_plan(ds_context(), &[], &[]);
        assert_ne!(
            plan.truth_state,
            mqk_portfolio::DYNAMIC_SELECTION_TRUTH_STATE_COMPUTED
        );

        let outcome = evaluate_shadow_plan_outcome(plan);
        assert_eq!(
            outcome.disposition,
            DynamicSelectionStartGateDisposition::ShadowInvalid
        );
        assert!(outcome.allowed);
        assert!(outcome
            .reasons
            .iter()
            .any(|r| matches!(r, DynamicSelectionStartGateReason::PlanInvalid { .. })));
    }

    /// Defect C: a genuinely valid, `computed`, non-empty-selection Shadow
    /// plan is the one and only path to `ShadowAllowed`.
    #[test]
    fn shadow_valid_computed_plan_with_a_selection_is_shadow_allowed() {
        let plan = computed_plan_all_selected();
        let outcome = evaluate_shadow_plan_outcome(plan);
        assert_eq!(
            outcome.disposition,
            DynamicSelectionStartGateDisposition::ShadowAllowed
        );
        assert!(outcome.allowed);
        assert!(outcome.reasons.is_empty());
        assert!(outcome.host_pool.is_none());
        assert!(outcome.plan.is_some());
    }

    /// IR3: a context whose `effective_mode` disagrees with the resolver's
    /// own output must be refused before any plan build is attempted.
    #[tokio::test]
    async fn incoherent_context_effective_mode_is_refused_before_plan_build() {
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
            configured_mode: DynamicSelectionMode::PaperEnforced,
            effective_mode: DynamicSelectionMode::PaperEnforced,
            invalid_configuration: None,
            live_lock_applied: false,
        };
        // context claims Shadow while the resolver says PaperEnforced.
        let mut mismatched = ds_context();
        mismatched.effective_mode = DynamicSelectionMode::Shadow;

        let outcome = evaluate_dynamic_selection_start_gate(
            &ctx,
            &cfg,
            &[],
            &effective,
            mismatched,
            "run-1",
            Utc::now(),
        )
        .await;
        assert_eq!(
            outcome.disposition,
            DynamicSelectionStartGateDisposition::PaperEnforcedRefused
        );
        assert!(!outcome.allowed);
        assert!(
            outcome.plan.is_none(),
            "no plan built on context incoherence"
        );
        assert!(outcome
            .reasons
            .iter()
            .any(|r| matches!(r, DynamicSelectionStartGateReason::ContextIncoherent { .. })));
    }

    /// Defect C: Shadow's own context incoherence must be `ShadowInvalid`
    /// with a bounded reason, never `ShadowAllowed` -- and, unlike the
    /// `PaperEnforced` case above, must never block run start.
    #[tokio::test]
    async fn shadow_incoherent_context_is_shadow_invalid_and_never_blocks() {
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
            effective_mode: DynamicSelectionMode::Shadow,
            invalid_configuration: None,
            live_lock_applied: false,
        };
        // context claims PaperEnforced while the resolver says Shadow.
        let mut mismatched = ds_context();
        mismatched.configured_mode = DynamicSelectionMode::PaperEnforced;
        mismatched.effective_mode = DynamicSelectionMode::PaperEnforced;

        let outcome = evaluate_dynamic_selection_start_gate(
            &ctx,
            &cfg,
            &[],
            &effective,
            mismatched,
            "run-1",
            Utc::now(),
        )
        .await;
        assert_eq!(
            outcome.disposition,
            DynamicSelectionStartGateDisposition::ShadowInvalid
        );
        assert!(outcome.allowed, "shadow never blocks run start");
        assert!(
            outcome.plan.is_none(),
            "no plan built on context incoherence"
        );
        assert!(outcome
            .reasons
            .iter()
            .any(|r| matches!(r, DynamicSelectionStartGateReason::ContextIncoherent { .. })));
    }

    /// IR3: a mismatched `run_id` (context claims a different run than the
    /// one actually being started) must refuse -- a context can never
    /// authenticate itself.
    #[tokio::test]
    async fn incoherent_context_run_id_is_refused() {
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
            ds_context(), // run_id = "run-1"
            "run-2-a-completely-different-run",
            Utc::now(),
        )
        .await;
        assert_eq!(
            outcome.disposition,
            DynamicSelectionStartGateDisposition::PaperEnforcedRefused
        );
        assert!(!outcome.allowed);
        assert!(outcome
            .reasons
            .iter()
            .any(|r| matches!(r, DynamicSelectionStartGateReason::ContextIncoherent { .. })));
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
        let now_utc = Utc::now();
        // Defect D: a genuinely coherent context, built the same way the
        // evaluator itself derives its authoritative expectation -- never a
        // hand-typed literal date that only happens to match "today".
        let context =
            build_dynamic_selection_context("run-1", &effective, &cfg, &calendar, now_utc);

        let outcome = evaluate_dynamic_selection_start_gate(
            &ctx,
            &cfg,
            &[],
            &effective,
            context,
            "run-1",
            now_utc,
        )
        .await;
        assert_eq!(
            outcome.disposition,
            DynamicSelectionStartGateDisposition::PaperEnforcedRefused
        );
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

    // ── Defect D: context source authority ───────────────────────────────

    fn env_fallback_cfg() -> crate::state::MultiSymbolRuntimeConfig {
        crate::state::MultiSymbolRuntimeConfig {
            schema_version: "multi-symbol-runtime-config-v1".to_string(),
            symbols: vec![crate::state::SymbolStrategyAssignment {
                symbol: "AAPL".to_string(),
                strategy_id: "swing_momentum".to_string(),
                timeframe: "1D".to_string(),
            }],
            max_concurrent_symbols: 1,
            source: MultiSymbolConfigSource::EnvSingleSymbolFallback,
        }
    }

    fn watchlist_cfg(path: &str) -> crate::state::MultiSymbolRuntimeConfig {
        crate::state::MultiSymbolRuntimeConfig {
            schema_version: "multi-symbol-runtime-config-v1".to_string(),
            symbols: vec![crate::state::SymbolStrategyAssignment {
                symbol: "AAPL".to_string(),
                strategy_id: "swing_momentum".to_string(),
                timeframe: "1D".to_string(),
            }],
            max_concurrent_symbols: 5,
            source: MultiSymbolConfigSource::WatchlistArtifactV2 {
                path: path.to_string(),
            },
        }
    }

    #[test]
    fn build_dynamic_selection_context_derives_source_kind_and_identity_from_the_real_config() {
        let calendar = crate::state::market_calendar::NyseWeekdaysProvider;
        let now_utc = Utc::now();
        let effective = EffectiveDynamicSelectionMode {
            configured_mode: DynamicSelectionMode::PaperEnforced,
            effective_mode: DynamicSelectionMode::PaperEnforced,
            invalid_configuration: None,
            live_lock_applied: false,
        };

        let env_ctx = build_dynamic_selection_context(
            "run-1",
            &effective,
            &env_fallback_cfg(),
            &calendar,
            now_utc,
        );
        assert_eq!(env_ctx.source_kind, "env_single_symbol_fallback");
        assert_eq!(env_ctx.source_identity, "env");

        let watchlist_cfg = watchlist_cfg("/approved/watchlist-v2.json");
        let watchlist_ctx = build_dynamic_selection_context(
            "run-1",
            &effective,
            &watchlist_cfg,
            &calendar,
            now_utc,
        );
        assert_eq!(watchlist_ctx.source_kind, "watchlist_v2");
        assert_eq!(watchlist_ctx.source_identity, "/approved/watchlist-v2.json");

        assert_eq!(
            env_ctx.schema_version,
            mqk_portfolio::DYNAMIC_SELECTION_SCHEMA_VERSION
        );
        assert_eq!(env_ctx.run_id, "run-1");
        assert_eq!(env_ctx.configured_mode, DynamicSelectionMode::PaperEnforced);
        assert_eq!(env_ctx.effective_mode, DynamicSelectionMode::PaperEnforced);
        assert!(!env_ctx.live_lock_applied);
        assert!(
            !env_ctx.market_date.is_empty(),
            "market_date must be derived, never blank"
        );
    }

    /// One coherent baseline used by every "reject a single wrong field"
    /// test below -- built the same way the evaluator itself derives its
    /// authoritative expectation, so each test can mutate exactly one field
    /// away from truth.
    fn coherent_context_and_deps() -> (
        crate::state::MultiSymbolRuntimeConfig,
        EffectiveDynamicSelectionMode,
        DateTime<Utc>,
        DynamicSelectionContext,
    ) {
        let cfg = env_fallback_cfg();
        let effective = EffectiveDynamicSelectionMode {
            configured_mode: DynamicSelectionMode::PaperEnforced,
            effective_mode: DynamicSelectionMode::PaperEnforced,
            invalid_configuration: None,
            live_lock_applied: false,
        };
        let now_utc = Utc::now();
        let calendar = crate::state::market_calendar::NyseWeekdaysProvider;
        let context =
            build_dynamic_selection_context("run-1", &effective, &cfg, &calendar, now_utc);
        (cfg, effective, now_utc, context)
    }

    async fn run_with_context(
        cfg: &crate::state::MultiSymbolRuntimeConfig,
        effective: &EffectiveDynamicSelectionMode,
        context: DynamicSelectionContext,
        now_utc: DateTime<Utc>,
    ) -> DynamicSelectionStartGateOutcome {
        use crate::state::OperatorAuthMode;
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
        evaluate_dynamic_selection_start_gate(&ctx, cfg, &[], effective, context, "run-1", now_utc)
            .await
    }

    fn assert_context_incoherent_refused(outcome: &DynamicSelectionStartGateOutcome) {
        assert_eq!(
            outcome.disposition,
            DynamicSelectionStartGateDisposition::PaperEnforcedRefused
        );
        assert!(!outcome.allowed);
        assert!(outcome.plan.is_none());
        assert!(outcome
            .reasons
            .iter()
            .any(|r| matches!(r, DynamicSelectionStartGateReason::ContextIncoherent { .. })));
    }

    #[tokio::test]
    async fn wrong_schema_version_is_refused() {
        let (cfg, effective, now_utc, mut context) = coherent_context_and_deps();
        context.schema_version = "some-other-schema-v0".to_string();
        let outcome = run_with_context(&cfg, &effective, context, now_utc).await;
        assert_context_incoherent_refused(&outcome);
    }

    #[tokio::test]
    async fn wrong_live_lock_applied_is_refused() {
        let (cfg, effective, now_utc, mut context) = coherent_context_and_deps();
        context.live_lock_applied = !context.live_lock_applied;
        let outcome = run_with_context(&cfg, &effective, context, now_utc).await;
        assert_context_incoherent_refused(&outcome);
    }

    #[tokio::test]
    async fn wrong_source_kind_is_refused() {
        let (cfg, effective, now_utc, mut context) = coherent_context_and_deps();
        context.source_kind = "watchlist_v2".to_string();
        let outcome = run_with_context(&cfg, &effective, context, now_utc).await;
        assert_context_incoherent_refused(&outcome);
    }

    #[tokio::test]
    async fn wrong_source_identity_is_refused() {
        let (cfg, effective, now_utc, mut context) = coherent_context_and_deps();
        context.source_identity = "/some/other/path.json".to_string();
        let outcome = run_with_context(&cfg, &effective, context, now_utc).await;
        assert_context_incoherent_refused(&outcome);
    }

    #[tokio::test]
    async fn wrong_market_date_is_refused() {
        let (cfg, effective, now_utc, mut context) = coherent_context_and_deps();
        context.market_date = "1999-01-01".to_string();
        let outcome = run_with_context(&cfg, &effective, context, now_utc).await;
        assert_context_incoherent_refused(&outcome);
    }

    /// Defect D: an env-fallback run cannot impersonate a watchlist-v2 run
    /// by claiming its `source_kind` *and* a nonblank `source_identity` --
    /// both fields are checked against the actual config together, not
    /// merely for non-blankness.
    #[tokio::test]
    async fn env_fallback_run_cannot_impersonate_a_watchlist_v2_source() {
        let (cfg, effective, now_utc, mut context) = coherent_context_and_deps();
        assert_eq!(cfg.source, MultiSymbolConfigSource::EnvSingleSymbolFallback);
        context.source_kind = "watchlist_v2".to_string();
        context.source_identity = "/approved/watchlist-v2.json".to_string();
        let outcome = run_with_context(&cfg, &effective, context, now_utc).await;
        assert_context_incoherent_refused(&outcome);
    }

    /// Defect D: the reverse impersonation -- a watchlist-v2 run's actual
    /// config, paired with a context claiming the env-fallback identity.
    #[tokio::test]
    async fn watchlist_v2_run_cannot_impersonate_env_fallback_source() {
        let cfg = watchlist_cfg("/approved/watchlist-v2.json");
        let effective = EffectiveDynamicSelectionMode {
            configured_mode: DynamicSelectionMode::PaperEnforced,
            effective_mode: DynamicSelectionMode::PaperEnforced,
            invalid_configuration: None,
            live_lock_applied: false,
        };
        let now_utc = Utc::now();
        let calendar = crate::state::market_calendar::NyseWeekdaysProvider;
        let mut context =
            build_dynamic_selection_context("run-1", &effective, &cfg, &calendar, now_utc);
        assert_eq!(context.source_kind, "watchlist_v2");
        context.source_kind = "env_single_symbol_fallback".to_string();
        context.source_identity = "env".to_string();

        let outcome = run_with_context(&cfg, &effective, context, now_utc).await;
        assert_context_incoherent_refused(&outcome);
    }
}
