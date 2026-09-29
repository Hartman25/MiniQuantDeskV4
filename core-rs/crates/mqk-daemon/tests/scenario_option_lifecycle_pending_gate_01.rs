//! D3 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION): the
//! fail-closed pending-lifecycle gate.
//! `mqk_daemon::state::option_lifecycle_pending_gate::evaluate_option_lifecycle_pending_gate`
//! must report `Pending` for any option symbol with unresolved D1 evidence,
//! `Clear` once D2 has applied it, must be scoped to exactly the symbol
//! asked about, and must be restart-safe (derived from durable DB state
//! only, not in-memory).
//!
//! # Coverage
//!
//! | Test | Claim                                                               |
//! |------|-----------------------------------------------------------------------|
//! | K01  | A symbol with an unresolved OPEXC (no paired OPTRD yet) is Pending    |
//! | K02  | A symbol with no lifecycle activity at all is Clear                   |
//! | K03  | A resolved (applied) OPEXP is Clear, not Pending                       |
//! | K04  | Two DIFFERENT option symbols: one Pending, one Clear -- unrelated     |
//! |      | symbols are unaffected by each other's state                          |
//! | K05  | Applying the blocking activity via D2 clears K01's Pending symbol     |
//! |      | to Clear -- complete evidence resolves it                             |
//! | K06  | The gate answer survives a fresh pool connection against the same     |
//! |      | committed DB state -- restart-safety, since the state is DB-derived   |
//!
//! # Proof boundary
//!
//! DB-backed (port 5434 test Postgres). Load-bearing institutional proof --
//! must fail hard if `MQK_DATABASE_URL` is absent, not skip.

use chrono::Utc;
use mqk_daemon::state::option_lifecycle_apply::{
    apply_option_lifecycle_activity, OptionContractTerms,
};
use mqk_daemon::state::option_lifecycle_pending_gate::{
    evaluate_option_lifecycle_pending_gate, OptionLifecycleGateStatus,
};
use mqk_db::option_lifecycle_activity::{
    insert_option_lifecycle_activity_if_new, NewOptionLifecycleActivity,
    OptionLifecycleActivityType,
};
use sqlx::PgPool;
use uuid::Uuid;

const TEST_BROKER_ACCOUNT_ID: &str = "alpaca:test-acct";

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

async fn require_pool(url: &str) -> anyhow::Result<PgPool> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await?;
    mqk_db::migrate(&pool).await?;
    for provider in ["test-acct", "acct-a", "acct-b"] {
        mqk_db::verify_or_register_broker_account_authority(
            &pool,
            &mqk_db::BrokerAccountAuthority::new("alpaca", provider, "paper")?,
            chrono::Utc::now(),
        )
        .await?;
    }
    Ok(pool)
}

fn unique_option_symbol(label: &str) -> String {
    format!(
        "test-pending-gate-{}-{}::AAPL260619C00200000",
        label,
        Uuid::new_v4()
    )
}

fn lifecycle_activity(
    activity_id: &str,
    engine_id: &str,
    activity_type: OptionLifecycleActivityType,
    option_symbol: &str,
    activity_date: &str,
    qty_raw: &str,
) -> NewOptionLifecycleActivity {
    NewOptionLifecycleActivity {
        activity_id: activity_id.to_string(),
        broker_account_id: TEST_BROKER_ACCOUNT_ID.to_string(),
        engine_id: engine_id.to_string(),
        mode: "PAPER".to_string(),
        activity_type,
        option_symbol: Some(option_symbol.to_string()),
        underlying_symbol_raw: None,
        activity_date: activity_date.to_string(),
        qty_raw: qty_raw.to_string(),
        price_raw: None,
        net_amount_raw: "0".to_string(),
        ingested_at_utc: Utc::now(),
    }
}

/// The paired OPTRD row must share `activity_id` with its OPEXC/OPASN
/// sibling (D1: the shared id is the real correlation evidence).
fn paired_trade_activity(
    activity_id: &str,
    engine_id: &str,
    underlying_symbol_raw: &str,
    activity_date: &str,
    qty_raw: &str,
    price_raw: &str,
    net_amount_raw: &str,
) -> NewOptionLifecycleActivity {
    NewOptionLifecycleActivity {
        activity_id: activity_id.to_string(),
        broker_account_id: TEST_BROKER_ACCOUNT_ID.to_string(),
        engine_id: engine_id.to_string(),
        mode: "PAPER".to_string(),
        activity_type: OptionLifecycleActivityType::PairedTrade,
        option_symbol: None,
        underlying_symbol_raw: Some(underlying_symbol_raw.to_string()),
        activity_date: activity_date.to_string(),
        qty_raw: qty_raw.to_string(),
        price_raw: Some(price_raw.to_string()),
        net_amount_raw: net_amount_raw.to_string(),
        ingested_at_utc: Utc::now(),
    }
}

