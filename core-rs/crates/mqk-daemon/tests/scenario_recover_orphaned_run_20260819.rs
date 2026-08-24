//! PAPER-SOAK-REPAIR-20260819-ORPHANED-RUN-RECOVERY-01 — `recover-orphaned-run`
//! operator route proof tests.
//!
//! ## Background
//!
//! A daemon crash/reboot (the exact residue this patch was written to
//! resolve) can leave a `runs` row durably `ARMED`/`RUNNING` with no local
//! runtime owner. `create_or_reuse_run_for_start` (`state/lifecycle.rs`)
//! already correctly refuses to start a fresh run in that state
//! (`runtime.truth_mismatch.durable_active_without_local_owner`, fail-closed,
//! `Manual` retry class per `autonomous_retry_policy.rs`) — but before this
//! patch nothing could ever *resolve* that refusal without raw SQL or the
//! unguarded `mqk_db::stop_run` primitive. `recover-orphaned-run` closes that
//! gap: an operator-facing `/api/v1/ops/action` route that reuses
//! `mqk_db::stop_run_if_evidence_clean` (proof tests in
//! `mqk-db/tests/scenario_stop_run_if_evidence_clean_20260819.rs`) plus a
//! local-ownership guard this route owns.
//!
//! ## Test matrix
//!
//! | Test | What it proves |
//! |------|-----------------|
//! | R01  | recover-orphaned-run without DB -> 503 fail-closed |
//! | R02  | catalog contains recover-orphaned-run with required fields (no DB, disabled) |
//! | R03  | no ARMED/RUNNING run for this engine/mode -> 409 no_active_run |
//! | R04  | a STOPPED run present is not "active" -> 409 no_active_run (not run_not_active) |
//! | R05  | the exact crash-orphan shape (RUNNING, no local owner, zero unacked outbox, \
//! |      | clean reconcile) -> 200 orphaned_run_stopped, durable STOPPED; \
//! |      | `create_or_reuse_run_for_start` is blocked before and succeeds after -- the \
//! |      | literal Thursday-readiness proof; sys_arm_state is left untouched throughout |
//! | R06  | unacked outbox negative control via the real route -> 409 unacknowledged_outbox, \
//! |      | zero mutation |
//! | R07  | dirty reconcile negative control via the real route -> 409 reconcile_dirty, \
//! |      | zero mutation |
//! | R08  | genuinely active local matching runtime -> 409 local_execution_loop_active, \
//! |      | zero mutation (never races a runtime this process actually owns) |
//! | R09  | mismatched local runtime (owns a DIFFERENT run) -> 409 local_execution_loop_active |
//! | R10  | unapplied inbox row negative control via the real route (PAPER-SOAK-UNRESOLVED- \
//! |      | BROKER-EVIDENCE-GATE-01) -> 409 unapplied_inbox, zero mutation |
//! | S01  | PAPER-SOAK-ORPHAN-RECOVERY-PAPER-SCOPE-01: non-Paper mode -> 403 not_paper_mode, \
//! |      | gated before any DB lookup |
//! | S02  | non-Paper catalog entry: disabled, paper-only reason, no DB query for a Live run |
//!
//! R01-R02 and S01-S02 are pure in-process (no DB required). R03-R10 require
//! `MQK_DATABASE_URL` and run against an isolated disposable database via
//! `mqk_db::run_isolated` (never the shared/Paper database) -- marked
//! `#[ignore]`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_recover_orphaned_run_20260819 \
//!   -- --include-ignored --test-threads=1 --nocapture

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use chrono::Utc;
use mqk_daemon::{
    routes::build_router,
    state::{AppState, BrokerKind, DeploymentMode},
};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn no_db_router() -> axum::Router {
    let st = Arc::new(AppState::new_for_test_with_mode(DeploymentMode::Paper));
    build_router(st)
}

fn live_capital_no_db_router() -> axum::Router {
    let st = Arc::new(AppState::new_for_test_with_mode(
        DeploymentMode::LiveCapital,
    ));
    build_router(st)
}

async fn post_action(
    router: axum::Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
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

fn run_id_for(seed: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, seed.as_bytes())
}

async fn seed_run(pool: &sqlx::PgPool, run_id: Uuid) {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await
    .expect("insert_run");
}

async fn seed_clean_reconcile(pool: &sqlx::PgPool) {
    mqk_db::persist_reconcile_status_state(
        pool,
        &mqk_db::PersistReconcileStatusState {
            status: "ok",
            last_run_at_utc: Some(Utc::now()),
            snapshot_watermark_ms: Some(1),
            mismatched_positions: 0,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: None,
            updated_at_utc: Utc::now(),
        },
    )
    .await
    .expect("persist_reconcile_status_state clean");
}

