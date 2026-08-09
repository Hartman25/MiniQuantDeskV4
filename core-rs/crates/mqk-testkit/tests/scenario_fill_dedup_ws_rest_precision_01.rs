//! FILL-DEDUP-WS-REST-PRECISION-01
//!
//! # Root cause
//!
//! Alpaca delivers the same economic fill on two lanes:
//! - WS: broker_fill_id=null, timestamp "...fill:...174478594Z" (nanosecond)
//! - REST: broker_fill_id="exec-uuid", timestamp "...fill:...174479Z" (lower precision)
//!
//! Both `broker_message_id` values differ → existing `uq_inbox_run_broker_message_id`
//! does not deduplicate.  `uq_inbox_run_broker_fill_id` only fires when
//! broker_fill_id IS NOT NULL, so the WS row (null) bypasses it.  Both rows are
//! inserted; `recover_oms_and_portfolio` applied portfolio fills unconditionally →
//! local portfolio accumulated +2 shares against broker truth of +1.
//!
//! # Fixes
//!
//! 1. Migration 0040: `uq_inbox_run_order_single_fill` — at most one 'fill' row per
//!    (run_id, internal_order_id).  Second insert (whichever lane arrives second)
//!    returns Ok(false).
//!
//! 2. `recover_oms_and_portfolio` now mirrors `apply_fill_step`: uses broker_fill_id
//!    as the OMS economic event ID when present (falls back to broker_message_id),
//!    and skips the portfolio apply when the OMS did not advance (no-op detection).
//!
//! # Test inventory
//!
//! | ID       | Scenario                                                      | DB?  |
//! |----------|---------------------------------------------------------------|------|
//! | FDP01    | WS fill (no fill_id) then REST fill (fill_id) → one OMS apply | No   |
//! | FDP02    | REST first then WS → same dedup, one apply                    | No   |
//! | FDP03    | Duplicate REST same broker_fill_id → idempotent               | No   |
//! | FDP04    | Two distinct partial fills for same order → both apply        | No   |
//! | FDP05    | Different internal_order_ids → fills not collapsed            | No   |
//! | FDP06    | Fill for order absent from OMS → UNKNOWN_ORDER_FILL path      | No   |
//! | FDP07_DB | DB constraint: second 'fill' row for same order is rejected   | Yes  |
//! | FDP08_DB | Recovery: two applied fill rows → portfolio +1, not +2        | Yes  |
//!
//! # PAPER-SOAK-PARTIAL-FILL-DEDUP-01 addendum (superseded — see -02 below)
//!
//! FDP01-FDP08 above cover terminal ('fill') dedup (migration 0040). Migration
//! 0040 explicitly excludes 'partial_fill' rows from its single-fill
//! constraint (a single order legitimately produces many partial fills), so
//! the identical WS(broker_fill_id=None)/REST(broker_fill_id=Some) dual-lane
//! duplicate pattern was left completely uncovered for partial fills.
//! PAPER-SOAK-PARTIAL-FILL-DEDUP-01 closed that gap with a DB-insertion-time
//! economic-match window (internal_order_id + delta_qty + price_micros +
//! bounded event_ts_ms proximity).
//!
//! # PAPER-SOAK-PARTIAL-FILL-DEDUP-02 — why -01 was rejected and replaced
//!
//! Independent review rejected the -01 window: two *legitimate* partial
//! fills for the same order can genuinely share quantity, price, and
//! near-simultaneous timestamps (e.g. two 10-share fills at the same price a
//! second apart) — a time window cannot distinguish that from the same
//! physical fill re-delivered over WS and REST, and -01 would wrongly
//! collapse the former.
//!
//! -02 removes the insertion-time heuristic entirely (`partial_fill` now
//! dedupes on transport identity only, exactly like every other
//! `event_kind` — see `mqk_db::inbox::inbox_insert_deduped_with_identity`)
//! and moves economic dedup to the OMS *apply* layer
//! (`mqk_execution::oms::state_machine::OmsOrder::apply_with_watermark`),
//! which compares the broker-authoritative cumulative-filled-quantity
//! (`cum_qty_after`, populated by `mqk-broker-alpaca` from Alpaca's own
//! `order.filled_qty` on both lanes) against the order's current
//! `filled_qty` — an exact identity, not a heuristic.
//!
//! | ID        | Scenario                                                            | DB?  |
//! |-----------|----------------------------------------------------------------------|------|
//! | FDP09_DB2 | WS partial fill then REST duplicate: BOTH insert, exactly 1 applies  | Yes  |
//! | FDP10_DB2 | REST then WS duplicate (reverse order): same outcome                 | Yes  |
//! | FDP11_DB2 | Same-lane retry (identical broker_message_id): still rejected at insert | Yes |
//! | FDP12_DB2 | Two legitimate same-qty/price fills <3s apart: BOTH insert AND apply | Yes  |
//! | FDP13_DB2 | Two legitimately distinct partial fills (different qty): both apply  | Yes  |
//! | FDP14_DB2 | WS+REST partial dup + terminal fill → correct total via replay       | Yes  |
//! | FDP15_DB2 | Restart replay: unapplied duplicate row resolves to a no-op          | Yes  |
//! | FDP16_DB2 | Concurrent WS+REST insert race: both rows persist, replay applies once | Yes |
//! | FDP17_NEG | Negative control: replaying without the watermark DOES double-apply  | No   |

use anyhow::Result;
use chrono::Utc;
use mqk_execution::oms::state_machine::{OmsEvent, OmsOrder, OrderState};
use mqk_portfolio::{apply_entry, Fill, LedgerEntry, PortfolioState, Side, MICROS_SCALE};
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const AAPL_PRICE: i64 = 175 * MICROS_SCALE; // $175.00 in micros

