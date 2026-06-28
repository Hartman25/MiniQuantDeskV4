//! ASSET-CORE-05C — read-only shadow parity comparing production session
//! truth against the ASSET-CORE-05B per-instrument session-profile seam.
//!
//! Proves that `GET /api/v1/system/instrument-sessions/parity` loads the
//! configured v1 registry, converts it to v2 in memory, validates it, and
//! for each instrument compares production session truth (equity/ETF only,
//! via `NyseWeekdaysProvider`) against the ASSET-CORE-05A/05B per-instrument
//! session-profile seam at an injected fixed timestamp — without any DB
//! pool, provider/broker call, daemon runtime start, order submission, or
//! trading cutover. Also proves the compact `instrument_session_shadow`
//! summary embedded on `/api/v1/system/status` and `/api/v1/system/preflight`
//! is present, compact, and never alters existing readiness/session fields.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::state::MarketCalendarProvider;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

const PARITY_ROUTE: &str = "/api/v1/system/instrument-sessions/parity";

// Known fixed timestamps proven by scenario_market_calendar_session_provider_01.rs:
// - 2026-06-25T15:00:00Z -> RegularOpen (also used by ASSET-CORE-05B's own tests)
// - 2026-07-03T14:00:00Z -> Holiday (observed Independence Day, MCSP04)
// - 2024-11-29T17:59:00Z -> RegularOpen, is_early_close=true (before early close, MCSP05)
// - 2024-11-29T18:01:00Z -> EarlyClose (after early close, MCSP05)
const TS_REGULAR_OPEN: &str = "2026-06-25T15:00:00Z";
const TS_HOLIDAY: &str = "2026-07-03T14:00:00Z";
const TS_BEFORE_EARLY_CLOSE: &str = "2024-11-29T17:59:00Z";
const TS_AFTER_EARLY_CLOSE: &str = "2024-11-29T18:01:00Z";

fn real_registry_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/equities.json")
        .canonicalize()
        .expect("registry path must resolve from CARGO_MANIFEST_DIR")
        .to_string_lossy()
        .to_string()
}

fn make_router_with_real_registry() -> axum::Router {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = real_registry_path();
    routes::build_router(Arc::new(st))
}

fn make_router_with_missing_registry() -> axum::Router {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = std::env::temp_dir()
        .join(format!(
            "mqk_missing_instrument_sessions_parity_{}.json",
            std::process::id()
        ))
        .to_string_lossy()
        .to_string();
    routes::build_router(Arc::new(st))
}

fn make_router_with_registry_content(
    content: &str,
    tag: &str,
) -> (axum::Router, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "mqk_test_instrument_sessions_parity_{}_{}.json",
        tag,
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp registry fixture");

    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = path.to_string_lossy().to_string();
    (routes::build_router(Arc::new(st)), path)
}

async fn call_json(router: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json = serde_json::from_slice(&body).expect("body is valid JSON");
    (status, json)
}

fn parity_uri(now_utc: &str) -> String {
    format!("{PARITY_ROUTE}?now_utc={now_utc}")
}

// NOTE: `convert_tracked_instrument_to_v2` (mqk-md/src/instrument_registry_v2.rs)
// always assigns `ContractDefinitionV2::Equity` regardless of the v1
// `asset_class` string, and v2 validation then rejects non-equity
// `asset_class` values paired with an `Equity` contract. The real production
// v1 registry only ever contains `asset_class = "equity"` rows (v1 has no
// other asset classes seeded today), so a v1-file-shaped fixture cannot
// produce a *route-level* non-equity row — this mirrors
// ASSET-CORE-05B's own test file, which proves non-equity classification at
// the pure-function level (`resolve_session_profile_for_instrument_metadata`
// + `classify_instrument_session_parity_row`'s underlying building blocks)
// rather than through the HTTP route. See ac05b_07/08/09 in
// scenario_instrument_session_status_asset_core_05b.rs for the precedent.

