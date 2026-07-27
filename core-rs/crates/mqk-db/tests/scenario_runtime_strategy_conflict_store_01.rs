//! MULTI-STRATEGY-CONFLICT-POLICY-01 Phase C: durable conflict-plan store proof.
//!
//! Proves `insert_runtime_strategy_conflict_plan` /
//! `fetch_runtime_strategy_conflict_plan` /
//! `fetch_recent_runtime_strategy_conflict_plans` round-trip and
//! idempotent-replay behavior, with zero writes to any portfolio/P&L/order
//! table beyond the one fixture run row each test creates for itself.
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-db --test scenario_runtime_strategy_conflict_store_01 -- --include-ignored --test-threads=1

use chrono::{TimeZone, Utc};
use mqk_db::{
    fetch_recent_runtime_strategy_conflict_plans, fetch_runtime_strategy_conflict_plan, insert_run,
    insert_runtime_strategy_conflict_plan, InsertRuntimeStrategyConflictPlanOutcome, NewRun,
    NewRuntimeStrategyConflictCandidate, NewRuntimeStrategyConflictPlan, ENV_DB_URL,
};
use uuid::Uuid;

async fn test_pool() -> anyhow::Result<sqlx::PgPool> {
    if std::env::var(ENV_DB_URL).is_err() {
        anyhow::bail!("SKIP: requires MQK_DATABASE_URL");
    }
    let pool = mqk_db::testkit_db_pool().await?;
    Ok(pool)
}

async fn cleanup(pool: &sqlx::PgPool, run_ids: &[Uuid]) {
    for run_id in run_ids {
        let _ = sqlx::query(
            "delete from sys_runtime_strategy_conflict_candidates where plan_id in \
             (select plan_id from sys_runtime_strategy_conflict_plans where run_id = $1)",
        )
        .bind(run_id)
        .execute(pool)
        .await;
        let _ = sqlx::query("delete from sys_runtime_strategy_conflict_plans where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("delete from runs where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
    }
}

async fn fixture_run(pool: &sqlx::PgPool, run_id: Uuid) {
    insert_run(
        pool,
        &NewRun {
            run_id,
            engine_id: "test-runtime-strategy-conflict-store".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 12, 0, 0).unwrap(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("fixture run insert should succeed");
}

fn fixed_run_id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.runtime-strategy-conflict-store-01.v1|{seed}").as_bytes(),
    )
}

fn fixed_plan_id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.runtime-strategy-conflict-store-01.plan.v1|{seed}").as_bytes(),
    )
}