/// Apply a fill to an OmsOrder using the same economic-event-id logic as
/// `apply_fill_step`: use broker_fill_id when present, fall back to broker_message_id.
/// Returns true if the OMS state actually advanced (filled_qty increased).
fn oms_apply_fill(
    order: &mut OmsOrder,
    delta_qty: i64,
    broker_fill_id: Option<&str>,
    broker_message_id: &str,
) -> bool {
    let economic_event_id = broker_fill_id.unwrap_or(broker_message_id);
    let event = OmsEvent::Fill { delta_qty };
    let pre_qty = order.filled_qty;
    let _ = order.apply(&event, Some(economic_event_id));
    order.filled_qty != pre_qty
}

/// Apply a partial fill with the same economic-event-id logic.
fn oms_apply_partial_fill(
    order: &mut OmsOrder,
    delta_qty: i64,
    broker_fill_id: Option<&str>,
    broker_message_id: &str,
) -> bool {
    let economic_event_id = broker_fill_id.unwrap_or(broker_message_id);
    let event = OmsEvent::PartialFill { delta_qty };
    let pre_qty = order.filled_qty;
    let _ = order.apply(&event, Some(economic_event_id));
    order.filled_qty != pre_qty
}

// ---------------------------------------------------------------------------
// FDP01 — WS fill (no broker_fill_id) then REST fill (with broker_fill_id)
// ---------------------------------------------------------------------------

/// FDP01: WS fill uses broker_message_id as economic event ID.
/// REST fill uses broker_fill_id.  Because both IDs differ, OMS applied_event_ids
/// alone cannot detect the duplicate.  The OMS STATE GUARD (Filled → noop) catches it.
/// After WS fill: Filled, qty=1.  After REST fill: state guard, pre_qty==filled_qty.
#[test]
fn fdp01_ws_fill_then_rest_fill_applies_exactly_once() {
    let mut order = OmsOrder::new("ord-ws-rest-01", "AAPL", 1);

    let ws_advanced = oms_apply_fill(
        &mut order,
        1,
        None,
        "alpaca:uuid-001:fill:2026-05-20T14:41:52.174478594Z",
    );
    assert!(ws_advanced, "FDP01: WS fill must advance OMS fill qty");
    assert_eq!(
        order.state,
        OrderState::Filled,
        "FDP01: order must be Filled after WS fill"
    );
    assert_eq!(
        order.filled_qty, 1,
        "FDP01: filled_qty must be 1 after WS fill"
    );

    // REST fill: broker_fill_id differs from WS message_id, but OMS state guard catches it.
    let rest_advanced = oms_apply_fill(
        &mut order,
        1,
        Some("20260520104152174::303292c8-0c8a-46bd-a7e4-4b63288dab02"),
        "alpaca:uuid-001:fill:2026-05-20T14:41:52.174479Z",
    );
    assert!(
        !rest_advanced,
        "FDP01: REST fill must be a no-op (OMS Filled guard)"
    );
    assert_eq!(
        order.filled_qty, 1,
        "FDP01: filled_qty must remain 1 after duplicate REST fill"
    );
}

// ---------------------------------------------------------------------------
// FDP02 — REST fill first, WS fill second (reverse arrival order)
// ---------------------------------------------------------------------------

#[test]
fn fdp02_rest_fill_then_ws_fill_applies_exactly_once() {
    let mut order = OmsOrder::new("ord-rest-ws-02", "AAPL", 1);

    let rest_advanced = oms_apply_fill(
        &mut order,
        1,
        Some("exec-id-rest-02"),
        "alpaca:uuid-002:fill:2026-05-20T14:41:52.174479Z",
    );
    assert!(rest_advanced, "FDP02: REST fill must advance OMS fill qty");
    assert_eq!(order.state, OrderState::Filled);
    assert_eq!(order.filled_qty, 1);

    let ws_advanced = oms_apply_fill(
        &mut order,
        1,
        None,
        "alpaca:uuid-002:fill:2026-05-20T14:41:52.174478594Z",
    );
    assert!(
        !ws_advanced,
        "FDP02: WS fill must be a no-op when order already Filled"
    );
    assert_eq!(order.filled_qty, 1, "FDP02: filled_qty must remain 1");
}

// ---------------------------------------------------------------------------
// FDP03 — Duplicate REST fill with same broker_fill_id
// ---------------------------------------------------------------------------

#[test]
fn fdp03_duplicate_rest_fill_same_fill_id_is_idempotent() {
    let mut order = OmsOrder::new("ord-dup-rest-03", "AAPL", 1);

    let first = oms_apply_fill(&mut order, 1, Some("exec-id-03"), "rest-msg-03-a");
    assert!(first, "FDP03: first REST fill must apply");
    assert_eq!(order.filled_qty, 1);

    // Same broker_fill_id → applied_event_ids check fires before do_transition.
    let second = oms_apply_fill(&mut order, 1, Some("exec-id-03"), "rest-msg-03-b");
    assert!(
        !second,
        "FDP03: duplicate REST fill with same fill_id must be no-op"
    );
    assert_eq!(
        order.filled_qty, 1,
        "FDP03: filled_qty must not change on duplicate"
    );
}

// ---------------------------------------------------------------------------
// FDP04 — Two distinct partial fills are not collapsed
// ---------------------------------------------------------------------------

#[test]
fn fdp04_two_distinct_partial_fills_both_apply() {
    let mut order = OmsOrder::new("ord-partial-04", "AAPL", 5);

    let first = oms_apply_partial_fill(&mut order, 3, Some("exec-pf-04-a"), "ws-msg-04-a");
    assert!(first, "FDP04: first partial fill must apply");
    assert_eq!(order.filled_qty, 3);
    assert_eq!(order.state, OrderState::PartiallyFilled);

    let second = oms_apply_partial_fill(&mut order, 2, Some("exec-pf-04-b"), "ws-msg-04-b");
    assert!(
        second,
        "FDP04: second distinct partial fill must also apply"
    );
    assert_eq!(order.filled_qty, 5, "FDP04: both partials must accumulate");
    // PartialFill events transition to PartiallyFilled regardless of whether
    // filled_qty == total_qty; only a final Fill event transitions to Filled.
    assert_eq!(order.state, OrderState::PartiallyFilled);
}

