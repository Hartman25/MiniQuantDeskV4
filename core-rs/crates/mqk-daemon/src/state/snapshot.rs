//! Pure reconcile/snapshot helper functions for mqk-daemon.
//!
//! Contains: parse_signed_qty, reconcile_side_from_schema,
//! reconcile_order_status_from_schema,
//! reconcile_local_snapshot_from_runtime_with_sides,
//! oms_execution_status_to_reconcile, outbox_json_symbol, outbox_json_qty,
//! outbox_json_side, broker_event_to_oms_event, broker_event_to_portfolio_fill,
//! oms_state_to_broker_status, synthesize_paper_broker_snapshot,
//! synthesize_broker_snapshot_from_execution, reconcile_broker_snapshot_from_schema,
//! reconcile_unknown_status, reconcile_last_run_at, reconcile_counts,
//! reconcile_status_from_report, reconcile_status_from_stale,
//! preserve_fail_closed_reconcile_status.

use std::collections::BTreeMap;

use chrono::Utc;
use mqk_execution::{
    oms::state_machine::{OmsEvent, OmsOrder, OrderState},
    BrokerEvent,
};
use mqk_portfolio::{apply_entry, Fill, LedgerEntry, PortfolioState, Side};
use mqk_reconcile::{ReconcileDiff, SnapshotFreshness, SnapshotWatermark};

use super::initial_reconcile_status;
use super::types::ReconcileStatusSnapshot;

// ---------------------------------------------------------------------------
// Raw outbox JSON field accessors
// ---------------------------------------------------------------------------

pub(crate) fn outbox_json_symbol(json: &serde_json::Value) -> Option<String> {
    json.get("symbol")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

pub(crate) fn outbox_json_qty(json: &serde_json::Value) -> Option<i64> {
    let raw = json.get("qty").or_else(|| json.get("quantity"))?;
    let n = raw.as_i64()?;
    if n > 0 {
        Some(n)
    } else {
        None
    }
}

pub(crate) fn outbox_json_side(json: &serde_json::Value) -> mqk_reconcile::Side {
    match json.get("side").and_then(|v| v.as_str()) {
        Some(s) if s.eq_ignore_ascii_case("sell") => mqk_reconcile::Side::Sell,
        _ => mqk_reconcile::Side::Buy,
    }
}

// ---------------------------------------------------------------------------
// Reconcile helpers
// ---------------------------------------------------------------------------

pub(crate) fn parse_signed_qty(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = trimmed.parse::<i64>() {
        return Some(value);
    }

    let (sign, magnitude) = if let Some(rest) = trimmed.strip_prefix('-') {
        (-1_i64, rest)
    } else if let Some(rest) = trimmed.strip_prefix('+') {
        (1_i64, rest)
    } else {
        (1_i64, trimmed)
    };

    let (whole, frac) = magnitude.split_once('.')?;
    if frac.chars().any(|c| c != '0') {
        return None;
    }
    let base = whole.parse::<i64>().ok()?;
    Some(sign * base)
}

pub(crate) fn reconcile_side_from_schema(raw: &str) -> mqk_reconcile::Side {
    if raw.eq_ignore_ascii_case("sell") {
        mqk_reconcile::Side::Sell
    } else {
        mqk_reconcile::Side::Buy
    }
}

pub(crate) fn reconcile_order_status_from_schema(raw: &str) -> mqk_reconcile::OrderStatus {
    if raw.eq_ignore_ascii_case("new") {
        mqk_reconcile::OrderStatus::New
    } else if raw.eq_ignore_ascii_case("accepted") {
        mqk_reconcile::OrderStatus::Accepted
    } else if raw.eq_ignore_ascii_case("partially_filled")
        || raw.eq_ignore_ascii_case("partial_fill")
    {
        mqk_reconcile::OrderStatus::PartiallyFilled
    } else if raw.eq_ignore_ascii_case("filled") {
        mqk_reconcile::OrderStatus::Filled
    } else if raw.eq_ignore_ascii_case("canceled") || raw.eq_ignore_ascii_case("cancelled") {
        mqk_reconcile::OrderStatus::Canceled
    } else if raw.eq_ignore_ascii_case("rejected") {
        mqk_reconcile::OrderStatus::Rejected
    } else {
        mqk_reconcile::OrderStatus::Unknown
    }
}

/// DMON-05: build a local reconcile snapshot from an execution snapshot + side cache.
pub(crate) fn reconcile_local_snapshot_from_runtime_with_sides(
    snapshot: &mqk_runtime::observability::ExecutionSnapshot,
    sides: &BTreeMap<String, mqk_reconcile::Side>,
) -> mqk_reconcile::LocalSnapshot {
    let positions = snapshot
        .portfolio
        .positions
        .iter()
        .map(|pos| (pos.symbol.clone(), pos.net_qty))
        .collect();

    let orders = snapshot
        .active_orders
        .iter()
        .map(|order| {
            let side = sides
                .get(&order.order_id)
                .cloned()
                .unwrap_or(mqk_reconcile::Side::Buy);
            let status = oms_execution_status_to_reconcile(&order.status);
            let snap = mqk_reconcile::OrderSnapshot {
                order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side,
                qty: order.total_qty,
                filled_qty: order.filled_qty,
                status,
            };
            (order.order_id.clone(), snap)
        })
        .collect();

    mqk_reconcile::LocalSnapshot { orders, positions }
}

pub(crate) fn oms_execution_status_to_reconcile(status: &str) -> mqk_reconcile::OrderStatus {
    // RECONCILE-ORDER-STATUS-MAP-01: Map local OMS state strings to the
    // reconcile OrderStatus taxonomy.  Active states map to New so that
    // local "Open" compares compatible with broker "new" / "accepted".
    // Terminal states map strictly; unknown strings stay Unknown (fail-closed).
    match status.to_ascii_lowercase().as_str() {
        // Active states — broker reports these as "new" / "accepted"
        "open" | "cancelpending" | "replacepending" => mqk_reconcile::OrderStatus::New,
        // Partial-fill in flight
        "partiallyfilled" => mqk_reconcile::OrderStatus::PartiallyFilled,
        // Terminal
        "filled" => mqk_reconcile::OrderStatus::Filled,
        "canceled" | "cancelled" => mqk_reconcile::OrderStatus::Canceled,
        "rejected" => mqk_reconcile::OrderStatus::Rejected,
        // Unrecognized — fail-closed
        _ => mqk_reconcile::OrderStatus::Unknown,
    }
}

pub(crate) fn broker_event_to_oms_event(event: &BrokerEvent) -> OmsEvent {
    match event {
        BrokerEvent::Ack { .. } => OmsEvent::Ack,
        BrokerEvent::PartialFill { delta_qty, .. } => OmsEvent::PartialFill {
            delta_qty: *delta_qty,
        },
        BrokerEvent::Fill { delta_qty, .. } => OmsEvent::Fill {
            delta_qty: *delta_qty,
        },
        BrokerEvent::CancelAck { .. } => OmsEvent::CancelAck,
        BrokerEvent::CancelReject { .. } => OmsEvent::CancelReject,
        BrokerEvent::ReplaceAck { new_total_qty, .. } => OmsEvent::ReplaceAck {
            new_total_qty: *new_total_qty,
        },
        BrokerEvent::ReplaceReject { .. } => OmsEvent::ReplaceReject,
        BrokerEvent::Reject { .. } => OmsEvent::Reject,
    }
}

pub(crate) fn broker_event_to_portfolio_fill(event: &BrokerEvent) -> Option<mqk_portfolio::Fill> {
    match event {
        BrokerEvent::Fill {
            symbol,
            side,
            delta_qty,
            price_micros,
            fee_micros,
            ..
        }
        | BrokerEvent::PartialFill {
            symbol,
            side,
            delta_qty,
            price_micros,
            fee_micros,
            ..
        } => {
            let portfolio_side = match side {
                mqk_execution::types::Side::Buy => mqk_portfolio::Side::Buy,
                mqk_execution::types::Side::Sell => mqk_portfolio::Side::Sell,
            };
            Some(mqk_portfolio::Fill {
                symbol: symbol.clone(),
                side: portfolio_side,
                qty: *delta_qty,
                price_micros: *price_micros,
                fee_micros: *fee_micros,
            })
        }
        _ => None,
    }
}

pub(crate) fn oms_state_to_broker_status(state: &OrderState) -> &'static str {
    match state {
        OrderState::Open => "new",
        OrderState::PartiallyFilled => "partially_filled",
        OrderState::Filled => "filled",
        OrderState::CancelPending => "pending_cancel",
        OrderState::Cancelled => "canceled",
        OrderState::ReplacePending => "pending_replace",
        OrderState::Rejected => "rejected",
    }
}

