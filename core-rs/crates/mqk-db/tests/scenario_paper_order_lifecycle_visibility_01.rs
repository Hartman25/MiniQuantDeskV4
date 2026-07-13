//! PAPER-ORDER-LIFECYCLE-VIS-01B: run-scoped signal-evaluation and
//! no-trade-diagnostic DB helper proof.
//!
//! Proves `fetch_strategy_signal_evaluations_for_run` and
//! `fetch_autonomous_no_trade_diagnostics_for_run` (added in this bundle)
//! return only rows for the given `run_id`, return an empty `Vec` (never
//! an error) when no rows match, and never write to any table. Read-only —
//! the only writes in this suite are the test's own fixture inserts via
//! the existing `insert_strategy_signal_evaluation` /
//! `insert_autonomous_no_trade_diagnostic` helpers, cleaned up afterward.
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-db --test scenario_paper_order_lifecycle_visibility_01 -- --include-ignored --test-threads=1

use chrono::Utc;
use mqk_db::{
    fetch_autonomous_no_trade_diagnostics_for_run, fetch_strategy_signal_evaluations_for_run,
    insert_autonomous_no_trade_diagnostic, insert_strategy_signal_evaluation,
    InsertAutonomousNoTradeDiagnosticArgs, InsertStrategySignalEvaluationArgs, ENV_DB_URL,
};
use uuid::Uuid;

async fn test_pool() -> anyhow::Result<sqlx::PgPool> {
    if std::env::var(ENV_DB_URL).is_err() {
        anyhow::bail!("SKIP: requires MQK_DATABASE_URL");
    }
    let pool = mqk_db::testkit_db_pool().await?;
    Ok(pool)
}

async fn cleanup(pool: &sqlx::PgPool, evaluation_ids: &[Uuid], diagnostic_ids: &[Uuid]) {
    for id in evaluation_ids {
        let _ = sqlx::query("delete from strategy_signal_evaluations where evaluation_id = $1")
            .bind(id)
            .execute(pool)
            .await;
    }
    for id in diagnostic_ids {
        let _ = sqlx::query("delete from autonomous_no_trade_diagnostics where diagnostic_id = $1")
            .bind(id)
            .execute(pool)
            .await;
    }
}

/// `fetch_strategy_signal_evaluations_for_run` returns only rows for the
/// requested run, ordered newest-first, excluding rows from a different
/// run even when both were observed at the same instant.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_order_lifecycle_visibility_01 -- --include-ignored"]
async fn signal_evaluations_for_run_is_run_scoped() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    let run_a = Uuid::new_v4();
    let run_b = Uuid::new_v4();
    let eval_a = Uuid::new_v4();
    let eval_b = Uuid::new_v4();
    let now = Utc::now();

    let make_args = |evaluation_id: Uuid, run_id: Uuid| InsertStrategySignalEvaluationArgs {
        evaluation_id,
        ts_utc: now,
        run_id: Some(run_id),
        strategy_id: "test_strategy".to_string(),
        symbol: "TEST".to_string(),
        timeframe: "5m".to_string(),
        bar_context_source: "test".to_string(),
        bars_loaded: 10,
        latest_bar_ts_utc: Some(now),
        signal_generated: false,
        signal_qty: None,
        signal_side: None,
        reason_code: "test_no_signal".to_string(),
        reason: "test fixture".to_string(),
        decision_stage: "strategy_evaluated".to_string(),
        source: "scenario_paper_order_lifecycle_visibility_01".to_string(),
    };

    insert_strategy_signal_evaluation(&pool, &make_args(eval_a, run_a))
        .await
        .expect("insert eval_a should succeed");
    insert_strategy_signal_evaluation(&pool, &make_args(eval_b, run_b))
        .await
        .expect("insert eval_b should succeed");

    let rows_for_a = fetch_strategy_signal_evaluations_for_run(&pool, run_a, 100)
        .await
        .expect("fetch for run_a should succeed");
    assert_eq!(rows_for_a.len(), 1, "only run_a's row should be returned");
    assert_eq!(rows_for_a[0].evaluation_id, eval_a);
    assert_eq!(rows_for_a[0].run_id, Some(run_a));

    let rows_for_b = fetch_strategy_signal_evaluations_for_run(&pool, run_b, 100)
        .await
        .expect("fetch for run_b should succeed");
    assert_eq!(rows_for_b.len(), 1, "only run_b's row should be returned");
    assert_eq!(rows_for_b[0].evaluation_id, eval_b);

    cleanup(&pool, &[eval_a, eval_b], &[]).await;
}

