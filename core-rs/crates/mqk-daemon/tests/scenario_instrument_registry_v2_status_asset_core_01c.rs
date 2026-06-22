//! ASSET-CORE-01C — GET /api/v1/system/instrument-registry-v2/status.
//!
//! Proves the read-only v1->v2 instrument-registry status surface: it must
//! load the configured v1 registry, convert it to `InstrumentRegistryV2` in
//! memory, validate it, and report truthful counts/truth_state for the
//! active/missing/malformed/validation-failure cases — without ever needing
//! a DB pool, provider, or broker, and without making `InstrumentRegistryV2`
//! a trading input (`production_cutover_enabled`/`trading_uses_v2` always
//! `false`).
//!
//! ## CI command
//!
//! ```sh
//! cargo test -p mqk-daemon --test scenario_instrument_registry_v2_status_asset_core_01c
//! ```

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use mqk_md::instrument_registry::TrackedInstrument;
use tower::ServiceExt;

const ROUTE: &str = "/api/v1/system/instrument-registry-v2/status";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Router pointing `instrument_registry_path` at the real, committed
/// `config/instruments/equities.json` (canonicalized from CARGO_MANIFEST_DIR
/// so it resolves regardless of `cargo test`'s working directory).
fn make_router_with_real_registry() -> axum::Router {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let registry_path = std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/equities.json")
        .canonicalize()
        .expect("registry path must resolve from CARGO_MANIFEST_DIR")
        .to_string_lossy()
        .to_string();

    let mut st = state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = registry_path;
    routes::build_router(Arc::new(st))
}

/// Router pointing `instrument_registry_path` at a guaranteed-nonexistent path.
fn make_router_with_missing_registry() -> axum::Router {
    let mut st = state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = "/nonexistent/path/that/cannot/exist/equities.json".to_string();
    routes::build_router(Arc::new(st))
}

/// Router pointing `instrument_registry_path` at a temp file containing
/// `content`. Returns the router and the temp path so the caller can clean
/// up after the request.
fn make_router_with_registry_content(content: &str, tag: &str) -> (axum::Router, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "mqk_test_registry_v2_status_{}_{}.json",
        tag,
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp registry fixture");

    let mut st = state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = path.to_string_lossy().to_string();
    (routes::build_router(Arc::new(st)), path)
}

