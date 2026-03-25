//! CC-02A: Durable strategy suppression DB-layer proof scenarios.
//!
//! Proves that the raw DB write functions:
//! - `insert_strategy_suppression` — inserts an active suppression durably
//! - `clear_strategy_suppression`  — transitions active → cleared
//! - `fetch_strategy_suppressions` — reads all suppressions including cleared
//!
//! Specifically:
//! - a new suppression is durable and readable via the canonical fetch
//! - inserting the same suppression_id twice is silent no-op (idempotent)
//! - clearing an active suppression sets state = 'cleared' and cleared_at_utc
//! - clearing an already-cleared suppression returns false (not an error)
//! - cleared suppressions remain visible in the canonical fetch (audit trail)
//! - fetch returns newest-first ordering (started_at_utc desc)
//!
//! All tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-db --test scenario_strategy_suppressions -- --include-ignored

use chrono::Utc;
use mqk_db::{
    clear_strategy_suppression, fetch_strategy_suppressions, insert_strategy_suppression,
    InsertStrategySuppressionArgs, ENV_DB_URL,
};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn test_pool() -> sqlx::PgPool {
    let url = std::env::var(ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-db --test scenario_strategy_suppressions -- --include-ignored"
        )
    });
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to test DB");
    mqk_db::migrate(&pool).await.expect("run migrations");
    pool
}

