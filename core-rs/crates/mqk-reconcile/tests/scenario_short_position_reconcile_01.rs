//! SHORT-SIDE-PAPER-BROKER-POSITION-PROOF-01 — Reconcile signed-short proofs.
//!
//! Proves that the reconcile engine and snapshot adapter correctly handle
//! negative signed quantities representing short positions.
//!
//! # Note on `parse_signed_qty` (daemon-internal)
//! The `parse_signed_qty` function in `mqk-daemon::state::snapshot` parses the
//! string wire format used in `mqk-schemas::BrokerPosition.qty` (e.g., "-50").
//! That function is `pub(crate)` and cannot be reached here.  The equivalent
//! path in this crate is `normalize()` / `normalize_json()` via
//! `RawBrokerPosition { qty_signed: i64 }`, which accepts negative i64 values
//! directly. RS06 and RS08 prove the normalization path preserves negative values.
//!
//! # Test inventory
//!
//! | ID   | Scenario                                                                     |
//! |------|------------------------------------------------------------------------------|
//! | RS01 | BrokerSnapshot positions map stores negative qty (short representation)      |
//! | RS02 | LocalSnapshot positions map stores negative qty (short representation)       |
//! | RS03 | Local short=-50, broker short=-50 → reconcile Clean                         |
//! | RS04 | Local short=-50, broker flat (symbol absent) → Halt PositionMismatch        |
//! | RS05 | Local flat (symbol absent), broker short=-50 → Halt + UnknownBrokerPosition  |
//! | RS06 | normalize() with RawBrokerPosition{qty_signed=-50} preserves negative value  |
//! | RS07 | Local short=-50, broker long=+50 → Halt (sign mismatch is drift)            |
//! | RS08 | normalize_json round-trip: JSON qty_signed=-50 → -50 preserved               |

use mqk_reconcile::{
    normalize, normalize_json, reconcile, BrokerSnapshot, LocalSnapshot, RawBrokerPosition,
    RawBrokerSnapshot, ReconcileAction, ReconcileDiff, ReconcileReason,
};

// ---------------------------------------------------------------------------
// RS01 — BrokerSnapshot positions map stores negative qty
// ---------------------------------------------------------------------------

#[test]
fn rs01_broker_snapshot_negative_qty_is_short() {
    let mut broker = BrokerSnapshot::empty_at(1_000);
    broker.positions.insert("GME".to_string(), -50);

    let qty = broker.positions.get("GME").copied().unwrap_or(0);
    assert_eq!(
        qty, -50,
        "RS01: BrokerSnapshot must store -50 for a short position"
    );
    assert!(
        qty < 0,
        "RS01: negative qty in broker positions represents a short"
    );
    assert!(
        broker.positions.contains_key("GME"),
        "RS01: short position must be present in positions map (not absent)"
    );
}

// ---------------------------------------------------------------------------
// RS02 — LocalSnapshot positions map stores negative qty
// ---------------------------------------------------------------------------

#[test]
fn rs02_local_snapshot_negative_qty_is_short() {
    let mut local = LocalSnapshot::empty();
    local.positions.insert("GME".to_string(), -50);

    let qty = local.positions.get("GME").copied().unwrap_or(0);
    assert_eq!(
        qty, -50,
        "RS02: LocalSnapshot must store -50 for a short position"
    );
    assert!(
        qty < 0,
        "RS02: negative qty in local positions represents a short"
    );
    assert!(
        local.positions.contains_key("GME"),
        "RS02: short position must be present in positions map (not absent)"
    );
}

// ---------------------------------------------------------------------------
// RS03 — Local short=-50, broker short=-50 → Clean
// ---------------------------------------------------------------------------

#[test]
fn rs03_matching_short_position_reconciles_clean() {
    let mut local = LocalSnapshot::empty();
    local.positions.insert("GME".to_string(), -50);

    let mut broker = BrokerSnapshot::empty_at(1_000);
    broker.positions.insert("GME".to_string(), -50);

    let r = reconcile(&local, &broker);
    assert_eq!(
        r.action,
        ReconcileAction::Clean,
        "RS03: local short=-50 vs broker short=-50 must be Clean; diffs: {:?}",
        r.diffs
    );
    assert!(
        r.diffs.is_empty(),
        "RS03: clean reconcile must produce no diffs"
    );
}

// ---------------------------------------------------------------------------
// RS04 — Local short=-50, broker flat (symbol absent) → Halt PositionMismatch
// ---------------------------------------------------------------------------

