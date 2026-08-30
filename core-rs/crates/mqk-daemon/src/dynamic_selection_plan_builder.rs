//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 4 — candidate universe and
//! immutable plan builder.
//!
//! Builds one [`mqk_portfolio::DynamicSelectionPlan`] from the current
//! [`MultiSymbolRuntimeConfig`] and configured strategy-ID fleet, gathering
//! every fact the pure selector needs and letting the pure selector alone
//! decide selection/ranking/refusal. This module performs no ranking logic
//! of its own — it only assembles evidence.
//!
//! # Caller contract
//! **This function must not be called at all in `off` mode** — off mode's
//! contract (Phase 2) is "no promotion query, artifact read, selector
//! computation, evidence persistence, secondary host construction, or
//! economic change", and this builder always performs a promotion query, an
//! artifact read, and selector computation. The mode gate belongs to the
//! caller (Phase 6's start-gate), not here.
//!
//! # Candidate universe (per symbol)
//! Candidate strategy IDs are the deduplicated union of the configured
//! `MQK_STRATEGY_IDS` fleet and the watchlist/legacy-assigned strategy for
//! that symbol (`MultiSymbolRuntimeConfig`'s own per-symbol assignment) —
//! `watchlist_assigned` is `true` exactly for that one assigned pair.
//!
//! # Independent per-candidate checks
//! For every `(symbol, strategy_id)` pair this module independently
//! computes, before ever consulting the pure selector:
//! - **Plugin instantiation**: `PluginRegistry::instantiate_verified` against
//!   a registry built fresh for that exact symbol
//!   (`mqk_strategy::engines::register_builtin_strategies`) — never the
//!   shared env-symbol-bound registry [`mqk_runtime::native_strategy::build_daemon_plugin_registry`]
//!   builds, since that is scoped to a single `MQK_STRATEGY_SYMBOL`.
//! - **Timeframe match**: the instantiated strategy's own
//!   `spec().timeframe_secs` against this candidate's `timeframe_secs`
//!   (parsed from the assignment's timeframe label via
//!   `mqk_md::Timeframe::parse`).
//! - **Daily data readiness**: [`crate::daily_data_readiness::evaluate_assignment`],
//!   called with a *synthetic* [`EffectiveRuntimeBinding`] constructed to
//!   exactly match this one candidate (`effective_runtime_strategy_id`/
//!   `_target_symbol`/`_timeframe_secs` all set to the candidate's own
//!   identity). This is deliberate, not a workaround: `evaluate_assignment`'s
//!   binding-mismatch checks exist to gate Tier A's single bound strategy;
//!   supplying a binding that trivially matches the candidate under
//!   evaluation neutralizes those checks and lets the *real* bar/provider/
//!   calendar readiness logic run standalone for this exact candidate,
//!   without duplicating or modifying that logic. Every other candidate for
//!   the same symbol gets its own independently-matching synthetic binding —
//!   never the single process-wide Tier A binding, which would make every
//!   candidate except the currently-bound one always evaluate not-ready.
//! - **Promotion/evidence validation**: the shared Phase 3 validator,
//!   [`crate::promotion_evidence_validation::validate_active_paper_candidate`].
//!
//! # Bounds (R7)
//! Refuses over-limit input — more than [`mqk_portfolio::MAX_ELIGIBLE_SYMBOLS`]
//! eligible symbols, or more than [`mqk_portfolio::MAX_CANDIDATE_PAIRS`]
//! candidate pairs — **before** any of the independent per-candidate checks
//! above run: the (symbol, strategy_id, timeframe_secs) identity triples are
//! cheap, zero-I/O, in-memory data, so the bound is checked against them
//! directly before a single DB query, artifact read, or plugin registry is
//! touched.
//!
//! # Freeze contract
//! This function performs one pass, once, and returns. It never rereads
//! config, rescores, or hot-switches mid-call — "build once at run start and
//! freeze for the whole run" (Phase 4) is the caller's responsibility: call
//! this exactly once per run start and hold the resulting
//! [`mqk_portfolio::DynamicSelectionPlan`] immutably for the run's duration.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use mqk_md::instrument_registry::TrackedInstrument;
use mqk_md::provider_registry::ProviderConfig;
use mqk_md::Timeframe;
use mqk_portfolio::{
    canonical_symbol as ds_canonical_symbol, compute_dynamic_selection_plan,
    DynamicSelectionContext, DynamicSelectionPlan, SelectionCandidateEvidence,
    SelectionCandidateInput, MAX_CANDIDATE_PAIRS, MAX_ELIGIBLE_SYMBOLS,
};
use mqk_runtime::native_strategy::EffectiveRuntimeBinding;
use mqk_strategy::PluginRegistry;
use sqlx::PgPool;

use crate::promotion_evidence_validation::{
    validate_active_paper_candidate, CandidateEvidenceReason,
};
use crate::state::market_calendar::MarketCalendarProvider;
use crate::state::{AppState, MultiSymbolRuntimeConfig, SymbolStrategyAssignment};

/// Everything this builder needs beyond `MultiSymbolRuntimeConfig`/the
/// strategy-ID fleet — mirrors exactly the composition
/// `daily_data_readiness::DailyDataReadinessContext` already assembles
/// (`load_readiness_context_from_env`), so a caller building both shares one
/// source of truth for calendar/provider/instrument state.
pub struct DynamicSelectionPlanBuildContext<'a> {
    pub db: Option<&'a PgPool>,
    pub st: &'a Arc<AppState>,
    pub calendar_provider: &'a dyn MarketCalendarProvider,
    pub provider_configs: &'a [ProviderConfig],
    pub instruments: &'a [TrackedInstrument],
}

/// One `(symbol, strategy_id)` pair pending independent-check evaluation —
/// cheap, zero-I/O identity data only.
struct PendingCandidate {
    symbol: String,
    strategy_id: String,
    timeframe_secs: i64,
    timeframe_label: String,
    watchlist_assigned: bool,
}

