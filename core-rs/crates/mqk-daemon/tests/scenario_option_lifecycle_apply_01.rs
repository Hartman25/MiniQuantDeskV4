//! D2 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION): the
//! distinct, idempotent options-lifecycle accounting transaction.
//! `mqk_daemon::state::option_lifecycle_apply::apply_option_lifecycle_activity`
//! composes D1's durable evidence ledger with the existing Wave D4
//! classification model.
//!
//! # Coverage
//!
//! | Test | Claim                                                               |
//! |------|-----------------------------------------------------------------------|
//! | J01  | Expiration applies without any paired OPTRD -- a worthless lapse     |
//! |      | needs no settlement evidence                                          |
//! | J02  | Exercise with matching paired OPTRD applies with the correct paired  |
//! |      | underlying/cash effect                                                |
//! | J03  | Exercise with NO paired OPTRD is Pending, never applied              |
//! | J04  | Exercise whose paired OPTRD price disagrees with the caller-supplied |
//! |      | strike is Pending, never silently trusted either side                |
//! | J05  | A second apply call for the same lifecycle_activity_id is            |
//! |      | AlreadyApplied with zero new mutation -- true idempotency            |
//! | J06  | Fractional contracts remain Pending (the underlying classification's |
//! |      | own fail-closed rule survives composition)                            |
//!
//! # Proof boundary
//!
//! DB-backed (port 5434 test Postgres). Load-bearing institutional proof --
//! must fail hard if `MQK_DATABASE_URL` is absent, not skip.

use chrono::Utc;
use mqk_daemon::state::option_lifecycle_apply::{
    apply_option_lifecycle_activity, ApplyOptionLifecycleOutcome, OptionContractTerms,
    PendingApplyReason,
};
use mqk_db::option_lifecycle_activity::{
    insert_option_lifecycle_activity_if_new, NewOptionLifecycleActivity,
    OptionLifecycleActivityType,
};
use mqk_portfolio::option_lifecycle::LifecyclePendingReason;
use sqlx::PgPool;
use uuid::Uuid;

const TEST_BROKER_ACCOUNT_ID: &str = "test-alpaca-key-id";

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
    Ok(pool)
}

fn test_engine_id(label: &str) -> String {
    format!("test-option-lifecycle-apply-{}-{}", label, Uuid::new_v4())
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
/// sibling (D1: the shared id is the real correlation evidence) and
/// carries `underlying_symbol_raw`, never `option_symbol`.
fn optrd(
    activity_id: &str,
    engine_id: &str,
    underlying_symbol_raw: &str,
    activity_date: &str,
    qty_raw: &str,
    price_raw: &str,
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
        net_amount_raw: "0".to_string(),
        ingested_at_utc: Utc::now(),
    }
}

fn call_terms(strike_micros: i64) -> OptionContractTerms {
    OptionContractTerms {
        strike_micros,
        multiplier: 100,
        is_call: true,
        underlying_symbol: "AAPL".to_string(),
    }
}

#[tokio::test]
async fn j01_expiration_applies_without_any_paired_trade() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("j01");
    let option_symbol = format!("{engine_id}::AAPL260619C00200000");
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

    let outcome = apply_option_lifecycle_activity(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        &activity_id,
        OptionLifecycleActivityType::Expiration,
        &call_terms(200_000_000),
        Utc::now(),
    )
    .await
    .expect("apply must not error");

    match outcome {
        ApplyOptionLifecycleOutcome::Applied(effect) => {
            assert_eq!(effect.underlying_shares_delivered_raw, None);
            assert_eq!(effect.cash_effect_micros, None);
        }
        other => panic!("expected Applied, got {other:?}"),
    }
}

#[tokio::test]
async fn j02_exercise_with_matching_paired_trade_applies_with_correct_effect() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("j02");
    let option_symbol = format!("{engine_id}::AAPL260619C00200000");
    let activity_id = format!("{engine_id}::opexc");

    let a = lifecycle_activity(
        &activity_id,
        &engine_id,
        OptionLifecycleActivityType::Exercise,
        &option_symbol,
        "2026-06-19",
        "-2",
    );
    insert_option_lifecycle_activity_if_new(&pool, &a)
        .await
        .unwrap();
    let trade = optrd(
        &activity_id,
        &engine_id,
        "AAPL",
        "2026-06-19",
        "200",
        "200.00",
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
        // 2 contracts * 100 multiplier = 200 shares; $200 strike * 200 = $40,000.
        &call_terms(200_000_000),
        Utc::now(),
    )
    .await
    .expect("apply must not error");

    match outcome {
        ApplyOptionLifecycleOutcome::Applied(effect) => {
            assert_eq!(
                effect.underlying_shares_delivered_raw.as_deref(),
                Some("200")
            );
            assert_eq!(effect.cash_effect_micros, Some(40_000_000_000));
            assert_eq!(effect.option_contracts_removed_raw, "2");
        }
        other => panic!("expected Applied, got {other:?}"),
    }
}

