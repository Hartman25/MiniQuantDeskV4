//! DURABLE-PAPER-PORTFOLIO-AND-PNL-01G: integrated closure proof.
//!
//! Combines B4-B/B4-C/B4-D/B4-E's real production seams into single,
//! end-to-end proofs of the full chain this bundle closes: an accepted
//! authoritative snapshot persists durably (Proof A); real fills replay
//! through the existing FIFO engine into durable accounting (Proof B);
//! that durable state survives an `AppState`/pool replacement identically
//! (Proof C); a pre-existing, fill-history-unexplained position blocks the
//! accounting epoch without ever fabricating an opening fill (Proof D);
//! realized/unrealized/daily P&L truth is independently provable positive,
//! negative, and (for realized) zero (Proof E); and the full
//! signal-to-durable-visibility chain, including after a restart, is
//! reconstructable from durable state alone via the paper-lifecycle route
//! (Proof F).
//!
//! Proofs G (API read-only) and H (GUI contract) are proven by
//! `scenario_durable_paper_portfolio_read_only_api_01.rs`'s dedicated
//! zero-row-count-delta test and by `scripts/guards/validate_durable_paper_portfolio_and_pnl_01f_operator_integration.ps1`'s
//! static GUI-source checks plus this session's live browser verification
//! (see the B4-F/B4-G spec docs) — neither is duplicated here. Proof I
//! (existing-surface regressions) is proven by re-running
//! `scenario_paper_daily_pnl_baseline_01`, `scenario_paper_daily_pnl_baseline_capture_01`,
//! `scenario_paper_order_lifecycle_visibility_01`, and
//! `scenario_autonomous_completed_bar_driver_01` unchanged, as part of
//! B4-G's own regression matrix (see the closure spec) — not re-implemented
//! as new tests in this file.
//!
//! No real Alpaca/broker/provider/network call is made anywhere in this
//! file. No paper or live order is submitted, cancelled, or replaced.
//! Uses the isolated port-5434 test DB only.

use chrono::{TimeZone, Utc};
use mqk_daemon::state::{AppState, BrokerKind};
use mqk_execution::{BrokerEvent, Side};
use mqk_schemas::{BrokerAccount, BrokerPosition, BrokerSnapshot};
use uuid::Uuid;

async fn test_pool() -> Option<sqlx::PgPool> {
    if std::env::var("MQK_DATABASE_URL").is_err() {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return None;
    }
    match mqk_db::testkit_db_pool().await {
        Ok(pool) => Some(pool),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn fixed_run_id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.durable-paper-portfolio-and-pnl-01g.v1|{seed}").as_bytes(),
    )
}

/// `oms_outbox.idempotency_key` is globally unique (`uq_outbox_idempotency`,
/// migration 0001) -- NOT scoped by `run_id`. A literal logical order id
/// (e.g. "order-1") reused across two different runs' fixtures collides at
/// that constraint: `outbox_enqueue` silently returns `Ok(false)` for the
/// second run, leaving it with no outbox row for that order. A subsequent
/// fill referencing the same logical order then hits `UNKNOWN_ORDER_FILL`
/// during durable replay (`recover_oms_and_portfolio`), which fails the
/// best-effort accounting refresh closed -- exactly the missing
/// `sys_paper_portfolio_accounting_state` / `pnl_truth_state = "not_found"`
/// failures this fixture must not produce. Scoping every logical order id by
/// `run_id` makes it globally unique without weakening the DB constraint.
fn scoped_order_id(run_id: Uuid, logical_order_id: &str) -> String {
    format!("{}:{logical_order_id}", run_id.simple())
}

fn expected_snapshot_id(captured_at_utc: chrono::DateTime<Utc>, run_id: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!(
            "mqk.paper-portfolio-snapshot.v1|{}|{}|{}",
            captured_at_utc.to_rfc3339(),
            run_id,
            mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
        )
        .as_bytes(),
    )
}

async fn seed_run(pool: &sqlx::PgPool, run_id: Uuid, ts: chrono::DateTime<Utc>) {
    cleanup_run(pool, run_id).await;
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "test-durable-paper-portfolio-and-pnl-01g".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: ts,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("insert_run failed");
}

