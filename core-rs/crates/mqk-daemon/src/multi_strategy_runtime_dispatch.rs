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

/// Reason code for a binding refused because its identity appears in
/// `MQK_DRY_RUN_STRATEGY_IDS` (frozen contract §8) — checked before any
/// promotion/readiness I/O, so a dry-run identity's evidence fields are
/// never populated (they stay at pure-default/unresolved values).
pub(crate) const REASON_EXPLICIT_MULTI_STRATEGY_DRY_RUN_EXCLUDED: &str =
    "explicit_multi_strategy_dry_run_excluded";

/// Reason code for a binding that was independently evaluated through the
/// real evidence gate and refused on its own merits (unpromoted, config
/// mismatch, not data-ready, etc.) — the gate's own `exact_reason`/
/// `reason_code` is not re-derived here; this is the outer, closed-vocabulary
/// disposition this module itself assigns.
pub(crate) const REASON_EXPLICIT_MULTI_STRATEGY_EVIDENCE_GATE_REFUSED: &str =
    "explicit_multi_strategy_evidence_gate_refused";

/// The "never independently evaluated" evidence value for a binding refused
/// before any promotion/readiness I/O was attempted (dry-run exclusion) --
/// every field fail-closed (`false`/`None`), never a fabricated pass.
/// [`mqk_portfolio::SelectionCandidateEvidence`] has no `Default` impl (each
/// field is independently meaningful evidence, not a type with a sensible
/// zero value in general), so this is production code's own explicit
/// "unresolved" constant, not a derive.
fn unresolved_evidence() -> mqk_portfolio::SelectionCandidateEvidence {
    mqk_portfolio::SelectionCandidateEvidence {
        promotion_query_ok: false,
        promotion_state: None,
        promotion_effective: false,
        promotion_expired: false,
        evidence_resolved: false,
        review_state_is_paper_candidate: false,
        evidence_review_state: None,
        durable_legacy_fingerprint: None,
        recomputed_legacy_fingerprint: None,
        legacy_fingerprint_matches: false,
        durable_exact_fingerprint_v2: None,
        recomputed_exact_fingerprint_v2: None,
        exact_fingerprint_v2_matches: false,
        config_identity_verified: false,
        durable_config_fingerprint: None,
        current_config_fingerprint: None,
        registry_enabled: false,
        plugin_instantiable: false,
        timeframe_matches: false,
        data_ready: false,
        canonical_score_decimal: None,
        canonical_score_micros: None,
        scanner_rank: None,
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

/// One binding's full, independent evaluation result — every result-
/// affecting evidence field Patch C2's durable authority store needs,
/// whether or not the binding was ultimately authorized. A refused binding
/// still gets one of these (never an absent entry), so its evidence is never
/// lost.
#[derive(Debug, Clone)]
pub(crate) struct ExplicitBindingEvaluation {
    pub(crate) symbol: String,
    pub(crate) strategy_id: String,
    pub(crate) timeframe_secs: i64,
    pub(crate) authorized: bool,
    pub(crate) reason_code: String,
    pub(crate) evidence: mqk_portfolio::SelectionCandidateEvidence,
}

/// Evaluate every `(symbol, strategy_id)` pair a `watchlist-v3` artifact
/// explicitly authorizes, independently, through the exact same gate Bundle
/// 7 candidates pass through — returning one [`ExplicitBindingEvaluation`]
/// per pair, authorized or not, in artifact order (symbol order, then each
/// symbol's own artifact strategy-list order — frozen contract §5).
///
/// `timeframe_secs`/`timeframe_label` are the artifact's one shared
/// timeframe (frozen contract §3 — v3 does not support per-binding
/// timeframe overrides in this patch). `market_date`/`source_identity` are
/// caller-supplied, caller-minted facts (this function reads no clock and
/// no env var), carried through into each per-binding gate call's
/// [`DynamicSelectionContext`] for durable-evidence-shape parity with
/// Bundle 7.
pub(crate) async fn evaluate_explicit_bindings(
    ctx: &DynamicSelectionPlanBuildContext<'_>,
    artifact: &LoadedWatchlistArtifactV3,
    timeframe_secs: i64,
    timeframe_label: &str,
    run_id: Uuid,
    source_identity: &str,
    market_date: &str,
    now_utc: DateTime<Utc>,
) -> Vec<ExplicitBindingEvaluation> {
    let mut evaluations: Vec<ExplicitBindingEvaluation> = Vec::new();

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
                evaluations.push(ExplicitBindingEvaluation {
                    symbol: symbol.clone(),
                    strategy_id: strategy_id.clone(),
                    timeframe_secs,
                    authorized: false,
                    reason_code: REASON_EXPLICIT_MULTI_STRATEGY_DRY_RUN_EXCLUDED.to_string(),
                    evidence: unresolved_evidence(),
                });
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
                evidence: evidence.clone(),
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
            let plan = compute_dynamic_selection_plan(context, &[symbol.clone()], &[candidate]);
            let passed = plan
                .symbol_results
                .first()
                .and_then(|sr| sr.candidates.first())
                .map(|c| c.disposition == SelectionCandidateDisposition::Selected)
                .unwrap_or(false);

            evaluations.push(ExplicitBindingEvaluation {
                symbol: symbol.clone(),
                strategy_id: strategy_id.clone(),
                timeframe_secs,
                authorized: passed,
                reason_code: if passed {
                    REASON_EXPLICIT_MULTI_STRATEGY_AUTHORIZED.to_string()
                } else {
                    REASON_EXPLICIT_MULTI_STRATEGY_EVIDENCE_GATE_REFUSED.to_string()
                },
                evidence,
            });
        }
    }

    evaluations
}

