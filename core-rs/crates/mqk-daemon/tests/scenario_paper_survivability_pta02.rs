//! # PTA-02 — Long-horizon paper survivability proof lane
//!
//! ## Purpose
//!
//! Proves that the paper supervision surfaces are honest about their
//! restart/session boundary, that mismatches are explicitly reviewable, and
//! that no surface serves stale data or fabricated truth after a daemon
//! restart simulation.
//!
//! ## What this file proves
//!
//! | Test | Claim |
//! |------|-------|
//! | S01 | `portfolio/positions` always reports `session_boundary="in_memory_only"` — no snapshot |
//! | S02 | `portfolio/positions` always reports `session_boundary="in_memory_only"` — active snapshot |
//! | S03 | `portfolio/orders/open` always reports `session_boundary="in_memory_only"` |
//! | S04 | `portfolio/fills` always reports `session_boundary="in_memory_only"` |
//! | S05 | Restart simulation: snapshot cleared → all three surfaces return `no_snapshot`, not stale data |
//! | S06 | Restart simulation: snapshot re-injected → all three surfaces return `active` with correct data |
//! | S07 | `snapshot_source` is `null` in `no_snapshot` state (never fabricated) |
//! | S08 | `snapshot_source="external"` for paper+alpaca active snapshot (correct broker kind) |
//! | S09 | `reconcile/status` reports `truth_state="never_run"` on fresh daemon (not ambiguous `"unknown"`) |
//! | S10 | `reconcile/status` `truth_state` is in the closed set: never_run / stale / active |
//! | S11 | `reconcile/mismatches` returns `truth_state="never_run"` when reconcile has never run |
//! | S12 | `reconcile/mismatches` `review_workflow` is `null` when `rows` is empty |
//! | S13 | `portfolio/summary` reports `truth_state="no_snapshot"` on fresh daemon |
//! | S14 | `portfolio/summary` reports `truth_state="active"` when snapshot present |
//! | S15 | All three portfolio detail surfaces consistently agree on `session_boundary` |
//!
//! ## Restart simulation contract
//!
//! Tests S05/S06 simulate a daemon restart by:
//! 1. Injecting a broker snapshot (simulating a running session)
//! 2. Reading the active state (proving data is served correctly)
//! 3. Clearing the broker snapshot (simulating daemon restart / snapshot loss)
//! 4. Asserting all surfaces return `no_snapshot` — no stale or fabricated data
//! 5. Re-injecting a snapshot (simulating post-restart snapshot reload)
//! 6. Asserting surfaces return `active` with the new data
//!
//! This is the key paper survivability proof: the in-memory surface fails
//! closed immediately on snapshot loss, then recovers cleanly on reload.
//!
//! ## What is NOT claimed
//!
//! - Durable position history across restarts (the daemon has none; this is explicit)
//! - Reconcile mismatch rows with real data (requires DB + execution loop)
//! - Fill-quality durable history (proven separately by TV-EXEC-01B)
//! - WS continuity across restarts (proven by BRK-07R)
//!
//! All tests are pure in-process.  No `MQK_DATABASE_URL` required.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::DateTime;
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use mqk_schemas::{BrokerAccount, BrokerFill, BrokerOrder, BrokerPosition, BrokerSnapshot};
use state::BrokerKind;
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

fn get(path: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap()
}

fn paper_alpaca_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ))
}

