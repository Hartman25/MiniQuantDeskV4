//! AP-03: Pure in-memory normalization tests for Alpaca broker snapshot helpers.
//!
//! These tests exercise `normalize_account`, `normalize_position`,
//! `normalize_open_order`, and `build_snapshot` directly, with no HTTP calls.
//! All tests are deterministic and require no environment variables.
//!
//! # Test groups
//!
//! | Group | Scope                                              |
//! |-------|----------------------------------------------------|
//! | N1    | `normalize_account` field pass-through             |
//! | N2    | `normalize_position` field mapping (avg_price)     |
//! | N3    | `normalize_open_order` happy-path, optional fields |
//! | N4    | `normalize_open_order` error path (bad timestamp)  |
//! | N5    | `build_snapshot` assembly (fills always empty)     |
//! | N6    | Snapshot determinism (same input → same output)    |

use chrono::{Datelike, TimeZone, Timelike, Utc};
use mqk_broker_alpaca::types::{AlpacaAccountRaw, AlpacaOpenOrderRaw, AlpacaPositionRaw};
use mqk_broker_alpaca::{
    build_snapshot, normalize_account, normalize_open_order, normalize_position,
};

// ---------------------------------------------------------------------------
// N1 — normalize_account
// ---------------------------------------------------------------------------

#[test]
fn n1_account_fields_passed_through() {
    let raw = AlpacaAccountRaw {
        equity: "123456.78".to_string(),
        cash: "9999.00".to_string(),
        currency: "USD".to_string(),
    };
    let account = normalize_account(&raw);
    assert_eq!(account.equity, "123456.78");
    assert_eq!(account.cash, "9999.00");
    assert_eq!(account.currency, "USD");
}

#[test]
fn n1_account_preserves_exact_broker_string() {
    // Verifies no numeric rounding or reformatting occurs.
    let raw = AlpacaAccountRaw {
        equity: "99999999.999999".to_string(),
        cash: "0.000001".to_string(),
        currency: "USD".to_string(),
    };
    let account = normalize_account(&raw);
    assert_eq!(account.equity, "99999999.999999");
    assert_eq!(account.cash, "0.000001");
}

// ---------------------------------------------------------------------------
// N2 — normalize_position
// ---------------------------------------------------------------------------

#[test]
fn n2_position_avg_price_maps_from_avg_entry_price() {
    let raw = AlpacaPositionRaw {
        symbol: "AAPL".to_string(),
        qty: "100".to_string(),
        avg_entry_price: "175.42".to_string(),
    };
    let pos = normalize_position(&raw);
    assert_eq!(pos.symbol, "AAPL");
    assert_eq!(pos.qty, "100");
    assert_eq!(pos.avg_price, "175.42"); // canonical field name is avg_price
}

#[test]
fn n2_position_symbol_and_qty_passed_through() {
    let raw = AlpacaPositionRaw {
        symbol: "TSLA".to_string(),
        qty: "-50".to_string(), // short position
        avg_entry_price: "210.00".to_string(),
    };
    let pos = normalize_position(&raw);
    assert_eq!(pos.symbol, "TSLA");
    assert_eq!(pos.qty, "-50");
}

// ---------------------------------------------------------------------------
// N3 — normalize_open_order happy path
// ---------------------------------------------------------------------------

fn make_order_raw(limit_price: Option<&str>, stop_price: Option<&str>) -> AlpacaOpenOrderRaw {
    AlpacaOpenOrderRaw {
        id: "broker-uuid-001".to_string(),
        client_order_id: "client-uuid-abc".to_string(),
        symbol: "MSFT".to_string(),
        side: "buy".to_string(),
        order_type: "limit".to_string(),
        status: "new".to_string(),
        qty: "200".to_string(),
        limit_price: limit_price.map(str::to_string),
        stop_price: stop_price.map(str::to_string),
        created_at: "2024-01-15T09:30:00Z".to_string(),
    }
}

