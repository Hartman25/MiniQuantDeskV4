//! Broker-account entitlement authority at the single gateway seam.
//!
//! E01  A refusing `admit_account_entitlement` blocks `submit` before the
//!      adapter and before every gate, carrying the adapter's code verbatim.
//! E02  An admitting adapter still reaches the broker (positive control).
//! E03  The refusal applies to a verified risk-reducing close too: there is
//!      no exemption for `is_risk_reducing`.
//! E04  The authority receives the order's own asset class (and `None` for
//!      replace), so class-specific entitlement cannot be bypassed.
//! E05  The adapter-capability gate still runs first; the entitlement gate
//!      runs before integrity/risk/reconcile gates.
//! E06  Under the test-only `for_test` constructor an adapter that declares
//!      nothing is a hermetic double and is admitted (production wiring refuses
//!      it: see `scenario_account_entitlement_unimplemented_01`).
//! E07  `replace` is refused by the same authority; `cancel` is not
//!      entitlement-gated (it remains subject to the existing three gates).

use std::cell::RefCell;
use std::rc::Rc;

use mqk_execution::{
    AccountEntitlementRefusal, AssetClass, BrokerAdapter, BrokerCancelResponse, BrokerError,
    BrokerEvent, BrokerGateway, BrokerInvokeToken, BrokerOrderMap, BrokerReplaceRequest,
    BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse, GateRefusal, IntegrityGate,
    OutboxClaimToken, QtyMicros, ReconcileGate, RiskGate, RiskRequestContext, Side, SubmitError,
};

struct EntitlementBroker {
    refuse_with: Option<&'static str>,
    seen_classes: Rc<RefCell<Vec<Option<AssetClass>>>>,
}

impl EntitlementBroker {
    fn new(refuse_with: Option<&'static str>) -> Self {
        Self {
            refuse_with,
            seen_classes: Rc::new(RefCell::new(vec![])),
        }
    }
}

impl BrokerAdapter for EntitlementBroker {
    fn admit_account_entitlement(
        &self,
        asset_class: Option<AssetClass>,
    ) -> Result<(), AccountEntitlementRefusal> {
        self.seen_classes.borrow_mut().push(asset_class);
        match self.refuse_with {
            Some(code) => Err(AccountEntitlementRefusal {
                code: code.to_string(),
                detail: "test refusal".to_string(),
            }),
            None => Ok(()),
        }
    }
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _t: &BrokerInvokeToken,
    ) -> Result<BrokerSubmitResponse, BrokerError> {
        Ok(BrokerSubmitResponse {
            broker_order_id: format!("b-{}", req.order_id),
            submitted_at: 0,
            status: "ok".to_string(),
        })
    }
    fn cancel_order(
        &self,
        order_id: &str,
        _t: &BrokerInvokeToken,
    ) -> Result<BrokerCancelResponse, BrokerError> {
        Ok(BrokerCancelResponse {
            broker_order_id: order_id.to_string(),
            cancelled_at: 0,
            status: "ok".to_string(),
        })
    }
    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _t: &BrokerInvokeToken,
    ) -> Result<BrokerReplaceResponse, BrokerError> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
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
}

/// Counts integrity-gate evaluations so "before every gate" is observable.
#[derive(Default)]
struct Armed(Rc<RefCell<u32>>);
impl Armed {
    fn counting() -> (Self, Rc<RefCell<u32>>) {
        let calls = Rc::new(RefCell::new(0));
        (Self(Rc::clone(&calls)), calls)
    }
}
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

fn req(asset_class: AssetClass) -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: "ord-ent-01".to_string(),
        symbol: "AAPL".to_string(),
        side: Side::Buy,
        quantity: QtyMicros::from_whole_units(1).unwrap(),
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
        asset_class,
    }
}

fn claim() -> OutboxClaimToken {
    OutboxClaimToken::for_test(1, "ord-ent-01")
}

#[test]
fn e01_refusal_blocks_before_adapter_and_before_gates() {
    let (armed, calls) = Armed::counting();
    let gw = BrokerGateway::for_test(
        EntitlementBroker::new(Some("account_trading_blocked")),
        armed,
        Allow,
        Clean,
    );
    let err = gw.submit(&claim(), req(AssetClass::Equity)).unwrap_err();
    match err {
        SubmitError::Gate(GateRefusal::AccountEntitlementRefused(r)) => {
            assert_eq!(r.code, "account_trading_blocked");
        }
        other => panic!("expected AccountEntitlementRefused, got {other:?}"),
    }
    assert_eq!(*calls.borrow(), 0, "no gate may run first");
}

#[test]
fn e02_admitting_adapter_reaches_broker() {
    let (armed, calls) = Armed::counting();
    let gw = BrokerGateway::for_test(
        EntitlementBroker::new(None),
        armed,
        Allow,
        Clean,
    );
    let ok = gw.submit(&claim(), req(AssetClass::Equity)).unwrap();
    assert_eq!(ok.broker_order_id, "b-ord-ent-01");
    assert_eq!(*calls.borrow(), 1);
}

