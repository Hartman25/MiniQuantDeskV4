//! DURABLE-PAPER-PORTFOLIO-AND-PNL-01D: durable fill accounting and P&L
//! truth proof.
//!
//! Exercises the real production seam (`AppState::accept_external_broker_snapshot_for_test`,
//! which now also refreshes the durable accounting projection -- see B4-D)
//! against real `oms_outbox`/`oms_inbox` fixture rows, so the FIFO replay
//! this patch adds is proven against the same durable data shape the daemon
//! actually writes, not a hand-rolled shortcut. No real Alpaca/broker/
//! provider/network call is made anywhere in this file.
//!
//! All DB-backed tests skip gracefully without `MQK_DATABASE_URL` pointing
//! at the isolated test DB.

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
        format!("test.durable-paper-portfolio-accounting.v1|{seed}").as_bytes(),
    )
}

async fn fixture_run(pool: &sqlx::PgPool, run_id: Uuid) {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "test-durable-portfolio-accounting".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc.with_ymd_and_hms(2099, 4, 1, 12, 0, 0).unwrap(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("fixture run insert should succeed");
}

async fn cleanup(pool: &sqlx::PgPool, run_id: Uuid) {
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
    let _ = sqlx::query("delete from oms_inbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from oms_outbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

/// Enqueues a `SENT` outbox order and a fully-applied `Fill` inbox event
/// referencing it -- the same shape the daemon's own recover/apply path
/// produces, so the FIFO replay under test sees exactly the durable rows a
/// real fill would leave behind, including the OMS per-order dedup guard's
/// prerequisite (a matching outbox row for `order_id`).
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
    mqk_db::outbox_enqueue(
        pool,
        run_id,
        order_id,
        serde_json::json!({"symbol": symbol, "qty": qty, "side": side_label}),
    )
    .await
    .expect("outbox_enqueue should succeed");
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

/// Enqueues one `SENT` outbox order and applies it via a `PartialFill`
/// followed by a completing `Fill` -- two distinct economic events on the
/// same order.
#[allow(clippy::too_many_arguments)]
async fn insert_partial_then_final_fill(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    order_id: &str,
    symbol: &str,
    side: Side,
    partial_qty: i64,
    final_qty: i64,
    price_micros: i64,
    at: chrono::DateTime<Utc>,
) {
    let side_label = match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    };
    mqk_db::outbox_enqueue(
        pool,
        run_id,
        order_id,
        serde_json::json!({"symbol": symbol, "qty": partial_qty + final_qty, "side": side_label}),
    )
    .await
    .expect("outbox_enqueue should succeed");
    sqlx::query("update oms_outbox set status = 'SENT' where idempotency_key = $1")
        .bind(order_id)
        .execute(pool)
        .await
        .expect("mark outbox SENT should succeed");

    let partial = BrokerEvent::PartialFill {
        broker_message_id: format!("{order_id}-partial"),
        broker_fill_id: None,
        internal_order_id: order_id.to_string(),
        broker_order_id: None,
        symbol: symbol.to_string(),
        side,
        delta_qty: partial_qty,
        price_micros,
        fee_micros: 0,
    };
    let json = serde_json::to_value(&partial).expect("serialize PartialFill");
    mqk_db::inbox_insert_deduped_with_identity(
        pool,
        run_id,
        &format!("{order_id}-partial"),
        None,
        order_id,
        "",
        "partial_fill",
        &json,
        0,
        at,
    )
    .await
    .expect("inbox insert (partial) should succeed");
    mqk_db::inbox_mark_applied(pool, run_id, &format!("{order_id}-partial"), at)
        .await
        .expect("inbox mark applied (partial) should succeed");

    let complete = BrokerEvent::Fill {
        broker_message_id: format!("{order_id}-final"),
        broker_fill_id: None,
        internal_order_id: order_id.to_string(),
        broker_order_id: None,
        symbol: symbol.to_string(),
        side,
        delta_qty: final_qty,
        price_micros,
        fee_micros: 0,
    };
    let json = serde_json::to_value(&complete).expect("serialize Fill");
    mqk_db::inbox_insert_deduped_with_identity(
        pool,
        run_id,
        &format!("{order_id}-final"),
        None,
        order_id,
        "",
        "fill",
        &json,
        0,
        at,
    )
    .await
    .expect("inbox insert (final) should succeed");
    mqk_db::inbox_mark_applied(pool, run_id, &format!("{order_id}-final"), at)
        .await
        .expect("inbox mark applied (final) should succeed");
}

fn fixture_snapshot(positions: Vec<BrokerPosition>) -> BrokerSnapshot {
    BrokerSnapshot {
        captured_at_utc: Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap(),
        account: BrokerAccount {
            equity: "100000.00".to_string(),
            cash: "100000.00".to_string(),
            currency: "USD".to_string(),
        },
        orders: vec![],
        fills: vec![],
        positions,
    }
}

async fn accept_and_refresh(pool: &sqlx::PgPool, run_id: Uuid, positions: Vec<BrokerPosition>) {
    let mut state = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    state.db = Some(pool.clone());
    state
        .accept_external_broker_snapshot_for_test(fixture_snapshot(positions), Some(run_id), None)
        .await;
}

/// A single buy opens a long position with zero realized P&L; cash
/// movement reflects the full cost.
#[tokio::test]
async fn buy_opens_long_position() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("buy_opens_long_position");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
    insert_full_fill(
        &pool, run_id, "order-1", "AAPL", Side::Buy, 10, 150_000_000, 0, "msg-1", at,
    )
    .await;

    accept_and_refresh(
        &pool,
        run_id,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "10".to_string(),
            avg_price: "150.00".to_string(),
        }],
    )
    .await;

    let state = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    assert_eq!(state.realized_pnl_micros, 0);
    assert_eq!(state.cash_micros, -1_500_000_000);
    assert_eq!(state.accounting_epoch, "complete");

    cleanup(&pool, run_id).await;
}