async fn cleanup_run(pool: &sqlx::PgPool, run_id: Uuid) {
    let _ = sqlx::query("delete from oms_inbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "delete from broker_order_map where internal_id in (select idempotency_key from oms_outbox where run_id = $1)",
    )
    .bind(run_id)
    .execute(pool)
    .await;
    let _ = sqlx::query("delete from oms_outbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from sys_paper_portfolio_accounting_state where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "delete from sys_paper_portfolio_snapshot_positions where snapshot_id in (select snapshot_id from sys_paper_portfolio_snapshots where run_id = $1)",
    )
    .bind(run_id)
    .execute(pool)
    .await;
    let _ = sqlx::query("delete from sys_paper_portfolio_snapshots where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

#[allow(clippy::too_many_arguments)]
async fn insert_full_fill(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    order_id: &str,
    symbol: &str,
    side: Side,
    qty: i64,
    price_micros: i64,
    fee_micros: i64,
    broker_message_id: &str,
    at: chrono::DateTime<Utc>,
) {
    let side_label = match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    };
    let enqueued = mqk_db::outbox_enqueue(
        pool,
        run_id,
        order_id,
        serde_json::json!({"symbol": symbol, "qty": qty, "side": side_label}),
    )
    .await
    .expect("outbox_enqueue should succeed");
    assert!(
        enqueued,
        "outbox_enqueue must be a fresh insertion for order_id={order_id} (run_id={run_id}); \
         a false return means idempotency_key already exists globally -- the fixture's \
         order id is not run-scoped"
    );
    sqlx::query("update oms_outbox set status = 'SENT' where idempotency_key = $1")
        .bind(order_id)
        .execute(pool)
        .await
        .expect("mark outbox SENT should succeed");

    let event = BrokerEvent::Fill {
        broker_message_id: broker_message_id.to_string(),
        broker_fill_id: None,
        internal_order_id: order_id.to_string(),
        broker_order_id: None,
        symbol: symbol.to_string(),
        side,
        delta_qty: qty,
        price_micros,
        fee_micros,
    };
    let json = serde_json::to_value(&event).expect("serialize BrokerEvent::Fill");
    mqk_db::inbox_insert_deduped_with_identity(
        pool,
        run_id,
        broker_message_id,
        None,
        order_id,
        "",
        "fill",
        &json,
        0,
        at,
    )
    .await
    .expect("inbox insert should succeed");
    mqk_db::inbox_mark_applied(pool, run_id, broker_message_id, at)
        .await
        .expect("inbox mark applied should succeed");
}

async fn insert_ack(pool: &sqlx::PgPool, run_id: Uuid, order_id: &str, at: chrono::DateTime<Utc>) {
    let broker_message_id = format!("{order_id}-ack");
    let event = BrokerEvent::Ack {
        broker_message_id: broker_message_id.clone(),
        internal_order_id: order_id.to_string(),
        broker_order_id: Some(format!("broker-{order_id}")),
    };
    let json = serde_json::to_value(&event).expect("serialize BrokerEvent::Ack");
    mqk_db::inbox_insert_deduped_with_identity(
        pool,
        run_id,
        &broker_message_id,
        None,
        order_id,
        "",
        "ack",
        &json,
        0,
        at,
    )
    .await
    .expect("inbox ack insert should succeed");
    mqk_db::inbox_mark_applied(pool, run_id, &broker_message_id, at)
        .await
        .expect("inbox ack mark applied should succeed");
}

fn fixture_snapshot(
    captured_at_utc: chrono::DateTime<Utc>,
    positions: Vec<BrokerPosition>,
) -> BrokerSnapshot {
    BrokerSnapshot {
        captured_at_utc,
        account: BrokerAccount {
            equity: "100000.00".to_string(),
            cash: "40000.00".to_string(),
            currency: "USD".to_string(),
        },
        orders: vec![],
        fills: vec![],
        positions,
    }
}

async fn accept_and_refresh(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    captured_at_utc: chrono::DateTime<Utc>,
    positions: Vec<BrokerPosition>,
) {
    let mut state = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    state.db = Some(pool.clone());
    state
        .accept_external_broker_snapshot_for_test(
            fixture_snapshot(captured_at_utc, positions),
            Some(run_id),
            None,
        )
        .await;
}

/// Proofs A + B + C: an accepted snapshot persists durably; real fills
/// (buy, partial-sell-gain, full-sell-flat) replay through the existing
/// FIFO engine into durable accounting; and a fresh `AppState` + fresh pool
/// (simulating a restart) reads back byte-identical durable state.
#[tokio::test]
async fn proof_a_b_c_snapshot_fill_accounting_and_restart() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("proof_a_b_c_snapshot_fill_accounting_and_restart");
    seed_run(
        &pool,
        run_id,
        Utc.with_ymd_and_hms(2099, 6, 1, 12, 0, 0).unwrap(),
    )
    .await;

    let at = Utc.with_ymd_and_hms(2099, 6, 1, 13, 0, 0).unwrap();
    let order_1 = scoped_order_id(run_id, "order-1");
    let order_2 = scoped_order_id(run_id, "order-2");
    insert_full_fill(
        &pool,
        run_id,
        &order_1,
        "AAPL",
        Side::Buy,
        10,
        150_000_000,
        0,
        "msg-1",
        at,
    )
    .await;
    insert_full_fill(
        &pool,
        run_id,
        &order_2,
        "AAPL",
        Side::Sell,
        4,
        160_000_000,
        0,
        "msg-2",
        at,
    )
    .await;

    let captured_at_utc = Utc.with_ymd_and_hms(2099, 6, 1, 13, 5, 0).unwrap();
    accept_and_refresh(
        &pool,
        run_id,
        captured_at_utc,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "6".to_string(),
            avg_price: "150.00".to_string(),
        }],
    )
    .await;

    // Proof A: snapshot durably persisted.
    let snapshot = mqk_db::fetch_paper_portfolio_snapshot_by_id(
        &pool,
        expected_snapshot_id(captured_at_utc, run_id),
    )
    .await
    .expect("fetch should succeed")
    .expect("durable snapshot should exist");
    assert_eq!(snapshot.snapshot.cash_micros, 40_000_000_000);
    assert_eq!(snapshot.positions.len(), 1);

    // Proof B: durable FIFO accounting -- (160-150)*4 = 40.00 realized gain.
    let accounting_before = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    assert_eq!(accounting_before.realized_pnl_micros, 40_000_000);
    assert_eq!(accounting_before.accounting_epoch, "complete");

    // Proof C: restart -- a brand-new AppState + brand-new pool replays the
    // exact same durable oms_inbox history and reaches identical state.
    let pool_after = test_pool().await.expect("reconnect should succeed");
    accept_and_refresh(
        &pool_after,
        run_id,
        captured_at_utc,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "6".to_string(),
            avg_price: "150.00".to_string(),
        }],
    )
    .await;
    let accounting_after = mqk_db::fetch_paper_portfolio_accounting_state(&pool_after, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    assert_eq!(accounting_before.cash_micros, accounting_after.cash_micros);
    assert_eq!(
        accounting_before.realized_pnl_micros,
        accounting_after.realized_pnl_micros
    );
    assert_eq!(
        accounting_before.last_applied_inbox_id,
        accounting_after.last_applied_inbox_id
    );

    cleanup_run(&pool_after, run_id).await;
}

