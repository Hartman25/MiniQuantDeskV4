//! MULTI-SYMBOL-DISPATCH-LOOP-01: per-symbol strategy dispatch loop proof tests.
//!
//! Proves [`mqk_daemon::state::AppState::tick_strategy_dispatch_multi_symbol`]
//! and [`mqk_daemon::state::AppState::retain_targets_matching_symbol`] — the
//! two new seams `loop_runner.rs`'s B1C block now calls — without requiring a
//! DB, broker, or running execution loop.
//!
//! # What is proved
//!
//! | ID  | Condition                                              | Expected                                   |
//! |-----|----------------------------------------------------------|------------------------------------------|
//! | M01 | No pending bar                                         | empty `Vec` (fail-closed, no dispatch)     |
//! | M02 | Pending bar, no bootstrap                              | empty `Vec`; bar still consumed            |
//! | M03 | Single (legacy-shaped) assignment, active bootstrap    | one result, `IntentMode::Live`             |
//! | M04 | 3 assignments, active bootstrap                        | 3 results, in artifact order               |
//! | M05 | After M04, second call same tick                       | empty `Vec` (bar consumed exactly once)    |
//! | M06 | `retain_targets_matching_symbol`, all symbols match    | no-op, 0 dropped                           |
//! | M07 | `retain_targets_matching_symbol`, one symbol mismatched| mismatched target dropped, count returned  |
//! | M08 | `try_claim_b5_alert` for two different symbols         | both claims independently succeed          |
//!
//! M03 is the "legacy single-symbol behavior preserved" proof:
//! [`AppState::tick_strategy_dispatch_multi_symbol`] with a single
//! `EnvSingleSymbolFallback`-shaped [`SymbolStrategyAssignment`] takes the
//! pending bar input exactly once and dispatches exactly once — the same
//! single `.take()` + single dispatch that
//! [`AppState::tick_strategy_dispatch`] performs.
//!
//! M04/M05 are the "watchlist-v2 multi-symbol dispatches in artifact order"
//! and "bar consumed exactly once" proofs: one externally-deposited bar-tick
//! signal (design doc §4.4) drives dispatch for every configured symbol —
//! each symbol gets its own DB bar-window lookup inside
//! `dispatch_native_strategy_for_symbol_with_bar`, but the underlying
//! `StrategyBarInput` is a single account-wide slot, taken once.
//!
//! M06/M07 prove the new `b1c_symbol_mismatch_skipped` fail-closed guard:
//! the native strategy bootstrap's `TargetPosition.symbol` is fixed at
//! construction time from `MQK_STRATEGY_SYMBOL`, independent of which
//! symbol's bar window was just dispatched (see
//! `docs/design/native_multi_symbol_dispatch.md`, per-symbol strategy
//! bootstrap gap). [`AppState::retain_targets_matching_symbol`] drops any
//! target whose symbol does not match the dispatched assignment, rather than
//! submitting a misattributed decision.
//!
//! M08 is the "B5 per-symbol proof": `try_claim_b5_alert` dedups per symbol,
//! so the short-sale-guard Discord alert for one symbol does not suppress the
//! alert for another symbol dispatched in the same tick.

use std::sync::Arc;

use mqk_daemon::state::{self, AppState, StrategyBarInput, SymbolStrategyAssignment};
use mqk_runtime::native_strategy::{build_daemon_plugin_registry, NativeStrategyBootstrap};
use mqk_strategy::{IntentMode, TargetPosition};

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

fn assignment(symbol: &str) -> SymbolStrategyAssignment {
    SymbolStrategyAssignment {
        symbol: symbol.to_string(),
        strategy_id: "swing_momentum".to_string(),
        timeframe: "1Min".to_string(),
    }
}

// ---------------------------------------------------------------------------
// M01 — no pending bar -> empty Vec
// ---------------------------------------------------------------------------

/// M01: No pending bar input -> `tick_strategy_dispatch_multi_symbol` returns
/// an empty `Vec`, matching `tick_strategy_dispatch`'s `None` on the majority
/// of ticks (fail-closed, not an error).
#[tokio::test]
async fn m01_no_pending_bar_returns_empty_vec() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;
    // No deposit — pending_strategy_bar_input slot is None.

    let results = st
        .tick_strategy_dispatch_multi_symbol(&[assignment("AAPL")])
        .await;

    assert!(
        results.is_empty(),
        "M01: no pending bar -> empty Vec (fail-closed, no dispatch)"
    );
}

