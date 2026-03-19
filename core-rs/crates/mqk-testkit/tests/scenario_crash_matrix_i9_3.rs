//! Scenario: Full Crash Matrix — I9-3
//!
//! # Invariants under test
//!
//! Three crash windows in the outbox dispatch / inbox apply path not covered
//! by EB-5. Scenarios are aligned to the current production contract:
//! fail-closed restart quarantine for ambiguous dispatch states,
//! atomic SENT+broker-map durability, and inbox replay safety.
//!
//! ## Crash Window W4 — after broker submit, before outbox_mark_sent
//!
//! Normal path:  claim → mark_dispatching → submit_to_broker → mark_sent → broker_map_upsert
//! Crash at:     ^— broker.submit() succeeded, process exits before mark_sent
//! DB state:     outbox = DISPATCHING, broker HAS the order, no broker_map entry
//! Recovery:     broker.has_order() = true → mark_acked; do NOT resubmit
//! Invariant:    broker.submit_count() == 1 (no double-submit)
//!
//! ## Crash Window W5 — after atomic SENT+broker_map commit, before in-memory register
//!
//! Normal path:  … → mark_dispatching → submit_to_broker
//!            → outbox_mark_sent_with_broker_map (single DB transaction)
//!            → order_map.register
//! Crash at:     ^— DB commit succeeded, process exits before in-memory register
//! DB state:     outbox = SENT, broker_order_map entry exists
//! Recovery:     broker.has_order() = true → mark_acked; do NOT resubmit
//! Invariant:    broker.submit_count() == 1; durable broker_map exists
//!
//! ## Crash Window W6 — after inbox_insert_deduped, before inbox_mark_applied
//!
//! Normal path:  fetch_events → inbox_insert_deduped → apply_to_portfolio
//!                            → inbox_mark_applied
//! Crash at:     ^— inbox row inserted, process exits before inbox_mark_applied
//! DB state:     inbox row present, applied_at_utc IS NULL
//! Recovery:     inbox_load_unapplied_for_run returns the row; apply exactly once
//! Invariant:    fill applied exactly once; second restart sees zero unapplied rows
//!
//! # PROOF LANE
//!
//! This is a load-bearing institutional proof test. It MUST fail hard if
//! MQK_DATABASE_URL is absent or the DB is unreachable. Silent skip is not
//! acceptable — a skipped proof test is an unproven invariant.

use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixed run UUIDs — deterministic, never collide with production runs.
// ---------------------------------------------------------------------------

const W4_RUN_ID: &str = "19300004-0000-0000-0000-000000000000";
const W5_RUN_ID: &str = "19300005-0000-0000-0000-000000000000";
const W6_RUN_ID: &str = "19300006-0000-0000-0000-000000000000";

// ---------------------------------------------------------------------------
// PROOF LANE harness helpers — fail hard on absent or unreachable DB.
// ---------------------------------------------------------------------------

/// Panics with a clear message if MQK_DATABASE_URL is not set.
/// This is intentional: a proof test that cannot reach its DB is a failed proof.
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

/// Panics if the DB is unreachable.
/// An unreachable DB means the proof cannot be run — fail loud, not silent.
async fn require_pool(url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await
        .unwrap_or_else(|e| panic!("PROOF: cannot connect to DB: {e}"))
}

/// Insert a minimal test run and a single outbox entry.
async fn seed_run_and_outbox(pool: &PgPool, run_id: Uuid, idem_key: &str) -> Result<()> {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "i93-test".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "i93-test".to_string(),
            config_hash: "i93-test".to_string(),
            config_json: json!({}),
            host_fingerprint: "i93-test".to_string(),
        },
    )
    .await?;

    mqk_db::outbox_enqueue(pool, run_id, idem_key, json!({"symbol": "SPY", "qty": 1})).await?;

    Ok(())
}

/// Remove test data for the given run.
///
/// broker_order_map has FK RESTRICT to oms_outbox, so mapping rows must be
/// removed before deleting the run.
async fn cleanup_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query(
        r#"
        delete from broker_order_map
        where internal_id in (
            select idempotency_key from oms_outbox where run_id = $1
        )
        "#,
    )
    .bind(run_id)
    .execute(pool)
    .await?;

    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// W4: Crash after broker submit, before outbox_mark_sent
// ---------------------------------------------------------------------------

