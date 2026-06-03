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
use mqk_execution::{AssetClass, BrokerSubmitRequest};

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
    let symbol = validated_order_symbol(order_json)?;
    let quantity = validated_order_quantity(order_json)?;
    let side = validated_order_side(order_json, quantity.signed_qty)?;
    let order_type = validated_order_type(order_json)?;
    let time_in_force = validated_order_time_in_force(order_json)?;
    let limit_price = validated_limit_price_for_order_type(order_json, &order_type)?;

    let asset_class = validated_asset_class(order_json)?;

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
    pub(super) signed_qty: i64,
    pub(super) quantity: i64,
}

fn validated_order_side(
    order_json: &serde_json::Value,
    signed_qty: i64,
) -> anyhow::Result<mqk_execution::Side> {
    // Compatibility rule restored from pre-EXE-01R submit building:
    // explicit side is authoritative; if absent, derive direction from the
    // legacy signed-quantity encoding already evidenced by local code/tests.
    let Some(side_value) = order_json.get("side") else {
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
    let signed_qty = match (order_json.get("qty"), order_json.get("quantity")) {
        (Some(qty), Some(quantity)) => {
            let qty = parse_signed_i64_field("qty", qty)?;
            let quantity = parse_signed_i64_field("quantity", quantity)?;
            if qty != quantity {
                return Err(anyhow!(
                    "invalid submit payload: qty and quantity disagree (qty={}, quantity={})",
                    qty,
                    quantity
                ));
            }
            qty
        }
        (Some(qty), None) => parse_signed_i64_field("qty", qty)?,
        (None, Some(quantity)) => parse_signed_i64_field("quantity", quantity)?,
        (None, None) => return Err(anyhow!("invalid submit payload: quantity missing")),
    };

    let effective_qty = signed_qty.checked_abs().ok_or_else(|| {
        anyhow!("invalid submit payload: quantity out of range for broker request")
    })?;
    if effective_qty > i32::MAX as i64 {
        return Err(anyhow!(
            "invalid submit payload: quantity out of range for broker request"
        ));
    }
    if effective_qty == 0 {
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

fn parse_signed_i64_field(name: &str, value: &serde_json::Value) -> anyhow::Result<i64> {
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

    Ok(parsed)
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
        // Non-equity asset classes are disabled. Explicit payload values must not
        // be silently converted to equity — reject so the caller quarantines the row.
        "crypto" | "future" | "futures" | "option" | "options" | "forex" => Err(anyhow!(
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
// O03  explicit "crypto" → parser Err, not panic
// O04  explicit "future" → parser Err
// O05  explicit "futures" (alias) → parser Err
// O06  explicit "option" → parser Err
// O07  explicit "options" (alias) → parser Err
// O08  explicit "forex" → parser Err
// O09  unknown string → parser Err
// O10  asset_class present but not a string → parser Err
// O11  case-insensitive: "CRYPTO" → parser Err (same gate)
// O12  production path: absent asset_class in a well-formed equity payload
//      produces a valid BrokerSubmitRequest with asset_class=Equity
#[cfg(test)]
mod disabled_asset_gate_tests {
    use super::*;
    use serde_json::json;

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

    // O03 — "crypto" rejects
    #[test]
    fn o03_crypto_rejects() {
        let err = validated_asset_class(&with_asset_class(json!("crypto"))).unwrap_err();
        assert!(err.to_string().contains("not enabled"), "{err}");
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

    // O11 — case-insensitive: "CRYPTO" is caught by the same gate
    #[test]
    fn o11_uppercase_crypto_rejects() {
        let err = validated_asset_class(&with_asset_class(json!("CRYPTO"))).unwrap_err();
        assert!(err.to_string().contains("not enabled"), "{err}");
    }

    // O12 — full build_validated_submit_request: absent asset_class → Equity
    #[test]
    fn o12_full_parser_absent_asset_class_produces_equity_submit_request() {
        let req = build_validated_submit_request("test-order-id", &equity_payload())
            .expect("valid equity payload must parse successfully");
        assert_eq!(req.asset_class, AssetClass::Equity);
        assert_eq!(req.symbol, "AAPL");
        assert_eq!(req.quantity, 10);
    }
}
