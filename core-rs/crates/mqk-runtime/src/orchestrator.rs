//! ExecutionOrchestrator — the single authoritative execution path.
//!
//! # Invariant
//!
//! `tick` is the ONLY code path that calls `BrokerGateway::submit`.
//! No other module may submit to the broker.  Enforcement is structural:
//! `BrokerAdapter` methods require a `&BrokerInvokeToken` that only
//! `BrokerGateway` can manufacture; `BrokerGateway` is only called here.
//!
//! # tick sequence
//!
//! 1.  Claim PENDING outbox rows (DB, `FOR UPDATE SKIP LOCKED`).
//! 2.  Construct `OutboxClaimToken` (unforgeable; from claimed row IDs).
//! 3.  Submit via `BrokerGateway` (gates enforced inside gateway).
//! 4.  Persist SENT + broker order ID mapping; register in-memory OMS order.
//! 5.  Fetch broker events via `BrokerGateway::fetch_events`.
//! 6.  Persist each event to `oms_inbox` (dedup on `broker_message_id`).
//! 7.  Load all unapplied inbox rows for the run.
//! 8.  For each unapplied row: apply OMS transition, apply portfolio change,
//!     assert capital invariants, mark applied.

use std::collections::BTreeMap;

use anyhow::anyhow;
use sqlx::PgPool;
use uuid::Uuid;

use mqk_execution::oms::state_machine::{OmsEvent, OmsOrder};
use mqk_execution::{
    BrokerAdapter, BrokerEvent, BrokerGateway, BrokerOrderMap, BrokerSubmitRequest, IntegrityGate,
    OutboxClaimToken, ReconcileGate, RiskGate,
};
use mqk_portfolio::{apply_entry, recompute_from_ledger, Fill, LedgerEntry, PortfolioState};

// ---------------------------------------------------------------------------
// ExecutionOrchestrator
// ---------------------------------------------------------------------------

/// The single authoritative execution runtime.
///
/// # Construction
///
/// The caller is responsible for pre-loading state from the DB before
/// constructing the orchestrator:
/// - `order_map`: load with `mqk_db::broker_map_load` and register pairs.
/// - `oms_orders`: rebuild from open outbox rows if needed.
/// - `portfolio`: load from persisted snapshot or start fresh.
///
/// # Usage
///
/// ```text
/// loop {
///     orchestrator.tick().await?;
///     // sleep for polling interval
/// }
/// ```
pub struct ExecutionOrchestrator<B, IG, RG, RecG>
where
    B: BrokerAdapter,
    IG: IntegrityGate,
    RG: RiskGate,
    RecG: ReconcileGate,
{
    pool: PgPool,
    gateway: BrokerGateway<B, IG, RG, RecG>,
    order_map: BrokerOrderMap,
    oms_orders: BTreeMap<String, OmsOrder>,
    portfolio: PortfolioState,
    run_id: Uuid,
    dispatcher_id: String,
}

