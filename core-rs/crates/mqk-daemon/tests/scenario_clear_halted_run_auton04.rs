//! AUTON-PAPER-OPS-04 — clear-halted-run operator path proof tests.
//!
//! Proves that the `clear-halted-run` ops/action correctly:
//! - Refuses without a DB (503 fail-closed)
//! - Reports required catalog fields (enabled=false when no halted run / no DB)
//! - Refuses when no run exists (409 no_run_found)
//! - Refuses when latest run is not HALTED (409 run_not_halted)
//! - Clears a HALTED run to STOPPED (200 halted_run_cleared, durable write)
//! - After clearing, start_execution_runtime is no longer blocked by halted lifecycle
//! - After clearing, startup creates a fresh run instead of reusing stale replay state
//!
//! ## Test matrix
//!
//! | Test  | What it proves                                                             |
//! |-------|----------------------------------------------------------------------------|
//! | H01   | clear-halted-run without DB → 503 fail-closed                             |
//! | H02   | catalog contains clear-halted-run with required fields (no DB)             |
//! | H03   | clear-halted-run with no run in DB → 409 no_run_found (DB-backed)         |
//! | H04   | clear-halted-run when latest run is STOPPED → 409 run_not_halted (DB)     |
//! | H05   | clear-halted-run on a HALTED run → 200 halted_run_cleared, durable (DB)  |
//! | H06   | after clear, halted_lifecycle gate cannot fire: run is STOPPED (DB proof;                  |
//! |       | calling start_execution_runtime after clear is not network-safe — fetch_events fires on   |
//! |       | every tick in LiveShadow+Alpaca; structural DB proof is the only safe option)             |
//! | H07   | after clear, next_daemon_run_id() formula yields computed_fresh_run_id != halted_run_id; |
//! |       | stale inbox rows remain under halted_run_id, invisible to computed_fresh_run_id           |
//!
//! H01-H02 are pure in-process (no DB required).
//! H03-H07 require MQK_DATABASE_URL and are #[ignore].
//!
//! ## Proof-strength status
//!
//! H01–H05: fully executed against a real DB; all pass.
//!
//! H06/H07: PARTIAL proofs.  H06 executes `start_execution_runtime()` before
//! clear (returns `halted_lifecycle`) but the after-clear assertion is a DB
//! status check, not a second `start_execution_runtime()` call.  H07 proves
//! inbox isolation via DB query but `computed_fresh_run_id` uses the test
//! process's `node_id` (which embeds `pid={}` from `default_node_id()`), so
//! it is NOT the exact UUID a production daemon restart would allocate.
//!
//! A stronger executed proof for H06/H07 is technically feasible (using a
//! local mock Alpaca server in the style of `start_mock_alpaca_server()` in
//! `scenario_daemon_runtime_lifecycle.rs`), but requires (a) the mock server
//! to serve `/v2/account/activities/FILL` (the adapter's current URL) rather
//! than the stale `/v2/account/activities`, and (b) Paper+Alpaca setup with
//! WS continuity, broker snapshot, and strategy registry wiring.  This scope
//! expansion is deferred to a future patch.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use mqk_daemon::{
    routes::build_router,
    state::{AppState, BrokerKind, DeploymentMode},
};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn no_db_router() -> axum::Router {
    let st = Arc::new(AppState::new_for_test_with_mode(DeploymentMode::Paper));
    build_router(st)
}

async fn post_action(
    router: axum::Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    // ExplicitDevNoToken mode: Authorization header is not checked.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, j)
}

async fn get_catalog(router: axum::Router) -> serde_json::Value {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/ops/catalog")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

// ---------------------------------------------------------------------------
// DB helpers
// ---------------------------------------------------------------------------

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_clear_halted_run_auton04 \
             -- --include-ignored --test-threads 1"
        )
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to test DB");
    mqk_db::migrate(&pool).await.expect("run migrations");
    pool
}