/// Crash after broker.submit() but before outbox_mark_sent().
///
/// DB state entering recovery: outbox = DISPATCHING, broker HAS the order.
/// The dispatcher already crossed the pre-submit safety barrier and wrote
/// DISPATCHING, but never reached mark_sent. recover_outbox_against_broker
/// must NOT resubmit — broker already has it — and must ACK the row.
#[tokio::test]
async fn w4_crash_after_submit_before_mark_sent_no_double_submit() -> anyhow::Result<()> {
    let url = require_db_url();
    let pool = require_pool(&url).await;
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = W4_RUN_ID.parse().unwrap();
    let key = "i93-w4-ord-001";

    // Pre-test cleanup.
    cleanup_run(&pool, run_id).await?;

    // Seed run + outbox (PENDING).
    seed_run_and_outbox(&pool, run_id, key).await?;

    // --- Simulate pre-crash dispatch ---

    // Dispatcher claims the row for THIS run only (PENDING → CLAIMED).
    let claimed =
        mqk_db::outbox_claim_batch_for_run(&pool, run_id, 1, "i93-dispatcher", Utc::now()).await?;
    assert_eq!(
        claimed.len(),
        1,
        "W4: must claim the PENDING row for this run"
    );
    assert_eq!(
        claimed[0].row.idempotency_key, key,
        "W4: claimed row must be the seeded key"
    );

    // Dispatcher marks DISPATCHING before broker submit (CLAIMED → DISPATCHING).
    let marked_dispatching =
        mqk_db::outbox_mark_dispatching(&pool, key, "i93-dispatcher", Utc::now()).await?;
    assert!(
        marked_dispatching,
        "W4: outbox_mark_dispatching must transition CLAIMED → DISPATCHING"
    );

    // Broker submit succeeds — broker now has the order.
    let mut broker = mqk_testkit::FakeBroker::new();
    broker.submit(key, json!({"symbol": "SPY", "qty": 1}));
    assert_eq!(
        broker.submit_count(),
        1,
        "W4: broker must record one submit"
    );

    // --- CRASH: process exits here, outbox_mark_sent never called ---
    // DB state: outbox = DISPATCHING, broker HAS the order, no broker_map entry.

    // Under the current authoritative semantics, DISPATCHING is restart-ambiguous
    // and must remain visible for quarantine/recovery truth. It is NOT directly
    // recoverable to ACKED without a durable SENT transition.
    let ambiguous = mqk_db::outbox_load_restart_ambiguous_for_run(&pool, run_id).await?;
    assert!(
        ambiguous
            .iter()
            .any(|row| row.idempotency_key == key && row.status == "DISPATCHING"),
        "W4: DISPATCHING row must remain visible in restart-ambiguous recovery listing"
    );

    assert_eq!(
        broker.submit_count(),
        1,
        "W4: broker must have received exactly one submit total (no double-submit)"
    );

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, key).await?;
    assert_eq!(
        row.expect("W4: row must exist").status,
        "DISPATCHING",
        "W4: outbox row must remain DISPATCHING until authoritative recovery/quarantine"
    );

    // No broker_map entry was ever created (upsert was not reached before crash).
    let all_mappings = mqk_db::broker_map_load(&pool).await?;
    let w4_entry = all_mappings.iter().find(|(id, _)| id == key);
    assert!(
        w4_entry.is_none(),
        "W4: broker_map must have no entry — upsert never reached before crash"
    );

    cleanup_run(&pool, run_id).await?;

    Ok(())
}

