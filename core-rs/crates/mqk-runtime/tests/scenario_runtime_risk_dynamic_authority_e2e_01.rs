//! RUNTIME-RISK-DYNAMIC-STATE-AUTHORITY-01: load-bearing end-to-end proof.
//!
//! Drives the REAL production wrappers — `mqk_execution::gateway::BrokerGateway`
//! wrapping the REAL `mqk_runtime::runtime_risk::RuntimeRiskGate` — through a
//! hermetic broker adapter. No `mqk_risk::evaluate` call is made directly by
//! this test; every decision flows through `RuntimeRiskGate` exactly as
//! production code (`mqk-daemon`'s `build_execution_orchestrator`) wires it,
//! proving:
//!
//! 1. current authoritative dynamic context (not construction-time input)
//!    drives every decision;
//! 2. a risk denial refuses BEFORE the broker adapter is invoked.
//!
//! No real Alpaca call, no real Paper DB — the broker adapter is a hermetic
//! in-memory stub and the account authority is a hermetic test double.

use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration as ChronoDuration, Utc};

use mqk_execution::gateway::{BrokerGateway, IntegrityGate, ReconcileGate};
use mqk_execution::{
    broker_error::BrokerError, AssetClass, BrokerAdapter, BrokerCancelResponse,
    BrokerInvokeToken, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, GateRefusal, OutboxClaimToken, Side, SubmitError,
};
use mqk_runtime::runtime_risk::{
    AccountAuthorityContext, AccountAuthorityError, RuntimeAccountAuthority, RuntimeClock,
    RuntimeRiskGate,
};

// ---------------------------------------------------------------------------
// Hermetic test doubles
// ---------------------------------------------------------------------------

struct TestClock(Mutex<DateTime<Utc>>);
impl TestClock {
    fn new(t: DateTime<Utc>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(t)))
    }
    fn set(&self, t: DateTime<Utc>) {
        *self.0.lock().unwrap() = t;
    }
    fn advance(&self, d: ChronoDuration) {
        let mut g = self.0.lock().unwrap();
        *g += d;
    }
}
impl RuntimeClock for TestClock {
    fn now_utc(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

/// Mirrors `mqk-daemon`'s `DaemonAccountAuthority`: equity + a captured-at
/// timestamp, with an explicit freshness bound checked against the clock's
/// `now`. Lets the test simulate a genuinely stale broker snapshot.
struct TestAccountAuthority {
    equity_micros: AtomicI64,
    captured_at: Mutex<DateTime<Utc>>,
    freshness_bound: ChronoDuration,
    unavailable: std::sync::atomic::AtomicBool,
}

impl TestAccountAuthority {
    fn new(equity_micros: i64, captured_at: DateTime<Utc>, freshness_bound: ChronoDuration) -> Arc<Self> {
        Arc::new(Self {
            equity_micros: AtomicI64::new(equity_micros),
            captured_at: Mutex::new(captured_at),
            freshness_bound,
            unavailable: std::sync::atomic::AtomicBool::new(false),
        })
    }
    fn set_equity(&self, equity_micros: i64, captured_at: DateTime<Utc>) {
        self.equity_micros.store(equity_micros, Ordering::SeqCst);
        *self.captured_at.lock().unwrap() = captured_at;
    }
    fn set_unavailable(&self, unavailable: bool) {
        self.unavailable.store(unavailable, Ordering::SeqCst);
    }
}

impl RuntimeAccountAuthority for TestAccountAuthority {
    fn current_account(
        &self,
        now: DateTime<Utc>,
    ) -> Result<AccountAuthorityContext, AccountAuthorityError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(AccountAuthorityError::Unavailable);
        }
        let captured_at = *self.captured_at.lock().unwrap();
        let age = now.signed_duration_since(captured_at);
        if age < ChronoDuration::zero() || age > self.freshness_bound {
            return Err(AccountAuthorityError::Stale);
        }
        Ok(AccountAuthorityContext {
            equity_micros: self.equity_micros.load(Ordering::SeqCst),
            pdt: mqk_runtime::runtime_risk::RiskPdtContext::ok(),
            kill_switch: None,
        })
    }
}

struct PassGate;
impl IntegrityGate for PassGate {
    fn is_armed(&self) -> bool {
        true
    }
}
impl ReconcileGate for PassGate {
    fn is_clean(&self) -> bool {
        true
    }
}

