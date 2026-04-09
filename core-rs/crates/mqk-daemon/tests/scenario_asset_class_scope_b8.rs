//! B8: Multi-asset scaffold closure — explicit equity-only truth contract proof.
//!
//! ## What B8 closes
//!
//! Before B8 the canonical execution path had zero operator-visible surfaces
//! declaring that only US equities are supported.  Specifically:
//!
//!   - `POST /api/v1/strategy/signal` accepted any `symbol` string including
//!     OCC options notation, futures tickers, crypto pairs, and FX symbols.
//!     A signal with `symbol = "SPY240119C00500000"` would pass all gates and
//!     land in the outbox — broker rejection or silent mis-fill was the only
//!     backstop.
//!   - No operator-readable surface stated the asset-class boundary.  Reading
//!     `/api/v1/system/status` gave no indication that options, futures, crypto,
//!     or FX were unsupported.
//!
//! B8 adds two complementary closures:
//!
//! 1. **Gate 0 enforcement** — `validate_strategy_signal` now checks the
//!    optional `asset_class` field on `StrategySignalRequest`.  Any value other
//!    than `"equity"` (or absent, which implies equity) is rejected fail-closed
//!    with an explicit `"unsupported_asset_class"` blocker before the request
//!    reaches any other gate.
//!
//! 2. **Operator truth surface** — `GET /api/v1/system/status` now returns
//!    `asset_class_scope: "equity_only"`, a machine-readable constant that makes
//!    the asset-class boundary explicit.  Strategy tooling and operators can
//!    consult this field rather than relying on absence of evidence.
//!
//! ## Tests
//!
//! Gate 0 signal validation (pure — no AppState needed):
//! - AS-01: `asset_class` absent → passes Gate 0 (equity implied, backward-compat)
//! - AS-02: `asset_class: "equity"` → passes Gate 0 (explicit equity OK)
//! - AS-03: `asset_class: "option"` → rejected; blocker contains "not supported"
//! - AS-04: `asset_class: "future"` → rejected
//! - AS-05: `asset_class: "crypto"` → rejected
//! - AS-06: `asset_class: "fx"` → rejected
//! - AS-07: arbitrary unknown value → rejected (not just known non-equity types)
//! - AS-08: case-insensitive: `"EQUITY"` passes; `"Option"` is still rejected
//!
//! System-status surface:
//! - AS-09: `GET /api/v1/system/status` returns `asset_class_scope: "equity_only"`
//! - AS-10: `asset_class_scope` is a string, never null
//! - AS-11: value is stable across two independent calls (stateless constant)
//! - AS-12: adjacent B7 `corp_actions_screening` field unaffected (regression guard)
//!
//! All tests are pure in-process.  No environment variables or DB required.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    (status, body)
}

fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

fn signal_req(body: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

fn system_status_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap()
}

fn oms_overview_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/oms/overview")
        .body(axum::body::Body::empty())
        .unwrap()
}

/// Minimal valid signal body — no `asset_class` field.
fn base_signal() -> serde_json::Value {
    serde_json::json!({
        "signal_id":   "b8-test-signal",
        "strategy_id": "strat-equity-01",
        "symbol":      "AAPL",
        "side":        "buy",
        "qty":         10
    })
}

/// Base signal with `asset_class` set to the given value.
fn signal_with_ac(ac: &str) -> serde_json::Value {
    let mut v = base_signal();
    v["asset_class"] = serde_json::Value::String(ac.to_string());
    v
}

// ---------------------------------------------------------------------------
// AS-01: asset_class absent → Gate 0 passes (equity implied, backward-compat)
//
// Proves: adding the B8 gate did not break existing callers that never send
// an asset_class field.  The absence of the field is treated as equity-implied
// and must not be rejected at Gate 0.
//
// Note: the signal will be refused at a later gate (Gate 1: signal ingestion
// not configured on the default AppState).  We verify Gate 0 did NOT fire by
// confirming the rejection disposition is NOT "rejected" (which would indicate
// a field-validation failure) and the blocker does not mention asset_class.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn as_01_asset_class_absent_passes_gate_0() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, signal_req(base_signal())).await;
    let json = parse_json(body);

    // Gate 1 fires (ingestion not configured) — disposition is "unavailable", not "rejected".
    // If Gate 0 had fired, disposition would be "rejected" and status would be 400.
    assert_ne!(
        status,
        StatusCode::BAD_REQUEST,
        "AS-01: absent asset_class must not trigger Gate 0 (400); \
         got status={status} body={json}"
    );
    let disposition = json["disposition"].as_str().unwrap_or("");
    assert_ne!(
        disposition, "rejected",
        "AS-01: absent asset_class must not produce disposition='rejected'; \
         Gate 0 must not fire; got: {json}"
    );

    // No asset_class blocker in blockers array.
    let blockers = json["blockers"].as_array().cloned().unwrap_or_default();
    for b in &blockers {
        assert!(
            !b.as_str().unwrap_or("").contains("asset_class"),
            "AS-01: absent asset_class must produce no asset_class blockers; \
             found unexpected blocker: {b}"
        );
    }
}