async fn cleanup_test_runs(pool: &sqlx::PgPool) {
    // Scoped to EXACTLY the run_ids seeded by this test file.
    // Never touches any other PAPER or LIVE-SHADOW run, so this function is
    // safe to run against a shared developer DB that may contain real daemon history.
    let test_run_ids = [
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"mqk-daemon.auton04.h03"),
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"mqk-daemon.auton04.h04"),
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"mqk-daemon.auton04.h05"),
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"mqk-daemon.auton04.h06"),
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"mqk-daemon.auton04.h07"),
    ];
    for rid in &test_run_ids {
        // Force-stop any test run stuck in ARMED/RUNNING (defensive; tests
        // should not leave runs in these states, but guard against prior crashes).
        sqlx::query(
            "update runs set status = 'STOPPED', stopped_at_utc = now() \
             where run_id = $1 and status in ('ARMED', 'RUNNING')",
        )
        .bind(rid)
        .execute(pool)
        .await
        .ok();

        // broker_order_map → oms_outbox uses ON DELETE RESTRICT; remove those
        // rows first before the cascade-delete via runs.
        sqlx::query(
            "delete from broker_order_map where internal_id in (\
               select idempotency_key from oms_outbox where run_id = $1)",
        )
        .bind(rid)
        .execute(pool)
        .await
        .ok();

        // Deleting the run cascades to oms_outbox and oms_inbox (ON DELETE CASCADE).
        sqlx::query("delete from runs where run_id = $1")
            .bind(rid)
            .execute(pool)
            .await
            .ok();
    }
}

// ---------------------------------------------------------------------------
// H01: clear-halted-run without DB → 503
// ---------------------------------------------------------------------------