/// A second buy on the same symbol adds a FIFO lot without realizing P&L.
#[tokio::test]
async fn second_buy_adds_fifo_lot() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("second_buy_adds_fifo_lot");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
    insert_full_fill(
        &pool, run_id, "order-1", "AAPL", Side::Buy, 10, 150_000_000, 0, "msg-1", at,
    )
    .await;
    insert_full_fill(
        &pool, run_id, "order-2", "AAPL", Side::Buy, 5, 155_000_000, 0, "msg-2", at,
    )
    .await;

    accept_and_refresh(
        &pool,
        run_id,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "15".to_string(),
            avg_price: "151.67".to_string(),
        }],
    )
    .await;

    let state = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    assert_eq!(state.realized_pnl_micros, 0, "two buys never realize P&L");
    assert_eq!(state.cash_micros, -(1_500_000_000 + 775_000_000));
    assert_eq!(state.accounting_epoch, "complete");

    cleanup(&pool, run_id).await;
}

/// A partial sell above the FIFO entry price realizes a gain.
#[tokio::test]
async fn partial_sell_realizes_fifo_gain() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("partial_sell_realizes_fifo_gain");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
    insert_full_fill(
        &pool, run_id, "order-1", "AAPL", Side::Buy, 10, 150_000_000, 0, "msg-1", at,
    )
    .await;
    insert_full_fill(
        &pool, run_id, "order-2", "AAPL", Side::Sell, 4, 160_000_000, 0, "msg-2", at,
    )
    .await;

    accept_and_refresh(
        &pool,
        run_id,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "6".to_string(),
            avg_price: "150.00".to_string(),
        }],
    )
    .await;

    let state = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    // (160 - 150) * 4 = 40 per-share-diff * 4 shares, in micros.
    assert_eq!(state.realized_pnl_micros, 40_000_000);
    assert_eq!(state.accounting_epoch, "complete");

    cleanup(&pool, run_id).await;
}

/// A partial sell below the FIFO entry price realizes a loss.
#[tokio::test]
async fn partial_sell_realizes_fifo_loss() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("partial_sell_realizes_fifo_loss");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
    insert_full_fill(
        &pool, run_id, "order-1", "AAPL", Side::Buy, 10, 150_000_000, 0, "msg-1", at,
    )
    .await;
    insert_full_fill(
        &pool, run_id, "order-2", "AAPL", Side::Sell, 4, 140_000_000, 0, "msg-2", at,
    )
    .await;

    accept_and_refresh(
        &pool,
        run_id,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "6".to_string(),
            avg_price: "150.00".to_string(),
        }],
    )
    .await;

    let state = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    assert_eq!(state.realized_pnl_micros, -40_000_000);
    assert_eq!(state.accounting_epoch, "complete");

    cleanup(&pool, run_id).await;
}

