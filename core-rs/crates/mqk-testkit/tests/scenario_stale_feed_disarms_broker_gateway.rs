//! Scenario: Stale Feed Disarms Broker Gateway — PATCH E2
//!
//! Invariant under test:
//! A stale feed detected by the integrity engine sets `disarmed = true` on
//! `IntegrityState`.  When `IntegrityState` is wired into `BrokerGateway` as
//! the `IntegrityGate`, all subsequent broker operations (submit, cancel,
//! replace) are refused with `GateRefusal::IntegrityDisarmed`.
//!
//! End-to-end chain proven here:
//!   stale feed → IntegrityAction::Disarm → disarmed=true
//!              → IntegrityAdapter::is_armed() = false
//!              → BrokerGateway::enforce_gates() returns IntegrityDisarmed
//!
//! Previously the chain was proven only in parts:
//!   - mqk-integrity: stale feed → is_execution_blocked()   (pure integrity)
//!   - mqk-execution: gate refusal with boolean stubs        (gateway unit)
//!
//! PATCH E2 closes the gap: real IntegrityState wired into BrokerGateway,
//! proving DISARM is enforced at the broker choke-point end-to-end.
//!
//! # IntegrityAdapter
//! Rust's orphan rule prevents implementing a foreign trait for a foreign type
//! outside their home crates.  `IntegrityAdapter(IntegrityState)` is a newtype
//! defined here that owns the `impl IntegrityGate`.  In production, an
//! equivalent adapter lives at the runtime orchestration wiring boundary
//! (e.g., mqk-daemon or the engine dispatch layer).

use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerGateway, BrokerInvokeToken, BrokerOrderMap,
    BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
    GateRefusal, IntegrityGate, OutboxClaimToken, ReconcileGate, RiskGate,
};
use mqk_integrity::{
    evaluate_bar, tick_feed, Bar, BarKey, CalendarSpec, FeedId, IntegrityAction, IntegrityConfig,
    IntegrityState, Timeframe,
};

// ---------------------------------------------------------------------------
// IntegrityAdapter: bridges IntegrityState → IntegrityGate
// ---------------------------------------------------------------------------

/// Newtype that implements `IntegrityGate` for `IntegrityState`.
///
/// `is_armed()` returns `true` only when execution is NOT blocked — i.e.,
/// neither the `disarmed` nor `halted` flag is set on the inner state.
struct IntegrityAdapter(IntegrityState);

impl IntegrityGate for IntegrityAdapter {
    fn is_armed(&self) -> bool {
        !self.0.is_execution_blocked()
    }
}

// ---------------------------------------------------------------------------
// Minimal "always OK" broker stub
// ---------------------------------------------------------------------------

struct OkBroker;

