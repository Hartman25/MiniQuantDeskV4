//! Outbox payload parsing and submit-request validation.
//!
//! These are pure functions that operate on JSON payloads from the outbox
//! table.  They have no runtime state and no side effects — all DB writes
//! are the caller's responsibility.
//!
//! # Exports
//!
//! - `ClaimedOutboxRequest` — discriminated outbox row intent (submit vs cancel).
//! - `build_claimed_outbox_request` — classify a claimed outbox row.
//! - `build_submit_request` / `build_validated_submit_request` — produce a
//!   `BrokerSubmitRequest` from a raw outbox JSON payload.
//! - `summarize_ambiguous_outbox` — human-readable summary for quarantine errors.

use anyhow::anyhow;
use mqk_execution::{AssetClass, BrokerSubmitRequest, QtyMicros};

// ---------------------------------------------------------------------------
// ClaimedOutboxRequest
// ---------------------------------------------------------------------------

pub(super) enum ClaimedOutboxRequest {
    Submit(BrokerSubmitRequest),
    Cancel { target_order_id: String },
}

pub(super) fn build_claimed_outbox_request(
    row: &mqk_db::OutboxRow,
) -> anyhow::Result<ClaimedOutboxRequest> {
    let request_type = row
        .order_json
        .get("request_type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);

    match request_type.as_deref() {
        None | Some("submit") => Ok(ClaimedOutboxRequest::Submit(build_submit_request(row)?)),
        Some("cancel") => {
            let target_order_id = row
                .order_json
                .get("target_order_id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .ok_or_else(|| {
                    anyhow!("invalid cancel payload: target_order_id missing or not a string")
                })?;
            if target_order_id.is_empty() {
                return Err(anyhow!("invalid cancel payload: target_order_id blank"));
            }
            Ok(ClaimedOutboxRequest::Cancel {
                target_order_id: target_order_id.to_string(),
            })
        }
        Some(other) => Err(anyhow!(
            "invalid outbox payload: unsupported request_type '{}'",
            other
        )),
    }
}

// ---------------------------------------------------------------------------
// Submit-request construction
// ---------------------------------------------------------------------------

/// Build a `BrokerSubmitRequest` from a claimed outbox row.
pub(super) fn build_validated_submit_request(
    order_id: &str,
    order_json: &serde_json::Value,
) -> anyhow::Result<BrokerSubmitRequest> {
    mqk_db::validate_order_json_schema_version(order_json)?;
    let symbol = validated_order_symbol(order_json)?;
    let quantity = validated_order_quantity(order_json)?;
    let side = validated_order_side(order_json, quantity.signed_qty, quantity.quantity)?;
    let order_type = validated_order_type(order_json)?;
    let time_in_force = validated_order_time_in_force(order_json)?;
    let limit_price = validated_limit_price_for_order_type(order_json, &order_type)?;

    let asset_class = validated_asset_class(order_json)?;

    // Equity whole-share invariant (QTY-MICROS-PRODUCTION-CUTOVER-01):
    // fractional quantity is only meaningful for asset classes that support
    // it. Preserved explicitly here rather than left implicit, since the
    // parser above no longer rejects a fractional decimal string outright.
    if asset_class == AssetClass::Equity && !quantity.quantity.is_whole() {
        return Err(anyhow!(
            "invalid submit payload: Equity quantity {} must be a whole share count",
            quantity.quantity
        ));
    }

    // BRK-PRICE-01B / M6:
    // Crypto cannot reach BrokerSubmitRequest construction unless the
    // durable outbox row carries the instrument-economics snapshot that
    // authorized the economic intent.
    if asset_class == AssetClass::Crypto {
        validate_crypto_order_economics(
            order_json,
            &symbol,
            quantity.quantity,
            limit_price,
        )?;
    }

    Ok(BrokerSubmitRequest {
        order_id: order_id.to_string(),
        symbol,
        side,
        quantity: quantity.quantity,
        order_type,
        limit_price,
        time_in_force,
        asset_class,
    })
}

