//! BRK-00R-06 / PT-PROOF-01: Supervised paper-trading proof bundle.
//!
//! # Honest supervised paper-trading claim (PT-PROOF-01)
//!
//! The supervised broker-backed paper runtime path (paper+alpaca) is
//! operator-truthful and fail-closed.  It requires ALL of the following:
//!
//! 1. Deployment mode = Paper AND broker = Alpaca.
//!    paper+paper is explicitly fail-closed (not a valid supervised paper path).
//! 2. Explicit operator arm of the integrity gate.
//! 3. Proven Alpaca WS continuity (`Live` state), established by the real
//!    transport's connect→auth→subscribe handshake before start is attempted.
//!
//! Any missing condition blocks start with an explicit named gate and a
//! machine-readable `fault_class`.  The operator is never silently refused.
//!
//! Once WS continuity reaches `Live`, the WS gate passes and the start request
//! falls through to the DB authority gate.  On WS disconnect, continuity
//! degrades to `GapDetected` and start is re-blocked until the transport
//! re-establishes `Live`.
//!
//! # Gate ordering for paper+alpaca startup
//!
//! ```text
//! POST /v1/run/start
//!   Gate 1: deployment_mode      (paper+paper → 403; paper+alpaca → pass)
//!   Gate 2: integrity_armed      (disarmed → 403)
//!   Gate 3: alpaca_ws_continuity (ColdStartUnproven|GapDetected → 403; Live → pass)
//!   Gate 4: db                   (no DB → 503)   ← first reachable without real runtime
//! ```
//!
//! # What is NOT claimed
//!
//! - Market-data strategy signal injection into the paper runtime path.
//!   The orchestrator tick loop does not receive trading signals from market data
//!   as of this proof bundle.  That wiring remains open.
//!
//! - Persisted WS cursor resume.  On reconnect the transport starts fresh
//!   (`ColdStartUnproven`) and re-establishes `Live` via the auth+subscribe
//!   handshake.  Loading the last known WS cursor from DB before reconnect
//!   is explicitly NOT implemented and NOT claimed.
//!
//! - Gap event recovery via REST polling.  Events that arrived during a WS
//!   disconnect window must be recovered via `BrokerAdapter::fetch_events` on
//!   the next run restart.  That flow is not yet wired for the paper path.
//!
//! - Strategy viability, profitability, or live readiness.
//!
//! # Prior art / related proof slices
//!
//! - BRK-00R-04: gate ordering proofs (P01-P06) in `scenario_ws_continuity_gate_brk00r04.rs`
//! - BRK-00R-05: WS transport helper proofs (T01-T08) in `scenario_alpaca_paper_ws_transport_brk00r05.rs`
//! - BRK-00R-05B: real session path proofs (S1-S4) in `alpaca_ws_transport.rs` unit tests
//! - PT-TRUTH-01: paper+paper fail-closed proof in `scenario_daemon_routes.rs`
//! - AP-05: continuity state type-level proofs in `scenario_daemon_routes.rs`
//!
//! This file closes the remaining proof gap: the happy-path WS gate pass-through
//! (Live continuity → start reaches DB gate) and the fail-closed round-trip
//! (Live → GapDetected → re-block → Live → unblock) as a start-gate sequence.
//!
//! All tests are pure in-process (no DB required).  They exercise the real
//! production gate path in `start_execution_runtime`.

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

async fn arm(st: &Arc<state::AppState>) {
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(routes::build_router(Arc::clone(st)), arm_req).await;
    assert_eq!(status, StatusCode::OK, "arm must succeed");
}

// ---------------------------------------------------------------------------
// BRK00R06-E01 — paper+paper is fail-closed; paper+alpaca is the honest path
//
// Proves:
// - Default config (paper+paper) → start → 403 gate=deployment_mode
//   (paper+paper is NOT a valid supervised paper path)
// - paper+alpaca (integrity not armed) → start → 403 gate=integrity_armed
//   (passes deployment gate; integrity gate is the next blocker)
//
// This establishes that paper+alpaca is the one honest supervised paper path:
// it advances past deployment_mode and is stopped by the gates that require
// operator action (arm) and proven WS continuity.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk00r06_e01_paper_alpaca_is_canonical_honest_paper_path() {
    // --- paper+paper is fail-closed at deployment readiness ---
    let st_pp = Arc::new(state::AppState::new());
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st_pp)), start_req).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "E01: paper+paper must be refused at deployment readiness gate; got: {status}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["gate"], "deployment_mode",
        "E01: paper+paper must fail at deployment_mode gate (not a supervised paper path); got: {json}"
    );

    // --- paper+alpaca passes deployment gate; blocked at integrity ---
    let st_pa = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    // Do NOT arm — integrity gate should be the blocker.
    let start_req2 = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status2, body2) = call(routes::build_router(Arc::clone(&st_pa)), start_req2).await;
    assert_eq!(
        status2,
        StatusCode::FORBIDDEN,
        "E01: paper+alpaca (disarmed) must be refused at integrity gate; got: {status2}"
    );
    let json2 = parse_json(body2);
    assert_eq!(
        json2["gate"], "integrity_armed",
        "E01: paper+alpaca must pass deployment gate and fail at integrity_armed; got: {json2}"
    );
}

