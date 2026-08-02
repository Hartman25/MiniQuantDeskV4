//! REGISTRY-V2-GATE-PARITY-01C — Gate 0 parity against
//! `InstrumentRegistryV2::asset_class` strings.
//!
//! Prerequisite #3 of `ASSET-CORE-01H`'s production-cutover checklist
//! requires Gate 0 (`mqk-daemon::routes::strategy::validate_strategy_signal`,
//! B8) to be re-verified against `InstrumentRegistryV2::asset_class`
//! parity. This file proves that `mqk_md::instrument_registry_v2`'s pure
//! `registry_v2_gate_asset_class` helper (`REGISTRY-V2-GATE-PARITY-01B`)
//! classifies every v2 asset-class string identically to Gate 0's actual,
//! already-tested behavior (`scenario_asset_class_scope_b8.rs`, `AS-01`
//! through `AS-08`). Gate 0 itself is not modified — it still reads only
//! the request-body `asset_class` string, never `InstrumentRegistryV2`.
//!
//! ## Coverage
//!
//! GD-01  Every `CANONICAL_ASSET_CLASSES_V2` string: the helper's
//!        Equity/NonEquity classification matches Gate 0's actual
//!        pass/reject decision for the identical string sent as
//!        `StrategySignalRequest.asset_class`.
//! GD-02  Malformed/unknown v2-shaped strings (empty, unknown, `"etf"`,
//!        plural aliases `"futures"`/`"options"`) are rejected by both the
//!        helper (`Err`) and Gate 0 (400/`disposition: "rejected"`).
//! GD-03  Case/whitespace-normalized equity (`"EQUITY"`, `" equity "`)
//!        passes both the helper and Gate 0.
//! GD-04  No `CANONICAL_ASSET_CLASSES_V2` value other than `"equity"`
//!        passes Gate 0 — exhaustive non-equity rejection proof.
//!
//! All tests are pure in-process. No environment variables, DB, or network
//! required.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use mqk_md::instrument_registry_v2::{
    registry_v2_gate_asset_class, RegistryV2GateAssetClass, CANONICAL_ASSET_CLASSES_V2,
};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers (mirrors scenario_asset_class_scope_b8.rs)
// ---------------------------------------------------------------------------

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body is not valid JSON");
    (status, json)
}

fn signal_req(body: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

fn signal_with_ac(ac: &str) -> serde_json::Value {
    serde_json::json!({
        "signal_id":   "gd-parity-signal",
        "strategy_id": "strat-equity-01",
        "symbol":      "AAPL",
        "side":        "buy",
        "qty":         10,
        "asset_class": ac,
    })
}

/// Gate 0's actual decision for `ac`, derived the same way
/// `scenario_asset_class_scope_b8.rs`'s `AS-03`..`AS-08` do: 400 status with
/// `disposition: "rejected"` means Gate 0 blocked it; anything else means
/// Gate 0 let it through (a later gate may still reject for unrelated
/// reasons, exactly as `AS-01`/`AS-02` document).
async fn gate_0_rejects(ac: &str) -> bool {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);
    let (status, json) = call(router, signal_req(signal_with_ac(ac))).await;
    status == StatusCode::BAD_REQUEST && json["disposition"] == "rejected"
}

// ---------------------------------------------------------------------------
// GD-01: every canonical v2 asset class — helper decision matches Gate 0
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gd01_every_canonical_v2_asset_class_matches_gate_0_decision() {
    for class in CANONICAL_ASSET_CLASSES_V2 {
        let helper_allows = matches!(
            registry_v2_gate_asset_class(class),
            Ok(RegistryV2GateAssetClass::Equity)
        );
        let gate_0_rejected = gate_0_rejects(class).await;

        assert_eq!(
            helper_allows,
            !gate_0_rejected,
            "parity mismatch for asset_class={class}: helper_allows={helper_allows}, gate_0_rejected={gate_0_rejected}"
        );
    }
}

// ---------------------------------------------------------------------------
// GD-02: malformed/unknown v2-shaped strings rejected by both sides
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gd02_malformed_and_unknown_v2_strings_rejected_by_both() {
    for ac in ["", "stock", "perpetual_swap", "etf", "futures", "options"] {
        let helper_result = registry_v2_gate_asset_class(ac);
        assert!(
            helper_result.is_err(),
            "helper must fail closed for asset_class={ac:?}, got {helper_result:?}"
        );

        let gate_0_rejected = gate_0_rejects(ac).await;
        assert!(
            gate_0_rejected,
            "Gate 0 must reject asset_class={ac:?} (parity with helper's fail-closed Err)"
        );
    }
}

// ---------------------------------------------------------------------------
// GD-03: case/whitespace-normalized equity passes both
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gd03_case_and_whitespace_normalized_equity_passes_both() {
    for ac in ["EQUITY", " equity ", "Equity"] {
        assert_eq!(
            registry_v2_gate_asset_class(ac),
            Ok(RegistryV2GateAssetClass::Equity),
            "helper must classify {ac:?} as Equity"
        );
        assert!(
            !gate_0_rejects(ac).await,
            "Gate 0 must not reject normalized-equity asset_class={ac:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// GD-04: exhaustive — no non-"equity" canonical class passes Gate 0
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gd04_no_non_equity_canonical_class_passes_gate_0() {
    for class in CANONICAL_ASSET_CLASSES_V2 {
        if *class == "equity" {
            continue;
        }
        assert!(
            gate_0_rejects(class).await,
            "non-equity canonical class={class} must be rejected by Gate 0"
        );
        assert!(
            !matches!(
                registry_v2_gate_asset_class(class),
                Ok(RegistryV2GateAssetClass::Equity)
            ),
            "non-equity canonical class={class} must not classify as Equity in the helper"
        );
    }
}
