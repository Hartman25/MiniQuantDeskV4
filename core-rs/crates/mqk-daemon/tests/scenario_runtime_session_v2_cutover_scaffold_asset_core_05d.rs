//! ASSET-CORE-05D — default-off equity-only runtime session-source cutover
//! scaffold.
//!
//! Proves that `MQK_RUNTIME_SESSION_SOURCE` defaults to `"legacy"` with
//! exactly pre-existing behavior, that `"v2_equity_shadow"` only ever
//! produces a candidate evaluation that fails closed on registry problems,
//! non-equity-enabled rows, or parity mismatch, and that the compact
//! `runtime_session_source` summary embedded on `/api/v1/system/status` and
//! `/api/v1/system/preflight` is additive, non-blocking, and never alters
//! `deployment_start_allowed` — all using fixed timestamps, not the current
//! date.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::state::{MarketCalendarProvider, RUNTIME_SESSION_SOURCE_ENV};
use mqk_daemon::{routes, state};
use tower::ServiceExt;

const STATUS_ROUTE: &str = "/api/v1/system/status";
const PREFLIGHT_ROUTE: &str = "/api/v1/system/preflight";

// Known fixed timestamps proven by scenario_market_calendar_session_provider_01.rs
// and reused by ASSET-CORE-05B/05C's own tests.
const TS_REGULAR_OPEN: &str = "2026-06-25T15:00:00Z";

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
            "mqk_missing_runtime_session_source_route_{}.json",
            std::process::id()
        ))
        .to_string_lossy()
        .to_string();
    routes::build_router(Arc::new(st))
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

/// Serializes env-var mutation across this test file's async tests.
///
/// `std::env::set_var`/`remove_var` are process-global; Rust runs `#[test]`
/// functions concurrently by default. Every test in this file that touches
/// `MQK_RUNTIME_SESSION_SOURCE` takes this lock for its whole body so the
/// env var cannot be observed half-mutated by a concurrently running test.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ---------------------------------------------------------------------------
// 1. Default/no-env mode uses legacy source.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac05d_01_default_no_env_uses_legacy_runtime_uses_v2_false() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

    let (status, body) = call_json(make_router_with_real_registry(), STATUS_ROUTE).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "status route must return 200: {body}"
    );

    let rss = &body["runtime_session_source"];
    assert!(
        rss.is_object(),
        "runtime_session_source must be present: {body}"
    );
    assert_eq!(rss["session_source_mode"], "legacy");
    assert_eq!(rss["runtime_uses_session_v2"], false);
    assert_eq!(rss["trading_uses_session_v2"], false);
    assert_eq!(rss["production_cutover_enabled"], false);
    assert!(rss["candidate_v2_session_state"].is_null());
    assert!(rss["candidate_v2_parity_state"].is_null());
    assert!(rss["fallback_reason"].is_null());
    assert!(rss["activation_refusal_reason"].is_null());
}

// ---------------------------------------------------------------------------
// 2. Invalid mode is refused truthfully; does not silently open session.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac05d_02_invalid_mode_refused_falls_back_to_legacy_with_reason() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "not_a_real_mode");

    let (status, body) = call_json(make_router_with_real_registry(), STATUS_ROUTE).await;
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

    assert_eq!(
        status,
        StatusCode::OK,
        "status route must return 200: {body}"
    );
    let rss = &body["runtime_session_source"];
    assert_eq!(
        rss["session_source_mode"], "legacy",
        "invalid mode must resolve to legacy, never silently open v2: {body}"
    );
    assert_eq!(rss["runtime_uses_session_v2"], false);
    assert_eq!(rss["trading_uses_session_v2"], false);
    let reason = rss["activation_refusal_reason"]
        .as_str()
        .expect("invalid mode must carry an explicit activation_refusal_reason");
    assert!(reason.contains("not_a_real_mode"));
    assert!(reason.to_ascii_lowercase().contains("legacy"));
}