/// Build one [`DynamicSelectionPlan`]. See module docs for the full
/// contract. `configured_strategy_ids` is the full (not just first-entry)
/// `MQK_STRATEGY_IDS` fleet, already split/trimmed
/// (mirrors `daily_data_readiness::fleet_ids_from_env`, deliberately not
/// re-derived here so the caller can supply either the env-sourced fleet or
/// a test fixture).
pub async fn build_dynamic_selection_plan(
    ctx: &DynamicSelectionPlanBuildContext<'_>,
    multi_symbol_config: &MultiSymbolRuntimeConfig,
    configured_strategy_ids: &[String],
    context: DynamicSelectionContext,
    now_utc: DateTime<Utc>,
) -> DynamicSelectionPlan {
    let eligible_symbols: Vec<String> = multi_symbol_config
        .symbols
        .iter()
        .map(|a| ds_canonical_symbol(&a.symbol))
        .collect();

    // Cheap, zero-I/O (symbol, strategy_id) pair assembly -- R7/Defect F:
    // refuse over-limit input (both the total pairs bound and, independently,
    // any single symbol's own universe-size bound) before any filesystem/DB/
    // host work.
    let mut pending: Vec<PendingCandidate> = Vec::new();
    // Defect F: `true` the moment any one symbol's deduplicated candidate
    // strategy-ID set exceeds `MAX_STRATEGY_UNIVERSE` (which mirrors
    // `mqk_strategy::engines::REGISTERED_STRATEGY_IDS`'s five identities) --
    // checked per symbol, independently of the total-pairs bound below,
    // since a single over-loaded symbol can stay under the total while still
    // locally violating this one.
    let mut per_symbol_over_limit = false;
    for assignment in &multi_symbol_config.symbols {
        let symbol = ds_canonical_symbol(&assignment.symbol);
        let Ok(tf) = Timeframe::parse(&assignment.timeframe) else {
            // Unparseable timeframe -- this symbol contributes no
            // candidates; it still appears in eligible_symbols (visible,
            // not silently omitted) and the pure model reports it
            // no_valid_candidate_for_symbol.
            continue;
        };
        let timeframe_secs = tf.duration_secs();
        let assigned_strategy_id = assignment.strategy_id.trim().to_string();

        // Defect F: trim/deduplicate configured IDs (already the existing
        // contract) -- the union of the configured fleet and the
        // watchlist-assigned strategy, deduplicated exactly once.
        let mut ids: Vec<String> = configured_strategy_ids
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !assigned_strategy_id.is_empty() && !ids.contains(&assigned_strategy_id) {
            ids.push(assigned_strategy_id.clone());
        }
        ids.sort();
        ids.dedup();

        if ids.len() > mqk_portfolio::MAX_STRATEGY_UNIVERSE {
            per_symbol_over_limit = true;
        }

        for strategy_id in ids {
            let watchlist_assigned = strategy_id == assigned_strategy_id;
            pending.push(PendingCandidate {
                symbol: symbol.clone(),
                strategy_id,
                timeframe_secs,
                timeframe_label: assignment.timeframe.clone(),
                watchlist_assigned,
            });
        }
    }

    if eligible_symbols.is_empty()
        || eligible_symbols.len() > MAX_ELIGIBLE_SYMBOLS
        || pending.len() > MAX_CANDIDATE_PAIRS
        || per_symbol_over_limit
    {
        // Bound violated -- the pure model's own bound check produces the
        // correct fail-closed truth state; identity fields are real
        // (computed above with zero I/O), only their evidence is an inert
        // placeholder, since it is discarded unread the moment the pure
        // model's length check runs, before any candidate is inspected.
        let stub_candidates: Vec<SelectionCandidateInput> = pending
            .iter()
            .map(|p| SelectionCandidateInput {
                symbol: p.symbol.clone(),
                strategy_id: p.strategy_id.clone(),
                timeframe_secs: p.timeframe_secs,
                evidence: placeholder_evidence(),
            })
            .collect();
        return compute_dynamic_selection_plan(context, &eligible_symbols, &stub_candidates);
    }

    let mut candidates: Vec<SelectionCandidateInput> = Vec::with_capacity(pending.len());
    for p in &pending {
        let evidence = evaluate_candidate(ctx, p, now_utc).await;
        candidates.push(SelectionCandidateInput {
            symbol: p.symbol.clone(),
            strategy_id: p.strategy_id.clone(),
            timeframe_secs: p.timeframe_secs,
            evidence,
        });
    }

    compute_dynamic_selection_plan(context, &eligible_symbols, &candidates)
}

