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
    reconcile_monotonic, BrokerSnapshot, LocalSnapshot, OrderStatus, SnapshotWatermark,
    StaleBrokerSnapshot,
};
use sqlx::types::chrono;
use sqlx::PgPool;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
// AUTON-RUNTIME-LEASE-01: TTL must exceed the maximum blocking duration of
// any single orchestrator phase.  `fetch_events` makes a synchronous HTTP
// call (reqwest via block_in_place) to the Alpaca REST API with no timeout
// configured on the client.  A smoke run on 2026-04-30 showed a 33-second
// block, which exceeded the old 15-second TTL and caused RUNTIME_LEASE_LOST.
// 90 seconds provides 3× headroom over the observed maximum while still
// detecting a dead owner within two minutes.  Do NOT lower this value without
// first configuring a shorter HTTP request timeout on the broker adapter.
const RUNTIME_LEASE_TTL_SECS: i64 = 90;
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
// PAPER-SOAK-ALPACA-FILL-AUTHORITY-FINAL-CLOSURE-02: narrow, deliberate
// public re-export. Everything else in `apply` is `pub(super)` (this module
// only); `effective_portfolio_fill` alone is `pub` in `apply.rs` and
// re-exported here so `mqk-daemon`'s durable restart/replay
// (`recover_oms_and_portfolio`) can share the exact live effective-fill
// semantic instead of re-deriving an approximation of it.
pub use apply::effective_portfolio_fill;
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
    /// RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01: ring buffer of recent terminal
    /// fills applied in Phase 3.  Used by Phase 0c to defer RECONCILE_DRIFT
    /// while the broker REST snapshot catches up after a fill.
    ///
    /// Capped at [`RECENT_FILLS_RING_CAP`] entries.  Empty on startup and after
    /// a crash/restart (in-memory only; not persisted).  On restart, the fill
    /// has already been applied and the broker snapshot will refresh within
    /// `TERMINAL_FILL_SETTLE_GRACE_SECS`; if not, genuine drift fires normally.
    recently_applied_fills: VecDeque<RecentTerminalFill>,
    /// RECONCILE-DRIFT-AFTER-TERMINAL-FILL-FRESH-SNAPSHOT-01: Injectable
    /// broker snapshot refresher called when the terminal-fill settle grace
    /// has expired and drift remains.  Returns a fresh `BrokerSnapshot` for
    /// immediate re-evaluation in Phase 0c, or `None` when the adapter is
    /// unavailable or the fetch fails.
    ///
    /// `None` by default (no refresh is attempted; fail-closed).  Set via
    /// `set_terminal_fill_expiry_refresher` after construction.  The production
    /// wiring in `orchestrator_build.rs` captures an `Arc<AlpacaBrokerAdapter>`
    /// and also updates `AppState::broker_snapshot` as a cache side-effect so
    /// subsequent ticks and the background reconcile see the fresh data.
    terminal_fill_expiry_refresher: Option<Box<dyn Fn() -> Option<BrokerSnapshot> + Send + Sync>>,
    /// DISCORD-TRADE-LIFECYCLE-ALERTS-01: optional best-effort alert sink.
    ///
    /// Injected via `set_alert_sink` after construction.  The production wiring
    /// in `orchestrator_build.rs` captures a `DiscordNotifier` clone and spawns
    /// an async delivery task for each event.  All other callsites (tests, CLI)
    /// leave this as `None` — no alerts are emitted.
    ///
    /// The sink is a sync `Fn` so it can be called from the tick loop without
    /// changing any `async` boundaries.  Callers spawn async work internally.
    /// Failure inside the sink must never propagate into the trading path.
    alert_fn: Option<std::sync::Arc<dyn Fn(crate::TradeLifecycleEvent) + Send + Sync>>,
}
/// Grace window for the pending-fill-propagation deferral in Phase 0c.
///
/// If a SENT+mapped outbox row was sent within this many seconds, the
/// reconcile check may be deferred while the WS fill event propagates.
/// Paper market orders on Alpaca typically fill in under one second; 30s
/// provides 30× headroom while bounding the deferral window.
///
/// After this window, RECONCILE_DRIFT fires regardless.
const PENDING_FILL_PROPAGATION_GRACE_SECS: i64 = 30;

/// Grace window for the post-terminal-fill broker snapshot settlement deferral.
///
/// After a terminal fill is applied locally (Phase 3), the local portfolio
/// advances immediately but the broker REST snapshot may still show the
/// pre-fill position.  Phase 0c defers RECONCILE_DRIFT for this window to
/// allow the broker snapshot to catch up.
///
/// Alpaca paper REST may be temporarily unavailable for up to ~2 minutes after
/// a fill (observed in live smoke: broker=stale for ~2 min after terminal fill).
/// 180 seconds provides headroom above the observed 120s worst-case outage
/// window.  The eager refresh (has_recent_terminal_fill) retries every tick so
/// the snapshot clears as soon as Alpaca REST recovers.
const TERMINAL_FILL_SETTLE_GRACE_SECS: i64 = 180;

/// Maximum recent terminal fills to track in memory.
const RECENT_FILLS_RING_CAP: usize = 50;
/// Extended lookback window for the terminal-fill expiry refresher.
///
/// When `TERMINAL_FILL_SETTLE_GRACE_SECS` has expired, Phase 0c calls the
/// refresher if any fill within this wider window still direction-explains
/// the drift.  2× the grace window bounds the search while covering the
/// case where the broker REST snapshot lags by more than the grace window.
const TERMINAL_FILL_EXPIRY_LOOKAHEAD_SECS: i64 = TERMINAL_FILL_SETTLE_GRACE_SECS * 2;

/// Record of a recently applied terminal fill.
///
/// Populated by Phase 3 when a terminal fill is durably applied.
/// Consumed by Phase 0c to defer RECONCILE_DRIFT while the broker REST
/// snapshot catches up to the local portfolio advance.
#[derive(Debug, Clone)]
struct RecentTerminalFill {
    symbol: String,
    /// Signed position delta: positive for buy fills, negative for sell fills.
    signed_delta: i64,
    applied_at: chrono::DateTime<chrono::Utc>,
}

