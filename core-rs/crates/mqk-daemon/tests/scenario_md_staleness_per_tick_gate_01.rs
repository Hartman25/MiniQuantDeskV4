//! MD-STALENESS-PER-TICK-GATE-01: per-dispatch-tick market-data staleness gate
//! proof.
//!
//! Proves the fail-closed staleness gate added to
//! `AppState::dispatch_native_strategy_for_symbol_with_bar` (cap #9, design
//! doc §6 "per_symbol_bar_staleness_guard"): before a per-symbol/per-tick
//! strategy dispatch can produce a `StrategyBarResult` (and therefore a
//! target/intent/order), the latest completed `md_bars` row for that
//! symbol/timeframe is checked against the configured staleness threshold via
//! the pure `classify_bar_staleness` helper (PER-SYMBOL-BAR-WINDOW-01).
//!
//! All tests are DB-backed (they read/write `md_bars`) and use the
//! established graceful-skip pattern: without `MQK_DATABASE_URL` pointing at
//! the paper DB, each test prints a skip notice and returns (passes
//! trivially), matching `scenario_data_freshness_readiness_gate_01.rs`.
//!
//! # Proof matrix
//!
//! | Test | What it proves                                                                |
//! |------|--------------------------------------------------------------------------------|
//! | S01  | Fresh bar within threshold -> `tick_strategy_dispatch_for_symbol` returns `Some` |
//! | S02  | Stale bar beyond threshold -> returns `None` (fail-closed, no target/intent/order) |
//! | S03  | Missing bar (no `md_bars` rows) -> returns `None` (fail-closed)              |
//! | S04  | Multi-symbol: stale symbol blocked without falsely blocking a fresh symbol    |
//! | S05  | Boundary: age just under cap dispatches, age just over cap is refused        |
//!
//! S05 only proves the boundary on the production wall-clock path with a +/-5s
//! buffer (avoids flakiness against `Utc::now()`); the exact `age == cap`
//! strict-`>` semantics of `classify_bar_staleness` are already proven by
//! P07/P08 in `scenario_per_symbol_bar_window_01.rs`.
//!
//! # Cross-references
//! - `core-rs/crates/mqk-daemon/src/state.rs`
//!   (`dispatch_native_strategy_for_symbol_with_bar`)
//! - `core-rs/crates/mqk-daemon/src/state/signal_intake.rs`
//!   (`per_symbol_bar_staleness_secs`, cap #9)
//! - `core-rs/crates/mqk-daemon/tests/scenario_per_symbol_bar_window_01.rs`
//!   (P05-P09: pure `classify_bar_staleness` boundary proofs)
//! - `docs/design/native_multi_symbol_dispatch.md` §6 (cap #9)

use std::sync::Arc;

use chrono::Utc;
use mqk_daemon::state::{self, AppState, StrategyBarInput, SymbolStrategyAssignment};
use mqk_runtime::native_strategy::{build_daemon_plugin_registry, NativeStrategyBootstrap};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Same paper-DB allowlist guard used by `scenario_data_freshness_readiness_gate_01.rs`.
fn get_paper_db_url() -> Option<String> {
    let url = std::env::var("MQK_DATABASE_URL").ok()?;
    if url.contains(":5440") || url.contains("miniquantdesk_paper") {
        Some(url)
    } else {
        None
    }
}

fn active_bootstrap() -> NativeStrategyBootstrap {
    let reg = build_daemon_plugin_registry();
    let ids = vec!["swing_momentum".to_string()];
    NativeStrategyBootstrap::bootstrap(Some(&ids), &reg)
}

fn test_bar_input(now_tick: u64) -> StrategyBarInput {
    StrategyBarInput {
        now_tick,
        end_ts: 1_700_000_000,
        limit_price: Some(150_000_000),
        qty: 10,
    }
}

fn assignment(symbol: &str, timeframe: &str) -> SymbolStrategyAssignment {
    SymbolStrategyAssignment {
        symbol: symbol.to_string(),
        strategy_id: "swing_momentum".to_string(),
        timeframe: timeframe.to_string(),
    }
}

