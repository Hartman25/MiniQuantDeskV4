//! MULTI-STRATEGY-RUNTIME-DRY-RUN-01: dry-run secondary strategy diagnostics.
//!
//! Proves [`mqk_daemon::state::evaluate_dry_run_strategy`] /
//! [`mqk_daemon::state::evaluate_dry_run_strategies`] (`state/dry_run_strategy.rs`)
//! let the runtime evaluate a secondary strategy (`intraday_short_scalper`)
//! alongside the primary strategy (`intraday_scalper`) on the same bar window,
//! without ever submitting an order — and that the existing single-strategy
//! primary path (`bar_result_to_decisions`, `StrategyHost`) is unchanged.
//!
//! # Test inventory
//!
//! | ID    | Requirement                                                          |
//! |-------|-----------------------------------------------------------------------|
//! | DR01  | No dry-run IDs configured -> existing single-strategy behavior unchanged |
//! | DR02  | Dry-run `intraday_short_scalper` evaluated alongside primary `intraday_scalper` on the same window |
//! | DR03  | Primary long strategy result remains eligible for the existing decision path |
//! | DR04  | Secondary strategy diagnostic always has `submitted=false`          |
//! | DR05  | Secondary strategy cannot call `submit_internal_strategy_decision` (structural: sync fn, no `AppState`) |
//! | DR06  | Secondary strategy cannot write an outbox row (structural: no DB handle; pure/idempotent output) |
//! | DR07  | Secondary bearish short signal is classified `ShortOpen`            |
//! | DR08  | Secondary short-open diagnostic reports B5 + default policy would block |
//! | DR09  | Live shorting remains blocked by the existing policy gate (re-checked from this file) |
//!
//! DR10/DR11 ("existing B5 tests pass unchanged" / "existing parallel
//! long/short strategy tests pass unchanged") are proved by running the
//! pre-existing, untouched test files in this patch's validation step
//! (`scenario_native_strategy_b5_short_guard`,
//! `scenario_parallel_long_short_strategy_01`) — not duplicated here.

use std::collections::BTreeMap;

use uuid::Uuid;

use mqk_daemon::capital_policy::{
    evaluate_short_entry_policy, ShortEntryConfig, ShortEntryIntent, ShortEntryPolicyDecision,
};
use mqk_daemon::decision::bar_result_to_decisions;
use mqk_daemon::state::{
    dry_run_strategy_ids_from_env, evaluate_dry_run_strategies, evaluate_dry_run_strategy,
    DRY_RUN_STRATEGY_IDS_ENV,
};
use mqk_strategy::{
    BarStub, IntentMode, RecentBarsWindow, StrategyBarResult, StrategyIntents, StrategyOutput,
    StrategySpec, TargetPosition,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn bar(close_micros: i64, is_complete: bool) -> BarStub {
    BarStub::new(0, is_complete, close_micros, 1)
}

fn bullish_window() -> RecentBarsWindow {
    let base = 200_000_000i64; // $200
    RecentBarsWindow::new(
        10,
        vec![
            bar(base, true),
            bar(base, true),
            bar(base, true),
            bar(base, true),
            bar(base + base / 400, true), // +25 bps >= 20 bps threshold
        ],
    )
}

fn bearish_window() -> RecentBarsWindow {
    let base = 200_000_000i64;
    RecentBarsWindow::new(
        10,
        vec![
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base, true), // -25 bps <= -20 bps threshold
        ],
    )
}

fn flat() -> BTreeMap<String, i64> {
    BTreeMap::new()
}

fn fixed_run_id() -> Uuid {
    Uuid::nil()
}

const FIXED_NOW_MICROS: i64 = 1_700_000_000_000_000;

// ---------------------------------------------------------------------------
// DR01 — no dry-run IDs configured -> unchanged single-strategy behavior
// ---------------------------------------------------------------------------

