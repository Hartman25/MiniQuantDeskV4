//! AUTON-NO-TRADE-02 — Autonomous no-trade follow-up proof
//!
//! Proves that after DB arm state is corrected (operator re-arms after a
//! ReconcileDrift disarm), the readiness surface correctly identifies and
//! surfaces the NEXT no-trade blockers in the autonomous paper chain:
//!
//! 1. **Session/market gate** (Classification B): when the NYSE session is
//!    outside the regular window the controller will not attempt a runtime
//!    start, and the readiness surface says so explicitly.
//! 2. **No bar context** (Classification G, pre-tick): before any bar tick
//!    has been dispatched, `bar_context_source = "no_dispatch_yet"` and no
//!    NO_SIGNAL blocker fires.
//! 3. **Stub bar / no-signal** (Classification G, post-tick): when a bar tick
//!    fires but uses the stub path (limit_price=None, no DB price history),
//!    the strategy returns signal_qty=0 and the readiness surface shows
//!    NO_SIGNAL_GENERATED + INCOMPLETE_BAR_CONTEXT blockers.
//!
//! None of these tests change gate logic, weaken fail-closed behaviour, or
//! produce fabricated signals, orders, or fills.
//!
//! | Test    | Claim                                                                            |
//! |---------|----------------------------------------------------------------------------------|
//! | ND2-01  | Outside session window → session blocker + overall_ready=false                   |
//! | ND2-02  | DB=ARMED armed → arm_ready=true even when session gate blocks                    |
//! | ND2-03  | No bar ticks yet → bar_context_source=no_dispatch_yet, no NO_SIGNAL blocker      |
//! | ND2-04  | Stub bar tick (signal=0) → NO_SIGNAL_GENERATED + INCOMPLETE_BAR_CONTEXT in blk  |
//! | ND2-05  | No active run → runtime_start_allowed=true (honest)                              |

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use state::{AlpacaWsContinuityState, BrokerKind};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

async fn db_pool_or_skip(label: &str) -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("{label}: skipping — MQK_DATABASE_URL not set");
            return None;
        }
    };
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("AUTON-NO-TRADE-02: failed to connect to MQK_DATABASE_URL"),
    )
}

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

/// Build a Paper+Alpaca AppState with WS advanced to Live.
fn make_live_paper_alpaca() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ))
}

/// Set session clock to a Saturday (NYSE market closed) so `is_in_session` returns false.
async fn set_session_weekend(st: &Arc<state::AppState>) {
    // Saturday 2026-03-28 15:00:00 UTC — NYSE closed.
    let ts = chrono::DateTime::parse_from_rfc3339("2026-03-28T15:00:00Z")
        .unwrap()
        .timestamp();
    st.set_session_clock_ts_for_test(ts).await;
}

// ---------------------------------------------------------------------------
// ND2-01 — Outside session window → session blocker + overall_ready=false
//
// After the operator has re-armed, the session controller will not attempt a
// runtime start until the NYSE window opens.  The readiness surface must say so.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nd2_01_outside_session_window_blocks_start_and_says_so() {
    let st = make_live_paper_alpaca();

    // WS = Live so that gate passes.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "nd2-01-msg".to_string(),
        last_event_at: "2026-03-28T15:00:00Z".to_string(),
    })
    .await;

    // Session clock → NYSE closed (Saturday).
    set_session_weekend(&st).await;

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    // Canonical path is active.
    assert_eq!(
        v["truth_state"], "active",
        "ND2-01: must be on canonical path"
    );
    assert_eq!(v["canonical_path"], true);

    // Session gate must be closed.
    assert_eq!(
        v["session_in_window"], false,
        "ND2-01: NYSE weekend must report session_in_window=false"
    );
    assert_eq!(
        v["session_window_state"], "outside_window",
        "ND2-01: session_window_state must be outside_window"
    );

    // No false success.
    assert_eq!(
        v["overall_ready"], false,
        "ND2-01: overall_ready must be false when session gate blocks"
    );

    // At least one blocker must mention the session window.
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    let session_blocked = blockers
        .iter()
        .any(|b| b.as_str().unwrap_or("").contains("session window"));
    assert!(
        session_blocked,
        "ND2-01: blockers must mention session window: {blockers:?}"
    );
}

