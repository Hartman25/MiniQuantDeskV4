//! # Proof bundle: paper+alpaca supervised paper trading
//! ## Phase 1 (low-supervision day) + Phase 2 (current autonomous slices)
//!
//! Patches: BRK-00R-06 · PT-PROOF-01 · PT-DAY-01 · PT-DAY-02 · PT-DAY-03
//!        · PT-DAY-04 · PT-DAY-05 · PT-AUTO-01 · PT-AUTO-01B · PT-AUTO-02
//!        · PT-AUTO-03 · PT-AUTO-04
//!
//! ─────────────────────────────────────────────────────────────────────────
//! ## Honest Phase 1 claim  (BRK-00R-06 through PT-DAY-05)
//! ─────────────────────────────────────────────────────────────────────────
//!
//! The paper+alpaca path is safe and operator-truthful for **low-supervision**
//! operation during NYSE regular session hours.
//!
//! **What Phase 1 proves:**
//!
//! 1. Deployment gate: paper+paper is fail-closed; paper+alpaca is the only
//!    valid broker-backed supervised paper path.
//! 2. Integrity gate: operator must arm before any execution is accepted.
//! 3. WS continuity gate on startup: `Live` state required before start;
//!    `ColdStartUnproven` and `GapDetected` block start.
//! 4. Strategy signal ingestion: `POST /api/v1/strategy/signal` is the
//!    production entry point.  External signal producers consume real market
//!    data and POST pre-computed signals.  The daemon validates, gates, and
//!    enqueues them for broker-backed execution.
//! 5. Continuity fault blocking on signal ingestion: signals are refused when
//!    WS continuity is `GapDetected` or `ColdStartUnproven` (PT-DAY-02).
//! 6. Session-boundary blocking: signals are refused outside NYSE regular
//!    session hours 09:30–16:00 ET, NYSE weekdays excluding holidays (PT-DAY-03).
//! 7. Operator escalation: the first `GapDetected` signal refusal per gap
//!    window triggers a Discord notification (PT-DAY-04).
//!
//! ─────────────────────────────────────────────────────────────────────────
//! ## Honest Phase 2 autonomous claim  (PT-AUTO-01 through PT-AUTO-04)
//! ─────────────────────────────────────────────────────────────────────────
//!
//! Once the operator has armed the integrity gate and started a run, **three
//! narrowly-scoped autonomous protections** operate without further operator
//! action.  All three are fail-closed additions to the Phase 1 gate chain.
//!
//! **What Phase 2 proves and claims:**
//!
//! A. **Execution-loop self-halt on WS gap** (PT-AUTO-01 / PT-AUTO-01B):
//!    If WS continuity transitions to `GapDetected` while the execution loop
//!    is running, the loop self-halts before the next `orchestrator.tick()`.
//!    No further orders are dispatched.  Integrity is disarmed and halted.
//!    The real production loop path (spawn_execution_loop) is exercised, not
//!    just the predicate.  `ColdStartUnproven` does not trigger mid-loop halt
//!    (the loop is not running in that state).
//!
//! B. **Per-run signal intake bound** (PT-AUTO-02):
//!    Gate 1d refuses further signals once 100 distinct new outbox enqueues
//!    have been accepted in the current run, returning 409/day_limit_reached.
//!    The counter resets to 0 at each run start.  Duplicate signals do not
//!    consume quota.  The bound is per-run, not per-process-lifetime.
//!
//! C. **Autonomous-paper state visible on system/status** (PT-AUTO-03):
//!    `GET /api/v1/system/status` surfaces `autonomous_signal_count` and
//!    `autonomous_signal_limit_hit` for paper+alpaca, derived from real
//!    enforced production state.  Both fields are `null` for other deployments.
//!    An operator can determine from a single API call whether Gate 1d is
//!    currently blocking all further signals and how many have been accepted.
//!
//! **The consolidated Phase 2 claim is exactly this:**
//!
//!    The paper+alpaca execution path now has three proven, narrowly-scoped
//!    autonomous protections that operate without further operator action once
//!    a run is started.  These protections tighten the existing fail-closed
//!    chain.  They do not remove the requirement for operator oversight of
//!    session start and stop.
//!
//! **What is NOT claimed (future work):**
//!
//! - Autonomous session start/stop.  The operator must arm, start, and stop
//!   runs manually.  Day-start and day-end automation is future work.
//! - Integrated signal generation.  Signal producers are external to the
//!   daemon.  End-to-end market-data → signal → execution within a single
//!   loop is future work.
//! - Gap event recovery.  Events that arrive during a WS disconnect must be
//!   recovered via `BrokerAdapter::fetch_events` on the next run restart.
//!   That flow is not yet wired for the paper path.
//! - Gap event recovery on reconnect.  The transport starts `ColdStartUnproven`
//!   after reconnect and re-establishes `Live` via handshake.  The in-session
//!   cursor is seeded from the last persisted broker cursor at each session
//!   start (BRK-07R), anchoring gap-detection to the prior position.  Events
//!   that arrived during the disconnect are NOT automatically recovered; REST
//!   gap recovery via `BrokerAdapter::fetch_events` on run restart is open.
//! - Broad alert coverage.  Only the continuity-gap fault class has Discord
//!   escalation.  Session-boundary and day-limit escalation are future work.
//! - Signal-limit fault signal in `build_fault_signals` / `alerts_active`.
//!   The day-limit state is visible on `system/status` but not yet in the
//!   fault-signal feed.
//! - Strategy viability, profitability, or live trading readiness.
//!
//! ## Full gate sequence: `POST /api/v1/strategy/signal`
//!
//! ```text
//! POST /api/v1/strategy/signal
//!   validate body
//!   Gate 1:  signal_ingestion_configured  (PT-DAY-01) → 503/unavailable if not ExternalSignalIngestion
//!   Gate 1b: alpaca_ws_continuity         (PT-DAY-02) → 503/continuity_gap if GapDetected
//!                                                      → 503/unavailable   if ColdStartUnproven
//!                                                      [escalation: one Discord notify per gap window]
//!   Gate 1c: nyse_session                 (PT-DAY-03) → 409/outside_session if not "regular"
//!   Gate 1d: day_signal_limit             (PT-AUTO-02) → 409/day_limit_reached if count >= MAX
//!   lifecycle_guard()
//!   Gate 2:  db_present                               → 503/unavailable if no DB
//!   Gate 3:  arm_state == ARMED                       → 403/rejected    if not armed
//!   Gate 4:  active_run present                       → 409/unavailable if no active run
//!   Gate 5:  runtime_state == "running"               → 409/unavailable if not running
//!   Gate 6:  strategy_not_suppressed                  → 409/suppressed  if active suppression
//!   Gate 7:  outbox_enqueue                           → 200/enqueued    (idempotent)
//! ```
//!
//! ## Gate sequence: `POST /v1/run/start`
//!
//! ```text
//! POST /v1/run/start
//!   Gate 1: deployment_mode      (paper+paper → 403; paper+alpaca → pass)
//!   Gate 2: integrity_armed      (disarmed → 403)
//!   Gate 3: alpaca_ws_continuity (ColdStartUnproven|GapDetected → 403; Live → pass)
//!   Gate 4: db                   (no DB → 503)   ← first reachable without real runtime
//! ```
//!
//! ## Proof map
//!
//! | Test | Covers | Patch |
//! |------|--------|-------|
//! | E01  | paper+paper fail-closed; paper+alpaca is the honest path | BRK-00R-06 |
//! | E02  | Live continuity unblocks start gate → reaches DB gate | BRK-00R-06 |
//! | E03  | Fail-closed round-trip: Live → Gap → re-block → Live | BRK-00R-06 |
//! | E04  | System status surface reflects continuity state | BRK-00R-06 |
//! | E05  | paper+alpaca market_data_health = "signal_ingestion_ready" | PT-DAY-01 |
//! | E06  | strategy signal route real; DB gate fires when all guards pass | PT-DAY-01 |
//! | E07  | GapDetected blocks signal (503/continuity_gap) | PT-DAY-02 |
//! | E08  | ColdStartUnproven blocks signal (503/unavailable) | PT-DAY-02 |
//! | E09  | Live continuity passes gate 1b; signal reaches DB gate | PT-DAY-02 |
//! | E10a | Saturday (closed) blocks signal (409/outside_session) | PT-DAY-03 |
//! | E10b | Premarket blocks signal (409/outside_session) | PT-DAY-03 |
//! | E10c | Regular session passes gate 1c; signal reaches DB gate | PT-DAY-03 |
//! | E10d | After-hours blocks signal (409/outside_session) | PT-DAY-03 |
//! | E11a | First gap refusal claims escalation flag | PT-DAY-04 |
//! | E11b | Second gap refusal deduped (flag stays true) | PT-DAY-04 |
//! | E11c | Live transition resets escalation flag | PT-DAY-04 |
//! | E11d | ColdStartUnproven does not claim escalation | PT-DAY-04 |
//! | E12  | Phase 1 happy-path: all gates satisfied → signal reaches DB gate | PT-DAY-05 |
//! | E13a | paper+alpaca + GapDetected → ws_continuity_gap_requires_halt() true | PT-AUTO-01 |
//! | E13b | paper+alpaca + Live → ws_continuity_gap_requires_halt() false | PT-AUTO-01 |
//! | E13c | paper+alpaca + ColdStartUnproven → ws_continuity_gap_requires_halt() false | PT-AUTO-01 |
//! | E13d | paper+paper (NotConfigured) → ws_continuity_gap_requires_halt() false | PT-AUTO-01 |
//! | E14a | real loop self-halts on GapDetected: exits w/ PT-AUTO-01 note + disarmed+halted | PT-AUTO-01B |
//! | E14b | real loop exits for different reason on Live (PT-AUTO-01 does not fire) | PT-AUTO-01B |
//! | E15a | count=0 + Live: Gate 1d passes, signal reaches DB gate (503/unavailable) | PT-AUTO-02 |
//! | E15b | count=MAX: Gate 1d fires, signal refused with 409/day_limit_reached | PT-AUTO-02 |
//! | E16a | system/status surfaces autonomous_signal_count=0, limit_hit=false (healthy) | PT-AUTO-03 |
//! | E16b | system/status surfaces autonomous_signal_count=MAX, limit_hit=true (blocked) | PT-AUTO-03 |
//! | E16c | system/status autonomous fields are null for non-ExternalSignalIngestion (paper+paper) | PT-AUTO-03 |
//! | E17  | consolidated Phase 2 healthy path: all 4 gates pass + status correct simultaneously | PT-AUTO-04 |
//!
//! ## Prior art / related proof slices
//!
//! - BRK-00R-04: start-gate ordering proofs (P01-P06) in `scenario_ws_continuity_gate_brk00r04.rs`
//! - BRK-00R-05: WS transport helper proofs (T01-T08) in `scenario_alpaca_paper_ws_transport_brk00r05.rs`
//! - BRK-00R-05B: real session path proofs (S1-S4) in `alpaca_ws_transport.rs` unit tests
//! - PT-TRUTH-01: paper+paper fail-closed proof in `scenario_daemon_routes.rs`
//! - AP-05: continuity state type-level proofs in `scenario_daemon_routes.rs`
//! - TV-EXEC-01: fill quality telemetry proofs in `scenario_fill_quality_orchestrator_tv_exec01b.rs`
//!
//! All tests are pure in-process (no DB required) and exercise real production
//! gate paths — not helper stubs.

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
        j2["fault_class"], "runtime.start_refused.paper_alpaca_ws_continuity_unproven",
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
// PT-DAY-01: paper+alpaca now configures ExternalSignalIngestion — strategy
//   signals are accepted via POST /api/v1/strategy/signal for broker-backed
//   paper execution.  market_data_health now reflects "signal_ingestion_ready".
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ptday01_e05_paper_alpaca_signal_ingestion_configured_in_status() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    // --- system/status: market_data_health is "signal_ingestion_ready" ---
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
        status_json["market_data_health"], "signal_ingestion_ready",
        "E05: market_data_health must be 'signal_ingestion_ready' for paper+alpaca \
         after PT-DAY-01 wires ExternalSignalIngestion; got: {status_json}"
    );

    // --- system/preflight: market_data_config_present is true (signal ingestion wired) ---
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
        preflight_json["market_data_config_present"], true,
        "E05: market_data_config_present must be true for paper+alpaca — \
         ExternalSignalIngestion is wired; got: {preflight_json}"
    );
    assert!(
        !preflight_json["market_data_config_present"].is_null(),
        "E05: market_data_config_present must not be null"
    );
}