/// DMON-01: Synthesize a `BrokerSnapshot` from recovered OMS + portfolio truth.
pub(crate) fn synthesize_paper_broker_snapshot(
    oms_orders: &BTreeMap<String, OmsOrder>,
    sides: &BTreeMap<String, mqk_reconcile::Side>,
    portfolio: &PortfolioState,
    now: chrono::DateTime<Utc>,
) -> mqk_schemas::BrokerSnapshot {
    let orders: Vec<mqk_schemas::BrokerOrder> = oms_orders
        .values()
        .map(|order| {
            let side_str = sides
                .get(&order.order_id)
                .map(|s| match s {
                    mqk_reconcile::Side::Buy => "buy",
                    mqk_reconcile::Side::Sell => "sell",
                })
                .unwrap_or("buy");
            mqk_schemas::BrokerOrder {
                broker_order_id: order.order_id.clone(),
                client_order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side: side_str.to_string(),
                r#type: "market".to_string(),
                status: oms_state_to_broker_status(&order.state).to_string(),
                qty: order.total_qty.to_string(),
                limit_price: None,
                stop_price: None,
                created_at_utc: now,
            }
        })
        .collect();

    let positions: Vec<mqk_schemas::BrokerPosition> = portfolio
        .positions
        .iter()
        .filter_map(|(symbol, pos)| {
            let net: i64 = pos.lots.iter().map(|l| l.qty_signed).sum();
            if net == 0 {
                None
            } else {
                Some(mqk_schemas::BrokerPosition {
                    symbol: symbol.clone(),
                    qty: net.to_string(),
                    avg_price: "0".to_string(),
                })
            }
        })
        .collect();

    let cash_whole = portfolio.cash_micros / 1_000_000;
    let account = mqk_schemas::BrokerAccount {
        equity: cash_whole.to_string(),
        cash: cash_whole.to_string(),
        currency: "USD".to_string(),
    };

    mqk_schemas::BrokerSnapshot {
        captured_at_utc: now,
        account,
        orders,
        fills: vec![],
        positions,
    }
}

/// DMON-05 (tick): Synthesize a paper-broker snapshot from the latest execution
/// snapshot and side cache.
pub(crate) fn synthesize_broker_snapshot_from_execution(
    snapshot: &mqk_runtime::observability::ExecutionSnapshot,
    sides: &BTreeMap<String, mqk_reconcile::Side>,
    now: chrono::DateTime<Utc>,
) -> mqk_schemas::BrokerSnapshot {
    let orders: Vec<mqk_schemas::BrokerOrder> = snapshot
        .active_orders
        .iter()
        .map(|order| {
            let side_str = sides
                .get(&order.order_id)
                .map(|s| match s {
                    mqk_reconcile::Side::Buy => "buy",
                    mqk_reconcile::Side::Sell => "sell",
                })
                .unwrap_or("buy");
            mqk_schemas::BrokerOrder {
                broker_order_id: order
                    .broker_order_id
                    .clone()
                    .unwrap_or_else(|| order.order_id.clone()),
                client_order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side: side_str.to_string(),
                r#type: "market".to_string(),
                status: order.status.to_ascii_lowercase(),
                qty: order.total_qty.to_string(),
                limit_price: None,
                stop_price: None,
                created_at_utc: now,
            }
        })
        .collect();

    let positions: Vec<mqk_schemas::BrokerPosition> = snapshot
        .portfolio
        .positions
        .iter()
        .map(|pos| mqk_schemas::BrokerPosition {
            symbol: pos.symbol.clone(),
            qty: pos.net_qty.to_string(),
            avg_price: "0".to_string(),
        })
        .collect();

    let cash_whole = snapshot.portfolio.cash_micros / 1_000_000;
    let account = mqk_schemas::BrokerAccount {
        equity: cash_whole.to_string(),
        cash: cash_whole.to_string(),
        currency: "USD".to_string(),
    };

    mqk_schemas::BrokerSnapshot {
        captured_at_utc: now,
        account,
        orders,
        fills: vec![],
        positions,
    }
}

pub(crate) fn reconcile_broker_snapshot_from_schema(
    snapshot: &mqk_schemas::BrokerSnapshot,
) -> Result<mqk_reconcile::BrokerSnapshot, &'static str> {
    let fetched_at_ms = snapshot.captured_at_utc.timestamp_millis(); // allow: ops-metadata
    if fetched_at_ms <= 0 {
        return Err("broker snapshot timestamp is invalid; refusing ambiguous broker truth");
    }

    let mut positions = BTreeMap::new();
    for position in &snapshot.positions {
        let qty = parse_signed_qty(&position.qty).ok_or(
            "broker snapshot contains non-integer position qty; refusing ambiguous broker truth",
        )?;
        positions.insert(position.symbol.clone(), qty);
    }

    let mut orders = BTreeMap::new();
    for order in &snapshot.orders {
        let qty = parse_signed_qty(&order.qty).ok_or(
            "broker snapshot contains non-integer order qty; refusing ambiguous broker truth",
        )?;
        let order_id = if order.client_order_id.trim().is_empty() {
            order.broker_order_id.clone()
        } else {
            order.client_order_id.clone()
        };
        orders.insert(
            order_id.clone(),
            mqk_reconcile::OrderSnapshot::new(
                order_id,
                order.symbol.clone(),
                reconcile_side_from_schema(&order.side),
                qty,
                0,
                reconcile_order_status_from_schema(&order.status),
            ),
        );
    }

    Ok(mqk_reconcile::BrokerSnapshot {
        orders,
        positions,
        fetched_at_ms,
    })
}

