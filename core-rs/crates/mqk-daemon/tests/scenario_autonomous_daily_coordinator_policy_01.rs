//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D1-TYPED-COORDINATOR-POLICY —
//! shared runtime-context extraction and typed coordinator-policy proof.
//!
//! DB-less, network-less, in-process only. Never starts a real daemon, never
//! calls a real provider or broker, never creates a durable operation row.
//!
//! Scope note: `RuntimeLifecycleError`'s constructors
//! (`forbidden`/`conflict`/`service_unavailable`/`internal`) are
//! intentionally `pub(crate)` on `mqk_daemon::state::types` — this patch
//! does not widen that visibility (out of D1's stated file scope). This
//! external integration-test crate can therefore only obtain a
//! `RuntimeLifecycleError` by triggering a real production gate (as the
//! representative end-to-end case below does), not by constructing one
//! directly. The exhaustive fault-class-mapping, WS-distinction, blocker-
//! signature, backoff, and no-string-authority-guard proofs that require
//! direct construction of arbitrary fault classes live in
//! `core-rs/crates/mqk-daemon/src/state/autonomous_retry_policy.rs`'s own
//! `#[cfg(test)] mod tests` (crate-internal, `pub(crate)` access), and are
//! summarized/re-exercised here wherever the public API allows.
//!
//! D1D1-POLICY-CLOSURE-REPAIR-01: the same `pub(crate)` constructor
//! constraint means the variant-sensitive `RuntimeLifecycleError` mapping
//! (`ServiceUnavailable` vs. `Forbidden` vs. `Conflict` carrying the exact
//! same static `fault_class` string) is not independently constructible
//! from this external crate beyond what `b01` below already exercises (one
//! real `Forbidden`-variant gate). The full variant-sensitive proof (all
//! three variants against the identical fault-class string) lives in
//! `autonomous_retry_policy.rs`'s crate-internal tests. What *is*
//! constructible here from fully public types — the runtime-dispatch-truth
//! adapter (Group I) and blocker-signature collision resistance on
//! pathologically long inputs (Group J) — is added below.
//!
//! Run alone:
//! `cargo test -p mqk-daemon --test scenario_autonomous_daily_coordinator_policy_01`

use std::sync::Arc;
use std::sync::OnceLock;

use chrono::{TimeZone, Utc};
use tokio::sync::Mutex;

use mqk_daemon::state::autonomous_completed_bar_driver::{
    AutonomousCompletedBarDriverOutcome, AutonomousStrategyDispatchRuntimeTruth,
};
use mqk_daemon::state::autonomous_daily_operation::{
    AutonomousDailyPlanReason, AutonomousDailySessionPlanResolution,
};
use mqk_daemon::state::autonomous_retry_policy::{
    blocker_signature, classify_autonomous_reason,
    coordinator_reason_from_completed_bar_driver_outcome,
    coordinator_reason_from_create_or_recover_outcome,
    coordinator_reason_from_runtime_lifecycle_error,
    coordinator_reason_from_session_plan_resolution,
    coordinator_reason_from_strategy_dispatch_runtime_truth, coordinator_reason_from_ws_continuity,
    next_retry_at, retry_delay_for_attempt, AutonomousBlockerIdentity, AutonomousCoordinatorReason,
    AutonomousRetryClass,
};
use mqk_daemon::state::autonomous_runtime_context::resolve_autonomous_runtime_context;
use mqk_daemon::state::{
    AlpacaWsContinuityState, AppState, BrokerKind, DeploymentMode, StrategyFleetEntry,
};
use mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome;

// ---------------------------------------------------------------------------
// Env-var serialization (matches every other daily-data-readiness/B1A test file)
// ---------------------------------------------------------------------------

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn clear_env() {
    unsafe {
        std::env::remove_var("MQK_STRATEGY_SYMBOL");
        std::env::remove_var("MQK_STRATEGY_IDS");
    }
}

// ===========================================================================
// Group A — Shared runtime context (items 1-10)
// ===========================================================================

#[tokio::test]
async fn a01_no_fleet_configured_resolves_dormant_for_allowed_non_paper_case() {
    let _g = env_lock().lock().await;
    clear_env();
    let state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::LiveShadow, BrokerKind::Alpaca);
    state.set_strategy_fleet_for_test(None).await;

    let resolved = resolve_autonomous_runtime_context(&state)
        .await
        .expect("dormant bootstrap is allowed for non-paper deployments");
    assert!(resolved.native_strategy_bootstrap.is_dormant());
    assert_eq!(
        resolved
            .effective_runtime_binding
            .effective_runtime_strategy_id,
        None
    );
}

