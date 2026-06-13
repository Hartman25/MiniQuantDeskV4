//! PER-SYMBOL-BAR-WINDOW-01: per-symbol bar input/window foundation proof
//! matrix.
//!
//! Proves the additive, symbol-keyed types and helpers added in
//! `core-rs/crates/mqk-daemon/src/state/per_symbol_bar_window.rs`:
//!
//! - [`mqk_daemon::state::PerSymbolPendingBarInputs`] — symbol-keyed pending
//!   bar input map (insert/take, normalization, isolation).
//! - [`mqk_daemon::state::classify_bar_staleness`] — pure staleness
//!   classification for the future cap #9.
//! - [`mqk_daemon::state::per_symbol_loaded_bars_from_rows`] — pure
//!   conversion from `mqk_db::MdBarRow` to
//!   [`mqk_daemon::state::PerSymbolLoadedBars`] (no DB connection required).
//! - [`mqk_daemon::state::AppState::tick_strategy_dispatch_for_symbol`] —
//!   symbol/timeframe-parameterized extraction of the legacy single-symbol
//!   dispatch body.
//!
//! All tests are pure in-process; no DB or network required.
//!
//! # Proof matrix
//!
//! | Test | What it proves                                                          |
//! |------|--------------------------------------------------------------------------|
//! | P01  | Pending inputs: insert + take one symbol                                |
//! | P02  | Pending inputs: taking AAPL does not remove MSFT                        |
//! | P03  | Pending inputs: symbol normalized (trim + uppercase)                    |
//! | P04  | Pending inputs: empty symbol is rejected, map stays empty               |
//! | P05  | Stale helper: `None` when cap disabled                                  |
//! | P06  | Stale helper: `Some(true)` when no latest bar and cap enabled           |
//! | P07  | Stale helper: `Some(true)` when latest bar older than cap               |
//! | P08  | Stale helper: `Some(false)` when latest bar within cap                  |
//! | P09  | Stale helper: future `latest_end_ts` does not panic, treated not stale  |
//! | P10  | `PerSymbolBarWindow` preserves symbol/timeframe + ordering metadata     |
//! | P11  | Missing bars represented honestly (`bars_loaded == 0`), not fabricated |
//! | P12  | Legacy `tick_strategy_dispatch()` remains compatible after refactor     |
//! | P13  | `tick_strategy_dispatch_for_symbol` is additive/independent of env path |
//! | P14  | New types do not introduce `approved_for_live`                         |
//!
//! # Cross-references
//! - `docs/design/native_multi_symbol_dispatch.md` §4.4, §6 (cap #9)
//! - `core-rs/crates/mqk-daemon/src/state/per_symbol_bar_window.rs`
//! - `core-rs/crates/mqk-daemon/src/state.rs`
//!   (`tick_strategy_dispatch`, `tick_strategy_dispatch_for_symbol`)

use mqk_daemon::state::{
    self, classify_bar_staleness, per_symbol_loaded_bars_from_rows, AppState, EmptySymbolError,
    PerSymbolBarInput, PerSymbolPendingBarInputs, StrategyBarInput,
};
use mqk_db::MdBarRow;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sample_input(now_tick: u64) -> PerSymbolBarInput {
    PerSymbolBarInput {
        symbol: String::new(),
        now_tick,
        end_ts: 1_700_000_000,
        limit_price: Some(100_000_000),
        qty: 10,
    }
}

fn sample_row(symbol: &str, timeframe: &str, end_ts: i64, close_micros: i64) -> MdBarRow {
    MdBarRow {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        end_ts,
        open_micros: close_micros,
        high_micros: close_micros,
        low_micros: close_micros,
        close_micros,
        volume: 1_000,
        is_complete: true,
    }
}

// ---------------------------------------------------------------------------
// P01-P04 — PerSymbolPendingBarInputs
// ---------------------------------------------------------------------------

/// P01: insert then take one symbol returns the deposited input, normalized.
#[test]
fn p01_pending_inputs_insert_and_take_one_symbol() {
    let mut pending = PerSymbolPendingBarInputs::new();
    assert!(pending.is_empty());

    pending.insert("AAPL", sample_input(1)).unwrap();
    assert_eq!(pending.len(), 1);

    let taken = pending.take("AAPL").expect("AAPL must be present");
    assert_eq!(taken.symbol, "AAPL");
    assert_eq!(taken.now_tick, 1);
    assert!(pending.is_empty());
}

/// P02: taking AAPL does not remove or modify MSFT's pending entry.
#[test]
fn p02_pending_inputs_isolate_aapl_from_msft() {
    let mut pending = PerSymbolPendingBarInputs::new();
    pending.insert("AAPL", sample_input(1)).unwrap();
    pending.insert("MSFT", sample_input(2)).unwrap();
    assert_eq!(pending.len(), 2);

    let aapl = pending.take("AAPL").expect("AAPL must be present");
    assert_eq!(aapl.symbol, "AAPL");
    assert_eq!(aapl.now_tick, 1);
    assert_eq!(pending.len(), 1, "MSFT entry must remain after taking AAPL");

    let msft = pending.take("MSFT").expect("MSFT must still be present");
    assert_eq!(msft.symbol, "MSFT");
    assert_eq!(msft.now_tick, 2);
    assert!(pending.is_empty());
}