/// Generate a unique strategy_id prefix for test isolation.
fn unique_sid(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

fn make_args(
    suppression_id: Uuid,
    strategy_id: &str,
    trigger_domain: &str,
    trigger_reason: &str,
) -> InsertStrategySuppressionArgs {
    InsertStrategySuppressionArgs {
        suppression_id,
        strategy_id: strategy_id.to_string(),
        trigger_domain: trigger_domain.to_string(),
        trigger_reason: trigger_reason.to_string(),
        started_at_utc: Utc::now(),
        note: String::new(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// CC-02A / DB-01: insert is durable and readable via the canonical fetch.
///
/// After a successful insert, `fetch_strategy_suppressions` must return the
/// row with `state = 'active'` and the correct fields.
#[tokio::test]
#[ignore]
async fn suppression_insert_is_durable_and_readable() {
    let pool = test_pool().await;
    let sid = unique_sid("dur");
    let sup_id = Uuid::new_v4();

    let args = make_args(sup_id, &sid, "operator", "manual suppression for test");
    insert_strategy_suppression(&pool, &args)
        .await
        .expect("insert_strategy_suppression failed");

    let rows = fetch_strategy_suppressions(&pool)
        .await
        .expect("fetch_strategy_suppressions failed");

    let row = rows
        .iter()
        .find(|r| r.suppression_id == sup_id)
        .expect("inserted suppression must appear in fetch result");

    assert_eq!(row.strategy_id, sid);
    assert_eq!(row.state, "active");
    assert_eq!(row.trigger_domain, "operator");
    assert_eq!(row.trigger_reason, "manual suppression for test");
    assert!(row.cleared_at_utc.is_none(), "active suppression must have no cleared_at_utc");
}

/// CC-02A / DB-02: inserting same suppression_id twice is a silent no-op.
///
/// The second insert must not fail or create a second row.  The first row's
/// data is preserved (ON CONFLICT DO NOTHING semantics).
#[tokio::test]
#[ignore]
async fn suppression_insert_idempotent_on_duplicate_id() {
    let pool = test_pool().await;
    let sid = unique_sid("idem");
    let sup_id = Uuid::new_v4();

    let args = make_args(sup_id, &sid, "risk", "first insert");
    insert_strategy_suppression(&pool, &args)
        .await
        .expect("first insert failed");

    // Second insert with same suppression_id but different fields.
    let args2 = InsertStrategySuppressionArgs {
        suppression_id: sup_id,
        strategy_id: sid.clone(),
        trigger_domain: "integrity".to_string(),
        trigger_reason: "second insert — must be ignored".to_string(),
        started_at_utc: Utc::now(),
        note: "should not appear".to_string(),
    };
    insert_strategy_suppression(&pool, &args2)
        .await
        .expect("second insert must not fail (DO NOTHING)");

    // Only one row with this suppression_id exists and it has the original fields.
    let rows = fetch_strategy_suppressions(&pool)
        .await
        .expect("fetch failed");
    let matching: Vec<_> = rows.iter().filter(|r| r.suppression_id == sup_id).collect();
    assert_eq!(matching.len(), 1, "exactly one row must exist after duplicate insert");
    assert_eq!(
        matching[0].trigger_reason, "first insert",
        "first insert's data must be preserved"
    );
}

/// CC-02A / DB-03: clearing an active suppression transitions state to 'cleared'.
///
/// After `clear_strategy_suppression` returns `Ok(true)`, the row must
/// have `state = 'cleared'` and a non-null `cleared_at_utc`.  It must
/// still be visible in `fetch_strategy_suppressions` (audit trail).
#[tokio::test]
#[ignore]
async fn suppression_clear_transitions_to_cleared() {
    let pool = test_pool().await;
    let sid = unique_sid("clr");
    let sup_id = Uuid::new_v4();

    insert_strategy_suppression(&pool, &make_args(sup_id, &sid, "operator", "will be cleared"))
        .await
        .expect("insert failed");

    let cleared_at = Utc::now();
    let was_cleared = clear_strategy_suppression(&pool, sup_id, cleared_at)
        .await
        .expect("clear_strategy_suppression failed");
    assert!(was_cleared, "clear must return true for an active suppression");

    let rows = fetch_strategy_suppressions(&pool)
        .await
        .expect("fetch failed");
    let row = rows
        .iter()
        .find(|r| r.suppression_id == sup_id)
        .expect("cleared suppression must still appear in fetch (audit trail)");

    assert_eq!(row.state, "cleared", "state must be 'cleared' after clear");
    assert!(
        row.cleared_at_utc.is_some(),
        "cleared_at_utc must be set after clear"
    );
}

/// CC-02A / DB-04: clearing an already-cleared suppression returns false.
///
/// `clear_strategy_suppression` uses `WHERE state = 'active'`, so a
/// second clear on the same suppression_id must return `Ok(false)` — not an
/// error.  This is honest accounting: no active row was found to transition.
#[tokio::test]
#[ignore]
async fn suppression_clear_returns_false_when_already_cleared() {
    let pool = test_pool().await;
    let sid = unique_sid("dbl");
    let sup_id = Uuid::new_v4();

    insert_strategy_suppression(&pool, &make_args(sup_id, &sid, "operator", "double clear"))
        .await
        .expect("insert failed");

    let cleared_at = Utc::now();
    clear_strategy_suppression(&pool, sup_id, cleared_at)
        .await
        .expect("first clear failed");

    let second = clear_strategy_suppression(&pool, sup_id, Utc::now())
        .await
        .expect("second clear must not return an error");
    assert!(
        !second,
        "second clear of an already-cleared suppression must return false"
    );
}

/// CC-02A / DB-05: clearing a non-existent suppression_id returns false.
///
/// A suppression_id that was never inserted produces `Ok(false)` — fail-closed
/// honest accounting rather than an error.
#[tokio::test]
#[ignore]
async fn suppression_clear_returns_false_for_unknown_id() {
    let pool = test_pool().await;
    let phantom = Uuid::new_v4(); // never inserted

    let result = clear_strategy_suppression(&pool, phantom, Utc::now())
        .await
        .expect("clear of unknown id must not error");
    assert!(!result, "clear of unknown suppression_id must return false");
}

/// CC-02A / DB-06: fetch returns suppressions newest-first.
///
/// Ordering is by `started_at_utc desc`.  Inserting two suppressions with
/// different timestamps must produce the newer one first in the result.
#[tokio::test]
#[ignore]
async fn suppression_fetch_ordered_newest_first() {
    let pool = test_pool().await;
    let sid = unique_sid("ord");

    let earlier = Utc::now() - chrono::Duration::seconds(60);
    let later = Utc::now();

    let old_id = Uuid::new_v4();
    let new_id = Uuid::new_v4();

    insert_strategy_suppression(
        &pool,
        &InsertStrategySuppressionArgs {
            suppression_id: old_id,
            strategy_id: format!("{sid}_old"),
            trigger_domain: "operator".to_string(),
            trigger_reason: "older".to_string(),
            started_at_utc: earlier,
            note: String::new(),
        },
    )
    .await
    .expect("insert old failed");

    insert_strategy_suppression(
        &pool,
        &InsertStrategySuppressionArgs {
            suppression_id: new_id,
            strategy_id: format!("{sid}_new"),
            trigger_domain: "operator".to_string(),
            trigger_reason: "newer".to_string(),
            started_at_utc: later,
            note: String::new(),
        },
    )
    .await
    .expect("insert new failed");

    let rows = fetch_strategy_suppressions(&pool)
        .await
        .expect("fetch failed");

    // Find positions of our two rows in the global result set.
    let pos_new = rows.iter().position(|r| r.suppression_id == new_id);
    let pos_old = rows.iter().position(|r| r.suppression_id == old_id);

    assert!(
        pos_new.is_some() && pos_old.is_some(),
        "both suppressions must be in fetch result"
    );
    assert!(
        pos_new.unwrap() < pos_old.unwrap(),
        "newer suppression must appear before older in fetch result (desc order)"
    );
}
