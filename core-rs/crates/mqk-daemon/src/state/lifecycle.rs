//! Operator-facing execution lifecycle: start / stop / halt / arm / shutdown.
//!
//! This module contains the `AppState` impl block for the five primary
//! operator-visible lifecycle transitions.  All private helpers (db_pool,
//! reap_finished_execution_loop, take_execution_loop_for_control, etc.) remain
//! in `state.rs`; they are accessible here because Rust allows child modules to
//! read items that are private to a parent module.

use std::sync::Arc;

use chrono::Utc;

use crate::artifact_intake::{
    evaluate_artifact_deployability, evaluate_artifact_intake_guarded, ArtifactIntakeOutcome,
    ENV_ARTIFACT_PATH,
};
use crate::capital_policy::{
    evaluate_capital_policy_from_env, evaluate_deployment_economics_from_env, CapitalPolicyOutcome,
    DeploymentEconomicsOutcome,
};
use crate::market_data_freshness::{
    evaluate_md_freshness_status_for_symbols, is_awaiting_first_session_bar, timeframe_secs,
};
use crate::parity_evidence::{evaluate_parity_evidence_from_env, ParityEvidenceOutcome};

use sqlx::PgPool;

use super::loop_runner::spawn_execution_loop;
use super::types::DaemonOrchestrator;
use super::{
    reconcile_broker_snapshot_from_schema, reconcile_local_snapshot_from_runtime_with_sides,
    spawn_reconcile_tick, uptime_secs,
};
use super::{
    AcceptedArtifactProvenance, BrokerKind, DeploymentMode, DynamicSelectionLifecycleFaultSeam,
    DynamicSelectionRuntimeState, OperatorAuthMode, RuntimeLifecycleError, StatusSnapshot,
    StrategyMarketDataSource,
};
use super::{AppState, DAEMON_ENGINE_ID, RECONCILE_TICK_INTERVAL};

use mqk_runtime::native_strategy::{bootstrap_with_effective_binding, NativeStrategyBootstrap};

// ---------------------------------------------------------------------------
// OPENING-BAR-FRESHNESS-AUTHORITY-REPAIR-01
// ---------------------------------------------------------------------------

/// True when every blocking symbol in `readiness` is blocked *only* by a
/// structurally-guaranteed pending first bar: `freshness_state == "stale"`
/// (never `"missing"`/`"insufficient"` — those mean no historical data
/// exists at all, a genuine problem unrelated to the session just opening)
/// and, for that exact symbol/timeframe, `now_utc` falls within
/// [`is_awaiting_first_session_bar`]'s window.
///
/// Fail-closed by construction: `false` when `readiness.per_symbol` carries
/// no blocker at all (the call site only reaches this when
/// `!readiness.start_allowed`, but this stays defensive rather than trusting
/// that invariant), when any blocker's timeframe cannot be parsed, or when
/// any blocker is anything other than exactly this one temporal condition —
/// a single non-covered blocker keeps the whole verdict on the original
/// `market_data_not_fresh` fail-closed path.
fn readiness_blocked_only_by_pending_first_session_bar(
    readiness: &crate::api_types::MultiSymbolFreshnessReport,
    calendar_provider: &dyn crate::state::market_calendar::MarketCalendarProvider,
    now_utc: chrono::DateTime<Utc>,
) -> bool {
    let blockers: Vec<_> = readiness
        .per_symbol
        .iter()
        .filter(|s| s.is_start_blocker())
        .collect();
    if blockers.is_empty() {
        return false;
    }
    let configured_grace = crate::daily_data_readiness::configured_grace_seconds_from_env();
    let schedule =
        crate::state::market_calendar::resolve_market_session_schedule(calendar_provider, now_utc);
    blockers.iter().all(|status| {
        if status.freshness_state != "stale" {
            return false;
        }
        let Some(tf_secs) = timeframe_secs(&status.timeframe) else {
            return false;
        };
        let grace = crate::daily_data_readiness::effective_grace_seconds(configured_grace, tf_secs);
        is_awaiting_first_session_bar(&schedule, tf_secs, grace, now_utc.timestamp())
    })
}

#[cfg(test)]
mod opening_bar_freshness_authority_tests {
    use super::*;
    use crate::api_types::MultiSymbolFreshnessReport;
    use crate::market_data_freshness::evaluate_md_freshness_snapshot;
    use crate::state::market_calendar::{resolve_market_session_schedule, NyseWeekdaysProvider};

    // Monday 2024-04-15, well inside NyseWeekdaysProvider's covered range.
    const REF_INSTANT: i64 = 1_713_188_100; // 2024-04-15 13:35 UTC

    fn session_open_ts() -> i64 {
        let provider = NyseWeekdaysProvider;
        let now = chrono::DateTime::<Utc>::from_timestamp(REF_INSTANT, 0).unwrap();
        resolve_market_session_schedule(&provider, now)
            .session_open_utc
            .timestamp()
    }

    fn report_from(statuses: Vec<crate::api_types::MarketDataFreshnessStatus>) -> MultiSymbolFreshnessReport {
        crate::market_data_freshness::aggregate_freshness_statuses(
            statuses.iter().map(|s| s.symbol.clone()).collect(),
            statuses,
        )
    }

    #[test]
    fn obf_lc_01_stale_45s_after_open_is_covered() {
        let open_ts = session_open_ts();
        let now = open_ts + 45;
        // Only a prior-session bar exists — structurally guaranteed this
        // early, so `evaluate_md_freshness_snapshot` reports "stale".
        let status = evaluate_md_freshness_snapshot("ZZLC01", "5m", 10, Some(open_ts - 20_000), now);
        assert_eq!(status.freshness_state, "stale");
        let report = report_from(vec![status]);
        let provider = NyseWeekdaysProvider;
        let now_utc = chrono::DateTime::<Utc>::from_timestamp(now, 0).unwrap();
        assert!(
            readiness_blocked_only_by_pending_first_session_bar(&report, &provider, now_utc),
            "TEST A: a lone stale blocker 45s after open must be covered by the carve-out"
        );
    }

    #[test]
    fn obf_lc_02_stale_well_inside_session_is_not_covered() {
        let open_ts = session_open_ts();
        let now = open_ts + 3600; // 1 hour into the session
        let status = evaluate_md_freshness_snapshot("ZZLC02", "5m", 10, Some(open_ts - 20_000), now);
        assert_eq!(status.freshness_state, "stale");
        let report = report_from(vec![status]);
        let provider = NyseWeekdaysProvider;
        let now_utc = chrono::DateTime::<Utc>::from_timestamp(now, 0).unwrap();
        assert!(
            !readiness_blocked_only_by_pending_first_session_bar(&report, &provider, now_utc),
            "TEST D: genuine staleness well inside the session must never be \
             waved through — no weakening"
        );
    }

    #[test]
    fn obf_lc_03_missing_is_never_covered_even_45s_after_open() {
        let open_ts = session_open_ts();
        let now = open_ts + 45;
        let status = evaluate_md_freshness_snapshot("ZZLC03", "5m", 0, None, now);
        assert_eq!(status.freshness_state, "missing");
        let report = report_from(vec![status]);
        let provider = NyseWeekdaysProvider;
        let now_utc = chrono::DateTime::<Utc>::from_timestamp(now, 0).unwrap();
        assert!(
            !readiness_blocked_only_by_pending_first_session_bar(&report, &provider, now_utc),
            "TEST E: no historical data at all is a genuine problem, never a \
             pending-first-bar condition, regardless of session timing"
        );
    }

    #[test]
    fn obf_lc_04_insufficient_is_never_covered_even_45s_after_open() {
        let open_ts = session_open_ts();
        let now = open_ts + 45;
        let status = evaluate_md_freshness_snapshot("ZZLC04", "5m", 2, Some(open_ts - 20_000), now);
        assert_eq!(status.freshness_state, "insufficient");
        let report = report_from(vec![status]);
        let provider = NyseWeekdaysProvider;
        let now_utc = chrono::DateTime::<Utc>::from_timestamp(now, 0).unwrap();
        assert!(
            !readiness_blocked_only_by_pending_first_session_bar(&report, &provider, now_utc),
            "insufficient history is a genuine problem, never a pending-first-bar condition"
        );
    }

    #[test]
    fn obf_lc_05_one_covered_one_not_refuses_the_whole_verdict() {
        let open_ts = session_open_ts();
        let now = open_ts + 45;
        let pending = evaluate_md_freshness_snapshot("ZZLC05A", "5m", 10, Some(open_ts - 20_000), now);
        let missing = evaluate_md_freshness_snapshot("ZZLC05B", "5m", 0, None, now);
        let report = report_from(vec![pending, missing]);
        let provider = NyseWeekdaysProvider;
        let now_utc = chrono::DateTime::<Utc>::from_timestamp(now, 0).unwrap();
        assert!(
            !readiness_blocked_only_by_pending_first_session_bar(&report, &provider, now_utc),
            "a mixed verdict must never be softened just because one of several \
             blockers happens to be a pending first bar"
        );
    }

    #[test]
    fn obf_lc_06_no_blockers_at_all_is_false_defensively() {
        let report = MultiSymbolFreshnessReport {
            aggregate_status: "ok".to_string(),
            start_allowed: true,
            required_symbols: vec![],
            per_symbol: vec![],
            blockers: vec![],
        };
        let provider = NyseWeekdaysProvider;
        let now_utc = chrono::DateTime::<Utc>::from_timestamp(REF_INSTANT, 0).unwrap();
        assert!(!readiness_blocked_only_by_pending_first_session_bar(
            &report, &provider, now_utc
        ));
    }
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2: typed autonomous arm outcome
// ---------------------------------------------------------------------------

/// Typed success outcome of [`AppState::try_autonomous_arm_typed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutonomousArmOutcome {
    /// Integrity was already armed (`integrity.disarmed == false`) —
    /// idempotent success, no DB access performed.
    AlreadyArmed,
    /// Integrity was disarmed but the persisted arm-state row was `ARMED`
    /// (the ordinary clean-stop-then-restart daily cycle); in-memory
    /// integrity was advanced to armed and re-persisted.
    ArmedFromPersistedState,
}

/// Typed failure outcome of [`AppState::try_autonomous_arm_typed`]. Never a
/// broad free-text variant -- the durable daily coordinator classifies this
/// value exclusively by variant, never by parsing rendered text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutonomousArmRejection {
    /// `integrity.halted == true`. Operator halt wins unconditionally and is
    /// never reversible by the autonomous arm seam.
    IntegrityHalted,
    /// No DB is configured on this daemon; autonomous arm cannot verify
    /// prior session state.
    DatabaseNotConfigured,
    /// A DB is configured, but no arm-state row exists yet (first-time
    /// install, or the DB was wiped). Requires one manual operator arm.
    NoPersistedArmState,
    /// The persisted arm-state row is `DISARMED`, with an optional stored
    /// reason.
    DurableDisarmed { reason: Option<String> },
    /// A DB operation (`load_arm_state` or `persist_arm_state_canonical`)
    /// failed against an otherwise-configured, otherwise-reachable
    /// database. `operation` names the specific call that failed.
    TemporaryDatabaseOperationFailure { operation: &'static str },
}

impl AutonomousArmRejection {
    /// Render the exact historical `try_autonomous_arm()` message text for
    /// each rejection, so the `Result<(), String>` compatibility wrapper
    /// remains byte-for-byte unchanged for every existing caller/test.
    fn legacy_message(&self) -> String {
        match self {
            Self::IntegrityHalted => {
                "operator halt asserted; autonomous arm refused (integrity.halted=true)".to_string()
            }
            Self::DatabaseNotConfigured => {
                "no DB configured; autonomous arm requires persisted arm state".to_string()
            }
            Self::NoPersistedArmState => "no prior arm state in DB; operator must arm manually \
                 at least once (first-time install or DB was wiped)"
                .to_string(),
            Self::DurableDisarmed { reason } => {
                let reason_str = reason.as_deref().unwrap_or("unknown");
                format!("DB arm state is DISARMED (reason={reason_str}); autonomous arm refused")
            }
            Self::TemporaryDatabaseOperationFailure { operation } => {
                format!("autonomous arm: {operation} failed")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A-ATOMICITY-SINGLE-SNAPSHOT-REPAIR:
// one frozen start-attempt authority snapshot.
//
// Every environment/config/calendar/provider/fleet/clock input a single
// `start_execution_runtime` attempt needs is resolved exactly once here and
// reused, by reference, for: the daily-data-readiness gate, the
// dynamic-selection start-gate evaluation, and the spawned execution loop's
// per-symbol dispatch assignments. No consumer re-reads env, the watchlist
// artifact, the calendar/provider/instrument registries, or the fleet-id
// env var a second time within the same start attempt — and `now_utc` goes
// through the same test-overridable clock (`AppState::daily_data_readiness_
// now`) every consumer previously read independently (the dynamic-selection
// evaluator used a raw, non-overridable `Utc::now()` before this repair,
// which could disagree with the readiness gate's clock across a test
// boundary or a slow tick).
// ---------------------------------------------------------------------------

/// BUNDLE-7-PHASE-7A-SINGLE-FROZEN-FLEET-AUTHORITY-CLOSURE: where a
/// [`FrozenStrategyFleet`] was sourced from — explicit and deterministic, so
/// the snapshot never reads two candidate fleet sources and silently prefers
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrozenStrategyFleetSource {
    /// `AppState::strategy_fleet_snapshot()` returned `Some(_)`: either the
    /// production boot-time `MQK_STRATEGY_IDS` cache, or a test-injected
    /// value via `AppState::set_strategy_fleet_for_test`. This is the
    /// authoritative source whenever it is present — the test-injection seam
    /// several existing integration tests rely on stays authoritative, with
    /// no second production fleet source competing with it.
    AppStateSnapshot,
    /// `AppState::strategy_fleet_snapshot()` was genuinely `None` (no test
    /// injection, no boot-time `MQK_STRATEGY_IDS`) — the one permitted
    /// fallback: a single direct `daily_data_readiness::fleet_ids_from_env()`
    /// read, performed here and nowhere else in the same start attempt.
    EnvFallback,
}

/// BUNDLE-7-PHASE-7A-SINGLE-FROZEN-FLEET-AUTHORITY-CLOSURE: one frozen
/// strategy-fleet capture per start attempt.
///
/// Prior to this repair, `StartAttemptAuthoritySnapshot::resolve` read the
/// strategy fleet from two independent sources —
/// `daily_data_readiness::fleet_ids_from_env()` (a live env read, for
/// `configured_strategy_ids`) and `AppState::strategy_fleet_snapshot()` (the
/// boot-time/test-injected cache, for the B1A bootstrap) — which could
/// disagree within the same start attempt if `MQK_STRATEGY_IDS` changed
/// between boot and start, or if a test injected one fleet via
/// `set_strategy_fleet_for_test` while leaving the process-global env var set
/// to something else. Every fleet-derived decision within a single
/// `start_execution_runtime` call now reads this one struct: B1A bootstrap/
/// effective binding, the dynamic-selection `configured_strategy_ids`
/// universe, and the legacy single-symbol assignment's `strategy_id`
/// (`MultiSymbolConfigRawInputs::legacy_strategy_id`, via
/// [`crate::state::read_multi_symbol_config_raw_inputs_from_env_and_fleet`]).
/// None of them call `AppState::strategy_fleet_snapshot()`,
/// `daily_data_readiness::fleet_ids_from_env()`, or read `MQK_STRATEGY_IDS`
/// a second time.
struct FrozenStrategyFleet {
    strategy_ids: Vec<String>,
    /// Consulted by `frozen_strategy_fleet_tests` (below) to prove requirement
    /// 1's "explicit and deterministic" source attribution; reserved for
    /// future operator-facing observability (e.g. surfacing which source a
    /// given start attempt's fleet came from). Not read by any other
    /// production decision in this patch.
    #[allow(dead_code)]
    source: FrozenStrategyFleetSource,
}

impl FrozenStrategyFleet {
    /// Exactly one fleet-resolution operation: read `AppState::
    /// strategy_fleet_snapshot()` once; only when that is genuinely `None`,
    /// fall back to exactly one direct `daily_data_readiness::
    /// fleet_ids_from_env()` read. `Some([])` and `None` are both treated as
    /// "no fleet configured" by `NativeStrategyBootstrap::bootstrap` (a
    /// present-but-empty vector behaves identically to `None`), so an empty
    /// frozen fleet still fails closed exactly where the existing Dormant/
    /// STRATEGY-DORMANCY-01 logic already requires a configured strategy.
    async fn resolve(state: &AppState) -> Self {
        match state.strategy_fleet_snapshot().await {
            Some(entries) => Self {
                strategy_ids: entries.into_iter().map(|e| e.strategy_id).collect(),
                source: FrozenStrategyFleetSource::AppStateSnapshot,
            },
            None => Self {
                strategy_ids: crate::daily_data_readiness::fleet_ids_from_env().unwrap_or_default(),
                source: FrozenStrategyFleetSource::EnvFallback,
            },
        }
    }

    /// First non-empty fleet entry, or `None` for an empty fleet — the
    /// legacy single-symbol path's `strategy_id` input.
    fn first(&self) -> Option<&str> {
        self.strategy_ids.first().map(String::as_str)
    }
}

struct StartAttemptAuthoritySnapshot {
    now_utc: chrono::DateTime<Utc>,
    effective_mode: crate::dynamic_selection_mode::EffectiveDynamicSelectionMode,
    multi_symbol_config:
        Result<crate::state::MultiSymbolRuntimeConfig, crate::state::MultiSymbolConfigError>,
    readiness_context: crate::daily_data_readiness::DailyDataReadinessContext,
    /// BUNDLE-7-PHASE-7A-SINGLE-FROZEN-FLEET-AUTHORITY-CLOSURE requirement 1:
    /// the one frozen fleet capture for this start attempt — see
    /// [`FrozenStrategyFleet`].
    frozen_fleet: FrozenStrategyFleet,
    /// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 1: the frozen
    /// premarket required symbol/timeframe vector — the legacy
    /// PREMARKET-DATA-READINESS-GATE-01 gate below must consume this field,
    /// never call `required_symbols_for_freshness_gate_from_env()` itself a
    /// second time within the same start attempt.
    required_freshness_symbols: Vec<crate::market_data_freshness::RequiredSymbolTimeframe>,
    /// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 1: B1A's native
    /// strategy bootstrap and its derived effective runtime binding, folded
    /// into this one authority object instead of being resolved as a
    /// separate local pair before the snapshot exists.
    native_strategy_bootstrap: NativeStrategyBootstrap,
    effective_runtime_binding: mqk_runtime::native_strategy::EffectiveRuntimeBinding,
}

impl StartAttemptAuthoritySnapshot {
    /// Resolve every start-attempt input exactly once, so no other part of
    /// `start_execution_runtime` reads env, watchlist state, the
    /// calendar/provider/instrument registries, the fleet-id env var, or
    /// the clock independently. Pure I/O only (env vars, at most one
    /// watchlist-artifact file read, the provider/instrument/calendar
    /// registry files, and the shared test-overridable clock) — no DB, no
    /// broker, no network. Infallible: this only *constructs* the B1A
    /// native strategy bootstrap once (folding that construction into this
    /// one authority object closes the ordering gap where the snapshot's
    /// other fields — mode, clock, `MultiSymbolRuntimeConfig`, readiness
    /// context, fleet ids — used to be resolved strictly after bootstrap/
    /// binding already existed as a separate local pair). It never decides
    /// whether the attempt may proceed with that bootstrap — the B1A
    /// Failed/Dormant gate check stays in `start_execution_runtime`, at its
    /// original position (after every pre-DB deployment/capital/policy
    /// gate, before the fixed-window-override check), reading this field
    /// by reference. Gate order, fault classes, and messages are unchanged
    /// — only *when the values are computed* moved earlier, to this one
    /// call.
    async fn resolve(state: &AppState) -> Self {
        let mode_resolution =
            crate::dynamic_selection_mode::resolve_dynamic_selection_mode_from_env();
        let effective_mode = crate::dynamic_selection_mode::effective_mode(
            &mode_resolution,
            state.deployment_mode(),
            state.runtime_selection.broker_kind,
        );
        let now_utc = state.daily_data_readiness_now().await;

        // BUNDLE-7-PHASE-7A-SINGLE-FROZEN-FLEET-AUTHORITY-CLOSURE
        // requirement 1: exactly one fleet-resolution operation for this
        // start attempt. Every fleet-derived value below (B1A bootstrap, the
        // legacy single-symbol assignment's strategy_id, and the
        // dynamic-selection `configured_strategy_ids` universe) is derived
        // from this one `FrozenStrategyFleet` — never a second,
        // independently-timed `AppState::strategy_fleet_snapshot()` or
        // `daily_data_readiness::fleet_ids_from_env()` call.
        let frozen_fleet = FrozenStrategyFleet::resolve(state).await;

        // BUNDLE-7-PHASE-7A-TRUE-ATOMIC requirement 1 (carried forward): one
        // raw read of the watchlist artifact and the legacy
        // MQK_STRATEGY_SYMBOL/timeframe env vars, shared by both
        // `MultiSymbolRuntimeConfig` construction and the premarket
        // freshness gate's required-symbol resolution below.
        // requirement 4 (this patch): `legacy_strategy_id` comes from
        // `frozen_fleet.first()` — never a second, independent
        // `MQK_STRATEGY_IDS` read via `first_strategy_id_from_env()`.
        let multi_symbol_raw_inputs =
            crate::state::read_multi_symbol_config_raw_inputs_from_env_and_fleet(
                frozen_fleet.first(),
            );
        let multi_symbol_config = multi_symbol_raw_inputs.build_config();
        let required_freshness_symbols =
            crate::market_data_freshness::required_symbols_with_source(
                &multi_symbol_raw_inputs.watchlist_outcome,
                multi_symbol_raw_inputs.legacy_timeframe.as_deref(),
                multi_symbol_raw_inputs.legacy_symbol.as_deref(),
            )
            .required;

        // B1A construction, folded in: the exact same
        // `bootstrap_with_effective_binding` call
        // `autonomous_runtime_context::resolve_autonomous_runtime_context_
        // from_fleet` uses (the equivalent pure helper), now driven from
        // `frozen_fleet.strategy_ids` — the same vector `configured_
        // strategy_ids` and the legacy single-symbol assignment above use.
        // `Some(&frozen_fleet.strategy_ids)` and the pre-repair `Option`
        // (`None` when `strategy_fleet_snapshot()` was absent) are
        // behaviorally identical inputs to `NativeStrategyBootstrap::
        // bootstrap`: `None` and `Some([])` both resolve to `Dormant`.
        // `resolve` only *constructs* the bootstrap once; it never decides
        // whether the attempt may proceed with it — the B1A Failed/Dormant
        // gate check stays in `start_execution_runtime`, at its original
        // position, reading this field by reference.
        let (native_strategy_bootstrap, effective_runtime_binding) =
            bootstrap_with_effective_binding(Some(frozen_fleet.strategy_ids.as_slice()));

        Self {
            now_utc,
            effective_mode,
            multi_symbol_config,
            readiness_context: crate::daily_data_readiness::load_readiness_context_from_env(),
            frozen_fleet,
            required_freshness_symbols,
            native_strategy_bootstrap,
            effective_runtime_binding,
        }
    }

    /// The frozen legacy assignment vector the execution loop dispatches
    /// against — `snapshot.multi_symbol_config`'s `symbols`, or empty when
    /// resolution failed (matches the pre-existing no-op behavior of
    /// `tick_strategy_dispatch` when strategy dispatch is not configured).
    fn legacy_assignments(&self) -> Vec<crate::state::SymbolStrategyAssignment> {
        self.multi_symbol_config
            .as_ref()
            .map(|cfg| cfg.symbols.clone())
            .unwrap_or_default()
    }
}

impl AppState {
    /// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: build the
    /// authoritative, frozen dynamic-selection start snapshot for `run_id`.
    ///
    /// Consumes the already-resolved `StartAttemptAuthoritySnapshot` — never
    /// re-resolves the mode, `MultiSymbolRuntimeConfig`, calendar/provider
    /// registries, fleet ids, or clock itself (ATOMICITY-SINGLE-SNAPSHOT-
    /// REPAIR). Evaluates `evaluate_dynamic_selection_start_gate` exactly
    /// once. Never touches `AppState` — the caller commits the returned
    /// value later, at the same point `ProductionRuntimeStartEffects`
    /// publishes every other run-start effect.
    ///
    /// `Ok(state)` covers every disposition except `PaperEnforcedRefused`:
    /// `Off` (zero further I/O), `ShadowAllowed`, `ShadowInvalid` (including
    /// a Shadow-mode `MultiSymbolRuntimeConfig` resolution failure — Shadow
    /// never blocks the run), and `PaperEnforcedAllowed`. `Err(_)` covers
    /// `PaperEnforcedRefused` and a PaperEnforced-mode config-resolution
    /// failure — both refuse the whole start before any run advancement,
    /// with no `AppState` mutation.
    async fn build_dynamic_selection_start_snapshot(
        self: &Arc<Self>,
        run_id: uuid::Uuid,
        snapshot: &StartAttemptAuthoritySnapshot,
    ) -> Result<
        (
            DynamicSelectionRuntimeState,
            crate::dynamic_selection_dispatch_authority::RuntimeStrategyDispatchAuthority,
        ),
        RuntimeLifecycleError,
    > {
        use crate::dynamic_selection_dispatch_authority::RuntimeStrategyDispatchAuthority;
        use crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition;
        use mqk_portfolio::DynamicSelectionMode;

        let effective = &snapshot.effective_mode;

        if effective.effective_mode == DynamicSelectionMode::Off {
            // Off: zero further I/O — no config resolution, no calendar
            // provider, no plan builder, no promotion query, no host pool.
            return Ok((
                DynamicSelectionRuntimeState {
                    run_id,
                    disposition: DynamicSelectionStartGateDisposition::Off,
                    configured_mode: effective.configured_mode,
                    effective_mode: effective.effective_mode,
                    live_lock_applied: effective.live_lock_applied,
                    plan: None,
                    plan_id: None,
                    selected_pairs: Vec::new(),
                    host_pool_present: false,
                    reasons: Vec::new(),
                    approved_for_live: false,
                    evidence_persisted: false,
                    evidence_validation_state: None,
                },
                RuntimeStrategyDispatchAuthority::Legacy {
                    assignments: snapshot.legacy_assignments(),
                },
            ));
        }

        // Non-Off (Shadow or PaperEnforced) is only reachable when
        // deployment_mode==Paper && broker_kind==Alpaca (the mode live-lock
        // proves this) — exactly the predicate the pre-existing
        // daily-data-readiness/premarket-freshness gates above already use —
        // so reading the frozen snapshot's config result unconditionally is
        // safe. ATOMICITY-SINGLE-SNAPSHOT-REPAIR: this is the same
        // `MultiSymbolRuntimeConfig` result the daily-data-readiness gate
        // (when applicable) and the spawned loop's dispatch assignments use
        // — never a second, independently-resolved value.
        let multi_symbol_config = match snapshot.multi_symbol_config.as_ref() {
            Ok(cfg) => cfg,
            Err(err) => {
                if effective.effective_mode == DynamicSelectionMode::Shadow {
                    // Shadow never blocks the run — record the truthful
                    // failure and let the legacy start continue.
                    return Ok((
                        DynamicSelectionRuntimeState {
                            run_id,
                            disposition: DynamicSelectionStartGateDisposition::ShadowInvalid,
                            configured_mode: effective.configured_mode,
                            effective_mode: effective.effective_mode,
                            live_lock_applied: effective.live_lock_applied,
                            plan: None,
                            plan_id: None,
                            selected_pairs: Vec::new(),
                            host_pool_present: false,
                            reasons: vec![
                                crate::dynamic_selection_start_gate::DynamicSelectionStartGateReason::PlanInvalid {
                                    truth_state: format!(
                                        "multi_symbol_config_unavailable:{}",
                                        err.as_str()
                                    ),
                                },
                            ],
                            approved_for_live: false,
                            evidence_persisted: false,
                            evidence_validation_state: None,
                        },
                        RuntimeStrategyDispatchAuthority::Legacy {
                            assignments: snapshot.legacy_assignments(),
                        },
                    ));
                }
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.dynamic_selection_config_unavailable",
                    "dynamic_selection",
                    format!(
                        "dynamic selection paper_enforced start refused: MultiSymbolRuntimeConfig \
                         resolution failed: {} (DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A)",
                        err.as_str()
                    ),
                ));
            }
        };

        // Calendar/session authority + provider/instrument registries +
        // fleet ids + clock: all frozen in `snapshot`, resolved exactly once
        // — never re-read here.
        let readiness_context = &snapshot.readiness_context;
        let configured_strategy_ids = &snapshot.frozen_fleet.strategy_ids;
        let now_utc = snapshot.now_utc;
        let run_id_str = run_id.to_string();

        let context = crate::dynamic_selection_start_gate::build_dynamic_selection_context(
            &run_id_str,
            effective,
            multi_symbol_config,
            readiness_context.calendar_provider.as_ref(),
            now_utc,
        );

        let plan_ctx = crate::dynamic_selection_plan_builder::DynamicSelectionPlanBuildContext {
            db: self.db.as_ref(),
            st: self,
            calendar_provider: readiness_context.calendar_provider.as_ref(),
            provider_configs: &readiness_context.provider_configs,
            instruments: &readiness_context.instruments,
        };

        let outcome = crate::dynamic_selection_start_gate::evaluate_dynamic_selection_start_gate(
            &plan_ctx,
            multi_symbol_config,
            configured_strategy_ids,
            effective,
            context,
            &run_id_str,
            now_utc,
        )
        .await;

        // Part 2: mint the one deterministic plan identity whenever a plan
        // was actually built (Shadow* and PaperEnforcedAllowed) — evidence/
        // status truth for every disposition that has a plan, not only the
        // dispatch-authoritative one.
        let plan_id = outcome
            .plan
            .as_ref()
            .map(crate::dynamic_selection_dispatch_authority::derive_dynamic_selection_plan_id);

        // Phase 7C Part 2 — one write authority: persist durable plan
        // evidence per disposition policy, before arm/begin/Starting
        // publication/pool activation/spawn/barrier release (all of which
        // happen only after this function's caller proceeds past this
        // point). Must run before the `PaperEnforcedRefused` early-return
        // below so refusal evidence is persisted before the start is
        // actually refused. Never runs for `Off` (`outcome.plan` is always
        // `None` there).
        let mut evidence_persisted = false;
        let mut evidence_validation_state: Option<String> = None;
        if let (Some(plan), Some(evidence_plan_id)) = (outcome.plan.as_ref(), plan_id) {
            let new_plan =
                crate::dynamic_selection_evidence_writer::build_new_dynamic_selection_plan(
                    plan,
                    evidence_plan_id,
                    run_id,
                    outcome.disposition,
                    now_utc,
                )
                .map_err(|e| {
                    RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.dynamic_selection_evidence_build_failed",
                        "dynamic_selection_evidence",
                        format!(
                            "dynamic selection start refused: could not build durable evidence \
                         payload for plan_id={evidence_plan_id}: {e} \
                         (DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7C)"
                        ),
                    )
                });

            match (new_plan, self.db.as_ref()) {
                (Ok(new_plan), Some(db)) => {
                    match mqk_db::insert_dynamic_selection_plan(db, new_plan).await {
                        Ok(mqk_db::InsertDynamicSelectionPlanOutcome::Inserted)
                        | Ok(mqk_db::InsertDynamicSelectionPlanOutcome::AlreadyExists) => {
                            evidence_persisted = true;
                        }
                        Ok(mqk_db::InsertDynamicSelectionPlanOutcome::PayloadCollision {
                            detail,
                        }) => {
                            if outcome.disposition
                                == DynamicSelectionStartGateDisposition::PaperEnforcedAllowed
                            {
                                return Err(RuntimeLifecycleError::forbidden(
                                    "runtime.start_refused.dynamic_selection_evidence_payload_collision",
                                    "dynamic_selection_evidence",
                                    format!(
                                        "dynamic selection paper_enforced start refused: durable \
                                         evidence payload collision for plan_id={evidence_plan_id}: \
                                         {detail} (DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7C)"
                                    ),
                                ));
                            }
                            // Shadow*/refused: never blocks, but must never
                            // claim evidence was durably persisted when a
                            // collision means it was not.
                        }
                        Err(e) => {
                            if outcome.disposition
                                == DynamicSelectionStartGateDisposition::PaperEnforcedAllowed
                            {
                                return Err(RuntimeLifecycleError::forbidden(
                                    "runtime.start_refused.dynamic_selection_evidence_write_failed",
                                    "dynamic_selection_evidence",
                                    format!(
                                        "dynamic selection paper_enforced start refused: durable \
                                         evidence write failed for plan_id={evidence_plan_id}: {e} \
                                         (DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7C)"
                                    ),
                                ));
                            }
                        }
                    }

                    if outcome.disposition
                        == DynamicSelectionStartGateDisposition::PaperEnforcedAllowed
                    {
                        // Part 3: read-validate the exact plan before this
                        // function returns — i.e. before arm/begin/Starting
                        // publication/pool activation/spawn/barrier release.
                        let expected_bindings =
                            crate::dynamic_selection_start_gate::selected_host_pool_keys(plan);
                        let validation =
                            crate::dynamic_selection_evidence_validator::validate_dynamic_selection_evidence(
                                db,
                                evidence_plan_id,
                                run_id,
                                Some(&expected_bindings),
                            )
                            .await;
                        evidence_validation_state = Some(validation.code().to_string());
                        if !validation.is_valid() {
                            return Err(RuntimeLifecycleError::forbidden(
                                "runtime.start_refused.dynamic_selection_evidence_invalid",
                                "dynamic_selection_evidence",
                                format!(
                                    "dynamic selection paper_enforced start refused: durable \
                                     evidence failed read-side validation for \
                                     plan_id={evidence_plan_id}: {validation:?} \
                                     (DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7C)"
                                ),
                            ));
                        }
                    }
                }
                (Ok(_), None) => {
                    if outcome.disposition
                        == DynamicSelectionStartGateDisposition::PaperEnforcedAllowed
                    {
                        return Err(RuntimeLifecycleError::forbidden(
                            "runtime.start_refused.dynamic_selection_evidence_db_unavailable",
                            "dynamic_selection_evidence",
                            "dynamic selection paper_enforced start refused: no DB pool \
                             available to persist durable plan evidence \
                             (DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7C)"
                                .to_string(),
                        ));
                    }
                }
                (Err(err), _) => {
                    if outcome.disposition
                        == DynamicSelectionStartGateDisposition::PaperEnforcedAllowed
                    {
                        return Err(err);
                    }
                }
            }
        }

        if outcome.disposition == DynamicSelectionStartGateDisposition::PaperEnforcedRefused {
            let reason_codes: Vec<&'static str> =
                outcome.reasons.iter().map(|r| r.code()).collect();
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.start_refused.dynamic_selection_paper_enforced_refused",
                "dynamic_selection_start_gate",
                format!(
                    "dynamic selection paper_enforced start gate refused start: run_id={run_id}; \
                     reasons={reason_codes:?} (DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A)"
                ),
            ));
        }