fn minimal_snapshot(ts: i64) -> BrokerSnapshot {
    BrokerSnapshot {
        captured_at_utc: DateTime::from_timestamp(ts, 0).expect("valid timestamp"),
        account: BrokerAccount {
            equity: "100000.00".to_string(),
            cash: "50000.00".to_string(),
            currency: "USD".to_string(),
        },
        orders: vec![BrokerOrder {
            broker_order_id: "broker-order-001".to_string(),
            client_order_id: "order-001".to_string(),
            symbol: "SPY".to_string(),
            side: "buy".to_string(),
            r#type: "market".to_string(),
            status: "accepted".to_string(),
            qty: "5".to_string(),
            limit_price: None,
            stop_price: None,
            created_at_utc: DateTime::from_timestamp(ts, 0).expect("valid ts"),
        }],
        fills: vec![BrokerFill {
            broker_fill_id: "fill-001".to_string(),
            broker_order_id: "broker-order-001".to_string(),
            client_order_id: "order-001".to_string(),
            symbol: "SPY".to_string(),
            side: "buy".to_string(),
            qty: "5".to_string(),
            price: "512.50".to_string(),
            fee: "0.00".to_string(),
            ts_utc: DateTime::from_timestamp(ts, 0).expect("valid ts"),
        }],
        positions: vec![BrokerPosition {
            symbol: "SPY".to_string(),
            qty: "5".to_string(),
            avg_price: "512.50".to_string(),
        }],
    }
}

// ---------------------------------------------------------------------------
// S01 — portfolio/positions session_boundary is in_memory_only (no snapshot)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s01_positions_session_boundary_in_memory_only_no_snapshot() {
    let st = paper_alpaca_state();
    let router = routes::build_router(st);

    let (status, body) = call(router, get("/api/v1/portfolio/positions")).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["snapshot_state"].as_str().unwrap(),
        "no_snapshot",
        "fresh daemon must report no_snapshot"
    );
    assert_eq!(
        json["session_boundary"].as_str().unwrap(),
        "in_memory_only",
        "session_boundary must always be in_memory_only — positions are not restart-safe"
    );
}

// ---------------------------------------------------------------------------
// S02 — portfolio/positions session_boundary is in_memory_only (active snapshot)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s02_positions_session_boundary_in_memory_only_active_snapshot() {
    let st = paper_alpaca_state();
    *st.broker_snapshot.write().await = Some(minimal_snapshot(1_700_000_000));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router, get("/api/v1/portfolio/positions")).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["snapshot_state"].as_str().unwrap(), "active");
    assert_eq!(
        json["session_boundary"].as_str().unwrap(),
        "in_memory_only",
        "session_boundary must be in_memory_only even when snapshot is active — still not durable"
    );
}

// ---------------------------------------------------------------------------
// S03 — portfolio/orders/open session_boundary is in_memory_only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s03_open_orders_session_boundary_in_memory_only() {
    let st = paper_alpaca_state();
    let router = routes::build_router(st);

    let (status, body) = call(router, get("/api/v1/portfolio/orders/open")).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["session_boundary"].as_str().unwrap(),
        "in_memory_only",
        "orders/open session_boundary must be in_memory_only"
    );
}

// ---------------------------------------------------------------------------
// S04 — portfolio/fills session_boundary is in_memory_only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s04_fills_session_boundary_in_memory_only() {
    let st = paper_alpaca_state();
    let router = routes::build_router(st);

    let (status, body) = call(router, get("/api/v1/portfolio/fills")).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["session_boundary"].as_str().unwrap(),
        "in_memory_only",
        "portfolio/fills session_boundary must be in_memory_only"
    );
}

