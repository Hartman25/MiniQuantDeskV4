//! LIVESHADOW-NO-ORDER-AUTHORITY-CLOSURE-01: LiveShadow observes real broker
//! truth but may never create or authorize a NEW economic order.
//!
//! Complements the manual-order proofs in `state/hermetic_positive_proofs.rs`
//! (`hermetic_live_shadow_manual_order_*`) and the mqk-db durable-seam proof
//! (`scenario_outbox_new_order_mode_fence_01`). Internal strategy decisions
//! under LiveShadow are proven by
//! `scenario_strategy_promotion_runtime_gate_01::internal_active_paper_denied_when_daemon_mode_is_live`;
//! the operator flatten route by `scenario_live_shadow_flatten_on_halt_01` (LSF-01).
//!
//! | Test | What it proves |
//! |------|-----------------|
//! | t1 | external `/strategy/signal` under LiveShadow is refused, zero outbox rows |
//! | t2 | pre-event flatten (system-generated order): LiveShadow enqueues nothing; \
//! |    | Paper positive control enqueues exactly one PENDING row from the same fixture; \
//! |    | the durable run-mode fence independently refuses a LIVE-SHADOW run |
//!
//! DB-backed (isolated disposable DB; skips without `MQK_DATABASE_URL`).

mod common;

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    pre_event_flatten::enqueue_pre_event_flatten_closes,
    routes,
    state::{self, DeploymentMode},
};
use mqk_execution::QtyMicros;
use tower::ServiceExt;
use uuid::Uuid;

async fn outbox_rows(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*)::bigint FROM oms_outbox")
        .fetch_one(pool)
        .await
        .expect("count outbox rows")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t1_external_signal_under_live_shadow_is_refused_with_zero_rows() {
    mqk_db::run_isolated("ls_no_order_t1", |pool| async move {
        let st = Arc::new(common::with_canonical_equity_registry(
            state::AppState::new_for_test_with_db_mode_and_broker(
                pool.clone(),
                DeploymentMode::LiveShadow,
                state::BrokerKind::Alpaca,
            ),
        ));
        // Regular NYSE session + Live WS continuity: nothing but the
        // deployment mode may be what refuses.
        st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
            last_message_id: "alpaca:ls-no-order:2024-01-08T14:00:00Z".to_string(),
            last_event_at: "2024-01-08T14:00:00Z".to_string(),
        })
        .await;
        st.set_session_clock_ts_for_test(1_704_726_000).await;

        let body = serde_json::json!({
            "signal_id": "ls-no-order-sig-1",
            "strategy_id": "swing_momentum",
            "symbol": "AAPL",
            "side": "buy",
            "qty": 10,
            "timeframe_secs": 86400,
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/strategy/signal")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = routes::build_router(st).oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");

        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "LiveShadow has no signal ingestion (Gate 1): {json}"
        );
        assert!(
            json["blockers"]
                .to_string()
                .contains("ingestion is not configured"),
            "{json}"
        );
        assert_eq!(json["accepted"], false, "{json}");
        assert_eq!(json["intent_placed"], false, "{json}");
        assert_eq!(outbox_rows(&pool).await, 0);
    })
    .await;
}

async fn seed_running_run(pool: &sqlx::PgPool, seed: &str, mode: DeploymentMode) -> Uuid {
    let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, seed.as_bytes());
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: mode.as_db_mode().to_string(),
            started_at_utc: chrono::Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "ls-no-order".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::arm_run(pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(pool, run_id).await.expect("begin_run");
    run_id
}

/// Sole test in this binary that touches process env (the blackout source
/// is env-configured); pointing it at an unreadable file makes the trigger
/// `Unavailable`, which is fail-closed flatten-required.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t2_pre_event_flatten_never_creates_an_order_under_live_shadow() {
    mqk_db::run_isolated("ls_no_order_t2", |pool| async move {
        std::env::set_var(
            mqk_daemon::event_risk_blackout::ENV_BLACKOUT_PATH,
            "/nonexistent/ls-no-order-blackout.json",
        );
        std::env::remove_var(mqk_daemon::earnings_calendar::ENV_EARNINGS_CALENDAR_PATH);
        let positions = vec![("AAPL".to_string(), QtyMicros::from_whole_units(10).unwrap())];

        // LiveShadow: nothing enqueued.
        let shadow_run =
            seed_running_run(&pool, "ls-no-order.t2.shadow", DeploymentMode::LiveShadow).await;
        let n = enqueue_pre_event_flatten_closes(
            DeploymentMode::LiveShadow,
            &pool,
            shadow_run,
            &positions,
            &|_| None,
        )
        .await;
        assert_eq!(n, 0);
        assert_eq!(outbox_rows(&pool).await, 0);

        // Durable seam alone: a Paper-configured caller against a LIVE-SHADOW
        // run is refused by the run-row mode fence.
        let n = enqueue_pre_event_flatten_closes(
            DeploymentMode::Paper,
            &pool,
            shadow_run,
            &positions,
            &|_| None,
        )
        .await;
        assert_eq!(n, 0);
        assert_eq!(outbox_rows(&pool).await, 0);

        // Positive control: the identical fixture under Paper enqueues one row.
        let paper_run =
            seed_running_run(&pool, "ls-no-order.t2.paper", DeploymentMode::Paper).await;
        let n = enqueue_pre_event_flatten_closes(
            DeploymentMode::Paper,
            &pool,
            paper_run,
            &positions,
            &|_| None,
        )
        .await;
        assert_eq!(n, 1);
        let row: (Uuid, String) = sqlx::query_as("SELECT run_id, status FROM oms_outbox")
            .fetch_one(&pool)
            .await
            .expect("one row");
        assert_eq!(row, (paper_run, "PENDING".to_string()));

        std::env::remove_var(mqk_daemon::event_risk_blackout::ENV_BLACKOUT_PATH);
    })
    .await;
}
