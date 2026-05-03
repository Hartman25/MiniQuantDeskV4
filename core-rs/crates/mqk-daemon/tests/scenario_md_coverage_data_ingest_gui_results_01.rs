//! DATA-INGEST-GUI-RESULTS-01: md_bars coverage route proof tests.
//!
//! Proves that:
//! - GET /api/v1/market-data/coverage returns 200 with db_unavailable when no DB
//! - GET /api/v1/market-data/coverage?timeframe=1D echoes timeframe in response
//! - GET /api/v1/market-data/coverage (no param) echoes null timeframe
//! - Response shape serializes correctly (canonical_route, truth_state, rows, error fields present)
//! - No live/paper execution state is touched
//!
//! # Proof matrix
//!
//! | Test   | What it proves                                                              |
//! |--------|-----------------------------------------------------------------------------|
//! | CR-01  | No DB pool → truth_state="db_unavailable", 200, rows=[]                   |
//! | CR-02  | ?timeframe=1D → timeframe echoed in response                               |
//! | CR-03  | No timeframe param → timeframe=null in response                            |
//! | CR-04  | ?timeframe= (empty string) → treated as all, timeframe=null in response   |
//! | CR-05  | execution_snapshot unaffected after coverage call (safety check)           |
//!
//! All tests are fully in-process (no DB, no disk I/O, no network).
//! No TwelveData API credits consumed. No broker adapter called.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_router() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

fn make_router_with_state() -> (Arc<state::AppState>, axum::Router) {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));
    (st, router)
}

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, body)
}

fn parse_json(body: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&body).expect("response body must be valid JSON")
}

fn json_str<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_else(|| panic!("key '{}' must be a string in: {}", key, v))
}

fn get_coverage(uri: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// CR-01: No DB pool → db_unavailable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cr_01_no_db_returns_db_unavailable() {
    let router = make_router();
    let (status, body) = call(router, get_coverage("/api/v1/market-data/coverage")).await;

    assert_eq!(status, StatusCode::OK, "coverage route must return 200");

    let v = parse_json(body);
    assert_eq!(
        json_str(&v, "truth_state"),
        "db_unavailable",
        "no DB pool must yield db_unavailable"
    );
    assert_eq!(
        v["rows"].as_array().map(|a| a.len()).unwrap_or(1),
        0,
        "rows must be empty when db unavailable"
    );
    assert_eq!(
        json_str(&v, "canonical_route"),
        "/api/v1/market-data/coverage"
    );
    assert!(
        v["error"].as_str().is_some(),
        "error field must be present when db_unavailable"
    );
}

// ---------------------------------------------------------------------------
// CR-02: ?timeframe=1D → timeframe echoed in response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cr_02_timeframe_param_echoed_in_response() {
    let router = make_router();
    let (status, body) = call(
        router,
        get_coverage("/api/v1/market-data/coverage?timeframe=1D"),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let v = parse_json(body);
    // truth_state will be db_unavailable (no pool) but timeframe must be echoed
    assert_eq!(
        json_str(&v, "timeframe"),
        "1D",
        "timeframe=1D must be echoed in response"
    );
}

// ---------------------------------------------------------------------------
// CR-03: No timeframe param → timeframe=null
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cr_03_no_timeframe_param_gives_null_timeframe() {
    let router = make_router();
    let (status, body) = call(router, get_coverage("/api/v1/market-data/coverage")).await;

    assert_eq!(status, StatusCode::OK);

    let v = parse_json(body);
    assert!(
        v["timeframe"].is_null(),
        "timeframe must be null when not supplied; got: {}",
        v["timeframe"]
    );
}

// ---------------------------------------------------------------------------
// CR-04: ?timeframe= (empty string) → treated as all timeframes, null in response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cr_04_empty_timeframe_param_treated_as_all() {
    let router = make_router();
    let (status, body) = call(
        router,
        get_coverage("/api/v1/market-data/coverage?timeframe="),
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let v = parse_json(body);
    // truth_state = db_unavailable is expected (no pool); but the empty-string
    // timeframe must be normalized to null (treated as "all timeframes").
    // The route handler normalizes "" to None via .filter(|s| !s.is_empty()).
    // An empty string URL-encodes to "" → handler sees Some("") → filters to None → echoes null.
    assert!(
        v["timeframe"].is_null(),
        "empty timeframe string must echo null (all timeframes); got: {}",
        v["timeframe"]
    );
}

// ---------------------------------------------------------------------------
// CR-05: execution_snapshot unaffected (safety: no live state touched)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cr_05_execution_snapshot_unaffected() {
    let (st, router) = make_router_with_state();

    // execution_snapshot starts None
    {
        let snap = st.execution_snapshot.read().await.clone();
        assert!(snap.is_none(), "execution_snapshot must be None initially");
    }

    let (status, _body) = call(router, get_coverage("/api/v1/market-data/coverage")).await;
    assert_eq!(status, StatusCode::OK);

    // Must still be None after a coverage call — no live state mutation
    {
        let snap = st.execution_snapshot.read().await.clone();
        assert!(
            snap.is_none(),
            "coverage route must not touch execution_snapshot"
        );
    }
}