// ---------------------------------------------------------------------------
// ND2-02 — DB=ARMED after re-arm → arm_ready=true even while session gate blocks
//
// Skips gracefully without MQK_DATABASE_URL.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nd2_02_db_armed_produces_arm_ready_true_independent_of_session() {
    let pool = match db_pool_or_skip("nd2-02").await {
        Some(p) => p,
        None => return,
    };

    // Operator action: persist ARMED (simulates arm-execution route outcome).
    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None)
        .await
        .expect("nd2-02: persist ARMED failed");

    let st_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&std::env::var("MQK_DATABASE_URL").unwrap())
        .await
        .expect("nd2-02: AppState pool connect failed");
    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        st_pool,
        state::DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));

    // Advance in-memory arm state (as try_autonomous_arm does on each tick).
    st.try_autonomous_arm()
        .await
        .expect("nd2-02: try_autonomous_arm must succeed when DB=ARMED");

    // WS = Live so that gate passes.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "nd2-02-msg".to_string(),
        last_event_at: "2026-03-28T15:00:00Z".to_string(),
    })
    .await;

    // Session clock → weekend → outside window.
    set_session_weekend(&st).await;

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    // After re-arm the arm gate must show armed.
    assert_eq!(
        v["arm_state"], "armed",
        "ND2-02: DB=ARMED + try_autonomous_arm must surface arm_state=armed"
    );
    assert_eq!(
        v["arm_ready"], true,
        "ND2-02: arm_ready must be true after DB arm is corrected"
    );

    // Session gate still blocks.
    assert_eq!(
        v["session_in_window"], false,
        "ND2-02: session gate must still block on weekend"
    );

    // No false success — session gate keeps overall_ready=false.
    assert_eq!(
        v["overall_ready"], false,
        "ND2-02: overall_ready must remain false until session window opens"
    );

    // Arm blocker must NOT appear (arm gate passed).
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    let arm_blocked = blockers
        .iter()
        .any(|b| b.as_str().unwrap_or("").contains("DISARMED"));
    assert!(
        !arm_blocked,
        "ND2-02: no DISARMED blocker must appear after successful re-arm: {blockers:?}"
    );

    // Session blocker must appear.
    let session_blocked = blockers
        .iter()
        .any(|b| b.as_str().unwrap_or("").contains("session window"));
    assert!(
        session_blocked,
        "ND2-02: session blocker must appear when outside window: {blockers:?}"
    );
}

// ---------------------------------------------------------------------------
// ND2-03 — No bar ticks yet → bar_context_source=no_dispatch_yet, no NO_SIGNAL
//
// Before any bar tick has been dispatched (session just started, or run not
// yet active), the readiness surface must report no_dispatch_yet and must NOT
// fire the NO_SIGNAL_GENERATED blocker.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nd2_03_no_bar_ticks_yet_reports_no_dispatch_yet() {
    let st = make_live_paper_alpaca();

    // WS = Live so we land on the paper+alpaca canonical path.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "nd2-03-msg".to_string(),
        last_event_at: "2026-03-28T15:00:00Z".to_string(),
    })
    .await;

    // No bar ticks set — default state (dispatch_count=0, sentinel=-1).
    // We do NOT call set_bar_tick_state_for_test here to prove the default.

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    // Before any tick: sentinel values.
    assert_eq!(
        v["bar_tick_dispatch_count"], 0,
        "ND2-03: bar_tick_dispatch_count must be 0 before any tick"
    );
    assert!(
        v["last_bar_signal_qty"].is_null(),
        "ND2-03: last_bar_signal_qty must be null before any tick"
    );
    assert_eq!(
        v["bar_context_source"], "no_dispatch_yet",
        "ND2-03: bar_context_source must be no_dispatch_yet before any tick"
    );
    assert!(
        v["bar_context_bars_loaded"].is_null(),
        "ND2-03: bar_context_bars_loaded must be null before any tick"
    );

    // NO_SIGNAL_GENERATED must NOT appear — no ticks have fired yet.
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    let no_signal_fired = blockers
        .iter()
        .any(|b| b.as_str().unwrap_or("").contains("NO_SIGNAL_GENERATED"));
    assert!(
        !no_signal_fired,
        "ND2-03: NO_SIGNAL_GENERATED must not fire before any bar tick: {blockers:?}"
    );
}

