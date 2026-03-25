//! LO-02B: Alpaca/live-shadow stressed recovery proof matrix.
//!
//! Proves safe, truthful recovery behavior across broker-connected
//! restart/reopen/reload conditions for LiveShadow+Alpaca operation.
//! Also covers the LiveCapital WS continuity gate (moved to pre-DB position
//! in the same patch that introduced this test file).
//!
//! # Test matrix
//!
//! | ID    | Mode            | Cursor / state           | Proof                                                      |
//! |-------|-----------------|--------------------------|------------------------------------------------------------|
//! | SH-01 | LiveShadow+Alp  | Live cursor in DB        | seed_ws_continuity_from_db → ColdStartUnproven (demotion)  |
//! | SH-02 | LiveShadow+Alp  | GapDetected cursor in DB | seed_ws_continuity_from_db → GapDetected preserved (f-c)   |
//! | SH-03 | LiveShadow+Alp  | Fresh boot (no DB)       | status surfaces "cold_start_unproven" honestly             |
//! | SH-04 | LiveShadow+Alp  | Injected GapDetected     | status surfaces "gap_detected" honestly                    |
//! | SH-05 | LiveShadow+Alp  | Injected GapDetected     | start falls through to DB gate (503) — no WS gate          |
//! | SH-06 | LiveCapital+Alp | Fresh boot (no DB)       | start refused at WS continuity gate (403) before DB        |
//!
//! # Gap filled vs existing coverage
//!
//! BRK-07R D03/D04 (cursor durability) use `DeploymentMode::Paper` only.
//! SH-01/SH-02 extend that coverage to `DeploymentMode::LiveShadow`:
//! the cursor seeding behavior for the broker-establishment mode was previously
//! unproven.  SH-05 proves the intentional LiveShadow design contract (no WS
//! gate blocks the repair/establishment path).  SH-06 proves the LiveCapital
//! gate fires correctly at its new position.
//!
//! # DB-backed tests
//!
//! SH-01 and SH-02 require MQK_DATABASE_URL; they skip gracefully without it.
//! Run them with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_shadow_recovery_lo02b
//!
//! SH-03 through SH-06 are pure in-process (no DB required); they run unconditionally.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_broker_alpaca::types::AlpacaFetchCursor;
use mqk_daemon::routes;
use mqk_daemon::state::{
    AlpacaWsContinuityState, AppState, BrokerKind, DeploymentMode, OperatorAuthMode,
};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn db_pool_or_skip() -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(v) => v,
        Err(_) => return None,
    };
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("LO-02B: failed to connect to MQK_DATABASE_URL"),
    )
}

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn arm_via_http(st: &Arc<AppState>) {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(routes::build_router(Arc::clone(st)), req).await;
    assert_eq!(status, StatusCode::OK, "arm_via_http: arm must succeed");
}

async fn get_status(st: &Arc<AppState>) -> serde_json::Value {
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, json) = call(routes::build_router(Arc::clone(st)), req).await;
    json
}

// ---------------------------------------------------------------------------
// SH-01: LiveShadow+Alpaca + Live cursor in DB → ColdStartUnproven at boot
// ---------------------------------------------------------------------------