pub(super) fn build_submit_request(row: &mqk_db::OutboxRow) -> anyhow::Result<BrokerSubmitRequest> {
    build_validated_submit_request(&row.idempotency_key, &row.order_json)
}

// ---------------------------------------------------------------------------
// BRK-PRICE-01B / M6 durable Crypto order-economics authority
// ---------------------------------------------------------------------------

/// Validate one Crypto order against the instrument-economics snapshot
/// persisted in its outbox envelope.
///
/// Every Crypto constraint is explicit. Missing or partial economics fail
/// closed. Equity retains its existing path and does not require this
/// additive envelope.
fn validate_crypto_order_economics(
    order_json: &serde_json::Value,
    order_symbol: &str,
    quantity: QtyMicros,
    limit_price_micros: Option<i64>,
) -> anyhow::Result<()> {
    let economics_json = order_json
        .get("instrument_economics")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            anyhow!(
                "invalid crypto submit payload: instrument_economics \
                 missing or not an object"
            )
        })?;

    let required_string = |name: &str| -> anyhow::Result<String> {
        economics_json
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow!(
                    "invalid crypto submit payload: \
                     instrument_economics.{} missing or blank",
                    name
                )
            })
    };

    let required_positive_i64 = |name: &str| -> anyhow::Result<i64> {
        let value = economics_json
            .get(name)
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                anyhow!(
                    "invalid crypto submit payload: \
                     instrument_economics.{} missing or not an integer",
                    name
                )
            })?;

        if value <= 0 {
            return Err(anyhow!(
                "invalid crypto submit payload: \
                 instrument_economics.{} must be positive",
                name
            ));
        }

        Ok(value)
    };

    let source = required_string("source")?;

    if source != "registry_v2" {
        return Err(anyhow!(
            "invalid crypto submit payload: instrument_economics.source \
             must be 'registry_v2', got '{}'",
            source
        ));
    }

    let instrument_id = required_string("instrument_id")?;
    let economics_symbol = required_string("symbol")?;

    if economics_symbol != order_symbol {
        return Err(anyhow!(
            "invalid crypto submit payload: instrument_economics.symbol \
             '{}' does not match order symbol '{}'",
            economics_symbol,
            order_symbol
        ));
    }

    let economics_asset_class =
        required_string("asset_class")?;

    if economics_asset_class != "crypto" {
        return Err(anyhow!(
            "invalid crypto submit payload: \
             instrument_economics.asset_class must be 'crypto', got '{}'",
            economics_asset_class
        ));
    }

    let quote_currency =
        required_string("quote_currency")?;

    let contract_multiplier_micros =
        required_positive_i64("contract_multiplier_micros")?;

    let quantity_scale =
        required_positive_i64("quantity_scale")?;

    let min_trade_qty_micros =
        required_positive_i64("min_trade_qty_micros")?;

    let tick_size_micros =
        required_positive_i64("tick_size_micros")?;

    let quantity_increment_micros =
        required_positive_i64("quantity_increment_micros")?;

    let economics = mqk_portfolio::InstrumentEconomics {
        instrument_id,
        symbol: economics_symbol,
        asset_class: economics_asset_class,
        quote_currency,
        contract_multiplier_micros,
        quantity_scale,
        min_trade_qty_micros: Some(min_trade_qty_micros),
        tick_size_micros: Some(tick_size_micros),
        quantity_increment_micros: Some(
            quantity_increment_micros,
        ),
    };

    mqk_portfolio::validate_order_against_economics(
        &economics,
        quantity.raw(),
        limit_price_micros,
    )
    .map_err(|violation| {
        anyhow!(
            "invalid crypto submit payload: registry-v2 instrument \
             economics rejected order: {:?}",
            violation
        )
    })
}

// ---------------------------------------------------------------------------
// Field validators
// ---------------------------------------------------------------------------

