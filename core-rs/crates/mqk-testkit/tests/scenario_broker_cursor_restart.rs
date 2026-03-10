//! Scenario: Broker Cursor Restart — Patch A2
//!
//! # Mission
//!
//! Prove that the durable broker event cursor contract is correct end-to-end.
//!
//! # Invariants under test
//!
//! **A1** — Paper broker cursor filters correctly (pure in-memory):
//!   `fetch_events(cursor)` returns only events whose seq > cursor value.
//!   Events are never drained.  `fetch_events(new_cursor)` after consuming all
//!   events returns an empty batch and `None` cursor.
//!
//! **A2** — Orchestrator advances DB cursor after inbox persist:
//!   After `tick()` completes, `broker_event_cursor` in DB holds the cursor
//!   value returned by the adapter.  The adapter is called with `None` when
//!   no prior cursor exists.
//!
//! **A3** — Orchestrator resumes from DB cursor on restart:
//!   A fresh orchestrator constructed with a cursor loaded from DB passes that
//!   cursor (not `None`) to the adapter on its first `fetch_events` call.
//!
//! # Test matrix
//!
//! | Test | Invariants | DB? |
//! |------|------------|-----|
//! | `a1_paper_broker_cursor_filters_events` | A1 | No  |
//! | `a2_orchestrator_advances_db_cursor`     | A2 | Yes |
//! | `a3_orchestrator_resumes_from_cursor`    | A3 | Yes |
//!
//! DB tests skip gracefully when `MQK_DATABASE_URL` is absent or unreachable.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::Utc;
use mqk_db::FixedClock;
use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerGateway, BrokerInvokeToken,
    BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, IntegrityGate, ReconcileGate, RiskGate, Side,
};
use mqk_portfolio::PortfolioState;
use mqk_runtime::orchestrator::ExecutionOrchestrator;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Cursor-tracking broker stub
// ---------------------------------------------------------------------------

/// State shared between the broker and test assertions.
#[derive(Default)]
struct CursorState {
    /// Cursors passed to each `fetch_events` call, in order.
    calls: Vec<Option<String>>,
    /// Pre-configured return values popped FIFO; exhausted → `([], None)`.
    events_to_return: Vec<(Vec<mqk_execution::BrokerEvent>, Option<String>)>,
}

/// Broker that records every cursor it receives and returns preconfigured batches.
struct CursorTrackingBroker {
    state: Arc<Mutex<CursorState>>,
}

impl BrokerAdapter for CursorTrackingBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        Ok(BrokerSubmitResponse {
            broker_order_id: format!("b-{}", req.order_id),
            submitted_at: 0,
            status: "ok".to_string(),
        })
    }

    fn cancel_order(
        &self,
        id: &str,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerCancelResponse, BrokerError> {
        Ok(BrokerCancelResponse {
            broker_order_id: id.to_string(),
            cancelled_at: 0,
            status: "ok".to_string(),
        })
    }

    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerReplaceResponse, BrokerError> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 0,
            status: "ok".to_string(),
        })
    }

    fn fetch_events(
        &self,
        cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<(Vec<mqk_execution::BrokerEvent>, Option<String>), BrokerError> {
        let mut state = self.state.lock().expect("poisoned");
        state.calls.push(cursor.map(|s| s.to_string()));
        if state.events_to_return.is_empty() {
            Ok((vec![], None))
        } else {
            Ok(state.events_to_return.remove(0))
        }
    }
}

/// Boolean gate: implements all three gate traits with a single `bool` value.
struct BoolGate(bool);
impl IntegrityGate for BoolGate {
    fn is_armed(&self) -> bool {
        self.0
    }
}
impl RiskGate for BoolGate {
    fn evaluate_gate(&self) -> mqk_execution::RiskDecision {
        if self.0 {
            mqk_execution::RiskDecision::Allow
        } else {
            mqk_execution::RiskDecision::Deny(mqk_execution::RiskDenial {
                reason: mqk_execution::RiskReason::RiskEngineUnavailable,
                evidence: mqk_execution::RiskEvidence::default(),
            })
        }
    }
}
impl ReconcileGate for BoolGate {
    fn is_clean(&self) -> bool {
        self.0
    }
}

// ---------------------------------------------------------------------------
// DB helpers
// ---------------------------------------------------------------------------

fn db_url_or_skip() -> Option<String> {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => {
            println!("SKIP: requires MQK_DATABASE_URL");
            None
        }
    }
}

async fn try_pool_or_skip(url: &str) -> Result<Option<PgPool>> {
    match PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect(url)
        .await
    {
        Ok(pool) => Ok(Some(pool)),
        Err(e) => {
            println!("SKIP: cannot connect to DB: {e}");
            Ok(None)
        }
    }
}

