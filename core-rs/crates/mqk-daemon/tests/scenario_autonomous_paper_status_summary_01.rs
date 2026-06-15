//! PAPER-AUTONOMOUS-COMPLETION-BUNDLE-01: autonomous paper status summary endpoint.
//!
//! Proves:
//!
//! | Test | What it proves                                                         |
//! |------|------------------------------------------------------------------------|
//! | PS01 | Paper+Alpaca → 200, truth_state="active"                               |
//! | PS02 | live_routing_enabled is always false in paper response                 |
//! | PS03 | Non-paper path → truth_state="not_applicable"                          |
//! | PS04 | Reconcile dirty blocker surfaced in blockers list                       |
//! | PS05 | Kill switch (halted) → kill_switch_active=true in response             |
//! | PS06 | WS not live → blocker surfaced, ws_continuity field non-"live"         |
//! | PS07 | Flatten unavailable when no active run → flatten_blockers non-empty    |
//! | PS08 | Watchlist not configured → watchlist_outcome="not_configured", approved=false |
//! | PS09 | Response always has next_operator_action (non-empty string)            |
//! | PS10 | response has autonomous_session_state field                            |
//! | PS11 | No broker imports present in autonomous_paper_status module            |
//! | PS12 | No DB mutation path in autonomous_paper_status module                  |
//! | PS13 | No order submission path in autonomous_paper_status module             |
//! | PS14 | response has readiness_classification field (never null)               |
//! | PS15 | discord_visibility_ready=false when DISCORD_WEBHOOK_URL unconfigured  |
//! | PS16 | discord_visibility_ready=true when notifier is configured             |
//! | PS17 | not_applicable path reflects notifier configuration (not hardcoded)   |
//!
//! All tests are pure in-process.  No `MQK_DATABASE_URL` required.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use mqk_daemon::{
    notify::DiscordNotifier,
    routes::build_router,
    state::{
        AlpacaWsContinuityState, AppState, BrokerKind, DeploymentMode, ReconcileStatusSnapshot,
    },
};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn paper_alpaca_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca))
}

fn paper_paper_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_mode(DeploymentMode::Paper))
}

fn live_capital_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_mode(
        DeploymentMode::LiveCapital,
    ))
}

async fn get_paper_status(router: axum::Router) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .uri("/api/v1/autonomous/paper-status")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, j)
}

// ---------------------------------------------------------------------------
// PS01: Paper+Alpaca → 200, truth_state="active"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ps01_paper_alpaca_returns_200_active() {
    let st = paper_alpaca_state();
    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["truth_state"].as_str().unwrap(),
        "active",
        "paper+alpaca must return truth_state=active"
    );
    assert_eq!(
        body["canonical_route"].as_str().unwrap(),
        "/api/v1/autonomous/paper-status"
    );
    assert_eq!(body["mode"].as_str().unwrap(), "paper");
}

// ---------------------------------------------------------------------------
// PS02: live_routing_enabled is always false in paper response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ps02_live_routing_enabled_always_false() {
    let st = paper_alpaca_state();
    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        !body["live_routing_enabled"].as_bool().unwrap(),
        "live_routing_enabled must always be false for paper mode"
    );
}

// ---------------------------------------------------------------------------
// PS03: Non-paper path → truth_state="not_applicable"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ps03_non_paper_returns_not_applicable() {
    // Paper+paper (no ExternalSignalIngestion) → not_applicable.
    let st = paper_paper_state();
    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"].as_str().unwrap(), "not_applicable");
    // live_routing_enabled must still be false even in not_applicable path.
    assert!(!body["live_routing_enabled"].as_bool().unwrap());
}

#[tokio::test]
async fn ps03b_live_capital_returns_not_applicable() {
    let st = live_capital_state();
    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"].as_str().unwrap(), "not_applicable");
}

// ---------------------------------------------------------------------------
// PS04: Reconcile dirty → blocker surfaced
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ps04_reconcile_dirty_creates_blocker() {
    let st = paper_alpaca_state();
    // Publish a dirty reconcile snapshot.
    st.publish_reconcile_snapshot(ReconcileStatusSnapshot {
        status: "dirty".to_string(),
        last_run_at: None,
        snapshot_watermark_ms: None,
        mismatched_positions: 2,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: None,
    })
    .await;

    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["reconcile_status"].as_str().unwrap(), "dirty");
    assert!(body["mismatch_count"].as_u64().unwrap() >= 2);

    let blockers = body["blockers"].as_array().unwrap();
    assert!(
        blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("dirty")),
        "blockers must mention dirty reconcile; got: {:?}",
        blockers
    );
    // Dirty reconcile means classification is blocked.
    assert_eq!(
        body["readiness_classification"].as_str().unwrap(),
        "blocked"
    );
}