/// Independently compute every evidence fact for one candidate: plugin
/// instantiation + timeframe match, daily data readiness, and promotion/
/// evidence validation (Phase 3) -- see module docs for why each check is
/// safe to run standalone per-candidate.
async fn evaluate_candidate(
    ctx: &DynamicSelectionPlanBuildContext<'_>,
    p: &PendingCandidate,
    now_utc: DateTime<Utc>,
) -> SelectionCandidateEvidence {
    // Defect F: classify an unregistered strategy_id cheaply, against
    // `mqk_strategy::engines::REGISTERED_STRATEGY_IDS` (the five-identity
    // authority) -- zero I/O, not even ephemeral registry construction --
    // and refuse immediately, before any promotion/artifact/readiness query
    // is ever dispatched. A watchlist-assigned unknown ID still gets a
    // visible `Refused` row here (never silently dropped); it simply never
    // reaches any I/O to produce one.
    if !mqk_strategy::engines::REGISTERED_STRATEGY_IDS.contains(&p.strategy_id.as_str()) {
        let mut evidence = refused_evidence(CandidateEvidenceReason::UnsupportedStrategyPlugin);
        evidence.plugin_instantiable = false;
        evidence.registry_enabled = false;
        evidence.timeframe_matches = false;
        evidence.data_ready = false;
        evidence.watchlist_assigned = p.watchlist_assigned;
        evidence.exact_reason =
            Some(mqk_portfolio::ExactSelectionReason::UnsupportedStrategyPlugin);
        return evidence;
    }

    let mut registry = PluginRegistry::new();
    // IR10: the ephemeral per-symbol registry's construction result must
    // never be discarded -- a construction failure fails this candidate
    // closed with its own distinct reason, rather than silently proceeding
    // against a registry that failed to build (which would make every
    // subsequent `instantiate_verified` call fail with the far less
    // informative `UnknownStrategy`).
    let registry_construction =
        mqk_strategy::engines::register_builtin_strategies(&mut registry, p.symbol.clone())
            .map_err(|_| CandidateEvidenceReason::RegistryConstructionFailed);

    let (plugin_probe_ok, timeframe_matches) = match registry_construction {
        Ok(()) => match registry.instantiate_verified(&p.strategy_id) {
            Ok(strategy) => (true, strategy.spec().timeframe_secs == p.timeframe_secs),
            Err(_) => (false, false),
        },
        Err(_) => (false, false),
    };

    let synthetic_binding = EffectiveRuntimeBinding {
        effective_runtime_strategy_id: Some(p.strategy_id.clone()),
        effective_runtime_target_symbol: Some(p.symbol.clone()),
        effective_runtime_timeframe_secs: Some(p.timeframe_secs),
    };
    let synthetic_assignment = SymbolStrategyAssignment {
        symbol: p.symbol.clone(),
        strategy_id: p.strategy_id.clone(),
        timeframe: p.timeframe_label.clone(),
    };
    let readiness = crate::daily_data_readiness::evaluate_assignment(
        ctx.db,
        &synthetic_assignment,
        &synthetic_binding,
        ctx.calendar_provider,
        ctx.provider_configs,
        ctx.instruments,
        &registry,
        now_utc,
    )
    .await;
    let data_ready = readiness.is_ready();

    // IR4: durable strategy-registry `enabled` truth -- independent of, and
    // checked in addition to, plugin instantiability above. Plugin
    // instantiation proves the engine *can* run; this proves the identity
    // is *durably admitted* to run, using the same authority internal
    // admission already consults (`sys_strategy_registry`).
    let registry_enabled_check: Result<(), CandidateEvidenceReason> = match ctx.db {
        None => Err(CandidateEvidenceReason::RegistryQueryFailed),
        Some(pool) => {
            crate::promotion_evidence_validation::validate_strategy_registry_enabled(
                pool,
                &p.strategy_id,
            )
            .await
        }
    };

    // IR6: captured before `registry_enabled_check` is consumed by the
    // `.and()` chain below, so the exact registry-enabled outcome can be
    // exposed on the evidence snapshot as its own field (IR4), distinct from
    // the combined `plugin_instantiable` gate.
    let registry_enabled = registry_enabled_check.is_ok();

    let plugin_stage: Result<(), CandidateEvidenceReason> = registry_construction
        .and(registry_enabled_check)
        .and(if plugin_probe_ok {
            Ok(())
        } else {
            Err(CandidateEvidenceReason::UnsupportedStrategyPlugin)
        });

    let (mut evidence, promotion_stage_reason) = match ctx.db {
        // No DB pool configured -- equivalent to the promotion query itself
        // being unable to run at all; reuses PromotionQueryFailed (the
        // pure-model-facing effect is identical: promotion_query_ok=false).
        None => (
            refused_evidence(CandidateEvidenceReason::PromotionQueryFailed),
            Some(CandidateEvidenceReason::PromotionQueryFailed),
        ),
        Some(pool) => match validate_active_paper_candidate(
            pool,
            ctx.st,
            &p.strategy_id,
            &p.symbol,
            p.timeframe_secs,
            now_utc,
        )
        .await
        {
            Ok(v) => {
                // DYNAMIC-SELECTION-CONFIG-ELIGIBILITY-CLOSURE-01: reuse the
                // canonical config-identity comparison mechanism (same rule
                // `promotion_gate::evaluate_promotion_tradability_with_
                // config_identity` enforces at the runtime dispatch gate,
                // C1/C2/R2) -- never a duplicated policy. `current_config_
                // fingerprint` is resolved fresh, right now, through the
                // authoritative registry-construction seam and is the frozen
                // "expected identity" this exact plan build establishes; a
                // later host-pool construction must be proven to match THIS
                // value, never a second independent re-derivation (see
                // `dynamic_selection_start_gate`'s host-identity cross-check).
                let current_config_fingerprint =
                    crate::strategy_config_identity::resolve_server_semantic_fingerprint(
                        &p.strategy_id,
                        &p.symbol,
                        p.timeframe_secs,
                    )
                    .ok();
                let config_identity_verified = v.config_identity_status
                    == crate::strategy_config_identity::CONFIG_IDENTITY_STATUS_VERIFIED_V1
                    && v.config_fingerprint
                        .as_deref()
                        .is_some_and(mqk_db::is_valid_evidence_fingerprint_v2_hex)
                    && current_config_fingerprint
                        .as_deref()
                        .is_some_and(mqk_db::is_valid_evidence_fingerprint_v2_hex)
                    && v.config_fingerprint.as_deref() == current_config_fingerprint.as_deref();
                (
                    SelectionCandidateEvidence {
                        promotion_query_ok: true,
                        promotion_state: Some(mqk_db::PROMOTION_STATE_ACTIVE_PAPER.to_string()),
                        promotion_effective: true,
                        promotion_expired: false,
                        evidence_resolved: true,
                        review_state_is_paper_candidate: true,
                        evidence_review_state: Some(v.evidence_review_state),
                        durable_legacy_fingerprint: Some(v.durable_legacy_fingerprint),
                        recomputed_legacy_fingerprint: Some(v.recomputed_legacy_fingerprint),
                        legacy_fingerprint_matches: true,
                        durable_exact_fingerprint_v2: Some(v.durable_exact_fingerprint_v2),
                        recomputed_exact_fingerprint_v2: Some(v.recomputed_exact_fingerprint_v2),
                        exact_fingerprint_v2_matches: true,
                        config_identity_verified,
                        durable_config_fingerprint: v.config_fingerprint,
                        current_config_fingerprint,
                        // Overwritten unconditionally below from `registry_enabled`
                        // (computed independently of this promotion-chain result).
                        registry_enabled: false,
                        plugin_instantiable: true,
                        timeframe_matches: true,
                        data_ready: true,
                        canonical_score_decimal: Some(v.canonical_score_decimal),
                        canonical_score_micros: v.canonical_score_micros,
                        scanner_rank: v.scanner_rank,
                        watchlist_assigned: false,
                        evidence_review_id: Some(v.evidence_review_id),
                        evidence_scanner_scan_id: Some(v.evidence_scanner_scan_id),
                        evidence_artifact_path: Some(v.evidence_artifact_path),
                        evidence_git_hash: Some(v.evidence_git_hash),
                        promotion_transition_id: Some(v.promotion_transition_id.to_string()),
                        promotion_effective_at: Some(v.promotion_effective_at.to_rfc3339()),
                        promotion_expires_at: v.promotion_expires_at.map(|t| t.to_rfc3339()),
                        evidence_transition_id: Some(v.evidence_transition_id.to_string()),
                        exact_reason: None,
                    },
                    None,
                )
            }
            Err(reason) => (refused_evidence(reason), Some(reason)),
        },
    };

    // Defect E: when data_ready is false, capture the *specific* daily-
    // readiness blocker (never just the boolean) as a typed, closed
    // exact_reason -- `readiness.blockers`'s first entry is this
    // candidate's own top-level blocker; an unrecognized/absent blocker
    // string falls back to the honest `DataNotReadyUnspecified` rather than
    // fabricating a specific one never observed.
    let readiness_exact_reason = if data_ready {
        None
    } else {
        Some(
            readiness
                .blockers
                .first()
                .and_then(|code| mqk_portfolio::ReadinessBlockerReason::parse_code(code))
                .map(mqk_portfolio::ExactSelectionReason::DataNotReady)
                .unwrap_or(mqk_portfolio::ExactSelectionReason::DataNotReadyUnspecified),
        )
    };
    let timeframe_exact_reason = if plugin_probe_ok && !timeframe_matches {
        Some(mqk_portfolio::ExactSelectionReason::TimeframeMismatch)
    } else {
        None
    };
    // DYNAMIC-SELECTION-CONFIG-ELIGIBILITY-CLOSURE-01: slots into the
    // precedence chain exactly where the pure gate itself checks it -- right
    // after the promotion/evidence/fingerprint checks, before plugin/
    // timeframe/readiness. `evidence.config_identity_verified` already
    // defaults `true` on every non-`Ok(v)` branch (via `placeholder_evidence`),
    // so this is only ever `Some` when the promotion chain itself succeeded
    // but config identity genuinely did not verify.
    let config_identity_exact_reason = if evidence.config_identity_verified {
        None
    } else {
        Some(mqk_portfolio::ExactSelectionReason::ConfigIdentityUnavailable)
    };

    // IR5/Defect E: exact_reason precedence mirrors the pure gate's own
    // fixed check order (`evaluate_evidence_gate` checks promotion/evidence
    // fields, then config identity, then plugin_instantiable, then
    // timeframe_matches, then data_ready) -- a promotion-chain failure is
    // always the true first-observed cause when one occurred; config
    // identity, then registry/plugin, then timeframe, then readiness are
    // only surfaced as exact_reason when every earlier-in-order stage passed.
    let exact_reason = promotion_stage_reason
        .map(CandidateEvidenceReason::to_exact_selection_reason)
        .or(config_identity_exact_reason)
        .or_else(|| {
            plugin_stage
                .as_ref()
                .err()
                .map(|r| r.to_exact_selection_reason())
        })
        .or(timeframe_exact_reason)
        .or(readiness_exact_reason);

    evidence.plugin_instantiable = plugin_stage.is_ok();
    evidence.registry_enabled = registry_enabled;
    evidence.timeframe_matches = timeframe_matches;
    evidence.data_ready = data_ready;
    evidence.watchlist_assigned = p.watchlist_assigned;
    evidence.exact_reason = exact_reason;
    evidence
}

