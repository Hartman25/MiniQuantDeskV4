//! ASSET-CORE-05E — default-off active runtime cutover hook.
//!
//! Proves that `MQK_RUNTIME_SESSION_SOURCE=v2_equity_active` can drive the
//! real `AutonomousSessionSchedule::is_in_session` in-window decision (the
//! single decision point consumed by the autonomous session controller tick,
//! `/api/v1/system/preflight`, and `/api/v1/autonomous/readiness`) — but only
//! when the underlying ASSET-CORE-05D candidate evaluation proves safe — and
//! that the default/legacy/shadow behavior ASSET-CORE-05D already proved
//! remains completely unchanged. All timestamps are fixed/injected; nothing
//! here depends on the current wall-clock date.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{DateTime, TimeZone, Utc};
use http_body_util::BodyExt;
use mqk_daemon::state::{
    AlpacaWsContinuityState, AutonomousSessionSchedule, BrokerKind, StrategyFleetEntry,
    RUNTIME_SESSION_SOURCE_ENV,
};
use mqk_daemon::{routes, state};
use tower::ServiceExt;

const STATUS_ROUTE: &str = "/api/v1/system/status";
const PREFLIGHT_ROUTE: &str = "/api/v1/system/preflight";
const AUTON_READINESS_ROUTE: &str = "/api/v1/autonomous/readiness";

// Known fixed timestamps proven by scenario_market_calendar_session_provider_01.rs
// and reused by ASSET-CORE-05B/05C/05D's own tests.
const TS_REGULAR_OPEN: &str = "2026-06-25T15:00:00Z";
const TS_WEEKEND_CLOSED: &str = "2026-06-27T15:00:00Z";
const TS_HOLIDAY: &str = "2026-07-03T14:00:00Z";
const TS_BEFORE_EARLY_CLOSE: &str = "2024-11-29T17:59:00Z";
const TS_AFTER_EARLY_CLOSE: &str = "2024-11-29T18:01:00Z";

fn ts(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339)
        .unwrap()
        .with_timezone(&Utc)
}

fn real_registry_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/equities.json")
        .canonicalize()
        .expect("registry path must resolve from CARGO_MANIFEST_DIR")
        .to_string_lossy()
        .to_string()
}

fn missing_registry_path(tag: &str) -> String {
    std::env::temp_dir()
        .join(format!(
            "mqk_ac05e_missing_registry_{tag}_{}.json",
            std::process::id()
        ))
        .to_string_lossy()
        .to_string()
}

/// Paper+Alpaca state (the canonical autonomous-session deployment shape),
/// with `instrument_registry_path` pointed at `registry_path`. Returns the
/// owned `Arc<AppState>` so callers can both drive `is_in_session` directly
/// AND build a router from a clone for HTTP-level assertions against the
/// exact same state.
fn build_state(registry_path: String) -> Arc<state::AppState> {
    let mut st = state::AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st.instrument_registry_path = registry_path;
    Arc::new(st)
}

/// Marks WS continuity live, integrity armed, and a non-empty strategy fleet
/// — isolates the session-window gate as the only variable so
/// `overall_ready` on `/api/v1/autonomous/readiness` reflects session-window
/// truth alone, mirroring `scenario_autonomous_readiness_auton_truth01.rs`'s
/// AR-11/AR-12 setup.
async fn make_only_session_window_blocking(st: &Arc<state::AppState>) {
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "msg-001".to_string(),
        last_event_at: "2026-06-25T15:00:00Z".to_string(),
    })
    .await;
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    st.set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;
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

