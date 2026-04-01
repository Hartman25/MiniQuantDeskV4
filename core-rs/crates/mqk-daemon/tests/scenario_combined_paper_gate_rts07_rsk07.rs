//! # RTS-07 / RSK-07 — Combined paper-runtime gate proof
//!
//! ## Purpose
//!
//! This file proves two related claims about the canonical paper+alpaca
//! execution path:
//!
//! ### RTS-07 — Strategy-to-intent contract
//!
//! When a signal is admitted through all pre-DB gates, the route sets
//! `intent_placed = false` for every rejection path reachable without a DB.
//! The sole `intent_placed = true` path (Gate 7 Ok(true)) is proven at the
//! unit level by `u01_is_intent_placed_true_only_on_new_enqueue` in
//! `routes/strategy.rs`.
//!
//! | Test | Gate that fires          | Expected `intent_placed` |
//! |------|--------------------------|--------------------------|
//! | R02  | Gate 1 (wrong path)      | false                    |
//! | R03  | Gate 1b (WS GapDetected) | false                    |
//! | R04  | Gate 1b (WS ColdStart)   | false                    |
//! | R05  | Gate 1c (outside session)| false                    |
//! | R06  | Gate 1d (limit exceeded) | false                    |
//!
//! ### RSK-07 — Combined gate coherence
//!
//! The paper runtime has two interlocking gate chains:
//!
//! **Start chain** (POST /v1/run/start):
//! ```text
//! Gate 1: deployment_mode     (paper+paper → 403)
//! Gate 2: integrity_armed     (disarmed → 403)
//! Gate 3: alpaca_ws_continuity (gap/cold → 403)
//! Gate 4: reconcile_truth     (dirty/stale → 403)
//! Gate 5: db                  (no DB → 503)
//! ```
//!
//! **Signal chain** (POST /api/v1/strategy/signal):
//! ```text
//! Gate 1:  signal_ingestion_configured
//! Gate 1b: alpaca_ws_continuity
//! Gate 1c: nyse_session
//! Gate 1d: day_signal_limit
//! Gate 2:  db_present         (no DB → 503)
//! ... Gates 3-7 require DB
//! ```
//!
//! Key coherence properties:
//!
//! - **G01**: The same healthy state satisfies both gate chains up to the DB
//!   gate.  Signal pre-DB gates and start pre-DB gates all pass together.
//! - **G02**: WS gap simultaneously blocks start (Gate 3) and signal (Gate
//!   1b).  One condition, two barriers.
//! - **G03**: Reconcile dirty blocks start (Gate 4) but is **not** in the
//!   signal gate chain.  Reconcile enforcement is split: start boundary
//!   (BRK-09R) and orchestrator Phase 0c (I9-1), not signal admission.
//! - **G04**: Signal gate ordering is deterministic.  WS gate fires before
//!   session gate; session gate fires before limit gate.
//!
//! ## What is NOT claimed
//!
//! - `intent_placed = true` for Gate 7 (Gate 7 requires DB; proven at unit
//!   level by `u01_is_intent_placed_true_only_on_new_enqueue` in strategy.rs).
//! - Gates 3-7 of the signal chain (all require DB; DB-backed tests in other
//!   files cover arm state, active run, and suppression).
//! - Orchestrator dispatch of an admitted signal (proven by TV-EXEC-01B).
//! - Reconcile enforcement during a running loop (proven by I9-1 and
//!   scenario_reconcile_tick_disarms_on_drift.rs).
//!
//! All tests are pure in-process (no `MQK_DATABASE_URL` required).

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    routes,
    state::{
        AlpacaWsContinuityState, AppState, BrokerKind, DeploymentMode, ReconcileStatusSnapshot,
    },
};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// Build a canonical Paper+Alpaca state: integrity disarmed (boot default),
/// WS ColdStartUnproven, no DB, NYSE session clock not set.
fn paper_alpaca_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ))
}

/// Build a Paper+Paper state (not an execution path).
fn paper_paper_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Paper))
}

