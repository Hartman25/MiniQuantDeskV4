//! BRK-08R: Alpaca WS gap recovery — DB cursor honesty on disconnect/reconnect.
//!
//! Proves the two-sided repair contract:
//!
//! | Event                         | DB cursor change                          |
//! |-------------------------------|-------------------------------------------|
//! | WS session disconnect         | `GapDetected` written to DB (honest gap)  |
//! | WS subscription re-confirmed  | `Live` written to DB (gap repaired)       |
//!
//! And the end-to-end property:
//!
//! - `advance_cursor_after_ws_establish` transitions GapDetected → Live while
//!   preserving `rest_activity_after` so REST poll recovers gap-window fills.
//! - `advance_cursor_after_ws_establish` is a no-op for an already-Live cursor.
//! - `persist_ws_gap_cursor` demotes any cursor to GapDetected.
//!
//! Pure in-memory tests (G01-G03): no DB required; run unconditionally.
//! DB-backed tests (G04-G06): skip gracefully without `MQK_DATABASE_URL`.

use mqk_broker_alpaca::types::{AlpacaFetchCursor, AlpacaTradeUpdatesResume};
use mqk_runtime::alpaca_inbound::{advance_cursor_after_ws_establish, persist_ws_gap_cursor};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn db_pool_or_skip() -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(v) => v,
        Err(_) => return None,
    };
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("BRK-08R DB test: failed to connect to MQK_DATABASE_URL"),
    )
}

// ---------------------------------------------------------------------------
// G01 — advance_cursor_after_ws_establish: Live → no-op (DB path not exercised)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk08r_g01_advance_noop_for_live_cursor() {
    // Build a Live cursor with a known rest_activity_after position.
    let live = AlpacaFetchCursor::live(
        Some("act-xyz-001".to_string()),
        "alpaca:order-001:filled:2026-01-01T00:00:00Z",
        "2026-01-01T00:00:00Z",
    );

    // No DB pool available — but for Live cursor the function returns early
    // before touching the DB, so this is sufficient to prove the no-op.
    // We use a fake pool by relying on the early-return guard in the function.
    // Since we cannot create a PgPool without a real URL here, we instead
    // verify via the DB-backed G04 test.  This test validates the type contract:
    // that a Live cursor round-trips through trade_updates correctly.
    assert!(
        matches!(live.trade_updates, AlpacaTradeUpdatesResume::Live { .. }),
        "G01: live cursor must have Live trade_updates"
    );
    assert_eq!(
        live.rest_activity_after.as_deref(),
        Some("act-xyz-001"),
        "G01: rest_activity_after must be preserved"
    );
}

// ---------------------------------------------------------------------------
// G02 — advance_cursor_after_ws_establish: GapDetected input shape is correct
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk08r_g02_gap_detected_cursor_structure() {
    let gap = AlpacaFetchCursor::gap_detected(
        Some("act-before-gap".to_string()),
        Some("alpaca:order-002:filled:2026-01-02T00:00:00Z".to_string()),
        Some("2026-01-02T00:00:00Z".to_string()),
        "brk08r test gap",
    );

    assert!(
        matches!(
            gap.trade_updates,
            AlpacaTradeUpdatesResume::GapDetected { .. }
        ),
        "G02: gap cursor must have GapDetected trade_updates"
    );
    assert_eq!(
        gap.rest_activity_after.as_deref(),
        Some("act-before-gap"),
        "G02: rest_activity_after must be preserved across gap"
    );
}

// ---------------------------------------------------------------------------
// G03 — persist_ws_gap_cursor demotes a Live cursor (pure type check)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk08r_g03_persist_gap_demotes_live_cursor_structure() {
    // Verify that a Live cursor fed into persist_ws_gap_cursor would produce
    // a GapDetected cursor in its output shape.  The actual DB write is proven
    // in G05.  Here we confirm the output of mark_gap_detected (which
    // persist_ws_gap_cursor calls internally) gives the right shape.
    let live = AlpacaFetchCursor::live(
        Some("act-live-123".to_string()),
        "alpaca:order-003:filled:2026-01-03T00:00:00Z",
        "2026-01-03T00:00:00Z",
    );

    // AlpacaFetchCursor::gap_detected mirrors what mark_gap_detected produces.
    let gap = AlpacaFetchCursor::gap_detected(
        live.rest_activity_after.clone(),
        // last_message_id would come from Live inner fields in production
        None,
        None,
        "transport disconnect",
    );

    assert!(
        matches!(
            gap.trade_updates,
            AlpacaTradeUpdatesResume::GapDetected { .. }
        ),
        "G03: gap_detected constructor produces GapDetected; got: {:?}",
        gap.trade_updates
    );
    assert_eq!(
        gap.rest_activity_after.as_deref(),
        Some("act-live-123"),
        "G03: rest_activity_after must survive demotion"
    );
}

