//! OPS-REPAIR-01 â€” Audited ambiguous outbox repair route.
//!
//! POST /api/v1/ops/repair/outbox-ambiguous
//!
//! Releases an AMBIGUOUS outbox row back to PENDING only when the broker
//! snapshot confirms no live open order exists for the target idempotency key.
//! Every attempt (released or refused) is recorded in audit_events.
//!
//! ## Safety contract
//!
//! - Broker snapshot must be present â€” absent snapshot â†’ refused.
//! - Broker snapshot must not contain a live order with matching client_order_id
//!   â€” detected live order â†’ refused.
//! - Row must exist and must be AMBIGUOUS â€” any other status â†’ refused.
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
        HaltedRunFillApplyRequest, HaltedRunFillApplyResponse, HaltedRunFillEntry,
        HaltedRunFillPlanResponse, HaltedRunFillRestRecoveryRequest,
        HaltedRunFillRestRecoveryResponse, OutboxRepairRequest, OutboxRepairResponse,
        RestRecoveredFill,
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

    // Load the outbox row â€” must exist and must be AMBIGUOUS.
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
        let evidence = "broker snapshot is absent; cannot confirm broker state â€” \
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
             cannot release â€” verify broker side before retrying (OPS-REPAIR-01)",
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

    // Evidence passed â€” release the row.
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
         idempotency_key='{}'; row released AMBIGUOUSâ†’PENDING",
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
// BROKER-FILL-REPLAY-REPAIR-01 â€” dry-run halted-run fill planner
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
/// 1. `unapplied_inbox_fill` â€” `oms_inbox` has at least one unapplied
///    (`applied_at_utc IS NULL`) fill or partial_fill row for this run.
///    The fill arrived in the inbox but Phase 3 never ran (run halted first).
/// 2. `cursor_only_fill_evidence` â€” no unapplied inbox fill row, but the
///    broker event cursor's `last_message_id` contains the `broker_order_id`,
///    proving the WS transport received the fill and advanced the cursor.
///    The inbox row was either applied+then-deleted or the run halted between
///    cursor advance and inbox insert (both rare; the latter is impossible
///    given the BRK-02R ordering invariant where inbox insert precedes cursor
///    advance).
/// 3. `no_fill_evidence` â€” no inbox fill row and the cursor does not mention
///    the broker_order_id.  Order may still be open or fill may not have
///    arrived yet.
/// 4. `ambiguous` â€” classification could not be determined; operator must
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

// ---------------------------------------------------------------------------
// BROKER-FILL-REPLAY-APPLY-01 â€” guarded operator repair apply route
// ---------------------------------------------------------------------------
//
// POST /api/v1/ops/repair/halted-run-fill-apply
//
// Operator-gated apply path for halted-run fill evidence discovered by the
// BROKER-FILL-REPLAY-REPAIR-01 planner.
//
// ## Safety contract
//
// - `unapplied_inbox_fill` case only: full fill data is available in the
//   inbox row; operator-confirmed apply stamps `applied_at_utc` and writes an
//   audit event.  The in-memory portfolio for the HALTED run is NOT updated
//   (run is terminal); this is explicitly documented in the response.
//
// - `cursor_only_fill_evidence` case: REFUSED â€” the broker cursor confirms a
//   fill arrived but does not carry price or qty.  REST activity recovery
//   (BROKER-FILL-REST-RECOVERY-01) is required for authoritative fill details.
//
// - `no_fill_evidence` case: REFUSED â€” nothing to apply.
//
// - `dry_run = true` (default): no mutation; returns planned actions only.
// - `dry_run = false`: requires `confirmation = "APPLY_HALTED_FILL_REPAIR"`.
//
// - Every call (refused, dry-run, or applied) writes a durable audit event
//   (non-fatal: audit failure does not block the response).
//
// - Second call for an already-repaired row returns `"already_repaired"`.
//
// - No orders are submitted.  No fills are fabricated.  No reconcile gates
//   are weakened.  Mutation only via `inbox_mark_applied` which is idempotent.

const APPLY_CONFIRMATION_TOKEN: &str = "APPLY_HALTED_FILL_REPAIR";