fn validated_order_symbol(order_json: &serde_json::Value) -> anyhow::Result<String> {
    let symbol = order_json
        .get("symbol")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .ok_or_else(|| anyhow!("invalid submit payload: symbol missing or not a string"))?;

    if symbol.is_empty() {
        return Err(anyhow!("invalid submit payload: symbol blank"));
    }

    Ok(symbol.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ValidatedOrderQuantity {
    /// Signed whole-unit share count, used only to derive side when the
    /// payload omits an explicit `side` field. Equity-only legacy shape:
    /// a fractional/crypto payload MUST carry an explicit `side` (enforced
    /// in `validated_order_side`), so `signed_qty` is never consulted for it.
    pub(super) signed_qty: i64,
    /// Fractional-capable (QTY-MICROS-PRODUCTION-CUTOVER-01): always positive.
    pub(super) quantity: QtyMicros,
}

fn validated_order_side(
    order_json: &serde_json::Value,
    signed_qty: i64,
    effective_qty: QtyMicros,
) -> anyhow::Result<mqk_execution::Side> {
    // Compatibility rule restored from pre-EXE-01R submit building:
    // explicit side is authoritative; if absent, derive direction from the
    // legacy signed-quantity encoding already evidenced by local code/tests.
    // QTY-MICROS-PRODUCTION-CUTOVER-01: that legacy inference only ever
    // covered whole-unit (equity) payloads -- a fractional quantity has no
    // whole-unit sign to report (`signed_qty` is always 0 for one) and must
    // therefore carry an explicit `side`, fail-closed rather than silently
    // defaulting to Sell.
    let Some(side_value) = order_json.get("side") else {
        if !effective_qty.is_whole() {
            return Err(anyhow!(
                "invalid submit payload: side is required when quantity is fractional"
            ));
        }
        return if signed_qty > 0 {
            Ok(mqk_execution::Side::Buy)
        } else {
            Ok(mqk_execution::Side::Sell)
        };
    };

    let side = side_value
        .as_str()
        .map(str::trim)
        .ok_or_else(|| anyhow!("invalid submit payload: side present but not a string"))?
        .to_ascii_lowercase();

    match side.as_str() {
        "buy" => Ok(mqk_execution::Side::Buy),
        "sell" => Ok(mqk_execution::Side::Sell),
        _ => Err(anyhow!(
            "invalid submit payload: unsupported side '{}'",
            side
        )),
    }
}

fn validated_order_quantity(
    order_json: &serde_json::Value,
) -> anyhow::Result<ValidatedOrderQuantity> {
    let signed = match (order_json.get("qty"), order_json.get("quantity")) {
        (Some(qty), Some(quantity)) => {
            let qty = parse_signed_qty_micros_field("qty", qty)?;
            let quantity = parse_signed_qty_micros_field("quantity", quantity)?;
            if qty != quantity {
                return Err(anyhow!(
                    "invalid submit payload: qty and quantity disagree (qty={}, quantity={})",
                    qty,
                    quantity
                ));
            }
            qty
        }
        (Some(qty), None) => parse_signed_qty_micros_field("qty", qty)?,
        (None, Some(quantity)) => parse_signed_qty_micros_field("quantity", quantity)?,
        (None, None) => return Err(anyhow!("invalid submit payload: quantity missing")),
    };

    // Legacy whole-unit sign, preserved only to let `validated_order_side`
    // infer direction when the payload omits an explicit `side` field
    // (equity backward-compat shape). A fractional value has no whole-unit
    // sign to report here -- 0 -- which forces `validated_order_side` to
    // require an explicit `side` field instead of silently guessing.
    let signed_qty = signed.to_whole_units_checked().unwrap_or(0);

    let effective_qty = signed.checked_abs().ok_or_else(|| {
        anyhow!("invalid submit payload: quantity out of range for broker request")
    })?;
    if effective_qty.raw() > (i32::MAX as i64) * mqk_execution::QTY_MICROS_SCALE {
        return Err(anyhow!(
            "invalid submit payload: quantity out of range for broker request"
        ));
    }
    if effective_qty.is_zero() {
        return Err(anyhow!(
            "invalid submit payload: effective quantity must be positive"
        ));
    }

    Ok(ValidatedOrderQuantity {
        signed_qty,
        quantity: effective_qty,
    })
}

fn validated_order_type(order_json: &serde_json::Value) -> anyhow::Result<String> {
    // Compatibility rule restored from pre-EXE-01R submit building:
    // absent order_type defaults to market, but explicit values are validated.
    let order_type = match order_json.get("order_type") {
        None => return Ok("market".to_string()),
        Some(value) => value
            .as_str()
            .map(str::trim)
            .ok_or_else(|| anyhow!("invalid submit payload: order_type present but not a string"))?
            .to_ascii_lowercase(),
    };

    match order_type.as_str() {
        "market" | "limit" => Ok(order_type),
        _ => Err(anyhow!(
            "invalid submit payload: unsupported order_type '{}'",
            order_type
        )),
    }
}

fn validated_order_time_in_force(order_json: &serde_json::Value) -> anyhow::Result<String> {
    // Compatibility rule restored from pre-EXE-01R submit building:
    // absent time_in_force defaults to day, but explicit values are validated.
    let time_in_force = match order_json.get("time_in_force") {
        None => return Ok("day".to_string()),
        Some(value) => value
            .as_str()
            .map(str::trim)
            .ok_or_else(|| {
                anyhow!("invalid submit payload: time_in_force present but not a string")
            })?
            .to_ascii_lowercase(),
    };

    match time_in_force.as_str() {
        "day" | "gtc" | "ioc" | "fok" | "opg" | "cls" => Ok(time_in_force),
        _ => Err(anyhow!(
            "invalid submit payload: unsupported time_in_force '{}'",
            time_in_force
        )),
    }
}

fn validated_limit_price_for_order_type(
    order_json: &serde_json::Value,
    order_type: &str,
) -> anyhow::Result<Option<i64>> {
    let limit_price = order_json.get("limit_price");

    match order_type {
        "limit" => {
            let limit_price = limit_price.ok_or_else(|| {
                anyhow!("invalid submit payload: limit order missing limit_price")
            })?;
            if limit_price.is_null() {
                return Err(anyhow!(
                    "invalid submit payload: limit order missing limit_price"
                ));
            }
            Ok(Some(parse_positive_i64_field("limit_price", limit_price)?))
        }
        "market" => {
            if limit_price.is_some_and(|value| !value.is_null()) {
                return Err(anyhow!(
                    "invalid submit payload: market order must not carry limit_price"
                ));
            }
            Ok(None)
        }
        _ => Err(anyhow!(
            "invalid submit payload: unsupported order_type '{}'",
            order_type
        )),
    }
}

// ---------------------------------------------------------------------------
// Numeric field parsers
// ---------------------------------------------------------------------------

/// Fractional-capable quantity parser (QTY-MICROS-PRODUCTION-CUTOVER-01).
///
/// A JSON integer is treated as whole units (equity backward-compat: `10`
/// means 10 shares). A JSON string is parsed as a canonical decimal via
/// `QtyMicros::from_str`, accepting up to 6 fraction digits (e.g. `"0.5"`
/// for half a BTC). A bare JSON float (has a fractional component but is
/// still `serde_json::Value::Number`) is rejected outright rather than
/// converted -- f64 cannot represent an arbitrary decimal exactly, and
/// silently rounding a quantity would violate the no-unchecked-narrowing
/// invariant for money-moving fields.
fn parse_signed_qty_micros_field(name: &str, value: &serde_json::Value) -> anyhow::Result<QtyMicros> {
    match value {
        serde_json::Value::Number(number) => {
            let whole = number.as_i64().ok_or_else(|| {
                anyhow!(
                    "invalid submit payload: {} must be a whole-unit integer, or a decimal \
                     string for a fractional quantity -- a floating-point JSON number is \
                     rejected to avoid silent precision loss",
                    name
                )
            })?;
            QtyMicros::from_whole_units(whole).ok_or_else(|| {
                anyhow!(
                    "invalid submit payload: {} out of range for broker request",
                    name
                )
            })
        }
        serde_json::Value::String(raw) => {
            raw.trim().parse::<QtyMicros>().map_err(|_| {
                anyhow!(
                    "invalid submit payload: {} must be an integer or a valid decimal-string quantity",
                    name
                )
            })
        }
        _ => Err(anyhow!(
            "invalid submit payload: {} missing or not an integer/decimal-string value",
            name
        )),
    }
}

fn parse_positive_i64_field(name: &str, value: &serde_json::Value) -> anyhow::Result<i64> {
    let parsed = match value {
        serde_json::Value::Number(number) => number.as_i64().ok_or_else(|| {
            anyhow!(
                "invalid submit payload: {} must be an integer without lossy conversion",
                name
            )
        })?,
        serde_json::Value::String(raw) => raw.trim().parse::<i64>().map_err(|_| {
            anyhow!(
                "invalid submit payload: {} must be an integer without lossy conversion",
                name
            )
        })?,
        _ => {
            return Err(anyhow!(
                "invalid submit payload: {} missing or not an integer-compatible value",
                name
            ))
        }
    };

    if parsed <= 0 {
        return Err(anyhow!("invalid submit payload: {} must be positive", name));
    }

    Ok(parsed)
}

fn validated_asset_class(order_json: &serde_json::Value) -> anyhow::Result<AssetClass> {
    // Absent asset_class: default to Equity for backward compatibility with
    // pre-DISABLED-ASSET-GATE-TESTS-01 payloads that predate the field.
    let Some(value) = order_json.get("asset_class") else {
        return Ok(AssetClass::Equity);
    };

    let cls = value
        .as_str()
        .map(str::trim)
        .ok_or_else(|| anyhow!("invalid submit payload: asset_class present but not a string"))?
        .to_ascii_lowercase();

    match cls.as_str() {
        "equity" => Ok(AssetClass::Equity),
        // M5-BROKER-ASSET-CAPABILITY-AUTHORITY-01 / M6: Crypto is accepted at
        // the JSON-shape layer, but this does NOT grant execution capability
        // by itself -- `BrokerGateway::submit_with_context` still consults
        // `BrokerAdapter::supports_asset_class` before any gate or adapter
        // call and refuses fail-closed (`GateRefusal::AssetClassDisabled`)
        // for any configured broker that has not explicitly opted in
        // (the Paper broker has not; `DaemonBroker::Alpaca` has). Other
        // non-equity classes remain hard-disabled here: no adapter in this
        // repository declares support for them yet, so surfacing that
        // refusal earlier (at parse time, not gateway time) avoids
        // needlessly claiming, dispatching, and marking outbox rows for
        // asset classes that cannot possibly execute.
        "crypto" => Ok(AssetClass::Crypto),
        // Non-equity asset classes are disabled. Explicit payload values must not
        // be silently converted to equity — reject so the caller quarantines the row.
        "future" | "futures" | "option" | "options" | "forex" => Err(anyhow!(
            "invalid submit payload: asset_class '{}' is not enabled for execution",
            cls
        )),
        _ => Err(anyhow!(
            "invalid submit payload: unknown asset_class '{}'",
            cls
        )),
    }
}

// ---------------------------------------------------------------------------
// Ambiguous outbox summary
// ---------------------------------------------------------------------------

pub(super) fn summarize_ambiguous_outbox(rows: &[mqk_db::AmbiguousOutboxRow]) -> String {
    rows.iter()
        .map(|r| match &r.broker_order_id {
            Some(bid) => format!("{}:{}:broker={}", r.idempotency_key, r.status, bid),
            None => format!("{}:{}", r.idempotency_key, r.status),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// DISABLED-ASSET-GATE-TESTS-01 — outbox payload asset_class guard
// ---------------------------------------------------------------------------
//
// Proves that the outbox JSON parser rejects disabled asset classes before
// a BrokerSubmitRequest is constructed, so no disabled class can reach
// BrokerGateway::submit.
//
// O01  absent asset_class field → defaults to Equity (backward compat)
// O02  explicit "equity" → Equity
// O03  explicit "crypto" → Crypto (M5-BROKER-ASSET-CAPABILITY-AUTHORITY-01:
//      JSON-shape acceptance only; BrokerGateway still gates execution
//      capability per configured broker adapter)
// O04  explicit "future" → parser Err
// O05  explicit "futures" (alias) → parser Err
// O06  explicit "option" → parser Err
// O07  explicit "options" (alias) → parser Err
// O08  explicit "forex" → parser Err
// O09  unknown string → parser Err
// O10  asset_class present but not a string → parser Err
// O11  case-insensitive: "CRYPTO" → Crypto (same gate as O03)
// O12  production path: absent asset_class in a well-formed equity payload
//      produces a valid BrokerSubmitRequest with asset_class=Equity
#[cfg(test)]
mod disabled_asset_gate_tests {
    use super::*;
    use serde_json::json;

    fn crypto_economics_fixture() -> serde_json::Value {
        json!({
            "source": "registry_v2",
            "instrument_id": "crypto:GLOBAL:BTCUSD",
            "symbol": "BTC/USD",
            "asset_class": "crypto",
            "quote_currency": "USD",
            "contract_multiplier_micros": 1_000_000,
            "quantity_scale": 1,
            "min_trade_qty_micros": 100,
            "tick_size_micros": 1_000_000,
            "quantity_increment_micros": 100
        })
    }

    fn equity_payload() -> serde_json::Value {
        json!({
            "symbol": "AAPL",
            "qty": 10,
            "side": "buy",
            "order_type": "market",
            "time_in_force": "day"
        })
    }

    fn with_asset_class(cls: serde_json::Value) -> serde_json::Value {
        let mut v = equity_payload();
        v["asset_class"] = cls;
        v
    }

    // O01 — absent asset_class defaults to Equity
    #[test]
    fn o01_absent_asset_class_defaults_to_equity() {
        let result = validated_asset_class(&equity_payload());
        assert!(matches!(result, Ok(AssetClass::Equity)));
    }

    // O02 — explicit "equity" is accepted
    #[test]
    fn o02_explicit_equity_passes() {
        let result = validated_asset_class(&with_asset_class(json!("equity")));
        assert!(matches!(result, Ok(AssetClass::Equity)));
    }

    // O03 — "crypto" is accepted at the JSON-shape layer
    // (M5-BROKER-ASSET-CAPABILITY-AUTHORITY-01 / M6).
    #[test]
    fn o03_crypto_accepted() {
        let result = validated_asset_class(&with_asset_class(json!("crypto")));
        assert!(matches!(result, Ok(AssetClass::Crypto)), "{result:?}");
    }

    // O04 — "future" rejects
    #[test]
    fn o04_future_rejects() {
        let err = validated_asset_class(&with_asset_class(json!("future"))).unwrap_err();
        assert!(err.to_string().contains("not enabled"), "{err}");
    }

    // O05 — "futures" (alias) rejects
    #[test]
    fn o05_futures_alias_rejects() {
        let err = validated_asset_class(&with_asset_class(json!("futures"))).unwrap_err();
        assert!(err.to_string().contains("not enabled"), "{err}");
    }

    // O06 — "option" rejects
    #[test]
    fn o06_option_rejects() {
        let err = validated_asset_class(&with_asset_class(json!("option"))).unwrap_err();
        assert!(err.to_string().contains("not enabled"), "{err}");
    }

    // O07 — "options" (alias) rejects
    #[test]
    fn o07_options_alias_rejects() {
        let err = validated_asset_class(&with_asset_class(json!("options"))).unwrap_err();
        assert!(err.to_string().contains("not enabled"), "{err}");
    }

    // O08 — "forex" rejects
    #[test]
    fn o08_forex_rejects() {
        let err = validated_asset_class(&with_asset_class(json!("forex"))).unwrap_err();
        assert!(err.to_string().contains("not enabled"), "{err}");
    }

    // O09 — unknown string rejects
    #[test]
    fn o09_unknown_string_rejects() {
        let err = validated_asset_class(&with_asset_class(json!("bond"))).unwrap_err();
        assert!(err.to_string().contains("unknown asset_class"), "{err}");
    }

    // O10 — non-string value rejects
    #[test]
    fn o10_non_string_value_rejects() {
        let err = validated_asset_class(&with_asset_class(json!(42))).unwrap_err();
        assert!(err.to_string().contains("not a string"), "{err}");
    }

    // O11 — case-insensitive: "CRYPTO" is caught by the same gate as O03
    #[test]
    fn o11_uppercase_crypto_accepted() {
        let result = validated_asset_class(&with_asset_class(json!("CRYPTO")));
        assert!(matches!(result, Ok(AssetClass::Crypto)), "{result:?}");
    }

    // O12 — full build_validated_submit_request: absent asset_class → Equity
    #[test]
    fn o12_full_parser_absent_asset_class_produces_equity_submit_request() {
        let req = build_validated_submit_request("test-order-id", &equity_payload())
            .expect("valid equity payload must parse successfully");
        assert_eq!(req.asset_class, AssetClass::Equity);
        assert_eq!(req.symbol, "AAPL");
        assert_eq!(req.quantity, QtyMicros::from_whole_units(10).unwrap());
    }

    // O13 — fractional decimal-string qty parses for Crypto and rejects
    // implicit side inference (fail-closed: fractional payloads must carry
    // an explicit side).
    #[test]
    fn o13_fractional_qty_requires_explicit_side() {
        let mut payload = with_asset_class(json!("crypto"));
        payload["qty"] = json!("0.5");
        payload.as_object_mut().unwrap().remove("side");
        let err = build_validated_submit_request("test-order-id", &payload).unwrap_err();
        assert!(err.to_string().contains("side is required"), "{err}");
    }

    // O14 — fractional decimal-string qty with explicit side parses to the
    // exact QtyMicros value (no precision loss).
    #[test]
    fn o14_fractional_qty_with_explicit_side_parses_exactly() {
        let mut payload = with_asset_class(json!("crypto"));
        payload["symbol"] = json!("BTC/USD");
        payload["qty"] = json!("0.5");
        payload["instrument_economics"] = crypto_economics_fixture();

        let req = build_validated_submit_request(
            "test-order-id",
            &payload,
        )
        .expect("valid fractional crypto payload must parse successfully");

        assert_eq!(req.quantity, QtyMicros::new(500_000));
        assert_eq!(req.asset_class, AssetClass::Crypto);
        assert_eq!(req.symbol, "BTC/USD");
    }

    // O15 — a bare JSON float is rejected outright (precision-loss guard),
    // even though it would otherwise represent the same value as O14.
    #[test]
    fn o15_float_json_number_qty_rejected() {
        let mut payload = with_asset_class(json!("crypto"));
        payload["symbol"] = json!("BTCUSD");
        payload["qty"] = json!(0.5);
        let err = build_validated_submit_request("test-order-id", &payload).unwrap_err();
        assert!(err.to_string().contains("floating-point"), "{err}");
    }
}


#[cfg(test)]
mod crypto_order_economics_tests {
    use super::*;
    use serde_json::json;

    fn economics() -> serde_json::Value {
        json!({
            "source": "registry_v2",
            "instrument_id": "crypto:GLOBAL:BTCUSD",
            "symbol": "BTC/USD",
            "asset_class": "crypto",
            "quote_currency": "USD",
            "contract_multiplier_micros": 1_000_000,
            "quantity_scale": 1,
            "min_trade_qty_micros": 100,
            "tick_size_micros": 1_000_000,
            "quantity_increment_micros": 100
        })
    }

    fn crypto_payload(
        qty: &str,
        order_type: &str,
        limit_price: Option<i64>,
    ) -> serde_json::Value {
        let mut payload = json!({
            "symbol": "BTC/USD",
            "side": "buy",
            "qty": qty,
            "order_type": order_type,
            "time_in_force": "gtc",
            "asset_class": "crypto",
            "instrument_economics": economics()
        });

        if let Some(price) = limit_price {
            payload["limit_price"] = json!(price);
        }

        payload
    }

    #[test]
    fn crypto_order_economics_missing_snapshot_fails_closed() {
        let mut payload =
            crypto_payload("0.0001", "market", None);

        payload
            .as_object_mut()
            .unwrap()
            .remove("instrument_economics");

        let err = build_validated_submit_request(
            "crypto-1",
            &payload,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("instrument_economics"),
            "{err}"
        );
    }

    #[test]
    fn crypto_order_economics_below_minimum_fails_closed() {
        let payload =
            crypto_payload("0.00001", "market", None);

        let err = build_validated_submit_request(
            "crypto-2",
            &payload,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("BelowMinimumQuantity"),
            "{err}"
        );
    }

    #[test]
    fn crypto_order_economics_off_increment_fails_closed() {
        let payload =
            crypto_payload("0.00015", "market", None);

        let err = build_validated_submit_request(
            "crypto-3",
            &payload,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("QuantityNotOnIncrement"),
            "{err}"
        );
    }

    #[test]
    fn crypto_order_economics_off_tick_limit_fails_closed() {
        let payload =
            crypto_payload(
                "0.0001",
                "limit",
                Some(60_000_500_000),
            );

        let err = build_validated_submit_request(
            "crypto-4",
            &payload,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("PriceNotOnTick"),
            "{err}"
        );
    }

    #[test]
    fn crypto_order_economics_exact_minimum_and_tick_pass() {
        let payload =
            crypto_payload(
                "0.0001",
                "limit",
                Some(60_000_000_000),
            );

        let req = build_validated_submit_request(
            "crypto-5",
            &payload,
        )
        .expect(
            "complete on-tick BTC/USD order must validate"
        );

        assert_eq!(req.asset_class, AssetClass::Crypto);
        assert_eq!(req.symbol, "BTC/USD");
        assert_eq!(req.quantity, QtyMicros::new(100));
        assert_eq!(
            req.limit_price,
            Some(60_000_000_000)
        );
    }

    #[test]
    fn crypto_order_economics_symbol_mismatch_fails_closed() {
        let mut payload =
            crypto_payload("0.0001", "market", None);

        payload["instrument_economics"]["symbol"] =
            json!("ETH/USD");

        let err = build_validated_submit_request(
            "crypto-6",
            &payload,
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("does not match order symbol"),
            "{err}"
        );
    }

    #[test]
    fn crypto_order_economics_non_registry_source_fails_closed() {
        let mut payload =
            crypto_payload("0.0001", "market", None);

        payload["instrument_economics"]["source"] =
            json!("hand_written");

        let err = build_validated_submit_request(
            "crypto-7",
            &payload,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("must be 'registry_v2'"),
            "{err}"
        );
    }

    #[test]
    fn crypto_order_economics_partial_snapshot_fails_closed() {
        let mut payload =
            crypto_payload("0.0001", "market", None);

        payload["instrument_economics"]
            .as_object_mut()
            .unwrap()
            .remove("tick_size_micros");

        let err = build_validated_submit_request(
            "crypto-8",
            &payload,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("tick_size_micros"),
            "{err}"
        );
    }
}