impl<B, IG, RG, RecG> ExecutionOrchestrator<B, IG, RG, RecG>
where
    B: BrokerAdapter,
    IG: IntegrityGate,
    RG: RiskGate,
    RecG: ReconcileGate,
{
    /// Construct the orchestrator.
    ///
    /// All in-memory state must be pre-populated by the caller before
    /// the first `tick` call.
    pub fn new(
        pool: PgPool,
        gateway: BrokerGateway<B, IG, RG, RecG>,
        order_map: BrokerOrderMap,
        oms_orders: BTreeMap<String, OmsOrder>,
        portfolio: PortfolioState,
        run_id: Uuid,
        dispatcher_id: impl Into<String>,
    ) -> Self {
        Self {
            pool,
            gateway,
            order_map,
            oms_orders,
            portfolio,
            run_id,
            dispatcher_id: dispatcher_id.into(),
        }
    }

    /// Execute one orchestrator tick.
    ///
    /// Phases:
    /// 1. Submit pending outbox rows via the gateway.
    /// 2. Fetch + ingest new broker events into oms_inbox.
    /// 3. Apply all unapplied inbox rows: OMS transition → portfolio apply →
    ///    capital invariant check → mark applied.
    ///
    /// Returns `Err` on any DB failure, gate refusal, OMS illegal transition,
    /// or invariant violation.  The caller must treat any `Err` as a halt
    /// signal and stop the tick loop.
    pub async fn tick(&mut self) -> anyhow::Result<()> {
        // ------------------------------------------------------------------
        // Phase 1: Claim and submit outbox rows.
        // ------------------------------------------------------------------
        let claimed = mqk_db::outbox_claim_batch(&self.pool, 1, &self.dispatcher_id).await?;

        for outbox_row in claimed {
            let order_id = outbox_row.idempotency_key.clone();

            // Step 2: construct unforgeable claim token from the claimed row.
            let claim = OutboxClaimToken::from_claimed_row(outbox_row.outbox_id, &order_id);

            // Build a submit request from the outbox order_json.
            let req = build_submit_request(&outbox_row)?;
            let symbol = order_json_symbol(&outbox_row.order_json);
            let qty = order_json_qty(&outbox_row.order_json);

            // Step 3: submit via BrokerGateway — the ONLY submit path.
            let resp = match self.gateway.submit(&claim, req) {
                Ok(r) => r,
                Err(e) => {
                    mqk_db::outbox_release_claim(&self.pool, &order_id).await?;
                    return Err(anyhow!("broker submit failed: {}", e));
                }
            };

            // Step 4: persist SENT status and broker order ID mapping.
            mqk_db::outbox_mark_sent(&self.pool, &order_id).await?;
            mqk_db::broker_map_upsert(&self.pool, &order_id, &resp.broker_order_id).await?;

            // Register in in-memory maps.
            self.order_map.register(&order_id, &resp.broker_order_id);
            self.oms_orders
                .insert(order_id.clone(), OmsOrder::new(&order_id, &symbol, qty));
        }

        // ------------------------------------------------------------------
        // Phase 2: Fetch broker events and ingest into oms_inbox.
        // ------------------------------------------------------------------
        let events = self
            .gateway
            .fetch_events()
            .map_err(|e| anyhow!("fetch_events failed: {}", e))?;

        for event in &events {
            let msg_json = serde_json::to_value(event)?;
            mqk_db::inbox_insert_deduped(
                &self.pool,
                self.run_id,
                event.broker_message_id(),
                msg_json,
            )
            .await?;
        }

        // ------------------------------------------------------------------
        // Phase 3: Apply all unapplied inbox rows.
        //
        // Loading after insertion handles both the current tick's events and
        // any rows that survived a crash between insert and mark_applied.
        // ------------------------------------------------------------------
        let unapplied = mqk_db::inbox_load_unapplied_for_run(&self.pool, self.run_id).await?;

        for row in unapplied {
            let msg_id = row.broker_message_id.clone();
            let event: BrokerEvent = serde_json::from_value(row.message_json)?;
            let internal_id = event.internal_order_id().to_string();

            // Step 6: apply OMS transition.
            let oms_event = broker_event_to_oms_event(&event);
            if let Some(order) = self.oms_orders.get_mut(&internal_id) {
                order
                    .apply(&oms_event, Some(&msg_id))
                    .map_err(|e| anyhow!("OMS transition error: {}", e))?;
            }

            // Step 7: apply portfolio change (fills only).
            if let Some(fill) = broker_event_to_fill(&event) {
                apply_entry(&mut self.portfolio, LedgerEntry::Fill(fill));
            }

            // Step 8: assert capital invariants before committing.
            check_capital_invariants(&self.portfolio)?;

            // Step 9: commit — mark the inbox row as applied.
            mqk_db::inbox_mark_applied(&self.pool, &msg_id).await?;
        }

        Ok(())
    }

    /// Immutable view of the current portfolio state.
    pub fn portfolio(&self) -> &PortfolioState {
        &self.portfolio
    }

    /// Immutable view of the current OMS order map.
    pub fn oms_orders(&self) -> &BTreeMap<String, OmsOrder> {
        &self.oms_orders
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Build a `BrokerSubmitRequest` from a claimed outbox row.
fn build_submit_request(row: &mqk_db::OutboxRow) -> anyhow::Result<BrokerSubmitRequest> {
    let json = &row.order_json;
    Ok(BrokerSubmitRequest {
        order_id: row.idempotency_key.clone(),
        symbol: json["symbol"]
            .as_str()
            .ok_or_else(|| anyhow!("order_json missing 'symbol'"))?
            .to_string(),
        quantity: json["quantity"]
            .as_i64()
            .ok_or_else(|| anyhow!("order_json missing 'quantity'"))? as i32,
        order_type: json["order_type"].as_str().unwrap_or("market").to_string(),
        limit_price: json["limit_price"].as_i64(),
        time_in_force: json["time_in_force"].as_str().unwrap_or("day").to_string(),
    })
}

/// Extract symbol from outbox order_json, defaulting to "UNKNOWN".
fn order_json_symbol(json: &serde_json::Value) -> String {
    json["symbol"].as_str().unwrap_or("UNKNOWN").to_string()
}

/// Extract quantity from outbox order_json, defaulting to 0.
fn order_json_qty(json: &serde_json::Value) -> i64 {
    json["quantity"].as_i64().unwrap_or(0)
}

/// Map a `BrokerEvent` to the corresponding `OmsEvent`.
fn broker_event_to_oms_event(event: &BrokerEvent) -> OmsEvent {
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
        BrokerEvent::ReplaceAck { .. } => OmsEvent::ReplaceAck,
        BrokerEvent::ReplaceReject { .. } => OmsEvent::ReplaceReject,
        BrokerEvent::Reject { .. } => OmsEvent::Reject,
    }
}

/// Extract a portfolio `Fill` from a `BrokerEvent`, if the event carries fill data.
///
/// Returns `None` for non-fill events (Ack, CancelAck, etc.).
/// Degenerate fills with `delta_qty <= 0` are rejected.
fn broker_event_to_fill(event: &BrokerEvent) -> Option<Fill> {
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
            if *delta_qty <= 0 {
                return None;
            }
            let portfolio_side = match side {
                mqk_execution::Side::Buy => mqk_portfolio::Side::Buy,
                mqk_execution::Side::Sell => mqk_portfolio::Side::Sell,
            };
            Some(Fill::new(
                symbol.clone(),
                portfolio_side,
                *delta_qty,
                *price_micros,
                *fee_micros,
            ))
        }
        _ => None,
    }
}