/// Fees reduce cash on both buy and sell sides.
#[tokio::test]
async fn fees_reduce_cash() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("fees_reduce_cash");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
    insert_full_fill(
        &pool, run_id, "order-1", "AAPL", Side::Buy, 10, 150_000_000, 1_000_000, "msg-1", at,
    )
    .await;

    accept_and_refresh(
        &pool,
        run_id,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "10".to_string(),
            avg_price: "150.00".to_string(),
        }],
    )
    .await;

    let state = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    assert_eq!(state.fees_micros, 1_000_000);
    assert_eq!(state.cash_micros, -(1_500_000_000 + 1_000_000));

    cleanup(&pool, run_id).await;
}

/// A full sell closes the position; the flat broker position (qty "0")
/// keeps the accounting epoch complete.
#[tokio::test]
async fn full_sell_closes_position() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("full_sell_closes_position");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
    insert_full_fill(
        &pool, run_id, "order-1", "AAPL", Side::Buy, 10, 150_000_000, 0, "msg-1", at,
    )
    .await;
    insert_full_fill(
        &pool, run_id, "order-2", "AAPL", Side::Sell, 10, 160_000_000, 0, "msg-2", at,
    )
    .await;

    accept_and_refresh(
        &pool,
        run_id,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "0".to_string(),
            avg_price: "0".to_string(),
        }],
    )
    .await;

    let state = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    assert_eq!(state.realized_pnl_micros, 100_000_000, "(160-150)*10");
    assert_eq!(state.accounting_epoch, "complete");

    cleanup(&pool, run_id).await;
}

/// Re-running the refresh with no new inbox rows is an idempotent no-op:
/// the watermark and computed values are unchanged.
#[tokio::test]
async fn duplicate_refresh_has_zero_delta() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("duplicate_refresh_has_zero_delta");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
    insert_full_fill(
        &pool, run_id, "order-1", "AAPL", Side::Buy, 10, 150_000_000, 0, "msg-1", at,
    )
    .await;

    let positions = vec![BrokerPosition {
        symbol: "AAPL".to_string(),
        qty: "10".to_string(),
        avg_price: "150.00".to_string(),
    }];
    accept_and_refresh(&pool, run_id, positions.clone()).await;
    let first = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");

    // Refresh again with the identical durable fill history (no new inbox
    // rows were added in between).
    accept_and_refresh(&pool, run_id, positions).await;
    let second = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");

    assert_eq!(first, second, "re-refreshing unchanged fill history must be a zero-delta no-op");

    cleanup(&pool, run_id).await;
}

/// A partial fill followed by its completing fill applies the exact total
/// exactly once -- not double-counted, not under-counted.
#[tokio::test]
async fn partial_then_final_fill_applies_exact_total_once() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("partial_then_final_fill_applies_exact_total_once");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
    insert_partial_then_final_fill(
        &pool, run_id, "order-1", "AAPL", Side::Buy, 6, 4, 150_000_000, at,
    )
    .await;

    accept_and_refresh(
        &pool,
        run_id,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "10".to_string(),
            avg_price: "150.00".to_string(),
        }],
    )
    .await;

    let state = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    // Exactly 10 shares' worth of cash movement -- not 6, not 4, not 20.
    assert_eq!(state.cash_micros, -1_500_000_000);
    assert_eq!(state.accounting_epoch, "complete");

    cleanup(&pool, run_id).await;
}

/// Restart replay: refreshing again after the fill history has not changed
/// (simulating a fresh process reading the same durable oms_inbox rows)
/// produces byte-identical state.
#[tokio::test]
async fn restart_replay_produces_identical_state() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("restart_replay_produces_identical_state");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
    insert_full_fill(
        &pool, run_id, "order-1", "AAPL", Side::Buy, 10, 150_000_000, 0, "msg-1", at,
    )
    .await;
    insert_full_fill(
        &pool, run_id, "order-2", "AAPL", Side::Sell, 4, 160_000_000, 0, "msg-2", at,
    )
    .await;

    let positions = vec![BrokerPosition {
        symbol: "AAPL".to_string(),
        qty: "6".to_string(),
        avg_price: "150.00".to_string(),
    }];

    accept_and_refresh(&pool, run_id, positions.clone()).await;
    let before = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");

    // A brand new AppState (simulating a fresh process) replays the same
    // durable fill history from scratch.
    accept_and_refresh(&pool, run_id, positions).await;
    let after = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");

    assert_eq!(before.cash_micros, after.cash_micros);
    assert_eq!(before.realized_pnl_micros, after.realized_pnl_micros);
    assert_eq!(before.last_applied_inbox_id, after.last_applied_inbox_id);

    cleanup(&pool, run_id).await;
}

