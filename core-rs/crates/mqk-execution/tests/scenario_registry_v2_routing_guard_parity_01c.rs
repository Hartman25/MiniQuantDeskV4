//! REGISTRY-V2-GATE-PARITY-01C — broker-submit routing-guard parity against
//! `InstrumentRegistryV2::asset_class` strings.
//!
//! Prerequisite #3 of `ASSET-CORE-01H`'s production-cutover checklist
//! requires the broker-submit routing guard
//! (`BrokerGateway::enforce_gates`, `MULTI-ASSET-ROUTING-GUARD-01`) to be
//! re-verified against `InstrumentRegistryV2::asset_class` parity. This
//! file proves that `mqk_md::instrument_registry_v2`'s pure
//! `registry_v2_gate_asset_class` helper (`REGISTRY-V2-GATE-PARITY-01B`)
//! classifies every v2 asset-class string identically to the routing
//! guard's actual, already-tested behavior
//! (`scenario_asset_class_guard_multi_asset_routing_guard_01.rs`, `G01`
//! through `G08`). The routing guard itself is not modified — it still
//! reads only `BrokerSubmitRequest.asset_class: mqk_schemas::AssetClass`,
//! never `InstrumentRegistryV2`.
//!
//! ## The `"rate"` gap
//!
//! `mqk_schemas::AssetClass` has exactly five variants (`Equity`, `Option`,
//! `Future`, `Crypto`, `Forex`) — there is no `Rate` variant, so a
//! `BrokerSubmitRequest` can never be *constructed* with an asset class
//! corresponding to `InstrumentRegistryV2`'s `"rate"` string in the first
//! place. This is itself the strongest possible parity result for `"rate"`:
//! it is unreachable through the routing guard's closed enum, not merely
//! rejected by a runtime check. `RG-06` documents this rather than
//! exercising `BrokerGateway::submit` with a value that cannot exist.
//!
//! ## Coverage
//!
//! RG-01  `"equity"` — helper classifies `Equity`; the routing guard
//!        allows `AssetClass::Equity` end-to-end through `EchoBroker`.
//! RG-02  `"crypto"` — helper classifies `NonEquity`; routing guard
//!        rejects `AssetClass::Crypto` via `AssetClassDisabled` before
//!        `PanicBroker` is ever invoked.
//! RG-03  `"future"` — same shape, `AssetClass::Future`.
//! RG-04  `"option"` — same shape, `AssetClass::Option`.
//! RG-05  `"forex"` — same shape, `AssetClass::Forex`.
//! RG-06  `"rate"` — documented unconstructable-in-the-enum parity (see
//!        above); also proves the helper still classifies it `NonEquity`.
//! RG-07  Exhaustive: every `CANONICAL_ASSET_CLASSES_V2` string with a
//!        `mqk_schemas::AssetClass` counterpart yields identical
//!        allow/reject decisions between the helper and the routing guard.
//! RG-08  Malformed/unknown v2-shaped strings (`""`, `"stock"`, `"etf"`,
//!        `"futures"`, `"options"`) fail closed in the helper; documented
//!        parity note that the routing guard has no string-typed input at
//!        all, so such a value could never reach it as a
//!        `BrokerSubmitRequest.asset_class` in the first place — an even
//!        stronger fail-closed guarantee than the helper's own `Err`.
//!
//! All tests are pure in-memory — no network, no DB, no wall-clock reads.

use mqk_execution::{
    AssetClass, BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerEvent, BrokerGateway,
    BrokerInvokeToken, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, GateRefusal, IntegrityGate, OutboxClaimToken, ReconcileGate, RiskGate,
    Side, SubmitError,
};
use mqk_md::instrument_registry_v2::{
    registry_v2_gate_asset_class, RegistryV2GateAssetClass, CANONICAL_ASSET_CLASSES_V2,
};

// ---------------------------------------------------------------------------
// Test doubles (mirrors scenario_asset_class_guard_multi_asset_routing_guard_01.rs)
// ---------------------------------------------------------------------------

struct PanicBroker;

