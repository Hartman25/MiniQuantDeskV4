//! BRK-GAP-REST-AUTO-TRIGGER-01 — Shared WS gap fill recovery service.
//!
//! Provides the core recovery logic used by both:
//!   - `POST /api/v1/ops/repair/ws-gap-fill-recovery` (manual operator route)
//!   - `AppState::try_ws_gap_auto_recovery` (startup auto-trigger)
//!
//! ## Auto-trigger contract
//!
//! `try_ws_gap_auto_recovery` fires only when ALL of the following are true at
//! daemon startup (after `seed_ws_continuity_from_db`):
//!
//! 1. Broker kind is Alpaca.
//! 2. Persisted WS continuity state is `GapDetected` (proven gap condition).
//! 3. `ws_gap_fill_fetcher` is configured (production Alpaca credentials present).
//! 4. DB pool is available.
//! 5. A non-halted prior run exists for this engine + deployment mode.
//!
//! Recovery failure is warn-only.  The BRK-00R-04 gate already blocks
//! `start_execution_runtime` when continuity is `GapDetected`, so the
//! auto-trigger cannot weaken the fail-closed start gate.
//!
//! The WS continuity state is NOT cleared by auto-trigger.  The operator must
//! re-establish WS continuity explicitly before `start_execution_runtime` is
//! allowed.

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use super::types::AlpacaWsContinuityState;
use super::{AppState, BrokerKind, DAEMON_ENGINE_ID};
use crate::api_types::WsGapRecoveredFill;

/// Adapter ID for Alpaca broker — used in cursor and audit paths.
pub(crate) const ALPACA_ADAPTER_ID: &str = "alpaca";

/// Stable `broker_message_id` prefix for gap-recovery REST-sourced inbox rows.
pub(crate) const GAP_RECOVERY_MSG_PREFIX: &str = "alpaca-rest-gap";

/// Outcome of a WS gap fill recovery run.
///
/// Returned by `run_ws_gap_fill_recovery_core`.  Used by:
/// - The route handler to build an HTTP response.
/// - `try_ws_gap_auto_recovery` for structured log emission.
pub struct WsGapRecoveryOutcome {
    pub run_id: Uuid,
    /// Cursor lower-bound used for fetching (from persisted broker cursor).
    /// `None` when no prior cursor was available (all activities fetched).
    pub rest_activity_after: Option<String>,
    pub activities_fetched: usize,
    pub recovered_count: usize,
    pub already_present_count: usize,
    pub unknown_order_count: usize,
    pub malformed_count: usize,
    pub dry_run: bool,
    pub cursor_advanced: bool,
    pub new_rest_activity_after: Option<String>,
    pub recovered_fills: Vec<WsGapRecoveredFill>,
    /// Non-`None` when a gate refused the operation (DB or REST error).
    /// Value is the gate identifier string (e.g. `"repair.db_error"`).
    pub gate: Option<String>,
    pub evidence: String,
}