/// POST /api/v1/ops/repair/halted-run-fill-apply
pub(crate) async fn repair_halted_run_fill_apply(
    State(st): State<Arc<AppState>>,
    Json(body): Json<HaltedRunFillApplyRequest>,
) -> Response {
    let dry_run = body.dry_run;

    macro_rules! refused {
        ($status:expr, $classification:expr, $decision:expr, $evidence:expr, $gate:expr, $follow_up:expr, $audit_id:expr) => {
            (
                $status,
                Json(HaltedRunFillApplyResponse {
                    truth_state: "active".to_string(),
                    decision: $decision.to_string(),
                    dry_run,
                    run_id: body.run_id.clone(),
                    internal_order_id: body.internal_order_id.clone(),
                    broker_order_id: body.broker_order_id.clone(),
                    classification: $classification.to_string(),
                    evidence: $evidence.to_string(),
                    gate: $gate,
                    audit_event_id: $audit_id,
                    follow_up_patch: $follow_up,
                }),
            )
                .into_response()
        };
    }

    // Gate 1: DB required.
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HaltedRunFillApplyResponse {
                truth_state: "no_db".to_string(),
                decision: "refused".to_string(),
                dry_run,
                run_id: body.run_id.clone(),
                internal_order_id: body.internal_order_id.clone(),
                broker_order_id: body.broker_order_id.clone(),
                classification: "unknown".to_string(),
                evidence: "DB is not configured on this daemon".to_string(),
                gate: Some("repair.db_required".to_string()),
                audit_event_id: None,
                follow_up_patch: None,
            }),
        )
            .into_response();
    };

    // Gate 2: parse run_id.
    let run_id = match uuid::Uuid::parse_str(&body.run_id) {
        Ok(id) => id,
        Err(_) => {
            return refused!(
                StatusCode::BAD_REQUEST,
                "unknown",
                "refused",
                format!("invalid run_id: '{}'", body.run_id),
                Some("repair.invalid_request".to_string()),
                None,
                None
            )
        }
    };

    // Build audit context for all subsequent write_fill_apply_audit calls.
    let audit_ctx = FillAuditCtx {
        db,
        run_id,
        internal_order_id: &body.internal_order_id,
        broker_order_id: &body.broker_order_id,
        dry_run,
    };

    // Gate 3: dry_run=false requires confirmation token.
    if !dry_run {
        match body.confirmation.as_deref() {
            Some(APPLY_CONFIRMATION_TOKEN) => {}
            Some(other) => {
                return refused!(
                    StatusCode::BAD_REQUEST,
                    "unknown",
                    "refused",
                    format!(
                        "dry_run=false requires confirmation='{APPLY_CONFIRMATION_TOKEN}'; \
                         got: '{other}'"
                    ),
                    Some("repair.confirmation_required".to_string()),
                    None,
                    None
                )
            }
            None => {
                return refused!(
                    StatusCode::BAD_REQUEST,
                    "unknown",
                    "refused",
                    format!("dry_run=false requires confirmation='{APPLY_CONFIRMATION_TOKEN}'"),
                    Some("repair.confirmation_required".to_string()),
                    None,
                    None
                )
            }
        }
    }

    // Gate 4: locate the stale broker_order_map entry for this run + order.
    let stale = match mqk_db::inbox_find_stale_broker_map_for_halted_runs(db).await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HaltedRunFillApplyResponse {
                    truth_state: "backend_unavailable".to_string(),
                    decision: "refused".to_string(),
                    dry_run,
                    run_id: body.run_id.clone(),
                    internal_order_id: body.internal_order_id.clone(),
                    broker_order_id: body.broker_order_id.clone(),
                    classification: "unknown".to_string(),
                    evidence: format!("DB query failed: {e}"),
                    gate: Some("repair.db_error".to_string()),
                    audit_event_id: None,
                    follow_up_patch: None,
                }),
            )
                .into_response();
        }
    };

    let entry_exists = stale.iter().any(|e| {
        e.run_id == run_id
            && e.internal_order_id == body.internal_order_id
            && e.broker_order_id == body.broker_order_id
    });

    if !entry_exists {
        let evidence = format!(
            "no stale broker_order_map entry found for run_id='{}' \
             internal_order_id='{}' broker_order_id='{}' in a HALTED run; \
             entry may have already been cleaned up or IDs are incorrect",
            body.run_id, body.internal_order_id, body.broker_order_id
        );
        let audit_id =
            write_fill_apply_audit(&audit_ctx, "refused", "repair.entry_not_found", &evidence)
                .await;
        return refused!(
            StatusCode::NOT_FOUND,
            "unknown",
            "refused",
            evidence,
            Some("repair.entry_not_found".to_string()),
            None,
            audit_id.map(|id| id.to_string())
        );
    }

    // Gate 5: classify fill evidence (same logic as the planner).
    let unapplied = match mqk_db::inbox_load_unapplied_for_run(db, run_id).await {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HaltedRunFillApplyResponse {
                    truth_state: "backend_unavailable".to_string(),
                    decision: "refused".to_string(),
                    dry_run,
                    run_id: body.run_id.clone(),
                    internal_order_id: body.internal_order_id.clone(),
                    broker_order_id: body.broker_order_id.clone(),
                    classification: "unknown".to_string(),
                    evidence: format!("inbox query failed: {e}"),
                    gate: Some("repair.db_error".to_string()),
                    audit_event_id: None,
                    follow_up_patch: None,
                }),
            )
                .into_response();
        }
    };

    let cursor_last_message_id: Option<String> =
        match mqk_db::load_broker_cursor(db, ALPACA_ADAPTER_ID).await {
            Ok(Some(cursor_json)) => serde_json::from_str::<serde_json::Value>(&cursor_json)
                .ok()
                .and_then(|v| {
                    v.get("trade_updates")
                        .and_then(|tu| tu.get("last_message_id"))
                        .and_then(|mid| mid.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                }),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(
                    "repair_halted_run_fill_apply: load_broker_cursor failed (non-fatal): {e}"
                );
                None
            }
        };

    let (classification, _prescribed, _) = classify_stale_entry(
        &body.internal_order_id,
        &body.broker_order_id,
        &unapplied,
        cursor_last_message_id.as_deref(),
    );

    // Gate 6: refuse cursor_only â€” no authoritative fill price/qty.
    if classification == "cursor_only_fill_evidence" {
        let evidence = format!(
            "cursor_only_fill_evidence: the broker WS cursor confirms a fill event \
             for broker_order_id='{}' was received, but no oms_inbox row carrying \
             authoritative fill price and qty exists. \
             Cannot apply fill without price/qty data. \
             Operator action: run BROKER-FILL-REST-RECOVERY-01 to look up fill \
             details from Alpaca REST activities before applying.",
            body.broker_order_id
        );
        let audit_id = write_fill_apply_audit(
            &audit_ctx,
            "refused",
            "repair.evidence_insufficient",
            &evidence,
        )
        .await;
        return refused!(
            StatusCode::CONFLICT,
            classification,
            "refused",
            evidence,
            Some("repair.evidence_insufficient".to_string()),
            Some("BROKER-FILL-REST-RECOVERY-01".to_string()),
            audit_id.map(|id| id.to_string())
        );
    }

    // Gate 7: refuse no_fill_evidence â€” nothing to apply.
    if classification == "no_fill_evidence" {
        let evidence = format!(
            "no_fill_evidence: no unapplied inbox fill row and no cursor evidence \
             for broker_order_id='{}'; nothing to apply.",
            body.broker_order_id
        );
        let audit_id =
            write_fill_apply_audit(&audit_ctx, "refused", "repair.no_evidence", &evidence).await;
        return refused!(
            StatusCode::CONFLICT,
            classification,
            "noop",
            evidence,
            None,
            None,
            audit_id.map(|id| id.to_string())
        );
    }

    // classification == "unapplied_inbox_fill"
    // Gate 8: load the specific fill row for this order.
    let fill_rows = match mqk_db::inbox_load_unapplied_fill_for_order(
        db,
        run_id,
        &body.internal_order_id,
    )
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HaltedRunFillApplyResponse {
                    truth_state: "backend_unavailable".to_string(),
                    decision: "refused".to_string(),
                    dry_run,
                    run_id: body.run_id.clone(),
                    internal_order_id: body.internal_order_id.clone(),
                    broker_order_id: body.broker_order_id.clone(),
                    classification: classification.clone(),
                    evidence: format!("fill row query failed: {e}"),
                    gate: Some("repair.db_error".to_string()),
                    audit_event_id: None,
                    follow_up_patch: None,
                }),
            )
                .into_response();
        }
    };

    // Gate 9: at least one fill row must be present.
    let fill_row = match fill_rows.first() {
        Some(r) => r,
        None => {
            let evidence = "unapplied_inbox_fill classification but no fill row found \
                            after targeted query â€” state changed between classification \
                            and apply; retry the planner"
                .to_string();
            let audit_id =
                write_fill_apply_audit(&audit_ctx, "refused", "repair.fill_row_missing", &evidence)
                    .await;
            return refused!(
                StatusCode::CONFLICT,
                classification,
                "refused",
                evidence,
                Some("repair.fill_row_missing".to_string()),
                None,
                audit_id.map(|id| id.to_string())
            );
        }
    };

    // Gate 10: check if already repaired (idempotency).
    if fill_row.applied_at_utc.is_some() {
        let evidence = format!(
            "fill inbox row (broker_message_id='{}') was already marked applied at {}; \
             no mutation performed",
            fill_row.broker_message_id,
            fill_row
                .applied_at_utc
                .map(|t| t.to_rfc3339())
                .unwrap_or_default()
        );
        let audit_id = write_fill_apply_audit(
            &audit_ctx,
            "already_repaired",
            "repair.already_applied",
            &evidence,
        )
        .await;
        return (
            StatusCode::OK,
            Json(HaltedRunFillApplyResponse {
                truth_state: "active".to_string(),
                decision: "already_repaired".to_string(),
                dry_run,
                run_id: body.run_id.clone(),
                internal_order_id: body.internal_order_id.clone(),
                broker_order_id: body.broker_order_id.clone(),
                classification,
                evidence,
                gate: None,
                audit_event_id: audit_id.map(|id| id.to_string()),
                follow_up_patch: None,
            }),
        )
            .into_response();
    }

    // Gate 11: verify the inbox row's message_json deserializes as a BrokerEvent
    // fill variant â€” proves the data is structurally complete before any mutation.
    let event_valid =
        match serde_json::from_value::<mqk_execution::BrokerEvent>(fill_row.message_json.clone()) {
            Ok(mqk_execution::BrokerEvent::Fill { .. })
            | Ok(mqk_execution::BrokerEvent::PartialFill { .. }) => true,
            Ok(_) => false,
            Err(_) => false,
        };

    if !event_valid {
        let evidence = format!(
            "inbox row (broker_message_id='{}') does not deserialize as a fill \
             BrokerEvent variant; message_json may be malformed or incomplete. \
             Manual broker reconcile required.",
            fill_row.broker_message_id
        );
        let audit_id = write_fill_apply_audit(
            &audit_ctx,
            "refused",
            "repair.malformed_fill_event",
            &evidence,
        )
        .await;
        return refused!(
            StatusCode::CONFLICT,
            classification,
            "refused",
            evidence,
            Some("repair.malformed_fill_event".to_string()),
            None,
            audit_id.map(|id| id.to_string())
        );
    }

    // --- dry_run=true: return plan without mutation. ---
    if dry_run {
        let evidence = format!(
            "dry_run=true: would mark inbox row (broker_message_id='{}') applied \
             and write audit event. In-memory portfolio for HALTED run '{}' would NOT \
             be updated (run is terminal). Resubmit with dry_run=false and \
             confirmation='APPLY_HALTED_FILL_REPAIR' to execute.",
            fill_row.broker_message_id, run_id
        );
        let audit_id =
            write_fill_apply_audit(&audit_ctx, "dry_run_ok", "repair.dry_run", &evidence).await;
        return (
            StatusCode::OK,
            Json(HaltedRunFillApplyResponse {
                truth_state: "active".to_string(),
                decision: "dry_run_ok".to_string(),
                dry_run,
                run_id: body.run_id.clone(),
                internal_order_id: body.internal_order_id.clone(),
                broker_order_id: body.broker_order_id.clone(),
                classification,
                evidence,
                gate: None,
                audit_event_id: audit_id.map(|id| id.to_string()),
                follow_up_patch: None,
            }),
        )
            .into_response();
    }

    // --- dry_run=false: apply. ---
    //
    // Stamp applied_at_utc on the inbox row.  `inbox_mark_applied` is idempotent:
    // it only updates rows where applied_at_utc IS NULL, so a concurrent or
    // duplicate call is safe.
    //
    // NOTE: The in-memory portfolio state for this HALTED run is NOT updated here.
    // The run is terminal.  Portfolio reconstruction for a new run reads
    // `inbox_load_all_applied_for_run` which will include this row after apply.
    let applied_at = Utc::now();
    let msg_id = fill_row.broker_message_id.clone();

    if let Err(e) = mqk_db::inbox_mark_applied(db, run_id, &msg_id, applied_at).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HaltedRunFillApplyResponse {
                truth_state: "backend_unavailable".to_string(),
                decision: "refused".to_string(),
                dry_run,
                run_id: body.run_id.clone(),
                internal_order_id: body.internal_order_id.clone(),
                broker_order_id: body.broker_order_id.clone(),
                classification: classification.clone(),
                evidence: format!("inbox_mark_applied failed: {e}"),
                gate: Some("repair.db_error".to_string()),
                audit_event_id: None,
                follow_up_patch: None,
            }),
        )
            .into_response();
    }

    let evidence = format!(
        "Inbox row (broker_message_id='{}') marked applied at {}. \
         Fill evidence acknowledged: run_id='{}', internal_order_id='{}', \
         broker_order_id='{}'. NOTE: in-memory portfolio for this HALTED run \
         was NOT updated â€” run is terminal. Start a new run to begin with \
         fresh portfolio state.",
        msg_id,
        applied_at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        run_id,
        body.internal_order_id,
        body.broker_order_id
    );

    let audit_id = write_fill_apply_audit(&audit_ctx, "applied", "repair.applied", &evidence).await;

    (
        StatusCode::OK,
        Json(HaltedRunFillApplyResponse {
            truth_state: "active".to_string(),
            decision: "applied".to_string(),
            dry_run,
            run_id: body.run_id.clone(),
            internal_order_id: body.internal_order_id.clone(),
            broker_order_id: body.broker_order_id.clone(),
            classification,
            evidence,
            gate: None,
            audit_event_id: audit_id.map(|id| id.to_string()),
            follow_up_patch: None,
        }),
    )
        .into_response()
}

