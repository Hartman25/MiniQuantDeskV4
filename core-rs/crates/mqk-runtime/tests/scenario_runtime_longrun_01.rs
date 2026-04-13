//! RUNTIME-LONGRUN-01 — Repeated-cycle runtime ingest / cursor / idempotency proofs.
//!
//! # Purpose
//!
//! Closes the runtime-side proof gap for RUNTIME-LONGRUN-01.  The daemon-side
//! file (`scenario_runtime_longrun_01.rs` in `mqk-daemon`) proves pure in-process
//! controller state-machine cycles (LR-01..LR-06, pure no-DB).  This file proves
//! the DB-backed runtime ingest path holds its invariants across **repeated cycles**.
//!
//! # Coverage
//!
//! | Test      | Claim                                                                         |
//! |-----------|-------------------------------------------------------------------------------|
//! | LR-RT-01  | Repeated duplicate WS ingest across 5 cycles remains idempotent: exactly     |
//! |           | one durable inbox row regardless of how many times the same event is fed.    |
//! | LR-RT-02  | Cursor truth is monotonic across 3 establish→gap→recover cycles: GapDetected |
//! |           | is never silently promoted to Live; gap repair produces a clean Live, not a  |
//! |           | stale rollback to a prior message_id.                                        |
//! | LR-RT-03  | Resumed ingest after gap repair does not double-apply prior effects: an      |
//! |           | event that entered the inbox before a gap is still deduped after repair      |
//! |           | and continued ingest; only genuinely new events add rows.                    |
//!
//! # Proof boundary
//!
//! - All tests are DB-backed and marked `#[ignore]`; they skip gracefully without
//!   `MQK_DATABASE_URL`.
//! - These tests prove the real `process_ws_inbound_batch` /
//!   `persist_ws_gap_cursor` / `advance_cursor_after_ws_establish` /
//!   `ws_continuity_from_cursor` seams in `mqk-runtime::alpaca_inbound`.
//! - Pure in-process cycle stability (no DB, no network) is proven by the
//!   daemon-side `scenario_runtime_longrun_01.rs` (LR-01..LR-06).
//!
//! # Running with a live DB
//!
//! ```text
//! MQK_DATABASE_URL=postgres://... cargo test -p mqk-runtime \
//!   scenario_runtime_longrun_01 -- --include-ignored --test-threads=1
//! ```

use chrono::Utc;
use mqk_broker_alpaca::types::{AlpacaFetchCursor, AlpacaTradeUpdatesResume};
use mqk_db::{NewRun, ENV_DB_URL};
use mqk_runtime::alpaca_inbound::{
    advance_cursor_after_ws_establish, check_alpaca_ws_continuity_from_opaque_cursor,
    persist_ws_gap_cursor, process_ws_inbound_batch, ws_continuity_from_cursor, WsIngestOutcome,
    WsLifecycleContinuity,
};
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared test harness (mirrors scenario_alpaca_inbound_rt_brk08r.rs)
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

async fn insert_test_run(pool: &sqlx::PgPool) -> Uuid {
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &NewRun {
            run_id,
            engine_id: format!("test-engine-{run_id}"),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "test-git-hash".to_string(),
            config_hash: "test-config-hash".to_string(),
            config_json: json!({}),
            host_fingerprint: "test-host".to_string(),
        },
    )
    .await
    .expect("insert_run");
    run_id
}

macro_rules! require_db {
    () => {
        if std::env::var(ENV_DB_URL).is_err() {
            eprintln!("SKIP: MQK_DATABASE_URL not set");
            return;
        }
    };
}

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

