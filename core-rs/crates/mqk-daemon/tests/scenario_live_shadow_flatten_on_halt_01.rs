//! LIVE-FLATTEN-PROOF-01 — LiveShadow-mode flatten-on-halt proof.
//!
//! Proof-only patch (`POST-WAVE06-LEDGER-BURN-01` W1-13): no production code
//! change is made by this file. `RISK-FLATTEN-ON-HALT-01`'s
//! `RiskRequestContext`/`BrokerGateway::submit_with_context` mechanism
//! (`mqk-execution`/`mqk-runtime`) has never been exercised end to end
//! against a LiveShadow-representative broker adapter — only the unrelated
//! `flatten-paper-positions` HTTP action route (`scenario_paper_flatten_
//! psf01.rs`) has scenario coverage, and that route never touches
//! `is_risk_reducing`/`submit_with_context` at all (it submits a normal
//! outbox-claimed order through the ordinary dispatch path).
//!
//! ## Proof matrix
//!
//! | Test  | Scope | What it proves                                                |
//! |-------|-------|----------------------------------------------------------------|
//! | LSF-01 | pure | The operator `flatten-paper-positions` HTTP action is hardcoded Paper-only: LiveShadow mode gets `403 not_paper_mode`, the same as any other non-Paper mode — no LiveShadow-specific flatten action route exists today. Documents a real, deterministic finding (see the new follow-on row this surfaced). |
//! | LSF-02 | pure | The broker-generic `RiskRequestContext.is_risk_reducing` bypass genuinely works end to end through the REAL `mqk_broker_alpaca::AlpacaBrokerAdapter` (hermetic httpmock, standing in for LiveShadow's live-base-URL Alpaca connection — zero real network) wrapped in the REAL `BrokerGateway`/`RuntimeRiskGate`: a sticky max-drawdown halt denies a normal `submit()` before the broker is invoked, but a `submit_with_context` call marked `is_risk_reducing: true` is allowed through and reaches the (mocked) Alpaca order-submit endpoint successfully. |
//!
//! Both tests are pure in-process (no DB required, no real network call).

use std::sync::atomic::Ordering;
use std::sync::{atomic::AtomicI64, Arc, Mutex};

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use chrono::{DateTime, Utc};
use tower::ServiceExt;

use mqk_broker_alpaca::{AlpacaBrokerAdapter, AlpacaConfig};
use mqk_daemon::{
    routes::build_router,
    state::{AppState, DeploymentMode},
};
use mqk_execution::gateway::{BrokerGateway, IntegrityGate, ReconcileGate, RiskRequestContext};
use mqk_execution::{
    AssetClass, BrokerSubmitRequest, GateRefusal, OutboxClaimToken, Side, SubmitError,
};
use mqk_runtime::runtime_risk::{
    AccountAuthorityContext, AccountAuthorityError, RuntimeAccountAuthority, RuntimeClock,
    RuntimeRiskGate,
};

// ---------------------------------------------------------------------------
// LSF-01 — operator flatten action route has no LiveShadow support
// ---------------------------------------------------------------------------