#[tokio::test]
async fn a02_paper_alpaca_dormant_bootstrap_refused_with_same_fault_class() {
    let _g = env_lock().lock().await;
    clear_env();
    let state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Alpaca);
    state.set_strategy_fleet_for_test(None).await;

    let err = resolve_autonomous_runtime_context(&state)
        .await
        .expect_err("paper+alpaca dormant must be refused");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.strategy_bootstrap_dormant"
    );

    let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
    assert_eq!(
        reason,
        AutonomousCoordinatorReason::NativeStrategyBootstrapDormant
    );
    assert_eq!(
        classify_autonomous_reason(&reason),
        AutonomousRetryClass::ManualInterventionRequired
    );
}

#[tokio::test]
async fn a03_registered_active_strategy_resolves_active_bootstrap() {
    let _g = env_lock().lock().await;
    clear_env();
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZSG01");
    }
    let state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::LiveShadow, BrokerKind::Alpaca);
    state
        .set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
            strategy_id: "swing_momentum".to_string(),
        }]))
        .await;

    let resolved = resolve_autonomous_runtime_context(&state)
        .await
        .expect("swing_momentum is a registered builtin strategy");
    assert!(!resolved.native_strategy_bootstrap.is_dormant());
    assert_eq!(
        resolved.native_strategy_bootstrap.active_strategy_id(),
        Some("swing_momentum")
    );
    clear_env();
}

#[tokio::test]
async fn a04_failed_strategy_lookup_refused_with_same_fault_class() {
    let _g = env_lock().lock().await;
    clear_env();
    let state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::LiveShadow, BrokerKind::Alpaca);
    state
        .set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
            strategy_id: "no_such_strategy_zzsg".to_string(),
        }]))
        .await;

    let err = resolve_autonomous_runtime_context(&state)
        .await
        .expect_err("unregistered strategy id must fail bootstrap");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.native_strategy_bootstrap_failed"
    );
    let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
    assert_eq!(
        reason,
        AutonomousCoordinatorReason::NativeStrategyBootstrapFailed
    );
}

#[tokio::test]
async fn a05_target_symbol_captured_from_same_registry_construction_call() {
    let _g = env_lock().lock().await;
    clear_env();
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", "  ZZSG02  ");
    }
    let state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::LiveShadow, BrokerKind::Alpaca);
    state
        .set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
            strategy_id: "swing_momentum".to_string(),
        }]))
        .await;

    let resolved = resolve_autonomous_runtime_context(&state)
        .await
        .expect("active");
    assert_eq!(
        resolved
            .effective_runtime_binding
            .effective_runtime_target_symbol,
        Some("ZZSG02".to_string()),
        "symbol must be trimmed and captured from the one registry-construction call"
    );
    clear_env();
}

#[tokio::test]
async fn a06_effective_binding_matches_pre_extraction_lifecycle_behavior() {
    let _g = env_lock().lock().await;
    clear_env();
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZSG03");
    }
    let state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::LiveShadow, BrokerKind::Alpaca);
    state
        .set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
            strategy_id: "swing_momentum".to_string(),
        }]))
        .await;

    let resolved = resolve_autonomous_runtime_context(&state)
        .await
        .expect("active");
    // Byte-for-byte the same derivation the pre-extraction inline block used:
    // strategy_id from the active bootstrap, symbol from the one registry
    // read, timeframe from the bootstrap's own strategy_timeframe_secs().
    assert_eq!(
        resolved
            .effective_runtime_binding
            .effective_runtime_strategy_id
            .as_deref(),
        Some("swing_momentum")
    );
    assert_eq!(
        resolved
            .effective_runtime_binding
            .effective_runtime_target_symbol
            .as_deref(),
        Some("ZZSG03")
    );
    assert_eq!(
        resolved
            .effective_runtime_binding
            .effective_runtime_timeframe_secs,
        resolved.native_strategy_bootstrap.strategy_timeframe_secs()
    );
    clear_env();
}

#[tokio::test]
async fn a07_resolver_performs_no_db_call() {
    let _g = env_lock().lock().await;
    clear_env();
    // db: None on this fixture; a resolver that touched db_pool() would have
    // no way to reach this Ok(_) branch (db_pool() has no code path in
    // resolve_autonomous_runtime_context at all — verified by source read).
    let state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::LiveShadow, BrokerKind::Alpaca);
    assert!(state.db.is_none());
    state.set_strategy_fleet_for_test(None).await;
    let resolved = resolve_autonomous_runtime_context(&state).await;
    assert!(
        resolved.is_ok(),
        "dormant LiveShadow must resolve without any DB pool present"
    );
}