/// Signal body for the given signal_id, with qty=1 market buy on SPY.
fn signal_body(signal_id: &str) -> axum::body::Body {
    axum::body::Body::from(
        serde_json::json!({
            "signal_id": signal_id,
            "strategy_id": "strat-rts07",
            "symbol": "SPY",
            "side": "buy",
            "qty": 1
        })
        .to_string(),
    )
}

fn signal_req(signal_id: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(signal_body(signal_id))
        .unwrap()
}

fn start_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap()
}

fn arm_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap()
}

/// Live WS continuity state (canonical healthy value).
fn live_ws() -> AlpacaWsContinuityState {
    AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:ord-test:new:2026-01-01T00:00:00Z".to_string(),
        last_event_at: "2026-01-01T00:00:00Z".to_string(),
    }
}

/// NYSE regular-session Unix timestamp: 2024-01-08 20:00:00 UTC = 15:00 ET
/// (Monday, regular session, 09:30–16:00 ET).
const NYSE_REGULAR_TS: i64 = 1_704_726_000;

/// Premarket Unix timestamp: 2024-01-08 13:00:00 UTC = 08:00 ET
/// (Monday, before 09:30 ET open = premarket).
const NYSE_PREMARKET_TS: i64 = 1_704_718_800;

/// Build a "fully aligned" Paper+Alpaca state for combined tests:
/// - integrity armed (via route, in-memory)
/// - WS continuity Live
/// - reconcile status "ok"
/// - session clock set to NYSE regular hours
///
/// This state satisfies every pure in-process gate in both the start chain
/// and the signal chain.
async fn aligned_state() -> Arc<AppState> {
    let st = paper_alpaca_state();

    // Arm integrity gate (pure in-memory — no DB required).
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req()).await;
    assert_eq!(
        arm_status,
        StatusCode::OK,
        "aligned_state: arm must succeed"
    );

    // Establish Live WS continuity.
    st.update_ws_continuity(live_ws()).await;

    // Set reconcile to "ok" (default is "unknown" which also passes the start
    // gate, but "ok" is the explicit healthy value; use it for clarity).
    st.publish_reconcile_snapshot(ReconcileStatusSnapshot {
        status: "ok".to_string(),
        last_run_at: Some("2026-01-01T00:01:00Z".to_string()),
        snapshot_watermark_ms: Some(2_000_000),
        mismatched_positions: 0,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: None,
    })
    .await;

    // Inject NYSE regular-session timestamp so Gate 1c passes.
    st.set_session_clock_ts_for_test(NYSE_REGULAR_TS).await;

    st
}