// ---------------------------------------------------------------------------
// LR-RT-01: Repeated duplicate WS ingest across 5 cycles remains idempotent.
//
// Proves that processing the same identifiable WS event across N independent
// ingest calls — simulating repeated cycle boundaries where the same event is
// replayed — produces exactly one durable inbox row in every case.
//
// The key difference from RT-I2 (which proves one-shot dedup): this test
// exercises the cursor reload between cycles.  After each cycle the persisted
// cursor is loaded from DB and fed as `prev_cursor` for the next call, exactly
// as a real daemon restart would do.  Dedup must hold even when the caller
// supplies a stale or equal cursor position.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL integration DB"]
async fn lr_rt_01_repeated_duplicate_ws_ingest_across_cycles_remains_idempotent() {
    require_db!();
    let pool = test_pool().await;
    let run_id = insert_test_run(&pool).await;
    let adapter_id = format!("alpaca-lr-rt-01-{}", Uuid::new_v4());

    let broker_id = format!("broker-lr-rt-01-{}", Uuid::new_v4());
    let client_id = format!("client-lr-rt-01-{}", Uuid::new_v4());
    let ts = "2024-06-15T09:30:00.000000Z";
    let raw = ws_bytes_new_order(&broker_id, &client_id, ts);
    let expected_mid = format!("alpaca:{broker_id}:new:{ts}");

    // Start from cold-start; after cycle 1 we reload the persisted cursor.
    let mut prev_cursor = AlpacaFetchCursor::cold_start_unproven(None);
    let now = Utc::now();

    for cycle in 1u32..=5 {
        let outcome = process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw, &prev_cursor, now)
            .await
            .unwrap_or_else(|e| panic!("LR-RT-01 cycle {cycle}: process failed: {e}"));

        // Even on dedup cycles the function returns EventsIngested because the
        // event passed through the pipeline (inbox_insert_deduped_with_identity
        // returns Ok(false) rather than Err; count still increments).
        assert!(
            matches!(outcome, WsIngestOutcome::EventsIngested { count: 1, .. }),
            "LR-RT-01 cycle {cycle}: expected EventsIngested{{count:1}}"
        );

        // Cursor in DB must stay Live after every cycle — never roll back.
        let loaded_json = mqk_db::load_broker_cursor(&pool, &adapter_id)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("LR-RT-01 cycle {cycle}: cursor missing after ingest"));
        let loaded: AlpacaFetchCursor = serde_json::from_str(&loaded_json)
            .unwrap_or_else(|e| panic!("LR-RT-01 cycle {cycle}: cursor deser failed: {e}"));

        assert!(
            matches!(loaded.trade_updates, AlpacaTradeUpdatesResume::Live { .. }),
            "LR-RT-01 cycle {cycle}: cursor must be Live after ingest"
        );

        // last_message_id must be the deterministic value derived from the event.
        // Same event → same message_id every cycle (monotonic identity).
        match &loaded.trade_updates {
            AlpacaTradeUpdatesResume::Live {
                last_message_id, ..
            } => {
                assert_eq!(
                    last_message_id, &expected_mid,
                    "LR-RT-01 cycle {cycle}: last_message_id must be stable and deterministic"
                );
            }
            _ => unreachable!(),
        }

        // Also verify via the opaque-cursor path (round-trip check).
        let continuity = check_alpaca_ws_continuity_from_opaque_cursor(Some(loaded_json.as_str()));
        assert!(
            matches!(continuity, Some(WsLifecycleContinuity::Live { .. })),
            "LR-RT-01 cycle {cycle}: opaque cursor must decode to Live"
        );

        // Reload persisted cursor for the next cycle (simulates restart boundary).
        prev_cursor = loaded;
    }

    // After 5 cycles: exactly one inbox row.
    let inbox = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .expect("inbox_load_unapplied_for_run failed");
    assert_eq!(
        inbox.len(),
        1,
        "LR-RT-01 post: exactly one inbox row despite 5 repeated ingest cycles"
    );
    assert_eq!(
        inbox[0].broker_message_id, expected_mid,
        "LR-RT-01 post: inbox row must carry the correct broker_message_id"
    );
}

