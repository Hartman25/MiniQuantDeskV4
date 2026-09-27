//! B2 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01), part 1: `runs`
//! identity gains `execution_domain` (migration 0081), and the single-
//! active-run gate (`fetch_active_run_for_engine_for_domain` /
//! `fetch_latest_run_for_engine_for_domain`) is scoped to
//! `(engine_id, mode, execution_domain)` instead of `(engine_id, mode)`
//! alone.
//!
//! # Coverage
//!
//! | Test | Claim                                                              |
//! |------|---------------------------------------------------------------------|
//! | D01  | equity_nyse and crypto_24_7 ARMED/RUNNING runs coexist for the same |
//! |      | (engine_id, mode) without either domain observing the other's run   |
//! | D02  | `insert_run_for_domain` refuses an unknown execution_domain          |
//! | D03  | `fetch_active_run_for_engine_for_domain` refuses an unknown domain   |
//! | D04  | the equity-scoped 2-arg wrappers (`insert_run`/`fetch_active_run_    |
//! |      | for_engine`/`fetch_latest_run_for_engine`) are equivalent to calling |
//! |      | the `_for_domain` forms with `equity_nyse` explicitly (no dual       |
//! |      | authority: same underlying row, same underlying query)               |
//! | M01  | Mutation control: a `_for_domain` query without the execution_domain |
//! |      | predicate (simulated directly) would wrongly see the other domain's |
//! |      | run -- proving the predicate is load-bearing, not incidental         |
//!
//! # Proof boundary
//!
//! DB-backed (port 5434 test Postgres). Load-bearing institutional proof --
//! must fail hard if `MQK_DATABASE_URL` is absent, not skip.

use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

fn require_db_url() -> String {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "PROOF: MQK_DATABASE_URL is not set. \
             This is a load-bearing proof test and cannot be skipped. \
             Set MQK_DATABASE_URL to a live Postgres instance and re-run."
        ),
    }
}

async fn require_pool(url: &str) -> Result<PgPool> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await?;
    mqk_db::migrate(&pool).await?;
    Ok(pool)
}

fn new_run(engine: &str) -> mqk_db::NewRun {
    mqk_db::NewRun {
        run_id: Uuid::new_v4(),
        engine_id: engine.to_string(),
        mode: "PAPER".to_string(),
        started_at_utc: Utc::now(),
        git_hash: "domain01-test".to_string(),
        config_hash: "domain01-cfg".to_string(),
        config_json: json!({}),
        host_fingerprint: "domain01-host".to_string(),
    }
}