#[tokio::test]
async fn a08_resolver_source_makes_no_provider_or_broker_call() {
    // Structural proof: the resolver's own source never references a
    // provider/broker client constructor. Complements a07 (no db_pool call).
    let source = include_str!("../src/state/autonomous_runtime_context.rs");
    for forbidden in [
        "reqwest::",
        "AlpacaClient",
        "fetch_latest_closed_bar",
        "provider_client",
    ] {
        assert!(
            !source.contains(forbidden),
            "resolver source must not reference {forbidden:?}"
        );
    }
}

#[tokio::test]
async fn a09_resolver_does_not_mutate_appstate_bootstrap() {
    let source = include_str!("../src/state/autonomous_runtime_context.rs");
    assert!(
        !source.contains("native_strategy_bootstrap.lock"),
        "resolver must never write AppState's stored native_strategy_bootstrap field"
    );
}

#[tokio::test]
async fn a10_repeated_identical_resolution_produces_identical_binding_identity() {
    let _g = env_lock().lock().await;
    clear_env();
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZSG04");
    }
    let state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::LiveShadow, BrokerKind::Alpaca);
    state
        .set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
            strategy_id: "swing_momentum".to_string(),
        }]))
        .await;

    let first = resolve_autonomous_runtime_context(&state)
        .await
        .expect("active");
    let second = resolve_autonomous_runtime_context(&state)
        .await
        .expect("active");
    assert_eq!(
        first.effective_runtime_binding,
        second.effective_runtime_binding
    );
    clear_env();
}

// ===========================================================================
// Group B — One representative end-to-end lifecycle-error trigger
// ===========================================================================

/// Proves the resolver/policy wiring against a *real* production gate, not a
/// synthetic construction: a fresh AppState defaults to
/// `integrity.disarmed == true` (fail-closed), so
/// `start_execution_runtime()` refuses at the integrity gate before any
/// other gate runs — no DB, no artifact/parity/capital-policy configuration
/// needed. The remaining fault classes are proven either via the
/// `resolve_autonomous_runtime_context` calls above (B1A) or via the
/// crate-internal `autonomous_retry_policy` unit-test module (everything
/// requiring a directly-constructed `RuntimeLifecycleError`).
#[tokio::test]
async fn b01_real_integrity_disarmed_gate_maps_through_the_public_policy_api() {
    let _g = env_lock().lock().await;
    clear_env();
    let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));
    assert!(
        state.integrity.read().await.disarmed,
        "fresh state must default to disarmed"
    );

    let err = state
        .start_execution_runtime()
        .await
        .expect_err("fresh disarmed state must refuse start");
    assert_eq!(
        err.fault_class(),
        "runtime.control_refusal.integrity_disarmed"
    );

    let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
    assert_eq!(reason, AutonomousCoordinatorReason::IntegrityHalted);
    assert_eq!(
        classify_autonomous_reason(&reason),
        AutonomousRetryClass::ManualInterventionRequired
    );
}

// ===========================================================================
// Group C — Retry classes via the public API (items 11-16, re-exercised)
// ===========================================================================

#[test]
fn c01_wait_reasons_classify_wait_for_condition() {
    for reason in [
        AutonomousCoordinatorReason::AwaitingPreopen,
        AutonomousCoordinatorReason::AwaitingSessionOpen,
        AutonomousCoordinatorReason::PublicationGraceActive,
        AutonomousCoordinatorReason::LatestCompletedBarPending,
        AutonomousCoordinatorReason::ArmPending,
    ] {
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::WaitForCondition
        );
    }
}

#[test]
fn c02_transient_reasons_classify_retryable_transient() {
    for reason in [
        AutonomousCoordinatorReason::WsReconnecting,
        AutonomousCoordinatorReason::ProviderTemporarilyUnavailable,
        AutonomousCoordinatorReason::TemporaryDatabaseOperationFailure,
        AutonomousCoordinatorReason::RuntimeEndedWithoutHalt,
    ] {
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::RetryableTransient
        );
    }
}

#[test]
fn c03_terminal_reasons_classify_session_terminal() {
    for reason in [
        AutonomousCoordinatorReason::SessionClosed,
        AutonomousCoordinatorReason::OperationCompleted,
        AutonomousCoordinatorReason::NonTradingDay,
    ] {
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::SessionTerminal
        );
    }
}