#[tokio::test]
async fn j03_exercise_without_paired_trade_is_pending() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("j03");
    let option_symbol = format!("{engine_id}::AAPL260619C00200000");
    let activity_id = format!("{engine_id}::opexc");

    let a = lifecycle_activity(
        &activity_id,
        &engine_id,
        OptionLifecycleActivityType::Exercise,
        &option_symbol,
        "2026-06-19",
        "-2",
    );
    insert_option_lifecycle_activity_if_new(&pool, &a)
        .await
        .unwrap();

    let outcome = apply_option_lifecycle_activity(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        &activity_id,
        OptionLifecycleActivityType::Exercise,
        &call_terms(200_000_000),
        Utc::now(),
    )
    .await
    .expect("apply must not error");

    assert_eq!(
        outcome,
        ApplyOptionLifecycleOutcome::Pending {
            reason: PendingApplyReason::PairedTradeEvidenceMissing
        },
        "J03: no OPTRD evidence has been ingested yet -- must be Pending, never applied"
    );
}

#[tokio::test]
async fn j04_paired_trade_strike_mismatch_is_pending_never_trusted() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("j04");
    let option_symbol = format!("{engine_id}::AAPL260619C00200000");
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
    // Broker's own paired trade reports a DIFFERENT price than the caller's
    // canonical strike -- a genuine anomaly.
    let trade = optrd(
        &activity_id,
        &engine_id,
        "AAPL",
        "2026-06-19",
        "100",
        "199.50",
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
        &call_terms(200_000_000), // caller believes strike is $200
        Utc::now(),
    )
    .await
    .expect("apply must not error");

    match outcome {
        ApplyOptionLifecycleOutcome::Pending {
            reason:
                PendingApplyReason::PairedTradeStrikeMismatch {
                    strike_micros,
                    optrd_price_raw,
                },
        } => {
            assert_eq!(strike_micros, 200_000_000);
            assert_eq!(optrd_price_raw, "199.50");
        }
        other => panic!("expected PairedTradeStrikeMismatch, got {other:?}"),
    }
}

#[tokio::test]
async fn j05_second_apply_call_is_already_applied_zero_new_mutation() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("j05");
    let option_symbol = format!("{engine_id}::AAPL260619C00200000");
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

    let first = apply_option_lifecycle_activity(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        &activity_id,
        OptionLifecycleActivityType::Expiration,
        &call_terms(200_000_000),
        Utc::now(),
    )
    .await
    .expect("first apply must not error");
    assert!(matches!(first, ApplyOptionLifecycleOutcome::Applied(_)));

    let second = apply_option_lifecycle_activity(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        &activity_id,
        OptionLifecycleActivityType::Expiration,
        &call_terms(200_000_000),
        Utc::now(),
    )
    .await
    .expect("second apply must not error");
    assert!(
        matches!(second, ApplyOptionLifecycleOutcome::AlreadyApplied(_)),
        "J05: a second apply for the same lifecycle_activity_id must be AlreadyApplied, got \
         {second:?}"
    );
}

#[tokio::test]
async fn j06_fractional_contracts_remain_pending_through_composition() {
    let url = require_db_url();
    let pool = require_pool(&url).await.expect("pool");
    let engine_id = test_engine_id("j06");
    let option_symbol = format!("{engine_id}::AAPL260619C00200000");
    let activity_id = format!("{engine_id}::opexp");

    let a = lifecycle_activity(
        &activity_id,
        &engine_id,
        OptionLifecycleActivityType::Expiration,
        &option_symbol,
        "2026-06-19",
        "-1.5",
    );
    insert_option_lifecycle_activity_if_new(&pool, &a)
        .await
        .unwrap();

    let outcome = apply_option_lifecycle_activity(
        &pool,
        TEST_BROKER_ACCOUNT_ID,
        &engine_id,
        "PAPER",
        &activity_id,
        OptionLifecycleActivityType::Expiration,
        &call_terms(200_000_000),
        Utc::now(),
    )
    .await
    .expect("apply must not error");

    assert_eq!(
        outcome,
        ApplyOptionLifecycleOutcome::Pending {
            reason: PendingApplyReason::Lifecycle(LifecyclePendingReason::FractionalContracts)
        }
    );
}