/// P03: symbols are trimmed and uppercased on insert; no truncation.
#[test]
fn p03_pending_inputs_normalize_symbol() {
    let mut pending = PerSymbolPendingBarInputs::new();
    pending.insert("  aapl  ", sample_input(1)).unwrap();
    assert_eq!(pending.len(), 1);

    let taken = pending
        .take("AAPL")
        .expect("normalized 'AAPL' key must be present");
    assert_eq!(taken.symbol, "AAPL");

    // Long symbol (no truncation): round-trips intact.
    let mut pending2 = PerSymbolPendingBarInputs::new();
    pending2.insert(" brk.b ", sample_input(3)).unwrap();
    let taken2 = pending2.take("BRK.B").expect("BRK.B must round-trip");
    assert_eq!(taken2.symbol, "BRK.B");
}

/// P04: an empty (or whitespace-only) symbol is rejected; map stays empty.
#[test]
fn p04_pending_inputs_reject_empty_symbol() {
    let mut pending = PerSymbolPendingBarInputs::new();

    assert_eq!(pending.insert("", sample_input(1)), Err(EmptySymbolError));
    assert_eq!(
        pending.insert("   ", sample_input(1)),
        Err(EmptySymbolError)
    );
    assert!(pending.is_empty());
}

// ---------------------------------------------------------------------------
// P05-P09 — classify_bar_staleness
// ---------------------------------------------------------------------------

/// P05: cap disabled (`None`) => `None`, regardless of latest bar presence.
#[test]
fn p05_stale_helper_none_when_cap_disabled() {
    assert_eq!(classify_bar_staleness(Some(1_000), 2_000, None), None);
    assert_eq!(classify_bar_staleness(None, 2_000, None), None);
}

/// P06: no latest bar at all, cap enabled => `Some(true)`.
#[test]
fn p06_stale_helper_stale_when_no_latest_bar_and_cap_enabled() {
    assert_eq!(classify_bar_staleness(None, 2_000, Some(60)), Some(true));
}

/// P07: latest bar older than the cap => `Some(true)`.
#[test]
fn p07_stale_helper_stale_when_older_than_cap() {
    // now=2000, latest=1000 -> age=1000 > cap=60 -> stale.
    assert_eq!(
        classify_bar_staleness(Some(1_000), 2_000, Some(60)),
        Some(true)
    );
}

/// P08: latest bar within the cap => `Some(false)`.
#[test]
fn p08_stale_helper_not_stale_when_within_cap() {
    // now=2000, latest=1995 -> age=5 <= cap=60 -> not stale.
    assert_eq!(
        classify_bar_staleness(Some(1_995), 2_000, Some(60)),
        Some(false)
    );
}

/// P09: a future `latest_end_ts` does not panic and is treated as not stale
/// (age clamped to zero, never negative).
#[test]
fn p09_stale_helper_future_latest_end_ts_safe() {
    assert_eq!(
        classify_bar_staleness(Some(3_000), 2_000, Some(60)),
        Some(false)
    );
    // Cap of zero with a future bar: age clamps to 0, 0 > 0 is false.
    assert_eq!(classify_bar_staleness(Some(3_000), 2_000, Some(0)), Some(false));
}

// ---------------------------------------------------------------------------
// P10-P11 — per_symbol_loaded_bars_from_rows (pure conversion, no DB)
// ---------------------------------------------------------------------------

/// P10: window metadata preserves symbol/timeframe and reflects the
/// oldest-first input ordering (latest_end_ts == last row's end_ts).
#[test]
fn p10_per_symbol_window_preserves_symbol_and_timeframe() {
    let rows = vec![
        sample_row("AAPL", "1Min", 1_000, 100_000_000),
        sample_row("AAPL", "1Min", 1_060, 101_000_000),
        sample_row("AAPL", "1Min", 1_120, 102_000_000),
    ];

    let loaded = per_symbol_loaded_bars_from_rows("AAPL", "1Min", &rows);

    assert_eq!(loaded.window.symbol, "AAPL");
    assert_eq!(loaded.window.timeframe, "1Min");
    assert_eq!(loaded.window.bars_loaded, 3);
    assert_eq!(loaded.window.latest_end_ts, Some(1_120));
    assert!(loaded.window.latest_end_utc.is_some());
    // is_stale is not classified by this pure conversion.
    assert_eq!(loaded.window.is_stale, None);

    // Bars preserve oldest-first ordering and end_ts values.
    assert_eq!(loaded.bars.len(), 3);
    assert_eq!(loaded.bars[0].end_ts, 1_000);
    assert_eq!(loaded.bars[2].end_ts, 1_120);
}