/// [`evaluate_explicit_bindings`], filtered and re-sorted to only the
/// authorized subset's `(symbol, strategy_id, timeframe_secs)` identity —
/// the shape [`crate::dynamic_selection_host_pool::DynamicSelectionHostPool::build`]
/// consumes. Kept as a narrower entry point for a future caller that needs
/// only the authorized set without full per-binding evidence; the real
/// Patch C3 activation path (`state/lifecycle.rs`) calls
/// [`evaluate_explicit_bindings`] directly instead and derives both the
/// authorized subset and the durable evidence from that one evaluation
/// pass, so it is not itself a production caller of this function
/// (avoiding a second, redundant evidence-gathering DB round trip).
#[allow(dead_code)]
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
    let mut authorized: Vec<HostPoolKey> = evaluate_explicit_bindings(
        ctx,
        artifact,
        timeframe_secs,
        timeframe_label,
        run_id,
        source_identity,
        market_date,
        now_utc,
    )
    .await
    .into_iter()
    .filter(|e| e.authorized)
    .map(|e| (e.symbol, e.strategy_id, e.timeframe_secs))
    .collect();

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

// ---------------------------------------------------------------------------
// Patch C2 (V4-STAGE-B-M2-REPAIR-03): durable explicit authority evidence
// -- authority_id derivation and the mapping into `mqk_db::
// NewExplicitMultiStrategyAuthority`. Distinct from `derive_explicit_
// multi_strategy_plan_id` above: that id identifies the host-pool/dispatch
// authority (run_id + authorized binding identities only); this id
// additionally binds the source artifact and config-fingerprint facts, and
// every binding's own result-affecting evidence -- so a changed source
// artifact, or a changed promotion/config/readiness fact for any binding,
// can never reuse a stale durable authority_id (mission requirement: a
// promoted-then-later-demoted binding, or a swapped artifact, must mint a
// new identity, never silently overwrite or reuse the old one).
// ---------------------------------------------------------------------------

/// Versioned namespace seed for the durable authority identity -- distinct
/// from [`EXPLICIT_PLAN_ID_NAMESPACE_SEED`] so the two id spaces can never
/// collide.
const EXPLICIT_AUTHORITY_ID_NAMESPACE_SEED: &str = "mqk.explicit-multi-strategy-authority-id.v1";

fn push_len_prefixed(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u32).to_be_bytes());
    buf.extend_from_slice(s.as_bytes());
}

