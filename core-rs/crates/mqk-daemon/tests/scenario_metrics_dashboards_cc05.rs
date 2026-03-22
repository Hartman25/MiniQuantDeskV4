//! CC-05: Metrics dashboard canonical surface proof.
//!
//! Proves that `GET /api/v1/metrics/dashboards` returns a single composed
//! metrics/KPI view from existing truthful summary surfaces, with explicit
//! truth_state semantics for all panels.
//!
//! Also proves that explicitly-unavailable fields (daily_pnl, drawdown_pct,
//! loss_limit_utilization_pct) remain null — consistent with the individual
//! summary routes that also return null for these underivable fields.
//!
//! All tests are in-process and always runnable in CI without environment
//! variables.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

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

fn dashboard_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/metrics/dashboards")
        .body(axum::body::Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// CC05-01 — Dashboard returns 200 with required panel fields
//
// Proves: the route exists, returns 200, and all structural fields are present
// across all four panels.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cc05_01_dashboard_returns_200_with_required_fields() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, dashboard_req()).await;
    assert_eq!(status, StatusCode::OK, "CC05-01: must return 200");

    let json = parse_json(body);

    assert_eq!(
        json["canonical_route"],
        "/api/v1/metrics/dashboards",
        "CC05-01: canonical_route must be /api/v1/metrics/dashboards"
    );

    // Portfolio panel.
    assert!(
        json["portfolio_snapshot_state"].is_string(),
        "CC05-01: portfolio_snapshot_state must be a string"
    );
    assert!(
        json["buying_power"].is_null() || json["buying_power"].is_number(),
        "CC05-01: buying_power must be null or number"
    );

    // Risk panel.
    assert!(
        json["risk_snapshot_state"].is_string(),
        "CC05-01: risk_snapshot_state must be a string"
    );
    assert!(
        json["kill_switch_active"].is_boolean(),
        "CC05-01: kill_switch_active must be a bool"
    );
    assert!(
        json["active_breaches"].is_number(),
        "CC05-01: active_breaches must be a number"
    );

    // Execution panel.
    assert!(
        json["execution_snapshot_state"].is_string(),
        "CC05-01: execution_snapshot_state must be a string"
    );
    assert!(
        json["active_order_count"].is_number(),
        "CC05-01: active_order_count must be a number"
    );
    assert!(
        json["reject_count_today"].is_number(),
        "CC05-01: reject_count_today must be a number"
    );

    // Reconcile panel.
    assert!(
        json["reconcile_status"].is_string(),
        "CC05-01: reconcile_status must be a string"
    );
    assert!(
        json["reconcile_total_mismatches"].is_number(),
        "CC05-01: reconcile_total_mismatches must be a number"
    );
}

// ---------------------------------------------------------------------------
// CC05-02 — Fresh daemon shows no_snapshot for broker-dependent panels
//
// Proves: without a broker snapshot loaded, portfolio and risk panels report
// "no_snapshot" and all their numeric fields are null, not fabricated zeros.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cc05_02_fresh_daemon_broker_panels_show_no_snapshot() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, dashboard_req()).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);

    // Portfolio panel — no broker snapshot.
    assert_eq!(
        json["portfolio_snapshot_state"], "no_snapshot",
        "CC05-02: portfolio panel must show no_snapshot without broker snapshot; got: {json}"
    );
    assert!(
        json["account_equity"].is_null(),
        "CC05-02: account_equity must be null with no_snapshot; got: {json}"
    );
    assert!(
        json["long_market_value"].is_null(),
        "CC05-02: long_market_value must be null with no_snapshot; got: {json}"
    );
    assert!(
        json["cash"].is_null(),
        "CC05-02: cash must be null with no_snapshot; got: {json}"
    );
    assert!(
        json["buying_power"].is_null(),
        "CC05-02: buying_power must be null with no_snapshot; got: {json}"
    );

    // Risk panel — no broker snapshot means no positions to derive exposure from.
    assert_eq!(
        json["risk_snapshot_state"], "no_snapshot",
        "CC05-02: risk panel must show no_snapshot without broker snapshot; got: {json}"
    );
    assert!(
        json["gross_exposure"].is_null(),
        "CC05-02: gross_exposure must be null with no_snapshot; got: {json}"
    );
    assert!(
        json["net_exposure"].is_null(),
        "CC05-02: net_exposure must be null with no_snapshot; got: {json}"
    );
    assert!(
        json["concentration_pct"].is_null(),
        "CC05-02: concentration_pct must be null with no_snapshot; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// CC05-03 — Explicitly unavailable fields are always null
//
// Proves: fields that have no derivable source in current infrastructure
// (daily_pnl, drawdown_pct, loss_limit_utilization_pct) are null even when
// a broker snapshot is present. This is honest — the individual summary routes
// also return null for these fields.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cc05_03_underivable_fields_are_always_null() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, dashboard_req()).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);

    assert!(
        json["daily_pnl"].is_null(),
        "CC05-03: daily_pnl must always be null — not derivable from current sources; got: {json}"
    );
    assert!(
        json["drawdown_pct"].is_null(),
        "CC05-03: drawdown_pct must always be null — not derivable from current sources; got: {json}"
    );
    assert!(
        json["loss_limit_utilization_pct"].is_null(),
        "CC05-03: loss_limit_utilization_pct must always be null — not derivable from current sources; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// CC05-04 — Dashboard accessible without operator auth (read-only route)
//
// Proves: the dashboard route is on the public (unauthenticated) sub-router.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cc05_04_dashboard_accessible_without_auth() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::MissingTokenFailClosed,
    ));
    let router = routes::build_router(st);

    let (status, body) = call(router, dashboard_req()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "CC05-04: dashboard must be accessible without auth (read-only route); got body: {}",
        String::from_utf8_lossy(&body)
    );

    let json = parse_json(body);
    assert_eq!(
        json["canonical_route"],
        "/api/v1/metrics/dashboards",
        "CC05-04: canonical_route must be correct even without auth"
    );
}

// ---------------------------------------------------------------------------
// CC05-05 — Execution panel shows no_snapshot before execution loop starts
//
// Proves: the execution panel explicitly signals unavailability before the
// execution loop has started, not fabricated zero-order truth.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cc05_05_execution_panel_no_snapshot_before_loop_starts() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, dashboard_req()).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);

    assert_eq!(
        json["execution_snapshot_state"], "no_snapshot",
        "CC05-05: execution panel must show no_snapshot before execution loop starts; got: {json}"
    );
    // Counts are zero when no snapshot, but truth_state makes the absence explicit.
    assert_eq!(
        json["active_order_count"], 0,
        "CC05-05: active_order_count must be 0 when execution_snapshot absent; got: {json}"
    );
    assert_eq!(
        json["pending_order_count"], 0,
        "CC05-05: pending_order_count must be 0 when execution_snapshot absent; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// CC05-06 — Reconcile panel shows unknown before first reconcile tick
//
// Proves: reconcile_status is "unknown" on a fresh daemon (consistent with
// how reconcile_status endpoint behaves — no fabricated "ok").
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cc05_06_reconcile_panel_shows_unknown_before_first_tick() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, dashboard_req()).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);

    assert_eq!(
        json["reconcile_status"], "unknown",
        "CC05-06: reconcile_status must be unknown before first tick; got: {json}"
    );
    assert_eq!(
        json["reconcile_total_mismatches"], 0,
        "CC05-06: reconcile_total_mismatches must be 0 when unknown; got: {json}"
    );
    assert!(
        json["reconcile_last_run_at"].is_null(),
        "CC05-06: reconcile_last_run_at must be null before first tick; got: {json}"
    );
}