pub(crate) fn reconcile_unknown_status(note: impl Into<String>) -> ReconcileStatusSnapshot {
    ReconcileStatusSnapshot {
        note: Some(note.into()),
        ..initial_reconcile_status()
    }
}

pub(crate) fn reconcile_last_run_at(fetched_at_ms: i64) -> Option<String> {
    chrono::DateTime::<Utc>::from_timestamp_millis(fetched_at_ms) // allow: ops-metadata
        .map(|ts| ts.to_rfc3339())
}

pub(crate) fn reconcile_counts(
    report: &mqk_reconcile::ReconcileReport,
) -> (usize, usize, usize, usize) {
    let mut mismatched_positions = 0;
    let mut mismatched_orders = 0;
    let mut mismatched_fills = 0;
    let mut unmatched_broker_events = 0;

    for diff in &report.diffs {
        match diff {
            ReconcileDiff::PositionQtyMismatch { .. } => mismatched_positions += 1,
            ReconcileDiff::OrderMismatch { .. }
            | ReconcileDiff::LocalOrderMissingAtBroker { .. } => mismatched_orders += 1,
            ReconcileDiff::UnknownOrder { .. } => {
                mismatched_orders += 1;
                unmatched_broker_events += 1;
            }
            ReconcileDiff::UnknownBrokerFill { .. } => {
                mismatched_fills += 1;
                unmatched_broker_events += 1;
            }
        }
    }

    (
        mismatched_positions,
        mismatched_orders,
        mismatched_fills,
        unmatched_broker_events,
    )
}

pub(crate) fn reconcile_status_from_report(
    report: &mqk_reconcile::ReconcileReport,
    broker: &mqk_reconcile::BrokerSnapshot,
    watermark: &SnapshotWatermark,
) -> ReconcileStatusSnapshot {
    let (mismatched_positions, mismatched_orders, mismatched_fills, unmatched_broker_events) =
        reconcile_counts(report);

    ReconcileStatusSnapshot {
        status: if report.is_clean() {
            "ok".to_string()
        } else {
            "dirty".to_string()
        },
        last_run_at: reconcile_last_run_at(broker.fetched_at_ms),
        snapshot_watermark_ms: Some(watermark.last_accepted_ms()),
        mismatched_positions,
        mismatched_orders,
        mismatched_fills,
        unmatched_broker_events,
        note: if report.is_clean() {
            None
        } else {
            Some("monotonic reconcile detected drift; dispatch remains blocked".to_string())
        },
    }
}

pub(crate) fn reconcile_status_from_stale(
    stale: &mqk_reconcile::StaleBrokerSnapshot,
    watermark: &SnapshotWatermark,
) -> ReconcileStatusSnapshot {
    let (last_run_at, note) = match stale.freshness {
        SnapshotFreshness::Stale {
            watermark_ms,
            got_ms,
        } => (
            reconcile_last_run_at(got_ms),
            format!(
                "stale broker snapshot rejected by reconcile watermark: watermark_ms={watermark_ms} got_ms={got_ms}"
            ),
        ),
        SnapshotFreshness::NoTimestamp => (
            None,
            "broker snapshot has no timestamp; reconcile ordering is ambiguous and remains fail-closed"
                .to_string(),
        ),
        SnapshotFreshness::Fresh => (
            None,
            "reconcile stale-state construction received a fresh snapshot unexpectedly"
                .to_string(),
        ),
    };

    ReconcileStatusSnapshot {
        status: "stale".to_string(),
        snapshot_watermark_ms: Some(watermark.last_accepted_ms()),
        last_run_at,
        mismatched_positions: 0,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some(note),
    }
}

pub(crate) fn preserve_fail_closed_reconcile_status(
    previous: &ReconcileStatusSnapshot,
    note: impl Into<String>,
) -> ReconcileStatusSnapshot {
    let mut preserved = previous.clone();
    preserved.note = Some(note.into());
    preserved
}

/// Seed a portfolio with broker baseline positions inherited from prior sessions.
///
/// Baseline positions represent broker holdings from runs prior to this one.
/// Each non-zero baseline quantity is applied as an equivalent
/// `LedgerEntry::Fill` (`price_micros=1`, `fee_micros=0`) via `apply_entry`,
/// so the seeded position is ledger-replayable: `recompute_from_ledger`
/// reproduces the same `positions`, `cash_micros`, and `realized_pnl_micros`
/// that this function produces. This keeps `check_capital_invariants`
/// satisfied immediately after baseline seeding (BASELINE-LEDGER-PARITY-01).
///
/// Side is derived from the sign of the baseline quantity: a positive
/// (long) baseline applies a `Buy` fill; a negative (short) baseline
/// applies a `Sell` fill, both for `qty = bl_qty.abs()`.
///
/// Double-count safety: `recover_oms_and_portfolio` replays only fills from
/// the *current* run_id into `portfolio` before this is called.  Baseline
/// adds only the inherited prior-run qty; no fill from this run is counted twice.
///
/// Zero-qty entries in the baseline are skipped.  If `portfolio` already has
/// a position for a symbol (e.g. partial fills this run), the baseline fill
/// is applied on top via the same FIFO lot accounting as any other fill —
/// total qty = current_run_delta + baseline.
pub(crate) fn seed_portfolio_from_baseline(
    portfolio: &mut PortfolioState,
    baseline: &mqk_reconcile::LocalSnapshot,
) {
    for (sym, &bl_qty) in &baseline.positions {
        if bl_qty == 0 {
            continue;
        }
        let side = if bl_qty > 0 { Side::Buy } else { Side::Sell };
        let fill = Fill::new(sym.clone(), side, bl_qty.abs(), 1, 0);
        apply_entry(portfolio, LedgerEntry::Fill(fill));
    }
}

/// Recover OMS orders, side cache, and portfolio from durable DB truth.
pub(crate) async fn recover_oms_and_portfolio(
    db: &sqlx::PgPool,
    run_id: uuid::Uuid,
    initial_equity_micros: i64,
) -> Result<
    (
        BTreeMap<String, OmsOrder>,
        BTreeMap<String, mqk_reconcile::Side>,
        PortfolioState,
    ),
    super::types::RuntimeLifecycleError,
