//! Scenario: arm_preflight requires a genuine reconcile checkpoint — Patch B1
//!
//! Verifies that LIVE arming is gated on `sys_reconcile_checkpoint`, NOT on
//! `audit_events`.  A CLEAN audit event (the old, forgeable path) is
//! insufficient after PATCH B1; only `reconcile_checkpoint_write` satisfies
//! the gate.

use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn arm_preflight_requires_clean_reconcile_when_configured() -> Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored");
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    mqk_db::migrate(&pool).await?;

    let run_id = Uuid::new_v4();
    let engine_id = format!("TEST_ENGINE_{}", Uuid::new_v4());

    // Config requires clean reconcile + valid LIVE risk/kill-switch settings
    let cfg = json!({
        "arming": { "require_clean_reconcile": true },
        "risk": { "daily_loss_limit": 0.02, "max_drawdown": 0.18, "flatten_on_critical": true },
        "runtime": { "deadman_file": "deadman.txt" },
        "data": { "stale_policy": "DISARM" }
    });

    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id,
            mode: "LIVE".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "TESTCFG".to_string(),
            config_json: cfg,
            host_fingerprint: "TEST|unit".to_string(),
        },
    )
    .await?;

    // Step 1: no reconcile checkpoint yet => must fail.
    let err = mqk_db::arm_preflight(&pool, run_id).await.unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("requires clean reconcile"),
        "expected reconcile gate failure (no checkpoint); got: {msg}"
    );

    // Step 2: insert a CLEAN audit event (retained for audit/logging, but NOT the gate).
    //         PATCH B1: this must no longer satisfy arming on its own.
    mqk_db::insert_audit_event(
        &pool,
        &mqk_db::NewAuditEvent {
            event_id: Uuid::new_v4(),
            run_id,
            ts_utc: Utc::now(),
            topic: "reconcile".to_string(),
            event_type: "CLEAN".to_string(),
            payload: json!({"note": "logged for audit, not a gate"}),
            hash_prev: None,
            hash_self: Some("h1".to_string()),
        },
    )
    .await?;

    // Step 3: audit event alone must still fail (PATCH B1 invariant).
    let err2 = mqk_db::arm_preflight(&pool, run_id).await.unwrap_err();
    let msg2 = format!("{err2:#}");
    assert!(
        msg2.contains("requires clean reconcile"),
        "PATCH B1: a forged audit event must not satisfy arming; got: {msg2}"
    );

    // Step 4: write a genuine reconcile checkpoint (what the reconcile engine does).
    mqk_db::reconcile_checkpoint_write(
        &pool,
        run_id,
        "CLEAN",
        1_700_000_000_000_i64, // snapshot_watermark_ms from SnapshotWatermark
        "sha256:abc123",       // result_hash (caller-computed from reconcile payload)
        Utc::now(),
    )
    .await?;

    // Step 5: now arm_preflight must pass and transition to ARMED.
    mqk_db::arm_preflight(&pool, run_id).await?;
    let r = mqk_db::fetch_run(&pool, run_id).await?;
    assert_eq!(r.status.as_str(), "ARMED");

    // cleanup
    mqk_db::halt_run(&pool, run_id).await?;
    Ok(())
}
