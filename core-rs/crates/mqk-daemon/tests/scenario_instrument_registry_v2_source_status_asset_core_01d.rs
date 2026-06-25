//! ASSET-CORE-01D-REGISTRY-V2-STATUS-01-COMBINED —
//! GET /api/v1/system/instrument-registry-v2-source/status.
//!
//! Proves the read-only operator-visibility status surface for the
//! *separate* v2 registry source configured via
//! `MQK_INSTRUMENT_REGISTRY_V2_PATH` (`AppState::instrument_registry_v2_path`).
//!
//! This is a distinct route/concern from
//! `scenario_instrument_registry_v2_status_asset_core_01c.rs`, which proves
//! the v1->v2 *conversion* diagnostic at `/api/v1/system/instrument-registry-v2/status`.
//! That route always reads `AppState::instrument_registry_path` (v1,
//! `equities.json`-shaped) and is unaffected by `instrument_registry_v2_path`.
//! This route reads only `AppState::instrument_registry_v2_path` and is
//! unaffected by `instrument_registry_path`.
//!
//! No DB pool, no provider or broker calls, no writes. `used_for_trading`,
//! `enabled_for_live_trading`, and `enabled_for_paper_trading` must be `false`
//! on every truth_state, including the happy path.
//!
//! ## CI command
//!
//! ```sh
//! cargo test -p mqk-daemon --test scenario_instrument_registry_v2_source_status_asset_core_01d
//! ```

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

const ROUTE: &str = "/api/v1/system/instrument-registry-v2-source/status";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn example_fixture_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/instruments_v2.backtest_suggestions.example.json")
        .canonicalize()
        .expect("committed example v2 fixture must resolve from CARGO_MANIFEST_DIR")
        .to_string_lossy()
        .to_string()
}

/// Router with only `instrument_registry_v2_path` set (or left `None`).
/// `instrument_registry_path` (v1) is left at its built-in default to prove
/// this route never reads it.
fn make_router_with_v2_path(v2_path: Option<String>) -> axum::Router {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_v2_path = v2_path;
    routes::build_router(Arc::new(st))
}

/// Write `content` to a uniquely-tagged temp file and return its path.
/// Callers must remove the file after the request.
fn write_temp_v2_fixture(content: &str, tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_test_registry_v2_source_status_{}_{}.json",
        tag,
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp v2 registry fixture");
    path
}

/// Parses as valid JSON but fails `validate_registry_v2`: `enabled=true` on a
/// non-equity instrument without the explicit test-only
/// `allow_enabled_non_equity_for_testing` escape hatch.
fn invalid_enabled_future_fixture(symbol: &str) -> String {
    serde_json::json!({
        "schema_version": 1,
        "instruments": [{
            "instrument_id": format!("future:CME:{symbol}"),
            "symbol": symbol,
            "asset_class": "future",
            "venue": "CME",
            "currency": "USD",
            "enabled": true,
            "timeframes": ["1D"],
            "contract": {
                "kind": "future",
                "root": "ES",
                "expiry": "2026-09-18",
                "multiplier": 50,
                "tick_size_micros": 250_000
            }
        }]
    })
    .to_string()
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

async fn get_status(router: axum::Router) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri(ROUTE)
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    (status, parse_json(body))
}

/// Common invariant assertions that must hold on every truth_state.
fn assert_never_implies_trading(body: &serde_json::Value) {
    assert_eq!(
        body["used_for_trading"], false,
        "used_for_trading must always be false: {body}"
    );
    assert_eq!(
        body["enabled_for_live_trading"], false,
        "enabled_for_live_trading must always be false: {body}"
    );
    assert_eq!(
        body["enabled_for_paper_trading"], false,
        "enabled_for_paper_trading must always be false: {body}"
    );
    assert_eq!(
        body["source"], "MQK_INSTRUMENT_REGISTRY_V2_PATH",
        "source must always name the env var: {body}"
    );
    assert_eq!(
        body["purpose"], "backtest_economics_suggestions_only",
        "purpose must always be backtest_economics_suggestions_only: {body}"
    );
}

// ---------------------------------------------------------------------------
// ARC01D-01: not_configured
// ---------------------------------------------------------------------------