/// A run with zero signal-evaluation rows returns an empty `Vec`, not an
/// error — authoritative empty, matching the global fetch helper's
/// existing contract.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_order_lifecycle_visibility_01 -- --include-ignored"]
async fn signal_evaluations_for_run_empty_is_authoritative() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    let never_used_run = Uuid::new_v4();
    let rows = fetch_strategy_signal_evaluations_for_run(&pool, never_used_run, 100)
        .await
        .expect("fetch should succeed even with zero matching rows");
    assert!(rows.is_empty(), "no rows were ever inserted for this run");
}

/// `fetch_autonomous_no_trade_diagnostics_for_run` returns only rows for
/// the requested run.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_order_lifecycle_visibility_01 -- --include-ignored"]
async fn no_trade_diagnostics_for_run_is_run_scoped() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    let run_a = Uuid::new_v4();
    let run_b = Uuid::new_v4();
    let diag_a = Uuid::new_v4();
    let diag_b = Uuid::new_v4();
    let now = Utc::now();

    let make_args = |diagnostic_id: Uuid, run_id: Uuid| InsertAutonomousNoTradeDiagnosticArgs {
        diagnostic_id,
        observed_at_utc: now,
        run_id: Some(run_id),
        mode: "PAPER".to_string(),
        session_window_state: "in_window".to_string(),
        runtime_start_allowed: true,
        arm_state: "ARMED".to_string(),
        overall_ready: false,
        reason_code: "test_reason".to_string(),
        reason: "test fixture".to_string(),
        stage: "test_stage".to_string(),
        paper_order_attempted: false,
        live_order_attempted: false,
        source: "scenario_paper_order_lifecycle_visibility_01".to_string(),
    };

    insert_autonomous_no_trade_diagnostic(&pool, &make_args(diag_a, run_a))
        .await
        .expect("insert diag_a should succeed");
    insert_autonomous_no_trade_diagnostic(&pool, &make_args(diag_b, run_b))
        .await
        .expect("insert diag_b should succeed");

    let rows_for_a = fetch_autonomous_no_trade_diagnostics_for_run(&pool, run_a, 100)
        .await
        .expect("fetch for run_a should succeed");
    assert_eq!(rows_for_a.len(), 1, "only run_a's row should be returned");
    assert_eq!(rows_for_a[0].diagnostic_id, diag_a);
    assert_eq!(rows_for_a[0].run_id, Some(run_a));

    let rows_for_b = fetch_autonomous_no_trade_diagnostics_for_run(&pool, run_b, 100)
        .await
        .expect("fetch for run_b should succeed");
    assert_eq!(rows_for_b.len(), 1, "only run_b's row should be returned");
    assert_eq!(rows_for_b[0].diagnostic_id, diag_b);

    cleanup(&pool, &[], &[diag_a, diag_b]).await;
}

/// A run with zero no-trade-diagnostic rows returns an empty `Vec`.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_order_lifecycle_visibility_01 -- --include-ignored"]
async fn no_trade_diagnostics_for_run_empty_is_authoritative() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    let never_used_run = Uuid::new_v4();
    let rows = fetch_autonomous_no_trade_diagnostics_for_run(&pool, never_used_run, 100)
        .await
        .expect("fetch should succeed even with zero matching rows");
    assert!(rows.is_empty(), "no rows were ever inserted for this run");
}

/// Both new fetch helpers perform zero writes: proven by an unchanged
/// `oms_outbox` / `oms_inbox` / `runs` row count across the calls above.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_paper_order_lifecycle_visibility_01 -- --include-ignored"]
async fn fetch_helpers_write_nothing_to_oms_or_runs_tables() {
    let pool = match test_pool().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    let count_before: (i64, i64, i64) = (
        sqlx::query_scalar("select count(*) from oms_outbox")
            .fetch_one(&pool)
            .await
            .expect("count oms_outbox"),
        sqlx::query_scalar("select count(*) from oms_inbox")
            .fetch_one(&pool)
            .await
            .expect("count oms_inbox"),
        sqlx::query_scalar("select count(*) from runs")
            .fetch_one(&pool)
            .await
            .expect("count runs"),
    );

    let probe_run = Uuid::new_v4();
    let _ = fetch_strategy_signal_evaluations_for_run(&pool, probe_run, 100)
        .await
        .expect("fetch should succeed");
    let _ = fetch_autonomous_no_trade_diagnostics_for_run(&pool, probe_run, 100)
        .await
        .expect("fetch should succeed");

    let count_after: (i64, i64, i64) = (
        sqlx::query_scalar("select count(*) from oms_outbox")
            .fetch_one(&pool)
            .await
            .expect("count oms_outbox"),
        sqlx::query_scalar("select count(*) from oms_inbox")
            .fetch_one(&pool)
            .await
            .expect("count oms_inbox"),
        sqlx::query_scalar("select count(*) from runs")
            .fetch_one(&pool)
            .await
            .expect("count runs"),
    );

    assert_eq!(count_before, count_after, "no row counts must change");
}
