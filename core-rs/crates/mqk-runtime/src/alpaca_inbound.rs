//! Alpaca websocket inbound integration lane — BRK-00R / BRK-01R / BRK-02R / BRK-07R.
//!
//! This module is the authoritative runtime-level wiring that ties together:
//!   - `mqk_broker_alpaca::inbound` (parse + normalize + `InboundBatch` contract)
//!   - `mqk_db` (durable inbox ingest + broker cursor persistence)
//!
//! It is the only code path that may durably ingest Alpaca websocket trade-update
//! events.  No other module may call `inbox_insert_deduped_with_identity` for
//! websocket-sourced events.
//!
//! # Ingest ordering invariant (BRK-02R)
//!
//! Within `process_ws_inbound_batch`, the ordering is:
//!
//! ```text
//! for each WS message:
//!     1. build_inbound_batch_from_ws_update  → InboundBatch (cursor private)
//!     2. inbox_insert_deduped_with_identity  → durable ingest
//!     3. batch.into_cursor_for_persist()     → cursor becomes accessible
//!     4. update in-memory current_cursor
//! after all messages:
//!     5. advance_broker_cursor               → persist cursor to DB
//! ```
//!
//! The `InboundBatch` type enforces step ordering structurally: the cursor
//! field is private and can only be extracted by the consuming
//! `into_cursor_for_persist` method.  If `inbox_insert` returns `Err`, the
//! function returns immediately — `into_cursor_for_persist` is never called
//! and `advance_broker_cursor` is never reached.
//!
//! # Fail-closed behavior (BRK-07R)
//!
//! When a websocket reconnect creates continuity uncertainty, the caller MUST
//! call `persist_ws_gap_cursor` before resuming event processing.  This writes
//! a `GapDetected` cursor to `broker_event_cursor` so that the REST polling
//! lane (`BrokerAdapter::fetch_events`) returns `InboundContinuityUnproven`
//! and the orchestrator halts rather than processing lifecycle events with
//! potentially missing gaps.
//!
//! # Duplicate and out-of-order safety
//!
//! `inbox_insert_deduped_with_identity` is idempotent on `(run_id,
//! broker_message_id)`.  Duplicate or out-of-order WS messages return
//! `Ok(false)` without error.  The cursor still advances to the last position
//! seen — the system does not stall on replay.
use anyhow::Context as _;
use chrono::{DateTime, Utc};
use mqk_broker_alpaca::types::{AlpacaFetchCursor, AlpacaTradeUpdatesResume};
use mqk_broker_alpaca::{
    build_inbound_batch_from_ws_update, mark_gap_detected, parse_ws_message, AlpacaWsMessage,
};
use mqk_execution::BrokerEvent;
use sqlx::PgPool;
use uuid::Uuid;
// ---------------------------------------------------------------------------
// WsIngestOutcome — result type
// ---------------------------------------------------------------------------
/// The outcome of processing a raw websocket frame through the inbound lane.
#[derive(Debug)]
pub enum WsIngestOutcome {
    /// At least one trade-update event was processed and the cursor was
    /// advanced.  `count` includes both newly-inserted and deduplicated events.
    EventsIngested {
        /// Number of BrokerEvents that passed through the ingest pipeline
        /// (includes deduped events that were already in the inbox).
        count: usize,
        /// The updated cursor after ingesting all events in this frame.
        /// This value has already been persisted to `broker_event_cursor`
        /// before this variant is returned.
        new_cursor: AlpacaFetchCursor,
    },
    /// The frame contained no actionable trade-update events (e.g. it was an
    /// authorization, listening, error, or unknown-type frame).  The cursor
    /// is unchanged and `advance_broker_cursor` was NOT called.
    NoActionableEvents,
}
// ---------------------------------------------------------------------------
// WsLifecycleContinuity — runtime-owned continuity state seam (BRK-00R-01)
// ---------------------------------------------------------------------------
/// Runtime-owned representation of the Alpaca websocket inbound lifecycle continuity state.
///
/// This is the production-facing seam that runtime uses to determine whether the
/// WS inbound lane is in a state where event processing is safe to proceed.
///
/// Derived from the persisted `AlpacaFetchCursor` via [`ws_continuity_from_cursor`].
///
/// # Fail-closed contract
///
/// Only [`WsLifecycleContinuity::Live`] is considered ready for event processing.
/// [`ColdStartUnproven`] and [`GapDetected`] must not be treated as ready by runtime.
/// Runtime code MUST call [`WsLifecycleContinuity::is_ready`] before proceeding with
/// any WS-lane-dependent event processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsLifecycleContinuity {
    /// The WS lane has proven continuity. Runtime may proceed with event processing.
    Live {
        last_message_id: String,
        last_event_at: String,
    },
    /// The WS lane has not yet received its first trade-update event.
    ///
    /// This state arises on cold start when no WS event has been processed since
    /// the cursor was initialized. Runtime MUST NOT treat this as ready.
    ColdStartUnproven,
    /// A continuity gap was detected (e.g., WS disconnect without full replay).
    ///
    /// Runtime MUST NOT treat this as ready. The gap must be resolved (by receiving
    /// new WS events via `process_ws_inbound_batch`) before event processing resumes.
    ///
    /// `last_message_id` and `last_event_at` carry the last-known WS position before
    /// the gap, when available.  Both are `None` when the gap arose from a cold-start
    /// reconnect (no prior position was ever established).
    GapDetected {
        last_message_id: Option<String>,
        last_event_at: Option<String>,
        detail: String,
    },
}
impl WsLifecycleContinuity {
    /// Returns `true` only when the WS lane has proven continuity.
    ///
    /// Returns `false` for `ColdStartUnproven` and `GapDetected`.
    /// Runtime MUST refuse to proceed with WS-lane-dependent event processing
    /// when this returns `false`.
    pub fn is_ready(&self) -> bool {
        matches!(self, WsLifecycleContinuity::Live { .. })
    }
}
/// Derive the runtime-owned [`WsLifecycleContinuity`] from a persisted Alpaca cursor.
///
/// This is the production-facing derivation function. Runtime calls this with the loaded
/// `AlpacaFetchCursor` to determine whether the WS inbound lane is in a state where
/// event processing is safe to proceed.
///
/// # Fail-closed mapping
///
/// | Cursor state                                  | Returns                                    |
/// |-----------------------------------------------|--------------------------------------------|
/// | `AlpacaTradeUpdatesResume::ColdStartUnproven` | `WsLifecycleContinuity::ColdStartUnproven` |
/// | `AlpacaTradeUpdatesResume::GapDetected { .. }` | `WsLifecycleContinuity::GapDetected`      |
/// | `AlpacaTradeUpdatesResume::Live { .. }`       | `WsLifecycleContinuity::Live`              |
pub fn ws_continuity_from_cursor(cursor: &AlpacaFetchCursor) -> WsLifecycleContinuity {
    match &cursor.trade_updates {
        AlpacaTradeUpdatesResume::ColdStartUnproven => WsLifecycleContinuity::ColdStartUnproven,
        AlpacaTradeUpdatesResume::GapDetected {
            last_message_id,
            last_event_at,
            detail,
        } => WsLifecycleContinuity::GapDetected {
            last_message_id: last_message_id.clone(),
            last_event_at: last_event_at.clone(),
            detail: detail.clone(),
        },
        AlpacaTradeUpdatesResume::Live {
            last_message_id,
            last_event_at,
        } => WsLifecycleContinuity::Live {
            last_message_id: last_message_id.clone(),
            last_event_at: last_event_at.clone(),
        },
    }
}
/// Attempt to derive [`WsLifecycleContinuity`] from an opaque broker cursor string.
///
/// Returns `Some(continuity)` if the string parses as an Alpaca `AlpacaFetchCursor` JSON.
/// Returns `None` if the cursor is absent or does not parse as an Alpaca cursor
/// (non-Alpaca adapters such as the paper broker use `None` or non-Alpaca-format
/// cursor strings; those adapters must not be gated by this check).
///
/// # Production use (BRK-00R-03)
///
/// Called by `orchestrator::tick()` Phase 2 when the adapter returns
/// `BrokerError::InboundContinuityUnproven`.  Allows the orchestrator to derive
/// and name the runtime-owned continuity state independently of adapter internals,
/// making runtime's continuity ownership explicit at the tick boundary.
pub fn check_alpaca_ws_continuity_from_opaque_cursor(
    cursor_json: Option<&str>,
) -> Option<WsLifecycleContinuity> {
    let json = cursor_json?;
    let cursor: AlpacaFetchCursor = serde_json::from_str(json).ok()?;
    Some(ws_continuity_from_cursor(&cursor))
}
// ---------------------------------------------------------------------------
// process_ws_inbound_batch — BRK-01R / BRK-02R authoritative ingest path
// ---------------------------------------------------------------------------
/// Process a raw Alpaca websocket frame through the authoritative inbound lane.
///
/// # Ordering contract (BRK-02R)
///
/// For each `TradeUpdate` message in the frame:
/// 1. Normalize via `build_inbound_batch_from_ws_update`.
/// 2. `inbox_insert_deduped_with_identity` for every event in the batch.
/// 3. **Only then** extract the cursor from the batch with `into_cursor_for_persist`.
///
/// After all messages are processed, `advance_broker_cursor` is called once
/// for the entire frame.
///
/// If `inbox_insert_deduped_with_identity` returns `Err` for any event, the
/// function returns that error immediately.  `advance_broker_cursor` is NOT
/// called.  On restart, the old cursor is used and inbox dedup prevents
/// double-apply.
///
/// # Normalization errors
///
/// If `build_inbound_batch_from_ws_update` returns `Err` (unknown event type),
/// the message is skipped with a diagnostic log.  The cursor does NOT advance
/// past a message that could not be normalized.
///
/// # Parameters
///
/// - `pool` - DB connection pool.
/// - `run_id` - The active run's UUID.  Events are scoped to this run.
/// - `adapter_id` - Opaque adapter identifier for `broker_event_cursor`.
/// - `raw_ws_bytes` - Raw bytes from the websocket frame.
/// - `prev_cursor` - The cursor at the start of this frame.
/// - `now` - Caller-supplied timestamp (no wall-clock reads in this function).
pub async fn process_ws_inbound_batch(
    pool: &PgPool,
    run_id: Uuid,
    adapter_id: &str,
    raw_ws_bytes: &[u8],
    prev_cursor: &AlpacaFetchCursor,
    now: DateTime<Utc>,
) -> anyhow::Result<WsIngestOutcome> {
    // Step 1: parse raw bytes into typed WS messages.
    let messages = parse_ws_message(raw_ws_bytes)
        .map_err(|e| anyhow::anyhow!("ws_inbound: frame parse failed: {e}"))?;
    let mut current_cursor = prev_cursor.clone();
    let mut total_processed: usize = 0;
    for msg in messages {
        // Only TradeUpdate messages carry lifecycle events.
        // Authorization, Listening, Error, Ping, and Unknown are protocol-level
        // messages that do not participate in inbox ingest or cursor advance.
        let update = match msg {
            AlpacaWsMessage::TradeUpdate(tu) => tu,
            _ => continue,
        };
        // Step 2: normalize the update and build an InboundBatch.
        // The batch's cursor is private until all events are ingested (BRK-02R).
        let batch = match build_inbound_batch_from_ws_update(&current_cursor, update) {
            Ok(b) => b,
            Err(e) => {
                // Normalization error: unknown or malformed event type.
                // Skip this message. The cursor does NOT advance past it.
                eprintln!("WARN ws_inbound: normalization failed, message skipped: {e}");
                continue;
            }
        };
        // Step 3: durably ingest every event in the batch BEFORE extracting
        // the cursor.  If any insert returns Err, the function returns
        // immediately and advance_broker_cursor is never reached (BRK-02R).
        for event in &batch.events {
            let msg_json =
                serde_json::to_value(event).context("ws_inbound: event serialization failed")?;
            let event_kind = broker_event_kind(event);
            mqk_db::inbox_insert_deduped_with_identity(
                pool,
                run_id,
                event.broker_message_id(),
                event.broker_fill_id(),
                event.internal_order_id(),
                event.broker_order_id().unwrap_or(event.internal_order_id()),
                event_kind,
                &msg_json,
                0,
                now,
            )
            .await
            .context("ws_inbound: inbox_insert_deduped_with_identity failed")?;
            // ^^^^ Return here on Err — cursor is NOT advanced (BRK-02R enforced).
            total_processed += 1;
        }
        // Step 4: all inserts for this message succeeded.
        // `into_cursor_for_persist` is the ONLY way to extract the cursor from
        // an InboundBatch, and it consumes the batch.  Calling it here — after
        // the inbox inserts — is the structural enforcement of BRK-02R.
        current_cursor = batch.into_cursor_for_persist();
    }
    if total_processed == 0 {
        return Ok(WsIngestOutcome::NoActionableEvents);
    }
    // Step 5: persist the final cursor AFTER all inbox inserts succeed.
    // This is the single call to advance_broker_cursor for the entire frame.
    let cursor_json = serde_json::to_string(&current_cursor)
        .context("ws_inbound: cursor serialization failed")?;
    mqk_db::advance_broker_cursor(pool, adapter_id, &cursor_json, now)
        .await
        .context("ws_inbound: advance_broker_cursor failed")?;
    Ok(WsIngestOutcome::EventsIngested {
        count: total_processed,
        new_cursor: current_cursor,
    })
}
// ---------------------------------------------------------------------------
// persist_ws_gap_cursor — BRK-07R fail-closed path
// ---------------------------------------------------------------------------
/// Persist a `GapDetected` cursor when WS continuity cannot be proven.
///
/// # When to call this (BRK-07R)
///
/// Call before resuming event processing after any of:
/// - WS disconnect followed by reconnect without replay of missed messages.
/// - Detection of a sequence gap in received message IDs.
/// - Any transport condition where the set of events since the last persisted
///   cursor position is uncertain or potentially incomplete.
///
/// After this call:
/// - `broker_event_cursor` in the DB carries `GapDetected` state.
/// - `BrokerAdapter::fetch_events` will return `InboundContinuityUnproven`.
/// - The orchestrator will persist the gap cursor and halt on the next tick
///   (via the `persist_cursor()` path in Phase 2).
///
/// Returns the `GapDetected` cursor that was persisted.
pub async fn persist_ws_gap_cursor(
    pool: &PgPool,
    adapter_id: &str,
    prev_cursor: &AlpacaFetchCursor,
    gap_detail: impl Into<String>,
    now: DateTime<Utc>,
) -> anyhow::Result<AlpacaFetchCursor> {
    let gap = mark_gap_detected(prev_cursor, gap_detail);
    let cursor_json =
        serde_json::to_string(&gap).context("persist_gap_cursor: cursor serialization failed")?;
    mqk_db::advance_broker_cursor(pool, adapter_id, &cursor_json, now)
        .await
        .context("persist_gap_cursor: advance_broker_cursor failed")?;
    Ok(gap)
}
// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------
/// Map a canonical `BrokerEvent` to its `event_kind` string for inbox storage.
///
/// Mirrors the same mapping used in `ExecutionOrchestrator::tick` Phase 2,
/// ensuring event_kind values are consistent between the REST and WS ingest paths.
fn broker_event_kind(event: &BrokerEvent) -> &'static str {
    match event {
        BrokerEvent::Ack { .. } => "ack",
        BrokerEvent::PartialFill { .. } => "partial_fill",
        BrokerEvent::Fill { .. } => "fill",
        BrokerEvent::CancelAck { .. } => "cancel_ack",
        BrokerEvent::CancelReject { .. } => "cancel_reject",
        BrokerEvent::ReplaceAck { .. } => "replace_ack",
        BrokerEvent::ReplaceReject { .. } => "replace_reject",
        BrokerEvent::Reject { .. } => "reject",
    }
}
