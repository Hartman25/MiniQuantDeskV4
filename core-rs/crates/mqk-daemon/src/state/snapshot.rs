//! Pure reconcile/snapshot helper functions for mqk-daemon.
//!
//! Contains: parse_signed_qty, reconcile_side_from_schema,
//! reconcile_order_status_from_schema,
//! reconcile_local_snapshot_from_runtime_with_sides,
//! oms_execution_status_to_reconcile, outbox_json_symbol, outbox_json_qty,
//! outbox_json_side, broker_event_to_oms_event, recover_oms_and_portfolio,
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
use mqk_runtime::orchestrator::effective_portfolio_fill;

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
        // PAPER-SOAK-ALPACA-FILL-AUTHORITY-FINAL-CLOSURE-02 (Defect #2): an
        // APPLIED durable row is authoritative restart/accounting evidence.
        // A row that fails to deserialize must fail the whole replay closed
        // -- silently dropping it (`continue`) would omit real economic
        // evidence from authoritative truth without any record that it was
        // ever skipped.
        let event: BrokerEvent = serde_json::from_value(row.message_json.clone()).map_err(|e| {
            super::types::RuntimeLifecycleError::internal(
                "recover_oms_and_portfolio.malformed_applied_event",
                format!(
                    "run_id={run_id} inbox_id={} broker_message_id={}: applied inbox row failed \
                     to deserialize as BrokerEvent: {e}",
                    row.inbox_id, row.broker_message_id,
                ),
            )
        })?;
        let internal_id = event.internal_order_id().to_string();
        let is_fill = matches!(
            event,
            BrokerEvent::Fill { .. } | BrokerEvent::PartialFill { .. }
        );
        let oms_event = broker_event_to_oms_event(&event);

        let Some(order) = oms_orders.get_mut(&internal_id) else {
            if is_fill {
                // PAPER-SOAK-ALPACA-FILL-AUTHORITY-FINAL-CLOSURE-02 (Defect
                // #2): mirror live apply's Section C invariant
                // (`apply_fill_step` in mqk-runtime::orchestrator::apply) --
                // a fill event must never mutate the portfolio without a
                // proven OMS order context. Durable replay must fail closed
                // here exactly as the live path does, not silently proceed
                // to a portfolio mutation with no OMS authority behind it.
                return Err(super::types::RuntimeLifecycleError::internal(
                    "recover_oms_and_portfolio.unknown_order_fill",
                    format!(
                        "UNKNOWN_ORDER_FILL: run_id={run_id} inbox_id={} broker_message_id={} \
                         internal_order_id={internal_id} - fill event has no OMS order context \
                         in durable outbox truth; refusing portfolio mutation (Section C)",
                        row.inbox_id, row.broker_message_id,
                    ),
                ));
            }
            // Non-fill event for an order absent from this run's submitted
            // outbox (e.g. already pruned) carries no portfolio effect --
            // safe to skip, matching live apply_fill_step's same policy.
            continue;
        };

        let pre_qty = order.filled_qty;
        // Mirror the live apply path: use broker_fill_id as the economic
        // event identity when present, fall back to broker_message_id.
        // This ensures WS (no fill_id, uses msg_id) and REST (fill_id
        // present) versions of the same fill collapse to a single OMS
        // advance via the applied_event_ids set.
        let economic_event_id = event
            .broker_fill_id()
            .map(str::to_string)
            .unwrap_or_else(|| row.broker_message_id.clone());
        // PAPER-SOAK-ALPACA-FILL-ECONOMIC-AUTHORITY-CLOSURE-01: durable
        // replay must use the SAME duplicate-protection semantics as the
        // live apply path (`apply_fill_step` in
        // mqk-runtime::orchestrator::apply). A plain `apply` here only
        // dedups on `economic_event_id`, which differs across transport
        // lanes for the SAME physical execution (WS vs REST) — durable
        // replay of a cross-lane duplicate could advance filled_qty
        // twice for one execution. `apply_with_watermark` additionally
        // treats the event's own `cum_qty_after` (broker-native,
        // lane-independent) as economic identity, so a cross-lane
        // duplicate is recognized as already-applied even under a
        // never-before-seen `economic_event_id`.
        //
        // The Result is propagated instead of discarded: durable replay is
        // authoritative accounting/restart truth and must fail closed on an
        // illegal OMS transition (e.g. a fill after a confirmed
        // cancel/reject) rather than silently dropping it and fabricating a
        // portfolio that never legally existed.
        order
            .apply_with_watermark(&oms_event, Some(&economic_event_id), event.cum_qty_after())
            .map_err(|e| {
                super::types::RuntimeLifecycleError::internal(
                    "recover_oms_and_portfolio.illegal_transition",
                    format!(
                        "run_id={run_id} inbox_id={} broker_message_id={} \
                         internal_order_id={internal_id} event={oms_event:?}: {e}",
                        row.inbox_id, row.broker_message_id,
                    ),
                )
            })?;
        let post_qty = order.filled_qty;

        // PAPER-SOAK-ALPACA-FILL-AUTHORITY-FINAL-CLOSURE-02 (Defect #1 /
        // Section 3): use the SAME canonical effective-fill semantic as live
        // apply (`mqk_runtime::orchestrator::effective_portfolio_fill`),
        // deriving portfolio fill quantity from the OMS's actual economic
        // advance (`post_qty - pre_qty`) rather than the event's raw
        // `delta_qty`. This mirrors live `apply_fill_step` exactly and
        // subsumes the old no-op check: when `post_qty == pre_qty` (a
        // duplicate/no-op transition), `effective_portfolio_fill` returns
        // `None` and no portfolio mutation occurs.
        if is_fill {
            if let Some(fill) = effective_portfolio_fill(&event, pre_qty, post_qty) {
                apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
            }
        }
    }

    oms_orders.retain(|_, o| !o.state.is_terminal());
    sides.retain(|order_id, _| oms_orders.contains_key(order_id));

    Ok((oms_orders, sides, portfolio))
}

// ---------------------------------------------------------------------------
// DURABLE-PAPER-PORTFOLIO-AND-PNL-01C: authoritative snapshot acceptance
// ---------------------------------------------------------------------------

/// Typed outcome of [`persist_external_broker_snapshot_best_effort`].
///
/// DURABLE-PAPER-PORTFOLIO-AND-PNL-01-FINAL-RUN-SCOPING-ACCOUNTING-AND-CLOSURE-REPAIR
/// (Repair C): accounting refresh must only ever run against a snapshot
/// that was itself durably confirmed -- `Confirmed` is the sole variant
/// that carries a `snapshot_id` a caller may pass to the accounting
/// refresh. Every other variant means "no confirmed snapshot id exists for
/// this call" and must never feed the accounting seam.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ExternalSnapshotPersistOutcome {
    /// The snapshot is durably present under `snapshot_id` -- either freshly
    /// inserted or an exact idempotent replay of an existing row.
    Confirmed {
        snapshot_id: uuid::Uuid,
        newly_inserted: bool,
    },
    /// Deployment mode / broker kind is outside the Paper+Alpaca lane this
    /// bundle supports -- a deliberate no-op, not a failure.
    SkippedUnsupported,
    /// No DB pool is configured.
    Unavailable,
    /// The same `snapshot_id` already exists with different content.
    Conflict,
    /// The persistence query itself failed.
    DatabaseFailure,
    /// The snapshot's own fields (equity/cash/position qty/avg price)
    /// could not be parsed into a durable row.
    InvalidSnapshot,
}

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
    let persist_outcome = persist_external_broker_snapshot_best_effort(
        state.db.clone(),
        state.deployment_mode(),
        state.runtime_selection.broker_kind,
        snapshot.clone(),
        run_id,
        operation_id,
    )
    .await;

    // DURABLE-PAPER-PORTFOLIO-AND-PNL-01D (repaired by the closure-repair
    // Confirmed-snapshot gate): every acceptance of a fresh authoritative
    // snapshot is also the natural point to refresh the durable
    // fill-accounting projection -- but only once the snapshot itself is
    // durably confirmed. A best-effort persistence failure/conflict must
    // never leave a stale accounting row claiming completeness against a
    // snapshot that was never actually written.
    if let (Some(run_id), ExternalSnapshotPersistOutcome::Confirmed { snapshot_id, .. }) =
        (run_id, &persist_outcome)
    {
        super::paper_portfolio_accounting::refresh_paper_portfolio_accounting_state_best_effort(
            state.db.clone(),
            state.deployment_mode(),
            state.runtime_selection.broker_kind,
            run_id,
            *snapshot_id,
            snapshot.positions,
            Utc::now(),
        )
        .await;
    }
}