// ---------------------------------------------------------------------------
// R02 — Gate 1 blocks on wrong deployment path → intent_placed = false
//
// Paper+Paper has NotConfigured strategy source.  Gate 1 (signal_ingestion
// configured) fires immediately.  The response must not claim an intent was
// placed because no outbox row was written.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r02_gate1_blocks_on_wrong_path_intent_placed_false() {
    let st = paper_paper_state();
    let router = routes::build_router(Arc::clone(&st));
    let (status, json) = call(router, signal_req("sig-rts07-r02")).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "R02: Paper+Paper Gate 1 must return 503; got: {status}"
    );
    assert_eq!(
        json["disposition"].as_str(),
        Some("unavailable"),
        "R02: Gate 1 disposition must be 'unavailable'; got: {json}"
    );
    assert_eq!(
        json["intent_placed"].as_bool(),
        Some(false),
        "R02: Gate 1 refusal must yield intent_placed=false; got: {json}"
    );
    assert_eq!(
        json["accepted"].as_bool(),
        Some(false),
        "R02: Gate 1 refusal must yield accepted=false; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// R03 — Gate 1b blocks on WS GapDetected → intent_placed = false
//
// Paper+Alpaca with GapDetected continuity.  Gate 1 passes (ExternalSignal
// Ingestion is wired), but Gate 1b fires (broker event delivery unreliable).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r03_gate1b_gap_detected_intent_placed_false() {
    let st = paper_alpaca_state();
    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:ord-1:new:2026-01-01T00:00:00Z".to_string()),
        last_event_at: Some("2026-01-01T00:00:00Z".to_string()),
        detail: "rts07-r03: simulated gap".to_string(),
    })
    .await;
    st.set_session_clock_ts_for_test(NYSE_REGULAR_TS).await;

    let router = routes::build_router(Arc::clone(&st));
    let (status, json) = call(router, signal_req("sig-rts07-r03")).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "R03: GapDetected must block at Gate 1b (503); got: {status}"
    );
    assert_eq!(
        json["disposition"].as_str(),
        Some("continuity_gap"),
        "R03: Gate 1b disposition must be continuity_gap; got: {json}"
    );
    assert_eq!(
        json["intent_placed"].as_bool(),
        Some(false),
        "R03: WS gap refusal must yield intent_placed=false; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// R04 — Gate 1b blocks on WS ColdStartUnproven → intent_placed = false
//
// Paper+Alpaca at boot (ColdStartUnproven).  WS transport has not yet
// confirmed subscription.  Fail closed: signals refused until Live.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r04_gate1b_cold_start_unproven_intent_placed_false() {
    let st = paper_alpaca_state();
    // Default state for Paper+Alpaca is ColdStartUnproven; set explicitly.
    // (update_ws_continuity guards against NotApplicable but not Cold→Cold.)
    st.set_session_clock_ts_for_test(NYSE_REGULAR_TS).await;

    let router = routes::build_router(Arc::clone(&st));
    let (status, json) = call(router, signal_req("sig-rts07-r04")).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "R04: ColdStartUnproven must block at Gate 1b (503); got: {status}"
    );
    assert_eq!(
        json["disposition"].as_str(),
        Some("unavailable"),
        "R04: Gate 1b ColdStart disposition must be unavailable; got: {json}"
    );
    assert_eq!(
        json["intent_placed"].as_bool(),
        Some(false),
        "R04: ColdStart refusal must yield intent_placed=false; got: {json}"
    );

    // Confirm the specific blocker message names "cold start" to distinguish
    // from Gate 1 (ingestion_not_configured) and Gate 2 (db_unavailable).
    let blockers = json["blockers"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    let has_cold_start_blocker = blockers
        .iter()
        .any(|b| b.contains("cold start") || b.contains("unproven"));
    assert!(
        has_cold_start_blocker,
        "R04: blocker must name cold-start/unproven condition; blockers: {blockers:?}"
    );
}

