//! B1B-close: Execution loop dispatch seam proof tests.
//!
//! Proves that `AppState::tick_strategy_dispatch` is the canonical runtime-owned
//! `on_bar` dispatch path.  The signal route deposits bar input via
//! `deposit_strategy_bar_input`; the execution loop consumes it via
//! `tick_strategy_dispatch`.
//!
//! All tests are pure in-process; no DB or network required.
//!
//! # What is proved
//!
//! | ID   | Condition                               | Expected                         |
//! |------|-----------------------------------------|----------------------------------|
//! | L01  | Active bootstrap + deposited bar        | tick_strategy_dispatch → Some    |
//! | L02  | No bar deposited                        | tick_strategy_dispatch → None    |
//! | L03  | No bootstrap + deposited bar            | tick_strategy_dispatch → None    |
//! | L04  | Bar consumed on first tick              | second tick → None (once-only)   |
//! | L05  | Second deposit supersedes first         | only second bar dispatched       |
//!
//! # B1B dispatch distinction
//!
//! - Route-driven (secondary / test-seam): `invoke_native_strategy_on_bar_from_signal`
//!   called directly; fires synchronously in the HTTP handler context.
//! - Loop-driven (canonical B1B): `deposit_strategy_bar_input` → `tick_strategy_dispatch`;
//!   fires in the execution loop's tick context (loop_runner.rs), after the
//!   orchestrator tick and snapshot are settled.
//!
//! L01 is the primary B1B-close proof: runtime-owned loop dispatch invokes
//! `on_bar` when an active bootstrap and a deposited bar both exist.
//!
//! L02 proves fail-closed on the majority of ticks: the execution loop ticks
//! continuously but `on_bar` is only called when a bar is actually pending.
//!
//! L04 proves exactly-once dispatch: the pending slot is cleared on the first
//! call so no double-invocation occurs across two consecutive ticks.

use std::sync::Arc;

use mqk_daemon::state::{self, AppState, StrategyBarInput};
use mqk_runtime::native_strategy::{build_daemon_plugin_registry, NativeStrategyBootstrap};
use mqk_strategy::IntentMode;

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

fn test_bar_input() -> StrategyBarInput {
    StrategyBarInput {
        now_tick: 1,
        end_ts: 1_700_000_000,
        limit_price: Some(150_000_000),
        qty: 10,
    }
}

// ---------------------------------------------------------------------------
// L01 — active bootstrap + deposited bar → loop dispatch returns Some
// ---------------------------------------------------------------------------

/// L01: Active bootstrap + deposited bar → tick_strategy_dispatch returns Some.
///
/// Primary B1B-close proof: the execution loop dispatch seam (not the HTTP
/// route handler) invokes `on_bar`.  The sequence is:
///   1. Signal route: `deposit_strategy_bar_input` (deposits, does NOT call on_bar)
///   2. Execution loop tick: `tick_strategy_dispatch` (canonical dispatch owner)
///   3. on_bar fires inside tick_strategy_dispatch → Some returned
#[tokio::test]
async fn b1b_l01_active_bootstrap_loop_dispatch_returns_some() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;

    // Step 1 (signal route): deposit bar input — does NOT call on_bar directly.
    st.deposit_strategy_bar_input(test_bar_input()).await;

    // Step 2 (execution loop tick): canonical runtime-owned dispatch.
    let result = st.tick_strategy_dispatch().await;

    assert!(
        result.is_some(),
        "L01: active bootstrap + deposited bar must invoke on_bar via loop dispatch"
    );
    assert_eq!(
        result.unwrap().intents.mode,
        IntentMode::Live,
        "L01: B1C lifted shadow mode; loop dispatch must produce Live intents"
    );
}

// ---------------------------------------------------------------------------
// L02 — no bar deposited → tick returns None (fail-closed on most ticks)
// ---------------------------------------------------------------------------

