//! Preflight reports the SAME broker-account entitlement state the gateway
//! enforces at submit (shared `AccountEvidenceCell::readiness` -> `admit`).
//!
//! P01  No observation yet: state `not_observed`, a warning, never a blocker
//!      and never `entitled` (a restarted daemon proves nothing).
//! P02  Fresh entitled evidence: `entitled`, no entitlement blocker.
//! P03  Fresh provider denial (trading_blocked): `denied` and a blocker.
//! P04  Stale evidence: `stale`, warning only, never `entitled`.
//! P05  Unknown/malformed evidence: `unknown`, warning only.
//! P06  Identity drift against the run-bound account: `denied` + blocker.
//! P07  A deployment whose broker has no provider account (Paper broker
//!      kind) reports no entitlement object rather than a fabricated one.
//!
//! The evidence fed here is an in-process fixture for the wire shape of
//! GET /v2/account. It proves readiness/enforcement parity, not that any real
//! provider account is entitled.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use mqk_broker_alpaca::AccountEntitlementEvidence;
use mqk_daemon::{routes, state};
use serde_json::{json, Value};
use tower::ServiceExt;

fn active() -> Value {
    json!({
        "id": "904837e3-3b76-47ec-b432-046db621571b",
        "status": "ACTIVE",
        "trading_blocked": false,
        "account_blocked": false,
        "trade_suspended_by_user": false
    })
}

fn alpaca_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ))
}

async fn preflight(st: Arc<state::AppState>) -> Value {
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/preflight")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = routes::build_router(st).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

fn observe(st: &state::AppState, account: &Value, age_secs: i64) {
    st.broker_account_evidence.observe(
        AccountEntitlementEvidence::from_account_json(account),
        Utc::now() - Duration::seconds(age_secs),
    );
}

fn mentions(list: &Value, needle: &str) -> bool {
    list.as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str().unwrap_or_default().contains(needle))
}

#[tokio::test]
async fn p01_not_observed_is_warning_not_entitled_not_blocker() {
    let pf = preflight(alpaca_state()).await;
    assert_eq!(pf["broker_account_entitlement"]["state"], "not_observed");
    assert!(mentions(&pf["warnings"], "broker account entitlement"));
    assert!(!mentions(&pf["blockers"], "broker account entitlement"));
}

#[tokio::test]
async fn p02_fresh_entitled_evidence_is_entitled() {
    let st = alpaca_state();
    observe(&st, &active(), 1);
    let pf = preflight(st).await;
    let ent = &pf["broker_account_entitlement"];
    assert_eq!(ent["state"], "entitled");
    assert_eq!(ent["provider_account_id"], "904837e3-3b76-47ec-b432-046db621571b");
    assert!(ent["observed_at_utc"].is_string());
    assert!(!mentions(&pf["warnings"], "broker account entitlement"));
    assert!(!mentions(&pf["blockers"], "broker account entitlement"));
}

#[tokio::test]
async fn p03_fresh_provider_denial_is_a_blocker() {
    let st = alpaca_state();
    let mut a = active();
    a["trading_blocked"] = json!(true);
    observe(&st, &a, 1);
    let pf = preflight(st).await;
    assert_eq!(pf["broker_account_entitlement"]["state"], "denied");
    assert_eq!(pf["broker_account_entitlement"]["code"], "account_trading_blocked");
    assert!(mentions(&pf["blockers"], "account_trading_blocked"));
}

#[tokio::test]
async fn p04_stale_evidence_is_never_entitled() {
    let st = alpaca_state();
    observe(&st, &active(), 3600);
    let pf = preflight(st).await;
    assert_eq!(pf["broker_account_entitlement"]["state"], "stale");
    assert!(mentions(&pf["warnings"], "broker account entitlement"));
    assert!(!mentions(&pf["blockers"], "broker account entitlement"));
}

#[tokio::test]
async fn p05_unknown_fields_are_unknown_not_entitled() {
    let st = alpaca_state();
    let mut a = active();
    a.as_object_mut().unwrap().remove("trading_blocked");
    observe(&st, &a, 1);
    let pf = preflight(st).await;
    assert_eq!(pf["broker_account_entitlement"]["state"], "unknown");
    assert!(!mentions(&pf["blockers"], "broker account entitlement"));
}

#[tokio::test]
async fn p06_identity_drift_against_bound_account_is_denied() {
    let st = alpaca_state();
    st.broker_account_evidence
        .pin_provider_account_id("11111111-2222-3333-4444-555555555555");
    observe(&st, &active(), 1);
    let pf = preflight(st).await;
    assert_eq!(pf["broker_account_entitlement"]["state"], "denied");
    assert_eq!(pf["broker_account_entitlement"]["code"], "account_identity_drift");
    assert!(mentions(&pf["blockers"], "account_identity_drift"));
}

#[tokio::test]
async fn p07_broker_without_provider_account_reports_none() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Paper,
    ));
    let pf = preflight(st).await;
    assert!(pf["broker_account_entitlement"].is_null());
}