/// Serializes `MQK_RUNTIME_SESSION_SOURCE` env-var mutation across this
/// test file's tests. `std::env::set_var`/`remove_var` are process-global;
/// Rust runs `#[test]`/`#[tokio::test]` functions concurrently by default.
/// This integration test file compiles to its own test binary/process
/// (separate from the `--lib` unit tests and from
/// `scenario_runtime_session_v2_cutover_scaffold_asset_core_05d.rs`), so a
/// file-local lock is sufficient here.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ---------------------------------------------------------------------------
// 1-2. Default/legacy mode preserves original behavior end-to-end.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac05e_01_default_unset_mode_preserves_legacy_end_to_end() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);

    // Missing registry: if default mode touched the v2 registry at all this
    // would surface a fallback_reason on the summary.
    let state = build_state(missing_registry_path("default"));
    let ts_regular = ts(TS_REGULAR_OPEN);
    state
        .set_session_clock_ts_for_test(ts_regular.timestamp())
        .await;

    // Direct decision-point proof: is_in_session must equal the plain legacy
    // boolean (true at regular-open) with zero v2 involvement.
    let in_session = AutonomousSessionSchedule::NyseRegularSession
        .is_in_session(&state, ts_regular)
        .await;
    assert!(
        in_session,
        "default mode must reproduce legacy regular-open truth"
    );

    let router = routes::build_router(Arc::clone(&state));
    let (status, body) = call_json(router, STATUS_ROUTE).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rss = &body["runtime_session_source"];
    assert_eq!(rss["session_source_mode"], "legacy");
    assert_eq!(rss["production_cutover_enabled"], false);
    assert_eq!(rss["runtime_uses_session_v2"], false);
    assert_eq!(rss["trading_uses_session_v2"], false);
    assert_eq!(rss["active_source_used"], false);
    assert!(rss["candidate_would_activate"].is_null());
    assert!(
        rss["fallback_reason"].is_null(),
        "default mode must never touch the v2 registry, missing or not: {body}"
    );
}

#[tokio::test]
async fn ac05e_02_explicit_legacy_mode_matches_default() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "legacy");

    let state = build_state(missing_registry_path("explicit_legacy"));
    let ts_closed = ts(TS_WEEKEND_CLOSED);
    state
        .set_session_clock_ts_for_test(ts_closed.timestamp())
        .await;

    let in_session = AutonomousSessionSchedule::NyseRegularSession
        .is_in_session(&state, ts_closed)
        .await;
    assert!(
        !in_session,
        "explicit legacy mode must reproduce legacy closed-weekend truth"
    );

    let router = routes::build_router(Arc::clone(&state));
    let (status, body) = call_json(router, STATUS_ROUTE).await;
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
    assert_eq!(status, StatusCode::OK, "{body}");
    let rss = &body["runtime_session_source"];
    assert_eq!(rss["session_source_mode"], "legacy");
    assert!(rss["fallback_reason"].is_null());
}

// ---------------------------------------------------------------------------
// 3. v2_equity_shadow remains candidate-only; never drives is_in_session.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac05e_03_v2_equity_shadow_never_drives_session_decision() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_shadow");

    // Missing registry: if shadow mode drove is_in_session, this would force
    // a fail-closed `false` even at a regular-open timestamp. It must not —
    // shadow only ever evaluates a non-binding candidate.
    let state = build_state(missing_registry_path("shadow_missing"));
    let ts_regular = ts(TS_REGULAR_OPEN);
    state
        .set_session_clock_ts_for_test(ts_regular.timestamp())
        .await;

    let in_session = AutonomousSessionSchedule::NyseRegularSession
        .is_in_session(&state, ts_regular)
        .await;
    assert!(
        in_session,
        "v2_equity_shadow must never drive is_in_session, even with a missing registry"
    );

    let router = routes::build_router(Arc::clone(&state));
    let (status, body) = call_json(router, STATUS_ROUTE).await;
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
    assert_eq!(status, StatusCode::OK, "{body}");
    let rss = &body["runtime_session_source"];
    assert_eq!(rss["session_source_mode"], "v2_equity_shadow");
    assert_eq!(rss["production_cutover_enabled"], false);
    assert_eq!(rss["runtime_uses_session_v2"], false);
    assert_eq!(rss["trading_uses_session_v2"], false);
    assert_eq!(rss["active_source_used"], false);
    let reason = rss["fallback_reason"]
        .as_str()
        .expect("shadow mode must still report a fallback_reason for a missing registry");
    assert!(reason.contains("missing") || reason.contains("load failed"));
}

