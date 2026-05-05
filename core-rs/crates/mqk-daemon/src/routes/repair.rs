//! OPS-REPAIR-01 — Audited ambiguous outbox repair route.
//!
//! POST /api/v1/ops/repair/outbox-ambiguous
//!
//! Releases an AMBIGUOUS outbox row back to PENDING only when the broker
//! snapshot confirms no live open order exists for the target idempotency key.
//! Every attempt (released or refused) is recorded in audit_events.
//!
//! ## Safety contract
//!
//! - Broker snapshot must be present — absent snapshot → refused.
//! - Broker snapshot must not contain a live order with matching client_order_id
//!   — detected live order → refused.
//! - Row must exist and must be AMBIGUOUS — any other status → refused.
//! - `outbox_reset_ambiguous_to_pending` is called only after all evidence
//!   checks pass.
//! - Every call writes a durable audit event (non-fatal: audit failure does
//!   not block the release or the refusal response).

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;

use crate::{
    api_types::{
        HaltedRunFillEntry, HaltedRunFillPlanResponse, OutboxRepairRequest, OutboxRepairResponse,
    },
    state::AppState,
};

pub(crate) async fn repair_outbox_ambiguous(
    State(st): State<Arc<AppState>>,
    Json(body): Json<OutboxRepairRequest>,
) -> Response {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(OutboxRepairResponse {
                accepted: false,
                decision: "refused".to_string(),
                idempotency_key: body.idempotency_key.clone(),
                evidence: "DB is not configured on this daemon".to_string(),
                gate: Some("repair.db_required".to_string()),
                audit_event_id: None,
            }),
        )
            .into_response();
    };

    let idempotency_key = body.idempotency_key.trim().to_string();
    if idempotency_key.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(OutboxRepairResponse {
                accepted: false,
                decision: "refused".to_string(),
                idempotency_key: idempotency_key.clone(),
                evidence: "idempotency_key must not be empty".to_string(),
                gate: Some("repair.invalid_request".to_string()),
                audit_event_id: None,
            }),
        )
            .into_response();
    }

    // Load the outbox row — must exist and must be AMBIGUOUS.
    let row = match mqk_db::outbox_fetch_by_idempotency_key(db, &idempotency_key).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(OutboxRepairResponse {
                    accepted: false,
                    decision: "refused".to_string(),
                    idempotency_key: idempotency_key.clone(),
                    evidence: "outbox row not found for this idempotency_key".to_string(),
                    gate: Some("repair.row_not_found".to_string()),
                    audit_event_id: None,
                }),
            )
                .into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(OutboxRepairResponse {
                    accepted: false,
                    decision: "refused".to_string(),
                    idempotency_key: idempotency_key.clone(),
                    evidence: format!("outbox query failed: {err}"),
                    gate: Some("repair.db_error".to_string()),
                    audit_event_id: None,
                }),
            )
                .into_response();
        }
    };

    if row.status != "AMBIGUOUS" {
        let evidence = format!(
            "outbox row status is '{}'; only AMBIGUOUS rows can be repaired",
            row.status
        );
        write_repair_audit(
            &st,
            db,
            row.run_id,
            &idempotency_key,
            "refused",
            "repair.row_not_ambiguous",
            &evidence,
        )
        .await;
        return (
            StatusCode::CONFLICT,
            Json(OutboxRepairResponse {
                accepted: false,
                decision: "refused".to_string(),
                idempotency_key: idempotency_key.clone(),
                evidence,
                gate: Some("repair.row_not_ambiguous".to_string()),
                audit_event_id: None,
            }),
        )
            .into_response();
    }

    // Broker snapshot must be present and must not contain a live open order
    // for this idempotency_key (== client_order_id in the broker snapshot).
    let broker_snap = st.current_broker_snapshot().await;
    let Some(snapshot) = broker_snap else {
        let evidence = "broker snapshot is absent; cannot confirm broker state — \
                        ensure a broker snapshot is loaded before repairing (OPS-REPAIR-01)"
            .to_string();
        write_repair_audit(
            &st,
            db,
            row.run_id,
            &idempotency_key,
            "refused",
            "repair.broker_snapshot_absent",
            &evidence,
        )
        .await;
        return (
            StatusCode::CONFLICT,
            Json(OutboxRepairResponse {
                accepted: false,
                decision: "refused".to_string(),
                idempotency_key: idempotency_key.clone(),
                evidence,
                gate: Some("repair.broker_snapshot_absent".to_string()),
                audit_event_id: None,
            }),
        )
            .into_response();
    };

    if let Some(live_order) = snapshot
        .orders
        .iter()
        .find(|o| o.client_order_id == idempotency_key)
    {
        let evidence = format!(
            "broker snapshot contains a live order for this idempotency_key \
             (broker_order_id={}, status='{}'); \
             cannot release — verify broker side before retrying (OPS-REPAIR-01)",
            live_order.broker_order_id, live_order.status
        );
        write_repair_audit(
            &st,
            db,
            row.run_id,
            &idempotency_key,
            "refused",
            "repair.live_broker_order_detected",
            &evidence,
        )
        .await;
        return (
            StatusCode::CONFLICT,
            Json(OutboxRepairResponse {
                accepted: false,
                decision: "refused".to_string(),
                idempotency_key: idempotency_key.clone(),
                evidence,
                gate: Some("repair.live_broker_order_detected".to_string()),
                audit_event_id: None,
            }),
        )
            .into_response();
    }

    // Evidence passed — release the row.
    match mqk_db::outbox_reset_ambiguous_to_pending(db, &idempotency_key).await {
        Ok(true) => {}
        Ok(false) => {
            // Row changed state between our fetch and the update (race or duplicate call).
            let evidence = "row was no longer AMBIGUOUS at release time (concurrent modification \
                 or already released)"
                .to_string();
            write_repair_audit(
                &st,
                db,
                row.run_id,
                &idempotency_key,
                "refused",
                "repair.row_no_longer_ambiguous",
                &evidence,
            )
            .await;
            return (
                StatusCode::CONFLICT,
                Json(OutboxRepairResponse {
                    accepted: false,
                    decision: "refused".to_string(),
                    idempotency_key: idempotency_key.clone(),
                    evidence,
                    gate: Some("repair.row_no_longer_ambiguous".to_string()),
                    audit_event_id: None,
                }),
            )
                .into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(OutboxRepairResponse {
                    accepted: false,
                    decision: "refused".to_string(),
                    idempotency_key: idempotency_key.clone(),
                    evidence: format!("outbox_reset_ambiguous_to_pending failed: {err}"),
                    gate: Some("repair.db_error".to_string()),
                    audit_event_id: None,
                }),
            )
                .into_response();
        }
    }

    let evidence = format!(
        "broker snapshot captured at {} confirmed no live open order for \
         idempotency_key='{}'; row released AMBIGUOUS→PENDING",
        snapshot.captured_at_utc.to_rfc3339(),
        idempotency_key
    );
    let audit_id = write_repair_audit(
        &st,
        db,
        row.run_id,
        &idempotency_key,
        "released",
        "repair.released",
        &evidence,
    )
    .await;

    (
        StatusCode::OK,
        Json(OutboxRepairResponse {
            accepted: true,
            decision: "released".to_string(),
            idempotency_key,
            evidence,
            gate: None,
            audit_event_id: audit_id.map(|id| id.to_string()),
        }),
    )
        .into_response()
}