// ---------------------------------------------------------------------------
// PT-DAY-01-E06 — strategy signal route is real and fail-closed
//
// Proves:
// - POST /api/v1/strategy/signal is reachable for paper+alpaca (wired on
//   the operator router).
// - Without DB the route returns 503 (fail-closed: DB required for execution
//   truth — this is the correct behavior for the honest paper path).
// - paper+paper (default) returns 503 at gate 1: signal ingestion not
//   configured — paper+paper is blocked at deployment gate anyway.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ptday01_e06_strategy_signal_route_is_real_and_fail_closed() {
    // paper+alpaca — signal ingestion is wired (ExternalSignalIngestion).
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    arm(&st).await;
    // Gate 1b (PT-DAY-02): continuity must be Live to pass the WS gate.
    // Set Live so this test exercises the DB gate (gate 2), not the WS gate.
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:ord-e06:new:2026-01-01T09:30:00Z".to_string(),
        last_event_at: "2026-01-01T09:30:00Z".to_string(),
    })
    .await;
    // Gate 1c (PT-DAY-03): inject a regular-session timestamp so this test
    // exercises the DB gate (gate 2), not the session gate.
    // 1_704_726_000 = 2024-01-08 Mon 10:00 ET (regular NYSE session).
    st.set_session_clock_ts_for_test(1_704_726_000).await;

    let signal_body = serde_json::to_string(&serde_json::json!({
        "signal_id": "ptday01-e06-signal-001",
        "strategy_id": "spy_test_v1",
        "symbol": "SPY",
        "side": "buy",
        "qty": 10,
    }))
    .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(signal_body.clone()))
        .unwrap();
    let (code, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    // No DB wired → gate 2 (db_present) fires → 503 fail-closed.
    assert_eq!(
        code,
        StatusCode::SERVICE_UNAVAILABLE,
        "E06: /api/v1/strategy/signal without DB must return 503 (fail-closed at DB gate); \
         got body: {}",
        String::from_utf8_lossy(&body)
    );
    let body_json = parse_json(body);
    assert_eq!(
        body_json["disposition"], "unavailable",
        "E06: disposition must be 'unavailable' when DB is absent"
    );
    assert_eq!(
        body_json["accepted"], false,
        "E06: accepted must be false when DB is absent"
    );

    // paper+paper does NOT configure signal ingestion — gate 1 fires first.
    let st_paper = Arc::new(state::AppState::new());
    let req2 = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(signal_body))
        .unwrap();
    let (code2, body2) = call(routes::build_router(Arc::clone(&st_paper)), req2).await;
    assert_eq!(
        code2,
        StatusCode::SERVICE_UNAVAILABLE,
        "E06: paper+paper must return 503 at signal ingestion gate (not configured)"
    );
    let body2_json = parse_json(body2);
    assert_eq!(
        body2_json["disposition"], "unavailable",
        "E06: paper+paper disposition must be 'unavailable' (ingestion not configured)"
    );
}

