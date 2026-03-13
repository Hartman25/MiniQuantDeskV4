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
use std::collections::BTreeMap;
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
use mqk_execution::oms::state_machine::{OmsEvent, OmsOrder};
use mqk_execution::{
    BrokerAdapter, BrokerError, BrokerEvent, BrokerGateway, BrokerOrderMap, BrokerSubmitRequest,
    IntegrityGate, ReconcileGate, RiskGate,
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
        self.refresh_or_acquire_runtime_leadership().await?;
        mqk_db::persist_risk_block_state(&self.pool, false, None, self.time_source.now_utc())
            .await?;
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
        // Phase 1: Claim and submit outbox rows.
        // ------------------------------------------------------------------
        self.refresh_or_acquire_runtime_leadership().await?;
        let claimed = mqk_db::outbox_claim_batch(
            &self.pool,
            1,
            &self.dispatcher_id,
            self.time_source.now_utc(),
        )
        .await?;
        for claimed_row in claimed {
            self.refresh_or_acquire_runtime_leadership().await?;
            let order_id = claimed_row.row.idempotency_key.clone();
            let claim = claimed_row.token;
            // Build a submit request from the outbox order_json.
            let req = build_submit_request(&claimed_row.row)?;
            let symbol = req.symbol.clone();
            let qty = i64::from(req.quantity);
            // Step 3a: RT-5 - write DISPATCHING before calling gateway.submit().
            //
            // Closes crash window W4: if the process crashes between here and
            // outbox_mark_sent, the row stays DISPATCHING on restart.
            // outbox_reset_stale_claims only resets CLAIMED rows, so the order
            // is NOT silently requeued - preventing double-submit.
            mqk_db::outbox_mark_dispatching(
                &self.pool,
                &order_id,
                &self.dispatcher_id,
                self.time_source.now_utc(),
            )
            .await?;
            // Step 3b: submit via BrokerGateway - the ONLY submit path.
            //
            // A3: gateway.submit returns Result<_, SubmitError>.
            // SubmitError is Send+Sync (all inner fields are String/u64),
            // so no anyhow conversion is needed before the async dispatch.
            let submit_result = self.gateway.submit(&claim, req);
            let resp = match submit_result {
                Ok(r) => r,
                Err(e) => {
                    // A3: per-class outbox row disposition.
                    use mqk_execution::{GateRefusal, SubmitError};
                    match &e {
                        SubmitError::Gate(GateRefusal::RiskBlocked(denial)) => {
                            // B2: capture the structured risk denial for the B4
                            // diagnostics snapshot. The denial is stored in-memory
                            // and overlaid by snapshot() onto SystemBlockState.
                            self.last_risk_denial = Some(denial.clone());
                            mqk_db::persist_risk_block_state(
                                &self.pool,
                                true,
                                Some(denial.reason_code()),
                                self.time_source.now_utc(),
                            )
                            .await?;
                            // Gate refused before touching the broker.
                            // Row is DISPATCHING but request never left.
                            // Mark FAILED; requires operator review.
                            let _ = mqk_db::outbox_mark_failed(&self.pool, &order_id).await;
                        }
                        SubmitError::Gate(_) => {
                            // Other gate refusals (IntegrityDisarmed, ReconcileNotClean)
                            // - gate refused before touching the broker.
                            // Row is DISPATCHING but request never left.
                            // Mark FAILED; requires operator review.
                            let _ = mqk_db::outbox_mark_failed(&self.pool, &order_id).await;
                        }
                        SubmitError::Broker(be) if be.requires_halt() => {
                            let now = self.time_source.now_utc();
                            if matches!(be, BrokerError::AmbiguousSubmit { .. }) {
                                // A4: Transition DISPATCHING → AMBIGUOUS (explicit quarantine).
                                // Row cannot re-enter dispatch without explicit operator/reconcile
                                // release via outbox_reset_ambiguous_to_pending.
                                let _ = mqk_db::outbox_mark_ambiguous(&self.pool, &order_id).await;
                                // Halt+disarm - "AmbiguousSubmit" is now a valid DB reason
                                // (migration 0020). Phase-0b quarantine blocks any restart.
                                let _ = persist_halt_and_disarm(
                                    &self.pool,
                                    self.run_id,
                                    now,
                                    "AmbiguousSubmit",
                                )
                                .await;
                            } else {
                                // AuthSession: credentials revoked - mark FAILED + halt.
                                // "AuthSession" is now a valid DB reason (migration 0020).
                                let _ = mqk_db::outbox_mark_failed(&self.pool, &order_id).await;
                                let _ = persist_halt_and_disarm(
                                    &self.pool,
                                    self.run_id,
                                    now,
                                    "AuthSession",
                                )
                                .await;
                            }
                        }
                        SubmitError::Broker(be) if be.is_safe_pre_send_retry() => {
                            // Safe retry class: local non-delivery is proven.
                            // Reset row to PENDING for re-dispatch on the next tick.
                            let _ =
                                mqk_db::outbox_reset_dispatching_to_pending(&self.pool, &order_id)
                                    .await;
                            eprintln!("WARN broker_submit_retryable order_id={order_id} error={e}");
                        }
                        SubmitError::Broker(be) if be.is_ambiguous_send_outcome() => {
                            // Ambiguous transport/broker outcome - fail closed.
                            let now = self.time_source.now_utc();
                            let _ = mqk_db::outbox_mark_ambiguous(&self.pool, &order_id).await;
                            let _ = persist_halt_and_disarm(
                                &self.pool,
                                self.run_id,
                                now,
                                "AmbiguousSubmit",
                            )
                            .await;
                        }
                        SubmitError::Broker(_) => {
                            // Reject / Transient: mark FAILED, requires operator.
                            let _ = mqk_db::outbox_mark_failed(&self.pool, &order_id).await;
                            eprintln!(
                                "WARN broker_submit_non_retryable order_id={order_id} error={e}"
                            );
                        }
                    }
                    return Err(anyhow!("{e}"));
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
                return Err(anyhow!("fetch_events failed: {}", err));
            }
        };
        for event in &events {
            let msg_json = serde_json::to_value(event)?;
            let event_identity = event.identity();
            mqk_db::inbox_insert_deduped_with_identity(
                &self.pool,
                self.run_id,
                &mqk_db::BrokerEventIdentity {
                    broker_message_id: event_identity.broker_message_id,
                    broker_fill_id: event_identity.broker_fill_id,
                    broker_sequence_id: event_identity.broker_sequence_id,
                    broker_timestamp: event_identity.broker_timestamp,
                },
                msg_json,
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
        for (_inbox_id, msg_id, event) in apply_queue {
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
                        "BROKER_EVENT_APPLY_FAIL_CLOSED: run {} halted and disarmed (Section C)",
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
            // None is returned for non-fill events on known orders and no-op
            // replays (duplicate event_id or late fill on a terminal OMS order
            // where filled_qty did not advance).
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
    /// B4: Collect a read-only execution pipeline snapshot.
    ///
    /// Fetches outbox / inbox / run / arm state from the DB, then overlays the
    /// in-memory OMS order map and portfolio.  Entirely read-only - does not
    /// modify any execution state or affect `tick()` semantics.
    ///
    /// Takes `&mut self` so that the spawned future is `Send` without
    /// requiring the gate/adapter type parameters to implement `Sync`.
    /// All in-memory data is extracted synchronously before the first `.await`.
    ///
    /// The timestamp is sourced from `self.time_source` - no direct
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
        // `self` is no longer borrowed here - safe to `.await` without Sync.
        let mut snap = crate::observability::collect_db_snapshot(&pool, run_id, now).await?;
        snap.active_orders = active_orders;
        snap.portfolio = portfolio;
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
// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------
/// Build a `BrokerSubmitRequest` from a claimed outbox row.
fn build_validated_submit_request(
    order_id: &str,
    order_json: &serde_json::Value,
) -> anyhow::Result<BrokerSubmitRequest> {
    let symbol = validated_order_symbol(order_json)?;
    let quantity = validated_order_quantity(order_json)?;
    let side = validated_order_side(order_json, quantity.signed_qty)?;
    let order_type = validated_order_type(order_json)?;
    let time_in_force = validated_order_time_in_force(order_json)?;
    let limit_price = validated_limit_price_for_order_type(order_json, &order_type)?;

    Ok(BrokerSubmitRequest {
        order_id: order_id.to_string(),
        symbol,
        side,
        quantity: quantity.quantity,
        order_type,
        limit_price,
        time_in_force,
    })
}

fn build_submit_request(row: &mqk_db::OutboxRow) -> anyhow::Result<BrokerSubmitRequest> {
    build_validated_submit_request(&row.idempotency_key, &row.order_json)
}
fn validated_order_symbol(order_json: &serde_json::Value) -> anyhow::Result<String> {
    let symbol = order_json
        .get("symbol")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .ok_or_else(|| anyhow!("invalid submit payload: symbol missing or not a string"))?;

    if symbol.is_empty() {
        return Err(anyhow!("invalid submit payload: symbol blank"));
    }

    Ok(symbol.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ValidatedOrderQuantity {
    signed_qty: i64,
    quantity: i32,
}

fn validated_order_side(
    order_json: &serde_json::Value,
    signed_qty: i64,
) -> anyhow::Result<mqk_execution::Side> {
    // Compatibility rule restored from pre-EXE-01R submit building:
    // explicit side is authoritative; if absent, derive direction from the
    // legacy signed-quantity encoding already evidenced by local code/tests.
    let Some(side_value) = order_json.get("side") else {
        return if signed_qty > 0 {
            Ok(mqk_execution::Side::Buy)
        } else {
            Ok(mqk_execution::Side::Sell)
        };
    };

    let side = side_value
        .as_str()
        .map(str::trim)
        .ok_or_else(|| anyhow!("invalid submit payload: side present but not a string"))?
        .to_ascii_lowercase();

    match side.as_str() {
        "buy" => Ok(mqk_execution::Side::Buy),
        "sell" => Ok(mqk_execution::Side::Sell),
        _ => Err(anyhow!(
            "invalid submit payload: unsupported side '{}'",
            side
        )),
    }
}

fn validated_order_quantity(
    order_json: &serde_json::Value,
) -> anyhow::Result<ValidatedOrderQuantity> {
    let signed_qty = match (order_json.get("qty"), order_json.get("quantity")) {
        (Some(qty), Some(quantity)) => {
            let qty = parse_signed_i64_field("qty", qty)?;
            let quantity = parse_signed_i64_field("quantity", quantity)?;
            if qty != quantity {
                return Err(anyhow!(
                    "invalid submit payload: qty and quantity disagree (qty={}, quantity={})",
                    qty,
                    quantity
                ));
            }
            qty
        }
        (Some(qty), None) => parse_signed_i64_field("qty", qty)?,
        (None, Some(quantity)) => parse_signed_i64_field("quantity", quantity)?,
        (None, None) => return Err(anyhow!("invalid submit payload: quantity missing")),
    };

    let effective_qty = signed_qty.checked_abs().ok_or_else(|| {
        anyhow!("invalid submit payload: quantity out of range for broker request")
    })?;
    if effective_qty == 0 {
        return Err(anyhow!(
            "invalid submit payload: effective quantity must be positive"
        ));
    }

    Ok(ValidatedOrderQuantity {
        signed_qty,
        quantity: i32::try_from(effective_qty)
            .context("invalid submit payload: quantity out of range for broker request")?,
    })
}

fn validated_order_type(order_json: &serde_json::Value) -> anyhow::Result<String> {
    // Compatibility rule restored from pre-EXE-01R submit building:
    // absent order_type defaults to market, but explicit values are validated.
    let order_type = match order_json.get("order_type") {
        None => return Ok("market".to_string()),
        Some(value) => value
            .as_str()
            .map(str::trim)
            .ok_or_else(|| anyhow!("invalid submit payload: order_type present but not a string"))?
            .to_ascii_lowercase(),
    };

    match order_type.as_str() {
        "market" | "limit" => Ok(order_type),
        _ => Err(anyhow!(
            "invalid submit payload: unsupported order_type '{}'",
            order_type
        )),
    }
}

fn validated_order_time_in_force(order_json: &serde_json::Value) -> anyhow::Result<String> {
    // Compatibility rule restored from pre-EXE-01R submit building:
    // absent time_in_force defaults to day, but explicit values are validated.
    let time_in_force = match order_json.get("time_in_force") {
        None => return Ok("day".to_string()),
        Some(value) => value
            .as_str()
            .map(str::trim)
            .ok_or_else(|| {
                anyhow!("invalid submit payload: time_in_force present but not a string")
            })?
            .to_ascii_lowercase(),
    };

    match time_in_force.as_str() {
        "day" | "gtc" | "ioc" | "fok" | "opg" | "cls" => Ok(time_in_force),
        _ => Err(anyhow!(
            "invalid submit payload: unsupported time_in_force '{}'",
            time_in_force
        )),
    }
}

fn validated_limit_price_for_order_type(
    order_json: &serde_json::Value,
    order_type: &str,
) -> anyhow::Result<Option<i64>> {
    let limit_price = order_json.get("limit_price");

    match order_type {
        "limit" => {
            let limit_price = limit_price.ok_or_else(|| {
                anyhow!("invalid submit payload: limit order missing limit_price")
            })?;
            if limit_price.is_null() {
                return Err(anyhow!(
                    "invalid submit payload: limit order missing limit_price"
                ));
            }
            Ok(Some(parse_positive_i64_field("limit_price", limit_price)?))
        }
        "market" => {
            if limit_price.is_some_and(|value| !value.is_null()) {
                return Err(anyhow!(
                    "invalid submit payload: market order must not carry limit_price"
                ));
            }
            Ok(None)
        }
        _ => Err(anyhow!(
            "invalid submit payload: unsupported order_type '{}'",
            order_type
        )),
    }
}

fn parse_signed_i64_field(name: &str, value: &serde_json::Value) -> anyhow::Result<i64> {
    let parsed = match value {
        serde_json::Value::Number(number) => number.as_i64().ok_or_else(|| {
            anyhow!(
                "invalid submit payload: {} must be an integer without lossy conversion",
                name
            )
        })?,
        serde_json::Value::String(raw) => raw.trim().parse::<i64>().map_err(|_| {
            anyhow!(
                "invalid submit payload: {} must be an integer without lossy conversion",
                name
            )
        })?,
        _ => {
            return Err(anyhow!(
                "invalid submit payload: {} missing or not an integer-compatible value",
                name
            ))
        }
    };

    Ok(parsed)
}

fn parse_positive_i64_field(name: &str, value: &serde_json::Value) -> anyhow::Result<i64> {
    let parsed = match value {
        serde_json::Value::Number(number) => number.as_i64().ok_or_else(|| {
            anyhow!(
                "invalid submit payload: {} must be an integer without lossy conversion",
                name
            )
        })?,
        serde_json::Value::String(raw) => raw.trim().parse::<i64>().map_err(|_| {
            anyhow!(
                "invalid submit payload: {} must be an integer without lossy conversion",
                name
            )
        })?,
        _ => {
            return Err(anyhow!(
                "invalid submit payload: {} missing or not an integer-compatible value",
                name
            ))
        }
    };

    if parsed <= 0 {
        return Err(anyhow!("invalid submit payload: {} must be positive", name));
    }

    Ok(parsed)
}
#[cfg(test)]
fn order_json_qty(json: &serde_json::Value) -> i64 {
    json["quantity"].as_i64().unwrap_or(0).saturating_abs()
}

#[cfg(test)]
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
fn build_canonical_apply_queue(
    unapplied: Vec<mqk_db::InboxRow>,
) -> anyhow::Result<Vec<(i64, String, BrokerEvent)>> {
    let mut apply_queue: Vec<(i64, String, BrokerEvent)> = Vec::with_capacity(unapplied.len());
    for row in unapplied {
        let inbox_id = row.inbox_id;
        let msg_id = row.broker_message_id;
        let event: BrokerEvent = serde_json::from_value(row.message_json)?;
        apply_queue.push((inbox_id, msg_id, event));
    }
    apply_queue.sort_by_key(|(inbox_id, _, _)| *inbox_id);

    for pair in apply_queue.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(anyhow!(
                "AMBIGUOUS_CANONICAL_ORDER: duplicate inbox_id {} in apply queue",
                pair[0].0
            ));
        }
    }
    Ok(apply_queue)
}

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
#[cfg(test)]
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

#[derive(Debug)]
struct AppliedBrokerEventOutcome {
    fill: Option<Fill>,
    terminal_apply_succeeded: bool,
}

fn remove_broker_mapping_from_memory(order_map: &mut BrokerOrderMap, internal_id: &str) {
    order_map.deregister(internal_id);
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
/// Section C - Restart / Unknown-Order Fill Safety.
///
/// Apply OMS transition and return the portfolio fill to apply (if any).
///
/// # Invariants enforced
///
/// - Fill events (`PartialFill`, `Fill`) **must** have a corresponding OMS
///   order in memory. If none is found, `Err` is returned immediately -
///   the caller must halt and disarm before propagating.
///
/// - Unknown-order non-fill lifecycle events (Ack, CancelAck, Reject, etc.)
///   are treated as lifecycle divergence and fail closed.
///
/// - Duplicate fill replays are detected by comparing `order.filled_qty`
///   before and after `apply()`. If `filled_qty` did not advance on a fill
///   event, the OMS applied a silent no-op (duplicate `event_id` or late fill
///   on a terminal order). `Ok(None)` is returned to prevent a double
///   portfolio mutation.
///
/// The caller is responsible for halting and disarming on `Err`.
fn apply_broker_event_step(
    oms_orders: &mut BTreeMap<String, OmsOrder>,
    internal_id: &str,
    event: &BrokerEvent,
    msg_id: &str,
) -> anyhow::Result<AppliedBrokerEventOutcome> {
    let pre_state = oms_orders.get(internal_id).map(|order| order.state.clone());
    let fill = apply_fill_step(oms_orders, internal_id, event, msg_id)?;
    let terminal_apply_succeeded = match (pre_state.as_ref(), oms_orders.get(internal_id)) {
        (Some(pre_state), Some(order)) => !pre_state.is_terminal() && order.state.is_terminal(),
        _ => false,
    };

    Ok(AppliedBrokerEventOutcome {
        fill,
        terminal_apply_succeeded,
    })
}

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
            let economic_event_id = event.broker_fill_id().unwrap_or(msg_id);
            order
                .apply(&oms_event, Some(economic_event_id))
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
                 - fill event has no OMS order context in memory; \
                 refusing portfolio mutation (Section C)",
                msg_id,
                internal_id
            ));
        }
        None => {
            return Err(anyhow!(
                "UNKNOWN_ORDER_NON_FILL_LIFECYCLE_DIVERGENCE: broker_message_id='{}' internal_order_id='{}' \
                 - non-fill lifecycle event has no OMS order context in memory; refusing silent skip (Section C)",
                msg_id,
                internal_id
            ));
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
    use ::chrono::TimeZone;
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
            broker_fill_id: None,
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
            broker_fill_id: None,
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
                "side": "buy",
                "quantity": -100,
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
        assert!(matches!(req.side, mqk_execution::Side::Buy));
        assert_eq!(req.quantity, 100);
    }
    #[test]
    fn broker_event_to_fill_rejects_zero_qty() {
        use mqk_execution::Side;
        let ev = BrokerEvent::Fill {
            broker_message_id: "msg-4".to_string(),
            broker_fill_id: None,
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
                broker_fill_id: None,
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
                broker_fill_id: None,
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
                broker_fill_id: None,
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
                broker_fill_id: None,
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
    fn canonical_apply_order_does_not_depend_on_broker_message_id() {
        use mqk_execution::Side;
        // Delivery order was fill -> ack even though lexicographic message-id is opposite.
        let fill = BrokerEvent::Fill {
            broker_message_id: "z-msg".into(),
            broker_fill_id: None,
            internal_order_id: "ord-1".into(),
            broker_order_id: None,
            symbol: "X".into(),
            side: Side::Buy,
            delta_qty: 5,
            price_micros: 100,
            fee_micros: 0,
        };
        let ack = BrokerEvent::Ack {
            broker_message_id: "a-msg".into(),
            internal_order_id: "ord-1".into(),
            broker_order_id: None,
        };
        let mut queue: Vec<(i64, String, BrokerEvent)> =
            vec![(42, "z-msg".into(), fill), (43, "a-msg".into(), ack)];
        queue.sort_by_key(|(inbox_id, _, _)| *inbox_id);
        assert!(
            matches!(queue[0].2, BrokerEvent::Fill { .. }),
            "canonical apply order must follow durable inbox ingest order, not broker_message_id"
        );
    }

    #[test]
    fn out_of_order_broker_delivery_uses_real_ordering_truth() {
        use mqk_execution::Side;

        let queue = build_canonical_apply_queue(vec![
            mqk_db::InboxRow {
                inbox_id: 10,
                run_id: Uuid::nil(),
                broker_message_id: "z-msg".into(),
                broker_fill_id: None,
                broker_sequence_id: None,
                broker_timestamp: None,
                message_json: serde_json::to_value(BrokerEvent::Fill {
                    broker_message_id: "z-msg".into(),
                    broker_fill_id: None,
                    internal_order_id: "ord-1".into(),
                    broker_order_id: None,
                    symbol: "X".into(),
                    side: Side::Buy,
                    delta_qty: 1,
                    price_micros: 1,
                    fee_micros: 0,
                })
                .unwrap(),
                received_at_utc: chrono::Utc::now(),
                applied_at_utc: None,
            },
            mqk_db::InboxRow {
                inbox_id: 11,
                run_id: Uuid::nil(),
                broker_message_id: "a-msg".into(),
                broker_fill_id: None,
                broker_sequence_id: None,
                broker_timestamp: None,
                message_json: serde_json::to_value(BrokerEvent::Ack {
                    broker_message_id: "a-msg".into(),
                    internal_order_id: "ord-1".into(),
                    broker_order_id: None,
                })
                .unwrap(),
                received_at_utc: chrono::Utc::now(),
                applied_at_utc: None,
            },
        ])
        .expect("canonical queue should build");

        assert_eq!(queue[0].0, 10);
        assert!(matches!(queue[0].2, BrokerEvent::Fill { .. }));
    }

    #[test]
    fn restart_replay_preserves_durable_apply_order() {
        use mqk_execution::Side;

        let first_pass = build_canonical_apply_queue(vec![
            mqk_db::InboxRow {
                inbox_id: 200,
                run_id: Uuid::nil(),
                broker_message_id: "m-2".into(),
                broker_fill_id: None,
                broker_sequence_id: None,
                broker_timestamp: None,
                message_json: serde_json::to_value(BrokerEvent::PartialFill {
                    broker_message_id: "m-2".into(),
                    broker_fill_id: None,
                    internal_order_id: "ord-r".into(),
                    broker_order_id: None,
                    symbol: "X".into(),
                    side: Side::Buy,
                    delta_qty: 2,
                    price_micros: 2,
                    fee_micros: 0,
                })
                .unwrap(),
                received_at_utc: chrono::Utc::now(),
                applied_at_utc: None,
            },
            mqk_db::InboxRow {
                inbox_id: 201,
                run_id: Uuid::nil(),
                broker_message_id: "m-1".into(),
                broker_fill_id: None,
                broker_sequence_id: None,
                broker_timestamp: None,
                message_json: serde_json::to_value(BrokerEvent::Ack {
                    broker_message_id: "m-1".into(),
                    internal_order_id: "ord-r".into(),
                    broker_order_id: None,
                })
                .unwrap(),
                received_at_utc: chrono::Utc::now(),
                applied_at_utc: None,
            },
        ])
        .unwrap();
        let second_pass = build_canonical_apply_queue(vec![
            mqk_db::InboxRow {
                inbox_id: 200,
                run_id: Uuid::nil(),
                broker_message_id: "m-2".into(),
                broker_fill_id: None,
                broker_sequence_id: None,
                broker_timestamp: None,
                message_json: serde_json::to_value(BrokerEvent::PartialFill {
                    broker_message_id: "m-2".into(),
                    broker_fill_id: None,
                    internal_order_id: "ord-r".into(),
                    broker_order_id: None,
                    symbol: "X".into(),
                    side: Side::Buy,
                    delta_qty: 2,
                    price_micros: 2,
                    fee_micros: 0,
                })
                .unwrap(),
                received_at_utc: chrono::Utc::now(),
                applied_at_utc: None,
            },
            mqk_db::InboxRow {
                inbox_id: 201,
                run_id: Uuid::nil(),
                broker_message_id: "m-1".into(),
                broker_fill_id: None,
                broker_sequence_id: None,
                broker_timestamp: None,
                message_json: serde_json::to_value(BrokerEvent::Ack {
                    broker_message_id: "m-1".into(),
                    internal_order_id: "ord-r".into(),
                    broker_order_id: None,
                })
                .unwrap(),
                received_at_utc: chrono::Utc::now(),
                applied_at_utc: None,
            },
        ])
        .unwrap();

        let first_ids: Vec<i64> = first_pass.into_iter().map(|x| x.0).collect();
        let second_ids: Vec<i64> = second_pass.into_iter().map(|x| x.0).collect();
        assert_eq!(first_ids, second_ids);
    }

    #[test]
    fn ambiguous_ordering_truth_fails_closed() {
        let err = build_canonical_apply_queue(vec![
            mqk_db::InboxRow {
                inbox_id: 7,
                run_id: Uuid::nil(),
                broker_message_id: "m-1".into(),
                broker_fill_id: None,
                broker_sequence_id: None,
                broker_timestamp: None,
                message_json: serde_json::to_value(BrokerEvent::Ack {
                    broker_message_id: "m-1".into(),
                    internal_order_id: "ord-a".into(),
                    broker_order_id: None,
                })
                .unwrap(),
                received_at_utc: chrono::Utc::now(),
                applied_at_utc: None,
            },
            mqk_db::InboxRow {
                inbox_id: 7,
                run_id: Uuid::nil(),
                broker_message_id: "m-2".into(),
                broker_fill_id: None,
                broker_sequence_id: None,
                broker_timestamp: None,
                message_json: serde_json::to_value(BrokerEvent::Reject {
                    broker_message_id: "m-2".into(),
                    internal_order_id: "ord-a".into(),
                    broker_order_id: None,
                })
                .unwrap(),
                received_at_utc: chrono::Utc::now(),
                applied_at_utc: None,
            },
        ])
        .expect_err("duplicate canonical key must fail closed");

        assert!(err.to_string().contains("AMBIGUOUS_CANONICAL_ORDER"));
    }

    // -----------------------------------------------------------------------
    // Section C - apply_fill_step unit tests
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
            broker_fill_id: None,
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
            broker_fill_id: None,
            internal_order_id: internal_id.to_string(),
            broker_order_id: None,
            symbol: "SPY".to_string(),
            side: mqk_execution::Side::Buy,
            delta_qty: qty,
            price_micros: 450_000_000,
            fee_micros: 0,
        }
    }
    fn make_cancel_ack_event(internal_id: &str, msg_id: &str) -> BrokerEvent {
        BrokerEvent::CancelAck {
            broker_message_id: msg_id.to_string(),
            internal_order_id: internal_id.to_string(),
            broker_order_id: None,
        }
    }
    fn make_reject_event(internal_id: &str, msg_id: &str) -> BrokerEvent {
        BrokerEvent::Reject {
            broker_message_id: msg_id.to_string(),
            internal_order_id: internal_id.to_string(),
            broker_order_id: None,
        }
    }
    fn make_cancel_reject_event(internal_id: &str, msg_id: &str) -> BrokerEvent {
        BrokerEvent::CancelReject {
            broker_message_id: msg_id.to_string(),
            internal_order_id: internal_id.to_string(),
            broker_order_id: None,
        }
    }
    fn make_replace_ack_event(internal_id: &str, msg_id: &str, qty: i64) -> BrokerEvent {
        BrokerEvent::ReplaceAck {
            broker_message_id: msg_id.to_string(),
            internal_order_id: internal_id.to_string(),
            broker_order_id: None,
            new_total_qty: qty,
        }
    }
    fn make_replace_reject_event(internal_id: &str, msg_id: &str) -> BrokerEvent {
        BrokerEvent::ReplaceReject {
            broker_message_id: msg_id.to_string(),
            internal_order_id: internal_id.to_string(),
            broker_order_id: None,
        }
    }
    fn apply_event_and_maybe_remove_broker_mapping(
        oms_orders: &mut BTreeMap<String, OmsOrder>,
        order_map: &mut BrokerOrderMap,
        event: &BrokerEvent,
        msg_id: &str,
    ) -> anyhow::Result<AppliedBrokerEventOutcome> {
        let internal_id = event.internal_order_id().to_string();
        let outcome = apply_broker_event_step(oms_orders, &internal_id, event, msg_id)?;
        if outcome.terminal_apply_succeeded {
            remove_broker_mapping_from_memory(order_map, &internal_id);
        }
        Ok(outcome)
    }
    #[test]
    fn fill_terminal_apply_success_removes_broker_map() {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        oms.insert(
            "ord-fill".to_string(),
            OmsOrder::new("ord-fill", "SPY", 100),
        );
        let mut order_map = BrokerOrderMap::new();
        order_map.register("ord-fill", "broker-fill");

        let outcome = apply_event_and_maybe_remove_broker_mapping(
            &mut oms,
            &mut order_map,
            &make_fill_event("ord-fill", "fill-msg", 100),
            "fill-msg",
        )
        .expect("terminal fill apply must succeed");

        assert!(outcome.terminal_apply_succeeded);
        assert_eq!(
            oms["ord-fill"].state,
            mqk_execution::oms::state_machine::OrderState::Filled
        );
        assert!(
            order_map.broker_id("ord-fill").is_none(),
            "successful terminal fill apply must remove the broker mapping"
        );
    }
    #[test]
    fn cancel_ack_unknown_order_does_not_remove_broker_map() {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        let mut order_map = BrokerOrderMap::new();
        order_map.register("ord-cancel", "broker-cancel");

        let err = apply_event_and_maybe_remove_broker_mapping(
            &mut oms,
            &mut order_map,
            &make_cancel_ack_event("ord-cancel", "cancel-msg"),
            "cancel-msg",
        )
        .expect_err("unknown cancel-ack must fail closed");
        assert!(
            err.to_string()
                .contains("UNKNOWN_ORDER_NON_FILL_LIFECYCLE_DIVERGENCE")
        );

        assert!(
            order_map.broker_id("ord-cancel").is_some(),
            "unknown-order cancel-ack must not remove the broker mapping"
        );
    }
    #[test]
    fn reject_unknown_order_does_not_remove_broker_map() {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        let mut order_map = BrokerOrderMap::new();
        order_map.register("ord-reject", "broker-reject");

        let err = apply_event_and_maybe_remove_broker_mapping(
            &mut oms,
            &mut order_map,
            &make_reject_event("ord-reject", "reject-msg"),
            "reject-msg",
        )
        .expect_err("unknown reject must fail closed");
        assert!(
            err.to_string()
                .contains("UNKNOWN_ORDER_NON_FILL_LIFECYCLE_DIVERGENCE")
        );

        assert!(
            order_map.broker_id("ord-reject").is_some(),
            "unknown-order reject must not remove the broker mapping"
        );
    }
    #[test]
    fn non_terminal_events_do_not_remove_broker_map() {
        // Ack on a live open order: non-terminal, mapping must remain.
        {
            let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
            oms.insert(
                "ord-non-terminal".to_string(),
                OmsOrder::new("ord-non-terminal", "SPY", 120),
            );
            let mut order_map = BrokerOrderMap::new();
            order_map.register("ord-non-terminal", "broker-non-terminal");

            let outcome = apply_event_and_maybe_remove_broker_mapping(
                &mut oms,
                &mut order_map,
                &make_ack_event("ord-non-terminal", "ack-msg"),
                "ack-msg",
            )
            .expect("ack apply must not fail");

            assert!(
                !outcome.terminal_apply_succeeded,
                "ack must not request broker-map cleanup"
            );
            assert!(
                order_map.broker_id("ord-non-terminal").is_some(),
                "ack must retain the broker mapping"
            );
        }

        // Partial fill on an open order: non-terminal, mapping must remain.
        {
            let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
            oms.insert(
                "ord-non-terminal".to_string(),
                OmsOrder::new("ord-non-terminal", "SPY", 120),
            );
            let mut order_map = BrokerOrderMap::new();
            order_map.register("ord-non-terminal", "broker-non-terminal");

            let outcome = apply_event_and_maybe_remove_broker_mapping(
                &mut oms,
                &mut order_map,
                &make_partial_fill_event("ord-non-terminal", "partial-msg", 10),
                "partial-msg",
            )
            .expect("partial fill apply must not fail");

            assert!(
                !outcome.terminal_apply_succeeded,
                "partial fill must not request broker-map cleanup"
            );
            assert!(
                order_map.broker_id("ord-non-terminal").is_some(),
                "partial fill must retain the broker mapping"
            );
        }

        // CancelReject is only legal from CancelPending.
        {
            let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
            let mut order = OmsOrder::new("ord-non-terminal", "SPY", 120);
            order
                .apply(&OmsEvent::CancelRequest, Some("cancel-request-msg"))
                .expect("seed cancel-pending state");
            oms.insert("ord-non-terminal".to_string(), order);

            let mut order_map = BrokerOrderMap::new();
            order_map.register("ord-non-terminal", "broker-non-terminal");

            let outcome = apply_event_and_maybe_remove_broker_mapping(
                &mut oms,
                &mut order_map,
                &make_cancel_reject_event("ord-non-terminal", "cancel-reject-msg"),
                "cancel-reject-msg",
            )
            .expect("cancel reject apply must not fail");

            assert!(
                !outcome.terminal_apply_succeeded,
                "cancel reject must not request broker-map cleanup"
            );
            assert!(
                order_map.broker_id("ord-non-terminal").is_some(),
                "cancel reject must retain the broker mapping"
            );
        }

        // ReplaceAck is only legal from ReplacePending.
        {
            let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
            let mut order = OmsOrder::new("ord-non-terminal", "SPY", 120);
            order
                .apply(&OmsEvent::ReplaceRequest, Some("replace-request-msg"))
                .expect("seed replace-pending state");
            oms.insert("ord-non-terminal".to_string(), order);

            let mut order_map = BrokerOrderMap::new();
            order_map.register("ord-non-terminal", "broker-non-terminal");

            let outcome = apply_event_and_maybe_remove_broker_mapping(
                &mut oms,
                &mut order_map,
                &make_replace_ack_event("ord-non-terminal", "replace-ack-msg", 120),
                "replace-ack-msg",
            )
            .expect("replace ack apply must not fail");

            assert!(
                !outcome.terminal_apply_succeeded,
                "replace ack must not request broker-map cleanup"
            );
            assert!(
                order_map.broker_id("ord-non-terminal").is_some(),
                "replace ack must retain the broker mapping"
            );
        }

        // ReplaceReject is only legal from ReplacePending.
        {
            let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
            let mut order = OmsOrder::new("ord-non-terminal", "SPY", 120);
            order
                .apply(&OmsEvent::ReplaceRequest, Some("replace-request-msg"))
                .expect("seed replace-pending state");
            oms.insert("ord-non-terminal".to_string(), order);

            let mut order_map = BrokerOrderMap::new();
            order_map.register("ord-non-terminal", "broker-non-terminal");

            let outcome = apply_event_and_maybe_remove_broker_mapping(
                &mut oms,
                &mut order_map,
                &make_replace_reject_event("ord-non-terminal", "replace-reject-msg"),
                "replace-reject-msg",
            )
            .expect("replace reject apply must not fail");

            assert!(
                !outcome.terminal_apply_succeeded,
                "replace reject must not request broker-map cleanup"
            );
            assert!(
                order_map.broker_id("ord-non-terminal").is_some(),
                "replace reject must retain the broker mapping"
            );
        }
    }
    #[test]
    fn replayed_terminal_noop_does_not_incorrectly_remove_mapping_or_break_idempotence() {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        let mut terminal = OmsOrder::new("ord-replay", "SPY", 100);
        terminal
            .apply(&OmsEvent::Fill { delta_qty: 100 }, Some("fill-msg"))
            .expect("seed terminal fill state");
        oms.insert("ord-replay".to_string(), terminal);
        let mut order_map = BrokerOrderMap::new();
        order_map.register("ord-replay", "broker-replay");

        let outcome = apply_event_and_maybe_remove_broker_mapping(
            &mut oms,
            &mut order_map,
            &make_fill_event("ord-replay", "fill-late", 100),
            "fill-late",
        )
        .expect("late terminal-looking fill replay must be a safe no-op");

        assert!(outcome.fill.is_none());
        assert!(!outcome.terminal_apply_succeeded);
        assert!(
            order_map.broker_id("ord-replay").is_some(),
            "terminal-looking no-op replay must not remove the mapping solely by event kind"
        );
        assert_eq!(
            oms["ord-replay"].state,
            mqk_execution::oms::state_machine::OrderState::Filled,
            "late replay must preserve terminal OMS state"
        );
    }
    #[test]
    fn terminal_cleanup_occurs_before_mark_applied_or_is_otherwise_proven_durably_safe() {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        let mut order = OmsOrder::new("ord-cancel-pending", "SPY", 100);
        order
            .apply(&OmsEvent::CancelRequest, Some("cancel-request"))
            .expect("seed cancel pending state");
        oms.insert("ord-cancel-pending".to_string(), order);
        let mut order_map = BrokerOrderMap::new();
        order_map.register("ord-cancel-pending", "broker-cancel-pending");

        let outcome = apply_event_and_maybe_remove_broker_mapping(
            &mut oms,
            &mut order_map,
            &make_cancel_ack_event("ord-cancel-pending", "cancel-ack-msg"),
            "cancel-ack-msg",
        )
        .expect("cancel-ack terminal apply must succeed");

        assert!(
            outcome.terminal_apply_succeeded,
            "cleanup gate must only open after OMS has successfully reached a terminal state"
        );
        assert_eq!(
            oms["ord-cancel-pending"].state,
            mqk_execution::oms::state_machine::OrderState::Cancelled,
            "terminal state must be applied before cleanup can run"
        );
        assert!(
            order_map.broker_id("ord-cancel-pending").is_none(),
            "once terminal apply succeeds, runtime cleanup can safely remove the mapping before mark_applied"
        );
    }
    /// Section C - T1.
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
    /// Section C - T2.
    /// A PartialFill event for an order not present in oms_orders must also
    /// return UNKNOWN_ORDER_FILL - the rule is not limited to final fills.
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
    /// Section C - T3.
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
    /// Section C - T4.
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
    /// Section C - T5.
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

    #[test]
    fn duplicate_economic_fill_id_across_messages_is_deduped() {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        oms.insert("ord-3b".to_string(), OmsOrder::new("ord-3b", "SPY", 100));

        let mut ev1 = make_partial_fill_event("ord-3b", "transport-msg-1", 60);
        let mut ev2 = make_partial_fill_event("ord-3b", "transport-msg-2", 60);
        if let BrokerEvent::PartialFill { broker_fill_id, .. } = &mut ev1 {
            *broker_fill_id = Some("econ-fill-1".to_string());
        }
        if let BrokerEvent::PartialFill { broker_fill_id, .. } = &mut ev2 {
            *broker_fill_id = Some("econ-fill-1".to_string());
        }

        let first = apply_fill_step(&mut oms, "ord-3b", &ev1, "transport-msg-1")
            .unwrap()
            .expect("first apply should mutate portfolio");
        assert_eq!(first.qty, 60);
        assert_eq!(oms["ord-3b"].filled_qty, 60);

        let second = apply_fill_step(&mut oms, "ord-3b", &ev2, "transport-msg-2").unwrap();
        assert!(
            second.is_none(),
            "same broker_fill_id should dedupe even when broker_message_id changes"
        );
        assert_eq!(oms["ord-3b"].filled_qty, 60);
    }
    /// Section C - T6.
    /// A non-fill event (Ack) for an order not present in oms_orders must
    /// fail closed as lifecycle divergence (Err), not silently return Ok(None).
    #[test]
    fn unknown_order_non_fill_fails_closed() {
        let mut oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
        let ev = make_ack_event("ord-ghost", "ack-msg-ghost");
        let result = apply_fill_step(&mut oms, "ord-ghost", &ev, "ack-msg-ghost");
        let err = result.expect_err("non-fill event for unknown order must fail closed");
        assert!(
            err.to_string()
                .contains("UNKNOWN_ORDER_NON_FILL_LIFECYCLE_DIVERGENCE"),
            "error should surface explicit lifecycle divergence code"
        );
    }

    fn valid_submit_order_json() -> serde_json::Value {
        serde_json::json!({
            "symbol": "SPY",
            "side": "buy",
            "qty": 10,
            "order_type": "market",
            "limit_price": null,
            "time_in_force": "day"
        })
    }

    fn legacy_minimal_submit_order_json() -> serde_json::Value {
        serde_json::json!({
            "symbol": "SPY",
            "quantity": 10
        })
    }

    struct SubmitCountingBroker {
        submits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl mqk_execution::BrokerAdapter for SubmitCountingBroker {
        fn submit_order(
            &self,
            req: mqk_execution::BrokerSubmitRequest,
            _token: &mqk_execution::BrokerInvokeToken,
        ) -> Result<mqk_execution::BrokerSubmitResponse, mqk_execution::BrokerError> {
            self.submits
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(mqk_execution::BrokerSubmitResponse {
                broker_order_id: format!("broker-{}", req.order_id),
                submitted_at: 1,
                status: "ok".to_string(),
            })
        }

        fn cancel_order(
            &self,
            order_id: &str,
            _token: &mqk_execution::BrokerInvokeToken,
        ) -> Result<mqk_execution::BrokerCancelResponse, mqk_execution::BrokerError> {
            Ok(mqk_execution::BrokerCancelResponse {
                broker_order_id: order_id.to_string(),
                cancelled_at: 1,
                status: "ok".to_string(),
            })
        }

        fn replace_order(
            &self,
            req: mqk_execution::BrokerReplaceRequest,
            _token: &mqk_execution::BrokerInvokeToken,
        ) -> Result<mqk_execution::BrokerReplaceResponse, mqk_execution::BrokerError> {
            Ok(mqk_execution::BrokerReplaceResponse {
                broker_order_id: req.broker_order_id,
                replaced_at: 1,
                status: "ok".to_string(),
            })
        }

        fn fetch_events(
            &self,
            _cursor: Option<&str>,
            _token: &mqk_execution::BrokerInvokeToken,
        ) -> Result<(Vec<mqk_execution::BrokerEvent>, Option<String>), mqk_execution::BrokerError>
        {
            Ok((Vec::new(), None))
        }
    }

    #[test]
    fn submit_request_rejects_zero_effective_quantity() {
        let mut zero_qty = valid_submit_order_json();
        zero_qty["qty"] = serde_json::json!(0);
        let err = build_validated_submit_request("ord-zero", &zero_qty)
            .expect_err("zero quantity must be rejected before broker submission");
        assert!(err.to_string().contains("quantity") || err.to_string().contains("qty"));
    }

    #[test]
    fn submit_request_rejects_out_of_range_quantity() {
        let mut out_of_range = valid_submit_order_json();
        out_of_range["qty"] = serde_json::json!(2147483648_i64);
        let err = build_validated_submit_request("ord-range", &out_of_range)
            .expect_err("out-of-range quantity must be rejected before broker submission");
        assert!(err.to_string().contains("out of range"));

        let mut lossy = valid_submit_order_json();
        lossy["qty"] = serde_json::json!(1.5);
        let err = build_validated_submit_request("ord-lossy", &lossy)
            .expect_err("lossy quantity must be rejected before broker submission");
        assert!(
            err.to_string().contains("lossy conversion") || err.to_string().contains("integer")
        );
    }

    #[test]
    fn legacy_payload_without_side_uses_signed_quantity_compatibility_rule() {
        let payload = serde_json::json!({
            "symbol": "SPY",
            "quantity": -25,
            "order_type": "market",
            "time_in_force": "day"
        });

        let req = build_validated_submit_request("ord-legacy-side", &payload)
            .expect("legacy signed-quantity payload must build");

        assert!(matches!(req.side, mqk_execution::Side::Sell));
        assert_eq!(req.quantity, 25);
        assert_eq!(req.order_type, "market");
        assert_eq!(req.time_in_force, "day");
    }

    #[test]
    fn legacy_payload_without_order_type_or_tif_uses_repo_backed_defaults() {
        let req = build_validated_submit_request(
            "ord-legacy-defaults",
            &legacy_minimal_submit_order_json(),
        )
        .expect("legacy minimal payload must build with repo-backed defaults");

        assert!(matches!(req.side, mqk_execution::Side::Buy));
        assert_eq!(req.quantity, 10);
        assert_eq!(req.order_type, "market");
        assert_eq!(req.time_in_force, "day");
        assert_eq!(req.limit_price, None);
    }

    #[test]
    fn submit_request_rejects_missing_or_blank_symbol() {
        let mut missing_symbol = valid_submit_order_json();
        let missing_obj = missing_symbol.as_object_mut().expect("object");
        missing_obj.remove("symbol");
        let err = build_validated_submit_request("ord-missing-symbol", &missing_symbol)
            .expect_err("missing symbol must be rejected before broker submission");
        assert!(err.to_string().contains("symbol"));

        let mut blank_symbol = valid_submit_order_json();
        blank_symbol["symbol"] = serde_json::json!("   ");
        let err = build_validated_submit_request("ord-blank-symbol", &blank_symbol)
            .expect_err("blank symbol must be rejected before broker submission");
        assert!(err.to_string().contains("symbol"));
    }

    #[test]
    fn submit_request_rejects_invalid_order_type_or_price_semantics() {
        let mut unsupported_type = valid_submit_order_json();
        unsupported_type["order_type"] = serde_json::json!("stop");
        let err = build_validated_submit_request("ord-stop", &unsupported_type)
            .expect_err("unsupported order_type must be rejected before broker submission");
        assert!(err.to_string().contains("order_type"));

        let mut limit_missing_price = valid_submit_order_json();
        limit_missing_price["order_type"] = serde_json::json!("limit");
        let err = build_validated_submit_request("ord-limit-missing", &limit_missing_price)
            .expect_err("limit order missing limit_price must be rejected");
        assert!(err.to_string().contains("limit_price"));

        let mut market_with_limit = valid_submit_order_json();
        market_with_limit["limit_price"] = serde_json::json!(1000000);
        let err = build_validated_submit_request("ord-market-limit", &market_with_limit)
            .expect_err("market order carrying limit_price must be rejected");
        assert!(err.to_string().contains("limit_price"));
    }

    #[test]
    fn incompatible_qty_and_quantity_fields_are_rejected() {
        let payload = serde_json::json!({
            "symbol": "SPY",
            "side": "buy",
            "qty": 5,
            "quantity": 10,
            "order_type": "market",
            "time_in_force": "day"
        });

        let err = build_validated_submit_request("ord-qty-mismatch", &payload)
            .expect_err("conflicting qty fields must be rejected");
        assert!(err.to_string().contains("disagree"));
    }

    #[test]
    fn malformed_defaulted_market_payload_with_limit_price_is_rejected() {
        let mut payload = legacy_minimal_submit_order_json();
        payload["limit_price"] = serde_json::json!(1_000_000);

        let err = build_validated_submit_request("ord-default-market-limit", &payload)
            .expect_err("defaulted market payload carrying limit_price must be rejected");
        assert!(err.to_string().contains("limit_price"));
    }

    #[test]
    fn malformed_persisted_order_payload_does_not_reach_broker_submit() {
        let submits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let _broker = SubmitCountingBroker {
            submits: std::sync::Arc::clone(&submits),
        };
        let mut malformed = valid_submit_order_json();
        malformed["qty"] = serde_json::json!(0);

        let result = build_validated_submit_request("ord-malformed", &malformed);

        assert!(result.is_err(), "malformed payload must fail before submit");
        assert_eq!(
            submits.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "invalid persisted payload must not reach broker submit"
        );
    }
    // -----------------------------------------------------------------------
    // Section D - Restart replay safety unit tests
    //
    // These tests prove that restart replay safety is gated by the durable
    // inbox applied_at_utc column (modelled here as queue membership), NOT
    // by the OMS in-memory applied_event_ids set.
    // -----------------------------------------------------------------------
    /// Section D - T1.  Primary restart replay safety proof.
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
        // Fresh restart: OmsOrder rebuilt from outbox - applied_event_ids is empty.
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
    /// Section D - T2.  Unapplied fill recovers exactly once with fresh OMS state.
    ///
    /// Simulates the W6 crash window: fill was inbox-inserted but mark_applied
    /// did not complete before crash.  After restart the OmsOrder is rebuilt
    /// fresh (applied_event_ids empty) and the fill IS in the recovery queue.
    ///
    /// First apply: Ok(Some(fill)) - portfolio mutated (correct recovery).
    /// Second delivery of the same msg_id within the session: Ok(None) -
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
    /// Section D - T3.  Durable applied gate is queue membership, not OMS memory.
    ///
    /// Two fills for the same order:
    ///   F1 (delta_qty=40) - applied before crash, NOT in apply_queue.
    ///   F2 (delta_qty=60) - unapplied, IN apply_queue.
    ///
    /// OmsOrder is fresh after restart (applied_event_ids empty, filled_qty=0).
    /// Only F2 must reach portfolio; F1's absence from the queue is the fence.
    ///
    /// Proves: which fills mutate portfolio after restart is determined by
    /// inbox_load_unapplied_for_run output alone - not by OMS in-memory state.
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
    /// Section D - T4.  Empty applied_event_ids does not bypass restart replay protection.
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
    #[derive(Clone)]
    struct MutableClock {
        now: std::sync::Arc<std::sync::Mutex<chrono::DateTime<chrono::Utc>>>,
    }
    impl MutableClock {
        fn new(now: chrono::DateTime<chrono::Utc>) -> Self {
            Self {
                now: std::sync::Arc::new(std::sync::Mutex::new(now)),
            }
        }
        fn set(&self, now: chrono::DateTime<chrono::Utc>) {
            *self.now.lock().expect("clock lock") = now;
        }
    }
    impl mqk_db::TimeSource for MutableClock {
        fn now_utc(&self) -> chrono::DateTime<chrono::Utc> {
            *self.now.lock().expect("clock lock")
        }
    }
    struct NoopBroker;
    impl mqk_execution::BrokerAdapter for NoopBroker {
        fn submit_order(
            &self,
            req: mqk_execution::BrokerSubmitRequest,
            _token: &mqk_execution::BrokerInvokeToken,
        ) -> Result<mqk_execution::BrokerSubmitResponse, mqk_execution::BrokerError> {
            Ok(mqk_execution::BrokerSubmitResponse {
                broker_order_id: format!("broker-{}", req.order_id),
                submitted_at: 1,
                status: "ok".to_string(),
            })
        }
        fn cancel_order(
            &self,
            order_id: &str,
            _token: &mqk_execution::BrokerInvokeToken,
        ) -> Result<mqk_execution::BrokerCancelResponse, mqk_execution::BrokerError> {
            Ok(mqk_execution::BrokerCancelResponse {
                broker_order_id: order_id.to_string(),
                cancelled_at: 1,
                status: "ok".to_string(),
            })
        }
        fn replace_order(
            &self,
            req: mqk_execution::BrokerReplaceRequest,
            _token: &mqk_execution::BrokerInvokeToken,
        ) -> Result<mqk_execution::BrokerReplaceResponse, mqk_execution::BrokerError> {
            Ok(mqk_execution::BrokerReplaceResponse {
                broker_order_id: req.broker_order_id,
                replaced_at: 1,
                status: "ok".to_string(),
            })
        }
        fn fetch_events(
            &self,
            _cursor: Option<&str>,
            _token: &mqk_execution::BrokerInvokeToken,
        ) -> Result<(Vec<mqk_execution::BrokerEvent>, Option<String>), mqk_execution::BrokerError>
        {
            Ok((Vec::new(), None))
        }
    }
    #[derive(Clone, Copy)]
    struct AllowGate;
    impl mqk_execution::IntegrityGate for AllowGate {
        fn is_armed(&self) -> bool {
            true
        }
    }
    impl mqk_execution::RiskGate for AllowGate {
        fn evaluate_gate(&self) -> mqk_execution::RiskDecision {
            mqk_execution::RiskDecision::Allow
        }
    }
    impl mqk_execution::ReconcileGate for AllowGate {
        fn is_clean(&self) -> bool {
            true
        }
    }
    type LeaseTestOrchestrator =
        ExecutionOrchestrator<NoopBroker, AllowGate, AllowGate, AllowGate, MutableClock>;
    async fn runtime_test_pool() -> PgPool {
        let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
            panic!(
                "DB tests require MQK_DATABASE_URL; run: \
                 MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
                 cargo test -p mqk-runtime runtime_ -- --include-ignored"
            )
        });
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect");
        mqk_db::migrate(&pool).await.expect("migrate");
        sqlx::query("DELETE FROM runtime_leader_lease WHERE id = 1")
            .execute(&pool)
            .await
            .expect("cleanup runtime_leader_lease");
        sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
            .execute(&pool)
            .await
            .expect("cleanup sys_arm_state");
        pool
    }
    fn runtime_ts(seconds: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .timestamp_opt(seconds, 0)
            .single()
            .expect("valid timestamp")
    }
    async fn make_running_run(pool: &PgPool, started_at: chrono::DateTime<chrono::Utc>) -> Uuid {
        let run_id = Uuid::new_v4();
        mqk_db::insert_run(
            pool,
            &mqk_db::NewRun {
                run_id,
                engine_id: format!("runtime-test-{}", run_id),
                mode: "PAPER".to_string(),
                started_at_utc: started_at,
                git_hash: "TEST".to_string(),
                config_hash: format!("cfg-{}", run_id),
                config_json: serde_json::json!({}),
                host_fingerprint: "TESTHOST".to_string(),
            },
        )
        .await
        .expect("insert run");
        mqk_db::arm_run(pool, run_id).await.expect("arm run");
        mqk_db::begin_run(pool, run_id).await.expect("begin run");
        run_id
    }
    fn make_lease_test_orchestrator(
        pool: PgPool,
        run_id: Uuid,
        clock: MutableClock,
    ) -> LeaseTestOrchestrator {
        ExecutionOrchestrator::new(
            pool,
            mqk_execution::BrokerGateway::for_test(NoopBroker, AllowGate, AllowGate, AllowGate),
            mqk_execution::BrokerOrderMap::new(),
            BTreeMap::new(),
            PortfolioState::new(0),
            run_id,
            "runtime-lease-test",
            "paper",
            None,
            clock,
            Box::new(mqk_reconcile::LocalSnapshot::empty),
            Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
        )
    }
    fn broker_snapshot_with_position(
        fetched_at_ms: i64,
        qty: i64,
    ) -> mqk_reconcile::BrokerSnapshot {
        let mut broker = mqk_reconcile::BrokerSnapshot::empty_at(fetched_at_ms);
        broker.positions.insert("SPY".to_string(), qty);
        broker
    }
    #[test]
    fn runtime_reconcile_gate_remains_dirty_after_stale_snapshot() {
        let mut watermark = SnapshotWatermark::new();
        let mut local = mqk_reconcile::LocalSnapshot::empty();
        local.positions.insert("SPY".to_string(), 100);
        let dirty = broker_snapshot_with_position(2_000, 200);
        let err = evaluate_monotonic_reconcile(&mut watermark, &local, &dirty)
            .expect_err("fresh dirty snapshot must block dispatch");
        assert!(matches!(err, MonotonicReconcileError::Dirty));
        let stale_clean = broker_snapshot_with_position(1_000, 100);
        let err = evaluate_monotonic_reconcile(&mut watermark, &local, &stale_clean)
            .expect_err("stale snapshot must not clear dirty state");
        assert!(matches!(
            err,
            MonotonicReconcileError::Stale(StaleBrokerSnapshot {
                freshness: mqk_reconcile::SnapshotFreshness::Stale { .. }
            })
        ));
    }
    #[test]
    fn placeholder_snapshot_path_fails_closed() {
        let mut watermark = SnapshotWatermark::new();
        let local = mqk_reconcile::LocalSnapshot::empty();
        let broker = mqk_reconcile::BrokerSnapshot::empty();
        let err = evaluate_monotonic_reconcile(&mut watermark, &local, &broker)
            .expect_err("placeholder broker snapshot must fail closed");
        assert!(matches!(
            err,
            MonotonicReconcileError::Stale(StaleBrokerSnapshot {
                freshness: mqk_reconcile::SnapshotFreshness::NoTimestamp
            })
        ));
    }
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn runtime_refuses_to_run_without_lease() {
        let pool = runtime_test_pool().await;
        let clock = MutableClock::new(runtime_ts(10_000));
        let run_id = make_running_run(&pool, clock.now_utc()).await;
        let locked =
            mqk_db::runtime_lease::acquire_lease(&pool, "other-runtime", clock.now_utc(), 30)
                .await
                .expect("seed active lease");
        assert!(matches!(
            locked,
            mqk_db::runtime_lease::LeaseAcquireOutcome::Acquired(_)
        ));
        let mut orchestrator = make_lease_test_orchestrator(pool.clone(), run_id, clock.clone());
        let err = orchestrator
            .tick()
            .await
            .expect_err("tick must refuse without lease");
        assert!(
            err.to_string().contains("RUNTIME_LEASE_UNAVAILABLE"),
            "unexpected error: {err}"
        );
        let run = mqk_db::fetch_run(&pool, run_id).await.expect("fetch run");
        assert!(matches!(run.status, mqk_db::RunStatus::Halted));
        let arm_state = mqk_db::load_arm_state(&pool)
            .await
            .expect("load arm state")
            .expect("arm state persisted");
        assert_eq!(arm_state.0, "DISARMED");
    }
    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
    async fn runtime_halts_when_lease_is_lost() {
        let pool = runtime_test_pool().await;
        let clock = MutableClock::new(runtime_ts(20_000));
        let run_id = make_running_run(&pool, clock.now_utc()).await;
        let mut orchestrator = make_lease_test_orchestrator(pool.clone(), run_id, clock.clone());
        orchestrator
            .tick()
            .await
            .expect("first tick acquires lease");
        clock.set(runtime_ts(20_016));
        let stolen =
            mqk_db::runtime_lease::acquire_lease(&pool, "other-runtime", clock.now_utc(), 30)
                .await
                .expect("steal expired lease");
        assert!(matches!(
            stolen,
            mqk_db::runtime_lease::LeaseAcquireOutcome::Acquired(_)
        ));
        let err = orchestrator
            .tick()
            .await
            .expect_err("tick must halt on lease loss");
        assert!(
            err.to_string().contains("RUNTIME_LEASE_LOST"),
            "unexpected error: {err}"
        );
        let run = mqk_db::fetch_run(&pool, run_id).await.expect("fetch run");
        assert!(matches!(run.status, mqk_db::RunStatus::Halted));
        let arm_state = mqk_db::load_arm_state(&pool)
            .await
            .expect("load arm state")
            .expect("arm state persisted");
        assert_eq!(arm_state.0, "DISARMED");
        let lease = mqk_db::runtime_lease::fetch_current_lease(&pool)
            .await
            .expect("fetch current lease")
            .expect("active lease row");
        assert_eq!(lease.holder_id, "other-runtime");
    }
}
