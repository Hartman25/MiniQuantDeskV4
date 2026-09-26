//! TV-EXEC-01B — Orchestrator-driven fill-quality telemetry write path.
//!
//! Closes the proof gap left by TV-EXEC-01 fixture tests (FQ-01..FQ-05), which
//! all used `insert_fill_quality_telemetry` directly and did NOT exercise the
//! real orchestrator write path.
//!
//! Each test below drives a real `ExecutionOrchestrator::tick()` call and then
//! asserts on the `fill_quality_telemetry` DB rows produced by Phase 3b.
//!
//! ## Test inventory
//!
//! | ID     | Scenario                                     | Write path proven |
//! |--------|----------------------------------------------|-------------------|
//! | FQB-01 | Limit-order fill → row with reference price  | Phase 3b → DB     |
//! | FQB-02 | Market-order fill → row, null reference      | Phase 3b → DB     |
//! | FQB-03 | CancelAck (non-fill) → zero rows written     | No telemetry row  |
//! | FQB-04 | PartialFill then Fill → two rows             | Phase 3b × 2      |
//!
//! Each test runs in its own disposable database (`mqk_db::run_isolated`),
//! dropped even when the test fails, so a failing scenario can never leave the
//! singleton runtime lease behind in the shared test database. Emits a
//! `SKIP_DB` marker and does nothing when `MQK_DATABASE_URL` is absent.

use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use sqlx::PgPool;
use std::collections::BTreeMap;
use uuid::Uuid;

use mqk_db::FixedClock;
use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerEvent, BrokerGateway,
    BrokerInvokeToken, BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse,
    BrokerSubmitRequest, BrokerSubmitResponse, IntegrityGate, QtyMicros, ReconcileGate, RiskGate,
    Side,
};
use mqk_portfolio::PortfolioState;
use mqk_runtime::orchestrator::ExecutionOrchestrator;

// ---------------------------------------------------------------------------
// Fixed run UUIDs
// ---------------------------------------------------------------------------

const FQB01_RUN_ID: &str = "1b010001-0000-0000-0000-000000000000";
const FQB02_RUN_ID: &str = "1b020002-0000-0000-0000-000000000000";
const FQB03_RUN_ID: &str = "1b030003-0000-0000-0000-000000000000";
const FQB04_RUN_ID: &str = "1b040004-0000-0000-0000-000000000000";

// ---------------------------------------------------------------------------
// Stubs
// ---------------------------------------------------------------------------

struct NullBroker;

impl BrokerAdapter for NullBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        Ok(BrokerSubmitResponse {
            broker_order_id: format!("null-{}", req.order_id),
            submitted_at: 1,
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
            cancelled_at: 1,
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
            replaced_at: 1,
            status: "ok".to_string(),
        })
    }

    fn fetch_events(
        &self,
        _cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<(Vec<mqk_execution::BrokerEvent>, Option<String>), BrokerError> {
        Ok((vec![], None))
    }
}

struct PassGate;

impl IntegrityGate for PassGate {
    fn is_armed(&self) -> bool {
        true
    }
}
impl RiskGate for PassGate {
    fn evaluate_gate(&self) -> mqk_execution::RiskDecision {
        mqk_execution::RiskDecision::Allow
    }
}
impl ReconcileGate for PassGate {
    fn is_clean(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Seed a run in RUNNING state: CREATED → ARMED → RUNNING.
async fn seed_running_run(pool: &PgPool, run_id: Uuid, tag: &str) -> Result<()> {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: format!("{tag}-test"),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: format!("{tag}-test"),
            config_hash: format!("{tag}-test"),
            config_json: json!({}),
            host_fingerprint: format!("{tag}-test"),
        },
    )
    .await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    Ok(())
}

/// Build a clean orchestrator (no OMS orders, honest portfolio).
///
/// Phase 1 of tick() will register OMS orders from PENDING outbox rows via
/// NullBroker, so the caller only needs to enqueue outbox rows before tick().
fn make_orchestrator(
    pool: PgPool,
    run_id: Uuid,
    initial_cash_micros: i64,
) -> ExecutionOrchestrator<NullBroker, PassGate, PassGate, PassGate, FixedClock> {
    let gateway = BrokerGateway::for_test(NullBroker, PassGate, PassGate, PassGate);
    let portfolio = PortfolioState::new(initial_cash_micros);
    ExecutionOrchestrator::new(
        pool,
        gateway,
        BrokerOrderMap::new(),
        BTreeMap::new(),
        portfolio,
        run_id,
        "fqb-dispatcher",
        "test",
        None,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
    )
}

