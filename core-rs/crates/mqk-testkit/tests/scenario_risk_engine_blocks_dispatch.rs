//! Scenario: Risk Engine Blocks Dispatch — Section G
//!
//! # Invariant under test
//!
//! `RiskDecisionAdapter` is the production bridge from `mqk-risk::evaluate()`
//! output to `BrokerGateway`'s `RiskGate` trait.  It proves that the gateway's
//! risk gate is satisfied by a real risk engine decision, not a boolean stub.
//!
//! End-to-end chain proven here:
//!
//!   mqk_risk::evaluate(cfg, &mut state, input) → RiskDecision { action }
//!       → RiskDecisionAdapter::is_allowed() = (action == RiskAction::Allow)
//!       → BrokerGateway::enforce_gates() — allows or returns GateRefusal::RiskBlocked
//!
//! Previously the chain was proven only in parts:
//!   - mqk-risk: evaluate() logic unit-tested inside mqk-risk crate
//!   - mqk-execution: gate refusal with BoolGate(false) stubs  (gateway unit tests)
//!
//! Section G closes the gap: real mqk-risk::evaluate() output wired into
//! BrokerGateway, proving the risk gate enforces actual engine decisions.
//!
//! # RiskDecisionAdapter
//!
//! Rust's orphan rule prevents implementing a foreign trait for a foreign type
//! outside their home crates.  `RiskDecisionAdapter(RiskDecision)` is a newtype
//! defined here that owns the `impl RiskGate`.  In production, an equivalent
//! adapter lives at the runtime orchestration wiring boundary.
//!
//! The adapter holds the most-recently evaluated `RiskDecision`.  The typical
//! production pattern is: evaluate risk once per dispatch tick, wrap the result
//! in the adapter, and wire it into the gateway before each submission attempt.
//!
//! All tests are pure in-process; no DB or network required.

use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerGateway, BrokerInvokeToken, BrokerOrderMap,
    BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
    GateRefusal, IntegrityGate, OutboxClaimToken, ReconcileGate, RiskGate,
};
use mqk_risk::{
    evaluate, KillSwitchEvent, KillSwitchType, PdtContext, RequestKind, RiskAction, RiskConfig,
    RiskDecision, RiskInput, RiskState,
};

// ---------------------------------------------------------------------------
// RiskDecisionAdapter: bridges RiskDecision → RiskGate (Section G)
// ---------------------------------------------------------------------------

/// Newtype that implements `RiskGate` for a pre-evaluated `RiskDecision`.
///
/// `is_allowed()` returns `true` ONLY when `decision.action == RiskAction::Allow`.
/// Any other action (`Reject`, `Halt`, `FlattenAndHalt`) is treated as denied.
///
/// This is fail-closed: an unknown or future `RiskAction` variant that is not
/// `Allow` produces `false` automatically via the `==` comparison.
struct RiskDecisionAdapter(RiskDecision);