/// A pre-existing broker position with no matching fill history in this
/// run's oms_inbox blocks the accounting epoch (never fabricates an opening
/// fill to force a match).
#[tokio::test]
async fn pre_existing_unmatched_position_blocks_accounting_epoch() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("pre_existing_unmatched_position_blocks_accounting_epoch");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    // No fills at all for this run -- but the broker reports a non-flat
    // position (e.g. adopted from a prior session).
    accept_and_refresh(
        &pool,
        run_id,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "25".to_string(),
            avg_price: "140.00".to_string(),
        }],
    )
    .await;

    let state = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    assert_eq!(state.accounting_epoch, "incomplete");
    assert!(state.accounting_epoch_reason.is_some());
    assert_eq!(
        state.realized_pnl_micros, 0,
        "no fabricated realized P&L -- there is simply no fill history to derive any from"
    );

    cleanup(&pool, run_id).await;
}

/// A partial mismatch (fill history exists but doesn't fully explain the
/// reported broker quantity) is also caught, not just total absence.
#[tokio::test]
async fn partial_mismatch_also_blocks_accounting_epoch() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("partial_mismatch_also_blocks_accounting_epoch");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
    // Only 5 shares' worth of fill history known, but broker reports 25 --
    // e.g. 20 shares were adopted from a prior, unrecorded session.
    insert_full_fill(
        &pool, run_id, "order-1", "AAPL", Side::Buy, 5, 150_000_000, 0, "msg-1", at,
    )
    .await;

    accept_and_refresh(
        &pool,
        run_id,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "25".to_string(),
            avg_price: "145.00".to_string(),
        }],
    )
    .await;

    let state = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    assert_eq!(state.accounting_epoch, "incomplete");

    cleanup(&pool, run_id).await;
}

/// A flat portfolio (no fills, no positions) has known-zero truth, not
/// merely absent truth.
#[tokio::test]
async fn flat_portfolio_has_known_zero_truth() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("flat_portfolio_has_known_zero_truth");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    accept_and_refresh(&pool, run_id, vec![]).await;

    let state = mqk_db::fetch_paper_portfolio_accounting_state(&pool, run_id)
        .await
        .expect("fetch should succeed")
        .expect("accounting state should exist");
    assert_eq!(state.cash_micros, 0);
    assert_eq!(state.realized_pnl_micros, 0);
    assert_eq!(state.fees_micros, 0);
    assert_eq!(state.last_applied_inbox_id, 0);
    assert_eq!(state.accounting_epoch, "complete");

    cleanup(&pool, run_id).await;
}

/// This patch never writes to `sys_account_equity_baseline` -- the
/// existing daily-P&L baseline table is reused unchanged, not touched by
/// the new accounting projection.
#[tokio::test]
async fn daily_pnl_baseline_table_untouched() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let run_id = fixed_run_id("daily_pnl_baseline_table_untouched");
    cleanup(&pool, run_id).await;
    fixture_run(&pool, run_id).await;

    let before: i64 = sqlx::query_scalar("select count(*) from sys_account_equity_baseline")
        .fetch_one(&pool)
        .await
        .expect("count should succeed");

    let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
    insert_full_fill(
        &pool, run_id, "order-1", "AAPL", Side::Buy, 10, 150_000_000, 0, "msg-1", at,
    )
    .await;
    accept_and_refresh(
        &pool,
        run_id,
        vec![BrokerPosition {
            symbol: "AAPL".to_string(),
            qty: "10".to_string(),
            avg_price: "150.00".to_string(),
        }],
    )
    .await;

    let after: i64 = sqlx::query_scalar("select count(*) from sys_account_equity_baseline")
        .fetch_one(&pool)
        .await
        .expect("count should succeed");
    assert_eq!(before, after, "daily equity baseline table must be untouched");

    cleanup(&pool, run_id).await;
}