/// Replace any existing `md_bars` rows for `symbol`/`timeframe` with a single
/// completed bar at `end_ts`.
///
/// Raw SQL: no insert helper exists for `md_bars` outside the CSV/provider
/// ingest paths (which apply additional validation irrelevant to this
/// fixture). Matches the column set from migration 0003_backtest_schema.sql.
async fn seed_one_bar(pool: &sqlx::PgPool, symbol: &str, timeframe: &str, end_ts: i64) {
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(symbol)
        .bind(timeframe)
        .execute(pool)
        .await
        .expect("delete prior md_bars rows");

    sqlx::query(
        "insert into md_bars \
           (symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros, volume, is_complete) \
         values ($1, $2, $3, $4, $4, $4, $4, $5, true)",
    )
    .bind(symbol)
    .bind(timeframe)
    .bind(end_ts)
    .bind(100_000_000_i64)
    .bind(1_000_i64)
    .execute(pool)
    .await
    .expect("insert md_bars fixture row");
}

/// Remove any existing `md_bars` rows for `symbol`/`timeframe`, leaving none
/// (S03: missing bar case).
async fn seed_no_bars(pool: &sqlx::PgPool, symbol: &str, timeframe: &str) {
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(symbol)
        .bind(timeframe)
        .execute(pool)
        .await
        .expect("delete prior md_bars rows");
}

/// Build a DB-backed `AppState` with an active native strategy bootstrap
/// (`swing_momentum`), in `LiveShadow`+`Alpaca` mode (matches
/// `scenario_multi_symbol_dispatch_loop_01.rs`'s `bare_state` shape, plus a
/// DB pool so `dispatch_native_strategy_for_symbol_with_bar` takes the
/// DB-bar-window path where the staleness gate lives).
async fn db_state(pool: sqlx::PgPool) -> Arc<AppState> {
    let st = AppState::new_for_test_with_db_mode_and_broker(
        pool,
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    );
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;
    Arc::new(st)
}

async fn connect(db_url: &str) -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(db_url)
        .await
        .expect("pool connect failed")
}

// ---------------------------------------------------------------------------
// S01 — fresh bar within threshold dispatches normally
// ---------------------------------------------------------------------------

/// S01: A completed bar well within the staleness cap dispatches exactly as
/// before — `tick_strategy_dispatch_for_symbol` returns `Some`.
#[tokio::test]
async fn s01_fresh_bar_within_threshold_dispatches() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("S01: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connect(&db_url).await;

    let symbol = "MDS01FRESH";
    let timeframe = "1Min";
    let now_ts = Utc::now().timestamp();

    // 60s old, well within a 300s cap.
    seed_one_bar(&pool, symbol, timeframe, now_ts - 60).await;

    let st = db_state(pool).await;
    st.set_per_symbol_bar_staleness_secs_for_test(Some(300));
    st.deposit_strategy_bar_input(test_bar_input(1)).await;

    let result = st
        .tick_strategy_dispatch_for_symbol(symbol, timeframe)
        .await;
    assert!(
        result.is_some(),
        "S01: fresh bar (age=60s, cap=300s) must dispatch normally"
    );
}

// ---------------------------------------------------------------------------
// S02 — stale bar beyond threshold blocks dispatch
// ---------------------------------------------------------------------------