// ---------------------------------------------------------------------------
// ND2-04 — Stub bar tick (signal=0) → NO_SIGNAL_GENERATED + INCOMPLETE_BAR_CONTEXT
//
// When a bar tick fires using the stub path (limit_price=None, no DB price
// history), the strategy returns signal_qty=0.  The readiness surface must
// expose both the NO_SIGNAL_GENERATED and the INCOMPLETE_BAR_CONTEXT blockers
// so the operator knows why no orders were attempted.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nd2_04_stub_bar_tick_signal_zero_surfaces_no_signal_and_context_blockers() {
    let st = make_live_paper_alpaca();

    // WS = Live so we land on the paper+alpaca canonical path.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "nd2-04-msg".to_string(),
        last_event_at: "2026-03-30T15:00:00Z".to_string(),
    })
    .await;

    // Simulate: 1 bar tick dispatched, strategy returned signal_qty=0, stub context used.
    // ctx_bars=0 means "stub_no_price" path (no price data; limit_price was None).
    st.set_bar_tick_state_for_test(
        1, // dispatch_count
        0, // signal_qty — strategy returned no targets
        0, // ctx_bars — stub path (no DB price history)
    );

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    // Counters must reflect the simulated tick.
    assert_eq!(
        v["bar_tick_dispatch_count"], 1,
        "ND2-04: bar_tick_dispatch_count must be 1"
    );
    assert_eq!(
        v["last_bar_signal_qty"], 0,
        "ND2-04: last_bar_signal_qty must be 0"
    );
    assert_eq!(
        v["bar_context_source"], "stub_no_price",
        "ND2-04: bar_context_source must be stub_no_price for ctx_bars=0"
    );
    assert!(
        v["bar_context_bars_loaded"].is_null(),
        "ND2-04: bar_context_bars_loaded must be null for stub path"
    );

    // Both no-trade blockers must appear.
    let blockers = v["blockers"].as_array().expect("blockers must be array");

    let no_signal = blockers
        .iter()
        .any(|b| b.as_str().unwrap_or("").contains("NO_SIGNAL_GENERATED"));
    assert!(
        no_signal,
        "ND2-04: NO_SIGNAL_GENERATED blocker must appear when ticks>0 and signal=0: {blockers:?}"
    );

    let incomplete_ctx = blockers
        .iter()
        .any(|b| b.as_str().unwrap_or("").contains("INCOMPLETE_BAR_CONTEXT"));
    assert!(
        incomplete_ctx,
        "ND2-04: INCOMPLETE_BAR_CONTEXT blocker must appear when stub context used: {blockers:?}"
    );

    // overall_ready must be false — no-trade state must not look like success.
    assert_eq!(
        v["overall_ready"], false,
        "ND2-04: overall_ready must be false when no signal produced"
    );
}

// ---------------------------------------------------------------------------
// ND2-05 — No active run → runtime_start_allowed=true (honest)
//
// When no execution run is locally owned, the readiness surface must report
// runtime_start_allowed=true so the operator knows the run-start gate is clear.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nd2_05_no_active_run_reports_runtime_start_allowed_true() {
    let st = make_live_paper_alpaca();

    // WS = Live so we land on the canonical path.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "nd2-05-msg".to_string(),
        last_event_at: "2026-03-28T15:00:00Z".to_string(),
    })
    .await;

    // No run injected — default state.

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(
        v["runtime_start_allowed"], true,
        "ND2-05: runtime_start_allowed must be true when no active run exists"
    );

    // No blocker about an existing run.
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    let run_blocked = blockers.iter().any(|b| {
        b.as_str()
            .unwrap_or("")
            .contains("locally-owned execution run")
    });
    assert!(
        !run_blocked,
        "ND2-05: no run-conflict blocker must appear when no active run: {blockers:?}"
    );
}