impl RiskGate for RiskDecisionAdapter {
    fn is_allowed(&self) -> bool {
        self.0.action == RiskAction::Allow
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

    fn fetch_events(
        &self,
        _token: &BrokerInvokeToken,
    ) -> Result<Vec<mqk_execution::BrokerEvent>, Box<dyn std::error::Error>> {
        Ok(vec![])
    }
}

// ---------------------------------------------------------------------------
// Always-pass stubs for integrity and reconcile
// (held constant so risk is the only variable under test)
// ---------------------------------------------------------------------------

struct AlwaysArmed;
impl IntegrityGate for AlwaysArmed {
    fn is_armed(&self) -> bool {
        true
    }
}

struct AlwaysClean;
impl ReconcileGate for AlwaysClean {
    fn is_clean(&self) -> bool {
        true
    }
}

/// Always-deny reconcile stub — used ONLY to prove gate evaluation order.
/// When risk AND reconcile both deny, the first error must be RiskBlocked.
struct AlwaysDirty;
impl ReconcileGate for AlwaysDirty {
    fn is_clean(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn make_claim() -> OutboxClaimToken {
    OutboxClaimToken::for_test(1, "ord-g-risk")
}

fn submit_req() -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: "ord-g-risk".to_string(),
        symbol: "SPY".to_string(),
        quantity: 10,
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
    }
}

/// Risk config: $1 000 daily loss limit, no drawdown limit, PDT disabled.
fn cfg_with_daily_loss_limit() -> RiskConfig {
    RiskConfig {
        daily_loss_limit_micros: 1_000 * 1_000_000, // $1 000 limit
        max_drawdown_limit_micros: 0,
        reject_storm_max_rejects_in_window: 100,
        pdt_auto_enabled: false,
        missing_protective_stop_flattens: false,
    }
}

/// Risk config with no limits — everything allowed.
fn cfg_permissive() -> RiskConfig {
    RiskConfig {
        daily_loss_limit_micros: 0,
        max_drawdown_limit_micros: 0,
        reject_storm_max_rejects_in_window: 100,
        pdt_auto_enabled: false,
        missing_protective_stop_flattens: false,
    }
}

/// Standard healthy new-order input at the given equity level.
fn new_order_input(equity_micros: i64) -> RiskInput {
    RiskInput {
        day_id: 20240101,
        equity_micros,
        reject_window_id: 1,
        request: RequestKind::NewOrder,
        is_risk_reducing: false,
        pdt: PdtContext::ok(),
        kill_switch: None,
    }
}

// ---------------------------------------------------------------------------
// 1. Risk Allow decision permits submit
// ---------------------------------------------------------------------------

/// Risk engine returns Allow when equity is healthy and no limits are breached.
/// `RiskDecisionAdapter` converts that to `is_allowed() = true`.
/// Gateway allows the submit.
///
/// This is the healthy all-clear path for the risk gate.
#[test]
fn risk_allow_decision_permits_submit() {
    let cfg = cfg_permissive();
    let mut st = RiskState::new(20240101, 100_000 * 1_000_000, 1);
    let inp = new_order_input(100_000 * 1_000_000);

    let decision = evaluate(&cfg, &mut st, &inp);
    assert_eq!(
        decision.action,
        RiskAction::Allow,
        "risk engine must return Allow when equity is healthy and no limits breached"
    );

    let gw = BrokerGateway::for_test(
        OkBroker,
        AlwaysArmed,
        RiskDecisionAdapter(decision),
        AlwaysClean,
    );
    let result = gw.submit(&make_claim(), submit_req());
    assert!(
        result.is_ok(),
        "gateway must allow submit when real risk engine decision is Allow"
    );
}

// ---------------------------------------------------------------------------
// 2. Daily loss limit breach → Halt → submit blocked
// ---------------------------------------------------------------------------

/// When daily loss limit is breached, `evaluate()` returns Halt.
/// `RiskDecisionAdapter::is_allowed()` returns `false`.
/// Gateway refuses with `GateRefusal::RiskBlocked`.
///
/// This is the primary Section G proof: real risk engine blocking real dispatch.
#[test]
fn daily_loss_limit_breach_blocks_submit() {
    let cfg = cfg_with_daily_loss_limit(); // $1 000 daily loss limit
    let start_equity = 100_000 * 1_000_000_i64; // $100 000 day-start equity
    let mut st = RiskState::new(20240101, start_equity, 1);

    // Equity dropped $1 001 below day-start → exceeds $1 000 daily loss limit.
    let current_equity = start_equity - 1_001 * 1_000_000;
    let inp = new_order_input(current_equity);

    let decision = evaluate(&cfg, &mut st, &inp);
    assert_ne!(
        decision.action,
        RiskAction::Allow,
        "daily loss limit breach must produce a non-Allow decision"
    );

    let gw = BrokerGateway::for_test(
        OkBroker,
        AlwaysArmed,
        RiskDecisionAdapter(decision),
        AlwaysClean,
    );
    let err = gw.submit(&make_claim(), submit_req()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::RiskBlocked,
        "daily loss limit breach must block gateway with RiskBlocked"
    );
}

// ---------------------------------------------------------------------------
// 3. Kill switch → FlattenAndHalt → submit blocked
// ---------------------------------------------------------------------------

/// A Manual kill switch forces `evaluate()` to return `FlattenAndHalt`.
/// `RiskDecisionAdapter` treats all non-Allow actions as denied.
/// Gateway refuses with `GateRefusal::RiskBlocked`.
#[test]
fn kill_switch_blocks_submit() {
    let cfg = cfg_permissive();
    let mut st = RiskState::new(20240101, 100_000 * 1_000_000, 1);

    let inp = RiskInput {
        day_id: 20240101,
        equity_micros: 100_000 * 1_000_000,
        reject_window_id: 1,
        request: RequestKind::NewOrder,
        is_risk_reducing: false,
        pdt: PdtContext::ok(),
        kill_switch: Some(KillSwitchEvent::new(KillSwitchType::Manual)),
    };

    let decision = evaluate(&cfg, &mut st, &inp);
    assert_ne!(
        decision.action,
        RiskAction::Allow,
        "kill switch event must produce a non-Allow decision"
    );

    let gw = BrokerGateway::for_test(
        OkBroker,
        AlwaysArmed,
        RiskDecisionAdapter(decision),
        AlwaysClean,
    );
    let err = gw.submit(&make_claim(), submit_req()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::RiskBlocked,
        "kill switch must block gateway with RiskBlocked"
    );
}

// ---------------------------------------------------------------------------
// 4. Gate evaluation order: risk is checked before reconcile
// ---------------------------------------------------------------------------

/// When both risk denies AND reconcile is dirty, the returned error MUST be
/// `GateRefusal::RiskBlocked` — not `ReconcileNotClean`.
///
/// This proves the enforced gate evaluation order:
///   1. IntegrityGate  (AlwaysArmed here)
///   2. RiskGate       (RiskDecisionAdapter — non-Allow → returns RiskBlocked here)
///   3. ReconcileGate  (AlwaysDirty — never reached)
#[test]
fn gate_order_risk_is_evaluated_before_reconcile() {
    let cfg = cfg_permissive();
    let mut st = RiskState::new(20240101, 100_000 * 1_000_000, 1);

    // Force risk to deny via kill switch.
    let inp = RiskInput {
        day_id: 20240101,
        equity_micros: 100_000 * 1_000_000,
        reject_window_id: 1,
        request: RequestKind::NewOrder,
        is_risk_reducing: false,
        pdt: PdtContext::ok(),
        kill_switch: Some(KillSwitchEvent::new(KillSwitchType::Manual)),
    };

    let decision = evaluate(&cfg, &mut st, &inp);
    assert_ne!(decision.action, RiskAction::Allow, "must be non-Allow");

    // Wire: integrity=pass, risk=deny (real engine), reconcile=deny (AlwaysDirty).
    // The first error must be RiskBlocked — reconcile is evaluated AFTER risk.
    let gw = BrokerGateway::for_test(
        OkBroker,
        AlwaysArmed,
        RiskDecisionAdapter(decision),
        AlwaysDirty,
    );

    let err = gw.submit(&make_claim(), submit_req()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::RiskBlocked,
        "risk must be evaluated before reconcile: when both deny, first error must be RiskBlocked"
    );
}

// ---------------------------------------------------------------------------
// 5. Risk denial blocks cancel
// ---------------------------------------------------------------------------

/// The risk gate is not bypassed for cancel operations.
/// A non-Allow risk decision blocks cancel with `GateRefusal::RiskBlocked`.
///
/// Cancel and replace must obey the same gate policy as submit (Section G, G4).
#[test]
fn risk_denial_blocks_cancel() {
    let cfg = cfg_permissive();
    let mut st = RiskState::new(20240101, 100_000 * 1_000_000, 1);

    let inp = RiskInput {
        day_id: 20240101,
        equity_micros: 100_000 * 1_000_000,
        reject_window_id: 1,
        request: RequestKind::NewOrder,
        is_risk_reducing: false,
        pdt: PdtContext::ok(),
        kill_switch: Some(KillSwitchEvent::new(KillSwitchType::Manual)),
    };

    let decision = evaluate(&cfg, &mut st, &inp);
    assert_ne!(decision.action, RiskAction::Allow, "must be non-Allow");

    let gw = BrokerGateway::for_test(
        OkBroker,
        AlwaysArmed,
        RiskDecisionAdapter(decision),
        AlwaysClean,
    );
    // Gate fires before map lookup (EB-2); empty map is correct here.
    let err = gw.cancel("ord-g-risk", &BrokerOrderMap::new()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::RiskBlocked,
        "risk denial must block cancel with RiskBlocked"
    );
}

// ---------------------------------------------------------------------------
// 6. Risk denial blocks replace
// ---------------------------------------------------------------------------

/// The risk gate is not bypassed for replace operations.
/// A non-Allow risk decision blocks replace with `GateRefusal::RiskBlocked`.
#[test]
fn risk_denial_blocks_replace() {
    let cfg = cfg_permissive();
    let mut st = RiskState::new(20240101, 100_000 * 1_000_000, 1);

    let inp = RiskInput {
        day_id: 20240101,
        equity_micros: 100_000 * 1_000_000,
        reject_window_id: 1,
        request: RequestKind::NewOrder,
        is_risk_reducing: false,
        pdt: PdtContext::ok(),
        kill_switch: Some(KillSwitchEvent::new(KillSwitchType::Manual)),
    };

    let decision = evaluate(&cfg, &mut st, &inp);
    assert_ne!(decision.action, RiskAction::Allow, "must be non-Allow");

    let gw = BrokerGateway::for_test(
        OkBroker,
        AlwaysArmed,
        RiskDecisionAdapter(decision),
        AlwaysClean,
    );
    // Gate fires before map lookup (EB-2); empty map is correct here.
    let err = gw
        .replace(
            "ord-g-risk",
            &BrokerOrderMap::new(),
            20,
            None,
            "day".to_string(),
        )
        .unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::RiskBlocked,
        "risk denial must block replace with RiskBlocked"
    );
}

