//! MULTI-STRATEGY-DRY-RUN-STATUS-01: dry-run diagnostics status route proof tests.
//!
//! Proves `GET /api/v1/strategy/dry-run/status` exposes the latest dry-run
//! secondary-strategy diagnostic snapshot stored by `AppState`
//! (`state/dry_run_strategy.rs` evaluator + `state/loop_runner.rs` storage
//! call), without ever submitting an order or touching the outbox/broker.
//!
//! # Test inventory (mapped to the patch's numbered minimum requirements)
//!
//! | ID       | Requirement                                                          |
//! |----------|-----------------------------------------------------------------------|
//! | S01/S01b | (1) No dry-run strategies configured -> status diagnostics empty, even with a stale in-memory snapshot |
//! | S02_03   | (2)+(3) Snapshot can be stored and retrieved; surfaced `submitted=false` |
//! | S04      | (4) Short diagnostic exposes strategy id `intraday_short_scalper`   |
//! | S05      | (5) Short diagnostic exposes negative target qty on a bearish window |
//! | S06      | (6) Short diagnostic exposes `would_b5_block=true`                  |
//! | S07      | (7) Short diagnostic exposes default short-entry policy block reason |
//! | S08/S08b | (8) Snapshot replacement does not grow unbounded across ticks       |
//! | S09      | Route handler source contains no broker/outbox/submission call      |
//! | S11      | Response carries `canonical_route` / `backend`                      |
//! | S12      | Configured ids surfaced even before the first evaluation tick       |
//! | S13      | `evaluated_at_utc` is populated once a snapshot is stored            |
//!
//! Requirements (9)-(12) ("existing dry-run evaluator / B5 / short-entry-policy
//! / readiness tests still pass") are proved by running the pre-existing,
//! untouched test files in this patch's validation step
//! (`scenario_multi_strategy_runtime_dry_run_01`,
//! `scenario_native_strategy_b5_short_guard`,
//! `scenario_short_entry_policy_gates_01`, `scenario_short_side_intent_model_01`)
//! — not duplicated here.

use std::sync::{Arc, OnceLock};

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    api_types::DryRunStrategyStatusResponse,
    routes,
    state::{
        self, evaluate_dry_run_strategies, evaluate_dry_run_strategy, AppState,
        DRY_RUN_STRATEGY_IDS_ENV,
    },
};
use mqk_strategy::{BarStub, RecentBarsWindow};
use tokio::sync::Mutex;
use tower::ServiceExt;

const ROUTE: &str = "/api/v1/strategy/dry-run/status";

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvVarGuard {
    key: &'static str,
    prior: Option<String>,
}

impl EnvVarGuard {
    fn remove(key: &'static str) -> Self {
        let prior = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, prior }
    }

    fn set(key: &'static str, value: &str) -> Self {
        let prior = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, prior }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

fn bare_state() -> Arc<AppState> {
    Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ))
}

fn bar(close_micros: i64, is_complete: bool) -> BarStub {
    BarStub::new(0, is_complete, close_micros, 1)
}

/// Same shape as the bearish fixture in `scenario_multi_strategy_runtime_dry_run_01.rs`:
/// a -25 bps move, comfortably past the 20 bps signal threshold.
fn bearish_window() -> RecentBarsWindow {
    let base = 200_000_000i64; // $200
    RecentBarsWindow::new(
        10,
        vec![
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base, true),
        ],
    )
}

async fn request_status(st: Arc<AppState>) -> DryRunStrategyStatusResponse {
    let router = routes::build_router(st);
    let req = Request::builder()
        .method("GET")
        .uri(ROUTE)
        .body(axum::body::Body::empty())
        .expect("request");
    let resp = router.oneshot(req).await.expect("oneshot failed");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect")
        .to_bytes();
    serde_json::from_slice(&body).expect("dry-run status response json")
}

// ---------------------------------------------------------------------------
// (1) Not configured -> empty diagnostics; default behavior unchanged.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s01_not_configured_by_default_yields_empty_diagnostics() {
    let _lock = env_lock().lock().await;
    let _env = EnvVarGuard::remove(DRY_RUN_STRATEGY_IDS_ENV);
    let st = bare_state();

    let response = request_status(st).await;

    assert_eq!(response.truth_state, "not_configured");
    assert!(response.configured_dry_run_strategy_ids.is_empty());
    assert!(response.dry_run_strategy_diagnostics.is_empty());
}

