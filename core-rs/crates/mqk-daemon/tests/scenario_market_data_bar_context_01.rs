//! MARKET-DATA-BAR-CONTEXT-01 — Market-data bar context truth proof
//!
//! ## Purpose
//!
//! Proves that the autonomous readiness surface truthfully distinguishes the
//! exact reason why `bar_context_source = "stub_no_price"` occurs, so the
//! operator knows what to fix before the session opens.
//!
//! Two distinct sub-cases of stub_no_price are now surfaced:
//! - **Sub-case A** (AUTON-SIGNAL-CONTEXT-01): one or both env vars
//!   (`MQK_STRATEGY_SYMBOL`, `MQK_STRATEGY_MD_TIMEFRAME`) are absent →
//!   operator must configure them.
//! - **Sub-case B** (MARKET-DATA-BAR-CONTEXT-01): both env vars are set but
//!   `md_bars` contains no completed bars for that symbol/timeframe →
//!   operator must ingest historical data.
//!
//! None of these tests change gate logic, weaken fail-closed behaviour, or
//! produce fabricated signals, orders, fills, or market data.
//!
//! | Test  | Claim                                                                                                                       |
//! |-------|-----------------------------------------------------------------------------------------------------------------------------|
//! | MD01  | Both vars set + ctx_bars=0 → INCOMPLETE_BAR_CONTEXT (MARKET-DATA-BAR-CONTEXT-01) and NO_SIGNAL_GENERATED (MARKET-DATA-BAR-CONTEXT-01) |
//! | MD02  | MQK_STRATEGY_MD_TIMEFRAME absent + ctx_bars=0 → INCOMPLETE_BAR_CONTEXT (AUTON-SIGNAL-CONTEXT-01) and NO_SIGNAL_GENERATED (AUTON-NO-TRADE-02) |
//! | MD03  | Both vars set + ctx_bars>0 (db_loaded) → no INCOMPLETE_BAR_CONTEXT, NO_SIGNAL_GENERATED (AUTON-SIGNAL-CONTEXT-01) |
//! | MD04  | No bar ticks yet → bar_context_source=no_dispatch_yet, no NO_SIGNAL_GENERATED                                               |
//! | MD05  | db-sourced tick + signal>0 → no INCOMPLETE_BAR_CONTEXT, no NO_SIGNAL_GENERATED                                              |

use std::sync::Arc;
use std::sync::OnceLock;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use state::{AlpacaWsContinuityState, BrokerKind};
use tokio::sync::Mutex;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Env-var serialization
//
// Tests that set process-global env vars must not run concurrently with each
// other.  `tokio::sync::Mutex` is used so the guard can be held across await
// points without triggering the `await_holding_lock` clippy lint.
// ---------------------------------------------------------------------------
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    (status, body)
}

fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

fn make_live_paper_alpaca() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ))
}

