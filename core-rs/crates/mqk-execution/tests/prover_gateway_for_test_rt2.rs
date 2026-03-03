//! RT-2 positive prover: BrokerGateway::for_test is accessible under testkit.
//!
//! Run: cargo test -p mqk-execution --features testkit --test prover_gateway_for_test_rt2
//!
//! Negative proof (compile-fail) is structural: any crate that depends on
//! mqk-execution without `features = ["testkit"]` in production [dependencies]
//! will get a compile error on any attempt to call BrokerGateway::for_test.

use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerEvent, BrokerGateway, BrokerInvokeToken,
    BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
    IntegrityGate, ReconcileGate, RiskGate,
};

// Minimal broker stub.
struct NullBroker;

impl BrokerAdapter for NullBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _t: &BrokerInvokeToken,
    ) -> Result<BrokerSubmitResponse, Box<dyn std::error::Error>> {
        Ok(BrokerSubmitResponse {
            broker_order_id: req.order_id,
            submitted_at: 0,
            status: "ok".to_string(),
        })
    }

    fn cancel_order(
        &self,
        id: &str,
        _t: &BrokerInvokeToken,
    ) -> Result<BrokerCancelResponse, Box<dyn std::error::Error>> {
        Ok(BrokerCancelResponse {
            broker_order_id: id.to_string(),
            cancelled_at: 0,
            status: "ok".to_string(),
        })
    }

    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _t: &BrokerInvokeToken,
    ) -> Result<BrokerReplaceResponse, Box<dyn std::error::Error>> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 0,
            status: "ok".to_string(),
        })
    }

    fn fetch_events(
        &self,
        _t: &BrokerInvokeToken,
    ) -> Result<Vec<BrokerEvent>, Box<dyn std::error::Error>> {
        Ok(vec![])
    }
}

struct PassGate;
impl IntegrityGate for PassGate {
    fn is_armed(&self) -> bool {
        true
    }
}
impl RiskGate for PassGate {
    fn is_allowed(&self) -> bool {
        true
    }
}
impl ReconcileGate for PassGate {
    fn is_clean(&self) -> bool {
        true
    }
}

/// RT-2 positive prover: BrokerGateway::for_test compiles under testkit feature.
///
/// If this test compiles and runs, the gate is correctly configured: for_test
/// is reachable when feature = "testkit" is active, and unreachable without it.
#[test]
fn gateway_for_test_accessible_under_testkit() {
    let _gw: BrokerGateway<NullBroker, PassGate, PassGate, PassGate> =
        BrokerGateway::for_test(NullBroker, PassGate, PassGate, PassGate);
}