/// Write a durable repair audit event.
///
/// Non-fatal: if the write fails the repair outcome is unaffected.
/// Returns the event UUID on success, `None` on failure.
async fn write_repair_audit(
    _st: &Arc<AppState>,
    db: &sqlx::PgPool,
    run_id: uuid::Uuid,
    idempotency_key: &str,
    decision: &str,
    gate: &str,
    evidence: &str,
) -> Option<uuid::Uuid> {
    let ts_utc = Utc::now();
    let event_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!(
            "mqk-daemon.repair.outbox-ambiguous.v1|{}|{}|{}|{}",
            run_id,
            idempotency_key,
            decision,
            ts_utc.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        )
        .as_bytes(),
    );
    let result = mqk_db::insert_audit_event(
        db,
        &mqk_db::NewAuditEvent {
            event_id,
            run_id,
            ts_utc,
            topic: "operator".to_string(),
            event_type: "ops.repair.outbox_ambiguous".to_string(),
            payload: serde_json::json!({
                "idempotency_key": idempotency_key,
                "decision": decision,
                "gate": gate,
                "evidence": evidence,
                "source": "mqk-daemon.routes.repair",
                "accepted": decision == "released",
            }),
            hash_prev: None,
            hash_self: None,
        },
    )
    .await;

    if let Err(err) = result {
        tracing::warn!("repair_outbox_ambiguous: audit write failed (non-fatal): {err}");
        return None;
    }

    Some(event_id)
}