// ---------------------------------------------------------------------------
// Fixture: current-epoch broker-event JSON
// ---------------------------------------------------------------------------

enum FillKind {
    Partial,
    Final,
}

/// `message_json` for a broker fill, built through the production
/// `BrokerEvent` type so quantities are `QtyMicros` on the wire. The inbox
/// writer stamps the current schema version; a hand-written `"delta_qty": 1`
/// there would decode as 0.000001 units, not 1 share.
fn broker_fill_json(
    kind: FillKind,
    broker_message_id: &str,
    internal_order_id: &str,
    whole_qty: i64,
    price_micros: i64,
) -> serde_json::Value {
    let broker_message_id = broker_message_id.to_string();
    let broker_order_id = Some(format!("null-{internal_order_id}"));
    let internal_order_id = internal_order_id.to_string();
    let symbol = "AAPL".to_string();
    let delta_qty = QtyMicros::from_whole_units(whole_qty).expect("whole-unit qty");
    let event = match kind {
        FillKind::Partial => BrokerEvent::PartialFill {
            broker_message_id,
            broker_fill_id: None,
            internal_order_id,
            broker_order_id,
            symbol,
            side: Side::Buy,
            delta_qty,
            price_micros,
            fee_micros: 0,
            cum_qty_after: None,
        },
        FillKind::Final => BrokerEvent::Fill {
            broker_message_id,
            broker_fill_id: None,
            internal_order_id,
            broker_order_id,
            symbol,
            side: Side::Buy,
            delta_qty,
            price_micros,
            fee_micros: 0,
        },
    };
    serde_json::to_value(event).expect("serialize BrokerEvent")
}

// ---------------------------------------------------------------------------
// FQB-01: limit-order fill → telemetry row with reference price and slippage
// ---------------------------------------------------------------------------

/// A real orchestrator tick processes a limit Fill event and writes exactly
/// one `fill_quality_telemetry` row. The reference price, slippage, and
/// fill kind are derived from the outbox row and the inbox Fill event.
async fn fqb01_limit_fill_writes_telemetry_with_slippage_body(pool: PgPool) -> Result<()> {
    let run_id: Uuid = FQB01_RUN_ID.parse().unwrap();

    seed_running_run(&pool, run_id, "fqb01").await?;

    // Enqueue a PENDING limit-order outbox row.
    // Phase 1 will submit this via NullBroker, mark SENT, and register the
    // OmsOrder in memory (symbol=AAPL, qty=1, side=buy, limit_price=10_000_000).
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        "fqb01-ord-001",
        json!({
            "symbol": "AAPL",
            "qty": 1,
            "side": "buy",
            "order_type": "limit",
            "limit_price": 10_000_000_i64
        }),
    )
    .await?;

    // Seed unapplied inbox Fill event.
    // delta_qty == total_qty (1 == 1) → OMS Fill transition → BrokerEvent::Fill.
    let fill_json = broker_fill_json(
        FillKind::Final,
        "fqb01-msg-001",
        "fqb01-ord-001",
        1,
        10_050_000,
    );
    let inserted = mqk_db::inbox_insert_deduped(&pool, run_id, "fqb01-msg-001", fill_json).await?;
    assert!(inserted, "FQB-01: inbox Fill row must be inserted");

    // tick(): Phase 1 submits outbox, registers OmsOrder; Phase 3b applies Fill
    // and writes telemetry row via build_fill_quality_row.
    let mut orch = make_orchestrator(pool.clone(), run_id, 500_000_000);
    orch.tick()
        .await
        .map_err(|e| anyhow::anyhow!("FQB-01: tick() must succeed for a valid Fill, got: {e}"))?;

    // Assert exactly one telemetry row.
    let rows = mqk_db::fetch_fill_quality_telemetry_recent(&pool, run_id, 10).await?;
    assert_eq!(
        rows.len(),
        1,
        "FQB-01: expected 1 telemetry row after limit Fill, got {}",
        rows.len()
    );

    let row = &rows[0];
    assert_eq!(
        row.internal_order_id, "fqb01-ord-001",
        "FQB-01: internal_order_id"
    );
    assert_eq!(row.symbol, "AAPL", "FQB-01: symbol");
    assert_eq!(row.side, "buy", "FQB-01: side");
    assert_eq!(row.fill_qty, 1, "FQB-01: fill_qty");
    assert_eq!(
        row.fill_price_micros, 10_050_000,
        "FQB-01: fill_price_micros"
    );
    assert_eq!(
        row.reference_price_micros,
        Some(10_000_000),
        "FQB-01: reference_price_micros must be limit_price from outbox"
    );
    // slippage = (10_050_000 - 10_000_000) * 10_000 / 10_000_000 = 50 bps
    assert_eq!(
        row.slippage_bps,
        Some(50),
        "FQB-01: slippage_bps must be 50 for 0.5% adverse buy fill"
    );
    assert_eq!(
        row.fill_kind, "final_fill",
        "FQB-01: fill_kind must be final_fill"
    );
    assert_eq!(
        row.provenance_ref, "oms_inbox:fqb01-msg-001",
        "FQB-01: provenance_ref"
    );

    Ok(())
}