/// P11: empty input rows produce an honest "no bars" result — no
/// fabricated bars, no fabricated latest timestamp.
#[test]
fn p11_missing_bars_represented_honestly_not_fabricated() {
    let rows: Vec<MdBarRow> = Vec::new();

    let loaded = per_symbol_loaded_bars_from_rows("AAPL", "1Min", &rows);

    assert_eq!(loaded.window.symbol, "AAPL");
    assert_eq!(loaded.window.timeframe, "1Min");
    assert_eq!(loaded.window.bars_loaded, 0);
    assert_eq!(loaded.window.latest_end_ts, None);
    assert_eq!(loaded.window.latest_end_utc, None);
    assert_eq!(loaded.window.is_stale, None);
    assert!(loaded.bars.is_empty());
}

// ---------------------------------------------------------------------------
// P12-P13 — tick_strategy_dispatch / tick_strategy_dispatch_for_symbol
// ---------------------------------------------------------------------------

/// P12: the legacy zero-arg `tick_strategy_dispatch()` entrypoint still
/// works end-to-end after delegating to `tick_strategy_dispatch_for_symbol`.
///
/// With no DB pool and no native strategy bootstrap (fresh `AppState::new()`),
/// the original behavior is `None` (fail-closed: no bootstrap). The
/// refactored delegation must preserve this exact result shape.
#[tokio::test]
async fn p12_legacy_tick_dispatch_compatible_after_refactor() {
    let st = AppState::new();

    st.deposit_strategy_bar_input(StrategyBarInput {
        now_tick: 1,
        end_ts: 1_700_000_000,
        limit_price: Some(100_000_000),
        qty: 10,
    })
    .await;

    let result = st.tick_strategy_dispatch().await;
    assert!(
        result.is_none(),
        "P12: legacy tick_strategy_dispatch() must remain fail-closed (None) \
         with no native strategy bootstrap, unchanged after the refactor"
    );

    // The pending bar was consumed by the dispatch above.
    assert!(st.pending_strategy_bar_input_is_none_for_test().await);
}

/// P13: `tick_strategy_dispatch_for_symbol` is an additive, independently
/// callable seam — it does not require `MQK_STRATEGY_SYMBOL` /
/// `MQK_STRATEGY_MD_TIMEFRAME` to be set, and is not invoked by
/// `loop_runner.rs` (enforced separately by the
/// `test_per_symbol_bar_window.ps1` script guard, which asserts
/// `loop_runner.rs` has zero references to it).
///
/// With no DB pool and no native strategy bootstrap, calling it directly
/// with an explicit symbol/timeframe (independent of any env var state)
/// returns `None` — the same fail-closed shape as the legacy path.
#[tokio::test]
async fn p13_per_symbol_helper_independent_of_loop_dispatch() {
    let st = AppState::new();

    st.deposit_strategy_bar_input(StrategyBarInput {
        now_tick: 7,
        end_ts: 1_700_000_060,
        limit_price: None,
        qty: 0,
    })
    .await;

    let result = st
        .tick_strategy_dispatch_for_symbol("AAPL", "1Min")
        .await;
    assert!(
        result.is_none(),
        "P13: tick_strategy_dispatch_for_symbol must be fail-closed (None) \
         with no native strategy bootstrap, callable independent of env vars"
    );

    // No pending bar remains — calling it again with nothing pending is None.
    let result2 = st
        .tick_strategy_dispatch_for_symbol("AAPL", "1Min")
        .await;
    assert!(
        result2.is_none(),
        "P13: with no pending bar, tick_strategy_dispatch_for_symbol returns None"
    );
}

// ---------------------------------------------------------------------------
// P14 — no approved_for_live in the new types
// ---------------------------------------------------------------------------

/// P14: none of the new per-symbol types carry an `approved_for_live` field.
///
/// `Debug` output is derived (field-complete), so if any of these structs
/// had introduced `approved_for_live`, it would appear in the formatted
/// output below.
#[test]
fn p14_per_symbol_types_do_not_introduce_approved_for_live() {
    let input = sample_input(1);
    let rows = vec![sample_row("AAPL", "1Min", 1_000, 100_000_000)];
    let loaded = per_symbol_loaded_bars_from_rows("AAPL", "1Min", &rows);

    let mut pending = PerSymbolPendingBarInputs::new();
    pending.insert("AAPL", sample_input(1)).unwrap();

    let debug_blob = format!("{input:?} {loaded:?} {pending:?}");
    assert!(
        !debug_blob.contains("approved_for_live"),
        "PER-SYMBOL-BAR-WINDOW-01 types must not introduce approved_for_live: {debug_blob}"
    );

    // Touch the re-export to ensure `state::` path also resolves (Option A
    // additive module, registered in state.rs).
    let _ = state::PerSymbolBarWindowError::EmptySymbol;
}