// ---------------------------------------------------------------------------
// BRK00R06-E02 — Live continuity unblocks the WS gate; start reaches DB gate
//
// This is the key missing proof for PT-PROOF-01.
//
// Proves:
// - paper+alpaca + armed + ColdStartUnproven → start → 403 gate=alpaca_ws_continuity
//   (confirms the baseline before Live is established)
// - update_ws_continuity(Live) on the real AppState seam
// - paper+alpaca + armed + Live → start → 503 (DB gate)
//   (proves the WS gate pass-through is real: Live continuity → start unblocked)
//
// This is the ONLY test in the repo that proves the happy-path WS gate
// pass-through against the real `start_execution_runtime` code path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk00r06_e02_live_continuity_unblocks_ws_gate_reaches_db_gate() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    arm(&st).await;

    // Baseline: ColdStartUnproven (default on boot) → start blocked at WS gate.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "E02: ColdStartUnproven must block start at WS gate (403); got: {status}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["gate"], "alpaca_ws_continuity",
        "E02: gate must be alpaca_ws_continuity for ColdStartUnproven; got: {json}"
    );

    // Transition continuity to Live (simulates WS transport establishing a live cursor).
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: String::new(),
        last_event_at: String::new(),
    })
    .await;
    assert!(
        st.alpaca_ws_continuity().await.is_continuity_proven(),
        "E02: continuity must be proven after Live update"
    );

    // Now start: WS gate must pass, DB gate must fire (503 — no DB configured).
    let start_req2 = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status2, body2) = call(routes::build_router(Arc::clone(&st)), start_req2).await;
    assert_eq!(
        status2,
        StatusCode::SERVICE_UNAVAILABLE,
        "E02: paper+alpaca + Live continuity must pass WS gate and reach DB gate (503); got: {status2}"
    );
    let json2 = parse_json(body2);
    assert!(
        json2["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "E02: error must be DB gate (not WS gate); got: {json2}"
    );
}

// ---------------------------------------------------------------------------
// BRK00R06-E03 — Fail-closed round-trip: Live → GapDetected → Live
//
// Proves the full continuity lifecycle effect on the start decision:
//
// Step 1: Live → start reaches DB gate (WS gate passes)
// Step 2: GapDetected → start re-blocked at WS gate (fail-closed on disconnect)
// Step 3: Live again → start reaches DB gate again (WS transport re-establishes)
//
// This proves the fail-closed reconnect cycle is enforced at the start gate.
// The operator cannot bypass GapDetected by retrying start — they must wait
// for the transport to re-establish Live.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk00r06_e03_continuity_round_trip_is_fail_closed() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    arm(&st).await;

    let try_start = |st: Arc<state::AppState>| async move {
        let req = Request::builder()
            .method("POST")
            .uri("/v1/run/start")
            .body(axum::body::Body::empty())
            .unwrap();
        call(routes::build_router(st), req).await
    };

    // --- Step 1: Live → WS gate passes → DB gate ---
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:ord-1:new:2026-01-01T00:00:00Z".to_string(),
        last_event_at: "2026-01-01T00:00:00Z".to_string(),
    })
    .await;
    let (s1, b1) = try_start(Arc::clone(&st)).await;
    assert_eq!(
        s1,
        StatusCode::SERVICE_UNAVAILABLE,
        "E03 step1: Live must pass WS gate and reach DB gate (503); got: {s1}"
    );
    assert!(
        parse_json(b1)["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "E03 step1: must be the DB gate, not the WS gate"
    );

    // --- Step 2: GapDetected → WS gate re-blocks → 403 ---
    st.update_ws_continuity(state::AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:ord-1:new:2026-01-01T00:00:00Z".to_string()),
        last_event_at: Some("2026-01-01T00:00:00Z".to_string()),
        detail: "E03: simulated WS disconnect".to_string(),
    })
    .await;
    assert!(
        !st.alpaca_ws_continuity().await.is_continuity_proven(),
        "E03 step2: GapDetected must not be proven"
    );
    let (s2, b2) = try_start(Arc::clone(&st)).await;
    assert_eq!(
        s2,
        StatusCode::FORBIDDEN,
        "E03 step2: GapDetected must re-block start at WS gate (403); got: {s2}"
    );
    let j2 = parse_json(b2);
    assert_eq!(
        j2["gate"], "alpaca_ws_continuity",
        "E03 step2: gate must be alpaca_ws_continuity for GapDetected; got: {j2}"
    );
    assert_eq!(
        j2["fault_class"],
        "runtime.start_refused.paper_alpaca_ws_continuity_unproven",
        "E03 step2: fault_class must identify paper+alpaca continuity refusal; got: {j2}"
    );

    // --- Step 3: Live again → WS gate passes again → DB gate ---
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:ord-1:new:2026-01-01T00:01:00Z".to_string(),
        last_event_at: "2026-01-01T00:01:00Z".to_string(),
    })
    .await;
    assert!(
        st.alpaca_ws_continuity().await.is_continuity_proven(),
        "E03 step3: continuity must be proven after re-establishing Live"
    );
    let (s3, b3) = try_start(Arc::clone(&st)).await;
    assert_eq!(
        s3,
        StatusCode::SERVICE_UNAVAILABLE,
        "E03 step3: re-established Live must again pass WS gate and reach DB gate (503); got: {s3}"
    );
    assert!(
        parse_json(b3)["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "E03 step3: must be DB gate again, not WS gate"
    );
}