/// LO-02B / SH-01: Live cursor demoted to ColdStartUnproven on LiveShadow restart.
///
/// A daemon restart after a prior live-shadow session will have a Live cursor
/// in the DB.  `seed_ws_continuity_from_db` must demote it to ColdStartUnproven
/// — the WS connection has not yet been re-established after restart, so
/// continuity is not proven.
///
/// Proves the LiveShadow mode behaves identically to Paper+Alpaca (BRK-07R D04)
/// for cursor demotion: neither mode fabricates broker connectivity on restart.
///
/// DB-backed; skips gracefully without MQK_DATABASE_URL.
#[tokio::test]
async fn lo02b_sh01_live_shadow_live_cursor_demoted_at_restart() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("SH-01: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    let adapter_id = "lo02b-sh01-test";

    let live_cursor = AlpacaFetchCursor::live(
        None,
        "alpaca:order-lo02b-sh01:filled:2026-01-10T10:00:00Z",
        "2026-01-10T10:00:00Z",
    );
    let cursor_json = serde_json::to_string(&live_cursor).expect("SH-01: serialize cursor");
    mqk_db::advance_broker_cursor(&pool, adapter_id, &cursor_json, Utc::now())
        .await
        .expect("SH-01: advance_broker_cursor failed");

    // Simulate restart: fresh LiveShadow+Alpaca AppState with the same adapter_id.
    let mut state_inner = AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::LiveShadow,
        BrokerKind::Alpaca,
    );
    state_inner.set_adapter_id_for_test(adapter_id);
    let st = Arc::new(state_inner);

    // Before seeding: must be ColdStartUnproven (fresh boot state).
    assert!(
        matches!(
            st.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::ColdStartUnproven
        ),
        "SH-01: before seed, fresh LiveShadow state must be ColdStartUnproven"
    );

    st.seed_ws_continuity_from_db().await;

    // Live cursor must be demoted — must NOT be preserved as "live" across a restart.
    let cont = st.alpaca_ws_continuity().await;
    assert!(
        matches!(cont, AlpacaWsContinuityState::ColdStartUnproven),
        "SH-01: Live cursor must be demoted to ColdStartUnproven at LiveShadow restart \
         (WS not yet reconnected after restart); got: {cont:?}"
    );
}

// ---------------------------------------------------------------------------
// SH-02: LiveShadow+Alpaca + GapDetected cursor in DB → GapDetected preserved
// ---------------------------------------------------------------------------

/// LO-02B / SH-02: GapDetected cursor preserved (fail-closed) on LiveShadow restart.
///
/// If a prior live-shadow session ended with a detected gap (e.g. WS reconnect
/// without confirmed replay), the cursor is GapDetected.  `seed_ws_continuity_from_db`
/// must preserve GapDetected on restart — not silently promote it to ColdStartUnproven.
///
/// This proves fail-closed behavior for the LiveShadow mode: a known-unsafe prior
/// state is faithfully surfaced rather than hidden.  Even though LiveShadow is the
/// gap-repair mode, it must still surface the gap so the operator knows the prior
/// session ended with unconfirmed continuity.
///
/// DB-backed; skips gracefully without MQK_DATABASE_URL.
#[tokio::test]
async fn lo02b_sh02_live_shadow_gap_detected_preserved_at_restart() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("SH-02: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    let adapter_id = "lo02b-sh02-test";

    let gap_cursor = AlpacaFetchCursor::gap_detected(
        None,
        Some("alpaca:order-lo02b-sh02:canceled:2026-01-10T11:00:00Z".to_string()),
        Some("2026-01-10T11:00:00Z".to_string()),
        "lo02b-sh02: prior session gap detected during WS reconnect",
    );
    let cursor_json = serde_json::to_string(&gap_cursor).expect("SH-02: serialize cursor");
    mqk_db::advance_broker_cursor(&pool, adapter_id, &cursor_json, Utc::now())
        .await
        .expect("SH-02: advance_broker_cursor failed");

    let mut state_inner = AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::LiveShadow,
        BrokerKind::Alpaca,
    );
    state_inner.set_adapter_id_for_test(adapter_id);
    let st = Arc::new(state_inner);

    st.seed_ws_continuity_from_db().await;

    let cont = st.alpaca_ws_continuity().await;
    assert!(
        matches!(cont, AlpacaWsContinuityState::GapDetected { .. }),
        "SH-02: GapDetected cursor must be preserved (fail-closed) at LiveShadow restart; \
         got: {cont:?}"
    );
    if let AlpacaWsContinuityState::GapDetected {
        last_message_id, ..
    } = cont
    {
        assert_eq!(
            last_message_id.as_deref(),
            Some("alpaca:order-lo02b-sh02:canceled:2026-01-10T11:00:00Z"),
            "SH-02: last_message_id must be preserved faithfully from GapDetected cursor"
        );
    }
}