// ---------------------------------------------------------------------------
// S05 — Restart simulation: cleared snapshot → all surfaces return no_snapshot
//
// This is the core survivability proof: after the broker_snapshot is cleared
// (simulating a daemon restart), no surface serves stale data.  All three
// portfolio detail surfaces must immediately return no_snapshot.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s05_restart_simulation_cleared_snapshot_returns_no_snapshot() {
    let st = paper_alpaca_state();

    // Step 1: inject a snapshot (simulates an active session)
    *st.broker_snapshot.write().await = Some(minimal_snapshot(1_700_000_000));

    // Step 2: verify active state
    {
        let router = routes::build_router(Arc::clone(&st));
        let (_, body) = call(router, get("/api/v1/portfolio/positions")).await;
        let json = parse_json(body);
        assert_eq!(
            json["snapshot_state"].as_str().unwrap(),
            "active",
            "precondition: snapshot must be active before simulated restart"
        );
    }

    // Step 3: clear snapshot (simulate daemon restart — broker_snapshot is in-memory only)
    *st.broker_snapshot.write().await = None;

    // Step 4: all surfaces must return no_snapshot — no stale data served
    let router = routes::build_router(Arc::clone(&st));

    let (_, body) = call(router.clone(), get("/api/v1/portfolio/positions")).await;
    let json = parse_json(body);
    assert_eq!(
        json["snapshot_state"].as_str().unwrap(),
        "no_snapshot",
        "positions must return no_snapshot after snapshot cleared (restart simulation)"
    );
    assert!(
        json["rows"].as_array().unwrap().is_empty(),
        "positions rows must be empty after snapshot cleared — no stale positions served"
    );
    assert!(
        json["snapshot_source"].is_null(),
        "snapshot_source must be null after snapshot cleared — not fabricated"
    );

    let (_, body) = call(router.clone(), get("/api/v1/portfolio/orders/open")).await;
    let json = parse_json(body);
    assert_eq!(
        json["snapshot_state"].as_str().unwrap(),
        "no_snapshot",
        "orders/open must return no_snapshot after snapshot cleared"
    );
    assert!(
        json["rows"].as_array().unwrap().is_empty(),
        "orders rows must be empty after snapshot cleared"
    );

    let (_, body) = call(router, get("/api/v1/portfolio/fills")).await;
    let json = parse_json(body);
    assert_eq!(
        json["snapshot_state"].as_str().unwrap(),
        "no_snapshot",
        "fills must return no_snapshot after snapshot cleared"
    );
    assert!(
        json["rows"].as_array().unwrap().is_empty(),
        "fills rows must be empty after snapshot cleared"
    );
}

// ---------------------------------------------------------------------------
// S06 — Restart simulation: re-injected snapshot → surfaces recover cleanly
//
// After clearing (S05), re-injecting a new snapshot must restore all three
// surfaces to active state with correct data.  Proves clean recovery path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s06_restart_simulation_reinjected_snapshot_recovers_correctly() {
    let st = paper_alpaca_state();

    // Simulate restart: start with no snapshot, then inject (post-restart load)
    *st.broker_snapshot.write().await = None;

    // Verify no_snapshot before injection
    {
        let router = routes::build_router(Arc::clone(&st));
        let (_, body) = call(router, get("/api/v1/portfolio/positions")).await;
        let json = parse_json(body);
        assert_eq!(
            json["snapshot_state"].as_str().unwrap(),
            "no_snapshot",
            "precondition: must be no_snapshot before post-restart snapshot load"
        );
    }

    // Post-restart snapshot load
    let snap_ts = 1_700_001_000i64;
    *st.broker_snapshot.write().await = Some(minimal_snapshot(snap_ts));

    let router = routes::build_router(Arc::clone(&st));

    // Positions should recover
    let (status, body) = call(router.clone(), get("/api/v1/portfolio/positions")).await;
    let json = parse_json(body);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["snapshot_state"].as_str().unwrap(),
        "active",
        "positions must recover to active after post-restart snapshot load"
    );
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        1,
        "positions rows must reflect re-injected snapshot"
    );
    assert_eq!(
        rows[0]["symbol"].as_str().unwrap(),
        "SPY",
        "position symbol must match re-injected snapshot"
    );
    assert_eq!(
        json["session_boundary"].as_str().unwrap(),
        "in_memory_only",
        "session_boundary must remain in_memory_only after recovery"
    );

    // Open orders should recover
    let (_, body) = call(router.clone(), get("/api/v1/portfolio/orders/open")).await;
    let json = parse_json(body);
    assert_eq!(json["snapshot_state"].as_str().unwrap(), "active");
    assert_eq!(json["rows"].as_array().unwrap().len(), 1);

    // Fills should recover
    let (_, body) = call(router, get("/api/v1/portfolio/fills")).await;
    let json = parse_json(body);
    assert_eq!(json["snapshot_state"].as_str().unwrap(), "active");
    assert_eq!(json["rows"].as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// S07 — snapshot_source is null in no_snapshot state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s07_snapshot_source_null_when_no_snapshot() {
    let st = paper_alpaca_state();
    let router = routes::build_router(Arc::clone(&st));

    for path in &[
        "/api/v1/portfolio/positions",
        "/api/v1/portfolio/orders/open",
        "/api/v1/portfolio/fills",
    ] {
        let (_, body) = call(router.clone(), get(path)).await;
        let json = parse_json(body);
        assert!(
            json["snapshot_source"].is_null(),
            "{path}: snapshot_source must be null in no_snapshot state — not fabricated; got: {}",
            json["snapshot_source"]
        );
    }
}

