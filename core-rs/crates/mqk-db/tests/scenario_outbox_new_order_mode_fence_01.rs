//! LIVESHADOW-NO-ORDER-AUTHORITY-CLOSURE-01: durable run-mode fence on
//! `outbox_enqueue_new_order_for_running_run`.
//!
//! LiveShadow observes real broker truth but may never create an economic
//! order. The fence reads the locked run row's durable `mode`, so it holds
//! for every production new-order writer, across restarts, regardless of the
//! calling process's configuration.
//!
//! | Test | What it proves |
//! |------|-----------------|
//! | t1   | canonical predicate table: only PAPER / LIVE-CAPITAL permit new orders |
//! | t2   | RUNNING run in each non-order mode -> RunModeForbidsNewOrder, zero outbox rows |
//! | t3   | positive control: RUNNING PAPER (and LIVE-CAPITAL) run -> Enqueued, one PENDING row |
//! | t4   | cancel disposition: the unfenced enqueue still admits a cancel request on a \
//! |      | LIVE-SHADOW run (it can only target an existing broker-mapped order) |
//! | t5   | the mode fence is evaluated against the durable run row, not the key: an \
//! |      | idempotency key refused on a LIVE-SHADOW run stays absent |
//!
//! DB tests: `MQK_DATABASE_URL=... cargo test -p mqk-db --features testkit
//! --test scenario_outbox_new_order_mode_fence_01 -- --include-ignored`.

use chrono::Utc;
use mqk_db::{
    arm_run, begin_run, insert_run, outbox_enqueue_for_running_run,
    outbox_enqueue_new_order_for_running_run, outbox_fetch_by_idempotency_key,
    run_mode_permits_new_economic_order, NewRun, OutboxEnqueueOutcome,
};
use uuid::Uuid;

fn order_json() -> serde_json::Value {
    serde_json::json!({"symbol": "AAPL", "side": "BUY", "qty": 1})
}

async fn seed_running(pool: &sqlx::PgPool, seed: &str, mode: &str) -> Uuid {
    let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, seed.as_bytes());
    insert_run(
        pool,
        &NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: mode.to_string(),
            started_at_utc: Utc::now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await
    .expect("insert_run");
    arm_run(pool, run_id).await.expect("arm_run");
    begin_run(pool, run_id).await.expect("begin_run");
    run_id
}

async fn outbox_rows(pool: &sqlx::PgPool, run_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*)::bigint FROM oms_outbox WHERE run_id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("count outbox rows")
}

#[test]
fn t1_predicate_permits_only_paper_and_live_capital() {
    for (mode, expected) in [
        ("PAPER", true),
        ("LIVE-CAPITAL", true),
        ("LIVE-SHADOW", false),
        ("BACKTEST", false),
        ("LIVE", false),
        ("paper", false),
        ("", false),
        ("UNKNOWN", false),
    ] {
        assert_eq!(
            run_mode_permits_new_economic_order(mode),
            expected,
            "mode {mode:?}"
        );
    }
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t2_non_order_modes_refuse_with_zero_rows() {
    mqk_db::run_isolated("outbox_mode_fence_t2", |pool| async move {
        for mode in ["LIVE-SHADOW", "BACKTEST", "LIVE"] {
            let run_id = seed_running(&pool, &format!("mode-fence.t2.{mode}"), mode).await;
            let key = format!("t2-key-{mode}");
            let outcome =
                outbox_enqueue_new_order_for_running_run(&pool, run_id, &key, order_json())
                    .await
                    .expect("enqueue call");
            assert_eq!(
                outcome,
                OutboxEnqueueOutcome::RunModeForbidsNewOrder {
                    run_mode: mode.to_string()
                },
                "mode {mode}"
            );
            assert_eq!(outbox_rows(&pool, run_id).await, 0, "mode {mode}");
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t3_order_capable_modes_still_enqueue() {
    mqk_db::run_isolated("outbox_mode_fence_t3", |pool| async move {
        for mode in ["PAPER", "LIVE-CAPITAL"] {
            let run_id = seed_running(&pool, &format!("mode-fence.t3.{mode}"), mode).await;
            let key = format!("t3-key-{mode}");
            let outcome =
                outbox_enqueue_new_order_for_running_run(&pool, run_id, &key, order_json())
                    .await
                    .expect("enqueue call");
            assert_eq!(outcome, OutboxEnqueueOutcome::Enqueued, "mode {mode}");
            let row = outbox_fetch_by_idempotency_key(&pool, &key)
                .await
                .expect("fetch")
                .expect("row present");
            assert_eq!(row.status, "PENDING");
            assert_eq!(outbox_rows(&pool, run_id).await, 1);
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t4_unfenced_enqueue_remains_cancel_only_seam() {
    mqk_db::run_isolated("outbox_mode_fence_t4", |pool| async move {
        let run_id = seed_running(&pool, "mode-fence.t4", "LIVE-SHADOW").await;
        let cancel = serde_json::json!({
            "request_type": "cancel",
            "cancel_request_id": "t4-cancel",
            "target_order_id": "existing-order",
        });
        let outcome = outbox_enqueue_for_running_run(&pool, run_id, "t4-cancel", cancel)
            .await
            .expect("cancel enqueue");
        assert_eq!(outcome, OutboxEnqueueOutcome::Enqueued);
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn t5_refused_key_stays_absent_and_is_not_resurrected() {
    mqk_db::run_isolated("outbox_mode_fence_t5", |pool| async move {
        let run_id = seed_running(&pool, "mode-fence.t5", "LIVE-SHADOW").await;
        for _ in 0..2 {
            let outcome =
                outbox_enqueue_new_order_for_running_run(&pool, run_id, "t5-key", order_json())
                    .await
                    .expect("enqueue call");
            assert!(matches!(
                outcome,
                OutboxEnqueueOutcome::RunModeForbidsNewOrder { .. }
            ));
        }
        assert!(outbox_fetch_by_idempotency_key(&pool, "t5-key")
            .await
            .expect("fetch")
            .is_none());
    })
    .await;
}