/// Core WS gap fill recovery logic, shared between the manual repair route and
/// the startup auto-trigger.
///
/// ## Caller responsibilities
///
/// The caller must verify DB availability and fetcher presence before calling.
/// Both are required; this function does not check them.
///
/// ## Safety contract
///
/// - No orders submitted. No OMS state mutated. No portfolio written.
/// - Inbox inserts are idempotent (`inbox_insert_deduped_with_identity`).
/// - Cursor advancement follows BRK-GAP-REST-CURSOR-ADVANCE-01 monotonic rules:
///   only advances when `dry_run=false`, no unsafe fills, and at least one
///   safe activity was confirmed.
pub(crate) async fn run_ws_gap_fill_recovery_core(
    db: &PgPool,
    fetcher: &dyn super::WsGapFillFetcher,
    run_id: Uuid,
    dry_run: bool,
) -> WsGapRecoveryOutcome {
    // Load broker cursor to get rest_activity_after anchor.
    let rest_activity_after: Option<String> =
        match mqk_db::load_broker_cursor(db, ALPACA_ADAPTER_ID).await {
            Ok(Some(cursor_json)) => serde_json::from_str::<serde_json::Value>(&cursor_json)
                .ok()
                .and_then(|v| {
                    v.get("rest_activity_after")
                        .and_then(|a| a.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                }),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(
                    "run_ws_gap_fill_recovery_core: load_broker_cursor failed (non-fatal): {e}"
                );
                None
            }
        };

    // Load broker_order_map entries for this run.
    let run_broker_map = match mqk_db::broker_map_load_for_run(db, run_id).await {
        Ok(entries) => entries,
        Err(e) => {
            return WsGapRecoveryOutcome {
                run_id,
                rest_activity_after,
                activities_fetched: 0,
                recovered_count: 0,
                already_present_count: 0,
                unknown_order_count: 0,
                malformed_count: 0,
                dry_run,
                cursor_advanced: false,
                new_rest_activity_after: None,
                recovered_fills: vec![],
                gate: Some("repair.db_error".to_string()),
                evidence: format!("broker_map_load_for_run failed: {e}"),
            };
        }
    };

    // Fetch activities since cursor — fail closed if REST unavailable.
    let activities = match fetcher.fetch_fills_since(rest_activity_after.as_deref()) {
        Ok(v) => v,
        Err(e) => {
            return WsGapRecoveryOutcome {
                run_id,
                rest_activity_after,
                activities_fetched: 0,
                recovered_count: 0,
                already_present_count: 0,
                unknown_order_count: 0,
                malformed_count: 0,
                dry_run,
                cursor_advanced: false,
                new_rest_activity_after: None,
                recovered_fills: vec![],
                gate: Some("repair.rest_unavailable".to_string()),
                evidence: format!(
                    "WsGapFillFetcher.fetch_fills_since returned error (fail closed): {e}"
                ),
            };
        }
    };

    let activities_fetched = activities.len();
    let mut recovered_count: usize = 0;
    let mut already_present_count: usize = 0;
    let mut unknown_order_count: usize = 0;
    let mut malformed_count: usize = 0;
    let mut recovered_fills: Vec<WsGapRecoveredFill> = Vec::new();
    // BRK-GAP-REST-CURSOR-ADVANCE-01: track safe cursor advancement eligibility.
    let mut last_safe_activity_id: Option<String> = None;
    let mut has_unsafe_fill = false;

    // Match each REST activity to a known broker order for this run.
    'activity: for activity in &activities {
        // Filter: only FILL or PARTIAL_FILL activity types.
        if activity.activity_type != "FILL" && activity.activity_type != "PARTIAL_FILL" {
            continue;
        }

        // Match order_id to broker_order_map for this run.
        let map_entry = run_broker_map
            .iter()
            .find(|e| e.broker_order_id == activity.order_id);
        let Some(entry) = map_entry else {
            unknown_order_count += 1;
            has_unsafe_fill = true;
            tracing::debug!(
                "run_ws_gap_fill_recovery_core: activity '{}' order_id='{}' not in \
                 broker_map for run '{}'; skipped",
                activity.id,
                activity.order_id,
                run_id,
            );
            continue 'activity;
        };

        // Validate required fields — fail closed per activity on malformed data.
        let qty_str = match activity.qty.as_deref().filter(|s| !s.is_empty()) {
            Some(s) => s.to_owned(),
            None => {
                malformed_count += 1;
                has_unsafe_fill = true;
                tracing::warn!(
                    "run_ws_gap_fill_recovery_core: activity '{}' has missing qty; skipped",
                    activity.id
                );
                continue 'activity;
            }
        };
        let price_str = match activity.price.as_deref().filter(|s| !s.is_empty()) {
            Some(s) => s.to_owned(),
            None => {
                malformed_count += 1;
                has_unsafe_fill = true;
                tracing::warn!(
                    "run_ws_gap_fill_recovery_core: activity '{}' has missing price; skipped",
                    activity.id
                );
                continue 'activity;
            }
        };
        let side = activity.side.as_str();
        if side != "buy" && side != "sell" {
            malformed_count += 1;
            has_unsafe_fill = true;
            tracing::warn!(
                "run_ws_gap_fill_recovery_core: activity '{}' has unrecognized side='{}'; skipped",
                activity.id,
                side
            );
            continue 'activity;
        }

        // Build stable broker_message_id — idempotency anchor for retry safety.
        let stable_msg_id = format!("{GAP_RECOVERY_MSG_PREFIX}:{}", activity.id);
        let event_kind = if activity.activity_type == "PARTIAL_FILL" {
            "partial_fill"
        } else {
            "fill"
        };

        // Parse qty / price — fail closed per activity.
        let qty_f64: f64 = match qty_str.parse::<f64>() {
            Ok(v) if v > 0.0 && v.is_finite() => v,
            _ => {
                malformed_count += 1;
                has_unsafe_fill = true;
                tracing::warn!(
                    "run_ws_gap_fill_recovery_core: activity '{}' qty='{}' not positive-finite; \
                     skipped",
                    activity.id,
                    qty_str
                );
                continue 'activity;
            }
        };
        let delta_qty = (qty_f64 * 1_000_000.0).round() as i64;

        let price_f64: f64 = match price_str.parse::<f64>() {
            Ok(v) if v >= 0.0 && v.is_finite() => v,
            _ => {
                malformed_count += 1;
                has_unsafe_fill = true;
                tracing::warn!(
                    "run_ws_gap_fill_recovery_core: activity '{}' price='{}' not valid; skipped",
                    activity.id,
                    price_str
                );
                continue 'activity;
            }
        };
        let price_micros = match mqk_execution::price_to_micros(price_f64) {
            Ok(m) => m,
            Err(_) => {
                malformed_count += 1;
                has_unsafe_fill = true;
                tracing::warn!(
                    "run_ws_gap_fill_recovery_core: activity '{}' price='{}' micros conversion \
                     failed; skipped",
                    activity.id,
                    price_str
                );
                continue 'activity;
            }
        };
        let exe_side = match side {
            "buy" => mqk_execution::Side::Buy,
            _ => mqk_execution::Side::Sell,
        };

        // Build canonical BrokerEvent payload.
        let broker_event = if event_kind == "partial_fill" {
            mqk_execution::BrokerEvent::PartialFill {
                broker_message_id: stable_msg_id.clone(),
                broker_fill_id: Some(activity.id.clone()),
                internal_order_id: entry.internal_order_id.clone(),
                broker_order_id: Some(activity.order_id.clone()),
                symbol: activity.symbol.clone(),
                side: exe_side,
                delta_qty,
                price_micros,
                fee_micros: 0,
                // Per this module's safety contract (see the doc comment on
                // `run_ws_gap_fill_recovery_core` above): this path only ever
                // inserts into oms_inbox — it never mutates OMS/portfolio
                // state directly, and correctness rests on the durable
                // idempotent insert plus the fact this only fires when WS
                // continuity was PROVEN broken (GapDetected) or on explicit
                // operator request. It does not fetch order.filled_qty, so
                // it has no cheap access to the broker cumulative watermark
                // PAPER-SOAK-PARTIAL-FILL-DEDUP-02 introduced; rows inserted
                // here fall back to the OMS apply layer's pre-existing
                // event-id dedup when later applied, unchanged from before.
                cum_qty_after: None,
            }
        } else {
            mqk_execution::BrokerEvent::Fill {
                broker_message_id: stable_msg_id.clone(),
                broker_fill_id: Some(activity.id.clone()),
                internal_order_id: entry.internal_order_id.clone(),
                broker_order_id: Some(activity.order_id.clone()),
                symbol: activity.symbol.clone(),
                side: exe_side,
                delta_qty,
                price_micros,
                fee_micros: 0,
            }
        };
        let event_json =
            serde_json::to_value(&broker_event).expect("BrokerEvent serializes to JSON infallibly");

        let received_at = chrono::DateTime::parse_from_rfc3339(&activity.transaction_time)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        // PAPER-SOAK-PARTIAL-FILL-DEDUP-01: `stable_msg_id` here is a synthetic
        // "{GAP_RECOVERY_MSG_PREFIX}:{activity.id}" identity, not Alpaca wire
        // format, so it carries no timestamp `alpaca_event_ts_ms_from_message_id`
        // could extract. inbox_insert_partial_fill_deduped's economic-match
        // window (mqk-db::inbox) needs a real event_ts_ms to discriminate
        // between distinct partial fills for the same order — this recovery
        // path exists specifically to backfill fills WS may have missed, so a
        // hardcoded 0 here would either falsely collapse two distinct
        // recovered fills or fail to match the WS row it is meant to
        // deduplicate against. Derive it from the activity's own transaction
        // timestamp instead; 0 only on genuine parse failure.
        let event_ts_ms = chrono::DateTime::parse_from_rfc3339(&activity.transaction_time)
            .map(|dt| dt.timestamp_millis()) // allow: broker-sourced timestamp, not wall-clock
            .unwrap_or(0);

        let fill = WsGapRecoveredFill {
            broker_activity_id: activity.id.clone(),
            broker_order_id: activity.order_id.clone(),
            internal_order_id: entry.internal_order_id.clone(),
            symbol: activity.symbol.clone(),
            side: side.to_owned(),
            qty_str: qty_str.clone(),
            price_str: price_str.clone(),
            event_kind: event_kind.to_owned(),
            inbox_broker_message_id: stable_msg_id.clone(),
            already_present: false,
        };

        if dry_run {
            recovered_count += 1;
            recovered_fills.push(fill);
            continue 'activity;
        }

        // dry_run=false: insert the canonical inbox row.
        match mqk_db::inbox_insert_deduped_with_identity(
            db,
            run_id,
            &stable_msg_id,
            Some(activity.id.as_str()),
            &entry.internal_order_id,
            &activity.order_id,
            event_kind,
            &event_json,
            event_ts_ms,
            received_at,
        )
        .await
        {
            Ok(true) => {
                recovered_count += 1;
                recovered_fills.push(fill);
                last_safe_activity_id = Some(activity.id.clone());
            }
            Ok(false) => {
                already_present_count += 1;
                let mut dup_fill = fill;
                dup_fill.already_present = true;
                recovered_fills.push(dup_fill);
                last_safe_activity_id = Some(activity.id.clone());
            }
            Err(e) => {
                tracing::error!(
                    "run_ws_gap_fill_recovery_core: inbox_insert_deduped_with_identity failed \
                     for activity='{}': {e}",
                    activity.id
                );
                malformed_count += 1;
                has_unsafe_fill = true;
            }
        }
    }

    // BRK-GAP-REST-CURSOR-ADVANCE-01: advance rest_activity_after when safe.
    // Conditions: dry_run=false, no unsafe fills, at least one confirmed fill.
    // Cursor advancement is monotonic: existing newer anchor is never overwritten.
    let (cursor_advanced, new_rest_activity_after) = if !dry_run && !has_unsafe_fill {
        if let Some(new_anchor) = last_safe_activity_id {
            match mqk_db::advance_broker_cursor_rest_anchor(
                db,
                ALPACA_ADAPTER_ID,
                &new_anchor,
                Utc::now(),
            )
            .await
            {
                Ok(true) => (true, Some(new_anchor)),
                Ok(false) => (false, None),
                Err(e) => {
                    tracing::warn!(
                        "run_ws_gap_fill_recovery_core: advance_broker_cursor_rest_anchor \
                         failed (non-fatal): {e}"
                    );
                    (false, None)
                }
            }
        } else {
            (false, None)
        }
    } else {
        (false, None)
    };

    let evidence = if dry_run {
        format!(
            "dry_run=true: fetched {activities_fetched} activities; \
             planned {recovered_count} recovery inserts; \
             {already_present_count} already present; \
             {unknown_order_count} unknown orders; \
             {malformed_count} malformed"
        )
    } else {
        format!(
            "dry_run=false: fetched {activities_fetched} activities; \
             inserted {recovered_count} new inbox rows; \
             {already_present_count} already present (idempotent); \
             {unknown_order_count} unknown orders skipped; \
             {malformed_count} malformed skipped"
        )
    };

    WsGapRecoveryOutcome {
        run_id,
        rest_activity_after,
        activities_fetched,
        recovered_count,
        already_present_count,
        unknown_order_count,
        malformed_count,
        dry_run,
        cursor_advanced,
        new_rest_activity_after,
        recovered_fills,
        gate: None,
        evidence,
    }
}

