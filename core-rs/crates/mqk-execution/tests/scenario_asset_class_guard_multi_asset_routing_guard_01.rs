//! MULTI-ASSET-ROUTING-GUARD-01 — disabled-asset dispatch guard proof tests.
//!
//! # Coverage
//!
//! G01  US equity order passes the asset class guard at BrokerGateway::submit.
//! G02  Crypto order is rejected with AssetClassDisabled before the broker adapter.
//! G03  Futures order is rejected with AssetClassDisabled before the broker adapter.
//! G04  Options order is rejected with AssetClassDisabled before the broker adapter.
//! G05  Forex order is rejected with AssetClassDisabled before the broker adapter.
//! G06  Rejection is an explicit error, not a panic.
//! G07  No disabled asset class has a broker adapter selected (NullBroker never called).
//! G08  Equity still routes end-to-end; non-equity returns before broker call.
//!
//! All tests are pure in-memory — no network, no DB, no wall-clock reads.

use mqk_execution::{
    AssetClass, BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerEvent, BrokerGateway,
    BrokerInvokeToken, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, GateRefusal, IntegrityGate, OutboxClaimToken, ReconcileGate, RiskGate,
    Side, SubmitError,
};

// ---------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------

/// Broker adapter that panics if any method is actually invoked.
/// Used to prove non-equity orders never reach the adapter.
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

/// Minimal echo broker: returns a fixed response so equity tests can succeed.
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
    OutboxClaimToken::for_test(1, "ord-guard-01")
}

fn equity_req() -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: "ord-guard-01".to_string(),
        symbol: "AAPL".to_string(),
        side: Side::Buy,
        quantity: 10,
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
        asset_class: AssetClass::Equity,
    }
}

fn req_with_class(asset_class: AssetClass) -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        asset_class,
        ..equity_req()
    }
}

// ---------------------------------------------------------------------------
// G01 — US equity passes
// ---------------------------------------------------------------------------

#[test]
fn g01_us_equity_passes_asset_class_guard() {
    let gw = BrokerGateway::for_test(EchoBroker, AllClear, AllClear, AllClear);
    let result = gw.submit(&claim(), equity_req());
    assert!(result.is_ok(), "equity should pass the guard: {result:?}");
}

// ---------------------------------------------------------------------------
// G02 — Crypto rejected before broker adapter
// ---------------------------------------------------------------------------