/// Attempts durable persistence of an already-accepted `External`-source
/// broker snapshot. Never panics, never returns an error to the caller —
/// every failure mode is logged, swallowed, and reported through the typed
/// [`ExternalSnapshotPersistOutcome`] return value, since this is additive
/// truth on top of the existing in-memory acceptance, not a gate for it.
/// Callers wanting to trigger the accounting refresh must inspect the
/// returned outcome for `Confirmed` and use its `snapshot_id` — never
/// derive one independently.
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
) -> ExternalSnapshotPersistOutcome {
    if deployment_mode != super::types::DeploymentMode::Paper {
        return ExternalSnapshotPersistOutcome::SkippedUnsupported;
    }
    if broker_kind != Some(super::types::BrokerKind::Alpaca) {
        return ExternalSnapshotPersistOutcome::SkippedUnsupported;
    }
    let Some(pool) = db else {
        tracing::warn!("durable_paper_portfolio_snapshot_skip: no_db_pool_configured");
        return ExternalSnapshotPersistOutcome::Unavailable;
    };

    // A1: an externally authoritative snapshot may be confirmed only when
    // it carries real run ownership -- a run_id-less snapshot can never be
    // resolved by any run-scoped route (durable-summary/positions,
    // paper-lifecycle), so persisting it would create an orphaned row no
    // reader could ever prove authoritative. Fail closed rather than
    // persist an unownable snapshot.
    let Some(run_id) = run_id else {
        tracing::warn!("durable_paper_portfolio_snapshot_skip: missing_run_ownership");
        return ExternalSnapshotPersistOutcome::InvalidSnapshot;
    };

    // A1: currency must be exactly "USD" -- this bundle's accounting
    // (cash/equity/P&L) has no cross-currency conversion; a non-USD
    // snapshot cannot be authoritative Paper+Alpaca truth.
    if snapshot.account.currency != "USD" {
        tracing::warn!(
            currency = %snapshot.account.currency,
            "durable_paper_portfolio_snapshot_skip: unsupported_currency"
        );
        return ExternalSnapshotPersistOutcome::InvalidSnapshot;
    }

    let Some(equity_micros) =
        crate::routes::helpers::parse_decimal_micros(&snapshot.account.equity)
    else {
        tracing::warn!("durable_paper_portfolio_snapshot_skip: account_equity_unparseable");
        return ExternalSnapshotPersistOutcome::InvalidSnapshot;
    };
    let Some(cash_micros) = crate::routes::helpers::parse_decimal_micros(&snapshot.account.cash)
    else {
        tracing::warn!("durable_paper_portfolio_snapshot_skip: account_cash_unparseable");
        return ExternalSnapshotPersistOutcome::InvalidSnapshot;
    };

    let mut positions = Vec::with_capacity(snapshot.positions.len());
    for p in &snapshot.positions {
        // A1: every position symbol must be nonblank after trimming -- a
        // blank symbol can never be authoritative position identity.
        if p.symbol.trim().is_empty() {
            tracing::warn!("durable_paper_portfolio_snapshot_skip: position_symbol_blank");
            return ExternalSnapshotPersistOutcome::InvalidSnapshot;
        }
        let Some(qty_signed) = parse_signed_qty(&p.qty) else {
            tracing::warn!(
                symbol = %p.symbol,
                "durable_paper_portfolio_snapshot_skip: position_qty_unparseable"
            );
            return ExternalSnapshotPersistOutcome::InvalidSnapshot;
        };
        let Some(avg_entry_price_micros) =
            crate::routes::helpers::parse_decimal_micros(&p.avg_price)
        else {
            tracing::warn!(
                symbol = %p.symbol,
                "durable_paper_portfolio_snapshot_skip: position_avg_price_unparseable"
            );
            return ExternalSnapshotPersistOutcome::InvalidSnapshot;
        };
        // A1: every NONZERO position's average entry price must be strictly
        // positive -- a flat (qty=0) position's avg price is not
        // economically meaningful and is not checked, but a non-flat
        // position with a zero or negative avg price is not a valid broker
        // fill history and must be refused rather than persisted.
        if qty_signed != 0 && avg_entry_price_micros <= 0 {
            tracing::warn!(
                symbol = %p.symbol,
                avg_entry_price_micros,
                "durable_paper_portfolio_snapshot_skip: position_avg_price_not_positive"
            );
            return ExternalSnapshotPersistOutcome::InvalidSnapshot;
        }
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
            run_id,
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
            run_id: Some(run_id),
            operation_id,
            positions,
        },
    )
    .await;

    match result {
        Ok(mqk_db::InsertPaperPortfolioSnapshotOutcome::Inserted { .. }) => {
            ExternalSnapshotPersistOutcome::Confirmed {
                snapshot_id,
                newly_inserted: true,
            }
        }
        Ok(mqk_db::InsertPaperPortfolioSnapshotOutcome::AlreadyExists { .. }) => {
            ExternalSnapshotPersistOutcome::Confirmed {
                snapshot_id,
                newly_inserted: false,
            }
        }
        Ok(mqk_db::InsertPaperPortfolioSnapshotOutcome::Conflict { detail }) => {
            tracing::warn!(detail = %detail, "durable_paper_portfolio_snapshot_conflict");
            ExternalSnapshotPersistOutcome::Conflict
        }
        Err(err) => {
            tracing::warn!(error = %err, "durable_paper_portfolio_snapshot_persist_failed");
            ExternalSnapshotPersistOutcome::DatabaseFailure
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
        broker_event_to_oms_event, seed_portfolio_from_baseline, BrokerEvent, OmsEvent,
        PortfolioState,
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
    // Both `recover_oms_and_portfolio` (durable replay) and live
    // `apply_fill_step` only ever mutate the portfolio for `Fill`/
    // `PartialFill` events. A non-fill `Ack` event maps to `OmsEvent::Ack`
    // (OMS-only — no portfolio mutation), so `portfolio` is byte-for-byte
    // unchanged by the ACK. The production failure was therefore entirely a
    // property of the seeded portfolio state itself: `check_capital_invariants`
    // ran (as it does after every inbox-apply pass) against a `portfolio`
    // whose `positions` already diverged from
    // `recompute_from_ledger(&portfolio.ledger)` *before* the ACK arrived.
    // Proving the seeded portfolio is invariant-safe — and that the ACK is a
    // portfolio no-op — is sufficient to prove the production halt sequence
    // (seed baseline -> broker ACK -> Phase 3b invariant check) no longer
    // fires.
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
        // ...and is not a fill-class event, so neither
        // `recover_oms_and_portfolio` nor live apply can ever extract a
        // portfolio fill from it — `pf` is untouched by Phase 3b for this
        // event.
        assert!(
            !matches!(
                ack,
                BrokerEvent::Fill { .. } | BrokerEvent::PartialFill { .. }
            ),
            "a non-fill Ack must not produce a portfolio mutation"
        );

        // `pf` is unchanged by the ACK — invariant must still hold.
        assert_capital_invariants_hold(&pf);
    }
}