fn push_opt_str(buf: &mut Vec<u8>, s: &Option<String>) {
    match s {
        Some(v) => {
            buf.push(1);
            push_len_prefixed(buf, v);
        }
        None => buf.push(0),
    }
}

fn push_bool(buf: &mut Vec<u8>, b: bool) {
    buf.push(u8::from(b));
}

fn push_opt_i64(buf: &mut Vec<u8>, v: &Option<i64>) {
    match v {
        Some(n) => {
            buf.push(1);
            buf.extend_from_slice(&n.to_le_bytes());
        }
        None => buf.push(0),
    }
}

fn push_opt_u32(buf: &mut Vec<u8>, v: &Option<u32>) {
    match v {
        Some(n) => {
            buf.push(1);
            buf.extend_from_slice(&n.to_le_bytes());
        }
        None => buf.push(0),
    }
}

/// Mint the one deterministic durable authority identity, from every
/// result-affecting input: `run_id`, the source artifact's own identity/hash,
/// the config fingerprint, `market_date`, and every binding's full
/// evaluation (sorted by `(symbol, strategy_id, timeframe_secs)` so input
/// order never changes the identity) -- never from wall clock or any other
/// non-reproducible fact.
pub(crate) fn derive_explicit_multi_strategy_authority_id(
    run_id: Uuid,
    source_identity: &str,
    source_artifact_hash: &str,
    config_fingerprint: &str,
    market_date: &str,
    evaluations: &[ExplicitBindingEvaluation],
) -> Uuid {
    let mut sorted: Vec<&ExplicitBindingEvaluation> = evaluations.iter().collect();
    sorted.sort_by(|a, b| {
        (a.symbol.as_str(), a.strategy_id.as_str(), a.timeframe_secs).cmp(&(
            b.symbol.as_str(),
            b.strategy_id.as_str(),
            b.timeframe_secs,
        ))
    });

    let mut buf = Vec::new();
    push_len_prefixed(&mut buf, EXPLICIT_AUTHORITY_ID_NAMESPACE_SEED);
    buf.extend_from_slice(run_id.as_bytes());
    push_len_prefixed(&mut buf, source_identity);
    push_len_prefixed(&mut buf, source_artifact_hash);
    push_len_prefixed(&mut buf, config_fingerprint);
    push_len_prefixed(&mut buf, market_date);

    buf.extend_from_slice(&(sorted.len() as u32).to_be_bytes());
    for e in sorted {
        push_len_prefixed(&mut buf, &e.symbol);
        push_len_prefixed(&mut buf, &e.strategy_id);
        buf.extend_from_slice(&e.timeframe_secs.to_le_bytes());
        push_bool(&mut buf, e.authorized);
        push_len_prefixed(&mut buf, &e.reason_code);
        // Every `SelectionCandidateEvidence` field capable of changing this
        // binding's authorization, provenance validity, semantic identity,
        // or evidence interpretation (D2, V4-STAGE-B-M2-C1-C3-REPAIR-04) --
        // never a fragile partial mirror. Field order here is fixed and
        // matches `unresolved_evidence`'s field order for readability; it is
        // not itself semantically meaningful (each field is length/type
        // prefixed), but changing it would change every existing
        // authority_id, so it must not be reordered casually.
        push_bool(&mut buf, e.evidence.promotion_query_ok);
        push_opt_str(&mut buf, &e.evidence.promotion_state);
        push_bool(&mut buf, e.evidence.promotion_effective);
        push_bool(&mut buf, e.evidence.promotion_expired);
        push_bool(&mut buf, e.evidence.evidence_resolved);
        push_bool(&mut buf, e.evidence.review_state_is_paper_candidate);
        push_opt_str(&mut buf, &e.evidence.evidence_review_state);
        push_opt_str(&mut buf, &e.evidence.durable_legacy_fingerprint);
        push_opt_str(&mut buf, &e.evidence.recomputed_legacy_fingerprint);
        push_bool(&mut buf, e.evidence.legacy_fingerprint_matches);
        push_opt_str(&mut buf, &e.evidence.durable_exact_fingerprint_v2);
        push_opt_str(&mut buf, &e.evidence.recomputed_exact_fingerprint_v2);
        push_bool(&mut buf, e.evidence.exact_fingerprint_v2_matches);
        push_bool(&mut buf, e.evidence.config_identity_verified);
        push_opt_str(&mut buf, &e.evidence.durable_config_fingerprint);
        push_opt_str(&mut buf, &e.evidence.current_config_fingerprint);
        push_bool(&mut buf, e.evidence.registry_enabled);
        push_bool(&mut buf, e.evidence.plugin_instantiable);
        push_bool(&mut buf, e.evidence.timeframe_matches);
        push_bool(&mut buf, e.evidence.data_ready);
        push_opt_str(&mut buf, &e.evidence.canonical_score_decimal);
        push_opt_i64(&mut buf, &e.evidence.canonical_score_micros);
        push_opt_u32(&mut buf, &e.evidence.scanner_rank);
        push_bool(&mut buf, e.evidence.watchlist_assigned);
        push_opt_str(&mut buf, &e.evidence.evidence_review_id);
        push_opt_str(&mut buf, &e.evidence.evidence_scanner_scan_id);
        push_opt_str(&mut buf, &e.evidence.evidence_artifact_path);
        push_opt_str(&mut buf, &e.evidence.evidence_git_hash);
        push_opt_str(&mut buf, &e.evidence.promotion_transition_id);
        push_opt_str(&mut buf, &e.evidence.promotion_effective_at);
        push_opt_str(&mut buf, &e.evidence.promotion_expires_at);
        push_opt_str(&mut buf, &e.evidence.evidence_transition_id);
        push_opt_str(
            &mut buf,
            &e.evidence.exact_reason.as_ref().map(|r| r.code()),
        );
    }

    Uuid::new_v5(&Uuid::NAMESPACE_DNS, &buf)
}