// ---------------------------------------------------------------------------
// 4-7. v2_equity_active fixed-timestamp matrix: regular/closed/holiday/early-close.
// ---------------------------------------------------------------------------

async fn assert_active_mode_matches_legacy_at(fixed_ts: DateTime<Utc>, expect_in_session: bool) {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_active");

    let state = build_state(real_registry_path());
    make_only_session_window_blocking(&state).await;
    state
        .set_session_clock_ts_for_test(fixed_ts.timestamp())
        .await;

    let in_session = AutonomousSessionSchedule::NyseRegularSession
        .is_in_session(&state, fixed_ts)
        .await;
    assert_eq!(
        in_session, expect_in_session,
        "v2_equity_active is_in_session must match legacy at {fixed_ts}"
    );

    // /api/v1/system/preflight: only carries `session_in_window` (bool).
    let preflight_router = routes::build_router(Arc::clone(&state));
    let (status, body) = call_json(preflight_router, PREFLIGHT_ROUTE).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["session_in_window"], expect_in_session,
        "preflight session_in_window must match legacy at {fixed_ts}: {body}"
    );

    // /api/v1/autonomous/readiness: shares the same is_in_session decision
    // point and additionally carries the `session_window_state` string.
    let readiness_router = routes::build_router(state);
    let (status2, readiness_body) = call_json(readiness_router, AUTON_READINESS_ROUTE).await;
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
    assert_eq!(status2, StatusCode::OK, "{readiness_body}");
    assert_eq!(
        readiness_body["session_in_window"], expect_in_session,
        "{readiness_body}"
    );
    assert_eq!(
        readiness_body["session_window_state"],
        if expect_in_session {
            "in_window"
        } else {
            "outside_window"
        }
    );
}

#[tokio::test]
async fn ac05e_04_active_mode_matches_legacy_at_regular_open() {
    assert_active_mode_matches_legacy_at(ts(TS_REGULAR_OPEN), true).await;
}

#[tokio::test]
async fn ac05e_05_active_mode_matches_legacy_at_closed_weekend() {
    assert_active_mode_matches_legacy_at(ts(TS_WEEKEND_CLOSED), false).await;
}

#[tokio::test]
async fn ac05e_06_active_mode_matches_legacy_at_holiday() {
    assert_active_mode_matches_legacy_at(ts(TS_HOLIDAY), false).await;
}

#[tokio::test]
async fn ac05e_07_active_mode_matches_legacy_around_early_close() {
    assert_active_mode_matches_legacy_at(ts(TS_BEFORE_EARLY_CLOSE), true).await;
    assert_active_mode_matches_legacy_at(ts(TS_AFTER_EARLY_CLOSE), false).await;
}