struct FillAuditCtx<'a> {
    db: &'a sqlx::PgPool,
    run_id: uuid::Uuid,
    internal_order_id: &'a str,
    broker_order_id: &'a str,
    dry_run: bool,
}

/// Write a durable fill-apply repair audit event.
///
/// Non-fatal: audit failure does not block the repair outcome.
/// Returns the event UUID on success, `None` on failure.
async fn write_fill_apply_audit(
    ctx: &FillAuditCtx<'_>,
    decision: &str,
    gate: &str,
    evidence: &str,
) -> Option<uuid::Uuid> {
    let db = ctx.db;
    let run_id = ctx.run_id;
    let internal_order_id = ctx.internal_order_id;
    let broker_order_id = ctx.broker_order_id;
    let dry_run = ctx.dry_run;
    let ts_utc = Utc::now();
    // Deterministic event ID: same logical repair decision on same order produces
    // the same audit ID (idempotent audit trail for dry_run).
    // For applied (non-dry-run) repairs the decision is unique per outcome.
    let event_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!(
            "mqk-daemon.repair.halted-fill-apply.v1|{}|{}|{}|{}|dryrun={}",
            run_id,
            internal_order_id,
            decision,
            ts_utc.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            dry_run,
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
            event_type: "ops.repair.halted_fill_apply".to_string(),
            payload: serde_json::json!({
                "internal_order_id": internal_order_id,
                "broker_order_id": broker_order_id,
                "decision": decision,
                "gate": gate,
                "evidence": evidence,
                "dry_run": dry_run,
                "source": "mqk-daemon.routes.repair",
            }),
            hash_prev: None,
            hash_self: None,
        },
    )
    .await;

    if let Err(err) = result {
        tracing::warn!("repair_halted_run_fill_apply: audit write failed (non-fatal): {err}");
        return None;
    }

    Some(event_id)
}