// ---------------------------------------------------------------------------
// PT-DAY-02-E07 — GapDetected blocks strategy signals (fail-closed mid-session)
//
// Proves that when the Alpaca WS transport degrades to GapDetected mid-session,
// new strategy signals are refused with 503 / disposition="continuity_gap".
//
// This is the primary ordinary-fault case: the WS connection drops while a run
// is in progress.  Without this gate, signals would queue to the outbox and the
// orchestrator would dispatch orders with no way to receive fills — creating
// unmonitored positions.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ptday02_e07_gap_detected_blocks_strategy_signals() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    arm(&st).await;
    st.update_ws_continuity(state::AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:ord-1:fill:2026-01-01T10:00:00Z".to_string()),
        last_event_at: Some("2026-01-01T10:00:00Z".to_string()),
        detail: "E07: simulated mid-session WS disconnect".to_string(),
    })
    .await;

    let signal_body = serde_json::to_string(&serde_json::json!({
        "signal_id": "ptday02-e07-signal-001",
        "strategy_id": "spy_test_v1",
        "symbol": "SPY",
        "side": "buy",
        "qty": 5,
    }))
    .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(signal_body))
        .unwrap();
    let (code, body) = call(routes::build_router(Arc::clone(&st)), req).await;

    assert_eq!(
        code,
        StatusCode::SERVICE_UNAVAILABLE,
        "E07: GapDetected must block signal with 503; got: {code}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["disposition"], "continuity_gap",
        "E07: disposition must be 'continuity_gap' for GapDetected; got: {json}"
    );
    assert_eq!(
        json["accepted"], false,
        "E07: accepted must be false when WS gap is detected; got: {json}"
    );
    assert!(
        json["blockers"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("gap detected"),
        "E07: blocker must name the gap; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// PT-DAY-02-E08 — ColdStartUnproven blocks strategy signals (fail-closed boot)
//
// Proves that on daemon boot (before the WS transport completes its first
// connect→auth→subscribe handshake), strategy signals are refused with 503 /
// disposition="unavailable".
//
// This is the paper+alpaca boot case: continuity is ColdStartUnproven until the
// WS transport reports Live.  Accepting signals before continuity is proven
// would enqueue orders that the orchestrator might dispatch without a live fill
// feed, creating unmonitored positions from the very first signal.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ptday02_e08_cold_start_unproven_blocks_strategy_signals() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    // Do NOT set Live — default boot state for paper+alpaca is ColdStartUnproven.
    arm(&st).await;

    let signal_body = serde_json::to_string(&serde_json::json!({
        "signal_id": "ptday02-e08-signal-001",
        "strategy_id": "spy_test_v1",
        "symbol": "SPY",
        "side": "sell",
        "qty": 3,
    }))
    .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(signal_body))
        .unwrap();
    let (code, body) = call(routes::build_router(Arc::clone(&st)), req).await;

    assert_eq!(
        code,
        StatusCode::SERVICE_UNAVAILABLE,
        "E08: ColdStartUnproven must block signal with 503; got: {code}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["disposition"], "unavailable",
        "E08: disposition must be 'unavailable' for ColdStartUnproven; got: {json}"
    );
    assert_eq!(
        json["accepted"], false,
        "E08: accepted must be false when continuity is unproven; got: {json}"
    );
    assert!(
        json["blockers"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("cold start"),
        "E08: blocker must name cold start; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// PT-DAY-02-E09 — Live continuity passes gate 1b; signal reaches DB gate
//
// Proves that once the WS transport establishes Live continuity, the WS gate
// (gate 1b) passes and the signal falls through to the next gate (DB present).
//
// This is the positive case: gate 1b is not a permanent block — it is an
// honest degradation gate that lifts as soon as continuity is re-established.
// The signal reaching DB gate (503) proves gate 1b does not over-block.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ptday02_e09_live_continuity_passes_gate_signal_reaches_db_gate() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    arm(&st).await;
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:ord-e09:new:2026-01-01T09:31:00Z".to_string(),
        last_event_at: "2026-01-01T09:31:00Z".to_string(),
    })
    .await;
    assert!(
        st.alpaca_ws_continuity().await.is_continuity_proven(),
        "E09: continuity must be proven before sending signal"
    );
    // Gate 1c (PT-DAY-03): inject a regular-session timestamp so this test
    // exercises the DB gate (gate 2), not the session gate.
    // 1_704_726_000 = 2024-01-08 Mon 10:00 ET (regular NYSE session).
    st.set_session_clock_ts_for_test(1_704_726_000).await;

    let signal_body = serde_json::to_string(&serde_json::json!({
        "signal_id": "ptday02-e09-signal-001",
        "strategy_id": "spy_test_v1",
        "symbol": "SPY",
        "side": "buy",
        "qty": 10,
    }))
    .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(signal_body))
        .unwrap();
    let (code, body) = call(routes::build_router(Arc::clone(&st)), req).await;

    // Gate 1b passed (Live); gate 1c passed (regular session); gate 2 fires (no DB) → 503.
    assert_eq!(
        code,
        StatusCode::SERVICE_UNAVAILABLE,
        "E09: Live continuity + regular session must pass gates 1b+1c and reach DB gate (503); got: {code}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["disposition"], "unavailable",
        "E09: disposition must be 'unavailable' at DB gate; got: {json}"
    );
    assert!(
        json["blockers"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("DB"),
        "E09: blocker must be the DB gate, not the WS continuity gate; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// PT-DAY-03-E10 — NYSE session gate blocks signals outside regular session
//
// Reference timestamps (verified against CalendarSpec::NyseWeekdays unit tests):
//   Regular session:  1_704_726_000 = 2024-01-08 Mon 10:00 ET (09:30–16:00 window)
//   Closed (weekend): 1_704_510_000 = 2024-01-06 Sat 10:00 ET
//   Premarket:        1_704_722_400 = 2024-01-08 Mon 09:00 ET (before 09:30 ET)
//   After-hours:      1_704_751_200 = 2024-01-08 Mon 17:00 ET (after 16:00 ET)
//
// All E10 tests:
//   - paper+alpaca (ExternalSignalIngestion wired)
//   - WS continuity = Live (gate 1b passes)
//   - Inject clock via set_session_clock_ts_for_test (hermetic, no real wall-clock)
// ---------------------------------------------------------------------------

/// Shared setup for E10 tests: paper+alpaca, armed, Live continuity.
async fn e10_setup() -> Arc<state::AppState> {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    arm(&st).await;
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:ord-e10:new:2024-01-08T14:00:00Z".to_string(),
        last_event_at: "2024-01-08T14:00:00Z".to_string(),
    })
    .await;
    st
}