/// Assert that the incremental portfolio state is consistent with a full
/// recompute from the ledger.
///
/// Checks:
/// - `cash_micros` matches ledger recompute.
/// - `realized_pnl_micros` matches ledger recompute.
/// - `positions` map matches ledger recompute.
///
/// A mismatch indicates a bug in the incremental apply path and must be
/// treated as a halt condition by the caller.
fn check_capital_invariants(portfolio: &PortfolioState) -> anyhow::Result<()> {
    let (recomputed_cash, recomputed_pnl, recomputed_positions) =
        recompute_from_ledger(portfolio.initial_cash_micros, &portfolio.ledger);

    if recomputed_cash != portfolio.cash_micros {
        return Err(anyhow!(
            "INVARIANT_VIOLATED: cash_micros mismatch: recomputed={} state={}",
            recomputed_cash,
            portfolio.cash_micros
        ));
    }
    if recomputed_pnl != portfolio.realized_pnl_micros {
        return Err(anyhow!(
            "INVARIANT_VIOLATED: realized_pnl_micros mismatch: recomputed={} state={}",
            recomputed_pnl,
            portfolio.realized_pnl_micros
        ));
    }
    if recomputed_positions != portfolio.positions {
        return Err(anyhow!(
            "INVARIANT_VIOLATED: positions map mismatch between ledger recompute and state"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invariant_check_passes_on_fresh_portfolio() {
        let pf = PortfolioState::new(1_000_000_000_i64);
        check_capital_invariants(&pf).unwrap();
    }

    #[test]
    fn invariant_check_detects_cash_corruption() {
        let mut pf = PortfolioState::new(1_000_000_000_i64);
        // Directly corrupt cash without a corresponding ledger entry.
        pf.cash_micros = 999_999_999;
        assert!(check_capital_invariants(&pf).is_err());
    }

    #[test]
    fn invariant_check_passes_after_apply_entry() {
        use mqk_portfolio::{Fill, LedgerEntry, Side};
        let mut pf = PortfolioState::new(1_000_000_000_i64);
        let fill = Fill::new("AAPL", Side::Buy, 10, 150_000_000, 0);
        apply_entry(&mut pf, LedgerEntry::Fill(fill));
        // After apply_entry (which appends to ledger), invariant must hold.
        check_capital_invariants(&pf).unwrap();
    }

    #[test]
    fn broker_event_accessors() {
        use mqk_execution::Side;
        let ev = BrokerEvent::Fill {
            broker_message_id: "msg-1".to_string(),
            internal_order_id: "ord-1".to_string(),
            symbol: "AAPL".to_string(),
            side: Side::Buy,
            delta_qty: 10,
            price_micros: 150_000_000,
            fee_micros: 0,
        };
        assert_eq!(ev.broker_message_id(), "msg-1");
        assert_eq!(ev.internal_order_id(), "ord-1");
    }

    #[test]
    fn broker_event_to_fill_converts_correctly() {
        use mqk_execution::Side;
        let ev = BrokerEvent::Fill {
            broker_message_id: "msg-2".to_string(),
            internal_order_id: "ord-2".to_string(),
            symbol: "MSFT".to_string(),
            side: Side::Sell,
            delta_qty: 5,
            price_micros: 300_000_000,
            fee_micros: 1_000,
        };
        let fill = broker_event_to_fill(&ev).unwrap();
        assert_eq!(fill.qty, 5);
        assert_eq!(fill.price_micros, 300_000_000);
        assert_eq!(fill.fee_micros, 1_000);
        assert_eq!(fill.side, mqk_portfolio::Side::Sell);
    }

    #[test]
    fn broker_event_to_fill_returns_none_for_ack() {
        let ev = BrokerEvent::Ack {
            broker_message_id: "msg-3".to_string(),
            internal_order_id: "ord-3".to_string(),
        };
        assert!(broker_event_to_fill(&ev).is_none());
    }

    #[test]
    fn broker_event_to_fill_rejects_zero_qty() {
        use mqk_execution::Side;
        let ev = BrokerEvent::Fill {
            broker_message_id: "msg-4".to_string(),
            internal_order_id: "ord-4".to_string(),
            symbol: "X".to_string(),
            side: Side::Buy,
            delta_qty: 0,
            price_micros: 100_000_000,
            fee_micros: 0,
        };
        assert!(broker_event_to_fill(&ev).is_none());
    }

    #[test]
    fn oms_event_mapping_covers_all_variants() {
        use mqk_execution::Side;
        let cases: &[BrokerEvent] = &[
            BrokerEvent::Ack {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
            },
            BrokerEvent::Fill {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
                symbol: "X".to_string(),
                side: Side::Buy,
                delta_qty: 1,
                price_micros: 1,
                fee_micros: 0,
            },
            BrokerEvent::PartialFill {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
                symbol: "X".to_string(),
                side: Side::Buy,
                delta_qty: 1,
                price_micros: 1,
                fee_micros: 0,
            },
            BrokerEvent::CancelAck {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
            },
            BrokerEvent::CancelReject {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
            },
            BrokerEvent::ReplaceAck {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
            },
            BrokerEvent::ReplaceReject {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
            },
            BrokerEvent::Reject {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
            },
        ];
        // Verify mapping does not panic for any variant.
        for ev in cases {
            let _ = broker_event_to_oms_event(ev);
        }
    }
}