impl BrokerAdapter for OkBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerSubmitResponse, Box<dyn std::error::Error>> {
        Ok(BrokerSubmitResponse {
            broker_order_id: format!("b-{}", req.order_id),
            submitted_at: 1,
            status: "submitted".to_string(),
        })
    }

    fn cancel_order(
        &self,
        order_id: &str,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerCancelResponse, Box<dyn std::error::Error>> {
        Ok(BrokerCancelResponse {
            broker_order_id: order_id.to_string(),
            cancelled_at: 1,
            status: "cancelled".to_string(),
        })
    }

    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerReplaceResponse, Box<dyn std::error::Error>> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 1,
            status: "replaced".to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Always-pass stubs for risk and reconcile
// ---------------------------------------------------------------------------

struct AlwaysPass;

impl RiskGate for AlwaysPass {
    fn is_allowed(&self) -> bool {
        true
    }
}

impl ReconcileGate for AlwaysPass {
    fn is_clean(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn make_claim() -> OutboxClaimToken {
    OutboxClaimToken::from_claimed_row(1, "ord-e2")
}

fn submit_req() -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: "ord-e2".to_string(),
        symbol: "SPY".to_string(),
        quantity: 5,
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
    }
}

/// IntegrityConfig with stale_threshold_ticks = 5.
/// Any feed lagging more than 5 ticks behind now triggers disarm.
fn stale_cfg() -> IntegrityConfig {
    IntegrityConfig {
        gap_tolerance_bars: 0,
        stale_threshold_ticks: 5,
        enforce_feed_disagreement: false,
        calendar: CalendarSpec::AlwaysOn,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Both feeds healthy — gateway allows submit.
///
/// Baseline: no stale condition → IntegrityAdapter::is_armed() = true →
/// gateway does not refuse the operation.
#[test]
fn healthy_integrity_allows_submit() {
    let cfg = stale_cfg();
    let mut st = IntegrityState::new();
    let feed_a = FeedId::new("feedA");
    let feed_b = FeedId::new("feedB");

    // Seed both feeds at the same tick — no stale condition.
    tick_feed(&cfg, &mut st, &feed_a, 10);
    tick_feed(&cfg, &mut st, &feed_b, 10);
    assert!(
        !st.is_execution_blocked(),
        "gate must be open before disarm"
    );

    let gw = BrokerGateway::new(OkBroker, IntegrityAdapter(st), AlwaysPass, AlwaysPass);
    let result = gw.submit(&make_claim(), submit_req());
    assert!(result.is_ok(), "healthy integrity must allow submit");
}

/// Stale feed → disarm → submit refused with IntegrityDisarmed.
///
/// This is the primary PATCH E2 success criterion:
///   stale feed → IntegrityAction::Disarm → gateway blocks submit.
///
/// Mechanism: seed feedA and feedB at tick 10, then advance only feedA to
/// tick 16.  feedB is still at 10.  Delta = 16 − 10 = 6 > threshold 5
/// → stale → disarm.
#[test]
fn stale_feed_disarms_gateway_blocks_submit() {
    let cfg = stale_cfg();
    let mut st = IntegrityState::new();
    let feed_a = FeedId::new("feedA");
    let feed_b = FeedId::new("feedB");

    // Seed both feeds.
    tick_feed(&cfg, &mut st, &feed_a, 10);
    tick_feed(&cfg, &mut st, &feed_b, 10);

    // Advance only feed_a; feed_b becomes stale (delta 6 > threshold 5).
    let decision = tick_feed(&cfg, &mut st, &feed_a, 16);
    assert_eq!(
        decision.action,
        IntegrityAction::Disarm,
        "must produce Disarm"
    );
    assert!(st.disarmed, "disarmed flag must be set");
    assert!(st.is_execution_blocked(), "execution must be blocked");

    let gw = BrokerGateway::new(OkBroker, IntegrityAdapter(st), AlwaysPass, AlwaysPass);
    let err = gw.submit(&make_claim(), submit_req()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::IntegrityDisarmed,
        "stale-feed disarm must block gateway with IntegrityDisarmed"
    );
}

/// Stale feed disarm also blocks cancel.
#[test]
fn stale_feed_disarms_gateway_blocks_cancel() {
    let cfg = stale_cfg();
    let mut st = IntegrityState::new();

    tick_feed(&cfg, &mut st, &FeedId::new("feedA"), 10);
    tick_feed(&cfg, &mut st, &FeedId::new("feedB"), 10);
    tick_feed(&cfg, &mut st, &FeedId::new("feedA"), 16); // triggers disarm

    assert!(st.disarmed);

    let gw = BrokerGateway::new(OkBroker, IntegrityAdapter(st), AlwaysPass, AlwaysPass);
    // Gate fires before map lookup (EB-2); empty map is correct here.
    let err = gw.cancel("b-ord-e2", &BrokerOrderMap::new()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("error must downcast to GateRefusal");
    assert_eq!(*refusal, GateRefusal::IntegrityDisarmed);
}

/// Stale feed disarm also blocks replace.
#[test]
fn stale_feed_disarms_gateway_blocks_replace() {
    let cfg = stale_cfg();
    let mut st = IntegrityState::new();

    tick_feed(&cfg, &mut st, &FeedId::new("feedA"), 10);
    tick_feed(&cfg, &mut st, &FeedId::new("feedB"), 10);
    tick_feed(&cfg, &mut st, &FeedId::new("feedA"), 16); // triggers disarm

    assert!(st.disarmed);

    let gw = BrokerGateway::new(OkBroker, IntegrityAdapter(st), AlwaysPass, AlwaysPass);
    // Gate fires before map lookup (EB-2); empty map is correct here.
    let err = gw
        .replace("b-ord-e2", &BrokerOrderMap::new(), 20, None, "day".to_string())
        .unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("error must downcast to GateRefusal");
    assert_eq!(*refusal, GateRefusal::IntegrityDisarmed);
}

/// Gap halt (not stale feed) also blocks the gateway.
///
/// Confirms that `is_execution_blocked()` covers both the `disarmed` and
/// `halted` flags, and that either produces `GateRefusal::IntegrityDisarmed`.
#[test]
fn gap_halt_disarms_gateway() {
    let cfg = IntegrityConfig {
        gap_tolerance_bars: 0,
        stale_threshold_ticks: 0, // stale disabled; test gap-halt path
        enforce_feed_disagreement: false,
        calendar: CalendarSpec::AlwaysOn,
    };
    let mut st = IntegrityState::new();
    let feed = FeedId::new("main");
    let tf = Timeframe::secs(60);

    // Bar 1: baseline, allowed.
    let bar1 = Bar::new(BarKey::new("SPY", tf, 1000), true, 500_000_000, 100);
    evaluate_bar(&cfg, &mut st, &feed, 1, &bar1);
    assert!(!st.is_execution_blocked());

    // Bar 2: end_ts = 1180 skips 2 bars (expected 1060); gap > tolerance 0 → HALT.
    let bar2 = Bar::new(BarKey::new("SPY", tf, 1180), true, 500_000_000, 100);
    evaluate_bar(&cfg, &mut st, &feed, 2, &bar2);
    assert!(st.halted, "halted flag must be set after gap");
    assert!(st.is_execution_blocked());

    let gw = BrokerGateway::new(OkBroker, IntegrityAdapter(st), AlwaysPass, AlwaysPass);
    let err = gw.submit(&make_claim(), submit_req()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::IntegrityDisarmed,
        "gap halt must block gateway with IntegrityDisarmed"
    );
}
