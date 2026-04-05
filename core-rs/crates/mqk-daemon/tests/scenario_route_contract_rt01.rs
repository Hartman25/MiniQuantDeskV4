//! ROUTE-TRUTH-01 — GUI probe manifest completeness gate.
//!
//! This test is the authoritative contract between the GUI's batch probe
//! manifest (fetchOperatorModel in api.ts) and the daemon's mounted routes.
//!
//! ## Purpose
//!
//! Prevent silent drift: any GUI-probed GET route that is not mounted on
//! the daemon will return 404 and this test fails CI.
//!
//! ## What is NOT in the manifest
//!
//! The following routes are intentionally absent — they are known-unmounted
//! and the GUI uses `notProbed()` stubs instead of live HTTP probes:
//!
//! | Route                                    | Status                    |
//! |------------------------------------------|---------------------------|
//! | `/api/v1/system/topology`                | not yet implemented       |
//! | `/api/v1/execution/transport`            | not yet implemented       |
//! | `/api/v1/incidents`                      | not yet implemented       |
//! | `/api/v1/execution/replace-cancel-chains`| not yet implemented       |
//! | `/api/v1/alerts/triage`                  | not yet implemented       |
//! | `/api/v1/market-data/quality`            | not yet implemented       |
//! | `/api/v1/execution/timeline/{id}`        | per-order detail, not mounted |
//! | `/api/v1/execution/trace/{id}`           | per-order detail, not mounted |
//! | `/api/v1/execution/replay/{id}`          | per-order detail, not mounted |
//! | `/api/v1/execution/chart/{id}`           | per-order detail, not mounted |
//! | `/api/v1/execution/causality/{id}`       | per-order detail, not mounted |
//!
//! POST/operator routes (ops/action, strategy/signal) require auth and are
//! tested separately in the contract gate.
//!
//! ## CI command
//!
//! ```sh
//! cargo test -p mqk-daemon --test scenario_route_contract_rt01
//! ```

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

fn make_router() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

async fn call_status(router: axum::Router, uri: &str) -> StatusCode {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    // Drain body so the connection is properly released.
    let status = resp.status();
    resp.into_body()
        .collect()
        .await
        .expect("body collect failed");
    status
}

/// Complete list of GET routes the GUI batch-probes in fetchOperatorModel.
///
/// Every entry MUST be mounted on the daemon. A 404 here means the GUI is
/// silently probing an unmounted route — which this test catches in CI.
///
/// Sort order matches the fetchOperatorModel probe sequence in api.ts for
/// easy cross-reference.
const GUI_PROBE_MANIFEST: &[&str] = &[
    // --- System surfaces ---
    "/api/v1/system/status",
    "/api/v1/system/metadata",
    "/api/v1/system/preflight",
    "/api/v1/system/session",
    "/api/v1/system/config-fingerprint",
    "/api/v1/system/config-diffs",
    "/api/v1/system/runtime-leadership",
    "/api/v1/system/artifact-intake",
    "/api/v1/system/run-artifact",
    "/api/v1/system/parity-evidence",
    // --- Execution surfaces ---
    "/api/v1/execution/summary",
    "/api/v1/execution/orders",
    "/api/v1/execution/outbox",
    "/api/v1/execution/fill-quality",
    // --- OMS and metrics ---
    "/api/v1/oms/overview",
    "/api/v1/metrics/dashboards",
    // --- Portfolio surfaces ---
    "/api/v1/portfolio/summary",
    "/api/v1/portfolio/positions",
    "/api/v1/portfolio/orders/open",
    "/api/v1/portfolio/fills",
    // --- Risk and reconcile ---
    "/api/v1/risk/summary",
    "/api/v1/risk/denials",
    "/api/v1/reconcile/status",
    "/api/v1/reconcile/mismatches",
    // --- Strategy surfaces ---
    "/api/v1/strategy/summary",
    "/api/v1/strategy/suppressions",
    // --- Audit and ops ---
    "/api/v1/audit/operator-actions",
    "/api/v1/audit/artifacts",
    "/api/v1/ops/operator-timeline",
    "/api/v1/ops/catalog",
    "/api/v1/ops/mode-change-guidance",
    // --- Alerts and events ---
    "/api/v1/alerts/active",
    "/api/v1/events/feed",
    // --- Paper journal ---
    "/api/v1/paper/journal",
    // --- Autonomous readiness ---
    "/api/v1/autonomous/readiness",
    // --- Legacy fallback routes (GUI falls through to these on canonical 404) ---
    "/v1/status",
    "/v1/trading/account",
    "/v1/trading/positions",
    "/v1/trading/orders",
    "/v1/trading/fills",
];

/// Every route in the GUI probe manifest must NOT return 404.
///
/// - 200 = mounted and healthy.
/// - 503 = mounted but truth unavailable (OMS no-snapshot, etc.) — correct
///   fail-closed behaviour, not a missing route.
/// - 404 = route not mounted → CI fails.
///
/// This test fails CI if any manifest route is unmounted, preventing silent
/// GUI/daemon contract drift.
#[tokio::test]
async fn rt01_all_gui_probed_routes_are_mounted() {
    let router = make_router();

    for &uri in GUI_PROBE_MANIFEST {
        let status = call_status(router.clone(), uri).await;
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "ROUTE-TRUTH-01: GUI probe manifest route '{uri}' returned 404 — route is not mounted \
             on the daemon. Either mount the route or remove it from the GUI probe manifest \
             (api.ts) and move it to the notProbed() list."
        );
    }
}

/// Verify the 6 known-unmounted routes return 404 so the test above
/// correctly excludes them. If a future patch mounts one of these, this
/// assertion will fail and the developer must move it into GUI_PROBE_MANIFEST
/// and update the GUI's notProbed() stub to a real fetchJsonCandidates call.
#[tokio::test]
async fn rt01_known_not_mounted_routes_stay_404_until_explicitly_promoted() {
    let router = make_router();

    const NOT_MOUNTED: &[&str] = &[
        "/api/v1/system/topology",
        "/api/v1/execution/transport",
        "/api/v1/incidents",
        "/api/v1/execution/replace-cancel-chains",
        "/api/v1/alerts/triage",
        "/api/v1/market-data/quality",
    ];

    for &uri in NOT_MOUNTED {
        let status = call_status(router.clone(), uri).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "ROUTE-TRUTH-01: previously-deferred route '{uri}' is now mounted (returned {status}). \
             Promote it: (1) move from notProbed() to fetchJsonCandidates in api.ts, \
             (2) move from NOT_MOUNTED to GUI_PROBE_MANIFEST in this test, \
             (3) add contract proof to scenario_gui_daemon_contract_gate.rs, \
             (4) update gui_daemon_contract_waivers.md."
        );
    }
}