fn e10_signal_body(signal_id: &str) -> String {
    serde_json::to_string(&serde_json::json!({
        "signal_id": signal_id,
        "strategy_id": "spy_test_v1",
        "symbol": "SPY",
        "side": "buy",
        "qty": 5,
    }))
    .unwrap()
}

async fn post_signal(st: &Arc<state::AppState>, body: String) -> (StatusCode, bytes::Bytes) {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();
    call(routes::build_router(Arc::clone(st)), req).await
}

// E10a: closed session (Saturday) blocks signals.
#[tokio::test]
async fn ptday03_e10a_closed_session_blocks_strategy_signals() {
    let st = e10_setup().await;
    // 1_704_542_400 = 2024-01-06T12:00:00Z = Saturday 07:00 ET — weekend, exchange closed.
    // (The calendar.rs comment for 1_704_510_000 is misleading; that ts is actually Fri 22:00 ET.)
    st.set_session_clock_ts_for_test(1_704_542_400).await;

    let (code, body) = post_signal(&st, e10_signal_body("ptday03-e10a-001")).await;

    assert_eq!(
        code,
        StatusCode::CONFLICT,
        "E10a: closed session (weekend) must block signal with 409; got: {code}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["disposition"], "outside_session",
        "E10a: disposition must be 'outside_session' for closed NYSE session; got: {json}"
    );
    assert_eq!(
        json["accepted"], false,
        "E10a: accepted must be false when session is closed; got: {json}"
    );
    assert!(
        json["blockers"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("closed"),
        "E10a: blocker must name 'closed' session state; got: {json}"
    );
}

// E10b: premarket session blocks signals.
#[tokio::test]
async fn ptday03_e10b_premarket_session_blocks_strategy_signals() {
    let st = e10_setup().await;
    // 1_704_722_400 = 2024-01-08 Mon 09:00 ET — premarket (before 09:30 ET).
    st.set_session_clock_ts_for_test(1_704_722_400).await;

    let (code, body) = post_signal(&st, e10_signal_body("ptday03-e10b-001")).await;

    assert_eq!(
        code,
        StatusCode::CONFLICT,
        "E10b: premarket session must block signal with 409; got: {code}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["disposition"], "outside_session",
        "E10b: disposition must be 'outside_session' for premarket NYSE session; got: {json}"
    );
    assert!(
        json["blockers"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("premarket"),
        "E10b: blocker must name 'premarket' session state; got: {json}"
    );
}

// E10c: regular session passes gate 1c; signal reaches DB gate (503).
//
// This proves gate 1c does not over-block: regular session → pass → DB gate fires.
// Mirrors E09 but explicitly tests the session gate seam rather than the WS gate.
#[tokio::test]
async fn ptday03_e10c_regular_session_passes_gate_reaches_db_gate() {
    let st = e10_setup().await;
    // 1_704_726_000 = 2024-01-08 Mon 10:00 ET — regular NYSE session.
    st.set_session_clock_ts_for_test(1_704_726_000).await;

    let (code, body) = post_signal(&st, e10_signal_body("ptday03-e10c-001")).await;

    // Gate 1c passes (regular); gate 2 fires (no DB) → 503.
    assert_eq!(
        code,
        StatusCode::SERVICE_UNAVAILABLE,
        "E10c: regular session must pass session gate and reach DB gate (503); got: {code}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["disposition"], "unavailable",
        "E10c: disposition must be 'unavailable' at DB gate; got: {json}"
    );
    assert!(
        json["blockers"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("DB"),
        "E10c: blocker must be the DB gate, not the session gate; got: {json}"
    );
}

// E10d: after-hours session blocks signals.
#[tokio::test]
async fn ptday03_e10d_after_hours_session_blocks_strategy_signals() {
    let st = e10_setup().await;
    // 1_704_751_200 = 2024-01-08 Mon 17:00 ET — after-hours (after 16:00 ET close).
    st.set_session_clock_ts_for_test(1_704_751_200).await;

    let (code, body) = post_signal(&st, e10_signal_body("ptday03-e10d-001")).await;

    assert_eq!(
        code,
        StatusCode::CONFLICT,
        "E10d: after-hours session must block signal with 409; got: {code}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["disposition"], "outside_session",
        "E10d: disposition must be 'outside_session' for after-hours NYSE session; got: {json}"
    );
    assert!(
        json["blockers"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("after_hours"),
        "E10d: blocker must name 'after_hours' session state; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// PT-DAY-04-E11 — Gap escalation deduplication and lifecycle
//
// Proves that the gap-escalation flag behaves correctly:
//   E11a: First GapDetected signal refusal claims the escalation (flag set to true).
//   E11b: Second refusal does NOT re-claim (dedup — flag stays true, no double-notify).
//   E11c: Live transition resets the flag (ready for next gap window).
//   E11d: ColdStartUnproven refusal does NOT claim the escalation (boot state is
//         not an actionable mid-session fault).
//
// These tests do not require a live Discord webhook — the notifier is a no-op
// when DISCORD_WEBHOOK_URL is unset (which is always the case in CI).  What
// is proven here is the deduplication semantics on the real production path:
// - try_claim_gap_escalation() on AppState
// - called from the real strategy_signal gate 1b GapDetected arm
// - reset by the real update_ws_continuity(Live) path
//
// The Discord delivery itself is a best-effort side-effect; the flag state is
// the authoritative proof that the right escalation decision was made.
// ---------------------------------------------------------------------------

// E11a: first GapDetected signal refusal claims the escalation.
#[tokio::test]
async fn ptday04_e11a_first_gap_refusal_claims_escalation() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    arm(&st).await;
    st.update_ws_continuity(state::AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:ord-1:fill:2024-01-08T14:30:00Z".to_string()),
        last_event_at: Some("2024-01-08T14:30:00Z".to_string()),
        detail: "E11a: simulated mid-session WS disconnect".to_string(),
    })
    .await;

    // DIS-01: update_ws_continuity(GapDetected) now claims the escalation at the
    // WS transport level (early operator notice), not waiting for the first signal
    // POST.  The flag is therefore already true after calling update_ws_continuity.
    assert!(
        st.gap_escalation_is_pending(),
        "E11a: escalation flag must be true immediately after update_ws_continuity(GapDetected) \
         (claimed by transport path, DIS-01)"
    );

    let (code, body) = post_signal(&st, e10_signal_body("ptday04-e11a-001")).await;

    // Gate 1b fires → 503 / continuity_gap.
    assert_eq!(
        code,
        StatusCode::SERVICE_UNAVAILABLE,
        "E11a: GapDetected must block signal with 503; got: {code}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["disposition"], "continuity_gap",
        "E11a: disposition must be continuity_gap; got: {json}"
    );

    // After first signal refusal: flag remains true (already claimed by transport;
    // signal POST does not re-claim it — dedup holds across both paths).
    assert!(
        st.gap_escalation_is_pending(),
        "E11a: escalation flag must remain true after signal refusal (dedup — already claimed)"
    );
}

// E11b: second GapDetected refusal does not re-claim the escalation (dedup).
#[tokio::test]
async fn ptday04_e11b_second_gap_refusal_does_not_reclain_escalation() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    arm(&st).await;
    st.update_ws_continuity(state::AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:ord-2:fill:2024-01-08T14:31:00Z".to_string()),
        last_event_at: Some("2024-01-08T14:31:00Z".to_string()),
        detail: "E11b: simulated mid-session WS disconnect".to_string(),
    })
    .await;

    // First refusal — claims escalation.
    let _ = post_signal(&st, e10_signal_body("ptday04-e11b-001")).await;
    assert!(
        st.gap_escalation_is_pending(),
        "E11b: flag must be true after first refusal"
    );

    // Second refusal — flag must still be true (not toggled or reset).
    let (code2, body2) = post_signal(&st, e10_signal_body("ptday04-e11b-002")).await;
    assert_eq!(
        code2,
        StatusCode::SERVICE_UNAVAILABLE,
        "E11b: second GapDetected refusal must still return 503; got: {code2}"
    );
    let json2 = parse_json(body2);
    assert_eq!(
        json2["disposition"], "continuity_gap",
        "E11b: second refusal disposition must still be continuity_gap; got: {json2}"
    );
    assert!(
        st.gap_escalation_is_pending(),
        "E11b: escalation flag must remain true after second refusal (dedup — no re-notification)"
    );
}

// E11c: Live transition resets the escalation flag for the next gap window.
#[tokio::test]
async fn ptday04_e11c_live_transition_resets_escalation_flag() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    arm(&st).await;
    st.update_ws_continuity(state::AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:ord-3:fill:2024-01-08T14:32:00Z".to_string()),
        last_event_at: Some("2024-01-08T14:32:00Z".to_string()),
        detail: "E11c: simulated gap".to_string(),
    })
    .await;

    // Claim the escalation via a signal refusal.
    let _ = post_signal(&st, e10_signal_body("ptday04-e11c-001")).await;
    assert!(
        st.gap_escalation_is_pending(),
        "E11c: flag must be true after gap refusal"
    );

    // Re-establish Live — flag must reset.
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:ord-3:fill:2024-01-08T14:35:00Z".to_string(),
        last_event_at: "2024-01-08T14:35:00Z".to_string(),
    })
    .await;
    assert!(
        !st.gap_escalation_is_pending(),
        "E11c: escalation flag must be false after Live transition (ready for next gap window)"
    );
}