/// H01: clear-halted-run without DB → 503 SERVICE_UNAVAILABLE (fail-closed).
#[tokio::test]
async fn h01_clear_halted_run_no_db_returns_503() {
    let (status, j) = post_action(
        no_db_router(),
        serde_json::json!({ "action_key": "clear-halted-run" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "H01: clear-halted-run without DB must return 503; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// H02: catalog entry present with required fields (pure, no DB)
// ---------------------------------------------------------------------------

/// H02: ops/catalog contains clear-halted-run with all required fields.
///
/// Also proves the entry is disabled (enabled=false) when no DB is available.
#[tokio::test]
async fn h02_catalog_contains_clear_halted_run_with_required_fields() {
    let j = get_catalog(no_db_router()).await;
    let actions = j["actions"].as_array().expect("actions must be an array");

    let entry = actions
        .iter()
        .find(|a| a["action_key"].as_str() == Some("clear-halted-run"))
        .expect("H02: clear-halted-run must be in catalog");

    assert!(
        entry["label"].is_string(),
        "H02: clear-halted-run must have label"
    );
    assert!(
        entry["level"].is_number(),
        "H02: clear-halted-run must have level"
    );
    assert!(
        entry["description"].is_string(),
        "H02: clear-halted-run must have description"
    );
    assert!(
        entry["requires_reason"].is_boolean(),
        "H02: clear-halted-run must have requires_reason"
    );
    assert!(
        entry["confirm_text"].is_string(),
        "H02: clear-halted-run must have confirm_text"
    );
    assert!(
        entry["enabled"].is_boolean(),
        "H02: clear-halted-run must have enabled"
    );
    // No DB → no halted run detectable → disabled.
    assert_eq!(
        entry["enabled"], false,
        "H02: clear-halted-run must be disabled when no DB; entry: {entry}"
    );
    assert!(
        entry["disabled_reason"].is_string(),
        "H02: clear-halted-run must have disabled_reason when no DB"
    );
}

// ---------------------------------------------------------------------------
// DB-backed tests (require MQK_DATABASE_URL, #[ignore] in CI)
// ---------------------------------------------------------------------------

/// H03: clear-halted-run with no run in DB → 409 no_run_found.
#[tokio::test]
#[ignore]
async fn h03_clear_halted_run_no_run_returns_409() {
    let pool = make_db_pool().await;
    cleanup_test_runs(&pool).await;

    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Paper,
    ));
    let router = build_router(st);

    let (status, j) = post_action(
        router,
        serde_json::json!({ "action_key": "clear-halted-run" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "H03: clear-halted-run with no run must return 409; body: {j}"
    );
    assert_eq!(j["accepted"], false, "H03: must not be accepted; body: {j}");
    assert_eq!(
        j["disposition"].as_str(),
        Some("no_run_found"),
        "H03: disposition must be no_run_found; body: {j}"
    );
}

/// H04: clear-halted-run when latest run is STOPPED → 409 run_not_halted.
#[tokio::test]
#[ignore]
async fn h04_clear_halted_run_stopped_run_returns_409() {
    let pool = make_db_pool().await;
    cleanup_test_runs(&pool).await;

    let run_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"mqk-daemon.auton04.h04");
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: chrono::Utc::now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await
    .expect("insert_run");
    // CREATED → ARMED → RUNNING → STOPPED
    mqk_db::arm_run(&pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(&pool, run_id).await.expect("begin_run");
    mqk_db::stop_run(&pool, run_id).await.expect("stop_run");

    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Paper,
    ));
    let router = build_router(st);

    let (status, j) = post_action(
        router,
        serde_json::json!({ "action_key": "clear-halted-run" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "H04: clear-halted-run on STOPPED run must return 409; body: {j}"
    );
    assert_eq!(j["accepted"], false, "H04: must not be accepted; body: {j}");
    assert_eq!(
        j["disposition"].as_str(),
        Some("run_not_halted"),
        "H04: disposition must be run_not_halted; body: {j}"
    );
    assert!(
        j["blockers"].as_array().is_some_and(|b| !b.is_empty()),
        "H04: blockers must explain the refusal; body: {j}"
    );
}

/// H05: clear-halted-run on a HALTED run → 200 halted_run_cleared, durable write.
#[tokio::test]
#[ignore]
async fn h05_clear_halted_run_on_halted_run_returns_200() {
    let pool = make_db_pool().await;
    cleanup_test_runs(&pool).await;

    let run_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"mqk-daemon.auton04.h05");
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: chrono::Utc::now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::halt_run(&pool, run_id, chrono::Utc::now())
        .await
        .expect("halt_run");

    // Confirm initial state is HALTED.
    let before = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
    assert!(
        matches!(before.status, mqk_db::RunStatus::Halted),
        "H05 precondition: run must be HALTED; status: {}",
        before.status.as_str()
    );

    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Paper,
    ));
    let router = build_router(st);

    let (status, j) = post_action(
        router,
        serde_json::json!({ "action_key": "clear-halted-run" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "H05: clear-halted-run on HALTED run must return 200; body: {j}"
    );
    assert_eq!(j["accepted"], true, "H05: must be accepted; body: {j}");
    assert_eq!(
        j["disposition"].as_str(),
        Some("halted_run_cleared"),
        "H05: disposition must be halted_run_cleared; body: {j}"
    );
    assert_eq!(
        j["audit"]["durable_db_write"], true,
        "H05: must report a durable DB write; body: {j}"
    );
    assert!(
        j["warnings"].as_array().is_some_and(|w| !w.is_empty()),
        "H05: warnings must instruct operator to re-arm and re-start; body: {j}"
    );

    // Durable verification: run must now be STOPPED.
    let after = mqk_db::fetch_run(&pool, run_id)
        .await
        .expect("fetch_run after");
    assert!(
        matches!(after.status, mqk_db::RunStatus::Stopped),
        "H05: run must be STOPPED after clear; status: {}",
        after.status.as_str()
    );
}

/// H06: after clear_halted_run, the halted_lifecycle gate can no longer block
/// start_execution_runtime.
///
/// Proves the gate structurally: the lifecycle code fires `halted_lifecycle`
/// only on `RunStatus::Halted`.  After clear the run is `Stopped`, which takes
/// the fresh-run-allocation branch — no error from that gate.
///
/// The "before clear" call to start_execution_runtime is safe: it fires at
/// `halted_lifecycle` before reaching build_execution_orchestrator, so no
/// broker client is constructed and no network path is entered.
///
/// The "after clear" proof is a DB fetch rather than a second
/// start_execution_runtime call.  Calling start_execution_runtime after clear
/// is not network-safe for LiveShadow+Alpaca: orchestrator.rs Phase 2 calls
/// gateway.fetch_events on every tick (a live Alpaca REST call that fires
/// regardless of run state or outbox load), and build_daemon_broker requires
/// ALPACA_API_KEY_LIVE — neither constraint can be bypassed without modifying
/// production code.  If live creds are absent the call fails at
/// build_daemon_broker (proof of gate, but relies on creds being absent); if
/// live creds are present fetch_events hits the live Alpaca REST API.
/// Verifying RunStatus::Stopped via fetch_run is the only intrinsically safe
/// proof option for this path.
///
/// Uses LiveShadow+Alpaca so the WS-continuity, reconcile, and
/// strategy-dormancy pre-DB gates are skipped, making halted_lifecycle the
/// definitive blocker before the clear.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn h06_after_clear_halted_lifecycle_gate_is_unblocked() {
    let pool = make_db_pool().await;
    cleanup_test_runs(&pool).await;

    let run_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"mqk-daemon.auton04.h06");
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "LIVE-SHADOW".to_string(),
            started_at_utc: chrono::Utc::now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::halt_run(&pool, run_id, chrono::Utc::now())
        .await
        .expect("halt_run");

    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::LiveShadow,
        BrokerKind::Alpaca,
    ));
    // Arm integrity so the integrity gate is not the blocker.
    st.integrity.write().await.disarmed = false;
    // Clear the strategy fleet so the B2A registry gate is skipped (Dormant
    // bootstrap → no registry lookup).  MQK_STRATEGY_IDS from .env.local
    // would otherwise cause the B2A gate to fire before halted_lifecycle.
    st.set_strategy_fleet_for_test(None).await;

    // Before clear: start must be blocked by halted_lifecycle.
    // This call is safe: the gate fires before build_execution_orchestrator,
    // so no broker client is constructed and no network path is entered.
    {
        let err = st
            .start_execution_runtime()
            .await
            .expect_err("H06: start must fail before clear");
        assert_eq!(
            err.fault_class(),
            "runtime.start_refused.halted_lifecycle",
            "H06: before clear the fault_class must be halted_lifecycle; got: {}",
            err
        );
    }

    // Clear via DB directly (same fn the route calls).
    mqk_db::clear_halted_run(&pool, run_id)
        .await
        .expect("clear_halted_run");

    // After clear: DB proof that halted_lifecycle cannot fire.
    //
    // We do NOT call start_execution_runtime() here because it is not network-safe
    // for LiveShadow+Alpaca (see function-level doc for full constraint analysis).
    // Instead: fetch_run verifies the exact status that start_execution_runtime()
    // reads via fetch_latest_run_for_engine.  lifecycle.rs fires halted_lifecycle
    // only on RunStatus::Halted — RunStatus::Stopped takes the fresh-run-allocation
    // branch unconditionally (lifecycle.rs Stopped arm).  STOPPED in DB is
    // proof that halted_lifecycle cannot fire on the next start attempt.
    //
    // H07 proves the complementary invariant: computed_fresh_run_id from the actual
    // next_daemon_run_id() formula differs from halted_run_id, so stale inbox rows
    // are structurally invisible to the fresh run.
    let after_clear = mqk_db::fetch_run(&pool, run_id)
        .await
        .expect("H06: fetch_run after clear must not fail");
    assert!(
        matches!(after_clear.status, mqk_db::RunStatus::Stopped),
        "H06: run must be STOPPED after clear; \
         halted_lifecycle fires only on RunStatus::Halted — \
         STOPPED takes the fresh-run-allocation branch; \
         got: {}",
        after_clear.status.as_str()
    );
    // No stop_for_shutdown: no execution loop was started in this test.
}
/// H07: clearing a halted run must not allow the next startup to re-consume
/// that run's stale inbox rows.
///
/// Regression seam proven here:
/// 1. A HALTED run carries an unapplied unknown-order fill in `oms_inbox`.
/// 2. `clear_halted_run()` transitions the run to STOPPED but does NOT delete
///    inbox rows — they remain scoped to `halted_run_id`.
/// 3. The next startup (lifecycle.rs Stopped branch) allocates a FRESH run_id
///    via `next_daemon_run_id()`.  This test computes a UUID using the same
///    UUIDv5 formula and live DB generation count, but with the TEST process's
///    `node_id`.  `default_node_id()` embeds `pid={}` — the test process PID
///    differs from any production daemon PID — so `computed_fresh_run_id` is
///    NOT the exact UUID a production restart would allocate.  The proof is
///    structural: any `next_daemon_run_id()` output uses the prefix
///    `"mqk-daemon.run.v2|..."`, which differs from `halted_run_id`'s seed
///    `"mqk-daemon.auton04.h07"`, so it returns 0 inbox rows regardless of PID.
/// 4. `inbox_load_unapplied_for_run(pool, computed_fresh_run_id)` returns 0 rows:
///    inbox rows are keyed by `(run_id, broker_message_id)`, so the fresh run_id
///    has no rows and the UNKNOWN_ORDER_FILL halt-loop cannot recur.
///
/// H06 proves the halted_lifecycle gate is structurally unblocked by STOPPED;
/// H07 proves the inbox isolation invariant (structural, not via exact production UUID).
/// See module-level "Proof-strength status" for the path to a stronger proof.
#[tokio::test]
#[ignore]
async fn h07_after_clear_fresh_run_sees_no_stale_inbox_rows() {
    let pool = make_db_pool().await;
    cleanup_test_runs(&pool).await;

    let halted_run_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"mqk-daemon.auton04.h07");
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: halted_run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "LIVE-SHADOW".to_string(),
            started_at_utc: chrono::Utc::now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::halt_run(&pool, halted_run_id, chrono::Utc::now())
        .await
        .expect("halt_run");

    // Insert a stale unapplied fill for an unknown order into the halted run's
    // inbox.  This is the fill that caused the original halt and must NOT be
    // visible to the fresh run that the next startup allocates.
    let inserted = mqk_db::inbox_insert_deduped(
        &pool,
        halted_run_id,
        "h07-msg-unknown-fill",
        serde_json::json!({
            "type": "fill",
            "broker_message_id": "h07-msg-unknown-fill",
            "internal_order_id": "h07-unknown-order",
            "broker_order_id": null,
            "symbol": "SPY",
            "side": "Buy",
            "delta_qty": 1_i64,
            "price_micros": 500_000_000_i64,
            "fee_micros": 0_i64
        }),
    )
    .await
    .expect("insert stale inbox fill");
    assert!(inserted, "H07: stale inbox row must be inserted");

    // Clear: halted_run_id transitions HALTED → STOPPED.
    // Inbox rows are NOT deleted — they remain scoped to halted_run_id.
    mqk_db::clear_halted_run(&pool, halted_run_id)
        .await
        .expect("clear_halted_run");

    // --- DB-level inbox isolation proof ---

    // The cleared run must be STOPPED (H06 proves this gate implies fresh run;
    // H07 proves the inbox consequence).
    let cleared_run = mqk_db::fetch_run(&pool, halted_run_id)
        .await
        .expect("H07: fetch cleared run must not fail");
    assert!(
        matches!(cleared_run.status, mqk_db::RunStatus::Stopped),
        "H07: cleared run must be STOPPED; got: {}",
        cleared_run.status.as_str()
    );

    // The stale inbox row must still exist under halted_run_id after clear.
    // clear_halted_run() does NOT delete or re-scope inbox rows.
    let stale_rows = mqk_db::inbox_load_unapplied_for_run(&pool, halted_run_id)
        .await
        .expect("H07: inbox_load_unapplied_for_run(halted_run_id) must not fail");
    assert_eq!(
        stale_rows.len(),
        1,
        "H07: stale inbox row must remain under halted_run_id after clear; \
         clearing does not consume, delete, or re-scope inbox rows"
    );
    assert_eq!(
        stale_rows[0].broker_message_id, "h07-msg-unknown-fill",
        "H07: the persisted stale row must be the seeded unknown-fill message"
    );

    // Compute the actual fresh_run_id that next_daemon_run_id() would allocate.
    //
    // Production formula (state/orchestrator_build.rs::next_daemon_run_id):
    //   generation = COUNT(*) + 1 FROM runs WHERE engine_id='mqk-daemon' AND mode='LIVE-SHADOW'
    //   fresh_run_id = Uuid::new_v5(NAMESPACE_DNS,
    //       "mqk-daemon.run.v2|{node_id}|mqk-daemon|LIVE-SHADOW|{generation}")
    //
    // Queried AFTER clear so generation matches the real call's DB view.
    // node_id is derived via the same AppState constructor code path the
    // production daemon uses, BUT default_node_id() embeds pid={pid} so the
    // test process PID differs from any production daemon PID.
    // computed_fresh_run_id is therefore NOT the exact UUID a production
    // restart would allocate — see function-level doc for the structural
    // proof rationale.
    //
    // Calling start_execution_runtime() after clear is not network-safe for
    // LiveShadow+Alpaca (fetch_events on every tick — see H06 constraint analysis).
    // Computing the formula directly proves computed_fresh_run_id != halted_run_id
    // without any live network risk.
    let node_id = AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::LiveShadow,
        BrokerKind::Alpaca,
    )
    .node_id;

    let live_shadow_count: i64 = sqlx::query_scalar(
        "SELECT COALESCE(COUNT(*), 0)::bigint \
         FROM runs WHERE engine_id = $1 AND mode = $2",
    )
    .bind("mqk-daemon")
    .bind("LIVE-SHADOW")
    .fetch_one(&pool)
    .await
    .expect("H07: count LIVE-SHADOW runs for fresh_run_id derivation");

    let next_generation = live_shadow_count + 1;
    let computed_fresh_run_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!(
            "mqk-daemon.run.v2|{}|mqk-daemon|LIVE-SHADOW|{}",
            node_id, next_generation
        )
        .as_bytes(),
    );
    assert_ne!(
        computed_fresh_run_id, halted_run_id,
        "H07: next_daemon_run_id() formula produces computed_fresh_run_id={computed_fresh_run_id} \
         which differs from halted_run_id={halted_run_id}; \
         seed 'mqk-daemon.run.v2|...|LIVE-SHADOW|{next_generation}' \
         cannot collide with 'mqk-daemon.auton04.h07'"
    );

    // inbox_load_unapplied_for_run queries WHERE run_id = $1 AND applied_at_utc IS NULL.
    // computed_fresh_run_id has no inbox rows at all — returns 0 —
    // the UNKNOWN_ORDER_FILL halt-loop cannot recur for the fresh run.
    let fresh_rows = mqk_db::inbox_load_unapplied_for_run(&pool, computed_fresh_run_id)
        .await
        .expect("H07: inbox_load_unapplied_for_run(computed_fresh_run_id) must not fail");
    assert_eq!(
        fresh_rows.len(),
        0,
        "H07: computed_fresh_run_id sees zero inbox rows; \
         inbox is keyed by (run_id, broker_message_id) — \
         stale rows from halted_run_id are structurally invisible to \
         any run_id the production code would allocate"
    );
    // No stop_for_shutdown: no execution loop was started in this test.
}
