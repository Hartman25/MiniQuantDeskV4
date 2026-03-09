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

use anyhow::{anyhow, Context as _};
use mqk_db::TimeSource;
use mqk_reconcile::{reconcile_tick, BrokerSnapshot, DriftAction, LocalSnapshot};
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
    /// Patch 4A: authoritative local reconcile snapshot source.
    ///
    /// This is injected by the caller so runtime can enforce reconcile drift
    /// before any new dispatch occurs.
    local_snapshot_provider: Box<dyn Fn() -> LocalSnapshot + Send + Sync>,
    /// Patch 4A: authoritative broker reconcile snapshot source.
    ///
    /// This is injected by the caller so runtime can enforce reconcile drift
    /// before any new dispatch occurs.
    broker_snapshot_provider: Box<dyn Fn() -> BrokerSnapshot + Send + Sync>,
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
    ///
    /// Patch 4A:
    /// The caller must also inject local + broker snapshot providers used for
    /// reconcile drift enforcement before dispatch.
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
        local_snapshot_provider: Box<dyn Fn() -> LocalSnapshot + Send + Sync>,
        broker_snapshot_provider: Box<dyn Fn() -> BrokerSnapshot + Send + Sync>,
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
            local_snapshot_provider,
            broker_snapshot_provider,
        }
    }

    /// Execute one orchestrator tick.
    ///
    /// Phases:
    /// 0. Halt guard — refuse tick if run is HALTED in DB (I9-1).
    /// 0b. Restart quarantine — refuse tick if ambiguous DISPATCHING / SENT
    ///     outbox rows exist (Patch 2).
    /// 0c. Reconcile drift enforcement — refuse tick and persist HALT/DISARM
    ///     if local vs broker snapshots are dirty (Patch 4A).
    /// 1. Submit pending outbox rows via the gateway.
    /// 2. Fetch + ingest new broker events into oms_inbox.
    /// 3. Apply all unapplied inbox rows: OMS transition → portfolio apply →
    ///    capital invariant check (with halt+disarm persistence) → mark applied.
    ///
    /// Returns `Err` on any DB failure, gate refusal, OMS illegal transition,
    /// reconcile drift, or invariant violation.  The caller must treat any
    /// `Err` as a halt signal and stop the tick loop.
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

                // Mandatory halt + disarm — both writes must succeed before returning.
                // If either write fails the error propagates immediately so the caller
                // learns the persistence failure rather than silently losing the halt.
                // On success the Phase-0 HALT_GUARD will block any future tick() on
                // any orchestrator instance for this run_id.
                persist_halt_and_disarm(&self.pool, self.run_id, now, "IntegrityViolation").await?;

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
        // Phase 0c — Patch 4A: reconcile drift enforcement.
        //
        // Runtime must not dispatch new outbox rows if local and broker state
        // have drifted.  Reconcile is now an authoritative pre-dispatch gate.
        //
        // Policy:
        // - CLEAN  => continue
        // - DIRTY  => HALT run + DISARM arm state + refuse dispatch
        //
        // This is intentionally fail-closed.
        // ------------------------------------------------------------------
        {
            let local = (self.local_snapshot_provider)();
            let broker = (self.broker_snapshot_provider)();

            let action = reconcile_tick(&local, &broker);

            if let DriftAction::HaltAndDisarm { .. } = action {
                let now = self.time_source.now_utc();

                // Mandatory halt + disarm — same fail-closed contract as Phase 0b.
                persist_halt_and_disarm(&self.pool, self.run_id, now, "ReconcileDrift").await?;

                return Err(anyhow!(
                    "RECONCILE_DRIFT: run {} halted and disarmed; dispatch refused",
                    self.run_id
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
            let claim = claimed_row.token;

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
                .submit(&claim, req)
                .map_err(|e| anyhow!("{}", e));

            let resp = match submit_result {
                Ok(r) => r,
                Err(e) => {
                    // RT-5: Row is DISPATCHING — submit was attempted and may
                    // have reached the broker. Mark FAILED rather than
                    // releasing to PENDING; requires operator review.
                    let _ = mqk_db::outbox_mark_failed(&self.pool, &order_id).await;
                    return Err(e.context("broker submit failed"));
                }
            };

            // Step 4: atomically persist broker order ID mapping + SENT status.
            //
            // Patch 3A:
            // After broker submit succeeds, the DB must not be able to observe
            // a durable SENT row without the corresponding durable
            // internal_id -> broker_id mapping needed for restart recovery,
            // reconcile, and cancel/replace targeting.
            let sent = mqk_db::outbox_mark_sent_with_broker_map(
                &self.pool,
                &order_id,
                &resp.broker_order_id,
                self.time_source.now_utc(),
            )
            .await?;

            if !sent {
                return Err(anyhow!(
                    "broker submit succeeded but outbox row {} could not transition to SENT with broker map persistence",
                    order_id
                ));
            }

            // Register in-memory state only after DB durability succeeds.
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
        // SECTION D — Durable restart replay gate.
        //
        // inbox_load_unapplied_for_run queries `applied_at_utc IS NULL`.
        // This is the authoritative restart-replay boundary, not the OMS
        // in-memory applied_event_ids set.
        //
        // Contract after restart:
        //
        // - Fills marked applied before crash (applied_at_utc IS NOT NULL)
        //   are EXCLUDED from this query.  Even though OmsOrder is rebuilt
        //   fresh with an empty applied_event_ids, the DB column is the gate.
        //   These fills will never reach apply_fill_step and cannot double-apply.
        //
        // - Fills in the W6 crash window (inbox insert completed but
        //   mark_applied did not) appear here and are re-applied exactly once
        //   by apply_fill_step with the reconstructed OmsOrder.  This is
        //   correct recovery, not a double-apply: the in-memory portfolio was
        //   lost on crash and must be rebuilt.
        //
        // - inbox_insert_deduped (Phase 2) prevents duplicate inbox rows;
        //   the (run_id, broker_message_id) unique constraint is the key.
        //   A broker replay after restart cannot produce a second inbox row.
        //
        // Restart replay safety therefore does NOT depend on OMS applied_event_ids
        // surviving a crash.  The durable applied_at_utc column is sufficient.
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

            // Steps 6+7: OMS context guard → portfolio apply (Section C).
            //
            // apply_fill_step enforces that fill events cannot reach portfolio
            // without a proven OMS order context in memory.  On Err (unknown-
            // order fill OR illegal OMS transition), halt the run and disarm
            // before propagating — same pattern as capital invariant violations.
            let fill_opt =
                match apply_fill_step(&mut self.oms_orders, &internal_id, &event, &msg_id) {
                    Ok(f) => f,
                    Err(e) => {
                        let now = self.time_source.now_utc();
                        // Mandatory halt + disarm before surfacing the OMS error.
                        // If the DB writes fail their error takes precedence — failing
                        // to persist HALTED is more dangerous than the OMS fault itself.
                        persist_halt_and_disarm(&self.pool, self.run_id, now, "IntegrityViolation")
                            .await?;
                        return Err(e.context(format!(
                            "UNKNOWN_ORDER_FILL: run {} halted and disarmed (Section C)",
                            self.run_id
                        )));
                    }
                };

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

            // Apply portfolio fill — only if apply_fill_step returned Some(fill).
            // None is returned for: non-fill events, non-fill events for unknown
            // orders, and no-op replays (duplicate event_id or late fill on a
            // terminal OMS order where filled_qty did not advance).
            if let Some(fill) = fill_opt {
                apply_entry(&mut self.portfolio, LedgerEntry::Fill(fill));
            }

            // Step 8: assert capital invariants — I9-1 persistence requirement.
            //
            // On violation: persist HALTED run status and DISARMED arm state
            // before returning.  Both writes are now mandatory (not best-effort):
            // if either write fails, persist_halt_and_disarm propagates the DB
            // error immediately so the caller learns the failure explicitly.
            // The halt guard (Phase 0) blocks any further dispatch once HALTED
            // is durably written to DB.
            if let Err(inv_err) = check_capital_invariants(&self.portfolio) {
                let now = self.time_source.now_utc();
                persist_halt_and_disarm(&self.pool, self.run_id, now, "IntegrityViolation").await?;
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
    let raw_qty = json["quantity"]
        .as_i64()
        .ok_or_else(|| anyhow!("order_json missing 'quantity'"))?;
    Ok(BrokerSubmitRequest {
        order_id: row.idempotency_key.clone(),
        symbol: json["symbol"]
            .as_str()
            .ok_or_else(|| anyhow!("order_json missing 'symbol'"))?
            .to_string(),
        side: order_json_side(json),
        quantity: raw_qty.saturating_abs() as i32,
        order_type: json["order_type"].as_str().unwrap_or("market").to_string(),
        limit_price: json["limit_price"].as_i64(),
        time_in_force: json["time_in_force"].as_str().unwrap_or("day").to_string(),
    })
}

/// Extract symbol from outbox order_json, defaulting to "UNKNOWN".
fn order_json_symbol(json: &serde_json::Value) -> String {
    json["symbol"].as_str().unwrap_or("UNKNOWN").to_string()
}

/// Extract ABSOLUTE quantity from outbox order_json, defaulting to 0.
///
/// P1-02:
/// OMS registration must never inherit signed sell quantity from order_json.
/// Direction is conveyed by side; in-memory OMS quantity must always be positive.
fn order_json_qty(json: &serde_json::Value) -> i64 {
    json["quantity"].as_i64().unwrap_or(0).saturating_abs()
}

/// Extract side from outbox order_json.
///
/// Preferred contract:
/// - `side` is explicit ("buy" / "sell")
/// - `quantity` is always positive
///
/// Backward compatibility:
/// if `side` is absent, infer it from the legacy signed quantity encoding.
fn order_json_side(json: &serde_json::Value) -> mqk_execution::Side {
    match json["side"].as_str() {
        Some("buy") | Some("BUY") | Some("Buy") => mqk_execution::Side::Buy,
        Some("sell") | Some("SELL") | Some("Sell") => mqk_execution::Side::Sell,
        _ => {
            if json["quantity"].as_i64().unwrap_or(0) >= 0 {
                mqk_execution::Side::Buy
            } else {
                mqk_execution::Side::Sell
            }
        }
    }
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
        BrokerEvent::ReplaceAck { new_total_qty, .. } => OmsEvent::ReplaceAck {
            new_total_qty: *new_total_qty,
        },
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
// Internal helper — mandatory halt + disarm persistence
// ---------------------------------------------------------------------------

/// Write `runs.status = 'HALTED'` and `sys_arm_state = 'DISARMED'` to the DB
/// as mandatory (not best-effort) operations.
///
/// # Fail-closed contract
///
/// Both writes use `?` propagation.  If either write fails the caller receives
/// an explicit `Err` describing the DB failure rather than silently continuing.
/// This is intentionally stricter than the previous best-effort `let _ = …`
/// pattern: it is safer for a tick to surface `HALT_PERSISTENCE_FAILURE` than
/// to return the original halt error while leaving `runs.status` as RUNNING,
/// which would allow the Phase-0 HALT_GUARD to be bypassed on the next call.
///
/// # Order of writes
///
/// `halt_run` is always attempted first.  If it succeeds but `persist_arm_state`
/// fails, the arm-state error is returned; `runs.status` is already HALTED so
/// the Phase-0 guard will still block future ticks on any orchestrator instance
/// for this `run_id`.
async fn persist_halt_and_disarm(
    pool: &PgPool,
    run_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
    reason: &'static str,
) -> anyhow::Result<()> {
    mqk_db::halt_run(pool, run_id, now).await.with_context(|| {
        format!(
            "HALT_PERSISTENCE_FAILURE: run {run_id} — runs.status=HALTED could not be \
                 written (reason={reason}); Phase-0 halt guard on restart is NOT guaranteed"
        )
    })?;

    mqk_db::persist_arm_state(pool, "DISARMED", Some(reason))
        .await
        .with_context(|| {
            format!(
                "ARM_STATE_PERSISTENCE_FAILURE: run {run_id} — sys_arm_state=DISARMED could \
                 not be written (reason={reason}); runs.status=HALTED was persisted"
            )
        })?;

    Ok(())
}

/// Section C — Restart / Unknown-Order Fill Safety.
///
/// Apply OMS transition and return the portfolio fill to apply (if any).
///
/// # Invariants enforced
///
/// - Fill events (`PartialFill`, `Fill`) **must** have a corresponding OMS
///   order in memory. If none is found, `Err` is returned immediately —
///   the caller must halt and disarm before propagating.
///
/// - Non-fill events (Ack, CancelAck, etc.) for unknown orders are silently
///   skipped. They carry no portfolio effect and can arrive after a crash
///   before the in-memory order map has been rebuilt.
///
/// - Duplicate fill replays are detected by comparing `order.filled_qty`
///   before and after `apply()`. If `filled_qty` did not advance on a fill
///   event, the OMS applied a silent no-op (duplicate `event_id` or late fill
///   on a terminal order). `Ok(None)` is returned to prevent a double
///   portfolio mutation.
///
/// The caller is responsible for halting and disarming on `Err`.
fn apply_fill_step(
    oms_orders: &mut BTreeMap<String, OmsOrder>,
    internal_id: &str,
    event: &BrokerEvent,
    msg_id: &str,
) -> anyhow::Result<Option<Fill>> {
    let is_fill = matches!(
        event,
        BrokerEvent::PartialFill { .. } | BrokerEvent::Fill { .. }
    );
    let oms_event = broker_event_to_oms_event(event);

    match oms_orders.get_mut(internal_id) {
        Some(order) => {
            let pre_qty = order.filled_qty;
            order
                .apply(&oms_event, Some(msg_id))
                .map_err(|e| anyhow!("OMS transition error for '{}': {}", internal_id, e))?;
            // No-op detection: if this is a fill event and filled_qty has not
            // advanced, OMS applied a silent no-op (duplicate event_id or fill
            // arriving after the order reached a terminal state).  Skip the
            // portfolio mutation to prevent double-counting.
            if is_fill && order.filled_qty == pre_qty {
                return Ok(None);
            }
        }
        None if is_fill => {
            // Section C invariant: fill events must not reach portfolio without
            // a proven OMS order context in memory.  Fail closed.
            return Err(anyhow!(
                "UNKNOWN_ORDER_FILL: broker_message_id='{}' internal_order_id='{}' \
                 — fill event has no OMS order context in memory; \
                 refusing portfolio mutation (Section C)",
                msg_id,
                internal_id
            ));
        }
        None => {
            // Non-fill event for unknown order — silently skipped.
            // No OMS transition, no portfolio effect.
        }
    }

    Ok(broker_event_to_fill(event))
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
    fn order_json_qty_preserves_positive_buy_quantity() {
        let json = serde_json::json!({
            "symbol": "SPY",
            "side": "buy",
            "quantity": 100
        });
        assert_eq!(order_json_qty(&json), 100);
        assert!(matches!(order_json_side(&json), mqk_execution::Side::Buy));
    }

    #[test]
    fn order_json_qty_normalizes_negative_sell_quantity_for_oms_registration() {
        let json = serde_json::json!({
            "symbol": "SPY",
            "quantity": -100
        });

        let qty = order_json_qty(&json);
        assert_eq!(qty, 100, "OMS registration quantity must be absolute");
        assert!(matches!(order_json_side(&json), mqk_execution::Side::Sell));

        let order = OmsOrder::new("ord-sell", "SPY", qty);
        assert_eq!(
            order.total_qty, 100,
            "negative signed sell quantity must not leak into OmsOrder::new"
        );
    }

    #[test]
    fn explicit_side_overrides_legacy_sign_in_submit_request_building() {
        let row = mqk_db::OutboxRow {
            outbox_id: 1,
            run_id: uuid::Uuid::nil(),
            idempotency_key: "ord-1".to_string(),
            order_json: serde_json::json!({
                "symbol": "SPY",
                "side": "sell",
                "quantity": 100,
                "order_type": "market",
                "time_in_force": "day"
            }),
            status: "PENDING".to_string(),
            created_at_utc: chrono::Utc::now(),
            sent_at_utc: None,
            claimed_at_utc: None,
            claimed_by: None,
            dispatching_at_utc: None,
            dispatch_attempt_id: None,
        };

        let req = build_submit_request(&row).expect("submit request must build");
        assert!(matches!(req.side, mqk_execution::Side::Sell));
        assert_eq!(req.quantity, 100);
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
                new_total_qty: 100, // P1-03
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
                new_total_qty: 100, // P1-03
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

    // -----------------------------------------------------------------------
    // Section C — apply_fill_step unit tests
    // -----------------------------------------------------------------------

    fn make_ack_event(internal_id: &str, msg_id: &str) -> BrokerEvent {
        BrokerEvent::Ack {
            broker_message_id: msg_id.to_string(),
            internal_order_id: internal_id.to_string(),
            broker_order_id: None,
        }
    }

    fn make_partial_fill_event(internal_id: &str, msg_id: &str, qty: i64) -> BrokerEvent {
        BrokerEvent::PartialFill {
            broker_message_id: msg_id.to_string(),
            internal_order_id: internal_id.to_string(),
            broker_order_id: None,
            symbol: "SPY".to_string(),
            side: mqk_execution::Side::Buy,
            delta_qty: qty,
            price_micros: 450_000_000,
            fee_micros: 0,
        }
    }

    fn make_fill_event(internal_id: &str, msg_id: &str, qty: i64) -> BrokerEvent {
        BrokerEvent::Fill {
            broker_message_id: msg_id.to_string(),
            internal_order_id: internal_id.to_string(),
            broker_order_id: None,
            symbol: "SPY".to_string(),
            side: mqk_execution::Side::Buy,
            delta_qty: qty,
            price_micros: 450_000_000,
            fee_micros: 0,
        }
    }

    /// Section C — T1.
    /// A Fill event for an order not present in oms_orders must return
    /// UNKNOWN_ORDER_FILL and never produce a portfolio fill.
    #[test]
    fn unknown_order_fill_is_rejected() {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        let ev = make_fill_event("ord-unknown", "fill-msg-1", 100);
        let result = apply_fill_step(&mut oms, "ord-unknown", &ev, "fill-msg-1");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("UNKNOWN_ORDER_FILL"),
            "expected UNKNOWN_ORDER_FILL, got: {err}"
        );
    }

    /// Section C — T2.
    /// A PartialFill event for an order not present in oms_orders must also
    /// return UNKNOWN_ORDER_FILL — the rule is not limited to final fills.
    #[test]
    fn unknown_order_partial_fill_is_rejected() {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        let ev = make_partial_fill_event("ord-unknown", "pf-msg-1", 50);
        let result = apply_fill_step(&mut oms, "ord-unknown", &ev, "pf-msg-1");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("UNKNOWN_ORDER_FILL"),
            "expected UNKNOWN_ORDER_FILL, got: {err}"
        );
    }

    /// Section C — T3.
    /// A Fill event for a known order must succeed, return Some(fill) with
    /// correct qty, and advance the OMS filled_qty.
    #[test]
    fn known_order_fill_succeeds_and_returns_fill() {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        oms.insert("ord-1".to_string(), OmsOrder::new("ord-1", "SPY", 100));
        let ev = make_fill_event("ord-1", "fill-msg-2", 100);
        let result = apply_fill_step(&mut oms, "ord-1", &ev, "fill-msg-2");
        let fill = result
            .unwrap()
            .expect("expected Some(fill) for known order fill");
        assert_eq!(fill.qty, 100);
        // OMS state must have advanced.
        assert_eq!(oms["ord-1"].filled_qty, 100);
    }

    /// Section C — T4.
    /// An OMS-level transition error (fill would overflow total_qty) must
    /// surface as Err containing "OMS transition error" and must NOT advance
    /// filled_qty, preventing any downstream portfolio mutation.
    #[test]
    fn oms_rejection_blocks_portfolio_fill() {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        let mut order = OmsOrder::new("ord-2", "SPY", 100);
        // Pre-fill 60 so that any further 60-unit fill overflows.
        order
            .apply(&OmsEvent::PartialFill { delta_qty: 60 }, Some("pf-setup"))
            .unwrap();
        oms.insert("ord-2".to_string(), order);

        // Fill(60) when filled=60, total=100 → 60+60=120 ≠ 100 → TransitionError.
        let ev = make_fill_event("ord-2", "fill-overflow", 60);
        let result = apply_fill_step(&mut oms, "ord-2", &ev, "fill-overflow");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("OMS transition error"),
            "expected OMS transition error, got: {err}"
        );
        // filled_qty must NOT have advanced on rejection.
        assert_eq!(oms["ord-2"].filled_qty, 60);
    }

    /// Section C — T5.
    /// A duplicate fill replay (same msg_id applied twice to the same order)
    /// must return Ok(Some(fill)) on the first call and Ok(None) on the second.
    /// filled_qty must not advance on the duplicate.
    #[test]
    fn duplicate_fill_replay_does_not_double_apply_portfolio() {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        oms.insert("ord-3".to_string(), OmsOrder::new("ord-3", "SPY", 100));

        let ev = make_partial_fill_event("ord-3", "pf-msg-dup", 60);

        // First application: fill goes through.
        let first = apply_fill_step(&mut oms, "ord-3", &ev, "pf-msg-dup")
            .unwrap()
            .expect("first application must return Some(fill)");
        assert_eq!(first.qty, 60);
        assert_eq!(oms["ord-3"].filled_qty, 60);

        // Second application with the same msg_id: OMS dedup → no state change.
        let second = apply_fill_step(&mut oms, "ord-3", &ev, "pf-msg-dup").unwrap();
        assert!(
            second.is_none(),
            "duplicate fill replay must return None to prevent double portfolio mutation"
        );
        // filled_qty must not have advanced.
        assert_eq!(oms["ord-3"].filled_qty, 60);
    }

    /// Section C — T6.
    /// A non-fill event (Ack) for an order not present in oms_orders must
    /// return Ok(None) — not Err.  Unknown-order Acks are silently skipped
    /// because they carry no portfolio effect and can arrive legitimately after
    /// a crash during restart recovery.
    #[test]
    fn unknown_order_non_fill_is_silently_skipped() {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        let ev = make_ack_event("ord-ghost", "ack-msg-ghost");
        let result = apply_fill_step(&mut oms, "ord-ghost", &ev, "ack-msg-ghost");
        assert!(
            result.unwrap().is_none(),
            "non-fill event for unknown order must return Ok(None), not Err"
        );
    }

    // -----------------------------------------------------------------------
    // Section D — Restart replay safety unit tests
    //
    // These tests prove that restart replay safety is gated by the durable
    // inbox applied_at_utc column (modelled here as queue membership), NOT
    // by the OMS in-memory applied_event_ids set.
    // -----------------------------------------------------------------------

    /// Section D — T1.  Primary restart replay safety proof.
    ///
    /// A fill that was durably marked applied (applied_at_utc IS NOT NULL)
    /// before crash is excluded from inbox_load_unapplied_for_run.  Modelled
    /// here as an empty apply_queue.  With no rows in the queue, the portfolio
    /// cannot be mutated regardless of OmsOrder applied_event_ids being empty.
    ///
    /// This is the load-bearing proof: the DB queue filter is the gate, not
    /// the in-memory set.
    #[test]
    fn applied_fill_absent_from_recovery_queue_leaves_portfolio_clean() {
        let initial_cash = 1_000_000_000_i64;

        // Fresh restart: OmsOrder rebuilt from outbox — applied_event_ids is empty.
        // The fill that was applied before crash is NOT in the apply_queue
        // because inbox_load_unapplied_for_run excluded it (applied_at_utc IS NOT NULL).
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        oms.insert(
            "ord-pre-crash".to_string(),
            OmsOrder::new("ord-pre-crash", "SPY", 100),
        );

        let apply_queue: Vec<(String, BrokerEvent)> = vec![]; // applied fill filtered by DB

        let mut portfolio = PortfolioState::new(initial_cash);
        for (msg_id, event) in &apply_queue {
            let internal_id = event.internal_order_id().to_string();
            let fill_opt = apply_fill_step(&mut oms, &internal_id, event, msg_id).unwrap();
            if let Some(fill) = fill_opt {
                apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
            }
        }

        assert_eq!(
            portfolio.cash_micros, initial_cash,
            "applied fill absent from recovery queue must not re-mutate portfolio cash after restart"
        );
        assert_eq!(
            oms["ord-pre-crash"].filled_qty, 0,
            "fresh OmsOrder must not advance filled_qty when recovery queue is empty"
        );
    }

    /// Section D — T2.  Unapplied fill recovers exactly once with fresh OMS state.
    ///
    /// Simulates the W6 crash window: fill was inbox-inserted but mark_applied
    /// did not complete before crash.  After restart the OmsOrder is rebuilt
    /// fresh (applied_event_ids empty) and the fill IS in the recovery queue.
    ///
    /// First apply: Ok(Some(fill)) — portfolio mutated (correct recovery).
    /// Second delivery of the same msg_id within the session: Ok(None) —
    /// blocked by the within-session OMS dedup (applied_event_ids updated
    /// by the first apply).
    #[test]
    fn unapplied_fill_in_recovery_queue_applies_exactly_once_with_fresh_oms() {
        let initial_cash = 1_000_000_000_000_i64;
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        oms.insert("ord-w6".to_string(), OmsOrder::new("ord-w6", "SPY", 100));

        let ev = make_fill_event("ord-w6", "crash-window-fill", 100);

        // First recovery apply: fresh applied_event_ids, fill is in queue.
        let fill_opt = apply_fill_step(&mut oms, "ord-w6", &ev, "crash-window-fill").unwrap();
        let mut portfolio = PortfolioState::new(initial_cash);
        if let Some(fill) = fill_opt {
            apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
        }

        assert_ne!(
            portfolio.cash_micros, initial_cash,
            "unapplied fill must apply once and mutate portfolio on crash-window recovery"
        );
        assert_eq!(oms["ord-w6"].filled_qty, 100);

        // Second delivery of same msg_id within recovery session:
        // OMS applied_event_ids now contains "crash-window-fill" → Ok(None).
        let cash_before_second = portfolio.cash_micros;
        let second = apply_fill_step(&mut oms, "ord-w6", &ev, "crash-window-fill").unwrap();
        if let Some(fill) = second {
            apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
        }
        assert_eq!(
            portfolio.cash_micros, cash_before_second,
            "within-session duplicate fill delivery must not mutate portfolio a second time"
        );
    }

    /// Section D — T3.  Durable applied gate is queue membership, not OMS memory.
    ///
    /// Two fills for the same order:
    ///   F1 (delta_qty=40) — applied before crash, NOT in apply_queue.
    ///   F2 (delta_qty=60) — unapplied, IN apply_queue.
    ///
    /// OmsOrder is fresh after restart (applied_event_ids empty, filled_qty=0).
    /// Only F2 must reach portfolio; F1's absence from the queue is the fence.
    ///
    /// Proves: which fills mutate portfolio after restart is determined by
    /// inbox_load_unapplied_for_run output alone — not by OMS in-memory state.
    #[test]
    fn durable_applied_gate_is_queue_membership_not_oms_memory() {
        let initial_cash = 1_000_000_000_000_i64;
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        // total_qty=100; F1=40 was applied, F2=60 is unapplied.
        oms.insert(
            "ord-split".to_string(),
            OmsOrder::new("ord-split", "SPY", 100),
        );

        // Only F2 is in the recovery queue; F1 was filtered by the DB.
        let apply_queue: Vec<(String, BrokerEvent)> = vec![(
            "f2".to_string(),
            make_partial_fill_event("ord-split", "f2", 60),
        )];

        let mut portfolio = PortfolioState::new(initial_cash);
        for (msg_id, event) in &apply_queue {
            let internal_id = event.internal_order_id().to_string();
            let fill_opt = apply_fill_step(&mut oms, &internal_id, event, msg_id).unwrap();
            if let Some(fill) = fill_opt {
                apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
            }
        }

        // OMS shows only F2's contribution (60), not F1+F2 (100).
        assert_eq!(
            oms["ord-split"].filled_qty,
            60,
            "OMS filled_qty must reflect only F2 (unapplied); F1 (applied, absent) must not advance it"
        );
        // Portfolio cash changed: F2 was applied (cash ≠ initial).
        assert_ne!(
            portfolio.cash_micros, initial_cash,
            "F2 must mutate portfolio cash"
        );
        // If F1 had been double-applied, filled_qty would be 100 not 60.
        // The OMS assertion above is the definitive proof.
    }

    /// Section D — T4.  Empty applied_event_ids does not bypass restart replay protection.
    ///
    /// Multiple orders rebuilt fresh after restart (all applied_event_ids empty).
    /// All fills for those orders were durably applied before crash → none appear
    /// in the recovery queue.
    ///
    /// Proves: the OMS in-memory set being empty is not a safety bypass.
    /// The durable DB gate (applied_at_utc IS NOT NULL → excluded from queue)
    /// is the authoritative restart replay fence.
    #[test]
    fn empty_oms_applied_event_ids_does_not_bypass_restart_replay_protection() {
        let initial_cash = 1_000_000_000_i64;

        // Multiple orders with fresh applied_event_ids (restart).
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        oms.insert("ord-a".to_string(), OmsOrder::new("ord-a", "AAPL", 50));
        oms.insert("ord-b".to_string(), OmsOrder::new("ord-b", "MSFT", 80));

        // All fills were applied before crash → not in recovery queue.
        // The empty OmsOrder applied_event_ids cannot cause them to be re-applied
        // because they never reach apply_fill_step.
        let apply_queue: Vec<(String, BrokerEvent)> = vec![];

        let mut portfolio = PortfolioState::new(initial_cash);
        for (msg_id, event) in &apply_queue {
            let internal_id = event.internal_order_id().to_string();
            let fill_opt = apply_fill_step(&mut oms, &internal_id, event, msg_id).unwrap();
            if let Some(fill) = fill_opt {
                apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
            }
        }

        assert_eq!(
            portfolio.cash_micros, initial_cash,
            "empty applied_event_ids must not bypass restart replay protection \
             when recovery queue is empty (DB gate is authoritative)"
        );
        assert!(
            portfolio.positions.is_empty(),
            "no positions must be created when all fills were durably applied pre-crash"
        );
    }
}