#[tokio::test]
async fn s01b_not_configured_even_with_a_stale_in_memory_snapshot() {
    // Defensive: if a snapshot was stored while configured and the env var is
    // since cleared, the route must still report empty/not_configured — it
    // re-reads the env var fresh per request rather than trusting only the
    // in-memory snapshot left over from a prior configuration.
    let _lock = env_lock().lock().await;
    let _env = EnvVarGuard::remove(DRY_RUN_STRATEGY_IDS_ENV);
    let st = bare_state();
    let diag = evaluate_dry_run_strategy("intraday_short_scalper", "AAPL", 0, bearish_window(), 0);
    st.set_dry_run_diagnostics_for_test(vec![diag], 1_700_000_000)
        .await;

    let response = request_status(st).await;

    assert_eq!(response.truth_state, "not_configured");
    assert!(response.dry_run_strategy_diagnostics.is_empty());
}

// ---------------------------------------------------------------------------
// (2)+(3) Snapshot store/retrieve roundtrip; submitted=false.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s02_03_stored_snapshot_is_retrieved_with_submitted_false() {
    let _lock = env_lock().lock().await;
    let _env = EnvVarGuard::set(DRY_RUN_STRATEGY_IDS_ENV, "intraday_short_scalper");
    let st = bare_state();
    let diag = evaluate_dry_run_strategy("intraday_short_scalper", "AAPL", 0, bearish_window(), 0);
    assert!(
        !diag.submitted,
        "precondition: the evaluator itself never sets submitted=true"
    );
    st.set_dry_run_diagnostics_for_test(vec![diag], 1_700_000_000)
        .await;

    let response = request_status(st).await;

    assert_eq!(response.truth_state, "active");
    assert_eq!(response.dry_run_strategy_diagnostics.len(), 1);
    assert!(
        !response.dry_run_strategy_diagnostics[0].submitted,
        "S03: stored diagnostic must surface submitted=false through the API"
    );
}

// ---------------------------------------------------------------------------
// (4) Strategy id exposure.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s04_short_diagnostic_exposes_strategy_id() {
    let _lock = env_lock().lock().await;
    let _env = EnvVarGuard::set(DRY_RUN_STRATEGY_IDS_ENV, "intraday_short_scalper");
    let st = bare_state();
    let diag = evaluate_dry_run_strategy("intraday_short_scalper", "AAPL", 0, bearish_window(), 0);
    st.set_dry_run_diagnostics_for_test(vec![diag], 1_700_000_000)
        .await;

    let response = request_status(st).await;

    assert_eq!(
        response.dry_run_strategy_diagnostics[0].strategy_id,
        "intraday_short_scalper"
    );
}

// ---------------------------------------------------------------------------
// (5) Negative target qty on a bearish window.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s05_short_diagnostic_exposes_negative_target_qty_when_bearish() {
    let _lock = env_lock().lock().await;
    let _env = EnvVarGuard::set(DRY_RUN_STRATEGY_IDS_ENV, "intraday_short_scalper");
    let st = bare_state();
    let diag = evaluate_dry_run_strategy("intraday_short_scalper", "AAPL", 0, bearish_window(), 0);
    st.set_dry_run_diagnostics_for_test(vec![diag], 1_700_000_000)
        .await;

    let response = request_status(st).await;

    assert!(
        response.dry_run_strategy_diagnostics[0].target_qty < 0,
        "bearish window must drive the short scalper's target negative"
    );
}

// ---------------------------------------------------------------------------
// (6) would_b5_block=true.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s06_short_diagnostic_exposes_would_b5_block_true() {
    let _lock = env_lock().lock().await;
    let _env = EnvVarGuard::set(DRY_RUN_STRATEGY_IDS_ENV, "intraday_short_scalper");
    let st = bare_state();
    let diag = evaluate_dry_run_strategy("intraday_short_scalper", "AAPL", 0, bearish_window(), 0);
    st.set_dry_run_diagnostics_for_test(vec![diag], 1_700_000_000)
        .await;

    let response = request_status(st).await;

    assert!(response.dry_run_strategy_diagnostics[0].would_b5_block);
}

// ---------------------------------------------------------------------------
// (7) Default short-entry policy block reason.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s07_short_diagnostic_exposes_default_policy_block_reason() {
    let _lock = env_lock().lock().await;
    let _env = EnvVarGuard::set(DRY_RUN_STRATEGY_IDS_ENV, "intraday_short_scalper");
    let st = bare_state();
    let diag = evaluate_dry_run_strategy("intraday_short_scalper", "AAPL", 0, bearish_window(), 0);
    st.set_dry_run_diagnostics_for_test(vec![diag], 1_700_000_000)
        .await;

    let response = request_status(st).await;
    let row = &response.dry_run_strategy_diagnostics[0];

    assert!(
        row.would_policy_block,
        "the default fail-closed short-entry policy blocks every ShortOpen"
    );
    assert_eq!(
        row.policy_reason_code.as_deref(),
        Some("short_entries_disabled"),
        "ShortEntryConfig::fail_closed has allow_short_entries=false, \
         the first gate a ShortOpen intent hits"
    );
}