async fn call_readiness(st: Arc<state::AppState>) -> serde_json::Value {
    let (_, body) = call(
        routes::build_router(st),
        Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    parse_json(body)
}

// ---------------------------------------------------------------------------
// MD01 — Both env vars set + ctx_bars=0 → MARKET-DATA-BAR-CONTEXT-01 blockers
//
// When MQK_STRATEGY_SYMBOL and MQK_STRATEGY_MD_TIMEFRAME are both configured
// but md_bars has no completed bars for that combination, the stub path fires.
// The readiness surface must distinguish this from the "env vars missing" case
// by naming the configured symbol/timeframe in the blocker text.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn md01_both_vars_set_no_db_bars_surfaces_market_data_blocker() {
    let _g = env_lock().lock().await;
    std::env::set_var("MQK_STRATEGY_SYMBOL", "TESTFOO");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");

    let st = make_live_paper_alpaca();
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "md01-msg".to_string(),
        last_event_at: "2026-03-30T15:00:00Z".to_string(),
    })
    .await;
    // 1 tick dispatched, signal=0, ctx_bars=0 (stub_no_price — both env vars
    // are set but DB returned no completed bars for TESTFOO/1D).
    st.set_bar_tick_state_for_test(1, 0, 0);

    let v = call_readiness(Arc::clone(&st)).await;

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");

    assert_eq!(
        v["bar_context_source"], "stub_no_price",
        "MD01: ctx_bars=0 must yield stub_no_price"
    );

    let blockers = v["blockers"].as_array().expect("blockers must be array");

    let ibc = blockers
        .iter()
        .find(|b| b.as_str().unwrap_or("").contains("INCOMPLETE_BAR_CONTEXT"))
        .expect("MD01: INCOMPLETE_BAR_CONTEXT blocker must appear when ctx_bars=0");
    let ibc_str = ibc.as_str().unwrap();
    assert!(
        ibc_str.contains("MARKET-DATA-BAR-CONTEXT-01"),
        "MD01: INCOMPLETE_BAR_CONTEXT must name MARKET-DATA-BAR-CONTEXT-01 when both env vars set; got: {ibc_str}"
    );
    assert!(
        ibc_str.contains("TESTFOO"),
        "MD01: blocker must include the configured symbol; got: {ibc_str}"
    );
    assert!(
        ibc_str.contains("1D"),
        "MD01: blocker must include the configured timeframe; got: {ibc_str}"
    );

    let nsg = blockers
        .iter()
        .find(|b| b.as_str().unwrap_or("").contains("NO_SIGNAL_GENERATED"))
        .expect("MD01: NO_SIGNAL_GENERATED blocker must appear when ticks>0 and signal=0");
    let nsg_str = nsg.as_str().unwrap();
    assert!(
        nsg_str.contains("MARKET-DATA-BAR-CONTEXT-01"),
        "MD01: NO_SIGNAL_GENERATED must name MARKET-DATA-BAR-CONTEXT-01 when both env vars set; got: {nsg_str}"
    );
    assert!(
        nsg_str.contains("TESTFOO"),
        "MD01: NO_SIGNAL_GENERATED must include the configured symbol; got: {nsg_str}"
    );
}

// ---------------------------------------------------------------------------
// MD02 — MQK_STRATEGY_MD_TIMEFRAME absent → AUTON-SIGNAL-CONTEXT-01 blockers
//
// When only MQK_STRATEGY_SYMBOL is set (MQK_STRATEGY_MD_TIMEFRAME absent),
// the readiness surface must name MQK_STRATEGY_MD_TIMEFRAME in the
// INCOMPLETE_BAR_CONTEXT blocker — sub-case A, not sub-case B.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn md02_timeframe_absent_surfaces_auton_signal_context_blocker() {
    let _g = env_lock().lock().await;
    std::env::set_var("MQK_STRATEGY_SYMBOL", "TESTFOO");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");

    let st = make_live_paper_alpaca();
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "md02-msg".to_string(),
        last_event_at: "2026-03-30T15:00:00Z".to_string(),
    })
    .await;
    st.set_bar_tick_state_for_test(1, 0, 0);

    let v = call_readiness(Arc::clone(&st)).await;

    std::env::remove_var("MQK_STRATEGY_SYMBOL");

    let blockers = v["blockers"].as_array().expect("blockers must be array");

    let ibc = blockers
        .iter()
        .find(|b| b.as_str().unwrap_or("").contains("INCOMPLETE_BAR_CONTEXT"))
        .expect("MD02: INCOMPLETE_BAR_CONTEXT must appear when timeframe is absent");
    let ibc_str = ibc.as_str().unwrap();
    assert!(
        ibc_str.contains("AUTON-SIGNAL-CONTEXT-01"),
        "MD02: INCOMPLETE_BAR_CONTEXT must name AUTON-SIGNAL-CONTEXT-01 when timeframe absent; got: {ibc_str}"
    );
    assert!(
        ibc_str.contains("MQK_STRATEGY_MD_TIMEFRAME"),
        "MD02: blocker must name MQK_STRATEGY_MD_TIMEFRAME as the missing var; got: {ibc_str}"
    );

    let nsg = blockers
        .iter()
        .find(|b| b.as_str().unwrap_or("").contains("NO_SIGNAL_GENERATED"))
        .expect("MD02: NO_SIGNAL_GENERATED must appear when ticks>0 and signal=0");
    let nsg_str = nsg.as_str().unwrap();
    assert!(
        nsg_str.contains("AUTON-NO-TRADE-02"),
        "MD02: NO_SIGNAL_GENERATED must be AUTON-NO-TRADE-02 when env vars missing; got: {nsg_str}"
    );
}