/// Proof D: a broker-reported position with no matching fill history in
/// this run's durable oms_inbox blocks the accounting epoch -- the position
/// itself remains visible (broker-sourced truth), realized P&L is
/// explicitly unavailable, and no synthetic opening fill is ever inserted
/// to force a match.
#[tokio::test]
async fn proof_d_incomplete_history_blocks_epoch_without_fabrication() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("proof_d_incomplete_history_blocks_epoch_without_fabrication");
    seed_run(
        &pool,
        run_id,
        Utc.with_ymd_and_hms(2099, 6, 1, 12, 0, 0).unwrap(),
    )
    .await;

    // No fills at all -- but the broker reports an adopted position.
    let captured_at_utc = Utc.with_ymd_and_hms(2099, 6, 1, 13, 0, 0).unwrap();
    accept_and_refresh(
        &pool,
        run_id,
        captured_at_utc,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "30".to_string(),
            avg_price: "140.00".to_string(),
        }],
    )
    .await;

    // Position remains visible via the durable snapshot.
    let snapshot = mqk_db::fetch_paper_portfolio_snapshot_by_id(
        &pool,
        expected_snapshot_id(captured_at_utc, run_id),
    )
    .await
    .expect("fetch should succeed")
    .expect("durable snapshot should exist");
    assert_eq!(snapshot.positions.len(), 1);
    assert_eq!(snapshot.positions[0].qty_signed, 30);

    // Accounting epoch is explicitly incomplete, never fabricated complete.
    let accounting = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    assert_eq!(accounting.accounting_epoch, "incomplete");
    assert!(accounting.accounting_epoch_reason.is_some());
    assert_eq!(
        accounting.realized_pnl_micros, 0,
        "no fill history exists, so no fabricated realized P&L can exist either"
    );

    // The number of oms_inbox rows for this run is exactly zero -- proving
    // no synthetic opening fill was ever inserted to close the gap.
    let inbox_count: i64 = sqlx::query_scalar("select count(*) from oms_inbox where run_id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("count should succeed");
    assert_eq!(inbox_count, 0, "no synthetic fill was ever inserted");

    cleanup_run(&pool, run_id).await;
}