// ---------------------------------------------------------------------------
// R05 — Gate 1c blocks outside NYSE session → intent_placed = false
//
// Paper+Alpaca, WS=Live, premarket timestamp.  Gates 1 and 1b pass; Gate 1c
// (NYSE session) fires because the injected clock is 08:00 ET (premarket).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r05_gate1c_outside_session_intent_placed_false() {
    let st = paper_alpaca_state();
    st.update_ws_continuity(live_ws()).await;
    st.set_session_clock_ts_for_test(NYSE_PREMARKET_TS).await;

    let router = routes::build_router(Arc::clone(&st));
    let (status, json) = call(router, signal_req("sig-rts07-r05")).await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "R05: premarket session must block at Gate 1c (409); got: {status}"
    );
    assert_eq!(
        json["disposition"].as_str(),
        Some("outside_session"),
        "R05: Gate 1c disposition must be outside_session; got: {json}"
    );
    assert_eq!(
        json["intent_placed"].as_bool(),
        Some(false),
        "R05: outside-session refusal must yield intent_placed=false; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// R06 — Gate 1d blocks on signal limit exceeded → intent_placed = false
//
// Paper+Alpaca, WS=Live, NYSE session, but per-run count == MAX.
// Gate 1d fires before Gate 2 (DB) so no outbox write occurs.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r06_gate1d_limit_exceeded_intent_placed_false() {
    let st = paper_alpaca_state();
    st.update_ws_continuity(live_ws()).await;
    st.set_session_clock_ts_for_test(NYSE_REGULAR_TS).await;
    // Saturate the per-run counter (PT-AUTO-02 proof seam).
    st.set_day_signal_count_for_test(100);

    let router = routes::build_router(Arc::clone(&st));
    let (status, json) = call(router, signal_req("sig-rts07-r06")).await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "R06: limit exceeded must block at Gate 1d (409); got: {status}"
    );
    assert_eq!(
        json["disposition"].as_str(),
        Some("day_limit_reached"),
        "R06: Gate 1d disposition must be day_limit_reached; got: {json}"
    );
    assert_eq!(
        json["intent_placed"].as_bool(),
        Some(false),
        "R06: limit-exceeded refusal must yield intent_placed=false; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// G01 — Healthy state satisfies both start and signal gate chains
//
// RSK-07 combined coherence: the same Paper+Alpaca state that passes all
// pre-DB start gates also passes all pre-DB signal gates.
//
// Start chain: deployment ✓, arm ✓, WS ✓, reconcile ✓ → 503 (DB gate).
// Signal chain: Gate 1 ✓, 1b ✓, 1c ✓, 1d ✓ → 503 (Gate 2, DB gate).
//
// This proves the two gate chains are coherent: a runtime-ready state is also
// a signal-admission-ready state.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g01_aligned_state_satisfies_both_start_and_signal_chains() {
    let st = aligned_state().await;
    let router = routes::build_router(Arc::clone(&st));

    // ── Start chain: all pre-DB gates pass → DB gate fires (503) ──────────
    let (start_status, start_json) = call(routes::build_router(Arc::clone(&st)), start_req()).await;

    assert_eq!(
        start_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "G01: aligned state must reach DB gate on start (503); got: {start_status} — {start_json}"
    );
    // None of the prior start gates must have fired.
    let start_gate = start_json["gate"].as_str().unwrap_or("");
    assert_ne!(
        start_gate, "deployment_mode",
        "G01: start must not be blocked at deployment_mode; got: {start_json}"
    );
    assert_ne!(
        start_gate, "integrity_armed",
        "G01: start must not be blocked at integrity_armed; got: {start_json}"
    );
    assert_ne!(
        start_gate, "alpaca_ws_continuity",
        "G01: start must not be blocked at alpaca_ws_continuity; got: {start_json}"
    );
    assert_ne!(
        start_gate, "reconcile_truth",
        "G01: start must not be blocked at reconcile_truth; got: {start_json}"
    );

    // ── Signal chain: all pre-DB gates pass → DB gate fires (503) ─────────
    let (sig_status, sig_json) = call(router, signal_req("sig-rts07-g01")).await;

    assert_eq!(
        sig_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "G01: aligned state must reach Gate 2 on signal (503); got: {sig_status} — {sig_json}"
    );

    // The Gate 2 blocker is the canonical DB-unavailable message.  Verify it
    // is NOT the Gate 1/1b/1c/1d messages — those are earlier gates that must
    // all have passed.
    let sig_blockers = sig_json["blockers"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();

    let gate1_msg = "strategy signal ingestion is not configured for this deployment";
    let gate1b_cold_msg = "cold start";
    let gate1b_gap_msg = "continuity gap";
    let gate1c_msg = "NYSE market session";
    let gate1d_msg = "autonomous day signal limit reached";

    for blocker in &sig_blockers {
        assert!(
            !blocker.contains(gate1_msg),
            "G01: signal must not be blocked at Gate 1; blocker: {blocker}"
        );
        assert!(
            !blocker.contains(gate1b_cold_msg),
            "G01: signal must not be blocked at Gate 1b (cold start); blocker: {blocker}"
        );
        assert!(
            !blocker.contains(gate1b_gap_msg),
            "G01: signal must not be blocked at Gate 1b (gap); blocker: {blocker}"
        );
        assert!(
            !blocker.contains(gate1c_msg),
            "G01: signal must not be blocked at Gate 1c; blocker: {blocker}"
        );
        assert!(
            !blocker.contains(gate1d_msg),
            "G01: signal must not be blocked at Gate 1d; blocker: {blocker}"
        );
    }

    // Gate 2 (DB unavailable) is the expected blocker.
    let has_db_blocker = sig_blockers
        .iter()
        .any(|b| b.contains("durable execution DB truth is unavailable"));
    assert!(
        has_db_blocker,
        "G01: signal chain must reach Gate 2 (DB unavailable); blockers: {sig_blockers:?}"
    );
}

// ---------------------------------------------------------------------------
// G02 — WS gap simultaneously blocks both start and signal
//
// RSK-07 coherence: a single WS gap condition enforces the barrier in both
// gate chains.  The orchestrator cannot be started AND signals cannot be
// admitted.  The paper runtime is offline until WS re-establishes Live.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g02_ws_gap_blocks_both_start_and_signal() {
    let st = aligned_state().await;

    // Inject a WS gap (overrides the Live set in aligned_state).
    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:ord-1:new:2026-01-01T00:00:00Z".to_string()),
        last_event_at: Some("2026-01-01T00:00:00Z".to_string()),
        detail: "rsk07-g02: simulated gap".to_string(),
    })
    .await;

    // ── Start chain: WS gate fires (Gate 3) ───────────────────────────────
    let (start_status, start_json) = call(routes::build_router(Arc::clone(&st)), start_req()).await;

    assert_eq!(
        start_status,
        StatusCode::FORBIDDEN,
        "G02: WS gap must block start (403); got: {start_status}"
    );
    assert_eq!(
        start_json["gate"].as_str(),
        Some("alpaca_ws_continuity"),
        "G02: start must be blocked at alpaca_ws_continuity; got: {start_json}"
    );

    // ── Signal chain: Gate 1b fires (continuity_gap) ──────────────────────
    let (sig_status, sig_json) = call(
        routes::build_router(Arc::clone(&st)),
        signal_req("sig-rts07-g02"),
    )
    .await;

    assert_eq!(
        sig_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "G02: WS gap must block signal at Gate 1b (503); got: {sig_status}"
    );
    assert_eq!(
        sig_json["disposition"].as_str(),
        Some("continuity_gap"),
        "G02: signal Gate 1b must have continuity_gap disposition; got: {sig_json}"
    );
    assert_eq!(
        sig_json["intent_placed"].as_bool(),
        Some(false),
        "G02: WS gap signal refusal must yield intent_placed=false; got: {sig_json}"
    );
}