#[test]
fn dr01_no_dry_run_ids_configured_is_a_pure_noop() {
    std::env::remove_var(DRY_RUN_STRATEGY_IDS_ENV);
    let ids = dry_run_strategy_ids_from_env();
    assert!(
        ids.is_empty(),
        "DR01: absent MQK_DRY_RUN_STRATEGY_IDS -> empty list (default off)"
    );
    // Calling the plural evaluator with an empty list is a no-op: zero
    // diagnostics, zero registry/host construction, zero evaluation.
    let diags = evaluate_dry_run_strategies(&ids, "AAPL", 0, &bullish_window(), 0);
    assert!(
        diags.is_empty(),
        "DR01: empty id list -> empty diagnostics; nothing evaluated"
    );

    // The existing primary path is unaffected by the dry-run module's mere
    // existence: a normal bullish long-strategy result still produces a buy
    // decision through the untouched `bar_result_to_decisions`.
    let primary_result = StrategyBarResult {
        spec: StrategySpec::new("intraday_scalper", 300),
        intents: StrategyIntents {
            mode: IntentMode::Live,
            output: StrategyOutput {
                targets: vec![TargetPosition {
                    symbol: "AAPL".to_string(),
                    qty: 1,
                }],
            },
        },
    };
    let decisions =
        bar_result_to_decisions(&primary_result, fixed_run_id(), FIXED_NOW_MICROS, &flat());
    assert_eq!(
        decisions.len(),
        1,
        "DR01: primary long decision path unchanged (one buy decision)"
    );
    assert_eq!(decisions[0].side, "buy", "DR01: buy from flat to target=1");
    assert_eq!(decisions[0].qty, 1, "DR01: qty=1");
}

// ---------------------------------------------------------------------------
// DR02 — dry-run secondary evaluated alongside primary on the same window
// ---------------------------------------------------------------------------

#[test]
fn dr02_secondary_evaluated_alongside_primary_same_window() {
    let ids = vec![
        "intraday_scalper".to_string(),
        "intraday_short_scalper".to_string(),
    ];
    let window = bearish_window();
    let diags = evaluate_dry_run_strategies(&ids, "AAPL", 0, &window, 0);

    assert_eq!(
        diags.len(),
        2,
        "DR02: both primary-shaped and secondary ids evaluated against the same window"
    );
    let long = diags
        .iter()
        .find(|d| d.strategy_id == "intraday_scalper")
        .expect("DR02: intraday_scalper diagnostic present");
    let short = diags
        .iter()
        .find(|d| d.strategy_id == "intraday_short_scalper")
        .expect("DR02: intraday_short_scalper diagnostic present");

    // Same bearish window, two different (independent) strategy identities:
    // long-only goes flat; short-only opens short. Proves both were actually
    // evaluated (not stubbed) and did not influence each other.
    assert_eq!(
        long.target_qty, 0,
        "DR02: long-only strategy is flat on bearish"
    );
    assert!(
        short.target_qty < 0,
        "DR02: short-only strategy opens short on bearish"
    );
}

// ---------------------------------------------------------------------------
// DR03 — primary long strategy result remains eligible for the existing path
// ---------------------------------------------------------------------------

#[test]
fn dr03_primary_long_strategy_result_remains_eligible_for_existing_path() {
    // Construct the result the way the real (untouched) primary StrategyHost
    // would for a bullish intraday_scalper bar, and confirm it still flows
    // through the unmodified `bar_result_to_decisions` exactly as before
    // this patch: buy decision, side/qty correct, B5 not involved.
    let result = StrategyBarResult {
        spec: StrategySpec::new("intraday_scalper", 300),
        intents: StrategyIntents {
            mode: IntentMode::Live,
            output: StrategyOutput {
                targets: vec![TargetPosition {
                    symbol: "AAPL".to_string(),
                    qty: 3,
                }],
            },
        },
    };
    let mut current = BTreeMap::new();
    current.insert("AAPL".to_string(), 1i64);

    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &current);
    assert_eq!(decisions.len(), 1, "DR03: one decision produced");
    assert_eq!(decisions[0].side, "buy", "DR03: buy to extend long 1 -> 3");
    assert_eq!(decisions[0].qty, 2, "DR03: delta = 3 - 1 = 2");
    assert_eq!(decisions[0].strategy_id, "intraday_scalper");
}

// ---------------------------------------------------------------------------
// DR04 — secondary diagnostic always has submitted=false
// ---------------------------------------------------------------------------

#[test]
fn dr04_secondary_diagnostic_always_has_submitted_false() {
    let scenarios = [
        ("intraday_short_scalper", bearish_window(), 0i64),
        ("intraday_short_scalper", bullish_window(), 0i64),
        ("intraday_scalper", bullish_window(), 0i64),
        ("intraday_scalper", bearish_window(), 5i64),
    ];
    for (id, window, current) in scenarios {
        let diag = evaluate_dry_run_strategy(id, "AAPL", 0, window, current);
        assert!(
            !diag.submitted,
            "DR04: submitted must always be false for strategy_id={id}"
        );
    }
}

// ---------------------------------------------------------------------------
// DR05 — secondary strategy cannot call submit_internal_strategy_decision
// ---------------------------------------------------------------------------