/// S02: A completed bar far older than the staleness cap must fail closed:
/// `tick_strategy_dispatch_for_symbol` returns `None` (no target/intent/order).
#[tokio::test]
async fn s02_stale_bar_beyond_threshold_blocks_dispatch() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("S02: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connect(&db_url).await;

    let symbol = "MDS02STALE";
    let timeframe = "1Min";
    let now_ts = Utc::now().timestamp();

    // ~2.8 hours old, far beyond a 300s cap.
    seed_one_bar(&pool, symbol, timeframe, now_ts - 10_000).await;

    let st = db_state(pool).await;
    st.set_per_symbol_bar_staleness_secs_for_test(Some(300));
    st.deposit_strategy_bar_input(test_bar_input(1)).await;

    let result = st
        .tick_strategy_dispatch_for_symbol(symbol, timeframe)
        .await;
    assert!(
        result.is_none(),
        "S02: stale bar (age=10000s, cap=300s) must block dispatch (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// S03 — missing bar blocks dispatch
// ---------------------------------------------------------------------------

/// S03: No completed bars at all for this symbol/timeframe must fail closed:
/// `tick_strategy_dispatch_for_symbol` returns `None`.
#[tokio::test]
async fn s03_missing_bar_blocks_dispatch() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("S03: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connect(&db_url).await;

    let symbol = "MDS03MISSING";
    let timeframe = "1Min";
    seed_no_bars(&pool, symbol, timeframe).await;

    let st = db_state(pool).await;
    st.set_per_symbol_bar_staleness_secs_for_test(Some(300));
    st.deposit_strategy_bar_input(test_bar_input(1)).await;

    let result = st
        .tick_strategy_dispatch_for_symbol(symbol, timeframe)
        .await;
    assert!(
        result.is_none(),
        "S03: missing bar (zero md_bars rows) must block dispatch (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// S04 — multi-symbol: stale symbol blocked, fresh symbol unaffected
// ---------------------------------------------------------------------------

/// S04: With one fresh symbol and one stale symbol assigned in the same tick,
/// `tick_strategy_dispatch_multi_symbol` dispatches the fresh symbol normally
/// and omits the stale symbol — the stale gate is per-symbol, not global.
#[tokio::test]
async fn s04_multi_symbol_stale_blocked_fresh_unaffected() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("S04: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connect(&db_url).await;

    let timeframe = "1Min";
    let fresh_symbol = "MDS04FRESH";
    let stale_symbol = "MDS04STALE";
    let now_ts = Utc::now().timestamp();

    seed_one_bar(&pool, fresh_symbol, timeframe, now_ts - 60).await;
    seed_one_bar(&pool, stale_symbol, timeframe, now_ts - 10_000).await;

    let st = db_state(pool).await;
    st.set_per_symbol_bar_staleness_secs_for_test(Some(300));
    st.deposit_strategy_bar_input(test_bar_input(1)).await;

    let results = st
        .tick_strategy_dispatch_multi_symbol(&[
            assignment(fresh_symbol, timeframe),
            assignment(stale_symbol, timeframe),
        ])
        .await;

    assert!(
        results.iter().any(|(a, _)| a.symbol == fresh_symbol),
        "S04: fresh symbol must be present in dispatch results, got: {:?}",
        results.iter().map(|(a, _)| &a.symbol).collect::<Vec<_>>()
    );
    assert!(
        results.iter().all(|(a, _)| a.symbol != stale_symbol),
        "S04: stale symbol must be absent from dispatch results, got: {:?}",
        results.iter().map(|(a, _)| &a.symbol).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// S05 — boundary: age just under cap dispatches, age just over cap refused
// ---------------------------------------------------------------------------

/// S05: With cap=300s, a bar aged 295s (cap-5) dispatches normally and a bar
/// aged 305s (cap+5) is refused. A +/-5s buffer avoids flakiness against the
/// production path's real `Utc::now()` call; the exact `age == cap` strict-`>`
/// boundary is proven purely by P07/P08 in
/// `scenario_per_symbol_bar_window_01.rs`.
#[tokio::test]
async fn s05_boundary_just_under_and_over_cap() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("S05: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let pool = connect(&db_url).await;

    let timeframe = "1Min";
    let cap_secs: i64 = 300;
    let now_ts = Utc::now().timestamp();

    let ok_symbol = "MDS05BOUNDOK";
    let bad_symbol = "MDS05BOUNDBAD";

    seed_one_bar(&pool, ok_symbol, timeframe, now_ts - (cap_secs - 5)).await;
    seed_one_bar(&pool, bad_symbol, timeframe, now_ts - (cap_secs + 5)).await;

    // age=295s, cap=300s -> not stale -> dispatches.
    let st_ok = db_state(pool.clone()).await;
    st_ok.set_per_symbol_bar_staleness_secs_for_test(Some(cap_secs));
    st_ok.deposit_strategy_bar_input(test_bar_input(1)).await;
    let ok_result = st_ok
        .tick_strategy_dispatch_for_symbol(ok_symbol, timeframe)
        .await;
    assert!(
        ok_result.is_some(),
        "S05: bar aged cap-5={}s (cap={}s) must dispatch normally",
        cap_secs - 5,
        cap_secs
    );

    // age=305s, cap=300s -> stale -> refused.
    let st_bad = db_state(pool).await;
    st_bad.set_per_symbol_bar_staleness_secs_for_test(Some(cap_secs));
    st_bad.deposit_strategy_bar_input(test_bar_input(1)).await;
    let bad_result = st_bad
        .tick_strategy_dispatch_for_symbol(bad_symbol, timeframe)
        .await;
    assert!(
        bad_result.is_none(),
        "S05: bar aged cap+5={}s (cap={}s) must be refused (fail-closed)",
        cap_secs + 5,
        cap_secs
    );
}