// ---------------------------------------------------------------------------
// R01: no DB -> 503
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r01_recover_orphaned_run_no_db_returns_503() {
    let (status, j) = post_action(
        no_db_router(),
        serde_json::json!({ "action_key": "recover-orphaned-run" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "R01: recover-orphaned-run without DB must return 503; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// R02: catalog entry present with required fields (no DB)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r02_catalog_contains_recover_orphaned_run_with_required_fields() {
    let j = get_catalog(no_db_router()).await;
    let actions = j["actions"].as_array().expect("actions must be an array");

    let entry = actions
        .iter()
        .find(|a| a["action_key"].as_str() == Some("recover-orphaned-run"))
        .expect("R02: recover-orphaned-run must be in catalog");

    assert!(entry["label"].is_string());
    assert!(entry["level"].is_number());
    assert!(entry["description"].is_string());
    assert!(entry["requires_reason"].is_boolean());
    assert!(entry["confirm_text"].is_string());
    assert!(entry["enabled"].is_boolean());
    assert_eq!(
        entry["enabled"], false,
        "R02: must be disabled when no DB; entry: {entry}"
    );
    assert!(
        entry["disabled_reason"].is_string(),
        "R02: must have disabled_reason when no DB"
    );
}

// ---------------------------------------------------------------------------
// R03/R04: no eligible active run.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn r03_no_active_run_returns_409() {
    mqk_db::run_isolated("recover_orphan_r03", |pool| async move {
        let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
            pool,
            DeploymentMode::Paper,
            BrokerKind::Paper,
        ));
        let router = build_router(st);

        let (status, j) = post_action(
            router,
            serde_json::json!({ "action_key": "recover-orphaned-run" }),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT, "R03: body: {j}");
        assert_eq!(j["accepted"], false);
        assert_eq!(j["disposition"].as_str(), Some("no_active_run"));
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn r04_stopped_run_is_not_active() {
    mqk_db::run_isolated("recover_orphan_r04", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair-route.r04");
        seed_run(&pool, run_id).await;
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
            serde_json::json!({ "action_key": "recover-orphaned-run" }),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT, "R04: body: {j}");
        assert_eq!(
            j["disposition"].as_str(),
            Some("no_active_run"),
            "R04: a STOPPED run must never be found as 'active' by fetch_active_run_for_engine"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// R05: the exact crash-orphan shape, end to end.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn r05_crash_orphaned_run_is_recovered_and_unblocks_fresh_start() {
    mqk_db::run_isolated("recover_orphan_r05", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair-route.r05");
        seed_run(&pool, run_id).await;
        mqk_db::arm_run(&pool, run_id).await.expect("arm_run");
        mqk_db::begin_run(&pool, run_id).await.expect("begin_run");
        seed_clean_reconcile(&pool).await;
        mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None)
            .await
            .expect("persist_arm_state_canonical ARMED");

        let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            DeploymentMode::Paper,
            BrokerKind::Paper,
        ));

        // Thursday-readiness precondition: a fresh daemon process (no local
        // runtime at all -- exactly this AppState, which never called
        // start_execution_runtime) must be BLOCKED from starting a new run
        // while the orphaned run is still durably active.
        let blocked = st
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect_err("R05 precondition: a fresh start must be blocked by the orphaned run");
        assert_eq!(
            blocked.fault_class(),
            "runtime.truth_mismatch.durable_active_without_local_owner",
            "R05 precondition: must be refused with the exact crash-orphan fault class"
        );

        let router = build_router(Arc::clone(&st));
        let (status, j) = post_action(
            router,
            serde_json::json!({ "action_key": "recover-orphaned-run" }),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "R05: body: {j}");
        assert_eq!(j["accepted"], true, "R05: body: {j}");
        assert_eq!(
            j["disposition"].as_str(),
            Some("orphaned_run_stopped"),
            "R05: body: {j}"
        );
        assert_eq!(
            j["audit"]["durable_db_write"], true,
            "R05: must report a durable DB write; body: {j}"
        );

        let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after.status, mqk_db::RunStatus::Stopped),
            "R05: run must be durably STOPPED after recovery"
        );

        // Stale-ARMED-authority behavior: this route never touches
        // sys_arm_state -- it stays exactly ARMED, unchanged. Re-arming (if
        // ever needed) is the coordinator's own idempotent typed arm call on
        // its next start attempt (attempt_canonical_start / D2.10), not this
        // route's job.
        let (arm_state, _) = mqk_db::load_arm_state(&pool)
            .await
            .expect("load_arm_state")
            .expect("arm state row must still exist");
        assert_eq!(
            arm_state, "ARMED",
            "R05: recover-orphaned-run must never touch sys_arm_state"
        );

        // Thursday-readiness proof: the same fresh-start check that was
        // refused above must now succeed -- a brand new run may be created.
        let fresh_run_id = st
            .create_or_reuse_run_for_start(&pool)
            .await
            .expect("R05: a fresh start must be unblocked once the orphan is recovered");
        assert_ne!(
            fresh_run_id, run_id,
            "R05: the fresh run must be a distinct identity from the recovered orphan"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// R06/R07: evidence negative controls via the real route.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn r06_unacked_outbox_blocks_release_via_route() {
    mqk_db::run_isolated("recover_orphan_r06", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair-route.r06");
        seed_run(&pool, run_id).await;
        mqk_db::arm_run(&pool, run_id).await.expect("arm_run");
        mqk_db::begin_run(&pool, run_id).await.expect("begin_run");
        seed_clean_reconcile(&pool).await;
        mqk_db::outbox_enqueue(
            &pool,
            run_id,
            "orphan-repair-route-r06-key",
            serde_json::json!({"symbol": "AAPL", "side": "BUY", "qty": 1}),
        )
        .await
        .expect("outbox_enqueue");

        let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            DeploymentMode::Paper,
            BrokerKind::Paper,
        ));
        let router = build_router(st);

        let (status, j) = post_action(
            router,
            serde_json::json!({ "action_key": "recover-orphaned-run" }),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT, "R06: body: {j}");
        assert_eq!(
            j["disposition"].as_str(),
            Some("unacknowledged_outbox"),
            "R06: body: {j}"
        );

        let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after.status, mqk_db::RunStatus::Running),
            "R06: a refused release must not mutate run status"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn r07_dirty_reconcile_blocks_release_via_route() {
    mqk_db::run_isolated("recover_orphan_r07", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair-route.r07");
        seed_run(&pool, run_id).await;
        mqk_db::arm_run(&pool, run_id).await.expect("arm_run");
        mqk_db::begin_run(&pool, run_id).await.expect("begin_run");
        mqk_db::persist_reconcile_status_state(
            &pool,
            &mqk_db::PersistReconcileStatusState {
                status: "dirty",
                last_run_at_utc: Some(Utc::now()),
                snapshot_watermark_ms: Some(1),
                mismatched_positions: 1,
                mismatched_orders: 0,
                mismatched_fills: 0,
                unmatched_broker_events: 0,
                note: Some("R07: injected mismatch"),
                updated_at_utc: Utc::now(),
            },
        )
        .await
        .expect("persist dirty reconcile");

        let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            DeploymentMode::Paper,
            BrokerKind::Paper,
        ));
        let router = build_router(st);

        let (status, j) = post_action(
            router,
            serde_json::json!({ "action_key": "recover-orphaned-run" }),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT, "R07: body: {j}");
        assert_eq!(
            j["disposition"].as_str(),
            Some("reconcile_dirty"),
            "R07: body: {j}"
        );

        let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after.status, mqk_db::RunStatus::Running),
            "R07: a refused release must not mutate run status"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// R08/R09: local-ownership negative controls.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn r08_genuinely_owned_local_run_refuses() {
    mqk_db::run_isolated("recover_orphan_r08", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair-route.r08");
        seed_run(&pool, run_id).await;
        mqk_db::arm_run(&pool, run_id).await.expect("arm_run");
        mqk_db::begin_run(&pool, run_id).await.expect("begin_run");
        seed_clean_reconcile(&pool).await;

        let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            DeploymentMode::Paper,
            BrokerKind::Paper,
        ));
        // This AppState IS the local owner of run_id -- the exact opposite
        // of the crash-orphan shape. recover-orphaned-run must refuse, never
        // race a runtime this process is actively driving.
        st.inject_running_loop_for_test(run_id).await;

        let router = build_router(Arc::clone(&st));
        let (status, j) = post_action(
            router,
            serde_json::json!({ "action_key": "recover-orphaned-run" }),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT, "R08: body: {j}");
        assert_eq!(
            j["disposition"].as_str(),
            Some("local_execution_loop_active"),
            "R08: body: {j}"
        );

        let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after.status, mqk_db::RunStatus::Running),
            "R08: a refused release must not mutate run status"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn r09_mismatched_local_run_also_refuses() {
    mqk_db::run_isolated("recover_orphan_r09", |pool| async move {
        let orphan_run_id = run_id_for("mqk-daemon.orphan-repair-route.r09-orphan");
        let other_run_id = run_id_for("mqk-daemon.orphan-repair-route.r09-other");
        seed_run(&pool, orphan_run_id).await;
        mqk_db::arm_run(&pool, orphan_run_id).await.expect("arm_run orphan");
        mqk_db::begin_run(&pool, orphan_run_id)
            .await
            .expect("begin_run orphan");
        seed_clean_reconcile(&pool).await;
        seed_run(&pool, other_run_id).await;
        mqk_db::arm_run(&pool, other_run_id).await.expect("arm_run other");
        mqk_db::begin_run(&pool, other_run_id)
            .await
            .expect("begin_run other");

        let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            DeploymentMode::Paper,
            BrokerKind::Paper,
        ));
        // This AppState locally owns a DIFFERENT run than the one
        // fetch_active_run_for_engine would surface first -- still refused:
        // the guard is "does this process own ANY local run", not an
        // exact-match check, because owning any run at all means this is
        // not the cold no-local-runtime case this route exists for.
        st.inject_running_loop_for_test(other_run_id).await;

        let router = build_router(Arc::clone(&st));
        let (status, j) = post_action(
            router,
            serde_json::json!({ "action_key": "recover-orphaned-run" }),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT, "R09: body: {j}");
        assert_eq!(
            j["disposition"].as_str(),
            Some("local_execution_loop_active"),
            "R09: body: {j}"
        );

        let orphan_after = mqk_db::fetch_run(&pool, orphan_run_id)
            .await
            .expect("fetch_run orphan");
        assert!(
            matches!(orphan_after.status, mqk_db::RunStatus::Running),
            "R09: a refused release must not mutate the orphaned run's status"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// R10: unapplied inbox negative control via the real route
// (PAPER-SOAK-UNRESOLVED-BROKER-EVIDENCE-GATE-01).
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn r10_unapplied_inbox_blocks_release_via_route() {
    mqk_db::run_isolated("recover_orphan_r10", |pool| async move {
        let run_id = run_id_for("mqk-daemon.orphan-repair-route.r10");
        seed_run(&pool, run_id).await;
        mqk_db::arm_run(&pool, run_id).await.expect("arm_run");
        mqk_db::begin_run(&pool, run_id).await.expect("begin_run");
        seed_clean_reconcile(&pool).await;
        mqk_db::inbox_insert_deduped(
            &pool,
            run_id,
            "orphan-repair-route-r10-msg",
            serde_json::json!({"event_kind": "fill"}),
        )
        .await
        .expect("inbox_insert_deduped");

        let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            DeploymentMode::Paper,
            BrokerKind::Paper,
        ));
        let router = build_router(st);

        let (status, j) = post_action(
            router,
            serde_json::json!({ "action_key": "recover-orphaned-run" }),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT, "R10: body: {j}");
        assert_eq!(
            j["disposition"].as_str(),
            Some("unapplied_inbox"),
            "R10: body: {j}"
        );

        let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert!(
            matches!(after.status, mqk_db::RunStatus::Running),
            "R10: a refused release must not mutate run status"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// S01/S02: PAPER-SOAK-ORPHAN-RECOVERY-PAPER-SCOPE-01 -- recover-orphaned-run
// is paper-only.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s01_non_paper_mode_returns_403_before_any_db_lookup() {
    let (status, j) = post_action(
        live_capital_no_db_router(),
        serde_json::json!({ "action_key": "recover-orphaned-run" }),
    )
    .await;

    // No DB is configured on this router at all -- a 403 here (rather than
    // the 503 db_unavailable R01 proves for Paper mode) is itself proof the
    // Paper-mode gate runs strictly before the DB-lookup gate.
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "S01: non-Paper mode must return 403, gated before any DB lookup; body: {j}"
    );
    assert_eq!(j["accepted"], false, "S01: body: {j}");
    assert_eq!(
        j["disposition"].as_str(),
        Some("not_paper_mode"),
        "S01: body: {j}"
    );
}

#[tokio::test]
async fn s02_non_paper_catalog_entry_is_disabled_with_paper_only_reason() {
    let j = get_catalog(live_capital_no_db_router()).await;
    let actions = j["actions"].as_array().expect("actions must be an array");

    let entry = actions
        .iter()
        .find(|a| a["action_key"].as_str() == Some("recover-orphaned-run"))
        .expect("S02: recover-orphaned-run must be in catalog");

    assert_eq!(
        entry["enabled"], false,
        "S02: must be disabled for non-Paper mode; entry: {entry}"
    );
    assert_eq!(
        entry["disabled_reason"].as_str(),
        Some("recover-orphaned-run is paper-only"),
        "S02: disabled_reason must truthfully state paper-only; entry: {entry}"
    );
}
