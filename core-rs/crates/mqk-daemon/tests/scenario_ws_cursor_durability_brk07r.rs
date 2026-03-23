//! BRK-07R: Alpaca WS cursor/continuity durability across daemon restart.
//!
//! Proves that `seed_ws_continuity_from_db` correctly derives boot-time
//! continuity from the persisted broker cursor:
//!
//! | Persisted cursor state  | Boot continuity    | Reason                              |
//! |-------------------------|--------------------|-------------------------------------|
//! | (no cursor)             | ColdStartUnproven  | First boot, no prior run            |
//! | Live                    | ColdStartUnproven  | WS must re-establish after restart  |
//! | GapDetected             | GapDetected        | Fail-closed: gate blocks start      |
//! | Parse error             | GapDetected        | Fail-closed                         |
//!
//! Pure in-memory tests (D01-D02): no DB required; run unconditionally.
//! DB-backed tests (D03-D06): skip gracefully without `MQK_DATABASE_URL`.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_broker_alpaca::types::AlpacaFetchCursor;
use mqk_daemon::routes;
use mqk_daemon::state::{AlpacaWsContinuityState, AppState, BrokerKind, DeploymentMode};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn db_pool_or_skip() -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(v) => v,
        Err(_) => return None,
    };
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("BRK-07R DB test: failed to connect to MQK_DATABASE_URL"),
    )
}

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn arm_via_http(st: &Arc<AppState>) {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(routes::build_router(Arc::clone(st)), req).await;
    assert_eq!(status, StatusCode::OK, "arm_via_http: arm must succeed");
}

// ---------------------------------------------------------------------------
// D01 — No DB: seed_ws_continuity_from_db is a no-op → stays ColdStartUnproven
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk07r_d01_no_db_seed_is_noop_stays_cold_start() {
    let state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Alpaca);
    assert!(
        matches!(
            state.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::ColdStartUnproven
        ),
        "D01: pre-seed must be ColdStartUnproven"
    );

    state.seed_ws_continuity_from_db().await;

    assert!(
        matches!(
            state.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::ColdStartUnproven
        ),
        "D01: no-op with no DB must leave ColdStartUnproven"
    );
}

// ---------------------------------------------------------------------------
// D02 — Non-Alpaca broker: seed_ws_continuity_from_db is a no-op → NotApplicable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk07r_d02_non_alpaca_broker_seed_is_noop() {
    let state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Paper);
    assert!(
        matches!(
            state.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::NotApplicable
        ),
        "D02: pre-seed must be NotApplicable for Paper broker"
    );

    state.seed_ws_continuity_from_db().await;

    assert!(
        matches!(
            state.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::NotApplicable
        ),
        "D02: non-Alpaca broker seed must not change NotApplicable"
    );
}

// ---------------------------------------------------------------------------
// D03 — DB with GapDetected cursor → boot reflects GapDetected (fail-closed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk07r_d03_gap_detected_cursor_preserved_at_boot() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("D03: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    // Unique adapter_id prevents parallel-test interference with D04/D06.
    let adapter_id = "brk07r-d03-test";

    let gap_cursor = AlpacaFetchCursor::gap_detected(
        None,
        Some("alpaca:order-abc:filled:2026-01-01T00:00:00Z".to_string()),
        Some("2026-01-01T00:00:00Z".to_string()),
        "brk07r-d03 test gap",
    );
    let cursor_json = serde_json::to_string(&gap_cursor).expect("D03: serialize");
    mqk_db::advance_broker_cursor(&pool, adapter_id, &cursor_json, Utc::now())
        .await
        .expect("D03: advance_broker_cursor failed");

    // Simulate restart: fresh AppState with same DB and same adapter_id.
    let mut state_inner =
        AppState::new_for_test_with_db_mode_and_broker(pool, DeploymentMode::Paper, BrokerKind::Alpaca);
    state_inner.set_adapter_id_for_test(adapter_id);
    let state = Arc::new(state_inner);

    assert!(
        matches!(
            state.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::ColdStartUnproven
        ),
        "D03: before seed, fresh state must be ColdStartUnproven"
    );

    state.seed_ws_continuity_from_db().await;

    let cont = state.alpaca_ws_continuity().await;
    assert!(
        matches!(cont, AlpacaWsContinuityState::GapDetected { .. }),
        "D03: GapDetected cursor must produce GapDetected continuity at boot; got: {cont:?}"
    );
    if let AlpacaWsContinuityState::GapDetected { last_message_id, .. } = cont {
        assert_eq!(
            last_message_id.as_deref(),
            Some("alpaca:order-abc:filled:2026-01-01T00:00:00Z"),
            "D03: last_message_id must be preserved from cursor"
        );
    }
}

