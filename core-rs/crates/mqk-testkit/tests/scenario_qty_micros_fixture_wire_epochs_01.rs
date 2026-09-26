//! QtyMicros wire-epoch controls for broker-event test fixtures.
//!
//! The inbox writer stamps the CURRENT `schema_version` on any `message_json`
//! that lacks one, so a hand-written `"delta_qty": 1` inserted through
//! `inbox_insert_deduped*` is a raw-micros 0.000001, not one share. Only rows
//! that already carry `schema_version = 1` (or none, written by raw SQL / prior
//! builds) mean whole units. These controls pin both epochs and the writer
//! behaviour that makes an unversioned fixture ambiguous, so the frozen
//! QtyMicros contract cannot be weakened to make a stale fixture pass.

use mqk_db::{
    quantity_unit_epoch, QuantityUnitEpoch, LEGACY_WHOLE_UNIT_SCHEMA_VERSION,
    MESSAGE_JSON_SCHEMA_VERSION,
};
use mqk_execution::{decode_broker_event, BrokerEvent, QtyMicros, Side};
use serde_json::json;
use uuid::Uuid;

fn fill_event(delta_qty: QtyMicros) -> BrokerEvent {
    BrokerEvent::Fill {
        broker_message_id: "qty-ci-msg".to_string(),
        broker_fill_id: None,
        internal_order_id: "qty-ci-ord".to_string(),
        broker_order_id: Some("qty-ci-broker".to_string()),
        symbol: "AAPL".to_string(),
        side: Side::Buy,
        delta_qty,
        price_micros: 100_000_000,
        fee_micros: 0,
    }
}

fn fill_delta(event: &BrokerEvent) -> QtyMicros {
    match event {
        BrokerEvent::Fill { delta_qty, .. } => *delta_qty,
        other => panic!("expected Fill, got {other:?}"),
    }
}

fn stamped_current(mut v: serde_json::Value) -> serde_json::Value {
    v["schema_version"] = json!(MESSAGE_JSON_SCHEMA_VERSION);
    v
}

/// QTY-CI-01: one whole share serialises as raw 1_000_000 and decodes back to
/// exactly one share.
#[test]
fn qty_ci_01_one_whole_share_round_trips_as_qty_micros() {
    let one = QtyMicros::from_whole_units(1).unwrap();
    let wire = serde_json::to_value(fill_event(one)).unwrap();
    assert_eq!(wire["delta_qty"], json!(1_000_000));
    let decoded = decode_broker_event(&stamped_current(wire)).unwrap();
    assert_eq!(fill_delta(&decoded), one);
    assert_eq!(fill_delta(&decoded).to_whole_units_checked(), Some(1));
}

/// QTY-CI-02: raw integer 1 in a current-schema envelope stays 0.000001.
#[test]
fn qty_ci_02_current_schema_raw_one_is_one_micro_not_one_share() {
    let mut wire = serde_json::to_value(fill_event(QtyMicros::ZERO)).unwrap();
    wire["delta_qty"] = json!(1);
    let decoded = decode_broker_event(&stamped_current(wire)).unwrap();
    assert_eq!(fill_delta(&decoded), QtyMicros::new(1));
    assert_ne!(
        fill_delta(&decoded),
        QtyMicros::from_whole_units(1).unwrap()
    );
    assert_eq!(fill_delta(&decoded).to_whole_units_checked(), None);
}

/// QTY-CI-03: a genuinely fractional current event round-trips exactly.
#[test]
fn qty_ci_03_fractional_current_event_round_trips_exactly() {
    for raw in [1, 123_456, 500_000, 1_500_000, 42_000_001] {
        let q = QtyMicros::new(raw);
        let wire = stamped_current(serde_json::to_value(fill_event(q)).unwrap());
        assert_eq!(wire["delta_qty"], json!(raw));
        assert_eq!(fill_delta(&decode_broker_event(&wire).unwrap()), q);
    }
}

/// QTY-CI-04: a missing or explicit-legacy `schema_version` still means whole
/// units.
#[test]
fn qty_ci_04_legacy_integer_one_still_means_one_whole_unit() {
    let one = QtyMicros::from_whole_units(1).unwrap();
    let mut unversioned = serde_json::to_value(fill_event(QtyMicros::ZERO)).unwrap();
    unversioned["delta_qty"] = json!(1);
    assert_eq!(
        quantity_unit_epoch(&unversioned).unwrap(),
        QuantityUnitEpoch::LegacyWholeUnits
    );
    assert_eq!(fill_delta(&decode_broker_event(&unversioned).unwrap()), one);

    let mut explicit = unversioned.clone();
    explicit["schema_version"] = json!(LEGACY_WHOLE_UNIT_SCHEMA_VERSION);
    assert_eq!(fill_delta(&decode_broker_event(&explicit).unwrap()), one);
}

/// The ambiguity that broke the stale fixtures: an unversioned hand-built
/// payload written through the production inbox writer is stored at the
/// CURRENT epoch, so its `1` decodes as one micro; the same fixture built via
/// `BrokerEvent` round-trips as one share.
#[tokio::test]
async fn qty_ci_writer_stamps_unversioned_fixture_at_current_epoch() {
    mqk_db::run_isolated("qty_ci_writer_epoch", |pool| async move {
        let run_id = Uuid::new_v4();
        mqk_db::insert_run(
            &pool,
            &mqk_db::NewRun {
                run_id,
                engine_id: "qty-ci".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: chrono::Utc::now(),
                git_hash: "qty-ci".to_string(),
                config_hash: "qty-ci".to_string(),
                config_json: json!({}),
                host_fingerprint: "qty-ci".to_string(),
            },
        )
        .await
        .unwrap();

        let mut hand_built = serde_json::to_value(fill_event(QtyMicros::ZERO)).unwrap();
        hand_built["delta_qty"] = json!(1);
        hand_built["broker_message_id"] = json!("qty-ci-hand");
        assert!(
            mqk_db::inbox_insert_deduped(&pool, run_id, "qty-ci-hand", hand_built)
                .await
                .unwrap()
        );

        let mut via_event =
            serde_json::to_value(fill_event(QtyMicros::from_whole_units(1).unwrap())).unwrap();
        via_event["broker_message_id"] = json!("qty-ci-event");
        via_event["internal_order_id"] = json!("qty-ci-ord-2");
        assert!(
            mqk_db::inbox_insert_deduped(&pool, run_id, "qty-ci-event", via_event)
                .await
                .unwrap()
        );

        let rows = mqk_db::inbox_load_all_for_run(&pool, run_id).await.unwrap();
        let decode = |id: &str| {
            let row = rows
                .iter()
                .find(|r| r.broker_message_id == id)
                .expect("row present");
            assert_eq!(
                quantity_unit_epoch(&row.message_json).unwrap(),
                QuantityUnitEpoch::QtyMicros,
                "writer must stamp the current epoch"
            );
            fill_delta(&decode_broker_event(&row.message_json).unwrap())
        };
        assert_eq!(decode("qty-ci-hand"), QtyMicros::new(1));
        assert_eq!(
            decode("qty-ci-event"),
            QtyMicros::from_whole_units(1).unwrap()
        );
    })
    .await;
}
