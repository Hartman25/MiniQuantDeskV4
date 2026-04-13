//! Causality submit-timing proof tests (DB-backed).
//!
//! These tests prove the submit-timing behavior of
//! `GET /api/v1/execution/orders/:order_id/causality` through a real DB.
//!
//! # What is proven
//!
//! CA-09 (DB): Submit timing is surfaced correctly.
//!   - Insert one fill row with `submit_ts_utc` + `submit_to_fill_ms`.
//!   - Route returns `submit_event` node first, then `execution_fill` node.
//!   - Both timing fields on the fill node match the DB values exactly.
//!
//! CA-10 (DB): Null-safe behavior.
//!   - Insert one fill row with `submit_ts_utc = NULL`, `submit_to_fill_ms = NULL`.
//!   - No `submit_event` node is created.
//!   - The `execution_fill` node carries null timing fields; nothing is fabricated.
//!
//! CA-11 (DB): Submit node ordering.
//!   - Insert 2 fills sharing the same `submit_ts_utc`.
//!   - `nodes[0]` is `submit_event` with timestamp equal to `submit_ts_utc`.
//!   - `nodes[1]` and `nodes[2]` are `execution_fill` in chronological order.
//!   - `nodes[1].elapsed_from_prev_ms` equals `nodes[1].submit_to_fill_ms`
//!     (both anchored to the same `submit_ts → fill1` interval).
//!
//! All tests require `MQK_DATABASE_URL` and skip gracefully without it.
//! Run with: `cargo test --workspace -- --include-ignored --test-threads=1`

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use chrono::Utc;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use mqk_daemon::{routes::build_router, state::AppState};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

async fn call(router: axum::Router, req: Request<Body>) -> (StatusCode, Bytes) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect")
        .to_bytes();
    (status, body)
}

async fn connect_db() -> Option<sqlx::PgPool> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(u) => u,
        Err(_) => return None,
    };
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .ok()
}

/// Seed a minimal PAPER run and return after cleanup of prior data for that run_id.
///
/// Uses `Utc::now()` for `started_at_utc` so this run is always the newest
/// PAPER run in the DB when the test immediately arms and queries it.
async fn seed_run(pool: &sqlx::PgPool, run_id: Uuid) {
    // Pre-test cleanup — deterministic by run_id.
    for table in &[
        "fill_quality_telemetry",
        "audit_events",
        "oms_inbox",
        "oms_outbox",
    ] {
        sqlx::query(&format!("delete from {table} where run_id = $1"))
            .bind(run_id)
            .execute(pool)
            .await
            .unwrap_or_else(|_| panic!("pre-test {table} cleanup failed"));
    }
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .expect("pre-test runs cleanup failed");

    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(), // newest run → wins fetch_latest_run_for_engine
            git_hash: "ca-submit-hash".to_string(),
            config_hash: "ca-submit-cfg".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "ca-submit-host".to_string(),
        },
    )
    .await
    .expect("seed_run: insert_run failed");
}

/// Derive a deterministic UUIDv5 telemetry_id for a (run_id, broker_message_id) pair.
fn telemetry_id(run_id: Uuid, msg_id: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("mqk.fill-quality.v1|{}|{}", run_id, msg_id).as_bytes(),
    )
}