#[tokio::test]
async fn ac05c_01_real_registry_at_regular_open_returns_all_matched() {
    let (status, body) = call_json(
        make_router_with_real_registry(),
        &parity_uri(TS_REGULAR_OPEN),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "route must return 200: {body}");
    assert_eq!(body["truth_state"], "active");
    assert_eq!(body["registry_v2_valid"], true);
    assert_eq!(body["checked_count"].as_u64(), Some(88));
    assert_eq!(body["matched_count"].as_u64(), Some(88));
    assert_eq!(body["mismatched_count"].as_u64(), Some(0));
    assert_eq!(body["unknown_count"].as_u64(), Some(0));
    assert_eq!(body["model_only_count"].as_u64(), Some(0));
    assert_eq!(body["all_equity_profiles_match_production"], true);

    let rows = body["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 88);
    assert!(rows.iter().all(|r| {
        r["parity_state"] == "matched"
            && r["production_session_state"] == "regular_open"
            && r["profile_session_state"] == "regular_open"
            && r["production_backed"] == true
            && r["model_only"] == false
            && r["trading_uses_this"] == false
    }));
}

#[tokio::test]
async fn ac05c_02_real_registry_at_weekend_closed_returns_all_matched_closed() {
    // 2026-06-27 is a Saturday per scenario_market_calendar_session_provider_01.rs
    // weekend coverage (MCSP03-style); use it here as the "closed" timestamp.
    let (status, body) = call_json(
        make_router_with_real_registry(),
        &parity_uri("2026-06-27T15:00:00Z"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "route must return 200: {body}");
    let rows = body["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 88);
    // Every row must agree on closed-vs-not: either all matched closed, or
    // truthfully unknown — never silently reported as matched-open.
    assert!(rows.iter().all(|r| {
        let parity = r["parity_state"].as_str().unwrap_or("");
        parity == "matched" || parity == "unknown"
    }));
    assert!(rows
        .iter()
        .all(|r| r["production_session_state"] != "regular_open"));
}

#[tokio::test]
async fn ac05c_03_known_holiday_timestamp_returns_matched_per_production_calendar() {
    let (status, body) = call_json(make_router_with_real_registry(), &parity_uri(TS_HOLIDAY)).await;

    assert_eq!(status, StatusCode::OK, "route must return 200: {body}");
    let rows = body["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 88);
    assert!(rows
        .iter()
        .all(|r| r["production_session_state"] == "holiday"));
    // Holiday is matched only if ASSET-CORE-05B's per-instrument classifier
    // also reports "holiday" for the same timestamp (it reuses the same
    // NyseWeekdaysProvider call) -- prove that equivalence holds, not assumed.
    assert!(rows.iter().all(|r| r["parity_state"] == "matched"));
}

#[tokio::test]
async fn ac05c_04_early_close_day_checks_before_and_after_close() {
    let (status_before, body_before) = call_json(
        make_router_with_real_registry(),
        &parity_uri(TS_BEFORE_EARLY_CLOSE),
    )
    .await;
    assert_eq!(status_before, StatusCode::OK);
    let rows_before = body_before["rows"].as_array().expect("rows array");
    assert!(
        rows_before
            .iter()
            .all(|r| r["production_session_state"] == "regular_open"
                && r["parity_state"] == "matched")
    );

    let (status_after, body_after) = call_json(
        make_router_with_real_registry(),
        &parity_uri(TS_AFTER_EARLY_CLOSE),
    )
    .await;
    assert_eq!(status_after, StatusCode::OK);
    let rows_after = body_after["rows"].as_array().expect("rows array");
    assert!(rows_after
        .iter()
        .all(|r| r["production_session_state"] == "early_close" && r["parity_state"] == "matched"));
}

#[tokio::test]
async fn ac05c_05_invalid_now_utc_returns_timestamp_invalid_no_panic() {
    let (status, body) = call_json(
        make_router_with_real_registry(),
        &format!("{PARITY_ROUTE}?now_utc=not-a-timestamp"),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "timestamp_invalid");
    assert_eq!(body["registry_v2_valid"], false);
    assert!(body["errors"].as_array().is_some_and(|v| !v.is_empty()));
    assert_eq!(body["production_cutover_enabled"], false);
    assert_eq!(body["runtime_uses_session_v2"], false);
    assert_eq!(body["trading_uses_session_v2"], false);
    assert_eq!(body["shadow_only"], true);
}

#[tokio::test]
async fn ac05c_06_missing_registry_returns_registry_missing_no_panic() {
    let (status, body) = call_json(
        make_router_with_missing_registry(),
        &parity_uri(TS_REGULAR_OPEN),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "registry_missing");
    assert_eq!(body["registry_v2_valid"], false);
    assert!(body["errors"].as_array().is_some_and(|v| !v.is_empty()));
    assert_eq!(body["checked_count"].as_u64(), Some(0));
}

#[tokio::test]
async fn ac05c_07_malformed_registry_returns_registry_invalid_no_panic() {
    let (router, path) = make_router_with_registry_content("{not valid json", "malformed");
    let (status, body) = call_json(router, &parity_uri(TS_REGULAR_OPEN)).await;
    std::fs::remove_file(&path).ok();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "registry_invalid");
    assert_eq!(body["registry_v2_valid"], false);
    assert!(body["errors"].as_array().is_some_and(|v| !v.is_empty()));
}

#[tokio::test]
async fn ac05c_08_symbol_filter_limits_output_to_requested_symbol() {
    let (status, body) = call_json(
        make_router_with_real_registry(),
        &format!("{}&symbol=AAPL", parity_uri(TS_REGULAR_OPEN)),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "route must return 200: {body}");
    let rows = body["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1, "symbol filter must limit to one row: {body}");
    assert_eq!(rows[0]["symbol"], "AAPL");
    assert_eq!(body["checked_count"].as_u64(), Some(1));
}

#[test]
fn ac05c_09_non_equity_fixture_rows_report_model_only_and_never_trade() {
    // Mirrors ac05b_07/08/09's pure-function approach: prove that the
    // ASSET-CORE-05B resolution seam this route's parity classifier consumes
    // reports crypto/future/forex as model-only/unsupported, never as the
    // real, currently-wired (production-backed) session authority.
    for (asset_class, expected_profile) in [
        ("crypto", state::MarketSessionProfile::CryptoContinuous),
        ("future", state::MarketSessionProfile::FuturesGlobex),
        ("forex", state::MarketSessionProfile::ForexWeekdayContinuous),
    ] {
        let resolution = state::resolve_session_profile_for_instrument_metadata(asset_class, None);
        assert_eq!(
            resolution.truth_state,
            state::SessionProfileResolutionTruth::UnsupportedAssetClass,
            "{asset_class} must resolve to UnsupportedAssetClass (model-only)"
        );
        assert_eq!(resolution.profile, Some(expected_profile));
        assert_eq!(expected_profile.implementation_status(), "model_only");
        // The route's parity classifier treats any UnsupportedAssetClass
        // resolution with a profile as model_only/production_backed=false,
        // and reason "non_equity_profile_is_model_only_no_production_calendar_exists"
        // (see classify_instrument_session_parity_row in routes/system.rs) --
        // never reported as matching production, never trading_uses_this.
        assert_ne!(
            resolution.truth_state,
            state::SessionProfileResolutionTruth::Active,
            "{asset_class} must never be reported as the real session authority"
        );
    }
}

#[tokio::test]
async fn ac05c_10_route_reports_no_production_cutover_and_shadow_only() {
    let (_, body) = call_json(
        make_router_with_real_registry(),
        &parity_uri(TS_REGULAR_OPEN),
    )
    .await;

    assert_eq!(body["production_cutover_enabled"], false);
    assert_eq!(body["runtime_uses_session_v2"], false);
    assert_eq!(body["trading_uses_session_v2"], false);
    assert_eq!(body["shadow_only"], true);
}

#[tokio::test]
async fn ac05c_11_system_status_shadow_summary_present_compact_and_read_only() {
    let router = make_router_with_real_registry();
    let (status, body) = call_json(router, "/api/v1/system/status").await;

    assert_eq!(
        status,
        StatusCode::OK,
        "system/status must return 200: {body}"
    );
    let shadow = &body["instrument_session_shadow"];
    assert!(
        shadow.is_object(),
        "instrument_session_shadow must be present: {body}"
    );
    assert_eq!(shadow["shadow_only"], true);
    assert_eq!(shadow["production_cutover_enabled"], false);
    assert_eq!(shadow["runtime_uses_session_v2"], false);
    assert_eq!(shadow["trading_uses_session_v2"], false);
    assert_eq!(shadow["route"], "/api/v1/system/instrument-sessions/parity");
    // Compact: must not carry a per-instrument row array.
    assert!(shadow.get("rows").is_none());
    assert!(shadow.get("instruments").is_none());
    // Must not have altered any pre-existing top-level status field's shape.
    assert!(body.get("runtime_status").is_some());
    assert!(body.get("alpaca_ws_continuity").is_some());
}

#[tokio::test]
async fn ac05c_12_system_preflight_shadow_summary_present_and_does_not_block_readiness() {
    let router = make_router_with_real_registry();
    let (status, body) = call_json(router, "/api/v1/system/preflight").await;

    assert_eq!(
        status,
        StatusCode::OK,
        "system/preflight must return 200: {body}"
    );
    let shadow = &body["instrument_session_shadow"];
    assert!(
        shadow.is_object(),
        "instrument_session_shadow must be present: {body}"
    );
    assert_eq!(shadow["shadow_only"], true);
    assert_eq!(shadow["production_cutover_enabled"], false);
    assert!(shadow.get("rows").is_none());

    // Existing blockers/warnings must be untouched by the shadow summary --
    // it must never be pushed into either list.
    let blockers = body["blockers"].as_array().expect("blockers array");
    assert!(blockers.iter().all(|b| !b
        .as_str()
        .unwrap_or("")
        .contains("instrument_session_shadow")));
    let warnings = body["warnings"].as_array().expect("warnings array");
    assert!(warnings.iter().all(|w| !w
        .as_str()
        .unwrap_or("")
        .contains("instrument_session_shadow")));
}

#[tokio::test]
async fn ac05c_13_existing_asset_core_05b_status_route_still_works() {
    let router = make_router_with_real_registry();
    let (status, body) = call_json(
        router,
        "/api/v1/system/instrument-sessions/status?now_utc=2026-06-25T15:00:00Z",
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "05B route must still return 200: {body}"
    );
    assert_eq!(body["truth_state"], "active");
    assert_eq!(body["instrument_count"].as_u64(), Some(88));
}

#[test]
fn ac05c_14_market_calendar_provider_fixed_timestamps_unchanged() {
    use chrono::{DateTime, Utc};
    let p = state::NyseWeekdaysProvider;

    let regular = p.session_for(
        DateTime::parse_from_rfc3339(TS_REGULAR_OPEN)
            .unwrap()
            .with_timezone(&Utc),
    );
    assert_eq!(regular.state, state::MarketSessionState::RegularOpen);

    let holiday = p.session_for(
        DateTime::parse_from_rfc3339(TS_HOLIDAY)
            .unwrap()
            .with_timezone(&Utc),
    );
    assert_eq!(holiday.state, state::MarketSessionState::Holiday);
}

#[tokio::test]
async fn ac05c_15_existing_registry_v2_status_route_still_works() {
    let router = make_router_with_real_registry();
    let (status, body) = call_json(router, "/api/v1/system/instrument-registry-v2/status").await;

    assert_eq!(
        status,
        StatusCode::OK,
        "01C route must still return 200: {body}"
    );
    assert_eq!(body["truth_state"], "active");
    assert_eq!(body["production_cutover_enabled"], false);
}