// ---------------------------------------------------------------------------
// M02 — pending bar, no bootstrap -> empty Vec; bar consumed
// ---------------------------------------------------------------------------

/// M02: Pending bar but no active bootstrap (no run) -> every per-symbol
/// dispatch returns `None`, so the overall result is an empty `Vec`. The bar
/// is still consumed (taken once), matching `tick_strategy_dispatch`'s
/// fail-closed L03 behavior.
#[tokio::test]
async fn m02_no_bootstrap_pending_bar_consumed_returns_empty_vec() {
    let st = bare_state().await;
    // No bootstrap stored (None = no active run).
    st.deposit_strategy_bar_input(test_bar_input()).await;

    let results = st
        .tick_strategy_dispatch_multi_symbol(&[assignment("AAPL"), assignment("MSFT")])
        .await;

    assert!(
        results.is_empty(),
        "M02: no bootstrap -> every per-symbol dispatch is None -> empty Vec"
    );
    assert!(
        st.pending_strategy_bar_input_is_none_for_test().await,
        "M02: the single pending bar input must be consumed even though no \
         dispatch produced a result"
    );
}

// ---------------------------------------------------------------------------
// M03 — single (legacy-shaped) assignment matches tick_strategy_dispatch
// ---------------------------------------------------------------------------

/// M03: `tick_strategy_dispatch_multi_symbol` with a single
/// `EnvSingleSymbolFallback`-shaped assignment is behaviorally identical to
/// `tick_strategy_dispatch`: one result, `IntentMode::Live` (B1C lifted
/// shadow mode).
#[tokio::test]
async fn m03_single_assignment_matches_legacy_tick_strategy_dispatch() {
    let st_legacy = bare_state().await;
    st_legacy
        .set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;
    st_legacy.deposit_strategy_bar_input(test_bar_input()).await;
    let legacy_result = st_legacy.tick_strategy_dispatch().await;
    assert!(
        legacy_result.is_some(),
        "M03 precondition: legacy single-symbol dispatch must produce Some"
    );

    let st_multi = bare_state().await;
    st_multi
        .set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;
    st_multi.deposit_strategy_bar_input(test_bar_input()).await;
    let multi_results = st_multi
        .tick_strategy_dispatch_multi_symbol(&[assignment("AAPL")])
        .await;

    assert_eq!(
        multi_results.len(),
        1,
        "M03: single-assignment multi dispatch must produce exactly one result"
    );
    assert_eq!(
        multi_results[0].1.intents.mode,
        legacy_result.unwrap().intents.mode,
        "M03: legacy and multi-symbol single-assignment dispatch must \
         produce the same IntentMode"
    );
    assert_eq!(
        multi_results[0].1.intents.mode,
        IntentMode::Live,
        "M03: B1C lifted shadow mode; loop dispatch must produce Live intents"
    );
}

// ---------------------------------------------------------------------------
// M04 — multi-symbol fan-out preserves artifact order
// ---------------------------------------------------------------------------

/// M04: With an active bootstrap and one pending bar, dispatching across
/// three configured symbols produces three results, in the same order as
/// `assignments` (design doc §5 Q4).
#[tokio::test]
async fn m04_multi_symbol_fan_out_preserves_artifact_order() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;
    st.deposit_strategy_bar_input(test_bar_input()).await;

    let assignments = vec![assignment("AAPL"), assignment("MSFT"), assignment("TSLA")];
    let results = st.tick_strategy_dispatch_multi_symbol(&assignments).await;

    assert_eq!(
        results.len(),
        3,
        "M04: active bootstrap dispatches the shared bar-tick signal to \
         every configured symbol"
    );
    let result_symbols: Vec<&str> = results.iter().map(|(a, _)| a.symbol.as_str()).collect();
    assert_eq!(
        result_symbols,
        vec!["AAPL", "MSFT", "TSLA"],
        "M04: results must preserve assignments order"
    );
    for (_, bar_result) in &results {
        assert_eq!(
            bar_result.intents.mode,
            IntentMode::Live,
            "M04: every per-symbol dispatch must produce Live intents \
             (B1C lifted shadow mode)"
        );
    }
}