// ---------------------------------------------------------------------------
// MD03 — Both vars set + ctx_bars>0 (db_loaded) → no INCOMPLETE_BAR_CONTEXT
//
// When md_bars has completed bars and the strategy is invoked with a real DB
// window, the stub path was NOT used.  No INCOMPLETE_BAR_CONTEXT should fire.
// NO_SIGNAL_GENERATED fires because signal=0, naming AUTON-SIGNAL-CONTEXT-01.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn md03_db_loaded_context_no_incomplete_bar_context_blocker() {
    let _g = env_lock().lock().await;
    std::env::set_var("MQK_STRATEGY_SYMBOL", "TESTFOO");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");

    let st = make_live_paper_alpaca();
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "md03-msg".to_string(),
        last_event_at: "2026-03-30T15:00:00Z".to_string(),
    })
    .await;
    // ctx_bars=20 → db_loaded path; signal=0 → strategy conditions not met.
    st.set_bar_tick_state_for_test(1, 0, 20);

    let v = call_readiness(Arc::clone(&st)).await;

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");

    assert_eq!(
        v["bar_context_source"], "db_loaded",
        "MD03: ctx_bars=20 must yield db_loaded"
    );
    assert_eq!(
        v["bar_context_bars_loaded"], 20,
        "MD03: bar_context_bars_loaded must reflect bar count"
    );

    let blockers = v["blockers"].as_array().expect("blockers must be array");

    let has_incomplete = blockers
        .iter()
        .any(|b| b.as_str().unwrap_or("").contains("INCOMPLETE_BAR_CONTEXT"));
    assert!(
        !has_incomplete,
        "MD03: INCOMPLETE_BAR_CONTEXT must NOT appear when db_loaded; got: {blockers:?}"
    );

    let nsg = blockers
        .iter()
        .find(|b| b.as_str().unwrap_or("").contains("NO_SIGNAL_GENERATED"))
        .expect("MD03: NO_SIGNAL_GENERATED must appear when ticks>0 and signal=0");
    let nsg_str = nsg.as_str().unwrap();
    assert!(
        nsg_str.contains("AUTON-SIGNAL-CONTEXT-01"),
        "MD03: NO_SIGNAL_GENERATED must name AUTON-SIGNAL-CONTEXT-01 for db_loaded; got: {nsg_str}"
    );
}

// ---------------------------------------------------------------------------
// MD04 — No bar ticks yet → no_dispatch_yet, no NO_SIGNAL_GENERATED
//
// Before any bar tick has been dispatched, the readiness surface must report
// no_dispatch_yet and must NOT fire the NO_SIGNAL_GENERATED blocker.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn md04_no_ticks_yet_reports_no_dispatch_yet() {
    let st = make_live_paper_alpaca();
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "md04-msg".to_string(),
        last_event_at: "2026-03-30T15:00:00Z".to_string(),
    })
    .await;
    // No set_bar_tick_state_for_test — default sentinel state (no ticks).

    let v = call_readiness(Arc::clone(&st)).await;

    assert_eq!(
        v["bar_context_source"], "no_dispatch_yet",
        "MD04: before any tick bar_context_source must be no_dispatch_yet"
    );
    assert!(
        v["bar_context_bars_loaded"].is_null(),
        "MD04: bar_context_bars_loaded must be null before any tick"
    );

    let blockers = v["blockers"].as_array().expect("blockers must be array");
    assert!(
        !blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("NO_SIGNAL_GENERATED")),
        "MD04: NO_SIGNAL_GENERATED must NOT fire before any bar tick; got: {blockers:?}"
    );
    assert!(
        !blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("INCOMPLETE_BAR_CONTEXT")),
        "MD04: INCOMPLETE_BAR_CONTEXT must NOT fire before any bar tick; got: {blockers:?}"
    );
}