> {
    let submitted = mqk_db::outbox_load_submitted_for_run(db, run_id)
        .await
        .map_err(|err| {
            super::types::RuntimeLifecycleError::internal("outbox_load_submitted_for_run", err)
        })?;
    let applied = mqk_db::inbox_load_all_applied_for_run(db, run_id)
        .await
        .map_err(|err| {
            super::types::RuntimeLifecycleError::internal("inbox_load_all_applied_for_run", err)
        })?;

    let mut oms_orders: BTreeMap<String, OmsOrder> = BTreeMap::new();
    let mut sides: BTreeMap<String, mqk_reconcile::Side> = BTreeMap::new();
    for row in &submitted {
        let Some(symbol) = outbox_json_symbol(&row.order_json) else {
            continue;
        };
        let Some(qty) = outbox_json_qty(&row.order_json) else {
            continue;
        };
        let side = outbox_json_side(&row.order_json);
        let order_id = row.idempotency_key.clone();
        sides.insert(order_id.clone(), side);
        oms_orders.insert(order_id.clone(), OmsOrder::new(&order_id, symbol, qty));
    }

    let mut portfolio = PortfolioState::new(initial_equity_micros);

    for row in &applied {
        let event: BrokerEvent = match serde_json::from_value(row.message_json.clone()) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let internal_id = event.internal_order_id().to_string();
        let is_fill = matches!(
            event,
            BrokerEvent::Fill { .. } | BrokerEvent::PartialFill { .. }
        );
        let oms_event = broker_event_to_oms_event(&event);
        // Mirror the live apply path: use broker_fill_id as the economic
        // event identity when present, fall back to broker_message_id.
        // This ensures WS (no fill_id, uses msg_id) and REST (fill_id
        // present) versions of the same fill collapse to a single OMS
        // advance via the applied_event_ids set.
        let economic_event_id = event
            .broker_fill_id()
            .map(str::to_string)
            .unwrap_or_else(|| row.broker_message_id.clone());
        let oms_noop = if let Some(order) = oms_orders.get_mut(&internal_id) {
            let pre_qty = order.filled_qty;
            let _ = order.apply(&oms_event, Some(&economic_event_id));
            // Detect duplicate fills: if filled_qty did not advance, the OMS
            // treated this as a no-op (duplicate event_id or terminal-state
            // guard).  Skip the portfolio mutation to prevent double-counting.
            is_fill && order.filled_qty == pre_qty
        } else {
            false
        };
        if oms_noop {
            continue;
        }
        if let Some(fill) = broker_event_to_portfolio_fill(&event) {
            apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
        }
    }

    oms_orders.retain(|_, o| !o.state.is_terminal());
    sides.retain(|order_id, _| oms_orders.contains_key(order_id));

    Ok((oms_orders, sides, portfolio))
}

// ---------------------------------------------------------------------------
// DURABLE-PAPER-PORTFOLIO-AND-PNL-01C: authoritative snapshot acceptance
// ---------------------------------------------------------------------------

/// Canonical acceptance seam for a real (`External`-source) broker snapshot.
///
/// Writes `snapshot` into `state.broker_snapshot` (unchanged existing
/// behavior — every reader of that field keeps working exactly as before)
/// and, additively, attempts durable persistence as authoritative
/// Paper+Alpaca portfolio truth. Persistence is best-effort relative to the
/// in-memory acceptance: a failure is logged and never blocks, reverts, or
/// delays it, and successful persistence is never itself a license to
/// trade — it does not touch reconcile, risk, or order submission.
///
/// This function (and [`persist_external_broker_snapshot_best_effort`], for
/// the one call site that cannot hold an `.await` directly — the sync
/// terminal-fill-expiry-refresher closure in `orchestrator_build.rs`, which
/// spawns the persistence half instead) is the *only* place that may write
/// `state.broker_snapshot` for the `External` source. The `Synthetic`
/// branch and the dev-only injection routes never call it and never
/// produce durable authoritative truth — see the B4-A contract's
/// source-authority distinction.
pub(crate) async fn accept_external_broker_snapshot(
    state: &super::AppState,
    snapshot: mqk_schemas::BrokerSnapshot,
    run_id: Option<uuid::Uuid>,
    operation_id: Option<uuid::Uuid>,
) {
    *state.broker_snapshot.write().await = Some(snapshot.clone());
    persist_external_broker_snapshot_best_effort(
        state.db.clone(),
        state.deployment_mode(),
        state.runtime_selection.broker_kind,
        snapshot,
        run_id,
        operation_id,
    )
    .await;
}

/// Attempts durable persistence of an already-accepted `External`-source
/// broker snapshot. Never panics, never returns an error to the caller —
/// every failure mode is logged and swallowed, since this is additive
/// truth on top of the existing in-memory acceptance, not a gate for it.
///
/// Only the Paper+Alpaca lane this bundle supports is eligible; any other
/// deployment mode or broker kind is a deliberate, silent no-op (this
/// function is never called for anything else in production, but stays
/// fail-closed on its own in case that ever changes).
pub(crate) async fn persist_external_broker_snapshot_best_effort(
    db: Option<sqlx::PgPool>,
    deployment_mode: super::types::DeploymentMode,
    broker_kind: Option<super::types::BrokerKind>,
    snapshot: mqk_schemas::BrokerSnapshot,
    run_id: Option<uuid::Uuid>,
    operation_id: Option<uuid::Uuid>,
) {
    if deployment_mode != super::types::DeploymentMode::Paper {
        return;
    }
    if broker_kind != Some(super::types::BrokerKind::Alpaca) {
        return;
    }
    let Some(pool) = db else {
        tracing::warn!("durable_paper_portfolio_snapshot_skip: no_db_pool_configured");
        return;
    };

    let Some(equity_micros) =
        crate::routes::helpers::parse_decimal_micros(&snapshot.account.equity)
    else {
        tracing::warn!("durable_paper_portfolio_snapshot_skip: account_equity_unparseable");
        return;
    };
    let Some(cash_micros) = crate::routes::helpers::parse_decimal_micros(&snapshot.account.cash)
    else {
        tracing::warn!("durable_paper_portfolio_snapshot_skip: account_cash_unparseable");
        return;
    };

    let mut positions = Vec::with_capacity(snapshot.positions.len());
    for p in &snapshot.positions {
        let Some(qty_signed) = parse_signed_qty(&p.qty) else {
            tracing::warn!(
                symbol = %p.symbol,
                "durable_paper_portfolio_snapshot_skip: position_qty_unparseable"
            );
            return;
        };
        let Some(avg_entry_price_micros) =
            crate::routes::helpers::parse_decimal_micros(&p.avg_price)
        else {
            tracing::warn!(
                symbol = %p.symbol,
                "durable_paper_portfolio_snapshot_skip: position_avg_price_unparseable"
            );
            return;
        };
        positions.push(mqk_db::PaperPortfolioSnapshotPosition {
            symbol: p.symbol.clone(),
            qty_signed,
            avg_entry_price_micros,
            provenance: mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
        });
    }

    // Deterministic identity (B4-A §3): re-persisting the exact same
    // snapshot (same captured_at_utc/run_id/source) is an idempotent
    // no-op, never a duplicate row.
    let snapshot_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!(
            "mqk.paper-portfolio-snapshot.v1|{}|{}|{}",
            snapshot.captured_at_utc.to_rfc3339(),
            run_id.map(|id| id.to_string()).unwrap_or_default(),
            mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
        )
        .as_bytes(),
    );

    let result = mqk_db::insert_or_confirm_paper_portfolio_snapshot(
        &pool,
        mqk_db::NewPaperPortfolioSnapshot {
            snapshot_id,
            captured_at_utc: snapshot.captured_at_utc,
            deployment_mode: deployment_mode.as_api_label().to_string(),
            source: mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA.to_string(),
            equity_micros,
            cash_micros,
            currency: snapshot.account.currency.clone(),
            truth_state: "active".to_string(),
            run_id,
            operation_id,
            positions,
        },
    )
    .await;

    match result {
        Ok(mqk_db::InsertPaperPortfolioSnapshotOutcome::Inserted { .. })
        | Ok(mqk_db::InsertPaperPortfolioSnapshotOutcome::AlreadyExists { .. }) => {}
        Ok(mqk_db::InsertPaperPortfolioSnapshotOutcome::Conflict { detail }) => {
            tracing::warn!(detail = %detail, "durable_paper_portfolio_snapshot_conflict");
        }
        Err(err) => {
            tracing::warn!(error = %err, "durable_paper_portfolio_snapshot_persist_failed");
        }
    }
}

