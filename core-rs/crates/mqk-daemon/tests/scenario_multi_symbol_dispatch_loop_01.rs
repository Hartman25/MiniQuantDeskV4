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

// ---------------------------------------------------------------------------
// A1-PANIC-ISOLATION-REVIEW-REPAIR-01
//
// Repairs two review defects in A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01:
//
//   1. The prior tests injected their panic via `set_panic_on_symbol_for_test`,
//      which fires at the very TOP of `dispatch_native_strategy_for_symbol_
//      with_bar_and_facts` -- before any bar-window prep or strategy
//      evaluation. That only proves an early dispatch-infrastructure panic
//      is survivable; it never exercises a panic inside the real
//      `StrategyHost -> Strategy::on_bar(&mut self)` callback, so it never
//      proved anything about that callback's own state-corruption risk.
//      That seam is kept below, repurposed as the A1-R6 infrastructure-panic
//      negative control.
//
//   2. Tier A holds exactly one mutable `StrategyHost`/`Box<dyn Strategy>`
//      for the whole run, shared across EVERY symbol dispatched this tick
//      and every future tick. `Strategy::on_bar` takes `&mut self`; a panic
//      mid-callback does not roll back whatever mutation the strategy
//      already made, and `AssertUnwindSafe` does not prove the object is
//      safe to reuse afterward. So the prior tests' expectation --  that
//      siblings dispatched AFTER a panicking symbol still produce normal
//      results -- was not structurally safe. The repaired production
//      behavior (`AppState::invoke_native_strategy_host_on_bar`) instead
//      quarantines the shared bootstrap to `Failed` the instant a real
//      on_bar panic is caught: every symbol from that point on (siblings
//      later in this tick, and every future tick) fails closed with
//      ordinary `None`, indistinguishable from any other Dormant/Failed
//      bootstrap -- never a fabricated result, never a retry.
//
// A1-R1/R2/R3 inject the panic through a real `Strategy` implementation
// (`PanicOnNthCallStrategy`) registered into a real `StrategyHost`, so the
// panic genuinely originates inside `Strategy::on_bar`, not a pre-dispatch
// test hook.
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};

use futures_util::FutureExt;

/// A real [`mqk_strategy::Strategy`] whose `on_bar` panics on its Nth
/// invocation (1-indexed) and otherwise returns an empty (no-op) output.
/// Dispatch is sequential, so "panics on call N" is equivalent to "panics
/// while evaluating the Nth assignment in the list".
struct PanicOnNthCallStrategy {
    spec: mqk_strategy::StrategySpec,
    call_count: AtomicU32,
    panic_on_call: u32,
}

impl PanicOnNthCallStrategy {
    fn new(panic_on_call: u32) -> Self {
        Self {
            spec: mqk_strategy::StrategySpec::new("a1_panic_probe", 60),
            call_count: AtomicU32::new(0),
            panic_on_call,
        }
    }
}

impl mqk_strategy::Strategy for PanicOnNthCallStrategy {
    fn spec(&self) -> mqk_strategy::StrategySpec {
        self.spec.clone()
    }

    fn on_bar(&mut self, _ctx: &mqk_strategy::StrategyContext) -> mqk_strategy::StrategyOutput {
        let n = self.call_count.fetch_add(1, AtomicOrdering::SeqCst) + 1;
        if n == self.panic_on_call {
            panic!("A1_REAL_ON_BAR_PANIC call {n}");
        }
        mqk_strategy::StrategyOutput::new(vec![])
    }
}

/// A real, Active `NativeStrategyBootstrap` whose host will panic on its
/// `panic_on_call`th `on_bar` invocation this run.
fn panic_probe_bootstrap(panic_on_call: u32) -> NativeStrategyBootstrap {
    let mut host = mqk_strategy::StrategyHost::new(mqk_strategy::ShadowMode::Off);
    host.register(Box::new(PanicOnNthCallStrategy::new(panic_on_call)))
        .expect("host registration must succeed");
    NativeStrategyBootstrap {
        outcome: mqk_runtime::native_strategy::NativeStrategyBootstrapOutcome::Active {
            host,
            strategy_id: "a1_panic_probe".to_string(),
        },
    }
}

fn result_symbols_wf(
    results: &[(
        SymbolStrategyAssignment,
        mqk_strategy::StrategyBarResult,
        Option<mqk_daemon::state::EvaluatedBarFacts>,
    )],
) -> Vec<&str> {
    results.iter().map(|(a, _, _)| a.symbol.as_str()).collect()
}

/// A1-R1: a REAL panic inside the middle symbol's `Strategy::on_bar`
/// callback quarantines the shared host. AAPL (dispatched before the
/// panic) keeps its result; MSFT (the panicking call) and SPY (dispatched
/// after quarantine) both produce no result -- SPY's absence is ordinary
/// fail-closed behavior against a now-Failed bootstrap, not a fabricated
/// decision.
#[tokio::test]
async fn a1_r1_real_middle_symbol_panic_quarantines_host_for_remaining_siblings() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(panic_probe_bootstrap(2)))
        .await;
    st.deposit_strategy_bar_input(test_bar_input()).await;

    let assignments = vec![assignment("AAPL"), assignment("MSFT"), assignment("SPY")];
    let results = st
        .tick_strategy_dispatch_multi_symbol_with_bar_facts(&assignments)
        .await;

    assert_eq!(
        result_symbols_wf(&results),
        vec!["AAPL"],
        "A1-R1: only AAPL (dispatched before the real on_bar panic) may keep a result -- \
         MSFT panicked and SPY ran against the now-quarantined host"
    );
    assert_eq!(
        st.dispatch_call_count_for_test(),
        3,
        "A1-R1: all three assignments must still be genuinely dispatched exactly once"
    );
    assert_eq!(
        st.native_strategy_bootstrap_truth_state_for_test().await,
        Some("failed"),
        "A1-R1: the shared host must be quarantined (Failed) after the real on_bar panic"
    );
}