        let selected_pairs = outcome
            .plan
            .as_ref()
            .map(crate::dynamic_selection_start_gate::selected_host_pool_keys)
            .unwrap_or_default();

        // Parts 1/3/9: `PaperEnforcedAllowed` is the only disposition that
        // ever builds a `DynamicPaperEnforced` dispatch authority. Building
        // it here — before this function returns, before the loop barrier
        // releases — and failing the whole start closed on any coherence
        // defect is the positive Phase 7B dispatch-authority guard that
        // replaces the prior temporary
        // `dynamic_selection_dispatch_not_wired` interlock.
        let (host_pool_present, dispatch_authority) = match (outcome.plan.as_ref(), outcome.host_pool)
        {
            (Some(plan), Some(host_pool)) => {
                // TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01: recompute
                // directly from `plan` (already available in this arm)
                // rather than trusting the outer `Option<Uuid>` invariant via
                // `.expect(...)` — no trust-boundary panic on this path, ever.
                let plan_id =
                    crate::dynamic_selection_dispatch_authority::derive_dynamic_selection_plan_id(
                        plan,
                    );
                match crate::dynamic_selection_dispatch_authority::build_dynamic_paper_enforced_dispatch_authority(
                    run_id, plan, plan_id, host_pool,
                ) {
                    Ok(authority) => (true, authority),
                    Err(build_err) => {
                        return Err(RuntimeLifecycleError::forbidden(
                            "runtime.start_refused.dynamic_selection_dispatch_authority_invalid",
                            "dynamic_selection_dispatch_authority",
                            format!(
                                "dynamic selection paper_enforced start refused: the selected-host \
                                 dispatch authority could not be built coherently from the frozen \
                                 plan and host pool for run_id={run_id}: {build_err:?} (code={}) \
                                 (DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7B-SELECTED-HOST-\
                                 ECONOMIC-DISPATCH-CLOSURE)",
                                build_err.code(),
                            ),
                        ));
                    }
                }
            }
            _ => (
                false,
                crate::dynamic_selection_dispatch_authority::RuntimeStrategyDispatchAuthority::Legacy {
                    assignments: snapshot.legacy_assignments(),
                },
            ),
        };