async fn seed_running_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "a2-cursor-test".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "a2-test".to_string(),
            config_hash: "a2-test".to_string(),
            config_json: json!({}),
            host_fingerprint: "a2-test".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    Ok(())
}

async fn cleanup_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn cleanup_cursor(pool: &PgPool, adapter_id: &str) -> Result<()> {
    sqlx::query("delete from broker_event_cursor where adapter_id = $1")
        .bind(adapter_id)
        .execute(pool)
        .await?;
    Ok(())
}

fn make_tracking_orch(
    pool: PgPool,
    run_id: Uuid,
    broker: CursorTrackingBroker,
    adapter_id: &str,
    broker_cursor: Option<String>,
) -> ExecutionOrchestrator<CursorTrackingBroker, BoolGate, BoolGate, BoolGate, FixedClock> {
    let gateway = BrokerGateway::for_test(broker, BoolGate(true), BoolGate(true), BoolGate(true));
    ExecutionOrchestrator::new(
        pool,
        gateway,
        BrokerOrderMap::new(),
        BTreeMap::new(),
        PortfolioState::new(1_000_000_000_i64),
        run_id,
        "a2-dispatcher",
        adapter_id,
        broker_cursor,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(mqk_reconcile::BrokerSnapshot::empty),
    )
}

// ---------------------------------------------------------------------------
// A1 — paper broker cursor filters correctly (pure in-memory, no DB)
// ---------------------------------------------------------------------------

/// A1: `fetch_events(cursor)` on the paper broker returns only events whose
/// sequence number exceeds the cursor value.  Events are never drained —
/// re-fetching from an older cursor replays the same events.
#[test]
fn a1_paper_broker_cursor_filters_events() {
    use mqk_broker_paper::LockedPaperBroker;

    let broker = LockedPaperBroker::new();
    let token = BrokerInvokeToken::for_test();

    // ── Submit two orders; each generates an Ack (seq = 1, then 2) ───────
    let submit = |id: &str, price: i64| BrokerSubmitRequest {
        order_id: id.to_string(),
        symbol: "AAPL".to_string(),
        side: Side::Buy,
        quantity: 10,
        order_type: "limit".to_string(),
        limit_price: Some(price),
        time_in_force: "day".to_string(),
    };

    broker
        .submit_order(submit("ord-1", 150_000_000), &token)
        .unwrap();
    broker
        .submit_order(submit("ord-2", 149_000_000), &token)
        .unwrap();

    // ── Fetch from start: both Ack events returned ────────────────────────
    let (events, cursor1) = broker.fetch_events(None, &token).unwrap();
    assert_eq!(events.len(), 2, "A1: start fetch must return 2 events");
    assert!(
        cursor1.is_some(),
        "A1: cursor must be Some after first fetch"
    );
    let cursor1 = cursor1.unwrap();

    // ── Fetch again from cursor1: nothing new ─────────────────────────────
    let (events2, cursor2) = broker.fetch_events(Some(&cursor1), &token).unwrap();
    assert_eq!(events2.len(), 0, "A1: no new events after consuming all");
    assert!(
        cursor2.is_none(),
        "A1: cursor must be None when no new events"
    );

    // ── Fetch from "1" (after first Ack): only second Ack returned ────────
    let (events3, cursor3) = broker.fetch_events(Some("1"), &token).unwrap();
    assert_eq!(
        events3.len(),
        1,
        "A1: resuming from seq 1 must return exactly the seq-2 event"
    );
    let c3 = cursor3.expect("A1: cursor must be Some for non-empty batch");
    assert_eq!(
        c3, "2",
        "A1: returned cursor must equal the highest seq in the batch"
    );

    // ── Events are NOT drained: fetch from None still returns both ─────────
    let (events4, _) = broker.fetch_events(None, &token).unwrap();
    assert_eq!(
        events4.len(),
        2,
        "A1: events must not be drained — re-fetch from None still returns 2"
    );
}

// ---------------------------------------------------------------------------
// A2 — orchestrator advances DB cursor after inbox persist
// ---------------------------------------------------------------------------

/// Fixed run UUID for the A2 cursor-advancement test.
const A2_RUN_ID: &str = "a2000002-0000-0000-0000-000000000000";
const A2_ADAPTER_ID: &str = "a2-cursor-adv-test";