async fn call_flatten(router: axum::Router, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .header("authorization", "Bearer test-token")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

// LSF-01: LiveShadow mode requesting flatten-paper-positions gets the exact
// same not_paper_mode refusal as any other non-Paper mode -- there is no
// LiveShadow-specific flatten action wired anywhere in the daemon today.
#[tokio::test]
async fn lsf01_live_shadow_flatten_action_route_is_paper_only_not_wired() {
    let st = Arc::new(AppState::new_for_test_with_mode(DeploymentMode::LiveShadow));
    let router = build_router(st);

    let (status, json) = call_flatten(
        router,
        serde_json::json!({ "action_key": "flatten-paper-positions" }),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json["disposition"], "not_paper_mode");
}

// ---------------------------------------------------------------------------
// LSF-02 — flatten-on-halt via the REAL Alpaca adapter (hermetic httpmock)
// ---------------------------------------------------------------------------

struct TestClock(Mutex<DateTime<Utc>>);
impl TestClock {
    fn new(t: DateTime<Utc>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(t)))
    }
    fn set(&self, t: DateTime<Utc>) {
        *self.0.lock().unwrap() = t;
    }
}
impl RuntimeClock for TestClock {
    fn now_utc(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

/// Mirrors `mqk-runtime`'s own e2e proof's `TestAccountAuthority` (see
/// `scenario_runtime_risk_dynamic_authority_e2e_01.rs`) -- a settable,
/// always-fresh account authority so this test can drive a genuine
/// max-drawdown breach without depending on any daemon-internal type.
struct TestAccountAuthority {
    equity_micros: AtomicI64,
}
impl TestAccountAuthority {
    fn new(equity_micros: i64) -> Arc<Self> {
        Arc::new(Self {
            equity_micros: AtomicI64::new(equity_micros),
        })
    }
    fn set_equity(&self, equity_micros: i64) {
        self.equity_micros.store(equity_micros, Ordering::SeqCst);
    }
}
impl RuntimeAccountAuthority for TestAccountAuthority {
    fn current_account(
        &self,
        _now: DateTime<Utc>,
    ) -> Result<AccountAuthorityContext, AccountAuthorityError> {
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

fn submit_req(order_id: &str) -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: order_id.to_string(),
        symbol: "AAPL".to_string(),
        side: Side::Sell,
        quantity: 5,
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
        asset_class: AssetClass::Equity,
    }
}

fn claim(id: &str) -> OutboxClaimToken {
    OutboxClaimToken::for_test(1, id)
}

// LSF-02: a sticky max-drawdown halt denies a normal order BEFORE the broker
// is invoked, but a request marked is_risk_reducing (a flatten/close order)
// bypasses the halt and reaches the real Alpaca adapter's HTTP layer
// (hermetic httpmock -- zero real network), proving flatten-on-halt is
// genuinely broker-kind-generic and works through the actual Alpaca adapter
// type LiveShadow mode would use, not just a hermetic stub broker.
#[test]
fn lsf02_flatten_on_halt_bypasses_sticky_max_drawdown_halt_via_real_alpaca_adapter() {
    use httpmock::prelude::*;

    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST).path("/v2/orders");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "id": "alpaca-order-lsf02",
                "client_order_id": "flatten-order",
                "created_at": "2024-01-15T09:04:00Z",
            }));
    });

    let clock = TestClock::new(chrono::TimeZone::with_ymd_and_hms(&Utc, 2024, 1, 15, 9, 0, 0).unwrap());
    let account = TestAccountAuthority::new(120_000 * 1_000_000);
    let risk_gate = RuntimeRiskGate::from_run_config_with_account_authority(
        &serde_json::json!({ "risk": { "daily_loss_limit": 0.50, "max_drawdown": 0.10 } }),
        account.clone(),
        clock.clone(),
    );

    // AlpacaBrokerAdapter standing in for LiveShadow's real Alpaca-live-
    // base-URL connection -- hermetic: base_url points at the local mock
    // server, never a real Alpaca host.
    let broker = AlpacaBrokerAdapter::new(AlpacaConfig {
        base_url: server.base_url(),
        api_key_id: "test-key".to_string(),
        api_secret_key: "test-secret".to_string(),
    });
    let gateway = BrokerGateway::for_test(broker, PassGate, risk_gate, PassGate);

    // Establish a peak, then breach the configured 10% max-drawdown
    // (peak 120k -> floor 108k; 107_999 breaches it) -- mirrors
    // mqk-runtime's own CASE 2 proof, reused here against a real adapter.
    clock.set(chrono::TimeZone::with_ymd_and_hms(&Utc, 2024, 1, 15, 9, 1, 0).unwrap());
    account.set_equity(107_999 * 1_000_000);

    let denied = gateway.submit(&claim("normal-order"), submit_req("normal-order"));
    assert!(
        matches!(
            denied,
            Err(SubmitError::Gate(GateRefusal::RiskBlocked(_)))
        ),
        "a normal order during a max-drawdown breach must be denied by the risk gate: {denied:?}"
    );
    mock.assert_hits(0);

    let flattened = gateway.submit_with_context(
        &claim("flatten-order"),
        submit_req("flatten-order"),
        RiskRequestContext {
            is_risk_reducing: true,
        },
    );
    assert!(
        flattened.is_ok(),
        "a risk-reducing flatten order must bypass the sticky max-drawdown halt: {flattened:?}"
    );
    assert_eq!(
        flattened.unwrap().broker_order_id,
        "alpaca-order-lsf02",
        "the flatten order must have actually reached the real Alpaca adapter's HTTP layer"
    );
    mock.assert_hits(1);
}