// ---------------------------------------------------------------------------
// M05 — bar consumed exactly once for multi-symbol dispatch
// ---------------------------------------------------------------------------

/// M05: The single pending bar input is taken exactly once per
/// `tick_strategy_dispatch_multi_symbol` call, regardless of how many symbols
/// are configured. A second call on the same (unrefreshed) tick returns an
/// empty `Vec` — no double-dispatch.
#[tokio::test]
async fn m05_bar_consumed_exactly_once_for_multi_symbol() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;
    st.deposit_strategy_bar_input(test_bar_input()).await;

    let assignments = vec![assignment("AAPL"), assignment("MSFT")];

    let first = st.tick_strategy_dispatch_multi_symbol(&assignments).await;
    assert_eq!(
        first.len(),
        2,
        "M05 precondition: first call dispatches to both symbols"
    );

    let second = st.tick_strategy_dispatch_multi_symbol(&assignments).await;
    assert!(
        second.is_empty(),
        "M05: bar slot is empty after the first call; second call must \
         return an empty Vec (exactly-once consumption)"
    );
}

// ---------------------------------------------------------------------------
// M06/M07 — retain_targets_matching_symbol (b1c_symbol_mismatch_skipped guard)
// ---------------------------------------------------------------------------

/// M06: When every target's symbol matches the dispatched symbol
/// (case-insensitive), `retain_targets_matching_symbol` is a no-op and
/// returns 0 dropped.
#[test]
fn m06_retain_targets_matching_symbol_is_noop_when_all_match() {
    let mut targets = vec![
        TargetPosition {
            symbol: "AAPL".to_string(),
            qty: 10,
        },
        TargetPosition {
            symbol: "aapl".to_string(),
            qty: -5,
        },
    ];

    let dropped = AppState::retain_targets_matching_symbol(&mut targets, "AAPL");

    assert_eq!(
        dropped, 0,
        "M06: case-insensitive matching symbols are not dropped"
    );
    assert_eq!(targets.len(), 2, "M06: no-op leaves all targets in place");
}

/// M07: A target whose symbol does not match the dispatched assignment is
/// dropped, and the returned count reflects exactly how many were dropped.
/// This is the `b1c_symbol_mismatch_skipped` fail-closed guard: without it, a
/// mismatched target would carry a qty computed from a *different* symbol's
/// bars under the dispatched symbol's name.
#[test]
fn m07_retain_targets_matching_symbol_drops_mismatched_targets() {
    let mut targets = vec![
        TargetPosition {
            symbol: "AAPL".to_string(),
            qty: 10,
        },
        TargetPosition {
            symbol: "MSFT".to_string(),
            qty: -5,
        },
    ];

    let dropped = AppState::retain_targets_matching_symbol(&mut targets, "AAPL");

    assert_eq!(
        dropped, 1,
        "M07: exactly one mismatched target (MSFT) must be dropped"
    );
    assert_eq!(targets.len(), 1, "M07: only the matching target remains");
    assert_eq!(targets[0].symbol, "AAPL");
    assert_eq!(targets[0].qty, 10);
}

// ---------------------------------------------------------------------------
// M08 — B5 short-sale-guard alert dedup is per-symbol
// ---------------------------------------------------------------------------

/// M08: `try_claim_b5_alert` dedups independently per symbol. A claim for one
/// symbol does not consume or block the claim for another symbol dispatched
/// in the same tick — required for the per-symbol B5 short-sale-guard Discord
/// alert in `loop_runner.rs`'s B1C block.
#[tokio::test]
async fn m08_b5_alert_claim_is_independent_per_symbol() {
    let st = bare_state().await;

    assert!(
        st.try_claim_b5_alert_for_test("AAPL").await,
        "M08: first claim for AAPL must succeed"
    );
    assert!(
        !st.try_claim_b5_alert_for_test("AAPL").await,
        "M08: second claim for AAPL must be deduped (already alerted)"
    );
    assert!(
        st.try_claim_b5_alert_for_test("MSFT").await,
        "M08: AAPL's claim must not block MSFT's independent claim"
    );
}