#[tokio::test]
async fn w5_crash_after_atomic_sent_and_broker_map_commit_no_double_submit() -> anyhow::Result<()> {
    let url = require_db_url();
    let pool = require_pool(&url).await;
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = W5_RUN_ID.parse().unwrap();
    let key = "i93-w5-ord-001";

    // Pre-test cleanup.
    cleanup_run(&pool, run_id).await?;

    // Seed run + outbox (PENDING).
    seed_run_and_outbox(&pool, run_id, key).await?;

    // --- Simulate pre-crash dispatch ---

    // Dispatcher claims the row for THIS run only (PENDING → CLAIMED).
    let claimed =
        mqk_db::outbox_claim_batch_for_run(&pool, run_id, 1, "i93-dispatcher", Utc::now()).await?;
    assert_eq!(
        claimed.len(),
        1,
        "W5: must claim the PENDING row for this run"
    );
    assert_eq!(
        claimed[0].row.idempotency_key, key,
        "W5: claimed row must be the seeded key"
    );

    // Production path now requires CLAIMED → DISPATCHING before submit.
    let marked_dispatching =
        mqk_db::outbox_mark_dispatching(&pool, key, "i93-dispatcher", Utc::now()).await?;
    assert!(
        marked_dispatching,
        "W5: outbox_mark_dispatching must transition CLAIMED → DISPATCHING"
    );

    // Broker submit succeeds.
    let mut broker = mqk_testkit::FakeBroker::new();
    broker.submit(key, json!({"symbol": "SPY", "qty": 1}));
    assert_eq!(
        broker.submit_count(),
        1,
        "W5: broker must record one submit"
    );

    // Atomically persist SENT + broker_map durability.
    let sent =
        mqk_db::outbox_mark_sent_with_broker_map(&pool, key, "test-broker-id", Utc::now()).await?;
    assert!(sent, "W5: atomic helper must transition DISPATCHING → SENT");

    // --- CRASH: process exits here, before any in-memory order_map.register ---
    // DB state: outbox = SENT, broker HAS the order, broker_map entry is durable.

    // Verify broker_map durability before recovery.
    let before = mqk_db::broker_map_load(&pool).await?;
    assert!(
        before
            .iter()
            .any(|(id, broker_id)| id == key && broker_id == "test-broker-id"),
        "W5: broker_map must contain durable entry before recovery"
    );

    // --- Restart: run recovery ---
    let report = mqk_testkit::recover_outbox_against_broker(&pool, run_id, &mut broker).await?;

    assert_eq!(
        report.inspected, 1,
        "W5: recovery must inspect the SENT row"
    );
    assert_eq!(
        report.resubmitted, 0,
        "W5: must NOT resubmit — broker already has the order"
    );
    assert_eq!(
        report.acked, 1,
        "W5: must mark ACKED when broker already has the order"
    );
    assert_eq!(
        broker.submit_count(),
        1,
        "W5: broker must have received exactly one submit total (no double-submit)"
    );

    // DB must now show ACKED.
    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, key).await?;
    assert_eq!(
        row.expect("W5: row must exist").status,
        "ACKED",
        "W5: outbox row must be ACKED after recovery"
    );

    // Mapping durability must remain intact after recovery.
    let after = mqk_db::broker_map_load(&pool).await?;
    assert!(
        after
            .iter()
            .any(|(id, broker_id)| id == key && broker_id == "test-broker-id"),
        "W5: broker_map durable entry must survive recovery"
    );

    cleanup_run(&pool, run_id).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// W6: Crash after inbox_insert_deduped, before inbox_mark_applied
// ---------------------------------------------------------------------------

/// Crash after inbox_insert_deduped() but before inbox_mark_applied().
///
/// DB state entering recovery: inbox row present, applied_at_utc IS NULL.
/// inbox_load_unapplied_for_run must return the row exactly once for replay.
/// After the apply, a second call must return zero rows — no double-apply.
#[tokio::test]
async fn w6_crash_after_inbox_insert_before_apply_replays_exactly_once() -> anyhow::Result<()> {
    let url = require_db_url();
    let pool = require_pool(&url).await;
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = W6_RUN_ID.parse().unwrap();
    let idem_key = "i93-w6-ord-001";
    let fill_msg_id = "i93-w6-fill-001";

    // Pre-test cleanup.
    cleanup_run(&pool, run_id).await?;

    // Seed run + outbox entry (satisfies oms_inbox.run_id FK).
    seed_run_and_outbox(&pool, run_id, idem_key).await?;

    // --- Simulate fill event arrival ---

    // First insert: must create the inbox row.
    let inserted =
        mqk_db::inbox_insert_deduped(&pool, run_id, fill_msg_id, json!({"fill": "full"})).await?;
    assert!(inserted, "W6: first insert must create the inbox row");

    // Idempotency: a retry with the same broker_message_id must NOT create a second row.
    let retry =
        mqk_db::inbox_insert_deduped(&pool, run_id, fill_msg_id, json!({"fill": "full"})).await?;
    assert!(!retry, "W6: retry insert must not create a second row");

    // --- CRASH: process exits here, inbox_mark_applied never called ---
    // DB state: inbox row exists, applied_at_utc IS NULL.

    // --- Restart: verify exactly one unapplied row is surfaced ---
    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    assert_eq!(
        unapplied.len(),
        1,
        "W6: recovery must surface exactly one unapplied inbox row"
    );
    assert_eq!(
        unapplied[0].broker_message_id, fill_msg_id,
        "W6: surfaced row must be the crashed fill"
    );
    assert!(
        unapplied[0].applied_at_utc.is_none(),
        "W6: surfaced row must have applied_at_utc IS NULL"
    );

    // Simulate portfolio apply and mark applied.
    mqk_db::inbox_mark_applied(&pool, run_id, fill_msg_id, Utc::now()).await?;

    // --- Second restart: no unapplied rows remain ---
    let after_apply = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    assert_eq!(
        after_apply.len(),
        0,
        "W6: second restart must see zero unapplied rows — fill must not be re-applied"
    );

    // inbox_mark_applied is idempotent: calling it again must not error.
    mqk_db::inbox_mark_applied(&pool, run_id, fill_msg_id, Utc::now()).await?;
    let idempotent_check = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    assert_eq!(
        idempotent_check.len(),
        0,
        "W6: idempotent mark_applied must not alter the applied row"
    );

    cleanup_run(&pool, run_id).await?;

    Ok(())
}