#[tokio::test]
async fn k01_unresolved_exercise_is_pending() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = format!("k01-{}", Uuid::new_v4());
    let option_symbol = unique_option_symbol("k01");
    let activity_id = format!("{engine_id}::opexc");

    let a = lifecycle_activity(
        &activity_id,
        &engine_id,
        OptionLifecycleActivityType::Exercise,
        &option_symbol,
        "2026-06-19",
        "-1",
    );
    insert_option_lifecycle_activity_if_new(&pool, &a)
        .await
        .unwrap();

    let status =
        evaluate_option_lifecycle_pending_gate(&pool, TEST_BROKER_ACCOUNT_ID, &option_symbol)
            .await
            .expect("gate must not error");

    assert!(status.must_fail_closed());
    match status {
        OptionLifecycleGateStatus::Pending {
            blocking_activity_id,
            blocking_activity_type,
        } => {
            assert_eq!(blocking_activity_id, activity_id);
            assert_eq!(
                blocking_activity_type,
                OptionLifecycleActivityType::Exercise
            );
        }
        other => panic!("K01: expected Pending, got {other:?}"),
    }
}

#[tokio::test]
async fn k02_symbol_with_no_activity_is_clear() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let option_symbol = unique_option_symbol("k02");

    let status =
        evaluate_option_lifecycle_pending_gate(&pool, TEST_BROKER_ACCOUNT_ID, &option_symbol)
            .await
            .expect("gate must not error");

    assert_eq!(status, OptionLifecycleGateStatus::Clear);
    assert!(!status.must_fail_closed());
}

#[tokio::test]
async fn k03_applied_expiration_is_clear_not_pending() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = format!("k03-{}", Uuid::new_v4());
    let option_symbol = unique_option_symbol("k03");
    let activity_id = format!("{engine_id}::opexp");

    let a = lifecycle_activity(
        &activity_id,
        &engine_id,
        OptionLifecycleActivityType::Expiration,
        &option_symbol,
        "2026-06-19",
        "-1",
    );
    insert_option_lifecycle_activity_if_new(&pool, &a)
        .await
        .unwrap();

    let terms = OptionContractTerms {
        strike_micros: 200_000_000,
        multiplier: 100,
        is_call: true,
        underlying_symbol: "AAPL".to_string(),
    };
    apply_option_lifecycle_activity(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        &activity_id,
        OptionLifecycleActivityType::Expiration,
        &terms,
        Utc::now(),
    )
    .await
    .expect("apply must not error");

    let status =
        evaluate_option_lifecycle_pending_gate(&pool, TEST_BROKER_ACCOUNT_ID, &option_symbol)
            .await
            .expect("gate must not error");

    assert_eq!(
        status,
        OptionLifecycleGateStatus::Clear,
        "K03: expiration already applied by D2 -- gate must report Clear"
    );
}

#[tokio::test]
async fn k04_unrelated_symbols_do_not_affect_each_other() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = format!("k04-{}", Uuid::new_v4());
    let pending_symbol = unique_option_symbol("k04-pending");
    let clear_symbol = unique_option_symbol("k04-clear");
    let activity_id = format!("{engine_id}::opexc");

    let a = lifecycle_activity(
        &activity_id,
        &engine_id,
        OptionLifecycleActivityType::Exercise,
        &pending_symbol,
        "2026-06-19",
        "-1",
    );
    insert_option_lifecycle_activity_if_new(&pool, &a)
        .await
        .unwrap();

    let pending_status =
        evaluate_option_lifecycle_pending_gate(&pool, TEST_BROKER_ACCOUNT_ID, &pending_symbol)
            .await
            .expect("gate must not error");
    let clear_status =
        evaluate_option_lifecycle_pending_gate(&pool, TEST_BROKER_ACCOUNT_ID, &clear_symbol)
            .await
            .expect("gate must not error");

    assert!(
        pending_status.must_fail_closed(),
        "K04: the symbol with unresolved evidence must fail closed"
    );
    assert!(
        !clear_status.must_fail_closed(),
        "K04: an unrelated symbol with zero activity of its own must remain Clear -- \
         one symbol's pending state must never leak into another's"
    );
}