/// Returns `true` if the broker-vs-local reconcile drift is fully explained by
/// the expected fills from `sent_mapped` outbox rows.
///
/// Conditions that must ALL hold for the drift to be considered consistent:
/// 1. Every non-zero position delta (broker - local) has the same sign as the
///    expected fill for that symbol (buy → positive, sell → negative) and does
///    not exceed the expected fill quantity.
/// 2. No position delta exists for a symbol that has no corresponding
///    SENT+mapped order (unexpected symbol → not consistent).
/// 3. Every local active order absent from the broker snapshot corresponds to
///    one of the SENT+mapped outbox rows (the broker filled and closed it).
/// 4. No unknown broker orders exist (broker has an order our OMS never saw).
///
/// If any condition fails, the drift is NOT explained and RECONCILE_DRIFT fires
/// immediately without waiting for the grace window to expire.
fn drift_is_consistent_with_pending_fills(
    sent_mapped: &[mqk_db::OutboxRow],
    local: &LocalSnapshot,
    broker: &BrokerSnapshot,
) -> bool {
    // Build expected position deltas and the set of mapped order IDs from the
    // SENT+mapped outbox rows.
    let mut expected_deltas: BTreeMap<String, i64> = BTreeMap::new();
    let mut mapped_ids: BTreeSet<String> = BTreeSet::new();

    for row in sent_mapped {
        mapped_ids.insert(row.idempotency_key.clone());
        let sym = row
            .order_json
            .get("symbol")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let side = row
            .order_json
            .get("side")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let qty = row
            .order_json
            .get("qty")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if !sym.is_empty() && qty > 0 {
            let delta = if side.eq_ignore_ascii_case("buy") {
                qty
            } else {
                -qty
            };
            *expected_deltas.entry(sym.to_string()).or_default() += delta;
        }
    }

    // Check position deltas across the union of all symbols.
    let mut all_syms: BTreeSet<String> = BTreeSet::new();
    all_syms.extend(local.positions.keys().cloned());
    all_syms.extend(broker.positions.keys().cloned());

    for sym in &all_syms {
        let lq = local.positions.get(sym).copied().unwrap_or(0);
        let bq = broker.positions.get(sym).copied().unwrap_or(0);
        let actual = bq - lq;
        if actual != 0 {
            let expected = expected_deltas.get(sym).copied().unwrap_or(0);
            if expected == 0 {
                return false; // drift on a symbol with no pending fill
            }
            if (expected > 0) != (actual > 0) {
                return false; // wrong direction
            }
            if actual.abs() > expected.abs() {
                return false; // more fill than ordered
            }
        }
    }

    // No unknown broker orders.
    for oid in broker.orders.keys() {
        if !local.orders.contains_key(oid) {
            return false;
        }
    }

    // All active local orders absent from the broker snapshot must correspond
    // to a SENT+mapped row (the broker filled and closed the order).
    for (oid, ord) in &local.orders {
        if !broker.orders.contains_key(oid) {
            let is_terminal = matches!(
                ord.status,
                OrderStatus::Filled | OrderStatus::Canceled | OrderStatus::Rejected
            );
            if !is_terminal && !mapped_ids.contains(oid) {
                return false;
            }
        }
    }

    true
}

/// Returns `true` if the broker-vs-local drift is fully explained by recently
/// applied terminal fills still within the settle grace window.
///
/// After Phase 3 applies a terminal fill, the local portfolio advances but
/// the broker REST snapshot may still reflect the pre-fill position.
/// `signed_delta` in each entry is positive for buy fills, negative for sells.
///
/// Conditions that must ALL hold:
/// 1. At least one fill is within `TERMINAL_FILL_SETTLE_GRACE_SECS`.
/// 2. Every symbol with non-zero drift (local ahead of broker, `lq - bq != 0`)
///    has a corresponding recent fill with the same sign.
/// 3. The drift magnitude does not exceed the accumulated fill magnitude.
/// 4. No symbol shows broker-ahead drift (`lq - bq < 0` for a net-buy
///    symbol) — that would be unexpected external broker activity.
fn drift_is_consistent_with_recent_terminal_fills(
    recent_fills: &std::collections::VecDeque<RecentTerminalFill>,
    local: &LocalSnapshot,
    broker: &BrokerSnapshot,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    // Accumulate expected local-ahead deltas from fills within the window.
    let mut expected: BTreeMap<String, i64> = BTreeMap::new();
    let mut any_in_grace = false;
    for fill in recent_fills {
        let elapsed = (now - fill.applied_at).num_seconds();
        if (0..TERMINAL_FILL_SETTLE_GRACE_SECS).contains(&elapsed) {
            any_in_grace = true;
            *expected.entry(fill.symbol.clone()).or_default() += fill.signed_delta;
        }
    }
    if !any_in_grace {
        return false;
    }

    let mut all_syms: BTreeSet<String> = BTreeSet::new();
    all_syms.extend(local.positions.keys().cloned());
    all_syms.extend(broker.positions.keys().cloned());

    for sym in &all_syms {
        let lq = local.positions.get(sym).copied().unwrap_or(0);
        let bq = broker.positions.get(sym).copied().unwrap_or(0);
        let actual = lq - bq; // positive = local ahead of broker
        if actual != 0 {
            let exp = expected.get(sym).copied().unwrap_or(0);
            if exp == 0 {
                return false; // unexplained drift on this symbol
            }
            if (exp > 0) != (actual > 0) {
                return false; // wrong direction
            }
            if actual.abs() > exp.abs() {
                return false; // drift exceeds what the fills explain
            }
        }
    }
    true
}