fn etf_missing_sector_fixture() -> String {
    let bad = TrackedInstrument {
        instrument_id: "equity:US:SPY".to_string(),
        symbol: "SPY".to_string(),
        asset_class: "equity".to_string(),
        provider: "twelvedata".to_string(),
        provider_symbol: "SPY".to_string(),
        venue: "NYSEARCA".to_string(),
        currency: "USD".to_string(),
        enabled: true,
        timeframes: vec!["1D".to_string()],
        notes: "fixture".to_string(),
        instrument_kind: Some("etf".to_string()),
        sector: None,
        category: None,
    };
    serde_json::to_string(&vec![bad]).expect("serialize fixture")
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

// ---------------------------------------------------------------------------
// Active path — real production registry
// ---------------------------------------------------------------------------

// ARC01C-01: real registry -> 200, truth_state=active, registry_path set,
// no DB pool required (AppState built without one).
#[tokio::test]
async fn arc01c_01_active_status_for_real_production_registry() {
    let router = make_router_with_real_registry();
    let (status, body) = get_status(router).await;

    assert_eq!(status, StatusCode::OK, "must return 200: {body}");
    assert_eq!(
        body["truth_state"], "active",
        "truth_state must be active: {body}"
    );
    assert!(
        body["registry_path"].as_str().is_some_and(|s| !s.is_empty()),
        "registry_path must be a non-empty string: {body}"
    );
    assert_eq!(
        body["schema_version"].as_u64(),
        Some(1),
        "schema_version must be 1: {body}"
    );
    assert!(
        body["validation_errors"]
            .as_array()
            .is_some_and(|v| v.is_empty()),
        "validation_errors must be empty on active: {body}"
    );
    assert_eq!(
        body["validation_passed"], true,
        "validation_passed must be true on active: {body}"
    );
}

// ARC01C-02: v1_count == v2_count == 88 for the real production registry.
#[tokio::test]
async fn arc01c_02_active_status_v1_count_equals_v2_count_88() {
    let router = make_router_with_real_registry();
    let (_, body) = get_status(router).await;

    assert_eq!(body["v1_count"].as_u64(), Some(88), "v1_count: {body}");
    assert_eq!(body["v2_count"].as_u64(), Some(88), "v2_count: {body}");
}

// ARC01C-03: etf_count == 14 for the real production registry.
#[tokio::test]
async fn arc01c_03_active_status_etf_count_14() {
    let router = make_router_with_real_registry();
    let (_, body) = get_status(router).await;

    assert_eq!(body["etf_count"].as_u64(), Some(14), "etf_count: {body}");
}

// ARC01C-04: enabled_non_equity_count == 0 and non_equity_count == 0 — the
// production registry is equities-only today.
#[tokio::test]
async fn arc01c_04_active_status_enabled_non_equity_count_zero() {
    let router = make_router_with_real_registry();
    let (_, body) = get_status(router).await;

    assert_eq!(
        body["non_equity_count"].as_u64(),
        Some(0),
        "non_equity_count: {body}"
    );
    assert_eq!(
        body["enabled_non_equity_count"].as_u64(),
        Some(0),
        "enabled_non_equity_count: {body}"
    );
}

// ARC01C-05: paper_trading_enabled_count == 0 and live_trading_enabled_count
// == 0 — v1 has no such fields, so every converted row defaults false.
#[tokio::test]
async fn arc01c_05_active_status_paper_and_live_trading_enabled_counts_zero() {
    let router = make_router_with_real_registry();
    let (_, body) = get_status(router).await;

    assert_eq!(
        body["paper_trading_enabled_count"].as_u64(),
        Some(0),
        "paper_trading_enabled_count: {body}"
    );
    assert_eq!(
        body["live_trading_enabled_count"].as_u64(),
        Some(0),
        "live_trading_enabled_count: {body}"
    );
}

// ARC01C-06: production_cutover_enabled == false and trading_uses_v2 ==
// false — invariant, must hold even on the active/happy path.
#[tokio::test]
async fn arc01c_06_active_status_production_cutover_and_trading_uses_v2_false() {
    let router = make_router_with_real_registry();
    let (_, body) = get_status(router).await;

    assert_eq!(
        body["production_cutover_enabled"], false,
        "production_cutover_enabled must be false: {body}"
    );
    assert_eq!(
        body["trading_uses_v2"], false,
        "trading_uses_v2 must be false: {body}"
    );
}

// ARC01C-12: asset_class/instrument_kind/contract_kind counts match the real
// production registry shape exactly (88 equity, 14 etf-tagged, 74 plain).
#[tokio::test]
async fn arc01c_12_asset_class_and_contract_kind_counts_match_production_shape() {
    let router = make_router_with_real_registry();
    let (_, body) = get_status(router).await;

    assert_eq!(
        body["asset_class_counts"]["equity"].as_u64(),
        Some(88),
        "asset_class_counts.equity: {body}"
    );
    assert_eq!(
        body["asset_class_counts"].as_object().unwrap().len(),
        1,
        "asset_class_counts must have exactly one key (equity): {body}"
    );
    assert_eq!(
        body["instrument_kind_counts"]["etf"].as_u64(),
        Some(14),
        "instrument_kind_counts.etf: {body}"
    );
    assert_eq!(
        body["instrument_kind_counts"].as_object().unwrap().len(),
        1,
        "instrument_kind_counts must have exactly one key (etf): {body}"
    );
    assert_eq!(
        body["contract_kind_counts"]["etf"].as_u64(),
        Some(14),
        "contract_kind_counts.etf: {body}"
    );
    assert_eq!(
        body["contract_kind_counts"]["equity"].as_u64(),
        Some(74),
        "contract_kind_counts.equity: {body}"
    );
    assert_eq!(
        body["enabled_count"].as_u64(),
        Some(88),
        "enabled_count: {body}"
    );
}

// ARC01C-11: the route is pure/read-only — calling it twice against the same
// state yields byte-identical JSON (no hidden counters, no side effects).
#[tokio::test]
async fn arc01c_11_route_is_idempotent_and_side_effect_free() {
    let router = make_router_with_real_registry();
    let (status_a, body_a) = get_status(router.clone()).await;
    let (status_b, body_b) = get_status(router).await;

    assert_eq!(status_a, StatusCode::OK);
    assert_eq!(status_b, StatusCode::OK);
    assert_eq!(body_a, body_b, "repeated calls must return identical JSON");
}

// ARC01C-10: AppState built via `new_with_operator_auth` carries no DB pool
// (constructed with `None`); the route still returns 200 active. This is
// the route-level proof that no DB pool is required and, by construction
// (there is no pool to write through), that no DB rows are written.
#[tokio::test]
async fn arc01c_10_route_succeeds_without_db_pool_and_cannot_write_db_rows() {
    let router = make_router_with_real_registry();
    let (status, body) = get_status(router).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "route must succeed with no DB pool configured: {body}"
    );
    assert_eq!(body["truth_state"], "active");
}