/// Defect 1: valid-shaped (64 lowercase hex) legacy fingerprint placeholder
/// -- see [`placeholder_evidence`]'s doc comment on why this must be
/// genuinely well-formed, not just a human-readable label.
const PLACEHOLDER_LEGACY_FINGERPRINT: &str =
    "86842b99db711429e7d503e0e3a5f6f2d31d9e8b91f19310a32220fc8fdd8b0e";
/// Defect 1: valid-shaped (64 lowercase hex) v2 fingerprint placeholder,
/// distinct from [`PLACEHOLDER_LEGACY_FINGERPRINT`] so the two never
/// accidentally collide in an equality assertion.
const PLACEHOLDER_V2_FINGERPRINT: &str =
    "8d82312144cef2506a296d2620bad311a2095990045cb4518aa4bac054a2de95";
/// DYNAMIC-SELECTION-CONFIG-ELIGIBILITY-CLOSURE-01: valid-shaped (64
/// lowercase hex) config-identity fingerprint placeholder, distinct from
/// both fingerprints above for the same reason.
const PLACEHOLDER_CONFIG_FINGERPRINT: &str =
    "c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f0c15f";

/// A "good" evidence baseline used only as a starting point for
/// [`refused_evidence`] and the over-limit short-circuit's inert
/// placeholder -- every field the failing reason doesn't explicitly flip is
/// irrelevant, since the pure model's gate (or the bound check) refuses
/// before reaching it.
fn placeholder_evidence() -> SelectionCandidateEvidence {
    SelectionCandidateEvidence {
        promotion_query_ok: true,
        promotion_state: Some(mqk_db::PROMOTION_STATE_ACTIVE_PAPER.to_string()),
        promotion_effective: true,
        promotion_expired: false,
        evidence_resolved: true,
        review_state_is_paper_candidate: true,
        evidence_review_state: None,
        // Defect 1: the pure gate now derives fingerprint-match truth from
        // these values themselves (format + byte equality), never just from
        // the `*_matches` booleans below -- these placeholders must
        // therefore be genuinely valid-shaped (64 lowercase hex) and
        // byte-equal on each side, or every failing reason ordered *after*
        // the fingerprint checks in `evaluate_evidence_gate`'s fixed order
        // (unsupported plugin, timeframe mismatch, data not ready, missing
        // score) would incorrectly surface as a fingerprint failure instead.
        durable_legacy_fingerprint: Some(PLACEHOLDER_LEGACY_FINGERPRINT.to_string()),
        recomputed_legacy_fingerprint: Some(PLACEHOLDER_LEGACY_FINGERPRINT.to_string()),
        legacy_fingerprint_matches: true,
        durable_exact_fingerprint_v2: Some(PLACEHOLDER_V2_FINGERPRINT.to_string()),
        recomputed_exact_fingerprint_v2: Some(PLACEHOLDER_V2_FINGERPRINT.to_string()),
        exact_fingerprint_v2_matches: true,
        config_identity_verified: true,
        durable_config_fingerprint: Some(PLACEHOLDER_CONFIG_FINGERPRINT.to_string()),
        current_config_fingerprint: Some(PLACEHOLDER_CONFIG_FINGERPRINT.to_string()),
        registry_enabled: true,
        plugin_instantiable: true,
        timeframe_matches: true,
        data_ready: true,
        canonical_score_decimal: Some("0".to_string()),
        canonical_score_micros: Some(0),
        scanner_rank: None,
        watchlist_assigned: false,
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

/// Map one [`CandidateEvidenceReason`] to the single [`SelectionCandidateEvidence`]
/// boolean/value the pure model's `evaluate_evidence_gate` checks at the
/// matching step in its own fixed gate order -- every earlier-in-order field
/// stays at its "good" default, which is safe because the pure gate returns
/// at the *first* failing field it encounters, in that same fixed order.
fn refused_evidence(reason: CandidateEvidenceReason) -> SelectionCandidateEvidence {
    use CandidateEvidenceReason::*;
    let mut e = placeholder_evidence();
    match reason {
        PromotionQueryFailed => e.promotion_query_ok = false,
        NoPromotionRecord | PromotionNotActivePaper => e.promotion_state = None,
        PromotionNotYetEffective => e.promotion_effective = false,
        PromotionExpired => e.promotion_expired = true,
        EvidenceLineageQueryFailed
        | EvidenceLineageBroken
        | ArtifactPathMissing
        | DurableFingerprintMissing
        | ArtifactRootUnavailable
        | ArtifactMissing
        | ArtifactRootEscape
        | ArtifactMalformed
        | ArtifactDuplicateIdentity
        | ArtifactNoMatchingRow
        | ArtifactRowIdentityMismatch
        | ArtifactOverLimit
        | RankOutOfRange => e.evidence_resolved = false,
        ArtifactNotPaperCandidate => e.review_state_is_paper_candidate = false,
        FingerprintMismatch => e.legacy_fingerprint_matches = false,
        // Defect 1: v2-missing and v2-mismatch are distinct causes at the
        // pure-model gate too -- flip the exact field that produces the
        // matching gate-stage refusal, never the coarser legacy flag.
        DurableFingerprintV2Missing => {
            e.durable_exact_fingerprint_v2 = None;
            e.recomputed_exact_fingerprint_v2 = None;
        }
        FingerprintV2Mismatch => e.exact_fingerprint_v2_matches = false,
        ScoreMissing | ScoreNotFinite => {
            e.canonical_score_decimal = None;
            e.canonical_score_micros = None;
        }
        RegistryQueryFailed | RegistryRowMissing | RegistryDisabled => {
            e.registry_enabled = false;
            e.plugin_instantiable = false;
        }
        UnsupportedStrategyPlugin | RegistryConstructionFailed => e.plugin_instantiable = false,
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::market_calendar::NyseWeekdaysProvider;
    use crate::state::{MultiSymbolConfigSource, OperatorAuthMode};

    /// Deterministic UUIDv5, namespaced by an explicit per-call-site seed
    /// string -- never `Uuid::new_v4()`. Fixture uniqueness comes from the
    /// seed text (always naming the test and the field it identifies), not
    /// from randomness.
    fn det_uuid(seed: &str) -> uuid::Uuid {
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, seed.as_bytes())
    }

    fn config(symbols: Vec<SymbolStrategyAssignment>) -> MultiSymbolRuntimeConfig {
        MultiSymbolRuntimeConfig {
            schema_version: "multi-symbol-runtime-config-v1".to_string(),
            symbols,
            max_concurrent_symbols: 5,
            source: MultiSymbolConfigSource::EnvSingleSymbolFallback,
        }
    }

    fn ctx_no_db<'a>(
        st: &'a Arc<AppState>,
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

    fn ds_context() -> DynamicSelectionContext {
        DynamicSelectionContext {
            run_id: "run-1".to_string(),
            schema_version: mqk_portfolio::DYNAMIC_SELECTION_SCHEMA_VERSION.to_string(),
            configured_mode: mqk_portfolio::DynamicSelectionMode::PaperEnforced,
            effective_mode: mqk_portfolio::DynamicSelectionMode::PaperEnforced,
            live_lock_applied: false,
            source_kind: "env_single_symbol_fallback".to_string(),
            source_identity: "env".to_string(),
            market_date: "2026-07-28".to_string(),
        }
    }

    #[tokio::test]
    async fn empty_symbols_fails_closed_with_zero_io() {
        let st = Arc::new(AppState::new_with_operator_auth(
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let calendar = NyseWeekdaysProvider;
        let ctx = ctx_no_db(&st, &calendar);
        let cfg = config(vec![]);

        let plan = build_dynamic_selection_plan(&ctx, &cfg, &[], ds_context(), Utc::now()).await;
        assert_eq!(
            plan.truth_state,
            mqk_portfolio::TRUTH_STATE_NO_ELIGIBLE_SYMBOLS
        );
    }

    #[tokio::test]
    async fn over_eligible_symbol_limit_fails_closed_with_zero_io() {
        let st = Arc::new(AppState::new_with_operator_auth(
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let calendar = NyseWeekdaysProvider;
        let ctx = ctx_no_db(&st, &calendar);
        let symbols = vec!["A", "B", "C", "D", "E", "F"]
            .into_iter()
            .map(|s| SymbolStrategyAssignment {
                symbol: s.to_string(),
                strategy_id: "swing_momentum".to_string(),
                timeframe: "1D".to_string(),
            })
            .collect();
        let cfg = config(symbols);

        let plan = build_dynamic_selection_plan(
            &ctx,
            &cfg,
            &["swing_momentum".to_string()],
            ds_context(),
            Utc::now(),
        )
        .await;
        assert_eq!(
            plan.truth_state,
            mqk_portfolio::TRUTH_STATE_ELIGIBLE_SYMBOLS_OVER_LIMIT
        );
        assert!(
            plan.symbol_results.is_empty(),
            "over-limit fails the whole plan closed with no per-symbol results"
        );
    }

    #[tokio::test]
    async fn over_candidate_pair_limit_fails_closed_with_zero_io() {
        let st = Arc::new(AppState::new_with_operator_auth(
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let calendar = NyseWeekdaysProvider;
        let ctx = ctx_no_db(&st, &calendar);
        // 5 symbols x 6 strategy ids (over MAX_STRATEGY_UNIVERSE=5) = 30 > 25.
        let symbols = vec!["A", "B", "C", "D", "E"]
            .into_iter()
            .map(|s| SymbolStrategyAssignment {
                symbol: s.to_string(),
                strategy_id: "assigned_only".to_string(),
                timeframe: "1D".to_string(),
            })
            .collect();
        let cfg = config(symbols);
        let fleet: Vec<String> = (0..6).map(|i| format!("strategy_{i}")).collect();

        let plan = build_dynamic_selection_plan(&ctx, &cfg, &fleet, ds_context(), Utc::now()).await;
        assert_eq!(
            plan.truth_state,
            mqk_portfolio::TRUTH_STATE_CANDIDATES_OVER_LIMIT
        );
    }

    #[tokio::test]
    async fn unparseable_timeframe_symbol_yields_no_candidates_not_a_panic() {
        let st = Arc::new(AppState::new_with_operator_auth(
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let calendar = NyseWeekdaysProvider;
        let ctx = ctx_no_db(&st, &calendar);
        let cfg = config(vec![SymbolStrategyAssignment {
            symbol: "AAPL".to_string(),
            strategy_id: "swing_momentum".to_string(),
            timeframe: "not-a-real-timeframe".to_string(),
        }]);

        let plan = build_dynamic_selection_plan(
            &ctx,
            &cfg,
            &["swing_momentum".to_string()],
            ds_context(),
            Utc::now(),
        )
        .await;
        assert_eq!(
            plan.truth_state,
            mqk_portfolio::DYNAMIC_SELECTION_TRUTH_STATE_COMPUTED
        );
        assert_eq!(plan.symbol_results.len(), 1, "AAPL still visible");
        let aapl = &plan.symbol_results[0];
        assert_eq!(aapl.symbol, "AAPL");
        assert!(
            aapl.candidates.is_empty(),
            "unparseable timeframe contributes zero candidates, not a fabricated one"
        );
        assert_eq!(
            aapl.reason_code,
            mqk_portfolio::DYNAMIC_SELECTION_REASON_NO_VALID_CANDIDATE
        );
    }

    #[tokio::test]
    async fn no_db_pool_refuses_every_candidate_promotion_query_failed() {
        let st = Arc::new(AppState::new_with_operator_auth(
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let calendar = NyseWeekdaysProvider;
        let ctx = ctx_no_db(&st, &calendar);
        let cfg = config(vec![SymbolStrategyAssignment {
            symbol: "AAPL".to_string(),
            strategy_id: "swing_momentum".to_string(),
            timeframe: "1D".to_string(),
        }]);

        let plan = build_dynamic_selection_plan(
            &ctx,
            &cfg,
            &["swing_momentum".to_string()],
            ds_context(),
            Utc::now(),
        )
        .await;
        let aapl = &plan.symbol_results[0];
        assert_eq!(aapl.candidates.len(), 1);
        assert_eq!(
            aapl.candidates[0].reason_code,
            mqk_portfolio::REASON_REFUSED_PROMOTION_QUERY_FAILED
        );
    }

    /// Defect F: an unregistered strategy_id is now classified cheaply
    /// against `mqk_strategy::engines::REGISTERED_STRATEGY_IDS` *before* any
    /// promotion/artifact/readiness query -- refused as
    /// `unsupported_strategy_plugin` even with no DB pool at all, which
    /// structurally proves the promotion query was never reached for this
    /// candidate (the old behavior, with no DB, always produced
    /// `promotion_query_failed` instead, since that query ran unconditionally).
    #[tokio::test]
    async fn unregistered_strategy_id_is_refused_unsupported_strategy() {
        let st = Arc::new(AppState::new_with_operator_auth(
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let calendar = NyseWeekdaysProvider;
        let ctx = ctx_no_db(&st, &calendar);
        let cfg = config(vec![SymbolStrategyAssignment {
            symbol: "AAPL".to_string(),
            strategy_id: "totally_unknown_strategy".to_string(),
            timeframe: "1D".to_string(),
        }]);

        let plan = build_dynamic_selection_plan(&ctx, &cfg, &[], ds_context(), Utc::now()).await;
        let aapl = &plan.symbol_results[0];
        assert_eq!(aapl.candidates.len(), 1);
        assert_eq!(
            aapl.candidates[0].reason_code,
            mqk_portfolio::REASON_REFUSED_UNSUPPORTED_STRATEGY
        );
        assert_eq!(
            aapl.candidates[0].exact_reason,
            mqk_portfolio::ExactSelectionReason::UnsupportedStrategyPlugin
        );
    }

    #[tokio::test]
    async fn watchlist_assigned_flag_is_set_only_for_the_assigned_pair() {
        let st = Arc::new(AppState::new_with_operator_auth(
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let calendar = NyseWeekdaysProvider;
        let ctx = ctx_no_db(&st, &calendar);
        let cfg = config(vec![SymbolStrategyAssignment {
            symbol: "AAPL".to_string(),
            strategy_id: "swing_momentum".to_string(),
            timeframe: "1D".to_string(),
        }]);
        let fleet = vec!["swing_momentum".to_string(), "mean_reversion".to_string()];

        let plan = build_dynamic_selection_plan(&ctx, &cfg, &fleet, ds_context(), Utc::now()).await;
        let aapl = &plan.symbol_results[0];
        assert_eq!(aapl.candidates.len(), 2, "union of fleet + assigned = 2");
        let swing = aapl
            .candidates
            .iter()
            .find(|c| c.strategy_id == "swing_momentum")
            .unwrap();
        let mean_rev = aapl
            .candidates
            .iter()
            .find(|c| c.strategy_id == "mean_reversion")
            .unwrap();
        assert!(
            swing.watchlist_assigned,
            "swing_momentum is the assigned pair"
        );
        assert!(!mean_rev.watchlist_assigned, "mean_reversion is fleet-only");
    }

    // ── DB-backed: full evidence chain proven, refused only on data readiness ──

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test cargo test -p mqk-daemon --features testkit --lib dynamic_selection_plan_builder -- --include-ignored"]
    async fn full_evidence_chain_passes_refused_only_on_data_readiness() {
        // `strategy_id` is fixed to a real builtin plugin name (plugin_instantiable
        // only recognizes 5 fixed names) and `symbol` is a deterministic UUIDv5
        // derivation -- both by design, documented below. Against the shared
        // MQK_DATABASE_URL database that determinism means any second
        // invocation of this exact test (this session or a prior one) finds the
        // identity already promoted to active_paper from the first run and
        // fails the very first shadow_approved transition as "illegal_transition"
        // (real failure observed under the default parallel test runner, where
        // this test's own prior invocation earlier in the same process becomes
        // the collision). A disposable per-test database removes that residue.
        mqk_db::run_isolated("dyn_sel_plan_builder_full_chain", |pool| async move {
        use axum::http::StatusCode;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let root = std::env::temp_dir().join(format!(
            "mqk_daemon_dyn_sel_plan_builder_{}",
            det_uuid("dynamic_selection_plan_builder::full_evidence_chain_passes_refused_only_on_data_readiness::root")
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", &root);

        // A real built-in strategy name is required for plugin_instantiable
        // to pass (the plugin registry only recognizes the 5 fixed builtin
        // names) -- the symbol, which register_builtin_strategies binds
        // fresh for any symbol string, real or fixture-only, is derived
        // deterministically below (never Uuid::new_v4()) so this identity
        // is reproducible across runs.
        let strategy_id = "swing_momentum".to_string();
        let symbol = format!(
            "ZZ{}",
            det_uuid("dynamic_selection_plan_builder::full_evidence_chain_passes_refused_only_on_data_readiness::symbol")
                .to_string()
                .replace('-', "")[..8]
                .to_ascii_uppercase()
        );
        let decision = mqk_backtest::StrategyScanReviewDecision {
            symbol: symbol.clone(),
            timeframe: "1D".to_string(),
            strategy_id: strategy_id.clone(),
            scanner_rank: Some(1),
            scanner_score: Some(9.0),
            review_state: mqk_backtest::StrategyScanReviewState::PaperCandidate,
            reason_codes: vec!["eligible_paper_candidate".to_string()],
            blockers: Vec::new(),
            warnings: Vec::new(),
        };
        let review_id = det_uuid("dynamic_selection_plan_builder::full_evidence_chain_passes_refused_only_on_data_readiness::review_id");
        let manifest = mqk_backtest::ReviewManifest {
            schema_version: 1,
            review_id: review_id.to_string(),
            scanner_scan_id: "scan-fixture".to_string(),
            source_artifact_dir: "fixture-source-not-on-disk".to_string(),
            created_at_utc: "2026-07-01T00:00:00Z".to_string(),
            git_hash: "test-git-hash".to_string(),
            policy_min_bars_used: 252,
            policy_min_trade_count: 5,
            policy_min_total_return_pct: 0.0,
            policy_min_alpha_pct: 0.0,
            policy_max_drawdown_pct: 25.0,
            policy_min_profit_factor: 1.05,
            candidate_count: 1,
            blocked_count: 0,
            needs_review_count: 0,
            watchlist_candidate_count: 0,
            paper_candidate_count: 1,
            rejected_count: 0,
            blockers: Vec::new(),
            warnings: Vec::new(),
        };
        let summary = mqk_backtest::ReviewSummary {
            scanner_scan_id: "scan-fixture".to_string(),
            review_id: review_id.to_string(),
            candidate_count: 1,
            blocked_count: 0,
            needs_review_count: 0,
            watchlist_candidate_count: 0,
            paper_candidate_count: 1,
            rejected_count: 0,
            top_paper_candidates: Vec::new(),
            top_watchlist_candidates: Vec::new(),
            blockers: Vec::new(),
            warnings: Vec::new(),
        };
        let review_dir = mqk_backtest::write_review_artifacts(
            &root,
            &mqk_backtest::ReviewRunOutput {
                review_id,
                manifest,
                decisions: vec![decision],
                summary,
            },
        )
        .expect("write fixture review artifacts");

        let st = Arc::new(AppState::new_with_db_and_operator_auth(
            pool.clone(),
            OperatorAuthMode::ExplicitDevNoToken,
        ));

        // IR4: the durable strategy registry is now an independent
        // admission gate alongside plugin instantiability -- this fixture
        // must durably register+enable the strategy_id under test so the
        // candidate can reach the readiness gate (the one gate this test
        // specifically proves), same as internal admission would require.
        mqk_db::upsert_strategy_registry_entry(
            &pool,
            &mqk_db::UpsertStrategyRegistryArgs {
                strategy_id: strategy_id.clone(),
                display_name: "Swing Momentum (fixture)".to_string(),
                enabled: true,
                kind: "bar_driven".to_string(),
                registered_at_utc: Utc::now(),
                updated_at_utc: Utc::now(),
                note: String::new(),
            },
        )
        .await
        .expect("upsert strategy registry entry");

        async fn transition(
            st: Arc<AppState>,
            body: serde_json::Value,
        ) -> (axum::http::StatusCode, bytes::Bytes) {
            let req = axum::http::Request::builder()
                .method("POST")
                .uri("/api/v1/strategy/promotions/transition")
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap();
            let resp = crate::routes::build_router(st).oneshot(req).await.unwrap();
            let status = resp.status();
            let body = resp.into_body().collect().await.unwrap().to_bytes();
            (status, body)
        }

        let (status, resp_body) = transition(
            Arc::clone(&st),
            serde_json::json!({
                "strategy_id": strategy_id,
                "symbol": symbol,
                "timeframe_secs": 86400,
                "target_state": "shadow_approved",
                "review_dir": review_dir.to_str().unwrap(),
                "effective_at_utc": Utc::now().to_rfc3339(),
                "expires_at_utc": null,
                "initiated_by": "test-operator",
                "reason": "scenario test",
            }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "shadow_approved failed: {:?}",
            String::from_utf8_lossy(&resp_body)
        );

        let (status, _) = transition(
            Arc::clone(&st),
            serde_json::json!({
                "strategy_id": strategy_id,
                "symbol": symbol,
                "timeframe_secs": 86400,
                "target_state": "paper_approved",
                "review_dir": null,
                "effective_at_utc": Utc::now().to_rfc3339(),
                "expires_at_utc": null,
                "initiated_by": "test-operator",
                "reason": "scenario test",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "paper_approved failed");

        let (status, _) = transition(
            Arc::clone(&st),
            serde_json::json!({
                "strategy_id": strategy_id,
                "symbol": symbol,
                "timeframe_secs": 86400,
                "target_state": "active_paper",
                "review_dir": null,
                "effective_at_utc": Utc::now().to_rfc3339(),
                "expires_at_utc": null,
                "initiated_by": "test-operator",
                "reason": "scenario test",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "active_paper failed");

        let calendar = NyseWeekdaysProvider;
        let readiness_ctx = crate::daily_data_readiness::load_readiness_context_from_env();
        let ctx = DynamicSelectionPlanBuildContext {
            db: Some(&pool),
            st: &st,
            calendar_provider: &calendar,
            provider_configs: &readiness_ctx.provider_configs,
            instruments: &readiness_ctx.instruments,
        };
        let cfg = config(vec![SymbolStrategyAssignment {
            symbol: symbol.clone(),
            strategy_id: strategy_id.clone(),
            timeframe: "1D".to_string(),
        }]);

        let plan = build_dynamic_selection_plan(&ctx, &cfg, &[], ds_context(), Utc::now()).await;
        let result = plan
            .symbol_results
            .iter()
            .find(|s| s.symbol == symbol)
            .expect("symbol result present");
        assert_eq!(result.candidates.len(), 1);
        let candidate = &result.candidates[0];
        // Every earlier-in-order gate (promotion query/active_paper/
        // effective/expired/evidence-resolved/paper_candidate/fingerprint/
        // plugin_instantiable/timeframe_matches) necessarily passed for the
        // pure model's fixed gate order to reach data_ready at all -- this
        // one reason code proves the entire Phase 3 + plugin/timeframe
        // integration chain, not just the readiness gate in isolation.
        assert_eq!(
            candidate.reason_code,
            mqk_portfolio::REASON_REFUSED_DATA_NOT_READY,
            "expected refusal at the data-readiness gate specifically (no md_bars \
             ingested for this fixture), proving every earlier gate passed"
        );
      })
      .await;
    }

    // ── Defect F: per-symbol strategy bound before I/O ───────────────────

    /// Defect F: `mqk_portfolio::MAX_STRATEGY_UNIVERSE` (the bound this
    /// builder enforces per symbol, before any I/O) must stay structurally
    /// tied to `mqk_strategy::engines::REGISTERED_STRATEGY_IDS`'s actual
    /// count -- a registration added to one without updating the other
    /// silently drifts the bound out from under the real universe.
    #[test]
    fn max_strategy_universe_matches_registered_strategy_ids_count() {
        assert_eq!(
            mqk_portfolio::MAX_STRATEGY_UNIVERSE,
            mqk_strategy::engines::REGISTERED_STRATEGY_IDS.len(),
            "MAX_STRATEGY_UNIVERSE has drifted from REGISTERED_STRATEGY_IDS's actual count"
        );
    }

    /// Defect F: six distinct strategy IDs configured for one symbol must
    /// stop the whole plan closed *before* any candidate I/O -- proven with
    /// `ctx_no_db` (a `None` DB pool would make every candidate's promotion
    /// query fail anyway, so the *only* way this test's assertion can pass
    /// is if the over-limit short-circuit fires before `evaluate_candidate`
    /// ever runs, since a computed plan would otherwise still be reachable
    /// via the zero-DB-refusal path).
    #[tokio::test]
    async fn six_strategy_ids_for_one_symbol_stops_before_io() {
        let st = Arc::new(AppState::new_with_operator_auth(
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let calendar = NyseWeekdaysProvider;
        let ctx = ctx_no_db(&st, &calendar);
        let cfg = config(vec![SymbolStrategyAssignment {
            symbol: "AAPL".to_string(),
            strategy_id: "assigned_only".to_string(),
            timeframe: "1D".to_string(),
        }]);
        let fleet: Vec<String> = (0..6).map(|i| format!("strategy_{i}")).collect();

        let plan = build_dynamic_selection_plan(&ctx, &cfg, &fleet, ds_context(), Utc::now()).await;
        assert_eq!(
            plan.truth_state,
            mqk_portfolio::TRUTH_STATE_SYMBOL_CANDIDATES_OVER_LIMIT
        );
        assert!(
            plan.symbol_results.is_empty(),
            "over-limit fails the whole plan closed with no per-symbol results"
        );
    }

    /// Defect F: exactly five known strategy IDs for one symbol must still
    /// evaluate normally (the bound is `<=` MAX_STRATEGY_UNIVERSE, not `<`).
    #[tokio::test]
    async fn five_known_strategy_ids_for_one_symbol_evaluate_normally() {
        let st = Arc::new(AppState::new_with_operator_auth(
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let calendar = NyseWeekdaysProvider;
        let ctx = ctx_no_db(&st, &calendar);
        let cfg = config(vec![SymbolStrategyAssignment {
            symbol: "AAPL".to_string(),
            strategy_id: "swing_momentum".to_string(),
            timeframe: "1D".to_string(),
        }]);
        let fleet: Vec<String> = mqk_strategy::engines::REGISTERED_STRATEGY_IDS
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(fleet.len(), 5);

        let plan = build_dynamic_selection_plan(&ctx, &cfg, &fleet, ds_context(), Utc::now()).await;
        assert_eq!(
            plan.truth_state,
            mqk_portfolio::DYNAMIC_SELECTION_TRUTH_STATE_COMPUTED
        );
        let aapl = &plan.symbol_results[0];
        assert_eq!(aapl.candidates.len(), 5, "all five known IDs evaluate");
        // With no DB, every known ID reaches the promotion-query stage (the
        // stage right after the cheap registration check) -- proving each
        // one was individually evaluated, not short-circuited as unknown.
        for c in &aapl.candidates {
            assert_eq!(
                c.reason_code,
                mqk_portfolio::REASON_REFUSED_PROMOTION_QUERY_FAILED
            );
        }
    }

    /// Defect F: an unknown strategy_id performs zero candidate I/O -- with
    /// `ctx_no_db`, a *known* ID's refusal reason is `promotion_query_failed`
    /// (it reached the DB stage and found no pool); an *unknown* ID's
    /// refusal reason must instead be `unsupported_strategy_plugin`,
    /// structurally proving it never reached that DB stage at all.
    #[tokio::test]
    async fn unknown_strategy_id_performs_zero_candidate_io() {
        let st = Arc::new(AppState::new_with_operator_auth(
            OperatorAuthMode::ExplicitDevNoToken,
        ));
        let calendar = NyseWeekdaysProvider;
        let ctx = ctx_no_db(&st, &calendar);
        let cfg = config(vec![SymbolStrategyAssignment {
            symbol: "AAPL".to_string(),
            strategy_id: "swing_momentum".to_string(),
            timeframe: "1D".to_string(),
        }]);
        let fleet = vec![
            "swing_momentum".to_string(),
            "totally_unknown_strategy".to_string(),
        ];

        let plan = build_dynamic_selection_plan(&ctx, &cfg, &fleet, ds_context(), Utc::now()).await;
        let aapl = &plan.symbol_results[0];
        assert_eq!(aapl.candidates.len(), 2);
        let known = aapl
            .candidates
            .iter()
            .find(|c| c.strategy_id == "swing_momentum")
            .unwrap();
        let unknown = aapl
            .candidates
            .iter()
            .find(|c| c.strategy_id == "totally_unknown_strategy")
            .unwrap();
        assert_eq!(
            known.reason_code,
            mqk_portfolio::REASON_REFUSED_PROMOTION_QUERY_FAILED,
            "the known ID reached the DB stage"
        );
        assert_eq!(
            unknown.reason_code,
            mqk_portfolio::REASON_REFUSED_UNSUPPORTED_STRATEGY,
            "the unknown ID never reached the DB stage -- watchlist/fleet visibility \
             preserved (it still appears as a refused row), just with zero I/O"
        );
    }
}