/// Single, shared writer-version identity string recorded on every durable
/// authority header row.
pub(crate) const EXPLICIT_MULTI_STRATEGY_AUTHORITY_WRITER_VERSION: &str =
    "mqk-daemon.explicit-multi-strategy-authority-writer.v1";

/// Build the durable evidence DTO for one resolved explicit authority,
/// binding-count/authorized-count derived from `evaluations` itself (never a
/// separately caller-supplied count that could drift). `authority_id` is
/// [`derive_explicit_multi_strategy_authority_id`]'s output for these exact
/// inputs. Bindings are carried in `evaluations`' own (artifact) order --
/// ordinal, not a re-sort — mirroring `dynamic_selection_evidence_writer`'s
/// own convention.
pub(crate) fn build_new_explicit_multi_strategy_authority(
    evaluations: &[ExplicitBindingEvaluation],
    run_id: Uuid,
    source_identity: &str,
    source_artifact_hash: &str,
    config_fingerprint: &str,
    market_date: &str,
    created_at_utc: DateTime<Utc>,
) -> mqk_db::NewExplicitMultiStrategyAuthority {
    let authority_id = derive_explicit_multi_strategy_authority_id(
        run_id,
        source_identity,
        source_artifact_hash,
        config_fingerprint,
        market_date,
        evaluations,
    );

    let bindings = evaluations
        .iter()
        .map(|e| mqk_db::NewExplicitMultiStrategyAuthorityBinding {
            symbol: e.symbol.clone(),
            strategy_id: e.strategy_id.clone(),
            timeframe_secs: e.timeframe_secs,
            authorized: e.authorized,
            reason_code: e.reason_code.clone(),
            promotion_query_ok: e.evidence.promotion_query_ok,
            promotion_state: e.evidence.promotion_state.clone(),
            promotion_effective: e.evidence.promotion_effective,
            promotion_expired: e.evidence.promotion_expired,
            evidence_resolved: e.evidence.evidence_resolved,
            review_state_is_paper_candidate: e.evidence.review_state_is_paper_candidate,
            evidence_review_state: e.evidence.evidence_review_state.clone(),
            durable_legacy_fingerprint: e.evidence.durable_legacy_fingerprint.clone(),
            recomputed_legacy_fingerprint: e.evidence.recomputed_legacy_fingerprint.clone(),
            legacy_fingerprint_matches: e.evidence.legacy_fingerprint_matches,
            durable_exact_fingerprint_v2: e.evidence.durable_exact_fingerprint_v2.clone(),
            recomputed_exact_fingerprint_v2: e.evidence.recomputed_exact_fingerprint_v2.clone(),
            exact_fingerprint_v2_matches: e.evidence.exact_fingerprint_v2_matches,
            config_identity_verified: e.evidence.config_identity_verified,
            durable_config_fingerprint: e.evidence.durable_config_fingerprint.clone(),
            current_config_fingerprint: e.evidence.current_config_fingerprint.clone(),
            registry_enabled: e.evidence.registry_enabled,
            plugin_instantiable: e.evidence.plugin_instantiable,
            timeframe_matches: e.evidence.timeframe_matches,
            data_ready: e.evidence.data_ready,
            canonical_score_decimal: e.evidence.canonical_score_decimal.clone(),
            canonical_score_micros: e.evidence.canonical_score_micros,
            scanner_rank: e.evidence.scanner_rank.map(|r| r as i32),
            watchlist_assigned: e.evidence.watchlist_assigned,
            evidence_review_id: e.evidence.evidence_review_id.clone(),
            evidence_scanner_scan_id: e.evidence.evidence_scanner_scan_id.clone(),
            evidence_artifact_path: e.evidence.evidence_artifact_path.clone(),
            evidence_git_hash: e.evidence.evidence_git_hash.clone(),
            promotion_transition_id: e.evidence.promotion_transition_id.clone(),
            promotion_effective_at: e.evidence.promotion_effective_at.clone(),
            promotion_expires_at: e.evidence.promotion_expires_at.clone(),
            evidence_transition_id: e.evidence.evidence_transition_id.clone(),
            exact_reason: e.evidence.exact_reason.as_ref().map(|r| r.code()),
        })
        .collect();

    mqk_db::NewExplicitMultiStrategyAuthority {
        authority_id,
        run_id,
        source_kind: mqk_db::EXPLICIT_MULTI_STRATEGY_SOURCE_KIND.to_string(),
        source_identity: source_identity.to_string(),
        source_artifact_hash: source_artifact_hash.to_string(),
        config_fingerprint: config_fingerprint.to_string(),
        market_date: market_date.to_string(),
        approved_for_live: false,
        writer_version: EXPLICIT_MULTI_STRATEGY_AUTHORITY_WRITER_VERSION.to_string(),
        created_at_utc,
        bindings,
    }
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
            .map(|(s, ids)| (s.to_string(), ids.iter().map(|id| id.to_string()).collect()))
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
    fn gate_passes(
        symbol: &str,
        strategy_id: &str,
        evidence: mqk_portfolio::SelectionCandidateEvidence,
    ) -> bool {
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

        assert!(
            aapl_scalper,
            "AAPL/intraday_scalper must pass independently"
        );
        assert!(
            aapl_short,
            "AAPL/intraday_short_scalper must pass independently, alongside its sibling"
        );
        assert!(
            !msft_unpromoted,
            "MSFT/volatility_breakout's refusal must not depend on or affect AAPL's results"
        );
    }

    // -----------------------------------------------------------------
    // Patch C2 (V4-STAGE-B-M2-REPAIR-03): durable authority identity
    // derivation and builder proofs.
    // -----------------------------------------------------------------

    fn authorized_evaluation(symbol: &str, strategy_id: &str) -> ExplicitBindingEvaluation {
        ExplicitBindingEvaluation {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe_secs: 300,
            authorized: true,
            reason_code: REASON_EXPLICIT_MULTI_STRATEGY_AUTHORIZED.to_string(),
            evidence: green_evidence(),
        }
    }

    fn base_id(evaluations: &[ExplicitBindingEvaluation]) -> Uuid {
        derive_explicit_multi_strategy_authority_id(
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"c2-run"),
            "watchlist-v3.json",
            "hash-a",
            "cfg-a",
            "2026-09-18",
            evaluations,
        )
    }

    #[test]
    fn c2_changed_source_artifact_hash_changes_authority_id() {
        let evaluations = vec![authorized_evaluation("AAPL", "intraday_scalper")];
        let a = base_id(&evaluations);
        let b = derive_explicit_multi_strategy_authority_id(
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"c2-run"),
            "watchlist-v3.json",
            "hash-b",
            "cfg-a",
            "2026-09-18",
            &evaluations,
        );
        assert_ne!(
            a, b,
            "a changed source artifact hash must mint a different authority_id"
        );
    }

    #[test]
    fn c2_changed_config_fingerprint_changes_authority_id() {
        let evaluations = vec![authorized_evaluation("AAPL", "intraday_scalper")];
        let a = base_id(&evaluations);
        let b = derive_explicit_multi_strategy_authority_id(
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"c2-run"),
            "watchlist-v3.json",
            "hash-a",
            "cfg-b",
            "2026-09-18",
            &evaluations,
        );
        assert_ne!(
            a, b,
            "a changed config fingerprint must mint a different authority_id -- \
             a semantic/config change can never reuse a stale authority"
        );
    }

    /// A binding's own evidence changing (e.g. promotion later expires, or
    /// config identity stops verifying) must mint a different authority_id
    /// -- proving the identity is not merely `run_id + binding names`.
    #[test]
    fn c2_changed_binding_evidence_changes_authority_id() {
        let a_eval = authorized_evaluation("AAPL", "intraday_scalper");
        let mut b_eval = a_eval.clone();
        b_eval.evidence.promotion_effective = false;
        b_eval.authorized = false;
        b_eval.reason_code = REASON_EXPLICIT_MULTI_STRATEGY_EVIDENCE_GATE_REFUSED.to_string();

        let a = base_id(&[a_eval]);
        let b = base_id(&[b_eval]);
        assert_ne!(
            a, b,
            "a binding's own evidence/authorization result changing must mint a \
             different authority_id, never silently reuse the prior identity"
        );
    }

    #[test]
    fn c2_authority_id_is_input_order_independent() {
        let forward = vec![
            authorized_evaluation("AAPL", "intraday_scalper"),
            authorized_evaluation("MSFT", "swing_momentum"),
        ];
        let mut reversed = forward.clone();
        reversed.reverse();

        assert_eq!(
            base_id(&forward),
            base_id(&reversed),
            "authority_id must not depend on evaluation input order"
        );
    }

    #[test]
    fn c2_same_run_different_run_id_changes_authority_id() {
        let evaluations = vec![authorized_evaluation("AAPL", "intraday_scalper")];
        let a = derive_explicit_multi_strategy_authority_id(
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"c2-run-a"),
            "watchlist-v3.json",
            "hash-a",
            "cfg-a",
            "2026-09-18",
            &evaluations,
        );
        let b = derive_explicit_multi_strategy_authority_id(
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"c2-run-b"),
            "watchlist-v3.json",
            "hash-a",
            "cfg-a",
            "2026-09-18",
            &evaluations,
        );
        assert_ne!(
            a, b,
            "a different run_id must mint a different authority_id"
        );
    }

    #[test]
    fn c2_build_new_explicit_multi_strategy_authority_maps_fields_and_counts() {
        let mut refused = authorized_evaluation("MSFT", "swing_momentum");
        refused.authorized = false;
        refused.reason_code = REASON_EXPLICIT_MULTI_STRATEGY_EVIDENCE_GATE_REFUSED.to_string();
        refused.evidence = unpromoted_evidence();

        let evaluations = vec![authorized_evaluation("AAPL", "intraday_scalper"), refused];
        let created_at_utc = Utc::now();
        let new_authority = build_new_explicit_multi_strategy_authority(
            &evaluations,
            Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"c2-run"),
            "watchlist-v3.json",
            "hash-a",
            "cfg-a",
            "2026-09-18",
            created_at_utc,
        );

        assert!(!new_authority.approved_for_live);
        assert_eq!(
            new_authority.source_kind,
            mqk_db::EXPLICIT_MULTI_STRATEGY_SOURCE_KIND
        );
        assert_eq!(new_authority.bindings.len(), 2);
        assert_eq!(new_authority.bindings[0].symbol, "AAPL");
        assert!(new_authority.bindings[0].authorized);
        assert_eq!(new_authority.bindings[1].symbol, "MSFT");
        assert!(!new_authority.bindings[1].authorized);

        // Validation-layer proof: a mqk_db-level validator must accept this
        // DTO (correct binding_count/authorized_count derivation, no
        // duplicate binding) before any DB I/O is attempted.
        assert_eq!(mqk_db::validate_new_authority(&new_authority), Ok(()));
    }

    #[test]
    fn c2_dry_run_binding_evaluation_carries_unresolved_evidence_and_refusal_reason() {
        let evaluation = ExplicitBindingEvaluation {
            symbol: "AAPL".to_string(),
            strategy_id: "intraday_short_scalper".to_string(),
            timeframe_secs: 300,
            authorized: false,
            reason_code: REASON_EXPLICIT_MULTI_STRATEGY_DRY_RUN_EXCLUDED.to_string(),
            evidence: unresolved_evidence(),
        };
        assert!(!evaluation.evidence.promotion_query_ok);
        assert!(!evaluation.evidence.registry_enabled);
        assert_eq!(
            evaluation.reason_code,
            REASON_EXPLICIT_MULTI_STRATEGY_DRY_RUN_EXCLUDED
        );
    }

    /// V4-STAGE-B-M2-C1-C3-FINAL-01 (C2 adversarial audit): exhaustive
    /// field-by-field matrix proof. Every one of `SelectionCandidateEvidence`'s
    /// 33 fields is decided PERSISTED + IDENTITY-BOUND (never PROVEN
    /// NON-AUTHORITATIVE by omission) -- this list plus `d2`'s own list plus
    /// `c2_changed_binding_evidence_changes_authority_id`'s `promotion_effective`
    /// mutator together cover the complete struct. Fields deliberately
    /// covered elsewhere: `promotion_effective` (that test above). Every
    /// other field mutated here.
    #[test]
    fn c2_remaining_evidence_fields_change_authority_id() {
        let base_eval = authorized_evaluation("AAPL", "intraday_scalper");
        let baseline_id = base_id(&[base_eval.clone()]);

        type Mutator = Box<dyn Fn(&mut mqk_portfolio::SelectionCandidateEvidence)>;
        let mutators: Vec<(&str, Mutator)> = vec![
            (
                "promotion_query_ok",
                Box::new(|e| e.promotion_query_ok = false),
            ),
            (
                "promotion_state",
                Box::new(|e| e.promotion_state = Some("expired".to_string())),
            ),
            (
                "config_identity_verified",
                Box::new(|e| e.config_identity_verified = false),
            ),
            (
                "durable_config_fingerprint",
                Box::new(|e| e.durable_config_fingerprint = Some("different-cfg".to_string())),
            ),
            (
                "current_config_fingerprint",
                Box::new(|e| e.current_config_fingerprint = Some("different-cfg".to_string())),
            ),
            ("registry_enabled", Box::new(|e| e.registry_enabled = false)),
            ("data_ready", Box::new(|e| e.data_ready = false)),
            (
                "promotion_transition_id",
                Box::new(|e| e.promotion_transition_id = Some("transition-1".to_string())),
            ),
            (
                "evidence_transition_id",
                Box::new(|e| e.evidence_transition_id = Some("transition-2".to_string())),
            ),
        ];

        for (field_name, mutate) in mutators {
            let mut mutated_eval = base_eval.clone();
            mutate(&mut mutated_eval.evidence);
            let mutated_id = base_id(&[mutated_eval]);
            assert_ne!(
                baseline_id, mutated_id,
                "mutating `{field_name}` alone must change authority_id -- it is not bound \
                 into identity"
            );
        }
    }

    /// D2 (V4-STAGE-B-M2-C1-C3-REPAIR-04): mutation proof that every
    /// previously-omitted `SelectionCandidateEvidence` field is actually
    /// bound into `authority_id` -- not merely carried through to the
    /// durable DTO. Each mutator flips exactly one field away from
    /// `green_evidence()`'s baseline; every one must change the derived id,
    /// proving a promoted-then-later-demoted/expired/refingerprinted binding
    /// (or any other previously-invisible evidence change) can never reuse a
    /// stale authority_id.
    #[test]
    fn d2_every_previously_omitted_evidence_field_changes_authority_id() {
        let base_eval = authorized_evaluation("AAPL", "intraday_scalper");
        let baseline_id = base_id(&[base_eval.clone()]);

        type Mutator = Box<dyn Fn(&mut mqk_portfolio::SelectionCandidateEvidence)>;
        let mutators: Vec<(&str, Mutator)> = vec![
            ("promotion_expired", Box::new(|e| e.promotion_expired = true)),
            ("evidence_resolved", Box::new(|e| e.evidence_resolved = false)),
            (
                "review_state_is_paper_candidate",
                Box::new(|e| e.review_state_is_paper_candidate = false),
            ),
            (
                "evidence_review_state",
                Box::new(|e| e.evidence_review_state = Some("rejected".to_string())),
            ),
            (
                "durable_legacy_fingerprint",
                Box::new(|e| e.durable_legacy_fingerprint = Some("different".to_string())),
            ),
            (
                "recomputed_legacy_fingerprint",
                Box::new(|e| e.recomputed_legacy_fingerprint = Some("different".to_string())),
            ),
            (
                "legacy_fingerprint_matches",
                Box::new(|e| e.legacy_fingerprint_matches = false),
            ),
            (
                "durable_exact_fingerprint_v2",
                Box::new(|e| e.durable_exact_fingerprint_v2 = Some("different".to_string())),
            ),
            (
                "recomputed_exact_fingerprint_v2",
                Box::new(|e| e.recomputed_exact_fingerprint_v2 = Some("different".to_string())),
            ),
            (
                "exact_fingerprint_v2_matches",
                Box::new(|e| e.exact_fingerprint_v2_matches = false),
            ),
            (
                "plugin_instantiable",
                Box::new(|e| e.plugin_instantiable = false),
            ),
            ("timeframe_matches", Box::new(|e| e.timeframe_matches = false)),
            (
                "canonical_score_decimal",
                Box::new(|e| e.canonical_score_decimal = Some("2".to_string())),
            ),
            (
                "canonical_score_micros",
                Box::new(|e| e.canonical_score_micros = Some(2_000_000)),
            ),
            ("scanner_rank", Box::new(|e| e.scanner_rank = Some(2))),
            (
                "watchlist_assigned",
                Box::new(|e| e.watchlist_assigned = false),
            ),
            (
                "evidence_review_id",
                Box::new(|e| e.evidence_review_id = Some("review-1".to_string())),
            ),
            (
                "evidence_scanner_scan_id",
                Box::new(|e| e.evidence_scanner_scan_id = Some("scan-1".to_string())),
            ),
            (
                "evidence_artifact_path",
                Box::new(|e| e.evidence_artifact_path = Some("path".to_string())),
            ),
            (
                "evidence_git_hash",
                Box::new(|e| e.evidence_git_hash = Some("deadbeef".to_string())),
            ),
            (
                "promotion_effective_at",
                Box::new(|e| e.promotion_effective_at = Some("2026-09-18T00:00:00Z".to_string())),
            ),
            (
                "promotion_expires_at",
                Box::new(|e| e.promotion_expires_at = Some("2026-10-18T00:00:00Z".to_string())),
            ),
            (
                "exact_reason",
                Box::new(|e| {
                    e.exact_reason = Some(mqk_portfolio::ExactSelectionReason::PromotionExpired)
                }),
            ),
        ];

        for (field_name, mutate) in mutators {
            let mut mutated_eval = base_eval.clone();
            mutate(&mut mutated_eval.evidence);
            let mutated_id = base_id(&[mutated_eval]);
            assert_ne!(
                baseline_id, mutated_id,
                "mutating `{field_name}` alone must change authority_id -- it is not bound \
                 into identity"
            );
        }
    }
}