// ---------------------------------------------------------------------------
// S08 — snapshot_source="external" for paper+alpaca active snapshot
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s08_snapshot_source_external_for_paper_alpaca() {
    let st = paper_alpaca_state(); // BrokerKind::Alpaca → BrokerSnapshotTruthSource::External
    *st.broker_snapshot.write().await = Some(minimal_snapshot(1_700_000_000));
    let router = routes::build_router(Arc::clone(&st));

    for path in &[
        "/api/v1/portfolio/positions",
        "/api/v1/portfolio/orders/open",
        "/api/v1/portfolio/fills",
    ] {
        let (_, body) = call(router.clone(), get(path)).await;
        let json = parse_json(body);
        assert_eq!(
            json["snapshot_source"].as_str().unwrap(),
            "external",
            "{path}: paper+alpaca must report snapshot_source=external; got: {}",
            json["snapshot_source"]
        );
    }
}

// ---------------------------------------------------------------------------
// S09 — reconcile/status truth_state="never_run" on fresh daemon
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s09_reconcile_status_truth_state_never_run_on_fresh_daemon() {
    let st = paper_alpaca_state();
    let router = routes::build_router(st);

    let (status, body) = call(router, get("/api/v1/reconcile/status")).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["truth_state"].as_str().unwrap(),
        "never_run",
        "fresh daemon reconcile/status must report truth_state=never_run, not ambiguous 'unknown'; \
         got: {json}"
    );
    // status field is still "unknown" for backward compat — but truth_state disambiguates it
    assert_eq!(
        json["status"].as_str().unwrap(),
        "unknown",
        "status field must still be 'unknown' for backward compatibility"
    );
}

// ---------------------------------------------------------------------------
// S10 — reconcile/status truth_state is from the closed allowed set
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s10_reconcile_status_truth_state_from_closed_set() {
    let allowed: &[&str] = &["never_run", "stale", "active"];

    let st = paper_alpaca_state();
    let router = routes::build_router(st);

    let (_, body) = call(router, get("/api/v1/reconcile/status")).await;
    let json = parse_json(body);

    let truth_state = json["truth_state"]
        .as_str()
        .expect("truth_state must be a string");
    assert!(
        allowed.contains(&truth_state),
        "reconcile/status truth_state '{truth_state}' is not in the allowed set {allowed:?}"
    );
}

// ---------------------------------------------------------------------------
// S11 — reconcile/mismatches returns truth_state="never_run" when never run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s11_reconcile_mismatches_never_run_when_reconcile_not_yet_run() {
    let st = paper_alpaca_state();
    let router = routes::build_router(st);

    let (status, body) = call(router, get("/api/v1/reconcile/mismatches")).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["truth_state"].as_str().unwrap(),
        "never_run",
        "reconcile/mismatches must return truth_state=never_run when reconcile loop \
         has not yet completed a tick; got: {json}"
    );
    assert!(
        json["rows"].as_array().unwrap().is_empty(),
        "rows must be empty in never_run state"
    );
    assert!(
        json["review_workflow"].is_null(),
        "review_workflow must be null in never_run state — not authoritative"
    );
}