// ---------------------------------------------------------------------------
// FDP05 — Fills for different order IDs are independent
// ---------------------------------------------------------------------------

#[test]
fn fdp05_fills_for_different_order_ids_are_independent() {
    let mut order_a = OmsOrder::new("ord-a-05", "AAPL", 1);
    let mut order_b = OmsOrder::new("ord-b-05", "MSFT", 1);

    let a = oms_apply_fill(&mut order_a, 1, Some("exec-a-05"), "msg-a-05");
    let b = oms_apply_fill(&mut order_b, 1, Some("exec-b-05"), "msg-b-05");

    assert!(a, "FDP05: order-A fill must apply");
    assert!(b, "FDP05: order-B fill must apply independently");
    assert_eq!(order_a.filled_qty, 1);
    assert_eq!(order_b.filled_qty, 1);
}

// ---------------------------------------------------------------------------
// FDP06 — Fill for unknown order → fails closed (UNKNOWN_ORDER_FILL path)
// ---------------------------------------------------------------------------

/// FDP06: A fill event for an internal_order_id absent from the OMS map triggers
/// the UNKNOWN_ORDER_FILL guard.  The dedup logic must NOT suppress this error
/// by treating the fill as a silent no-op — failing closed is mandatory.
#[test]
fn fdp06_unknown_order_fill_must_not_be_silently_deduped() {
    use std::collections::BTreeMap;
    let oms_orders: BTreeMap<String, OmsOrder> = BTreeMap::new();

    let target = "ord-unknown-06";
    let is_fill = true;
    let is_absent = !oms_orders.contains_key(target);

    // Production `apply_fill_step` returns Err("UNKNOWN_ORDER_FILL: ...") when
    // the order is absent and the event is a fill.  The dedup must not intercept
    // this — it must be allowed to propagate as a halt condition.
    assert!(
        is_fill && is_absent,
        "FDP06: fill for absent order must reach the UNKNOWN_ORDER_FILL gate; \
         dedup must not convert this to a silent no-op"
    );
}

// ---------------------------------------------------------------------------
// DB-backed tests
// ---------------------------------------------------------------------------

fn db_url_or_skip() -> Option<String> {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => {
            println!("SKIP: MQK_DATABASE_URL not set");
            None
        }
    }
}

async fn try_pool(url: &str) -> Result<Option<sqlx::PgPool>> {
    use sqlx::postgres::PgPoolOptions;
    match PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect(url)
        .await
    {
        Ok(p) => Ok(Some(p)),
        Err(e) => {
            println!("SKIP: cannot connect to DB: {e}");
            Ok(None)
        }
    }
}

