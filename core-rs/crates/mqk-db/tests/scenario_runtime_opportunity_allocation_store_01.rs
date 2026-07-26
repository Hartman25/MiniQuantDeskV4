//! RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase G: durable allocation-plan store proof.
//!
//! Proves `insert_runtime_opportunity_allocation_plan` /
//! `fetch_runtime_opportunity_allocation_plan` /
//! `fetch_recent_runtime_opportunity_allocation_plans` round-trip and
//! idempotent-replay behavior, with zero writes to any portfolio/P&L/order
//! table beyond the one fixture run row each test creates for itself.
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-db --test scenario_runtime_opportunity_allocation_store_01 -- --include-ignored --test-threads=1

use chrono::{TimeZone, Utc};
use mqk_db::{
    fetch_recent_runtime_opportunity_allocation_plans, fetch_runtime_opportunity_allocation_plan,
    insert_run, insert_runtime_opportunity_allocation_plan,
    InsertRuntimeOpportunityAllocationPlanOutcome, NewRun,
    NewRuntimeOpportunityAllocationCandidate, NewRuntimeOpportunityAllocationPlan, ENV_DB_URL,
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
            "delete from sys_runtime_opportunity_allocation_candidates where plan_id in \
             (select plan_id from sys_runtime_opportunity_allocation_plans where run_id = $1)",
        )
        .bind(run_id)
        .execute(pool)
        .await;
        let _ =
            sqlx::query("delete from sys_runtime_opportunity_allocation_plans where run_id = $1")
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
            engine_id: "test-runtime-opportunity-allocation-store".to_string(),
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
        format!("test.runtime-opportunity-allocation-store-01.v1|{seed}").as_bytes(),
    )
}

fn fixed_plan_id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.runtime-opportunity-allocation-store-01.plan.v1|{seed}").as_bytes(),
    )
}

fn sample_plan(run_id: Uuid, plan_id: Uuid) -> NewRuntimeOpportunityAllocationPlan {
    NewRuntimeOpportunityAllocationPlan {
        plan_id,
        cycle_id: plan_id,
        run_id,
        mode: "shadow".to_string(),
        opportunity_artifact_id: "artifact-1".to_string(),
        source_snapshot_id: None,
        equity_micros: 100_000 * 1_000_000,
        candidate_count: 2,
        allowed_count: 1,
        gross_weight_micros: 200_000,
        net_weight_micros: 200_000,
        truth_state: "computed".to_string(),
        blockers: vec![],
        created_at_utc: Utc.with_ymd_and_hms(2099, 2, 1, 12, 5, 0).unwrap(),
        candidates: vec![
            NewRuntimeOpportunityAllocationCandidate {
                ordinal: 0,
                symbol: "AAPL".to_string(),
                strategy_id: "intraday_scalper".to_string(),
                input_score_micros: 900_000,
                target_weight_micros: 200_000,
                current_qty: 0,
                strategy_target_qty: 10,
                allocation_target_qty: 10,
                final_target_qty: 10,
                disposition: "allowed".to_string(),
                reason_code: "allocator_full_target_granted".to_string(),
                evaluation_price_micros: 100_000_000,
            },
            NewRuntimeOpportunityAllocationCandidate {
                ordinal: 1,
                symbol: "MSFT".to_string(),
                strategy_id: "intraday_scalper".to_string(),
                input_score_micros: 100_000,
                target_weight_micros: 0,
                current_qty: 0,
                strategy_target_qty: 5,
                allocation_target_qty: 0,
                final_target_qty: 0,
                disposition: "refused_no_capital".to_string(),
                reason_code: "allocator_max_positions_reached".to_string(),
                evaluation_price_micros: 50_000_000,
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

    let outcome = insert_runtime_opportunity_allocation_plan(&pool, plan.clone())
        .await
        .expect("insert should succeed");
    assert_eq!(
        outcome,
        InsertRuntimeOpportunityAllocationPlanOutcome::Inserted
    );

    let (fetched_plan, fetched_candidates) =
        fetch_runtime_opportunity_allocation_plan(&pool, plan_id)
            .await
            .expect("fetch should succeed")
            .expect("plan should exist");

    assert_eq!(fetched_plan.plan_id, plan_id);
    assert_eq!(fetched_plan.mode, "shadow");
    assert_eq!(fetched_plan.candidate_count, 2);
    assert_eq!(fetched_plan.allowed_count, 1);
    assert_eq!(fetched_candidates.len(), 2);
    assert_eq!(fetched_candidates[0].symbol, "AAPL");
    assert_eq!(fetched_candidates[0].disposition, "allowed");
    assert_eq!(fetched_candidates[1].symbol, "MSFT");
    assert_eq!(fetched_candidates[1].disposition, "refused_no_capital");

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

    let first = insert_runtime_opportunity_allocation_plan(&pool, plan.clone())
        .await
        .expect("first insert should succeed");
    assert_eq!(
        first,
        InsertRuntimeOpportunityAllocationPlanOutcome::Inserted
    );

    let second = insert_runtime_opportunity_allocation_plan(&pool, plan.clone())
        .await
        .expect("second insert should succeed as a no-op");
    assert_eq!(
        second,
        InsertRuntimeOpportunityAllocationPlanOutcome::AlreadyExists
    );

    let (_, candidates) = fetch_runtime_opportunity_allocation_plan(&pool, plan_id)
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
        insert_runtime_opportunity_allocation_plan(&pool, plan)
            .await
            .expect("insert should succeed");
    }

    let recent = fetch_recent_runtime_opportunity_allocation_plans(&pool, run_id, 10)
        .await
        .expect("fetch should succeed");

    assert_eq!(recent.len(), 2, "must not include the other run's plan");
    assert_eq!(recent[0].plan_id, plan_b.plan_id, "newest first");
    assert_eq!(recent[1].plan_id, plan_a.plan_id);

    cleanup(&pool, &[run_id, other_run_id]).await;
}