// ---------------------------------------------------------------------------
// LR-RT-02: Cursor truth is monotonic across 3 establish→gap→recover cycles.
//
// Each cycle exercises the full:
//   process event → Live cursor
//   persist_ws_gap_cursor → GapDetected cursor
//   advance_cursor_after_ws_establish → repaired Live cursor
//
// Invariants proven across all 3 cycles:
//
//   (a) GapDetected is NEVER silently promoted to Live — ws_continuity_from_cursor
//       on the loaded GapDetected JSON always returns GapDetected, is_ready==false.
//   (b) GapDetected preserves last_message_id from the prior Live position.
//   (c) Gap repair (advance_cursor_after_ws_establish) produces a clean Live with
//       EMPTY last_message_id — it is NOT a rollback to the prior event position.
//   (d) After repair, is_ready()==true and the opaque-cursor round-trip is Live.
//
// These properties are checked for each of the 3 cycles, proving they do not
// drift or degrade over repeated transitions.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL integration DB"]
async fn lr_rt_02_cursor_truth_remains_monotonic_across_establish_gap_recover_cycles() {
    require_db!();
    let pool = test_pool().await;
    let run_id = insert_test_run(&pool).await;
    let adapter_id = format!("alpaca-lr-rt-02-{}", Uuid::new_v4());
    let client_base = format!("client-lr-rt-02-{}", Uuid::new_v4());
    let ts = "2024-06-15T09:30:00.000000Z";
    let now = Utc::now();

    for cycle in 1u32..=3 {
        // --- Step 1: Load current cursor and process a cycle-unique event.
        //
        // After cycle 1 the cursor is the repaired Live from the previous cycle
        // (empty last_message_id).  Each new event has a unique broker_id so
        // message_ids are distinct across cycles.
        let prev_cursor = if cycle == 1 {
            AlpacaFetchCursor::cold_start_unproven(None)
        } else {
            let json = mqk_db::load_broker_cursor(&pool, &adapter_id)
                .await
                .unwrap()
                .unwrap_or_else(|| {
                    panic!("LR-RT-02 cycle {cycle}: expected cursor from prior cycle")
                });
            serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("LR-RT-02 cycle {cycle}: deser prev failed: {e}"))
        };

        let broker_id = format!("broker-lr-rt-02-c{cycle}-{}", Uuid::new_v4());
        let raw = ws_bytes_new_order(&broker_id, &client_base, ts);
        let expected_live_mid = format!("alpaca:{broker_id}:new:{ts}");

        let outcome = process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw, &prev_cursor, now)
            .await
            .unwrap_or_else(|e| panic!("LR-RT-02 cycle {cycle} step1: process failed: {e}"));
        assert!(
            matches!(outcome, WsIngestOutcome::EventsIngested { count: 1, .. }),
            "LR-RT-02 cycle {cycle} step1: expected EventsIngested{{count:1}}"
        );

        // Verify cursor is Live with the correct message_id.
        let live_json = mqk_db::load_broker_cursor(&pool, &adapter_id)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("LR-RT-02 cycle {cycle} step1: cursor must be set"));
        let live_cursor: AlpacaFetchCursor = serde_json::from_str(&live_json).unwrap();

        assert!(
            ws_continuity_from_cursor(&live_cursor).is_ready(),
            "LR-RT-02 cycle {cycle} step1: Live cursor must be ready"
        );
        match &live_cursor.trade_updates {
            AlpacaTradeUpdatesResume::Live {
                last_message_id, ..
            } => {
                assert_eq!(
                    last_message_id, &expected_live_mid,
                    "LR-RT-02 cycle {cycle} step1: Live cursor must carry event's message_id"
                );
            }
            other => panic!("LR-RT-02 cycle {cycle} step1: expected Live, got {other:?}"),
        }

        // --- Step 2: Persist gap cursor.
        let gap_detail = format!("lr-rt-02 cycle-{cycle} test disconnect");
        let gap_returned =
            persist_ws_gap_cursor(&pool, &adapter_id, &live_cursor, &gap_detail, now)
                .await
                .unwrap_or_else(|e| {
                    panic!("LR-RT-02 cycle {cycle} step2: persist_gap failed: {e}")
                });

        assert!(
            matches!(
                gap_returned.trade_updates,
                AlpacaTradeUpdatesResume::GapDetected { .. }
            ),
            "LR-RT-02 cycle {cycle} step2: returned cursor must be GapDetected"
        );

        // --- Step 3: Load cursor from DB and verify it is GapDetected.
        //
        // Invariant (a): GapDetected is NEVER silently promoted to Live.
        // Invariant (b): GapDetected preserves last_message_id from prior Live.
        let gap_json = mqk_db::load_broker_cursor(&pool, &adapter_id)
            .await
            .unwrap()
            .unwrap_or_else(|| {
                panic!("LR-RT-02 cycle {cycle} step3: cursor must be set after gap persist")
            });
        let gap_loaded: AlpacaFetchCursor = serde_json::from_str(&gap_json).unwrap();
        let continuity_gap = ws_continuity_from_cursor(&gap_loaded);

        assert!(
            matches!(continuity_gap, WsLifecycleContinuity::GapDetected { .. }),
            "LR-RT-02 cycle {cycle} step3: loaded cursor must be GapDetected (not promoted to Live)"
        );
        assert!(
            !continuity_gap.is_ready(),
            "LR-RT-02 cycle {cycle} step3: GapDetected must NOT be ready"
        );

        // Verify via opaque-cursor path (simulates orchestrator reading from DB).
        let opaque_continuity =
            check_alpaca_ws_continuity_from_opaque_cursor(Some(gap_json.as_str()));
        assert!(
            matches!(
                opaque_continuity,
                Some(WsLifecycleContinuity::GapDetected { .. })
            ),
            "LR-RT-02 cycle {cycle} step3: opaque cursor decode must be GapDetected"
        );

        // last_message_id preserved from the Live cursor (invariant b).
        match &gap_loaded.trade_updates {
            AlpacaTradeUpdatesResume::GapDetected {
                last_message_id, ..
            } => {
                assert_eq!(
                    last_message_id.as_deref(),
                    Some(expected_live_mid.as_str()),
                    "LR-RT-02 cycle {cycle} step3: GapDetected must preserve last_message_id"
                );
            }
            _ => unreachable!(),
        }

        // --- Step 4: Advance cursor after WS re-establish (gap repair).
        let repaired = advance_cursor_after_ws_establish(&pool, &adapter_id, &gap_loaded, now)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "LR-RT-02 cycle {cycle} step4: advance_cursor_after_ws_establish failed: {e}"
                )
            });

        // --- Step 5: Verify repaired cursor is Live and ready (invariant d).
        let repaired_json = mqk_db::load_broker_cursor(&pool, &adapter_id)
            .await
            .unwrap()
            .unwrap_or_else(|| {
                panic!("LR-RT-02 cycle {cycle} step5: cursor must be set after repair")
            });
        let repaired_loaded: AlpacaFetchCursor = serde_json::from_str(&repaired_json).unwrap();

        assert!(
            ws_continuity_from_cursor(&repaired_loaded).is_ready(),
            "LR-RT-02 cycle {cycle} step5: repaired cursor must be ready (Live)"
        );

        // --- Step 6: Repaired Live has EMPTY last_message_id — not a rollback to
        //             the prior event position (invariant c).
        //
        // advance_cursor_after_ws_establish writes:
        //   Live { last_message_id: "", last_event_at: "" }
        // so the WS session starts fresh from "established but no events yet".
        // This is correct: gap repair does not restore the old Live position,
        // it begins a new WS session.
        match &repaired.trade_updates {
            AlpacaTradeUpdatesResume::Live {
                last_message_id, ..
            } => {
                assert!(
                    last_message_id.is_empty(),
                    "LR-RT-02 cycle {cycle} step6: repaired Live must have empty last_message_id \
                     (no rollback to prior event position), got: {last_message_id}"
                );
                assert_ne!(
                    last_message_id.as_str(),
                    expected_live_mid.as_str(),
                    "LR-RT-02 cycle {cycle} step6: repaired cursor must NOT carry \
                     last_message_id from the pre-gap event (no stale rollback)"
                );
            }
            other => panic!(
                "LR-RT-02 cycle {cycle} step6: repaired returned cursor must be Live, got {other:?}"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// LR-RT-03: Resumed ingest after gap repair does not double-apply prior effects.
//
// Sequence:
//   Phase 1: Process event A → 1 inbox row.
//   Phase 2: Declare gap.
//   Phase 3: Repair cursor (advance_cursor_after_ws_establish).
//   Phase 4: Re-submit event A (simulating WS replay / resumed ingest from gap
//            window) → still exactly 1 inbox row.
//   Phase 5: Process event B (genuinely new) → 2 inbox rows.
//   Phase 6: Re-submit both A and B again → still 2 inbox rows.
//
// The final state proves: gap repair + resumed ingest does NOT silently
// double-apply events that were durably ingested before the gap.  Only
// genuinely new events (B) add rows.  Repeated cycle submissions of A
// collapse to the existing row every time.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL integration DB"]
async fn lr_rt_03_resumed_ingest_after_gap_repair_does_not_double_apply_prior_effects() {
    require_db!();
    let pool = test_pool().await;
    let run_id = insert_test_run(&pool).await;
    let adapter_id = format!("alpaca-lr-rt-03-{}", Uuid::new_v4());
    let client_id = format!("client-lr-rt-03-{}", Uuid::new_v4());
    let now = Utc::now();

    let broker_id_a = format!("broker-lr-rt-03-A-{}", Uuid::new_v4());
    let broker_id_b = format!("broker-lr-rt-03-B-{}", Uuid::new_v4());
    let ts_a = "2024-06-15T09:30:00.000000Z";
    let ts_b = "2024-06-15T09:31:00.000000Z";
    let expected_mid_a = format!("alpaca:{broker_id_a}:new:{ts_a}");
    let expected_mid_b = format!("alpaca:{broker_id_b}:new:{ts_b}");

    let raw_a = ws_bytes_new_order(&broker_id_a, &client_id, ts_a);
    let raw_b = ws_bytes_new_order(&broker_id_b, &client_id, ts_b);

    // --- Phase 1: Process event A (pre-gap).
    let cold = AlpacaFetchCursor::cold_start_unproven(None);
    let outcome_a1 = process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw_a, &cold, now)
        .await
        .expect("LR-RT-03 phase1: process A failed");
    assert!(
        matches!(outcome_a1, WsIngestOutcome::EventsIngested { count: 1, .. }),
        "LR-RT-03 phase1: event A must be ingested"
    );

    let live_after_a_json = mqk_db::load_broker_cursor(&pool, &adapter_id)
        .await
        .unwrap()
        .expect("LR-RT-03 phase1: cursor must be set after A");
    let live_after_a: AlpacaFetchCursor =
        serde_json::from_str(&live_after_a_json).expect("LR-RT-03 phase1: deser after A");

    let inbox_after_a = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .unwrap();
    assert_eq!(
        inbox_after_a.len(),
        1,
        "LR-RT-03 phase1: exactly 1 inbox row after event A"
    );

    // --- Phase 2: Declare gap on the Live cursor.
    let gap = persist_ws_gap_cursor(
        &pool,
        &adapter_id,
        &live_after_a,
        "lr-rt-03 gap before replay",
        now,
    )
    .await
    .expect("LR-RT-03 phase2: persist gap failed");
    assert!(
        matches!(
            gap.trade_updates,
            AlpacaTradeUpdatesResume::GapDetected { .. }
        ),
        "LR-RT-03 phase2: cursor must be GapDetected after persist"
    );

    // --- Phase 3: Repair cursor (WS re-establish after gap).
    let repaired = advance_cursor_after_ws_establish(&pool, &adapter_id, &gap, now)
        .await
        .expect("LR-RT-03 phase3: advance_cursor_after_ws_establish failed");
    assert!(
        matches!(
            repaired.trade_updates,
            AlpacaTradeUpdatesResume::Live { .. }
        ),
        "LR-RT-03 phase3: cursor must be Live after repair"
    );

    // --- Phase 4: Re-submit event A after gap repair (simulates resumed ingest
    //              replaying events from the gap window or from a prior position).
    //              The dedup constraint must prevent a second inbox row.
    let outcome_a2 = process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw_a, &repaired, now)
        .await
        .expect("LR-RT-03 phase4: process A replay failed");
    assert!(
        matches!(outcome_a2, WsIngestOutcome::EventsIngested { count: 1, .. }),
        "LR-RT-03 phase4: A replay must return EventsIngested (dedup path, count==1)"
    );

    let inbox_after_a_replay = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .unwrap();
    assert_eq!(
        inbox_after_a_replay.len(),
        1,
        "LR-RT-03 phase4: still exactly 1 inbox row after A replay — \
         dedup must hold across gap repair boundary"
    );
    assert_eq!(
        inbox_after_a_replay[0].broker_message_id, expected_mid_a,
        "LR-RT-03 phase4: inbox row must have event A's message_id"
    );

    // --- Phase 5: Process event B (genuinely new after repair).
    let cursor_before_b_json = mqk_db::load_broker_cursor(&pool, &adapter_id)
        .await
        .unwrap()
        .expect("LR-RT-03 phase5: cursor must be set");
    let cursor_before_b: AlpacaFetchCursor =
        serde_json::from_str(&cursor_before_b_json).expect("LR-RT-03 phase5: deser before B");

    let outcome_b1 =
        process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw_b, &cursor_before_b, now)
            .await
            .expect("LR-RT-03 phase5: process B failed");
    assert!(
        matches!(outcome_b1, WsIngestOutcome::EventsIngested { count: 1, .. }),
        "LR-RT-03 phase5: event B must be ingested as genuinely new"
    );

    let inbox_after_b = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .unwrap();
    assert_eq!(
        inbox_after_b.len(),
        2,
        "LR-RT-03 phase5: exactly 2 inbox rows after B (A + B)"
    );

    // --- Phase 6: Re-submit both A and B again — no new rows on either.
    let cursor_p6_json = mqk_db::load_broker_cursor(&pool, &adapter_id)
        .await
        .unwrap()
        .expect("LR-RT-03 phase6: cursor must be set");
    let cursor_p6: AlpacaFetchCursor =
        serde_json::from_str(&cursor_p6_json).expect("LR-RT-03 phase6: deser p6");

    // Re-submit A a third time.
    let outcome_a3 = process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw_a, &cursor_p6, now)
        .await
        .expect("LR-RT-03 phase6: process A third time failed");
    assert!(
        matches!(outcome_a3, WsIngestOutcome::EventsIngested { count: 1, .. }),
        "LR-RT-03 phase6: A third time must return EventsIngested{{count:1}} (dedup)"
    );

    // Re-submit B a second time.
    let cursor_after_a3_json = mqk_db::load_broker_cursor(&pool, &adapter_id)
        .await
        .unwrap()
        .expect("LR-RT-03 phase6: cursor after A3 must be set");
    let cursor_after_a3: AlpacaFetchCursor =
        serde_json::from_str(&cursor_after_a3_json).expect("LR-RT-03 phase6: deser after A3");

    let outcome_b2 =
        process_ws_inbound_batch(&pool, run_id, &adapter_id, &raw_b, &cursor_after_a3, now)
            .await
            .expect("LR-RT-03 phase6: process B second time failed");
    assert!(
        matches!(outcome_b2, WsIngestOutcome::EventsIngested { count: 1, .. }),
        "LR-RT-03 phase6: B second time must return EventsIngested{{count:1}} (dedup)"
    );

    // Final state: still exactly 2 rows — one per unique event.
    let inbox_final = mqk_db::inbox_load_unapplied_for_run(&pool, run_id)
        .await
        .unwrap();
    assert_eq!(
        inbox_final.len(),
        2,
        "LR-RT-03 final: exactly 2 inbox rows after all A+B re-submissions — \
         resumed ingest does NOT double-apply prior effects"
    );

    let mids: std::collections::HashSet<&str> = inbox_final
        .iter()
        .map(|r| r.broker_message_id.as_str())
        .collect();
    assert!(
        mids.contains(expected_mid_a.as_str()),
        "LR-RT-03 final: inbox must contain event A's message_id"
    );
    assert!(
        mids.contains(expected_mid_b.as_str()),
        "LR-RT-03 final: inbox must contain event B's message_id"
    );
}
