//! B1B-close: Daemon-level input bridge proof tests.
//!
//! Proves that `AppState::invoke_native_strategy_on_bar_from_signal` delegates
//! correctly to the bootstrap seam.  All tests are pure in-process; no DB or
//! network required.
//!
//! # Tests
//!
//! | ID   | Bootstrap state | Signal        | Expected             |
//! |------|-----------------|---------------|----------------------|
//! | D01  | None (no run)   | any           | None (fail-closed)   |
//! | D02  | Dormant         | any           | None (fail-closed)   |
//! | D03  | Active          | limit signal  | Some (callback made) |
//! | D04  | Active          | market signal | Some (callback made) |
//! | D05  | Failed          | any           | None (fail-closed)   |
//!
//! D01 is the most important: no active run (None in field) must never invoke
//! the callback under any signal payload.

use std::sync::Arc;

use mqk_daemon::state::{self, AppState};
use mqk_runtime::native_strategy::{build_daemon_plugin_registry, NativeStrategyBootstrap};
use mqk_strategy::PluginRegistry;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn bare_state() -> Arc<AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ))
}

fn active_bootstrap() -> NativeStrategyBootstrap {
    let reg = build_daemon_plugin_registry();
    let ids = vec!["swing_momentum".to_string()];
    NativeStrategyBootstrap::bootstrap(Some(&ids), &reg)
}

fn dormant_bootstrap() -> NativeStrategyBootstrap {
    let reg = PluginRegistry::new();
    NativeStrategyBootstrap::bootstrap(None, &reg)
}

fn failed_bootstrap() -> NativeStrategyBootstrap {
    let reg = PluginRegistry::new(); // empty — fleet entry will not resolve
    let ids = vec!["no_such_strategy".to_string()];
    NativeStrategyBootstrap::bootstrap(Some(&ids), &reg)
}

// ---------------------------------------------------------------------------
// D01 — no bootstrap stored → fail-closed
// ---------------------------------------------------------------------------

/// D01: No bootstrap in AppState (no active run) → invoke returns None.
///
/// This is the most important daemon-level proof: when no execution run is
/// active, no strategy callback is ever made regardless of signal payload.
#[tokio::test]
async fn b1b_d01_no_bootstrap_stored_is_fail_closed() {
    let st = bare_state().await;
    // Default: native_strategy_bootstrap = None (no run active).

    let result = st
        .invoke_native_strategy_on_bar_from_signal(1, 1_700_000_000, Some(100_000_000), 5)
        .await;

    assert!(
        result.is_none(),
        "D01: no bootstrap stored → invoke must return None (fail-closed, no run active)"
    );
}

// ---------------------------------------------------------------------------
// D02 — Dormant bootstrap → fail-closed
// ---------------------------------------------------------------------------

/// D02: Dormant bootstrap in AppState → invoke returns None.
#[tokio::test]
async fn b1b_d02_dormant_bootstrap_is_fail_closed() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(dormant_bootstrap()))
        .await;

    let result = st
        .invoke_native_strategy_on_bar_from_signal(1, 1_700_000_000, Some(100_000_000), 5)
        .await;

    assert!(
        result.is_none(),
        "D02: dormant bootstrap must not invoke callback"
    );
}

// ---------------------------------------------------------------------------
// D03 — Active bootstrap + limit signal → callback made
// ---------------------------------------------------------------------------

/// D03: Active bootstrap + limit-order signal → invoke returns Some.
///
/// Proves the daemon wiring end-to-end: AppState → NativeStrategyBootstrap →
/// StrategyHost::on_bar.
#[tokio::test]
async fn b1b_d03_active_bootstrap_limit_signal_callback_made() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;

    let result = st
        .invoke_native_strategy_on_bar_from_signal(
            1,
            1_700_000_000,
            Some(100_000_000), // limit signal
            10,
        )
        .await;

    assert!(
        result.is_some(),
        "D03: active bootstrap + limit signal must invoke callback and return Some"
    );
    // B1C: shadow mode lifted; Live intents produced at daemon level.
    assert_eq!(
        result.unwrap().intents.mode,
        mqk_strategy::IntentMode::Live,
        "D03: B1C lifted shadow mode; daemon dispatch must produce Live intents"
    );
}

// ---------------------------------------------------------------------------
// D04 — Active bootstrap + market signal → callback made
// ---------------------------------------------------------------------------

/// D04: Active bootstrap + market-order signal → invoke returns Some.
///
/// Market orders carry no price reference; bar is incomplete.  The callback
/// is still invoked — the strategy receives the incomplete bar and returns
/// empty targets.  No silent suppression at the input bridge level.
#[tokio::test]
async fn b1b_d04_active_bootstrap_market_signal_callback_made() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;

    let result = st
        .invoke_native_strategy_on_bar_from_signal(
            2,
            1_700_000_000,
            None, // market order — no price reference
            5,
        )
        .await;

    assert!(
        result.is_some(),
        "D04: active bootstrap + market signal must still invoke callback"
    );
}

// ---------------------------------------------------------------------------
// D05 — Failed bootstrap → fail-closed
// ---------------------------------------------------------------------------

/// D05: Failed bootstrap in AppState → invoke returns None.
///
/// A Failed bootstrap means the fleet named an unregistered strategy.  The
/// daemon would have refused to start (bootstrap gate), but even if a Failed
/// bootstrap were hypothetically stored, no callback must be made.
#[tokio::test]
async fn b1b_d05_failed_bootstrap_is_fail_closed() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(failed_bootstrap()))
        .await;

    let result = st
        .invoke_native_strategy_on_bar_from_signal(1, 1_700_000_000, Some(100_000_000), 5)
        .await;

    assert!(
        result.is_none(),
        "D05: failed bootstrap must not invoke callback (fail-closed)"
    );
}