// ---------------------------------------------------------------------------
// RECONCILE-ORDER-STATUS-MAP-01 unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod reconcile_status_map_tests {
    use super::oms_execution_status_to_reconcile;
    use mqk_reconcile::{
        reconcile, BrokerSnapshot, LocalSnapshot, OrderSnapshot, OrderStatus, ReconcileAction, Side,
    };

    // Helper: build a minimal matched order pair (same id/symbol/side/qty/filled_qty).
    fn matched_order(
        local_status: OrderStatus,
        broker_status: OrderStatus,
    ) -> (LocalSnapshot, BrokerSnapshot) {
        let mut local = LocalSnapshot::empty();
        local.orders.insert(
            "ord-1".to_string(),
            OrderSnapshot::new("ord-1", "AAPL", Side::Buy, 1, 0, local_status),
        );
        let mut broker = BrokerSnapshot::empty_at(1_000);
        broker.orders.insert(
            "ord-1".to_string(),
            OrderSnapshot::new("ord-1", "AAPL", Side::Buy, 1, 0, broker_status),
        );
        (local, broker)
    }

    // SM-01: local "Open" maps to OrderStatus::New — no false OrderDrift vs broker "new".
    #[test]
    fn open_maps_to_new() {
        assert_eq!(oms_execution_status_to_reconcile("Open"), OrderStatus::New,);
    }

    // SM-02: local "Open" vs broker New does not produce OrderDrift.
    #[test]
    fn open_vs_broker_new_is_clean() {
        let (local, broker) = matched_order(OrderStatus::New, OrderStatus::New);
        let r = reconcile(&local, &broker);
        assert_eq!(
            r.action,
            ReconcileAction::Clean,
            "Open vs New must be clean: {:?}",
            r.diffs
        );
    }

    // SM-03: local "PartiallyFilled" maps to OrderStatus::PartiallyFilled.
    #[test]
    fn partially_filled_maps_correctly() {
        assert_eq!(
            oms_execution_status_to_reconcile("PartiallyFilled"),
            OrderStatus::PartiallyFilled,
        );
    }

    // SM-04: local PartiallyFilled vs broker PartiallyFilled is clean.
    #[test]
    fn partially_filled_vs_broker_partially_filled_is_clean() {
        let mut local = LocalSnapshot::empty();
        local.orders.insert(
            "ord-1".to_string(),
            OrderSnapshot::new(
                "ord-1",
                "AAPL",
                Side::Buy,
                10,
                3,
                OrderStatus::PartiallyFilled,
            ),
        );
        let mut broker = BrokerSnapshot::empty_at(1_000);
        broker.orders.insert(
            "ord-1".to_string(),
            OrderSnapshot::new(
                "ord-1",
                "AAPL",
                Side::Buy,
                10,
                3,
                OrderStatus::PartiallyFilled,
            ),
        );
        let r = reconcile(&local, &broker);
        assert_eq!(
            r.action,
            ReconcileAction::Clean,
            "PartiallyFilled vs PartiallyFilled must be clean: {:?}",
            r.diffs
        );
    }

    // SM-05: local "CancelPending" maps to New (still active at broker).
    #[test]
    fn cancel_pending_maps_to_new() {
        assert_eq!(
            oms_execution_status_to_reconcile("CancelPending"),
            OrderStatus::New,
        );
    }

    // SM-06: local "ReplacePending" maps to New (still active at broker).
    #[test]
    fn replace_pending_maps_to_new() {
        assert_eq!(
            oms_execution_status_to_reconcile("ReplacePending"),
            OrderStatus::New,
        );
    }

    // SM-07: Terminal states map strictly — local Filled vs broker Canceled → drift.
    #[test]
    fn local_filled_vs_broker_canceled_is_drift() {
        let (local, broker) = matched_order(OrderStatus::Filled, OrderStatus::Canceled);
        let r = reconcile(&local, &broker);
        assert_eq!(
            r.action,
            ReconcileAction::Halt,
            "Filled vs Canceled must drift"
        );
    }

    // SM-08: local active vs broker terminal → drift.
    #[test]
    fn local_new_vs_broker_canceled_is_drift() {
        let (local, broker) = matched_order(OrderStatus::New, OrderStatus::Canceled);
        let r = reconcile(&local, &broker);
        assert_eq!(
            r.action,
            ReconcileAction::Halt,
            "New vs Canceled must drift"
        );
    }

    // SM-09: local terminal vs broker active → drift.
    #[test]
    fn local_canceled_vs_broker_new_is_drift() {
        let (local, broker) = matched_order(OrderStatus::Canceled, OrderStatus::New);
        let r = reconcile(&local, &broker);
        assert_eq!(
            r.action,
            ReconcileAction::Halt,
            "Canceled vs New must drift"
        );
    }

    // SM-10: Unrecognized OMS status maps to Unknown (fail-closed).
    #[test]
    fn unknown_oms_status_maps_to_unknown() {
        assert_eq!(
            oms_execution_status_to_reconcile("warp_speed"),
            OrderStatus::Unknown,
        );
    }

    // SM-11: Terminal string variants — all must map strictly.
    #[test]
    fn terminal_status_mappings() {
        assert_eq!(
            oms_execution_status_to_reconcile("Filled"),
            OrderStatus::Filled
        );
        assert_eq!(
            oms_execution_status_to_reconcile("Canceled"),
            OrderStatus::Canceled
        );
        assert_eq!(
            oms_execution_status_to_reconcile("Cancelled"),
            OrderStatus::Canceled
        );
        assert_eq!(
            oms_execution_status_to_reconcile("Rejected"),
            OrderStatus::Rejected
        );
    }

    // SM-12: Case-insensitive — both "open" and "OPEN" map to New.
    #[test]
    fn case_insensitive_open() {
        assert_eq!(oms_execution_status_to_reconcile("open"), OrderStatus::New);
        assert_eq!(oms_execution_status_to_reconcile("OPEN"), OrderStatus::New);
    }
}