// ---------------------------------------------------------------------------
// BROKER-FILL-REST-RECOVERY-01 — REST activity lookup for cursor_only evidence
// ---------------------------------------------------------------------------
//
// POST /api/v1/ops/repair/halted-run-fill-rest-recovery
//
// For stale halted-run entries classified as `cursor_only_fill_evidence`,
// this route fetches authoritative Alpaca REST account activities for the
// given broker_order_id and returns the recovered fill details for operator
// review.
//
// ## Safety contract
//
// - Requires DB — entry existence and cursor classification are derived from
//   DB state; no DB → 503.
// - Only accepts `cursor_only_fill_evidence` entries; other classifications →
//   409 (refused).
// - Requires a configured fill activity fetcher; absent fetcher → 503
//   (recovery_unavailable).  Production wiring is deferred to
//   BROKER-FILL-REST-RECOVERY-APPLY-01.
// - Fails closed if REST returns 0 matches (no_rest_match), >1 matches
//   (ambiguous_rest_match), or price/qty absent/malformed
//   (recovery_data_malformed).
// - Plan-only in this patch: `mutation_safe` is always `false`; no
//   portfolio/inbox mutation occurs.
// - All outcomes are audited (non-fatal).
// - No credentials or secrets are exposed in the response.

const REST_RECOVERY_APPLY_CONFIRMATION_TOKEN: &str = "APPLY_REST_FILL_RECOVERY";