#[test]
fn dr05_evaluator_is_a_plain_sync_fn_with_no_appstate_capability() {
    // Structural proof, not a behavioral assertion: `evaluate_dry_run_strategy`
    // and `evaluate_dry_run_strategies` are plain (non-async) functions whose
    // signatures take a strategy id, symbol, tick, bar window, and qty — no
    // `&Arc<AppState>`, no `Arc<AppState>` of any kind. This test calls them
    // from an ordinary `#[test]` fn with no tokio runtime present at all; if
    // either function internally awaited or otherwise reached
    // `submit_internal_strategy_decision` (an `async fn` requiring
    // `&Arc<AppState>`), this would not compile, let alone run, without a
    // runtime. It compiles and runs synchronously to completion below.
    let diag = evaluate_dry_run_strategy("intraday_short_scalper", "AAPL", 0, bearish_window(), 0);
    assert!(!diag.submitted, "DR05: no submission occurred");
}

// ---------------------------------------------------------------------------
// DR06 — secondary strategy cannot write an outbox row
// ---------------------------------------------------------------------------

#[test]
fn dr06_evaluator_has_no_db_handle_and_is_referentially_transparent() {
    // Structural proof: the evaluator takes no `PgPool` / DB handle of any
    // kind, so it cannot reach `mqk_db::outbox_enqueue`. As a corollary of
    // being pure, calling it twice with identical inputs must yield identical
    // output (no hidden counter, idempotency key, or mutable state) — unlike
    // outbox enqueue, which is idempotent-by-key but stateful.
    let a = evaluate_dry_run_strategy("intraday_short_scalper", "AAPL", 0, bearish_window(), 0);
    let b = evaluate_dry_run_strategy("intraday_short_scalper", "AAPL", 0, bearish_window(), 0);
    assert_eq!(
        a, b,
        "DR06: pure function; identical inputs -> identical output"
    );
}

// ---------------------------------------------------------------------------
// DR07 — secondary bearish short signal is classified ShortOpen
// ---------------------------------------------------------------------------

#[test]
fn dr07_secondary_bearish_signal_classified_as_short_open() {
    let diag = evaluate_dry_run_strategy(
        "intraday_short_scalper",
        "AAPL",
        0,
        bearish_window(),
        0, // flat
    );
    assert_eq!(
        diag.would_classify_as, "ShortOpen",
        "DR07: bearish short-scalper from flat classifies as ShortOpen"
    );
}

// ---------------------------------------------------------------------------
// DR08 — secondary short-open diagnostic reports B5 + default policy block
// ---------------------------------------------------------------------------

#[test]
fn dr08_secondary_short_open_reports_b5_and_default_policy_block() {
    let diag = evaluate_dry_run_strategy("intraday_short_scalper", "AAPL", 0, bearish_window(), 0);
    assert!(
        diag.would_b5_block,
        "DR08: would_b5_block=true for ShortOpen"
    );
    assert_eq!(
        diag.decision, "b5_short_sale_guard",
        "DR08: decision names the B5 guard"
    );

    // Cross-check against the real (untouched) short-entry policy evaluator:
    // with no policy file configured, ShortEntryConfig::fail_closed() must
    // also block this exact intent in paper mode.
    let policy_decision = evaluate_short_entry_policy(
        &ShortEntryConfig::fail_closed(),
        ShortEntryIntent::ShortOpen,
        true, // paper mode
        None,
        None,
        None,
    );
    assert!(
        matches!(
            policy_decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { .. }
        ),
        "DR08: default fail-closed short-entry policy also blocks ShortOpen in paper mode"
    );
}

// ---------------------------------------------------------------------------
// DR09 — live shorting remains blocked by the existing policy gate
// ---------------------------------------------------------------------------

#[test]
fn dr09_live_shorting_remains_blocked_regardless_of_config() {
    // Even with every gate flipped on, live mode is an unconditional block —
    // this patch does not touch `short_entry_policy.rs` and re-verifies the
    // existing invariant from this new test file too.
    let permissive = ShortEntryConfig {
        allow_short_entries: true,
        paper_short_entries_enabled: true,
        live_short_entries_enabled: true,
        require_shortable_check: false,
        max_short_notional_usd: None,
        max_short_shares: None,
    };
    let decision = evaluate_short_entry_policy(
        &permissive,
        ShortEntryIntent::ShortOpen,
        false, // live mode
        Some(true),
        None,
        None,
    );
    assert_eq!(
        decision,
        ShortEntryPolicyDecision::ShortOpenBlocked {
            reason_code: "live_short_entries_blocked",
            detail: "short-open rejected: live short entries are not supported in this version; \
                     the live_short_entries_blocked invariant is enforced regardless of \
                     live_short_entries_enabled in the policy file"
                .to_string(),
        },
        "DR09: live mode is blocked unconditionally, even with a fully permissive config"
    );
}