#[test]
fn e03_risk_reducing_close_has_no_exemption() {
    let gw = BrokerGateway::for_test(
        EntitlementBroker::new(Some("account_blocked")),
        Armed::default(),
        Allow,
        Clean,
    );
    let err = gw
        .submit_with_context(
            &claim(),
            req(AssetClass::Equity),
            RiskRequestContext {
                is_risk_reducing: true,
            },
        )
        .unwrap_err();
    assert!(matches!(
        err,
        SubmitError::Gate(GateRefusal::AccountEntitlementRefused(_))
    ));
}

#[test]
fn e04_authority_receives_order_class_and_none_for_replace() {
    let broker = EntitlementBroker::new(None);
    let seen = Rc::clone(&broker.seen_classes);
    let gw = BrokerGateway::for_test(broker, Armed::default(), Allow, Clean);
    let mut map = BrokerOrderMap::new();
    map.register("ord-x", "b-x");
    gw.submit(&claim(), req(AssetClass::Equity)).unwrap();
    gw.replace(
        "ord-x",
        &map,
        QtyMicros::from_whole_units(2).unwrap(),
        None,
        "day".to_string(),
    )
    .unwrap();
    assert_eq!(*seen.borrow(), vec![Some(AssetClass::Equity), None]);
}

#[test]
fn e05_asset_capability_gate_runs_before_entitlement() {
    // Default adapter supports Equity only; a Crypto order must be refused
    // by the capability gate, and the entitlement authority never consulted.
    struct Probe(Rc<RefCell<u32>>);
    impl BrokerAdapter for Probe {
        fn admit_account_entitlement(
            &self,
            _c: Option<AssetClass>,
        ) -> Result<(), AccountEntitlementRefusal> {
            *self.0.borrow_mut() += 1;
            Ok(())
        }
        fn submit_order(
            &self,
            _r: BrokerSubmitRequest,
            _t: &BrokerInvokeToken,
        ) -> Result<BrokerSubmitResponse, BrokerError> {
            unreachable!()
        }
        fn cancel_order(
            &self,
            _o: &str,
            _t: &BrokerInvokeToken,
        ) -> Result<BrokerCancelResponse, BrokerError> {
            unreachable!()
        }
        fn replace_order(
            &self,
            _r: BrokerReplaceRequest,
            _t: &BrokerInvokeToken,
        ) -> Result<BrokerReplaceResponse, BrokerError> {
            unreachable!()
        }
        fn fetch_events(
            &self,
            _c: Option<&str>,
            _t: &BrokerInvokeToken,
        ) -> Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
            Ok((vec![], None))
        }
    }
    let consulted = Rc::new(RefCell::new(0));
    let gw = BrokerGateway::for_test(
        Probe(Rc::clone(&consulted)),
        Armed::default(),
        Allow,
        Clean,
    );
    let err = gw.submit(&claim(), req(AssetClass::Crypto)).unwrap_err();
    assert_eq!(*consulted.borrow(), 0, "capability gate must refuse first");
    assert!(matches!(
        err,
        SubmitError::Gate(GateRefusal::AssetClassDisabled { .. })
    ));
}

#[test]
fn e06_hermetic_gateway_admits_an_undeclared_double() {
    struct Plain;
    impl BrokerAdapter for Plain {
        fn submit_order(
            &self,
            r: BrokerSubmitRequest,
            _t: &BrokerInvokeToken,
        ) -> Result<BrokerSubmitResponse, BrokerError> {
            Ok(BrokerSubmitResponse {
                broker_order_id: format!("b-{}", r.order_id),
                submitted_at: 0,
                status: "ok".to_string(),
            })
        }
        fn cancel_order(
            &self,
            _o: &str,
            _t: &BrokerInvokeToken,
        ) -> Result<BrokerCancelResponse, BrokerError> {
            unreachable!()
        }
        fn replace_order(
            &self,
            _r: BrokerReplaceRequest,
            _t: &BrokerInvokeToken,
        ) -> Result<BrokerReplaceResponse, BrokerError> {
            unreachable!()
        }
        fn fetch_events(
            &self,
            _c: Option<&str>,
            _t: &BrokerInvokeToken,
        ) -> Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
            Ok((vec![], None))
        }
    }
    let gw = BrokerGateway::for_test(Plain, Armed::default(), Allow, Clean);
    assert!(gw.submit(&claim(), req(AssetClass::Equity)).is_ok());
}

#[test]
fn e07_replace_refused_cancel_not_entitlement_gated() {
    let gw = BrokerGateway::for_test(
        EntitlementBroker::new(Some("account_trading_blocked")),
        Armed::default(),
        Allow,
        Clean,
    );
    let mut map = BrokerOrderMap::new();
    map.register("ord-x", "b-x");
    let replace_err = gw
        .replace(
            "ord-x",
            &map,
            QtyMicros::from_whole_units(2).unwrap(),
            None,
            "day".to_string(),
        )
        .unwrap_err();
    let refusal = replace_err
        .downcast_ref::<GateRefusal>()
        .expect("replace refusal is a GateRefusal");
    assert!(matches!(
        refusal,
        GateRefusal::AccountEntitlementRefused(r) if r.code == "account_trading_blocked"
    ));
    // Cancel is unchanged: governed only by the three existing gates.
    assert!(gw.cancel("ord-x", &map).is_ok());
}