// ---------------------------------------------------------------------------
// (8) Snapshot replacement does not grow unbounded across ticks.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s08_snapshot_replacement_does_not_grow_unbounded_across_ticks() {
    let _lock = env_lock().lock().await;
    let _env = EnvVarGuard::set(DRY_RUN_STRATEGY_IDS_ENV, "intraday_short_scalper");
    let st = bare_state();

    for _ in 0..25 {
        let diag =
            evaluate_dry_run_strategy("intraday_short_scalper", "AAPL", 0, bearish_window(), 0);
        st.set_dry_run_diagnostics_for_test(vec![diag], 1_700_000_000)
            .await;
    }

    let response = request_status(st).await;

    assert_eq!(
        response.dry_run_strategy_diagnostics.len(),
        1,
        "25 ticks with 1 configured dry-run strategy must still yield exactly 1 row, \
         never 25 — storage replaces wholesale, never appends"
    );
}

#[tokio::test]
async fn s08b_snapshot_size_bounded_by_configured_strategy_count_not_tick_count() {
    let _lock = env_lock().lock().await;
    let _env = EnvVarGuard::set(
        DRY_RUN_STRATEGY_IDS_ENV,
        "intraday_scalper,intraday_short_scalper",
    );
    let st = bare_state();
    let ids = vec![
        "intraday_scalper".to_string(),
        "intraday_short_scalper".to_string(),
    ];

    for _ in 0..10 {
        let diags = evaluate_dry_run_strategies(&ids, "AAPL", 0, &bearish_window(), 0);
        st.set_dry_run_diagnostics_for_test(diags, 1_700_000_000)
            .await;
    }

    let response = request_status(st).await;

    assert_eq!(
        response.dry_run_strategy_diagnostics.len(),
        2,
        "10 ticks with 2 configured dry-run strategies must still yield exactly 2 rows"
    );
}

// ---------------------------------------------------------------------------
// Route purity: no broker/outbox/submission call in the new handler.
// ---------------------------------------------------------------------------

#[test]
fn s09_route_handler_source_contains_no_broker_or_outbox_calls() {
    let handler_src = include_str!("../src/routes/strategy/dry_run_status.rs");
    for forbidden in [
        "mqk_db::",
        "outbox_enqueue",
        "submit_order",
        "cancel_order",
        "replace_order",
        "submit_internal_strategy_decision",
        "broker_snapshot.write",
    ] {
        assert!(
            !handler_src.contains(forbidden),
            "dry-run status handler must not contain a read/write/broker/submission path: {forbidden}"
        );
    }
}

// ---------------------------------------------------------------------------
// Response shape: canonical_route / backend.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s11_response_includes_canonical_route_and_backend() {
    let _lock = env_lock().lock().await;
    let _env = EnvVarGuard::remove(DRY_RUN_STRATEGY_IDS_ENV);
    let st = bare_state();

    let response = request_status(st).await;

    assert_eq!(response.canonical_route, ROUTE);
    assert_eq!(response.backend, "daemon.runtime_state");
}

// ---------------------------------------------------------------------------
// Configured-but-not-yet-evaluated is honestly distinct from not-configured.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s12_configured_ids_surfaced_even_before_first_evaluation_tick() {
    let _lock = env_lock().lock().await;
    let _env = EnvVarGuard::set(
        DRY_RUN_STRATEGY_IDS_ENV,
        "intraday_short_scalper, swing_momentum",
    );
    let st = bare_state();

    let response = request_status(st).await;

    assert_eq!(response.truth_state, "active");
    assert_eq!(
        response.configured_dry_run_strategy_ids,
        vec![
            "intraday_short_scalper".to_string(),
            "swing_momentum".to_string()
        ]
    );
    assert!(
        response.dry_run_strategy_diagnostics.is_empty(),
        "configured but not yet evaluated this process lifetime -> empty diagnostics, \
         not a fabricated row"
    );
}

// ---------------------------------------------------------------------------
// evaluated_at_utc is populated once a snapshot is stored.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s13_evaluated_at_utc_is_populated_when_snapshot_present() {
    let _lock = env_lock().lock().await;
    let _env = EnvVarGuard::set(DRY_RUN_STRATEGY_IDS_ENV, "intraday_short_scalper");
    let st = bare_state();
    let diag = evaluate_dry_run_strategy("intraday_short_scalper", "AAPL", 0, bearish_window(), 0);
    st.set_dry_run_diagnostics_for_test(vec![diag], 1_700_000_000)
        .await;

    let response = request_status(st).await;

    assert!(!response.dry_run_strategy_diagnostics[0]
        .evaluated_at_utc
        .is_empty());
}