// ---------------------------------------------------------------------------
// MD05 — db-sourced tick + signal>0 → no INCOMPLETE_BAR_CONTEXT, no NO_SIGNAL
//
// When a complete DB-sourced bar drives the strategy and the strategy returns
// a non-zero signal, neither INCOMPLETE_BAR_CONTEXT nor NO_SIGNAL_GENERATED
// should appear.  Proves the happy path: real bars → real signal.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn md05_db_bar_with_nonzero_signal_no_blockers() {
    let st = make_live_paper_alpaca();
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "md05-msg".to_string(),
        last_event_at: "2026-03-30T15:00:00Z".to_string(),
    })
    .await;
    // ctx_bars=20 = db_loaded; signal=100 > 0 → strategy generated a target.
    st.set_bar_tick_state_for_test(1, 100, 20);

    let v = call_readiness(Arc::clone(&st)).await;

    assert_eq!(
        v["bar_context_source"], "db_loaded",
        "MD05: ctx_bars=20 must yield db_loaded"
    );
    assert_eq!(
        v["last_bar_signal_qty"], 100,
        "MD05: last_bar_signal_qty must reflect the strategy output"
    );

    let blockers = v["blockers"].as_array().expect("blockers must be array");
    assert!(
        !blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("INCOMPLETE_BAR_CONTEXT")),
        "MD05: INCOMPLETE_BAR_CONTEXT must NOT appear when db_loaded + signal>0; got: {blockers:?}"
    );
    assert!(
        !blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("NO_SIGNAL_GENERATED")),
        "MD05: NO_SIGNAL_GENERATED must NOT appear when signal_qty>0; got: {blockers:?}"
    );
}

// ---------------------------------------------------------------------------
// MD06 — Trailing \r in env vars is stripped (CRLF .env.local regression)
//
// On Windows, dotenvy 0.15 does not strip the trailing \r from values when
// the .env.local file uses CRLF line endings (UTF-8 with BOM).  Without the
// .trim() fix, MQK_STRATEGY_SYMBOL="AAPL\r" would be passed verbatim to the
// DB lookup and to readiness blocker text, causing "AAPL\r/1D\r" lookups that
// return 0 rows and blocker messages with embedded carriage-return characters.
//
// This test sets env vars with an explicit trailing \r and verifies:
//  - The INCOMPLETE_BAR_CONTEXT blocker references the trimmed symbol/timeframe.
//  - The blocker text does not contain \r (no raw carriage-return in output).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn md06_trailing_cr_in_env_vars_is_trimmed() {
    let _g = env_lock().lock().await;
    // Simulate CRLF .env.local values — the \r is the carriage-return byte.
    std::env::set_var("MQK_STRATEGY_SYMBOL", "TESTFOO\r");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D\r");

    let st = make_live_paper_alpaca();
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "md06-msg".to_string(),
        last_event_at: "2026-03-30T15:00:00Z".to_string(),
    })
    .await;
    // 1 tick dispatched, signal=0, ctx_bars=0 — sub-case B: both vars set, no DB bars.
    st.set_bar_tick_state_for_test(1, 0, 0);

    let v = call_readiness(Arc::clone(&st)).await;

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");

    let blockers = v["blockers"].as_array().expect("blockers must be array");

    let ibc = blockers
        .iter()
        .find(|b| b.as_str().unwrap_or("").contains("INCOMPLETE_BAR_CONTEXT"))
        .expect("MD06: INCOMPLETE_BAR_CONTEXT blocker must appear");
    let ibc_str = ibc.as_str().unwrap();

    assert!(
        ibc_str.contains("TESTFOO"),
        "MD06: blocker must contain trimmed symbol 'TESTFOO'; got: {ibc_str}"
    );
    assert!(
        !ibc_str.contains("TESTFOO\r"),
        "MD06: blocker must NOT contain 'TESTFOO\\r' (raw \\r must be stripped); got: {ibc_str:?}"
    );
    assert!(
        ibc_str.contains("1D"),
        "MD06: blocker must contain trimmed timeframe '1D'; got: {ibc_str}"
    );
    assert!(
        !ibc_str.contains("1D\r"),
        "MD06: blocker must NOT contain '1D\\r' (raw \\r must be stripped); got: {ibc_str:?}"
    );

    let nsg = blockers
        .iter()
        .find(|b| b.as_str().unwrap_or("").contains("NO_SIGNAL_GENERATED"))
        .expect("MD06: NO_SIGNAL_GENERATED blocker must appear");
    let nsg_str = nsg.as_str().unwrap();
    assert!(
        !nsg_str.contains("TESTFOO\r"),
        "MD06: NO_SIGNAL_GENERATED must NOT contain 'TESTFOO\\r'; got: {nsg_str:?}"
    );
}