// ---------------------------------------------------------------------------
// S12 — reconcile/mismatches review_workflow is null when rows is empty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s12_reconcile_mismatches_review_workflow_null_when_rows_empty() {
    // In all non-active or active-but-clean states, review_workflow must be null.
    // This test covers the never_run path (most accessible without DB).
    let st = paper_alpaca_state();
    let router = routes::build_router(st);

    let (_, body) = call(router, get("/api/v1/reconcile/mismatches")).await;
    let json = parse_json(body);

    let rows = json["rows"].as_array().unwrap();
    let review_workflow = &json["review_workflow"];

    // Either rows is empty and review_workflow is null, or both are populated.
    // In the never_run path, rows is always empty.
    if rows.is_empty() {
        assert!(
            review_workflow.is_null(),
            "review_workflow must be null when rows is empty; got: {review_workflow}"
        );
    }
    // If rows were non-empty (active with mismatches), review_workflow being
    // non-null would be correct — but that path requires a live DB.
}

// ---------------------------------------------------------------------------
// S13 — portfolio/summary truth_state="no_snapshot" on fresh daemon
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s13_portfolio_summary_truth_state_no_snapshot_on_fresh_daemon() {
    let st = paper_alpaca_state();
    let router = routes::build_router(st);

    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["truth_state"].as_str().unwrap(),
        "no_snapshot",
        "portfolio/summary truth_state must be no_snapshot on fresh daemon; got: {json}"
    );
    // has_snapshot must also be false (backward compat field)
    assert_eq!(
        json["has_snapshot"].as_bool().unwrap(),
        false,
        "has_snapshot must be false on fresh daemon"
    );
}

// ---------------------------------------------------------------------------
// S14 — portfolio/summary truth_state="active" when snapshot present
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s14_portfolio_summary_truth_state_active_when_snapshot_present() {
    let st = paper_alpaca_state();
    *st.broker_snapshot.write().await = Some(minimal_snapshot(1_700_000_000));
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = call(router, get("/api/v1/portfolio/summary")).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["truth_state"].as_str().unwrap(),
        "active",
        "portfolio/summary truth_state must be active when snapshot is present; got: {json}"
    );
    assert_eq!(
        json["has_snapshot"].as_bool().unwrap(),
        true,
        "has_snapshot must be true when snapshot is present"
    );
    // Financial fields must be populated
    assert!(
        json["account_equity"].as_f64().is_some(),
        "account_equity must be populated in active state"
    );
}

// ---------------------------------------------------------------------------
// S15 — all three portfolio detail surfaces agree on session_boundary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s15_all_portfolio_surfaces_agree_on_session_boundary() {
    let st = paper_alpaca_state();
    *st.broker_snapshot.write().await = Some(minimal_snapshot(1_700_000_000));
    let router = routes::build_router(Arc::clone(&st));

    let paths = [
        "/api/v1/portfolio/positions",
        "/api/v1/portfolio/orders/open",
        "/api/v1/portfolio/fills",
    ];

    let mut boundaries = Vec::new();
    for path in &paths {
        let (_, body) = call(router.clone(), get(path)).await;
        let json = parse_json(body);
        let boundary = json["session_boundary"]
            .as_str()
            .expect("session_boundary must be a string")
            .to_string();
        boundaries.push((path, boundary));
    }

    // All must agree
    let first = &boundaries[0].1;
    for (path, boundary) in &boundaries {
        assert_eq!(
            boundary, first,
            "session_boundary must be consistent across all portfolio surfaces; \
             {path} reports {boundary:?} but {} reports {first:?}",
            boundaries[0].0
        );
    }

    // And the agreed value must be "in_memory_only"
    assert_eq!(
        first, "in_memory_only",
        "all portfolio surfaces must agree on session_boundary=in_memory_only"
    );
}