// ---------------------------------------------------------------------------
// BROKER-FILL-REPLAY-REPAIR-01 — dry-run halted-run fill planner
// ---------------------------------------------------------------------------
//
// GET /api/v1/ops/repair/halted-run-fill-plan
//
// Read-only diagnostic: finds all broker_order_map entries belonging to HALTED
// runs and classifies whether a fill event was received but not applied to
// portfolio state.
//
// No state is mutated.  Mutation is deferred to BROKER-FILL-REPLAY-APPLY-01.
//
// Adapter ID used for broker cursor lookup: "alpaca" (canonical paper adapter).
const ALPACA_ADAPTER_ID: &str = "alpaca";

/// Classify one stale broker-order-map entry.
///
/// Classification rules:
/// 1. `unapplied_inbox_fill` — `oms_inbox` has at least one unapplied
///    (`applied_at_utc IS NULL`) fill or partial_fill row for this run.
///    The fill arrived in the inbox but Phase 3 never ran (run halted first).
/// 2. `cursor_only_fill_evidence` — no unapplied inbox fill row, but the
///    broker event cursor's `last_message_id` contains the `broker_order_id`,
///    proving the WS transport received the fill and advanced the cursor.
///    The inbox row was either applied+then-deleted or the run halted between
///    cursor advance and inbox insert (both rare; the latter is impossible
///    given the BRK-02R ordering invariant where inbox insert precedes cursor
///    advance).
/// 3. `no_fill_evidence` — no inbox fill row and the cursor does not mention
///    the broker_order_id.  Order may still be open or fill may not have
///    arrived yet.
/// 4. `ambiguous` — classification could not be determined; operator must
///    investigate directly.
fn classify_stale_entry(
    internal_order_id: &str,
    broker_order_id: &str,
    unapplied_rows: &[mqk_db::InboxRow],
    cursor_last_message_id: Option<&str>,
) -> (String, String, bool) {
    let fill_kinds = ["fill", "partial_fill"];

    // Check unapplied inbox for fill evidence.
    let has_unapplied_fill = unapplied_rows
        .iter()
        .any(|r| fill_kinds.contains(&r.event_kind.as_str()));

    if has_unapplied_fill {
        return (
            "unapplied_inbox_fill".to_string(),
            format!(
                "Unapplied fill row exists in oms_inbox for run owning order '{}'. \
                 Phase 3 did not apply this fill before the run halted. \
                 Operator action: run BROKER-FILL-REPLAY-APPLY-01 to apply the fill \
                 to portfolio state and mark the inbox row applied.",
                internal_order_id
            ),
            true,
        );
    }

    // Check broker cursor for fill evidence.
    let cursor_confirms = cursor_last_message_id
        .map(|mid| mid.contains(broker_order_id))
        .unwrap_or(false);

    if cursor_confirms {
        return (
            "cursor_only_fill_evidence".to_string(),
            format!(
                "Broker cursor confirms fill was received for broker_order_id='{}' \
                 (internal='{}'), but no unapplied inbox row exists. \
                 The inbox row was either applied before the halt (no action needed) \
                 or was deleted before being applied (portfolio may be inconsistent). \
                 Operator action: verify portfolio position against broker snapshot \
                 before running BROKER-FILL-REPLAY-APPLY-01.",
                broker_order_id, internal_order_id
            ),
            true,
        );
    }

    // No fill evidence found.
    (
        "no_fill_evidence".to_string(),
        format!(
            "No fill evidence found for order '{}' (broker='{}') in oms_inbox or broker cursor. \
             The order may not have been filled, or the fill predates the retained cursor window. \
             Operator action: verify order status directly against broker API before taking action.",
            internal_order_id, broker_order_id
        ),
        false,
    )
}

