//! Scenario: DB CHECK constraints reject invalid enum values — Patch D1
//!
//! # Invariant under test
//!
//! Every closed-enum text column in the schema has a CHECK constraint that
//! rejects out-of-range values at the DB level (PostgreSQL SQLSTATE 23514 —
//! `check_violation`), independent of any application-layer validation.
//!
//! Columns verified:
//!   - `oms_outbox.status`               (PENDING|CLAIMED|SENT|ACKED|FAILED)
//!   - `runs.mode`                       (PAPER|LIVE|BACKTEST)
//!   - `sys_arm_state.state`             (ARMED|DISARMED)
//!   - `sys_arm_state.reason`            (nullable DisarmReason variants)
//!   - `sys_reconcile_checkpoint.verdict` (CLEAN|DIRTY)
//!
//! DB-backed test. Skips if `MQK_DATABASE_URL` is not set.

use chrono::Utc;
use uuid::Uuid;

/// Returns true if `err` is a PostgreSQL CHECK constraint violation (SQLSTATE 23514).
fn is_check_violation(err: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = err {
        db_err.code().as_deref() == Some("23514")
    } else {
        false
    }
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn check_constraints_reject_invalid_enum_values() -> anyhow::Result<()> {
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

    // Create a run so FK-dependent tests have a valid parent row.
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: format!("TEST_D1_{}", Uuid::new_v4()),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG_D1".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    // -----------------------------------------------------------------------
    // 1. oms_outbox.status CHECK — value outside allowed set must be rejected
    // -----------------------------------------------------------------------

    let err = sqlx::query(
        r#"
        insert into oms_outbox (run_id, idempotency_key, order_json, status)
        values ($1, $2, '{}', 'NOT_A_STATUS')
        "#,
    )
    .bind(run_id)
    .bind(format!("ik-d1-{}", Uuid::new_v4()))
    .execute(&pool)
    .await
    .unwrap_err();

    assert!(
        is_check_violation(&err),
        "oms_outbox.status: 'NOT_A_STATUS' must fail with CHECK violation (23514); got: {err}"
    );

    // -----------------------------------------------------------------------
    // 2. runs.mode CHECK — value outside allowed set must be rejected
    // -----------------------------------------------------------------------

    let err = sqlx::query(
        r#"
        insert into runs (run_id, engine_id, mode, git_hash, config_hash, config_json, host_fingerprint)
        values ($1, $2, 'INVALID_MODE', 'h', 'c', '{}', 'host')
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(format!("TEST_D1_{}", Uuid::new_v4()))
    .execute(&pool)
    .await
    .unwrap_err();

    assert!(
        is_check_violation(&err),
        "runs.mode: 'INVALID_MODE' must fail with CHECK violation (23514); got: {err}"
    );

    // -----------------------------------------------------------------------
    // 3. sys_arm_state.state CHECK — invalid state must be rejected
    // -----------------------------------------------------------------------
    // The ON CONFLICT upsert is used so the test is idempotent regardless of
    // whether a prior sentinel row exists. PostgreSQL evaluates CHECK on the
    // resulting row value, so both INSERT and UPDATE paths are covered.

    let err = sqlx::query(
        r#"
        insert into sys_arm_state (sentinel_id, state)
        values (1, 'NOT_ARMED')
        on conflict (sentinel_id) do update set state = excluded.state
        "#,
    )
    .execute(&pool)
    .await
    .unwrap_err();

    assert!(
        is_check_violation(&err),
        "sys_arm_state.state: 'NOT_ARMED' must fail with CHECK violation (23514); got: {err}"
    );

    // -----------------------------------------------------------------------
    // 4. sys_arm_state.reason CHECK — invalid reason must be rejected
    // -----------------------------------------------------------------------

    let err = sqlx::query(
        r#"
        insert into sys_arm_state (sentinel_id, state, reason)
        values (1, 'DISARMED', 'NotARealReason')
        on conflict (sentinel_id) do update set reason = excluded.reason
        "#,
    )
    .execute(&pool)
    .await
    .unwrap_err();

    assert!(
        is_check_violation(&err),
        "sys_arm_state.reason: 'NotARealReason' must fail with CHECK violation (23514); got: {err}"
    );

    // -----------------------------------------------------------------------
    // 5. sys_reconcile_checkpoint.verdict CHECK — invalid verdict must be rejected
    // -----------------------------------------------------------------------

    let err = sqlx::query(
        r#"
        insert into sys_reconcile_checkpoint (run_id, verdict, snapshot_watermark_ms, result_hash)
        values ($1, 'MAYBE', 0, 'h')
        "#,
    )
    .bind(run_id)
    .execute(&pool)
    .await
    .unwrap_err();

    assert!(
        is_check_violation(&err),
        "sys_reconcile_checkpoint.verdict: 'MAYBE' must fail with CHECK violation (23514); got: {err}"
    );

    Ok(())
}