// ---------------------------------------------------------------------------
// D04 — DB with Live cursor → boot demotes to ColdStartUnproven
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk07r_d04_live_cursor_demoted_to_cold_start_at_boot() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("D04: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    let adapter_id = "brk07r-d04-test";

    let live_cursor = AlpacaFetchCursor::live(
        None,
        "alpaca:order-xyz:filled:2026-01-02T10:00:00Z",
        "2026-01-02T10:00:00Z",
    );
    let cursor_json = serde_json::to_string(&live_cursor).expect("D04: serialize");
    mqk_db::advance_broker_cursor(&pool, adapter_id, &cursor_json, Utc::now())
        .await
        .expect("D04: advance_broker_cursor failed");

    let mut state_inner =
        AppState::new_for_test_with_db_mode_and_broker(pool, DeploymentMode::Paper, BrokerKind::Alpaca);
    state_inner.set_adapter_id_for_test(adapter_id);
    let state = Arc::new(state_inner);

    state.seed_ws_continuity_from_db().await;

    let cont = state.alpaca_ws_continuity().await;
    assert!(
        matches!(cont, AlpacaWsContinuityState::ColdStartUnproven),
        "D04: Live cursor must be demoted to ColdStartUnproven at boot \
         (WS not yet reconnected); got: {cont:?}"
    );
}

// ---------------------------------------------------------------------------
// D05 — No cursor in DB → stays ColdStartUnproven (via no-DB proxy)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk07r_d05_no_cursor_stays_cold_start() {
    // The absent-cursor DB path is functionally identical to the no-DB path:
    // both return None to `seed_ws_continuity_from_db`, leaving ColdStartUnproven.
    let state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Alpaca);
    state.seed_ws_continuity_from_db().await;
    let cont = state.alpaca_ws_continuity().await;
    assert!(
        matches!(cont, AlpacaWsContinuityState::ColdStartUnproven),
        "D05: absent cursor must leave ColdStartUnproven; got: {cont:?}"
    );
}

// ---------------------------------------------------------------------------
// D06 — Gate ordering: GapDetected from DB immediately blocks start via HTTP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk07r_d06_gap_detected_from_db_blocks_start_gate() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("D06: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    let adapter_id = "brk07r-d06-test";

    let gap_cursor = AlpacaFetchCursor::gap_detected(
        None,
        Some("alpaca:order-d06:canceled:2026-01-03T00:00:00Z".to_string()),
        Some("2026-01-03T00:00:00Z".to_string()),
        "brk07r-d06 gate test",
    );
    let cursor_json = serde_json::to_string(&gap_cursor).expect("D06: serialize");
    mqk_db::advance_broker_cursor(&pool, adapter_id, &cursor_json, Utc::now())
        .await
        .expect("D06: persist gap cursor");

    let mut state_inner =
        AppState::new_for_test_with_db_mode_and_broker(pool, DeploymentMode::Paper, BrokerKind::Alpaca);
    state_inner.set_adapter_id_for_test(adapter_id);
    let st = Arc::new(state_inner);

    // Arm integrity gate so it is not the blocker.
    arm_via_http(&st).await;

    // Seed from DB → continuity becomes GapDetected.
    st.seed_ws_continuity_from_db().await;

    assert!(
        matches!(
            st.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::GapDetected { .. }
        ),
        "D06: after seed, continuity must be GapDetected"
    );

    // Start request must fail at the WS continuity gate.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, json) = call(routes::build_router(Arc::clone(&st)), start_req).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "D06: start must be blocked (403) when GapDetected from DB; got: {status}"
    );
    assert_eq!(
        json["gate"], "alpaca_ws_continuity",
        "D06: gate must be alpaca_ws_continuity; got: {json}"
    );
}