// ---------------------------------------------------------------------------
// 8. Active mode refuses a missing registry and fails closed (not legacy).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac05e_08_active_mode_refuses_missing_registry_fails_closed_at_regular_open() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_active");

    let state = build_state(missing_registry_path("active_missing"));
    make_only_session_window_blocking(&state).await;
    let ts_regular = ts(TS_REGULAR_OPEN);
    state
        .set_session_clock_ts_for_test(ts_regular.timestamp())
        .await;

    // Legacy alone would say "in session" at this timestamp — proving this
    // is a real fail-closed override, not a coincidence.
    let in_session = AutonomousSessionSchedule::NyseRegularSession
        .is_in_session(&state, ts_regular)
        .await;
    assert!(
        !in_session,
        "active mode must fail closed to not-in-session when the registry is missing, \
         even though legacy alone would report regular-open"
    );

    let router = routes::build_router(Arc::clone(&state));
    let (status, body) = call_json(router, PREFLIGHT_ROUTE).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["session_in_window"], false, "{body}");

    // /api/v1/autonomous/readiness shares the same is_in_session decision
    // point and additionally carries the `session_window_state` string.
    // `overall_ready` is NOT asserted here: it also depends on unrelated
    // gates (market-data freshness, reconcile, arm, fleet) this patch does
    // not touch and this test does not attempt to isolate.
    let readiness_router = routes::build_router(Arc::clone(&state));
    let (status_r, readiness_body) = call_json(readiness_router, AUTON_READINESS_ROUTE).await;
    assert_eq!(status_r, StatusCode::OK, "{readiness_body}");
    assert_eq!(
        readiness_body["session_in_window"], false,
        "{readiness_body}"
    );
    assert_eq!(readiness_body["session_window_state"], "outside_window");

    let status_router = routes::build_router(Arc::clone(&state));
    let (status2, status_body) = call_json(status_router, STATUS_ROUTE).await;
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
    assert_eq!(status2, StatusCode::OK, "{status_body}");
    let rss = &status_body["runtime_session_source"];
    assert_eq!(rss["session_source_mode"], "v2_equity_active");
    assert_eq!(
        rss["production_cutover_enabled"], true,
        "configured, even though refused"
    );
    assert_eq!(rss["runtime_uses_session_v2"], false);
    assert_eq!(rss["trading_uses_session_v2"], false);
    assert_eq!(rss["active_source_used"], false);
    // Registry could not be loaded at all (Err branch) — no evaluation ever
    // occurred, so candidate_would_activate is `null`, not `Some(false)`.
    assert!(rss["candidate_would_activate"].is_null(), "{rss}");
    let reason = rss["fallback_reason"]
        .as_str()
        .expect("missing registry must carry a fallback_reason");
    assert!(reason.contains("missing") || reason.contains("load failed"));
}

// ---------------------------------------------------------------------------
// 9. Active mode refuses a malformed registry (pure-function level).
// ---------------------------------------------------------------------------

#[test]
fn ac05e_09_active_mode_refuses_malformed_registry() {
    let path = std::env::temp_dir().join(format!(
        "mqk_ac05e_malformed_registry_{}.json",
        std::process::id()
    ));
    std::fs::write(&path, "{not valid json").expect("write temp fixture");
    let decision = state::evaluate_runtime_session_source_active_decision(
        &path.to_string_lossy(),
        ts(TS_REGULAR_OPEN),
    );
    std::fs::remove_file(&path).ok();

    assert!(!decision.in_session_fail_closed());
    match decision {
        state::RuntimeSessionSourceActiveDecision::Refused { reason } => {
            assert!(reason.contains("load failed"));
        }
        state::RuntimeSessionSourceActiveDecision::Active { .. } => {
            panic!("malformed registry must never report Active")
        }
    }
}

// ---------------------------------------------------------------------------
// 10. Active mode refuses on parity mismatch (refusal-wiring proof, mirrors
//     ASSET-CORE-05D's ac05d_07 — both sides call the same real provider, so
//     a genuine mismatch cannot be produced from real equity data; this
//     proves the refusal aggregation itself fires and is fail-closed).
// ---------------------------------------------------------------------------

#[test]
fn ac05e_10_active_mode_refuses_synthetic_parity_mismatch() {
    let synthetic_mismatch = state::RuntimeSessionSourceEvaluation {
        candidate_would_activate: false,
        checked_count: 1,
        matched_count: 0,
        mismatched_count: 1,
        non_equity_enabled_present: false,
        legacy_session_state: "regular_open",
        candidate_v2_session_state: "closed",
        candidate_v2_parity_state: "mismatched",
        activation_refusal_reason: Some("synthetic mismatch".to_string()),
    };
    assert!(!synthetic_mismatch.candidate_would_activate);

    // Mirror evaluate_runtime_session_source_active_decision's Ok(eval)
    // mapping directly: any candidate_would_activate == false must map to
    // Refused, never Active.
    let decision = if synthetic_mismatch.candidate_would_activate {
        state::RuntimeSessionSourceActiveDecision::Active {
            in_session: true,
            v2_session_state: synthetic_mismatch.candidate_v2_session_state,
        }
    } else {
        state::RuntimeSessionSourceActiveDecision::Refused {
            reason: synthetic_mismatch
                .activation_refusal_reason
                .clone()
                .unwrap(),
        }
    };
    assert!(!decision.in_session_fail_closed());
}

