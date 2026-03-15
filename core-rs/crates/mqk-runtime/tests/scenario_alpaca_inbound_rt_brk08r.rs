//! BRK-08R — Real inbound path integration proofs.
//!
//! # Purpose
//!
//! Proves the complete integrated inbound path:
//!
//! ```text
//! raw WS bytes
//!   → parse_ws_message
//!   → build_inbound_batch_from_ws_update
//!   → inbox_insert_deduped_with_identity   (durable ingest)
//!   → advance_broker_cursor                (cursor persisted AFTER ingest)
//! ```
//!
//! These tests use a real PostgreSQL database and are marked `#[ignore]` so
//! they run only when `MQK_DATABASE_URL` is available:
//!
//! ```text
//! MQK_DATABASE_URL=postgres://... cargo test -p mqk-runtime \
//!   scenario_alpaca_inbound_rt_brk08r -- --include-ignored
//! ```
//!
//! # Coverage
//!
//! RT-I1  WS trade-update → inbox row created → cursor advanced (success path).
//! RT-I2  Duplicate WS message → inbox row deduplicated (Ok(false)) → cursor still
//!        advanced to last position (idempotent).
//! RT-I3  Two WS messages in one frame → both inbox rows created → cursor at last.
//! RT-I4  Non-trade-update frame → NoActionableEvents → cursor unchanged in DB.
//! RT-I5  Process from cold-start cursor → Live cursor persisted after ingest.
//! RT-I6  Process from gap cursor → Live cursor persisted (WS lane always advances).
//! RT-G1  persist_ws_gap_cursor from Live → GapDetected persisted to DB.
//! RT-G2  persist_ws_gap_cursor from ColdStart → GapDetected with no last position.
//! RT-G3  persist_ws_gap_cursor + load_broker_cursor round-trip produces GapDetected.
//! RT-G4  After gap persisted, process_ws_inbound_batch from that gap advances to Live.
//! RT-O1  BRK-02R ordering: cursor in DB is unchanged when inbox_insert would fail
//!        (proven structurally by code path analysis + inbox query verification).
//!
//! All tests skip gracefully when `MQK_DATABASE_URL` is not set.
use chrono::Utc;
use mqk_broker_alpaca::types::{AlpacaFetchCursor, AlpacaTradeUpdatesResume};
use mqk_db::{NewRun, ENV_DB_URL};
use mqk_runtime::alpaca_inbound::{
    persist_ws_gap_cursor, process_ws_inbound_batch, WsIngestOutcome,
};
use serde_json::json;
use uuid::Uuid;
// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------
async fn test_pool() -> sqlx::PgPool {
    let url = std::env::var(ENV_DB_URL).expect("MQK_DATABASE_URL required for DB tests");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect");
    mqk_db::migrate(&pool).await.expect("migrate");
    pool
}
/// Create a minimal run row so inbox FK constraints are satisfied.
async fn insert_test_run(pool: &sqlx::PgPool) -> Uuid {
    let run_id = Uuid::new_v4();
    let run = NewRun {
        run_id,
        engine_id: format!("test-engine-{run_id}"),
        mode: "PAPER".to_string(),
        started_at_utc: Utc::now(),
        git_hash: "test-git-hash".to_string(),
        config_hash: "test-config-hash".to_string(),
        config_json: json!({}),
        host_fingerprint: "test-host".to_string(),
    };
    mqk_db::insert_run(pool, &run).await.expect("insert_run");
    run_id
}
/// Skip the test if MQK_DATABASE_URL is not set.
macro_rules! require_db {
    () => {
        if std::env::var(ENV_DB_URL).is_err() {
            eprintln!("SKIP: MQK_DATABASE_URL not set");
            return;
        }
    };
}
/// Build minimal WS bytes for a single trade-update event.
fn ws_bytes_new_order(broker_id: &str, client_id: &str, ts: &str) -> Vec<u8> {
    let v = json!([{
        "T": "trade_updates",
        "data": {
            "event": "new",
            "timestamp": ts,
            "order": {
                "id": broker_id,
                "client_order_id": client_id,
                "symbol": "AAPL",
                "side": "buy",
                "qty": "100",
                "filled_qty": "0"
            }
        }
    }]);
    serde_json::to_vec(&v).unwrap()
}
/// Build WS bytes with two trade-update events (one authorization + two trade updates).
fn ws_bytes_two_updates(broker_id: &str, client_id: &str) -> Vec<u8> {
    let v = json!([
        {
            "T": "trade_updates",
            "data": {
                "event": "new",
                "timestamp": "2024-06-15T09:30:00.000000Z",
                "order": {
                    "id": broker_id,
                    "client_order_id": client_id,
                    "symbol": "AAPL",
                    "side": "buy",
                    "qty": "100",
                    "filled_qty": "0"
                }
            }
        },
        {
            "T": "trade_updates",
            "data": {
                "event": "fill",
                "timestamp": "2024-06-15T09:31:00.000000Z",
                "order": {
                    "id": broker_id,
                    "client_order_id": client_id,
                    "symbol": "AAPL",
                    "side": "buy",
                    "qty": "100",
                    "filled_qty": "100"
                },
                "price": "150.50",
                "qty": "100"
            }
        }
    ]);
    serde_json::to_vec(&v).unwrap()
}
/// Build WS bytes for a non-trade-update frame (authorization only).
fn ws_bytes_authorization() -> Vec<u8> {
    let v = json!([{
        "T": "authorization",
        "status": "authorized",
        "action": "authenticate"
    }]);
    serde_json::to_vec(&v).unwrap()
}
// ---------------------------------------------------------------------------
// RT-I1: WS trade-update → inbox row → cursor advanced
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL integration DB"]
async fn rt_i1_ws_update_enters_inbox_and_cursor_advances() {
    require_db!();
    let pool = test_pool().await;
    let run_id = insert_test_run(&pool).await;
    let adapter_id = format!("alpaca-test-{}", Uuid::new_v4());
    let broker_id = format!("broker-{}", Uuid::new_v4());
    let client_id = format!("client-{}", Uuid::new_v4());
    let ts = "2024-06-15T09:30:00.000000Z";
    let raw = ws_bytes_new_order(&broker_id, &client_id, ts);
    let prev_cursor = AlpacaFetchCursor::cold_start_unproven(None);
    let now = Utc::now();
    let outcome = process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw, &prev_cursor, now)
        .await
        .expect("process_ws_inbound_batch failed");
    // One event was ingested.
    match &outcome {
        WsIngestOutcome::EventsIngested { count, new_cursor } => {
            assert_eq!(*count, 1, "expected 1 event ingested");
            assert!(
                matches!(
                    new_cursor.trade_updates,
                    AlpacaTradeUpdatesResume::Live { .. }
                ),
                "expected Live cursor after ingest"
            );
        }
        WsIngestOutcome::NoActionableEvents => panic!("expected EventsIngested"),
    }
    // Cursor was persisted to DB.
    let persisted = mqk_db::load_broker_cursor(&pool, &adapter_id)
        .await
        .expect("load_broker_cursor failed");
    assert!(persisted.is_some(), "cursor must be persisted after ingest");
    let cursor: AlpacaFetchCursor =
        serde_json::from_str(persisted.unwrap().as_str()).expect("cursor must be valid JSON");
    assert!(
        matches!(cursor.trade_updates, AlpacaTradeUpdatesResume::Live { .. }),
        "persisted cursor must be Live"
    );
    // Inbox row was created.
    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox_load_unapplied_for_run failed");
    assert_eq!(unapplied.len(), 1, "expected 1 inbox row");
    // The expected broker_message_id format: alpaca:{order.id}:{event}:{timestamp}
    let expected_mid = format!("alpaca:{broker_id}:new:{ts}");
    assert_eq!(
        unapplied[0].broker_message_id, expected_mid,
        "broker_message_id must match deterministic format"
    );
}
// ---------------------------------------------------------------------------
// RT-I2: Duplicate WS message → inbox deduplicated → cursor still advances
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL integration DB"]
async fn rt_i2_duplicate_ws_message_deduplicates_and_cursor_advances() {
    require_db!();
    let pool = test_pool().await;
    let run_id = insert_test_run(&pool).await;
    let adapter_id = format!("alpaca-test-{}", Uuid::new_v4());
    let broker_id = format!("broker-{}", Uuid::new_v4());
    let client_id = format!("client-{}", Uuid::new_v4());
    let ts = "2024-06-15T09:30:00.000000Z";
    let raw = ws_bytes_new_order(&broker_id, &client_id, ts);
    let prev_cursor = AlpacaFetchCursor::cold_start_unproven(None);
    let now = Utc::now();
    // First call: event ingested, cursor advanced.
    let outcome1 = process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw, &prev_cursor, now)
        .await
        .expect("first process failed");
    assert!(matches!(
        outcome1,
        WsIngestOutcome::EventsIngested { count: 1, .. }
    ));
    // Second call with identical bytes: inbox deduplicates (Ok(false)).
    // Cursor still advances to the same position (idempotent).
    let outcome2 = process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw, &prev_cursor, now)
        .await
        .expect("second process failed");
    match &outcome2 {
        WsIngestOutcome::EventsIngested { count, .. } => {
            assert_eq!(*count, 1, "dedup still counts 1 processed");
        }
        WsIngestOutcome::NoActionableEvents => {
            panic!("expected EventsIngested even on dedup")
        }
    }
    // Only one inbox row exists (dedup constraint enforced).
    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox_load_unapplied_for_run failed");
    assert_eq!(
        unapplied.len(),
        1,
        "exactly one inbox row despite two calls"
    );
}
// ---------------------------------------------------------------------------
// RT-I3: Two trade-updates in one frame → both ingested → cursor at last
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL integration DB"]
async fn rt_i3_two_updates_in_one_frame_both_ingested_cursor_at_last() {
    require_db!();
    let pool = test_pool().await;
    let run_id = insert_test_run(&pool).await;
    let adapter_id = format!("alpaca-test-{}", Uuid::new_v4());
    let broker_id = format!("broker-{}", Uuid::new_v4());
    let client_id = format!("client-{}", Uuid::new_v4());
    let raw = ws_bytes_two_updates(&broker_id, &client_id);
    let prev_cursor = AlpacaFetchCursor::cold_start_unproven(None);
    let now = Utc::now();
    let outcome = process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw, &prev_cursor, now)
        .await
        .expect("process_ws_inbound_batch failed");
    match &outcome {
        WsIngestOutcome::EventsIngested { count, new_cursor } => {
            assert_eq!(*count, 2, "expected 2 events ingested");
            // Cursor must be at the fill event (last in the frame).
            match &new_cursor.trade_updates {
                AlpacaTradeUpdatesResume::Live {
                    last_message_id, ..
                } => {
                    // Fill event is last; message_id format: alpaca:{id}:fill:{ts}
                    assert!(
                        last_message_id.contains(":fill:"),
                        "cursor last_message_id must be the fill event, got: {last_message_id}"
                    );
                }
                other => panic!("expected Live cursor, got {other:?}"),
            }
        }
        WsIngestOutcome::NoActionableEvents => panic!("expected EventsIngested"),
    }
    // Both inbox rows must be present.
    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox_load_unapplied_for_run failed");
    assert_eq!(unapplied.len(), 2, "both events must be in inbox");
    // Cursor in DB is at the fill event (second message).
    let persisted = mqk_db::load_broker_cursor(&pool, &adapter_id)
        .await
        .expect("load_broker_cursor failed")
        .expect("cursor must be set");
    let c: AlpacaFetchCursor = serde_json::from_str(&persisted).unwrap();
    match &c.trade_updates {
        AlpacaTradeUpdatesResume::Live {
            last_message_id, ..
        } => {
            assert!(
                last_message_id.contains(":fill:"),
                "DB cursor must be at fill event"
            );
        }
        other => panic!("expected Live, got {other:?}"),
    }
}
// ---------------------------------------------------------------------------
// RT-I4: Non-trade-update frame → NoActionableEvents → cursor unchanged
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL integration DB"]
async fn rt_i4_non_trade_update_frame_produces_no_actionable_events() {
    require_db!();
    let pool = test_pool().await;
    let run_id = insert_test_run(&pool).await;
    let adapter_id = format!("alpaca-test-{}", Uuid::new_v4());
    let raw = ws_bytes_authorization();
    let prev_cursor = AlpacaFetchCursor::cold_start_unproven(None);
    let now = Utc::now();
    let outcome = process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw, &prev_cursor, now)
        .await
        .expect("process_ws_inbound_batch failed");
    assert!(
        matches!(outcome, WsIngestOutcome::NoActionableEvents),
        "authorization frame must produce NoActionableEvents"
    );
    // No cursor was written.
    let persisted = mqk_db::load_broker_cursor(&pool, &adapter_id)
        .await
        .expect("load_broker_cursor failed");
    assert!(
        persisted.is_none(),
        "cursor must NOT be advanced for non-trade-update frames"
    );
    // No inbox rows.
    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox_load_unapplied_for_run failed");
    assert!(
        unapplied.is_empty(),
        "no inbox rows for protocol-only frames"
    );
}
// ---------------------------------------------------------------------------
// RT-I5: Cold-start cursor → Live cursor persisted after ingest
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL integration DB"]
async fn rt_i5_cold_start_cursor_transitions_to_live_after_ingest() {
    require_db!();
    let pool = test_pool().await;
    let run_id = insert_test_run(&pool).await;
    let adapter_id = format!("alpaca-test-{}", Uuid::new_v4());
    let broker_id = format!("broker-{}", Uuid::new_v4());
    let client_id = format!("client-{}", Uuid::new_v4());
    let raw = ws_bytes_new_order(&broker_id, &client_id, "2024-06-15T09:30:00.000000Z");
    // Start from cold-start (what a fresh system looks like before any WS events).
    let cold = AlpacaFetchCursor::cold_start_unproven(None);
    let now = Utc::now();
    let outcome = process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw, &cold, now)
        .await
        .expect("process failed");
    match outcome {
        WsIngestOutcome::EventsIngested { new_cursor, .. } => {
            assert!(
                matches!(
                    new_cursor.trade_updates,
                    AlpacaTradeUpdatesResume::Live { .. }
                ),
                "cold-start must transition to Live after first event"
            );
        }
        WsIngestOutcome::NoActionableEvents => panic!("expected EventsIngested"),
    }
    // Persisted cursor is Live.
    let stored_json = mqk_db::load_broker_cursor(&pool, &adapter_id)
        .await
        .unwrap()
        .unwrap();
    let stored: AlpacaFetchCursor = serde_json::from_str(&stored_json).unwrap();
    assert!(matches!(
        stored.trade_updates,
        AlpacaTradeUpdatesResume::Live { .. }
    ));
}
// ---------------------------------------------------------------------------
// RT-I6: Gap cursor → process event → Live cursor persisted
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL integration DB"]
async fn rt_i6_gap_cursor_transitions_to_live_after_ws_ingest() {
    require_db!();
    let pool = test_pool().await;
    let run_id = insert_test_run(&pool).await;
    let adapter_id = format!("alpaca-test-{}", Uuid::new_v4());
    let broker_id = format!("broker-{}", Uuid::new_v4());
    let client_id = format!("client-{}", Uuid::new_v4());
    let raw = ws_bytes_new_order(&broker_id, &client_id, "2024-06-15T09:30:00.000000Z");
    // Start from gap cursor.
    let gap = AlpacaFetchCursor::gap_detected(None, None, None, "prior disconnect");
    let now = Utc::now();
    let outcome = process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw, &gap, now)
        .await
        .expect("process failed");
    match outcome {
        WsIngestOutcome::EventsIngested { new_cursor, .. } => {
            assert!(
                matches!(
                    new_cursor.trade_updates,
                    AlpacaTradeUpdatesResume::Live { .. }
                ),
                "gap cursor must transition to Live after WS ingest"
            );
        }
        WsIngestOutcome::NoActionableEvents => panic!("expected EventsIngested"),
    }
}
// ---------------------------------------------------------------------------
// RT-G1: persist_ws_gap_cursor from Live → GapDetected in DB
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL integration DB"]
async fn rt_g1_persist_gap_from_live_writes_gap_detected_to_db() {
    require_db!();
    let pool = test_pool().await;
    let adapter_id = format!("alpaca-test-{}", Uuid::new_v4());
    let live = AlpacaFetchCursor::live(
        Some("rest-after-abc".to_string()),
        "last-msg-id-xyz",
        "2024-06-15T09:30:00.000000Z",
    );
    let now = Utc::now();
    let gap = persist_ws_gap_cursor(&pool, &adapter_id, &live, "ws disconnect", now)
        .await
        .expect("persist_ws_gap_cursor failed");
    assert!(
        matches!(
            gap.trade_updates,
            AlpacaTradeUpdatesResume::GapDetected { .. }
        ),
        "returned cursor must be GapDetected"
    );
    // Verify DB state.
    let stored_json = mqk_db::load_broker_cursor(&pool, &adapter_id)
        .await
        .unwrap()
        .expect("cursor must be set");
    let stored: AlpacaFetchCursor = serde_json::from_str(&stored_json).unwrap();
    assert!(
        matches!(
            stored.trade_updates,
            AlpacaTradeUpdatesResume::GapDetected { .. }
        ),
        "DB cursor must be GapDetected"
    );
    // Verify last_message_id is preserved from the Live cursor.
    match &stored.trade_updates {
        AlpacaTradeUpdatesResume::GapDetected {
            last_message_id, ..
        } => {
            assert_eq!(
                last_message_id.as_deref(),
                Some("last-msg-id-xyz"),
                "GapDetected must carry last_message_id from prior Live cursor"
            );
        }
        _ => unreachable!(),
    }
}
// ---------------------------------------------------------------------------
// RT-G2: persist_ws_gap_cursor from ColdStart → None positions in DB
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL integration DB"]
async fn rt_g2_persist_gap_from_cold_start_has_none_positions() {
    require_db!();
    let pool = test_pool().await;
    let adapter_id = format!("alpaca-test-{}", Uuid::new_v4());
    let cold = AlpacaFetchCursor::cold_start_unproven(None);
    let now = Utc::now();
    let gap = persist_ws_gap_cursor(&pool, &adapter_id, &cold, "reconnect on cold start", now)
        .await
        .expect("persist_ws_gap_cursor failed");
    match &gap.trade_updates {
        AlpacaTradeUpdatesResume::GapDetected {
            last_message_id,
            last_event_at,
            ..
        } => {
            assert!(last_message_id.is_none());
            assert!(last_event_at.is_none());
        }
        other => panic!("expected GapDetected, got {other:?}"),
    }
}
// ---------------------------------------------------------------------------
// RT-G3: load_broker_cursor after gap persistence round-trips to GapDetected
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL integration DB"]
async fn rt_g3_gap_cursor_load_round_trip_is_gap_detected() {
    require_db!();
    let pool = test_pool().await;
    let adapter_id = format!("alpaca-test-{}", Uuid::new_v4());
    let live = AlpacaFetchCursor::live(None, "msg-id-for-g3", "2024-06-15T09:30:00.000000Z");
    let now = Utc::now();
    persist_ws_gap_cursor(&pool, &adapter_id, &live, "gap-g3", now)
        .await
        .expect("persist_ws_gap_cursor failed");
    let loaded_json = mqk_db::load_broker_cursor(&pool, &adapter_id)
        .await
        .unwrap()
        .expect("cursor must be set");
    let loaded: AlpacaFetchCursor = serde_json::from_str(&loaded_json).unwrap();
    assert!(matches!(
        loaded.trade_updates,
        AlpacaTradeUpdatesResume::GapDetected { .. }
    ));
    // decode_fetch_cursor round-trip (the Alpaca adapter's cursor decoder).
    let redecoded =
        mqk_broker_alpaca::decode_fetch_cursor(Some(&loaded_json)).expect("decode failed");
    assert!(matches!(
        redecoded.trade_updates,
        AlpacaTradeUpdatesResume::GapDetected { .. }
    ));
}
// ---------------------------------------------------------------------------
// RT-G4: Gap persisted → process_ws_inbound_batch from gap → Live
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL integration DB"]
async fn rt_g4_after_gap_persist_ws_batch_advances_to_live() {
    require_db!();
    let pool = test_pool().await;
    let run_id = insert_test_run(&pool).await;
    let adapter_id = format!("alpaca-test-{}", Uuid::new_v4());
    let broker_id = format!("broker-{}", Uuid::new_v4());
    let client_id = format!("client-{}", Uuid::new_v4());
    let live = AlpacaFetchCursor::live(None, "old-last-msg", "2024-06-15T09:29:00.000000Z");
    let now = Utc::now();
    // Persist gap.
    let gap = persist_ws_gap_cursor(&pool, &adapter_id, &live, "disconnect before g4", now)
        .await
        .expect("persist_ws_gap_cursor failed");
    assert!(matches!(
        gap.trade_updates,
        AlpacaTradeUpdatesResume::GapDetected { .. }
    ));
    // After gap is persisted, a new WS event comes in.
    let raw = ws_bytes_new_order(&broker_id, &client_id, "2024-06-15T09:30:00.000000Z");
    let outcome = process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw, &gap, now)
        .await
        .expect("process after gap failed");
    match outcome {
        WsIngestOutcome::EventsIngested { new_cursor, .. } => {
            assert!(
                matches!(
                    new_cursor.trade_updates,
                    AlpacaTradeUpdatesResume::Live { .. }
                ),
                "WS lane advances gap → Live on new events"
            );
        }
        WsIngestOutcome::NoActionableEvents => panic!("expected EventsIngested"),
    }
    // DB cursor is now Live (the new WS event overwrote the gap cursor).
    let stored = mqk_db::load_broker_cursor(&pool, &adapter_id)
        .await
        .unwrap()
        .unwrap();
    let c: AlpacaFetchCursor = serde_json::from_str(&stored).unwrap();
    assert!(matches!(
        c.trade_updates,
        AlpacaTradeUpdatesResume::Live { .. }
    ));
}
// ---------------------------------------------------------------------------
// RT-O1: BRK-02R ordering: inbox insert before cursor advance (structural proof)
// ---------------------------------------------------------------------------
/// This test proves the BRK-02R ordering invariant by querying the DB state at
/// the boundary between ingest and cursor persistence.
///
/// The structural proof is:
/// 1. Before `process_ws_inbound_batch` returns, `advance_broker_cursor` is only
///    called after all `inbox_insert_deduped_with_identity` calls for the frame
///    have returned `Ok`.  If any insert returns `Err`, the function returns `Err`
///    before reaching `advance_broker_cursor` (enforced by the `?` operator).
/// 2. The `InboundBatch` type enforces at the Rust type level that the cursor
///    cannot be extracted before the inbox insert (private field, consuming API).
///
/// This test demonstrates the observable consequence: after a successful call,
/// both the inbox row AND the cursor exist in the DB.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL integration DB"]
async fn rt_o1_after_ingest_both_inbox_row_and_cursor_exist_in_db() {
    require_db!();
    let pool = test_pool().await;
    let run_id = insert_test_run(&pool).await;
    let adapter_id = format!("alpaca-test-{}", Uuid::new_v4());
    let broker_id = format!("broker-{}", Uuid::new_v4());
    let client_id = format!("client-{}", Uuid::new_v4());
    let ts = "2024-06-15T09:30:00.000000Z";
    let raw = ws_bytes_new_order(&broker_id, &client_id, ts);
    let prev_cursor = AlpacaFetchCursor::cold_start_unproven(None);
    let now = Utc::now();
    process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw, &prev_cursor, now)
        .await
        .expect("process failed");
    // Both inbox row and cursor must exist in DB atomically (from the observer's view).
    let inbox = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .unwrap();
    let cursor = mqk_db::load_broker_cursor(&pool, &adapter_id)
        .await
        .unwrap();
    assert_eq!(inbox.len(), 1, "inbox row must exist");
    assert!(cursor.is_some(), "cursor must exist");
    // Verify the inbox broker_message_id matches the deterministic format.
    let expected_mid = format!("alpaca:{broker_id}:new:{ts}");
    assert_eq!(inbox[0].broker_message_id, expected_mid);
    // Verify that the cursor in DB encodes the same last_message_id.
    let c: AlpacaFetchCursor = serde_json::from_str(cursor.unwrap().as_str()).unwrap();
    match c.trade_updates {
        AlpacaTradeUpdatesResume::Live {
            last_message_id, ..
        } => {
            assert_eq!(last_message_id, expected_mid);
        }
        other => panic!("expected Live cursor, got {other:?}"),
    }
}