// ---------------------------------------------------------------------------
// SH-03: LiveShadow+Alpaca fresh boot → status surfaces "cold_start_unproven"
// ---------------------------------------------------------------------------

/// LO-02B / SH-03: Fresh LiveShadow+Alpaca boot surfaces "cold_start_unproven" on status.
///
/// On fresh boot (no prior cursor, no DB configured), the system status endpoint must
/// surface `alpaca_ws_continuity = "cold_start_unproven"`.  This is the honest truth:
/// no WS connection has been established yet.
///
/// Proves the status route reads live in-memory continuity state for the LiveShadow
/// deployment mode (same contract as LiveCapital from AP-08).
///
/// In-process test — no DB required.
#[tokio::test]
async fn lo02b_sh03_live_shadow_fresh_boot_surfaces_cold_start_unproven() {
    let st = Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::LiveShadow,
        BrokerKind::Alpaca,
    ));

    let json = get_status(&st).await;
    assert_eq!(
        json["alpaca_ws_continuity"], "cold_start_unproven",
        "SH-03: fresh LiveShadow+Alpaca boot must surface 'cold_start_unproven' on status; \
         got: {json}"
    );
}

// ---------------------------------------------------------------------------
// SH-04: LiveShadow+Alpaca with injected GapDetected → status surfaces "gap_detected"
// ---------------------------------------------------------------------------

/// LO-02B / SH-04: Injected GapDetected surfaces honestly on LiveShadow system status.
///
/// After injecting GapDetected continuity state (simulating a seeded gap from a
/// prior session), the system status endpoint must report
/// `alpaca_ws_continuity = "gap_detected"`.
///
/// Proves the status route reads live in-memory truth and surfaces the unsafe state
/// honestly rather than masking it with a nominal value.
///
/// In-process test — no DB required.
#[tokio::test]
async fn lo02b_sh04_live_shadow_gap_detected_surfaces_on_status() {
    let st = Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::LiveShadow,
        BrokerKind::Alpaca,
    ));

    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:order-lo02b-sh04:filled:2026-01-10T12:00:00Z".to_string()),
        last_event_at: Some("2026-01-10T12:00:00Z".to_string()),
        detail: "lo02b-sh04: simulated gap seeded from prior session cursor".to_string(),
    })
    .await;

    let json = get_status(&st).await;
    assert_eq!(
        json["alpaca_ws_continuity"], "gap_detected",
        "SH-04: injected GapDetected must surface as 'gap_detected' on status route; \
         got: {json}"
    );
}

// ---------------------------------------------------------------------------
// SH-05: LiveShadow+Alpaca with GapDetected → start falls through to DB gate
// ---------------------------------------------------------------------------

