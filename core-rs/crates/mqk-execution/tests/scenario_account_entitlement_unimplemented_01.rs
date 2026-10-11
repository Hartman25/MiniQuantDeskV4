//! A real-provider adapter that forgets to implement account-entitlement
//! admission must not silently authorize submission.
//!
//! U1  Production wiring (`wiring::build_gateway`): an adapter that declares
//!     nothing is refused with `account_entitlement_not_implemented`, before
//!     any gate and with zero broker invocations.
//! U2  `replace` is refused the same way.
//! U3  The explicit no-external-account declaration (`Ok(())`, as the
//!     in-process simulator does) is admitted by the production wiring.
//! U4  Only the test-only `BrokerGateway::for_test` treats the undeclared
//!     default as a hermetic double and admits it.
//!
//! Requires features `testkit` + `runtime-boundary`.

use std::cell::RefCell;
use std::rc::Rc;

use mqk_execution::wiring::build_gateway;
use mqk_execution::{
    AccountEntitlementRefusal, AssetClass, BrokerAdapter, BrokerCancelResponse, BrokerError,
    BrokerEvent, BrokerGateway, BrokerInvokeToken, BrokerOrderMap, BrokerReplaceRequest,
    BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse, GateRefusal, IntegrityGate,
    OutboxClaimToken, QtyMicros, ReconcileGate, RiskGate, Side, SubmitError,
    ACCOUNT_ENTITLEMENT_NOT_IMPLEMENTED,
};

/// Implements nothing about entitlement (the trait default applies).
struct Undeclared {
    submits: Rc<RefCell<u32>>,
}

/// Declares "no external provider account" explicitly.
struct DeclaresNone {
    submits: Rc<RefCell<u32>>,
}

macro_rules! plumbing {
    ($t:ty) => {
        fn submit_order(
            &self,
            r: BrokerSubmitRequest,
            _t: &BrokerInvokeToken,
        ) -> Result<BrokerSubmitResponse, BrokerError> {
            *self.submits.borrow_mut() += 1;
            Ok(BrokerSubmitResponse {
                broker_order_id: format!("b-{}", r.order_id),
                submitted_at: 0,
                status: "ok".to_string(),
            })
        }
        fn cancel_order(
            &self,
            id: &str,
            _t: &BrokerInvokeToken,
        ) -> Result<BrokerCancelResponse, BrokerError> {
            Ok(BrokerCancelResponse {
                broker_order_id: id.to_string(),
                cancelled_at: 0,
                status: "ok".to_string(),
            })
        }
        fn replace_order(
            &self,
            r: BrokerReplaceRequest,
            _t: &BrokerInvokeToken,
        ) -> Result<BrokerReplaceResponse, BrokerError> {
            Ok(BrokerReplaceResponse {
                broker_order_id: r.broker_order_id,
                replaced_at: 0,
                status: "ok".to_string(),
            })
        }
        fn fetch_events(
            &self,
            _c: Option<&str>,
            _t: &BrokerInvokeToken,
        ) -> Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
            Ok((vec![], None))
        }
    };
}

impl BrokerAdapter for Undeclared {
    plumbing!(Undeclared);
}

impl BrokerAdapter for DeclaresNone {
    fn admit_account_entitlement(
        &self,
        _c: Option<AssetClass>,
    ) -> Result<(), AccountEntitlementRefusal> {
        Ok(())
    }
    plumbing!(DeclaresNone);
}

struct Armed(Rc<RefCell<u32>>);
impl IntegrityGate for Armed {
    fn is_armed(&self) -> bool {
        *self.0.borrow_mut() += 1;
        true
    }
}
struct Allow;
impl RiskGate for Allow {
    fn evaluate_gate(&self) -> mqk_execution::RiskDecision {
        mqk_execution::RiskDecision::Allow
    }
}
struct Clean;
impl ReconcileGate for Clean {
    fn is_clean(&self) -> bool {
        true
    }
}

fn req() -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: "ord-unimpl-01".to_string(),
        symbol: "AAPL".to_string(),
        side: Side::Buy,
        quantity: QtyMicros::from_whole_units(1).unwrap(),
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
        asset_class: AssetClass::Equity,
    }
}

fn claim() -> OutboxClaimToken {
    OutboxClaimToken::for_test(1, "ord-unimpl-01")
}

#[test]
fn u1_production_wiring_refuses_an_adapter_that_declares_nothing() {
    let submits = Rc::new(RefCell::new(0));
    let gate_calls = Rc::new(RefCell::new(0));
    let gw = build_gateway(
        Undeclared {
            submits: Rc::clone(&submits),
        },
        Armed(Rc::clone(&gate_calls)),
        Allow,
        Clean,
    );
    match gw.submit(&claim(), req()).unwrap_err() {
        SubmitError::Gate(GateRefusal::AccountEntitlementRefused(r)) => {
            assert_eq!(r.code, ACCOUNT_ENTITLEMENT_NOT_IMPLEMENTED);
        }
        other => panic!("expected the fail-closed refusal, got {other:?}"),
    }
    assert_eq!(*submits.borrow(), 0, "broker must never be invoked");
    assert_eq!(*gate_calls.borrow(), 0, "refused before any gate");
}

#[test]
fn u2_replace_is_refused_the_same_way() {
    let gw = build_gateway(
        Undeclared {
            submits: Rc::new(RefCell::new(0)),
        },
        Armed(Rc::new(RefCell::new(0))),
        Allow,
        Clean,
    );
    let mut map = BrokerOrderMap::new();
    map.register("ord-x", "b-x");
    let err = gw
        .replace(
            "ord-x",
            &map,
            QtyMicros::from_whole_units(2).unwrap(),
            None,
            "day".to_string(),
        )
        .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<GateRefusal>(),
        Some(GateRefusal::AccountEntitlementRefused(r))
            if r.code == ACCOUNT_ENTITLEMENT_NOT_IMPLEMENTED
    ));
}

#[test]
fn u3_explicit_no_external_account_declaration_is_admitted() {
    let submits = Rc::new(RefCell::new(0));
    let gw = build_gateway(
        DeclaresNone {
            submits: Rc::clone(&submits),
        },
        Armed(Rc::new(RefCell::new(0))),
        Allow,
        Clean,
    );
    assert!(gw.submit(&claim(), req()).is_ok());
    assert_eq!(*submits.borrow(), 1);
}

#[test]
fn u4_only_the_test_constructor_admits_an_undeclared_double() {
    let submits = Rc::new(RefCell::new(0));
    let gw = BrokerGateway::for_test(
        Undeclared {
            submits: Rc::clone(&submits),
        },
        Armed(Rc::new(RefCell::new(0))),
        Allow,
        Clean,
    );
    assert!(gw.submit(&claim(), req()).is_ok());
    assert_eq!(*submits.borrow(), 1);
}