// E11d: ColdStartUnproven refusal does NOT claim the escalation.
//
// ColdStartUnproven is a boot-time state — expected, not an actionable
// mid-session fault.  The escalation path is reserved for GapDetected only.
#[tokio::test]
async fn ptday04_e11d_cold_start_refusal_does_not_claim_escalation() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    arm(&st).await;
    // Default boot state for paper+alpaca is ColdStartUnproven — no override needed.
    assert!(
        !st.gap_escalation_is_pending(),
        "E11d: escalation flag must be false before any signal"
    );

    let (code, body) = post_signal(&st, e10_signal_body("ptday04-e11d-001")).await;

    assert_eq!(
        code,
        StatusCode::SERVICE_UNAVAILABLE,
        "E11d: ColdStartUnproven must block signal with 503; got: {code}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["disposition"], "unavailable",
        "E11d: ColdStartUnproven disposition is 'unavailable', not 'continuity_gap'; got: {json}"
    );

    // Escalation must NOT have been claimed — ColdStartUnproven is not a mid-session fault.
    assert!(
        !st.gap_escalation_is_pending(),
        "E11d: ColdStartUnproven refusal must NOT claim gap escalation; got pending=true"
    );
}

// ---------------------------------------------------------------------------
// PT-DAY-05-E12 — Phase 1 happy-path consolidation
//
// Proves that when ALL Phase 1 gate conditions are satisfied simultaneously
// on the real production strategy_signal path, the signal passes through
// gates 1, 1b, and 1c and reaches the first downstream gate (DB present).
//
// This is the canonical "paper+alpaca is operational" proof.  It is not a
// trivial pass-through check — it exercises the complete gate sequence as a
// single narrative and verifies the system does not over-block when healthy.
//
// Conditions satisfied:
//   Gate 1:  ExternalSignalIngestion wired (paper+alpaca)
//   Gate 1b: WS continuity is Live
//   Gate 1c: NYSE session is "regular" (injected: Mon 10:00 ET)
//   No DB:   reaches gate 2 → 503/unavailable (expected in test environment)
//   No escalation: gap_escalation_is_pending() must be false (no false alarm)
//
// Phase 2 gap: the DB gate (and all downstream gates 3–7) require a live DB
// and a running orchestrator.  Those are proven in the DB-backed testkit
// suite (scenario_fill_quality_orchestrator_tv_exec01b.rs and related).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ptday05_e12_phase1_happy_path_all_gates_satisfied_reaches_db_gate() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    // Gate 1: paper+alpaca → ExternalSignalIngestion configured.
    // (Implicit in the constructor — verified by E05.)

    // Gate 1b: Live continuity → WS gate passes.
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:ord-e12:new:2024-01-08T14:00:00Z".to_string(),
        last_event_at: "2024-01-08T14:00:00Z".to_string(),
    })
    .await;

    // Gate 1c: regular NYSE session (2024-01-08 Mon 10:00 ET) → session gate passes.
    st.set_session_clock_ts_for_test(1_704_726_000).await;

    // Arm so control-plane state is consistent (not required by signal gates
    // 1/1b/1c — those are pre-lifecycle-guard — but represents realistic state).
    arm(&st).await;

    // Verify no false escalation state from prior test leakage.
    assert!(
        !st.gap_escalation_is_pending(),
        "E12: escalation flag must be false before the happy-path signal"
    );

    let signal_body = serde_json::to_string(&serde_json::json!({
        "signal_id": "ptday05-e12-phase1-001",
        "strategy_id": "spy_momentum_v1",
        "symbol": "SPY",
        "side": "buy",
        "qty": 10,
    }))
    .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(signal_body))
        .unwrap();
    let (code, body) = call(routes::build_router(Arc::clone(&st)), req).await;

    // Gates 1, 1b, 1c all pass → gate 2 (DB absent) fires → 503/unavailable.
    // This is the correct response in a test environment without a live DB.
    assert_eq!(
        code,
        StatusCode::SERVICE_UNAVAILABLE,
        "E12: happy-path signal must pass gates 1/1b/1c and reach DB gate (503); got: {code}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["disposition"], "unavailable",
        "E12: disposition must be 'unavailable' at DB gate (not a signal-gate refusal); got: {json}"
    );
    assert!(
        json["blockers"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("DB"),
        "E12: blocker must name the DB gate — all three signal gates must have passed; got: {json}"
    );

    // No gap escalation on the happy path.
    assert!(
        !st.gap_escalation_is_pending(),
        "E12: gap escalation must NOT be pending on the happy path (Live continuity, no gap)"
    );
}