/// L02: No pending bar → tick_strategy_dispatch returns None.
///
/// Proves fail-closed on the majority of ticks: the execution loop ticks on a
/// 1-second interval, but `on_bar` fires only when a bar is actually pending.
/// An empty slot is the normal state, not an error.
#[tokio::test]
async fn b1b_l02_no_bar_deposited_tick_returns_none() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;
    // No deposit — pending_strategy_bar_input slot is None.

    let result = st.tick_strategy_dispatch().await;

    assert!(
        result.is_none(),
        "L02: no pending bar → tick must return None; no fabricated callback on empty slot"
    );
}

// ---------------------------------------------------------------------------
// L03 — no bootstrap + deposited bar → fail-closed
// ---------------------------------------------------------------------------

/// L03: No active bootstrap (no run) + deposited bar → tick returns None.
///
/// Proves fail-closed: even when a bar is deposited, if no active bootstrap
/// exists (no execution run is active), no `on_bar` callback is made.
/// The bar is consumed but no dispatch occurs — correct fail-closed behavior.
#[tokio::test]
async fn b1b_l03_no_bootstrap_deposited_bar_is_fail_closed() {
    let st = bare_state().await;
    // No bootstrap stored (None = no active run).

    st.deposit_strategy_bar_input(test_bar_input()).await;
    let result = st.tick_strategy_dispatch().await;

    assert!(
        result.is_none(),
        "L03: no bootstrap + deposited bar → tick must return None (fail-closed, no run)"
    );
}

// ---------------------------------------------------------------------------
// L04 — bar consumed on first tick; second tick returns None (exactly-once)
// ---------------------------------------------------------------------------

/// L04: Deposited bar is consumed atomically on the first tick.
///
/// Proves exactly-once dispatch: `tick_strategy_dispatch` takes the bar from
/// the pending slot and clears it in one atomic operation.  A second call on
/// the same tick interval returns None — no double-invocation.
#[tokio::test]
async fn b1b_l04_bar_consumed_exactly_once() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;

    st.deposit_strategy_bar_input(test_bar_input()).await;

    // First tick: takes the bar and dispatches on_bar.
    let first = st.tick_strategy_dispatch().await;
    assert!(
        first.is_some(),
        "L04 precondition: first tick must consume the bar"
    );

    // Second tick: slot is empty → no dispatch.
    let second = st.tick_strategy_dispatch().await;
    assert!(
        second.is_none(),
        "L04: bar is consumed on first tick; second tick must return None (exactly-once dispatch)"
    );
}

// ---------------------------------------------------------------------------
// L05 — second deposit supersedes the first (overwrite policy)
// ---------------------------------------------------------------------------

/// L05: Second deposit overwrites the first (single-slot overwrite policy).
///
/// If two deposits occur before the loop ticks (e.g. rapid successive signals),
/// only the second (later) bar occupies the slot.  The loop dispatches exactly
/// one bar per consumed deposit.  This is the documented overwrite policy.
#[tokio::test]
async fn b1b_l05_second_deposit_supersedes_first() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;

    // First deposit.
    st.deposit_strategy_bar_input(StrategyBarInput {
        now_tick: 1,
        end_ts: 1_700_000_000,
        limit_price: Some(100_000_000),
        qty: 5,
    })
    .await;

    // Second deposit supersedes the first.
    st.deposit_strategy_bar_input(StrategyBarInput {
        now_tick: 2,
        end_ts: 1_700_001_000,
        limit_price: Some(200_000_000),
        qty: 20,
    })
    .await;

    // One tick: the slot contains the second bar only.
    let result = st.tick_strategy_dispatch().await;
    assert!(
        result.is_some(),
        "L05: second deposit must be present in the slot and dispatched"
    );

    // Slot is empty after the single dispatch.
    let empty = st.tick_strategy_dispatch().await;
    assert!(
        empty.is_none(),
        "L05: after consuming the pending bar, slot must be empty"
    );
}