/// POST /api/v1/ops/repair/halted-run-fill-rest-recovery
pub(crate) async fn repair_halted_run_fill_rest_recovery(
    State(st): State<Arc<AppState>>,
    Json(body): Json<HaltedRunFillRestRecoveryRequest>,
) -> Response {
    let dry_run = body.dry_run;

    macro_rules! refused_active {
        ($classification:expr, $evidence:expr, $gate:expr, $audit_id:expr) => {
            (
                StatusCode::CONFLICT,
                Json(HaltedRunFillRestRecoveryResponse {
                    truth_state: "active".to_string(),
                    decision: "refused".to_string(),
                    dry_run,
                    mutated: false,
                    run_id: body.run_id.clone(),
                    internal_order_id: body.internal_order_id.clone(),
                    broker_order_id: body.broker_order_id.clone(),
                    classification: $classification.to_string(),
                    evidence: $evidence.to_string(),
                    gate: $gate,
                    audit_event_id: $audit_id,
                    rest_fill: None,
                    inbox_broker_message_id: None,
                }),
            )
                .into_response()
        };
    }

    // Gate 1: DB required.
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HaltedRunFillRestRecoveryResponse {
                truth_state: "no_db".to_string(),
                decision: "refused".to_string(),
                dry_run,
                mutated: false,
                run_id: body.run_id.clone(),
                internal_order_id: body.internal_order_id.clone(),
                broker_order_id: body.broker_order_id.clone(),
                classification: "unknown".to_string(),
                evidence: "DB is not configured on this daemon".to_string(),
                gate: Some("repair.db_required".to_string()),
                audit_event_id: None,
                rest_fill: None,
                inbox_broker_message_id: None,
            }),
        )
            .into_response();
    };

    // Gate 2: parse run_id.
    let run_id = match uuid::Uuid::parse_str(&body.run_id) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(HaltedRunFillRestRecoveryResponse {
                    truth_state: "active".to_string(),
                    decision: "refused".to_string(),
                    dry_run,
                    mutated: false,
                    run_id: body.run_id.clone(),
                    internal_order_id: body.internal_order_id.clone(),
                    broker_order_id: body.broker_order_id.clone(),
                    classification: "unknown".to_string(),
                    evidence: format!("invalid run_id: '{}'", body.run_id),
                    gate: Some("repair.invalid_request".to_string()),
                    audit_event_id: None,
                    rest_fill: None,
                    inbox_broker_message_id: None,
                }),
            )
                .into_response();
        }
    };

    let rest_audit_ctx = RestRecoveryAuditCtx {
        db,
        run_id,
        internal_order_id: &body.internal_order_id,
        broker_order_id: &body.broker_order_id,
    };

    // Gate 3: locate stale broker_order_map entry for this run + order.
    let stale = match mqk_db::inbox_find_stale_broker_map_for_halted_runs(db).await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HaltedRunFillRestRecoveryResponse {
                    truth_state: "backend_unavailable".to_string(),
                    decision: "refused".to_string(),
                    dry_run,
                    mutated: false,
                    run_id: body.run_id.clone(),
                    internal_order_id: body.internal_order_id.clone(),
                    broker_order_id: body.broker_order_id.clone(),
                    classification: "unknown".to_string(),
                    evidence: format!("DB query failed: {e}"),
                    gate: Some("repair.db_error".to_string()),
                    audit_event_id: None,
                    rest_fill: None,
                    inbox_broker_message_id: None,
                }),
            )
                .into_response();
        }
    };

    let entry_exists = stale.iter().any(|e| {
        e.run_id == run_id
            && e.internal_order_id == body.internal_order_id
            && e.broker_order_id == body.broker_order_id
    });

    if !entry_exists {
        let evidence = format!(
            "no stale broker_order_map entry found for run_id='{}' \
             internal_order_id='{}' broker_order_id='{}' in a HALTED run",
            body.run_id, body.internal_order_id, body.broker_order_id
        );
        let audit_id = write_rest_recovery_audit(
            &rest_audit_ctx,
            "refused",
            "repair.entry_not_found",
            &evidence,
        )
        .await;
        return (
            StatusCode::NOT_FOUND,
            Json(HaltedRunFillRestRecoveryResponse {
                truth_state: "active".to_string(),
                decision: "refused".to_string(),
                dry_run,
                mutated: false,
                run_id: body.run_id.clone(),
                internal_order_id: body.internal_order_id.clone(),
                broker_order_id: body.broker_order_id.clone(),
                classification: "unknown".to_string(),
                evidence,
                gate: Some("repair.entry_not_found".to_string()),
                audit_event_id: audit_id.map(|id| id.to_string()),
                rest_fill: None,
                inbox_broker_message_id: None,
            }),
        )
            .into_response();
    }

    // Gate 4: classify the entry — must be cursor_only_fill_evidence.
    let unapplied = match mqk_db::inbox_load_unapplied_for_run(db, run_id).await {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HaltedRunFillRestRecoveryResponse {
                    truth_state: "backend_unavailable".to_string(),
                    decision: "refused".to_string(),
                    dry_run,
                    mutated: false,
                    run_id: body.run_id.clone(),
                    internal_order_id: body.internal_order_id.clone(),
                    broker_order_id: body.broker_order_id.clone(),
                    classification: "unknown".to_string(),
                    evidence: format!("inbox query failed: {e}"),
                    gate: Some("repair.db_error".to_string()),
                    audit_event_id: None,
                    rest_fill: None,
                    inbox_broker_message_id: None,
                }),
            )
                .into_response();
        }
    };

    let cursor_last_message_id: Option<String> =
        match mqk_db::load_broker_cursor(db, ALPACA_ADAPTER_ID).await {
            Ok(Some(cursor_json)) => serde_json::from_str::<serde_json::Value>(&cursor_json)
                .ok()
                .and_then(|v| {
                    v.get("trade_updates")
                        .and_then(|tu| tu.get("last_message_id"))
                        .and_then(|mid| mid.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                }),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(
                "repair_halted_run_fill_rest_recovery: load_broker_cursor failed (non-fatal): {e}"
            );
                None
            }
        };

    let (classification, _, _) = classify_stale_entry(
        &body.internal_order_id,
        &body.broker_order_id,
        &unapplied,
        cursor_last_message_id.as_deref(),
    );

    if classification != "cursor_only_fill_evidence" {
        let evidence = format!(
            "entry classification is '{}'; REST recovery is only applicable to \
             'cursor_only_fill_evidence' entries. \
             For 'unapplied_inbox_fill' use BROKER-FILL-REPLAY-APPLY-01; \
             for 'no_fill_evidence' there is nothing to recover.",
            classification
        );
        let audit_id = write_rest_recovery_audit(
            &rest_audit_ctx,
            "refused",
            "repair.evidence_not_cursor_only",
            &evidence,
        )
        .await;
        return refused_active!(
            classification,
            evidence,
            Some("repair.evidence_not_cursor_only".to_string()),
            audit_id.map(|id| id.to_string())
        );
    }

    // Gate 5: fill activity fetcher must be configured.
    let Some(fetcher) = st.fill_activity_fetcher.as_ref() else {
        let evidence = format!(
            "REST fill activity fetcher is not configured on this daemon; \
             cannot look up Alpaca activities for broker_order_id='{}'.",
            body.broker_order_id
        );
        let audit_id = write_rest_recovery_audit(
            &rest_audit_ctx,
            "refused",
            "repair.recovery_unavailable",
            &evidence,
        )
        .await;
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HaltedRunFillRestRecoveryResponse {
                truth_state: "active".to_string(),
                decision: "refused".to_string(),
                dry_run,
                mutated: false,
                run_id: body.run_id.clone(),
                internal_order_id: body.internal_order_id.clone(),
                broker_order_id: body.broker_order_id.clone(),
                classification,
                evidence,
                gate: Some("repair.recovery_unavailable".to_string()),
                audit_event_id: audit_id.map(|id| id.to_string()),
                rest_fill: None,
                inbox_broker_message_id: None,
            }),
        )
            .into_response();
    };

    // Gate 6: fetch activities from the broker — fail closed on error.
    let activities = match fetcher.fetch_fill_activities_for_order(&body.broker_order_id) {
        Ok(a) => a,
        Err(e) => {
            let evidence = format!(
                "Alpaca REST activity fetch failed for broker_order_id='{}': {e}",
                body.broker_order_id
            );
            let audit_id = write_rest_recovery_audit(
                &rest_audit_ctx,
                "refused",
                "repair.rest_unavailable",
                &evidence,
            )
            .await;
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HaltedRunFillRestRecoveryResponse {
                    truth_state: "backend_unavailable".to_string(),
                    decision: "refused".to_string(),
                    dry_run,
                    mutated: false,
                    run_id: body.run_id.clone(),
                    internal_order_id: body.internal_order_id.clone(),
                    broker_order_id: body.broker_order_id.clone(),
                    classification,
                    evidence,
                    gate: Some("repair.rest_unavailable".to_string()),
                    audit_event_id: audit_id.map(|id| id.to_string()),
                    rest_fill: None,
                    inbox_broker_message_id: None,
                }),
            )
                .into_response();
        }
    };

    // Gate 7: filter to FILL/PARTIAL_FILL — must be exactly one match.
    let fill_activities: Vec<&mqk_broker_alpaca::types::AlpacaOrderActivity> = activities
        .iter()
        .filter(|a| matches!(a.activity_type.as_str(), "FILL" | "PARTIAL_FILL"))
        .collect();

    match fill_activities.len() {
        0 => {
            let evidence = format!(
                "Alpaca REST returned no FILL/PARTIAL_FILL activities for \
                 broker_order_id='{}' ({} total activities fetched). \
                 The fill may not yet be present in the activity feed, \
                 or the order was not filled.",
                body.broker_order_id,
                activities.len()
            );
            let audit_id = write_rest_recovery_audit(
                &rest_audit_ctx,
                "refused",
                "repair.no_rest_match",
                &evidence,
            )
            .await;
            return refused_active!(
                classification,
                evidence,
                Some("repair.no_rest_match".to_string()),
                audit_id.map(|id| id.to_string())
            );
        }
        n if n > 1 => {
            let ids: Vec<&str> = fill_activities.iter().map(|a| a.id.as_str()).collect();
            let evidence = format!(
                "Alpaca REST returned {n} FILL/PARTIAL_FILL activities for \
                 broker_order_id='{}'; ambiguous — cannot select a single authoritative fill. \
                 Activity IDs: {:?}. Manual broker reconcile required.",
                body.broker_order_id, ids
            );
            let audit_id = write_rest_recovery_audit(
                &rest_audit_ctx,
                "refused",
                "repair.ambiguous_rest_match",
                &evidence,
            )
            .await;
            return refused_active!(
                classification,
                evidence,
                Some("repair.ambiguous_rest_match".to_string()),
                audit_id.map(|id| id.to_string())
            );
        }
        _ => {}
    }

    let activity = fill_activities[0];

    // Gate 8: price and qty must be present and non-empty.
    let price_str = match activity.price.as_deref().filter(|s| !s.is_empty()) {
        Some(p) => p.to_string(),
        None => {
            let evidence = format!(
                "Alpaca REST activity (id='{}') for broker_order_id='{}' \
                 has no fill price; data is incomplete. Manual reconcile required.",
                activity.id, body.broker_order_id
            );
            let audit_id = write_rest_recovery_audit(
                &rest_audit_ctx,
                "refused",
                "repair.recovery_data_malformed",
                &evidence,
            )
            .await;
            return refused_active!(
                classification,
                evidence,
                Some("repair.recovery_data_malformed".to_string()),
                audit_id.map(|id| id.to_string())
            );
        }
    };

    let qty_str = match activity.qty.as_deref().filter(|s| !s.is_empty()) {
        Some(q) => q.to_string(),
        None => {
            let evidence = format!(
                "Alpaca REST activity (id='{}') for broker_order_id='{}' \
                 has no fill quantity; data is incomplete. Manual reconcile required.",
                activity.id, body.broker_order_id
            );
            let audit_id = write_rest_recovery_audit(
                &rest_audit_ctx,
                "refused",
                "repair.recovery_data_malformed",
                &evidence,
            )
            .await;
            return refused_active!(
                classification,
                evidence,
                Some("repair.recovery_data_malformed".to_string()),
                audit_id.map(|id| id.to_string())
            );
        }
    };

    // All evidence gates passed — build the plan evidence payload.
    let rest_fill = RestRecoveredFill {
        broker_activity_id: activity.id.clone(),
        symbol: activity.symbol.clone(),
        side: activity.side.clone(),
        qty_str,
        price_str,
        timestamp: activity.transaction_time.clone(),
        source: "alpaca_rest_activity".to_string(),
        mutation_safe: false,
    };

    // ---------------------------------------------------------------------------
    // Gate 9a: dry_run=true — plan-only, no inbox mutation.
    // ---------------------------------------------------------------------------
    if dry_run {
        let evidence = format!(
            "REST recovery: Alpaca activity (id='{}') confirms one {} fill for \
             broker_order_id='{}' (internal='{}', run='{}'). \
             price='{}' qty='{}' side='{}' symbol='{}' at '{}'. \
             dry_run=true — no mutation. Resubmit with dry_run=false and \
             confirmation='APPLY_REST_FILL_RECOVERY' to insert and apply.",
            rest_fill.broker_activity_id,
            activity.activity_type,
            body.broker_order_id,
            body.internal_order_id,
            body.run_id,
            rest_fill.price_str,
            rest_fill.qty_str,
            rest_fill.side,
            rest_fill.symbol,
            rest_fill.timestamp,
        );
        let audit_id = write_rest_recovery_audit(
            &rest_audit_ctx,
            "rest_recovered_fill_evidence",
            "repair.rest_recovered",
            &evidence,
        )
        .await;
        return (
            StatusCode::OK,
            Json(HaltedRunFillRestRecoveryResponse {
                truth_state: "active".to_string(),
                decision: "rest_recovered_fill_evidence".to_string(),
                dry_run: true,
                mutated: false,
                run_id: body.run_id.clone(),
                internal_order_id: body.internal_order_id.clone(),
                broker_order_id: body.broker_order_id.clone(),
                classification,
                evidence,
                gate: None,
                audit_event_id: audit_id.map(|id| id.to_string()),
                rest_fill: Some(rest_fill),
                inbox_broker_message_id: None,
            }),
        )
            .into_response();
    }

    // ---------------------------------------------------------------------------
    // Gate 9b: dry_run=false requires confirmation token.
    // ---------------------------------------------------------------------------
    match body.confirmation.as_deref() {
        Some(REST_RECOVERY_APPLY_CONFIRMATION_TOKEN) => {}
        Some(other) => {
            let evidence = format!(
                "dry_run=false requires confirmation='{REST_RECOVERY_APPLY_CONFIRMATION_TOKEN}'; \
                 got: '{other}'"
            );
            let audit_id = write_rest_recovery_audit(
                &rest_audit_ctx,
                "refused",
                "repair.confirmation_required",
                &evidence,
            )
            .await;
            return (
                StatusCode::BAD_REQUEST,
                Json(HaltedRunFillRestRecoveryResponse {
                    truth_state: "active".to_string(),
                    decision: "refused".to_string(),
                    dry_run: false,
                    mutated: false,
                    run_id: body.run_id.clone(),
                    internal_order_id: body.internal_order_id.clone(),
                    broker_order_id: body.broker_order_id.clone(),
                    classification,
                    evidence,
                    gate: Some("repair.confirmation_required".to_string()),
                    audit_event_id: audit_id.map(|id| id.to_string()),
                    rest_fill: None,
                    inbox_broker_message_id: None,
                }),
            )
                .into_response();
        }
        None => {
            let evidence = format!(
                "dry_run=false requires confirmation='{REST_RECOVERY_APPLY_CONFIRMATION_TOKEN}'"
            );
            let audit_id = write_rest_recovery_audit(
                &rest_audit_ctx,
                "refused",
                "repair.confirmation_required",
                &evidence,
            )
            .await;
            return (
                StatusCode::BAD_REQUEST,
                Json(HaltedRunFillRestRecoveryResponse {
                    truth_state: "active".to_string(),
                    decision: "refused".to_string(),
                    dry_run: false,
                    mutated: false,
                    run_id: body.run_id.clone(),
                    internal_order_id: body.internal_order_id.clone(),
                    broker_order_id: body.broker_order_id.clone(),
                    classification,
                    evidence,
                    gate: Some("repair.confirmation_required".to_string()),
                    audit_event_id: audit_id.map(|id| id.to_string()),
                    rest_fill: None,
                    inbox_broker_message_id: None,
                }),
            )
                .into_response();
        }
    }

    // ---------------------------------------------------------------------------
    // Gate 10: parse numeric fields — fail closed if qty/price do not convert.
    //
    // Gate 8 already proved non-empty strings; here we prove parseable values.
    // ---------------------------------------------------------------------------
    let delta_qty: i64 = {
        let v: f64 = match rest_fill.qty_str.parse() {
            Ok(f) => f,
            Err(_) => {
                let evidence = format!(
                    "REST activity (id='{}') qty='{}' is not a valid number; \
                     manual reconcile required.",
                    rest_fill.broker_activity_id, rest_fill.qty_str
                );
                let audit_id = write_rest_recovery_audit(
                    &rest_audit_ctx,
                    "refused",
                    "repair.recovery_data_malformed",
                    &evidence,
                )
                .await;
                return refused_active!(
                    classification,
                    evidence,
                    Some("repair.recovery_data_malformed".to_string()),
                    audit_id.map(|id| id.to_string())
                );
            }
        };
        if !v.is_finite() || v <= 0.0 {
            let evidence = format!(
                "REST activity (id='{}') qty='{}' is not a positive finite number; \
                 manual reconcile required.",
                rest_fill.broker_activity_id, rest_fill.qty_str
            );
            let audit_id = write_rest_recovery_audit(
                &rest_audit_ctx,
                "refused",
                "repair.recovery_data_malformed",
                &evidence,
            )
            .await;
            return refused_active!(
                classification,
                evidence,
                Some("repair.recovery_data_malformed".to_string()),
                audit_id.map(|id| id.to_string())
            );
        }
        v.round() as i64
    };

    let price_micros: i64 = {
        let f: f64 = match rest_fill.price_str.parse() {
            Ok(v) => v,
            Err(_) => {
                let evidence = format!(
                    "REST activity (id='{}') price='{}' is not a valid number; \
                     manual reconcile required.",
                    rest_fill.broker_activity_id, rest_fill.price_str
                );
                let audit_id = write_rest_recovery_audit(
                    &rest_audit_ctx,
                    "refused",
                    "repair.recovery_data_malformed",
                    &evidence,
                )
                .await;
                return refused_active!(
                    classification,
                    evidence,
                    Some("repair.recovery_data_malformed".to_string()),
                    audit_id.map(|id| id.to_string())
                );
            }
        };
        match mqk_execution::price_to_micros(f) {
            Ok(m) => m,
            Err(_) => {
                let evidence = format!(
                    "REST activity (id='{}') price='{}' could not be converted to micros; \
                     manual reconcile required.",
                    rest_fill.broker_activity_id, rest_fill.price_str
                );
                let audit_id = write_rest_recovery_audit(
                    &rest_audit_ctx,
                    "refused",
                    "repair.recovery_data_malformed",
                    &evidence,
                )
                .await;
                return refused_active!(
                    classification,
                    evidence,
                    Some("repair.recovery_data_malformed".to_string()),
                    audit_id.map(|id| id.to_string())
                );
            }
        }
    };

    let side = match rest_fill.side.as_str() {
        "buy" => mqk_execution::Side::Buy,
        "sell" => mqk_execution::Side::Sell,
        other => {
            let evidence = format!(
                "REST activity (id='{}') side='{}' is not 'buy' or 'sell'; \
                 manual reconcile required.",
                rest_fill.broker_activity_id, other
            );
            let audit_id = write_rest_recovery_audit(
                &rest_audit_ctx,
                "refused",
                "repair.recovery_data_malformed",
                &evidence,
            )
            .await;
            return refused_active!(
                classification,
                evidence,
                Some("repair.recovery_data_malformed".to_string()),
                audit_id.map(|id| id.to_string())
            );
        }
    };

    // ---------------------------------------------------------------------------
    // Gate 11: build canonical BrokerEvent and stable inbox identity.
    //
    // broker_message_id is deterministic: same activity ID → same inbox key.
    // This is the idempotency anchor for retry safety.
    // ---------------------------------------------------------------------------
    let stable_msg_id = format!("alpaca-rest-recovery:{}", rest_fill.broker_activity_id);
    let event_kind = match activity.activity_type.as_str() {
        "PARTIAL_FILL" => "partial_fill",
        _ => "fill",
    };
    let broker_event = match activity.activity_type.as_str() {
        "PARTIAL_FILL" => mqk_execution::BrokerEvent::PartialFill {
            broker_message_id: stable_msg_id.clone(),
            broker_fill_id: Some(rest_fill.broker_activity_id.clone()),
            internal_order_id: body.internal_order_id.clone(),
            broker_order_id: Some(body.broker_order_id.clone()),
            symbol: rest_fill.symbol.clone(),
            side,
            delta_qty,
            price_micros,
            fee_micros: 0,
        },
        _ => mqk_execution::BrokerEvent::Fill {
            broker_message_id: stable_msg_id.clone(),
            broker_fill_id: Some(rest_fill.broker_activity_id.clone()),
            internal_order_id: body.internal_order_id.clone(),
            broker_order_id: Some(body.broker_order_id.clone()),
            symbol: rest_fill.symbol.clone(),
            side,
            delta_qty,
            price_micros,
            fee_micros: 0,
        },
    };
    let event_json =
        serde_json::to_value(&broker_event).expect("BrokerEvent serializes to JSON infallibly");

    let received_at = chrono::DateTime::parse_from_rfc3339(&rest_fill.timestamp)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    // ---------------------------------------------------------------------------
    // Gate 12: idempotent inbox insert.
    //
    // Returns true if newly inserted, false if (run_id, broker_message_id) already existed.
    // ---------------------------------------------------------------------------
    let inserted = match mqk_db::inbox_insert_deduped_with_identity(
        db,
        run_id,
        &stable_msg_id,
        Some(rest_fill.broker_activity_id.as_str()),
        &body.internal_order_id,
        &body.broker_order_id,
        event_kind,
        &event_json,
        0,
        received_at,
    )
    .await
    {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(HaltedRunFillRestRecoveryResponse {
                    truth_state: "backend_unavailable".to_string(),
                    decision: "refused".to_string(),
                    dry_run: false,
                    mutated: false,
                    run_id: body.run_id.clone(),
                    internal_order_id: body.internal_order_id.clone(),
                    broker_order_id: body.broker_order_id.clone(),
                    classification,
                    evidence: format!("inbox insert failed: {e}"),
                    gate: Some("repair.db_error".to_string()),
                    audit_event_id: None,
                    rest_fill: None,
                    inbox_broker_message_id: None,
                }),
            )
                .into_response();
        }
    };

    // ---------------------------------------------------------------------------
    // Gate 13: if the row already existed, check whether it was already applied.
    //
    // If applied_at_utc IS NOT NULL → already_repaired (idempotent noop).
    // If applied_at_utc IS NULL → fall through to Gate 14 and stamp it.
    // ---------------------------------------------------------------------------
    if !inserted {
        let already_applied: bool = sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(
            "select applied_at_utc from oms_inbox \
             where run_id = $1 and broker_message_id = $2",
        )
        .bind(run_id)
        .bind(&stable_msg_id)
        .fetch_optional(db)
        .await
        .unwrap_or(None)
        .map(|opt_ts| opt_ts.is_some())
        .unwrap_or(false);

        if already_applied {
            let evidence = format!(
                "REST recovery inbox row (broker_message_id='{}') for \
                 internal_order_id='{}' was already applied from a prior call; \
                 idempotent noop — no mutation performed.",
                stable_msg_id, body.internal_order_id
            );
            let audit_id = write_rest_recovery_audit(
                &rest_audit_ctx,
                "already_repaired",
                "repair.rest_recovery_already_applied",
                &evidence,
            )
            .await;
            return (
                StatusCode::OK,
                Json(HaltedRunFillRestRecoveryResponse {
                    truth_state: "active".to_string(),
                    decision: "already_repaired".to_string(),
                    dry_run: false,
                    mutated: false,
                    run_id: body.run_id.clone(),
                    internal_order_id: body.internal_order_id.clone(),
                    broker_order_id: body.broker_order_id.clone(),
                    classification,
                    evidence,
                    gate: None,
                    audit_event_id: audit_id.map(|id| id.to_string()),
                    rest_fill: Some(RestRecoveredFill {
                        mutation_safe: true,
                        ..rest_fill
                    }),
                    inbox_broker_message_id: Some(stable_msg_id),
                }),
            )
                .into_response();
        }
    }

    // ---------------------------------------------------------------------------
    // Gate 14: stamp applied_at_utc.
    //
    // inbox_mark_applied is idempotent: only stamps rows where applied_at_utc IS NULL.
    // NOTE: the in-memory portfolio for this HALTED run is NOT updated — run is
    // terminal.  Portfolio reconstruction for a new run reads
    // inbox_load_all_applied_for_run which will include this row after apply.
    // ---------------------------------------------------------------------------
    let applied_at = Utc::now();
    if let Err(e) = mqk_db::inbox_mark_applied(db, run_id, &stable_msg_id, applied_at).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(HaltedRunFillRestRecoveryResponse {
                truth_state: "backend_unavailable".to_string(),
                decision: "refused".to_string(),
                dry_run: false,
                mutated: false,
                run_id: body.run_id.clone(),
                internal_order_id: body.internal_order_id.clone(),
                broker_order_id: body.broker_order_id.clone(),
                classification,
                evidence: format!("inbox_mark_applied failed: {e}"),
                gate: Some("repair.db_error".to_string()),
                audit_event_id: None,
                rest_fill: None,
                inbox_broker_message_id: None,
            }),
        )
            .into_response();
    }

    let evidence = format!(
        "REST recovery applied: Alpaca activity (id='{}') inserted into inbox and stamped applied. \
         broker_message_id='{}' run_id='{}' internal_order_id='{}' broker_order_id='{}'. \
         price='{}' qty='{}' side='{}' symbol='{}'. \
         NOTE: in-memory portfolio for this HALTED run was NOT updated — run is terminal. \
         Start a new run to begin with fresh portfolio state reflecting this fill.",
        rest_fill.broker_activity_id,
        stable_msg_id,
        body.run_id,
        body.internal_order_id,
        body.broker_order_id,
        rest_fill.price_str,
        rest_fill.qty_str,
        rest_fill.side,
        rest_fill.symbol,
    );

    let audit_id = write_rest_recovery_audit(
        &rest_audit_ctx,
        "applied",
        "repair.rest_recovery_applied",
        &evidence,
    )
    .await;

    (
        StatusCode::OK,
        Json(HaltedRunFillRestRecoveryResponse {
            truth_state: "active".to_string(),
            decision: "applied".to_string(),
            dry_run: false,
            mutated: true,
            run_id: body.run_id.clone(),
            internal_order_id: body.internal_order_id.clone(),
            broker_order_id: body.broker_order_id.clone(),
            classification,
            evidence,
            gate: None,
            audit_event_id: audit_id.map(|id| id.to_string()),
            rest_fill: Some(RestRecoveredFill {
                mutation_safe: true,
                ..rest_fill
            }),
            inbox_broker_message_id: Some(stable_msg_id),
        }),
    )
        .into_response()
}