impl BrokerAdapter for PanicBroker {
    fn submit_order(
        &self,
        _req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        panic!("PanicBroker::submit_order must not be called for disabled asset classes");
    }

    fn cancel_order(
        &self,
        _order_id: &str,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerCancelResponse, BrokerError> {
        panic!("PanicBroker::cancel_order called unexpectedly");
    }

    fn replace_order(
        &self,
        _req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerReplaceResponse, BrokerError> {
        panic!("PanicBroker::replace_order called unexpectedly");
    }

    fn fetch_events(
        &self,
        _cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
        Ok((vec![], None))
    }
}

struct EchoBroker;

impl BrokerAdapter for EchoBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        Ok(BrokerSubmitResponse {
            broker_order_id: format!("b-{}", req.order_id),
            submitted_at: 0,
            status: "ok".to_string(),
        })
    }

    fn cancel_order(
        &self,
        order_id: &str,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerCancelResponse, BrokerError> {
        Ok(BrokerCancelResponse {
            broker_order_id: order_id.to_string(),
            cancelled_at: 0,
            status: "ok".to_string(),
        })
    }

    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerReplaceResponse, BrokerError> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 0,
            status: "ok".to_string(),
        })
    }

    fn fetch_events(
        &self,
        _cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
        Ok((vec![], None))
    }
}

struct AllClear;
impl IntegrityGate for AllClear {
    fn is_armed(&self) -> bool {
        true
    }
}
impl RiskGate for AllClear {
    fn evaluate_gate(&self) -> mqk_execution::risk_decision::RiskDecision {
        mqk_execution::risk_decision::RiskDecision::Allow
    }
}
impl ReconcileGate for AllClear {
    fn is_clean(&self) -> bool {
        true
    }
}

fn claim() -> OutboxClaimToken {
    OutboxClaimToken::for_test(1, "ord-v2-parity-01")
}

fn req_with_class(asset_class: AssetClass) -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: "ord-v2-parity-01".to_string(),
        symbol: "AAPL".to_string(),
        side: Side::Buy,
        quantity: 10,
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
        asset_class,
    }
}

/// Test-only mapping from a v2 `asset_class` string to its
/// `mqk_schemas::AssetClass` counterpart, where one exists. `None` for
/// `"rate"` (no counterpart — see module docs) and for anything not in
/// `CANONICAL_ASSET_CLASSES_V2`.
fn schema_asset_class_for_v2_string(v2_class: &str) -> Option<AssetClass> {
    match v2_class {
        "equity" => Some(AssetClass::Equity),
        "option" => Some(AssetClass::Option),
        "future" => Some(AssetClass::Future),
        "crypto" => Some(AssetClass::Crypto),
        "forex" => Some(AssetClass::Forex),
        _ => None,
    }
}

/// `true` if the routing guard allows `asset_class` end-to-end (reaches
/// `EchoBroker`); `false` if it is rejected with `AssetClassDisabled`
/// before the broker adapter is invoked. Uses `PanicBroker` for the
/// rejected path so any accidental broker invocation still panics the test.
fn routing_guard_allows(asset_class: AssetClass) -> bool {
    if asset_class == AssetClass::Equity {
        let gw = BrokerGateway::for_test(EchoBroker, AllClear, AllClear, AllClear);
        gw.submit(&claim(), req_with_class(asset_class)).is_ok()
    } else {
        let gw = BrokerGateway::for_test(PanicBroker, AllClear, AllClear, AllClear);
        let err = gw
            .submit(&claim(), req_with_class(asset_class))
            .unwrap_err();
        assert!(
            matches!(err, SubmitError::Gate(GateRefusal::AssetClassDisabled { .. })),
            "non-equity {asset_class:?} must be rejected by the gate, not any other error: {err:?}"
        );
        false
    }
}

// ---------------------------------------------------------------------------
// RG-01: equity — helper Equity, routing guard allows end-to-end
// ---------------------------------------------------------------------------

#[test]
fn rg01_equity_parity() {
    assert_eq!(
        registry_v2_gate_asset_class("equity"),
        Ok(RegistryV2GateAssetClass::Equity)
    );
    assert!(routing_guard_allows(AssetClass::Equity));
}