fn sample_plan(run_id: Uuid, plan_id: Uuid) -> NewRuntimeStrategyConflictPlan {
    NewRuntimeStrategyConflictPlan {
        plan_id,
        cycle_id: plan_id,
        run_id,
        mode: "shadow".to_string(),
        market_date: "2099-02-01".to_string(),
        policy_schema_version: "multi-strategy-conflict-policy-v1".to_string(),
        symbol_group_count: 1,
        candidate_count: 2,
        selected_count: 1,
        refused_count: 1,
        truth_state: "computed".to_string(),
        blockers: vec![],
        created_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 12, 5, 0).unwrap(),
        candidates: vec![
            NewRuntimeStrategyConflictCandidate {
                ordinal: 0,
                symbol: "AAPL".to_string(),
                strategy_id: "strategy_a".to_string(),
                timeframe_secs: 300,
                side: "sell".to_string(),
                qty: 5,
                current_qty: 20,
                proposed_target_qty: Some(15),
                bar_end_ts: None,
                selected: true,
                disposition: "selected".to_string(),
                reason_code: "risk_reducing_candidate_selected".to_string(),
            },
            NewRuntimeStrategyConflictCandidate {
                ordinal: 1,
                symbol: "AAPL".to_string(),
                strategy_id: "strategy_b".to_string(),
                timeframe_secs: 300,
                side: "buy".to_string(),
                qty: 10,
                current_qty: 20,
                proposed_target_qty: Some(30),
                bar_end_ts: Some(1_000),
                selected: false,
                disposition: "not_selected".to_string(),
                reason_code: "increase_overridden_by_risk_reduction".to_string(),
            },
        ],
    }
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn insert_and_fetch_round_trips() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("round_trip");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_id = fixed_plan_id("round_trip");
    let plan = sample_plan(run_id, plan_id);

    let outcome = insert_runtime_strategy_conflict_plan(&pool, plan.clone())
        .await
        .expect("insert should succeed");
    assert_eq!(outcome, InsertRuntimeStrategyConflictPlanOutcome::Inserted);

    let (fetched_plan, fetched_candidates) = fetch_runtime_strategy_conflict_plan(&pool, plan_id)
        .await
        .expect("fetch should succeed")
        .expect("plan should exist");

    assert_eq!(fetched_plan.plan_id, plan_id);
    assert_eq!(fetched_plan.mode, "shadow");
    assert_eq!(fetched_plan.candidate_count, 2);
    assert_eq!(fetched_plan.selected_count, 1);
    assert_eq!(
        fetched_candidates.len(),
        2,
        "candidate order persists canonically"
    );
    assert_eq!(fetched_candidates[0].ordinal, 0);
    assert_eq!(fetched_candidates[0].disposition, "selected");
    assert!(fetched_candidates[0].selected);
    assert_eq!(fetched_candidates[1].ordinal, 1);
    assert_eq!(fetched_candidates[1].disposition, "not_selected");
    assert!(!fetched_candidates[1].selected);

    cleanup(&pool, &[run_id]).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn re_persisting_same_plan_id_is_idempotent_no_op() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("idempotent");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_id = fixed_plan_id("idempotent");
    let plan = sample_plan(run_id, plan_id);

    let first = insert_runtime_strategy_conflict_plan(&pool, plan.clone())
        .await
        .expect("first insert should succeed");
    assert_eq!(first, InsertRuntimeStrategyConflictPlanOutcome::Inserted);

    let second = insert_runtime_strategy_conflict_plan(&pool, plan.clone())
        .await
        .expect("second insert should succeed as a no-op");
    assert_eq!(
        second,
        InsertRuntimeStrategyConflictPlanOutcome::AlreadyExists
    );

    let (_, candidates) = fetch_runtime_strategy_conflict_plan(&pool, plan_id)
        .await
        .expect("fetch should succeed")
        .expect("plan should exist");
    assert_eq!(
        candidates.len(),
        2,
        "no duplicate candidate rows from replay"
    );

    cleanup(&pool, &[run_id]).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn changed_cycle_creates_a_distinct_plan() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("distinct_cycle");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_a = sample_plan(run_id, fixed_plan_id("distinct_cycle_a"));
    let mut plan_b = sample_plan(run_id, fixed_plan_id("distinct_cycle_b"));
    plan_b.market_date = "2099-02-02".to_string();

    insert_runtime_strategy_conflict_plan(&pool, plan_a.clone())
        .await
        .expect("insert a should succeed");
    insert_runtime_strategy_conflict_plan(&pool, plan_b.clone())
        .await
        .expect("insert b should succeed");

    let recent = fetch_recent_runtime_strategy_conflict_plans(&pool, run_id, 10)
        .await
        .expect("fetch should succeed");
    assert_eq!(recent.len(), 2, "distinct cycles produce distinct rows");

    cleanup(&pool, &[run_id]).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn recent_plans_ordered_newest_first_and_scoped_to_run() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("recent_ordering");
    let other_run_id = fixed_run_id("recent_ordering_other");
    cleanup(&pool, &[run_id, other_run_id]).await;
    fixture_run(&pool, run_id).await;
    fixture_run(&pool, other_run_id).await;

    let mut plan_a = sample_plan(run_id, fixed_plan_id("recent_a"));
    plan_a.created_at_utc = Utc.with_ymd_and_hms(2099, 2, 1, 12, 0, 0).unwrap();
    let mut plan_b = sample_plan(run_id, fixed_plan_id("recent_b"));
    plan_b.created_at_utc = Utc.with_ymd_and_hms(2099, 2, 1, 12, 10, 0).unwrap();
    let mut plan_other_run = sample_plan(other_run_id, fixed_plan_id("recent_other_run"));
    plan_other_run.created_at_utc = Utc.with_ymd_and_hms(2099, 2, 1, 12, 20, 0).unwrap();

    for plan in [plan_a.clone(), plan_b.clone(), plan_other_run.clone()] {
        insert_runtime_strategy_conflict_plan(&pool, plan)
            .await
            .expect("insert should succeed");
    }

    let recent = fetch_recent_runtime_strategy_conflict_plans(&pool, run_id, 10)
        .await
        .expect("fetch should succeed");

    assert_eq!(recent.len(), 2, "must not include the other run's plan");
    assert_eq!(recent[0].plan_id, plan_b.plan_id, "newest first");
    assert_eq!(recent[1].plan_id, plan_a.plan_id);

    cleanup(&pool, &[run_id, other_run_id]).await;
}