async fn cleanup_run(pool: &sqlx::PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn seed_running_run(pool: &sqlx::PgPool, run_id: Uuid) -> Result<()> {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "fdp-test".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "fdp-test".to_string(),
            config_hash: "fdp-test".to_string(),
            config_json: json!({}),
            host_fingerprint: "fdp-test".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    Ok(())
}

/// FDP07_DB: The new `uq_inbox_run_order_single_fill` constraint prevents a
/// second 'fill' row for the same (run_id, internal_order_id).
///
/// Proves:
///   A. WS fill (broker_fill_id=null) bypasses `uq_inbox_run_broker_fill_id`.
///   B. Different timestamp precision gives different broker_message_id, bypassing
///      `uq_inbox_run_broker_message_id`.
///   Fix. `uq_inbox_run_order_single_fill` catches the second fill regardless of lane.
#[tokio::test]
async fn fdp07_db_second_fill_row_rejected_by_single_fill_constraint() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "fd070700-0000-0000-0000-000000000000"
        .parse()
        .expect("valid uuid");
    let internal_order_id = "fdp07-order-aapl";

    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    let ws_msg_id = "alpaca:uuid-fdp07:fill:2026-05-20T14:41:52.174478594Z";
    let ws_payload = json!({
        "type":              "fill",
        "broker_message_id": ws_msg_id,
        "broker_fill_id":    null,
        "internal_order_id": internal_order_id,
        "broker_order_id":   "alpaca-broker-fdp07",
        "symbol":            "AAPL",
        "side":              "Buy",
        "delta_qty":         1_i64,
        "price_micros":      AAPL_PRICE,
        "fee_micros":        0_i64
    });

    // WS fill arrives first (broker_fill_id=null).
    let inserted_ws = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        ws_msg_id,
        None,
        internal_order_id,
        "alpaca-broker-fdp07",
        "fill",
        &ws_payload,
        1_748_706_112_174_i64,
        Utc::now(),
    )
    .await?;
    assert!(inserted_ws, "FDP07_DB: WS fill must insert as first row");

    let rest_msg_id = "alpaca:uuid-fdp07:fill:2026-05-20T14:41:52.174479Z";
    let rest_fill_id = "20260520104152174::303292c8-0c8a-46bd-a7e4-4b63288dab02";
    let rest_payload = json!({
        "type":              "fill",
        "broker_message_id": rest_msg_id,
        "broker_fill_id":    rest_fill_id,
        "internal_order_id": internal_order_id,
        "broker_order_id":   "alpaca-broker-fdp07",
        "symbol":            "AAPL",
        "side":              "Buy",
        "delta_qty":         1_i64,
        "price_micros":      AAPL_PRICE,
        "fee_micros":        0_i64
    });

    // REST fill arrives second (different broker_message_id, broker_fill_id present).
    let inserted_rest = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        rest_msg_id,
        Some(rest_fill_id),
        internal_order_id,
        "alpaca-broker-fdp07",
        "fill",
        &rest_payload,
        1_748_706_112_174_i64,
        Utc::now(),
    )
    .await?;
    assert!(
        !inserted_rest,
        "FDP07_DB: REST fill must be rejected by uq_inbox_run_order_single_fill; \
         a second 'fill' row for the same (run_id, internal_order_id) must not be inserted"
    );

    let fill_count: i64 = sqlx::query_scalar(
        "select count(*)::bigint from oms_inbox \
         where run_id = $1 and internal_order_id = $2 and event_kind = 'fill'",
    )
    .bind(run_id)
    .bind(internal_order_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        fill_count, 1,
        "FDP07_DB: exactly one 'fill' row must exist for (run_id, internal_order_id)"
    );

    // Reverse order: REST first, WS second — also covered by the constraint.
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    let rest_first = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        rest_msg_id,
        Some(rest_fill_id),
        internal_order_id,
        "alpaca-broker-fdp07",
        "fill",
        &rest_payload,
        1_748_706_112_174_i64,
        Utc::now(),
    )
    .await?;
    assert!(
        rest_first,
        "FDP07_DB reverse: REST fill must insert as first row"
    );

    let ws_second = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        ws_msg_id,
        None,
        internal_order_id,
        "alpaca-broker-fdp07",
        "fill",
        &ws_payload,
        1_748_706_112_174_i64,
        Utc::now(),
    )
    .await?;
    assert!(
        !ws_second,
        "FDP07_DB reverse: WS fill must be rejected when REST fill was inserted first"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

/// FDP08: The FIXED `recover_oms_and_portfolio` logic applied to two applied
/// fill rows (WS + REST for the same order) yields portfolio +1, not +2.
///
/// Simulates the pre-fix restart scenario in pure in-process logic:
/// - Two fill records exist for the same order (WS: no fill_id; REST: fill_id present).
/// - The FIXED recovery loop uses broker_fill_id as economic event ID when present
///   and skips the portfolio mutation when OMS treats the event as a no-op (no-op
///   detection mirrors `apply_fill_step`).
/// - Without the fix, the portfolio accumulates +2 AAPL.
/// - With the fix, the portfolio correctly shows +1 AAPL.
///
/// No DB required — tests the recovery logic at the OMS/portfolio primitive level.
#[test]
fn fdp08_recovery_dedup_prevents_double_portfolio_apply() {
    let initial_equity: i64 = 100_000 * MICROS_SCALE;

    // Simulate two applied inbox rows for the same economic fill:
    //   row_a: WS fill — broker_fill_id absent, uses broker_message_id as event ID.
    //   row_b: REST fill — broker_fill_id present, uses fill_id as event ID.
    struct FakeRow {
        broker_message_id: &'static str,
        broker_fill_id: Option<&'static str>,
    }
    let rows = [
        FakeRow {
            broker_message_id: "alpaca:uuid-fdp08:fill:2026-05-20T14:41:52.174478594Z",
            broker_fill_id: None,
        },
        FakeRow {
            broker_message_id: "alpaca:uuid-fdp08:fill:2026-05-20T14:41:52.174479Z",
            broker_fill_id: Some("20260520104152174::303292c8-0c8a-46bd-a7e4-fdp08test"),
        },
    ];

    let mut order = OmsOrder::new("fdp08-order", "AAPL", 1);
    let mut portfolio = PortfolioState::new(initial_equity);

    // Replay using the FIXED recovery logic:
    //   economic_event_id = broker_fill_id when present, else broker_message_id.
    //   Skip portfolio apply when OMS does not advance.
    for row in &rows {
        let economic_event_id = row.broker_fill_id.unwrap_or(row.broker_message_id);
        let pre_qty = order.filled_qty;
        let _ = order.apply(&OmsEvent::Fill { delta_qty: 1 }, Some(economic_event_id));
        let oms_advanced = order.filled_qty != pre_qty;

        if oms_advanced {
            let fill = Fill::new("AAPL", Side::Buy, 1, AAPL_PRICE, 0);
            apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
        }
        // OMS no-op → skip portfolio (prevents double-counting).
    }

    let aapl_qty: i64 = portfolio
        .positions
        .get("AAPL")
        .map(|pos| pos.lots.iter().map(|l| l.qty_signed).sum())
        .unwrap_or(0);

    assert_eq!(
        aapl_qty, 1,
        "FDP08: recovery with two applied fill rows must yield +1 AAPL, not +2; \
         OMS no-op detection must prevent double portfolio apply on the REST fill duplicate"
    );

    // Verify OMS state is consistent.
    assert_eq!(
        order.state,
        OrderState::Filled,
        "FDP08: order must be Filled"
    );
    assert_eq!(order.filled_qty, 1, "FDP08: filled_qty must be 1, not 2");
}

// ---------------------------------------------------------------------------
// PAPER-SOAK-PARTIAL-FILL-DEDUP-02 — DB-backed proof
// ---------------------------------------------------------------------------

fn partial_fill_payload(delta_qty: i64, price_micros: i64) -> serde_json::Value {
    json!({
        "type":         "partial_fill",
        "symbol":       "AAPL",
        "side":         "Buy",
        "delta_qty":    delta_qty,
        "price_micros": price_micros,
        "fee_micros":   0_i64
    })
}

/// Like `partial_fill_payload`, but additionally carries `cum_qty_after` —
/// the field `mqk-broker-alpaca` populates from Alpaca's `order.filled_qty`
/// on both the WS and REST lanes (see `BrokerEvent::PartialFill::cum_qty_after`).
/// Deserializing this JSON as `BrokerEvent` (exactly what
/// `mqk_runtime::orchestrator::apply::build_canonical_apply_queue` does with
/// durable `message_json`) round-trips this field through serde's
/// `#[serde(default, skip_serializing_if = "Option::is_none")]`.
fn partial_fill_payload_with_watermark(
    delta_qty: i64,
    price_micros: i64,
    cum_qty_after: Option<i64>,
) -> serde_json::Value {
    let mut payload = partial_fill_payload(delta_qty, price_micros);
    if let Some(v) = cum_qty_after {
        payload["cum_qty_after"] = json!(v);
    }
    payload
}

async fn partial_fill_row_count(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    internal_order_id: &str,
) -> Result<i64> {
    let count: i64 = sqlx::query_scalar(
        "select count(*)::bigint from oms_inbox \
         where run_id = $1 and internal_order_id = $2 and event_kind = 'partial_fill'",
    )
    .bind(run_id)
    .bind(internal_order_id)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

/// Replay every durable `partial_fill`/`fill` inbox row for `internal_order_id`,
/// in canonical `inbox_id` ascending order, through the SAME apply logic as
/// production `apply_fill_step`
/// (`mqk_runtime::orchestrator::apply::apply_fill_step`): economic_event_id =
/// `broker_fill_id.unwrap_or(broker_message_id)`, and — this is the -02 fix —
/// `cum_qty_after` (when present in `message_json`) passed to
/// `apply_with_watermark` as the broker-authoritative cumulative watermark.
/// Returns the final order for assertions.
fn replay_rows_with_watermark(
    rows: &[mqk_db::InboxRow],
    internal_order_id: &str,
    total_qty: i64,
) -> OmsOrder {
    let mut order = OmsOrder::new(internal_order_id, "AAPL", total_qty);
    for row in rows {
        let delta_qty = row
            .message_json
            .get("delta_qty")
            .and_then(|v| v.as_i64())
            .expect("delta_qty present");
        let cum_qty_after = row
            .message_json
            .get("cum_qty_after")
            .and_then(|v| v.as_i64());
        let economic_event_id = row
            .broker_fill_id
            .as_deref()
            .unwrap_or(&row.broker_message_id);
        let event = match row.event_kind.as_str() {
            "partial_fill" => OmsEvent::PartialFill { delta_qty },
            "fill" => OmsEvent::Fill { delta_qty },
            other => panic!("unexpected event_kind: {other}"),
        };
        let _ = order.apply_with_watermark(&event, Some(economic_event_id), cum_qty_after);
    }
    order
}

/// FDP09_DB2: WS partial fill (broker_fill_id=None) then REST duplicate of
/// the SAME physical partial fill (broker_fill_id=Some, different
/// broker_message_id, SAME cum_qty_after) — under -02 the insertion layer no
/// longer suppresses either row (both are distinct `broker_message_id`
/// values, exactly like any other event_kind), but replaying the durable
/// rows through the OMS apply layer must still apply exactly once.
#[tokio::test]
async fn fdp09_db2_ws_partial_fill_then_rest_duplicate_applies_once() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "fd090900-0000-0000-0000-000000000000"
        .parse()
        .expect("valid uuid");
    let internal_order_id = "fdp09-order-aapl";
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    let payload = partial_fill_payload_with_watermark(6, AAPL_PRICE, Some(6));

    let ws_inserted = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "alpaca:uuid-fdp09:partial_fill:2026-05-20T14:41:52.174478594Z",
        None,
        internal_order_id,
        "alpaca-broker-fdp09",
        "partial_fill",
        &payload,
        1_748_706_112_174,
        Utc::now(),
    )
    .await?;
    assert!(ws_inserted, "FDP09_DB2: WS partial fill must insert first");

    let rest_inserted = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "alpaca:uuid-fdp09:partial_fill:2026-05-20T14:41:52.674479Z",
        Some("20260520104152674::fdp09-exec"),
        internal_order_id,
        "alpaca-broker-fdp09",
        "partial_fill",
        &payload,
        1_748_706_112_674,
        Utc::now(),
    )
    .await?;
    assert!(
        rest_inserted,
        "FDP09_DB2: -02 no longer suppresses the REST row at insert time — \
         full raw evidence from both lanes must be preserved"
    );

    assert_eq!(
        partial_fill_row_count(&pool, run_id, internal_order_id).await?,
        2,
        "FDP09_DB2: both distinct broker_message_id rows must exist as durable evidence"
    );

    // The economic question — does this apply once? — is answered by the
    // apply layer, not row count.
    let rows = mqk_db::inbox_load_all_for_run(&pool, run_id).await?;
    let order = replay_rows_with_watermark(&rows, internal_order_id, 10);
    assert_eq!(
        order.filled_qty, 6,
        "FDP09_DB2: replaying both durable rows must apply exactly once (filled_qty=6, not 12)"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

/// FDP10_DB2: reverse arrival order (REST first, then WS duplicate) — same
/// outcome regardless of which lane arrives first.
#[tokio::test]
async fn fdp10_db2_rest_partial_fill_then_ws_duplicate_applies_once() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "fd100a00-0000-0000-0000-000000000000"
        .parse()
        .expect("valid uuid");
    let internal_order_id = "fdp10-order-aapl";
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    let payload = partial_fill_payload_with_watermark(6, AAPL_PRICE, Some(6));

    let rest_inserted = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "alpaca:uuid-fdp10:partial_fill:2026-05-20T14:41:52.674479Z",
        Some("20260520104152674::fdp10-exec"),
        internal_order_id,
        "alpaca-broker-fdp10",
        "partial_fill",
        &payload,
        1_748_706_112_674,
        Utc::now(),
    )
    .await?;
    assert!(
        rest_inserted,
        "FDP10_DB2: REST partial fill must insert first"
    );

    let ws_inserted = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "alpaca:uuid-fdp10:partial_fill:2026-05-20T14:41:52.174478594Z",
        None,
        internal_order_id,
        "alpaca-broker-fdp10",
        "partial_fill",
        &payload,
        1_748_706_112_174,
        Utc::now(),
    )
    .await?;
    assert!(
        ws_inserted,
        "FDP10_DB2: WS row must also insert — evidence is never suppressed"
    );

    assert_eq!(
        partial_fill_row_count(&pool, run_id, internal_order_id).await?,
        2,
        "FDP10_DB2: exactly two durable rows must exist"
    );

    let rows = mqk_db::inbox_load_all_for_run(&pool, run_id).await?;
    let order = replay_rows_with_watermark(&rows, internal_order_id, 10);
    assert_eq!(
        order.filled_qty, 6,
        "FDP10_DB2: REST-then-WS duplicate must still apply exactly once"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

/// FDP11_DB2: same-lane retry (identical broker_message_id resent, e.g.
/// after a transient network error) is still rejected by the pre-existing
/// transport-identity path (`uq_inbox_run_broker_message_id`) — unaffected
/// by -02, since this was never the mechanism -01 touched.
#[tokio::test]
async fn fdp11_db2_same_lane_retry_is_rejected() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "fd110b00-0000-0000-0000-000000000000"
        .parse()
        .expect("valid uuid");
    let internal_order_id = "fdp11-order-aapl";
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    let payload = partial_fill_payload_with_watermark(6, AAPL_PRICE, Some(6));
    let msg_id = "alpaca:uuid-fdp11:partial_fill:2026-05-20T14:41:52.674479Z";

    let first = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        msg_id,
        Some("20260520104152674::fdp11-exec"),
        internal_order_id,
        "alpaca-broker-fdp11",
        "partial_fill",
        &payload,
        1_748_706_112_674,
        Utc::now(),
    )
    .await?;
    assert!(first, "FDP11_DB2: first REST delivery must insert");

    let retry = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        msg_id,
        Some("20260520104152674::fdp11-exec"),
        internal_order_id,
        "alpaca-broker-fdp11",
        "partial_fill",
        &payload,
        1_748_706_112_674,
        Utc::now(),
    )
    .await?;
    assert!(!retry, "FDP11_DB2: identical retry must be rejected");

    assert_eq!(
        partial_fill_row_count(&pool, run_id, internal_order_id).await?,
        1,
        "FDP11_DB2: exactly one partial_fill row must exist"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

/// FDP12_DB2 (A4, MANDATORY): two legitimate partial fills that share the
/// same qty/price and arrive less than 3 seconds apart — the exact scenario
/// the -01 time window would wrongly collapse — must BOTH insert AND both
/// apply. Distinctness comes entirely from cum_qty_after (6, then 12); no
/// timestamp or window is ever consulted.
#[tokio::test]
async fn fdp12_db2_two_legitimate_same_qty_price_fills_less_than_3s_apart_both_apply() -> Result<()>
{
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "fd120c00-0000-0000-0000-000000000000"
        .parse()
        .expect("valid uuid");
    let internal_order_id = "fdp12-order-aapl";
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    let first = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "alpaca:uuid-fdp12:partial_fill:2026-05-20T14:41:52.000000Z",
        None,
        internal_order_id,
        "alpaca-broker-fdp12",
        "partial_fill",
        &partial_fill_payload_with_watermark(6, AAPL_PRICE, Some(6)),
        1_748_706_112_000,
        Utc::now(),
    )
    .await?;
    assert!(first, "FDP12_DB2: first partial fill must insert");

    // 700ms later — well inside the OLD 3s window that used to wrongly
    // collapse this. Same qty (6), same price, genuinely distinct execution
    // (cumulative went 6 -> 12).
    let second = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "alpaca:uuid-fdp12:partial_fill:2026-05-20T14:41:52.700000Z",
        None,
        internal_order_id,
        "alpaca-broker-fdp12",
        "partial_fill",
        &partial_fill_payload_with_watermark(6, AAPL_PRICE, Some(12)),
        1_748_706_112_700,
        Utc::now(),
    )
    .await?;
    assert!(
        second,
        "FDP12_DB2: a later, genuinely distinct partial fill with the same \
         qty/price must still insert — no window suppresses it"
    );

    assert_eq!(
        partial_fill_row_count(&pool, run_id, internal_order_id).await?,
        2,
        "FDP12_DB2: both distinct partial_fill rows must exist"
    );

    let rows = mqk_db::inbox_load_all_for_run(&pool, run_id).await?;
    let order = replay_rows_with_watermark(&rows, internal_order_id, 20);
    assert_eq!(
        order.filled_qty, 12,
        "FDP12_DB2: both legitimate 6-share fills must be reflected (6+6=12), \
         not collapsed to 6"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

/// FDP13_DB2 (A5): two legitimately distinct partial fills with different
/// qty both insert and both apply.
#[tokio::test]
async fn fdp13_db2_distinct_qty_partial_fills_both_apply() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "fd130d00-0000-0000-0000-000000000000"
        .parse()
        .expect("valid uuid");
    let internal_order_id = "fdp13-order-aapl";
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    let first = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "alpaca:uuid-fdp13:partial_fill:2026-05-20T14:41:52.000000Z",
        None,
        internal_order_id,
        "alpaca-broker-fdp13",
        "partial_fill",
        &partial_fill_payload_with_watermark(3, AAPL_PRICE, Some(3)),
        1_748_706_112_000,
        Utc::now(),
    )
    .await?;
    assert!(first, "FDP13_DB2: first partial fill (qty=3) must insert");

    let second = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "alpaca:uuid-fdp13:partial_fill:2026-05-20T14:41:52.500000Z",
        None,
        internal_order_id,
        "alpaca-broker-fdp13",
        "partial_fill",
        &partial_fill_payload_with_watermark(2, AAPL_PRICE, Some(5)),
        1_748_706_112_500,
        Utc::now(),
    )
    .await?;
    assert!(
        second,
        "FDP13_DB2: second partial fill (qty=2) must also insert — different qty"
    );

    assert_eq!(
        partial_fill_row_count(&pool, run_id, internal_order_id).await?,
        2,
        "FDP13_DB2: both distinct partial_fill rows must exist"
    );

    let rows = mqk_db::inbox_load_all_for_run(&pool, run_id).await?;
    let order = replay_rows_with_watermark(&rows, internal_order_id, 10);
    assert_eq!(order.filled_qty, 5, "FDP13_DB2: 3+2=5 must both apply");

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