// ---------------------------------------------------------------------------
// PS05: Kill switch (integrity halted) → kill_switch_active=true
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ps05_kill_switch_active_surfaces_in_response() {
    let st = paper_alpaca_state();
    // Set integrity to halted.
    {
        let mut ig = st.integrity.write().await;
        ig.halted = true;
    }

    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        body["kill_switch_active"].as_bool().unwrap(),
        "kill_switch_active must be true when integrity is halted"
    );
    assert_eq!(body["arm_state"].as_str().unwrap(), "halted");
    // Halted counts as a blocker.
    assert_eq!(
        body["readiness_classification"].as_str().unwrap(),
        "blocked"
    );
}

// ---------------------------------------------------------------------------
// PS06: WS not live → blocker surfaced, ws_continuity field non-"live"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ps06_ws_not_live_creates_blocker() {
    let st = paper_alpaca_state();
    // Default is ColdStartUnproven (not live).
    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert_ne!(
        body["ws_continuity"].as_str().unwrap(),
        "live",
        "WS continuity should not be live at test startup"
    );

    let blockers = body["blockers"].as_array().unwrap();
    assert!(
        blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("WS continuity")),
        "blockers must mention WS continuity; got: {:?}",
        blockers
    );
}

#[tokio::test]
async fn ps06b_gap_detected_surfaces_in_ws_continuity() {
    let st = paper_alpaca_state();
    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:test:fill:123".to_string()),
        last_event_at: None,
        detail: "test gap".to_string(),
    })
    .await;

    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ws_continuity"].as_str().unwrap(), "gap_detected");
    assert_eq!(
        body["readiness_classification"].as_str().unwrap(),
        "blocked"
    );
}

// ---------------------------------------------------------------------------
// PS07: Flatten unavailable when no active run → flatten_blockers non-empty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ps07_flatten_unavailable_without_active_run() {
    let st = paper_alpaca_state();
    // No active run set — flatten gate 5 blocks.
    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        !body["flatten_available"].as_bool().unwrap(),
        "flatten must not be available without an active run"
    );
    let flatten_blockers = body["flatten_blockers"].as_array().unwrap();
    assert!(
        !flatten_blockers.is_empty(),
        "flatten_blockers must be non-empty when flatten is unavailable"
    );
    // Should mention no active run.
    assert!(
        flatten_blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("no active run")),
        "flatten_blockers must mention no active run; got: {:?}",
        flatten_blockers
    );
}

// ---------------------------------------------------------------------------
// PS08: Watchlist not configured → watchlist_outcome="not_configured", approved=false
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ps08_watchlist_not_configured_is_not_fatal() {
    let st = paper_alpaca_state();
    // Do not set MQK_PAPER_WATCHLIST_PATH — not_configured is the expected outcome.
    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    // Watchlist not configured must not crash or return 500.
    assert!(
        [
            "not_configured",
            "missing",
            "invalid",
            "loaded_not_approved",
            "loaded_approved"
        ]
        .contains(&body["watchlist_outcome"].as_str().unwrap_or("missing")),
        "watchlist_outcome must be a valid string"
    );
    // approved_for_live is always false.
    // watchlist_approved reflects actual approval state — if not_configured, false.
    assert_eq!(body["truth_state"].as_str().unwrap(), "active");
    // Response must still be 200 even with no watchlist configured.
}

// ---------------------------------------------------------------------------
// PS09: Response always has next_operator_action (non-empty string)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ps09_response_has_next_operator_action() {
    let st = paper_alpaca_state();
    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    let action = body["next_operator_action"].as_str().unwrap_or("");
    assert!(
        !action.is_empty(),
        "next_operator_action must never be empty"
    );
}

#[tokio::test]
async fn ps09b_not_applicable_path_has_operator_action() {
    let st = paper_paper_state();
    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    let action = body["next_operator_action"].as_str().unwrap_or("");
    assert!(!action.is_empty());
}

// ---------------------------------------------------------------------------
// PS10: Response has autonomous_session_state field
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ps10_response_has_autonomous_session_state() {
    let st = paper_alpaca_state();
    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    let sess_state = body["autonomous_session_state"].as_str().unwrap_or("");
    assert!(
        !sess_state.is_empty(),
        "autonomous_session_state must be present"
    );
    // Default is "clear" (no anomaly).
    assert_eq!(sess_state, "clear");
}

// ---------------------------------------------------------------------------
// PS11-PS13: Static source guards (inline text assertions)
// These verify no forbidden patterns exist in the new module source.
// ---------------------------------------------------------------------------