// ---------------------------------------------------------------------------
// 7. Sticky halt state blocks new orders
// ---------------------------------------------------------------------------

/// Once `RiskState::halted = true`, subsequent `evaluate()` calls return
/// `Reject` for any `NewOrder` request (sticky halt).
///
/// Proves the sticky halt property of the risk engine is honoured at the
/// gateway choke-point: a previously halted risk state cannot be bypassed
/// by re-submitting.
#[test]
fn sticky_halt_state_blocks_new_order_submit() {
    let cfg = cfg_permissive();
    let mut st = RiskState::new(20240101, 100_000 * 1_000_000, 1);
    // Directly set halted — simulates state after a prior kill switch / limit breach.
    st.halted = true;

    let inp = new_order_input(100_000 * 1_000_000);
    let decision = evaluate(&cfg, &mut st, &inp);
    assert_ne!(
        decision.action,
        RiskAction::Allow,
        "halted risk state must return non-Allow for NewOrder"
    );

    let gw = BrokerGateway::for_test(
        OkBroker,
        AlwaysArmed,
        RiskDecisionAdapter(decision),
        AlwaysClean,
    );
    let err = gw.submit(&make_claim(), submit_req()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::RiskBlocked,
        "sticky halted risk state must block gateway with RiskBlocked"
    );
}