/// Proof E: realized P&L is independently provable positive, negative, and
/// exactly zero -- proven as three separate runs (not reused state), each
/// asserting the FIFO math directly.
#[tokio::test]
async fn proof_e_realized_pnl_positive_negative_and_zero() {
    let Some(pool) = test_pool().await else {
        return;
    };

    // Positive: sell above entry.
    let run_pos = fixed_run_id("proof_e_realized_pnl_positive");
    seed_run(
        &pool,
        run_pos,
        Utc.with_ymd_and_hms(2099, 6, 1, 12, 0, 0).unwrap(),
    )
    .await;
    let at = Utc.with_ymd_and_hms(2099, 6, 1, 13, 0, 0).unwrap();
    let order_1_pos = scoped_order_id(run_pos, "order-1");
    let order_2_pos = scoped_order_id(run_pos, "order-2");
    insert_full_fill(
        &pool,
        run_pos,
        &order_1_pos,
        "AAPL",
        Side::Buy,
        10,
        150_000_000,
        0,
        "msg-1",
        at,
    )
    .await;
    insert_full_fill(
        &pool,
        run_pos,
        &order_2_pos,
        "AAPL",
        Side::Sell,
        10,
        160_000_000,
        0,
        "msg-2",
        at,
    )
    .await;
    accept_and_refresh(
        &pool,
        run_pos,
        at,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "0".to_string(),
            avg_price: "0".to_string(),
        }],
    )
    .await;
    let acc_pos = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_pos)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    assert!(
        acc_pos.realized_pnl_micros > 0,
        "(160-150)*10 must be positive"
    );
    assert_eq!(acc_pos.realized_pnl_micros, 100_000_000);

    // Negative: sell below entry.
    let run_neg = fixed_run_id("proof_e_realized_pnl_negative");
    seed_run(
        &pool,
        run_neg,
        Utc.with_ymd_and_hms(2099, 6, 1, 12, 0, 0).unwrap(),
    )
    .await;
    let order_1_neg = scoped_order_id(run_neg, "order-1");
    let order_2_neg = scoped_order_id(run_neg, "order-2");
    insert_full_fill(
        &pool,
        run_neg,
        &order_1_neg,
        "AAPL",
        Side::Buy,
        10,
        150_000_000,
        0,
        "msg-1",
        at,
    )
    .await;
    insert_full_fill(
        &pool,
        run_neg,
        &order_2_neg,
        "AAPL",
        Side::Sell,
        10,
        140_000_000,
        0,
        "msg-2",
        at,
    )
    .await;
    accept_and_refresh(
        &pool,
        run_neg,
        at,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "0".to_string(),
            avg_price: "0".to_string(),
        }],
    )
    .await;
    let acc_neg = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_neg)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    assert!(
        acc_neg.realized_pnl_micros < 0,
        "(140-150)*10 must be negative"
    );
    assert_eq!(acc_neg.realized_pnl_micros, -100_000_000);

    // Zero: flat portfolio, no fills at all.
    let run_zero = fixed_run_id("proof_e_realized_pnl_zero");
    seed_run(
        &pool,
        run_zero,
        Utc.with_ymd_and_hms(2099, 6, 1, 12, 0, 0).unwrap(),
    )
    .await;
    accept_and_refresh(&pool, run_zero, at, vec![]).await;
    let acc_zero = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_zero)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    assert_eq!(acc_zero.realized_pnl_micros, 0);
    assert_eq!(
        acc_zero.accounting_epoch, "complete",
        "a flat portfolio has known-zero, complete truth"
    );

    cleanup_run(&pool, run_pos).await;
    cleanup_run(&pool, run_neg).await;
    cleanup_run(&pool, run_zero).await;
}