/// Hermetic broker: counts submit invocations and returns either Ok or a
/// configured error every time. Proves "broker adapter NOT invoked" by
/// comparing `submit_count()` before/after a gate-refused call.
///
/// `Clone` shares the SAME underlying `Arc` state — a clone handed to
/// `BrokerGateway::for_test` and the original kept by the test observe the
/// identical counter.
#[derive(Clone)]
struct HermeticBroker {
    submit_count: Arc<AtomicU32>,
    behavior: Arc<Mutex<Box<dyn Fn() -> Result<BrokerSubmitResponse, BrokerError> + Send>>>,
}
impl HermeticBroker {
    fn always_ok() -> Self {
        Self {
            submit_count: Arc::new(AtomicU32::new(0)),
            behavior: Arc::new(Mutex::new(Box::new(|| {
                Ok(BrokerSubmitResponse {
                    broker_order_id: "b-ok".to_string(),
                    submitted_at: 1,
                    status: "ok".to_string(),
                })
            }))),
        }
    }
    fn always_reject() -> Self {
        Self {
            submit_count: Arc::new(AtomicU32::new(0)),
            behavior: Arc::new(Mutex::new(Box::new(|| {
                Err(BrokerError::Reject {
                    code: "insufficient_qty".to_string(),
                    detail: "hermetic reject".to_string(),
                })
            }))),
        }
    }
    fn submit_count(&self) -> u32 {
        self.submit_count.load(Ordering::SeqCst)
    }
}
impl BrokerAdapter for HermeticBroker {
    fn submit_order(
        &self,
        _req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerSubmitResponse, BrokerError> {
        self.submit_count.fetch_add(1, Ordering::SeqCst);
        (self.behavior.lock().unwrap())()
    }
    fn cancel_order(
        &self,
        order_id: &str,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerCancelResponse, BrokerError> {
        Ok(BrokerCancelResponse {
            broker_order_id: order_id.to_string(),
            cancelled_at: 1,
            status: "ok".to_string(),
        })
    }
    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerReplaceResponse, BrokerError> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 1,
            status: "ok".to_string(),
        })
    }
    fn fetch_events(
        &self,
        _cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> Result<(Vec<mqk_execution::BrokerEvent>, Option<String>), BrokerError> {
        Ok((vec![], None))
    }
}

fn t(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    chrono::TimeZone::with_ymd_and_hms(&Utc, y, mo, d, h, mi, 0).unwrap()
}

fn submit_req(order_id: &str) -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: order_id.to_string(),
        symbol: "AAPL".to_string(),
        side: Side::Buy,
        quantity: 10,
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
        asset_class: AssetClass::Equity,
    }
}

fn claim(id: &str) -> OutboxClaimToken {
    OutboxClaimToken::for_test(1, id)
}

// ---------------------------------------------------------------------------
// CASE 1 — DAILY LOSS
// ---------------------------------------------------------------------------
#[test]
fn case1_daily_loss_denies_before_broker_invocation() {
    let clock = TestClock::new(t(2024, 1, 15, 9, 0));
    let start = t(2024, 1, 15, 9, 0);
    let account = TestAccountAuthority::new(100_000 * 1_000_000, start, ChronoDuration::seconds(180));
    let risk_gate = RuntimeRiskGate::from_run_config_with_account_authority(
        &serde_json::json!({ "risk": { "daily_loss_limit": 0.02, "max_drawdown": 0.50 } }),
        100_000 * 1_000_000,
        account.clone(),
        clock.clone(),
    );
    let broker = HermeticBroker::always_ok();
    let gateway = BrokerGateway::for_test(broker.clone(), PassGate, risk_gate, PassGate);

    // Above the floor: allowed, broker invoked once.
    let ok = gateway.submit(&claim("c1"), submit_req("c1"));
    assert!(ok.is_ok(), "starting equity must be allowed");
    assert_eq!(broker.submit_count(), 1);

    // Update authoritative current equity below the daily-loss floor
    // (100k - 2% = 98k floor; 97_999 breaches it).
    account.set_equity(97_999 * 1_000_000, t(2024, 1, 15, 9, 1));
    clock.set(t(2024, 1, 15, 9, 2));

    let err = gateway
        .submit(&claim("c2"), submit_req("c2"))
        .expect_err("daily-loss breach must deny before broker invocation");
    assert!(matches!(err, SubmitError::Gate(GateRefusal::RiskBlocked(_))));
    assert_eq!(
        broker.submit_count(),
        1,
        "broker adapter must NOT be invoked when the risk gate denies"
    );
}

// ---------------------------------------------------------------------------
// CASE 2 — MAX DRAWDOWN
// ---------------------------------------------------------------------------
#[test]
fn case2_max_drawdown_denies_before_broker_invocation() {
    let clock = TestClock::new(t(2024, 1, 15, 9, 0));
    let start = t(2024, 1, 15, 9, 0);
    let account = TestAccountAuthority::new(100_000 * 1_000_000, start, ChronoDuration::seconds(180));
    let risk_gate = RuntimeRiskGate::from_run_config_with_account_authority(
        // daily_loss_limit set very loose (50%) so it can never fire first;
        // max_drawdown at 10% is the only limit under test.
        &serde_json::json!({ "risk": { "daily_loss_limit": 0.50, "max_drawdown": 0.10 } }),
        100_000 * 1_000_000,
        account.clone(),
        clock.clone(),
    );
    let broker = HermeticBroker::always_ok();
    let gateway = BrokerGateway::for_test(broker.clone(), PassGate, risk_gate, PassGate);

    // Establish a higher peak through current context (peak tracks the
    // MAXIMUM equity ever observed by evaluate()).
    account.set_equity(120_000 * 1_000_000, t(2024, 1, 15, 9, 1));
    clock.set(t(2024, 1, 15, 9, 2));
    let ok = gateway.submit(&claim("c1"), submit_req("c1"));
    assert!(ok.is_ok(), "peak-setting evaluation must be allowed");
    assert_eq!(broker.submit_count(), 1);

    // Lower current equity beyond the configured 10% max-drawdown from the
    // 120k peak (floor = 108k); 107_999 breaches it.
    account.set_equity(107_999 * 1_000_000, t(2024, 1, 15, 9, 3));
    clock.set(t(2024, 1, 15, 9, 4));

    let err = gateway
        .submit(&claim("c2"), submit_req("c2"))
        .expect_err("max-drawdown breach must deny before broker invocation");
    assert!(matches!(err, SubmitError::Gate(GateRefusal::RiskBlocked(_))));
    assert_eq!(
        broker.submit_count(),
        1,
        "broker adapter must NOT be invoked when the risk gate denies"
    );
}