#[test]
fn g02_crypto_rejected_before_broker_adapter() {
    // PanicBroker panics if submit_order is ever called, proving the guard fires first.
    let gw = BrokerGateway::for_test(PanicBroker, AllClear, AllClear, AllClear);
    let err = gw
        .submit(&claim(), req_with_class(AssetClass::Crypto))
        .unwrap_err();
    assert!(
        matches!(
            err,
            SubmitError::Gate(GateRefusal::AssetClassDisabled {
                asset_class: AssetClass::Crypto
            })
        ),
        "expected AssetClassDisabled(Crypto), got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// G03 — Futures rejected before broker adapter
// ---------------------------------------------------------------------------

#[test]
fn g03_futures_rejected_before_broker_adapter() {
    let gw = BrokerGateway::for_test(PanicBroker, AllClear, AllClear, AllClear);
    let err = gw
        .submit(&claim(), req_with_class(AssetClass::Future))
        .unwrap_err();
    assert!(
        matches!(
            err,
            SubmitError::Gate(GateRefusal::AssetClassDisabled {
                asset_class: AssetClass::Future
            })
        ),
        "expected AssetClassDisabled(Future), got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// G04 — Options rejected before broker adapter
// ---------------------------------------------------------------------------

#[test]
fn g04_options_rejected_before_broker_adapter() {
    let gw = BrokerGateway::for_test(PanicBroker, AllClear, AllClear, AllClear);
    let err = gw
        .submit(&claim(), req_with_class(AssetClass::Option))
        .unwrap_err();
    assert!(
        matches!(
            err,
            SubmitError::Gate(GateRefusal::AssetClassDisabled {
                asset_class: AssetClass::Option
            })
        ),
        "expected AssetClassDisabled(Option), got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// G05 — Forex rejected before broker adapter
// ---------------------------------------------------------------------------

#[test]
fn g05_forex_rejected_before_broker_adapter() {
    let gw = BrokerGateway::for_test(PanicBroker, AllClear, AllClear, AllClear);
    let err = gw
        .submit(&claim(), req_with_class(AssetClass::Forex))
        .unwrap_err();
    assert!(
        matches!(
            err,
            SubmitError::Gate(GateRefusal::AssetClassDisabled {
                asset_class: AssetClass::Forex
            })
        ),
        "expected AssetClassDisabled(Forex), got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// G06 — Rejection is an explicit error, not a panic
// ---------------------------------------------------------------------------

#[test]
fn g06_rejection_is_explicit_error_not_panic() {
    let gw = BrokerGateway::for_test(PanicBroker, AllClear, AllClear, AllClear);
    for ac in [
        AssetClass::Crypto,
        AssetClass::Future,
        AssetClass::Option,
        AssetClass::Forex,
    ] {
        let result = gw.submit(&claim(), req_with_class(ac));
        assert!(
            result.is_err(),
            "disabled asset class {ac:?} must return Err, not Ok"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("GATE_REFUSED"),
            "error message must contain GATE_REFUSED for {ac:?}: {msg}"
        );
        assert!(
            msg.contains("disabled"),
            "error message must describe the asset class as disabled for {ac:?}: {msg}"
        );
    }
}

// ---------------------------------------------------------------------------
// G07 — No disabled asset class has a broker adapter invocation
// ---------------------------------------------------------------------------

#[test]
fn g07_no_disabled_asset_class_reaches_broker_adapter() {
    // All four disabled classes: if PanicBroker is ever invoked, test panics.
    let gw = BrokerGateway::for_test(PanicBroker, AllClear, AllClear, AllClear);
    for ac in [
        AssetClass::Crypto,
        AssetClass::Future,
        AssetClass::Option,
        AssetClass::Forex,
    ] {
        let result = std::panic::catch_unwind(|| {
            // We need to reconstruct inside catch_unwind since gw is not UnwindSafe.
            // Instead just check the error type proves it without invoking broker.
            let _ = ac;
        });
        assert!(result.is_ok());

        // Verify via the error type: AssetClassDisabled means broker was NOT reached.
        let err = gw.submit(&claim(), req_with_class(ac)).unwrap_err();
        assert!(
            matches!(
                err,
                SubmitError::Gate(GateRefusal::AssetClassDisabled { .. })
            ),
            "non-equity {ac:?} must be rejected by gate, not broker: {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// G08 — Equity routes end-to-end; non-equity returns before broker
// ---------------------------------------------------------------------------

#[test]
fn g08_equity_routes_end_to_end_non_equity_returns_before_broker() {
    // Equity: reaches EchoBroker and gets a response.
    let eq_gw = BrokerGateway::for_test(EchoBroker, AllClear, AllClear, AllClear);
    let eq_result = eq_gw.submit(&claim(), equity_req());
    assert!(eq_result.is_ok(), "equity must reach broker: {eq_result:?}");
    assert_eq!(
        eq_result.unwrap().broker_order_id,
        "b-ord-guard-01",
        "equity response must come from broker"
    );

    // Non-equity: never reaches PanicBroker, returns an error.
    for ac in [
        AssetClass::Crypto,
        AssetClass::Future,
        AssetClass::Option,
        AssetClass::Forex,
    ] {
        let ne_gw = BrokerGateway::for_test(PanicBroker, AllClear, AllClear, AllClear);
        let ne_result = ne_gw.submit(&claim(), req_with_class(ac));
        assert!(
            ne_result.is_err(),
            "{ac:?} must be rejected before reaching broker"
        );
    }
}