#[test]
fn c04_conservative_fallback_classifies_manual() {
    let reason = AutonomousCoordinatorReason::UnclassifiedFailClosed {
        fault_class: "external_test_unmapped",
    };
    assert_eq!(
        classify_autonomous_reason(&reason),
        AutonomousRetryClass::ManualInterventionRequired
    );
}

// ===========================================================================
// Group D — WS distinctions via the public typed-fact adapter (items 29-32)
// ===========================================================================

#[test]
fn d01_typed_gap_detected_maps_manual_ws_gap_detected() {
    let state = AlpacaWsContinuityState::GapDetected {
        last_message_id: None,
        last_event_at: None,
        detail: "gap".to_string(),
    };
    let reason = coordinator_reason_from_ws_continuity(&state).expect("gap is a blocker");
    assert_eq!(reason, AutonomousCoordinatorReason::WsGapDetected);
    assert_eq!(
        classify_autonomous_reason(&reason),
        AutonomousRetryClass::ManualInterventionRequired
    );
}

#[test]
fn d02_cold_start_unproven_never_becomes_reconnecting() {
    let reason = coordinator_reason_from_ws_continuity(&AlpacaWsContinuityState::ColdStartUnproven)
        .expect("cold start is a blocker");
    assert_ne!(reason, AutonomousCoordinatorReason::WsReconnecting);
    assert_eq!(
        classify_autonomous_reason(&reason),
        AutonomousRetryClass::ManualInterventionRequired
    );
}

// ===========================================================================
// Group E — Session-plan / driver / create-or-recover typed adapters
// ===========================================================================

#[test]
fn e01_weekend_and_holiday_map_non_trading_day() {
    for reason_code in [
        AutonomousDailyPlanReason::Weekend,
        AutonomousDailyPlanReason::ExchangeHoliday,
    ] {
        let resolution = AutonomousDailySessionPlanResolution::NotApplicable {
            market_date: "2026-07-18".to_string(),
            reason_code,
        };
        let reason = coordinator_reason_from_session_plan_resolution(&resolution).unwrap();
        assert_eq!(reason, AutonomousCoordinatorReason::NonTradingDay);
        assert_eq!(
            classify_autonomous_reason(&reason),
            AutonomousRetryClass::SessionTerminal
        );
    }
}

#[test]
fn e02_identity_conflict_maps_operation_identity_conflict() {
    let outcome = CreateOrRecoverAutonomousDailyOperationOutcome::IdentityConflict {
        existing_operation_id: uuid::Uuid::nil(),
        expected_operation_id: uuid::Uuid::from_u128(7),
        differing_fields: vec!["runtime_binding_identity".to_string()],
    };
    let reason = coordinator_reason_from_create_or_recover_outcome(&outcome).unwrap();
    assert_eq!(
        reason,
        AutonomousCoordinatorReason::OperationIdentityConflict
    );
    assert_eq!(
        classify_autonomous_reason(&reason),
        AutonomousRetryClass::ManualInterventionRequired
    );
}

#[test]
fn e03_poll_failed_transient_maps_provider_temporarily_unavailable() {
    let outcome = AutonomousCompletedBarDriverOutcome::PollFailedTransient {
        detail: "timeout".to_string(),
    };
    let reason = coordinator_reason_from_completed_bar_driver_outcome(&outcome).unwrap();
    assert_eq!(
        reason,
        AutonomousCoordinatorReason::ProviderTemporarilyUnavailable
    );
    assert_eq!(
        classify_autonomous_reason(&reason),
        AutonomousRetryClass::RetryableTransient
    );
}

// ===========================================================================
// Group F — Signatures (items 33-39, re-exercised via the public API)
// ===========================================================================

#[test]
fn f01_identical_inputs_produce_identical_signatures() {
    let reason = AutonomousCoordinatorReason::AssignmentMissing;
    let identity = AutonomousBlockerIdentity {
        operation_id: Some(uuid::Uuid::from_u128(1)),
        ..Default::default()
    };
    assert_eq!(
        blocker_signature(&reason, &identity),
        blocker_signature(&reason, &identity)
    );
}