impl AppState {
    /// BRK-GAP-REST-AUTO-TRIGGER-01: Startup WS gap fill recovery.
    ///
    /// Called at daemon boot after `seed_ws_continuity_from_db()`.  Fires only
    /// when all five conditions are met (see module doc).  Returns the recovery
    /// outcome for log emission; `None` means the trigger did not fire.
    ///
    /// Recovery is best-effort and warn-only.  Success does NOT clear
    /// `GapDetected` continuity — the operator must re-establish WS continuity
    /// before `start_execution_runtime` is allowed.
    pub async fn try_ws_gap_auto_recovery(&self) -> Option<WsGapRecoveryOutcome> {
        // Condition 1: Alpaca broker only.
        if self.runtime_selection.broker_kind != Some(BrokerKind::Alpaca) {
            return None;
        }
        // Condition 2: Must be in GapDetected state (proven gap condition).
        if !matches!(
            self.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::GapDetected { .. }
        ) {
            return None;
        }
        // Condition 3: Fetcher must be configured.
        let fetcher = match self.ws_gap_fill_fetcher.as_deref() {
            Some(f) => f,
            None => {
                tracing::warn!(
                    "BRK-GAP-REST-AUTO-TRIGGER-01: GapDetected at startup but \
                     ws_gap_fill_fetcher is not configured; auto-recovery skipped"
                );
                return None;
            }
        };
        // Condition 4: DB must be available.
        let db = match self.db.as_ref() {
            Some(d) => d,
            None => {
                tracing::warn!(
                    "BRK-GAP-REST-AUTO-TRIGGER-01: GapDetected at startup but \
                     DB is not configured; auto-recovery skipped"
                );
                return None;
            }
        };
        // Condition 5: Find the most recent non-halted run for this engine.
        let run = match mqk_db::fetch_latest_run_for_engine(
            db,
            DAEMON_ENGINE_ID,
            self.deployment_mode().as_db_mode(),
        )
        .await
        {
            Ok(Some(run)) => run,
            Ok(None) => {
                tracing::debug!(
                    "BRK-GAP-REST-AUTO-TRIGGER-01: GapDetected but no prior run found; \
                     auto-recovery skipped"
                );
                return None;
            }
            Err(e) => {
                tracing::warn!(
                    "BRK-GAP-REST-AUTO-TRIGGER-01: fetch_latest_run_for_engine failed: {e}; \
                     auto-recovery skipped"
                );
                return None;
            }
        };
        // Skip halted runs — those require manual repair via the OPS-REPAIR-01 path.
        if matches!(run.status, mqk_db::RunStatus::Halted) {
            tracing::info!(
                run_id = %run.run_id,
                "BRK-GAP-REST-AUTO-TRIGGER-01: latest run is HALTED; \
                 manual repair required; auto-recovery skipped"
            );
            return None;
        }
        tracing::info!(
            run_id = %run.run_id,
            run_status = ?run.status,
            "BRK-GAP-REST-AUTO-TRIGGER-01: GapDetected — invoking startup REST gap fill recovery"
        );
        let outcome = run_ws_gap_fill_recovery_core(db, fetcher, run.run_id, false).await;
        Some(outcome)
    }
}