#[tokio::test]
async fn k05_applying_the_blocking_activity_clears_the_gate() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = format!("k05-{}", Uuid::new_v4());
    let option_symbol = unique_option_symbol("k05");
    let activity_id = format!("{engine_id}::opexc");

    let a = lifecycle_activity(
        &activity_id,
        &engine_id,
        OptionLifecycleActivityType::Exercise,
        &option_symbol,
        "2026-06-19",
        "-1",
    );
    insert_option_lifecycle_activity_if_new(&pool, &a)
        .await
        .unwrap();

    let before =
        evaluate_option_lifecycle_pending_gate(&pool, TEST_BROKER_ACCOUNT_ID, &option_symbol)
            .await
            .expect("gate must not error");
    assert!(
        before.must_fail_closed(),
        "K05: precondition -- must start Pending before evidence completes"
    );

    // No paired OPTRD ingested -- D2 itself must remain Pending, and the
    // gate must therefore remain Pending too (evidence is genuinely
    // incomplete, not merely unattempted).
    let terms = OptionContractTerms {
        strike_micros: 200_000_000,
        multiplier: 100,
        is_call: true,
        underlying_symbol: "AAPL".to_string(),
    };
    let outcome = apply_option_lifecycle_activity(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        &activity_id,
        OptionLifecycleActivityType::Exercise,
        &terms,
        Utc::now(),
    )
    .await
    .expect("apply must not error");
    assert!(matches!(
        outcome,
        mqk_daemon::state::option_lifecycle_apply::ApplyOptionLifecycleOutcome::Pending { .. }
    ));

    let still_pending =
        evaluate_option_lifecycle_pending_gate(&pool, TEST_BROKER_ACCOUNT_ID, &option_symbol)
            .await
            .expect("gate must not error");
    assert!(
        still_pending.must_fail_closed(),
        "K05: an apply attempt that itself resolves to Pending (missing OPTRD) \
         must not clear the gate -- no synthetic evidence"
    );

    // Now ingest the paired OPTRD (SAME activity_id -- the real
    // provider-documented correlation evidence) and apply again -- genuine
    // completion.
    let trade = paired_trade_activity(
        &activity_id,
        &engine_id,
        "AAPL",
        "2026-06-19",
        "100",
        "200.00",
        "-20000",
    );
    insert_option_lifecycle_activity_if_new(&pool, &trade)
        .await
        .unwrap();

    let outcome = apply_option_lifecycle_activity(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        &activity_id,
        OptionLifecycleActivityType::Exercise,
        &terms,
        Utc::now(),
    )
    .await
    .expect("apply must not error");
    assert!(matches!(
        outcome,
        mqk_daemon::state::option_lifecycle_apply::ApplyOptionLifecycleOutcome::Applied(_)
    ));

    let after =
        evaluate_option_lifecycle_pending_gate(&pool, TEST_BROKER_ACCOUNT_ID, &option_symbol)
            .await
            .expect("gate must not error");
    assert_eq!(
        after,
        OptionLifecycleGateStatus::Clear,
        "K05: complete evidence + broker agreement (matching OPTRD) must clear the gate"
    );
}

#[tokio::test]
async fn k06_gate_answer_survives_a_fresh_pool_restart_simulation() {
    let url = require_db_url();
    let pool_a = require_pool(&url).await.expect("pool a");
    let engine_id = format!("k06-{}", Uuid::new_v4());
    let option_symbol = unique_option_symbol("k06");
    let activity_id = format!("{engine_id}::opexc");

    let a = lifecycle_activity(
        &activity_id,
        &engine_id,
        OptionLifecycleActivityType::Exercise,
        &option_symbol,
        "2026-06-19",
        "-1",
    );
    insert_option_lifecycle_activity_if_new(&pool_a, &a)
        .await
        .unwrap();
    pool_a.close().await;

    // A brand new pool/connection, as a fresh process would open on
    // restart -- no in-memory state is shared with pool_a.
    let pool_b = require_pool(&url).await.expect("pool b");
    let status =
        evaluate_option_lifecycle_pending_gate(&pool_b, TEST_BROKER_ACCOUNT_ID, &option_symbol)
            .await
            .expect("gate must not error");

    assert!(
        status.must_fail_closed(),
        "K06: pending state must be visible to a completely fresh connection -- \
         it is derived from committed DB rows, not process memory"
    );
}