/// LO-02B / SH-05: LiveShadow intentionally has no WS continuity start gate.
///
/// Even with GapDetected continuity, LiveShadow+Alpaca must NOT be refused at
/// the WS continuity gate.  LiveShadow is the cursor establishment and gap-repair
/// mode — it is how the system re-establishes a Live cursor after a gap.
///
/// Blocking it at the WS continuity gate would create an unrecoverable deadlock:
/// the operator could not use the repair mode to fix a GapDetected state.  This
/// is the intentional design contract that distinguishes LiveShadow from
/// Paper+Alpaca (which IS blocked by the gate).
///
/// After arming (clearing the integrity gate), start must fall through to the
/// DB gate (503 service unavailable — no DB configured) rather than being
/// refused at the continuity gate (which would return 403 with
/// gate=alpaca_ws_continuity).
///
/// In-process test — no DB required.
#[tokio::test]
async fn lo02b_sh05_live_shadow_gap_detected_reaches_db_gate_not_continuity_gate() {
    let st = Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::LiveShadow,
        BrokerKind::Alpaca,
    ));

    // Inject GapDetected — the unsafe state that blocks Paper+Alpaca and LiveCapital,
    // but must NOT block LiveShadow (the gap-repair path).
    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:order-lo02b-sh05:canceled:2026-01-10T13:00:00Z".to_string()),
        last_event_at: Some("2026-01-10T13:00:00Z".to_string()),
        detail: "lo02b-sh05: gap to be repaired by this live-shadow session".to_string(),
    })
    .await;

    // Arm so the integrity gate is not the blocker.
    arm_via_http(&st).await;

    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("POST")
            .uri("/v1/run/start")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    // Must reach the DB gate (503), not be refused at the WS continuity gate (403).
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "SH-05: LiveShadow+Alpaca with GapDetected must reach DB gate (503), \
         not be refused at continuity gate (403); got: {status}, body: {json}"
    );
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "SH-05: error must name DB gate as the blocker, not WS continuity; got: {json}"
    );
    // Belt-and-suspenders: confirm the WS continuity gate was not the refusal point.
    assert_ne!(
        json["gate"], "alpaca_ws_continuity",
        "SH-05: gate must NOT be alpaca_ws_continuity for LiveShadow; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// SH-06: LiveCapital + ColdStartUnproven → refused at WS continuity gate (pre-DB)
// ---------------------------------------------------------------------------

/// LO-02B / SH-06: LiveCapital WS continuity gate fires before DB gate (production fix).
///
/// The LiveCapital WS continuity check was previously placed after
/// `build_execution_orchestrator`, which could leave dangling "Created" run rows
/// in the DB when the check failed.  This patch moved it to before `db_pool()`.
///
/// This test proves the gate fires correctly at its new position:
///
///   integrity gate (arm) →
///   capital token gate (bypassed via TokenRequired auth) →
///   WS continuity gate (403 for LiveCapital) →
///   (never reaches DB gate)
///
/// In-process test — no DB required.
#[tokio::test]
async fn lo02b_sh06_live_capital_cold_start_refused_at_continuity_gate_before_db() {
    let mut state_inner = AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::LiveCapital,
        BrokerKind::Alpaca,
    );
    // Override operator auth to bypass the capital token gate
    // (which requires TokenRequired; ExplicitDevNoToken would be blocked there first).
    state_inner.operator_auth =
        OperatorAuthMode::TokenRequired("lo02b-sh06-test-token".to_string());
    let st = Arc::new(state_inner);

    // Arm directly (integrity field manipulation) — TokenRequired auth blocks the
    // HTTP arm endpoint, so we bypass it here; the test subject is the WS continuity
    // gate, not the arm flow.
    {
        let mut integrity = st.integrity.write().await;
        integrity.disarmed = false;
        integrity.halted = false;
    }

    // Default continuity is ColdStartUnproven — do not inject anything.
    // Include the operator token so the auth middleware passes (the subject
    // under test is the WS continuity gate, not the auth middleware).
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("POST")
            .uri("/v1/run/start")
            .header("Authorization", "Bearer lo02b-sh06-test-token")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    // WS continuity gate must fire at 403 before the DB gate can return 503.
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "SH-06: LiveCapital with ColdStartUnproven must return 403 (WS continuity gate); \
         got: {status}"
    );
    assert_eq!(
        json["gate"], "alpaca_ws_continuity",
        "SH-06: gate must be alpaca_ws_continuity; got: {json}"
    );
    assert_eq!(
        json["fault_class"], "runtime.start_refused.capital_ws_continuity_unproven",
        "SH-06: fault_class must identify live-capital continuity refusal; got: {json}"
    );
    // Confirm the DB gate was not reached (no dangling run row possible).
    assert!(
        !json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "SH-06: must not reach DB gate — WS continuity gate must fire first; got: {json}"
    );
}