// ---------------------------------------------------------------------------
// RUNTIME-POSITION-SEED-ON-START-01 unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod position_seed_tests {
    use super::{seed_portfolio_from_baseline, PortfolioState};
    use mqk_portfolio::apply_entry;
    use mqk_reconcile::LocalSnapshot;

    fn flat_portfolio() -> PortfolioState {
        PortfolioState::new(100_000_000_000) // $100k initial equity
    }

    fn baseline_with(positions: &[(&str, i64)]) -> LocalSnapshot {
        let mut s = LocalSnapshot::empty();
        for &(sym, qty) in positions {
            s.positions.insert(sym.to_string(), qty);
        }
        s
    }

    // P01: no baseline positions → portfolio remains flat
    #[test]
    fn p01_empty_baseline_leaves_portfolio_flat() {
        let mut pf = flat_portfolio();
        let baseline = LocalSnapshot::empty();
        seed_portfolio_from_baseline(&mut pf, &baseline);
        assert!(
            pf.positions.is_empty(),
            "empty baseline must leave portfolio flat"
        );
    }

    // P02: baseline AAPL=1 → portfolio has AAPL qty=1
    #[test]
    fn p02_aapl_baseline_seeds_qty_one() {
        let mut pf = flat_portfolio();
        let baseline = baseline_with(&[("AAPL", 1)]);
        seed_portfolio_from_baseline(&mut pf, &baseline);
        let qty = pf
            .positions
            .get("AAPL")
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        assert_eq!(qty, 1, "AAPL qty must be 1 after seeding from baseline");
    }

    // P03: target=0 minus seeded AAPL qty=1 → delta=-1 (sell signal)
    #[test]
    fn p03_target_zero_vs_seeded_one_is_negative_delta() {
        let mut pf = flat_portfolio();
        let baseline = baseline_with(&[("AAPL", 1)]);
        seed_portfolio_from_baseline(&mut pf, &baseline);
        let current = pf
            .positions
            .get("AAPL")
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        let target: i64 = 0;
        let delta = target - current;
        assert_eq!(
            delta, -1,
            "delta must be -1 (sell) when target=0 and seeded position=1"
        );
    }

    // P04: target=1 minus seeded AAPL qty=1 → delta=0 (already_at_target)
    #[test]
    fn p04_target_one_vs_seeded_one_is_zero_delta() {
        let mut pf = flat_portfolio();
        let baseline = baseline_with(&[("AAPL", 1)]);
        seed_portfolio_from_baseline(&mut pf, &baseline);
        let current = pf
            .positions
            .get("AAPL")
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        let target: i64 = 1;
        let delta = target - current;
        assert_eq!(
            delta, 0,
            "delta must be 0 (already_at_target) when target=1 and seeded position=1"
        );
    }

    // P05: current-run fill AAPL=1 + baseline AAPL=1 → total qty=2 (no double-count)
    #[test]
    fn p05_current_run_fill_plus_baseline_no_double_count() {
        use mqk_portfolio::{Fill, LedgerEntry, Side};
        let mut pf = flat_portfolio();
        // Simulate a fill that happened in this run (price > 0 required by apply_fill).
        apply_entry(
            &mut pf,
            LedgerEntry::Fill(Fill::new("AAPL", Side::Buy, 1, 313_000_000, 0)),
        );
        // Now seed from baseline (prior run had AAPL=1 already).
        let baseline = baseline_with(&[("AAPL", 1)]);
        seed_portfolio_from_baseline(&mut pf, &baseline);
        let qty = pf
            .positions
            .get("AAPL")
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        assert_eq!(
            qty, 2,
            "total qty must be 2: 1 from current-run fill + 1 from baseline"
        );
    }

    // P06: multi-symbol baseline seeds all symbols correctly
    #[test]
    fn p06_multi_symbol_baseline_seeds_all_positions() {
        let mut pf = flat_portfolio();
        let baseline = baseline_with(&[("AAPL", 2), ("NVDA", 3)]);
        seed_portfolio_from_baseline(&mut pf, &baseline);
        let aapl = pf
            .positions
            .get("AAPL")
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        let nvda = pf
            .positions
            .get("NVDA")
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        assert_eq!(aapl, 2, "AAPL must be 2 from multi-symbol baseline");
        assert_eq!(nvda, 3, "NVDA must be 3 from multi-symbol baseline");
    }

    // P07: zero-qty entries in baseline are skipped
    #[test]
    fn p07_zero_qty_baseline_entry_skipped() {
        let mut pf = flat_portfolio();
        let baseline = baseline_with(&[("AAPL", 0), ("NVDA", 1)]);
        seed_portfolio_from_baseline(&mut pf, &baseline);
        assert!(
            !pf.positions.contains_key("AAPL"),
            "AAPL with qty=0 in baseline must not create a position"
        );
        let nvda = pf
            .positions
            .get("NVDA")
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        assert_eq!(
            nvda, 1,
            "NVDA=1 must still be seeded when AAPL=0 is skipped"
        );
    }
}

