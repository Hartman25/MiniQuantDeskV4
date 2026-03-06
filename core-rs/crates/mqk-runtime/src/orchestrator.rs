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
//! 2.  Receive `OutboxClaimToken` from `outbox_claim_batch` (FC-2: only the DB
//!     claim path constructs the token).
//! 3.  Submit via `BrokerGateway` (gates enforced inside gateway).
//! 4.  Persist SENT + broker order ID mapping; register in-memory OMS order.
//! 5.  Fetch broker events via `BrokerGateway::fetch_events`.
//! 6.  Persist each event to `oms_inbox` (dedup on `broker_message_id`).
//! 7.  Load all unapplied inbox rows for the run.
//! 8.  For each unapplied row: apply OMS transition, apply portfolio change,
//!     assert capital invariants, mark applied.

use std::collections::BTreeMap;

use anyhow::anyhow;
use mqk_db::TimeSource;
use sqlx::types::chrono;
use sqlx::PgPool;
use uuid::Uuid;
/// Production wall-clock time source.
///
/// NOTE: wall-clock reads must not live in `mqk-db` (guards enforce that).
#[derive(Clone, Copy, Debug, Default)]
pub struct WallClock;

impl TimeSource for WallClock {
    fn now_utc(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }
}

use mqk_execution::oms::state_machine::{OmsEvent, OmsOrder};
use mqk_execution::{
    BrokerAdapter, BrokerEvent, BrokerGateway, BrokerOrderMap, BrokerSubmitRequest,
    BrokerSubmitResponse, IntegrityGate, ReconcileGate, RiskGate,
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
pub struct ExecutionOrchestrator<B, IG, RG, RecG, TS>
where
    B: BrokerAdapter,
    IG: IntegrityGate,
    RG: RiskGate,
    RecG: ReconcileGate,
    TS: TimeSource,
{
    pool: PgPool,
    gateway: BrokerGateway<B, IG, RG, RecG>,
    order_map: BrokerOrderMap,
    oms_orders: BTreeMap<String, OmsOrder>,
    portfolio: PortfolioState,
    run_id: Uuid,
    dispatcher_id: String,
    /// FC-5: injected clock — no direct `Utc::now()` in the dispatch path.
    time_source: TS,
}

impl<B, IG, RG, RecG, TS> ExecutionOrchestrator<B, IG, RG, RecG, TS>
where
    B: BrokerAdapter,
    IG: IntegrityGate,
    RG: RiskGate,
    RecG: ReconcileGate,
    TS: TimeSource,
{
    /// Construct the orchestrator.
    ///
    /// All in-memory state must be pre-populated by the caller before
    /// the first `tick` call.
    ///
    /// `time_source` provides the UTC clock for dispatch timestamps (FC-5).
    /// Pass [`WallClock`] in production; inject a deterministic stub
    /// in tests.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: PgPool,
        gateway: BrokerGateway<B, IG, RG, RecG>,
        order_map: BrokerOrderMap,
        oms_orders: BTreeMap<String, OmsOrder>,
        portfolio: PortfolioState,
        run_id: Uuid,
        dispatcher_id: impl Into<String>,
        time_source: TS,
    ) -> Self {
        Self {
            pool,
            gateway,
            order_map,
            oms_orders,
            portfolio,
            run_id,
            dispatcher_id: dispatcher_id.into(),
            time_source,
        }
    }

    /// Execute one orchestrator tick.
    ///
    /// Phases:
    /// 0. Halt guard — refuse tick if run is HALTED in DB (I9-1).
    /// 1. Submit pending outbox rows via the gateway.
    /// 2. Fetch + ingest new broker events into oms_inbox.
    /// 3. Apply all unapplied inbox rows: OMS transition → portfolio apply →
    ///    capital invariant check (with halt+disarm persistence) → mark applied.
    ///
    /// Returns `Err` on any DB failure, gate refusal, OMS illegal transition,
    /// or invariant violation.  The caller must treat any `Err` as a halt
    /// signal and stop the tick loop.
    pub async fn tick(&mut self) -> anyhow::Result<()> {
        // ------------------------------------------------------------------
        // Phase 0 — I9-1 HALT GUARD.
        //
        // Load run status from DB at the top of every tick.  If the run is
        // already HALTED (written by a prior tick or by a concurrent process),
        // refuse immediately — no outbox claim, no submit, no inbox apply.
        //
        // This ensures a persisted halt is honoured across crash+restart and
        // multi-instance scenarios where a second process calls tick() after
        // the first has already written HALTED.
        // ------------------------------------------------------------------
        {
            let run = mqk_db::fetch_run(&self.pool, self.run_id).await?;
            if matches!(run.status, mqk_db::RunStatus::Halted) {
                return Err(anyhow!(
                    "HALT_GUARD: run {} is HALTED — tick refused (I9-1)",
                    self.run_id
                ));
            }
        }

        // ------------------------------------------------------------------
        // Phase 0b — Patch 2: restart quarantine for ambiguous outbox rows.
        //
        // Policy:
        // - DISPATCHING => submit may have been attempted, never silently requeue.
        // - SENT        => ambiguous only when broker-map evidence is missing.
        //
        // Without a broker-driven repair/reconcile path, the only safe behavior
        // is quarantine + halt/disarm before any new dispatch occurs.
        // ------------------------------------------------------------------
        {
            let ambiguous =
                mqk_db::outbox_load_restart_ambiguous_for_run(&self.pool, self.run_id).await?;

            if !ambiguous.is_empty() {
                let now = self.time_source.now_utc();

                // Best-effort persistence; still surface the recovery-quarantine error
                // even if one of these writes fails.
                let _ = mqk_db::halt_run(&self.pool, self.run_id, now).await;
                let _ =
                    mqk_db::persist_arm_state(&self.pool, "DISARMED", Some("RecoveryQuarantine"))
                        .await;

                let details = summarize_ambiguous_outbox(&ambiguous);
                return Err(anyhow!(
                    "RECOVERY_QUARANTINE: run {} has {} ambiguous outbox row(s); \
                     dispatch refused until operator action. rows=[{}]",
                    self.run_id,
                    ambiguous.len(),
                    details
                ));
            }
        }

        // ------------------------------------------------------------------
        // Phase 1: Claim and submit outbox rows.
        // ------------------------------------------------------------------
        let claimed = mqk_db::outbox_claim_batch(
            &self.pool,
            1,
            &self.dispatcher_id,
            self.time_source.now_utc(),
        )
        .await?;

        for claimed_row in claimed {
            let order_id = claimed_row.row.idempotency_key.clone();

            // Step 2: unforgeable claim token — returned by outbox_claim_batch (FC-2).
            let claim = &claimed_row.token;

            // Build a submit request from the outbox order_json.
            let req = build_submit_request(&claimed_row.row)?;
            let symbol = order_json_symbol(&claimed_row.row.order_json);
            let qty = order_json_qty(&claimed_row.row.order_json);

            // Step 3a: RT-5 — write DISPATCHING before calling gateway.submit().
            //
            // Closes crash window W4: if the process crashes between here and
            // outbox_mark_sent, the row stays DISPATCHING on restart.
            // outbox_reset_stale_claims only resets CLAIMED rows, so the order
            // is NOT silently requeued — preventing double-submit.
            mqk_db::outbox_mark_dispatching(
                &self.pool,
                &order_id,
                &self.dispatcher_id,
                self.time_source.now_utc(),
            )
            .await?;

            // Step 3b: submit via BrokerGateway — the ONLY submit path.
            //
            // Box<dyn Error> is !Send. Convert to anyhow::Error (Send+Sync)
            // in a typed binding BEFORE any match or await so the async
            // generator state never holds a !Send type.
            let submit_result: anyhow::Result<BrokerSubmitResponse> = self
                .gateway
                .submit(claim, req)
                .map_err(|e| anyhow!("{}", e));
            let resp = match submit_result {
                Ok(r) => r,
                Err(e) => {
                    // RT-5: Row is DISPATCHING — submit was attempted and may
                    // have reached the broker.  Mark FAILED rather than
                    // releasing to PENDING; requires operator review.
                    let _ = mqk_db::outbox_mark_failed(&self.pool, &order_id).await;
                    return Err(e.context("broker submit failed"));
                }
            };

            // Step 4: persist SENT status and broker order ID mapping.
            mqk_db::outbox_mark_sent(&self.pool, &order_id, self.time_source.now_utc()).await?;
            mqk_db::broker_map_upsert(&self.pool, &order_id, &resp.broker_order_id).await?;

            // Register in in-memory maps.
            self.order_map.register(&order_id, &resp.broker_order_id);
            self.oms_orders
                .insert(order_id.clone(), OmsOrder::new(&order_id, &symbol, qty));
        }

        // ------------------------------------------------------------------
        // Phase 2: Fetch broker events and ingest into oms_inbox.
        // ------------------------------------------------------------------
        // Same pattern: convert Box<dyn Error> → anyhow::Error in a typed
        // binding before the ? so Box<dyn Error> is never in the generator state.
        let events_result: anyhow::Result<Vec<BrokerEvent>> = self
            .gateway
            .fetch_events()
            .map_err(|e| anyhow!("fetch_events failed: {}", e));
        let events = events_result?;

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

        // Phase 3a: RT-4 — deserialize all rows, then sort into canonical apply order.
        //
        // Sort key: (broker_message_id ASC, internal_order_id ASC, event_kind_rank ASC).
        // The DB already orders by broker_message_id ASC; the in-process sort makes
        // the final order deterministic regardless of DB row ordering or future index changes.
        let mut apply_queue: Vec<(String, BrokerEvent)> = Vec::with_capacity(unapplied.len());
        for row in unapplied {
            let msg_id = row.broker_message_id;
            let event: BrokerEvent = serde_json::from_value(row.message_json)?;
            apply_queue.push((msg_id, event));
        }
        apply_queue.sort_by(|(a_msg, a_ev), (b_msg, b_ev)| {
            a_msg
                .cmp(b_msg)
                .then_with(|| a_ev.internal_order_id().cmp(b_ev.internal_order_id()))
                .then_with(|| event_kind_rank(a_ev).cmp(&event_kind_rank(b_ev)))
        });

        // Phase 3b: apply in canonical order.
        for (msg_id, event) in apply_queue {
            let internal_id = event.internal_order_id().to_string();

            // Step 6: apply OMS transition.
            let oms_event = broker_event_to_oms_event(&event);
            if let Some(order) = self.oms_orders.get_mut(&internal_id) {
                order
                    .apply(&oms_event, Some(&msg_id))
                    .map_err(|e| anyhow!("OMS transition error: {}", e))?;
            }

            // RT-9: Phase 3b — when a live broker Ack carries the exchange-assigned
            // order ID, register it in the in-memory order map.  For paper brokers
            // `broker_order_id` is `None` (the ID was already registered in Phase 1
            // from `BrokerSubmitResponse.broker_order_id`).  Re-registering with the
            // same value is idempotent.
            if let BrokerEvent::Ack {
                broker_order_id: Some(bid),
                ..
            } = &event
            {
                self.order_map.register(&internal_id, bid);
            }

            // Step 7: apply portfolio change (fills only).
            if let Some(fill) = broker_event_to_fill(&event) {
                apply_entry(&mut self.portfolio, LedgerEntry::Fill(fill));
            }

            // Step 8: assert capital invariants — I9-1 persistence requirement.
            //
            // On violation: persist HALTED run status and DISARMED arm state
            // before returning.  Best-effort DB writes: if either write fails,
            // the invariant error is still surfaced to the caller.  The halt
            // guard (Phase 0) prevents any further dispatch once HALTED is
            // written to DB.
            if let Err(inv_err) = check_capital_invariants(&self.portfolio) {
                let now = self.time_source.now_utc();
                let _ = mqk_db::halt_run(&self.pool, self.run_id, now).await;
                let _ =
                    mqk_db::persist_arm_state(&self.pool, "DISARMED", Some("IntegrityViolation"))
                        .await;
                return Err(inv_err.context(format!(
                    "INVARIANT_VIOLATED: run {} halted and disarmed (I9-1)",
                    self.run_id
                )));
            }

            // Step 9: commit — mark the inbox row as applied.
            mqk_db::inbox_mark_applied(
                &self.pool,
                self.run_id,
                &msg_id,
                self.time_source.now_utc(),
            )
            .await?;
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

/// RT-4: Canonical rank for `BrokerEvent` variants in the deterministic apply queue.
///
/// Ordering intent: Ack before fills before cancel/replace; within a kind the
/// SQL `broker_message_id ASC` order dominates.  This rank is used only as a
/// tie-breaker when two events share the same `broker_message_id` AND the same
/// `internal_order_id`.
fn event_kind_rank(event: &BrokerEvent) -> u8 {
    match event {
        BrokerEvent::Ack { .. } => 0,
        BrokerEvent::PartialFill { .. } => 1,
        BrokerEvent::Fill { .. } => 2,
        BrokerEvent::CancelAck { .. } => 3,
        BrokerEvent::CancelReject { .. } => 4,
        BrokerEvent::ReplaceAck { .. } => 5,
        BrokerEvent::ReplaceReject { .. } => 6,
        BrokerEvent::Reject { .. } => 7,
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

fn summarize_ambiguous_outbox(rows: &[mqk_db::AmbiguousOutboxRow]) -> String {
    rows.iter()
        .map(|r| match &r.broker_order_id {
            Some(bid) => format!("{}:{}:broker={}", r.idempotency_key, r.status, bid),
            None => format!("{}:{}", r.idempotency_key, r.status),
        })
        .collect::<Vec<_>>()
        .join(", ")
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
            broker_order_id: None,
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
            broker_order_id: None,
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
            broker_order_id: None,
        };
        assert!(broker_event_to_fill(&ev).is_none());
    }

    #[test]
    fn broker_event_to_fill_rejects_zero_qty() {
        use mqk_execution::Side;
        let ev = BrokerEvent::Fill {
            broker_message_id: "msg-4".to_string(),
            internal_order_id: "ord-4".to_string(),
            broker_order_id: None,
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
                broker_order_id: None,
            },
            BrokerEvent::Fill {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
                broker_order_id: None,
                symbol: "X".to_string(),
                side: Side::Buy,
                delta_qty: 1,
                price_micros: 1,
                fee_micros: 0,
            },
            BrokerEvent::PartialFill {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
                broker_order_id: None,
                symbol: "X".to_string(),
                side: Side::Buy,
                delta_qty: 1,
                price_micros: 1,
                fee_micros: 0,
            },
            BrokerEvent::CancelAck {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
                broker_order_id: None,
            },
            BrokerEvent::CancelReject {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
                broker_order_id: None,
            },
            BrokerEvent::ReplaceAck {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
                broker_order_id: None,
            },
            BrokerEvent::ReplaceReject {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
                broker_order_id: None,
            },
            BrokerEvent::Reject {
                broker_message_id: "m".to_string(),
                internal_order_id: "o".to_string(),
                broker_order_id: None,
            },
        ];
        // Verify mapping does not panic for any variant.
        for ev in cases {
            let _ = broker_event_to_oms_event(ev);
        }
    }

    #[test]
    fn event_kind_rank_is_strictly_ordered() {
        use mqk_execution::Side;
        // Verify no two distinct variant kinds map to the same rank.
        let events: Vec<BrokerEvent> = vec![
            BrokerEvent::Ack {
                broker_message_id: "m".into(),
                internal_order_id: "o".into(),
                broker_order_id: None,
            },
            BrokerEvent::PartialFill {
                broker_message_id: "m".into(),
                internal_order_id: "o".into(),
                broker_order_id: None,
                symbol: "X".into(),
                side: Side::Buy,
                delta_qty: 1,
                price_micros: 1,
                fee_micros: 0,
            },
            BrokerEvent::Fill {
                broker_message_id: "m".into(),
                internal_order_id: "o".into(),
                broker_order_id: None,
                symbol: "X".into(),
                side: Side::Buy,
                delta_qty: 1,
                price_micros: 1,
                fee_micros: 0,
            },
            BrokerEvent::CancelAck {
                broker_message_id: "m".into(),
                internal_order_id: "o".into(),
                broker_order_id: None,
            },
            BrokerEvent::CancelReject {
                broker_message_id: "m".into(),
                internal_order_id: "o".into(),
                broker_order_id: None,
            },
            BrokerEvent::ReplaceAck {
                broker_message_id: "m".into(),
                internal_order_id: "o".into(),
                broker_order_id: None,
            },
            BrokerEvent::ReplaceReject {
                broker_message_id: "m".into(),
                internal_order_id: "o".into(),
                broker_order_id: None,
            },
            BrokerEvent::Reject {
                broker_message_id: "m".into(),
                internal_order_id: "o".into(),
                broker_order_id: None,
            },
        ];
        let mut ranks: Vec<u8> = events.iter().map(event_kind_rank).collect();
        ranks.sort_unstable();
        ranks.dedup();
        assert_eq!(
            ranks.len(),
            events.len(),
            "each variant must have a unique rank"
        );
    }

    #[test]
    fn apply_queue_sort_is_canonical() {
        use mqk_execution::Side;
        // Two events for the same message/order: Ack must sort before Fill by rank.
        let fill = BrokerEvent::Fill {
            broker_message_id: "msg-a".into(),
            internal_order_id: "ord-1".into(),
            broker_order_id: None,
            symbol: "X".into(),
            side: Side::Buy,
            delta_qty: 5,
            price_micros: 100,
            fee_micros: 0,
        };
        let ack = BrokerEvent::Ack {
            broker_message_id: "msg-a".into(),
            internal_order_id: "ord-1".into(),
            broker_order_id: None,
        };
        let mut queue: Vec<(String, BrokerEvent)> =
            vec![("msg-a".into(), fill), ("msg-a".into(), ack)];
        queue.sort_by(|(a_msg, a_ev), (b_msg, b_ev)| {
            a_msg
                .cmp(b_msg)
                .then_with(|| a_ev.internal_order_id().cmp(b_ev.internal_order_id()))
                .then_with(|| event_kind_rank(a_ev).cmp(&event_kind_rank(b_ev)))
        });
        assert!(
            matches!(queue[0].1, BrokerEvent::Ack { .. }),
            "Ack (rank 0) must sort before Fill (rank 2) on same message/order"
        );
    }
}