#[tokio::test]
async fn fqb01_limit_fill_writes_telemetry_with_slippage() {
    mqk_db::run_isolated(
        "fqb01_limit_fill_writes_telemetry_with_slippage",
        |pool| async move {
            fqb01_limit_fill_writes_telemetry_with_slippage_body(pool)
                .await
                .expect("FQB01");
        },
    )
    .await;
}

// ---------------------------------------------------------------------------
// FQB-02: market-order fill → telemetry row, null reference price and slippage
// ---------------------------------------------------------------------------

/// Market orders carry no limit_price in the outbox row.
/// The reference_price_micros and slippage_bps fields must be null — no
/// fabrication of slippage when the reference is undefined.
async fn fqb02_market_fill_writes_telemetry_null_slippage_body(pool: PgPool) -> Result<()> {
    let run_id: Uuid = FQB02_RUN_ID.parse().unwrap();

    seed_running_run(&pool, run_id, "fqb02").await?;

    // Market order: no limit_price, order_type defaults to "market".
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        "fqb02-ord-001",
        json!({
            "symbol": "AAPL",
            "qty": 1,
            "side": "buy"
        }),
    )
    .await?;

    let fill_json = broker_fill_json(
        FillKind::Final,
        "fqb02-msg-001",
        "fqb02-ord-001",
        1,
        10_050_000,
    );
    let inserted = mqk_db::inbox_insert_deduped(&pool, run_id, "fqb02-msg-001", fill_json).await?;
    assert!(inserted, "FQB-02: inbox Fill row must be inserted");

    let mut orch = make_orchestrator(pool.clone(), run_id, 500_000_000);
    orch.tick()
        .await
        .map_err(|e| anyhow::anyhow!("FQB-02: tick() must succeed for market Fill, got: {e}"))?;

    let rows = mqk_db::fetch_fill_quality_telemetry_recent(&pool, run_id, 10).await?;
    assert_eq!(
        rows.len(),
        1,
        "FQB-02: expected 1 telemetry row after market Fill, got {}",
        rows.len()
    );

    let row = &rows[0];
    assert_eq!(row.fill_qty, 1, "FQB-02: fill_qty");
    assert_eq!(
        row.fill_price_micros, 10_050_000,
        "FQB-02: fill_price_micros"
    );
    assert_eq!(
        row.reference_price_micros, None,
        "FQB-02: reference_price_micros must be null for market order"
    );
    assert_eq!(
        row.slippage_bps, None,
        "FQB-02: slippage_bps must be null when reference is undefined"
    );
    assert_eq!(row.fill_kind, "final_fill", "FQB-02: fill_kind");

    Ok(())
}