// ---------------------------------------------------------------------------
// PT-AUTO-01-E13 — execution loop self-halt predicate proofs
//
// Proves the decision predicate `ws_continuity_gap_requires_halt()` that the
// execution loop evaluates each tick before calling `orchestrator.tick()`.
//
// The predicate is the policy authority: when it returns `true` the loop
// self-halts, disarms integrity, and releases runtime leadership without
// dispatching any further orders.
//
// Tests are pure in-process — no orchestrator or DB setup required.
// The production wiring lives in `state/loop_runner.rs` (PT-AUTO-01).
// ---------------------------------------------------------------------------

// E13a: paper+alpaca + GapDetected → halt predicate returns true.
//
// This is the only state that triggers the self-halt: fill tracking is broken
// and the daemon must not continue dispatching orders.
#[tokio::test]
async fn ptauto01_e13a_gap_detected_requires_halt() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    st.update_ws_continuity(state::AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:ord-e13a:fill:2024-01-08T14:00:00Z".to_string()),
        last_event_at: Some("2024-01-08T14:00:00Z".to_string()),
        detail: "E13a: simulated mid-session gap".to_string(),
    })
    .await;

    assert!(
        st.ws_continuity_gap_requires_halt().await,
        "E13a: paper+alpaca + GapDetected must return true from \
         ws_continuity_gap_requires_halt() — execution loop must self-halt"
    );
}

// E13b: paper+alpaca + Live → halt predicate returns false.
//
// Normal operation: WS continuity is confirmed.  The loop must not halt.
#[tokio::test]
async fn ptauto01_e13b_live_continuity_does_not_require_halt() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:ord-e13b:new:2024-01-08T14:01:00Z".to_string(),
        last_event_at: "2024-01-08T14:01:00Z".to_string(),
    })
    .await;

    assert!(
        !st.ws_continuity_gap_requires_halt().await,
        "E13b: paper+alpaca + Live must return false from \
         ws_continuity_gap_requires_halt() — normal operation, loop must not halt"
    );
}

// E13c: paper+alpaca + ColdStartUnproven → halt predicate returns false.
//
// ColdStartUnproven is the boot-time state before the WS session is
// established.  Signal ingestion is blocked at the route layer (PT-DAY-02)
// so the execution loop is not yet running in this state.  The self-halt
// predicate must not trigger on cold start.
#[tokio::test]
async fn ptauto01_e13c_cold_start_does_not_require_halt() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    // Default boot state for paper+alpaca is ColdStartUnproven — no override needed.

    assert!(
        !st.ws_continuity_gap_requires_halt().await,
        "E13c: paper+alpaca + ColdStartUnproven must return false from \
         ws_continuity_gap_requires_halt() — self-halt is reserved for GapDetected only"
    );
}

// E13d: paper+paper (NotConfigured strategy) → halt predicate returns false.
//
// paper+paper is not on the ExternalSignalIngestion path.  WS continuity
// self-halt must not apply — it is specific to the broker-backed paper path.
#[tokio::test]
async fn ptauto01_e13d_paper_paper_not_on_ws_path_does_not_require_halt() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Paper,
    ));
    // Force GapDetected to confirm the predicate still returns false when
    // strategy_market_data_source is NotConfigured, regardless of continuity state.
    // (paper+paper continuity is NotApplicable, but this tests the policy guard.)
    // update_ws_continuity no-ops when current state is NotApplicable, so we
    // verify the predicate directly on the NotApplicable / NotConfigured state.

    assert!(
        !st.ws_continuity_gap_requires_halt().await,
        "E13d: paper+paper (NotConfigured strategy) must return false from \
         ws_continuity_gap_requires_halt() — WS self-halt is only for paper+alpaca"
    );
}

// ---------------------------------------------------------------------------
// PT-AUTO-01B-E14 — real execution loop self-halt proof
//
// These tests exercise the REAL production execution loop path changed by
// PT-AUTO-01A (`spawn_execution_loop` in `state/loop_runner.rs`).  The seam
// `AppState::run_loop_one_tick_for_test` constructs a minimal DaemonOrchestrator
// (Paper broker, lazy/disconnected pool) and spawns the real loop.  The loop
// exits within one tick interval (~1 second) and the exit note + integrity
// state are observable on the same AppState.
//
// E13a–d proved the PREDICATE.  E14a–b prove the LOOP EFFECTS.
//
// AppState constructed without DB (new_for_test_with_mode_and_broker has
// db=None), so the deadman block in the loop is skipped.  PT-AUTO-01 fires
// clean on the first tick for GapDetected.  For Live continuity, the loop
// falls through to orchestrator.tick() which fails at Phase-0 DB check
// (lazy/disconnected pool) — a different, non-PT-AUTO-01 exit reason.
//
// Fixed run_id: avoids Uuid::new_v4() in test code (per D1 guard policy).
// ---------------------------------------------------------------------------

const E14_RUN_ID_STR: &str = "e14a1b2c-3d4e-5f60-a7b8-c9d0e1f2a3b4";

// E14a: GapDetected causes the real execution loop to self-halt.
//
// Proves:
// - The real spawn_execution_loop code path exits with the PT-AUTO-01 note.
// - integrity.disarmed is set to true by the halt path.
// - integrity.halted is set to true by the halt path.
// - Both effects are observable on the AppState after the loop exits.
#[tokio::test]
async fn ptauto01b_e14a_gap_detected_halts_real_execution_loop() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    // Induce GapDetected so PT-AUTO-01 fires on the first tick.
    st.update_ws_continuity(state::AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:e14a:fill:2024-01-08T14:00:00Z".to_string()),
        last_event_at: Some("2024-01-08T14:00:00Z".to_string()),
        detail: "E14a: simulated mid-session gap for loop halt proof".to_string(),
    })
    .await;

    // Clear integrity flags so we can observe them being set by the halt path.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    let run_id = uuid::Uuid::parse_str(E14_RUN_ID_STR).expect("E14 fixed run_id must parse");
    let exit_note = st.run_loop_one_tick_for_test(run_id).await;

    // The loop must exit with the PT-AUTO-01 note (not tick error, not deadman).
    assert_eq!(
        exit_note.as_deref(),
        Some("execution loop halted: Alpaca WS continuity gap detected"),
        "E14a: real loop must exit with PT-AUTO-01 halt note on GapDetected; \
         got: {exit_note:?}"
    );

    // Integrity must be disarmed AND halted — both are set by the PT-AUTO-01 path.
    // (Tick-error path sets only halted; deadman sets both but is skipped here.)
    let ig = st.integrity.read().await;
    assert!(
        ig.disarmed,
        "E14a: integrity must be disarmed after PT-AUTO-01 loop halt"
    );
    assert!(
        ig.halted,
        "E14a: integrity must be halted after PT-AUTO-01 loop halt"
    );
}

