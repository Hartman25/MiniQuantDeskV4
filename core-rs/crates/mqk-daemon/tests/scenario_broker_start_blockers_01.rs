//! Preflight and autonomous-readiness report the same start-refusing broker
//! blockers the start path enforces (Paper endpoint identity, fresh provider
//! denial). One sequential test: it mutates `ALPACA_PAPER_BASE_URL`, so it owns
//! this binary's process environment.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_broker_alpaca::AccountEntitlementEvidence;
use mqk_daemon::{routes, state};
use serde_json::{json, Value};
use tower::ServiceExt;

const ENV: &str = "ALPACA_PAPER_BASE_URL";

async fn get(st: &Arc<state::AppState>, uri: &str) -> Value {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = routes::build_router(Arc::clone(st)).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

fn mentions(v: &Value, key: &str, needle: &str) -> bool {
    v[key]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b.as_str().unwrap_or_default().contains(needle))
}

async fn both(st: &Arc<state::AppState>) -> (Value, Value) {
    (
        get(st, "/api/v1/system/preflight").await,
        get(st, "/api/v1/autonomous/readiness").await,
    )
}

#[tokio::test]
async fn broker_environment_start_refusals_are_reported_by_both_readiness_routes() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    // Baseline: no override, no evidence => neither route reports a broker blocker.
    std::env::remove_var(ENV);
    let (pf, ar) = both(&st).await;
    assert!(!mentions(&pf, "blockers", ENV));
    assert!(!mentions(&ar, "blockers", ENV));

    // Paper pointed at the live host: both routes block, naming the override.
    std::env::set_var(ENV, "https://api.alpaca.markets");
    let (pf, ar) = both(&st).await;
    assert!(mentions(&pf, "blockers", ENV), "preflight: {pf}");
    assert!(mentions(&ar, "blockers", ENV), "autonomous readiness: {ar}");
    assert_eq!(ar["overall_ready"], false);

    // Paper host and loopback mocks are accepted.
    for ok in ["https://paper-api.alpaca.markets", "http://127.0.0.1:9"] {
        std::env::set_var(ENV, ok);
        let (pf, ar) = both(&st).await;
        assert!(!mentions(&pf, "blockers", ENV), "{ok}: {pf}");
        assert!(!mentions(&ar, "blockers", ENV), "{ok}: {ar}");
    }
    std::env::remove_var(ENV);

    // Fresh provider denial blocks both routes; entitled evidence does not.
    let account = |blocked: bool| {
        AccountEntitlementEvidence::from_account_json(&json!({
            "id": "904837e3-3b76-47ec-b432-046db621571b",
            "status": "ACTIVE",
            "trading_blocked": blocked,
            "account_blocked": false,
            "trade_suspended_by_user": false
        }))
    };
    st.broker_account_evidence.observe(account(false), Utc::now());
    let (pf, ar) = both(&st).await;
    assert!(!mentions(&pf, "blockers", "entitlement is denied"), "{pf}");
    assert!(!mentions(&ar, "blockers", "entitlement is denied"), "{ar}");

    st.broker_account_evidence.observe(account(true), Utc::now());
    let (pf, ar) = both(&st).await;
    assert!(mentions(&pf, "blockers", "account_trading_blocked"), "{pf}");
    assert!(mentions(&ar, "blockers", "account_trading_blocked"), "{ar}");
    assert_eq!(ar["overall_ready"], false);
}