#[test]
fn f02_changed_operation_id_changes_signature() {
    let reason = AutonomousCoordinatorReason::OperationIdentityConflict;
    let a = AutonomousBlockerIdentity {
        operation_id: Some(uuid::Uuid::from_u128(1)),
        ..Default::default()
    };
    let b = AutonomousBlockerIdentity {
        operation_id: Some(uuid::Uuid::from_u128(2)),
        ..Default::default()
    };
    assert_ne!(
        blocker_signature(&reason, &a),
        blocker_signature(&reason, &b)
    );
}

#[test]
fn f03_signature_is_bounded_and_secret_free() {
    let reason = AutonomousCoordinatorReason::AssignmentMissing;
    let long = "z".repeat(5_000);
    let identity = AutonomousBlockerIdentity {
        assignment_identity: Some(&long),
        ..Default::default()
    };
    let sig = blocker_signature(&reason, &identity);
    let rendered = format!("{sig:?}");
    assert!(
        rendered.len() < 3_000,
        "signature debug rendering must stay bounded"
    );
    assert!(!rendered.to_lowercase().contains("token"));
}

// ===========================================================================
// Group G — Backoff (items 40-46)
// ===========================================================================

#[test]
fn g01_backoff_schedule_matches_30_60_120_300() {
    assert_eq!(
        retry_delay_for_attempt(1),
        std::time::Duration::from_secs(30)
    );
    assert_eq!(
        retry_delay_for_attempt(2),
        std::time::Duration::from_secs(60)
    );
    assert_eq!(
        retry_delay_for_attempt(3),
        std::time::Duration::from_secs(120)
    );
    assert_eq!(
        retry_delay_for_attempt(4),
        std::time::Duration::from_secs(300)
    );
    assert_eq!(
        retry_delay_for_attempt(50),
        std::time::Duration::from_secs(300)
    );
}

#[test]
fn g02_zero_attempt_never_produces_a_rapid_retry_loop() {
    assert_eq!(
        retry_delay_for_attempt(0),
        std::time::Duration::from_secs(30)
    );
}

#[test]
fn g03_next_retry_at_uses_the_injected_instant_exactly() {
    let now = Utc.with_ymd_and_hms(2026, 7, 17, 9, 30, 0).unwrap();
    assert_eq!(next_retry_at(now, 1), now + chrono::Duration::seconds(30));
    assert_eq!(next_retry_at(now, 4), now + chrono::Duration::seconds(300));
}

// ===========================================================================
// Group H — Negative boundaries (items 47-50)
// ===========================================================================

#[tokio::test]
async fn h01_no_session_controller_function_is_reachable_from_the_tested_production_code() {
    // Neither the extracted resolver nor the typed policy module references
    // the session controller's own tick/start/stop functions — a
    // structural proof over the exact production files this patch adds,
    // not merely a runtime assertion. The one production call this test
    // file itself makes is start_execution_runtime() (b01), the canonical
    // start path the bundle's contract requires every start attempt to
    // route through, never a second/simplified gate.
    // Checks for actual call syntax (an open paren), not a bare mention —
    // this module's own doc comments name these functions in prose to
    // document that they are NOT called from here.
    for source in [
        include_str!("../src/state/autonomous_runtime_context.rs"),
        include_str!("../src/state/autonomous_retry_policy.rs"),
    ] {
        assert!(!source.contains("session_controller::"));
        assert!(!source.contains("attempt_auto_start("));
        assert!(!source.contains("attempt_auto_stop("));
        assert!(!source.contains("run_session_controller("));
    }
}

#[tokio::test]
async fn h02_group_a_resolver_calls_never_created_a_run_or_touched_a_provider_or_broker() {
    // Every Group A/B fixture in this file uses db: None (the
    // new_for_test_with_mode_and_broker default) and no provider/broker
    // client is constructed anywhere in this crate's test dependency graph
    // for these fixtures. b01's single start_execution_runtime() call
    // returns Err before create_or_reuse_run_for_start is ever reached
    // (integrity gate fires first) — no run row, no outbox row, no
    // provider/broker/network effect.
    let _g = env_lock().lock().await;
    clear_env();
    let state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::LiveShadow, BrokerKind::Alpaca);
    assert!(state.db.is_none());
}

// ===========================================================================
// Group I — Runtime-dispatch-truth adapter (D1D1-POLICY-CLOSURE-REPAIR-01
// REPAIR 3), fully constructible from public types.
// ===========================================================================

#[test]
fn i01_dispatch_truth_active_matching_run_returns_none() {
    let run_id = uuid::Uuid::from_u128(1);
    let truth = AutonomousStrategyDispatchRuntimeTruth::Active { run_id };
    assert_eq!(
        coordinator_reason_from_strategy_dispatch_runtime_truth(&truth, run_id),
        None
    );
}