// ---------------------------------------------------------------------------
// Failure paths
// ---------------------------------------------------------------------------

// ARC01C-07: missing registry path -> 200 (truth envelope), truth_state=unavailable.
#[tokio::test]
async fn arc01c_07_missing_registry_path_reports_unavailable() {
    let router = make_router_with_missing_registry();
    let (status, body) = get_status(router).await;

    assert_eq!(status, StatusCode::OK, "must still return 200: {body}");
    assert_eq!(
        body["truth_state"], "unavailable",
        "truth_state must be unavailable: {body}"
    );
    assert_eq!(body["v1_count"].as_u64(), Some(0));
    assert_eq!(body["v2_count"].as_u64(), Some(0));
    assert_eq!(body["validation_passed"], false);
    assert!(
        body["validation_errors"]
            .as_array()
            .is_some_and(|v| !v.is_empty()),
        "validation_errors must be populated: {body}"
    );
    assert!(
        body["schema_version"].is_null(),
        "schema_version must be null when v1 never loaded: {body}"
    );
    assert_eq!(
        body["production_cutover_enabled"], false,
        "production_cutover_enabled must stay false on failure too: {body}"
    );
    assert_eq!(body["trading_uses_v2"], false);
}

// ARC01C-08: malformed (unparseable) JSON content -> truth_state=v1_load_failed.
#[tokio::test]
async fn arc01c_08_malformed_json_reports_v1_load_failed() {
    let (router, path) = make_router_with_registry_content("{not valid json", "malformed");
    let (status, body) = get_status(router).await;
    std::fs::remove_file(&path).ok();

    assert_eq!(status, StatusCode::OK, "must still return 200: {body}");
    assert_eq!(
        body["truth_state"], "v1_load_failed",
        "truth_state must be v1_load_failed: {body}"
    );
    assert_eq!(body["v1_count"].as_u64(), Some(0));
    assert_eq!(body["v2_count"].as_u64(), Some(0));
    assert_eq!(body["validation_passed"], false);
    assert!(
        body["validation_errors"]
            .as_array()
            .is_some_and(|v| !v.is_empty()),
        "validation_errors must be populated: {body}"
    );
}

// ARC01C-09: v1 JSON that parses cleanly but converts to a v2 shape that
// fails validate_registry_v2 (ETF tagged with no sector/category) ->
// truth_state=v2_validation_failed, with the violation message surfaced and
// v1/v2 counts still reported (conversion itself succeeded; only validation
// failed).
#[tokio::test]
async fn arc01c_09_valid_v1_but_invalid_v2_reports_v2_validation_failed() {
    let (router, path) = make_router_with_registry_content(&etf_missing_sector_fixture(), "etfbad");
    let (status, body) = get_status(router).await;
    std::fs::remove_file(&path).ok();

    assert_eq!(status, StatusCode::OK, "must still return 200: {body}");
    assert_eq!(
        body["truth_state"], "v2_validation_failed",
        "truth_state must be v2_validation_failed: {body}"
    );
    assert_eq!(body["v1_count"].as_u64(), Some(1), "v1 load succeeded: {body}");
    assert_eq!(
        body["v2_count"].as_u64(),
        Some(1),
        "conversion is infallible and total: {body}"
    );
    assert_eq!(body["validation_passed"], false);
    let errors = body["validation_errors"]
        .as_array()
        .expect("validation_errors must be an array");
    assert_eq!(errors.len(), 1, "validator fails closed on first violation: {body}");
    assert!(
        errors[0]
            .as_str()
            .is_some_and(|s| s.contains("missing sector and/or category")),
        "violation message must identify the etf metadata gap: {body}"
    );
    assert_eq!(
        body["production_cutover_enabled"], false,
        "production_cutover_enabled must stay false: {body}"
    );
    assert_eq!(body["trading_uses_v2"], false);
}

// ARC01C-13: route does not 404 / is actually mounted on the public router
// (sanity check distinct from the semantic assertions above).
#[tokio::test]
async fn arc01c_13_route_is_mounted_and_public_no_auth_required() {
    let router = make_router_with_real_registry();
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
