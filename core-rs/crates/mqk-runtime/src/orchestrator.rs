//! ExecutionOrchestrator - the single authoritative execution path.
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
use anyhow::{anyhow, Context as _};
use mqk_db::TimeSource;
use mqk_reconcile::{
    reconcile_monotonic, BrokerSnapshot, LocalSnapshot, SnapshotWatermark, StaleBrokerSnapshot,
};
use sqlx::types::chrono;
use sqlx::PgPool;
use std::collections::{BTreeMap, VecDeque};
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
const RUNTIME_LEASE_TTL_SECS: i64 = 15;
/// Maximum entries in the in-memory risk denial ring buffer.
///
/// Older entries are evicted when the cap is reached.  The buffer is
/// bounded to prevent unbounded memory growth in long-running sessions.
const DENIAL_RING_BUFFER_CAP: usize = 100;
use mqk_execution::oms::state_machine::OmsOrder;
use mqk_execution::{
    BrokerAdapter, BrokerError, BrokerEvent, BrokerGateway, BrokerOrderMap, IntegrityGate,
    ReconcileGate, RiskGate,
};
use mqk_portfolio::{apply_entry, LedgerEntry, PortfolioState};

mod apply;
mod cancel;
mod dispatch;
mod fill_quality;
mod lifecycle_events;
mod outbox;
use apply::*;
use fill_quality::build_fill_quality_row;
use lifecycle_events::build_lifecycle_event_row;
use outbox::*;
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
    /// A2: opaque broker adapter identifier used to scope the cursor in DB.
    adapter_id: String,
    /// A2: last-consumed broker event cursor; `None` = start from beginning.
    broker_cursor: Option<String>,
    /// FC-5: injected clock - no direct `Utc::now()` in the dispatch path.
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
    /// DB-backed runtime leader lease holder identity.
    runtime_holder_id: String,
    /// Current DB lease epoch held by this runtime, if any.
    runtime_epoch: Option<i64>,
    /// Lease TTL in seconds for renewals.
    runtime_lease_ttl_secs: i64,
    /// Monotonic watermark that prevents stale broker snapshots from relaxing
    /// reconcile state across ticks.
    reconcile_watermark: SnapshotWatermark,
    /// B2: last structured risk gate denial seen during a tick.
    ///
    /// `None` until the first `RiskGate::evaluate_gate()` denial is captured.
    /// Surfaced through the B4 observability snapshot as `SystemBlockState`
    /// with `reason_code = denial.reason_code()`.
    last_risk_denial: Option<mqk_execution::RiskDenial>,
    /// Bounded ring buffer of recent risk gate denial records.
    ///
    /// Capped at [`DENIAL_RING_BUFFER_CAP`] entries; oldest entry evicted when
    /// cap is reached.  Populated at every `RiskGate::evaluate_gate()` denial.
    /// Surfaced through the B4 observability snapshot as
    /// `ExecutionSnapshot::recent_risk_denials`.  Empty only when the gate has
    /// not denied any order since the execution loop started.
    recent_denials: VecDeque<crate::observability::RiskDenialRecord>,
}
#[derive(Debug)]
enum MonotonicReconcileError {
    Dirty,
    Stale(StaleBrokerSnapshot),
}
fn evaluate_monotonic_reconcile(
    reconcile_watermark: &mut SnapshotWatermark,
    local: &LocalSnapshot,
    broker: &BrokerSnapshot,
) -> Result<(), MonotonicReconcileError> {
    match reconcile_monotonic(reconcile_watermark, local, broker) {
        Ok(report) if report.is_clean() => Ok(()),
        Ok(_report) => Err(MonotonicReconcileError::Dirty),
        Err(stale) => Err(MonotonicReconcileError::Stale(stale)),
    }
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
        adapter_id: impl Into<String>,
        broker_cursor: Option<String>,
        time_source: TS,
        local_snapshot_provider: Box<dyn Fn() -> LocalSnapshot + Send + Sync>,
        broker_snapshot_provider: Box<dyn Fn() -> BrokerSnapshot + Send + Sync>,
    ) -> Self {
        let dispatcher_id = dispatcher_id.into();
        Self {
            pool,
            gateway,
            order_map,
            oms_orders,
            portfolio,
            run_id,
            dispatcher_id: dispatcher_id.clone(),
            adapter_id: adapter_id.into(),
            broker_cursor,
            time_source,
            local_snapshot_provider,
            broker_snapshot_provider,
            runtime_holder_id: derive_runtime_holder_id(&dispatcher_id, run_id),
            runtime_epoch: None,
            runtime_lease_ttl_secs: RUNTIME_LEASE_TTL_SECS,
            reconcile_watermark: SnapshotWatermark::new(),
            last_risk_denial: None,
            recent_denials: VecDeque::new(),
        }
    }
    /// Execute one orchestrator tick.
    ///
    /// Phases:
    /// 0. Halt guard - refuse tick if run is HALTED in DB (I9-1).
    /// 0b. Restart quarantine - refuse tick if ambiguous DISPATCHING / SENT
    ///     outbox rows exist (Patch 2).
    /// 0c. Reconcile drift enforcement - refuse tick and persist HALT/DISARM
    ///     if local vs broker snapshots are dirty (Patch 4A).
    /// 1. Dispatch claimed outbox rows via the gateway.
    /// 2. Fetch + ingest new broker events into oms_inbox.
    /// 3. Apply all unapplied inbox rows: OMS transition → portfolio apply →
    ///    capital invariant check (with halt+disarm persistence) → mark applied.
    ///
    /// Returns `Err` on any DB failure, gate refusal, OMS illegal transition,
    /// reconcile drift, or invariant violation.  The caller must treat any
    /// `Err` as a halt signal and stop the tick loop.
    pub async fn tick(&mut self) -> anyhow::Result<()> {
        // ------------------------------------------------------------------
        // Phase 0 - I9-1 HALT GUARD.
        //
        // Load run status from DB at the top of every tick.  If the run is
        // already HALTED (written by a prior tick or by a concurrent process),
        // refuse immediately - no outbox claim, no submit, no inbox apply.
        //
        // This ensures a persisted halt is honoured across crash+restart and
        // multi-instance scenarios where a second process calls tick() after
        // the first has already written HALTED.
        // ------------------------------------------------------------------
        {
            let run = mqk_db::fetch_run(&self.pool, self.run_id).await?;
            if matches!(run.status, mqk_db::RunStatus::Halted) {
                return Err(anyhow!(
                    "HALT_GUARD: run {} is HALTED - tick refused (I9-1)",
                    self.run_id
                ));
            }
        }
        // ------------------------------------------------------------------
        // Phase 0b - A4: restart quarantine for ambiguous outbox rows.
        //
        // Policy (A4):
        // - AMBIGUOUS    => BrokerError::AmbiguousSubmit was returned; outcome
        //                   definitively unknown. Never silently re-dispatch.
        // - DISPATCHING  => submit may have been attempted before crash; never
        //                   silently requeue.
        // - SENT (no map) => ambiguous only when broker-map evidence is missing.
        //
        // Without a broker-driven repair/reconcile path, the only safe behavior
        // is quarantine + halt/disarm before any new dispatch occurs.
        // ------------------------------------------------------------------
        {
            let ambiguous =
                mqk_db::outbox_load_restart_ambiguous_for_run(&self.pool, self.run_id).await?;
            if !ambiguous.is_empty() {
                let now = self.time_source.now_utc();
                // Mandatory halt + disarm - both writes must succeed before returning.
                // If either write fails the error propagates immediately so the caller
                // learns the persistence failure rather than silently losing the halt.
                // On success the Phase-0 HALT_GUARD will block any future tick() on
                // any orchestrator instance for this run_id.
                // A4: use "RecoveryQuarantine" (added in migration 0017 for this purpose).
                persist_halt_and_disarm(&self.pool, self.run_id, now, "RecoveryQuarantine").await?;
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
        self.refresh_or_acquire_runtime_leadership().await?;
        mqk_db::persist_risk_block_state(&self.pool, false, None, self.time_source.now_utc())
            .await?;
        // ------------------------------------------------------------------
        // Phase 0c - Patch 4A: reconcile drift enforcement.
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
            if let Err(err) =
                evaluate_monotonic_reconcile(&mut self.reconcile_watermark, &local, &broker)
            {
                let now = self.time_source.now_utc();
                // Mandatory halt + disarm - same fail-closed contract as Phase 0b.
                persist_halt_and_disarm(&self.pool, self.run_id, now, "ReconcileDrift").await?;
                return match err {
                    MonotonicReconcileError::Dirty => Err(anyhow!(
                        "RECONCILE_DRIFT: run {} halted and disarmed; dispatch refused",
                        self.run_id
                    )),
                    MonotonicReconcileError::Stale(stale) => Err(anyhow!(
                        "RECONCILE_SNAPSHOT_STALE: run {} halted and disarmed; dispatch refused: {}",
                        self.run_id,
                        stale
                    )),
                };
            }
        }
        // ------------------------------------------------------------------
        // Phase 1: Claim and dispatch outbox rows.
        // ------------------------------------------------------------------
        self.refresh_or_acquire_runtime_leadership().await?;
        let claimed = mqk_db::outbox_claim_batch_for_run(
            &self.pool,
            self.run_id,
            1,
            &self.dispatcher_id,
            self.time_source.now_utc(),
        )
        .await?;
        for claimed_row in claimed {
            self.refresh_or_acquire_runtime_leadership().await?;
            match build_claimed_outbox_request(&claimed_row.row)? {
                ClaimedOutboxRequest::Submit(req) => {
                    self.dispatch_submit_claimed_outbox_row(claimed_row, req)
                        .await?;
                }
                ClaimedOutboxRequest::Cancel { target_order_id } => {
                    self.dispatch_cancel_claimed_outbox_row(claimed_row, target_order_id)
                        .await?;
                }
            }
        }
        // ------------------------------------------------------------------
        // Phase 2: Fetch broker events and ingest into oms_inbox.
        //
        // A2: pass the current cursor so the adapter resumes from its last
        // acknowledged position.  Crash-safe ordering:
        //   1. inbox_insert_deduped for every event  (dedup key = broker_message_id)
        //   2. advance_broker_cursor                 (only after all inserts succeed)
        // If the process crashes between (1) and (2), the next tick re-fetches
        // from the old cursor; the inbox unique constraint silently discards
        // duplicates, so no event is double-applied.
        // ------------------------------------------------------------------
        self.refresh_or_acquire_runtime_leadership().await?;
        let (events, new_cursor) = match self.gateway.fetch_events(self.broker_cursor.as_deref()) {
            Ok(batch) => batch,
            Err(err) => {
                if let Some(cursor) = err.persist_cursor() {
                    let cursor = cursor.to_string();
                    let now = self.time_source.now_utc();
                    mqk_db::advance_broker_cursor(&self.pool, &self.adapter_id, &cursor, now)
                        .await?;
                    self.broker_cursor = Some(cursor);
                }
                // BRK-00R-03: when the adapter signals a continuity failure, derive
                // the runtime-owned continuity state from the (now-updated) cursor
                // and name the failure explicitly.  This is the orchestrator's own
                // ownership of the continuity decision, independent of adapter
                // internals.  For non-Alpaca adapters `check_alpaca_ws_continuity...`
                // returns `None`, which is included in the error for transparency.
                if matches!(err, BrokerError::InboundContinuityUnproven { .. }) {
                    let continuity =
                        crate::alpaca_inbound::check_alpaca_ws_continuity_from_opaque_cursor(
                            self.broker_cursor.as_deref(),
                        );
                    return Err(anyhow!(
                        "WS_CONTINUITY_UNPROVEN: tick refused by runtime-owned gate; \
                         continuity={:?}; adapter_detail={}",
                        continuity,
                        err
                    ));
                }
                return Err(anyhow!("fetch_events failed: {}", err));
            }
        };
        for event in &events {
            let msg_json = serde_json::to_value(event)?;
            let event_kind = match event {
                BrokerEvent::Ack { .. } => "ack",
                BrokerEvent::PartialFill { .. } => "partial_fill",
                BrokerEvent::Fill { .. } => "fill",
                BrokerEvent::CancelAck { .. } => "cancel_ack",
                BrokerEvent::CancelReject { .. } => "cancel_reject",
                BrokerEvent::ReplaceAck { .. } => "replace_ack",
                BrokerEvent::ReplaceReject { .. } => "replace_reject",
                BrokerEvent::Reject { .. } => "reject",
            };
            let now = self.time_source.now_utc();
            let _inserted = mqk_db::inbox_insert_deduped_with_identity(
                &self.pool,
                self.run_id,
                event.broker_message_id(),
                event.broker_fill_id(),
                event.internal_order_id(),
                event.broker_order_id().unwrap_or(event.internal_order_id()),
                event_kind,
                &msg_json,
                0,
                now,
            )
            .await?;
        }
        // Advance cursor only after all inbox persists succeed (crash-safe).
        if let Some(ref cursor) = new_cursor {
            let now = self.time_source.now_utc();
            mqk_db::advance_broker_cursor(&self.pool, &self.adapter_id, cursor, now).await?;
            self.broker_cursor = new_cursor;
        }
        // ------------------------------------------------------------------
        // Phase 3: Apply all unapplied inbox rows.
        //
        // SECTION D - Durable restart replay gate.
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
        self.refresh_or_acquire_runtime_leadership().await?;
        let unapplied = mqk_db::inbox_load_unapplied_for_run(&self.pool, self.run_id).await?;
        // Phase 3a: canonical key is durable inbox ingest order (`inbox_id ASC`),
        // not `broker_message_id`. Message IDs remain dedupe identity only.
        let apply_queue = build_canonical_apply_queue(unapplied)?;

        // Phase 3b: apply in canonical order.
        for (_inbox_id, msg_id, event, fill_received_at_utc) in apply_queue {
            self.refresh_or_acquire_runtime_leadership().await?;
            let internal_id = event.internal_order_id().to_string();
            // Steps 6+7: OMS context guard → portfolio apply (Section C).
            //
            // apply_fill_step enforces that fill events cannot reach portfolio
            // without a proven OMS order context in memory.  On Err (unknown-
            // order fill OR illegal OMS transition), halt the run and disarm
            // before propagating - same pattern as capital invariant violations.
            let apply_outcome = match apply_broker_event_step(
                &mut self.oms_orders,
                &internal_id,
                &event,
                &msg_id,
            ) {
                Ok(outcome) => outcome,
                Err(e) => {
                    let now = self.time_source.now_utc();
                    // Mandatory halt + disarm before surfacing the OMS error.
                    // If the DB writes fail their error takes precedence - failing
                    // to persist HALTED is more dangerous than the OMS fault itself.
                    persist_halt_and_disarm(&self.pool, self.run_id, now, "IntegrityViolation")
                        .await?;
                    return Err(e.context(format!(
                        "UNKNOWN_ORDER_FILL: run {} halted and disarmed (Section C)",
                        self.run_id
                    )));
                }
            };
            // RT-9: Phase 3b - when a live broker Ack carries the exchange-assigned
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
            // Apply portfolio fill - only if apply_fill_step returned Some(fill).
            // None is returned for: non-fill events, non-fill events for unknown
            // orders, and no-op replays (duplicate event_id or late fill on a
            // terminal OMS order where filled_qty did not advance).
            if let Some(fill) = apply_outcome.fill {
                apply_entry(&mut self.portfolio, LedgerEntry::Fill(fill));
            }
            // Step 8: assert capital invariants - I9-1 persistence requirement.
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
            // EXE-03R: terminal lifecycle events must clear broker-order mappings
            // before the inbox row is marked applied. If a crash occurs after
            // mark_applied but before cleanup, the durable inbox fence would
            // suppress replay and strand a stale broker mapping permanently.
            if apply_outcome.terminal_apply_succeeded {
                mqk_db::broker_map_remove(&self.pool, &internal_id).await?;
                remove_broker_mapping_from_memory(&mut self.order_map, &internal_id);
            }
            // TV-EXEC-01: best-effort fill-quality telemetry write.
            // Only emitted for Fill/PartialFill events — no fabrication for
            // non-fill events. Failure is non-fatal (logged and swallowed) so
            // that telemetry errors cannot corrupt the primary execution path.
            if let Some(telemetry_row) = build_fill_quality_row(
                self.run_id,
                &msg_id,
                &event,
                fill_received_at_utc,
                &self.pool,
                self.time_source.now_utc(),
            )
            .await
            {
                if let Err(e) =
                    mqk_db::insert_fill_quality_telemetry(&self.pool, &telemetry_row).await
                {
                    tracing::warn!(
                        run_id = %self.run_id,
                        broker_message_id = %msg_id,
                        error = %e,
                        "TV-EXEC-01: fill_quality_telemetry write failed (non-fatal)"
                    );
                }
            }
            // EXEC-02: best-effort lifecycle event write for cancel/replace events.
            // Emitted for CancelAck, ReplaceAck, CancelReject, ReplaceReject only.
            // Failure is non-fatal so that telemetry errors cannot block the
            // primary execution path.
            if let Some(lc_row) =
                build_lifecycle_event_row(self.run_id, &msg_id, &event, self.time_source.now_utc())
            {
                if let Err(e) = mqk_db::insert_order_lifecycle_event(&self.pool, &lc_row).await {
                    tracing::warn!(
                        run_id = %self.run_id,
                        broker_message_id = %msg_id,
                        error = %e,
                        "EXEC-02: order_lifecycle_event write failed (non-fatal)"
                    );
                }
            }
            // Step 9: commit - mark the inbox row as applied.
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
    async fn capture_risk_denial(
        &mut self,
        symbol: &str,
        denial: &mqk_execution::RiskDenial,
    ) -> anyhow::Result<()> {
        self.last_risk_denial = Some(denial.clone());
        let denied_at = self.time_source.now_utc();
        let record = crate::observability::RiskDenialRecord {
            id: format!("{}:{}", denied_at.timestamp_micros(), denial.reason_code()),
            denied_at_utc: denied_at,
            rule: denial.reason_code().to_string(),
            message: denial.reason_summary().to_string(),
            symbol: Some(symbol.to_string()),
            requested_qty: denial.evidence.requested_qty,
            limit: denial.evidence.limit,
            severity: "critical".to_string(),
        };
        if let Err(err) = mqk_db::persist_risk_denial_event(
            &self.pool,
            &mqk_db::RiskDenialEventRow {
                id: record.id.clone(),
                denied_at_utc: record.denied_at_utc,
                rule: record.rule.clone(),
                message: record.message.clone(),
                symbol: record.symbol.clone(),
                requested_qty: record.requested_qty,
                limit_qty: record.limit,
                severity: record.severity.clone(),
            },
        )
        .await
        {
            tracing::warn!(
                "risk_denial_event_persist_failed id={} err={err}",
                record.id
            );
        }
        if self.recent_denials.len() >= DENIAL_RING_BUFFER_CAP {
            self.recent_denials.pop_front();
        }
        self.recent_denials.push_back(record);
        mqk_db::persist_risk_block_state(
            &self.pool,
            true,
            Some(denial.reason_code()),
            self.time_source.now_utc(),
        )
        .await?;
        Ok(())
    }

    /// B4: Collect a read-only execution pipeline snapshot.
    ///
    /// Fetches outbox / inbox / run / arm state from the DB, then overlays the
    /// in-memory OMS order map and portfolio. Entirely read-only — does not
    /// modify any execution state or affect `tick()` semantics.
    ///
    /// Takes `&mut self` so that the spawned future is `Send` without
    /// requiring the gate/adapter type parameters to implement `Sync`.
    /// All in-memory data is extracted synchronously before the first `.await`.
    ///
    /// The timestamp is sourced from `self.time_source` — no direct
    /// `Utc::now()` call ([T]-guard compliant).
    pub async fn snapshot(&mut self) -> anyhow::Result<crate::observability::ExecutionSnapshot> {
        // Extract everything needed from `self` synchronously, before any `.await`,
        // so `&mut self` is not live across a suspension point.
        let now = self.time_source.now_utc();
        let run_id = self.run_id;
        let pool = self.pool.clone(); // PgPool is Clone (Arc internally)
        let active_orders =
            crate::observability::build_order_snapshots(&self.oms_orders, &self.order_map);
        let portfolio = crate::observability::build_portfolio_snapshot(&self.portfolio);
        // B2: extract before await so no borrow of self crosses the suspension point.
        let last_risk_denial = self.last_risk_denial.clone();
        // Denial ring buffer: snapshot is a Vec so it is cheap to clone for the
        // observer; the VecDeque is not moved.
        let recent_denials: Vec<crate::observability::RiskDenialRecord> =
            self.recent_denials.iter().cloned().collect();
        // `self` is no longer borrowed here - safe to `.await` without Sync.
        let mut snap = crate::observability::collect_db_snapshot(&pool, run_id, now).await?;
        snap.active_orders = active_orders;
        snap.portfolio = portfolio;
        snap.recent_risk_denials = recent_denials;
        // B2: overlay risk denial if no higher-priority block state already exists.
        //
        // Priority (matches gateway evaluation order):
        //   HALTED_IN_DB      - built by collect_db_snapshot (highest)
        //   INTEGRITY_DISARMED - built by collect_db_snapshot
        //   RISK_BLOCKED       - overlaid here (lowest)
        if snap.system_block_state.is_none() {
            if let Some(denial) = last_risk_denial {
                snap.system_block_state = Some(crate::observability::SystemBlockState {
                    reason_code: denial.reason_code().to_string(),
                    reason_summary: denial.reason_summary().to_string(),
                    evidence: denial.evidence.to_kv_pairs(),
                });
            }
        }
        Ok(snap)
    }
    pub async fn release_runtime_leadership(&mut self) -> anyhow::Result<()> {
        let Some(epoch) = self.runtime_epoch.take() else {
            return Ok(());
        };
        mqk_db::runtime_lease::release_lease(&self.pool, &self.runtime_holder_id, epoch).await
    }
    async fn refresh_or_acquire_runtime_leadership(&mut self) -> anyhow::Result<()> {
        let now = self.time_source.now_utc();
        if let Some(epoch) = self.runtime_epoch {
            match mqk_db::runtime_lease::refresh_lease(
                &self.pool,
                &self.runtime_holder_id,
                epoch,
                now,
                self.runtime_lease_ttl_secs,
            )
            .await
            {
                Ok(lease) => {
                    self.runtime_epoch = Some(lease.epoch);
                    return Ok(());
                }
                Err(err) => {
                    self.runtime_epoch = None;
                    persist_halt_and_disarm(&self.pool, self.run_id, now, "LeaderLeaseLost")
                        .await?;
                    return Err(anyhow!(
                        "RUNTIME_LEASE_LOST: run {} holder={} epoch={} error={}",
                        self.run_id,
                        self.runtime_holder_id,
                        epoch,
                        err
                    ));
                }
            }
        }
        match mqk_db::runtime_lease::acquire_lease(
            &self.pool,
            &self.runtime_holder_id,
            now,
            self.runtime_lease_ttl_secs,
        )
        .await?
        {
            mqk_db::runtime_lease::LeaseAcquireOutcome::Acquired(lease) => {
                self.runtime_epoch = Some(lease.epoch);
                Ok(())
            }
            mqk_db::runtime_lease::LeaseAcquireOutcome::HeldByOther(current) => {
                persist_halt_and_disarm(&self.pool, self.run_id, now, "LeaderLeaseUnavailable")
                    .await?;
                Err(anyhow!(
                    "RUNTIME_LEASE_UNAVAILABLE: run {} refused holder={} current_holder={} current_epoch={} expires_at={}",
                    self.run_id,
                    self.runtime_holder_id,
                    current.holder_id,
                    current.epoch,
                    current.lease_expires_at.to_rfc3339(),
                ))
            }
        }
    }
}
fn derive_runtime_holder_id(dispatcher_id: &str, run_id: Uuid) -> String {
    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "UNKNOWN_HOST".to_string());
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "UNKNOWN_USER".to_string());
    format!(
        "{}|{}|{}|pid={}|run={}",
        dispatcher_id,
        host,
        user,
        std::process::id(),
        run_id
    )
}
// ---------------------------------------------------------------------------
// Internal helper - mandatory halt + disarm persistence
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
            "HALT_PERSISTENCE_FAILURE: run {run_id} - runs.status=HALTED could not be \
                 written (reason={reason}); Phase-0 halt guard on restart is NOT guaranteed"
        )
    })?;
    mqk_db::persist_arm_state(pool, "DISARMED", Some(reason))
        .await
        .with_context(|| {
            format!(
                "ARM_STATE_PERSISTENCE_FAILURE: run {run_id} - sys_arm_state=DISARMED could \
                 not be written (reason={reason}); runs.status=HALTED was persisted"
            )
        })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests;