#[tokio::test]
async fn arc01d_01_unset_env_reports_not_configured() {
    let router = make_router_with_v2_path(None);
    let (status, body) = get_status(router).await;

    assert_eq!(status, StatusCode::OK, "must return 200: {body}");
    assert_eq!(body["truth_state"], "not_configured", "{body}");
    assert_eq!(body["configured"], false, "{body}");
    assert!(body["path"].is_null(), "path must be null: {body}");
    assert_eq!(body["total_instruments"].as_u64(), Some(0));
    assert!(
        body["asset_class_counts"].as_object().unwrap().is_empty(),
        "{body}"
    );
    assert_eq!(body["non_equity_present"], false);
    assert_eq!(
        body["non_equity_all_disabled"], true,
        "vacuously true with nothing configured: {body}"
    );
    assert!(
        body["validation_errors"]
            .as_array()
            .is_some_and(|v| v.is_empty()),
        "{body}"
    );
    assert_never_implies_trading(&body);
}

// ---------------------------------------------------------------------------
// ARC01D-02: configured_valid via the committed example fixture
// ---------------------------------------------------------------------------

#[tokio::test]
async fn arc01d_02_committed_example_fixture_reports_configured_valid() {
    let router = make_router_with_v2_path(Some(example_fixture_path()));
    let (status, body) = get_status(router).await;

    assert_eq!(status, StatusCode::OK, "must return 200: {body}");
    assert_eq!(body["truth_state"], "configured_valid", "{body}");
    assert_eq!(body["configured"], true, "{body}");
    assert!(
        body["path"].as_str().is_some_and(|s| !s.is_empty()),
        "path must be populated: {body}"
    );
    assert_eq!(body["schema_version"].as_u64(), Some(1), "{body}");
    assert!(
        body["validation_errors"]
            .as_array()
            .is_some_and(|v| v.is_empty()),
        "{body}"
    );
    assert_never_implies_trading(&body);
}

#[tokio::test]
async fn arc01d_03_committed_example_fixture_counts_match_expected_shape() {
    let router = make_router_with_v2_path(Some(example_fixture_path()));
    let (_, body) = get_status(router).await;

    assert_eq!(body["total_instruments"].as_u64(), Some(3), "{body}");
    assert_eq!(
        body["asset_class_counts"]["future"].as_u64(),
        Some(2),
        "{body}"
    );
    assert_eq!(
        body["asset_class_counts"]["crypto"].as_u64(),
        Some(1),
        "{body}"
    );
    assert_eq!(
        body["asset_class_counts"].as_object().unwrap().len(),
        2,
        "{body}"
    );

    let counts = &body["enabled_counts"];
    assert_eq!(counts["enabled"].as_u64(), Some(0), "{body}");
    assert_eq!(counts["paper_trading_enabled"].as_u64(), Some(0), "{body}");
    assert_eq!(counts["live_trading_enabled"].as_u64(), Some(0), "{body}");
}

#[tokio::test]
async fn arc01d_04_committed_example_fixture_proves_non_equity_disabled_and_suggestion_only() {
    let router = make_router_with_v2_path(Some(example_fixture_path()));
    let (_, body) = get_status(router).await;

    assert_eq!(body["non_equity_present"], true, "{body}");
    assert_eq!(body["non_equity_all_disabled"], true, "{body}");
    assert_eq!(body["has_economics_metadata"], true, "{body}");

    let samples: Vec<&str> = body["sample_symbols"]
        .as_array()
        .expect("sample_symbols must be an array")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        samples,
        vec!["ES_TEST", "MES_TEST", "BTCUSD_TEST"],
        "{body}"
    );
}

// ---------------------------------------------------------------------------
// ARC01D-05: registry_unavailable (missing path)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn arc01d_05_missing_configured_path_reports_registry_unavailable() {
    let router = make_router_with_v2_path(Some(
        "/nonexistent/path/that/cannot/exist/instruments_v2.json".to_string(),
    ));
    let (status, body) = get_status(router).await;

    assert_eq!(status, StatusCode::OK, "must still return 200: {body}");
    assert_eq!(body["truth_state"], "registry_unavailable", "{body}");
    assert_eq!(body["configured"], true, "{body}");
    assert!(
        body["path"].as_str().is_some_and(|s| !s.is_empty()),
        "path must still be reported on failure: {body}"
    );
    assert_eq!(body["total_instruments"].as_u64(), Some(0));
    assert!(
        body["validation_errors"]
            .as_array()
            .is_some_and(|v| !v.is_empty()),
        "validation_errors must be populated: {body}"
    );
    assert_never_implies_trading(&body);
}