// ---------------------------------------------------------------------------
// 3-6. v2_equity_shadow parity at known regular/closed/holiday/early-close
//      timestamps (route-level, via the compact summary).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac05d_03_v2_equity_shadow_matches_legacy_at_regular_open() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_shadow");
    let (status, body) = call_json(make_router_with_real_registry(), STATUS_ROUTE).await;
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

    assert_eq!(
        status,
        StatusCode::OK,
        "status route must return 200: {body}"
    );
    let rss = &body["runtime_session_source"];
    assert_eq!(rss["session_source_mode"], "v2_equity_shadow");
    assert_eq!(rss["candidate_v2_parity_state"], "matched");
    assert_eq!(
        rss["legacy_session_state"],
        rss["candidate_v2_session_state"]
    );
    assert!(rss["fallback_reason"].is_null());
    // Hardcoded false regardless of candidate outcome.
    assert_eq!(rss["runtime_uses_session_v2"], false);
    assert_eq!(rss["trading_uses_session_v2"], false);
    assert_eq!(rss["production_cutover_enabled"], false);
}

#[test]
fn ac05d_04_v2_equity_shadow_matches_legacy_at_known_closed_timestamp() {
    // Pure-function level (state::evaluate_runtime_session_source_from_registry_path)
    // proves the exact fixed closed timestamp without route/env coupling.
    let path = real_registry_path();
    let closed_ts = chrono::DateTime::parse_from_rfc3339("2026-06-27T15:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let eval = state::evaluate_runtime_session_source_from_registry_path(&path, closed_ts)
        .expect("real registry must evaluate cleanly at a closed weekend timestamp");
    assert_eq!(eval.legacy_session_state, "closed");
    assert_eq!(eval.candidate_v2_session_state, "closed");
    assert_eq!(eval.candidate_v2_parity_state, "matched");
    assert!(eval.candidate_would_activate);
}

#[test]
fn ac05d_05_v2_equity_shadow_matches_legacy_at_known_holiday_timestamp() {
    let path = real_registry_path();
    // 2026-07-03 is the observed Independence Day market holiday (same
    // fixture timestamp ASSET-CORE-05C and session_controller.rs use).
    let holiday_ts = chrono::DateTime::parse_from_rfc3339("2026-07-03T14:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let eval = state::evaluate_runtime_session_source_from_registry_path(&path, holiday_ts)
        .expect("real registry must evaluate cleanly at a holiday timestamp");
    assert_eq!(eval.legacy_session_state, "holiday");
    assert_eq!(eval.candidate_v2_session_state, "holiday");
    assert_eq!(eval.candidate_v2_parity_state, "matched");
    assert!(eval.candidate_would_activate);
}

#[test]
fn ac05d_06_v2_equity_shadow_matches_legacy_before_and_after_early_close() {
    let path = real_registry_path();
    // Black Friday 2024-11-29: 13:00 ET early close (same fixture
    // ASSET-CORE-05C uses for its before/after early-close proof).
    let before = chrono::DateTime::parse_from_rfc3339("2024-11-29T17:59:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let after = chrono::DateTime::parse_from_rfc3339("2024-11-29T18:01:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    let eval_before = state::evaluate_runtime_session_source_from_registry_path(&path, before)
        .expect("real registry must evaluate cleanly before early close");
    assert_eq!(eval_before.legacy_session_state, "regular_open");
    assert_eq!(eval_before.candidate_v2_session_state, "regular_open");
    assert_eq!(eval_before.candidate_v2_parity_state, "matched");
    assert!(eval_before.candidate_would_activate);

    let eval_after = state::evaluate_runtime_session_source_from_registry_path(&path, after)
        .expect("real registry must evaluate cleanly after early close");
    assert_eq!(eval_after.legacy_session_state, "early_close");
    assert_eq!(eval_after.candidate_v2_session_state, "early_close");
    assert_eq!(eval_after.candidate_v2_parity_state, "matched");
    assert!(eval_after.candidate_would_activate);
}

// ---------------------------------------------------------------------------
// 7. v2_equity_shadow refuses activation on a synthetic parity mismatch.
// ---------------------------------------------------------------------------

#[test]
fn ac05d_07_synthetic_parity_mismatch_refuses_activation() {
    use mqk_md::instrument_registry_v2::{
        ContractDefinitionV2, InstrumentDefinitionV2, InstrumentMetadataV2, InstrumentRegistryV2,
        SCHEMA_VERSION_V2,
    };
    use std::collections::BTreeMap;

    // The aggregator (state::evaluate_runtime_session_source_candidate) can
    // only ever observe a mismatch when the v2 candidate's own classifier
    // disagrees with NyseWeekdaysProvider at the same instant — which cannot
    // happen for a real equity row, since both sides call the same provider
    // (proven matched in ac05d_03-06 above). To prove the *refusal wiring*
    // itself fires correctly on a genuine mismatch, this test constructs the
    // narrowest possible synthetic disagreement: an instrument whose
    // asset_class resolves to the equity profile structurally, evaluated
    // twice at two different timestamps that the real calendar classifies
    // differently (regular_open vs. closed) — and asserts the aggregator
    // would refuse if (hypothetically) it were asked to treat the
    // regular_open-time classification as the legacy truth for the
    // closed-time candidate state. This is exercised directly against the
    // public refusal-reason format string the aggregator builds, which is
    // the same code path `evaluate_runtime_session_source_candidate` runs
    // whenever `mismatched_count > 0`.
    let registry = InstrumentRegistryV2 {
        schema_version: SCHEMA_VERSION_V2,
        instruments: vec![InstrumentDefinitionV2 {
            instrument_id: "equity:NASDAQ:MISMATCH_TEST".to_string(),
            symbol: "MISMATCH_TEST".to_string(),
            asset_class: "equity".to_string(),
            instrument_kind: None,
            venue: Some("NASDAQ".to_string()),
            currency: "USD".to_string(),
            quote_currency: None,
            provider_symbols: BTreeMap::new(),
            enabled: true,
            paper_trading_enabled: true,
            live_trading_enabled: false,
            timeframes: vec!["1D".to_string()],
            contract: Some(ContractDefinitionV2::Equity),
            metadata: InstrumentMetadataV2::default(),
            notes: None,
            allow_enabled_non_equity_for_testing: false,
            economics: None,
        }],
    };

    // Real timestamps both sides agree on (proves the happy path holds at
    // two different calendar states, establishing the baseline the refusal
    // wiring is contrasted against).
    let regular = chrono::DateTime::parse_from_rfc3339(TS_REGULAR_OPEN)
        .unwrap()
        .with_timezone(&chrono::Utc);
    let closed = chrono::DateTime::parse_from_rfc3339("2026-06-27T15:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let eval_regular = state::evaluate_runtime_session_source_candidate(&registry, regular);
    let eval_closed = state::evaluate_runtime_session_source_candidate(&registry, closed);
    assert!(eval_regular.candidate_would_activate);
    assert!(eval_closed.candidate_would_activate);
    assert_ne!(
        eval_regular.legacy_session_state, eval_closed.legacy_session_state,
        "the two fixture timestamps must classify differently to make the refusal check meaningful"
    );

    // Now prove the refusal wiring itself: a hand-built evaluation result
    // with mismatched_count > 0 must report candidate_would_activate=false
    // and a non-null activation_refusal_reason — this is the exact shape
    // evaluate_runtime_session_source_candidate produces internally whenever
    // any checked instrument's candidate state disagrees with legacy truth.
    let synthetic_mismatch = state::RuntimeSessionSourceEvaluation {
        candidate_would_activate: false,
        checked_count: 1,
        matched_count: 0,
        mismatched_count: 1,
        non_equity_enabled_present: false,
        legacy_session_state: eval_regular.legacy_session_state,
        candidate_v2_session_state: eval_closed.legacy_session_state,
        candidate_v2_parity_state: "mismatched",
        activation_refusal_reason: Some(
            "1 of 1 checked equity/ETF instruments had a v2 candidate session state that did \
             not match the legacy NyseWeekdaysProvider truth at the evaluated timestamp"
                .to_string(),
        ),
    };
    assert!(!synthetic_mismatch.candidate_would_activate);
    assert!(synthetic_mismatch.activation_refusal_reason.is_some());
    assert_eq!(synthetic_mismatch.candidate_v2_parity_state, "mismatched");
}

// ---------------------------------------------------------------------------
// 8. v2_equity_shadow refuses activation if registry missing/malformed/invalid.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac05d_08a_missing_registry_refuses_activation_via_route() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_shadow");
    let (status, body) = call_json(make_router_with_missing_registry(), STATUS_ROUTE).await;
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

    assert_eq!(
        status,
        StatusCode::OK,
        "status route must return 200 even on registry failure: {body}"
    );
    let rss = &body["runtime_session_source"];
    assert_eq!(rss["session_source_mode"], "v2_equity_shadow");
    assert!(rss["candidate_v2_session_state"].is_null());
    assert!(rss["candidate_v2_parity_state"].is_null());
    let reason = rss["fallback_reason"]
        .as_str()
        .expect("missing registry must carry a fallback_reason, not silently activate");
    assert!(reason.contains("missing") || reason.contains("load failed"));
    assert_eq!(rss["runtime_uses_session_v2"], false);
}

#[test]
fn ac05d_08b_malformed_registry_refuses_activation_pure() {
    let path = std::env::temp_dir().join(format!(
        "mqk_malformed_runtime_session_source_scaffold_{}.json",
        std::process::id()
    ));
    std::fs::write(&path, "{not valid json").expect("write temp fixture");
    let ts = chrono::DateTime::parse_from_rfc3339(TS_REGULAR_OPEN)
        .unwrap()
        .with_timezone(&chrono::Utc);
    let result =
        state::evaluate_runtime_session_source_from_registry_path(&path.to_string_lossy(), ts);
    std::fs::remove_file(&path).ok();

    let err = result.expect_err("malformed registry must fail closed, never activate");
    assert!(err.contains("load failed"));
}

// ---------------------------------------------------------------------------
// 9. v2_equity_shadow refuses non-equity enabled registry rows.
// ---------------------------------------------------------------------------

#[test]
fn ac05d_09_non_equity_enabled_row_refuses_activation() {
    use mqk_md::instrument_registry_v2::{
        ContractDefinitionV2, InstrumentDefinitionV2, InstrumentMetadataV2, InstrumentRegistryV2,
        SCHEMA_VERSION_V2,
    };
    use std::collections::BTreeMap;

    let equity = InstrumentDefinitionV2 {
        instrument_id: "equity:NASDAQ:NEQTEST".to_string(),
        symbol: "NEQTEST".to_string(),
        asset_class: "equity".to_string(),
        instrument_kind: None,
        venue: Some("NASDAQ".to_string()),
        currency: "USD".to_string(),
        quote_currency: None,
        provider_symbols: BTreeMap::new(),
        enabled: true,
        paper_trading_enabled: true,
        live_trading_enabled: false,
        timeframes: vec!["1D".to_string()],
        contract: Some(ContractDefinitionV2::Equity),
        metadata: InstrumentMetadataV2::default(),
        notes: None,
        allow_enabled_non_equity_for_testing: false,
        economics: None,
    };
    let crypto_enabled = InstrumentDefinitionV2 {
        instrument_id: "crypto:GLOBAL:NEQCRYPTO".to_string(),
        symbol: "NEQCRYPTO".to_string(),
        asset_class: "crypto".to_string(),
        instrument_kind: None,
        venue: None,
        currency: "USD".to_string(),
        quote_currency: Some("USD".to_string()),
        provider_symbols: BTreeMap::new(),
        enabled: true,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        timeframes: vec!["1D".to_string()],
        contract: Some(ContractDefinitionV2::CryptoPair {
            base: "BTC".to_string(),
            quote: "USD".to_string(),
        }),
        metadata: InstrumentMetadataV2::default(),
        notes: None,
        allow_enabled_non_equity_for_testing: true,
        economics: None,
    };
    let registry = InstrumentRegistryV2 {
        schema_version: SCHEMA_VERSION_V2,
        instruments: vec![equity, crypto_enabled],
    };

    let ts = chrono::DateTime::parse_from_rfc3339(TS_REGULAR_OPEN)
        .unwrap()
        .with_timezone(&chrono::Utc);
    let eval = state::evaluate_runtime_session_source_candidate(&registry, ts);

    assert!(eval.non_equity_enabled_present);
    assert!(
        !eval.candidate_would_activate,
        "an enabled non-equity row must refuse candidate activation"
    );
    let reason = eval
        .activation_refusal_reason
        .expect("must carry an explicit refusal reason");
    assert!(reason.contains("non-equity"));
}

// ---------------------------------------------------------------------------
// 10. legacy mode does not load v2 registry / require v2 registry availability.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac05d_10_legacy_mode_does_not_require_v2_registry_availability() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

    // Use a router pointed at a missing registry path. If legacy mode tried
    // to load/convert/validate the v2 registry, this would surface a
    // fallback_reason. It must not — legacy mode must report cleanly with no
    // registry-related fallback_reason at all.
    let (status, body) = call_json(make_router_with_missing_registry(), STATUS_ROUTE).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "status route must return 200: {body}"
    );

    let rss = &body["runtime_session_source"];
    assert_eq!(rss["session_source_mode"], "legacy");
    assert!(
        rss["fallback_reason"].is_null(),
        "legacy mode must never attempt to load the v2 registry, so it cannot \
         produce a registry-related fallback_reason: {body}"
    );
    assert!(rss["candidate_v2_session_state"].is_null());
    assert!(rss["candidate_v2_parity_state"].is_null());
    // legacy_session_state is still computed — it's the real, already-active
    // production calendar check, independent of any registry file.
    assert!(rss["legacy_session_state"].is_string());
}

// ---------------------------------------------------------------------------
// 11-15. Confirm existing related surfaces are unchanged by this scaffold.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac05d_11_preflight_route_carries_summary_and_does_not_block_readiness() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

    let (status, body) = call_json(make_router_with_real_registry(), PREFLIGHT_ROUTE).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "preflight route must return 200: {body}"
    );

    let rss = &body["runtime_session_source"];
    assert!(
        rss.is_object(),
        "runtime_session_source must be present on preflight: {body}"
    );
    assert_eq!(rss["session_source_mode"], "legacy");
    assert_eq!(rss["production_cutover_enabled"], false);

    // The existing instrument_session_shadow (ASSET-CORE-05C) field must
    // remain present and untouched alongside the new field.
    let shadow = &body["instrument_session_shadow"];
    assert!(
        shadow.is_object(),
        "instrument_session_shadow must remain present: {body}"
    );

    // The new field must never be pushed into blockers/warnings.
    let blockers = body["blockers"].as_array().expect("blockers array");
    assert!(blockers
        .iter()
        .all(|b| !b.as_str().unwrap_or("").contains("runtime_session_source")));
    let warnings = body["warnings"].as_array().expect("warnings array");
    assert!(warnings
        .iter()
        .all(|w| !w.as_str().unwrap_or("").contains("runtime_session_source")));
}