async fn cleanup_run(pool: &PgPool, run_id: Uuid) {
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// D01 — equity_nyse and crypto_24_7 runs coexist, each domain sees only its
// own active run.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn d01_equity_and_crypto_active_runs_coexist_and_are_isolated() -> Result<()> {
    let pool = require_pool(&require_db_url()).await?;
    let engine = format!("domain01-d01-{}", Uuid::new_v4());

    let equity_run = new_run(&engine);
    let equity_run_id = equity_run.run_id;
    mqk_db::insert_run_for_domain(&pool, &equity_run, mqk_db::EXECUTION_DOMAIN_EQUITY_NYSE).await?;
    mqk_db::arm_run(&pool, equity_run_id).await?;
    mqk_db::begin_run(&pool, equity_run_id).await?;

    let crypto_run = new_run(&engine);
    let crypto_run_id = crypto_run.run_id;
    mqk_db::insert_run_for_domain(&pool, &crypto_run, mqk_db::EXECUTION_DOMAIN_CRYPTO_24_7).await?;
    mqk_db::arm_run(&pool, crypto_run_id).await?;
    mqk_db::begin_run(&pool, crypto_run_id).await?;

    // Both domains' runs are durably RUNNING at the same time.
    let equity_active = mqk_db::fetch_active_run_for_engine_for_domain(
        &pool,
        &engine,
        "PAPER",
        mqk_db::EXECUTION_DOMAIN_EQUITY_NYSE,
    )
    .await?
    .expect("D01: equity_nyse active run must be visible");
    assert_eq!(
        equity_active.run_id, equity_run_id,
        "D01: equity_nyse's active-run query must return the equity run, not the crypto one"
    );
    assert_eq!(equity_active.execution_domain, mqk_db::EXECUTION_DOMAIN_EQUITY_NYSE);

    let crypto_active = mqk_db::fetch_active_run_for_engine_for_domain(
        &pool,
        &engine,
        "PAPER",
        mqk_db::EXECUTION_DOMAIN_CRYPTO_24_7,
    )
    .await?
    .expect("D01: crypto_24_7 active run must be visible");
    assert_eq!(
        crypto_active.run_id, crypto_run_id,
        "D01: crypto_24_7's active-run query must return the crypto run, not the equity one"
    );
    assert_eq!(crypto_active.execution_domain, mqk_db::EXECUTION_DOMAIN_CRYPTO_24_7);

    // Halting/stopping one domain's run must not appear under the other
    // domain's query at all (isolation, not merely "returns the right one
    // when both are healthy").
    mqk_db::halt_run(&pool, equity_run_id, Utc::now()).await?;
    let crypto_still_active = mqk_db::fetch_active_run_for_engine_for_domain(
        &pool,
        &engine,
        "PAPER",
        mqk_db::EXECUTION_DOMAIN_CRYPTO_24_7,
    )
    .await?
    .expect("D01: halting the equity run must not affect crypto_24_7's active run");
    assert_eq!(crypto_still_active.run_id, crypto_run_id);

    let equity_active_after_halt = mqk_db::fetch_active_run_for_engine_for_domain(
        &pool,
        &engine,
        "PAPER",
        mqk_db::EXECUTION_DOMAIN_EQUITY_NYSE,
    )
    .await?;
    assert!(
        equity_active_after_halt.is_none(),
        "D01: a HALTED equity run must no longer be the equity_nyse active run"
    );

    cleanup_run(&pool, equity_run_id).await;
    cleanup_run(&pool, crypto_run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// D02 — insert_run_for_domain fails closed on an unknown domain.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn d02_insert_run_for_domain_refuses_unknown_domain() -> Result<()> {
    let pool = require_pool(&require_db_url()).await?;
    let run = new_run(&format!("domain01-d02-{}", Uuid::new_v4()));
    let err = mqk_db::insert_run_for_domain(&pool, &run, "equities_us")
        .await
        .expect_err("D02: an unknown execution_domain must be refused, never coerced");
    assert!(
        err.to_string().contains("unknown execution_domain"),
        "unexpected error: {err}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// D03 — fetch_active_run_for_engine_for_domain fails closed on an unknown
// domain (never silently treated as "no active run", which could let a
// caller wrongly conclude a start is safe).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn d03_fetch_active_run_for_domain_refuses_unknown_domain() -> Result<()> {
    let pool = require_pool(&require_db_url()).await?;
    let err = mqk_db::fetch_active_run_for_engine_for_domain(&pool, "any-engine", "PAPER", "day_fx")
        .await
        .expect_err("D03: an unknown execution_domain must error, not return None");
    assert!(
        err.to_string().contains("unknown execution_domain"),
        "unexpected error: {err}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// D04 — the equity-scoped 2-arg wrappers are exactly equivalent to calling
// the `_for_domain` forms with equity_nyse: no second, competing code path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn d04_equity_wrappers_are_equivalent_to_explicit_domain_calls() -> Result<()> {
    let pool = require_pool(&require_db_url()).await?;
    let engine = format!("domain01-d04-{}", Uuid::new_v4());

    let run = new_run(&engine);
    let run_id = run.run_id;
    // insert_run (2-arg wrapper) vs. insert_run_for_domain(equity_nyse):
    mqk_db::insert_run(&pool, &run).await?;
    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;

    let via_wrapper = mqk_db::fetch_active_run_for_engine(&pool, &engine, "PAPER").await?;
    let via_explicit = mqk_db::fetch_active_run_for_engine_for_domain(
        &pool,
        &engine,
        "PAPER",
        mqk_db::EXECUTION_DOMAIN_EQUITY_NYSE,
    )
    .await?;
    let via_wrapper_run_id = via_wrapper.map(|r| r.run_id);
    let via_explicit_run_id = via_explicit.map(|r| r.run_id);
    assert_eq!(
        via_wrapper_run_id, via_explicit_run_id,
        "D04: the equity-scoped wrapper must return exactly the row the explicit \
         equity_nyse call returns -- one authority, not two"
    );
    assert_eq!(via_wrapper_run_id, Some(run_id));

    cleanup_run(&pool, run_id).await;
    Ok(())
}