        Ok((
            DynamicSelectionRuntimeState {
                run_id,
                disposition: outcome.disposition,
                configured_mode: effective.configured_mode,
                effective_mode: effective.effective_mode,
                live_lock_applied: effective.live_lock_applied,
                plan: outcome.plan.map(Arc::new),
                plan_id,
                selected_pairs,
                host_pool_present,
                reasons: outcome.reasons,
                approved_for_live: false,
                evidence_persisted,
                evidence_validation_state,
            },
            dispatch_authority,
        ))
    }

    pub async fn start_execution_runtime(
        self: &Arc<Self>,
    ) -> Result<StatusSnapshot, RuntimeLifecycleError> {
        let _op = self.lifecycle_op.lock().await;
        self.reap_finished_execution_loop().await?;

        if !self.deployment_readiness().start_allowed {
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.start_refused.deployment_mode_unproven",
                "deployment_mode",
                self.deployment_readiness()
                    .blocker
                    .clone()
                    .unwrap_or_else(|| "deployment mode is not start-ready".to_string()),
            ));
        }

        if self.integrity.read().await.is_execution_blocked() {
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.control_refusal.integrity_disarmed",
                "integrity_armed",
                "GATE_REFUSED: integrity disarmed or halted; arm integrity first",
            ));
        }

        if self.deployment_mode() == DeploymentMode::LiveCapital
            && !matches!(self.operator_auth, OperatorAuthMode::TokenRequired(_))
        {
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.start_refused.capital_requires_operator_token",
                "operator_auth",
                "live-capital mode requires a real operator token; \
                 dev-no-token and missing-token modes are not permitted for capital execution",
            ));
        }

        if let Some(run_id) = self.active_owned_run_id().await {
            return Err(RuntimeLifecycleError::conflict(
                "runtime.control_refusal.already_owned",
                format!("runtime already active under local ownership: {run_id}"),
            ));
        }

        // BRK-00R-04: paper+alpaca WS continuity start gate.
        //
        // The Alpaca paper path requires proven WS continuity before runtime start.
        // ColdStartUnproven and GapDetected are not start-safe: no live WS cursor
        // has been established, so event delivery ordering cannot be trusted.
        //
        // Placed before db_pool() so the check is:
        //   - at the earliest honest enforcement point (continuity state is in-memory)
        //   - in-process testable without a database
        //   - before any DB resources or runtime lease are acquired
        //
        // Full WS transport implementation (subscribe/reconnect/cursor establishment)
        // remains open; this patch only moves the failure forward from first tick.
        if self.deployment_mode() == DeploymentMode::Paper
            && self.runtime_selection.broker_kind == Some(BrokerKind::Alpaca)
        {
            let continuity = self.alpaca_ws_continuity().await;
            if !continuity.is_continuity_proven() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.paper_alpaca_ws_continuity_unproven",
                    "alpaca_ws_continuity",
                    format!(
                        "paper+alpaca requires proven Alpaca WS continuity before starting; \
                         current state: '{}' (WS_CONTINUITY_UNPROVEN) — the WS transport \
                         must establish a live cursor before paper+alpaca can proceed; \
                         full WS transport work remains open",
                        continuity.as_status_str()
                    ),
                ));
            }
        }

        // BRK-09R: Reconcile truth start gate for broker-backed paper path.
        //
        // If the persisted reconcile status is "dirty" or "stale" — meaning the
        // prior session ended with a known broker/local drift condition — refuse
        // start until the operator has investigated and the reconcile state is
        // cleared to "ok" (or the DB row is absent, meaning no prior evidence).
        //
        // "unknown" is the initial in-memory state at fresh boot (no prior run),
        // and is allowed through: it carries no evidence of prior drift.
        //
        // Gate ordering: fires after the WS continuity gate so WS issues are
        // surfaced first.  A dirty reconcile AND a non-live WS yields the WS gate
        // as the named blocker; reconcile is only surfaced when WS is clean.
        //
        // current_reconcile_snapshot() reads from DB when available, falling back
        // to in-memory; it does not require db_pool() to be non-None.
        if self.deployment_mode() == DeploymentMode::Paper
            && self.runtime_selection.broker_kind == Some(BrokerKind::Alpaca)
        {
            let reconcile = self.current_reconcile_snapshot().await;
            if matches!(reconcile.status.as_str(), "dirty" | "stale") {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.reconcile_dirty",
                    "reconcile_truth",
                    format!(
                        "paper+alpaca cannot start with dirty or stale reconcile truth; \
                         current reconcile status: '{}'; the prior session's broker/local \
                         drift must be investigated and the reconcile state must be cleared \
                         before restarting; reconcile note: {}",
                        reconcile.status,
                        reconcile.note.as_deref().unwrap_or("none"),
                    ),
                ));
            }
        }

        // Live-capital WS continuity gate.
        //
        // Placed here — before db_pool() — so it is:
        //   - in-process testable without a database or real broker credentials
        //   - before any DB resources or run rows are acquired (prevents dangling
        //     "Created" run rows on a continuity-refused start)
        //   - symmetric with the Paper+Alpaca continuity gate above
        //
        // Previous position (after build_execution_orchestrator) required
        // orchestrator.release_runtime_leadership() on failure and could leave
        // a "Created" run row in the DB if the check failed after insert_run.
        if self.deployment_mode() == DeploymentMode::LiveCapital {
            let continuity = self.alpaca_ws_continuity().await;
            if !continuity.is_continuity_proven() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.capital_ws_continuity_unproven",
                    "alpaca_ws_continuity",
                    format!(
                        "live-capital requires proven Alpaca WS continuity before starting; \
                         current continuity state: '{}' — \
                         run in live-shadow mode to establish a proven cursor, \
                         then transition to capital",
                        continuity.as_status_str()
                    ),
                ));
            }
        }

        // TV-01 / TV-02C: Evaluate artifact intake exactly once.
        //
        // Hoisted here so the same evaluation result is used for:
        //   - TV-02C deployability gate (below)
        //   - TV-01C provenance capture at successful run start (further below)
        //
        // Evaluating twice would create a TOCTOU window: a file swap or env-var
        // mutation between the gate check and the provenance capture could let
        // a different artifact identity pass the gate while a different one is
        // recorded as the run's provenance.  Single evaluation closes that gap.
        let artifact_intake = evaluate_artifact_intake_guarded();

        // TV-02C: Artifact deployability gate.
        //
        // If MQK_ARTIFACT_PATH is configured and intake is Accepted, the artifact
        // must also pass the deployability gate (deployability_gate.json written by
        // the Python TV-02 pipeline) before runtime start is allowed.
        //
        // Contract:
        //   NotConfigured            → no artifact configured; gate not applicable; pass through.
        //   Accepted + Deployable    → minimum criteria met; pass through.
        //   Accepted + not Deployable→ fail-closed: block start with explicit reason.
        //   Invalid / Unavailable   → artifact configured but intake failed; fail-closed.
        //
        // Placed before db_pool() so it is:
        //   - in-process testable without a database
        //   - before any DB resources or run rows are acquired (no dangling rows on refusal)
        {
            match &artifact_intake {
                ArtifactIntakeOutcome::NotConfigured => {
                    // No artifact configured — deployability gate not applicable.
                }
                ArtifactIntakeOutcome::Accepted { artifact_id, .. } => {
                    let raw = std::env::var(ENV_ARTIFACT_PATH).unwrap_or_default();
                    let manifest_path = std::path::PathBuf::from(raw.trim());
                    let deployability =
                        evaluate_artifact_deployability(Some(&manifest_path), artifact_id);
                    if !deployability.is_deployable() {
                        return Err(RuntimeLifecycleError::forbidden(
                            "runtime.start_refused.artifact_not_deployable",
                            "artifact_deployability",
                            format!(
                                "configured artifact failed the deployability gate \
                                 (truth_state='{}'): artifact_id='{}' was accepted for intake \
                                 but did not pass minimum deployability/tradability criteria; \
                                 run the TV-02 Python gate on this artifact to produce a \
                                 deployability_gate.json that passes all checks",
                                deployability.truth_state(),
                                artifact_id,
                            ),
                        ));
                    }
                }
                ArtifactIntakeOutcome::Invalid { reason } => {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.artifact_intake_invalid",
                        "artifact_intake",
                        format!(
                            "artifact intake failed; runtime cannot proceed with a configured \
                             but invalid artifact: {reason}"
                        ),
                    ));
                }
                ArtifactIntakeOutcome::Unavailable { reason } => {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.artifact_intake_unavailable",
                        "artifact_intake",
                        format!(
                            "artifact intake evaluator failed; runtime cannot proceed when \
                             artifact state is unknown: {reason}"
                        ),
                    ));
                }
            }
        }

        // TV-03C: Parity evidence gate.
        //
        // If MQK_ARTIFACT_PATH is configured, parity evidence for the artifact
        // must exist in the same directory and be structurally valid before
        // runtime start is allowed.
        //
        // Contract:
        //   NotConfigured   → no artifact path configured; gate not applicable; pass through.
        //   Present { .. }  → parity_evidence.json readable and valid; pass through.
        //   Absent          → configured artifact has no parity evidence; fail-closed.
        //   Invalid { .. }  → parity_evidence.json exists but is invalid; fail-closed.
        //   Unavailable { .. } → evaluator failed; fail-closed.
        //
        // Placed after TV-02C (artifact deployability) and before TV-04A (capital policy)
        // so the evidence chain is verified before capital authorization runs.
        // Both TV-02C and TV-03C read MQK_ARTIFACT_PATH; absent path → NotConfigured on both.
        //
        // Cross-validation: when both intake and parity evidence are resolved, the
        // artifact_id embedded in parity_evidence.json must match the accepted intake
        // artifact_id.  This mirrors the TV-02C deployability gate cross-validation and
        // closes the artifact-associated evidence chain: parity evidence produced for a
        // different artifact must not satisfy this gate.  `artifact_intake` is the same
        // evaluation result used for TV-02C above (TOCTOU-safe, evaluated once).
        {
            let parity = evaluate_parity_evidence_from_env();
            // Artifact identity cross-validation: Present evidence for a different
            // artifact is not evidence for this artifact.
            if let (
                ArtifactIntakeOutcome::Accepted {
                    artifact_id: ref accepted_id,
                    ..
                },
                ParityEvidenceOutcome::Present {
                    artifact_id: ref parity_id,
                    ..
                },
            ) = (&artifact_intake, &parity)
            {
                if parity_id != accepted_id {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.parity_evidence_artifact_mismatch",
                        "parity_evidence",
                        format!(
                            "parity evidence artifact_id '{}' does not match the accepted \
                             intake artifact_id '{}'; the parity_evidence.json in the artifact \
                             directory was not produced for the configured artifact — re-run the \
                             TV-03 pipeline against the correct artifact",
                            parity_id, accepted_id
                        ),
                    ));
                }
            }
            if !parity.is_start_safe() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.parity_evidence_not_present",
                    "parity_evidence",
                    format!(
                        "parity evidence gate failed \
                         (truth_state='{}'): {}",
                        parity.truth_state(),
                        match &parity {
                            ParityEvidenceOutcome::Absent => {
                                "parity_evidence.json is absent in the artifact directory; \
                                 run the Python TV-03 pipeline to produce parity evidence \
                                 before starting the runtime"
                                    .to_string()
                            }
                            ParityEvidenceOutcome::Invalid { reason } => {
                                format!("parity_evidence.json is structurally invalid: {reason}")
                            }
                            ParityEvidenceOutcome::Unavailable { reason } => {
                                format!("parity evidence evaluator failed: {reason}")
                            }
                            _ => "parity evidence evaluation failed".to_string(),
                        }
                    ),
                ));
            }
        }

        // TV-03D: LiveCapital requires a complete parity trust chain.
        //
        // TV-03C (above) only requires parity evidence to be present and
        // structurally valid — by design (LIVE-TRUST-01 / LT-04), so
        // LiveShadow can run with parity evidence present while
        // `live_trust_complete` is still false. LiveCapital is semantically
        // distinct: `evaluate_mode_transition` already hardcodes every
        // upward transition into LiveCapital as `FailClosed` specifically
        // because `live_trust_complete=false` is a current-build ceiling of
        // the TV-03 Python pipeline — but that policy was previously
        // enforced only at the advisory `mode-change-guidance` route, never
        // at actual cold start (a daemon launched directly into LiveCapital
        // via MQK_DAEMON_DEPLOYMENT_MODE bypassed it entirely).
        //
        // This gate wires the same fail-closed policy into the actual start
        // path, LiveCapital-only, leaving TV-03C's LiveShadow-safe
        // evidence-presence contract untouched
        // (LIVE-CAPITAL-PARITY-COMPLETE-GATE-01, closes A2-FIND-007 /
        // A2-PATCH-008).
        //
        // Placed after TV-03C (evidence presence) and before TV-04F (capital
        // policy) so evidence-chain refusals are surfaced before policy
        // refusals, matching the existing TV-03C→TV-04F ordering rationale.
        if self.deployment_mode() == DeploymentMode::LiveCapital {
            let parity = evaluate_parity_evidence_from_env();
            let live_trust_complete = matches!(
                parity,
                ParityEvidenceOutcome::Present {
                    live_trust_complete: true,
                    ..
                }
            );
            if !live_trust_complete {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.live_capital_parity_trust_incomplete",
                    "live_capital_parity_trust",
                    format!(
                        "live-capital mode requires a complete parity trust chain \
                         (parity_evidence.json present with live_trust_complete=true); \
                         current parity evidence truth_state='{}' does not satisfy this. \
                         This mirrors the fail-closed policy evaluate_mode_transition already \
                         advertises for upward transitions into LiveCapital — no operator \
                         action can lift this until the TV-03 parity pipeline can produce \
                         live_trust_complete=true for this artifact",
                        parity.truth_state()
                    ),
                ));
            }
        }

        // TV-04F: Live-capital requires an explicit capital allocation policy.
        //
        // Paper and LiveShadow modes are permissive: absent policy →
        // NotConfigured → gate not applicable; callers pass through.  This is
        // correct for simulation modes where capital policy enforcement is
        // optional at the operator's discretion.
        //
        // LiveCapital is semantically distinct: real capital requires an
        // explicit, operator-configured capital allocation policy before any
        // live-capital execution is authorized.  NotConfigured in live-capital
        // mode is fail-closed — the operator must explicitly configure and
        // enable a policy.  This prevents silent conflation of paper-safe
        // "no policy = no enforcement" with live-capital authorization.
        //
        // Gate ordering: placed after TV-03C (parity evidence) and before
        // TV-04A (policy validity check).  TV-04A then validates the policy
        // is enabled and structurally correct once TV-04F confirms it exists.
        if self.deployment_mode() == DeploymentMode::LiveCapital {
            let policy = evaluate_capital_policy_from_env();
            if matches!(policy, CapitalPolicyOutcome::NotConfigured) {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.live_capital_requires_capital_policy",
                    "live_capital_policy_required",
                    "live-capital mode requires an explicit capital allocation policy; \
                     set MQK_CAPITAL_POLICY_PATH to a valid capital_allocation_policy.json \
                     before starting live-capital execution; paper and live-shadow modes \
                     do not require a policy — this gate is live-capital-only and enforces \
                     the semantic distinction between paper safety and live-capital authorization",
                ));
            }
        }

        // TV-04A: Capital allocation policy gate.
        //
        // If MQK_CAPITAL_POLICY_PATH is configured, the policy file must be
        // valid and `enabled = true` before runtime start is allowed.
        //
        // Contract:
        //   NotConfigured → no policy configured; gate not applicable; pass through.
        //   Authorized    → policy valid and enabled; pass through.
        //   Denied        → policy present but enabled=false; fail-closed.
        //   PolicyInvalid → policy configured but structurally invalid; fail-closed.
        //   Unavailable   → reserved; fail-closed.
        //
        // Placed before db_pool() so the check is:
        //   - in-process testable without a database
        //   - before any DB resources or run rows are acquired (no dangling rows)
        //   - ordered after TV-02C (artifact deployability) so artifact refusals
        //     are surfaced before capital policy refusals
        {
            let policy = evaluate_capital_policy_from_env();
            if !policy.is_start_safe() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.capital_policy_not_authorized",
                    "capital_allocation_policy",
                    format!(
                        "capital allocation policy gate failed \
                         (truth_state='{}'): {}",
                        policy.truth_state(),
                        match &policy {
                            CapitalPolicyOutcome::Denied { reason } => reason.clone(),
                            CapitalPolicyOutcome::PolicyInvalid { reason } => {
                                format!("policy file is invalid: {reason}")
                            }
                            CapitalPolicyOutcome::Unavailable { reason } => {
                                format!("policy evaluator unavailable: {reason}")
                            }
                            _ => "capital policy evaluation failed".to_string(),
                        }
                    ),
                ));
            }
        }

        // TV-04D: Deployment economics gate.
        //
        // An enabled capital policy must carry a valid `max_portfolio_notional_usd`
        // before runtime start is allowed.  This gate is independent of TV-04A:
        // TV-04A checks whether the policy is enabled; TV-04D checks whether the
        // enabled policy specifies deployment economics bounds.
        //
        // Contract:
        //   NotConfigured      → no policy configured; gate not applicable; pass through.
        //   PolicyDisabled     → enabled=false; TV-04A already blocked; pass through.
        //   EconomicsSpecified → policy enabled + valid portfolio cap; pass through.
        //   EconomicsNotSpecified → policy enabled but no economics bound; fail-closed.
        //   PolicyInvalid      → policy configured but structurally invalid; fail-closed.
        //   Unavailable        → reserved; fail-closed.
        //
        // Placed immediately after TV-04A so that capital policy authorization
        // is confirmed before the economics bound is checked.  Placed before
        // db_pool() so the check is in-process testable without a database.
        {
            let economics = evaluate_deployment_economics_from_env();
            if !economics.is_start_safe() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.deployment_economics_not_specified",
                    "deployment_economics",
                    format!(
                        "deployment economics gate failed \
                         (truth_state='{}'): {}",
                        economics.truth_state(),
                        match &economics {
                            DeploymentEconomicsOutcome::EconomicsNotSpecified { reason } => {
                                reason.clone()
                            }
                            DeploymentEconomicsOutcome::PolicyInvalid { reason } => {
                                format!("economics policy file is invalid: {reason}")
                            }
                            DeploymentEconomicsOutcome::Unavailable { reason } => {
                                format!("economics evaluator unavailable: {reason}")
                            }
                            _ => "deployment economics evaluation failed".to_string(),
                        }
                    ),
                ));
            }
        }

        // B1A: Native strategy bootstrap gate, folded into the one
        // start-attempt authority snapshot (ATOMIC-OWNERSHIP-AND-ROLLBACK-
        // TRUTH-01 requirement 1).
        //
        // Evaluate the native strategy bootstrap from fleet truth (MQK_STRATEGY_IDS)
        // and the daemon plugin registry before acquiring any DB resources.
        //
        // Contract:
        //   Dormant (fleet absent/empty) → pass through.
        //   Active (fleet entry + registry match) → pass through; bootstrap stored.
        //   Failed (fleet entry present, not in registry) → fail-closed.
        //
        // Placed before db_pool() so it is:
        //   - in-process testable without a database
        //   - before any DB resources or run rows are acquired (no dangling rows)
        //   - ordered after all deployment/capital/policy gates (last pre-DB gate)
        //
        // The bootstrap lives only in `start_attempt_snapshot` (a local
        // binding) and is stored in AppState only after a fully successful
        // run start (inside `ProductionRuntimeStartEffects::start_runtime_
        // effects`'s local commit bundle), so the field is never left
        // populated by a failed start attempt.
        //
        // ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 1: this is now
        // the single point every start-attempt input is resolved —
        // `StartAttemptAuthoritySnapshot::resolve` folds in B1A's bootstrap/
        // binding construction alongside the dynamic-selection mode,
        // `MultiSymbolRuntimeConfig`, readiness context, fleet ids,
        // premarket required-symbols vector, and clock — resolved here,
        // once, and reused by every gate/evaluator below (this gate itself,
        // the strict readiness gate, the legacy premarket freshness gate,
        // the dynamic-selection start-gate evaluation, and the spawned
        // execution loop's dispatch assignments via `snapshot.
        // legacy_assignments()`). Gate order, fault classes, and messages
        // are unchanged — only *when* the values are computed moved
        // earlier, to this one call.
        let start_attempt_snapshot = StartAttemptAuthoritySnapshot::resolve(self).await;

        if start_attempt_snapshot.native_strategy_bootstrap.is_failed() {
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.start_refused.native_strategy_bootstrap_failed",
                "native_strategy_bootstrap",
                format!(
                    "native strategy bootstrap failed (truth_state='{}'): {}; \
                     ensure the strategy named in MQK_STRATEGY_IDS is registered \
                     in the daemon plugin registry before starting; \
                     operators must not set MQK_STRATEGY_IDS until the target \
                     strategy engine is wired into the registry",
                    start_attempt_snapshot
                        .native_strategy_bootstrap
                        .truth_state(),
                    start_attempt_snapshot
                        .native_strategy_bootstrap
                        .failure_reason()
                        .unwrap_or("unknown"),
                ),
            ));
        }

        // STRATEGY-DORMANCY-01: Paper+Alpaca autonomous path requires an
        // active strategy bootstrap. Dormant is allowed for non-paper
        // deployments (e.g. LiveShadow running in monitor-only mode); this
        // block is scoped to Paper+Alpaca only, matching the pre-extraction
        // gate exactly.
        if start_attempt_snapshot
            .native_strategy_bootstrap
            .is_dormant()
            && self.deployment_mode() == DeploymentMode::Paper
            && self.runtime_selection.broker_kind == Some(BrokerKind::Alpaca)
        {
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.start_refused.strategy_bootstrap_dormant",
                "native_strategy_bootstrap",
                "paper+alpaca autonomous path requires an active strategy bootstrap; \
                 MQK_STRATEGY_IDS is absent or empty — no strategy engine will generate \
                 decisions; set MQK_STRATEGY_IDS to a registered strategy name \
                 (e.g. 'swing_momentum') and ensure it is enabled in \
                 sys_strategy_registry before starting the autonomous paper path \
                 (STRATEGY-DORMANCY-01)",
            ));
        }

        // DAILY-DATA-READINESS-01C-ENFORCEMENT-01: strict daily data
        // readiness start gate.
        //
        // Applicable only to Paper+ExternalSignalIngestion — the same
        // predicate the PREMARKET-DATA-READINESS-GATE-01 legacy gate below
        // uses, never hardcoded to BrokerKind::Alpaca (contract §C.5).
        //
        // Uses the exact `start_attempt_snapshot.native_strategy_bootstrap`/
        // `.effective_runtime_binding` pair resolved above (B1A, folded into
        // the snapshot) — never a second, independently constructed
        // bootstrap (Phase B's `evaluate_daily_data_readiness_from_env`
        // lifecycle-safety contract).
        //
        // Placed after B1A (bootstrap/binding resolution) and before
        // db_pool()? so this evaluator — not a generic db_pool() error —
        // produces the canonical db_unavailable verdict when the DB is
        // absent (contract §16), and so a blocked verdict denies before any
        // DB resource, run row, or broker/provider call.
        //
        // DAILY-DATA-READINESS-01C-MISSING-ASSIGNMENT-EVIDENCE-REPAIR-01:
        // required ordering is bootstrap/binding resolution (B1A, above) ->
        // capture evaluated_at_utc -> allocate attempt_seq -> attempt
        // assignment resolution -> construct resolved-or-blocked assignment
        // identity -> compute evaluation_id -> construct readiness report ->
        // attempt pre-start evidence persistence -> return blocked verdict or
        // continue. An applicable attempt whose assignment resolution itself
        // fails (`build_multi_symbol_runtime_config_from_env()` returns
        // `Err`) still receives a distinct `attempt_seq`, a real
        // `evaluation_id`, a truthful bounded blocked report, and a genuine
        // pre-start evidence persist attempt — never an early return before
        // any of that exists.
        // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-CANONICAL-CALENDAR-REPAIR-01:
        // narrow shared-calendar blocker input. A configured-but-invalid
        // fixed session-window override (MQK_SESSION_START_HH_MM/
        // _STOP_HH_MM) must never be silently treated as absent by an
        // applicable autonomous start attempt — refuse before any readiness
        // evaluation, run creation, or provider/broker call. Absent or Valid
        // configuration is the ordinary case and falls through unchanged,
        // so this never disturbs the existing readiness evidence ordering
        // below.
        if self.deployment_mode() == DeploymentMode::Paper
            && self.strategy_market_data_source()
                == StrategyMarketDataSource::ExternalSignalIngestion
        {
            if let super::autonomous_daily_operation::FixedWindowOverrideConfig::Invalid {
                detail,
            } =
                super::autonomous_daily_operation::resolve_fixed_window_override_config_from_env()
            {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.fixed_window_override_invalid",
                    "fixed_window_override",
                    format!(
                        "fixed session-window override configuration is invalid: {detail} ({})",
                        super::autonomous_daily_operation::AutonomousDailyPlanReason::FixedWindowOverrideInvalid
                            .as_str()
                    ),
                ));
            }
        }

        let mut daily_data_readiness_evaluation_id: Option<uuid::Uuid> = None;
        if self.deployment_mode() == DeploymentMode::Paper
            && self.strategy_market_data_source()
                == StrategyMarketDataSource::ExternalSignalIngestion
        {
            let evaluated_at_utc = start_attempt_snapshot.now_utc;
            // REPAIR 1 (...MISSING-ASSIGNMENT-EVIDENCE-REPAIR-01): allocate a
            // fresh attempt sequence number for this actual start-gate
            // evaluation (never for a GET/preview evaluation) BEFORE
            // assignment resolution is even attempted, so two
            // otherwise-identical attempts — including two that both fail
            // assignment resolution — never collide on `evaluation_id`.
            let attempt_seq = self.next_daily_data_readiness_attempt_seq();

            // REPAIR 2/3: construct the resolved-or-blocked assignment
            // identity, `evaluation_id`, and readiness report from whichever
            // branch actually happened — never a fabricated
            // `MultiSymbolRuntimeConfig` for the failure branch. Consumes
            // the frozen snapshot's config result — never a second
            // `build_multi_symbol_runtime_config_from_env()` call.
            let (report, evaluation_id, assignment_resolution_error) =
                match &start_attempt_snapshot.multi_symbol_config {
                    Ok(config) => {
                        let readiness_context = &start_attempt_snapshot.readiness_context;
                        let report = crate::daily_data_readiness::evaluate_readiness_with_binding(
                            self.db.as_ref(),
                            config,
                            &start_attempt_snapshot.effective_runtime_binding,
                            readiness_context,
                            evaluated_at_utc,
                        )
                        .await;
                        let evaluation_id = crate::daily_data_readiness::compute_evaluation_id(
                            evaluated_at_utc,
                            attempt_seq,
                            &start_attempt_snapshot.effective_runtime_binding,
                            config,
                        );
                        (report, evaluation_id, None::<&'static str>)
                    }
                    Err(err) => {
                        let top_level_blocker =
                            crate::daily_data_readiness::top_level_blocker_for_config_error(err);
                        let report = crate::daily_data_readiness::blocked_report(top_level_blocker);
                        // Bounded, stable failure identity (REPAIR 2) — never
                        // secrets, full environment dumps, or unbounded
                        // filesystem errors; `MultiSymbolConfigError::as_str()`
                        // is itself already a fixed, small set of literal
                        // strings.
                        let assignment_identity =
                            vec![format!("assignment_resolution_error:{}", err.as_str())];
                        let evaluation_id =
                        crate::daily_data_readiness::compute_evaluation_id_from_assignment_identity(
                            evaluated_at_utc,
                            attempt_seq,
                            &start_attempt_snapshot.effective_runtime_binding,
                            &assignment_identity,
                        );
                        (report, evaluation_id, Some(err.as_str()))
                    }
                };

            // The real write is always attempted (regardless of any test
            // override) — the pre-start event must be attempted before run
            // creation for every applicable evaluation (§C.8), including an
            // assignment-resolution failure. The override (test-only) only
            // substitutes what gets *reported*/gated on afterward, so tests
            // can prove the §C.9 failure policy without needing the real
            // write to actually fail.
            let real_evidence_persisted = match self.db.as_ref() {
                Some(db) => {
                    crate::daily_data_readiness::persist_pre_start_readiness_evidence(
                        db,
                        evaluation_id,
                        evaluated_at_utc,
                        "applicable",
                        &report,
                    )
                    .await
                }
                None => false,
            };
            let evidence_persisted = self
                .daily_data_readiness_evidence_override()
                .await
                .unwrap_or(real_evidence_persisted);

            if report.start_allowed {
                if !evidence_persisted {
                    return Err(RuntimeLifecycleError::service_unavailable(
                        "runtime.start_refused.readiness_evidence_persist_failed",
                        format!(
                            "strict daily data readiness evaluation_id={evaluation_id} was ready \
                             but durable pre-start evidence could not be persisted; refusing \
                             start ({})",
                            crate::daily_data_readiness::REASON_READINESS_EVIDENCE_PERSIST_FAILED
                        ),
                    ));
                }
                daily_data_readiness_evaluation_id = Some(evaluation_id);
            } else {
                let assignment_blockers: Vec<String> = report
                    .assignments
                    .iter()
                    .map(|a| {
                        format!(
                            "{}/{}: {:?}",
                            a.assignment_symbol, a.assignment_timeframe, a.blockers
                        )
                    })
                    .collect();
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.daily_data_readiness_blocked",
                    "daily_data_readiness",
                    format!(
                        "strict daily data readiness gate refused start: evaluation_id={evaluation_id}; \
                         start_allowed=false; top_level_blocker={:?}; \
                         assignment_resolution_error={assignment_resolution_error:?}; \
                         assignment_blockers={assignment_blockers:?}; \
                         evidence_persisted={evidence_persisted} (DAILY-DATA-READINESS-01C-ENFORCEMENT-01)",
                        report.top_level_blocker,
                    ),
                ));
            }
        }

        let db = self.db_pool()?;

        // B2A: DB strategy registry gate.
        //
        // When a native strategy is Active (plugin bootstrap passed), the strategy
        // must also be present AND enabled in the durable sys_strategy_registry.
        // This is the final activation authority: plugin presence is necessary but
        // not sufficient — registry truth is authoritative.
        //
        // Contract:
        //   Dormant bootstrap    → skip (no fleet configured; allowed).
        //   Active + enabled     → pass through.
        //   Active + disabled    → fail-closed (403, gate=strategy_registry).
        //   Active + missing     → fail-closed (403, gate=strategy_registry).
        //   Active + DB error    → fail-closed (503, gate=strategy_registry).
        //
        // Placed immediately after db_pool() so the gate runs once, before any
        // run rows are created or leadership is acquired.
        if let Some(strategy_id) = start_attempt_snapshot
            .native_strategy_bootstrap
            .active_strategy_id()
        {
            match mqk_db::fetch_strategy_registry_entry(&db, strategy_id).await {
                Ok(Some(record)) if record.enabled => {
                    // Registered and enabled — pass through.
                }
                Ok(Some(_record)) => {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.strategy_registry_disabled",
                        "strategy_registry",
                        format!(
                            "native strategy '{strategy_id}' is registered but disabled \
                             in the strategy registry; enable the strategy in \
                             sys_strategy_registry before starting",
                        ),
                    ));
                }
                Ok(None) => {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.strategy_registry_missing",
                        "strategy_registry",
                        format!(
                            "native strategy '{strategy_id}' is not registered in \
                             the strategy registry; insert an enabled row in \
                             sys_strategy_registry before starting",
                        ),
                    ));
                }
                Err(err) => {
                    return Err(RuntimeLifecycleError::internal(
                        "start strategy_registry lookup failed",
                        err,
                    ));
                }
            }
        }

        // PREMARKET-DATA-READINESS-GATE-01: Multi-symbol premarket market-data
        // readiness gate (extends DATA-FRESHNESS-READINESS-GATE-01 from a single
        // symbol/timeframe check to every symbol the current deployment requires —
        // the approved watchlist-v2 artifact's symbols when one is configured and
        // approved, otherwise the legacy single MQK_STRATEGY_SYMBOL).
        //
        // For the Paper+Alpaca path only: verify that md_bars contains sufficient
        // fresh completed bars for every required symbol/timeframe before any
        // execution run is created. Any single required symbol failing blocks
        // start for the whole run (fail-closed).
        //
        // Contract (per required symbol, same thresholds as the prior single-symbol gate):
        //   not_applicable — no symbol configured; gate not applicable; pass.
        //   unavailable    — DB query failed; cannot assert missing; pass (honest).
        //   ok             — sufficient fresh bars exist; pass.
        //   missing        — 0 completed bars; fail-closed.
        //   insufficient   — fewer than MD_FRESHNESS_MIN_BARS completed bars; fail-closed.
        //   stale          — latest bar older than MD_FRESHNESS_STALE_SECS; fail-closed.
        //
        // Placed after db_pool() (requires DB) and before insert_run / lease acquisition
        // so a readiness refusal leaves no dangling run rows.
        //
        // ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 1/8: consumes
        // `start_attempt_snapshot.required_freshness_symbols` and `.now_utc`
        // — the exact frozen vector and evaluation timestamp every other
        // gate in this attempt uses — instead of independently calling
        // `required_symbols_for_freshness_gate_from_env()` and `Utc::now()`
        // a second time. Missing/insufficient/stale semantics and gate
        // ordering are unchanged.
        if self.deployment_mode() == DeploymentMode::Paper
            && self.strategy_market_data_source()
                == StrategyMarketDataSource::ExternalSignalIngestion
        {
            let readiness = evaluate_md_freshness_status_for_symbols(
                Some(&db),
                &start_attempt_snapshot.required_freshness_symbols,
                start_attempt_snapshot.now_utc.timestamp(),
            )
            .await;
            if !readiness.start_allowed {
                // OPENING-BAR-FRESHNESS-AUTHORITY-REPAIR-01: a "stale"
                // verdict this early in the session (before the first bar's
                // interval + publication grace has elapsed) is structurally
                // guaranteed, not a genuine data problem — see
                // `readiness_blocked_only_by_pending_first_session_bar`.
                // Every other blocking case (missing/insufficient, or stale
                // outside that narrow window) is completely unchanged below.
                if readiness_blocked_only_by_pending_first_session_bar(
                    &readiness,
                    start_attempt_snapshot
                        .readiness_context
                        .calendar_provider
                        .as_ref(),
                    start_attempt_snapshot.now_utc,
                ) {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.latest_completed_bar_pending",
                        "market_data_freshness",
                        format!(
                            "latest completed bar for the current session is not \
                             yet available (aggregate_status='{}', \
                             required_symbols={:?}): {} the session has opened \
                             but the first bar's interval plus publication grace \
                             has not yet elapsed; the autonomous coordinator will \
                             retry automatically \
                             (OPENING-BAR-FRESHNESS-AUTHORITY-REPAIR-01)",
                            readiness.aggregate_status,
                            readiness.required_symbols,
                            readiness.blockers.join("; "),
                        ),
                    ));
                }
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.market_data_not_fresh",
                    "market_data_freshness",
                    format!(
                        "premarket market-data readiness gate failed \
                         (aggregate_status='{}', required_symbols={:?}): {} \
                         Run Prep-PremarketMarketData.ps1 before starting \
                         (PREMARKET-DATA-READINESS-GATE-01)",
                        readiness.aggregate_status,
                        readiness.required_symbols,
                        readiness.blockers.join("; "),
                    ),
                ));
            }
        }

        let run_id = self.create_or_reuse_run_for_start(&db).await?;

        // BUNDLE-7-PHASE-7A fault seam: after run row creation, before
        // dynamic-selection evaluation. No AppState selection state exists
        // yet — nothing to clean up beyond returning the error.
        if self
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::AfterRunRowCreation,
            )
            .await
        {
            return Err(RuntimeLifecycleError::internal(
                "dynamic_selection.fault_seam.after_run_row_creation",
                "test-injected fault after run row creation",
            ));
        }

        // DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: build the frozen
        // dynamic-selection outcome for this exact run_id now — `run_id`
        // must exist before the evaluator can be called (it validates the
        // caller-supplied context's run_id against this one). A
        // `PaperEnforcedRefused` disposition refuses the whole start here,
        // before any run advancement — the run row is left `Created`
        // (unarmed/unbegun/unspawned), matching this codebase's existing
        // fail-closed convention for other pre-advancement refusals (see
        // `RunLinkPersistFailed` below). Every other disposition (`Off`,
        // `ShadowAllowed`, `ShadowInvalid`, `PaperEnforcedAllowed`) is
        // committed to `AppState` later, inside the same atomic
        // start-commit sequence that publishes every other run-start
        // effect — never here.
        let (dynamic_selection_outcome, dispatch_authority) = self
            .build_dynamic_selection_start_snapshot(run_id, &start_attempt_snapshot)
            .await?;

        // BUNDLE-7-PHASE-7A fault seam: after selection evaluation, before
        // effects construction. No AppState selection state has been
        // committed yet — nothing to clean up beyond returning the error.
        if self
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::AfterSelectionEvaluation,
            )
            .await
        {
            return Err(RuntimeLifecycleError::internal(
                "dynamic_selection.fault_seam.after_selection_evaluation",
                "test-injected fault after selection evaluation",
            ));
        }

        // PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE Part 9: the prior
        // temporary `paper_enforced` interlock
        // (`paper_enforced_dispatch_not_wired_refusal`) is removed — the
        // positive dispatch-authority guard now lives inside
        // `build_dynamic_selection_start_snapshot` itself
        // (`build_dynamic_paper_enforced_dispatch_authority` fails the whole
        // start closed on any coherence defect before this point is ever
        // reached). `PaperEnforcedAllowed` may now proceed through arm/
        // begin/barrier/Active carrying `dispatch_authority ==
        // DynamicPaperEnforced`; `PaperEnforcedRefused` still blocks inside
        // `build_dynamic_selection_start_snapshot` above; `Off`/`Shadow`
        // always carry `dispatch_authority == Legacy`.

        // DAILY-DATA-READINESS-01C-ENFORCEMENT-01 / REPAIR 3-4
        // (DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01): advance the
        // created/reused run to an active execution loop. `run_id` now
        // exists — link it to the pre-start evaluation_id captured by the
        // strict readiness gate (fail-closed on a link-persist failure: no
        // arm/begin/tick/spawn — REPAIR 4 replaces the prior non-fatal
        // sticky-degraded policy for this specific evidence boundary), then
        // perform runtime construction/safety-setup and spawn the execution
        // loop, in this exact order. Non-applicable deployments have no
        // evaluation_id to link and proceed straight to runtime effects,
        // exactly as before this patch.
        //
        // The production start path and the synthetic lifecycle proof test
        // (`scenario_daily_data_readiness_start_gate_01.rs`) both drive this
        // exact sequencing function against `RuntimeStartEffects` — never
        // two independently-ordered implementations.
        let readiness_link = daily_data_readiness_evaluation_id.map(|id| (id, Utc::now()));
        let effects = ProductionRuntimeStartEffects {
            state: self,
            db: db.clone(),
            artifact_intake: std::sync::Mutex::new(Some(artifact_intake)),
            // ATOMICITY-SINGLE-SNAPSHOT-REPAIR: the frozen legacy assignment
            // vector from the one start-attempt snapshot resolved above —
            // `spawn_loop` passes this to `spawn_execution_loop` instead of
            // letting it re-read env/watchlist state a third time. Read
            // before the partial move of `native_strategy_bootstrap` below
            // (both are fields of `start_attempt_snapshot`, and this method
            // call needs the whole struct still intact).
            legacy_assignments: start_attempt_snapshot.legacy_assignments(),
            // Moves `native_strategy_bootstrap` out of `start_attempt_
            // snapshot` — the last use of that field; every other field
            // consumed above was read by reference only.
            native_strategy_bootstrap: std::sync::Mutex::new(Some(
                start_attempt_snapshot.native_strategy_bootstrap,
            )),
            orchestrator: std::sync::Mutex::new(None),
            dynamic_selection_outcome: std::sync::Mutex::new(Some(dynamic_selection_outcome)),
            dispatch_authority: std::sync::Mutex::new(Some(dispatch_authority)),
            start_phase: std::sync::Mutex::new(
                crate::daily_data_readiness::RuntimeStartPhase::BeforeArm,
            ),
            leadership_release_outcome: std::sync::Mutex::new(None),
            task_side_leadership_release_outcome: std::sync::Mutex::new(None),
            task_side_join_outcome: std::sync::Mutex::new(None),
        };
        let mut lifecycle_trace: Vec<&'static str> = Vec::new();
        crate::daily_data_readiness::advance_run_to_active(
            &db,
            &effects,
            run_id,
            readiness_link,
            &mut lifecycle_trace,
        )
        .await
        .map_err(|err| match err {
            crate::daily_data_readiness::RuntimeStartSequenceError::RunLinkPersistFailed {
                evaluation_id,
                run_id,
                rollback,
            } => RuntimeLifecycleError::service_unavailable(
                "runtime.start_refused.readiness_run_link_persist_failed",
                format!(
                    "strict daily data readiness run-linked evidence persist failed for \
                     evaluation_id={evaluation_id} run_id={run_id}; refusing to arm, begin, \
                     tick, or spawn the execution loop — the run row exists but must not be \
                     presented as an actively started runtime ({}); local ownership \
                     reservation rollback: phase_reached={:?} durable={:?} \
                     durable_status_unknown={}",
                    crate::daily_data_readiness::REASON_READINESS_RUN_LINK_PERSIST_FAILED,
                    rollback.phase_reached,
                    rollback.durable,
                    rollback.durable_status_unknown,
                ),
            ),
            crate::daily_data_readiness::RuntimeStartSequenceError::Effects {
                original,
                rollback,
            } => {
                let base = match original.kind {
                    crate::daily_data_readiness::RuntimeStartEffectsErrorKind::Internal => {
                        RuntimeLifecycleError::internal(original.fault_class, original.message)
                    }
                    crate::daily_data_readiness::RuntimeStartEffectsErrorKind::Conflict => {
                        RuntimeLifecycleError::conflict(original.fault_class, original.message)
                    }
                };
                // ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 5: never
                // mask the original fault, but never present an ordinary
                // start error when rollback itself could not confirm a safe
                // terminal state — the DB may still say Armed/Running, or
                // the orchestrator lease release may have failed.
                if rollback.is_degraded() {
                    RuntimeLifecycleError::service_unavailable(
                        "runtime.start_refused.rollback_degraded",
                        format!(
                            "start attempt failed and rollback could not confirm a safe \
                             terminal state; original fault ({}): {base}; \
                             rollback: phase_reached={:?} durable={:?} \
                             durable_status_unknown={} leadership_release_outcome={:?}; \
                             operator must verify the durable run status directly \
                             before retrying ({})",
                            base.fault_class(),
                            rollback.phase_reached,
                            rollback.durable,
                            rollback.durable_status_unknown,
                            rollback.local.leadership_release_outcome,
                            "ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01",
                        ),
                    )
                } else {
                    base
                }
            }
        })?;

        {
            let snap_arc = Arc::clone(&self.execution_snapshot);
            // Separate clone for the settle closure — snap_arc is moved into local_fn.
            let snap_arc_settle = Arc::clone(&self.execution_snapshot);
            let sides_arc = Arc::clone(&self.local_order_sides);
            let broker_arc = Arc::clone(&self.broker_snapshot);
            // BROKER-POSITION-BASELINE-ADOPTION-01: when no execution run is active,
            // use the operator-adopted baseline (if any) as local truth so the
            // reconcile tick sees local == broker and publishes clean state.
            let baseline_arc = Arc::clone(&self.broker_baseline);
            let local_fn = move || {
                let snapshot = snap_arc.try_read().ok().and_then(|g| g.clone());
                if let Some(snapshot) = snapshot {
                    let sides = sides_arc.try_read().map(|g| g.clone()).unwrap_or_default();
                    // RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01: execution_snapshot
                    // .portfolio.positions already carries the adopted broker
                    // baseline (seeded once at run start by
                    // seed_portfolio_from_baseline — see the comment above the
                    // seeding call in build_execution_orchestrator) plus any
                    // same-run fill delta. Re-merging baseline_arc here would
                    // double-count it: local = fills + 2x baseline, while broker
                    // truth = fills + baseline, producing a false ReconcileDrift
                    // halt/disarm. Derive local truth directly from the seeded
                    // snapshot — no merge.
                    reconcile_local_snapshot_from_runtime_with_sides(&snapshot, &sides)
                } else {
                    // No active run: use adopted baseline if present, else empty.
                    baseline_arc
                        .try_read()
                        .ok()
                        .and_then(|g| g.clone())
                        .unwrap_or_else(mqk_reconcile::LocalSnapshot::empty)
                }
            };
            let broker_fn = move || {
                let schema = broker_arc.try_read().ok().and_then(|g| g.clone())?;
                reconcile_broker_snapshot_from_schema(&schema).ok()
            };
            // RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01: defer background reconcile
            // disarm when the execution snapshot signals a recent terminal fill.
            let settle_fn = move || {
                snap_arc_settle
                    .try_read()
                    .ok()
                    .and_then(|g| g.as_ref().map(|s| s.has_recent_terminal_fill))
                    .unwrap_or(false)
            };
            spawn_reconcile_tick(
                Arc::clone(self),
                local_fn,
                broker_fn,
                settle_fn,
                RECONCILE_TICK_INTERVAL,
            );
        }

        let snapshot = StatusSnapshot {
            daemon_uptime_secs: uptime_secs(),
            active_run_id: Some(run_id),
            state: "running".to_string(),
            notes: Some("daemon owns active execution loop".to_string()),
            integrity_armed: self.integrity_armed().await,
            deadman_status: "healthy".to_string(),
            deadman_last_heartbeat_utc: Some(Utc::now().to_rfc3339()),
        };
        self.publish_status(snapshot.clone()).await;
        Ok(snapshot)
    }

    /// Look up any existing durable run for this engine/mode and either
    /// reuse a `Created` run or create a fresh one.
    ///
    /// REPAIR 3 (DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01): extracted from
    /// `start_execution_runtime`'s previously-inline lookup/creation logic
    /// (unchanged) so the production start path and the synthetic lifecycle
    /// proof test both create a run through the exact same code — never a
    /// separate test-only reimplementation of this identity resolution.
    pub async fn create_or_reuse_run_for_start(
        self: &Arc<Self>,
        db: &PgPool,
    ) -> Result<uuid::Uuid, RuntimeLifecycleError> {
        if let Some(active) = mqk_db::fetch_active_run_for_engine(
            db,
            DAEMON_ENGINE_ID,
            self.deployment_mode().as_db_mode(),
        )
        .await
        .map_err(|err| RuntimeLifecycleError::internal("start active-run lookup failed", err))?
        {
            return Err(RuntimeLifecycleError::conflict(
                "runtime.truth_mismatch.durable_active_without_local_owner",
                format!(
                    "durable active run exists without local ownership: {}",
                    active.run_id
                ),
            ));
        }

        let latest = mqk_db::fetch_latest_run_for_engine(
            db,
            DAEMON_ENGINE_ID,
            self.deployment_mode().as_db_mode(),
        )
        .await
        .map_err(|err| RuntimeLifecycleError::internal("start latest-run lookup failed", err))?;

        match latest.as_ref() {
            Some(run) => match run.status {
                mqk_db::RunStatus::Created => Ok(run.run_id),
                mqk_db::RunStatus::Stopped => {
                    let run_id = self.next_daemon_run_id(db).await?;
                    mqk_db::insert_run(
                        db,
                        &mqk_db::NewRun {
                            run_id,
                            engine_id: DAEMON_ENGINE_ID.to_string(),
                            mode: self.deployment_mode().as_db_mode().to_string(),
                            started_at_utc: Utc::now(),
                            git_hash: "UNKNOWN".to_string(),
                            config_hash: self.run_config_hash().to_string(),
                            config_json: serde_json::json!({
                                "runtime": "mqk-daemon",
                                "adapter": self.adapter_id(),
                                "mode": self.deployment_mode().as_db_mode(),
                            }),
                            host_fingerprint: self.node_id.clone(),
                        },
                    )
                    .await
                    .map_err(|err| RuntimeLifecycleError::internal("start insert_run failed", err))?;
                    Ok(run_id)
                }
                mqk_db::RunStatus::Halted => Err(RuntimeLifecycleError::conflict(
                    "runtime.start_refused.halted_lifecycle",
                    format!(
                        "durable run {} is halted; operator must clear the halted lifecycle before starting again",
                        run.run_id
                    ),
                )),
                mqk_db::RunStatus::Armed | mqk_db::RunStatus::Running => {
                    Err(RuntimeLifecycleError::conflict(
                        "runtime.start_refused.durable_run_active",
                        format!("durable run {} is already active", run.run_id),
                    ))
                }
            },
            None => {
                let run_id = self.next_daemon_run_id(db).await?;
                mqk_db::insert_run(
                    db,
                    &mqk_db::NewRun {
                        run_id,
                        engine_id: DAEMON_ENGINE_ID.to_string(),
                        mode: self.deployment_mode().as_db_mode().to_string(),
                        started_at_utc: Utc::now(),
                        git_hash: "UNKNOWN".to_string(),
                        config_hash: self.run_config_hash().to_string(),
                        config_json: serde_json::json!({
                            "runtime": "mqk-daemon",
                            "adapter": self.adapter_id(),
                            "mode": self.deployment_mode().as_db_mode(),
                        }),
                        host_fingerprint: self.node_id.clone(),
                    },
                )
                .await
                .map_err(|err| RuntimeLifecycleError::internal("start insert_run failed", err))?;
                Ok(run_id)
            }
        }
    }

    pub async fn stop_execution_runtime(
        self: &Arc<Self>,
    ) -> Result<StatusSnapshot, RuntimeLifecycleError> {
        let _op = self.lifecycle_op.lock().await;
        self.reap_finished_execution_loop().await?;

        // PAPER-SOAK-INBOUND-DRAIN-OWNERSHIP-01: before tearing down local
        // ownership, prove every broker-reachable order for this run has
        // resolved to a terminal broker outcome. A normal stop must not
        // silently discard ownership of unresolved orders -- the WS inbound
        // transport (alpaca_ws_transport.rs) gates durable ingest on "is a
        // run currently locally owned", so the instant ownership clears, any
        // late fill/partial_fill/cancel_ack/reject frame for this run would
        // be dropped with zero logging and zero durable trace.
        //
        // Read-only peek (does not touch ownership) so a run with no
        // unresolved orders falls straight through to the existing teardown
        // below, byte-for-byte unchanged from before this patch. Skipped
        // entirely when no DB is configured, matching this function's
        // pre-existing no-DB behavior (the teardown below already requires
        // DB to complete successfully). Scoped to `status == RUNNING` only --
        // a run that is already HALTED goes through the halt/clear-halt path
        // instead, which this patch deliberately leaves untouched (halt is a
        // sticky, fail-closed emergency stop; retrofitting drain-gating onto
        // halt recovery is out of this patch's scope).
        if let Some(run_id) = self.locally_owned_run_id().await {
            if let Some(db) = self.db.as_ref() {
                let run = mqk_db::fetch_run(db, run_id)
                    .await
                    .map_err(|err| RuntimeLifecycleError::internal("stop fetch_run failed", err))?;

                if matches!(run.status, mqk_db::RunStatus::Running) {
                    // Idempotent: safe to call on every stop attempt while
                    // drainage is still in progress -- does not move `status`
                    // off RUNNING and does not overwrite an earlier request time.
                    mqk_db::request_stop_run(db, run_id).await.map_err(|err| {
                        RuntimeLifecycleError::internal("stop request_stop_run failed", err)
                    })?;

                    let unresolved = mqk_db::outbox_unresolved_broker_reachable_orders(db, run_id)
                        .await
                        .map_err(|err| {
                            RuntimeLifecycleError::internal("stop drain-check failed", err)
                        })?;

                    if !unresolved.is_empty() {
                        return Err(RuntimeLifecycleError::conflict(
                            "runtime.stop.orders_draining",
                            format!(
                                "run {run_id} has {} broker-reachable order(s) still unresolved; \
                                 new order admission is suppressed and the run continues to drain \
                                 automatically as broker events arrive -- retry stop once drained: {}",
                                unresolved.len(),
                                unresolved.join(", "),
                            ),
                        ));
                    }
                }
            }
        }

        // BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement
        // 4: the single unified cleanup authority — clears ownership
        // (including dynamic-selection truth, via its metadata) and every
        // economic mirror together, before any DB call below that could
        // fail, and before either the truth-mismatch conflict return or
        // the no-local-owner idle return.
        let Some((run_id, outcome)) = self
            .clear_currently_owned_local_runtime(crate::state::LifecycleClearReason::OperatorStop)
            .await
        else {
            if let Some(db) = self.db.as_ref() {
                if let Some(active) = mqk_db::fetch_active_run_for_engine(
                    db,
                    DAEMON_ENGINE_ID,
                    self.deployment_mode().as_db_mode(),
                )
                .await
                .map_err(|err| {
                    RuntimeLifecycleError::internal("stop active-run lookup failed", err)
                })? {
                    return Err(RuntimeLifecycleError::conflict(
                        "runtime.truth_mismatch.durable_active_without_local_owner",
                        format!(
                            "durable active run exists without local ownership: {}",
                            active.run_id
                        ),
                    ));
                }
            }
            return self.current_status_snapshot().await;
        };

        // PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 4: a join
        // failure (task panic) or a reported leadership-release failure
        // both mean this run's true final state is unconfirmed — ownership
        // was already unconditionally moved to `Idle` by the clear above,
        // so without this the operator would see a clean-looking `Idle`
        // instead of the truth. Mark `Degraded` before returning, exactly
        // like the pre-existing `stop_run`-failure branch below already
        // does for a durable-transition failure.
        if outcome.is_degraded() {
            let detail = match (&outcome.join_error, &outcome.leadership_release_outcome) {
                (Some(join_err), _) => format!("execution loop join failed: {join_err}"),
                (None, Some(Err(release_err))) => {
                    format!("runtime leadership release failed: {release_err}")
                }
                _ => "unknown degraded cleanup".to_string(),
            };
            self.note_local_runtime_degraded(
                run_id,
                crate::state::BoundedLifecycleDegradation {
                    operation: "stop_join_or_release",
                    detail: detail.clone(),
                },
            )
            .await;
            return Err(RuntimeLifecycleError::internal(
                "stop join or release failed",
                detail,
            ));
        }

        let db = self.db_pool()?;
        let run = mqk_db::fetch_run(&db, run_id)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("stop fetch_run failed", err))?;
        if matches!(
            run.status,
            mqk_db::RunStatus::Armed | mqk_db::RunStatus::Running
        ) {
            if let Err(err) = mqk_db::stop_run(&db, run_id).await {
                // Requirement 4: local authority is already fully removed
                // (handle stopped+joined, mirrors cleared) above — mark
                // truth honestly `Degraded` rather than silently leaving a
                // clean-looking `Idle` behind a failed durable transition.
                self.note_local_runtime_degraded(
                    run_id,
                    crate::state::BoundedLifecycleDegradation {
                        operation: "stop_run",
                        detail: err.to_string(),
                    },
                )
                .await;
                return Err(RuntimeLifecycleError::internal("stop_run failed", err));
            }
        }

        let snapshot = self.current_status_snapshot().await?;
        Ok(snapshot)
    }

    pub async fn halt_execution_runtime(
        self: &Arc<Self>,
    ) -> Result<StatusSnapshot, RuntimeLifecycleError> {
        let _op = self.lifecycle_op.lock().await;
        self.reap_finished_execution_loop().await?;

        // BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement
        // 4: the single unified cleanup authority — clears ownership
        // (including dynamic-selection truth) and every economic mirror
        // together, before any DB call below that could fail, and before
        // either the truth-mismatch conflict return or the no-local-owner
        // path.
        let cleared = self
            .clear_currently_owned_local_runtime(crate::state::LifecycleClearReason::OperatorHalt)
            .await;

        if cleared.is_none() {
            if let Some(db) = self.db.as_ref() {
                if let Some(active) = mqk_db::fetch_active_run_for_engine(
                    db,
                    DAEMON_ENGINE_ID,
                    self.deployment_mode().as_db_mode(),
                )
                .await
                .map_err(|err| {
                    RuntimeLifecycleError::internal("halt active-run lookup failed", err)
                })? {
                    return Err(RuntimeLifecycleError::conflict(
                        "runtime.truth_mismatch.durable_active_without_local_owner",
                        format!(
                            "durable active run exists without local ownership: {}",
                            active.run_id
                        ),
                    ));
                }
            }
        }

        {
            let mut integrity = self.integrity.write().await;
            integrity.disarmed = true;
            integrity.halted = true;
        }

        let db = self.db_pool()?;
        if let Some((run_id, outcome)) = cleared {
            // PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 4: same
            // degraded-truth requirement as `stop_execution_runtime` — a
            // join failure or reported release failure must not be
            // followed by a silent `Idle`.
            if outcome.is_degraded() {
                let detail = match (&outcome.join_error, &outcome.leadership_release_outcome) {
                    (Some(join_err), _) => format!("execution loop join failed: {join_err}"),
                    (None, Some(Err(release_err))) => {
                        format!("runtime leadership release failed: {release_err}")
                    }
                    _ => "unknown degraded cleanup".to_string(),
                };
                self.note_local_runtime_degraded(
                    run_id,
                    crate::state::BoundedLifecycleDegradation {
                        operation: "halt_join_or_release",
                        detail: detail.clone(),
                    },
                )
                .await;
                return Err(RuntimeLifecycleError::internal(
                    "halt join or release failed",
                    detail,
                ));
            }
            if let Err(err) = mqk_db::halt_run(&db, run_id, Utc::now()).await {
                // Requirement 4: local authority is already fully removed
                // above — mark truth honestly `Degraded` rather than
                // silently leaving a clean-looking `Idle` behind a failed
                // durable transition.
                self.note_local_runtime_degraded(
                    run_id,
                    crate::state::BoundedLifecycleDegradation {
                        operation: "halt_run",
                        detail: err.to_string(),
                    },
                )
                .await;
                return Err(RuntimeLifecycleError::internal("halt_run failed", err));
            }
        }
        mqk_db::persist_arm_state_canonical(
            &db,
            mqk_db::ArmState::Disarmed,
            Some(mqk_db::DisarmReason::OperatorHalt),
        )
        .await
        .map_err(|err| RuntimeLifecycleError::internal("persist_arm_state failed", err))?;

        let snapshot = StatusSnapshot {
            daemon_uptime_secs: uptime_secs(),
            active_run_id: self.current_status_snapshot().await?.active_run_id,
            state: "halted".to_string(),
            notes: Some("operator halt asserted; execution loop disarmed".to_string()),
            integrity_armed: false,
            deadman_status: "expired".to_string(),
            deadman_last_heartbeat_utc: None,
        };
        self.publish_status(snapshot.clone()).await;
        Ok(snapshot)
    }

    /// AUTON-PAPER-01B: Pre-session autonomous arm seam.
    ///
    /// Attempts to advance in-memory integrity state from disarmed to armed by
    /// reading the persisted arm state from the DB.  Called by the autonomous
    /// session controller immediately before `start_execution_runtime` so the
    /// daily session can start without a manual operator arm.
    ///
    /// # Gate rules (fail-closed ordering)
    ///
    /// 1. `integrity.halted == true` → refuse unconditionally (operator halt
    ///    wins; not reversible by the controller).
    /// 2. `integrity.disarmed == false` → already armed; return `Ok(())`.
    /// 3. No DB configured → refuse (cannot verify prior session state).
    /// 4. No DB row → refuse (first-time install; operator must arm once).
    /// 5. DB state = `"ARMED"` → auto-arm: set `disarmed=false, halted=false`,
    ///    re-persist `Armed`, return `Ok(())`.
    /// 6. DB state = anything else (`"DISARMED"`) → refuse with stored reason.
    ///
    /// # Daily-cycle property
    ///
    /// `stop_execution_runtime` does NOT write `Disarmed` to the DB, so after a
    /// clean daily stop the DB remains `ARMED`.  On the next daemon boot the
    /// in-memory integrity state starts as `disarmed=true` (fail-closed), but
    /// the DB row carries the prior `ARMED` state → auto-arm succeeds → the
    /// session controller can start the next day without operator intervention.
    ///
    /// Only `halt_execution_runtime` writes `DISARMED` to the DB.  A halted
    /// daemon therefore requires manual operator arm before the controller can
    /// restart, which is the correct safety posture.
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2: typed autonomous arm seam.
    ///
    /// Same gate order/rules as [`Self::try_autonomous_arm`]'s doc comment
    /// above, but returns a closed [`AutonomousArmOutcome`]/
    /// [`AutonomousArmRejection`] pair instead of a free-form
    /// `Result<(), String>`, so the durable daily coordinator (D2) can
    /// classify the result exclusively by typed variant -- never by parsing
    /// rendered text. [`Self::try_autonomous_arm`] becomes a thin
    /// compatibility wrapper over this method.
    pub async fn try_autonomous_arm_typed(
        &self,
    ) -> Result<AutonomousArmOutcome, AutonomousArmRejection> {
        // Gate 1: operator halt wins unconditionally.
        // Gate 2: already armed is idempotent success.
        {
            let ig = self.integrity.read().await;
            if ig.halted {
                return Err(AutonomousArmRejection::IntegrityHalted);
            }
            if !ig.disarmed {
                return Ok(AutonomousArmOutcome::AlreadyArmed);
            }
        }

        // Gate 3: DB required to verify prior session state.
        let db = match self.db.as_ref() {
            Some(db) => db,
            None => return Err(AutonomousArmRejection::DatabaseNotConfigured),
        };

        // Gate 4/5/6: load prior arm state from the singleton row.
        let row = mqk_db::load_arm_state(db).await.map_err(|_err| {
            AutonomousArmRejection::TemporaryDatabaseOperationFailure {
                operation: "load_arm_state",
            }
        })?;

        match row {
            None => Err(AutonomousArmRejection::NoPersistedArmState),
            Some((ref state_str, _)) if state_str == "ARMED" => {
                // Prior session ended cleanly (stop does not write DISARMED).
                // Advance in-memory integrity to armed.
                {
                    let mut ig = self.integrity.write().await;
                    ig.disarmed = false;
                    ig.halted = false;
                }
                // Re-persist Armed so another daemon restart also sees ARMED.
                mqk_db::persist_arm_state_canonical(db, mqk_db::ArmState::Armed, None)
                    .await
                    .map_err(
                        |_err| AutonomousArmRejection::TemporaryDatabaseOperationFailure {
                            operation: "persist_arm_state_canonical",
                        },
                    )?;
                Ok(AutonomousArmOutcome::ArmedFromPersistedState)
            }
            Some((_, reason)) => Err(AutonomousArmRejection::DurableDisarmed { reason }),
        }
    }

    pub async fn try_autonomous_arm(&self) -> Result<(), String> {
        self.try_autonomous_arm_typed()
            .await
            .map(|_| ())
            .map_err(|rejection| rejection.legacy_message())
    }

    pub async fn stop_for_shutdown(self: &Arc<Self>) {
        // BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement
        // 4: acquire the same lifecycle serialization every other local
        // start/stop/halt transition uses. Because every start attempt
        // holds this lock for its *entire* duration (including the whole
        // reserve -> prepare-metadata -> barriered-install sequence),
        // acquiring it here means ownership can never actually be observed
        // as `Reserved`/`Starting` by the time this function's own body
        // runs — shutdown either runs before a start attempt begins, or
        // waits for one already in flight to reach its final `Idle`/
        // `Active`/`Degraded` state first. This is what "shutdown safely
        // cancels Reserved/Starting" means in practice here: safety by
        // construction via serialization, not an explicit cancellation
        // race.
        let _op = self.lifecycle_op.lock().await;

        // Clears ownership (including dynamic-selection truth) AND every
        // economic mirror together — closing the pre-existing asymmetry
        // where shutdown cleared dynamic-selection state but left
        // `accepted_artifact`/`native_strategy_bootstrap`/other mirrors
        // stale.
        let cleared = self
            .clear_currently_owned_local_runtime(crate::state::LifecycleClearReason::Shutdown)
            .await;

        let Some((run_id, outcome)) = cleared else {
            return;
        };
        // PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 4: same
        // degraded-truth requirement as stop/halt — shutdown has no
        // caller to return an `Err` to, but it must still record `Degraded`
        // rather than leave a silent `Idle` behind an unconfirmed cleanup.
        if outcome.is_degraded() {
            let detail = match (&outcome.join_error, &outcome.leadership_release_outcome) {
                (Some(join_err), _) => format!("execution loop join failed: {join_err}"),
                (None, Some(Err(release_err))) => {
                    format!("runtime leadership release failed: {release_err}")
                }
                _ => "unknown degraded cleanup".to_string(),
            };
            tracing::warn!("shutdown join_or_release failed for {run_id}: {detail}");
            self.note_local_runtime_degraded(
                run_id,
                crate::state::BoundedLifecycleDegradation {
                    operation: "shutdown_join_or_release",
                    detail,
                },
            )
            .await;
            return;
        }
        let Some(db) = self.db.as_ref() else {
            return;
        };
        match mqk_db::fetch_run(db, run_id).await {
            Ok(run) => {
                if matches!(
                    run.status,
                    mqk_db::RunStatus::Armed | mqk_db::RunStatus::Running
                ) {
                    if let Err(err) = mqk_db::stop_run(db, run_id).await {
                        tracing::warn!("shutdown stop_run failed for {run_id}: {err}");
                        self.note_local_runtime_degraded(
                            run_id,
                            crate::state::BoundedLifecycleDegradation {
                                operation: "stop_run",
                                detail: err.to_string(),
                            },
                        )
                        .await;
                    }
                }
            }
            Err(err) => {
                tracing::warn!("shutdown fetch_run_failed for {run_id}: {err}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// REPAIR 3 (DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01): production
// implementation of `daily_data_readiness::RuntimeStartEffects`.
//
// Wraps the exact runtime-construction/arm/begin/tick/heartbeat/snapshot/
// counter-reset/provenance-capture/bootstrap-storage/spawn sequence
// `start_execution_runtime` previously performed inline, unchanged in
// substance — only relocated so the identical trait the synthetic lifecycle
// proof test implements can be driven by `daily_data_readiness::
// advance_run_to_active` in production too.
// ---------------------------------------------------------------------------

struct ProductionRuntimeStartEffects<'a> {
    state: &'a Arc<AppState>,
    db: PgPool,
    /// Consumed exactly once by `start_runtime_effects` (TV-01C provenance
    /// capture) — `std::sync::Mutex` for interior mutability behind `&self`;
    /// never held across an `.await`.
    artifact_intake: std::sync::Mutex<Option<ArtifactIntakeOutcome>>,
    /// Consumed exactly once by `start_runtime_effects` (B1A bootstrap
    /// storage) — held locally until the run-start bundle commit.
    native_strategy_bootstrap: std::sync::Mutex<Option<NativeStrategyBootstrap>>,
    /// Populated by `start_runtime_effects`, consumed by `spawn_loop`.
    orchestrator: std::sync::Mutex<Option<DaemonOrchestrator>>,
    /// BUNDLE-7-PHASE-7A: the already-evaluated, frozen dynamic-selection
    /// outcome for this run, consumed exactly once by `start_runtime_effects`
    /// (committed to `AppState` as part of the same run-start bundle as
    /// `native_strategy_bootstrap`).
    dynamic_selection_outcome: std::sync::Mutex<Option<DynamicSelectionRuntimeState>>,
    /// ATOMICITY-SINGLE-SNAPSHOT-REPAIR: the frozen legacy assignment
    /// vector from the one `StartAttemptAuthoritySnapshot` resolved by
    /// `start_execution_runtime` — handed to `spawn_execution_loop` by
    /// `spawn_loop` below instead of letting the loop re-read env/watchlist
    /// state a third time.
    legacy_assignments: Vec<crate::state::SymbolStrategyAssignment>,
    /// PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE Part 1: the one
    /// frozen run-scoped dispatch authority built by
    /// `build_dynamic_selection_start_snapshot` — `Legacy` for `Off`/
    /// `Shadow`, `DynamicPaperEnforced` only for `PaperEnforcedAllowed`.
    /// Consumed exactly once by `spawn_loop`, which moves it wholesale into
    /// `spawn_execution_loop` — never cloned, never rebuilt.
    dispatch_authority: std::sync::Mutex<
        Option<crate::dynamic_selection_dispatch_authority::RuntimeStrategyDispatchAuthority>,
    >,
    /// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 4: how far this
    /// attempt has progressed. Updated synchronously (no `.await` held)
    /// at every milestone; read back by `start_phase_reached`.
    start_phase: std::sync::Mutex<crate::daily_data_readiness::RuntimeStartPhase>,
    /// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 5: the outcome of
    /// releasing the orchestrator's runtime leadership lease for this
    /// attempt, if it was ever acquired — recorded at the exact call site
    /// that released it (`release_orchestrator_leadership`), read back by
    /// `rollback_local_effects` into the structured `LocalRollbackOutcome`
    /// instead of only a `tracing::warn!` line.
    leadership_release_outcome: std::sync::Mutex<Option<Result<(), String>>>,
    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 1: populated by
    /// `spawn_loop` only when `install_active_runtime` fails *after* the
    /// orchestrator was already handed off to the spawned task — the
    /// task-side pre-barrier exit branch's own
    /// `ExecutionLoopExit::leadership_release_outcome`, folded into
    /// `rollback_local_effects`'s returned `LocalRollbackOutcome` instead of
    /// being lost once `install_active_runtime` discarded it.
    task_side_leadership_release_outcome: std::sync::Mutex<Option<Result<(), String>>>,
    /// Same provenance as above: `Some(Err(_))` when joining the spawned
    /// task itself failed (a panic), never discarded.
    task_side_join_outcome: std::sync::Mutex<Option<Result<(), String>>>,
}

impl ProductionRuntimeStartEffects<'_> {
    /// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 7: recovers from
    /// a poisoned mutex (another thread panicked while holding it) instead
    /// of propagating the poison as a second panic here — this is pure,
    /// process-local progress-tracking state with no invariant that a
    /// poisoned write could violate, so taking the possibly-stale inner
    /// value is strictly safer than panicking a second time.
    fn set_phase(&self, phase: crate::daily_data_readiness::RuntimeStartPhase) {
        *self
            .start_phase
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = phase;
    }

    /// Release `orchestrator`'s runtime leadership lease and record the
    /// outcome for `rollback_local_effects` to surface, instead of only
    /// logging it. `context` is a short label for the `tracing::warn!` line
    /// on failure (matches the pre-existing per-call-site message suffixes).
    async fn release_orchestrator_leadership(
        &self,
        mut orchestrator: DaemonOrchestrator,
        context: &'static str,
    ) {
        // PHASE-7A-FINAL-PRIVATE-PRODUCTION-EFFECTS-PROOF requirement 6:
        // hermetic injection point for a forced leadership-release
        // failure. Always `false` in production and for every test that
        // does not explicitly enable it; the real release call and its
        // real error path are otherwise unchanged.
        let outcome = if self.state.leadership_release_failure_forced() {
            Err("test-injected leadership release failure".to_string())
        } else {
            orchestrator
                .release_runtime_leadership()
                .await
                .map_err(|err| err.to_string())
        };
        drop(orchestrator);
        if let Err(ref message) = outcome {
            tracing::warn!("runtime_lease_release_failed_on_{context} error={message}");
        }
        *self
            .leadership_release_outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(outcome);
    }
}

#[async_trait::async_trait]
impl crate::daily_data_readiness::RuntimeStartEffects for ProductionRuntimeStartEffects<'_> {
    /// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 2: the very first
    /// `RuntimeStartEffects` call `advance_run_to_active` makes for this
    /// attempt — strictly before any runtime construction or `AppState`
    /// publication. Relies on `AppState::lifecycle_op` (held for the whole
    /// duration of `start_execution_runtime`, including this call and the
    /// later `start_runtime_effects`/`spawn_loop`/rollback) to serialize
    /// every local start/stop/halt attempt within one process — no other
    /// writer can install a handle into `execution_loop` between this
    /// reservation and `spawn_loop`'s later install. A real ownership
    /// conflict here is refused before any DB write or runtime construction
    /// for this attempt happens at all.
    async fn reserve_local_ownership(
        &self,
        run_id: uuid::Uuid,
    ) -> Result<(), crate::daily_data_readiness::RuntimeStartEffectsError> {
        use crate::daily_data_readiness::RuntimeStartEffectsError;

        self.state
            .reserve_runtime_ownership(run_id)
            .await
            .map_err(|_conflicting_run_id| {
                RuntimeStartEffectsError::conflict(
                    "runtime.start_refused.local_ownership_conflict",
                    "runtime ownership changed while starting; refusing duplicate loop",
                )
            })
    }

    /// Only called after `reserve_local_ownership` has returned `Ok`.
    /// Constructs/arms/begins/ticks the runtime using purely local bindings,
    /// then commits the prepared [`RunStartLocalBundle`] to `AppState` in
    /// one call (`commit_run_start_bundle`) — no field is published
    /// individually as it becomes available.
    async fn start_runtime_effects(
        &self,
        run_id: uuid::Uuid,
    ) -> Result<(), crate::daily_data_readiness::RuntimeStartEffectsError> {
        use crate::daily_data_readiness::{RuntimeStartEffectsError, RuntimeStartPhase};

        let mut orchestrator = self
            .state
            .build_execution_orchestrator(self.db.clone(), run_id)
            .await
            .map_err(|err| {
                let fault_class = err.fault_class();
                let message = err.to_string();
                match err {
                    RuntimeLifecycleError::Conflict { .. } => {
                        RuntimeStartEffectsError::conflict(fault_class, message)
                    }
                    _ => RuntimeStartEffectsError::internal(fault_class, message),
                }
            })?;

        // BUNDLE-7-PHASE-7A fault seam: after orchestrator construction,
        // before arm/begin/tick. No dynamic-selection state has been
        // committed yet — release the freshly-acquired lease and return;
        // nothing else to clean up. Phase remains `BeforeArm`.
        if self
            .state
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::AfterOrchestratorConstruction,
            )
            .await
        {
            self.release_orchestrator_leadership(orchestrator, "dynamic_selection_fault_seam")
                .await;
            return Err(RuntimeStartEffectsError::internal(
                "dynamic_selection.fault_seam.after_orchestrator_construction",
                "test-injected fault after orchestrator construction",
            ));
        }

        // PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 3 row 27
        // ("durable rollback query failure"): real perturbation, always
        // `false` in production and for every test that does not
        // explicitly enable it. Deletes the run row for real — the
        // rollback path's own `fetch_run` call then organically fails
        // (`RowNotFound`) because the row genuinely no longer exists,
        // rather than a fabricated query error.
        if self
            .state
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::DeleteRunRowBeforeArm,
            )
            .await
        {
            self.release_orchestrator_leadership(orchestrator, "dynamic_selection_fault_seam")
                .await;
            let _ = sqlx::query("delete from runs where run_id = $1")
                .bind(run_id)
                .execute(&self.db)
                .await;
            return Err(RuntimeStartEffectsError::internal(
                "dynamic_selection.fault_seam.delete_run_row_before_arm",
                "test-injected run-row deletion before arm",
            ));
        }

        if let Err(err) = mqk_db::arm_run(&self.db, run_id).await {
            self.release_orchestrator_leadership(orchestrator, "arm_rollback")
                .await;
            return Err(RuntimeStartEffectsError::internal(
                "start arm_run failed",
                err.to_string(),
            ));
        }
        self.set_phase(RuntimeStartPhase::ArmedBeforeBegin);

        // PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 3 row 11:
        // real perturbation, always `false` in production and for every
        // test that does not explicitly enable it. Uses the same real
        // `mqk_db::stop_run` production callers use — the immediately
        // following real `begin_run` call then organically fails against
        // the perturbed state, rather than a fabricated error.
        if self
            .state
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::PerturbRunStoppedBeforeBegin,
            )
            .await
        {
            let _ = mqk_db::stop_run(&self.db, run_id).await;
        }

        if let Err(err) = mqk_db::begin_run(&self.db, run_id).await {
            self.release_orchestrator_leadership(orchestrator, "begin_rollback")
                .await;
            return Err(RuntimeStartEffectsError::internal(
                "start begin_run failed",
                err.to_string(),
            ));
        }

        // PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 3 row 12:
        // same technique, one call site later — perturbs after a genuine
        // `begin_run` success so the real initial `heartbeat_run` call
        // organically fails.
        if self
            .state
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::PerturbRunStoppedBeforeInitialHeartbeat,
            )
            .await
        {
            let _ = mqk_db::stop_run(&self.db, run_id).await;
        }

        if let Err(err) = mqk_db::heartbeat_run(&self.db, run_id, Utc::now()).await {
            self.release_orchestrator_leadership(orchestrator, "heartbeat_rollback")
                .await;
            return Err(RuntimeStartEffectsError::internal(
                "start initial heartbeat failed",
                err.to_string(),
            ));
        }
        self.set_phase(RuntimeStartPhase::RunningBeforeInitialTick);

        // Phase advances to `InitialTickStarted` before the call, not after
        // — a failure discovered here (including a panic unwind, since the
        // phase is already recorded) must be treated as "tick's outcome is
        // unknown", never as "tick never ran" (requirement 4).
        self.set_phase(RuntimeStartPhase::InitialTickStarted);
        if let Err(err) = orchestrator.tick().await {
            let message = err.to_string();
            self.release_orchestrator_leadership(orchestrator, "tick_rollback")
                .await;
            if message.contains("RUNTIME_LEASE") {
                return Err(RuntimeStartEffectsError::conflict(
                    "runtime.start_refused.service_unavailable",
                    format!("runtime leader lease unavailable: {message}"),
                ));
            }
            return Err(RuntimeStartEffectsError::internal(
                "start initial tick failed",
                message,
            ));
        }
        self.set_phase(RuntimeStartPhase::InitialTickCompleted);

        // DEADMAN-EXPIRED-AFTER-START-01: refresh heartbeat after the initial
        // tick.  orchestrator.tick() may block for tens of seconds (Alpaca
        // fetch_events has no HTTP timeout).  The heartbeat written above can
        // be stale by the time tick() returns; the execution loop's first
        // pre-tick deadman check would then fire immediately.  A fresh
        // heartbeat here ensures the loop starts with a current timestamp.
        if let Err(err) = mqk_db::heartbeat_run(&self.db, run_id, Utc::now()).await {
            self.release_orchestrator_leadership(orchestrator, "post_tick_heartbeat")
                .await;
            return Err(RuntimeStartEffectsError::internal(
                "post-initial-tick heartbeat refresh failed",
                err.to_string(),
            ));
        }

        // BUNDLE-7-PHASE-7A fault seam: after run arm/begin/initial tick
        // (and the post-tick heartbeat refresh), before any counter
        // reset/snapshot/provenance/bootstrap/selection commit. No
        // dynamic-selection state has been committed yet. Phase is already
        // `InitialTickCompleted` — not cleanly stoppable — matching
        // requirement 4's "once tick begins or completes, Halted" policy.
        if self
            .state
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::AfterRunArmBeginInitialTick,
            )
            .await
        {
            self.release_orchestrator_leadership(orchestrator, "dynamic_selection_fault_seam")
                .await;
            return Err(RuntimeStartEffectsError::internal(
                "dynamic_selection.fault_seam.after_run_arm_begin_initial_tick",
                "test-injected fault after run arm/begin/initial tick",
            ));
        }

        self.set_phase(RuntimeStartPhase::LocalCommitStarted);

        // ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 2: build every
        // run-scoped local value first (unpublished), then commit the whole
        // bundle to `AppState` in one call — never field-by-field as each
        // becomes available.
        let execution_snapshot = orchestrator.snapshot().await.ok();

        // TV-01C: artifact provenance. Uses the artifact intake result
        // evaluated once above (TV-01 hoist) — the same identity that
        // passed all pre-DB gates is the identity recorded as this run's
        // provenance. Only `Accepted` carries positive provenance; every
        // other outcome yields `None` (fail-closed: absent/invalid/
        // unavailable artifacts are not recorded as consumed).
        // BUNDLE-7-PHASE-7A-TRUE-ATOMIC requirement 7: mutex-poison recovery
        // via `unwrap_or_else(|poisoned| poisoned.into_inner())` (matches
        // `set_phase`/`release_orchestrator_leadership` — pure process-local
        // progress state, no invariant a poisoned write could violate).  The
        // inner `.take()` returning `None` here is a genuine caller-ordering
        // bug (this trait method must be driven at most once per attempt),
        // and is now a typed error instead of a panic.
        let artifact_intake_value = self
            .artifact_intake
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let accepted_artifact = match artifact_intake_value {
            Some(ArtifactIntakeOutcome::Accepted {
                artifact_id,
                artifact_type,
                stage,
                produced_by,
            }) => Some(AcceptedArtifactProvenance {
                artifact_id,
                artifact_type,
                stage,
                produced_by,
            }),
            Some(_) => None,
            None => {
                self.release_orchestrator_leadership(
                    orchestrator,
                    "start_runtime_effects_called_twice",
                )
                .await;
                return Err(RuntimeStartEffectsError::internal(
                    "runtime.start_refused.start_runtime_effects_called_twice",
                    "start_runtime_effects consumed artifact_intake a second time; \
                     this indicates a caller-ordering bug (each RuntimeStartEffects \
                     instance must drive at most one start attempt)",
                ));
            }
        };

        // B1A: native strategy bootstrap for the active run — taken here,
        // after all DB operations and the initial tick succeeded, so it is
        // only ever committed for a fully-live run.
        let native_strategy_bootstrap = self
            .native_strategy_bootstrap
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();

        // BUNDLE-7-PHASE-7A: the already-evaluated, frozen dynamic-selection
        // outcome, committed as part of the same bundle as every other
        // run-start effect — never separately, and never before ownership
        // is reserved (already true: reservation is this trait's very first
        // call).
        let dynamic_selection_outcome = self
            .dynamic_selection_outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();

        // BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement
        // 2/3: build `RunStartMetadata` and commit every economic-mirror
        // field as one step, transitioning ownership `Reserved{run_id} ->
        // Starting{run_id, metadata}`. Under correct caller ordering this
        // cannot fail (reservation already succeeded above and
        // `AppState::lifecycle_op` has serialized every other local
        // start/stop/halt attempt for the whole duration of this call) —
        // but the typed error is still handled without a panic.
        if let Err(err) = self
            .state
            .prepare_starting_metadata_and_mirrors(
                run_id,
                crate::state::RunStartLocalBundle {
                    execution_snapshot,
                    accepted_artifact,
                    native_strategy_bootstrap,
                    dynamic_selection_outcome,
                },
                self.legacy_assignments.clone(),
                "start_attempt_snapshot",
            )
            .await
        {
            self.release_orchestrator_leadership(orchestrator, "prepare_starting_metadata_failed")
                .await;
            return Err(RuntimeStartEffectsError::internal(
                "runtime.start_refused.prepare_starting_metadata_failed",
                err.to_string(),
            ));
        }

        // BUNDLE-7-PHASE-7A fault seam: immediately after the run-start
        // bundle commit above, before this function returns `Ok(())`. The
        // bundle IS committed at this point — but the caller
        // (`advance_run_to_active`) now runs `rollback_local_effects` (a
        // run_id-scoped compare-and-clear) on this `Err` uniformly, so this
        // branch itself only needs to release the orchestrator lease it
        // holds locally.
        if self
            .state
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::AfterProcessLocalSelectionCommit,
            )
            .await
        {
            self.release_orchestrator_leadership(orchestrator, "dynamic_selection_fault_seam")
                .await;
            return Err(RuntimeStartEffectsError::internal(
                "dynamic_selection.fault_seam.after_process_local_selection_commit",
                "test-injected fault after process-local selection commit",
            ));
        }

        *self
            .orchestrator
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(orchestrator);
        Ok(())
    }

    /// Only called after `start_runtime_effects` has returned `Ok` — the
    /// slot is already `Reserved{run_id}` (from `reserve_local_ownership`,
    /// this trait's first call) and the run-start bundle is already
    /// committed, so this only has to spawn the task and flip the slot to
    /// `Active`.
    async fn spawn_loop(
        &self,
        run_id: uuid::Uuid,
    ) -> Result<(), crate::daily_data_readiness::RuntimeStartEffectsError> {
        use crate::daily_data_readiness::{RuntimeStartEffectsError, RuntimeStartPhase};

        // BUNDLE-7-PHASE-7A fault seam: immediately before loop spawn.
        if self
            .state
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::ImmediatelyBeforeLoopSpawn,
            )
            .await
        {
            let orchestrator_to_release = self
                .orchestrator
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if let Some(orchestrator) = orchestrator_to_release {
                self.release_orchestrator_leadership(orchestrator, "dynamic_selection_fault_seam")
                    .await;
            }
            return Err(RuntimeStartEffectsError::internal(
                "dynamic_selection.fault_seam.immediately_before_loop_spawn",
                "test-injected fault immediately before loop spawn",
            ));
        }

        let orchestrator_to_spawn = self
            .orchestrator
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(orchestrator) = orchestrator_to_spawn else {
            // ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 7: a
            // duplicate/misordered `spawn_loop` call (no orchestrator left
            // to spawn — `start_runtime_effects` must have failed or never
            // ran) is a stable typed error, not a panic.
            return Err(RuntimeStartEffectsError::internal(
                "runtime.start_refused.spawn_loop_missing_orchestrator",
                "spawn_loop called without a constructed orchestrator; \
                 start_runtime_effects must succeed first",
            ));
        };
        // BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement
        // 3: the startup barrier. The task is spawned already blocked on
        // `barrier_rx` (raced against its own stop signal) — it does zero
        // economic work (no ticker, no deadman, no tick, no outbox/broker
        // touch) until this barrier is released, which only happens below,
        // strictly after `install_active_runtime` has atomically installed
        // this exact handle as `Active` for `run_id`.
        // PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE Part 1/3: move the
        // one frozen dispatch authority (built once, before this point, by
        // `build_dynamic_selection_start_snapshot`) wholesale into the
        // spawned loop task — never cloned, never rebuilt. Taken from the
        // Mutex exactly once per start attempt, matching this trait's
        // existing single-consumption convention for `orchestrator`/
        // `native_strategy_bootstrap`/`dynamic_selection_outcome` above.
        let dispatch_authority = self
            .dispatch_authority
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .unwrap_or_else(|| {
                crate::dynamic_selection_dispatch_authority::RuntimeStrategyDispatchAuthority::Legacy {
                    assignments: self.legacy_assignments.clone(),
                }
            });
        let (barrier_tx, barrier_rx) = tokio::sync::oneshot::channel();
        let handle = spawn_execution_loop(
            Arc::clone(self.state),
            orchestrator,
            run_id,
            dispatch_authority,
            barrier_rx,
        );
        if let Err(install_err) = self.state.install_active_runtime(run_id, handle).await {
            // `install_active_runtime` already sent the task its stop
            // signal and joined it on any mismatch — the task wakes via
            // the stop arm of its startup select, never the barrier arm,
            // so it never touches economic state. Dropping the barrier
            // sender here (rather than sending) is inert either way; no
            // detached task remains.
            drop(barrier_tx);
            let message = format!("failed to install the spawned execution loop: {install_err}");
            // PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 1
            // requirement 4/5: fold the task's own join/leadership-release
            // truth into this attempt's rollback truth instead of letting
            // `install_active_runtime`'s typed cleanup evaporate once this
            // function returns only a `RuntimeStartEffectsError` string.
            // `rollback_local_effects` (driven next by `advance_run_to_
            // active`'s fail-closed rollback) reads these back into
            // `LocalRollbackOutcome` — never a double release, since
            // `self.orchestrator` is already `None` by this point (moved
            // into the spawned task above).
            let cleanup = install_err.into_cleanup();
            if cleanup.is_degraded() {
                tracing::error!(
                    run_id = %run_id,
                    "runtime_start_barrier_task_cleanup_degraded join_outcome={:?} \
                     leadership_release_outcome={:?}",
                    cleanup.join_outcome,
                    cleanup.leadership_release_outcome,
                );
            }
            *self
                .task_side_join_outcome
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(cleanup.join_outcome);
            *self
                .task_side_leadership_release_outcome
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                cleanup.leadership_release_outcome;
            return Err(RuntimeStartEffectsError::internal(
                "runtime.start_refused.spawn_loop_install_failed",
                message,
            ));
        }
        self.set_phase(RuntimeStartPhase::LoopInstalled);
        // Barrier release: only now may the task build its ticker and
        // begin real economic work.
        let _ = barrier_tx.send(());
        Ok(())
    }

    /// BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement 4:
    /// clear every local effect this attempt may have committed,
    /// run_id-scoped, via the single unified cleanup authority
    /// (`AppState::clear_local_runtime_for_run`) — whatever ownership state
    /// this attempt reached (`Reserved`/`Starting`/`Active`), its metadata
    /// (including dynamic-selection truth) and every economic-mirror field
    /// are cleared together. Idempotent — safe to call regardless of how
    /// far the attempt got. Also releases any orchestrator lease this
    /// attempt still holds (defense in depth — every `start_runtime_
    /// effects`/`spawn_loop` failure branch above already released it
    /// inline before returning `Err`, recording the outcome via
    /// `release_orchestrator_leadership`; this only catches a future
    /// failure path that forgets to).
    async fn rollback_local_effects(
        &self,
        run_id: uuid::Uuid,
    ) -> crate::daily_data_readiness::LocalRollbackOutcome {
        let orchestrator_to_release = self
            .orchestrator
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(orchestrator) = orchestrator_to_release {
            self.release_orchestrator_leadership(orchestrator, "rollback_local_effects")
                .await;
        }

        self.state
            .clear_local_runtime_for_run(run_id, crate::state::LifecycleClearReason::FailedStart)
            .await;

        crate::daily_data_readiness::LocalRollbackOutcome {
            leadership_release_outcome: self
                .leadership_release_outcome
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take(),
            task_side_leadership_release_outcome: self
                .task_side_leadership_release_outcome
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take(),
            task_side_join_outcome: self
                .task_side_join_outcome
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take(),
        }
    }

    fn start_phase_reached(&self) -> crate::daily_data_readiness::RuntimeStartPhase {
        *self
            .start_phase
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

// ---------------------------------------------------------------------------
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A-FINAL-PRIVATE-PRODUCTION-
// EFFECTS-PROOF requirement 6: private, in-crate hermetic seam that drives
// the exact production `ProductionRuntimeStartEffects` +
// `daily_data_readiness::advance_run_to_active` sequencing directly,
// bypassing `start_execution_runtime`'s outer deployment/capital/artifact/
// policy gates — which, for `BrokerKind::Paper`, would refuse before ever
// reaching this code (`deployment_mode_readiness` refuses `(Paper, Paper)`
// outright; see `scenario_bundle7_phase7a_lifecycle_wiring_01.rs`).
//
// This used to be a `pub` non-cfg-gated seam on `AppState`
// (`drive_production_start_effects_for_test`), reachable from any
// downstream crate or external `tests/*.rs` integration test — a
// default-build production-accessible driver of `ProductionRuntimeStartEffects`,
// exactly the R5 bypass this patch closes. It is now `pub(crate)` and
// `#[cfg(test)]`: reachable only from this crate's own test build, never
// from production code or an external integration test.
//
// `self.runtime_selection.broker_kind` must be `BrokerKind::Paper` for this
// to be hermetic: `build_execution_orchestrator` then constructs
// `DaemonBroker::Paper(LockedPaperBroker::default())` — in-process, zero
// network, zero credentials. This lets a test exercise the *real*
// reservation/commit/spawn/rollback code path (not a fake
// `RuntimeStartEffects`) against a real isolated test DB, without ever
// constructing a broker capable of live capital.
// ---------------------------------------------------------------------------
#[cfg(test)]
impl AppState {
    pub(crate) async fn drive_production_start_effects_for_test(
        self: &Arc<Self>,
        db: PgPool,
        run_id: uuid::Uuid,
        dynamic_selection_outcome: Option<DynamicSelectionRuntimeState>,
    ) -> (
        Result<(), crate::daily_data_readiness::RuntimeStartSequenceError>,
        Vec<&'static str>,
    ) {
        let effects = ProductionRuntimeStartEffects {
            state: self,
            db: db.clone(),
            artifact_intake: std::sync::Mutex::new(Some(ArtifactIntakeOutcome::NotConfigured)),
            native_strategy_bootstrap: std::sync::Mutex::new(None),
            orchestrator: std::sync::Mutex::new(None),
            dynamic_selection_outcome: std::sync::Mutex::new(dynamic_selection_outcome),
            legacy_assignments: Vec::new(),
            dispatch_authority: std::sync::Mutex::new(None),
            start_phase: std::sync::Mutex::new(
                crate::daily_data_readiness::RuntimeStartPhase::BeforeArm,
            ),
            leadership_release_outcome: std::sync::Mutex::new(None),
            task_side_leadership_release_outcome: std::sync::Mutex::new(None),
            task_side_join_outcome: std::sync::Mutex::new(None),
        };
        let mut trace: Vec<&'static str> = Vec::new();
        let result = crate::daily_data_readiness::advance_run_to_active(
            &db, &effects, run_id, None, &mut trace,
        )
        .await;
        (result, trace)
    }

    /// TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 Blocker 3: identical to
    /// [`Self::drive_production_start_effects_for_test`] except it also
    /// injects a pre-built `dispatch_authority` — mirrors exactly what real
    /// production `start_execution_runtime` does (build the dispatch
    /// authority via `build_dynamic_selection_start_snapshot`, then hand it
    /// to the same `ProductionRuntimeStartEffects.dispatch_authority` field
    /// consumed by the real `spawn_loop`). The prior test-only seam always
    /// passed `None` here, silently defaulting every dynamic-selection
    /// fixture (including `PaperEnforcedAllowed`) to `Legacy` inside the
    /// real spawned loop — this is the exact gap this repair closes: a real
    /// `DynamicPaperEnforced` authority can now reach a genuinely spawned,
    /// hermetic `spawn_execution_loop` task. `pub(crate)` + `#[cfg(test)]`,
    /// reachable only from this crate's own test build.
    pub(crate) async fn drive_production_start_effects_with_dispatch_authority_for_test(
        self: &Arc<Self>,
        db: PgPool,
        run_id: uuid::Uuid,
        dynamic_selection_outcome: Option<DynamicSelectionRuntimeState>,
        dispatch_authority: Option<
            crate::dynamic_selection_dispatch_authority::RuntimeStrategyDispatchAuthority,
        >,
    ) -> (
        Result<(), crate::daily_data_readiness::RuntimeStartSequenceError>,
        Vec<&'static str>,
    ) {
        let effects = ProductionRuntimeStartEffects {
            state: self,
            db: db.clone(),
            artifact_intake: std::sync::Mutex::new(Some(ArtifactIntakeOutcome::NotConfigured)),
            native_strategy_bootstrap: std::sync::Mutex::new(None),
            orchestrator: std::sync::Mutex::new(None),
            dynamic_selection_outcome: std::sync::Mutex::new(dynamic_selection_outcome),
            legacy_assignments: Vec::new(),
            dispatch_authority: std::sync::Mutex::new(dispatch_authority),
            start_phase: std::sync::Mutex::new(
                crate::daily_data_readiness::RuntimeStartPhase::BeforeArm,
            ),
            leadership_release_outcome: std::sync::Mutex::new(None),
            task_side_leadership_release_outcome: std::sync::Mutex::new(None),
            task_side_join_outcome: std::sync::Mutex::new(None),
        };
        let mut trace: Vec<&'static str> = Vec::new();
        let result = crate::daily_data_readiness::advance_run_to_active(
            &db, &effects, run_id, None, &mut trace,
        )
        .await;
        (result, trace)
    }

    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 3 row 9
    /// ("run-linked evidence persistence failure"): identical to
    /// [`Self::drive_production_start_effects_for_test`] except it drives
    /// `advance_run_to_active` with a real `readiness_link`, so the real
    /// `persist_run_linked_readiness_evidence` call executes (never a fake
    /// bypass). `pub(crate)` + `#[cfg(test)]`, reachable only from this
    /// crate's own test build.
    #[cfg(test)]
    pub(crate) async fn drive_production_start_effects_with_readiness_link_for_test(
        self: &Arc<Self>,
        db: PgPool,
        run_id: uuid::Uuid,
        dynamic_selection_outcome: Option<DynamicSelectionRuntimeState>,
        readiness_link: Option<(uuid::Uuid, chrono::DateTime<Utc>)>,
    ) -> (
        Result<(), crate::daily_data_readiness::RuntimeStartSequenceError>,
        Vec<&'static str>,
    ) {
        let effects = ProductionRuntimeStartEffects {
            state: self,
            db: db.clone(),
            artifact_intake: std::sync::Mutex::new(Some(ArtifactIntakeOutcome::NotConfigured)),
            native_strategy_bootstrap: std::sync::Mutex::new(None),
            orchestrator: std::sync::Mutex::new(None),
            dynamic_selection_outcome: std::sync::Mutex::new(dynamic_selection_outcome),
            legacy_assignments: Vec::new(),
            dispatch_authority: std::sync::Mutex::new(None),
            start_phase: std::sync::Mutex::new(
                crate::daily_data_readiness::RuntimeStartPhase::BeforeArm,
            ),
            leadership_release_outcome: std::sync::Mutex::new(None),
            task_side_leadership_release_outcome: std::sync::Mutex::new(None),
            task_side_join_outcome: std::sync::Mutex::new(None),
        };
        let mut trace: Vec<&'static str> = Vec::new();
        let result = crate::daily_data_readiness::advance_run_to_active(
            &db,
            &effects,
            run_id,
            readiness_link,
            &mut trace,
        )
        .await;
        (result, trace)
    }
}

// ---------------------------------------------------------------------------
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A-FINAL-PRIVATE-PRODUCTION-
// EFFECTS-PROOF requirement 6: real-effects success/failure matrix driven
// through `AppState::drive_production_start_effects_for_test` above — the
// actual `ProductionRuntimeStartEffects` implementation, never a fake.
//
// Moved in-crate from the former external integration test
// `tests/scenario_bundle7_phase7a_final_atomic_ownership_and_rollback_
// truth_01.rs` (deleted) so it can reach the now-private (`pub(crate)`,
// `#[cfg(test)]`) seams above. Per repo DB-test rule, hard-refuses any
// `MQK_DATABASE_URL` that is not explicitly the port-5434 local test DB.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod real_production_effects_matrix_tests {
    use super::*;
    use crate::state::StrategyBarInput;

    /// Serializes every test in this module: they all contend for the same
    /// `(DAEMON_ENGINE_ID, Paper)` "active run" slot via
    /// `AppState::create_or_reuse_run_for_start`.
    fn db_test_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    /// Hard-refuses any `MQK_DATABASE_URL` that is not explicitly the
    /// port-5434 local test database — never 5440 (paper), never an
    /// unqualified host.
    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 5: name of the
    /// env var that turns every early-return below from a silent skip into
    /// a hard test failure. Unset (the default) preserves the pre-existing
    /// behavior — general `cargo test` runs remain DB-optional. Set by the
    /// dedicated fail-fast matrix command only.
    const REQUIRE_MATRIX_ENV: &str = "MQK_REQUIRE_PHASE7A_R6_MATRIX";

    fn require_matrix() -> bool {
        std::env::var(REQUIRE_MATRIX_ENV).as_deref() == Ok("1")
    }

    async fn db_pool_or_skip(label: &str) -> Option<PgPool> {
        let Ok(url) = std::env::var("MQK_DATABASE_URL") else {
            let message = format!("{label}: MQK_DATABASE_URL is not set");
            if require_matrix() {
                panic!(
                    "{message} — MQK_REQUIRE_PHASE7A_R6_MATRIX=1 requires the port-5434 \
                     local test DB; refusing to silently skip"
                );
            }
            eprintln!("{message}; skipped");
            return None;
        };
        if !url.contains(":5434") {
            let message = format!(
                "{label}: MQK_DATABASE_URL must be the port-5434 local test DB, \
                 refusing to run against: {url}"
            );
            if require_matrix() {
                panic!("{message}");
            }
            eprintln!("{message}; skipped");
            return None;
        }
        let pool = match sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(&url)
            .await
        {
            Ok(pool) => pool,
            Err(e) => {
                let message = format!("{label}: could not connect to MQK_DATABASE_URL: {e}");
                if require_matrix() {
                    panic!("{message}");
                }
                eprintln!("{message}; skipped");
                return None;
            }
        };
        if let Err(e) = mqk_db::migrate(&pool).await {
            let message = format!("{label}: mqk_db::migrate failed: {e}");
            if require_matrix() {
                panic!("{message}");
            }
            eprintln!("{message}; skipped");
            return None;
        }
        Some(pool)
    }

    async fn clear_any_preexisting_active_daemon_run(pool: &PgPool) {
        let _ = sqlx::query(
            "update runs set status = 'STOPPED', stopped_at_utc = now() \
             where engine_id = 'mqk-daemon' and mode = 'PAPER' and status in ('ARMED', 'RUNNING')",
        )
        .execute(pool)
        .await;
        // PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 4: a test in
        // this module (e.g. CLEANUP-HALT-01) may leave the shared reusable
        // run row `HALTED` — `create_or_reuse_run_for_start` refuses to
        // reuse a Halted run without an explicit operator clear, exactly
        // like production. Delete it (not merely clear the status) so every
        // test in this module starts from a genuinely clean slate
        // regardless of run order or a prior test panicking before its own
        // cleanup ran.
        let _ = sqlx::query(
            "delete from sys_autonomous_session_events where run_id in \
             (select run_id from runs where engine_id = 'mqk-daemon' and mode = 'PAPER' \
              and status = 'HALTED')",
        )
        .execute(pool)
        .await;
        let _ = sqlx::query(
            "delete from runs where engine_id = 'mqk-daemon' and mode = 'PAPER' \
             and status = 'HALTED'",
        )
        .execute(pool)
        .await;
    }

    async fn delete_run_and_its_events(pool: &PgPool, run_id: uuid::Uuid) {
        let _ = sqlx::query("delete from sys_autonomous_session_events where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("delete from runs where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
    }

    /// A hermetic `Paper`+`Paper` `AppState` wired to the real test DB. Never
    /// start-able to a genuine `Active` loop through the real broker-
    /// construction path (`build_daemon_broker` refuses `BrokerKind::Paper`)
    /// — this module uses that refusal itself as a real, deterministic
    /// `start_runtime_effects` failure (FA-01), and never needs a working
    /// broker for the reservation-only proof (FA-02).
    fn hermetic_paper_state(pool: &PgPool) -> Arc<AppState> {
        let mut state =
            AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Paper);
        state.db = Some(pool.clone());
        Arc::new(state)
    }

    fn off_disposition_fixture(run_id: uuid::Uuid) -> DynamicSelectionRuntimeState {
        DynamicSelectionRuntimeState {
            run_id,
            disposition:
                crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::Off,
            configured_mode: mqk_portfolio::DynamicSelectionMode::Off,
            effective_mode: mqk_portfolio::DynamicSelectionMode::Off,
            live_lock_applied: false,
            plan: None,
            plan_id: None,
            selected_pairs: Vec::new(),
            host_pool_present: false,
            reasons: Vec::new(),
            approved_for_live: false,
            evidence_persisted: false,
            evidence_validation_state: None,
        }
    }

    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 2 success
    /// scenario 2: `ShadowAllowed` — a positive Shadow-mode outcome with a
    /// non-empty selected-pair set, never a host pool (Shadow has no
    /// economic authority to withhold).
    fn shadow_allowed_disposition_fixture(run_id: uuid::Uuid) -> DynamicSelectionRuntimeState {
        DynamicSelectionRuntimeState {
            run_id,
            disposition:
                crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::ShadowAllowed,
            configured_mode: mqk_portfolio::DynamicSelectionMode::Shadow,
            effective_mode: mqk_portfolio::DynamicSelectionMode::Shadow,
            live_lock_applied: false,
            plan: None,
            plan_id: None,
            selected_pairs: vec![("AAPL".to_string(), "swing_momentum".to_string(), 300)],
            host_pool_present: false,
            reasons: Vec::new(),
            approved_for_live: false,
            evidence_persisted: false,
            evidence_validation_state: None,
        }
    }

    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 2 success
    /// scenario 3: `ShadowInvalid` — Shadow never blocks the run even when
    /// its own outcome was not a genuine positive one; the invalid reasons
    /// are preserved, no host pool, no selected pairs.
    fn shadow_invalid_disposition_fixture(run_id: uuid::Uuid) -> DynamicSelectionRuntimeState {
        DynamicSelectionRuntimeState {
            run_id,
            disposition:
                crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::ShadowInvalid,
            configured_mode: mqk_portfolio::DynamicSelectionMode::Shadow,
            effective_mode: mqk_portfolio::DynamicSelectionMode::Shadow,
            live_lock_applied: false,
            plan: None,
            plan_id: None,
            selected_pairs: Vec::new(),
            host_pool_present: false,
            reasons: vec![
                crate::dynamic_selection_start_gate::DynamicSelectionStartGateReason::PlanInvalid {
                    truth_state: "test_injected_invalid".to_string(),
                },
            ],
            approved_for_live: false,
            evidence_persisted: false,
            evidence_validation_state: None,
        }
    }

    // -------------------------------------------------------------------
    // FA-01: a genuine `start_runtime_effects` failure (real broker
    // construction refusing `BrokerKind::Paper`) is cleanly rolled back —
    // reservation released, phase stays `BeforeArm` (arm_run was never
    // attempted), durable disposition is `AlreadyNonActive` (nothing was
    // ever armed).
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn fa_01_real_orchestrator_construction_failure_is_cleanly_rolled_back() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("FA-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");

        let (result, trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;

        assert!(
            result.is_err(),
            "FA-01: expected failure (Paper broker construction is refused)"
        );
        let (original, rollback) = match result.unwrap_err() {
            crate::daily_data_readiness::RuntimeStartSequenceError::Effects {
                original,
                rollback,
            } => (original, rollback),
            other => panic!("FA-01: expected Effects, got {other:?}"),
        };
        assert_eq!(
            original.fault_class, "runtime.start_refused.paper_broker_not_execution_path",
            "FA-01: this must be the real build_daemon_broker refusal, not a \
             different failure mode: {original:?}"
        );
        assert_eq!(
            trace,
            vec!["ownership_reserved"],
            "FA-01: reservation succeeds (it never touches a broker); the \
             failure happens inside the next step, start_runtime_effects, \
             before any further trace tag fires"
        );
        assert!(
            matches!(
                rollback.phase_reached,
                crate::daily_data_readiness::RuntimeStartPhase::BeforeArm
            ),
            "FA-01: the real orchestrator construction failure happens before \
             arm_run is ever attempted — phase must still be BeforeArm: \
             {rollback:?}"
        );
        assert!(
            matches!(
                rollback.durable,
                crate::daily_data_readiness::DurableRollbackDisposition::AlreadyNonActive
            ),
            "FA-01: nothing was ever armed, so the durable run was never \
             Armed/Running: {rollback:?}"
        );
        assert!(!rollback.durable_status_unknown);

        assert_eq!(
            state.locally_owned_run_id().await,
            None,
            "FA-01: a failed start must release its own reservation"
        );
        assert!(
            state.dynamic_selection_runtime_snapshot().await.is_none(),
            "FA-01: no dynamic-selection state may survive a failed start — \
             start_runtime_effects failed before the local bundle was ever built"
        );
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Created
        ));

        delete_run_and_its_events(&pool, run_id).await;
    }

    // -------------------------------------------------------------------
    // FA-02 (requirement 3, explicit ask): "Add a production-AppState test
    // with sentinel values for an existing owner. After a conflicting
    // start, prove loop/artifact/bootstrap/snapshot/counters/targets/
    // dynamic-selection state are unchanged."
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn fa_02_ownership_conflict_preserves_existing_owner_sentinel_state() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("FA-02").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);

        let run_a = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.final.fa02.run_a",
        );
        state
            .establish_db_backed_active_run_for_test(run_a)
            .await
            .expect("FA-02: run A must be established as the active owner");

        let sentinel_artifact = AcceptedArtifactProvenance {
            artifact_id: "fa-02-sentinel-artifact".to_string(),
            artifact_type: "sentinel".to_string(),
            stage: "sentinel".to_string(),
            produced_by: "fa-02-test".to_string(),
        };
        state
            .plant_accepted_artifact_for_test(Some(sentinel_artifact.clone()))
            .await;
        state.plant_day_signal_count_for_test(4242);
        state
            .commit_dynamic_selection_runtime_state_for_test(off_disposition_fixture(run_a))
            .await;

        assert_eq!(state.locally_owned_run_id().await, Some(run_a));

        let run_b = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            format!("mqk-daemon.phase7a.final.fa02.run_b.{run_a}").as_bytes(),
        );
        mqk_db::insert_run(
            &pool,
            &mqk_db::NewRun {
                run_id: run_b,
                engine_id: "mqk-daemon".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: Utc::now(),
                git_hash: "UNKNOWN".to_string(),
                config_hash: "fa-02-test".to_string(),
                config_json: serde_json::json!({"test": "fa-02"}),
                host_fingerprint: "fa-02-test-host".to_string(),
            },
        )
        .await
        .expect("FA-02: run_b insert must succeed");

        let (result_b, trace_b) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_b,
                Some(off_disposition_fixture(run_b)),
            )
            .await;

        assert!(result_b.is_err(), "FA-02: run B must be refused");
        assert!(
            trace_b.is_empty(),
            "FA-02: no ordered-trace tag may fire for a reservation conflict"
        );
        let (original_b, rollback_b) = match result_b.unwrap_err() {
            crate::daily_data_readiness::RuntimeStartSequenceError::Effects {
                original,
                rollback,
            } => (original, rollback),
            other => panic!("FA-02: expected Effects, got {other:?}"),
        };
        assert_eq!(
            original_b.fault_class, "runtime.start_refused.local_ownership_conflict",
            "FA-02: this must be the real reservation conflict, not a different \
             failure mode: {original_b:?}"
        );
        assert!(
            matches!(
                rollback_b.durable,
                crate::daily_data_readiness::DurableRollbackDisposition::AlreadyNonActive
            ),
            "FA-02: run B was never armed, so its own durable rollback is a \
             no-op: {rollback_b:?}"
        );

        assert_eq!(
            state.locally_owned_run_id().await,
            Some(run_a),
            "FA-02: the slot must still show run A as the Active owner — run \
             B's rollback must never clear a different run's reservation"
        );
        let after_selection = state
            .dynamic_selection_runtime_snapshot()
            .await
            .expect("FA-02: run A's dynamic-selection state must still be present");
        assert_eq!(
            after_selection.run_id, run_a,
            "FA-02: run A's committed dynamic-selection state must be unchanged \
             (still tagged with run A's run_id, never cleared or overwritten by \
             run B's rollback)"
        );
        assert_eq!(
            state.day_signal_count_snapshot_for_test(),
            4242,
            "FA-02: run A's sentinel day_signal_count must be unchanged"
        );
        assert_eq!(
            state.accepted_artifact_snapshot_for_test().await,
            Some(sentinel_artifact),
            "FA-02: run A's sentinel accepted_artifact provenance must be unchanged"
        );
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_a)
                .await
                .expect("fetch_run run_a")
                .status,
            mqk_db::RunStatus::Running
        ));
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_b)
                .await
                .expect("fetch_run run_b")
                .status,
            mqk_db::RunStatus::Created
        ));

        let _ = state.stop_execution_runtime().await;
        delete_run_and_its_events(&pool, run_a).await;
        delete_run_and_its_events(&pool, run_b).await;
    }

    // -------------------------------------------------------------------
    // SUCCESS-01 (R6 success scenario 1, `Off`): genuine end-to-end success
    // through the real production-effects path. The hermetic broker
    // override lets `build_execution_orchestrator` construct a real
    // (in-process, zero-network, zero-credential) Paper orchestrator
    // instead of `build_daemon_broker` refusing `BrokerKind::Paper` — the
    // one thing that made a genuine success unreachable before this patch
    // (see the module doc above and the deleted external test file's own
    // documented conclusion). Proves reserve -> arm -> begin -> initial
    // heartbeat -> initial tick -> post-tick heartbeat -> Starting
    // metadata/mirrors -> barriered spawn -> Active -> barrier release,
    // then a real stop, through the exact production
    // `ProductionRuntimeStartEffects` implementation — never a fake.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn success_off_disposition_reaches_active_and_stops_cleanly() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("SUCCESS-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");

        let (result, trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;

        assert!(
            result.is_ok(),
            "SUCCESS-01: expected a genuine success through the real \
             production-effects path with the hermetic broker override \
             enabled: {result:?}"
        );
        assert_eq!(
            trace,
            vec![
                "ownership_reserved",
                "local_bundle_committed",
                "loop_spawned"
            ],
            "SUCCESS-01: every ordered-trace tag must fire exactly once, in order"
        );

        assert_eq!(state.locally_owned_run_id().await, Some(run_id));
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Running
        ));

        // A real stop through the real cleanup path.
        let stop_result = state.stop_execution_runtime().await;
        assert!(
            stop_result.is_ok(),
            "SUCCESS-01: stop must succeed: {stop_result:?}"
        );
        assert_eq!(
            state.locally_owned_run_id().await,
            None,
            "SUCCESS-01: stop must fully clear local ownership"
        );

        delete_run_and_its_events(&pool, run_id).await;
    }

    // -------------------------------------------------------------------
    // SUCCESS-02 (R6 success scenario 2): ShadowAllowed reaches Active
    // through the real production-effects path, preserves its
    // dynamic-selection metadata (selected pairs, no host pool,
    // approved_for_live=false), and stop clears everything.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn success_shadow_allowed_disposition_reaches_active_and_stops_cleanly() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("SUCCESS-02").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");

        let (result, trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(shadow_allowed_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;

        assert!(
            result.is_ok(),
            "SUCCESS-02: expected a genuine ShadowAllowed success: {result:?}"
        );
        assert_eq!(
            trace,
            vec![
                "ownership_reserved",
                "local_bundle_committed",
                "loop_spawned"
            ]
        );
        assert_eq!(state.locally_owned_run_id().await, Some(run_id));
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Running
        ));

        let snapshot = state
            .dynamic_selection_runtime_snapshot()
            .await
            .expect("SUCCESS-02: dynamic-selection metadata must be committed for ShadowAllowed");
        assert!(matches!(
            snapshot.disposition,
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::ShadowAllowed
        ));
        assert_eq!(
            snapshot.selected_pairs,
            vec![("AAPL".to_string(), "swing_momentum".to_string(), 300)],
            "SUCCESS-02: selected pairs must be preserved exactly"
        );
        assert!(
            !snapshot.host_pool_present,
            "SUCCESS-02: Shadow must never retain a host pool"
        );
        assert!(!snapshot.approved_for_live);

        let stop_result = state.stop_execution_runtime().await;
        assert!(
            stop_result.is_ok(),
            "SUCCESS-02: stop must succeed: {stop_result:?}"
        );
        assert_eq!(state.locally_owned_run_id().await, None);
        assert!(
            state.dynamic_selection_runtime_snapshot().await.is_none(),
            "SUCCESS-02: stop must clear dynamic-selection metadata too"
        );

        delete_run_and_its_events(&pool, run_id).await;
    }

    // -------------------------------------------------------------------
    // SUCCESS-03 (R6 success scenario 3): ShadowInvalid still reaches
    // Active through the real production-effects path (Shadow never
    // blocks the run) and preserves its invalid reasons; stop clears
    // everything.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn success_shadow_invalid_disposition_reaches_active_and_stops_cleanly() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("SUCCESS-03").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");

        let (result, trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(shadow_invalid_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;

        assert!(
            result.is_ok(),
            "SUCCESS-03: ShadowInvalid must never block the run: {result:?}"
        );
        assert_eq!(
            trace,
            vec![
                "ownership_reserved",
                "local_bundle_committed",
                "loop_spawned"
            ]
        );
        assert_eq!(state.locally_owned_run_id().await, Some(run_id));
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Running
        ));

        let snapshot = state
            .dynamic_selection_runtime_snapshot()
            .await
            .expect("SUCCESS-03: dynamic-selection metadata must be committed for ShadowInvalid");
        assert!(matches!(
            snapshot.disposition,
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::ShadowInvalid
        ));
        assert!(snapshot.selected_pairs.is_empty());
        assert!(!snapshot.host_pool_present);
        assert_eq!(
            snapshot.reasons.len(),
            1,
            "SUCCESS-03: invalid reasons must be preserved"
        );
        assert!(matches!(
            snapshot.reasons[0],
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateReason::PlanInvalid { .. }
        ));

        let stop_result = state.stop_execution_runtime().await;
        assert!(
            stop_result.is_ok(),
            "SUCCESS-03: stop must succeed: {stop_result:?}"
        );
        assert_eq!(state.locally_owned_run_id().await, None);

        delete_run_and_its_events(&pool, run_id).await;
    }

    // -------------------------------------------------------------------
    // CLEANUP-HALT-01 (R6 item 23, Part 4): starting from a run that
    // reached Active through the real hermetic ProductionRuntimeStartEffects
    // path, a real `halt_execution_runtime()` call must clear local
    // ownership, disarm+halt integrity, and durably halt the run.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn cleanup_halt_from_real_active_run_clears_ownership_and_halts_durably() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("CLEANUP-HALT-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");
        let (result, _trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        result.expect("CLEANUP-HALT-01: setup must reach Active");
        assert_eq!(state.locally_owned_run_id().await, Some(run_id));

        let halt_result = state.halt_execution_runtime().await;
        assert!(
            halt_result.is_ok(),
            "CLEANUP-HALT-01: halt must succeed: {halt_result:?}"
        );
        assert_eq!(
            state.locally_owned_run_id().await,
            None,
            "CLEANUP-HALT-01: halt must fully clear local ownership"
        );
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Halted
        ));
        assert!(
            state.integrity.read().await.is_execution_blocked(),
            "CLEANUP-HALT-01: integrity must be disarmed+halted after an operator halt"
        );

        // Restart after this exit must succeed (Part 4: "restart succeeds
        // after each exit") — but `create_or_reuse_run_for_start` refuses to
        // reuse a durably Halted run without an explicit operator
        // acknowledgment first, exactly like the real operator flow.
        mqk_db::clear_halted_run(&pool, run_id)
            .await
            .expect("CLEANUP-HALT-01: operator halt-clear must succeed");
        {
            let mut integrity = state.integrity.write().await;
            integrity.disarmed = false;
            integrity.halted = false;
        }
        let run_id_2 = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("CLEANUP-HALT-01: restart run creation must succeed");
        state.set_hermetic_test_broker_override_for_test(true).await;
        let (restart_result, _trace2) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id_2,
                Some(off_disposition_fixture(run_id_2)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        assert!(
            restart_result.is_ok(),
            "CLEANUP-HALT-01: restart after halt must succeed: {restart_result:?}"
        );
        state
            .stop_execution_runtime()
            .await
            .expect("CLEANUP-HALT-01: cleanup stop must succeed");

        delete_run_and_its_events(&pool, run_id).await;
        delete_run_and_its_events(&pool, run_id_2).await;
    }

    // -------------------------------------------------------------------
    // CLEANUP-SHUTDOWN-01 (R6 item 24, Part 4): starting from a run that
    // reached Active through the real hermetic path, `stop_for_shutdown()`
    // must clear local ownership and durably stop the run (Armed/Running ->
    // Stopped) — never leave the durable run dangling as Armed/Running with
    // no local owner.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn cleanup_shutdown_from_real_active_run_clears_ownership_and_stops_durably() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("CLEANUP-SHUTDOWN-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");
        let (result, _trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        result.expect("CLEANUP-SHUTDOWN-01: setup must reach Active");
        assert_eq!(state.locally_owned_run_id().await, Some(run_id));

        state.stop_for_shutdown().await;

        assert_eq!(
            state.locally_owned_run_id().await,
            None,
            "CLEANUP-SHUTDOWN-01: shutdown must fully clear local ownership"
        );
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Stopped
        ));

        // Restart after shutdown must succeed too.
        let run_id_2 = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("CLEANUP-SHUTDOWN-01: restart run creation must succeed");
        state.set_hermetic_test_broker_override_for_test(true).await;
        let (restart_result, _trace2) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id_2,
                Some(off_disposition_fixture(run_id_2)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        assert!(
            restart_result.is_ok(),
            "CLEANUP-SHUTDOWN-01: restart after shutdown must succeed: {restart_result:?}"
        );
        state
            .stop_execution_runtime()
            .await
            .expect("CLEANUP-SHUTDOWN-01: cleanup stop must succeed");

        delete_run_and_its_events(&pool, run_id).await;
        delete_run_and_its_events(&pool, run_id_2).await;
    }

    // -------------------------------------------------------------------
    // FAULT-SEAM-01: the `AfterOrchestratorConstruction` seam, now finally
    // exercised end-to-end through a *genuinely successful* orchestrator
    // construction (hermetic broker override) rather than the real
    // `paper_broker_not_execution_path` refusal FA-01 already covers. This
    // seam fires strictly before `arm_run`, so the durable rollback must be
    // `AlreadyNonActive` and the phase must stay `BeforeArm`.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn fault_seam_after_orchestrator_construction_rolls_back_cleanly() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("FAULT-SEAM-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        state
            .set_dynamic_selection_fault_seam_for_test(Some(
                DynamicSelectionLifecycleFaultSeam::AfterOrchestratorConstruction,
            ))
            .await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");

        let (result, trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        state.set_dynamic_selection_fault_seam_for_test(None).await;

        assert!(
            result.is_err(),
            "FAULT-SEAM-01: expected an injected failure"
        );
        let (original, rollback) = match result.unwrap_err() {
            crate::daily_data_readiness::RuntimeStartSequenceError::Effects {
                original,
                rollback,
            } => (original, rollback),
            other => panic!("FAULT-SEAM-01: expected Effects, got {other:?}"),
        };
        assert_eq!(
            original.fault_class,
            "dynamic_selection.fault_seam.after_orchestrator_construction"
        );
        assert_eq!(
            trace,
            vec!["ownership_reserved"],
            "FAULT-SEAM-01: the seam fires inside start_runtime_effects, \
             strictly before local_bundle_committed"
        );
        assert!(matches!(
            rollback.phase_reached,
            crate::daily_data_readiness::RuntimeStartPhase::BeforeArm
        ));
        assert!(matches!(
            rollback.durable,
            crate::daily_data_readiness::DurableRollbackDisposition::AlreadyNonActive
        ));
        assert_eq!(
            rollback.local.leadership_release_outcome,
            Some(Ok(())),
            "FAULT-SEAM-01: a real orchestrator was constructed (leadership \
             acquired) before the seam fired, so rollback must release it \
             exactly once, successfully"
        );
        assert_eq!(state.locally_owned_run_id().await, None);

        delete_run_and_its_events(&pool, run_id).await;
    }

    // -------------------------------------------------------------------
    // FAULT-ARM-01 (R6 row 10, "arm failure"): the durable run is already
    // `Armed` (a real, organic `mqk_db::arm_run` call made before driving
    // the start attempt) — the real `arm_run` inside `start_runtime_effects`
    // then genuinely fails against that pre-existing state, no injected
    // seam. Phase stays `BeforeArm` (arm_run is attempted but fails before
    // completing), so rollback sees the pre-armed row and durably stops it.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn fault_arm_01_real_arm_run_failure_against_already_armed_row_rolls_back_cleanly() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("FAULT-ARM-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");
        mqk_db::arm_run(&pool, run_id)
            .await
            .expect("FAULT-ARM-01: pre-arming the row must succeed");

        let (result, trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;

        assert!(
            result.is_err(),
            "FAULT-ARM-01: expected a real arm_run failure"
        );
        let (original, rollback) = match result.unwrap_err() {
            crate::daily_data_readiness::RuntimeStartSequenceError::Effects {
                original,
                rollback,
            } => (original, rollback),
            other => panic!("FAULT-ARM-01: expected Effects, got {other:?}"),
        };
        assert_eq!(original.fault_class, "start arm_run failed");
        assert_eq!(trace, vec!["ownership_reserved"]);
        assert!(matches!(
            rollback.phase_reached,
            crate::daily_data_readiness::RuntimeStartPhase::BeforeArm
        ));
        assert!(matches!(
            rollback.durable,
            crate::daily_data_readiness::DurableRollbackDisposition::Stopped
        ));
        assert!(!rollback.durable_status_unknown);
        assert_eq!(
            rollback.local.leadership_release_outcome,
            Some(Ok(())),
            "FAULT-ARM-01: a real orchestrator was constructed before the \
             real arm_run failure, so rollback must release its lease"
        );
        assert_eq!(state.locally_owned_run_id().await, None);
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Stopped
        ));

        // Restart after this failure must succeed.
        let restart_run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("FAULT-ARM-01: restart run creation must succeed");
        state.set_hermetic_test_broker_override_for_test(true).await;
        let (restart_result, _trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                restart_run_id,
                Some(off_disposition_fixture(restart_run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        assert!(
            restart_result.is_ok(),
            "FAULT-ARM-01: restart must succeed: {restart_result:?}"
        );
        state
            .stop_execution_runtime()
            .await
            .expect("FAULT-ARM-01: cleanup stop must succeed");

        delete_run_and_its_events(&pool, run_id).await;
        delete_run_and_its_events(&pool, restart_run_id).await;
    }

    // -------------------------------------------------------------------
    // FAULT-BEGIN-01 (R6 row 11, "begin failure"): after a genuine arm_run
    // success, the row is perturbed to `Stopped` via the real `stop_run`
    // (PerturbRunStoppedBeforeBegin) — the real `begin_run` call
    // immediately afterward then genuinely fails against that state.
    // Rollback finds the row already non-Armed/Running (`AlreadyNonActive`)
    // rather than needing its own transition.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn fault_begin_01_real_begin_run_failure_after_perturbed_stop_rolls_back_cleanly() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("FAULT-BEGIN-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        state
            .set_dynamic_selection_fault_seam_for_test(Some(
                DynamicSelectionLifecycleFaultSeam::PerturbRunStoppedBeforeBegin,
            ))
            .await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");

        let (result, trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        state.set_dynamic_selection_fault_seam_for_test(None).await;

        assert!(
            result.is_err(),
            "FAULT-BEGIN-01: expected a real begin_run failure"
        );
        let (original, rollback) = match result.unwrap_err() {
            crate::daily_data_readiness::RuntimeStartSequenceError::Effects {
                original,
                rollback,
            } => (original, rollback),
            other => panic!("FAULT-BEGIN-01: expected Effects, got {other:?}"),
        };
        assert_eq!(original.fault_class, "start begin_run failed");
        assert_eq!(trace, vec!["ownership_reserved"]);
        assert!(matches!(
            rollback.phase_reached,
            crate::daily_data_readiness::RuntimeStartPhase::ArmedBeforeBegin
        ));
        assert!(matches!(
            rollback.durable,
            crate::daily_data_readiness::DurableRollbackDisposition::AlreadyNonActive
        ));
        assert!(!rollback.durable_status_unknown);
        assert_eq!(rollback.local.leadership_release_outcome, Some(Ok(())));
        assert_eq!(state.locally_owned_run_id().await, None);
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Stopped
        ));

        delete_run_and_its_events(&pool, run_id).await;
    }

    // -------------------------------------------------------------------
    // FAULT-HEARTBEAT-01 (R6 row 12, "initial heartbeat failure"): after a
    // genuine begin_run success, the row is perturbed to `Stopped`
    // (PerturbRunStoppedBeforeInitialHeartbeat) — the real initial
    // `heartbeat_run` call immediately afterward then genuinely fails.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn fault_heartbeat_01_real_initial_heartbeat_failure_after_perturbed_stop_rolls_back_cleanly(
    ) {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("FAULT-HEARTBEAT-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        state
            .set_dynamic_selection_fault_seam_for_test(Some(
                DynamicSelectionLifecycleFaultSeam::PerturbRunStoppedBeforeInitialHeartbeat,
            ))
            .await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");

        let (result, trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        state.set_dynamic_selection_fault_seam_for_test(None).await;

        assert!(
            result.is_err(),
            "FAULT-HEARTBEAT-01: expected a real initial heartbeat failure"
        );
        let (original, rollback) = match result.unwrap_err() {
            crate::daily_data_readiness::RuntimeStartSequenceError::Effects {
                original,
                rollback,
            } => (original, rollback),
            other => panic!("FAULT-HEARTBEAT-01: expected Effects, got {other:?}"),
        };
        assert_eq!(original.fault_class, "start initial heartbeat failed");
        assert_eq!(trace, vec!["ownership_reserved"]);
        assert!(matches!(
            rollback.phase_reached,
            crate::daily_data_readiness::RuntimeStartPhase::ArmedBeforeBegin
        ));
        assert!(matches!(
            rollback.durable,
            crate::daily_data_readiness::DurableRollbackDisposition::AlreadyNonActive
        ));
        assert!(!rollback.durable_status_unknown);
        assert_eq!(rollback.local.leadership_release_outcome, Some(Ok(())));
        assert_eq!(state.locally_owned_run_id().await, None);
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Stopped
        ));

        delete_run_and_its_events(&pool, run_id).await;
    }

    // -------------------------------------------------------------------
    // FAULT-POST-TICK-01 (R6 row 16, "post-initial-tick fault seam"): the
    // `AfterRunArmBeginInitialTick` seam, fired after a genuinely successful
    // real arm/begin/initial-tick/post-tick-heartbeat sequence. Phase is
    // `InitialTickCompleted` (past the "tick was called" boundary), so the
    // durable rollback must be `Halted`, not `Stopped` — no evidence a
    // clean retry is safe once tick() has run.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn fault_post_tick_01_seam_after_real_tick_rolls_back_halted() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("FAULT-POST-TICK-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        state
            .set_dynamic_selection_fault_seam_for_test(Some(
                DynamicSelectionLifecycleFaultSeam::AfterRunArmBeginInitialTick,
            ))
            .await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");

        let (result, trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        state.set_dynamic_selection_fault_seam_for_test(None).await;

        assert!(
            result.is_err(),
            "FAULT-POST-TICK-01: expected the injected failure"
        );
        let (original, rollback) = match result.unwrap_err() {
            crate::daily_data_readiness::RuntimeStartSequenceError::Effects {
                original,
                rollback,
            } => (original, rollback),
            other => panic!("FAULT-POST-TICK-01: expected Effects, got {other:?}"),
        };
        assert_eq!(
            original.fault_class,
            "dynamic_selection.fault_seam.after_run_arm_begin_initial_tick"
        );
        assert_eq!(trace, vec!["ownership_reserved"]);
        assert!(matches!(
            rollback.phase_reached,
            crate::daily_data_readiness::RuntimeStartPhase::InitialTickCompleted
        ));
        assert!(matches!(
            rollback.durable,
            crate::daily_data_readiness::DurableRollbackDisposition::Halted
        ));
        assert!(!rollback.durable_status_unknown);
        assert_eq!(rollback.local.leadership_release_outcome, Some(Ok(())));
        assert_eq!(state.locally_owned_run_id().await, None);
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Halted
        ));

        delete_run_and_its_events(&pool, run_id).await;
    }

    // -------------------------------------------------------------------
    // FAULT-POST-COMMIT-01 (R6 row 18 boundary, "immediately after the
    // run-start bundle commit"): the `AfterProcessLocalSelectionCommit`
    // seam fires after the local run-start bundle (including
    // dynamic-selection truth) is already committed to `AppState` —
    // `rollback_local_effects`'s run_id-scoped clear must remove that
    // already-published metadata, not merely a reservation.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn fault_post_commit_01_seam_after_bundle_commit_clears_published_metadata() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("FAULT-POST-COMMIT-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        state
            .set_dynamic_selection_fault_seam_for_test(Some(
                DynamicSelectionLifecycleFaultSeam::AfterProcessLocalSelectionCommit,
            ))
            .await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");

        let (result, trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        state.set_dynamic_selection_fault_seam_for_test(None).await;

        assert!(
            result.is_err(),
            "FAULT-POST-COMMIT-01: expected the injected failure"
        );
        let (original, rollback) = match result.unwrap_err() {
            crate::daily_data_readiness::RuntimeStartSequenceError::Effects {
                original,
                rollback,
            } => (original, rollback),
            other => panic!("FAULT-POST-COMMIT-01: expected Effects, got {other:?}"),
        };
        assert_eq!(
            original.fault_class,
            "dynamic_selection.fault_seam.after_process_local_selection_commit"
        );
        // The bundle IS committed (Starting{run_id, metadata}) before this
        // seam fires, but `advance_run_to_active`'s trace only records
        // `local_bundle_committed` once `start_runtime_effects` itself
        // returns `Ok` — this seam returns `Err`, so the trace stops at
        // reservation.
        assert_eq!(trace, vec!["ownership_reserved"]);
        assert!(matches!(
            rollback.phase_reached,
            crate::daily_data_readiness::RuntimeStartPhase::LocalCommitStarted
        ));
        assert!(matches!(
            rollback.durable,
            crate::daily_data_readiness::DurableRollbackDisposition::Halted
        ));
        assert!(!rollback.durable_status_unknown);
        assert_eq!(rollback.local.leadership_release_outcome, Some(Ok(())));
        assert_eq!(
            state.locally_owned_run_id().await,
            None,
            "FAULT-POST-COMMIT-01: the already-published Starting metadata \
             must be cleared by rollback, not left dangling"
        );
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Halted
        ));

        delete_run_and_its_events(&pool, run_id).await;
    }

    // -------------------------------------------------------------------
    // FAULT-PRE-SPAWN-01 (R6 row 18, "immediately-before-spawn fault
    // seam"): the `ImmediatelyBeforeLoopSpawn` seam fires inside
    // `spawn_loop`, after `start_runtime_effects` already returned `Ok`
    // (so the trace does reach `local_bundle_committed`) but before the
    // execution loop is ever spawned — no task, no barrier, no Active
    // transition.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn fault_pre_spawn_01_seam_before_loop_spawn_never_spawns_a_task() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("FAULT-PRE-SPAWN-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        state
            .set_dynamic_selection_fault_seam_for_test(Some(
                DynamicSelectionLifecycleFaultSeam::ImmediatelyBeforeLoopSpawn,
            ))
            .await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");

        let (result, trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        state.set_dynamic_selection_fault_seam_for_test(None).await;

        assert!(
            result.is_err(),
            "FAULT-PRE-SPAWN-01: expected the injected failure"
        );
        let (original, rollback) = match result.unwrap_err() {
            crate::daily_data_readiness::RuntimeStartSequenceError::Effects {
                original,
                rollback,
            } => (original, rollback),
            other => panic!("FAULT-PRE-SPAWN-01: expected Effects, got {other:?}"),
        };
        assert_eq!(
            original.fault_class,
            "dynamic_selection.fault_seam.immediately_before_loop_spawn"
        );
        assert_eq!(
            trace,
            vec!["ownership_reserved", "local_bundle_committed"],
            "FAULT-PRE-SPAWN-01: start_runtime_effects succeeded (bundle \
             committed); loop_spawned must never fire"
        );
        assert!(matches!(
            rollback.phase_reached,
            crate::daily_data_readiness::RuntimeStartPhase::LocalCommitStarted
        ));
        assert!(matches!(
            rollback.durable,
            crate::daily_data_readiness::DurableRollbackDisposition::Halted
        ));
        assert!(!rollback.durable_status_unknown);
        assert_eq!(rollback.local.leadership_release_outcome, Some(Ok(())));
        assert_eq!(state.locally_owned_run_id().await, None);
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Halted
        ));

        delete_run_and_its_events(&pool, run_id).await;
    }

    // -------------------------------------------------------------------
    // RUN-LINK-01 (R6 row 9 boundary, positive path): driving
    // `advance_run_to_active` with a real `readiness_link` exercises the
    // real `persist_run_linked_readiness_evidence` write and the
    // `run_link_event_persisted` trace tag — previously untested by any
    // `real_production_effects_matrix_tests` case (every other test passes
    // `readiness_link=None`). The genuine *failure* of this exact write
    // (row 9's other half) is not exercised here — see the final handoff
    // for why (no FK constraint on `sys_autonomous_session_events.run_id`,
    // an upsert `on conflict (id) do nothing`, and no existing test-only
    // override at this exact call site inside `advance_run_to_active`).
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn run_link_01_real_readiness_link_persists_and_reaches_active() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("RUN-LINK-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");
        let evaluation_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"phase7a.run_link_01.evaluation_id",
        );
        let linked_at_utc = Utc::now();

        let (result, trace) = state
            .drive_production_start_effects_with_readiness_link_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
                Some((evaluation_id, linked_at_utc)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;

        assert!(result.is_ok(), "RUN-LINK-01: expected success: {result:?}");
        assert_eq!(
            trace,
            vec![
                "ownership_reserved",
                "run_link_event_persisted",
                "local_bundle_committed",
                "loop_spawned"
            ],
            "RUN-LINK-01: run_link_event_persisted must fire between \
             reservation and local bundle commit"
        );
        assert_eq!(state.locally_owned_run_id().await, Some(run_id));

        state
            .stop_execution_runtime()
            .await
            .expect("RUN-LINK-01: cleanup stop must succeed");
        delete_run_and_its_events(&pool, run_id).await;
    }

    // -------------------------------------------------------------------
    // TASK-PANIC-01 (R6 row 21, "spawned-task panic/join failure"): a run
    // reaches genuine `Active` through the real hermetic path, then its
    // spawned task panics immediately after the startup barrier releases.
    // `stop_execution_runtime` must surface this as structured degraded
    // truth (join failure -> `note_local_runtime_degraded`), never a false
    // clean `Idle`, and a subsequent restart must still succeed.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn task_panic_01_spawned_task_panic_surfaces_as_degraded_not_idle() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("TASK-PANIC-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        state.set_execution_loop_panic_for_test(true);
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");

        let (result, _trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        result.expect("TASK-PANIC-01: setup must reach Active before the task panics");

        // Give the spawned task a moment to actually panic after the
        // barrier release (it does so before any ticker/economic work, so
        // this is generous, not load-bearing timing).
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        state.set_execution_loop_panic_for_test(false);

        // By the time `stop_execution_runtime` runs, the panicked task is
        // already finished — `reap_finished_execution_loop` (called first,
        // at the top of `stop_execution_runtime`) reaps it directly and
        // surfaces the join failure itself, before `clear_currently_owned_
        // local_runtime`'s own join-failure path would ever get a chance to.
        let stop_result = state.stop_execution_runtime().await;
        assert!(
            stop_result.is_err(),
            "TASK-PANIC-01: a joined panic must be reported as an error, not silently absorbed"
        );
        let err = stop_result.unwrap_err();
        assert_eq!(
            err.fault_class(),
            "loop join failed",
            "TASK-PANIC-01: unexpected fault class: {err:?}"
        );

        // Degraded truth, not a clean Idle.
        match &*state.runtime_ownership.lock().await {
            crate::state::LocalRuntimeOwnership::Degraded {
                run_id: degraded_run_id,
                ..
            } => {
                assert_eq!(*degraded_run_id, run_id);
            }
            other => panic!(
                "TASK-PANIC-01: expected Degraded after a panicked task's join failure, got {other:?}"
            ),
        }

        // Clear the local Degraded slot the same way a real operator
        // recovery would before attempting a new run.
        {
            let mut lock = state.runtime_ownership.lock().await;
            *lock = crate::state::LocalRuntimeOwnership::Idle;
        }
        mqk_db::clear_halted_run(&pool, run_id).await.ok();
        mqk_db::stop_run(&pool, run_id).await.ok();
        let run_id_2 = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("TASK-PANIC-01: restart run creation must succeed");
        state.set_hermetic_test_broker_override_for_test(true).await;
        let (restart_result, _trace2) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id_2,
                Some(off_disposition_fixture(run_id_2)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        // A panicked task never runs its own release-leadership cleanup
        // (a genuine Rust panic skips ordinary code, only `Drop` runs) —
        // the DB-side runtime lease this attempt's orchestrator held is
        // therefore still live, held by a now-dead in-memory holder, until
        // it naturally expires. This is a real, pre-existing property of
        // `mqk_runtime`'s lease (out of this patch's scope to change) —
        // the correct, fail-closed outcome is that an immediate restart is
        // refused, not silently allowed to race a lease it cannot prove is
        // safe to take over. Proving this refusal (rather than assuming a
        // clean restart) is itself the honest completion of this row.
        match restart_result {
            Err(crate::daily_data_readiness::RuntimeStartSequenceError::Effects {
                original,
                ..
            }) => {
                assert_eq!(
                    original.fault_class, "runtime.start_refused.service_unavailable",
                    "TASK-PANIC-01: expected a lease-unavailable refusal, got {original:?}"
                );
            }
            other => panic!(
                "TASK-PANIC-01: expected the still-live lease to refuse an immediate \
                 restart, got {other:?}"
            ),
        }

        delete_run_and_its_events(&pool, run_id).await;
        delete_run_and_its_events(&pool, run_id_2).await;
    }

    // -------------------------------------------------------------------
    // FAULT-ROLLBACK-QUERY-01 (R6 row 27, "durable rollback query
    // failure"): the run row is genuinely deleted before `arm_run` is
    // attempted — `rollback_failed_start_attempt`'s own `fetch_run` call
    // then organically fails (`RowNotFound`), never a fabricated query
    // error. Must surface as `QueryFailed` with `durable_status_unknown =
    // true`, and `RollbackOutcome::is_degraded()` must reflect it.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn fault_rollback_query_01_deleted_run_row_makes_fetch_run_fail_organically() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("FAULT-ROLLBACK-QUERY-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        state
            .set_dynamic_selection_fault_seam_for_test(Some(
                DynamicSelectionLifecycleFaultSeam::DeleteRunRowBeforeArm,
            ))
            .await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");

        let (result, trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        state.set_dynamic_selection_fault_seam_for_test(None).await;

        assert!(
            result.is_err(),
            "FAULT-ROLLBACK-QUERY-01: expected the injected failure"
        );
        let (original, rollback) = match result.unwrap_err() {
            crate::daily_data_readiness::RuntimeStartSequenceError::Effects {
                original,
                rollback,
            } => (original, rollback),
            other => panic!("FAULT-ROLLBACK-QUERY-01: expected Effects, got {other:?}"),
        };
        assert_eq!(
            original.fault_class,
            "dynamic_selection.fault_seam.delete_run_row_before_arm"
        );
        assert_eq!(trace, vec!["ownership_reserved"]);
        assert!(matches!(
            rollback.durable,
            crate::daily_data_readiness::DurableRollbackDisposition::QueryFailed
        ));
        assert!(
            rollback.durable_status_unknown,
            "FAULT-ROLLBACK-QUERY-01: a failed fetch_run means the durable status is \
             genuinely unknown, not safely assumed clean"
        );
        assert!(
            rollback.is_degraded(),
            "FAULT-ROLLBACK-QUERY-01: is_degraded() must reflect durable_status_unknown"
        );
        assert_eq!(state.locally_owned_run_id().await, None);

        // The row was deleted; nothing further to clean up.
    }

    // -------------------------------------------------------------------
    // BARRIER-TRUTH-01 (R6 items 19/20, "explicitly inspect barrier-cancel
    // and Active-install-failure paths"): a forced `install_active_runtime`
    // conflict after a genuinely successful `start_runtime_effects` (real
    // arm/begin/heartbeat/tick, real orchestrator leadership acquired,
    // orchestrator already moved into the spawned task) must still release
    // that leadership lease exactly once, and the spawned task must never
    // proceed past the startup barrier (no detached task, no economic
    // work). This test is what surfaced the gap fixed in this same patch:
    // `spawn_execution_loop`'s two pre-barrier exit branches previously
    // only `drop_outside_async_context(orchestrator)`-ed without releasing
    // the lease first.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn barrier_truth_install_conflict_releases_leadership_and_cancels_barrier() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("BARRIER-TRUTH-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        state.set_install_active_runtime_conflict_for_test(true);
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");

        let (result, trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        state.set_install_active_runtime_conflict_for_test(false);

        assert!(
            result.is_err(),
            "BARRIER-TRUTH-01: expected the forced install conflict to fail the start"
        );
        let (original, rollback) = match result.unwrap_err() {
            crate::daily_data_readiness::RuntimeStartSequenceError::Effects {
                original,
                rollback,
            } => (original, rollback),
            other => panic!("BARRIER-TRUTH-01: expected Effects, got {other:?}"),
        };
        assert_eq!(
            original.fault_class,
            "runtime.start_refused.spawn_loop_install_failed"
        );
        assert_eq!(
            trace,
            vec!["ownership_reserved", "local_bundle_committed"],
            "BARRIER-TRUTH-01: loop_spawned must never fire — install_active_runtime \
             failed before spawn_loop could report success"
        );
        // start_runtime_effects fully succeeded (real arm/begin/tick), so the
        // durable run reached Armed/Running before this failure — and
        // `LocalCommitStarted` is not `cleanly_stoppable()` (tick() was
        // already called), so the durable rollback must be `Halted`, not
        // `Stopped`.
        assert!(matches!(
            rollback.phase_reached,
            crate::daily_data_readiness::RuntimeStartPhase::LocalCommitStarted
        ));
        assert!(matches!(
            rollback.durable,
            crate::daily_data_readiness::DurableRollbackDisposition::Halted
        ));
        assert!(!rollback.durable_status_unknown);
        // The core barrier-leadership-truth proof: leadership was acquired
        // (a real orchestrator was constructed and moved into the spawned
        // task before install failed) and must be released exactly once —
        // by the spawned task's pre-barrier exit path, not by
        // `rollback_local_effects` (which finds `self.orchestrator` already
        // taken and has nothing left to release directly).
        assert_eq!(
            rollback.local.leadership_release_outcome, None,
            "BARRIER-TRUTH-01: by the time install_active_runtime fails, the \
             orchestrator has already been moved into the spawned task — \
             rollback_local_effects itself has nothing left to release \
             (the release happens task-side instead; see \
             task_side_leadership_release_outcome below)"
        );
        // PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 1: the
        // task-side release truth (previously only a `tracing::warn!` line,
        // never reaching this struct) must now be visible here as genuine
        // structured proof the lease was released exactly once.
        assert_eq!(
            rollback.local.task_side_leadership_release_outcome,
            Some(Ok(())),
            "BARRIER-TRUTH-01: the spawned task's own pre-barrier exit \
             branch (stop-before-barrier-release) must have released the \
             lease successfully, and that outcome must reach rollback \
             truth instead of being discarded by install_active_runtime's \
             join"
        );
        assert_eq!(
            rollback.local.task_side_join_outcome,
            Some(Ok(())),
            "BARRIER-TRUTH-01: install_active_runtime's join of the \
             stopped task must be recorded, not discarded via `let _ = \
             handle.join_handle.await;`"
        );
        assert_eq!(
            state.pre_barrier_leadership_release_count_for_test(),
            1,
            "BARRIER-TRUTH-01: the pre-barrier release path must fire \
             exactly once for this attempt"
        );

        assert_eq!(
            state.locally_owned_run_id().await,
            None,
            "BARRIER-TRUTH-01: the failed install must not leave a stuck \
             Starting/Active slot"
        );
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Halted
        ));

        delete_run_and_its_events(&pool, run_id).await;
    }

    // -------------------------------------------------------------------
    // LEADERSHIP-RELEASE-FAILURE-01 (R6 item 29): a forced leadership-
    // release failure on an otherwise-successful start's rollback path
    // must surface in structured degraded truth (`Some(Err(_))`), never be
    // silently discarded, and must not prevent the rest of rollback
    // (reservation release) from completing.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn leadership_release_failure_is_recorded_not_discarded() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("LEADERSHIP-RELEASE-FAILURE-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        state.set_leadership_release_failure_for_test(true);
        state
            .set_dynamic_selection_fault_seam_for_test(Some(
                DynamicSelectionLifecycleFaultSeam::AfterOrchestratorConstruction,
            ))
            .await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");

        let (result, trace) = state
            .drive_production_start_effects_for_test(
                pool.clone(),
                run_id,
                Some(off_disposition_fixture(run_id)),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        state.set_leadership_release_failure_for_test(false);
        state.set_dynamic_selection_fault_seam_for_test(None).await;

        assert!(result.is_err());
        let (_original, rollback) = match result.unwrap_err() {
            crate::daily_data_readiness::RuntimeStartSequenceError::Effects {
                original,
                rollback,
            } => (original, rollback),
            other => panic!("LEADERSHIP-RELEASE-FAILURE-01: expected Effects, got {other:?}"),
        };
        assert_eq!(trace, vec!["ownership_reserved"]);
        let Some(Err(release_err)) = rollback.local.leadership_release_outcome else {
            panic!(
                "LEADERSHIP-RELEASE-FAILURE-01: a real orchestrator was \
                 constructed (leadership acquired), and the release was \
                 forced to fail — the outcome must be `Some(Err(_))`, never \
                 discarded: {:?}",
                rollback.local.leadership_release_outcome
            );
        };
        assert_eq!(release_err, "test-injected leadership release failure");
        // The release failure must not prevent the rest of rollback
        // (reservation release) from completing.
        assert_eq!(
            state.locally_owned_run_id().await,
            None,
            "LEADERSHIP-RELEASE-FAILURE-01: reservation must still be \
             released even though the leadership-release sub-step failed"
        );

        delete_run_and_its_events(&pool, run_id).await;
    }

    // =====================================================================
    // TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 Blocker 3: required
    // full-loop proof. Every test below drives a genuinely spawned
    // `spawn_execution_loop` task carrying a real `DynamicPaperEnforced`
    // authority (never `Legacy`) through the exact production
    // `ProductionRuntimeStartEffects`/`spawn_loop` path — the same hermetic
    // broker/DB mechanism SUCCESS-01/SUCCESS-02 above already use, with the
    // one prior gap closed: `dispatch_authority` is now actually populated
    // instead of always silently defaulting to `Legacy`.
    //
    // Call-site truth (Bundle 6/Bundle 5/cap #6/submission ordering and
    // counts, legacy-invocation count, host-invocation identity, and
    // provenance-at-submission) is observed via `AppState::loop_call_trace`
    // — an ordered event log pushed at the exact real call sites in
    // `state/loop_runner.rs` and `state.rs` (see the `#[cfg(test)]`-gated
    // `loop_call_trace_push_for_test` calls there). No fake coordinator, no
    // fake economic pipeline: these are the real production call sites,
    // only observed.
    // =====================================================================

    fn blocker3_valid_evidence(score_micros: i64) -> mqk_portfolio::SelectionCandidateEvidence {
        let decimal = {
            let micros = score_micros.max(0);
            let int_part = micros / 1_000_000;
            let frac_part = micros % 1_000_000;
            mqk_portfolio::canonicalize_decimal_token(&format!("{int_part}.{frac_part:06}"))
                .expect("valid fixture token")
        };
        mqk_portfolio::SelectionCandidateEvidence {
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
            config_identity_verified: true,
            durable_config_fingerprint: Some("c".repeat(64)),
            current_config_fingerprint: Some("c".repeat(64)),
            registry_enabled: true,
            plugin_instantiable: true,
            timeframe_matches: true,
            data_ready: true,
            canonical_score_decimal: Some(decimal),
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

    /// A real, coherent two-symbol mixed-timeframe `PaperEnforcedAllowed`
    /// plan and its matching `DynamicPaperEnforced` dispatch authority --
    /// built through the exact same pure selector and builder every other
    /// module in this crate uses (never hand-faked). `plan.context.run_id`
    /// is set to `run_id` itself, satisfying the start-authority check.
    fn blocker3_plan_and_authority(
        run_id: uuid::Uuid,
        aapl_symbol: &str,
        msft_symbol: &str,
    ) -> (
        mqk_portfolio::DynamicSelectionPlan,
        crate::dynamic_selection_dispatch_authority::RuntimeStrategyDispatchAuthority,
    ) {
        let context = mqk_portfolio::DynamicSelectionContext {
            run_id: run_id.to_string(),
            schema_version: mqk_portfolio::DYNAMIC_SELECTION_SCHEMA_VERSION.to_string(),
            configured_mode: mqk_portfolio::DynamicSelectionMode::PaperEnforced,
            effective_mode: mqk_portfolio::DynamicSelectionMode::PaperEnforced,
            live_lock_applied: false,
            source_kind: "env_single_symbol_fallback".to_string(),
            source_identity: "env".to_string(),
            market_date: "2026-08-01".to_string(),
        };
        let candidates = vec![
            mqk_portfolio::SelectionCandidateInput {
                symbol: aapl_symbol.to_string(),
                strategy_id: "intraday_scalper".to_string(),
                timeframe_secs: 300,
                evidence: blocker3_valid_evidence(500_000),
            },
            mqk_portfolio::SelectionCandidateInput {
                symbol: msft_symbol.to_string(),
                strategy_id: "volatility_breakout".to_string(),
                timeframe_secs: 3600,
                evidence: blocker3_valid_evidence(700_000),
            },
        ];
        let plan = mqk_portfolio::compute_dynamic_selection_plan(
            context,
            &[aapl_symbol.to_string(), msft_symbol.to_string()],
            &candidates,
        );
        assert_eq!(
            plan.selected_count(),
            2,
            "blocker3 fixture plan must select both candidates: {plan:?}"
        );
        let keys = crate::dynamic_selection_start_gate::selected_host_pool_keys(&plan);
        let host_pool = crate::dynamic_selection_host_pool::DynamicSelectionHostPool::build(&keys)
            .expect("blocker3 fixture plan must build a real host pool");
        let plan_id =
            crate::dynamic_selection_dispatch_authority::derive_dynamic_selection_plan_id(&plan);
        let authority = crate::dynamic_selection_dispatch_authority::build_dynamic_paper_enforced_dispatch_authority(
            run_id, &plan, plan_id, host_pool,
        )
        .expect("blocker3 fixture plan must build a coherent dispatch authority");
        (plan, authority)
    }

    fn blocker3_paper_enforced_allowed_disposition_fixture(
        run_id: uuid::Uuid,
        plan: &mqk_portfolio::DynamicSelectionPlan,
    ) -> DynamicSelectionRuntimeState {
        let plan_id =
            crate::dynamic_selection_dispatch_authority::derive_dynamic_selection_plan_id(plan);
        DynamicSelectionRuntimeState {
            run_id,
            disposition:
                crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::PaperEnforcedAllowed,
            configured_mode: mqk_portfolio::DynamicSelectionMode::PaperEnforced,
            effective_mode: mqk_portfolio::DynamicSelectionMode::PaperEnforced,
            live_lock_applied: false,
            plan: Some(Arc::new(plan.clone())),
            plan_id: Some(plan_id),
            selected_pairs: crate::dynamic_selection_start_gate::selected_host_pool_keys(plan),
            host_pool_present: true,
            reasons: Vec::new(),
            approved_for_live: false,
            evidence_persisted: false,
            evidence_validation_state: None,
        }
    }

    /// Seeds `n` completed bars for `symbol`/`timeframe`, `interval_secs`
    /// apart, ending `now - 60` (comfortably fresh), with the given
    /// oldest-first close prices — enough real signal for the real
    /// `intraday_scalper`/`volatility_breakout` engines to compute a
    /// genuine non-flat target, never a fabricated decision.
    async fn blocker3_seed_bars(
        pool: &PgPool,
        symbol: &str,
        timeframe: &str,
        interval_secs: i64,
        closes_oldest_first: &[i64],
    ) {
        let _ = sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
            .bind(symbol)
            .bind(timeframe)
            .execute(pool)
            .await;
        let now = Utc::now().timestamp();
        let last_end_ts = now - 60;
        let n = closes_oldest_first.len() as i64;
        for (i, close_micros) in closes_oldest_first.iter().enumerate() {
            let end_ts = last_end_ts - (n - 1 - i as i64) * interval_secs;
            sqlx::query(
                r#"
                insert into md_bars (
                  symbol, timeframe, end_ts, open_micros, high_micros, low_micros,
                  close_micros, volume, is_complete, provider_id, provider_source,
                  provider_symbol, ingest_mode, ingested_at
                ) values ($1,$2,$3,$4,$4,$4,$4,1000,true,
                          'blocker3_test','blocker3_test',$1,'historical_sync',now())
                on conflict do nothing
                "#,
            )
            .bind(symbol)
            .bind(timeframe)
            .bind(end_ts)
            .bind(close_micros)
            .execute(pool)
            .await
            .expect("blocker3 seed bar insert failed");
        }
    }

    /// `intraday_scalper`: exactly `LOOKBACK` (5) bars, flat except the
    /// final (newest) bar +50bps above the anchor -- comfortably past the
    /// engine's real 20bps threshold, so `direction = +1` (buy) is a
    /// genuine strategy output, not a fabricated one.
    async fn blocker3_seed_aapl_buy_signal_bars(pool: &PgPool, symbol: &str) {
        blocker3_seed_bars(
            pool,
            symbol,
            "5m",
            300,
            &[
                100_000_000,
                100_000_000,
                100_000_000,
                100_000_000,
                100_500_000,
            ],
        )
        .await;
    }

    /// `volatility_breakout`: `LOOKBACK + 1` (21) bars, 20 flat at the same
    /// close, then a final (newest) bar breaking above that prior window's
    /// max -- a genuine `direction = +1` (buy) engine output.
    async fn blocker3_seed_msft_buy_signal_bars(pool: &PgPool, symbol: &str) {
        let mut closes = vec![100_000_000i64; 20];
        closes.push(105_000_000);
        blocker3_seed_bars(pool, symbol, "1H", 3600, &closes).await;
    }

    async fn blocker3_cleanup_symbol_evidence(pool: &PgPool, symbol: &str) {
        let _ = sqlx::query("delete from md_bars where symbol = $1")
            .bind(symbol)
            .execute(pool)
            .await;
        let _ = sqlx::query("delete from strategy_signal_evaluations where symbol = $1")
            .bind(symbol)
            .execute(pool)
            .await;
        let _ = sqlx::query("delete from oms_outbox where order_json->>'symbol' = $1")
            .bind(symbol)
            .execute(pool)
            .await;
    }

    /// The single-row `runtime_leader_lease` is process-scoped but
    /// persisted in the (shared, local) test DB, so a prior test process
    /// that ended abnormally (panicked task, killed test binary) can leave
    /// a non-expired lease row that blocks every subsequent orchestrator
    /// leadership acquisition attempt, in any later test run, until the
    /// lease's own TTL naturally elapses. Clearing it here before each
    /// Blocker 3 test is exactly analogous to
    /// `clear_any_preexisting_active_daemon_run`'s existing pattern for the
    /// `runs` table above -- hermetic test-setup hygiene, never a change to
    /// the real acquire/release/refresh contract itself.
    async fn blocker3_clear_stale_runtime_leader_lease(pool: &PgPool) {
        let _ = sqlx::query("delete from runtime_leader_lease")
            .execute(pool)
            .await;
    }

    // -------------------------------------------------------------------
    // BLOCKER3-01 (requirement 1): `PaperEnforcedAllowed` reaches `Active`
    // through the real hermetic path carrying a genuine
    // `DynamicPaperEnforced` authority.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn blocker3_01_paper_enforced_allowed_reaches_active_with_dynamic_paper_enforced_authority(
    ) {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("BLOCKER3-01").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        blocker3_clear_stale_runtime_leader_lease(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");
        let (plan, authority) = blocker3_plan_and_authority(run_id, "PB301AAPL", "PB301MSFT");
        assert!(
            authority.is_dynamic_paper_enforced(),
            "fixture authority must genuinely be DynamicPaperEnforced"
        );

        let (result, trace) = state
            .drive_production_start_effects_with_dispatch_authority_for_test(
                pool.clone(),
                run_id,
                Some(blocker3_paper_enforced_allowed_disposition_fixture(
                    run_id, &plan,
                )),
                Some(authority),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;

        assert!(
            result.is_ok(),
            "BLOCKER3-01: expected a genuine PaperEnforcedAllowed success: {result:?}"
        );
        assert_eq!(
            trace,
            vec![
                "ownership_reserved",
                "local_bundle_committed",
                "loop_spawned"
            ]
        );
        assert_eq!(state.locally_owned_run_id().await, Some(run_id));

        let snapshot = state
            .dynamic_selection_runtime_snapshot()
            .await
            .expect("BLOCKER3-01: dynamic-selection metadata must be committed");
        assert!(matches!(
            snapshot.disposition,
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::PaperEnforcedAllowed
        ));
        assert!(
            snapshot.host_pool_present,
            "BLOCKER3-01: PaperEnforcedAllowed must retain a host pool"
        );
        assert_eq!(snapshot.selected_pairs.len(), 2);

        state
            .stop_execution_runtime()
            .await
            .expect("BLOCKER3-01: cleanup stop must succeed");
        delete_run_and_its_events(&pool, run_id).await;
        blocker3_cleanup_symbol_evidence(&pool, "PB301AAPL").await;
        blocker3_cleanup_symbol_evidence(&pool, "PB301MSFT").await;
    }

    // -------------------------------------------------------------------
    // BLOCKER3-02 (requirements 2-10): one real deposited bar input
    // produces one real loop tick that dispatches both selected hosts
    // exactly once, invokes the legacy native bootstrap zero times, calls
    // Bundle 6 exactly once, then Bundle 5 exactly once (in that order),
    // checks cap #6 after Bundle 5 and before submission, submits each
    // accepted decision exactly once, and the exact provenance envelope
    // reaches the canonical submission call site unchanged.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn blocker3_02_one_tick_dispatches_bundle6_then_bundle5_then_cap6_then_submission_with_intact_provenance(
    ) {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("BLOCKER3-02").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        blocker3_clear_stale_runtime_leader_lease(&pool).await;
        let aapl = "PB302AAPL";
        let msft = "PB302MSFT";
        blocker3_cleanup_symbol_evidence(&pool, aapl).await;
        blocker3_cleanup_symbol_evidence(&pool, msft).await;
        blocker3_seed_aapl_buy_signal_bars(&pool, aapl).await;
        blocker3_seed_msft_buy_signal_bars(&pool, msft).await;

        let state = hermetic_paper_state(&pool);
        state.set_per_symbol_bar_staleness_secs_for_test(Some(3600));
        state.loop_call_trace_clear_for_test();
        state.set_hermetic_test_broker_override_for_test(true).await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");
        let (plan, authority) = blocker3_plan_and_authority(run_id, aapl, msft);

        let (result, _trace) = state
            .drive_production_start_effects_with_dispatch_authority_for_test(
                pool.clone(),
                run_id,
                Some(blocker3_paper_enforced_allowed_disposition_fixture(
                    run_id, &plan,
                )),
                Some(authority),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        result.expect("BLOCKER3-02: setup must reach Active");

        // Deposit the one pending bar input after Active/barrier-release so
        // it cannot be raced away by the ticker's immediate first (empty)
        // tick, then wait for a real subsequent 1-second tick to consume it.
        state
            .deposit_strategy_bar_input(StrategyBarInput {
                now_tick: 1,
                end_ts: Utc::now().timestamp(),
                limit_price: Some(100_000_000),
                qty: 0,
            })
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(2_200)).await;

        let trace = state.loop_call_trace_snapshot_for_test();
        assert!(
            !trace.is_empty(),
            "BLOCKER3-02: the real loop tick must have produced at least \
             one recorded call-site event; got an empty trace"
        );

        // Requirement 4: legacy native-bootstrap invocation count is zero.
        assert_eq!(
            trace
                .iter()
                .filter(|e| e.starts_with("legacy_dispatch:"))
                .count(),
            0,
            "BLOCKER3-02: the legacy native bootstrap must never be invoked \
             while DynamicPaperEnforced is active: {trace:?}"
        );

        // Requirement 3: two selected bindings each invoke their exact host
        // exactly once.
        let host_calls: Vec<&String> = trace
            .iter()
            .filter(|e| e.starts_with("host_call:"))
            .collect();
        assert_eq!(
            host_calls.len(),
            2,
            "BLOCKER3-02: exactly two host invocations expected: {trace:?}"
        );
        assert!(host_calls
            .iter()
            .any(|e| e.as_str() == format!("host_call:{aapl}:intraday_scalper")));
        assert!(host_calls
            .iter()
            .any(|e| e.as_str() == format!("host_call:{msft}:volatility_breakout")));

        // Requirements 5/6/7: Bundle 6 exactly once, Bundle 5 exactly once,
        // Bundle 6 strictly before Bundle 5.
        let bundle6_idx = trace.iter().position(|e| e == "bundle6");
        let bundle5_idx = trace.iter().position(|e| e == "bundle5");
        assert_eq!(
            trace.iter().filter(|e| e.as_str() == "bundle6").count(),
            1,
            "BLOCKER3-02: Bundle 6 must be called exactly once: {trace:?}"
        );
        assert_eq!(
            trace.iter().filter(|e| e.as_str() == "bundle5").count(),
            1,
            "BLOCKER3-02: Bundle 5 must be called exactly once: {trace:?}"
        );
        let (Some(b6), Some(b5)) = (bundle6_idx, bundle5_idx) else {
            panic!("BLOCKER3-02: both bundle6 and bundle5 must appear: {trace:?}");
        };
        assert!(
            b6 < b5,
            "BLOCKER3-02: Bundle 6 must precede Bundle 5: {trace:?}"
        );

        // Requirement 8: cap #6 checks occur strictly after Bundle 5 and
        // strictly before every submission.
        let cap6_positions: Vec<usize> = trace
            .iter()
            .enumerate()
            .filter(|(_, e)| e.starts_with("cap6_check:"))
            .map(|(i, _)| i)
            .collect();
        assert!(
            !cap6_positions.is_empty(),
            "BLOCKER3-02: at least one cap #6 check expected: {trace:?}"
        );
        assert!(
            cap6_positions.iter().all(|&i| i > b5),
            "BLOCKER3-02: every cap #6 check must be after Bundle 5: {trace:?}"
        );
        let submit_positions: Vec<usize> = trace
            .iter()
            .enumerate()
            .filter(|(_, e)| e.starts_with("submit:"))
            .map(|(i, _)| i)
            .collect();
        for &cap_idx in &cap6_positions {
            assert!(
                submit_positions.iter().any(|&s| s > cap_idx)
                    || submit_positions.is_empty()
                    || cap_idx > *submit_positions.last().unwrap(),
                "BLOCKER3-02: cap #6 must sit between Bundle 5 and its \
                 corresponding submission decision: {trace:?}"
            );
        }

        // Requirement 9: no accepted decision is submitted twice (distinct
        // decision_ids across every submit event).
        let submit_events: Vec<&String> =
            trace.iter().filter(|e| e.starts_with("submit:")).collect();
        assert!(
            !submit_events.is_empty(),
            "BLOCKER3-02: at least one decision must have been accepted \
             and submitted (both seeded symbols were built to produce a \
             genuine buy signal): {trace:?}"
        );
        let mut seen_decision_ids = std::collections::HashSet::new();
        for e in &submit_events {
            // "submit:{decision_id}:{symbol}"
            let decision_id = e.split(':').nth(1).expect("submit event shape");
            assert!(
                seen_decision_ids.insert(decision_id.to_string()),
                "BLOCKER3-02: decision_id {decision_id} was submitted more \
                 than once: {trace:?}"
            );
        }

        // Requirement 10: the exact provenance envelope (run_id, plan_id,
        // symbol, strategy_id, timeframe_secs) reaches submission unchanged
        // from the moment it was derived.
        let derive_events: Vec<&String> = trace
            .iter()
            .filter(|e| e.starts_with("derive_provenance:"))
            .collect();
        let submit_provenance_events: Vec<&String> = trace
            .iter()
            .filter(|e| e.starts_with("submit_provenance:"))
            .collect();
        assert!(
            !derive_events.is_empty() && !submit_provenance_events.is_empty(),
            "BLOCKER3-02: provenance must be recorded at both derivation \
             and submission: {trace:?}"
        );
        for submit_event in &submit_provenance_events {
            let fields: Vec<&str> = submit_event
                .trim_start_matches("submit_provenance:")
                .split(':')
                .collect();
            let symbol = fields[0];
            let matching_derive = format!(
                "derive_provenance:{}",
                submit_event.trim_start_matches("submit_provenance:")
            );
            assert!(
                derive_events.iter().any(|d| d.as_str() == matching_derive),
                "BLOCKER3-02: submitted provenance for {symbol} does not \
                 match any derived provenance byte-for-byte: submit={submit_event} \
                 derive_events={derive_events:?}"
            );
        }

        // Durable evidence corroborates the trace: both symbols' selected
        // strategies actually ran and produced a journaled row (Blocker 2
        // authority proof reused here as additional real-world corroboration).
        let rows = mqk_db::fetch_recent_strategy_signal_evaluations(&pool, 50)
            .await
            .expect("fetch_recent_strategy_signal_evaluations failed");
        assert!(
            rows.iter()
                .any(|r| r.symbol == aapl && r.strategy_id == "intraday_scalper"),
            "BLOCKER3-02: AAPL journal row missing or wrong strategy"
        );
        assert!(
            rows.iter()
                .any(|r| r.symbol == msft && r.strategy_id == "volatility_breakout"),
            "BLOCKER3-02: MSFT journal row missing or wrong strategy"
        );

        state
            .stop_execution_runtime()
            .await
            .expect("BLOCKER3-02: cleanup stop must succeed");
        delete_run_and_its_events(&pool, run_id).await;
        blocker3_cleanup_symbol_evidence(&pool, aapl).await;
        blocker3_cleanup_symbol_evidence(&pool, msft).await;
    }

    // -------------------------------------------------------------------
    // BLOCKER3-03 (requirement 11): stop from a real DynamicPaperEnforced
    // Active run drops the authority/pool (structural: the spawned task is
    // the sole owner, so dropping the task drops the whole authority
    // including its host pool -- no separate clear call exists for the
    // pool itself, exactly like the accepted Part 1 design) and clears
    // exact-run dynamic-selection metadata.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn blocker3_03_stop_from_real_paper_enforced_active_run_clears_authority_metadata() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("BLOCKER3-03").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        blocker3_clear_stale_runtime_leader_lease(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");
        let (plan, authority) = blocker3_plan_and_authority(run_id, "PB303AAPL", "PB303MSFT");
        let (result, _trace) = state
            .drive_production_start_effects_with_dispatch_authority_for_test(
                pool.clone(),
                run_id,
                Some(blocker3_paper_enforced_allowed_disposition_fixture(
                    run_id, &plan,
                )),
                Some(authority),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        result.expect("BLOCKER3-03: setup must reach Active");
        assert_eq!(state.locally_owned_run_id().await, Some(run_id));

        let stop_result = state.stop_execution_runtime().await;
        assert!(
            stop_result.is_ok(),
            "BLOCKER3-03: stop must succeed: {stop_result:?}"
        );
        assert_eq!(
            state.locally_owned_run_id().await,
            None,
            "BLOCKER3-03: stop must fully clear local ownership"
        );
        assert!(
            state.dynamic_selection_runtime_snapshot().await.is_none(),
            "BLOCKER3-03: stop must clear dynamic-selection metadata, \
             including the DynamicPaperEnforced authority's own run/plan \
             binding"
        );

        delete_run_and_its_events(&pool, run_id).await;
        blocker3_cleanup_symbol_evidence(&pool, "PB303AAPL").await;
        blocker3_cleanup_symbol_evidence(&pool, "PB303MSFT").await;
    }

    // -------------------------------------------------------------------
    // BLOCKER3-04 (requirements 12 + 16): halt from a real
    // DynamicPaperEnforced Active run clears authority metadata and durably
    // halts the run; a subsequent restart builds a genuinely fresh
    // run/plan/pool -- no prior host state or provenance survives.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn blocker3_04_halt_clears_authority_and_restart_builds_fresh_run_plan_pool() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("BLOCKER3-04").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        blocker3_clear_stale_runtime_leader_lease(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");
        let (plan, authority) = blocker3_plan_and_authority(run_id, "PB304AAPL", "PB304MSFT");
        let (result, _trace) = state
            .drive_production_start_effects_with_dispatch_authority_for_test(
                pool.clone(),
                run_id,
                Some(blocker3_paper_enforced_allowed_disposition_fixture(
                    run_id, &plan,
                )),
                Some(authority),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        result.expect("BLOCKER3-04: setup must reach Active");

        let halt_result = state.halt_execution_runtime().await;
        assert!(
            halt_result.is_ok(),
            "BLOCKER3-04: halt must succeed: {halt_result:?}"
        );
        assert_eq!(state.locally_owned_run_id().await, None);
        assert!(
            state.dynamic_selection_runtime_snapshot().await.is_none(),
            "BLOCKER3-04: halt must clear the DynamicPaperEnforced authority's \
             run/plan metadata"
        );
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Halted
        ));

        // Restart: a genuinely fresh run_id, plan, and host pool -- built
        // from scratch by a second, independent call to
        // `blocker3_plan_and_authority`, never reusing the first authority
        // value (which was moved into, and dropped with, the first spawned
        // task).
        mqk_db::clear_halted_run(&pool, run_id)
            .await
            .expect("BLOCKER3-04: operator halt-clear must succeed");
        {
            let mut integrity = state.integrity.write().await;
            integrity.disarmed = false;
            integrity.halted = false;
        }
        let run_id_2 = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("BLOCKER3-04: restart run creation must succeed");
        assert_ne!(
            run_id_2, run_id,
            "BLOCKER3-04: restart must mint a fresh run_id"
        );
        let (plan_2, authority_2) =
            blocker3_plan_and_authority(run_id_2, "PB304AAPL2", "PB304MSFT2");
        state.set_hermetic_test_broker_override_for_test(true).await;
        let (restart_result, _trace2) = state
            .drive_production_start_effects_with_dispatch_authority_for_test(
                pool.clone(),
                run_id_2,
                Some(blocker3_paper_enforced_allowed_disposition_fixture(
                    run_id_2, &plan_2,
                )),
                Some(authority_2),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        assert!(
            restart_result.is_ok(),
            "BLOCKER3-04: restart after halt must succeed: {restart_result:?}"
        );
        let snapshot_2 = state
            .dynamic_selection_runtime_snapshot()
            .await
            .expect("BLOCKER3-04: restart must commit fresh dynamic-selection metadata");
        assert_eq!(
            snapshot_2.run_id, run_id_2,
            "BLOCKER3-04: restart's committed metadata must name the fresh \
             run_id, never the halted run's"
        );
        assert_eq!(
            snapshot_2.selected_pairs,
            crate::dynamic_selection_start_gate::selected_host_pool_keys(&plan_2),
            "BLOCKER3-04: restart's selected pairs must come from the fresh \
             plan, never the halted run's prior bindings"
        );

        state
            .stop_execution_runtime()
            .await
            .expect("BLOCKER3-04: cleanup stop must succeed");
        delete_run_and_its_events(&pool, run_id).await;
        delete_run_and_its_events(&pool, run_id_2).await;
        blocker3_cleanup_symbol_evidence(&pool, "PB304AAPL").await;
        blocker3_cleanup_symbol_evidence(&pool, "PB304MSFT").await;
        blocker3_cleanup_symbol_evidence(&pool, "PB304AAPL2").await;
        blocker3_cleanup_symbol_evidence(&pool, "PB304MSFT2").await;
    }

    // -------------------------------------------------------------------
    // BLOCKER3-05 (requirement 13): shutdown from a real
    // DynamicPaperEnforced Active run clears authority metadata and durably
    // stops the run; restart after shutdown succeeds.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn blocker3_05_shutdown_clears_authority_and_restart_succeeds() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("BLOCKER3-05").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        blocker3_clear_stale_runtime_leader_lease(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");
        let (plan, authority) = blocker3_plan_and_authority(run_id, "PB305AAPL", "PB305MSFT");
        let (result, _trace) = state
            .drive_production_start_effects_with_dispatch_authority_for_test(
                pool.clone(),
                run_id,
                Some(blocker3_paper_enforced_allowed_disposition_fixture(
                    run_id, &plan,
                )),
                Some(authority),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        result.expect("BLOCKER3-05: setup must reach Active");

        state.stop_for_shutdown().await;

        assert_eq!(state.locally_owned_run_id().await, None);
        assert!(
            state.dynamic_selection_runtime_snapshot().await.is_none(),
            "BLOCKER3-05: shutdown must clear the DynamicPaperEnforced \
             authority's run/plan metadata"
        );
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_id)
                .await
                .expect("fetch_run")
                .status,
            mqk_db::RunStatus::Stopped
        ));

        let run_id_2 = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("BLOCKER3-05: restart run creation must succeed");
        let (plan_2, authority_2) =
            blocker3_plan_and_authority(run_id_2, "PB305AAPL2", "PB305MSFT2");
        state.set_hermetic_test_broker_override_for_test(true).await;
        let (restart_result, _trace2) = state
            .drive_production_start_effects_with_dispatch_authority_for_test(
                pool.clone(),
                run_id_2,
                Some(blocker3_paper_enforced_allowed_disposition_fixture(
                    run_id_2, &plan_2,
                )),
                Some(authority_2),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        assert!(
            restart_result.is_ok(),
            "BLOCKER3-05: restart after shutdown must succeed: {restart_result:?}"
        );

        state
            .stop_execution_runtime()
            .await
            .expect("BLOCKER3-05: cleanup stop must succeed");
        delete_run_and_its_events(&pool, run_id).await;
        delete_run_and_its_events(&pool, run_id_2).await;
        blocker3_cleanup_symbol_evidence(&pool, "PB305AAPL").await;
        blocker3_cleanup_symbol_evidence(&pool, "PB305MSFT").await;
        blocker3_cleanup_symbol_evidence(&pool, "PB305AAPL2").await;
        blocker3_cleanup_symbol_evidence(&pool, "PB305MSFT2").await;
    }

    // -------------------------------------------------------------------
    // BLOCKER3-06 (requirements 14 + 15): the spawned task panics
    // immediately after the startup barrier releases while carrying a real
    // DynamicPaperEnforced authority. `stop_execution_runtime` (which reaps
    // the finished loop first) must surface this as structured degraded
    // truth, never a false clean Idle, and must leave no detached
    // selected-host authority (the panicked task's own `dispatch_authority`
    // — including its host pool — was dropped when the task unwound; no
    // other owner ever held a clone).
    //
    // This test deliberately does NOT chain an immediate restart (unlike
    // BLOCKER3-04/05, which prove requirement 16 via halt/shutdown): a
    // panicked orchestrator's own `runtime_leader_lease` row is only
    // released by its natural TTL expiry (the orchestrator instance that
    // held it was dropped mid-unwind, never explicitly releasing it) — a
    // real, pre-existing, out-of-scope safety property of the runtime
    // leadership lease (unrelated to dynamic selection/provenance/signal
    // journal), not something this patch touches. Requirement 16 (restart
    // builds fresh run/plan/pool) is already fully proven by BLOCKER3-04
    // and BLOCKER3-05 above via a clean halt/shutdown exit, which releases
    // the lease immediately.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn blocker3_06_panic_reports_degraded_and_leaves_no_detached_authority() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("BLOCKER3-06").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        blocker3_clear_stale_runtime_leader_lease(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        state.set_execution_loop_panic_for_test(true);
        let run_id = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run creation must succeed");
        let (plan, authority) = blocker3_plan_and_authority(run_id, "PB306AAPL", "PB306MSFT");

        let (result, _trace) = state
            .drive_production_start_effects_with_dispatch_authority_for_test(
                pool.clone(),
                run_id,
                Some(blocker3_paper_enforced_allowed_disposition_fixture(
                    run_id, &plan,
                )),
                Some(authority),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        result.expect("BLOCKER3-06: setup must reach Active before the task panics");

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        state.set_execution_loop_panic_for_test(false);

        let stop_result = state.stop_execution_runtime().await;
        assert!(
            stop_result.is_err(),
            "BLOCKER3-06: a joined panic must be reported as an error, not \
             silently absorbed"
        );
        assert_eq!(stop_result.unwrap_err().fault_class(), "loop join failed");

        match &*state.runtime_ownership.lock().await {
            crate::state::LocalRuntimeOwnership::Degraded {
                run_id: degraded_run_id,
                ..
            } => {
                assert_eq!(*degraded_run_id, run_id);
            }
            other => panic!(
                "BLOCKER3-06: expected Degraded after a panicked \
                 DynamicPaperEnforced task's join failure, got {other:?}"
            ),
        }
        assert!(
            state.dynamic_selection_runtime_snapshot().await.is_none(),
            "BLOCKER3-06: no detached selected-host authority/metadata may \
             survive a panicked task"
        );

        {
            let mut lock = state.runtime_ownership.lock().await;
            *lock = crate::state::LocalRuntimeOwnership::Idle;
        }
        mqk_db::clear_halted_run(&pool, run_id).await.ok();
        mqk_db::stop_run(&pool, run_id).await.ok();

        delete_run_and_its_events(&pool, run_id).await;
        blocker3_cleanup_symbol_evidence(&pool, "PB306AAPL").await;
        blocker3_cleanup_symbol_evidence(&pool, "PB306MSFT").await;
        blocker3_cleanup_symbol_evidence(&pool, "PB306AAPL2").await;
        blocker3_cleanup_symbol_evidence(&pool, "PB306MSFT2").await;
    }

    // -------------------------------------------------------------------
    // BLOCKER3-07 (requirement 17): run A cannot clear run B. Run A reaches
    // real Active with a genuine DynamicPaperEnforced authority; a
    // conflicting second start attempt (run B) is refused by the real
    // ownership-reservation seam; run A's dynamic-selection metadata
    // (authority binding included) must be completely unchanged afterward.
    // -------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread")]
    async fn blocker3_07_run_a_cleanup_cannot_clear_run_b_dynamic_paper_enforced_state() {
        let _g = db_test_lock().lock().await;
        let Some(pool) = db_pool_or_skip("BLOCKER3-07").await else {
            return;
        };
        clear_any_preexisting_active_daemon_run(&pool).await;
        blocker3_clear_stale_runtime_leader_lease(&pool).await;
        let state = hermetic_paper_state(&pool);
        state.set_hermetic_test_broker_override_for_test(true).await;
        let run_a = state
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("run A creation must succeed");
        let (plan_a, authority_a) = blocker3_plan_and_authority(run_a, "PB307AAPL", "PB307MSFT");
        let (result_a, _trace_a) = state
            .drive_production_start_effects_with_dispatch_authority_for_test(
                pool.clone(),
                run_a,
                Some(blocker3_paper_enforced_allowed_disposition_fixture(
                    run_a, &plan_a,
                )),
                Some(authority_a),
            )
            .await;
        state
            .set_hermetic_test_broker_override_for_test(false)
            .await;
        result_a.expect("BLOCKER3-07: run A setup must reach Active");
        assert_eq!(state.locally_owned_run_id().await, Some(run_a));

        // Run B: a distinct run_id attempting to start while run A still
        // genuinely owns the local runtime slot -- refused by the real
        // `reserve_local_ownership` conflict path, never a fake stub.
        let run_b = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            format!("mqk-daemon.blocker3.07.run_b.{run_a}").as_bytes(),
        );
        mqk_db::insert_run(
            &pool,
            &mqk_db::NewRun {
                run_id: run_b,
                engine_id: "mqk-daemon".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: Utc::now(),
                git_hash: "UNKNOWN".to_string(),
                config_hash: "blocker3-07-test".to_string(),
                config_json: serde_json::json!({"test": "blocker3-07"}),
                host_fingerprint: "blocker3-07-test-host".to_string(),
            },
        )
        .await
        .expect("run B row must insert");
        let (plan_b, authority_b) = blocker3_plan_and_authority(run_b, "PB307AAPL2", "PB307MSFT2");
        let (result_b, trace_b) = state
            .drive_production_start_effects_with_dispatch_authority_for_test(
                pool.clone(),
                run_b,
                Some(blocker3_paper_enforced_allowed_disposition_fixture(
                    run_b, &plan_b,
                )),
                Some(authority_b),
            )
            .await;
        assert!(
            result_b.is_err(),
            "BLOCKER3-07: run B must be refused while run A still owns the slot"
        );
        assert_eq!(
            trace_b,
            Vec::<&str>::new(),
            "BLOCKER3-07: run B must fail at the very first step (ownership \
             reservation), before any local bundle is ever committed"
        );

        // Run A's committed dynamic-selection state must be completely
        // unchanged by run B's refused attempt and its rollback.
        let snapshot_after = state
            .dynamic_selection_runtime_snapshot()
            .await
            .expect("BLOCKER3-07: run A's metadata must still be present");
        assert_eq!(
            snapshot_after.run_id, run_a,
            "BLOCKER3-07: run A's dynamic-selection metadata must still name \
             run A, never run B"
        );
        assert_eq!(
            snapshot_after.selected_pairs,
            crate::dynamic_selection_start_gate::selected_host_pool_keys(&plan_a),
            "BLOCKER3-07: run A's selected pairs must be exactly its own \
             original binding set, never run B's"
        );
        assert!(snapshot_after.host_pool_present);
        assert_eq!(state.locally_owned_run_id().await, Some(run_a));
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_a)
                .await
                .expect("fetch_run run_a")
                .status,
            mqk_db::RunStatus::Running
        ));
        assert!(matches!(
            mqk_db::fetch_run(&pool, run_b)
                .await
                .expect("fetch_run run_b")
                .status,
            mqk_db::RunStatus::Created
        ));

        state
            .stop_execution_runtime()
            .await
            .expect("BLOCKER3-07: cleanup stop must succeed");
        delete_run_and_its_events(&pool, run_a).await;
        delete_run_and_its_events(&pool, run_b).await;
        blocker3_cleanup_symbol_evidence(&pool, "PB307AAPL").await;
        blocker3_cleanup_symbol_evidence(&pool, "PB307MSFT").await;
        blocker3_cleanup_symbol_evidence(&pool, "PB307AAPL2").await;
        blocker3_cleanup_symbol_evidence(&pool, "PB307MSFT2").await;
    }
}