/// Returns `true` if any fill that has JUST expired from the settle grace
/// window still direction-explains the current drift.
///
/// Used by Phase 0c to decide whether to attempt a final broker snapshot
/// refresh before firing RECONCILE_DRIFT.  Unlike
/// `drift_is_consistent_with_recent_terminal_fills`, this function accepts
/// fills whose elapsed time is in `[TERMINAL_FILL_SETTLE_GRACE_SECS,
/// TERMINAL_FILL_EXPIRY_LOOKAHEAD_SECS)` — i.e. fills that have recently
/// expired from the grace window.
///
/// Returns `false` if no qualifying fills exist or the drift direction /
/// magnitude is inconsistent with the expired fills.
fn has_expired_terminal_fill_explaining_drift(
    recent_fills: &std::collections::VecDeque<RecentTerminalFill>,
    local: &LocalSnapshot,
    broker: &BrokerSnapshot,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let mut expected: BTreeMap<String, i64> = BTreeMap::new();
    let mut any_in_lookahead = false;
    for fill in recent_fills {
        let elapsed = (now - fill.applied_at).num_seconds();
        if (TERMINAL_FILL_SETTLE_GRACE_SECS..TERMINAL_FILL_EXPIRY_LOOKAHEAD_SECS).contains(&elapsed)
        {
            any_in_lookahead = true;
            *expected.entry(fill.symbol.clone()).or_default() += fill.signed_delta;
        }
    }
    if !any_in_lookahead {
        return false;
    }
    let mut all_syms: BTreeSet<String> = BTreeSet::new();
    all_syms.extend(local.positions.keys().cloned());
    all_syms.extend(broker.positions.keys().cloned());
    for sym in &all_syms {
        let lq = local.positions.get(sym).copied().unwrap_or(0);
        let bq = broker.positions.get(sym).copied().unwrap_or(0);
        let actual = lq - bq;
        if actual != 0 {
            let exp = expected.get(sym).copied().unwrap_or(0);
            if exp == 0 {
                return false;
            }
            if (exp > 0) != (actual > 0) {
                return false;
            }
            if actual.abs() > exp.abs() {
                return false;
            }
        }
    }
    true
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
            recently_applied_fills: VecDeque::new(),
            terminal_fill_expiry_refresher: None,
            alert_fn: None,
        }
    }

    /// Inject a best-effort trade lifecycle alert sink (DISCORD-TRADE-LIFECYCLE-ALERTS-01).
    ///
    /// The sink is called synchronously inside `tick()` *after* each durable
    /// DB write completes.  It must never panic and must never propagate errors
    /// into the trading path.  The production implementation spawns an async
    /// Discord delivery task; tests inject a recording closure.
    ///
    /// Calling this is optional.  When not set, all lifecycle events are
    /// silently dropped (no-op).
    pub fn set_alert_sink(
        &mut self,
        f: std::sync::Arc<dyn Fn(crate::TradeLifecycleEvent) + Send + Sync>,
    ) {
        self.alert_fn = Some(f);
    }

    /// Fire a lifecycle alert via the injected sink (no-op when not set).
    ///
    /// Errors inside the sink are swallowed here — the trading path must not
    /// be affected by alert delivery failures.
    pub(super) fn fire_alert(&self, event: crate::TradeLifecycleEvent) {
        if let Some(ref f) = self.alert_fn {
            // Catch panics inside the sink so a broken alert implementation
            // cannot crash the orchestrator tick loop.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(event)));
            if result.is_err() {
                tracing::warn!(
                    "trade_lifecycle_alert_sink_panicked (best-effort; trading unaffected)"
                );
            }
        }
    }

    /// Set the terminal-fill expiry refresher.
    ///
    /// Called after construction in production to wire the Alpaca REST adapter.
    /// Tests that exercise the forced-refresh path inject a pure closure here.
    /// (RECONCILE-DRIFT-AFTER-TERMINAL-FILL-FRESH-SNAPSHOT-01)
    pub fn set_terminal_fill_expiry_refresher(
        &mut self,
        f: Box<dyn Fn() -> Option<BrokerSnapshot> + Send + Sync>,
    ) {
        self.terminal_fill_expiry_refresher = Some(f);
    }

    /// Whether a terminal-fill expiry refresher is configured.
    ///
    /// Used by daemon wiring tests (PAPER-TERMINAL-FILL-REFRESHER-AND-RETEST-01)
    /// to prove the refresher is wired for Paper+Alpaca regardless of
    /// `broker_snapshot` seed state at run start.
    pub fn has_terminal_fill_expiry_refresher(&self) -> bool {
        self.terminal_fill_expiry_refresher.is_some()
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
        // PAPER-SOAK-INBOUND-DRAIN-OWNERSHIP-01: `run` stays in scope through
        // Phase 1 below (no longer block-scoped) so its `stop_requested_at_utc`
        // can gate new-order admission without a second `fetch_run` round trip.
        let run = mqk_db::fetch_run(&self.pool, self.run_id).await?;
        if matches!(run.status, mqk_db::RunStatus::Halted) {
            return Err(anyhow!(
                "HALT_GUARD: run {} is HALTED - tick refused (I9-1)",
                self.run_id
            ));
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
                self.fire_alert(crate::TradeLifecycleEvent::RecoveryQuarantine {
                    run_id: self.run_id,
                });
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
        // - DIRTY  => check pending-fill-propagation window (see below)
        // - STALE  => HALT run + DISARM arm state + refuse dispatch
        //
        // This is intentionally fail-closed.
        //
        // AUTON-SENT-ORDER-BROKER-TRUTH-01: inbox-pending guard.
        //
        // The local reconcile snapshot is built from the execution snapshot
        // published after the PREVIOUS tick.  WS-ingested fill/ack events can
        // arrive between ticks and sit in oms_inbox (applied_at_utc IS NULL)
        // before Phase 3 processes them.  Comparing a stale local snapshot
        // (no fill applied) against the broker snapshot (fill reflected) causes
        // a false-positive dirty reconcile that halts the run before Phase 3
        // ever gets to apply the inbox rows.
        //
        // Guard: skip the inline reconcile check when there are unapplied inbox
        // rows for this run.  Phase 3 will drain them; the reconcile check runs
        // on the next tick once the local snapshot is current.
        //
        // Safety: a single-tick deferral cannot suppress genuine drift
        // indefinitely — Phase 3 either applies the rows (resolving the
        // discrepancy) or halts on an OMS error (UNKNOWN_ORDER_FILL, invariant
        // violation, etc.).  If the inbox is permanently stuck (DB failure),
        // the fetch itself returns Err and the tick halts.
        //
        // RECONCILE-DRIFT-AFTER-FAST-PAPER-FILL-01: pending-fill-propagation
        // deferral for the ack→fill window.
        //
        // Even when the inbox is empty, a false-positive DIRTY reconcile can
        // occur when:
        //   1. The ACK inbox rows have already been applied.
        //   2. The WS fill event has NOT yet arrived in oms_inbox at all.
        //   3. The broker REST snapshot already reflects the fill.
        //
        // In this window the outbox row is still SENT (not yet ACKED — that
        // only happens when the terminal fill/cancel event applies in Phase 3)
        // and its broker_order_map entry is still present.
        //
        // Guard: when DIRTY fires and the inbox is empty, check for SENT outbox
        // rows with confirmed broker_order_map entries that were sent within the
        // grace window.  If such rows exist AND the drift is directionally
        // consistent with their expected fills (same symbol, same side, within
        // qty), defer rather than halt.  The fill event either arrives (resolving
        // drift in Phase 3) or the window expires (RECONCILE_DRIFT fires on the
        // next tick when in_grace is false).
        //
        // RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01: terminal-fill settle window.
        //
        // A second false-positive case occurs AFTER the fill is applied:
        //   1. Phase 3 applied the terminal fill this tick or a prior tick.
        //   2. broker_map_remove ran → outbox_load_sent_with_broker_map returns 0.
        //   3. The in-grace path above does NOT fire (in_grace = false).
        //   4. The broker REST snapshot still shows the pre-fill position.
        //
        // This is structurally different from the pending-fill case: the fill
        // HAS been applied to the local portfolio but the broker snapshot hasn't
        // refreshed yet.  The local-ahead delta (lq - bq > 0 for a buy) is the
        // mirror of the pending-fill case (bq - lq > 0).
        //
        // Guard: when the pending-fill path does not cover the drift, also check
        // the in-memory `recently_applied_fills` ring buffer.  If a terminal fill
        // was applied within `TERMINAL_FILL_SETTLE_GRACE_SECS` AND the drift is
        // directionally consistent with those fills, defer rather than halt.
        //
        // RECONCILE_DRIFT still fires immediately when:
        //   - The broker snapshot is stale (Stale error path; unchanged).
        //   - No SENT+mapped orders exist for this run (no evidence of a pending
        //     fill; genuine drift).
        //   - SENT+mapped orders exist but are outside the grace window (fill
        //     never arrived; genuine drift).
        //   - The drift direction/symbol does not match any pending fill
        //     (unexpected broker state; genuine drift).
        //   - The broker has unknown orders we did not submit (fail-closed).
        //   - Local active orders missing at broker are not in the SENT+mapped
        //     set (unmapped drift; fail-closed).
        //   - The terminal-fill settle window expires and broker snapshot still
        //     disagrees (genuine drift or broker reporting error).
        // ------------------------------------------------------------------
        {
            let pending_inbox =
                mqk_db::inbox_load_unapplied_for_run(&self.pool, self.run_id).await?;
            if pending_inbox.is_empty() {
                let local = (self.local_snapshot_provider)();
                let broker = (self.broker_snapshot_provider)();
                match evaluate_monotonic_reconcile(&mut self.reconcile_watermark, &local, &broker) {
                    Ok(()) => {}
                    Err(MonotonicReconcileError::Dirty) => {
                        let sent_mapped = mqk_db::outbox_load_sent_with_broker_map_for_run(
                            &self.pool,
                            self.run_id,
                        )
                        .await?;
                        let now = self.time_source.now_utc();
                        let in_grace = sent_mapped.iter().any(|row| {
                            row.sent_at_utc.is_some_and(|sent_at| {
                                let elapsed = (now - sent_at).num_seconds();
                                (0..PENDING_FILL_PROPAGATION_GRACE_SECS).contains(&elapsed)
                            })
                        });
                        if in_grace
                            && drift_is_consistent_with_pending_fills(&sent_mapped, &local, &broker)
                        {
                            tracing::debug!(
                                run_id = %self.run_id,
                                sent_mapped_count = sent_mapped.len(),
                                grace_secs = PENDING_FILL_PROPAGATION_GRACE_SECS,
                                "reconcile_deferred_pending_fill_propagation: \
                                 SENT+mapped orders within grace window and drift \
                                 consistent with pending fills; re-evaluate after \
                                 WS fill event arrives \
                                 (RECONCILE-DRIFT-AFTER-FAST-PAPER-FILL-01)"
                            );
                        } else if drift_is_consistent_with_recent_terminal_fills(
                            &self.recently_applied_fills,
                            &local,
                            &broker,
                            now,
                        ) {
                            // RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01: local portfolio
                            // advanced after terminal fill; broker snapshot not yet
                            // refreshed.  Defer until broker catches up or grace expires.
                            tracing::debug!(
                                run_id = %self.run_id,
                                grace_secs = TERMINAL_FILL_SETTLE_GRACE_SECS,
                                recent_fills = self.recently_applied_fills.len(),
                                "reconcile_deferred_terminal_fill_settle: \
                                 recently applied terminal fill explains drift; \
                                 re-evaluating after broker snapshot refreshes \
                                 (RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01)"
                            );
                        } else {
                            // RECONCILE-DRIFT-AFTER-TERMINAL-FILL-FRESH-SNAPSHOT-01:
                            // Grace expired or drift direction is not explained by
                            // in-grace fills.  Before halting, check if a recently-
                            // expired terminal fill (within the lookahead window) still
                            // direction-explains the drift.  If so, force one final
                            // broker snapshot refresh and re-evaluate:
                            //
                            //   - Clean after refresh → broker finally caught up; no halt.
                            //   - Still dirty after refresh → halt with fresh evidence.
                            //   - Refresh unavailable/failed → fail closed (halt).
                            //   - No explaining expired fill → genuine drift; halt.
                            let should_try_refresh = has_expired_terminal_fill_explaining_drift(
                                &self.recently_applied_fills,
                                &local,
                                &broker,
                                now,
                            );
                            if should_try_refresh {
                                match self.terminal_fill_expiry_refresher.as_ref().map(|f| f()) {
                                    Some(Some(fresh_broker)) => {
                                        tracing::info!(
                                            run_id = %self.run_id,
                                            "terminal_fill_expiry_refresh_attempted: \
                                             grace expired; re-evaluating with fresh \
                                             broker snapshot \
                                             (RECONCILE-DRIFT-AFTER-TERMINAL-FILL-FRESH-SNAPSHOT-01)"
                                        );
                                        match evaluate_monotonic_reconcile(
                                            &mut self.reconcile_watermark,
                                            &local,
                                            &fresh_broker,
                                        ) {
                                            Ok(()) => {
                                                tracing::info!(
                                                    run_id = %self.run_id,
                                                    "terminal_fill_expiry_refresh_resolved: \
                                                     fresh broker snapshot matches local \
                                                     position; RECONCILE_DRIFT suppressed \
                                                     (RECONCILE-DRIFT-AFTER-TERMINAL-FILL-FRESH-SNAPSHOT-01)"
                                                );
                                                // Fresh snapshot is clean — continue.
                                            }
                                            Err(MonotonicReconcileError::Dirty) => {
                                                tracing::error!(
                                                    run_id = %self.run_id,
                                                    "terminal_fill_expiry_refresh_still_dirty: \
                                                     fresh broker snapshot still mismatches; \
                                                     RECONCILE_DRIFT confirmed \
                                                     (RECONCILE-DRIFT-AFTER-TERMINAL-FILL-FRESH-SNAPSHOT-01)"
                                                );
                                                persist_halt_and_disarm(
                                                    &self.pool,
                                                    self.run_id,
                                                    now,
                                                    "ReconcileDrift",
                                                )
                                                .await?;
                                                self.fire_alert(crate::TradeLifecycleEvent::ReconcileDriftHalt {
                                                    run_id: self.run_id,
                                                    reason: "fresh_snapshot_still_mismatches".to_string(),
                                                });
                                                return Err(anyhow!(
                                                    "RECONCILE_DRIFT: run {} halted and disarmed; \
                                                     fresh broker snapshot still mismatches after \
                                                     terminal-fill expiry refresh; dispatch refused",
                                                    self.run_id
                                                ));
                                            }
                                            Err(MonotonicReconcileError::Stale(stale)) => {
                                                tracing::error!(
                                                    run_id = %self.run_id,
                                                    stale = %stale,
                                                    "terminal_fill_expiry_refresh_stale: \
                                                     refreshed snapshot rejected as stale \
                                                     (RECONCILE-DRIFT-AFTER-TERMINAL-FILL-FRESH-SNAPSHOT-01)"
                                                );
                                                persist_halt_and_disarm(
                                                    &self.pool,
                                                    self.run_id,
                                                    now,
                                                    "ReconcileDrift",
                                                )
                                                .await?;
                                                self.fire_alert(crate::TradeLifecycleEvent::ReconcileDriftHalt {
                                                    run_id: self.run_id,
                                                    reason: "reconcile_snapshot_stale_after_refresh".to_string(),
                                                });
                                                return Err(anyhow!(
                                                    "RECONCILE_SNAPSHOT_STALE: run {} halted; \
                                                     refreshed snapshot stale after \
                                                     terminal-fill expiry: {}",
                                                    self.run_id,
                                                    stale
                                                ));
                                            }
                                        }
                                    }
                                    Some(None) | None => {
                                        // Refresher absent or fetch failed — fail closed.
                                        tracing::error!(
                                            run_id = %self.run_id,
                                            refresher_configured =
                                                self.terminal_fill_expiry_refresher.is_some(),
                                            "terminal_fill_expiry_refresh_unavailable: \
                                             terminal-fill settle grace expired; broker \
                                             snapshot refresh unavailable or failed; \
                                             halting fail-closed \
                                             (RECONCILE-DRIFT-AFTER-TERMINAL-FILL-FRESH-SNAPSHOT-01)"
                                        );
                                        persist_halt_and_disarm(
                                            &self.pool,
                                            self.run_id,
                                            now,
                                            "ReconcileDrift",
                                        )
                                        .await?;
                                        self.fire_alert(
                                            crate::TradeLifecycleEvent::ReconcileDriftHalt {
                                                run_id: self.run_id,
                                                reason: "settle_grace_expired_refresh_unavailable"
                                                    .to_string(),
                                            },
                                        );
                                        return Err(anyhow!(
                                            "RECONCILE_DRIFT: run {} halted and disarmed; \
                                             terminal-fill settle grace expired and broker \
                                             snapshot refresh unavailable; dispatch refused",
                                            self.run_id
                                        ));
                                    }
                                }
                            } else {
                                // No explaining fills in lookahead window — genuine drift.
                                persist_halt_and_disarm(
                                    &self.pool,
                                    self.run_id,
                                    now,
                                    "ReconcileDrift",
                                )
                                .await?;
                                self.fire_alert(crate::TradeLifecycleEvent::ReconcileDriftHalt {
                                    run_id: self.run_id,
                                    reason: "genuine_drift".to_string(),
                                });
                                return Err(anyhow!(
                                    "RECONCILE_DRIFT: run {} halted and disarmed; \
                                     dispatch refused",
                                    self.run_id
                                ));
                            }
                        }
                    }
                    Err(MonotonicReconcileError::Stale(stale)) => {
                        let now = self.time_source.now_utc();
                        // Mandatory halt + disarm - same fail-closed contract as Phase 0b.
                        persist_halt_and_disarm(&self.pool, self.run_id, now, "ReconcileDrift")
                            .await?;
                        self.fire_alert(crate::TradeLifecycleEvent::ReconcileDriftHalt {
                            run_id: self.run_id,
                            reason: "reconcile_snapshot_stale".to_string(),
                        });
                        return Err(anyhow!(
                            "RECONCILE_SNAPSHOT_STALE: run {} halted and disarmed; \
                             dispatch refused: {}",
                            self.run_id,
                            stale
                        ));
                    }
                }
            } else {
                tracing::debug!(
                    run_id = %self.run_id,
                    pending_count = pending_inbox.len(),
                    "reconcile_check_deferred: unapplied inbox rows present; \
                     reconcile will re-evaluate after Phase 3 drains the inbox \
                     (AUTON-SENT-ORDER-BROKER-TRUTH-01)"
                );
            }
        }
        // ------------------------------------------------------------------
        // Phase 1: Claim and dispatch outbox rows.
        //
        // PAPER-SOAK-INBOUND-DRAIN-OWNERSHIP-01: once an operator stop has
        // been durably requested (runs.stop_requested_at_utc, migration
        // 0063), no new outbox row is claimed or dispatched -- this is the
        // "no new economic order creation during drainage" gate. Phase 2/3
        // below are NOT gated by this and keep running exactly as before, so
        // broker-reachable orders already in flight (SENT/ACKED/DISPATCHING/
        // AMBIGUOUS) still drain to a terminal outcome via ordinary inbound
        // processing. `claimed` is simply empty in this case; nothing else
        // about Phase 1's shape changes.
        // ------------------------------------------------------------------
        let claimed = if run.stop_requested_at_utc.is_some() {
            Vec::new()
        } else {
            self.refresh_or_acquire_runtime_leadership().await?;
            // PAPER-SOAK-STALE-CLAIM-RECOVERY-03: the fenced claim call
            // re-verifies run RUNNING + exact lease identity atomically with
            // the claim mutation itself, closing the check-then-act gap
            // between the refresh above and this claim (§24 — "a separate
            // check followed by a separate mutation is NOT an atomic
            // fence").
            let epoch = self.runtime_epoch.ok_or_else(|| {
                anyhow!(
                    "INVARIANT_VIOLATION: run {} refresh_or_acquire_runtime_leadership \
                     succeeded without setting runtime_epoch",
                    self.run_id
                )
            })?;
            match mqk_db::outbox_claim_batch_for_run_with_lease_authority(
                &self.pool,
                self.run_id,
                &self.runtime_holder_id,
                epoch,
                1,
                &self.dispatcher_id,
                self.time_source.now_utc(),
            )
            .await?
            {
                mqk_db::FencedClaimOutcome::Claimed(rows) => rows,
                mqk_db::FencedClaimOutcome::RunNotRunning { actual_status } => {
                    self.runtime_epoch = None;
                    return Err(anyhow!(
                        "RUNTIME_FENCED_RUN_NOT_RUNNING: run {} is '{}', not RUNNING — claim \
                         refused at the pre-mutation dispatch fence (no lease/claim/dispatch, \
                         no re-halt)",
                        self.run_id,
                        actual_status
                    ));
                }
                mqk_db::FencedClaimOutcome::LeaseInvalid => {
                    self.runtime_epoch = None;
                    let now = self.time_source.now_utc();
                    persist_halt_and_disarm(&self.pool, self.run_id, now, "LeaderLeaseLost")
                        .await?;
                    return Err(anyhow!(
                        "RUNTIME_LEASE_LOST: run {} holder={} epoch={} — claim-time lease \
                         re-verification failed",
                        self.run_id,
                        self.runtime_holder_id,
                        epoch,
                    ));
                }
            }
        };
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
                    // EXEC-CONT-01: durable halt+disarm on inbound continuity failure.
                    // This is mandatory — the same fail-closed contract that applies to
                    // RecoveryQuarantine, ReconcileDrift, IntegrityViolation, etc.
                    // Without it, runs.status stays RUNNING and sys_arm_state stays ARMED
                    // across a crash/restart, bypassing the Phase-0 halt guard on the
                    // next tick call.  The cursor was already persisted in the block above.
                    let now = self.time_source.now_utc();
                    persist_halt_and_disarm(
                        &self.pool,
                        self.run_id,
                        now,
                        "InboundContinuityUnproven",
                    )
                    .await?;
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
            // REB-01: Skip fill events for orders with no ownership in this run.
            //
            // REST polling may fetch historical fills from prior runs when the
            // broker cursor's rest_activity_after position is anchored before
            // those fills (e.g. first start, cursor reset, or baseline adoption).
            // Historical fills carry internal_order_ids that were never submitted
            // by this run — they are not in self.oms_orders and would halt with
            // UNKNOWN_ORDER_FILL in Phase 3 apply (Section C).
            //
            // Guard: only skip when the order is absent from this run's OMS map.
            // Any order submitted by this run is present in self.oms_orders:
            //   - at startup via recover_oms_and_portfolio (outbox_load_submitted)
            //   - during Phase 1 dispatch (self.oms_orders.insert at submit time)
            // Historical fills for orders from prior runs are therefore reliably
            // identified by absence from self.oms_orders.
            //
            // Non-fill events (Ack, CancelAck, etc.) are safe to ingest for
            // unknown orders: Phase 3 apply silently skips them without halting.
            if is_historical_fill_no_run_ownership(event, &self.oms_orders) {
                tracing::info!(
                    run_id = %self.run_id,
                    broker_message_id = %event.broker_message_id(),
                    internal_order_id = %event.internal_order_id(),
                    "exec_broker_fill_skipped_no_run_ownership"
                );
                continue;
            }
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
            // OBS-TIME-01: propagate real broker event time from the Alpaca
            // broker_message_id ("alpaca:{order_id}:{event}:{iso_ts}").
            // Returns 0 for non-Alpaca adapters — safe default, no regression.
            let event_ts_ms = mqk_broker_alpaca::normalize::alpaca_event_ts_ms_from_message_id(
                event.broker_message_id(),
            );
            let _inserted = mqk_db::inbox_insert_deduped_with_identity(
                &self.pool,
                self.run_id,
                event.broker_message_id(),
                event.broker_fill_id(),
                event.internal_order_id(),
                event.broker_order_id().unwrap_or(event.internal_order_id()),
                event_kind,
                &msg_json,
                event_ts_ms,
                now,
            )
            .await?;
            tracing::info!(
                run_id = %self.run_id,
                order_id = %event.internal_order_id(),
                event_kind = %event_kind,
                "exec_broker_event_ingested"
            );
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
            // WS-REB-01: fills that reached the inbox via WS ingest for orders
            // with no run ownership.  The Phase 2 REB-01 guard only covers
            // REST-polled events; WS-ingested fills for orphan orders bypass
            // that guard and must be intercepted here before Section C halts.
            if is_historical_fill_no_run_ownership(&event, &self.oms_orders) {
                tracing::info!(
                    run_id = %self.run_id,
                    broker_message_id = %msg_id,
                    internal_order_id = %internal_id,
                    "exec_broker_fill_skipped_no_run_ownership_phase3"
                );
                let now = self.time_source.now_utc();
                mqk_db::inbox_mark_applied(&self.pool, self.run_id, &msg_id, now).await?;
                continue;
            }
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
                    // OPTR-LABEL-01: use BROKER_EVENT_APPLY_FAILED as the outer
                    // operator-visible label.  The inner error already names the
                    // specific cause:
                    //   - "UNKNOWN_ORDER_FILL: ..."  for fills on absent OMS orders
                    //   - "OMS transition error for '...': ..."  for illegal transitions
                    // Wrapping both as UNKNOWN_ORDER_FILL mislabels OMS transition
                    // failures in operator logs and traces.
                    return Err(e.context(format!(
                        "BROKER_EVENT_APPLY_FAILED: run {} halted and disarmed (Section C)",
                        self.run_id
                    )));
                }
            };
            // EXEC-OBS-LIVENESS-01: structured apply milestone log.
            // Fill events include symbol/side/qty/price for chain reconstruction.
            {
                let ek = match &event {
                    BrokerEvent::Ack { .. } => "ack",
                    BrokerEvent::PartialFill { .. } => "partial_fill",
                    BrokerEvent::Fill { .. } => "fill",
                    BrokerEvent::CancelAck { .. } => "cancel_ack",
                    BrokerEvent::CancelReject { .. } => "cancel_reject",
                    BrokerEvent::ReplaceAck { .. } => "replace_ack",
                    BrokerEvent::ReplaceReject { .. } => "replace_reject",
                    BrokerEvent::Reject { .. } => "reject",
                };
                match &event {
                    BrokerEvent::Fill {
                        symbol,
                        side,
                        delta_qty,
                        price_micros,
                        ..
                    }
                    | BrokerEvent::PartialFill {
                        symbol,
                        side,
                        delta_qty,
                        price_micros,
                        ..
                    } => {
                        tracing::info!(
                            run_id = %self.run_id,
                            order_id = %internal_id,
                            event_kind = %ek,
                            symbol = %symbol,
                            side = ?side,
                            delta_qty = %delta_qty,
                            price_micros = %price_micros,
                            terminal = %apply_outcome.terminal_apply_succeeded,
                            "exec_broker_event_applied"
                        );
                    }
                    _ => {
                        tracing::info!(
                            run_id = %self.run_id,
                            order_id = %internal_id,
                            event_kind = %ek,
                            terminal = %apply_outcome.terminal_apply_succeeded,
                            "exec_broker_event_applied"
                        );
                    }
                }
            }

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
            // RECOVERY-QUARANTINE-AFTER-PAPER-FILL-01: advance oms_outbox status
            // SENT → ACKED before removing the broker_order_map entry.
            //
            // Phase 0b (outbox_load_restart_ambiguous_for_run) quarantines any
            // SENT outbox row whose broker_order_map entry is absent.  Without
            // this call, the sequence is:
            //   terminal fill applied → broker_map_remove (map entry gone)
            //   next tick Phase 0b: SENT + no broker_map → RECOVERY_QUARANTINE
            //
            // By marking ACKED first, the outbox row is excluded from the
            // ambiguous query on the very next tick.  The call is idempotent
            // (already-ACKED rows return Ok(false)); a missing row is also
            // treated as a no-op.
            //
            // Crash-safety ordering (critical):
            //   1. outbox_mark_acked   → SENT → ACKED durably in DB
            //   2. broker_map_remove   → entry deleted
            //   3. inbox_mark_applied  → applied_at_utc written (end of loop)
            //
            // Crash between (1) and (2): ACKED + broker_map present.
            //   Phase 0b: no SENT row → no quarantine.  Broker map entry is
            //   harmless; cleanup retried via Phase 3 replay of the still-
            //   unapplied inbox row on restart.
            //
            // Crash between (2) and (3): ACKED + no broker_map + inbox row
            //   unapplied.  Phase 0b: no SENT row → no quarantine.  Phase 3
            //   replays and calls outbox_mark_acked again (idempotent Ok(false))
            //   then broker_map_remove (silent no-op).
            //
            // EXE-03R: terminal lifecycle events must clear broker-order mappings
            // before the inbox row is marked applied. If a crash occurs after
            // mark_applied but before cleanup, the durable inbox fence would
            // suppress replay and strand a stale broker mapping permanently.
            // RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01: track fill delta so Phase 0c
            // can defer RECONCILE_DRIFT during broker snapshot settlement.
            // Track both partial and terminal fills: a partial+terminal sequence
            // contributes cumulative position drift that the broker snapshot may
            // not yet reflect.  Tracking only the terminal fill understates the
            // expected delta and produces a false-positive genuine_drift halt.
            if let BrokerEvent::Fill {
                symbol,
                side,
                delta_qty,
                ..
            }
            | BrokerEvent::PartialFill {
                symbol,
                side,
                delta_qty,
                ..
            } = &event
            {
                let signed = if matches!(side, mqk_execution::Side::Buy) {
                    *delta_qty
                } else {
                    -*delta_qty
                };
                if self.recently_applied_fills.len() >= RECENT_FILLS_RING_CAP {
                    self.recently_applied_fills.pop_front();
                }
                self.recently_applied_fills.push_back(RecentTerminalFill {
                    symbol: symbol.clone(),
                    signed_delta: signed,
                    applied_at: self.time_source.now_utc(),
                });
            }
            if apply_outcome.terminal_apply_succeeded {
                mqk_db::outbox_mark_acked(&self.pool, &internal_id).await?;
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
            // DISCORD-TRADE-LIFECYCLE-ALERTS-01: best-effort alert after durable apply.
            // Fires only for Ack and Fill/PartialFill — the events operators care about.
            match &event {
                BrokerEvent::Ack {
                    broker_order_id,
                    internal_order_id,
                    ..
                } => {
                    // Derive symbol from OMS order if available (it is — Ack is applied
                    // before the order map entry is removed).
                    let sym = self
                        .oms_orders
                        .get(internal_order_id.as_str())
                        .map(|o| o.symbol.clone());
                    self.fire_alert(crate::TradeLifecycleEvent::OrderAcked {
                        run_id: self.run_id,
                        order_id: internal_order_id.clone(),
                        broker_order_id: broker_order_id.clone(),
                        symbol: sym,
                    });
                }
                BrokerEvent::Fill {
                    symbol,
                    side,
                    delta_qty,
                    price_micros,
                    internal_order_id,
                    ..
                }
                | BrokerEvent::PartialFill {
                    symbol,
                    side,
                    delta_qty,
                    price_micros,
                    internal_order_id,
                    ..
                } => {
                    let partial = matches!(&event, BrokerEvent::PartialFill { .. });
                    self.fire_alert(crate::TradeLifecycleEvent::FillApplied {
                        run_id: self.run_id,
                        order_id: internal_order_id.clone(),
                        symbol: symbol.clone(),
                        side: format!("{:?}", side),
                        qty: *delta_qty,
                        price_micros: *price_micros,
                        terminal: !partial,
                    });
                }
                _ => {}
            }
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
    /// RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01: inject a recent terminal fill
    /// into the settle window buffer for testing purposes.
    ///
    /// Allows tests to simulate the Phase 3 recording without running a full
    /// tick with broker events.  Only compiled under test/testkit builds.
    #[cfg(any(test, feature = "testkit"))]
    pub fn inject_recent_terminal_fill_for_test(
        &mut self,
        symbol: impl Into<String>,
        signed_delta: i64,
        applied_at: chrono::DateTime<chrono::Utc>,
    ) {
        if self.recently_applied_fills.len() >= RECENT_FILLS_RING_CAP {
            self.recently_applied_fills.pop_front();
        }
        self.recently_applied_fills.push_back(RecentTerminalFill {
            symbol: symbol.into(),
            signed_delta,
            applied_at,
        });
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
        // RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01: expose whether any terminal fill
        // is within the settle grace window so the execution loop can force an eager
        // external broker REST refresh.  Extracted before the await so &self is not
        // live across the suspension point.
        let has_recent_terminal_fill = self.recently_applied_fills.iter().any(|f| {
            let elapsed = (now - f.applied_at).num_seconds();
            (0..TERMINAL_FILL_SETTLE_GRACE_SECS).contains(&elapsed)
        });
        // RISK-ENGINE-HALTED-VISIBILITY-01: read-only passthrough to the live
        // risk gate's sticky halt flag. Extracted before the await alongside
        // the other in-memory overlays.
        let risk_engine_sticky_halt = self.gateway.risk_engine_sticky_halt();
        // `self` is no longer borrowed here - safe to `.await` without Sync.
        let mut snap = crate::observability::collect_db_snapshot(&pool, run_id, now).await?;
        snap.active_orders = active_orders;
        snap.portfolio = portfolio;
        snap.recent_risk_denials = recent_denials;
        snap.has_recent_terminal_fill = has_recent_terminal_fill;
        snap.risk_engine_sticky_halt = risk_engine_sticky_halt;
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
    /// PAPER-SOAK-STALE-CLAIM-RECOVERY-03: run-aware, transactionally-fenced
    /// leadership refresh/acquire.
    ///
    /// Routed through `mqk_db::runtime_lease::acquire_or_refresh_lease_for_running_run`
    /// instead of the unfenced `refresh_lease`/`acquire_lease` primitives —
    /// see that function's doc comment for the serialization boundary it
    /// shares with `clear_halted_run_and_reset_stale_claims`. The critical
    /// behavioral difference from the pre-03 implementation is
    /// `RunNotRunning`: when the run is no longer durably `RUNNING` (e.g.
    /// already `STOPPED` by an operator's `clear-halted-run`), this function
    /// must NOT call `persist_halt_and_disarm` — doing so would let a stale
    /// runtime that resumes after a successful clear rewrite `STOPPED` back
    /// to `HALTED` (§9 rehalt hazard). `RunNotRunning` is therefore a fenced
    /// refusal, distinct from genuine lease loss/contest (`Lost`,
    /// `HeldByOther`), which keep the existing fail-closed halt+disarm
    /// behavior unchanged.
    async fn refresh_or_acquire_runtime_leadership(&mut self) -> anyhow::Result<()> {
        let now = self.time_source.now_utc();
        let outcome = mqk_db::runtime_lease::acquire_or_refresh_lease_for_running_run(
            &self.pool,
            self.run_id,
            &self.runtime_holder_id,
            self.runtime_epoch,
            now,
            self.runtime_lease_ttl_secs,
        )
        .await?;

        match outcome {
            mqk_db::runtime_lease::RunLeaseAuthorityOutcome::Acquired(lease)
            | mqk_db::runtime_lease::RunLeaseAuthorityOutcome::Refreshed(lease) => {
                self.runtime_epoch = Some(lease.epoch);
                Ok(())
            }
            mqk_db::runtime_lease::RunLeaseAuthorityOutcome::RunNotRunning { actual_status } => {
                self.runtime_epoch = None;
                Err(anyhow!(
                    "RUNTIME_FENCED_RUN_NOT_RUNNING: run {} is '{}', not RUNNING — dispatch \
                     authority refused (no lease acquired/refreshed, no claim, no dispatch, \
                     and no re-halt of a non-HALTED run)",
                    self.run_id,
                    actual_status
                ))
            }
            mqk_db::runtime_lease::RunLeaseAuthorityOutcome::Lost => {
                self.runtime_epoch = None;
                persist_halt_and_disarm(&self.pool, self.run_id, now, "LeaderLeaseLost").await?;
                Err(anyhow!(
                    "RUNTIME_LEASE_LOST: run {} holder={} — refresh CAS failed (holder \
                     mismatch, epoch mismatch, or lease already expired)",
                    self.run_id,
                    self.runtime_holder_id,
                ))
            }
            mqk_db::runtime_lease::RunLeaseAuthorityOutcome::HeldByOther(current) => {
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
// REB-01: Historical fill gate
// ---------------------------------------------------------------------------

/// Returns `true` when a fetched broker event is a fill (Fill or PartialFill)
/// for an order that is absent from this run's OMS map.
///
/// Fills for absent orders are **historical fills from prior runs**.  They were
/// fetched by REST polling because the broker cursor's `rest_activity_after`
/// position was anchored before those activities (e.g. first start, cursor
/// reset, or baseline adoption).  Ingesting them as active-run events would
/// produce `UNKNOWN_ORDER_FILL` in Phase 3 apply and halt the run incorrectly.
///
/// Non-fill events (Ack, CancelAck, etc.) are excluded from this gate because
/// Phase 3 apply already silently skips non-fill events for unknown orders
/// without halting — ingesting them is harmless.
///
/// # Invariant preserved
///
/// Any order submitted by the current run is present in `oms_orders`:
/// - At run start via `recover_oms_and_portfolio` (loads outbox rows).
/// - During Phase 1 dispatch (inserted immediately at submit time).
///
/// A fill for an order absent from `oms_orders` therefore cannot belong to
/// the current run under correct code paths.
pub fn is_historical_fill_no_run_ownership(
    event: &BrokerEvent,
    oms_orders: &std::collections::BTreeMap<String, mqk_execution::oms::state_machine::OmsOrder>,
) -> bool {
    matches!(
        event,
        BrokerEvent::PartialFill { .. } | BrokerEvent::Fill { .. }
    ) && !oms_orders.contains_key(event.internal_order_id())
}

fn derive_runtime_holder_id(dispatcher_id: &str, run_id: Uuid) -> String {
    // `dispatcher_id` is produced by `default_node_id` which already embeds
    // "{service}|{host}|{user}|pid={pid}".  Re-reading COMPUTERNAME/USERNAME/
    // PID here produced a doubled string in the DB lease row:
    //   "mqk-daemon|HOST|USER|pid=N|HOST|USER|pid=N|run=..."
    // Appending only the run_id is sufficient to scope the holder to this
    // specific execution run while keeping the lease row readable.
    format!("{dispatcher_id}|run={run_id}")
}
// ---------------------------------------------------------------------------
// Internal helper - mandatory halt + disarm persistence
// ---------------------------------------------------------------------------
/// Atomically write `runs.status = 'HALTED'` and `sys_arm_state = 'DISARMED'`
/// through [`mqk_db::halt_and_disarm_run_if_active`] — the single fenced
/// authority boundary every safety-halt caller in this module shares
/// (PRE-SOAK-RUNTIME-HALT-FENCE-CAS-01).
///
/// # Fenced-refusal contract
///
/// By the time any caller here decides to safety-halt, it has already
/// learned its lease/claim authority for `run_id` was lost, contested, or
/// invalidated (`Lost`, `HeldByOther`, `LeaseInvalid`) — but that decision
/// was reached in a transaction that has *already released* the `runs` row
/// lock before returning. A concurrent operator recovery
/// (`clear_halted_run_and_reset_stale_claims`) can win that lock in the gap
/// between the decision and this call. `halt_and_disarm_run_if_active`
/// re-proves authority under its own row lock rather than trusting the
/// stale decision: if the run is no longer `RUNNING` by the time this call
/// actually executes, it performs **zero** mutation and this function
/// returns a distinct `RUNTIME_SAFETY_HALT_SUPERSEDED` error instead of the
/// caller's original halt reason — every callsite's existing
/// `persist_halt_and_disarm(...).await?; return Err(anyhow!(<original
/// reason>))` shape means that superseded error propagates immediately via
/// `?`, and the original (now-misleading) reason is never returned. No
/// per-caller status checks are needed; centralizing the fence here covers
/// every reason (`RecoveryQuarantine`, `ReconcileDrift`, `LeaderLeaseLost`,
/// `LeaderLeaseUnavailable`, `InboundContinuityUnproven`,
/// `IntegrityViolation`, `CancelTargetMissing`, …) uniformly.
///
/// # Fail-closed contract (ordinary halt path)
///
/// When the run is genuinely still `RUNNING` (the common case), the halt+
/// disarm write is mandatory, not best-effort: a DB failure propagates as
/// an explicit `Err` rather than silently leaving `runs.status` as RUNNING,
/// which would let the Phase-0 HALT_GUARD be bypassed on the next tick.
async fn persist_halt_and_disarm(
    pool: &PgPool,
    run_id: Uuid,
    now: chrono::DateTime<chrono::Utc>,
    reason: &'static str,
) -> anyhow::Result<()> {
    #[cfg(test)]
    tests::pause_before_safety_halt_persist().await;

    match mqk_db::halt_and_disarm_run_if_active(pool, run_id, now, reason)
        .await
        .with_context(|| {
            format!(
                "HALT_PERSISTENCE_FAILURE: run {run_id} - fenced safety-halt authority check \
                 failed (reason={reason})"
            )
        })? {
        mqk_db::SafetyHaltOutcome::Halted | mqk_db::SafetyHaltOutcome::AlreadyHalted => {}
        mqk_db::SafetyHaltOutcome::RunNotActive { actual_status } => {
            return Err(anyhow!(
                "RUNTIME_SAFETY_HALT_SUPERSEDED: run {run_id} is '{actual_status}', not \
                 RUNNING — this runtime's halt authority (reason={reason}) was superseded by \
                 recovery before it could persist; no run/arm-state mutation performed, no \
                 broker call permitted"
            ));
        }
    }
    // EVIDENCE-DURABILITY-01: best-effort halt-reason audit event.
    //
    // The two mandatory writes above have already succeeded. This write makes the
    // halt reason queryable from audit_events (topic='orchestrator') so a morning-
    // after operator can answer "why was run X halted?" without relying on logs.
    //
    // Non-fatal: a telemetry write failure must not prevent the halt from being
    // visible via runs.halted_at_utc and sys_arm_state (already persisted above).
    {
        let event_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            format!(
                "mqk-runtime.halt-audit.v1|{}|{}|{}",
                run_id,
                reason,
                now.timestamp_micros(),
            )
            .as_bytes(),
        );
        if let Err(err) = mqk_db::insert_audit_event(
            pool,
            &mqk_db::NewAuditEvent {
                event_id,
                run_id,
                ts_utc: now,
                topic: "orchestrator".to_string(),
                event_type: reason.to_string(),
                payload: serde_json::json!({
                    "halt_reason": reason,
                    "source": "mqk-runtime.orchestrator.persist_halt_and_disarm",
                }),
                hash_prev: None,
                hash_self: None,
            },
        )
        .await
        {
            tracing::warn!(
                run_id = %run_id,
                reason = reason,
                error = %err,
                "EVIDENCE-DURABILITY-01: halt_audit_event write failed (non-fatal)"
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests;