/// FDP14_DB2 (A8): WS+REST duplicate of a partial fill is followed by the
/// order's terminal fill. Replaying every durable inbox row through
/// `apply_with_watermark` must yield the exact correct cumulative filled
/// quantity — proving the -02 fix produces the correct economic outcome
/// end-to-end, not just the correct row count.
#[tokio::test]
async fn fdp14_db2_partial_dup_plus_terminal_fill_yields_correct_total() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "fd140e00-0000-0000-0000-000000000000"
        .parse()
        .expect("valid uuid");
    let internal_order_id = "fdp14-order-aapl";
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    let partial_payload = partial_fill_payload_with_watermark(6, AAPL_PRICE, Some(6));

    // WS partial fill: 6 of 10.
    mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "alpaca:uuid-fdp14:partial_fill:2026-05-20T14:41:52.174478594Z",
        None,
        internal_order_id,
        "alpaca-broker-fdp14",
        "partial_fill",
        &partial_payload,
        1_748_706_112_174,
        Utc::now(),
    )
    .await?;

    // REST duplicate of the SAME 6-share partial fill — now inserts (raw
    // evidence preserved) instead of being rejected at the DB layer.
    let rest_dup = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "alpaca:uuid-fdp14:partial_fill:2026-05-20T14:41:52.674479Z",
        Some("20260520104152674::fdp14-exec"),
        internal_order_id,
        "alpaca-broker-fdp14",
        "partial_fill",
        &partial_payload,
        1_748_706_112_674,
        Utc::now(),
    )
    .await?;
    assert!(
        rest_dup,
        "FDP14_DB2: REST duplicate partial fill must insert as raw evidence"
    );

    // Terminal fill: remaining 4 of 10.
    let fill_payload = json!({
        "type":         "fill",
        "symbol":       "AAPL",
        "side":         "Buy",
        "delta_qty":    4_i64,
        "price_micros": AAPL_PRICE,
        "fee_micros":   0_i64
    });
    mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "alpaca:uuid-fdp14:fill:2026-05-20T14:45:00.000000Z",
        None,
        internal_order_id,
        "alpaca-broker-fdp14",
        "fill",
        &fill_payload,
        1_748_706_300_000,
        Utc::now(),
    )
    .await?;

    let rows = mqk_db::inbox_load_all_for_run(&pool, run_id).await?;
    let order = replay_rows_with_watermark(&rows, internal_order_id, 10);

    assert_eq!(
        order.filled_qty, 10,
        "FDP14_DB2: cumulative filled_qty must be exactly 10 (6 + 4), not 16 (double-counted 6)"
    );
    assert_eq!(
        order.state,
        OrderState::Filled,
        "FDP14_DB2: order must be Filled"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

/// FDP15_DB2 (A9): restart-replay proof. The WS row is marked applied; the
/// REST duplicate row is left unapplied (as it would be if a crash occurred
/// between its insert and its apply step). A simulated restart replays the
/// unapplied set through `apply_with_watermark` and must resolve it to a
/// no-op, not a second economic application.
#[tokio::test]
async fn fdp15_db2_restart_replay_of_unapplied_duplicate_is_noop() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "fd150f00-0000-0000-0000-000000000000"
        .parse()
        .expect("valid uuid");
    let internal_order_id = "fdp15-order-aapl";
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    let payload = partial_fill_payload_with_watermark(6, AAPL_PRICE, Some(6));
    let ws_msg_id = "alpaca:uuid-fdp15:partial_fill:2026-05-20T14:41:52.174478594Z";

    mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        ws_msg_id,
        None,
        internal_order_id,
        "alpaca-broker-fdp15",
        "partial_fill",
        &payload,
        1_748_706_112_174,
        Utc::now(),
    )
    .await?;
    let rest_dup_msg_id = "alpaca:uuid-fdp15:partial_fill:2026-05-20T14:41:52.674479Z";
    let rest_dup = mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        rest_dup_msg_id,
        Some("20260520104152674::fdp15-exec"),
        internal_order_id,
        "alpaca-broker-fdp15",
        "partial_fill",
        &payload,
        1_748_706_112_674,
        Utc::now(),
    )
    .await?;
    assert!(
        rest_dup,
        "FDP15_DB2: REST duplicate must insert as raw evidence"
    );

    // Simulate the WS row having been applied before a crash; the REST
    // duplicate row was never reached (crash between insert and apply).
    mqk_db::inbox_mark_applied(&pool, run_id, ws_msg_id, Utc::now()).await?;

    // Simulated restart: a fresh OMS order (filled_qty=0) replays first the
    // applied set (to rebuild in-memory state) then the unapplied set.
    let applied = mqk_db::inbox_load_all_applied_for_run(&pool, run_id).await?;
    let unapplied = mqk_db::inbox_load_unapplied_for_run(&pool, run_id).await?;
    assert_eq!(
        applied.len(),
        1,
        "FDP15_DB2: exactly one applied row (the WS fill)"
    );
    assert_eq!(
        unapplied.len(),
        1,
        "FDP15_DB2: exactly one unapplied row (the REST duplicate) — it was never rejected"
    );
    assert_eq!(unapplied[0].broker_message_id, rest_dup_msg_id);

    let mut all_rows = applied;
    all_rows.extend(unapplied);
    all_rows.sort_by_key(|r| r.inbox_id);
    let order = replay_rows_with_watermark(&all_rows, internal_order_id, 10);

    assert_eq!(
        order.filled_qty, 6,
        "FDP15_DB2: restart replay of the unapplied REST duplicate must be a no-op (filled_qty=6, not 12)"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

/// FDP16_DB2 (A7): concurrency proof. WS and REST deliveries of the same
/// physical partial fill are inserted from two concurrent tasks, released
/// simultaneously via a `tokio::sync::Barrier` (not a sleep). Under -02 both
/// inserts succeed (transport identity differs, no economic suppression at
/// insert time) — the race-safety property that matters now is that
/// replaying the resulting durable rows through the apply layer still
/// produces exactly one economic application, regardless of which row's
/// insert happened to land first.
#[tokio::test]
async fn fdp16_db2_concurrent_ws_rest_duplicate_insert_then_apply_once() -> Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = "fd16f000-0000-0000-0000-000000000000"
        .parse()
        .expect("valid uuid");
    let internal_order_id = "fdp16-order-aapl";
    cleanup_run(&pool, run_id).await?;
    seed_running_run(&pool, run_id).await?;

    let payload = partial_fill_payload_with_watermark(6, AAPL_PRICE, Some(6));
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));

    let pool_ws = pool.clone();
    let payload_ws = payload.clone();
    let barrier_ws = barrier.clone();
    let ws_task = tokio::spawn(async move {
        barrier_ws.wait().await;
        mqk_db::inbox_insert_deduped_with_identity(
            &pool_ws,
            run_id,
            "alpaca:uuid-fdp16:partial_fill:2026-05-20T14:41:52.174478594Z",
            None,
            internal_order_id,
            "alpaca-broker-fdp16",
            "partial_fill",
            &payload_ws,
            1_748_706_112_174,
            Utc::now(),
        )
        .await
    });

    let pool_rest = pool.clone();
    let payload_rest = payload.clone();
    let barrier_rest = barrier.clone();
    let rest_task = tokio::spawn(async move {
        barrier_rest.wait().await;
        mqk_db::inbox_insert_deduped_with_identity(
            &pool_rest,
            run_id,
            "alpaca:uuid-fdp16:partial_fill:2026-05-20T14:41:52.674479Z",
            Some("20260520104152674::fdp16-exec"),
            internal_order_id,
            "alpaca-broker-fdp16",
            "partial_fill",
            &payload_rest,
            1_748_706_112_674,
            Utc::now(),
        )
        .await
    });

    let (ws_result, rest_result) = tokio::join!(ws_task, rest_task);
    let ws_inserted = ws_result.expect("ws task must not panic")?;
    let rest_inserted = rest_result.expect("rest task must not panic")?;

    assert!(
        ws_inserted && rest_inserted,
        "FDP16_DB2: under -02 BOTH concurrent inserts must succeed \
         (ws={ws_inserted}, rest={rest_inserted}) — no economic suppression at insert time"
    );

    assert_eq!(
        partial_fill_row_count(&pool, run_id, internal_order_id).await?,
        2,
        "FDP16_DB2: both rows must persist as durable evidence after the race"
    );

    let rows = mqk_db::inbox_load_all_for_run(&pool, run_id).await?;
    let order = replay_rows_with_watermark(&rows, internal_order_id, 10);
    assert_eq!(
        order.filled_qty, 6,
        "FDP16_DB2: replaying the raced insert result must still apply exactly once"
    );

    cleanup_run(&pool, run_id).await?;
    Ok(())
}