// ---------------------------------------------------------------------------
// PAPER-SOAK-ALPACA-FILL-ECONOMIC-AUTHORITY-CLOSURE-01: recover_oms_and_portfolio
// durable-replay watermark + fail-closed-transition proofs.
//
// These exercise the REAL `recover_oms_and_portfolio` function (not a
// reimplementation) against a disposable Postgres DB, using the exact
// oms_outbox/oms_inbox row shapes production writes
// (`mqk_db::outbox_enqueue` + raw SQL SENT transition +
// `mqk_db::inbox_insert_deduped_with_identity` + `mqk_db::inbox_mark_applied`).
//
// NEG-1 / E03 / E04: a single physical partial-fill execution observed on
// both the WS lane (no broker_fill_id, economic identity = broker_message_id)
// and the REST lane (broker_fill_id = the Alpaca activity id) leaves two
// durable raw inbox rows with DIFFERENT broker_message_id / economic
// identity. Before this patch, durable replay called plain `OmsOrder::apply`
// (event_id-only dedup), so the second row was NOT recognized as a
// duplicate and advanced filled_qty a second time — final qty 20 for one
// physical 10-share execution. After this patch, replay uses
// `apply_with_watermark` with the event's own broker-native `cum_qty_after`,
// so the second row's cumulative (10) does not exceed the already-applied
// filled_qty (10) and is a no-op regardless of raw-row order.
//
// NEG-6 / E20: an illegal durable transition (a Fill event applied after a
// confirmed Reject) must now propagate as `Err`, not be silently discarded
// by a `let _ = order.apply(...)`.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod fill_economic_authority_closure_tests {
    use super::{effective_portfolio_fill, recover_oms_and_portfolio, PortfolioState};
    use chrono::{TimeZone, Utc};
    use mqk_execution::{
        oms::state_machine::{OmsEvent, OmsOrder},
        BrokerEvent, Side,
    };
    use uuid::Uuid;

    fn fixed_run_id(seed: &str) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            format!("test.fill-economic-authority-closure.v1|{seed}").as_bytes(),
        )
    }

    async fn fixture_run(pool: &sqlx::PgPool, run_id: Uuid) {
        mqk_db::insert_run(
            pool,
            &mqk_db::NewRun {
                run_id,
                engine_id: "test-fill-economic-authority".to_string(),
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

    /// Enqueues a SENT outbox order for `order_id`/`symbol`/`total_qty` — the
    /// prerequisite `recover_oms_and_portfolio` requires to construct an
    /// `OmsOrder` for a given `internal_order_id`.
    async fn fixture_order(
        pool: &sqlx::PgPool,
        run_id: Uuid,
        order_id: &str,
        symbol: &str,
        total_qty: i64,
        side: &str,
    ) {
        mqk_db::outbox_enqueue(
            pool,
            run_id,
            order_id,
            serde_json::json!({"symbol": symbol, "qty": total_qty, "side": side}),
        )
        .await
        .expect("outbox_enqueue should succeed");
        sqlx::query("update oms_outbox set status = 'SENT' where idempotency_key = $1")
            .bind(order_id)
            .execute(pool)
            .await
            .expect("mark outbox SENT should succeed");
    }

    /// Inserts one already-applied inbox row carrying `event`.
    #[allow(clippy::too_many_arguments)]
    async fn fixture_applied_event(
        pool: &sqlx::PgPool,
        run_id: Uuid,
        broker_message_id: &str,
        broker_fill_id: Option<&str>,
        internal_order_id: &str,
        broker_order_id: &str,
        event_kind: &str,
        event: &BrokerEvent,
        at: chrono::DateTime<Utc>,
    ) {
        let json = serde_json::to_value(event).expect("serialize BrokerEvent");
        mqk_db::inbox_insert_deduped_with_identity(
            pool,
            run_id,
            broker_message_id,
            broker_fill_id,
            internal_order_id,
            broker_order_id,
            event_kind,
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

    fn ws_partial(order_id: &str, delta_qty: i64, price_micros: i64, cum: i64) -> BrokerEvent {
        BrokerEvent::PartialFill {
            broker_message_id: format!("alpaca:{order_id}:partial_fill:ws-{cum}"),
            broker_fill_id: None,
            internal_order_id: order_id.to_string(),
            broker_order_id: Some("broker-order-1".to_string()),
            symbol: "AAPL".to_string(),
            side: Side::Buy,
            delta_qty,
            price_micros,
            fee_micros: 0,
            cum_qty_after: Some(cum),
        }
    }

    fn rest_partial_duplicate_of(
        order_id: &str,
        activity_id: &str,
        delta_qty: i64,
        price_micros: i64,
        cum: i64,
    ) -> BrokerEvent {
        BrokerEvent::PartialFill {
            broker_message_id: format!("alpaca-rest-recovery:{activity_id}"),
            broker_fill_id: Some(activity_id.to_string()),
            internal_order_id: order_id.to_string(),
            broker_order_id: Some("broker-order-1".to_string()),
            symbol: "AAPL".to_string(),
            side: Side::Buy,
            delta_qty,
            price_micros,
            fee_micros: 0,
            cum_qty_after: Some(cum),
        }
    }

    /// NEG-1 / E03: WS partial applied first, REST duplicate of the SAME
    /// physical execution applied second (different broker_message_id AND
    /// different broker_fill_id-derived economic identity). Durable replay
    /// must converge to qty=10, not 20.
    #[tokio::test]
    async fn e03_ws_then_rest_cross_lane_duplicate_replays_to_ten_not_twenty() {
        mqk_db::run_isolated("e03_ws_then_rest_dup", |pool| async move {
            let run_id = fixed_run_id("e03_ws_then_rest_dup");
            let order_id = "order-e03";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 10, "buy").await;

            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
            let ws_ev = ws_partial(order_id, 10, 100_000_000, 10);
            fixture_applied_event(
                &pool,
                run_id,
                ws_ev.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "partial_fill",
                &ws_ev,
                at,
            )
            .await;
            let rest_ev = rest_partial_duplicate_of(order_id, "activity-e03", 10, 100_000_000, 10);
            fixture_applied_event(
                &pool,
                run_id,
                rest_ev.broker_message_id(),
                Some("activity-e03"),
                order_id,
                "broker-order-1",
                "partial_fill",
                &rest_ev,
                at,
            )
            .await;

            let (_, _, portfolio) = recover_oms_and_portfolio(&pool, run_id, 100_000_000_000)
                .await
                .expect("recover_oms_and_portfolio should succeed");
            let qty = portfolio
                .positions
                .get("AAPL")
                .map(|p| p.qty_signed())
                .unwrap_or(0);
            assert_eq!(
                qty, 10,
                "cross-lane WS-then-REST duplicate of one physical 10@$100 execution \
                 must replay to qty=10, not 20"
            );
        })
        .await;
    }

    /// NEG-1 / E04: same physical duplicate, REST row applied FIRST and WS
    /// row SECOND (reversed raw-row order). Final economic result must be
    /// identical to E03 — order of durable observation must not matter.
    #[tokio::test]
    async fn e04_rest_then_ws_cross_lane_duplicate_replays_to_ten_not_twenty() {
        mqk_db::run_isolated("e04_rest_then_ws_dup", |pool| async move {
            let run_id = fixed_run_id("e04_rest_then_ws_dup");
            let order_id = "order-e04";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 10, "buy").await;

            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
            let rest_ev = rest_partial_duplicate_of(order_id, "activity-e04", 10, 100_000_000, 10);
            fixture_applied_event(
                &pool,
                run_id,
                rest_ev.broker_message_id(),
                Some("activity-e04"),
                order_id,
                "broker-order-1",
                "partial_fill",
                &rest_ev,
                at,
            )
            .await;
            let ws_ev = ws_partial(order_id, 10, 100_000_000, 10);
            fixture_applied_event(
                &pool,
                run_id,
                ws_ev.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "partial_fill",
                &ws_ev,
                at,
            )
            .await;

            let (_, _, portfolio) = recover_oms_and_portfolio(&pool, run_id, 100_000_000_000)
                .await
                .expect("recover_oms_and_portfolio should succeed");
            let qty = portfolio
                .positions
                .get("AAPL")
                .map(|p| p.qty_signed())
                .unwrap_or(0);
            assert_eq!(
                qty, 10,
                "cross-lane REST-then-WS duplicate of one physical 10@$100 execution \
                 must replay to qty=10, not 20 — order of raw observation must not matter"
            );
        })
        .await;
    }

    /// E06: two legitimate distinct partials (A=10@$100 cum10, B=10@$101
    /// cum20) followed by a LATE duplicate of A (cum10) arriving after B.
    /// The late duplicate must no-op; final qty must be exactly 20 (A+B),
    /// never 30.
    #[tokio::test]
    async fn e06_late_duplicate_after_newer_partial_does_not_inflate_qty() {
        mqk_db::run_isolated("e06_late_dup_after_newer", |pool| async move {
            let run_id = fixed_run_id("e06_late_dup_after_newer");
            let order_id = "order-e06";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 20, "buy").await;

            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
            let a = ws_partial(order_id, 10, 100_000_000, 10);
            fixture_applied_event(
                &pool,
                run_id,
                a.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "partial_fill",
                &a,
                at,
            )
            .await;
            let b = ws_partial(order_id, 10, 101_000_000, 20);
            fixture_applied_event(
                &pool,
                run_id,
                b.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "partial_fill",
                &b,
                at,
            )
            .await;
            // Late duplicate of A over the REST lane — different economic
            // identity, same cum_qty_after=10 (already reflected).
            let late_dup_a =
                rest_partial_duplicate_of(order_id, "activity-e06-late-a", 10, 100_000_000, 10);
            fixture_applied_event(
                &pool,
                run_id,
                late_dup_a.broker_message_id(),
                Some("activity-e06-late-a"),
                order_id,
                "broker-order-1",
                "partial_fill",
                &late_dup_a,
                at,
            )
            .await;

            let (_, _, portfolio) = recover_oms_and_portfolio(&pool, run_id, 100_000_000_000)
                .await
                .expect("recover_oms_and_portfolio should succeed");
            let qty = portfolio
                .positions
                .get("AAPL")
                .map(|p| p.qty_signed())
                .unwrap_or(0);
            assert_eq!(
                qty, 20,
                "late duplicate of an earlier partial arriving after a newer partial \
                 must no-op; final qty must be A+B=20, never 30"
            );
        })
        .await;
    }

    /// E07: mixed-price sequence — partial 10@$100 then terminal fill
    /// 10@$102. Final qty must be 20 with exact price attribution preserved
    /// per-fill (proven via exact cash_micros, which sums each fill's own
    /// price — a price-collapsing bug would produce a different total).
    #[tokio::test]
    async fn e07_mixed_price_terminal_sequence_preserves_exact_price_attribution() {
        mqk_db::run_isolated("e07_mixed_price", |pool| async move {
            let run_id = fixed_run_id("e07_mixed_price");
            let order_id = "order-e07";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 20, "buy").await;

            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
            let partial = ws_partial(order_id, 10, 100_000_000, 10);
            fixture_applied_event(
                &pool,
                run_id,
                partial.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "partial_fill",
                &partial,
                at,
            )
            .await;
            let terminal = BrokerEvent::Fill {
                broker_message_id: format!("alpaca:{order_id}:fill:term"),
                broker_fill_id: None,
                internal_order_id: order_id.to_string(),
                broker_order_id: Some("broker-order-1".to_string()),
                symbol: "AAPL".to_string(),
                side: Side::Buy,
                delta_qty: 10,
                price_micros: 102_000_000,
                fee_micros: 0,
            };
            fixture_applied_event(
                &pool,
                run_id,
                terminal.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "fill",
                &terminal,
                at,
            )
            .await;

            let (_, _, portfolio) = recover_oms_and_portfolio(&pool, run_id, 100_000_000_000)
                .await
                .expect("recover_oms_and_portfolio should succeed");
            let qty = portfolio
                .positions
                .get("AAPL")
                .map(|p| p.qty_signed())
                .unwrap_or(0);
            assert_eq!(qty, 20, "10@$100 + 10@$102 must total 20 shares exactly");
            let expected_cash = 100_000_000_000 - (10 * 100_000_000 + 10 * 102_000_000);
            assert_eq!(
                portfolio.cash_micros, expected_cash,
                "cash must reflect each fill's OWN price, not a collapsed/averaged price"
            );
        })
        .await;
    }

    /// NEG-6 / E20: an illegal durable transition — a Fill arriving after a
    /// confirmed Reject — must now propagate as `Err`, not be silently
    /// discarded. This is the fail-closed proof for durable/restart replay.
    #[tokio::test]
    async fn neg6_e20_illegal_transition_after_reject_fails_closed() {
        mqk_db::run_isolated("neg6_illegal_after_reject", |pool| async move {
            let run_id = fixed_run_id("neg6_illegal_after_reject");
            let order_id = "order-neg6";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 10, "buy").await;

            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
            let reject = BrokerEvent::Reject {
                broker_message_id: "reject-1".to_string(),
                internal_order_id: order_id.to_string(),
                broker_order_id: Some("broker-order-1".to_string()),
            };
            fixture_applied_event(
                &pool,
                run_id,
                "reject-1",
                None,
                order_id,
                "broker-order-1",
                "reject",
                &reject,
                at,
            )
            .await;
            // Illegal: a Fill arriving on an order the broker already
            // confirmed Rejected.
            let late_fill = BrokerEvent::Fill {
                broker_message_id: "fill-late".to_string(),
                broker_fill_id: None,
                internal_order_id: order_id.to_string(),
                broker_order_id: Some("broker-order-1".to_string()),
                symbol: "AAPL".to_string(),
                side: Side::Buy,
                delta_qty: 10,
                price_micros: 100_000_000,
                fee_micros: 0,
            };
            fixture_applied_event(
                &pool,
                run_id,
                "fill-late",
                None,
                order_id,
                "broker-order-1",
                "fill",
                &late_fill,
                at,
            )
            .await;

            let result = recover_oms_and_portfolio(&pool, run_id, 100_000_000_000).await;
            let err = result.expect_err(
                "durable replay must fail closed on an illegal OMS transition, \
                 not silently swallow it and fabricate a portfolio",
            );
            let msg = format!("{err}");
            assert!(
                msg.contains("run_id="),
                "error must carry run_id for diagnosis: {msg}"
            );
            assert!(
                msg.contains("fill-late"),
                "error must carry the offending broker_message_id: {msg}"
            );
            assert!(
                msg.contains(order_id),
                "error must carry the internal_order_id: {msg}"
            );
        })
        .await;
    }

    // -----------------------------------------------------------------------
    // PAPER-SOAK-ALPACA-FILL-AUTHORITY-FINAL-CLOSURE-02
    //
    // FC-1: terminal effective-delta replay parity (Defect #1).
    // FC-2A/FC-2B: fail-closed on malformed/unknown-order durable evidence
    // (Defect #2).
    // Section 8: direct live-vs-durable parity proof.
    // Section 9 (D01-D06): cross-lane durable parity matrix.
    // Section 10 (Q01-Q06): durable data-quality matrix.
    // -----------------------------------------------------------------------

    fn terminal_fill(
        order_id: &str,
        broker_message_id: &str,
        delta_qty: i64,
        price_micros: i64,
    ) -> BrokerEvent {
        BrokerEvent::Fill {
            broker_message_id: broker_message_id.to_string(),
            broker_fill_id: None,
            internal_order_id: order_id.to_string(),
            broker_order_id: Some("broker-order-1".to_string()),
            symbol: "AAPL".to_string(),
            side: Side::Buy,
            delta_qty,
            price_micros,
            fee_micros: 0,
        }
    }

    fn ledger_fills(portfolio: &PortfolioState) -> Vec<(i64, i64)> {
        portfolio
            .ledger
            .iter()
            .filter_map(|e| match e {
                mqk_portfolio::LedgerEntry::Fill(f) => Some((f.qty, f.price_micros)),
                _ => None,
            })
            .collect()
    }

    /// FC-1: order total=3. Partial 2@$100 (cum=2), then a terminal `Fill`
    /// whose RAW `delta_qty=2` overstates the true 1-share remainder
    /// (Alpaca's observed paper-WS terminal-fill quirk). Before this patch,
    /// durable replay applied `broker_event_to_portfolio_fill(&event)`,
    /// which reads the event's raw `delta_qty` directly -- producing
    /// portfolio qty=4 (2+2) for a 3-share order, diverging from OMS truth
    /// (filled_qty capped at 3) and from live `apply_fill_step`, which uses
    /// the OMS-derived effective delta (1, not 2) for the terminal event.
    /// After this patch, durable replay uses the SAME
    /// `effective_portfolio_fill` semantic live apply uses, so it must
    /// produce qty=3, not 4.
    #[tokio::test]
    async fn fc1_terminal_overfill_effective_delta_caps_at_true_remainder() {
        mqk_db::run_isolated("fc1_terminal_overfill", |pool| async move {
            let run_id = fixed_run_id("fc1_terminal_overfill");
            let order_id = "order-fc1";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 3, "buy").await;

            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
            let partial = ws_partial(order_id, 2, 100_000_000, 2);
            fixture_applied_event(
                &pool,
                run_id,
                partial.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "partial_fill",
                &partial,
                at,
            )
            .await;
            let terminal = terminal_fill(
                order_id,
                &format!("alpaca:{order_id}:fill:term"),
                2,
                102_000_000,
            );
            fixture_applied_event(
                &pool,
                run_id,
                terminal.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "fill",
                &terminal,
                at,
            )
            .await;

            let (oms_orders, _, portfolio) =
                recover_oms_and_portfolio(&pool, run_id, 100_000_000_000)
                    .await
                    .expect("recover_oms_and_portfolio should succeed");

            let qty = portfolio
                .positions
                .get("AAPL")
                .map(|p| p.qty_signed())
                .unwrap_or(0);
            assert_eq!(
                qty, 3,
                "FC-1: 2@$100 + true-remainder 1@$102 must total qty=3, not qty=4 from the \
                 raw terminal delta_qty=2"
            );
            let expected_cash = 100_000_000_000 - (2 * 100_000_000 + 102_000_000);
            assert_eq!(
                portfolio.cash_micros, expected_cash,
                "FC-1: cash must reflect the effective 1-share terminal fill at its OWN \
                 price ($102), not the raw 2-share delta"
            );
            assert_eq!(
                ledger_fills(&portfolio),
                vec![(2, 100_000_000), (1, 102_000_000)],
                "FC-1: ledger fill quantities/prices must be exactly [2@$100, 1@$102]"
            );
            assert!(
                !oms_orders.contains_key(order_id),
                "FC-1: the order reached OMS Filled (terminal) and must be pruned from the \
                 returned map, confirming filled_qty capped at total_qty=3"
            );
        })
        .await;
    }

    /// FC-2A: an APPLIED durable row whose `message_json` does not
    /// deserialize as `BrokerEvent` must fail the whole replay closed, with
    /// the error carrying run_id/inbox_id/broker_message_id and
    /// deserialization context -- never silently `continue`.
    #[tokio::test]
    async fn fc2a_malformed_applied_row_fails_closed() {
        mqk_db::run_isolated("fc2a_malformed_applied_row", |pool| async move {
            let run_id = fixed_run_id("fc2a_malformed_applied_row");
            let order_id = "order-fc2a";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 10, "buy").await;

            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
            mqk_db::inbox_insert_deduped_with_identity(
                &pool,
                run_id,
                "fc2a-malformed-msg",
                None,
                order_id,
                "broker-order-1",
                "fill",
                &serde_json::json!({"not_a_broker_event": true}),
                0,
                at,
            )
            .await
            .expect("insert malformed row");
            mqk_db::inbox_mark_applied(&pool, run_id, "fc2a-malformed-msg", at)
                .await
                .expect("mark applied");

            let result = recover_oms_and_portfolio(&pool, run_id, 100_000_000_000).await;
            let err = result.expect_err(
                "FC-2A: a malformed applied durable row must fail the whole replay closed",
            );
            let msg = format!("{err}");
            assert!(msg.contains("run_id="), "error must carry run_id: {msg}");
            assert!(
                msg.contains("inbox_id="),
                "error must carry inbox_id: {msg}"
            );
            assert!(
                msg.contains("fc2a-malformed-msg"),
                "error must carry the offending broker_message_id: {msg}"
            );
            assert!(
                msg.to_lowercase().contains("deserial"),
                "error must carry deserialization error context: {msg}"
            );
        })
        .await;
    }

    /// FC-2B: an APPLIED fill event whose `internal_order_id` has no
    /// corresponding submitted/outbox OMS order must fail the whole replay
    /// closed as `UNKNOWN_ORDER_FILL` -- mirroring live `apply_fill_step`'s
    /// Section C invariant. Since the function returns `Err`, no partial
    /// portfolio is ever returned to the caller: the portfolio cannot be
    /// mutated by an economically-unauthorized fill.
    #[tokio::test]
    async fn fc2b_unknown_order_fill_fails_closed() {
        mqk_db::run_isolated("fc2b_unknown_order_fill", |pool| async move {
            let run_id = fixed_run_id("fc2b_unknown_order_fill");
            // Deliberately no fixture_order() call: no outbox/OMS context
            // exists for "order-fc2b-unknown".
            fixture_run(&pool, run_id).await;

            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
            let fill = terminal_fill("order-fc2b-unknown", "fc2b-fill-1", 10, 100_000_000);
            fixture_applied_event(
                &pool,
                run_id,
                "fc2b-fill-1",
                None,
                "order-fc2b-unknown",
                "broker-order-1",
                "fill",
                &fill,
                at,
            )
            .await;

            let result = recover_oms_and_portfolio(&pool, run_id, 100_000_000_000).await;
            let err = result.expect_err(
                "FC-2B: a fill referencing an order absent from outbox truth must fail closed, \
                 not mutate the portfolio",
            );
            let msg = format!("{err}");
            assert!(
                msg.contains("UNKNOWN_ORDER_FILL"),
                "error must be UNKNOWN_ORDER_FILL: {msg}"
            );
            assert!(
                msg.contains("order-fc2b-unknown"),
                "error must carry the internal_order_id: {msg}"
            );
        })
        .await;
    }

    /// Section 8: direct live-vs-durable parity proof. Both sides apply the
    /// exact SAME `BrokerEvent` sequence (partial 2@$100 then terminal
    /// raw-delta=2@$102 on a 3-share order) and must produce identical
    /// results: OMS final filled=3, portfolio qty=3, ledger fills [2, 1] at
    /// [$100, $102].
    ///
    /// The "live" side exercises the REAL shared building blocks live apply
    /// uses -- `OmsOrder::apply_with_watermark` (the exact OMS transition
    /// function) and `mqk_runtime::orchestrator::effective_portfolio_fill`
    /// (the exact, narrowly-exported pure fill-quantity helper
    /// `apply_fill_step` itself calls) -- rather than a hand-coded
    /// imitation of live behavior. `apply_fill_step` itself is
    /// `pub(super)`-scoped to `mqk_runtime::orchestrator` and not reachable
    /// across the crate boundary; the two primitives it is built from are
    /// exercised directly and unmodified here.
    #[tokio::test]
    async fn section8_live_vs_durable_terminal_overfill_parity() {
        mqk_db::run_isolated("section8_live_durable_parity", |pool| async move {
            let run_id = fixed_run_id("section8_live_durable_parity");
            let order_id = "order-parity";

            let partial = ws_partial(order_id, 2, 100_000_000, 2);
            let terminal = terminal_fill(
                order_id,
                &format!("alpaca:{order_id}:fill:term"),
                2,
                102_000_000,
            );

            // ---- Side A: live apply semantics.
            let mut live_order = OmsOrder::new(order_id, "AAPL", 3);
            let mut live_fills: Vec<(i64, i64)> = Vec::new();

            let pre1 = live_order.filled_qty;
            live_order
                .apply_with_watermark(
                    &OmsEvent::PartialFill { delta_qty: 2 },
                    Some("live-partial"),
                    Some(2),
                )
                .expect("live partial apply must succeed");
            if let Some(fill) = effective_portfolio_fill(&partial, pre1, live_order.filled_qty) {
                live_fills.push((fill.qty, fill.price_micros));
            }

            let pre2 = live_order.filled_qty;
            live_order
                .apply_with_watermark(
                    &OmsEvent::Fill { delta_qty: 2 },
                    Some("live-terminal"),
                    None,
                )
                .expect("live terminal apply must succeed");
            if let Some(fill) = effective_portfolio_fill(&terminal, pre2, live_order.filled_qty) {
                live_fills.push((fill.qty, fill.price_micros));
            }

            assert_eq!(
                live_order.filled_qty, 3,
                "Section 8 LIVE: OMS final filled_qty must be 3"
            );
            assert_eq!(
                live_fills,
                vec![(2, 100_000_000), (1, 102_000_000)],
                "Section 8 LIVE: effective fills must be [2@$100, 1@$102]"
            );

            // ---- Side B: durable replay through the real
            // `recover_oms_and_portfolio` (the actual production function,
            // not a reimplementation).
            let run_id2 = run_id;
            fixture_run(&pool, run_id2).await;
            fixture_order(&pool, run_id2, order_id, "AAPL", 3, "buy").await;
            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
            fixture_applied_event(
                &pool,
                run_id2,
                partial.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "partial_fill",
                &partial,
                at,
            )
            .await;
            fixture_applied_event(
                &pool,
                run_id2,
                terminal.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "fill",
                &terminal,
                at,
            )
            .await;

            let (_, _, portfolio) = recover_oms_and_portfolio(&pool, run_id2, 0)
                .await
                .expect("durable replay should succeed");
            let durable_qty = portfolio
                .positions
                .get("AAPL")
                .map(|p| p.qty_signed())
                .unwrap_or(0);
            let durable_fills = ledger_fills(&portfolio);

            assert_eq!(
                durable_qty, 3,
                "Section 8 DURABLE: portfolio qty must be 3, matching LIVE"
            );
            assert_eq!(
                durable_fills, live_fills,
                "Section 8: durable fill sequence must exactly match the live fill sequence"
            );
        })
        .await;
    }

    // -----------------------------------------------------------------------
    // Section 9: cross-lane durable parity matrix (D01-D06).
    //
    // Every case asserts OMS economic progress (via final portfolio qty),
    // portfolio quantity, cash, AND ledger fill quantities/prices -- never
    // final quantity alone.
    // -----------------------------------------------------------------------

    /// D01: WS partial (cum=10) then a REST duplicate of the SAME physical
    /// execution (different broker_message_id/broker_fill_id, same cum=10).
    #[tokio::test]
    async fn d01_ws_partial_then_rest_duplicate() {
        mqk_db::run_isolated("d01_ws_then_rest_dup", |pool| async move {
            let run_id = fixed_run_id("d01_ws_then_rest_dup");
            let order_id = "order-d01";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 10, "buy").await;
            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();

            let ws = ws_partial(order_id, 10, 100_000_000, 10);
            fixture_applied_event(
                &pool,
                run_id,
                ws.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "partial_fill",
                &ws,
                at,
            )
            .await;
            let rest = rest_partial_duplicate_of(order_id, "d01-activity", 10, 100_000_000, 10);
            fixture_applied_event(
                &pool,
                run_id,
                rest.broker_message_id(),
                Some("d01-activity"),
                order_id,
                "broker-order-1",
                "partial_fill",
                &rest,
                at,
            )
            .await;

            let (_, _, pf) = recover_oms_and_portfolio(&pool, run_id, 0)
                .await
                .expect("D01 replay");
            assert_eq!(
                pf.positions
                    .get("AAPL")
                    .map(|p| p.qty_signed())
                    .unwrap_or(0),
                10,
                "D01: qty must be 10"
            );
            assert_eq!(
                pf.cash_micros,
                -(10 * 100_000_000),
                "D01: cash must reflect ONE 10-share fill"
            );
            assert_eq!(
                ledger_fills(&pf),
                vec![(10, 100_000_000)],
                "D01: ledger must have exactly one fill entry"
            );
        })
        .await;
    }

    /// D02: REST partial applied FIRST, WS duplicate of the SAME execution
    /// SECOND (reversed lane order vs D01) -- result must be identical.
    #[tokio::test]
    async fn d02_rest_partial_then_ws_duplicate() {
        mqk_db::run_isolated("d02_rest_then_ws_dup", |pool| async move {
            let run_id = fixed_run_id("d02_rest_then_ws_dup");
            let order_id = "order-d02";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 10, "buy").await;
            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();

            let rest = rest_partial_duplicate_of(order_id, "d02-activity", 10, 100_000_000, 10);
            fixture_applied_event(
                &pool,
                run_id,
                rest.broker_message_id(),
                Some("d02-activity"),
                order_id,
                "broker-order-1",
                "partial_fill",
                &rest,
                at,
            )
            .await;
            let ws = ws_partial(order_id, 10, 100_000_000, 10);
            fixture_applied_event(
                &pool,
                run_id,
                ws.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "partial_fill",
                &ws,
                at,
            )
            .await;

            let (_, _, pf) = recover_oms_and_portfolio(&pool, run_id, 0)
                .await
                .expect("D02 replay");
            assert_eq!(
                pf.positions
                    .get("AAPL")
                    .map(|p| p.qty_signed())
                    .unwrap_or(0),
                10,
                "D02: qty must be 10"
            );
            assert_eq!(
                pf.cash_micros,
                -(10 * 100_000_000),
                "D02: cash must reflect ONE 10-share fill"
            );
            assert_eq!(
                ledger_fills(&pf),
                vec![(10, 100_000_000)],
                "D02: ledger must have exactly one fill entry"
            );
        })
        .await;
    }

    /// D03: partial A (cum=10), partial B (cum=20), then a LATE duplicate of
    /// A (cum=10) arriving after B -- must no-op; final must be A+B=20.
    #[tokio::test]
    async fn d03_late_duplicate_after_newer_partial() {
        mqk_db::run_isolated("d03_late_dup_after_newer", |pool| async move {
            let run_id = fixed_run_id("d03_late_dup_after_newer");
            let order_id = "order-d03";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 20, "buy").await;
            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();

            let a = ws_partial(order_id, 10, 100_000_000, 10);
            fixture_applied_event(
                &pool,
                run_id,
                a.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "partial_fill",
                &a,
                at,
            )
            .await;
            let b = ws_partial(order_id, 10, 101_000_000, 20);
            fixture_applied_event(
                &pool,
                run_id,
                b.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "partial_fill",
                &b,
                at,
            )
            .await;
            let late_a = rest_partial_duplicate_of(order_id, "d03-late-a", 10, 100_000_000, 10);
            fixture_applied_event(
                &pool,
                run_id,
                late_a.broker_message_id(),
                Some("d03-late-a"),
                order_id,
                "broker-order-1",
                "partial_fill",
                &late_a,
                at,
            )
            .await;

            let (_, _, pf) = recover_oms_and_portfolio(&pool, run_id, 0)
                .await
                .expect("D03 replay");
            assert_eq!(
                pf.positions
                    .get("AAPL")
                    .map(|p| p.qty_signed())
                    .unwrap_or(0),
                20,
                "D03: qty must be A+B=20, never 30"
            );
            assert_eq!(
                pf.cash_micros,
                -(10 * 100_000_000 + 10 * 101_000_000),
                "D03: cash must reflect exactly A+B, not the late duplicate"
            );
            assert_eq!(
                ledger_fills(&pf),
                vec![(10, 100_000_000), (10, 101_000_000)],
                "D03: ledger must have exactly the two legitimate fills"
            );
        })
        .await;
    }

    /// D04: two legitimate SAME-size/SAME-price partials (10@$100 cum10,
    /// 10@$100 cum20) must remain DISTINCT economic fills -- not collapsed
    /// as if they were a duplicate of each other.
    #[tokio::test]
    async fn d04_same_size_same_price_partials_remain_distinct() {
        mqk_db::run_isolated("d04_same_size_same_price", |pool| async move {
            let run_id = fixed_run_id("d04_same_size_same_price");
            let order_id = "order-d04";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 20, "buy").await;
            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();

            let a = ws_partial(order_id, 10, 100_000_000, 10);
            fixture_applied_event(
                &pool,
                run_id,
                a.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "partial_fill",
                &a,
                at,
            )
            .await;
            let b = ws_partial(order_id, 10, 100_000_000, 20);
            fixture_applied_event(
                &pool,
                run_id,
                b.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "partial_fill",
                &b,
                at,
            )
            .await;

            let (_, _, pf) = recover_oms_and_portfolio(&pool, run_id, 0)
                .await
                .expect("D04 replay");
            assert_eq!(
                pf.positions
                    .get("AAPL")
                    .map(|p| p.qty_signed())
                    .unwrap_or(0),
                20,
                "D04: two distinct same-size/same-price partials must both apply: qty=20"
            );
            assert_eq!(
                pf.cash_micros,
                -(20 * 100_000_000),
                "D04: cash must reflect BOTH fills"
            );
            assert_eq!(
                ledger_fills(&pf),
                vec![(10, 100_000_000), (10, 100_000_000)],
                "D04: ledger must have TWO separate fill entries, not one"
            );
        })
        .await;
    }

    /// D05: duplicate TERMINAL fill across lanes (WS terminal then REST
    /// duplicate of the same terminal execution) -- OMS's pre-existing
    /// already-terminal idempotency must apply the fill exactly once.
    #[tokio::test]
    async fn d05_duplicate_terminal_fill_across_lanes() {
        mqk_db::run_isolated("d05_dup_terminal_cross_lane", |pool| async move {
            let run_id = fixed_run_id("d05_dup_terminal_cross_lane");
            let order_id = "order-d05";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 10, "buy").await;
            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();

            let ws_term = terminal_fill(
                order_id,
                &format!("alpaca:{order_id}:fill:ws-term"),
                10,
                100_000_000,
            );
            fixture_applied_event(
                &pool,
                run_id,
                ws_term.broker_message_id(),
                None,
                order_id,
                "broker-order-1",
                "fill",
                &ws_term,
                at,
            )
            .await;

            let rest_term = BrokerEvent::Fill {
                broker_message_id: "alpaca-rest-recovery:d05-activity".to_string(),
                broker_fill_id: Some("d05-activity".to_string()),
                internal_order_id: order_id.to_string(),
                broker_order_id: Some("broker-order-1".to_string()),
                symbol: "AAPL".to_string(),
                side: Side::Buy,
                delta_qty: 10,
                price_micros: 100_000_000,
                fee_micros: 0,
            };
            fixture_applied_event(
                &pool,
                run_id,
                rest_term.broker_message_id(),
                Some("d05-activity"),
                order_id,
                "broker-order-1",
                "fill",
                &rest_term,
                at,
            )
            .await;

            let (_, _, pf) = recover_oms_and_portfolio(&pool, run_id, 0)
                .await
                .expect("D05 replay");
            assert_eq!(
                pf.positions
                    .get("AAPL")
                    .map(|p| p.qty_signed())
                    .unwrap_or(0),
                10,
                "D05: duplicate terminal fill across lanes must apply exactly once: qty=10"
            );
            assert_eq!(
                pf.cash_micros,
                -(10 * 100_000_000),
                "D05: cash must reflect exactly ONE terminal fill"
            );
            assert_eq!(
                ledger_fills(&pf),
                vec![(10, 100_000_000)],
                "D05: ledger must have exactly one fill entry"
            );
        })
        .await;
    }

    /// D06: partial (2@$100 cum2) + Alpaca-style oversized/cumulative
    /// terminal Fill (raw delta_qty=5, order total=3) -- the effective
    /// delta must cap at the true remainder (1), never apply the raw
    /// overfill.
    #[tokio::test]
    async fn d06_partial_plus_oversized_cumulative_terminal_fill() {
        mqk_db::run_isolated("d06_partial_plus_oversized_terminal", |pool| async move {
            let run_id = fixed_run_id("d06_partial_plus_oversized_terminal");
            let order_id = "order-d06";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 3, "buy").await;
            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();

            let partial = ws_partial(order_id, 2, 100_000_000, 2);
            fixture_applied_event(&pool, run_id, partial.broker_message_id(), None, order_id, "broker-order-1", "partial_fill", &partial, at).await;
            // Deliberately oversized raw delta_qty (5) -- far beyond the
            // true 1-share remainder -- to prove the cap is not merely
            // "off by one" tolerant but genuinely bounded by OMS truth.
            let terminal = terminal_fill(order_id, &format!("alpaca:{order_id}:fill:oversized"), 5, 102_000_000);
            fixture_applied_event(&pool, run_id, terminal.broker_message_id(), None, order_id, "broker-order-1", "fill", &terminal, at).await;

            let (_, _, pf) = recover_oms_and_portfolio(&pool, run_id, 0).await.expect("D06 replay");
            assert_eq!(pf.positions.get("AAPL").map(|p| p.qty_signed()).unwrap_or(0), 3, "D06: oversized terminal delta_qty=5 must still cap at true remainder; qty=3, not 7");
            assert_eq!(pf.cash_micros, -(2 * 100_000_000 + 102_000_000), "D06: cash must reflect only the effective 1-share terminal fill");
            assert_eq!(ledger_fills(&pf), vec![(2, 100_000_000), (1, 102_000_000)], "D06: ledger must show the effective 1-share terminal fill, not the raw 5");
        })
        .await;
    }

    // -----------------------------------------------------------------------
    // Section 10: durable data-quality matrix (Q01-Q06).
    //
    // Q01/Q02 are FC-2A/FC-2B above (malformed applied JSON / unknown-order
    // fill). Q03-Q06 are new.
    // -----------------------------------------------------------------------

    /// Q03: an inbound `CancelAck` durable event, replayed with no prior
    /// `CancelPending` state, must fail closed rather than be silently
    /// accepted or skipped.
    ///
    /// `OmsEvent::CancelRequest` (the transition INTO `CancelPending`) is a
    /// purely local/outbound event never carried by any inbound
    /// `BrokerEvent` -- it is never present in the applied inbox durable
    /// evidence `recover_oms_and_portfolio` reconstructs OMS state from.
    /// `(CancelPending, CancelAck) => Cancelled` is therefore the only legal
    /// path to a confirmed cancel in the OMS state machine, and pure
    /// inbound-only replay cannot reconstruct that intermediate state. This
    /// is a structural property of inbound-only replay (not a
    /// fill-economic-authority defect this patch's four defects cover), but
    /// it is exercised here to prove the SAME fail-closed propagation this
    /// patch's Defect #2 repair added (the `illegal_transition` `Err`, not a
    /// silently discarded `let _ =`) makes it safe: a `CancelAck` durable
    /// replay never silently succeeds or fabricates a `Cancelled` order out
    /// of an unproven transition.
    #[tokio::test]
    async fn q03_cancel_ack_without_reconstructed_cancel_pending_fails_closed() {
        mqk_db::run_isolated("q03_cancel_ack_no_pending", |pool| async move {
            let run_id = fixed_run_id("q03_cancel_ack_no_pending");
            let order_id = "order-q03";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 10, "buy").await;
            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();

            let cancel_ack = BrokerEvent::CancelAck {
                broker_message_id: "q03-cancel-ack".to_string(),
                internal_order_id: order_id.to_string(),
                broker_order_id: Some("broker-order-1".to_string()),
            };
            fixture_applied_event(
                &pool,
                run_id,
                "q03-cancel-ack",
                None,
                order_id,
                "broker-order-1",
                "cancel_ack",
                &cancel_ack,
                at,
            )
            .await;

            let err = recover_oms_and_portfolio(&pool, run_id, 0)
                .await
                .expect_err(
                    "Q03: a CancelAck replayed with no reconstructed CancelPending must fail \
                     closed, not be silently accepted",
                );
            let msg = format!("{err}");
            assert!(
                msg.contains("q03-cancel-ack"),
                "error must identify the offending event: {msg}"
            );
        })
        .await;
    }

    /// Q04: a Fill arriving after a confirmed Reject must fail closed
    /// (duplicate coverage of NEG-6/E20 above under the Q-series name).
    #[tokio::test]
    async fn q04_fill_after_confirmed_reject_fails_closed() {
        mqk_db::run_isolated("q04_fill_after_reject", |pool| async move {
            let run_id = fixed_run_id("q04_fill_after_reject");
            let order_id = "order-q04";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 10, "buy").await;
            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();

            let reject = BrokerEvent::Reject {
                broker_message_id: "q04-reject".to_string(),
                internal_order_id: order_id.to_string(),
                broker_order_id: Some("broker-order-1".to_string()),
            };
            fixture_applied_event(
                &pool,
                run_id,
                "q04-reject",
                None,
                order_id,
                "broker-order-1",
                "reject",
                &reject,
                at,
            )
            .await;

            let late_fill = terminal_fill(order_id, "q04-late-fill", 10, 100_000_000);
            fixture_applied_event(
                &pool,
                run_id,
                "q04-late-fill",
                None,
                order_id,
                "broker-order-1",
                "fill",
                &late_fill,
                at,
            )
            .await;

            let err = recover_oms_and_portfolio(&pool, run_id, 0)
                .await
                .expect_err("Q04: a fill after a confirmed Reject must fail closed");
            let msg = format!("{err}");
            assert!(
                msg.contains("q04-late-fill"),
                "error must identify the offending fill: {msg}"
            );
        })
        .await;
    }

    /// Q05: a malformed NON-fill applied event (Ack payload that fails to
    /// deserialize) must ALSO fail the whole replay closed -- the
    /// deserialize step in `recover_oms_and_portfolio` runs before the
    /// event is classified as fill/non-fill, so malformed evidence can
    /// never be silently skipped regardless of its intended kind.
    #[tokio::test]
    async fn q05_malformed_non_fill_event_fails_closed() {
        mqk_db::run_isolated("q05_malformed_non_fill", |pool| async move {
            let run_id = fixed_run_id("q05_malformed_non_fill");
            let order_id = "order-q05";
            fixture_run(&pool, run_id).await;
            fixture_order(&pool, run_id, order_id, "AAPL", 10, "buy").await;
            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();

            mqk_db::inbox_insert_deduped_with_identity(
                &pool,
                run_id,
                "q05-malformed-ack",
                None,
                order_id,
                "broker-order-1",
                "ack",
                &serde_json::json!({"type": "Ack", "broker_message_id": "q05-malformed-ack"}),
                0,
                at,
            )
            .await
            .expect("insert malformed ack row (missing required fields)");
            mqk_db::inbox_mark_applied(&pool, run_id, "q05-malformed-ack", at)
                .await
                .expect("mark applied");

            let err = recover_oms_and_portfolio(&pool, run_id, 0)
                .await
                .expect_err("Q05: a malformed non-fill applied event must ALSO fail closed, not be silently skipped");
            let msg = format!("{err}");
            assert!(msg.contains("q05-malformed-ack"), "error must identify the offending row: {msg}");
        })
        .await;
    }

    /// Q06: an outbox row exists for the referenced order but cannot supply
    /// a valid symbol/positive quantity (malformed `order_json`) -- the
    /// order is never constructed into `oms_orders`, so an applied fill
    /// referencing it must fail closed as `UNKNOWN_ORDER_FILL` exactly like
    /// FC-2B (no outbox row at all), never reconstruct a portfolio without
    /// proven OMS context.
    #[tokio::test]
    async fn q06_outbox_row_missing_valid_symbol_qty_fails_closed() {
        mqk_db::run_isolated("q06_outbox_invalid_order_json", |pool| async move {
            let run_id = fixed_run_id("q06_outbox_invalid_order_json");
            let order_id = "order-q06";
            fixture_run(&pool, run_id).await;
            // Seed an outbox row that is SENT but whose order_json carries
            // no "symbol"/"qty" -- `outbox_json_symbol`/`outbox_json_qty`
            // both return `None`, so the submitted-orders loop in
            // `recover_oms_and_portfolio` skips constructing an `OmsOrder`
            // for it (matching production behavior for a legacy/malformed
            // outbox row).
            mqk_db::outbox_enqueue(
                &pool,
                run_id,
                order_id,
                serde_json::json!({"source": "q06-no-symbol-or-qty"}),
            )
            .await
            .expect("outbox_enqueue should succeed");
            sqlx::query("update oms_outbox set status = 'SENT' where idempotency_key = $1")
                .bind(order_id)
                .execute(&pool)
                .await
                .expect("mark outbox SENT should succeed");

            let at = Utc.with_ymd_and_hms(2099, 4, 1, 13, 0, 0).unwrap();
            let fill = terminal_fill(order_id, "q06-fill", 10, 100_000_000);
            fixture_applied_event(&pool, run_id, "q06-fill", None, order_id, "broker-order-1", "fill", &fill, at).await;

            let err = recover_oms_and_portfolio(&pool, run_id, 0)
                .await
                .expect_err("Q06: a fill referencing an order with no valid symbol/qty in outbox truth must fail closed");
            let msg = format!("{err}");
            assert!(msg.contains("UNKNOWN_ORDER_FILL"), "error must be UNKNOWN_ORDER_FILL: {msg}");
        })
        .await;
    }
}