// ---------------------------------------------------------------------------
// ARC01D-06: validation_failed (configured, loads, but invalid)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn arc01d_06_invalid_registry_content_reports_validation_failed() {
    let path = write_temp_v2_fixture(&invalid_enabled_future_fixture("ES_BAD"), "invalid");
    let router = make_router_with_v2_path(Some(path.to_string_lossy().to_string()));
    let (status, body) = get_status(router).await;
    std::fs::remove_file(&path).ok();

    assert_eq!(status, StatusCode::OK, "must still return 200: {body}");
    assert_eq!(body["truth_state"], "validation_failed", "{body}");
    assert_eq!(body["configured"], true, "{body}");
    assert_eq!(body["total_instruments"].as_u64(), Some(0));
    let errors = body["validation_errors"]
        .as_array()
        .expect("validation_errors must be an array");
    assert_eq!(errors.len(), 1, "{body}");
    assert!(
        errors[0]
            .as_str()
            .is_some_and(|s| s.contains("allow_enabled_non_equity_for_testing")),
        "violation message must identify the enablement gap: {body}"
    );
    assert_never_implies_trading(&body);
}

// ---------------------------------------------------------------------------
// Independence from the v1 registry path
// ---------------------------------------------------------------------------

// ARC01D-07: this route is unaffected by `instrument_registry_path` (v1) —
// pointing v1 at a guaranteed-nonexistent path must not change the v2-source
// response in any way, proving no cross-talk between the two surfaces.
#[tokio::test]
async fn arc01d_07_route_is_independent_of_v1_registry_path() {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = "/nonexistent/v1/path/equities.json".to_string();
    st.instrument_registry_v2_path = Some(example_fixture_path());
    let router = routes::build_router(Arc::new(st));

    let (status, body) = get_status(router).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["truth_state"], "configured_valid",
        "v1 path health must not affect the v2-source route: {body}"
    );
    assert_eq!(body["total_instruments"].as_u64(), Some(3), "{body}");
}

// ---------------------------------------------------------------------------
// Route-level invariants
// ---------------------------------------------------------------------------

// ARC01D-08: idempotent / side-effect free — repeated calls return identical JSON.
#[tokio::test]
async fn arc01d_08_route_is_idempotent_and_side_effect_free() {
    let router = make_router_with_v2_path(Some(example_fixture_path()));
    let (status_a, body_a) = get_status(router.clone()).await;
    let (status_b, body_b) = get_status(router).await;

    assert_eq!(status_a, StatusCode::OK);
    assert_eq!(status_b, StatusCode::OK);
    assert_eq!(body_a, body_b, "repeated calls must return identical JSON");
}

// ARC01D-09: mounted and public (no auth required), distinct from the
// ASSET-CORE-01C route path (no collision).
#[tokio::test]
async fn arc01d_09_route_is_mounted_and_public_no_auth_required() {
    let router = make_router_with_v2_path(None);
    let req = Request::builder()
        .method("GET")
        .uri(ROUTE)
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(router, req).await;
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "route must be mounted on the daemon"
    );
    assert_ne!(
        status,
        StatusCode::UNAUTHORIZED,
        "route must be public (no operator token required)"
    );
}

// ARC01D-10: AppState built via `new_with_operator_auth` carries no DB pool;
// the route still returns 200 configured_valid. Route-level proof that no DB
// pool is required and, by construction, that no DB rows are written.
#[tokio::test]
async fn arc01d_10_route_succeeds_without_db_pool_and_cannot_write_db_rows() {
    let router = make_router_with_v2_path(Some(example_fixture_path()));
    let (status, body) = get_status(router).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "route must succeed with no DB pool configured: {body}"
    );
    assert_eq!(body["truth_state"], "configured_valid");
}