// ---------------------------------------------------------------------------
// BRK00R06-E04 — System status surface reflects paper+alpaca continuity truth
//
// Proves that GET /api/v1/system/status returns the correct
// alpaca_ws_continuity string at each state in the paper+alpaca lifecycle:
//
// Boot:          "cold_start_unproven"
// After Live:    "live"
// After Gap:     "gap_detected"
//
// This proves the operator-visible surface (GUI, monitoring) is truthful and
// tracks the real in-memory continuity state through transitions.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk00r06_e04_system_status_surface_reflects_paper_alpaca_continuity_truth() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    let get_continuity_str = |st: Arc<state::AppState>| async move {
        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/system/status")
            .body(axum::body::Body::empty())
            .unwrap();
        let (status, body) = call(routes::build_router(st), req).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "E04: /api/v1/system/status must return 200"
        );
        let json = parse_json(body);
        json["alpaca_ws_continuity"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };

    // Boot: paper+alpaca starts ColdStartUnproven.
    let s1 = get_continuity_str(Arc::clone(&st)).await;
    assert_eq!(
        s1, "cold_start_unproven",
        "E04: boot state for paper+alpaca must be cold_start_unproven; got: {s1}"
    );

    // After WS transport establishes Live (simulated via the seam).
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:ord-1:new:2026-01-01T00:00:00Z".to_string(),
        last_event_at: "2026-01-01T00:00:00Z".to_string(),
    })
    .await;
    let s2 = get_continuity_str(Arc::clone(&st)).await;
    assert_eq!(
        s2, "live",
        "E04: system status must report 'live' after WS transport establishes continuity; got: {s2}"
    );

    // After WS disconnect (GapDetected).
    st.update_ws_continuity(state::AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:ord-1:new:2026-01-01T00:00:00Z".to_string()),
        last_event_at: Some("2026-01-01T00:00:00Z".to_string()),
        detail: "E04: simulated WS disconnect".to_string(),
    })
    .await;
    let s3 = get_continuity_str(Arc::clone(&st)).await;
    assert_eq!(
        s3, "gap_detected",
        "E04: system status must report 'gap_detected' after WS disconnect; got: {s3}"
    );
}

// ---------------------------------------------------------------------------
// PT-MD-01 / BRK00R06-E05 — Paper+alpaca market-data truth is explicit
//
// Proves the paper+alpaca strategy market-data truth claim for PT-MD-01:
//
// - system/status: market_data_health == "not_configured"
//   (StrategyMarketDataSource::NotConfigured → honestly absent, not "unknown")
//
// - system/preflight: market_data_config_present == false (not null)
//   (null would mean "not probed"; false means "probed and explicitly absent")
//
// This closes the truth gap where preflight previously returned null for
// market_data_config_present, implying the status was unknown rather than
// explicitly not wired.
//
// The honest paper+alpaca market-data claim after this patch:
//   Strategy market-data is NOT configured in the current paper+alpaca path.
//   StrategyMarketDataSource has only NotConfigured as a defined variant.
//   Strategy-driven paper execution requires market-data wiring that is
//   explicitly open work and is NOT claimed by the current proof bundle.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ptmd01_e05_paper_alpaca_market_data_truth_is_explicit_not_null() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    // --- system/status: market_data_health is "not_configured" ---
    let status_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status_code, status_body) = call(routes::build_router(Arc::clone(&st)), status_req).await;
    assert_eq!(
        status_code,
        StatusCode::OK,
        "E05: /api/v1/system/status must return 200"
    );
    let status_json = parse_json(status_body);
    assert_eq!(
        status_json["market_data_health"], "not_configured",
        "E05: market_data_health must be 'not_configured' for paper+alpaca (not 'unknown', \
         not null); got: {status_json}"
    );

    // --- system/preflight: market_data_config_present is false (not null) ---
    let preflight_req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/preflight")
        .body(axum::body::Body::empty())
        .unwrap();
    let (preflight_code, preflight_body) =
        call(routes::build_router(Arc::clone(&st)), preflight_req).await;
    assert_eq!(
        preflight_code,
        StatusCode::OK,
        "E05: /api/v1/system/preflight must return 200"
    );
    let preflight_json = parse_json(preflight_body);
    assert_eq!(
        preflight_json["market_data_config_present"], false,
        "E05: market_data_config_present must be false (not null) — \
         explicitly absent, not unchecked; got: {preflight_json}"
    );
    // Confirm null is NOT returned — the old behavior was ambiguous.
    assert!(
        !preflight_json["market_data_config_present"].is_null(),
        "E05: market_data_config_present must not be null; null implies \
         'not probed' but the honest answer is 'probed and explicitly absent'"
    );
}