#[test]
fn n3_open_order_all_fields_normalized() {
    let raw = make_order_raw(Some("300.00"), None);
    let order = normalize_open_order(&raw).expect("valid order must normalize");
    assert_eq!(order.broker_order_id, "broker-uuid-001");
    assert_eq!(order.client_order_id, "client-uuid-abc");
    assert_eq!(order.symbol, "MSFT");
    assert_eq!(order.side, "buy");
    assert_eq!(order.r#type, "limit");
    assert_eq!(order.status, "new");
    assert_eq!(order.qty, "200");
    assert_eq!(order.limit_price, Some("300.00".to_string()));
    assert_eq!(order.stop_price, None);
}

#[test]
fn n3_open_order_created_at_parsed_correctly() {
    let raw = make_order_raw(None, None);
    let order = normalize_open_order(&raw).expect("valid order must normalize");
    // 2024-01-15T09:30:00Z → epoch seconds
    let expected = Utc.with_ymd_and_hms(2024, 1, 15, 9, 30, 0).unwrap();
    assert_eq!(order.created_at_utc, expected);
}

#[test]
fn n3_open_order_stop_price_optional() {
    let raw = make_order_raw(None, Some("290.00"));
    let order = normalize_open_order(&raw).expect("valid order must normalize");
    assert_eq!(order.limit_price, None);
    assert_eq!(order.stop_price, Some("290.00".to_string()));
}

#[test]
fn n3_open_order_both_optional_none() {
    let raw = make_order_raw(None, None);
    let order = normalize_open_order(&raw).expect("valid order must normalize");
    assert_eq!(order.limit_price, None);
    assert_eq!(order.stop_price, None);
}

#[test]
fn n3_open_order_fractional_second_timestamp() {
    // Alpaca sometimes returns sub-second timestamps.
    let mut raw = make_order_raw(None, None);
    raw.created_at = "2024-06-01T14:23:45.123456789Z".to_string();
    let order = normalize_open_order(&raw).expect("fractional-second RFC 3339 must parse");
    // Sub-second precision is accepted; date/time components must be correct.
    assert_eq!(order.created_at_utc.date_naive().year(), 2024);
    assert_eq!(order.created_at_utc.date_naive().month(), 6);
    assert_eq!(order.created_at_utc.date_naive().day(), 1);
}

// ---------------------------------------------------------------------------
// N4 — normalize_open_order error path
// ---------------------------------------------------------------------------

#[test]
fn n4_bad_timestamp_returns_transient_error() {
    use mqk_execution::BrokerError;
    let mut raw = make_order_raw(None, None);
    raw.created_at = "not-a-timestamp".to_string();
    let result = normalize_open_order(&raw);
    assert!(
        matches!(result, Err(BrokerError::Transient { .. })),
        "malformed created_at must return BrokerError::Transient; got: {result:?}"
    );
}

#[test]
fn n4_empty_timestamp_returns_transient_error() {
    use mqk_execution::BrokerError;
    let mut raw = make_order_raw(None, None);
    raw.created_at = "".to_string();
    let result = normalize_open_order(&raw);
    assert!(
        matches!(result, Err(BrokerError::Transient { .. })),
        "empty created_at must return BrokerError::Transient"
    );
}

#[test]
fn n4_non_utc_offset_timestamp_is_accepted() {
    // RFC 3339 allows non-UTC offsets; normalize_open_order converts to UTC.
    let mut raw = make_order_raw(None, None);
    raw.created_at = "2024-03-01T10:00:00-05:00".to_string();
    let order = normalize_open_order(&raw).expect("RFC 3339 with offset must parse");
    // 10:00 ET (-05:00) = 15:00 UTC
    assert_eq!(order.created_at_utc.hour(), 15);
}

// ---------------------------------------------------------------------------
// N5 — build_snapshot assembly
// ---------------------------------------------------------------------------

#[test]
fn n5_snapshot_fills_always_empty() {
    let now = Utc.with_ymd_and_hms(2024, 1, 15, 16, 0, 0).unwrap();
    let account = normalize_account(&AlpacaAccountRaw {
        equity: "1000.00".to_string(),
        cash: "500.00".to_string(),
        currency: "USD".to_string(),
    });
    let snapshot = build_snapshot(now, account, vec![], vec![]);
    assert!(
        snapshot.fills.is_empty(),
        "AP-03: fills must always be empty in a snapshot"
    );
}

#[test]
fn n5_snapshot_captured_at_is_caller_injected() {
    let injected = Utc.with_ymd_and_hms(2024, 6, 15, 12, 34, 56).unwrap();
    let account = normalize_account(&AlpacaAccountRaw {
        equity: "0.00".to_string(),
        cash: "0.00".to_string(),
        currency: "USD".to_string(),
    });
    let snapshot = build_snapshot(injected, account, vec![], vec![]);
    assert_eq!(snapshot.captured_at_utc, injected);
}

#[test]
fn n5_snapshot_positions_and_orders_forwarded() {
    let now = Utc::now();
    let account = normalize_account(&AlpacaAccountRaw {
        equity: "5000.00".to_string(),
        cash: "2000.00".to_string(),
        currency: "USD".to_string(),
    });
    let pos = normalize_position(&AlpacaPositionRaw {
        symbol: "NVDA".to_string(),
        qty: "10".to_string(),
        avg_entry_price: "800.00".to_string(),
    });
    let ord = normalize_open_order(&make_order_raw(Some("150.00"), None)).unwrap();

    let snapshot = build_snapshot(now, account, vec![pos], vec![ord]);
    assert_eq!(snapshot.positions.len(), 1);
    assert_eq!(snapshot.positions[0].symbol, "NVDA");
    assert_eq!(snapshot.orders.len(), 1);
    assert_eq!(snapshot.orders[0].broker_order_id, "broker-uuid-001");
}

// ---------------------------------------------------------------------------
// N6 — determinism
// ---------------------------------------------------------------------------

#[test]
fn n6_same_input_produces_identical_snapshots() {
    let now = Utc.with_ymd_and_hms(2024, 1, 15, 16, 0, 0).unwrap();
    let make = || {
        let account = normalize_account(&AlpacaAccountRaw {
            equity: "12345.67".to_string(),
            cash: "999.00".to_string(),
            currency: "USD".to_string(),
        });
        let pos = normalize_position(&AlpacaPositionRaw {
            symbol: "GOOG".to_string(),
            qty: "5".to_string(),
            avg_entry_price: "140.00".to_string(),
        });
        build_snapshot(now, account, vec![pos], vec![])
    };

    let s1 = make();
    let s2 = make();
    assert_eq!(s1.captured_at_utc, s2.captured_at_utc);
    assert_eq!(s1.account.equity, s2.account.equity);
    assert_eq!(s1.account.cash, s2.account.cash);
    assert_eq!(s1.positions.len(), s2.positions.len());
    assert_eq!(s1.positions[0].symbol, s2.positions[0].symbol);
    assert_eq!(s1.positions[0].avg_price, s2.positions[0].avg_price);
    assert!(s1.fills.is_empty());
    assert!(s2.fills.is_empty());
}