// ---------------------------------------------------------------------------
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: `build_dynamic_selection_
// start_snapshot` disposition-determination unit proof.
//
// These exercise the private method directly (in-crate, same module tree),
// deliberately bypassing every earlier unrelated start gate (WS continuity,
// daily-data-readiness, artifact intake, capital policy) that a full
// `start_execution_runtime()` call would also have to satisfy for
// Paper+Alpaca — none of which this patch touches or needs to re-prove.
// None of these tests require a database connection or Alpaca credentials:
// `build_dynamic_selection_start_snapshot` never constructs a broker.
//
// Full end-to-end lifecycle wiring (atomic commit, fault seams, cleanup on
// every exit path, real loop-ownership-conflict cleanup) is proven
// separately in `tests/scenario_bundle7_phase7a_lifecycle_wiring_01.rs`
// against Paper+Paper (Off disposition, zero Alpaca dependency) using only
// the public API.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod dynamic_selection_start_snapshot_tests {
    use super::*;
    use tokio::sync::Mutex as TokioMutex;

    /// Serializes every test in this module that touches the process-global
    /// `MQK_STRATEGY_SYMBOL` / `MQK_STRATEGY_IDS` / `MQK_STRATEGY_MD_TIMEFRAME`
    /// / `MQK_DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE` env vars — mirrors the
    /// `env_lock()` convention already used by the daily-data-readiness
    /// start-gate scenario tests. Scoped to this compiled test binary
    /// (`cargo test -p mqk-daemon --lib`) only.
    ///
    /// BUNDLE-7-PHASE-7A-SINGLE-FROZEN-FLEET-AUTHORITY-CLOSURE: delegates to
    /// the one crate-wide `strategy_fleet_env_test_lock` so this module's
    /// `MQK_STRATEGY_SYMBOL`/`MQK_STRATEGY_IDS`/`MQK_STRATEGY_MD_TIMEFRAME`
    /// mutations can never race `frozen_strategy_fleet_tests`'s (below) or
    /// `multi_symbol_config`'s own env-mutating tests, which mutate the same
    /// process-global env vars.
    fn env_lock() -> &'static TokioMutex<()> {
        crate::state::shared_test_locks::strategy_fleet_env_test_lock()
    }

    fn clear_dynamic_selection_env() {
        std::env::remove_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
        );
        std::env::remove_var("MQK_STRATEGY_SYMBOL");
        std::env::remove_var("MQK_STRATEGY_IDS");
        std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    }

    /// Off: broker=Paper (never Alpaca) live-locks to Off regardless of the
    /// configured mode env var — proves the live lock AND that Off requires
    /// no DB and no `MQK_STRATEGY_SYMBOL`/`MQK_STRATEGY_IDS` configuration
    /// (zero selection I/O).
    #[tokio::test]
    async fn off_via_live_lock_needs_no_db_and_no_symbol_config() {
        let _guard = env_lock().lock().await;
        clear_dynamic_selection_env();
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "shadow",
        );

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Paper,
        ));
        assert!(state.db.is_none(), "precondition: no DB configured");

        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.off_live_lock",
        );
        let snapshot = StartAttemptAuthoritySnapshot::resolve(&state).await;
        let result = state
            .build_dynamic_selection_start_snapshot(run_id, &snapshot)
            .await;
        clear_dynamic_selection_env();

        let (outcome, dispatch_authority) = result.expect("Off must never refuse the start");
        assert_eq!(
            outcome.disposition,
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::Off
        );
        assert_eq!(
            outcome.configured_mode,
            mqk_portfolio::DynamicSelectionMode::Shadow,
            "configured_mode must reflect the raw env value, even though it was demoted"
        );
        assert_eq!(
            outcome.effective_mode,
            mqk_portfolio::DynamicSelectionMode::Off
        );
        assert!(
            outcome.live_lock_applied,
            "broker=Paper must trigger the live lock"
        );
        assert!(outcome.plan.is_none());
        assert!(outcome.selected_pairs.is_empty());
        assert!(!outcome.host_pool_present);
        assert!(outcome.reasons.is_empty());
        assert!(!outcome.approved_for_live);
        assert_eq!(outcome.run_id, run_id);
        assert!(!dispatch_authority.is_dynamic_paper_enforced());
    }

    /// Off: mode env var unset (the honest default) on Paper+Alpaca — Off
    /// without any live-lock demotion (`live_lock_applied` must be `false`
    /// since `configured_mode` was already `Off`, not forced there).
    #[tokio::test]
    async fn off_by_default_configuration_is_not_live_locked() {
        let _guard = env_lock().lock().await;
        clear_dynamic_selection_env();

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.off_default",
        );
        let snapshot = StartAttemptAuthoritySnapshot::resolve(&state).await;
        let (outcome, _dispatch_authority) = state
            .build_dynamic_selection_start_snapshot(run_id, &snapshot)
            .await
            .expect("Off must never refuse the start");

        assert_eq!(
            outcome.disposition,
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::Off
        );
        assert_eq!(
            outcome.configured_mode,
            mqk_portfolio::DynamicSelectionMode::Off
        );
        assert!(!outcome.live_lock_applied);
    }

    /// Shadow, `MultiSymbolRuntimeConfig` resolution itself fails
    /// (`MQK_STRATEGY_SYMBOL` unset) — Shadow must never block the run: the
    /// truthful failure is recorded as `ShadowInvalid` with no plan/pool.
    #[tokio::test]
    async fn shadow_with_unresolvable_config_is_invalid_not_blocking() {
        let _guard = env_lock().lock().await;
        clear_dynamic_selection_env();
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "shadow",
        );

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.shadow_no_config",
        );
        let snapshot = StartAttemptAuthoritySnapshot::resolve(&state).await;
        let result = state
            .build_dynamic_selection_start_snapshot(run_id, &snapshot)
            .await;
        clear_dynamic_selection_env();

        let (outcome, dispatch_authority) =
            result.expect("Shadow must never refuse the start, even on its own config failure");
        assert_eq!(
            outcome.disposition,
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::ShadowInvalid
        );
        assert!(outcome.plan.is_none());
        assert!(!outcome.host_pool_present);
        assert!(outcome.selected_pairs.is_empty());
        assert!(!dispatch_authority.is_dynamic_paper_enforced());
        assert_eq!(outcome.reasons.len(), 1);
        assert!(matches!(
            &outcome.reasons[0],
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateReason::PlanInvalid { truth_state }
                if truth_state.starts_with("multi_symbol_config_unavailable:")
        ));
    }

    /// PaperEnforced, `MultiSymbolRuntimeConfig` resolution fails — must
    /// refuse the whole start (`Err`), before any run advancement.
    #[tokio::test]
    async fn paper_enforced_with_unresolvable_config_is_refused() {
        let _guard = env_lock().lock().await;
        clear_dynamic_selection_env();
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "paper_enforced",
        );

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.paper_enforced_no_config",
        );
        let snapshot = StartAttemptAuthoritySnapshot::resolve(&state).await;
        let result = state
            .build_dynamic_selection_start_snapshot(run_id, &snapshot)
            .await;
        clear_dynamic_selection_env();

        let err = result.expect_err("PaperEnforced must refuse when config cannot be resolved");
        assert_eq!(
            err.fault_class(),
            "runtime.start_refused.dynamic_selection_config_unavailable"
        );
    }

    /// Shadow, `MultiSymbolRuntimeConfig` resolves successfully but no DB is
    /// configured — the evaluator's own `DbUnavailable` path fires
    /// (distinct from this function's own config-resolution failure above).
    #[tokio::test]
    async fn shadow_with_resolved_config_and_no_db_is_invalid_via_db_unavailable() {
        let _guard = env_lock().lock().await;
        clear_dynamic_selection_env();
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "shadow",
        );
        std::env::set_var("MQK_STRATEGY_SYMBOL", "AAPL");
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        assert!(state.db.is_none(), "precondition: no DB configured");
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.shadow_db_unavail",
        );
        let snapshot = StartAttemptAuthoritySnapshot::resolve(&state).await;
        let result = state
            .build_dynamic_selection_start_snapshot(run_id, &snapshot)
            .await;
        clear_dynamic_selection_env();

        let (outcome, _dispatch_authority) = result.expect("Shadow must never refuse the start");
        assert_eq!(
            outcome.disposition,
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::ShadowInvalid
        );
        assert!(outcome.plan.is_none());
        assert!(!outcome.host_pool_present);
        assert_eq!(
            outcome.reasons,
            vec![
                crate::dynamic_selection_start_gate::DynamicSelectionStartGateReason::DbUnavailable
            ]
        );
    }

    /// PaperEnforced, config resolves but no DB — the evaluator's own
    /// `DbUnavailable` refusal fires (distinct from this function's own
    /// config-resolution refusal above).
    #[tokio::test]
    async fn paper_enforced_with_resolved_config_and_no_db_is_refused_via_db_unavailable() {
        let _guard = env_lock().lock().await;
        clear_dynamic_selection_env();
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "paper_enforced",
        );
        std::env::set_var("MQK_STRATEGY_SYMBOL", "AAPL");
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.paper_enforced_db_unavail",
        );
        let snapshot = StartAttemptAuthoritySnapshot::resolve(&state).await;
        let result = state
            .build_dynamic_selection_start_snapshot(run_id, &snapshot)
            .await;
        clear_dynamic_selection_env();

        let err = result.expect_err("PaperEnforced must refuse when DB is unavailable");
        assert_eq!(
            err.fault_class(),
            "runtime.start_refused.dynamic_selection_paper_enforced_refused"
        );
    }

    /// Live deployments resolve Off regardless of configured mode, and
    /// cannot store a pool (Off never builds one) — proves the live lock for
    /// the `LiveCapital` boundary specifically, not just non-Alpaca brokers.
    #[tokio::test]
    async fn live_capital_resolves_off_and_stores_no_pool() {
        let _guard = env_lock().lock().await;
        clear_dynamic_selection_env();
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "paper_enforced",
        );

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::LiveCapital,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.live_capital_off",
        );
        let snapshot = StartAttemptAuthoritySnapshot::resolve(&state).await;
        let result = state
            .build_dynamic_selection_start_snapshot(run_id, &snapshot)
            .await;
        clear_dynamic_selection_env();

        let (outcome, dispatch_authority) = result.expect("Off must never refuse the start");
        assert_eq!(
            outcome.disposition,
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::Off
        );
        assert!(outcome.live_lock_applied);
        assert!(!outcome.host_pool_present);
        assert!(outcome.plan.is_none());
        assert!(!dispatch_authority.is_dynamic_paper_enforced());
    }
}