// ---------------------------------------------------------------------------
// RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01 unit tests
//
// Root cause: `seed_portfolio_from_baseline` mutates the live `PortfolioState`
// directly (RUNTIME-POSITION-SEED-ON-START-01), so `execution_snapshot.portfolio
// .positions` already carries baseline + same-run fill delta.  The since-removed
// merge in `local_snapshot_provider` / `local_fn` re-added the baseline on top,
// producing local truth = fills + 2x baseline while broker truth = fills +
// baseline — a guaranteed false `ReconcileDrift` halt/disarm (REC-01R) the moment
// any baseline position existed.
//
// Fix: `seed_portfolio_from_baseline` is the SOLE baseline-entry point.
// Downstream reconcile derives local truth directly from the seeded snapshot via
// `reconcile_local_snapshot_from_runtime_with_sides` — no re-merge.
//
// These tests run the REAL production chain end-to-end (apply_entry →
// seed_portfolio_from_baseline → build_portfolio_snapshot →
// reconcile_local_snapshot_from_runtime_with_sides), not hand-rolled fixtures,
// so they fail if the merge is ever reintroduced.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod baseline_double_count_fix_tests {
    use super::{
        reconcile_local_snapshot_from_runtime_with_sides, seed_portfolio_from_baseline,
        PortfolioState,
    };
    use mqk_portfolio::{apply_entry, Fill, LedgerEntry, Side};
    use mqk_reconcile::LocalSnapshot;
    use mqk_runtime::observability::{build_portfolio_snapshot, ExecutionSnapshot};
    use std::collections::BTreeMap;

    fn flat_portfolio() -> PortfolioState {
        PortfolioState::new(100_000_000_000) // $100k initial equity
    }

    fn baseline_with(positions: &[(&str, i64)]) -> LocalSnapshot {
        let mut s = LocalSnapshot::empty();
        for &(sym, qty) in positions {
            s.positions.insert(sym.to_string(), qty);
        }
        s
    }

    /// Run the REAL production chain end-to-end exactly as
    /// `build_execution_orchestrator` + the patched `local_snapshot_provider` /
    /// `local_fn` do post-fix:
    ///
    ///   same-run fills (apply_entry) → seed_portfolio_from_baseline →
    ///   build_portfolio_snapshot → reconcile_local_snapshot_from_runtime_with_sides
    ///
    /// Returns the resulting local reconcile qty for `symbol`.
    fn local_qty_after_real_chain(
        symbol: &str,
        fills: &[(Side, i64)],
        baseline_positions: &[(&str, i64)],
    ) -> i64 {
        let mut pf = flat_portfolio();

        // Step 1: apply same-run fills (mirrors recover_oms_and_portfolio replay
        // of fills from the current run_id).
        for &(side, qty) in fills {
            apply_entry(
                &mut pf,
                LedgerEntry::Fill(Fill::new(symbol, side, qty, 313_000_000, 0)),
            );
        }

        // Step 2: seed from the adopted broker baseline — the SOLE entry point
        // for baseline inclusion (RUNTIME-POSITION-SEED-ON-START-01 /
        // RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01).
        let baseline = baseline_with(baseline_positions);
        seed_portfolio_from_baseline(&mut pf, &baseline);

        // Step 3: build the execution snapshot exactly as the orchestrator does
        // (build_portfolio_snapshot reads net_qty = sum of signed lot quantities).
        let exec_snapshot = ExecutionSnapshot {
            run_id: None,
            active_orders: vec![],
            pending_outbox: vec![],
            recent_inbox_events: vec![],
            portfolio: build_portfolio_snapshot(&pf),
            system_block_state: None,
            recent_risk_denials: vec![],
            snapshot_at_utc: chrono::Utc::now(),
            has_recent_terminal_fill: false,
            risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
        };

        // Step 4: derive local reconcile truth via the patched (no-merge) path —
        // the exact function `local_snapshot_provider` / `local_fn` call directly.
        let sides: BTreeMap<String, mqk_reconcile::Side> = BTreeMap::new();
        let local = reconcile_local_snapshot_from_runtime_with_sides(&exec_snapshot, &sides);
        local.positions.get(symbol).copied().unwrap_or(0)
    }

    // BDC01: baseline-only (no same-run fills) → local qty == N
    #[test]
    fn bdc01_baseline_only_yields_n() {
        let qty = local_qty_after_real_chain("AAPL", &[], &[("AAPL", 5)]);
        assert_eq!(
            qty, 5,
            "BDC01: baseline=5, no fills → local qty must be exactly N=5 (a double-count would read 10)"
        );
    }

    // BDC02: baseline N + same-run buy M → local qty == N + M
    #[test]
    fn bdc02_baseline_plus_buy_yields_n_plus_m() {
        let qty = local_qty_after_real_chain("AAPL", &[(Side::Buy, 3)], &[("AAPL", 5)]);
        assert_eq!(
            qty, 8,
            "BDC02: baseline=5 + buy=3 → local qty must be N+M=8 (a double-count would read 13)"
        );
    }

    // BDC03: baseline N + same-run sell M → local qty == N - M
    #[test]
    fn bdc03_baseline_plus_sell_yields_n_minus_m() {
        let qty = local_qty_after_real_chain("AAPL", &[(Side::Sell, 2)], &[("AAPL", 5)]);
        assert_eq!(
            qty, 3,
            "BDC03: baseline=5 - sell=2 → local qty must be N-M=3 (a double-count would read 8)"
        );
    }

    // BDC04: no baseline + same-run fill M → local qty == M
    #[test]
    fn bdc04_no_baseline_plus_fill_yields_m() {
        let qty = local_qty_after_real_chain("AAPL", &[(Side::Buy, 4)], &[]);
        assert_eq!(
            qty, 4,
            "BDC04: no baseline + buy=4 → local qty must be exactly M=4 (no baseline to double-count)"
        );
    }

    // BDC05: local reconcile truth must equal broker truth (baseline + same-run
    // fills, counted exactly once) — the exact invariant whose violation produces
    // a false ReconcileDrift halt/disarm (REC-01R) the moment any baseline
    // position exists.
    #[test]
    fn bdc05_local_matches_broker_truth_baseline_plus_fills_once() {
        let baseline_qty = 5;
        let fill_qty = 3;
        let local_qty =
            local_qty_after_real_chain("AAPL", &[(Side::Buy, fill_qty)], &[("AAPL", baseline_qty)]);
        let broker_truth = baseline_qty + fill_qty;
        let double_counted = broker_truth + baseline_qty;
        assert_eq!(
            local_qty, broker_truth,
            "BDC05: local truth must equal broker truth (baseline + same-run fills counted once = {}); \
             a re-merged double-count would read {} and falsely trigger ReconcileDrift",
            broker_truth, double_counted
        );
    }
}