// E14b: Live continuity does NOT trigger the PT-AUTO-01 self-halt.
//
// Proves that the halt predicate guard in the loop is correctly conditional:
// when continuity is Live, the loop does NOT exit via PT-AUTO-01.  It falls
// through to orchestrator.tick() which fails at the Phase-0 DB check (the
// seam uses a lazy/disconnected pool) — a different exit reason.
//
// This prevents a regression where PT-AUTO-01 might fire on all paths.
#[tokio::test]
async fn ptauto01b_e14b_live_continuity_does_not_trigger_pt_auto01_halt() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    // Live continuity — PT-AUTO-01 must NOT fire.
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:e14b:new:2024-01-08T14:01:00Z".to_string(),
        last_event_at: "2024-01-08T14:01:00Z".to_string(),
    })
    .await;

    let run_id = uuid::Uuid::parse_str(E14_RUN_ID_STR).expect("E14 fixed run_id must parse");
    let exit_note = st.run_loop_one_tick_for_test(run_id).await;

    // The exit note must NOT be the PT-AUTO-01 message — PT-AUTO-01 did not fire.
    // (The loop exits via tick error when the lazy pool fails Phase-0 DB check.)
    assert_ne!(
        exit_note.as_deref(),
        Some("execution loop halted: Alpaca WS continuity gap detected"),
        "E14b: Live continuity must NOT trigger PT-AUTO-01 self-halt; \
         got: {exit_note:?}"
    );
    // The loop does exit — it is not stuck — it just exits for a different reason.
    assert!(
        exit_note.is_some(),
        "E14b: loop must exit (either tick error or stop); got None"
    );
}

// ---------------------------------------------------------------------------
// PT-AUTO-02 — Per-run autonomous signal intake bound
//
// E15a: counter at 0, Live continuity → Gate 1d passes → signal reaches DB gate
//       (proves the gate does not fire when count < MAX).
//
// E15b: counter at MAX → Gate 1d fires → 409/day_limit_reached
//       (proves the gate trips and refuses signals when the bound is reached).
//
// Both tests are pure in-process.  Gate 1d fires before lifecycle_guard() /
// Gate 2 (DB), so no database is required.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ptauto02_e15a_under_limit_passes_gate_1d_reaches_db_gate() {
    // paper+alpaca, Live continuity, NYSE regular-session clock, count = 0.
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    // Set Live continuity so Gate 1b passes.
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "test-msg-e15a".to_string(),
        last_event_at: "2026-01-02T10:00:00Z".to_string(),
    })
    .await;

    // Inject a NYSE regular-session timestamp (Friday 2026-01-02 14:00:00 UTC = 09:00 ET
    // would be premarket; use 15:00 UTC = 10:00 ET which is regular session).
    // 2026-01-02 is a Friday; 15:00 UTC = 10:00 ET → regular session.
    let regular_ts = chrono::DateTime::parse_from_rfc3339("2026-01-02T15:00:00Z")
        .unwrap()
        .timestamp();
    st.set_session_clock_ts_for_test(regular_ts).await;

    // count = 0 (default) → Gate 1d should pass.
    assert_eq!(
        st.day_signal_count(),
        0,
        "E15a: day_signal_count must be 0 at test start"
    );

    let signal_body = serde_json::json!({
        "signal_id": "e15a-sig-0001",
        "strategy_id": "test-strat",
        "symbol": "AAPL",
        "side": "buy",
        "qty": 1,
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(signal_body.to_string()))
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    let json = parse_json(body);

    // Gate 1d passes → signal falls through to Gate 2 (DB) → 503/unavailable.
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "E15a: under-limit signal must reach DB gate (503); got: {status} body: {json}"
    );
    assert_eq!(
        json["disposition"], "unavailable",
        "E15a: disposition must be 'unavailable' (DB gate); got: {json}"
    );
    // Confirm the refusal was NOT day_limit_reached.
    assert_ne!(
        json["disposition"], "day_limit_reached",
        "E15a: Gate 1d must NOT fire when count is under limit; got: {json}"
    );
}

#[tokio::test]
async fn ptauto02_e15b_at_limit_gate_1d_fires_409_day_limit_reached() {
    // paper+alpaca, Live continuity, NYSE regular-session clock, count = MAX.
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    // Set Live continuity so Gate 1b passes.
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "test-msg-e15b".to_string(),
        last_event_at: "2026-01-02T10:00:00Z".to_string(),
    })
    .await;

    // NYSE regular session.
    let regular_ts = chrono::DateTime::parse_from_rfc3339("2026-01-02T15:00:00Z")
        .unwrap()
        .timestamp();
    st.set_session_clock_ts_for_test(regular_ts).await;

    // Saturate the counter to exactly MAX_AUTONOMOUS_SIGNALS_PER_RUN (100).
    st.set_day_signal_count_for_test(100);
    assert!(
        st.day_signal_limit_exceeded(),
        "E15b: day_signal_limit_exceeded() must be true at count=100"
    );

    let signal_body = serde_json::json!({
        "signal_id": "e15b-sig-0001",
        "strategy_id": "test-strat",
        "symbol": "AAPL",
        "side": "buy",
        "qty": 1,
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(signal_body.to_string()))
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "E15b: at-limit signal must be refused with 409; got: {status} body: {json}"
    );
    assert_eq!(
        json["disposition"], "day_limit_reached",
        "E15b: disposition must be 'day_limit_reached'; got: {json}"
    );
    assert_eq!(
        json["accepted"], false,
        "E15b: accepted must be false for day_limit_reached refusal; got: {json}"
    );
    // Counter must not increment on refusal.
    assert_eq!(
        st.day_signal_count(),
        100,
        "E15b: day_signal_count must remain 100 after Gate 1d refusal"
    );
}

// ---------------------------------------------------------------------------
// PT-AUTO-03 — Autonomous signal intake visibility on GET /api/v1/system/status
//
// E16a: paper+alpaca + count=0 → status surfaces autonomous_signal_count=0,
//       autonomous_signal_limit_hit=false  (healthy, Gate 1d not tripping).
//
// E16b: paper+alpaca + count=MAX → status surfaces autonomous_signal_count=100,
//       autonomous_signal_limit_hit=true  (blocked, Gate 1d currently tripping).
//
// E16c: paper+paper (NotConfigured) → both fields are null (not applicable).
//
// All three are pure in-process.  Each hits the real production system_status
// handler via routes::build_router + tower::ServiceExt::oneshot.
// ---------------------------------------------------------------------------

fn get_system_status_req() -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap()
}

#[tokio::test]
async fn ptauto03_e16a_system_status_surfaces_signal_count_healthy() {
    // paper+alpaca, fresh state: count = 0, limit not hit.
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    assert_eq!(st.day_signal_count(), 0, "E16a: count must be 0 at boot");

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        get_system_status_req(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "E16a: /api/v1/system/status must return 200"
    );
    let json = parse_json(body);

    assert_eq!(
        json["autonomous_signal_count"], 0,
        "E16a: system/status must surface autonomous_signal_count=0 for fresh paper+alpaca; got: {json}"
    );
    assert_eq!(
        json["autonomous_signal_limit_hit"], false,
        "E16a: system/status must surface autonomous_signal_limit_hit=false when count=0; got: {json}"
    );
}