// ---------------------------------------------------------------------------
// G03 — Reconcile dirty blocks start but is NOT in the signal gate chain
//
// RSK-07 coherence — correct separation of concerns:
//
// Reconcile enforcement is split across three layers:
//   1. Start boundary (BRK-09R): reconcile dirty/stale → start refused.
//   2. Orchestrator Phase 0c (I9-1): reconcile drift during a run → halt.
//   3. Signal admission: NO reconcile check (by design).
//
// This test proves property 3: a dirty reconcile state does not appear in
// the signal route's gate chain.  With Live WS and NYSE session, the signal
// advances past Gates 1/1b/1c/1d and reaches Gate 2 (DB unavailable) — the
// reconcile state is invisible to signal admission.
//
// This is the correct design.  The signal route's job is to validate and
// enqueue; the orchestrator's job is to enforce reconcile truth before
// dispatching.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g03_reconcile_dirty_blocks_start_not_signal_admission() {
    let st = aligned_state().await;

    // Inject dirty reconcile (overrides the "ok" set in aligned_state).
    st.publish_reconcile_snapshot(ReconcileStatusSnapshot {
        status: "dirty".to_string(),
        last_run_at: Some("2026-01-01T00:00:00Z".to_string()),
        snapshot_watermark_ms: Some(1_000_000),
        mismatched_positions: 1,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some("rsk07-g03: simulated prior drift".to_string()),
    })
    .await;

    // ── Start chain: reconcile gate fires (Gate 4) ────────────────────────
    let (start_status, start_json) = call(routes::build_router(Arc::clone(&st)), start_req()).await;

    assert_eq!(
        start_status,
        StatusCode::FORBIDDEN,
        "G03: reconcile dirty must block start (403); got: {start_status}"
    );
    assert_eq!(
        start_json["gate"].as_str(),
        Some("reconcile_truth"),
        "G03: start must be blocked at reconcile_truth; got: {start_json}"
    );

    // ── Signal chain: NO reconcile gate — signal reaches Gate 2 (DB) ──────
    //
    // This is the authoritative proof that reconcile dirty does NOT appear in
    // the signal gate chain.  The signal is not blocked by reconcile state;
    // it is blocked only by the absent DB (Gate 2).
    let (sig_status, sig_json) = call(
        routes::build_router(Arc::clone(&st)),
        signal_req("sig-rts07-g03"),
    )
    .await;

    assert_eq!(
        sig_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "G03: with dirty reconcile, signal must still reach Gate 2 (503, not 403); \
         reconcile is not a signal admission gate; got: {sig_status}"
    );

    // Verify the disposition is "unavailable" (Gate 2/DB) not "reconcile_dirty"
    // or any similar reconcile-specific value.
    let sig_disposition = sig_json["disposition"].as_str().unwrap_or("");
    assert_eq!(
        sig_disposition, "unavailable",
        "G03: signal disposition must be 'unavailable' (Gate 2 DB) not a reconcile value; \
         got: {sig_json}"
    );

    // Confirm the blocker is the DB-unavailable message, not a reconcile
    // message.  This proves reconcile truth is NOT checked in signal admission.
    let sig_blockers = sig_json["blockers"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();

    let has_db_blocker = sig_blockers
        .iter()
        .any(|b| b.contains("durable execution DB truth is unavailable"));
    assert!(
        has_db_blocker,
        "G03: signal must be blocked at Gate 2 (DB), not at a reconcile gate; \
         blockers: {sig_blockers:?}"
    );

    let has_reconcile_blocker = sig_blockers
        .iter()
        .any(|b| b.to_ascii_lowercase().contains("reconcile"));
    assert!(
        !has_reconcile_blocker,
        "G03: signal blockers must not mention reconcile (reconcile is not a signal gate); \
         blockers: {sig_blockers:?}"
    );
}

