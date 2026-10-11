//! Preflight and autonomous readiness report the same start-refusing broker
//! blockers the start path enforces: Paper endpoint identity, missing or
//! malformed credentials, and a fresh provider denial. One sequential test: it
//! mutates the process environment, so it owns this binary's environment.
//! Credential VALUES are dummy tokens and must never appear in any response.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_broker_alpaca::AccountEntitlementEvidence;
use mqk_daemon::{routes, state};
use serde_json::{json, Value};
use tower::ServiceExt;

const ENV: &str = "ALPACA_PAPER_BASE_URL";
const KEY: &str = "ALPACA_API_KEY_PAPER";
const SECRET: &str = "ALPACA_API_SECRET_PAPER";
const GOOD_KEY: &str = "PKDUMMYKEY0001";
const GOOD_SECRET: &str = "DUMMYSECRET0001";

async fn get(st: &Arc<state::AppState>, uri: &str) -> Value {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = routes::build_router(Arc::clone(st))
        .oneshot(req)
        .await
        .unwrap();
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

fn structured_codes(pf: &Value) -> Vec<String> {
    pf["broker_start_blockers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["code"].as_str().unwrap().to_string())
        .collect()
}

async fn both(st: &Arc<state::AppState>) -> (Value, Value) {
    (
        get(st, "/api/v1/system/preflight").await,
        get(st, "/api/v1/autonomous/readiness").await,
    )
}

fn set_credentials(key: Option<&str>, secret: Option<&str>) {
    for (var, val) in [(KEY, key), (SECRET, secret)] {
        match val {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
    }
}

#[tokio::test]
async fn broker_environment_start_refusals_are_reported_by_both_readiness_routes() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    std::env::remove_var(ENV);

    // Valid prerequisites: no broker blocker, configuration present.
    set_credentials(Some(GOOD_KEY), Some(GOOD_SECRET));
    let (pf, ar) = both(&st).await;
    for needle in [ENV, KEY, SECRET] {
        assert!(!mentions(&pf, "blockers", needle), "{needle}: {pf}");
        assert!(!mentions(&ar, "blockers", needle), "{needle}: {ar}");
    }
    assert_eq!(pf["broker_config_present"], true);
    assert!(structured_codes(&pf).is_empty());

    // Missing / empty / malformed credentials: both routes block with an
    // actionable, value-free message and a structured code; config not present.
    for (label, key, secret, want_code, want_var) in [
        (
            "missing key",
            None,
            Some(GOOD_SECRET),
            "runtime.start_refused.alpaca_creds_missing",
            KEY,
        ),
        (
            "empty key",
            Some(""),
            Some(GOOD_SECRET),
            "runtime.start_refused.alpaca_creds_missing",
            KEY,
        ),
        (
            "whitespace key",
            Some("   "),
            Some(GOOD_SECRET),
            "runtime.start_refused.alpaca_creds_missing",
            KEY,
        ),
        (
            "missing secret",
            Some(GOOD_KEY),
            None,
            "runtime.start_refused.alpaca_creds_missing",
            SECRET,
        ),
        (
            "malformed key",
            Some("bad key value"),
            Some(GOOD_SECRET),
            "runtime.start_refused.alpaca_creds_malformed",
            KEY,
        ),
        (
            "malformed secret",
            Some(GOOD_KEY),
            Some("bad\tsecret"),
            "runtime.start_refused.alpaca_creds_malformed",
            SECRET,
        ),
    ] {
        set_credentials(key, secret);
        let (pf, ar) = both(&st).await;
        assert!(mentions(&pf, "blockers", want_var), "{label}: {pf}");
        assert!(mentions(&ar, "blockers", want_var), "{label}: {ar}");
        assert!(
            structured_codes(&pf).contains(&want_code.to_string()),
            "{label}: {pf}"
        );
        assert_eq!(pf["broker_config_present"], false, "{label}");
        assert_eq!(ar["overall_ready"], false, "{label}");
        let text = format!("{pf}{ar}");
        for secret_value in ["bad key value", "bad\\tsecret", GOOD_KEY, GOOD_SECRET] {
            assert!(
                !text.contains(secret_value),
                "{label}: credential value leaked"
            );
        }
    }
    set_credentials(Some(GOOD_KEY), Some(GOOD_SECRET));

    // Paper pointed at the live host: both routes block, naming the override.
    std::env::set_var(ENV, "https://api.alpaca.markets");
    let (pf, ar) = both(&st).await;
    assert!(mentions(&pf, "blockers", ENV), "preflight: {pf}");
    assert!(mentions(&ar, "blockers", ENV), "autonomous readiness: {ar}");
    assert!(structured_codes(&pf)
        .contains(&"runtime.start_refused.alpaca_paper_base_url_not_paper".to_string()));
    assert_eq!(ar["overall_ready"], false);

    // The Paper host is accepted; a loopback mock is accepted ONLY because this
    // test build enables the hermetic authority (production refuses it).
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
    st.broker_account_evidence
        .observe(account(false), Utc::now());
    let (pf, ar) = both(&st).await;
    assert!(!mentions(&pf, "blockers", "entitlement is denied"), "{pf}");
    assert!(!mentions(&ar, "blockers", "entitlement is denied"), "{ar}");
    // fresh + entitled but unbound is NOT order-authorized readiness
    assert_eq!(pf["broker_account_entitlement"]["state"], "unbound");

    st.broker_account_evidence
        .observe(account(true), Utc::now());
    let (pf, ar) = both(&st).await;
    assert!(mentions(&pf, "blockers", "account_trading_blocked"), "{pf}");
    assert!(mentions(&ar, "blockers", "account_trading_blocked"), "{ar}");
    assert!(structured_codes(&pf).contains(&"account_trading_blocked".to_string()));
    assert_eq!(ar["overall_ready"], false);
}