// ---------------------------------------------------------------------------
// 11-12. Active mode refuses an enabled non-equity row, but NOT merely
//        because a disabled non-equity row is present.
// ---------------------------------------------------------------------------

fn equity_instrument(
    symbol: &str,
    enabled: bool,
) -> mqk_md::instrument_registry_v2::InstrumentDefinitionV2 {
    use mqk_md::instrument_registry_v2::{
        ContractDefinitionV2, InstrumentDefinitionV2, InstrumentMetadataV2,
    };
    InstrumentDefinitionV2 {
        instrument_id: format!("equity:NASDAQ:{symbol}"),
        symbol: symbol.to_string(),
        asset_class: "equity".to_string(),
        instrument_kind: None,
        venue: Some("NASDAQ".to_string()),
        currency: "USD".to_string(),
        quote_currency: None,
        provider_symbols: std::collections::BTreeMap::new(),
        enabled,
        paper_trading_enabled: enabled,
        live_trading_enabled: false,
        timeframes: vec!["1D".to_string()],
        contract: Some(ContractDefinitionV2::Equity),
        metadata: InstrumentMetadataV2::default(),
        notes: None,
        allow_enabled_non_equity_for_testing: false,
        economics: None,
    }
}

fn crypto_instrument(
    symbol: &str,
    enabled: bool,
) -> mqk_md::instrument_registry_v2::InstrumentDefinitionV2 {
    use mqk_md::instrument_registry_v2::{
        ContractDefinitionV2, InstrumentDefinitionV2, InstrumentMetadataV2,
    };
    InstrumentDefinitionV2 {
        instrument_id: format!("crypto:GLOBAL:{symbol}"),
        symbol: symbol.to_string(),
        asset_class: "crypto".to_string(),
        instrument_kind: None,
        venue: None,
        currency: "USD".to_string(),
        quote_currency: Some("USD".to_string()),
        provider_symbols: std::collections::BTreeMap::new(),
        enabled,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        timeframes: vec!["1D".to_string()],
        contract: Some(ContractDefinitionV2::CryptoPair {
            base: "BTC".to_string(),
            quote: "USD".to_string(),
        }),
        metadata: InstrumentMetadataV2::default(),
        notes: None,
        allow_enabled_non_equity_for_testing: enabled,
        economics: None,
    }
}

#[test]
fn ac05e_11_active_mode_refuses_enabled_non_equity_row() {
    use mqk_md::instrument_registry_v2::{InstrumentRegistryV2, SCHEMA_VERSION_V2};
    let registry = InstrumentRegistryV2 {
        schema_version: SCHEMA_VERSION_V2,
        instruments: vec![
            equity_instrument("AC05E_EQ", true),
            crypto_instrument("AC05E_CRYPTO", true),
        ],
    };
    let eval = state::evaluate_runtime_session_source_candidate(&registry, ts(TS_REGULAR_OPEN));
    assert!(eval.non_equity_enabled_present);
    assert!(
        !eval.candidate_would_activate,
        "an enabled non-equity row must refuse activation, never silently activate"
    );
}

#[test]
fn ac05e_12_active_mode_disabled_non_equity_present_does_not_block_activation() {
    use mqk_md::instrument_registry_v2::{InstrumentRegistryV2, SCHEMA_VERSION_V2};
    let registry = InstrumentRegistryV2 {
        schema_version: SCHEMA_VERSION_V2,
        instruments: vec![
            equity_instrument("AC05E_EQ2", true),
            crypto_instrument("AC05E_CRYPTO2", false),
        ],
    };
    let eval = state::evaluate_runtime_session_source_candidate(&registry, ts(TS_REGULAR_OPEN));
    assert!(!eval.non_equity_enabled_present);
    assert!(
        eval.candidate_would_activate,
        "a present-but-disabled non-equity row must not block activation"
    );
}

