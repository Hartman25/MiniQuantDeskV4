//! Scenario: forged reconcile audit event cannot satisfy arming — Patch B1
//!
//! # Invariant under test
//!
//! Before PATCH B1, inserting an `audit_events` row with
//! `topic='reconcile', event_type='CLEAN'` was sufficient to pass
//! `arm_preflight`.  That row is trivially forgeable by anyone with DB write
//! access (or by calling the general-purpose `insert_audit_event` function).
//!
//! After PATCH B1, `arm_preflight` checks `sys_reconcile_checkpoint` instead.
//! A CLEAN audit event is stored (for audit trail purposes) but plays no part
//! in the arming gate.  Only `reconcile_checkpoint_write` — called by the
//! reconcile engine after a genuine reconcile pass — can satisfy the gate.
//!
//! Requires `MQK_DATABASE_URL`. Skips with a diagnostic message if absent.

use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn forged_audit_event_cannot_satisfy_arming() -> Result<()> {
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

    let cfg = json!({
        "arming": { "require_clean_reconcile": true },
        "risk": { "daily_loss_limit": 0.05 }
    });

    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: format!("FORGE_TEST_{}", Uuid::new_v4()),
            mode: "LIVE".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: cfg,
            host_fingerprint: "TEST|unit".to_string(),
        },
    )
    .await?;

    // Attacker inserts multiple forged CLEAN audit events via the general-purpose
    // insert_audit_event function.  This simulates an adversary with DB write access
    // who knows the old (pre-B1) reconcile gating mechanism.
    for i in 0..3 {
        mqk_db::insert_audit_event(
            &pool,
            &mqk_db::NewAuditEvent {
                event_id: Uuid::new_v4(),
                run_id,
                ts_utc: Utc::now(),
                topic: "reconcile".to_string(),
                event_type: "CLEAN".to_string(),
                payload: json!({"forged": true, "attempt": i}),
                hash_prev: None,
                hash_self: Some(format!("fake-hash-{i}")),
            },
        )
        .await?;
    }

    // Arming must still fail — forged audit events are not a valid reconcile gate.
    let err = mqk_db::arm_preflight(&pool, run_id).await.unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("requires clean reconcile"),
        "PATCH B1: forged audit events must not satisfy arming; got: {msg}"
    );

    // Confirm no checkpoint exists (the attacker did not call reconcile_checkpoint_write).
    let checkpoint = mqk_db::reconcile_checkpoint_load_latest(&pool, run_id).await?;
    assert!(
        checkpoint.is_none(),
        "no checkpoint must exist after audit-only forgery attempt"
    );

    // Only after a genuine reconcile checkpoint does arming succeed.
    mqk_db::reconcile_checkpoint_write(&pool, run_id, "CLEAN", 0, "sha256:real").await?;
    mqk_db::arm_preflight(&pool, run_id).await?;
    let r = mqk_db::fetch_run(&pool, run_id).await?;
    assert_eq!(
        r.status.as_str(),
        "ARMED",
        "arming must succeed after genuine checkpoint"
    );

    // cleanup
    mqk_db::halt_run(&pool, run_id).await?;
    Ok(())
}