// ---------------------------------------------------------------------------
// AS-02: asset_class "equity" → Gate 0 passes (explicit equity OK)
//
// Proves: strategy authors who explicitly declare asset_class="equity" pass
// through Gate 0 identically to the absent case.  This is the forward-compat
// path for callers that want to be explicit about their asset class.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn as_02_asset_class_equity_passes_gate_0() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, signal_req(signal_with_ac("equity"))).await;
    let json = parse_json(body);

    assert_ne!(
        status,
        StatusCode::BAD_REQUEST,
        "AS-02: explicit asset_class='equity' must not trigger Gate 0 (400); \
         got status={status} body={json}"
    );
    let disposition = json["disposition"].as_str().unwrap_or("");
    assert_ne!(
        disposition, "rejected",
        "AS-02: explicit asset_class='equity' must not produce disposition='rejected'; \
         got: {json}"
    );
}

// ---------------------------------------------------------------------------
// AS-03: asset_class "option" → rejected at Gate 0
//
// Proves: OCC-style options signals are rejected before reaching the outbox.
// Options have contract multipliers, Greeks, and exercise mechanics that the
// current execution, portfolio, and risk paths cannot handle.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn as_03_asset_class_option_rejected_at_gate_0() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, signal_req(signal_with_ac("option"))).await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "AS-03: asset_class='option' must be rejected at Gate 0 (400); \
         got status={status} body={json}"
    );
    assert_eq!(
        json["disposition"], "rejected",
        "AS-03: disposition must be 'rejected' for unsupported asset class; got: {json}"
    );

    let blockers = json["blockers"].as_array().cloned().unwrap_or_default();
    assert!(
        !blockers.is_empty(),
        "AS-03: blockers must be non-empty when asset_class is rejected; got: {json}"
    );
    let combined = blockers
        .iter()
        .map(|b| b.as_str().unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        combined.contains("not supported"),
        "AS-03: blocker must mention 'not supported'; got blockers: {blockers:?}"
    );
}

// ---------------------------------------------------------------------------
// AS-04: asset_class "future" → rejected at Gate 0
// ---------------------------------------------------------------------------

#[tokio::test]
async fn as_04_asset_class_future_rejected_at_gate_0() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, signal_req(signal_with_ac("future"))).await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "AS-04: asset_class='future' must be rejected at Gate 0 (400); body={json}"
    );
    assert_eq!(
        json["disposition"], "rejected",
        "AS-04: disposition must be 'rejected'; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// AS-05: asset_class "crypto" → rejected at Gate 0
// ---------------------------------------------------------------------------

#[tokio::test]
async fn as_05_asset_class_crypto_rejected_at_gate_0() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, signal_req(signal_with_ac("crypto"))).await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "AS-05: asset_class='crypto' must be rejected at Gate 0 (400); body={json}"
    );
    assert_eq!(
        json["disposition"], "rejected",
        "AS-05: disposition must be 'rejected'; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// AS-06: asset_class "fx" → rejected at Gate 0
// ---------------------------------------------------------------------------

#[tokio::test]
async fn as_06_asset_class_fx_rejected_at_gate_0() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, signal_req(signal_with_ac("fx"))).await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "AS-06: asset_class='fx' must be rejected at Gate 0 (400); body={json}"
    );
    assert_eq!(
        json["disposition"], "rejected",
        "AS-06: disposition must be 'rejected'; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// AS-07: Arbitrary unknown asset class → rejected
//
// Proves: the Gate 0 check is an allowlist ("equity" only), not a denylist of
// known non-equity types.  An unknown/misspelled value is also rejected.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn as_07_unknown_asset_class_rejected_at_gate_0() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, signal_req(signal_with_ac("perpetual_swap"))).await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "AS-07: unknown asset_class='perpetual_swap' must be rejected at Gate 0 (400); body={json}"
    );
    assert_eq!(
        json["disposition"], "rejected",
        "AS-07: disposition must be 'rejected'; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// AS-08: Gate 0 is case-insensitive — "EQUITY" passes, "Option" is rejected
//
// Proves: callers that uppercase the value for consistency are not penalised
// for equity, and case variations of unsupported classes are still blocked.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn as_08_gate_0_is_case_insensitive() {
    let st = Arc::new(state::AppState::new());

    // "EQUITY" (uppercased) must pass Gate 0.
    let router1 = routes::build_router(Arc::clone(&st));
    let (status1, body1) = call(router1, signal_req(signal_with_ac("EQUITY"))).await;
    let json1 = parse_json(body1);
    assert_ne!(
        status1,
        StatusCode::BAD_REQUEST,
        "AS-08a: 'EQUITY' (uppercase) must not trigger Gate 0 (400); body={json1}"
    );

    // "Option" (title-case) must still be rejected.
    let router2 = routes::build_router(Arc::clone(&st));
    let (status2, body2) = call(router2, signal_req(signal_with_ac("Option"))).await;
    let json2 = parse_json(body2);
    assert_eq!(
        status2,
        StatusCode::BAD_REQUEST,
        "AS-08b: 'Option' (title-case) must still be rejected at Gate 0 (400); body={json2}"
    );
    assert_eq!(
        json2["disposition"], "rejected",
        "AS-08b: disposition must be 'rejected' for 'Option'; got: {json2}"
    );
}