// ---------------------------------------------------------------------------
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: cleanup-contract proof.
//
// `commit_dynamic_selection_runtime_state` is `pub(crate)`, so these must be
// in-crate tests. They deliberately bypass the credential-gated start path
// entirely (see `tests/scenario_bundle7_phase7a_lifecycle_wiring_01.rs` for
// why a real `start_execution_runtime()` success cannot be driven here
// without Alpaca credentials this patch must not load): a fixture
// `DynamicSelectionRuntimeState` is committed directly, then the real public
// `stop_execution_runtime`/`halt_execution_runtime`/`stop_for_shutdown`/
// `reap_finished_execution_loop` functions are exercised to prove they clear
// it — genuine proof of the cleanup wiring this patch adds, independent of
// whichever disposition produced the committed state.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod dynamic_selection_cleanup_contract_tests {
    use super::*;

    fn fixture_off_state(run_id: uuid::Uuid) -> DynamicSelectionRuntimeState {
        DynamicSelectionRuntimeState {
            run_id,
            disposition:
                crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::Off,
            configured_mode: mqk_portfolio::DynamicSelectionMode::Off,
            effective_mode: mqk_portfolio::DynamicSelectionMode::Off,
            live_lock_applied: false,
            plan: None,
            plan_id: None,
            selected_pairs: Vec::new(),
            host_pool_present: false,
            reasons: Vec::new(),
            approved_for_live: false,
            evidence_persisted: false,
            evidence_validation_state: None,
        }
    }

    /// Test 10: `stop_execution_runtime` clears committed dynamic-selection
    /// state AND every other economic mirror before it ever reaches a
    /// DB-dependent step that can fail. `commit_dynamic_selection_runtime_
    /// state`'s fixture establishes a real (trivial) `Active` loop, so stop
    /// genuinely stops+joins it, then reaches `db_pool()?` — which errors
    /// with no DB configured, mirroring the halt test below. The clear
    /// itself, proven by the mirror assertions, already happened by then.
    #[tokio::test]
    async fn stop_clears_committed_state_with_no_active_loop_and_no_db() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.cleanup.stop",
        );
        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_id))
            .await;
        assert!(state.dynamic_selection_runtime_snapshot().await.is_some());
        state
            .plant_accepted_artifact_for_test(Some(AcceptedArtifactProvenance {
                artifact_id: "stop-sentinel".to_string(),
                artifact_type: "sentinel".to_string(),
                stage: "sentinel".to_string(),
                produced_by: "test".to_string(),
            }))
            .await;
        state.plant_day_signal_count_for_test(77);

        let err = state
            .stop_execution_runtime()
            .await
            .expect_err("stop must reach db_pool() once the trivial fixture loop is stopped");
        assert_eq!(
            err.fault_class(),
            "runtime.start_refused.service_unavailable"
        );
        assert!(
            state.dynamic_selection_runtime_snapshot().await.is_none(),
            "stop_execution_runtime must clear dynamic-selection state before the \
             DB-dependent step that can fail"
        );
        assert!(
            state.accepted_artifact_snapshot_for_test().await.is_none(),
            "stop_execution_runtime must clear accepted_artifact too, not only \
             dynamic-selection state"
        );
        assert_eq!(
            state.day_signal_count_snapshot_for_test(),
            0,
            "stop_execution_runtime must clear day_signal_count too"
        );
    }

    /// Test 11: `halt_execution_runtime` clears committed dynamic-selection
    /// state and every other economic mirror *even when the overall call
    /// itself errors* on a missing DB — the clear happens before `db_pool()?`
    /// deliberately, so local authority is disowned the instant an operator
    /// halts, independent of whether the DB bookkeeping step later succeeds.
    #[tokio::test]
    async fn halt_clears_committed_state_even_when_the_call_itself_errors() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.cleanup.halt",
        );
        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_id))
            .await;
        assert!(state.dynamic_selection_runtime_snapshot().await.is_some());
        state
            .plant_accepted_artifact_for_test(Some(AcceptedArtifactProvenance {
                artifact_id: "halt-sentinel".to_string(),
                artifact_type: "sentinel".to_string(),
                stage: "sentinel".to_string(),
                produced_by: "test".to_string(),
            }))
            .await;
        state.plant_day_signal_count_for_test(88);

        let err = state
            .halt_execution_runtime()
            .await
            .expect_err("halt without a configured DB must error at db_pool()");
        assert_eq!(
            err.fault_class(),
            "runtime.start_refused.service_unavailable"
        );
        assert!(
            state.dynamic_selection_runtime_snapshot().await.is_none(),
            "halt_execution_runtime must clear dynamic-selection state before the DB-dependent \
             steps that can fail, not only on a fully successful halt"
        );
        assert!(
            state.accepted_artifact_snapshot_for_test().await.is_none(),
            "halt_execution_runtime must clear accepted_artifact too, before the \
             DB-dependent steps that can fail"
        );
        assert_eq!(state.day_signal_count_snapshot_for_test(), 0);
    }

    /// Test 12: `stop_for_shutdown` clears committed dynamic-selection state
    /// AND every other economic mirror, even with no DB configured — unlike
    /// the pre-existing behavior this patch closes (shutdown previously left
    /// `accepted_artifact`/`native_strategy_bootstrap` stale).
    #[tokio::test]
    async fn stop_for_shutdown_clears_committed_state_with_no_active_loop() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.cleanup.shutdown",
        );
        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_id))
            .await;
        state
            .plant_accepted_artifact_for_test(Some(AcceptedArtifactProvenance {
                artifact_id: "shutdown-sentinel".to_string(),
                artifact_type: "sentinel".to_string(),
                stage: "sentinel".to_string(),
                produced_by: "test".to_string(),
            }))
            .await;
        state.plant_day_signal_count_for_test(99);

        state.stop_for_shutdown().await;
        assert!(
            state.dynamic_selection_runtime_snapshot().await.is_none(),
            "stop_for_shutdown must clear dynamic-selection state"
        );
        assert!(
            state.accepted_artifact_snapshot_for_test().await.is_none(),
            "stop_for_shutdown must now also clear accepted_artifact — closing the \
             pre-existing asymmetry this patch fixes"
        );
        assert_eq!(state.day_signal_count_snapshot_for_test(), 0);
    }

    /// Cleanup is idempotent: clearing twice (or clearing when already
    /// `None`) never panics and always leaves `None`.
    #[tokio::test]
    async fn clear_is_idempotent() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        state.clear_dynamic_selection_runtime_state().await;
        assert!(state.dynamic_selection_runtime_snapshot().await.is_none());

        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.cleanup.idempotent",
        );
        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_id))
            .await;
        state.clear_dynamic_selection_runtime_state().await;
        state.clear_dynamic_selection_runtime_state().await;
        assert!(state.dynamic_selection_runtime_snapshot().await.is_none());
    }

    /// A fresh commit is always a full overwrite, never a merge with a
    /// stale prior value — proves restart-cannot-reuse-stale-state at the
    /// container level, independent of run-creation machinery.
    #[tokio::test]
    async fn commit_always_overwrites_never_merges() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_a = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.cleanup.run_a",
        );
        let run_b = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.cleanup.run_b",
        );

        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_a))
            .await;
        assert_eq!(
            state
                .dynamic_selection_runtime_snapshot()
                .await
                .unwrap()
                .run_id,
            run_a
        );

        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_b))
            .await;
        let after = state.dynamic_selection_runtime_snapshot().await.unwrap();
        assert_eq!(
            after.run_id, run_b,
            "the second commit must fully replace the first"
        );
    }

    /// Test 13: `reap_finished_execution_loop` clears committed
    /// dynamic-selection state AND every other economic mirror when it
    /// reaps a loop that finished on its own (crash/supervisor exit),
    /// independent of `stop_execution_runtime`/`halt_execution_runtime`
    /// ever being called.
    #[tokio::test]
    async fn reap_of_a_finished_loop_clears_committed_state() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.cleanup.reap",
        );

        // BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE: dynamic-
        // selection truth now lives inside ownership metadata, so the
        // fixture and the pre-finished handle are established together as
        // one `Active` binding, rather than via a separate `commit_
        // dynamic_selection_runtime_state` call plus a second, independent
        // `execution_loop` write.
        let (stop_tx, _stop_rx) =
            tokio::sync::watch::channel(crate::state::types::ExecutionLoopCommand::Run);
        let join_handle = tokio::spawn(async {
            crate::state::types::ExecutionLoopExit {
                note: None,
                leadership_release_outcome: None,
            }
        });
        // Let the trivial task actually finish before installing it, so
        // `reap_finished_execution_loop`'s `is_finished()` check takes the
        // "already finished" branch deterministically.
        while !join_handle.is_finished() {
            tokio::task::yield_now().await;
        }
        let handle = crate::state::types::ExecutionLoopHandle {
            run_id,
            stop_tx,
            join_handle,
        };
        let metadata = std::sync::Arc::new(crate::state::types::RunStartMetadata {
            run_id,
            accepted_artifact: None,
            native_bootstrap_present: false,
            dynamic_selection: tokio::sync::RwLock::new(Some(fixture_off_state(run_id))),
            frozen_assignments: Vec::new(),
            frozen_assignments_source: "test_fixture",
            approved_for_live: false,
        });
        *state.runtime_ownership.lock().await =
            crate::state::types::LocalRuntimeOwnership::Active {
                run_id,
                metadata,
                handle,
            };
        state
            .plant_accepted_artifact_for_test(Some(AcceptedArtifactProvenance {
                artifact_id: "reap-sentinel".to_string(),
                artifact_type: "sentinel".to_string(),
                stage: "sentinel".to_string(),
                produced_by: "test".to_string(),
            }))
            .await;
        state.plant_day_signal_count_for_test(66);

        let exit = state
            .reap_finished_execution_loop()
            .await
            .expect("reap must not error");
        assert!(exit.is_some(), "reap must observe the finished loop");
        assert!(
            state.dynamic_selection_runtime_snapshot().await.is_none(),
            "reap_finished_execution_loop must clear dynamic-selection state \
             for a loop that finished on its own"
        );
        assert!(
            state.accepted_artifact_snapshot_for_test().await.is_none(),
            "reap_finished_execution_loop must clear accepted_artifact too"
        );
        assert_eq!(state.day_signal_count_snapshot_for_test(), 0);
    }

    /// The fault-seam get/set primitive round-trips correctly and defaults
    /// to `None` (no seam installed) — the invariant every fault-seam check
    /// inside `start_execution_runtime`/`ProductionRuntimeStartEffects`
    /// depends on.
    #[tokio::test]
    async fn fault_seam_primitive_round_trips_and_defaults_to_none() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        assert!(
            !state
                .dynamic_selection_fault_seam_is(
                    DynamicSelectionLifecycleFaultSeam::AfterRunRowCreation
                )
                .await,
            "a fresh state must have no fault seam installed"
        );

        state
            .set_dynamic_selection_fault_seam_for_test(Some(
                DynamicSelectionLifecycleFaultSeam::AfterRunRowCreation,
            ))
            .await;
        assert!(
            state
                .dynamic_selection_fault_seam_is(
                    DynamicSelectionLifecycleFaultSeam::AfterRunRowCreation
                )
                .await
        );
        assert!(
            !state
                .dynamic_selection_fault_seam_is(
                    DynamicSelectionLifecycleFaultSeam::ImmediatelyBeforeLoopSpawn
                )
                .await,
            "a different seam variant must not match"
        );

        state.set_dynamic_selection_fault_seam_for_test(None).await;
        assert!(
            !state
                .dynamic_selection_fault_seam_is(
                    DynamicSelectionLifecycleFaultSeam::AfterRunRowCreation
                )
                .await,
            "clearing the seam must be respected"
        );
    }

    /// ATOMICITY-SINGLE-SNAPSHOT-REPAIR requirement 4: run_id-scoped
    /// compare-and-clear. A rollback for an *older* run_id must never clear
    /// a *newer* run's already-committed state — the load-bearing property
    /// that lets a slow/late rollback for a failed attempt race safely
    /// against a subsequent successful start within the same process.
    #[tokio::test]
    async fn clear_for_run_never_clears_a_different_newer_run() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_a = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.atomicity.run_a",
        );
        let run_b = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.atomicity.run_b",
        );

        // Run A committed, then Run B supersedes it (the ordinary
        // stop-A/start-B sequence).
        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_a))
            .await;
        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_b))
            .await;
        assert_eq!(
            state
                .dynamic_selection_runtime_snapshot()
                .await
                .unwrap()
                .run_id,
            run_b
        );

        // A late rollback for the now-superseded run A must not clear B.
        state
            .clear_dynamic_selection_runtime_state_for_run(run_a)
            .await;
        assert_eq!(
            state
                .dynamic_selection_runtime_snapshot()
                .await
                .unwrap()
                .run_id,
            run_b,
            "a compare-and-clear for a stale run_id must never clear a newer run's state"
        );

        // The matching run_id does clear it.
        state
            .clear_dynamic_selection_runtime_state_for_run(run_b)
            .await;
        assert!(state.dynamic_selection_runtime_snapshot().await.is_none());
    }

    /// Compare-and-clear against an already-`None` value is a safe no-op
    /// (idempotent), regardless of which run_id is passed.
    #[tokio::test]
    async fn clear_for_run_on_absent_state_is_a_safe_noop() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.atomicity.absent",
        );
        state
            .clear_dynamic_selection_runtime_state_for_run(run_id)
            .await;
        assert!(state.dynamic_selection_runtime_snapshot().await.is_none());
    }
}