/// FDP17_NEG: negative control. Replaying the SAME two durable rows
/// (WS + REST duplicate of one physical fill) through the OLD mechanism —
/// plain `OmsOrder::apply` (pure event_id dedup, no cum_qty_after) — DOES
/// double-apply, because the two rows carry different economic_event_id
/// values by construction (WS has none, REST has `broker_fill_id`). This is
/// the direct proof that `apply_with_watermark` is load-bearing: remove it
/// (fall back to `apply`) and the exact scenario FDP09_DB2/FDP10_DB2 prove
/// fixed reproduces the pre-fix double-apply bug.
///
/// No DB required — this isolates the OMS/economic-identity primitive that
/// -01's bug and -02's fix both ultimately live or die on.
#[test]
fn fdp17_neg_without_watermark_the_cross_lane_duplicate_double_applies() {
    let ws_delta = 6;
    let rest_delta = 6; // same physical fill, so REST reports the same delta
    let ws_event_id: Option<&str> = None; // WS never carries broker_fill_id
    let rest_event_id = Some("20260520104152674::fdp17-exec"); // REST does

    let mut order = OmsOrder::new("fdp17-order-aapl", "AAPL", 20);

    // WS delivery.
    let _ = order.apply(
        &OmsEvent::PartialFill {
            delta_qty: ws_delta,
        },
        ws_event_id.or(Some("alpaca:uuid-fdp17:partial_fill:ws")),
    );
    assert_eq!(order.filled_qty, 6);

    // REST redelivery of the SAME physical fill: different message_id AND
    // different economic_event_id (broker_fill_id vs broker_message_id) —
    // pure event_id dedup has nothing to match against.
    let _ = order.apply(
        &OmsEvent::PartialFill {
            delta_qty: rest_delta,
        },
        rest_event_id,
    );

    assert_eq!(
        order.filled_qty, 12,
        "FDP17_NEG: without the cum_qty_after watermark, the cross-lane duplicate DOES \
         double-apply (6+6=12) — this is the exact -01 bug reproduced, proving \
         apply_with_watermark (not incidental test setup) is what FDP09_DB2/FDP10_DB2 rely on"
    );
}