// ---------------------------------------------------------------------------
// AS-09: system/status returns asset_class_scope: "equity_only"
//
// Proves: the canonical operator status surface explicitly declares the
// asset-class boundary.  An operator reading this surface sees "equity_only"
// rather than silence about what is supported.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn as_09_system_status_asset_class_scope_is_equity_only() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, system_status_req()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "AS-09: /api/v1/system/status must return 200; got {status}\nbody: {}",
        String::from_utf8_lossy(&body)
    );

    let json = parse_json(body);
    assert_eq!(
        json["asset_class_scope"], "equity_only",
        "AS-09: system status must carry asset_class_scope='equity_only'; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// AS-10: asset_class_scope is a string, never null
//
// Proves: the field is always present and always a non-empty string.  A null
// or missing field could be interpreted as "unknown / possibly multi-asset",
// which would be a false operator impression.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn as_10_asset_class_scope_field_never_null() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (_, body) = call(router, system_status_req()).await;
    let json = parse_json(body);

    assert!(
        json["asset_class_scope"].is_string(),
        "AS-10: asset_class_scope must be a string, never null or missing; got: {json}"
    );
    assert!(
        !json["asset_class_scope"]
            .as_str()
            .unwrap_or("")
            .is_empty(),
        "AS-10: asset_class_scope must be non-empty; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// AS-11: asset_class_scope is stable across two independent calls
//
// Proves: the value is a stateless constant derived from execution-path
// capability, not from ephemeral runtime state.  Restarting the daemon or
// calling the endpoint twice must yield the same value.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn as_11_asset_class_scope_is_stateless_constant() {
    let st = Arc::new(state::AppState::new());

    let router1 = routes::build_router(Arc::clone(&st));
    let (_, body1) = call(router1, system_status_req()).await;
    let json1 = parse_json(body1);

    let router2 = routes::build_router(Arc::clone(&st));
    let (_, body2) = call(router2, system_status_req()).await;
    let json2 = parse_json(body2);

    assert_eq!(
        json1["asset_class_scope"], json2["asset_class_scope"],
        "AS-11: asset_class_scope must be identical across two calls; \
         first={}, second={}",
        json1["asset_class_scope"], json2["asset_class_scope"]
    );
    assert_eq!(
        json1["asset_class_scope"], "equity_only",
        "AS-11: both calls must return 'equity_only'; got: {}",
        json1["asset_class_scope"]
    );
}

// ---------------------------------------------------------------------------
// AS-12: B7 corp_actions_screening unaffected (regression guard)
//
// Proves: adding the B8 field did not alter any existing operator truth fields.
// B7's corp_actions_screening must still appear correctly on oms/overview.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn as_12_b7_corp_actions_screening_unaffected_by_b8() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, oms_overview_req()).await;
    assert_eq!(status, StatusCode::OK, "AS-12: oms/overview must return 200");

    let json = parse_json(body);
    assert_eq!(
        json["corp_actions_screening"], "not_wired",
        "AS-12: B7 corp_actions_screening must remain 'not_wired' after B8 patch; got: {json}"
    );
}