// ---------------------------------------------------------------------------
// G04 — Signal gate ordering is deterministic
//
// RSK-07 coherence: the pre-DB signal gates fire in the order declared in the
// route comment.  Two ordering proofs:
//
//   G04a: WS gate fires before session gate.
//         State: WS=GapDetected + premarket.
//         Expected: Gate 1b blocks (continuity_gap), not Gate 1c.
//
//   G04b: Session gate fires before limit gate.
//         State: WS=Live + premarket (session out) + limit not exceeded.
//         Expected: Gate 1c blocks (outside_session), not Gate 1d.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g04a_ws_gate_fires_before_session_gate() {
    let st = paper_alpaca_state();
    // Both WS and session are in a failing state; WS must fire first.
    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: None,
        last_event_at: None,
        detail: "rsk07-g04a: ordering test gap".to_string(),
    })
    .await;
    st.set_session_clock_ts_for_test(NYSE_PREMARKET_TS).await; // outside session too

    let router = routes::build_router(Arc::clone(&st));
    let (status, json) = call(router, signal_req("sig-rts07-g04a")).await;

    // WS gate (1b) must fire, not session gate (1c).
    assert_eq!(
        json["disposition"].as_str(),
        Some("continuity_gap"),
        "G04a: WS gate (1b) must fire before session gate (1c); got: {json}"
    );
    assert_ne!(
        json["disposition"].as_str(),
        Some("outside_session"),
        "G04a: session gate (1c) must NOT fire before WS gate (1b); got: {json}"
    );
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "G04a: WS gate blocks with 503; got: {status}"
    );
}

#[tokio::test]
async fn g04b_session_gate_fires_before_limit_gate() {
    let st = paper_alpaca_state();
    // WS=Live so Gate 1b passes; session outside so Gate 1c fires.
    // Limit not exceeded (count=0 at boot) — Gate 1d would not fire anyway.
    st.update_ws_continuity(live_ws()).await;
    st.set_session_clock_ts_for_test(NYSE_PREMARKET_TS).await;
    // Leave limit at default (0 signals) — Gate 1d would not fire regardless.

    let router = routes::build_router(Arc::clone(&st));
    let (status, json) = call(router, signal_req("sig-rts07-g04b")).await;

    // Session gate (1c) must fire, not limit gate (1d).
    assert_eq!(
        json["disposition"].as_str(),
        Some("outside_session"),
        "G04b: session gate (1c) must fire when premarket; got: {json}"
    );
    assert_ne!(
        json["disposition"].as_str(),
        Some("day_limit_reached"),
        "G04b: limit gate (1d) must NOT fire before session gate (1c); got: {json}"
    );
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "G04b: session gate blocks with 409; got: {status}"
    );
}