// ---------------------------------------------------------------------------
// RG-02..RG-05: crypto/future/option/forex — helper NonEquity, guard rejects
// ---------------------------------------------------------------------------

#[test]
fn rg02_crypto_parity() {
    assert_eq!(
        registry_v2_gate_asset_class("crypto"),
        Ok(RegistryV2GateAssetClass::NonEquity {
            asset_class: "crypto".to_string()
        })
    );
    assert!(!routing_guard_allows(AssetClass::Crypto));
}

#[test]
fn rg03_future_parity() {
    assert_eq!(
        registry_v2_gate_asset_class("future"),
        Ok(RegistryV2GateAssetClass::NonEquity {
            asset_class: "future".to_string()
        })
    );
    assert!(!routing_guard_allows(AssetClass::Future));
}

#[test]
fn rg04_option_parity() {
    assert_eq!(
        registry_v2_gate_asset_class("option"),
        Ok(RegistryV2GateAssetClass::NonEquity {
            asset_class: "option".to_string()
        })
    );
    assert!(!routing_guard_allows(AssetClass::Option));
}

#[test]
fn rg05_forex_parity() {
    assert_eq!(
        registry_v2_gate_asset_class("forex"),
        Ok(RegistryV2GateAssetClass::NonEquity {
            asset_class: "forex".to_string()
        })
    );
    assert!(!routing_guard_allows(AssetClass::Forex));
}

// ---------------------------------------------------------------------------
// RG-06: "rate" — unconstructable in the routing guard's closed enum
// ---------------------------------------------------------------------------

#[test]
fn rg06_rate_has_no_schema_asset_class_counterpart_and_classifies_non_equity() {
    assert_eq!(
        registry_v2_gate_asset_class("rate"),
        Ok(RegistryV2GateAssetClass::NonEquity {
            asset_class: "rate".to_string()
        })
    );
    assert_eq!(
        schema_asset_class_for_v2_string("rate"),
        None,
        "\"rate\" must have no mqk_schemas::AssetClass counterpart -- a \
         BrokerSubmitRequest can never be constructed with a \"rate\" asset \
         class, which is a stronger guarantee than a runtime rejection"
    );
}

// ---------------------------------------------------------------------------
// RG-07: exhaustive parity for every v2 class with a schema counterpart
// ---------------------------------------------------------------------------

#[test]
fn rg07_exhaustive_parity_for_every_v2_class_with_a_schema_counterpart() {
    let mut checked = 0;
    for v2_class in CANONICAL_ASSET_CLASSES_V2 {
        let Some(schema_class) = schema_asset_class_for_v2_string(v2_class) else {
            continue;
        };
        checked += 1;

        let helper_allows = matches!(
            registry_v2_gate_asset_class(v2_class),
            Ok(RegistryV2GateAssetClass::Equity)
        );
        let guard_allows = routing_guard_allows(schema_class);

        assert_eq!(
            helper_allows, guard_allows,
            "parity mismatch for v2_class={v2_class} (schema={schema_class:?}): \
             helper_allows={helper_allows}, guard_allows={guard_allows}"
        );
    }
    assert_eq!(
        checked, 5,
        "expected exactly 5 canonical v2 classes with a schema counterpart \
         (equity/option/future/crypto/forex); \"rate\" is the sole documented gap"
    );
}

// ---------------------------------------------------------------------------
// RG-08: malformed/unknown v2 strings fail closed (helper); unreachable
// (routing guard has no string input at all)
// ---------------------------------------------------------------------------

#[test]
fn rg08_malformed_and_unknown_v2_strings_fail_closed_in_helper() {
    for ac in ["", "stock", "perpetual_swap", "etf", "futures", "options"] {
        assert!(
            registry_v2_gate_asset_class(ac).is_err(),
            "helper must fail closed for asset_class={ac:?}"
        );
        assert_eq!(
            schema_asset_class_for_v2_string(ac),
            None,
            "malformed/unknown v2 string {ac:?} must have no schema \
             counterpart -- it can never be encoded into a \
             BrokerSubmitRequest.asset_class at all"
        );
    }
}