/// A2: After `tick()` where the adapter returns `new_cursor = Some("seq-99")`,
/// the orchestrator writes "seq-99" to `broker_event_cursor` in DB.
/// The adapter is called with `None` cursor because no prior cursor existed.
///
/// Requires `MQK_DATABASE_URL`.  Skips gracefully if absent or unreachable.
#[tokio::test]
async fn a2_orchestrator_advances_db_cursor_after_tick() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = A2_RUN_ID.parse().expect("A2_RUN_ID must be a valid UUID");

    // ── Pre-test cleanup ───────────────────────────────────────────────────
    cleanup_run(&pool, run_id).await?;
    cleanup_cursor(&pool, A2_ADAPTER_ID).await?;

    // ── Seed a RUNNING run ─────────────────────────────────────────────────
    seed_running_run(&pool, run_id).await?;

    // ── Broker: returns empty batch with cursor "seq-99" ───────────────────
    //
    // Using an empty event batch so Phase 3 has nothing to apply; tick() will
    // succeed.  The cursor "seq-99" is still advanced because the contract is
    // `new_cursor != None` → advance, regardless of batch size.
    let state = Arc::new(Mutex::new(CursorState {
        calls: vec![],
        events_to_return: vec![(vec![], Some("seq-99".to_string()))],
    }));
    let broker = CursorTrackingBroker {
        state: Arc::clone(&state),
    };

    let mut orch = make_tracking_orch(pool.clone(), run_id, broker, A2_ADAPTER_ID, None);
    orch.tick().await?;

    // ── Assert: DB cursor must be "seq-99" ────────────────────────────────
    let stored = mqk_db::load_broker_cursor(&pool, A2_ADAPTER_ID).await?;
    assert_eq!(
        stored.as_deref(),
        Some("seq-99"),
        "A2: broker_event_cursor must be 'seq-99' after tick with new_cursor=Some"
    );

    // ── Assert: adapter was called once with None (no prior cursor) ────────
    {
        let s = state.lock().unwrap();
        assert_eq!(
            s.calls.len(),
            1,
            "A2: fetch_events must be called exactly once"
        );
        assert_eq!(
            s.calls[0], None,
            "A2: adapter must receive None cursor on first tick with no prior DB cursor"
        );
    }

    // ── Post-test cleanup ──────────────────────────────────────────────────
    cleanup_run(&pool, run_id).await?;
    cleanup_cursor(&pool, A2_ADAPTER_ID).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// A3 — orchestrator resumes from DB cursor on restart
// ---------------------------------------------------------------------------

/// Fixed run UUID for the A3 cursor-resume test.
const A3_RUN_ID: &str = "a3000003-0000-0000-0000-000000000000";
const A3_ADAPTER_ID: &str = "a3-cursor-resume-test";

/// A3: A fresh orchestrator constructed with a cursor loaded from DB passes
/// that cursor — not `None` — to the adapter on its first `fetch_events` call.
///
/// This simulates the correct restart sequence:
///   1. Prior process consumed events up to cursor "resume-from-42" and wrote it to DB.
///   2. Process crashes.
///   3. New process calls `load_broker_cursor` then passes it to `ExecutionOrchestrator::new`.
///   4. First `tick()` calls `fetch_events(Some("resume-from-42"))`, skipping
///      already-consumed events.
///
/// Requires `MQK_DATABASE_URL`.  Skips gracefully if absent or unreachable.
#[tokio::test]
async fn a3_orchestrator_resumes_from_db_cursor() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = A3_RUN_ID.parse().expect("A3_RUN_ID must be a valid UUID");

    // ── Pre-test cleanup ───────────────────────────────────────────────────
    cleanup_run(&pool, run_id).await?;
    cleanup_cursor(&pool, A3_ADAPTER_ID).await?;

    // ── Seed a RUNNING run ─────────────────────────────────────────────────
    seed_running_run(&pool, run_id).await?;

    // ── Simulate a prior process having written cursor "resume-from-42" ───
    mqk_db::advance_broker_cursor(&pool, A3_ADAPTER_ID, "resume-from-42", Utc::now()).await?;

    // ── Verify load works ─────────────────────────────────────────────────
    let loaded_cursor = mqk_db::load_broker_cursor(&pool, A3_ADAPTER_ID).await?;
    assert_eq!(
        loaded_cursor.as_deref(),
        Some("resume-from-42"),
        "A3: load_broker_cursor must return the seeded value"
    );

    // ── Construct fresh orchestrator with loaded cursor (restart simulation)
    let state = Arc::new(Mutex::new(CursorState {
        calls: vec![],
        events_to_return: vec![],
    }));
    let broker = CursorTrackingBroker {
        state: Arc::clone(&state),
    };
    let mut orch = make_tracking_orch(pool.clone(), run_id, broker, A3_ADAPTER_ID, loaded_cursor);

    orch.tick().await?;

    // ── Assert: adapter received the loaded cursor, not None ──────────────
    {
        let s = state.lock().unwrap();
        assert_eq!(
            s.calls.len(),
            1,
            "A3: fetch_events must be called exactly once"
        );
        assert_eq!(
            s.calls[0].as_deref(),
            Some("resume-from-42"),
            "A3: adapter must receive the DB-loaded cursor on restart, not None"
        );
    }

    // ── Post-test cleanup ──────────────────────────────────────────────────
    cleanup_run(&pool, run_id).await?;
    cleanup_cursor(&pool, A3_ADAPTER_ID).await?;
    Ok(())
}
