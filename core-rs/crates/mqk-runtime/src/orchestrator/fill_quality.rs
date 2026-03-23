//! Fill quality telemetry row construction.
//!
//! Pure (aside from the best-effort DB outbox lookup) async helper that maps a
//! fill `BrokerEvent` to a `NewFillQualityTelemetry` row.  Non-fill events
//! return `None`.
//!
//! # Exports
//!
//! - `build_fill_quality_row` — produce telemetry for a fill event (async).

use mqk_execution::BrokerEvent;
use sqlx::types::chrono;
use sqlx::PgPool;
use uuid::Uuid;

/// Build a `NewFillQualityTelemetry` row for a Fill or PartialFill event.
///
/// Returns `None` for all other event kinds — no fabrication for non-fill events.
///
/// Outbox lookup is best-effort: if the row is absent or the DB call fails,
/// submit_ts_utc / reference_price / ordered_qty fall back to None / null.
pub(super) async fn build_fill_quality_row(
    run_id: Uuid,
    broker_message_id: &str,
    event: &BrokerEvent,
    fill_received_at_utc: chrono::DateTime<chrono::Utc>,
    pool: &PgPool,
    now_utc: chrono::DateTime<chrono::Utc>,
) -> Option<mqk_db::NewFillQualityTelemetry> {
    // Only emit telemetry for fill events.
    let (
        internal_order_id,
        broker_order_id,
        broker_fill_id,
        symbol,
        side_str,
        fill_qty,
        fill_price_micros,
        fill_kind,
    ) = match event {
        BrokerEvent::Fill {
            internal_order_id,
            broker_order_id,
            broker_fill_id,
            symbol,
            side,
            delta_qty,
            price_micros,
            ..
        } => (
            internal_order_id.clone(),
            broker_order_id.clone(),
            broker_fill_id.clone(),
            symbol.clone(),
            side_to_str(side),
            *delta_qty,
            *price_micros,
            "final_fill",
        ),
        BrokerEvent::PartialFill {
            internal_order_id,
            broker_order_id,
            broker_fill_id,
            symbol,
            side,
            delta_qty,
            price_micros,
            ..
        } => (
            internal_order_id.clone(),
            broker_order_id.clone(),
            broker_fill_id.clone(),
            symbol.clone(),
            side_to_str(side),
            *delta_qty,
            *price_micros,
            "partial_fill",
        ),
        _ => return None,
    };

    // Skip degenerate fill events — same guard as broker_event_to_fill.
    if fill_qty <= 0 {
        return None;
    }

    // Best-effort outbox lookup to derive ordered_qty, reference_price, submit_ts.
    let (ordered_qty, reference_price_micros, submit_ts_utc) =
        match mqk_db::outbox_fetch_by_idempotency_key(pool, &internal_order_id).await {
            Ok(Some(outbox)) => {
                let ordered_qty = outbox
                    .order_json
                    .get("qty")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(fill_qty);
                let reference_price_micros = outbox
                    .order_json
                    .get("limit_price")
                    .and_then(|v| v.as_i64());
                (ordered_qty, reference_price_micros, outbox.sent_at_utc)
            }
            _ => (fill_qty, None, None),
        };

    // Slippage in bps — only meaningful when a reference (limit) price exists.
    // slippage = (fill_price - reference_price) / reference_price * 10_000
    // For a buy: positive = paid more than limit (adverse); for sell: positive = received more.
    let slippage_bps = reference_price_micros.and_then(|ref_price| {
        if ref_price == 0 {
            return None;
        }
        let diff = fill_price_micros - ref_price;
        Some(diff * 10_000 / ref_price)
    });

    // Submit-to-fill latency in ms.
    let submit_to_fill_ms =
        submit_ts_utc.map(|submit| (fill_received_at_utc - submit).num_milliseconds());

    // Deterministic telemetry_id — idempotent on replay.
    let telemetry_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("mqk.fill-quality.v1|{}|{}", run_id, broker_message_id).as_bytes(),
    );

    Some(mqk_db::NewFillQualityTelemetry {
        telemetry_id,
        run_id,
        internal_order_id,
        broker_order_id,
        broker_fill_id,
        broker_message_id: broker_message_id.to_string(),
        symbol,
        side: side_str.to_string(),
        ordered_qty,
        fill_qty,
        fill_price_micros,
        reference_price_micros,
        slippage_bps,
        submit_ts_utc,
        fill_received_at_utc,
        submit_to_fill_ms,
        fill_kind: fill_kind.to_string(),
        provenance_ref: format!("oms_inbox:{}", broker_message_id),
        created_at_utc: now_utc,
    })
}

fn side_to_str(side: &mqk_execution::Side) -> &'static str {
    match side {
        mqk_execution::Side::Buy => "buy",
        mqk_execution::Side::Sell => "sell",
    }
}