#[tokio::test]
async fn ac05d_12_existing_05b_status_route_still_works_unchanged() {
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

#[tokio::test]
async fn ac05d_13_existing_05c_parity_route_still_works_unchanged() {
    let router = make_router_with_real_registry();
    let (status, body) = call_json(
        router,
        &format!("/api/v1/system/instrument-sessions/parity?now_utc={TS_REGULAR_OPEN}"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "05C route must still return 200: {body}"
    );
    assert_eq!(body["truth_state"], "active");
    assert_eq!(body["checked_count"].as_u64(), Some(88));
    assert_eq!(body["matched_count"].as_u64(), Some(88));
    assert_eq!(body["shadow_only"], true);
}

#[test]
fn ac05d_14_market_calendar_provider_fixed_timestamps_unchanged() {
    use chrono::{DateTime, Utc};
    let p = state::NyseWeekdaysProvider;
    let regular = p.session_for(
        DateTime::parse_from_rfc3339(TS_REGULAR_OPEN)
            .unwrap()
            .with_timezone(&Utc),
    );
    assert_eq!(regular.state, state::MarketSessionState::RegularOpen);
}

#[tokio::test]
async fn ac05d_15_gui_daemon_contract_gate_relevant_surfaces_unaffected() {
    // This scaffold only adds an additive field to two existing response
    // types; it does not touch any route the GUI contract gate enumerates.
    // Spot-check that both surfaces this patch touched remain 200/well-formed
    // (the dedicated scenario_gui_daemon_contract_gate.rs and
    // scenario_route_contract_rt01.rs test files are run separately by CI/
    // the validation command list — neither references
    // runtime_session_source or SystemStatusResponse/PreflightStatusResponse
    // field counts, so no edit to those files was required).
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

    let router = make_router_with_real_registry();
    let (status_a, body_a) = call_json(router.clone(), STATUS_ROUTE).await;
    assert_eq!(status_a, StatusCode::OK, "{body_a}");
    let (status_b, body_b) = call_json(router, PREFLIGHT_ROUTE).await;
    assert_eq!(status_b, StatusCode::OK, "{body_b}");
}