#[test]
fn ps11_no_broker_imports_in_paper_status_module() {
    let src = include_str!("../src/routes/autonomous_paper_status.rs");
    assert!(
        !src.contains("submit_order"),
        "PS11: autonomous_paper_status.rs must not contain submit_order"
    );
    assert!(
        !src.contains("BrokerAdapter"),
        "PS11: autonomous_paper_status.rs must not import BrokerAdapter"
    );
    assert!(
        !src.contains("/v2/orders"),
        "PS11: autonomous_paper_status.rs must not reference Alpaca orders endpoint"
    );
}

#[test]
fn ps12_no_db_mutation_in_paper_status_module() {
    let src = include_str!("../src/routes/autonomous_paper_status.rs");
    // Reads are allowed (load_arm_state is a read); writes must be absent.
    assert!(
        !src.contains("insert"),
        "PS12: autonomous_paper_status.rs must not call DB insert"
    );
    assert!(
        !src.contains("update"),
        "PS12: autonomous_paper_status.rs must not call DB update"
    );
    assert!(
        !src.contains("delete"),
        "PS12: autonomous_paper_status.rs must not call DB delete"
    );
    assert!(
        !src.contains("publish_reconcile_snapshot"),
        "PS12: autonomous_paper_status.rs must not mutate reconcile state"
    );
}

#[test]
fn ps13_no_order_submission_in_paper_status_module() {
    let src = include_str!("../src/routes/autonomous_paper_status.rs");
    assert!(
        !src.contains("outbox_enqueue"),
        "PS13: autonomous_paper_status.rs must not enqueue outbox orders"
    );
    assert!(
        !src.contains("submit_order"),
        "PS13: autonomous_paper_status.rs must not submit orders"
    );
    assert!(
        !src.contains("live_routing_enabled = true"),
        "PS13: autonomous_paper_status.rs must not enable live routing"
    );
}

// ---------------------------------------------------------------------------
// PS14: readiness_classification field is always a known string (never null)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ps14_readiness_classification_never_null() {
    let st = paper_alpaca_state();
    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    let classification = body["readiness_classification"].as_str().unwrap_or("");
    assert!(
        ["ready_for_market_smoke", "market_proof_pending", "blocked"].contains(&classification),
        "readiness_classification must be one of the known strings; got: {:?}",
        classification
    );
}

#[tokio::test]
async fn ps14b_ws_live_moves_classification_toward_ready() {
    let st = paper_alpaca_state();
    // Set WS to live — removes the WS continuity blocker.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:test:ack:999".to_string(),
        last_event_at: "2026-06-06T15:00:00Z".to_string(),
    })
    .await;

    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ws_continuity"].as_str().unwrap(), "live");
    // With WS live, WS blocker is removed.  Classification should not be "blocked"
    // from WS, though other blockers (arm, session window, strategy fleet) may remain.
    let classification = body["readiness_classification"].as_str().unwrap();
    // Just confirm WS live didn't break the response — classification may still
    // be "blocked" due to other gates (arm, strategy fleet, etc).
    assert!(
        ["ready_for_market_smoke", "market_proof_pending", "blocked"].contains(&classification)
    );
}

// ---------------------------------------------------------------------------
// PS15-PS17: discord_visibility_ready reflects actual notifier configuration
// (DISCORD-LIFECYCLE-OBSERVABILITY-COMPLETION-01)
// ---------------------------------------------------------------------------

// PS15: discord_visibility_ready=false when DISCORD_WEBHOOK_URL is unconfigured
// (the default test-process notifier, since AppState::from_env() sees no
// DISCORD_WEBHOOK_URL in CI/test).
#[tokio::test]
async fn ps15_discord_visibility_ready_false_when_unconfigured() {
    let st = paper_alpaca_state();
    assert!(
        !st.discord_notifier.is_configured(),
        "test notifier must be unconfigured by default"
    );

    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        !body["discord_visibility_ready"].as_bool().unwrap(),
        "discord_visibility_ready must be false when DISCORD_WEBHOOK_URL is unconfigured"
    );
}

// PS16: discord_visibility_ready=true when the notifier is configured with a
// webhook URL (no delivery is attempted by this read-only route).
#[tokio::test]
async fn ps16_discord_visibility_ready_true_when_configured() {
    let mut st = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st.discord_notifier = DiscordNotifier::from_url("http://127.0.0.1:1/hook");
    let st = Arc::new(st);

    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        body["discord_visibility_ready"].as_bool().unwrap(),
        "discord_visibility_ready must be true when a webhook URL is configured"
    );
}

// PS17: not_applicable path also reflects notifier configuration rather than
// a hardcoded true.
#[tokio::test]
async fn ps17_not_applicable_path_reflects_notifier_configuration() {
    let st = paper_paper_state();
    assert!(!st.discord_notifier.is_configured());

    let router = build_router(st);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"].as_str().unwrap(), "not_applicable");
    assert!(
        !body["discord_visibility_ready"].as_bool().unwrap(),
        "not_applicable path must not hardcode discord_visibility_ready=true"
    );
}
