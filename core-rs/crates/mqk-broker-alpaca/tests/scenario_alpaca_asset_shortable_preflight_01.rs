//! SHORT-SIDE-SHORTABLE-PREFLIGHT-01 — Alpaca asset wire type JSON parse tests.
//!
//! Proves that:
//! - `AlpacaAssetRaw` deserializes correctly from valid Alpaca asset JSON.
//! - `parse_alpaca_asset_json` returns `None` on malformed or missing-required-field JSON.
//! - `shortable=false` parses correctly (fail-closed asset confirmation).
//! - `easy_to_borrow` is optional and defaults to `None` when absent.
//! - `tradable` is required; absent `tradable` fails the parse (fail-closed).
//!
//! # Test inventory
//!
//! | ID   | Scenario                                                         |
//! |------|------------------------------------------------------------------|
//! | A01  | Full valid asset JSON: shortable=true, easy_to_borrow=true       |
//! | A02  | Asset JSON with shortable=false parses correctly                 |
//! | A03  | easy_to_borrow absent → defaults to None (optional field)        |
//! | A04  | tradable absent → parse fails (fail-closed; required field)      |
//! | A05  | shortable absent → parse fails (fail-closed; required field)     |
//! | A06  | Entirely malformed JSON → parse returns None                     |
//! | A07  | marginable and fractionable present → parsed into Option<bool>   |

use mqk_broker_alpaca::types::parse_alpaca_asset_json;

// ---------------------------------------------------------------------------
// A01 — Full valid asset JSON: shortable=true, easy_to_borrow=true
// ---------------------------------------------------------------------------

#[test]
fn a01_full_valid_asset_shortable_true() {
    let json = r#"{
        "id": "b0b6dd9d-8b9b-48a9-ba46-b9d54906e415",
        "class": "us_equity",
        "exchange": "NASDAQ",
        "symbol": "AAPL",
        "status": "active",
        "tradable": true,
        "marginable": true,
        "shortable": true,
        "easy_to_borrow": true,
        "fractionable": true
    }"#;

    let asset =
        parse_alpaca_asset_json(json).expect("A01: valid asset JSON must parse successfully");

    assert_eq!(asset.symbol, "AAPL", "A01: symbol");
    assert!(asset.tradable, "A01: tradable=true");
    assert!(asset.shortable, "A01: shortable=true");
    assert_eq!(
        asset.easy_to_borrow,
        Some(true),
        "A01: easy_to_borrow=Some(true)"
    );
    assert_eq!(asset.marginable, Some(true), "A01: marginable=Some(true)");
    assert_eq!(
        asset.fractionable,
        Some(true),
        "A01: fractionable=Some(true)"
    );
}

// ---------------------------------------------------------------------------
// A02 — Asset JSON with shortable=false parses correctly
// ---------------------------------------------------------------------------

#[test]
fn a02_asset_shortable_false_parses_correctly() {
    let json = r#"{
        "symbol": "MEME",
        "tradable": true,
        "shortable": false,
        "easy_to_borrow": false
    }"#;

    let asset = parse_alpaca_asset_json(json)
        .expect("A02: shortable=false asset JSON must parse successfully");

    assert_eq!(asset.symbol, "MEME", "A02: symbol");
    assert!(asset.tradable, "A02: tradable=true");
    assert!(!asset.shortable, "A02: shortable=false");
    assert_eq!(
        asset.easy_to_borrow,
        Some(false),
        "A02: easy_to_borrow=Some(false)"
    );
}

// ---------------------------------------------------------------------------
// A03 — easy_to_borrow absent → defaults to None
// ---------------------------------------------------------------------------

#[test]
fn a03_easy_to_borrow_absent_defaults_to_none() {
    let json = r#"{
        "symbol": "SPY",
        "tradable": true,
        "shortable": true
    }"#;

    let asset = parse_alpaca_asset_json(json)
        .expect("A03: JSON without easy_to_borrow must parse successfully");

    assert_eq!(asset.symbol, "SPY", "A03: symbol");
    assert!(asset.tradable, "A03: tradable=true");
    assert!(asset.shortable, "A03: shortable=true");
    assert_eq!(
        asset.easy_to_borrow, None,
        "A03: easy_to_borrow=None when absent"
    );
    assert_eq!(asset.marginable, None, "A03: marginable=None when absent");
    assert_eq!(
        asset.fractionable, None,
        "A03: fractionable=None when absent"
    );
}

// ---------------------------------------------------------------------------
// A04 — tradable absent → parse fails (required field)
// ---------------------------------------------------------------------------

#[test]
fn a04_tradable_absent_parse_fails_closed() {
    // `tradable` is a required bool field; its absence must cause parse failure.
    let json = r#"{
        "symbol": "XYZ",
        "shortable": true
    }"#;

    let result = parse_alpaca_asset_json(json);
    assert!(
        result.is_none(),
        "A04: absent tradable must return None (fail-closed); got: {:?}",
        result
    );
}

// ---------------------------------------------------------------------------
// A05 — shortable absent → parse fails (required field)
// ---------------------------------------------------------------------------

#[test]
fn a05_shortable_absent_parse_fails_closed() {
    // `shortable` is a required bool field; its absence must cause parse failure.
    let json = r#"{
        "symbol": "XYZ",
        "tradable": true
    }"#;

    let result = parse_alpaca_asset_json(json);
    assert!(
        result.is_none(),
        "A05: absent shortable must return None (fail-closed); got: {:?}",
        result
    );
}

// ---------------------------------------------------------------------------
// A06 — Entirely malformed JSON → parse returns None
// ---------------------------------------------------------------------------

#[test]
fn a06_malformed_json_returns_none() {
    let not_json = "this is not json at all";
    let result = parse_alpaca_asset_json(not_json);
    assert!(
        result.is_none(),
        "A06: malformed JSON must return None (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// A07 — marginable and fractionable present → parsed into Option<bool>
// ---------------------------------------------------------------------------

#[test]
fn a07_marginable_and_fractionable_parsed_correctly() {
    let json = r#"{
        "symbol": "QQQ",
        "tradable": true,
        "shortable": true,
        "marginable": false,
        "fractionable": false,
        "easy_to_borrow": true
    }"#;

    let asset =
        parse_alpaca_asset_json(json).expect("A07: valid asset JSON must parse successfully");

    assert_eq!(asset.marginable, Some(false), "A07: marginable=Some(false)");
    assert_eq!(
        asset.fractionable,
        Some(false),
        "A07: fractionable=Some(false)"
    );
    assert_eq!(
        asset.easy_to_borrow,
        Some(true),
        "A07: easy_to_borrow=Some(true)"
    );
}