#[tokio::test]
async fn fqb02_market_fill_writes_telemetry_null_slippage() {
    mqk_db::run_isolated(
        "fqb02_market_fill_writes_telemetry_null_slippage",
        |pool| async move {
            fqb02_market_fill_writes_telemetry_null_slippage_body(pool)
                .await
                .expect("FQB02");
        },
    )
    .await;
}

// ---------------------------------------------------------------------------
// FQB-03: CancelAck (non-fill path) → zero telemetry rows
// ---------------------------------------------------------------------------

/// A CancelAck inbox event must not produce any `fill_quality_telemetry` rows.
/// `build_fill_quality_row` returns `None` for non-fill events — this test
/// proves the no-fabrication contract via the real orchestrator path.
///
/// CancelAck for an order that is not in the in-memory OmsOrders map is
/// silently skipped (non-fill events for unknown orders are no-ops).
async fn fqb03_cancel_ack_writes_no_telemetry_body(pool: PgPool) -> Result<()> {
    let run_id: Uuid = FQB03_RUN_ID.parse().unwrap();

    seed_running_run(&pool, run_id, "fqb03").await?;

    // No outbox row — nothing for Phase 1 to submit.
    // Seed a CancelAck for an order not in oms_orders.
    // Non-fill events for unknown orders are silently skipped by apply_fill_step.
    let cancel_json = json!({
        "type":              "cancel_ack",
        "broker_message_id": "fqb03-msg-001",
        "internal_order_id": "fqb03-ord-unknown",
        "broker_order_id":   null
    });
    let inserted =
        mqk_db::inbox_insert_deduped(&pool, run_id, "fqb03-msg-001", cancel_json).await?;
    assert!(inserted, "FQB-03: inbox CancelAck row must be inserted");

    let mut orch = make_orchestrator(pool.clone(), run_id, 500_000_000);
    orch.tick().await.map_err(|e| {
        anyhow::anyhow!("FQB-03: tick() must succeed for CancelAck of unknown order, got: {e}")
    })?;

    let rows = mqk_db::fetch_fill_quality_telemetry_recent(&pool, run_id, 10).await?;
    assert_eq!(
        rows.len(),
        0,
        "FQB-03: zero telemetry rows expected after CancelAck non-fill, got {}",
        rows.len()
    );

    Ok(())
}

#[tokio::test]
async fn fqb03_cancel_ack_writes_no_telemetry() {
    mqk_db::run_isolated("fqb03_cancel_ack_writes_no_telemetry", |pool| async move {
        fqb03_cancel_ack_writes_no_telemetry_body(pool)
            .await
            .expect("FQB03");
    })
    .await;
}

// ---------------------------------------------------------------------------
// FQB-04: PartialFill then Fill → exactly two telemetry rows
// ---------------------------------------------------------------------------