/// A1-R2: a REAL panic on the FIRST symbol's callback quarantines the host
/// before any sibling runs -- no symbol in this tick produces a result.
#[tokio::test]
async fn a1_r2_real_first_symbol_panic_blocks_all_siblings() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(panic_probe_bootstrap(1)))
        .await;
    st.deposit_strategy_bar_input(test_bar_input()).await;

    let assignments = vec![assignment("AAPL"), assignment("MSFT"), assignment("SPY")];
    let results = st
        .tick_strategy_dispatch_multi_symbol_with_bar_facts(&assignments)
        .await;

    assert!(
        results.is_empty(),
        "A1-R2: AAPL's real on_bar panic quarantines the host before MSFT/SPY run -- \
         no symbol this tick may produce a result"
    );
    assert_eq!(
        st.dispatch_call_count_for_test(),
        3,
        "A1-R2: all three assignments must still be genuinely dispatched exactly once"
    );
}

/// A1-R3: fault isolation must not retry the panicking strategy callback --
/// exactly one dispatch call per assignment, never two.
#[tokio::test]
async fn a1_r3_panic_isolation_does_not_retry_the_panicking_symbol() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(panic_probe_bootstrap(2)))
        .await;
    st.deposit_strategy_bar_input(test_bar_input()).await;

    let assignments = vec![assignment("AAPL"), assignment("MSFT"), assignment("SPY")];
    let _results = st
        .tick_strategy_dispatch_multi_symbol_with_bar_facts(&assignments)
        .await;

    assert_eq!(
        st.dispatch_call_count_for_test(),
        3,
        "A1-R3: exactly one dispatch call per assignment -- no retry of the panicking symbol"
    );
}

/// A1-R4: an ordinary `None` result (no active bootstrap -- ordinary
/// fail-closed dormant behavior) is NOT classified as a panic/fault. Both
/// symbols are genuinely invoked (proven by the call counter), and neither
/// produces a result, exactly as before this patch.
#[tokio::test]
async fn a1_r4_ordinary_none_result_is_not_classified_as_panic() {
    let st = bare_state().await;
    // No bootstrap stored -- every per-symbol dispatch returns None.
    st.deposit_strategy_bar_input(test_bar_input()).await;

    let assignments = vec![assignment("AAPL"), assignment("MSFT")];
    let results = st
        .tick_strategy_dispatch_multi_symbol_with_bar_facts(&assignments)
        .await;

    assert!(
        results.is_empty(),
        "A1-R4: no active bootstrap -- ordinary None for every symbol, unchanged"
    );
    assert_eq!(
        st.dispatch_call_count_for_test(),
        2,
        "A1-R4: both symbols were genuinely dispatched (real None, not skipped/faulted)"
    );
}

/// A1-R5: with no panic injected, ordinary successful multi-symbol dispatch
/// is unchanged -- every symbol produces a result, in assignment order.
#[tokio::test]
async fn a1_r5_ordinary_successful_dispatch_is_unchanged() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;
    st.deposit_strategy_bar_input(test_bar_input()).await;

    let assignments = vec![assignment("AAPL"), assignment("MSFT"), assignment("SPY")];
    let results = st
        .tick_strategy_dispatch_multi_symbol_with_bar_facts(&assignments)
        .await;

    assert_eq!(
        result_symbols_wf(&results),
        vec!["AAPL", "MSFT", "SPY"],
        "A1-R5: with no panic, every symbol must still produce a result in order"
    );
}

/// A1-R6: infrastructure-panic negative control. `set_panic_on_symbol_for_test`
/// injects a panic at the very TOP of the per-symbol dispatch function --
/// before any bar-window preparation or strategy evaluation, squarely in
/// dispatch infrastructure, never inside `Strategy::on_bar`. The repaired
/// catch boundary lives ONLY inside `invoke_native_strategy_host_on_bar`
/// (narrowly around the real on_bar call), so this infrastructure panic
/// must NOT be caught/converted into a per-symbol strategy fault: it must
/// escape as a genuine unwind out of the whole multi-symbol dispatch call,
/// losing the entire tick's results (including AAPL's, dispatched earlier
/// in iteration order) -- unlike a real on_bar panic, which is contained
/// and leaves earlier per-symbol results intact (A1-R1). This proves the
/// repaired boundary is actually narrow, not merely relabeled.
#[tokio::test]
async fn a1_r6_infrastructure_panic_is_not_downgraded_to_a_strategy_fault() {
    let st = bare_state().await;
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;
    st.set_panic_on_symbol_for_test(Some("MSFT".to_string()))
        .await;
    st.deposit_strategy_bar_input(test_bar_input()).await;

    let assignments = vec![assignment("AAPL"), assignment("MSFT"), assignment("SPY")];
    let outcome = std::panic::AssertUnwindSafe(
        st.tick_strategy_dispatch_multi_symbol_with_bar_facts(&assignments),
    )
    .catch_unwind()
    .await;

    assert!(
        outcome.is_err(),
        "A1-R6: an infrastructure-seam panic (outside on_bar) must escape as a real unwind, \
         never be silently converted into a per-symbol strategy fault"
    );
    assert_eq!(
        st.native_strategy_bootstrap_truth_state_for_test().await,
        Some("active"),
        "A1-R6: an infrastructure panic must not quarantine the strategy host -- only a real \
         on_bar panic does that"
    );
}