// ---------------------------------------------------------------------------
// CA-09 (DB): Submit timing is surfaced correctly through the HTTP route.
//
// Proves: when a fill row has submit_ts_utc + submit_to_fill_ms, the causality
// route returns:
//   - nodes[0]: submit_event with timestamp == submit_ts_utc (rfc3339, no transform)
//   - nodes[1]: execution_fill with submit_ts_utc + submit_to_fill_ms matching DB exactly
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ca09_db_submit_timing_surfaced_through_route() {
    let pool = match connect_db().await {
        Some(p) => p,
        None => {
            eprintln!("CA-09: skipping — MQK_DATABASE_URL not set");
            return;
        }
    };

    let run_id = Uuid::parse_str("ca090000-cafe-4000-8000-000000000009").unwrap();
    seed_run(&pool, run_id).await;

    // Fixed deterministic timing values.
    let submit_ts: chrono::DateTime<Utc> =
        chrono::DateTime::parse_from_rfc3339("2026-01-15T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
    let fill_received_at: chrono::DateTime<Utc> =
        chrono::DateTime::parse_from_rfc3339("2026-01-15T10:00:01Z")
            .unwrap()
            .with_timezone(&Utc);
    let submit_to_fill_ms: i64 = 1000; // 1 second exactly

    let order_id = "ord-ca09-submit";
    let msg_id = "ca09-fill-msg-1";

    mqk_db::insert_fill_quality_telemetry(
        &pool,
        &mqk_db::NewFillQualityTelemetry {
            telemetry_id: telemetry_id(run_id, msg_id),
            run_id,
            internal_order_id: order_id.to_string(),
            broker_order_id: Some("brk-ca09".to_string()),
            broker_fill_id: Some("fill-ca09".to_string()),
            broker_message_id: msg_id.to_string(),
            symbol: "NVDA".to_string(),
            side: "buy".to_string(),
            ordered_qty: 10,
            fill_qty: 10,
            fill_price_micros: 500_000_000,
            reference_price_micros: None,
            slippage_bps: None,
            submit_ts_utc: Some(submit_ts),
            fill_received_at_utc: fill_received_at,
            submit_to_fill_ms: Some(submit_to_fill_ms),
            fill_kind: "final_fill".to_string(),
            provenance_ref: format!("oms_inbox:{msg_id}"),
            created_at_utc: Utc::now(),
        },
    )
    .await
    .expect("CA-09: insert fill must succeed");

    mqk_db::arm_run(&pool, run_id)
        .await
        .expect("CA-09: arm_run must succeed");

    let st = AppState::new_with_db_and_operator_auth(
        pool.clone(),
        mqk_daemon::state::OperatorAuthMode::ExplicitDevNoToken,
    );
    let router = build_router(Arc::new(st));

    let (status, body_bytes) = call(
        router,
        Request::builder()
            .method("GET")
            .uri(format!("/api/v1/execution/orders/{order_id}/causality"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(
        status,
        200,
        "CA-09: route must return 200; body: {}",
        String::from_utf8_lossy(&body_bytes)
    );

    let body: Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(
        body["truth_state"].as_str().unwrap(),
        "partial",
        "CA-09: truth_state must be partial when fills exist"
    );

    let nodes = body["nodes"]
        .as_array()
        .expect("CA-09: nodes must be array");
    assert_eq!(
        nodes.len(),
        2,
        "CA-09: must have 2 nodes: submit_event + execution_fill"
    );

    // ---- nodes[0]: synthetic submit_event anchor ----

    assert_eq!(
        nodes[0]["node_type"].as_str().unwrap(),
        "submit_event",
        "CA-09: nodes[0] must be submit_event"
    );

    // The anchor timestamp must round-trip exactly through to_rfc3339().
    let expected_submit_ts_str = submit_ts.to_rfc3339();
    assert_eq!(
        nodes[0]["timestamp"].as_str().unwrap(),
        expected_submit_ts_str,
        "CA-09: submit_event timestamp must equal DB submit_ts_utc (no transformation)"
    );

    // The submit_event node itself must NOT carry submit_ts_utc / submit_to_fill_ms.
    assert!(
        nodes[0]["submit_ts_utc"].is_null(),
        "CA-09: submit_event node must not carry submit_ts_utc"
    );
    assert!(
        nodes[0]["submit_to_fill_ms"].is_null(),
        "CA-09: submit_event node must not carry submit_to_fill_ms"
    );

    // ---- nodes[1]: execution_fill with timing back-reference ----

    assert_eq!(
        nodes[1]["node_type"].as_str().unwrap(),
        "execution_fill",
        "CA-09: nodes[1] must be execution_fill"
    );
    assert_eq!(
        nodes[1]["submit_ts_utc"].as_str().unwrap(),
        expected_submit_ts_str,
        "CA-09: fill node submit_ts_utc must match DB value exactly (no transformation)"
    );
    assert_eq!(
        nodes[1]["submit_to_fill_ms"].as_i64().unwrap(),
        submit_to_fill_ms,
        "CA-09: fill node submit_to_fill_ms must match DB value exactly (no transformation)"
    );
}

// ---------------------------------------------------------------------------
// CA-10 (DB): Null-safe — null submit_ts produces no submit_event node.
//
// Proves: when submit_ts_utc is NULL in the DB:
//   - No submit_event node is prepended
//   - The execution_fill node has null submit_ts_utc and submit_to_fill_ms
//   - No values are fabricated
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ca10_db_null_submit_ts_produces_no_submit_event_node() {
    let pool = match connect_db().await {
        Some(p) => p,
        None => {
            eprintln!("CA-10: skipping — MQK_DATABASE_URL not set");
            return;
        }
    };

    let run_id = Uuid::parse_str("ca100000-cafe-4000-8000-000000000010").unwrap();
    seed_run(&pool, run_id).await;

    let fill_received_at: chrono::DateTime<Utc> =
        chrono::DateTime::parse_from_rfc3339("2026-01-15T11:00:01Z")
            .unwrap()
            .with_timezone(&Utc);

    let order_id = "ord-ca10-null-submit";
    let msg_id = "ca10-fill-msg-1";

    mqk_db::insert_fill_quality_telemetry(
        &pool,
        &mqk_db::NewFillQualityTelemetry {
            telemetry_id: telemetry_id(run_id, msg_id),
            run_id,
            internal_order_id: order_id.to_string(),
            broker_order_id: None,
            broker_fill_id: Some("fill-ca10".to_string()),
            broker_message_id: msg_id.to_string(),
            symbol: "AAPL".to_string(),
            side: "buy".to_string(),
            ordered_qty: 5,
            fill_qty: 5,
            fill_price_micros: 200_000_000,
            reference_price_micros: None,
            slippage_bps: None,
            submit_ts_utc: None, // null — no submit anchor
            fill_received_at_utc: fill_received_at,
            submit_to_fill_ms: None, // null — no fabricated timing
            fill_kind: "final_fill".to_string(),
            provenance_ref: format!("oms_inbox:{msg_id}"),
            created_at_utc: Utc::now(),
        },
    )
    .await
    .expect("CA-10: insert fill must succeed");

    mqk_db::arm_run(&pool, run_id)
        .await
        .expect("CA-10: arm_run must succeed");

    let st = AppState::new_with_db_and_operator_auth(
        pool.clone(),
        mqk_daemon::state::OperatorAuthMode::ExplicitDevNoToken,
    );
    let router = build_router(Arc::new(st));

    let (status, body_bytes) = call(
        router,
        Request::builder()
            .method("GET")
            .uri(format!("/api/v1/execution/orders/{order_id}/causality"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(
        status,
        200,
        "CA-10: route must return 200; body: {}",
        String::from_utf8_lossy(&body_bytes)
    );

    let body: Value = serde_json::from_slice(&body_bytes).unwrap();

    let nodes = body["nodes"]
        .as_array()
        .expect("CA-10: nodes must be array");

    // No submit_event node when submit_ts_utc is null.
    let submit_nodes: Vec<&Value> = nodes
        .iter()
        .filter(|n| n["node_type"].as_str() == Some("submit_event"))
        .collect();
    assert!(
        submit_nodes.is_empty(),
        "CA-10: no submit_event node must be present when submit_ts_utc is null; \
         got: {submit_nodes:?}"
    );

    // Exactly one execution_fill node.
    let fill_nodes: Vec<&Value> = nodes
        .iter()
        .filter(|n| n["node_type"].as_str() == Some("execution_fill"))
        .collect();
    assert_eq!(
        fill_nodes.len(),
        1,
        "CA-10: exactly one execution_fill node"
    );

    // Fill node timing fields are null — nothing fabricated.
    assert!(
        fill_nodes[0]["submit_ts_utc"].is_null(),
        "CA-10: submit_ts_utc must be null on fill node when absent from DB"
    );
    assert!(
        fill_nodes[0]["submit_to_fill_ms"].is_null(),
        "CA-10: submit_to_fill_ms must be null on fill node when absent from DB"
    );
}

// ---------------------------------------------------------------------------
// CA-11 (DB): Submit node ordering — submit_event first, fills after;
//             elapsed_from_prev_ms of first fill equals submit_to_fill_ms.
//
// Proves: with 2 fills sharing the same submit_ts_utc:
//   - nodes[0] is submit_event (timestamp == submit_ts_utc)
//   - nodes[1] is first fill (partial_fill)
//   - nodes[2] is second fill (final_fill)
//   - nodes[1].elapsed_from_prev_ms == nodes[1].submit_to_fill_ms
//     (both measure the same submit_ts → fill1 interval)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ca11_db_submit_node_is_first_and_elapsed_matches_submit_to_fill_ms() {
    let pool = match connect_db().await {
        Some(p) => p,
        None => {
            eprintln!("CA-11: skipping — MQK_DATABASE_URL not set");
            return;
        }
    };

    let run_id = Uuid::parse_str("ca110000-cafe-4000-8000-000000000011").unwrap();
    seed_run(&pool, run_id).await;

    // Fixed deterministic timing values.
    let submit_ts: chrono::DateTime<Utc> =
        chrono::DateTime::parse_from_rfc3339("2026-01-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
    let fill1_received_at: chrono::DateTime<Utc> =
        chrono::DateTime::parse_from_rfc3339("2026-01-15T12:00:01Z")
            .unwrap()
            .with_timezone(&Utc);
    let fill2_received_at: chrono::DateTime<Utc> =
        chrono::DateTime::parse_from_rfc3339("2026-01-15T12:00:02Z")
            .unwrap()
            .with_timezone(&Utc);

    // submit_to_fill_ms set to match the actual interval so that
    // elapsed_from_prev_ms (computed by route) equals the stored value.
    let submit_to_fill_ms_1: i64 =
        fill1_received_at.timestamp_millis() - submit_ts.timestamp_millis(); // 1000
    let submit_to_fill_ms_2: i64 =
        fill2_received_at.timestamp_millis() - submit_ts.timestamp_millis(); // 2000

    let order_id = "ord-ca11-ordering";
    let msg1 = "ca11-fill-msg-1";
    let msg2 = "ca11-fill-msg-2";

    // Insert fill1 (partial_fill, older timestamp).
    mqk_db::insert_fill_quality_telemetry(
        &pool,
        &mqk_db::NewFillQualityTelemetry {
            telemetry_id: telemetry_id(run_id, msg1),
            run_id,
            internal_order_id: order_id.to_string(),
            broker_order_id: Some("brk-ca11".to_string()),
            broker_fill_id: Some("fill-ca11-1".to_string()),
            broker_message_id: msg1.to_string(),
            symbol: "NVDA".to_string(),
            side: "buy".to_string(),
            ordered_qty: 10,
            fill_qty: 5,
            fill_price_micros: 500_000_000,
            reference_price_micros: None,
            slippage_bps: None,
            submit_ts_utc: Some(submit_ts),
            fill_received_at_utc: fill1_received_at,
            submit_to_fill_ms: Some(submit_to_fill_ms_1),
            fill_kind: "partial_fill".to_string(),
            provenance_ref: format!("oms_inbox:{msg1}"),
            created_at_utc: Utc::now(),
        },
    )
    .await
    .expect("CA-11: insert fill1 must succeed");

    // Insert fill2 (final_fill, newer timestamp).
    mqk_db::insert_fill_quality_telemetry(
        &pool,
        &mqk_db::NewFillQualityTelemetry {
            telemetry_id: telemetry_id(run_id, msg2),
            run_id,
            internal_order_id: order_id.to_string(),
            broker_order_id: Some("brk-ca11".to_string()),
            broker_fill_id: Some("fill-ca11-2".to_string()),
            broker_message_id: msg2.to_string(),
            symbol: "NVDA".to_string(),
            side: "buy".to_string(),
            ordered_qty: 10,
            fill_qty: 5,
            fill_price_micros: 500_100_000,
            reference_price_micros: None,
            slippage_bps: None,
            submit_ts_utc: Some(submit_ts),
            fill_received_at_utc: fill2_received_at,
            submit_to_fill_ms: Some(submit_to_fill_ms_2),
            fill_kind: "final_fill".to_string(),
            provenance_ref: format!("oms_inbox:{msg2}"),
            created_at_utc: Utc::now(),
        },
    )
    .await
    .expect("CA-11: insert fill2 must succeed");

    mqk_db::arm_run(&pool, run_id)
        .await
        .expect("CA-11: arm_run must succeed");

    let st = AppState::new_with_db_and_operator_auth(
        pool.clone(),
        mqk_daemon::state::OperatorAuthMode::ExplicitDevNoToken,
    );
    let router = build_router(Arc::new(st));

    let (status, body_bytes) = call(
        router,
        Request::builder()
            .method("GET")
            .uri(format!("/api/v1/execution/orders/{order_id}/causality"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(
        status,
        200,
        "CA-11: route must return 200; body: {}",
        String::from_utf8_lossy(&body_bytes)
    );

    let body: Value = serde_json::from_slice(&body_bytes).unwrap();

    let nodes = body["nodes"]
        .as_array()
        .expect("CA-11: nodes must be array");
    assert_eq!(
        nodes.len(),
        3,
        "CA-11: must have 3 nodes: submit_event + 2 execution_fills"
    );

    // ---- nodes[0]: submit_event anchor ----

    assert_eq!(
        nodes[0]["node_type"].as_str().unwrap(),
        "submit_event",
        "CA-11: nodes[0] must be submit_event"
    );
    assert_eq!(
        nodes[0]["timestamp"].as_str().unwrap(),
        submit_ts.to_rfc3339(),
        "CA-11: submit_event timestamp must equal submit_ts_utc from DB"
    );

    // ---- nodes[1]: first fill (partial_fill, chronologically earlier) ----

    assert_eq!(
        nodes[1]["node_type"].as_str().unwrap(),
        "execution_fill",
        "CA-11: nodes[1] must be execution_fill"
    );
    assert_eq!(
        nodes[1]["title"].as_str().unwrap(),
        "partial_fill NVDA",
        "CA-11: nodes[1] title must reflect partial_fill for NVDA"
    );

    // ---- nodes[2]: second fill (final_fill, chronologically later) ----

    assert_eq!(
        nodes[2]["node_type"].as_str().unwrap(),
        "execution_fill",
        "CA-11: nodes[2] must be execution_fill"
    );
    assert_eq!(
        nodes[2]["title"].as_str().unwrap(),
        "final_fill NVDA",
        "CA-11: nodes[2] title must reflect final_fill for NVDA"
    );

    // ---- Elapsed proof: nodes[1].elapsed_from_prev_ms == submit_to_fill_ms_1 ----
    //
    // The route computes elapsed_from_prev_ms as:
    //   fill1.timestamp_millis() - submit_anchor.timestamp_millis()
    //
    // submit_to_fill_ms_1 was set to the same interval:
    //   fill1_received_at.timestamp_millis() - submit_ts.timestamp_millis()
    //
    // Therefore both must be equal.

    let elapsed_fill1 = nodes[1]["elapsed_from_prev_ms"]
        .as_i64()
        .expect("CA-11: nodes[1].elapsed_from_prev_ms must be i64 (not null)");

    assert_eq!(
        elapsed_fill1, submit_to_fill_ms_1,
        "CA-11: nodes[1].elapsed_from_prev_ms ({elapsed_fill1}) must equal \
         submit_to_fill_ms_1 ({submit_to_fill_ms_1}) — both measure \
         the submit_ts → first_fill interval"
    );

    // All fill nodes must carry the submit timing back-reference (not null).
    for (i, fill) in [&nodes[1], &nodes[2]].iter().enumerate() {
        assert!(
            !fill["submit_ts_utc"].is_null(),
            "CA-11: fill nodes[{i}] must carry non-null submit_ts_utc"
        );
        assert!(
            !fill["submit_to_fill_ms"].is_null(),
            "CA-11: fill nodes[{i}] must carry non-null submit_to_fill_ms"
        );
    }
}

// ---------------------------------------------------------------------------
// CA-12 (DB): Intent lane proven — outbox_enqueued node present, intent in
//             proven_lanes, intent absent from unproven_lanes.
//
// Proves: when an oms_outbox row exists for the order_id:
//   - nodes contains an outbox_enqueued node with correct timestamp
//   - proven_lanes includes "intent"
//   - unproven_lanes does NOT include "intent"
//   - truth_state is "partial" (intent lane alone is sufficient)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ca12_db_intent_lane_proven_when_outbox_row_present() {
    let pool = match connect_db().await {
        Some(p) => p,
        None => {
            eprintln!("CA-12: skipping — MQK_DATABASE_URL not set");
            return;
        }
    };

    let run_id = Uuid::parse_str("ca120000-cafe-4000-8000-000000000012").unwrap();
    seed_run(&pool, run_id).await;

    let order_id = "ord-ca12-intent";

    // Deterministic outbox_id via UUIDv5 is not needed — use a fixed timestamp.
    let enqueued_at: chrono::DateTime<Utc> =
        chrono::DateTime::parse_from_rfc3339("2026-01-15T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

    // Insert outbox row (SENT status, no fill rows — intent lane only).
    sqlx::query(
        r#"
        insert into oms_outbox
            (run_id, idempotency_key, order_json, status, created_at_utc)
        values
            ($1, $2, '{"symbol":"NVDA","side":"buy","qty":10}'::jsonb, 'SENT', $3)
        on conflict (idempotency_key) do nothing
        "#,
    )
    .bind(run_id)
    .bind(order_id)
    .bind(enqueued_at)
    .execute(&pool)
    .await
    .expect("CA-12: insert outbox row must succeed");

    mqk_db::arm_run(&pool, run_id)
        .await
        .expect("CA-12: arm_run must succeed");

    let st = AppState::new_with_db_and_operator_auth(
        pool.clone(),
        mqk_daemon::state::OperatorAuthMode::ExplicitDevNoToken,
    );
    let router = build_router(Arc::new(st));

    let (status, body_bytes) = call(
        router,
        Request::builder()
            .method("GET")
            .uri(format!("/api/v1/execution/orders/{order_id}/causality"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(
        status,
        200,
        "CA-12: route must return 200; body: {}",
        String::from_utf8_lossy(&body_bytes)
    );

    let body: Value = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(
        body["truth_state"].as_str().unwrap(),
        "partial",
        "CA-12: truth_state must be partial when outbox row exists"
    );

    // "intent" must be in proven_lanes.
    let proven = body["proven_lanes"]
        .as_array()
        .expect("CA-12: proven_lanes must be array");
    let proven_strs: Vec<&str> = proven.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        proven_strs.contains(&"intent"),
        "CA-12: proven_lanes must include 'intent' when outbox row present; got: {proven_strs:?}"
    );

    // "intent" must NOT be in unproven_lanes.
    let unproven = body["unproven_lanes"]
        .as_array()
        .expect("CA-12: unproven_lanes must be array");
    let unproven_strs: Vec<&str> = unproven.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        !unproven_strs.contains(&"intent"),
        "CA-12: unproven_lanes must NOT include 'intent' when outbox row present; got: {unproven_strs:?}"
    );

    // outbox_enqueued node must be present.
    let nodes = body["nodes"]
        .as_array()
        .expect("CA-12: nodes must be array");
    let enqueued_nodes: Vec<&Value> = nodes
        .iter()
        .filter(|n| n["node_type"].as_str() == Some("outbox_enqueued"))
        .collect();
    assert_eq!(
        enqueued_nodes.len(),
        1,
        "CA-12: exactly one outbox_enqueued node expected; nodes: {nodes:?}"
    );

    // Timestamp must match what was inserted.
    assert_eq!(
        enqueued_nodes[0]["timestamp"].as_str().unwrap(),
        enqueued_at.to_rfc3339(),
        "CA-12: outbox_enqueued timestamp must match created_at_utc from DB"
    );

    // node_key must be deterministic.
    assert_eq!(
        enqueued_nodes[0]["node_key"].as_str().unwrap(),
        format!("outbox_enqueued:{order_id}"),
        "CA-12: outbox_enqueued node_key must be deterministic"
    );
}

// ---------------------------------------------------------------------------
// CA-13 (DB): outbox_sent node present when sent_at_utc is set; ordering is
//             outbox_enqueued → outbox_sent (both before any fill nodes).
//
// Proves: when oms_outbox has both created_at_utc and sent_at_utc:
//   - Two outbox nodes: outbox_enqueued at index 0, outbox_sent at index 1
//   - outbox_sent.elapsed_from_prev_ms == sent_at - enqueued_at (millis)
//   - No outbox_sent node when sent_at_utc is NULL (null-safe)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ca13_db_outbox_sent_node_present_when_sent_at_set() {
    let pool = match connect_db().await {
        Some(p) => p,
        None => {
            eprintln!("CA-13: skipping — MQK_DATABASE_URL not set");
            return;
        }
    };

    let run_id = Uuid::parse_str("ca130000-cafe-4000-8000-000000000013").unwrap();
    seed_run(&pool, run_id).await;

    let order_id = "ord-ca13-sent";

    let enqueued_at: chrono::DateTime<Utc> =
        chrono::DateTime::parse_from_rfc3339("2026-01-15T14:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
    let sent_at: chrono::DateTime<Utc> =
        chrono::DateTime::parse_from_rfc3339("2026-01-15T14:00:00.250Z")
            .unwrap()
            .with_timezone(&Utc);
    let expected_elapsed_ms = sent_at.timestamp_millis() - enqueued_at.timestamp_millis(); // 250

    // Insert outbox row with sent_at_utc set.
    sqlx::query(
        r#"
        insert into oms_outbox
            (run_id, idempotency_key, order_json, status, created_at_utc, sent_at_utc)
        values
            ($1, $2, '{"symbol":"AAPL","side":"sell","qty":5}'::jsonb, 'ACKED', $3, $4)
        on conflict (idempotency_key) do nothing
        "#,
    )
    .bind(run_id)
    .bind(order_id)
    .bind(enqueued_at)
    .bind(sent_at)
    .execute(&pool)
    .await
    .expect("CA-13: insert outbox row must succeed");

    mqk_db::arm_run(&pool, run_id)
        .await
        .expect("CA-13: arm_run must succeed");

    let st = AppState::new_with_db_and_operator_auth(
        pool.clone(),
        mqk_daemon::state::OperatorAuthMode::ExplicitDevNoToken,
    );
    let router = build_router(Arc::new(st));

    let (status, body_bytes) = call(
        router,
        Request::builder()
            .method("GET")
            .uri(format!("/api/v1/execution/orders/{order_id}/causality"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(
        status,
        200,
        "CA-13: route must return 200; body: {}",
        String::from_utf8_lossy(&body_bytes)
    );

    let body: Value = serde_json::from_slice(&body_bytes).unwrap();

    let nodes = body["nodes"]
        .as_array()
        .expect("CA-13: nodes must be array");

    // First two nodes must be outbox intent nodes in order.
    assert!(
        nodes.len() >= 2,
        "CA-13: at least 2 nodes expected (enqueued + sent); got: {}",
        nodes.len()
    );
    assert_eq!(
        nodes[0]["node_type"].as_str().unwrap(),
        "outbox_enqueued",
        "CA-13: nodes[0] must be outbox_enqueued"
    );
    assert_eq!(
        nodes[1]["node_type"].as_str().unwrap(),
        "outbox_sent",
        "CA-13: nodes[1] must be outbox_sent"
    );

    // Timestamps must match DB values.
    assert_eq!(
        nodes[0]["timestamp"].as_str().unwrap(),
        enqueued_at.to_rfc3339(),
        "CA-13: outbox_enqueued timestamp must match created_at_utc"
    );
    assert_eq!(
        nodes[1]["timestamp"].as_str().unwrap(),
        sent_at.to_rfc3339(),
        "CA-13: outbox_sent timestamp must match sent_at_utc"
    );

    // elapsed_from_prev_ms on outbox_sent must equal sent_at - enqueued_at.
    assert_eq!(
        nodes[1]["elapsed_from_prev_ms"].as_i64().unwrap(),
        expected_elapsed_ms,
        "CA-13: outbox_sent.elapsed_from_prev_ms must equal sent_at - enqueued_at millis"
    );

    // outbox_enqueued has no elapsed (it is the first intent node).
    assert!(
        nodes[0]["elapsed_from_prev_ms"].is_null(),
        "CA-13: outbox_enqueued must have null elapsed_from_prev_ms (first node)"
    );
}

// ---------------------------------------------------------------------------
// CA-15 (DB): broker_ack lane proven — oms_inbox ACK row surfaces as
//             broker_ack node; broker_ack in proven_lanes, not unproven_lanes.
//
// Proves: when an oms_inbox row with event_kind='ack' and internal_order_id
// matching the order_id exists for the active run:
//   - Exactly one broker_ack node present in nodes
//   - node_type == "broker_ack"
//   - linked_id carries the broker_message_id
//   - timestamp matches received_at_utc exactly (no transformation)
//   - proven_lanes includes "broker_ack"
//   - unproven_lanes does NOT include "broker_ack"
//   - truth_state is "partial"
//   - backend includes "postgres.oms_inbox"
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ca15_db_broker_ack_lane_proven_when_inbox_ack_row_present() {
    let pool = match connect_db().await {
        Some(p) => p,
        None => {
            eprintln!("CA-15: skipping — MQK_DATABASE_URL not set");
            return;
        }
    };

    let run_id = Uuid::parse_str("ca150000-cafe-4000-8000-000000000015").unwrap();
    seed_run(&pool, run_id).await;

    let order_id = "ord-ca15-broker-ack";
    let broker_msg_id = "alpaca:ord-ca15-broker-ack:new:2026-01-15T15:00:00.000Z";

    let received_at: chrono::DateTime<Utc> =
        chrono::DateTime::parse_from_rfc3339("2026-01-15T15:00:00.050Z")
            .unwrap()
            .with_timezone(&Utc);

    // Insert an oms_inbox row simulating a WS ACK event.
    // Mirrors what alpaca_inbound::process_ws_inbound_batch writes for BrokerEvent::Ack.
    sqlx::query(
        r#"
        insert into oms_inbox (
            run_id, broker_message_id, broker_fill_id,
            internal_order_id, broker_order_id, event_kind,
            message_json, event_ts_ms, received_at_utc, applied_at_utc
        )
        values ($1, $2, null, $3, $3, 'ack', '{}'::jsonb, 0, $4, null)
        on conflict (run_id, broker_message_id) do nothing
        "#,
    )
    .bind(run_id)
    .bind(broker_msg_id)
    .bind(order_id)
    .bind(received_at)
    .execute(&pool)
    .await
    .expect("CA-15: insert oms_inbox ack row must succeed");

    mqk_db::arm_run(&pool, run_id)
        .await
        .expect("CA-15: arm_run must succeed");

    let st = AppState::new_with_db_and_operator_auth(
        pool.clone(),
        mqk_daemon::state::OperatorAuthMode::ExplicitDevNoToken,
    );
    let router = build_router(Arc::new(st));

    let (status, body_bytes) = call(
        router,
        Request::builder()
            .method("GET")
            .uri(format!("/api/v1/execution/orders/{order_id}/causality"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(
        status,
        200,
        "CA-15: route must return 200; body: {}",
        String::from_utf8_lossy(&body_bytes)
    );

    let body: Value = serde_json::from_slice(&body_bytes).unwrap();

    // truth_state must be "partial" — broker_ack lane alone is sufficient.
    assert_eq!(
        body["truth_state"].as_str().unwrap(),
        "partial",
        "CA-15: truth_state must be partial when ack row exists"
    );

    // backend must include oms_inbox.
    let backend = body["backend"].as_str().unwrap_or("");
    assert!(
        backend.contains("oms_inbox"),
        "CA-15: backend must include oms_inbox when ack rows present; got: {backend}"
    );

    // proven_lanes must include "broker_ack".
    let proven = body["proven_lanes"]
        .as_array()
        .expect("CA-15: proven_lanes must be array");
    let proven_strs: Vec<&str> = proven.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        proven_strs.contains(&"broker_ack"),
        "CA-15: proven_lanes must include broker_ack; got: {proven_strs:?}"
    );

    // unproven_lanes must NOT include "broker_ack".
    let unproven = body["unproven_lanes"]
        .as_array()
        .expect("CA-15: unproven_lanes must be array");
    let unproven_strs: Vec<&str> = unproven.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        !unproven_strs.contains(&"broker_ack"),
        "CA-15: unproven_lanes must NOT include broker_ack when ack row present; \
         got: {unproven_strs:?}"
    );

    // Exactly one broker_ack node.
    let nodes = body["nodes"]
        .as_array()
        .expect("CA-15: nodes must be array");
    let ack_nodes: Vec<&Value> = nodes
        .iter()
        .filter(|n| n["node_type"].as_str() == Some("broker_ack"))
        .collect();
    assert_eq!(
        ack_nodes.len(),
        1,
        "CA-15: exactly one broker_ack node expected; nodes: {nodes:?}"
    );

    // linked_id carries the broker_message_id.
    assert_eq!(
        ack_nodes[0]["linked_id"].as_str().unwrap(),
        broker_msg_id,
        "CA-15: broker_ack linked_id must equal broker_message_id"
    );

    // timestamp matches received_at_utc exactly.
    assert_eq!(
        ack_nodes[0]["timestamp"].as_str().unwrap(),
        received_at.to_rfc3339(),
        "CA-15: broker_ack timestamp must match received_at_utc from oms_inbox"
    );
}