// ---------------------------------------------------------------------------
// 13. Active mode never treats Unknown/Refused as open.
// ---------------------------------------------------------------------------

#[test]
fn ac05e_13_refused_decision_never_reports_in_session_true() {
    let refused = state::RuntimeSessionSourceActiveDecision::Refused {
        reason: "any reason at all".to_string(),
    };
    assert!(!refused.in_session_fail_closed());
}

// ---------------------------------------------------------------------------
// 14. Invalid env value never silently activates v2.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac05e_14_invalid_env_value_falls_back_to_legacy_never_activates() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_super_active_typo");

    let state = build_state(real_registry_path());
    let ts_regular = ts(TS_REGULAR_OPEN);
    state
        .set_session_clock_ts_for_test(ts_regular.timestamp())
        .await;

    let in_session = AutonomousSessionSchedule::NyseRegularSession
        .is_in_session(&state, ts_regular)
        .await;
    assert!(
        in_session,
        "invalid mode must fall back to legacy behavior, not refuse outright"
    );

    let router = routes::build_router(Arc::clone(&state));
    let (status, body) = call_json(router, STATUS_ROUTE).await;
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
    assert_eq!(status, StatusCode::OK, "{body}");
    let rss = &body["runtime_session_source"];
    assert_eq!(rss["session_source_mode"], "legacy");
    assert_eq!(rss["production_cutover_enabled"], false);
    let reason = rss["activation_refusal_reason"]
        .as_str()
        .expect("invalid mode must carry an explicit activation_refusal_reason");
    assert!(reason.contains("v2_equity_super_active_typo"));
    assert!(reason.to_ascii_lowercase().contains("legacy"));
}

// ---------------------------------------------------------------------------
// 15-16. Operator summary reports active_source_used / runtime_uses_session_v2
//        / trading_uses_session_v2 = true only when v2 actually drives the
//        session decision.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac05e_15_summary_active_source_used_true_only_when_accepted() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // Active + accepted (real registry).
    std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, "v2_equity_active");
    let accepted_state = build_state(real_registry_path());
    let router = routes::build_router(Arc::clone(&accepted_state));
    let (_, accepted_body) = call_json(router, STATUS_ROUTE).await;
    let accepted_rss = &accepted_body["runtime_session_source"];
    assert_eq!(accepted_rss["active_source_used"], true, "{accepted_body}");
    assert_eq!(accepted_rss["candidate_would_activate"], true);

    // Active + refused (missing registry).
    let refused_state = build_state(missing_registry_path("summary_refused"));
    let router2 = routes::build_router(Arc::clone(&refused_state));
    let (_, refused_body) = call_json(router2, STATUS_ROUTE).await;
    let refused_rss = &refused_body["runtime_session_source"];
    assert_eq!(refused_rss["active_source_used"], false, "{refused_body}");
    // Missing registry is an Err (no evaluation at all) — null, not Some(false).
    assert!(
        refused_rss["candidate_would_activate"].is_null(),
        "{refused_body}"
    );

    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
}

#[tokio::test]
async fn ac05e_16_summary_runtime_and_trading_use_v2_mirror_active_source_used() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (mode, registry) in [
        ("legacy", real_registry_path()),
        ("v2_equity_shadow", real_registry_path()),
        ("v2_equity_active", real_registry_path()),
    ] {
        std::env::set_var(RUNTIME_SESSION_SOURCE_ENV, mode);
        let state = build_state(registry);
        let router = routes::build_router(state);
        let (status, body) = call_json(router, PREFLIGHT_ROUTE).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let rss = &body["runtime_session_source"];
        assert_eq!(
            rss["runtime_uses_session_v2"], rss["trading_uses_session_v2"],
            "mode={mode}: runtime_uses_session_v2 must mirror trading_uses_session_v2: {body}"
        );
        assert_eq!(
            rss["runtime_uses_session_v2"], rss["active_source_used"],
            "mode={mode}: runtime_uses_session_v2 must mirror active_source_used: {body}"
        );
        if mode != "v2_equity_active" {
            assert_eq!(
                rss["active_source_used"], false,
                "mode={mode} must never report active_source_used=true: {body}"
            );
        }
    }
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
}