/// GET /api/v1/ops/repair/halted-run-fill-plan
///
/// Dry-run planner: identifies stale broker-order-map entries for HALTED runs
/// and classifies fill evidence.  No state is mutated.
pub(crate) async fn repair_halted_run_fill_plan(State(st): State<Arc<AppState>>) -> Response {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HaltedRunFillPlanResponse {
                truth_state: "no_db".to_string(),
                entries: vec![],
                summary: "DB is not configured on this daemon; plan cannot be computed."
                    .to_string(),
                repair_required: false,
                follow_up_patch: None,
            }),
        )
            .into_response();
    };

    // Load all stale broker_order_map entries for HALTED runs.
    let stale = match mqk_db::inbox_find_stale_broker_map_for_halted_runs(db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("repair_halted_run_fill_plan: DB query failed: {e}");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HaltedRunFillPlanResponse {
                    truth_state: "backend_unavailable".to_string(),
                    entries: vec![],
                    summary: format!("DB query failed: {e}"),
                    repair_required: false,
                    follow_up_patch: None,
                }),
            )
                .into_response();
        }
    };

    // Load broker cursor once for fill evidence checks.
    let cursor_last_message_id: Option<String> =
        match mqk_db::load_broker_cursor(db, ALPACA_ADAPTER_ID).await {
            Ok(Some(cursor_json)) => {
                // Parse as AlpacaFetchCursor to extract the WS last_message_id.
                serde_json::from_str::<serde_json::Value>(&cursor_json)
                    .ok()
                    .and_then(|v| {
                        v.get("trade_updates")
                            .and_then(|tu| tu.get("last_message_id"))
                            .and_then(|mid| mid.as_str())
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string())
                    })
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(
                    "repair_halted_run_fill_plan: load_broker_cursor failed (non-fatal): {e}"
                );
                None
            }
        };

    let mut entries: Vec<HaltedRunFillEntry> = Vec::with_capacity(stale.len());
    let mut any_repair_required = false;

    for entry in stale {
        // Load unapplied inbox rows for this run (read-only).
        let unapplied = match mqk_db::inbox_load_unapplied_for_run(db, entry.run_id).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(
                    run_id = %entry.run_id,
                    "repair_halted_run_fill_plan: inbox_load_unapplied_for_run failed: {e}"
                );
                vec![]
            }
        };

        let unapplied_event_kinds: Vec<String> =
            unapplied.iter().map(|r| r.event_kind.clone()).collect();
        let unapplied_count = unapplied.len();

        let (classification, prescribed_action, fill_repair_needed) = classify_stale_entry(
            &entry.internal_order_id,
            &entry.broker_order_id,
            &unapplied,
            cursor_last_message_id.as_deref(),
        );

        if fill_repair_needed {
            any_repair_required = true;
        }

        let broker_order_id = entry.broker_order_id;
        let cursor_fill_evidence = cursor_last_message_id
            .as_deref()
            .map(|mid| mid.contains(broker_order_id.as_str()))
            .unwrap_or(false);

        entries.push(HaltedRunFillEntry {
            internal_order_id: entry.internal_order_id,
            broker_order_id,
            run_id: entry.run_id.to_string(),
            outbox_status: entry.outbox_status,
            halted_at_utc: entry
                .halted_at_utc
                .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)),
            unapplied_inbox_count: unapplied_count,
            unapplied_inbox_event_kinds: unapplied_event_kinds,
            cursor_fill_evidence,
            cursor_last_message_id: cursor_last_message_id.clone(),
            classification,
            prescribed_action,
            mutation_safe: false,
        });
    }

    let n = entries.len();
    let repair_count = entries
        .iter()
        .filter(|e| e.classification != "no_fill_evidence")
        .count();
    let summary = if n == 0 {
        "No stale broker_order_map entries for HALTED runs. No repair required.".to_string()
    } else {
        format!(
            "{n} stale broker_order_map entry/entries for HALTED run(s) found; \
             {repair_count} with fill evidence requiring operator action."
        )
    };

    (
        StatusCode::OK,
        Json(HaltedRunFillPlanResponse {
            truth_state: "active".to_string(),
            entries,
            summary,
            repair_required: any_repair_required,
            follow_up_patch: if any_repair_required {
                Some("BROKER-FILL-REPLAY-APPLY-01".to_string())
            } else {
                None
            },
        }),
    )
        .into_response()
}