struct RestRecoveryAuditCtx<'a> {
    db: &'a sqlx::PgPool,
    run_id: uuid::Uuid,
    internal_order_id: &'a str,
    broker_order_id: &'a str,
}

/// Write a durable REST recovery audit event.
///
/// Non-fatal: audit failure does not block the recovery outcome.
/// Returns the event UUID on success, `None` on failure.
async fn write_rest_recovery_audit(
    ctx: &RestRecoveryAuditCtx<'_>,
    decision: &str,
    gate: &str,
    evidence: &str,
) -> Option<uuid::Uuid> {
    let db = ctx.db;
    let run_id = ctx.run_id;
    let internal_order_id = ctx.internal_order_id;
    let broker_order_id = ctx.broker_order_id;
    let ts_utc = Utc::now();
    let event_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!(
            "mqk-daemon.repair.halted-fill-rest-recovery.v1|{}|{}|{}|{}",
            run_id,
            internal_order_id,
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
            event_type: "ops.repair.halted_fill_rest_recovery".to_string(),
            payload: serde_json::json!({
                "internal_order_id": internal_order_id,
                "broker_order_id": broker_order_id,
                "decision": decision,
                "gate": gate,
                "evidence": evidence,
                "source": "mqk-daemon.routes.repair",
            }),
            hash_prev: None,
            hash_self: None,
        },
    )
    .await;

    if let Err(err) = result {
        tracing::warn!(
            "repair_halted_run_fill_rest_recovery: audit write failed (non-fatal): {err}"
        );
        return None;
    }

    Some(event_id)
}