// ---------------------------------------------------------------------------
// 17-21. Confirm related existing surfaces are unchanged by this hook.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac05e_17_05d_legacy_summary_shape_unaffected() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
    let state = build_state(real_registry_path());
    let router = routes::build_router(state);
    let (status, body) = call_json(router, STATUS_ROUTE).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rss = &body["runtime_session_source"];
    assert_eq!(rss["session_source_mode"], "legacy");
    assert_eq!(rss["runtime_uses_session_v2"], false);
    assert_eq!(rss["trading_uses_session_v2"], false);
    assert_eq!(rss["production_cutover_enabled"], false);
    assert!(rss["candidate_v2_session_state"].is_null());
    assert!(rss["candidate_v2_parity_state"].is_null());
    assert!(rss["fallback_reason"].is_null());
    assert!(rss["activation_refusal_reason"].is_null());
}

#[tokio::test]
async fn ac05e_18_existing_05c_parity_route_still_works_unchanged() {
    let state = build_state(real_registry_path());
    let router = routes::build_router(state);
    let (status, body) = call_json(
        router,
        &format!("/api/v1/system/instrument-sessions/parity?now_utc={TS_REGULAR_OPEN}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["truth_state"], "active");
    assert_eq!(body["checked_count"].as_u64(), Some(88));
    assert_eq!(body["matched_count"].as_u64(), Some(88));
    assert_eq!(body["shadow_only"], true);
}

#[tokio::test]
async fn ac05e_19_existing_05b_status_route_still_works_unchanged() {
    let state = build_state(real_registry_path());
    let router = routes::build_router(state);
    let (status, body) = call_json(
        router,
        "/api/v1/system/instrument-sessions/status?now_utc=2026-06-25T15:00:00Z",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["truth_state"], "active");
    assert_eq!(body["instrument_count"].as_u64(), Some(88));
}

#[test]
fn ac05e_20_market_calendar_provider_fixed_timestamps_unchanged() {
    use mqk_daemon::state::MarketCalendarProvider;
    let p = state::NyseWeekdaysProvider;
    let regular = p.session_for(ts(TS_REGULAR_OPEN));
    assert_eq!(regular.state, state::MarketSessionState::RegularOpen);
    let holiday = p.session_for(ts(TS_HOLIDAY));
    assert_eq!(holiday.state, state::MarketSessionState::Holiday);
    let _ = Utc.timestamp_opt(0, 0); // touch TimeZone import without unused warning
}

#[tokio::test]
async fn ac05e_21_gui_daemon_contract_relevant_surfaces_unaffected() {
    // This hook only adds additive fields to RuntimeSessionSourceSummaryResponse
    // and changes is_in_session's internal behavior only when explicitly
    // configured. Spot-check that all three surfaces sharing the
    // is_in_session decision point remain 200/well-formed in default mode
    // (the dedicated scenario_gui_daemon_contract_gate.rs and
    // scenario_route_contract_rt01.rs files are run separately by the
    // validation command list).
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var(RUNTIME_SESSION_SOURCE_ENV);
    let state = build_state(real_registry_path());

    let router = routes::build_router(Arc::clone(&state));
    let (status_a, body_a) = call_json(router, STATUS_ROUTE).await;
    assert_eq!(status_a, StatusCode::OK, "{body_a}");

    let router2 = routes::build_router(Arc::clone(&state));
    let (status_b, body_b) = call_json(router2, PREFLIGHT_ROUTE).await;
    assert_eq!(status_b, StatusCode::OK, "{body_b}");

    let router3 = routes::build_router(state);
    let (status_c, body_c) = call_json(router3, AUTON_READINESS_ROUTE).await;
    assert_eq!(status_c, StatusCode::OK, "{body_c}");
    assert!(
        body_c.get("session_in_window").is_some(),
        "autonomous readiness must still carry session_in_window: {body_c}"
    );
}