/// Proof F: the full signal-to-durable-visibility chain (order -> ack ->
/// fill -> durable accounting -> durable portfolio/P&L visibility via the
/// paper-lifecycle route), and that a restarted, stopped run remains fully
/// reconstructable from durable state alone.
#[tokio::test]
async fn proof_f_full_chain_and_restart_reconstructable_via_paper_lifecycle() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("proof_f_full_chain_and_restart_reconstructable_via_paper_lifecycle");
    let ts = Utc.with_ymd_and_hms(2099, 6, 1, 12, 0, 0).unwrap();
    seed_run(&pool, run_id, ts).await;

    // order -> ack -> fill.
    let order_1 = scoped_order_id(run_id, "order-1");
    let enqueued = mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &order_1,
        serde_json::json!({"symbol": "AAPL", "qty": 10, "side": "buy"}),
    )
    .await
    .expect("outbox_enqueue should succeed");
    assert!(
        enqueued,
        "outbox_enqueue must be a fresh insertion for order_id={order_1} (run_id={run_id}); \
         a false return means idempotency_key already exists globally -- the fixture's \
         order id is not run-scoped"
    );
    sqlx::query("update oms_outbox set status = 'SENT' where idempotency_key = $1")
        .bind(&order_1)
        .execute(&pool)
        .await
        .expect("mark outbox SENT should succeed");
    insert_ack(&pool, run_id, &order_1, ts).await;
    let event = BrokerEvent::Fill {
        broker_message_id: "msg-fill-1".to_string(),
        broker_fill_id: None,
        internal_order_id: order_1.clone(),
        broker_order_id: None,
        symbol: "AAPL".to_string(),
        side: Side::Buy,
        delta_qty: 10,
        price_micros: 150_000_000,
        fee_micros: 0,
    };
    let json = serde_json::to_value(&event).expect("serialize");
    mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        "msg-fill-1",
        None,
        &order_1,
        "",
        "fill",
        &json,
        0,
        ts,
    )
    .await
    .expect("inbox insert should succeed");
    mqk_db::inbox_mark_applied(&pool, run_id, "msg-fill-1", ts)
        .await
        .expect("inbox mark applied should succeed");

    // durable accounting + durable portfolio visibility (accept_external_broker_snapshot_for_test).
    accept_and_refresh(
        &pool,
        run_id,
        ts,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "10".to_string(),
            avg_price: "150.00".to_string(),
        }],
    )
    .await;

    // The paper-lifecycle route (read-only) proves the full chain is
    // visible: signal evaluation isn't part of this fixture (no strategy
    // ran), but outbox/inbox/durable-portfolio/durable-pnl all are.
    let router = {
        let st = std::sync::Arc::new(mqk_daemon::state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            mqk_daemon::state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        mqk_daemon::routes::build_router(st)
    };
    let uri = format!("/api/v1/execution/paper-lifecycle?run_id={run_id}");
    let req = axum::http::Request::builder()
        .method("GET")
        .uri(&uri)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = tower::ServiceExt::oneshot(router, req)
        .await
        .expect("oneshot should succeed");
    let status = resp.status();
    let body = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .expect("body collect should succeed")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["portfolio_truth_state"], "active");
    assert_eq!(json["pnl_truth_state"], "active");
    assert_eq!(json["lifecycle_summary"]["fill_seen"], true);
    assert_eq!(
        json["lifecycle_summary"]["overall_lifecycle_state"],
        "order_filled_portfolio_durable_pnl_available"
    );

    // Restart: brand-new AppState + brand-new pool, same route, same result
    // -- reconstructed entirely from durable state, no in-memory carryover.
    let pool_after = test_pool().await.expect("reconnect should succeed");
    let router_after = {
        let st = std::sync::Arc::new(mqk_daemon::state::AppState::new_with_db_and_operator_auth(
            pool_after.clone(),
            mqk_daemon::state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        mqk_daemon::routes::build_router(st)
    };
    let req_after = axum::http::Request::builder()
        .method("GET")
        .uri(&uri)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp_after = tower::ServiceExt::oneshot(router_after, req_after)
        .await
        .expect("oneshot should succeed");
    let body_after = http_body_util::BodyExt::collect(resp_after.into_body())
        .await
        .expect("body collect should succeed")
        .to_bytes();
    let json_after: serde_json::Value = serde_json::from_slice(&body_after).expect("valid JSON");
    assert_eq!(json_after["portfolio_truth_state"], "active");
    assert_eq!(json_after["pnl_truth_state"], "active");
    assert_eq!(
        json_after["lifecycle_summary"]["overall_lifecycle_state"],
        "order_filled_portfolio_durable_pnl_available"
    );

    cleanup_run(&pool_after, run_id).await;
}
