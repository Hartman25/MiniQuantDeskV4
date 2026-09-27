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
    if !fill_qty.is_positive() {
        return None;
    }

    // Best-effort outbox lookup to derive ordered_qty, reference_price, submit_ts.
    let (ordered_qty_micros, reference_price_micros, submit_ts_utc) =
        match mqk_db::outbox_fetch_by_idempotency_key(pool, &internal_order_id).await {
            Ok(Some(outbox)) => {
                let reference_price_micros = outbox
                    .order_json
                    .get("limit_price")
                    .and_then(|v| v.as_i64());
                (
                    ordered_qty_micros_from_order_json(&outbox.order_json, fill_qty),
                    reference_price_micros,
                    outbox.sent_at_utc,
                )
            }
            _ => (Some(fill_qty), None, None),
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

    // D6/A5, D7 (CUTOVER-1B-OMS-QTY-MICROS-01 superseded): a genuinely
    // fractional Crypto fill is no longer skipped -- migration 0079 gives
    // `fill_quality_telemetry` an explicit qty_micros_v1 encoding
    // (`fill_qty_encoding_check`) so the exact `QtyMicros` value is captured
    // instead of silently omitting telemetry for the event. A whole fill
    // writes the historical whole-unit column unchanged
    // (`quantity_schema_version = NULL`, byte-identical to a pre-D6/A5 row).
    let (fill_qty_col, fill_qty_micros_col, quantity_schema_version) =
        match fill_qty.to_whole_units_checked() {
            Some(whole) => (Some(whole), None, None),
            None => (
                None,
                Some(fill_qty.raw()),
                Some("qty_micros_v1".to_string()),
            ),
        };
    // Ordered-qty encoding is independent (`ordered_qty_encoding_check`):
    // whole when representable, exact micros when genuinely fractional, or
    // both absent when the order quantity was never resolvable at all.
    let (ordered_qty_col, ordered_qty_micros_col) = match ordered_qty_micros {
        None => (None, None),
        Some(q) => match q.to_whole_units_checked() {
            Some(whole) => (Some(whole), None),
            None => (None, Some(q.raw())),
        },
    };

    Some(mqk_db::NewFillQualityTelemetry {
        telemetry_id,
        run_id,
        internal_order_id,
        broker_order_id,
        broker_fill_id,
        broker_message_id: broker_message_id.to_string(),
        symbol,
        side: side_str.to_string(),
        ordered_qty: ordered_qty_col,
        ordered_qty_micros: ordered_qty_micros_col,
        fill_qty: fill_qty_col,
        fill_qty_micros: fill_qty_micros_col,
        quantity_schema_version,
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

/// Ordered quantity (raw `QtyMicros`) for telemetry. An absent `qty` falls
/// back to the fill's own exact quantity (unchanged best-effort behavior,
/// now exact rather than whole-unit-only); a present-but-unparseable `qty`
/// (malformed JSON) yields `None` -- never the fill quantity standing in for
/// an order size that is not knowable here.
fn ordered_qty_micros_from_order_json(
    order_json: &serde_json::Value,
    fill_qty: mqk_execution::QtyMicros,
) -> Option<mqk_execution::QtyMicros> {
    match order_json.get("qty") {
        None => Some(fill_qty),
        Some(serde_json::Value::Number(n)) => n
            .as_i64()
            .and_then(mqk_execution::QtyMicros::from_whole_units),
        Some(serde_json::Value::String(s)) => s.parse().ok(),
        Some(_) => None,
    }
}

fn side_to_str(side: &mqk_execution::Side) -> &'static str {
    match side {
        mqk_execution::Side::Buy => "buy",
        mqk_execution::Side::Sell => "sell",
    }
}

#[cfg(test)]
mod cutover_1d_a3_tests {
    use super::*;
    use mqk_execution::QtyMicros;

    fn whole(n: i64) -> QtyMicros {
        QtyMicros::from_whole_units(n).unwrap()
    }

    #[test]
    fn ordered_qty_is_never_fabricated_for_an_unparseable_order() {
        let j = |v: serde_json::Value| ordered_qty_micros_from_order_json(&v, whole(3));
        assert_eq!(j(serde_json::json!({"qty": 10})), Some(whole(10)));
        assert_eq!(j(serde_json::json!({})), Some(whole(3)));
        // D6/A5: a fractional decimal STRING is now genuinely parseable
        // (exact), unlike the old whole-unit-only `as_i64()` path.
        assert_eq!(
            j(serde_json::json!({"qty": "0.0001"})),
            Some(QtyMicros::new(100))
        );
        // A fractional JSON *number* (float) is never fabricated -- lossy
        // f64 decimal reconstruction is refused, not approximated.
        assert_eq!(j(serde_json::json!({"qty": 0.5})), None);
        assert_eq!(j(serde_json::json!({"qty": "garbage"})), None);
        assert_eq!(j(serde_json::json!({"qty": null})), None);
    }
}