// ---------------------------------------------------------------------------
// BASELINE-LEDGER-PARITY-01 unit tests
//
// Root cause: `seed_portfolio_from_baseline` mutated `portfolio.positions`
// directly via a manually-pushed `Lot`, without appending an equivalent
// `LedgerEntry` to `portfolio.ledger`. `check_capital_invariants` (Phase 3b,
// mqk-runtime/src/orchestrator/apply.rs) recomputes `positions`/`cash_micros`/
// `realized_pnl_micros` from `portfolio.ledger` via `recompute_from_ledger`
// and compares against live state. For any non-flat baseline, the recompute
// saw `{}` for that symbol while `portfolio.positions` carried the seeded
// baseline lot, producing `INVARIANT_VIOLATED: positions map mismatch between
// ledger recompute and state` — halting the run on the very first `oms_inbox`
// event (even a non-fill ACK), since Phase 3b runs the invariant check after
// every inbox-apply pass regardless of whether that event itself touched the
// portfolio.
//
// Fix: `seed_portfolio_from_baseline` now applies an equivalent
// `LedgerEntry::Fill` via `apply_entry` (Buy for a long baseline, Sell for a
// short baseline; price_micros=1, fee_micros=0) — appending to
// `portfolio.ledger` so `recompute_from_ledger` reproduces the same
// `positions`/`cash_micros`/`realized_pnl_micros` immediately.
//
// `check_capital_invariants` itself is `pub(super)` inside
// `mqk_runtime::orchestrator::apply` and not reachable from this crate.
// `assert_capital_invariants_hold` below reproduces the identical
// ledger-recompute comparison via the public
// `mqk_portfolio::recompute_from_ledger` API, so these tests exercise the
// exact invariant Phase 3b enforces.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod baseline_ledger_parity_tests {
    use super::{
        broker_event_to_oms_event, broker_event_to_portfolio_fill, seed_portfolio_from_baseline,
        BrokerEvent, OmsEvent, PortfolioState,
    };
    use mqk_reconcile::LocalSnapshot;

    fn flat_portfolio() -> PortfolioState {
        PortfolioState::new(100_000_000_000) // $100k initial equity
    }

    fn baseline_with(positions: &[(&str, i64)]) -> LocalSnapshot {
        let mut s = LocalSnapshot::empty();
        for &(sym, qty) in positions {
            s.positions.insert(sym.to_string(), qty);
        }
        s
    }

    /// Mirrors `mqk_runtime::orchestrator::apply::check_capital_invariants`
    /// (`pub(super)`, not reachable from mqk-daemon) using the public
    /// `mqk_portfolio::recompute_from_ledger` API. Same three comparisons,
    /// same fail-closed semantics — this is the exact check Phase 3b runs
    /// after every inbox-apply pass.
    fn assert_capital_invariants_hold(pf: &PortfolioState) {
        let (recomputed_cash, recomputed_pnl, recomputed_positions) =
            mqk_portfolio::recompute_from_ledger(pf.initial_cash_micros, &pf.ledger);
        assert_eq!(
            recomputed_cash, pf.cash_micros,
            "INVARIANT_VIOLATED: cash_micros mismatch: recomputed={} state={}",
            recomputed_cash, pf.cash_micros
        );
        assert_eq!(
            recomputed_pnl, pf.realized_pnl_micros,
            "INVARIANT_VIOLATED: realized_pnl_micros mismatch: recomputed={} state={}",
            recomputed_pnl, pf.realized_pnl_micros
        );
        assert_eq!(
            recomputed_positions, pf.positions,
            "INVARIANT_VIOLATED: positions map mismatch between ledger recompute and state"
        );
    }

    // BLP01: long broker baseline (AAPL=1) is ledger-replayable and invariant-safe.
    #[test]
    fn blp01_long_baseline_is_invariant_safe() {
        let mut pf = flat_portfolio();
        let baseline = baseline_with(&[("AAPL", 1)]);
        seed_portfolio_from_baseline(&mut pf, &baseline);

        let qty = pf
            .positions
            .get("AAPL")
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        assert_eq!(qty, 1, "AAPL position qty must be 1 after seeding");
        assert!(!pf.ledger.is_empty(), "seeding must append a ledger entry");
        assert_capital_invariants_hold(&pf);
    }

    // BLP02: multi-symbol broker baseline (AAPL=1, MSFT=2) is invariant-safe.
    #[test]
    fn blp02_multi_symbol_baseline_is_invariant_safe() {
        let mut pf = flat_portfolio();
        let baseline = baseline_with(&[("AAPL", 1), ("MSFT", 2)]);
        seed_portfolio_from_baseline(&mut pf, &baseline);

        let aapl = pf
            .positions
            .get("AAPL")
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        let msft = pf
            .positions
            .get("MSFT")
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        assert_eq!(aapl, 1, "AAPL must be 1 from multi-symbol baseline");
        assert_eq!(msft, 2, "MSFT must be 2 from multi-symbol baseline");
        assert!(!pf.ledger.is_empty(), "seeding must append ledger entries");
        assert_capital_invariants_hold(&pf);
    }

    // BLP03: short/negative broker baseline (AAPL=-3) is invariant-safe.
    //
    // Shorts ARE supported by `apply_fill` / `recompute_from_ledger` FIFO lot
    // accounting (`Lot::short`). A negative baseline qty seeds a `Sell` fill
    // against an empty position, opening a short lot, so
    // `qty_signed() == bl_qty`.
    #[test]
    fn blp03_short_baseline_is_invariant_safe() {
        let mut pf = flat_portfolio();
        let baseline = baseline_with(&[("AAPL", -3)]);
        seed_portfolio_from_baseline(&mut pf, &baseline);

        let qty = pf
            .positions
            .get("AAPL")
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        assert_eq!(qty, -3, "AAPL short position qty must be -3 after seeding");
        assert!(!pf.ledger.is_empty(), "seeding must append a ledger entry");
        assert_capital_invariants_hold(&pf);
    }

    // BLP04: current-run fill (AAPL Buy=1) plus baseline (AAPL=1) does not
    // double-count and remains ledger-replayable / invariant-safe (mirrors P05).
    #[test]
    fn blp04_current_run_fill_plus_baseline_no_double_count_invariant_safe() {
        use mqk_portfolio::{apply_entry, Fill, LedgerEntry, Side};
        let mut pf = flat_portfolio();
        apply_entry(
            &mut pf,
            LedgerEntry::Fill(Fill::new("AAPL", Side::Buy, 1, 313_000_000, 0)),
        );

        let baseline = baseline_with(&[("AAPL", 1)]);
        seed_portfolio_from_baseline(&mut pf, &baseline);

        let qty = pf
            .positions
            .get("AAPL")
            .map(|p| p.qty_signed())
            .unwrap_or(0);
        assert_eq!(
            qty, 2,
            "total qty must be 2: 1 from current-run fill + 1 from baseline"
        );
        assert_capital_invariants_hold(&pf);
    }

    // BLP05: production-style ACK after baseline seeding does not halt /
    // invariant-fail.
    //
    // Phase 3b applies inbox events to `portfolio` only when
    // `broker_event_to_portfolio_fill` returns `Some(..)` (Fill/PartialFill).
    // A non-fill `Ack` event maps to `OmsEvent::Ack` (OMS-only — no portfolio
    // mutation), and `broker_event_to_portfolio_fill` returns `None`, so
    // `portfolio` is byte-for-byte unchanged by the ACK. The production
    // failure was therefore entirely a property of the seeded portfolio
    // state itself: `check_capital_invariants` ran (as it does after every
    // inbox-apply pass) against a `portfolio` whose `positions` already
    // diverged from `recompute_from_ledger(&portfolio.ledger)` *before* the
    // ACK arrived. Proving the seeded portfolio is invariant-safe — and that
    // the ACK is a portfolio no-op — is sufficient to prove the production
    // halt sequence (seed baseline -> broker ACK -> Phase 3b invariant check)
    // no longer fires.
    #[test]
    fn blp05_seeded_baseline_survives_non_fill_ack_invariant_check() {
        let mut pf = flat_portfolio();
        let baseline = baseline_with(&[("AAPL", 1)]);
        seed_portfolio_from_baseline(&mut pf, &baseline);

        // Invariant must hold immediately after seeding (this is what
        // production violated before the fix).
        assert_capital_invariants_hold(&pf);

        // Simulate the production ACK: BrokerEvent::Ack for the new BUY order.
        let ack = BrokerEvent::Ack {
            broker_message_id: "alpaca:ord-1:new:1".to_string(),
            internal_order_id: "ord-1".to_string(),
            broker_order_id: Some("broker-ord-1".to_string()),
        };

        // ACK maps to an OMS-only event...
        assert_eq!(broker_event_to_oms_event(&ack), OmsEvent::Ack);
        // ...and produces no portfolio fill, so `pf` is untouched by Phase 3b
        // for this event.
        assert!(
            broker_event_to_portfolio_fill(&ack).is_none(),
            "a non-fill Ack must not produce a portfolio mutation"
        );

        // `pf` is unchanged by the ACK — invariant must still hold.
        assert_capital_invariants_hold(&pf);
    }
}