#[tokio::test]
async fn ptauto03_e16b_system_status_surfaces_signal_count_at_limit() {
    // paper+alpaca, count saturated to MAX (100): limit hit.
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    st.set_day_signal_count_for_test(100);
    assert!(
        st.day_signal_limit_exceeded(),
        "E16b: day_signal_limit_exceeded() must be true at count=100"
    );

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        get_system_status_req(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "E16b: /api/v1/system/status must return 200"
    );
    let json = parse_json(body);

    assert_eq!(
        json["autonomous_signal_count"], 100,
        "E16b: system/status must surface autonomous_signal_count=100 when saturated; got: {json}"
    );
    assert_eq!(
        json["autonomous_signal_limit_hit"], true,
        "E16b: system/status must surface autonomous_signal_limit_hit=true at count=MAX; got: {json}"
    );
}

#[tokio::test]
async fn ptauto03_e16c_system_status_autonomous_fields_null_for_non_external_ingestion() {
    // paper+paper (default): ExternalSignalIngestion is NOT configured.
    // Both autonomous fields must be null (not applicable, not cosmetically zeroed).
    let st = Arc::new(state::AppState::new());

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        get_system_status_req(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "E16c: /api/v1/system/status must return 200"
    );
    let json = parse_json(body);

    assert!(
        json["autonomous_signal_count"].is_null(),
        "E16c: autonomous_signal_count must be null for paper+paper (not applicable); got: {json}"
    );
    assert!(
        json["autonomous_signal_limit_hit"].is_null(),
        "E16c: autonomous_signal_limit_hit must be null for paper+paper (not applicable); got: {json}"
    );
}

// ---------------------------------------------------------------------------
// PT-AUTO-04-E17 — Consolidated Phase 2 autonomous healthy path
//
// This is the canonical proof that the three Phase 2 autonomous protections
// operate correctly *together* on the healthy paper+alpaca path — none fire
// spuriously when all conditions are satisfied.
//
// The proof is not a Phase 1 happy-path repeat.  E12 (Phase 1) exercises
// Gates 1/1b/1c only and does not verify Gate 1d or the status surface.
// E17 closes that gap by exercising all four autonomous controls simultaneously
// in a single coherent narrative:
//
//   Signal-route dimension (Gates 1 / 1b / 1c / 1d all pass):
//     - Gate 1:  ExternalSignalIngestion wired (paper+alpaca)
//     - Gate 1b: WS continuity is Live  (no spurious continuity block)
//     - Gate 1c: NYSE session is regular (no spurious session block)
//     - Gate 1d: count=5 < MAX=100      (no spurious day-limit block)
//     → signal reaches Gate 2 (DB absent) → 503/unavailable
//       i.e. all autonomous protections passed without false positive
//
//   Status-surface dimension (system/status shows healthy autonomous state):
//     - autonomous_signal_count == 5     (correct non-zero in-flight count)
//     - autonomous_signal_limit_hit == false (Gate 1d not tripping)
//
//   Self-halt dimension (healthy path does NOT self-halt):
//     - ws_continuity_gap_requires_halt() == false on Live path
//       (predicate verified inline; real loop self-halt proven by E14a)
//
// Both the signal-route and the status surface are exercised via
// routes::build_router + tower::ServiceExt::oneshot — real production code.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ptauto04_e17_phase2_consolidated_healthy_path_all_autonomous_controls_pass() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    // ── Gate 1b: Live WS continuity ──────────────────────────────────────────
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:ord-e17:new:2026-01-05T15:00:00Z".to_string(),
        last_event_at: "2026-01-05T15:00:00Z".to_string(),
    })
    .await;

    // ── Gate 1c: NYSE regular session (2026-01-05 Mon 15:00 UTC = 10:00 ET) ──
    let regular_ts = chrono::DateTime::parse_from_rfc3339("2026-01-05T15:00:00Z")
        .unwrap()
        .timestamp();
    st.set_session_clock_ts_for_test(regular_ts).await;

    // ── Gate 1d: count=5, well under MAX=100 ─────────────────────────────────
    st.set_day_signal_count_for_test(5);
    assert!(
        !st.day_signal_limit_exceeded(),
        "E17: day_signal_limit_exceeded() must be false at count=5"
    );

    // ── Self-halt predicate: Live → no autonomous loop halt ──────────────────
    // This assertion proves the self-halt gate (PT-AUTO-01) does not fire
    // spuriously on the healthy path.  The real loop self-halt effects on
    // GapDetected are covered by E14a.
    assert!(
        !st.ws_continuity_gap_requires_halt().await,
        "E17: ws_continuity_gap_requires_halt() must be false on Live continuity; \
         no spurious autonomous self-halt on healthy path"
    );

    // ── Signal route: send signal through real production router ─────────────
    let signal_body = serde_json::json!({
        "signal_id": "ptauto04-e17-consolidated-001",
        "strategy_id": "spy_momentum_v1",
        "symbol": "SPY",
        "side": "buy",
        "qty": 5,
    });
    let signal_req = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(signal_body.to_string()))
        .unwrap();
    let (signal_status, signal_body_bytes) =
        call(routes::build_router(Arc::clone(&st)), signal_req).await;
    let signal_json = parse_json(signal_body_bytes);

    // All four signal gates pass (1/1b/1c/1d) → falls through to Gate 2 (DB).
    // 503/unavailable is the correct response in a test environment without DB.
    assert_eq!(
        signal_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "E17: signal must pass all 4 autonomous gates and reach DB gate (503); \
         got: {signal_status} — if 409, an autonomous gate fired spuriously; \
         body: {signal_json}"
    );
    assert_eq!(
        signal_json["disposition"], "unavailable",
        "E17: disposition must be 'unavailable' (DB gate, not an autonomous refusal); \
         got: {signal_json}"
    );
    // Confirm the refusal is at the DB gate, not any autonomous protection.
    assert_ne!(
        signal_json["disposition"], "day_limit_reached",
        "E17: Gate 1d must NOT fire spuriously at count=5; got: {signal_json}"
    );
    assert_ne!(
        signal_json["disposition"], "continuity_gap",
        "E17: Gate 1b must NOT fire on Live continuity; got: {signal_json}"
    );
    assert_ne!(
        signal_json["disposition"], "outside_session",
        "E17: Gate 1c must NOT fire in regular session; got: {signal_json}"
    );

    // ── Status surface: verify autonomous state correctly reflects healthy run ─
    let (status_code, status_body_bytes) = call(
        routes::build_router(Arc::clone(&st)),
        get_system_status_req(),
    )
    .await;
    assert_eq!(
        status_code,
        StatusCode::OK,
        "E17: /api/v1/system/status must return 200"
    );
    let status_json = parse_json(status_body_bytes);

    assert_eq!(
        status_json["autonomous_signal_count"], 5,
        "E17: system/status must surface autonomous_signal_count=5 (non-zero, under limit); \
         got: {status_json}"
    );
    assert_eq!(
        status_json["autonomous_signal_limit_hit"], false,
        "E17: system/status must surface autonomous_signal_limit_hit=false on healthy path; \
         got: {status_json}"
    );
    assert_eq!(
        status_json["alpaca_ws_continuity"], "live",
        "E17: system/status must surface alpaca_ws_continuity='live' on healthy path; \
         got: {status_json}"
    );
}
