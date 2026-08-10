use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

/// PATCH 18: deadman enforcement must halt a RUNNING run when heartbeat is stale.
///
/// PRE-SOAK-DAEMON-SUPERVISOR-HALT-FENCE-CLOSURE-01: `enforce_deadman_or_halt`
/// now persists its halt through `halt_and_disarm_run_if_active`, which also
/// writes the global `sys_arm_state` singleton row atomically with the halt —
/// so, like every other test that exercises that seam
/// (`scenario_runtime_halt_fence_cas_01.rs`), this test uses
/// `mqk_db::run_isolated` (a genuinely disposable per-test database) instead
/// of the shared `MQK_DATABASE_URL` database, to avoid racing concurrently
/// running tests that assert on that same singleton row.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn deadman_enforce_halts_running_when_heartbeat_stale() {
    mqk_db::run_isolated("deadman_enforce_halts_running", |pool| async move {
        let run_id = Uuid::new_v4();
        let engine_id = format!("TEST_ENGINE_{}", Uuid::new_v4());

        mqk_db::insert_run(
            &pool,
            &mqk_db::NewRun {
                run_id,
                engine_id,
                mode: "PAPER".to_string(),
                started_at_utc: Utc::now(),
                git_hash: "TEST".to_string(),
                config_hash: "CFG_TEST".to_string(),
                config_json: serde_json::json!({}),
                host_fingerprint: "TESTHOST".to_string(),
            },
        )
        .await
        .expect("insert_run");

        mqk_db::arm_run(&pool, run_id).await.expect("arm_run");
        mqk_db::begin_run(&pool, run_id).await.expect("begin_run");

        // Set an old heartbeat manually (stale).
        let old: DateTime<Utc> = Utc::now() - Duration::seconds(3600);
        sqlx::query("update runs set last_heartbeat_utc = $1 where run_id = $2")
            .bind(old)
            .bind(run_id)
            .execute(&pool)
            .await
            .expect("seed stale heartbeat");

        // TTL 10 seconds => should expire.
        let halted = mqk_db::enforce_deadman_or_halt(&pool, run_id, 10, Utc::now())
            .await
            .expect("enforce_deadman_or_halt");
        assert!(halted, "expected deadman to halt the run");

        let r = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
        assert_eq!(r.status.as_str(), "HALTED");

        // The halt and the durable disarm are atomic (PRE-SOAK-DAEMON-
        // SUPERVISOR-HALT-FENCE-CLOSURE-01): a fenced deadman halt must also
        // leave `sys_arm_state` DISARMED, not just `runs.status`.
        let arm = mqk_db::load_arm_state(&pool)
            .await
            .expect("load_arm_state")
            .expect("arm state row must exist after a deadman halt");
        assert_eq!(arm.0, "DISARMED");
        assert_eq!(arm.1.as_deref(), Some("DeadmanExpired"));
    })
    .await;
}