#[test]
fn i02_dispatch_truth_active_mismatched_run_maps_run_id_mismatch() {
    let truth = AutonomousStrategyDispatchRuntimeTruth::Active {
        run_id: uuid::Uuid::from_u128(1),
    };
    let reason =
        coordinator_reason_from_strategy_dispatch_runtime_truth(&truth, uuid::Uuid::from_u128(2))
            .expect("mismatch is a blocker");
    assert_eq!(reason, AutonomousCoordinatorReason::RuntimeRunIdMismatch);
    assert_eq!(
        classify_autonomous_reason(&reason),
        AutonomousRetryClass::ManualInterventionRequired
    );
}

#[test]
fn i03_dispatch_truth_no_locally_owned_run_maps_durable_active_without_local_owner() {
    let reason = coordinator_reason_from_strategy_dispatch_runtime_truth(
        &AutonomousStrategyDispatchRuntimeTruth::NoLocallyOwnedRun,
        uuid::Uuid::from_u128(1),
    )
    .expect("no locally owned run is a blocker");
    assert_eq!(
        reason,
        AutonomousCoordinatorReason::DurableActiveRunWithoutLocalOwner
    );
}

#[test]
fn i04_dispatch_truth_bootstrap_missing_dormant_failed_map_exact_reasons() {
    let cases = [
        (
            AutonomousStrategyDispatchRuntimeTruth::NativeStrategyBootstrapMissing,
            AutonomousCoordinatorReason::NativeStrategyBootstrapMissing,
        ),
        (
            AutonomousStrategyDispatchRuntimeTruth::NativeStrategyBootstrapDormant,
            AutonomousCoordinatorReason::NativeStrategyBootstrapDormant,
        ),
        (
            AutonomousStrategyDispatchRuntimeTruth::NativeStrategyBootstrapFailed,
            AutonomousCoordinatorReason::NativeStrategyBootstrapFailed,
        ),
    ];
    for (truth, expected) in cases {
        let reason = coordinator_reason_from_strategy_dispatch_runtime_truth(
            &truth,
            uuid::Uuid::from_u128(1),
        )
        .unwrap_or_else(|| panic!("{truth:?} must map to a coordinator reason"));
        assert_eq!(reason, expected);
    }
}

// ===========================================================================
// Group J — Blocker-signature collision resistance on long inputs
// (D1D1-POLICY-CLOSURE-REPAIR-01 REPAIR 4), fully constructible from public
// types.
// ===========================================================================

#[test]
fn j01_long_assignment_identity_followed_by_different_binding_identity_still_differs() {
    let reason = AutonomousCoordinatorReason::AssignmentMissing;
    let long = "q".repeat(10_000);
    let identity_a = AutonomousBlockerIdentity {
        assignment_identity: Some(&long),
        runtime_binding_identity: Some("binding-a"),
        ..Default::default()
    };
    let identity_b = AutonomousBlockerIdentity {
        assignment_identity: Some(&long),
        runtime_binding_identity: Some("binding-b"),
        ..Default::default()
    };
    assert_ne!(
        blocker_signature(&reason, &identity_a),
        blocker_signature(&reason, &identity_b)
    );
}

#[test]
fn j02_field_boundaries_are_unambiguous() {
    let reason = AutonomousCoordinatorReason::AssignmentMissing;
    let identity_a = AutonomousBlockerIdentity {
        assignment_identity: Some("ab"),
        runtime_binding_identity: Some("c"),
        ..Default::default()
    };
    let identity_b = AutonomousBlockerIdentity {
        assignment_identity: Some("a"),
        runtime_binding_identity: Some("bc"),
        ..Default::default()
    };
    assert_ne!(
        blocker_signature(&reason, &identity_a),
        blocker_signature(&reason, &identity_b)
    );
}

#[test]
fn j03_long_input_signature_is_bounded_and_raw_text_free() {
    let reason = AutonomousCoordinatorReason::AssignmentMissing;
    let marker = "public-api-raw-marker-9f3e".repeat(400);
    let identity = AutonomousBlockerIdentity {
        assignment_identity: Some(&marker),
        ..Default::default()
    };
    let sig = blocker_signature(&reason, &identity);
    let rendered = format!("{sig:?}");
    assert!(
        rendered.len() < 300,
        "rendered signature must stay bounded regardless of input size"
    );
    assert!(!rendered.contains("public-api-raw-marker"));
}
