//! M5-BROKER-ASSET-CAPABILITY-AUTHORITY-01 — explicit broker×asset-class
//! capability authority proof tests.
//!
//! Replaces `MULTI-ASSET-ROUTING-GUARD-01`'s single hardcoded
//! `AssetClass::Equity`-only check at `BrokerGateway::submit_with_context`
//! with a per-adapter capability declaration
//! (`BrokerAdapter::supports_asset_class`). The pre-existing equity-only
//! behavior for any adapter that does not override the trait default is
//! covered by `scenario_asset_class_guard_multi_asset_routing_guard_01.rs`
//! and is unchanged; this file proves the *new* capability, negative
//! controls for it, and that a genuinely unrecognized broker/asset-class
//! combination still fails closed.
//!
//! # Coverage
//!
//! C01  A broker that declares `Equity + Crypto` support routes a Crypto
//!      order end-to-end through the gateway (reaches the broker adapter).
//! C02  The same broker still refuses `Future`/`Option`/`Forex` — declaring
//!      one non-equity class does not implicitly widen to all of them.
//! C03  A broker declaring ONLY `Crypto` (not `Equity`) refuses an Equity
//!      order — the capability set is exact, not equity-plus-extra.
//! C04  A broker that declares NO asset classes at all (empty capability)
//!      refuses every class including Equity — proves there is no implicit
//!      fallback to Equity once an adapter opts into overriding the method.
//! C05  `GateRefusal::AssetClassDisabled` still carries the exact rejected
//!      asset class, unchanged wire/observability contract.

use mqk_execution::{
    AssetClass, BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerEvent, BrokerGateway,
    BrokerInvokeToken, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, GateRefusal, IntegrityGate, OutboxClaimToken, ReconcileGate, RiskGate,
    Side, SubmitError,
};

// ---------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------

/// Broker adapter whose declared capability set is configurable per-instance,
/// so one test double can prove both the positive (declared class routes
/// through) and negative (undeclared class still refused) cases.
struct CapabilityBroker {
    supported: &'static [AssetClass],
}

impl BrokerAdapter for CapabilityBroker {
    fn supports_asset_class(&self, asset_class: AssetClass) -> bool {
        self.supported.contains(&asset_class)
    }

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

/// Panics if invoked — proves a refused order never reaches the adapter.
struct PanicBroker {
    supported: &'static [AssetClass],
}

impl BrokerAdapter for PanicBroker {
    fn supports_asset_class(&self, asset_class: AssetClass) -> bool {
        self.supported.contains(&asset_class)
    }

    fn submit_order(
        &self,
        _req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        panic!("PanicBroker::submit_order must not be called for a disabled asset class");
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
    OutboxClaimToken::for_test(1, "ord-cap-01")
}

fn req_with_class(asset_class: AssetClass) -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: "ord-cap-01".to_string(),
        symbol: "BTC/USD".to_string(),
        side: Side::Buy,
        quantity: mqk_execution::QtyMicros::from_whole_units(1).unwrap(),
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
        asset_class,
    }
}

// ---------------------------------------------------------------------------
// C01 — declared Crypto capability routes end-to-end
// ---------------------------------------------------------------------------

#[test]
fn c01_declared_crypto_capability_routes_end_to_end() {
    let broker = CapabilityBroker {
        supported: &[AssetClass::Equity, AssetClass::Crypto],
    };
    let gw = BrokerGateway::for_test(broker, AllClear, AllClear, AllClear);
    let result = gw.submit(&claim(), req_with_class(AssetClass::Crypto));
    assert!(
        result.is_ok(),
        "crypto must route through a broker that declares crypto support: {result:?}"
    );
    assert_eq!(result.unwrap().broker_order_id, "b-ord-cap-01");
}

// ---------------------------------------------------------------------------
// C02 — declaring Crypto does not implicitly widen to Future/Option/Forex
// ---------------------------------------------------------------------------

#[test]
fn c02_crypto_capability_does_not_widen_to_other_non_equity_classes() {
    for undeclared in [AssetClass::Future, AssetClass::Option, AssetClass::Forex] {
        let broker = PanicBroker {
            supported: &[AssetClass::Equity, AssetClass::Crypto],
        };
        let gw = BrokerGateway::for_test(broker, AllClear, AllClear, AllClear);
        let err = gw.submit(&claim(), req_with_class(undeclared)).unwrap_err();
        assert!(
            matches!(
                err,
                SubmitError::Gate(GateRefusal::AssetClassDisabled { asset_class }) if asset_class == undeclared
            ),
            "{undeclared:?} must still be refused by a broker that only declared Equity+Crypto: {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// C03 — capability-set is exact: Crypto-only declaration refuses Equity
// ---------------------------------------------------------------------------

#[test]
fn c03_crypto_only_broker_refuses_equity() {
    let broker = PanicBroker {
        supported: &[AssetClass::Crypto],
    };
    let gw = BrokerGateway::for_test(broker, AllClear, AllClear, AllClear);
    let err = gw
        .submit(&claim(), req_with_class(AssetClass::Equity))
        .unwrap_err();
    assert!(
        matches!(
            err,
            SubmitError::Gate(GateRefusal::AssetClassDisabled {
                asset_class: AssetClass::Equity
            })
        ),
        "a broker that only declared Crypto must refuse Equity, not fall back to it: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// C04 — empty declared capability refuses every class, including Equity
// ---------------------------------------------------------------------------

#[test]
fn c04_empty_capability_broker_refuses_every_asset_class_including_equity() {
    for ac in [
        AssetClass::Equity,
        AssetClass::Crypto,
        AssetClass::Future,
        AssetClass::Option,
        AssetClass::Forex,
    ] {
        let broker = PanicBroker { supported: &[] };
        let gw = BrokerGateway::for_test(broker, AllClear, AllClear, AllClear);
        let err = gw.submit(&claim(), req_with_class(ac)).unwrap_err();
        assert!(
            matches!(
                err,
                SubmitError::Gate(GateRefusal::AssetClassDisabled { asset_class }) if asset_class == ac
            ),
            "an adapter declaring zero supported classes must fail closed for {ac:?}: {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// C05 — GateRefusal still carries the exact rejected asset class
// ---------------------------------------------------------------------------

#[test]
fn c05_gate_refusal_carries_exact_rejected_asset_class() {
    let broker = PanicBroker {
        supported: &[AssetClass::Equity],
    };
    let gw = BrokerGateway::for_test(broker, AllClear, AllClear, AllClear);
    let err = gw
        .submit(&claim(), req_with_class(AssetClass::Forex))
        .unwrap_err();
    match err {
        SubmitError::Gate(GateRefusal::AssetClassDisabled { asset_class }) => {
            assert_eq!(asset_class, AssetClass::Forex);
        }
        other => panic!("expected AssetClassDisabled(Forex), got {other:?}"),
    }
    let msg = err_to_string(&gw, req_with_class(AssetClass::Forex));
    assert!(msg.contains("GATE_REFUSED"), "message: {msg}");
    assert!(msg.contains("disabled"), "message: {msg}");
}

fn err_to_string<B: BrokerAdapter, IG: IntegrityGate, RG: RiskGate, RecG: ReconcileGate>(
    gw: &BrokerGateway<B, IG, RG, RecG>,
    req: BrokerSubmitRequest,
) -> String {
    gw.submit(&claim(), req).unwrap_err().to_string()
}