#[test]
fn rs04_local_short_vs_broker_flat_is_halt() {
    let mut local = LocalSnapshot::empty();
    local.positions.insert("GME".to_string(), -50);

    let broker = BrokerSnapshot::empty_at(1_000); // GME absent (flat at broker)

    let r = reconcile(&local, &broker);
    assert_eq!(
        r.action,
        ReconcileAction::Halt,
        "RS04: local short=-50 vs broker flat must Halt"
    );
    assert!(
        r.reasons.contains(&ReconcileReason::PositionMismatch),
        "RS04: must emit PositionMismatch; got {:?}",
        r.reasons
    );
    let has_diff = r.diffs.iter().any(|d| {
        if let ReconcileDiff::PositionQtyMismatch {
            symbol,
            local_qty,
            broker_qty,
        } = d
        {
            symbol == "GME" && *local_qty == -50 && *broker_qty == 0
        } else {
            false
        }
    });
    assert!(
        has_diff,
        "RS04: must emit PositionQtyMismatch(GME, -50, 0); diffs: {:?}",
        r.diffs
    );
}

// ---------------------------------------------------------------------------
// RS05 — Local flat, broker short=-50 → Halt PositionMismatch + UnknownBrokerPosition
// ---------------------------------------------------------------------------

#[test]
fn rs05_local_flat_vs_broker_short_is_halt_unknown_position() {
    let local = LocalSnapshot::empty(); // GME absent locally

    let mut broker = BrokerSnapshot::empty_at(1_000);
    broker.positions.insert("GME".to_string(), -50);

    let r = reconcile(&local, &broker);
    assert_eq!(
        r.action,
        ReconcileAction::Halt,
        "RS05: local flat vs broker short=-50 must Halt"
    );
    assert!(
        r.reasons.contains(&ReconcileReason::PositionMismatch),
        "RS05: must emit PositionMismatch; got {:?}",
        r.reasons
    );
    assert!(
        r.reasons.contains(&ReconcileReason::UnknownBrokerPosition),
        "RS05: broker holds short our portfolio has no record of → UnknownBrokerPosition; \
         got {:?}",
        r.reasons
    );
}

// ---------------------------------------------------------------------------
// RS06 — normalize() with RawBrokerPosition{qty_signed=-50} preserves negative
// ---------------------------------------------------------------------------

#[test]
fn rs06_normalize_preserves_negative_qty_signed() {
    let raw = RawBrokerSnapshot {
        orders: vec![],
        positions: vec![RawBrokerPosition {
            symbol: "GME".to_string(),
            qty_signed: -50,
        }],
        fetched_at_ms: 1_000,
    };

    let snap = normalize(raw).unwrap();
    let qty = snap.positions.get("GME").copied().unwrap_or(0);
    assert_eq!(
        qty, -50,
        "RS06: normalize() must preserve negative qty_signed=-50; got {qty}"
    );
    assert!(
        qty < 0,
        "RS06: short position must survive normalization as negative"
    );
}

// ---------------------------------------------------------------------------
// RS07 — Local short=-50 vs broker long=+50 → Halt (sign mismatch is drift)
// ---------------------------------------------------------------------------

#[test]
fn rs07_local_short_vs_broker_long_is_halt() {
    let mut local = LocalSnapshot::empty();
    local.positions.insert("GME".to_string(), -50);

    let mut broker = BrokerSnapshot::empty_at(1_000);
    broker.positions.insert("GME".to_string(), 50); // broker shows LONG

    let r = reconcile(&local, &broker);
    assert_eq!(
        r.action,
        ReconcileAction::Halt,
        "RS07: local short=-50 vs broker long=+50 must Halt (sign mismatch is real drift)"
    );
    assert!(
        r.reasons.contains(&ReconcileReason::PositionMismatch),
        "RS07: must emit PositionMismatch; got {:?}",
        r.reasons
    );
    let has_diff = r.diffs.iter().any(|d| {
        if let ReconcileDiff::PositionQtyMismatch {
            symbol,
            local_qty,
            broker_qty,
        } = d
        {
            symbol == "GME" && *local_qty == -50 && *broker_qty == 50
        } else {
            false
        }
    });
    assert!(
        has_diff,
        "RS07: must emit PositionQtyMismatch(GME, -50, +50); diffs: {:?}",
        r.diffs
    );
}

// ---------------------------------------------------------------------------
// RS08 — normalize_json round-trip: JSON with qty_signed=-50 → -50 preserved
// ---------------------------------------------------------------------------

#[test]
fn rs08_normalize_json_roundtrip_short_position() {
    let json = r#"{
        "orders": [],
        "positions": [
            { "symbol": "GME", "qty_signed": -50 }
        ],
        "fetched_at_ms": 1000
    }"#;

    let snap = normalize_json(json).unwrap();
    let qty = snap.positions.get("GME").copied().unwrap_or(0);
    assert_eq!(
        qty, -50,
        "RS08: normalize_json must parse qty_signed=-50 and preserve it; got {qty}"
    );
    assert!(
        qty < 0,
        "RS08: parsed short qty must be negative after JSON round-trip"
    );
    assert_eq!(
        snap.fetched_at_ms, 1_000,
        "RS08: fetched_at_ms must be preserved"
    );
}