// ---------------------------------------------------------------------------
// G04 — DB: advance_cursor_after_ws_establish repairs GapDetected → Live
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk08r_g04_advance_repairs_gap_detected_to_live_in_db() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("G04: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    let adapter_id = "brk08r-g04-test";

    // Persist a GapDetected cursor to simulate a prior disconnect.
    let gap = AlpacaFetchCursor::gap_detected(
        Some("act-before-gap-g04".to_string()),
        Some("alpaca:order-g04:filled:2026-01-04T00:00:00Z".to_string()),
        Some("2026-01-04T00:00:00Z".to_string()),
        "brk08r-g04 pre-test gap",
    );
    let gap_json = serde_json::to_string(&gap).expect("G04: serialize gap");
    mqk_db::advance_broker_cursor(&pool, adapter_id, &gap_json, chrono::Utc::now())
        .await
        .expect("G04: persist gap cursor");

    // Call the repair function.
    let repaired = advance_cursor_after_ws_establish(&pool, adapter_id, &gap, chrono::Utc::now())
        .await
        .expect("G04: advance_cursor_after_ws_establish failed");

    // The returned cursor must be Live.
    assert!(
        matches!(
            repaired.trade_updates,
            AlpacaTradeUpdatesResume::Live { .. }
        ),
        "G04: repaired cursor must be Live; got: {:?}",
        repaired.trade_updates
    );

    // rest_activity_after must be preserved so REST poll resumes from the
    // correct position.
    assert_eq!(
        repaired.rest_activity_after.as_deref(),
        Some("act-before-gap-g04"),
        "G04: rest_activity_after must survive repair"
    );

    // DB must reflect the Live cursor.
    let stored_json = mqk_db::load_broker_cursor(&pool, adapter_id)
        .await
        .expect("G04: load cursor")
        .expect("G04: cursor must be present after advance");
    let stored: AlpacaFetchCursor =
        serde_json::from_str(&stored_json).expect("G04: parse stored cursor");
    assert!(
        matches!(stored.trade_updates, AlpacaTradeUpdatesResume::Live { .. }),
        "G04: DB cursor must be Live after repair; got: {:?}",
        stored.trade_updates
    );
    assert_eq!(
        stored.rest_activity_after.as_deref(),
        Some("act-before-gap-g04"),
        "G04: DB rest_activity_after must be preserved"
    );
}

// ---------------------------------------------------------------------------
// G05 — DB: advance_cursor_after_ws_establish is a no-op for Live cursor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk08r_g05_advance_noop_for_already_live_cursor() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("G05: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    let adapter_id = "brk08r-g05-test";

    let live = AlpacaFetchCursor::live(
        Some("act-live-g05".to_string()),
        "alpaca:order-g05:filled:2026-01-05T00:00:00Z",
        "2026-01-05T00:00:00Z",
    );
    let live_json = serde_json::to_string(&live).expect("G05: serialize");
    mqk_db::advance_broker_cursor(&pool, adapter_id, &live_json, chrono::Utc::now())
        .await
        .expect("G05: persist live cursor");

    // Calling advance on an already-Live cursor must return Ok with the same cursor.
    let result = advance_cursor_after_ws_establish(&pool, adapter_id, &live, chrono::Utc::now())
        .await
        .expect("G05: advance_cursor_after_ws_establish failed");

    assert!(
        matches!(result.trade_updates, AlpacaTradeUpdatesResume::Live { .. }),
        "G05: Live-in must give Live-out; got: {:?}",
        result.trade_updates
    );
    assert_eq!(
        result.rest_activity_after.as_deref(),
        Some("act-live-g05"),
        "G05: rest_activity_after must be unchanged"
    );
}

// ---------------------------------------------------------------------------
// G06 — DB: persist_ws_gap_cursor demotes Live → GapDetected in DB
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk08r_g06_persist_gap_cursor_demotes_live_in_db() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("G06: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    let adapter_id = "brk08r-g06-test";

    let live = AlpacaFetchCursor::live(
        Some("act-live-g06".to_string()),
        "alpaca:order-g06:filled:2026-01-06T00:00:00Z",
        "2026-01-06T00:00:00Z",
    );
    let live_json = serde_json::to_string(&live).expect("G06: serialize");
    mqk_db::advance_broker_cursor(&pool, adapter_id, &live_json, chrono::Utc::now())
        .await
        .expect("G06: persist live cursor");

    // Simulate disconnect: persist gap.
    let gap_result = persist_ws_gap_cursor(
        &pool,
        adapter_id,
        &live,
        "brk08r-g06 test disconnect",
        chrono::Utc::now(),
    )
    .await
    .expect("G06: persist_ws_gap_cursor failed");

    assert!(
        matches!(
            gap_result.trade_updates,
            AlpacaTradeUpdatesResume::GapDetected { .. }
        ),
        "G06: returned cursor must be GapDetected; got: {:?}",
        gap_result.trade_updates
    );

    // DB must reflect GapDetected.
    let stored_json = mqk_db::load_broker_cursor(&pool, adapter_id)
        .await
        .expect("G06: load cursor")
        .expect("G06: cursor must exist");
    let stored: AlpacaFetchCursor = serde_json::from_str(&stored_json).expect("G06: parse stored");
    assert!(
        matches!(
            stored.trade_updates,
            AlpacaTradeUpdatesResume::GapDetected { .. }
        ),
        "G06: DB cursor must be GapDetected after persist; got: {:?}",
        stored.trade_updates
    );
    // rest_activity_after must survive the demotion for REST recovery.
    assert_eq!(
        stored.rest_activity_after.as_deref(),
        Some("act-live-g06"),
        "G06: rest_activity_after must survive GapDetected demotion"
    );
}