/// Two fill events in one tick produce two distinct telemetry rows:
/// one for the PartialFill (fill_kind = "partial_fill") and one for the
/// final Fill (fill_kind = "final_fill").
///
/// OMS invariant enforced: PartialFill(delta_qty=6) + Fill(delta_qty=4)
/// sums to 10 == total_qty(10). The orchestrator must process both events
/// in canonical inbox_id ASC order in a single tick().
async fn fqb04_partial_fill_then_fill_writes_two_rows_body(pool: PgPool) -> Result<()> {
    let run_id: Uuid = FQB04_RUN_ID.parse().unwrap();

    seed_running_run(&pool, run_id, "fqb04").await?;

    // Limit order for 10 shares — Phase 1 will register OmsOrder(total_qty=10).
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        "fqb04-ord-001",
        json!({
            "symbol": "AAPL",
            "qty": 10,
            "side": "buy",
            "order_type": "limit",
            "limit_price": 10_000_000_i64
        }),
    )
    .await?;

    // PartialFill: 6 of 10 shares → OmsOrder(PartiallyFilled, filled_qty=6).
    let pfill_json = broker_fill_json(
        FillKind::Partial,
        "fqb04-msg-pfill",
        "fqb04-ord-001",
        6,
        9_990_000,
    );
    let ins1 = mqk_db::inbox_insert_deduped(&pool, run_id, "fqb04-msg-pfill", pfill_json).await?;
    assert!(ins1, "FQB-04: PartialFill inbox row must be inserted");

    // Final Fill: remaining 4 of 10 → filled_qty(6) + delta_qty(4) = 10 == total_qty.
    let fill_json = broker_fill_json(
        FillKind::Final,
        "fqb04-msg-fill",
        "fqb04-ord-001",
        4,
        10_050_000,
    );
    let ins2 = mqk_db::inbox_insert_deduped(&pool, run_id, "fqb04-msg-fill", fill_json).await?;
    assert!(ins2, "FQB-04: Fill inbox row must be inserted");

    // Initial cash must cover 10 shares: 6 * 9_990_000 + 4 * 10_050_000 = 99_960_000.
    // 500_000_000 is well clear.
    let mut orch = make_orchestrator(pool.clone(), run_id, 500_000_000);
    orch.tick().await.map_err(|e| {
        anyhow::anyhow!("FQB-04: tick() must succeed for PartialFill+Fill sequence, got: {e}")
    })?;

    let rows = mqk_db::fetch_fill_quality_telemetry_recent(&pool, run_id, 10).await?;
    assert_eq!(
        rows.len(),
        2,
        "FQB-04: expected 2 telemetry rows (PartialFill + Fill), got {}",
        rows.len()
    );

    // fetch_fill_quality_telemetry_recent returns most-recent-first
    // (fill_received_at_utc DESC). Both events were inserted at the same clock
    // tick (FixedClock), so we identify by broker_message_id.
    let pfill_row = rows
        .iter()
        .find(|r| r.broker_message_id == "fqb04-msg-pfill")
        .expect("FQB-04: must have PartialFill telemetry row");
    let fill_row = rows
        .iter()
        .find(|r| r.broker_message_id == "fqb04-msg-fill")
        .expect("FQB-04: must have final Fill telemetry row");

    assert_eq!(
        pfill_row.fill_kind, "partial_fill",
        "FQB-04: PartialFill must be partial_fill"
    );
    assert_eq!(
        pfill_row.fill_qty, 6,
        "FQB-04: PartialFill fill_qty must be 6"
    );
    assert_eq!(
        pfill_row.fill_price_micros, 9_990_000,
        "FQB-04: PartialFill price"
    );
    // slippage for PartialFill: (9_990_000 - 10_000_000) * 10_000 / 10_000_000
    //   = -10_000 * 10_000 / 10_000_000 = -10 bps (favourable)
    assert_eq!(
        pfill_row.slippage_bps,
        Some(-10),
        "FQB-04: PartialFill slippage must be -10 bps (favourable fill)"
    );

    assert_eq!(
        fill_row.fill_kind, "final_fill",
        "FQB-04: Fill must be final_fill"
    );
    assert_eq!(fill_row.fill_qty, 4, "FQB-04: Fill fill_qty must be 4");
    assert_eq!(fill_row.fill_price_micros, 10_050_000, "FQB-04: Fill price");
    // slippage for Fill: (10_050_000 - 10_000_000) * 10_000 / 10_000_000 = 50 bps
    assert_eq!(
        fill_row.slippage_bps,
        Some(50),
        "FQB-04: Fill slippage must be 50 bps"
    );

    // Both rows reference the same order and share the outbox ordered_qty.
    assert_eq!(
        pfill_row.ordered_qty, 10,
        "FQB-04: PartialFill ordered_qty from outbox"
    );
    assert_eq!(
        fill_row.ordered_qty, 10,
        "FQB-04: Fill ordered_qty from outbox"
    );
    assert_eq!(
        pfill_row.reference_price_micros,
        Some(10_000_000),
        "FQB-04: PartialFill reference_price from outbox limit_price"
    );
    assert_eq!(
        fill_row.reference_price_micros,
        Some(10_000_000),
        "FQB-04: Fill reference_price from outbox limit_price"
    );

    Ok(())
}

#[tokio::test]
async fn fqb04_partial_fill_then_fill_writes_two_rows() {
    mqk_db::run_isolated(
        "fqb04_partial_fill_then_fill_writes_two_rows",
        |pool| async move {
            fqb04_partial_fill_then_fill_writes_two_rows_body(pool)
                .await
                .expect("FQB04");
        },
    )
    .await;
}