// ---------------------------------------------------------------------------
// CASE 3 — REJECT STORM
// ---------------------------------------------------------------------------
#[test]
fn case3_reject_storm_denies_next_new_risk_before_broker_invocation() {
    let clock = TestClock::new(t(2024, 1, 15, 9, 0));
    let start = t(2024, 1, 15, 9, 0);
    let account = TestAccountAuthority::new(100_000 * 1_000_000, start, ChronoDuration::seconds(180));
    let risk_gate = RuntimeRiskGate::from_run_config_with_account_authority(
        &serde_json::json!({
            "risk": {
                "daily_loss_limit": 0.90,
                "max_drawdown": 0.90,
                "reject_storm": { "max_rejects": 3 },
            }
        }),
        100_000 * 1_000_000,
        account,
        clock.clone(),
    );
    let broker = HermeticBroker::always_reject();
    let gateway = BrokerGateway::for_test(broker.clone(), PassGate, risk_gate, PassGate);

    for i in 0..3 {
        let err = gateway.submit(&claim(&format!("r{i}")), submit_req(&format!("r{i}")));
        let Err(SubmitError::Broker(BrokerError::Reject { .. })) = err else {
            panic!("reject #{i} must reach the broker and surface as a Reject");
        };
    }
    assert_eq!(broker.submit_count(), 3, "exactly 3 broker submits so far");

    // Exact threshold reached: the NEXT new-risk submit must be refused by
    // the risk gate BEFORE the broker adapter is invoked again.
    let err = gateway
        .submit(&claim("r-next"), submit_req("r-next"))
        .expect_err("reject-storm threshold must deny the next new-risk submit");
    assert!(matches!(err, SubmitError::Gate(GateRefusal::RiskBlocked(_))));
    assert_eq!(
        broker.submit_count(),
        3,
        "broker adapter must NOT be invoked once the reject-storm threshold denies"
    );
}

// ---------------------------------------------------------------------------
// CASE 4 — STALE AUTHORITY
// ---------------------------------------------------------------------------
#[test]
fn case4_stale_authority_denies_before_broker_invocation() {
    let clock = TestClock::new(t(2024, 1, 15, 9, 0));
    let start = t(2024, 1, 15, 9, 0);
    let account = TestAccountAuthority::new(100_000 * 1_000_000, start, ChronoDuration::seconds(180));
    let risk_gate = RuntimeRiskGate::from_run_config_with_account_authority(
        &serde_json::json!({ "risk": { "daily_loss_limit": 0.02, "max_drawdown": 0.50 } }),
        100_000 * 1_000_000,
        account.clone(),
        clock.clone(),
    );
    let broker = HermeticBroker::always_ok();
    let gateway = BrokerGateway::for_test(broker.clone(), PassGate, risk_gate, PassGate);

    let ok = gateway.submit(&claim("c1"), submit_req("c1"));
    assert!(ok.is_ok(), "fresh authority must be allowed");
    assert_eq!(broker.submit_count(), 1);

    // Advance the clock well past the 180s freshness bound WITHOUT updating
    // the authority's captured_at — simulating a broker snapshot that
    // stopped refreshing.
    clock.advance(ChronoDuration::seconds(500));

    let err = gateway
        .submit(&claim("c2"), submit_req("c2"))
        .expect_err("stale account authority must deny before broker invocation");
    assert!(matches!(err, SubmitError::Gate(GateRefusal::RiskBlocked(_))));
    assert_eq!(
        broker.submit_count(),
        1,
        "broker adapter must NOT be invoked when the account authority is stale"
    );

    // Unavailable authority (e.g. no snapshot ever captured) must also deny.
    account.set_unavailable(true);
    let err2 = gateway
        .submit(&claim("c3"), submit_req("c3"))
        .expect_err("unavailable account authority must deny before broker invocation");
    assert!(matches!(err2, SubmitError::Gate(GateRefusal::RiskBlocked(_))));
    assert_eq!(broker.submit_count(), 1);
}