// ---------------------------------------------------------------------------
// PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE Part 9: the prior
// temporary `paper_enforced_dispatch_not_wired_refusal` interlock (and its
// dedicated unit-test module) is removed — replaced by the positive
// dispatch-authority guard embedded in `build_dynamic_selection_start_
// snapshot` (which now returns `Ok((state, DynamicPaperEnforced{..}))` for a
// coherent `PaperEnforcedAllowed` plan instead of refusing). That guard's
// pure, directly-testable core is
// `crate::dynamic_selection_dispatch_authority::build_dynamic_paper_
// enforced_dispatch_authority` (see that module's own unit tests) and the
// full end-to-end proof lives in the `dynamic_selection_start_snapshot_tests`
// module above (`live_capital_resolves_off_and_stores_no_pool` and friends,
// each now asserting `!dispatch_authority.is_dynamic_paper_enforced()` for
// every non-`PaperEnforcedAllowed` disposition) and in the Phase 7B closure
// tests (`dynamic_selection_phase7b_dispatch_tests`, below).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A-SINGLE-FROZEN-FLEET-
// AUTHORITY-CLOSURE: `FrozenStrategyFleet` / `StartAttemptAuthoritySnapshot::
// resolve` proof. Pure/in-memory — no DB, no broker, no Alpaca credentials:
// `StartAttemptAuthoritySnapshot::resolve` never touches either.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod frozen_strategy_fleet_tests {
    use super::*;
    use tokio::sync::Mutex as TokioMutex;

    /// Serializes every test in this module that touches the process-global
    /// `MQK_STRATEGY_SYMBOL` / `MQK_STRATEGY_IDS` / `MQK_STRATEGY_MD_TIMEFRAME`
    /// env vars — the same shared, crate-wide lock
    /// `dynamic_selection_start_snapshot_tests`'s `env_lock()` (above) and
    /// `multi_symbol_config`'s own env-mutating tests delegate to, so none of
    /// these modules can race each other under `cargo test`'s default
    /// parallelism.
    fn env_lock() -> &'static TokioMutex<()> {
        crate::state::shared_test_locks::strategy_fleet_env_test_lock()
    }

    fn clear_fleet_env() {
        std::env::remove_var("MQK_STRATEGY_IDS");
        std::env::remove_var("MQK_STRATEGY_SYMBOL");
        std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    }

    /// Requirements 1/2/3/4/9/10 (test scenario 1): an `AppState`-injected
    /// fleet is authoritative even though the process-global
    /// `MQK_STRATEGY_IDS` disagrees, both before *and* after the snapshot is
    /// captured — B1A bootstrap, the legacy single-symbol assignment, and the
    /// dynamic-selection `configured_strategy_ids` binding (which is
    /// literally `&snapshot.frozen_fleet.strategy_ids` in
    /// `build_dynamic_selection_start_snapshot` above) all read the exact
    /// same frozen vector.
    #[tokio::test]
    async fn appstate_fleet_wins_over_a_disagreeing_env_var_before_and_after_capture() {
        let _guard = env_lock().lock().await;
        clear_fleet_env();
        std::env::set_var("MQK_STRATEGY_IDS", "strategy-x");
        std::env::set_var("MQK_STRATEGY_SYMBOL", "AAPL");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        state
            .set_strategy_fleet_for_test(Some(vec![
                crate::state::StrategyFleetEntry {
                    strategy_id: "strategy-a".to_string(),
                },
                crate::state::StrategyFleetEntry {
                    strategy_id: "strategy-b".to_string(),
                },
            ]))
            .await;

        let snapshot = StartAttemptAuthoritySnapshot::resolve(&state).await;

        // requirement 10: mutating MQK_STRATEGY_IDS after snapshot capture
        // cannot alter any field or decision in this start attempt.
        std::env::set_var("MQK_STRATEGY_IDS", "strategy-y");
        clear_fleet_env();

        // requirements 1/2: frozen fleet is the AppState-injected vector,
        // sourced from AppStateSnapshot — never the disagreeing env value.
        assert_eq!(
            snapshot.frozen_fleet.strategy_ids,
            vec!["strategy-a".to_string(), "strategy-b".to_string()]
        );
        assert_eq!(
            snapshot.frozen_fleet.source,
            FrozenStrategyFleetSource::AppStateSnapshot
        );

        // requirement 3: B1A bootstrap consumed the same vector's first
        // entry. "strategy-a" is not a real registered strategy, so the
        // bootstrap fails closed (Failed, not Dormant) — the *attempted*
        // strategy_id proves which vector's first entry it read.
        match &snapshot.native_strategy_bootstrap.outcome {
            mqk_runtime::native_strategy::NativeStrategyBootstrapOutcome::Failed {
                strategy_id,
                ..
            } => assert_eq!(strategy_id, "strategy-a"),
            _ => panic!(
                "expected Failed (strategy-a is not registered); got truth_state={} \
                 — the bootstrap did not consume the frozen fleet",
                snapshot.native_strategy_bootstrap.truth_state()
            ),
        }

        // requirement 4: the legacy single-symbol assignment's strategy_id
        // came from the frozen fleet's first entry, not a second
        // MQK_STRATEGY_IDS read (env was already cleared/mutated above by
        // the time this reads the already-resolved config).
        let cfg = snapshot
            .multi_symbol_config
            .as_ref()
            .expect("legacy single-symbol config must resolve");
        assert_eq!(cfg.symbols.len(), 1);
        assert_eq!(cfg.symbols[0].strategy_id, "strategy-a");
        assert_eq!(cfg.symbols[0].symbol, "AAPL");

        // requirement 2/5: `build_dynamic_selection_start_snapshot`'s
        // `configured_strategy_ids` binding reads `&snapshot.frozen_fleet.
        // strategy_ids` directly (see that function, above) — the same
        // vector asserted above, never a second, independent resolution.
        assert_eq!(
            snapshot.frozen_fleet.strategy_ids,
            vec!["strategy-a".to_string(), "strategy-b".to_string()],
            "dynamic-selection configured_strategy_ids source"
        );
    }

    /// Requirement 2 (test scenario 2): no `AppState` fleet
    /// (`set_strategy_fleet_for_test(None)`) — the one permitted fallback,
    /// exactly one direct `MQK_STRATEGY_IDS` env read, feeds every consumer
    /// the same normalized (split/trimmed/filtered) vector.
    #[tokio::test]
    async fn env_fallback_used_only_when_appstate_fleet_is_genuinely_absent() {
        let _guard = env_lock().lock().await;
        clear_fleet_env();
        std::env::set_var("MQK_STRATEGY_IDS", " strategy-c , strategy-d ,, ");
        std::env::set_var("MQK_STRATEGY_SYMBOL", "MSFT");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1m");

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        // Explicitly force "no AppState fleet" (test seam) independent of
        // whatever MQK_STRATEGY_IDS happened to be at AppState construction.
        state.set_strategy_fleet_for_test(None).await;

        let snapshot = StartAttemptAuthoritySnapshot::resolve(&state).await;
        clear_fleet_env();

        assert_eq!(
            snapshot.frozen_fleet.strategy_ids,
            vec!["strategy-c".to_string(), "strategy-d".to_string()],
            "the one permitted fallback env read must trim/split/filter \
             exactly like the AppState boot-time reader"
        );
        assert_eq!(
            snapshot.frozen_fleet.source,
            FrozenStrategyFleetSource::EnvFallback
        );

        let cfg = snapshot
            .multi_symbol_config
            .as_ref()
            .expect("legacy single-symbol config must resolve");
        assert_eq!(cfg.symbols[0].strategy_id, "strategy-c");

        match &snapshot.native_strategy_bootstrap.outcome {
            mqk_runtime::native_strategy::NativeStrategyBootstrapOutcome::Failed {
                strategy_id,
                ..
            } => assert_eq!(strategy_id, "strategy-c"),
            _ => panic!(
                "expected Failed (strategy-c is not registered); got truth_state={}",
                snapshot.native_strategy_bootstrap.truth_state()
            ),
        }
    }

    /// Requirement 3 (test scenario 3): an empty frozen fleet — whether from
    /// an explicitly-injected empty `AppState` vector or (implicitly) an
    /// absent/empty env fallback — fails closed: Dormant bootstrap, and the
    /// legacy single-symbol path refuses with `MissingStrategyId` rather than
    /// fabricating a strategy id.
    #[tokio::test]
    async fn empty_frozen_fleet_fails_closed_dormant_and_missing_strategy_id() {
        let _guard = env_lock().lock().await;
        clear_fleet_env();
        std::env::set_var("MQK_STRATEGY_SYMBOL", "AAPL");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
        // Deliberately no MQK_STRATEGY_IDS at all.

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        state.set_strategy_fleet_for_test(Some(Vec::new())).await;

        let snapshot = StartAttemptAuthoritySnapshot::resolve(&state).await;
        clear_fleet_env();

        assert!(snapshot.frozen_fleet.strategy_ids.is_empty());
        assert_eq!(
            snapshot.frozen_fleet.source,
            FrozenStrategyFleetSource::AppStateSnapshot,
            "an explicitly-injected empty Vec is still Some(_) — \
             AppStateSnapshot, not EnvFallback"
        );
        assert!(
            snapshot.native_strategy_bootstrap.is_dormant(),
            "an empty frozen fleet must bootstrap Dormant, exactly like the \
             pre-repair None input — never fabricate a strategy id"
        );
        assert_eq!(
            snapshot.multi_symbol_config,
            Err(crate::state::MultiSymbolConfigError::MissingStrategyId),
            "the legacy single-symbol path must fail closed on a missing \
             strategy_id — no watchlist is configured and the frozen fleet \
             is empty"
        );
    }
}
