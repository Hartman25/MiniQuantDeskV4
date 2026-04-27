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
    api_types::{OutboxRepairRequest, OutboxRepairResponse},
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
