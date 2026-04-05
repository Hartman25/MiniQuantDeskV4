//! # AUTON-HIST-01 — Autonomous supervisor-history durability proof
//!
//! ## Purpose
//!
//! Proves that autonomous session event persistence failures are operator-visible
//! rather than silently dropped.  The `autonomous_history_degraded` flag is set
//! whenever an event cannot be persisted (no DB configured OR DB write failure)
//! and is surfaced in `/api/v1/autonomous/readiness`.
//!
//! ## What this file proves
//!
//! | Test  | Claim                                                                                      |
//! |-------|--------------------------------------------------------------------------------------------|
//! | AH-01 | No-DB path: calling `set_autonomous_session_truth` on a no-DB Paper+Alpaca state sets     |
//! |       | `autonomous_history_degraded=true` (event cannot be persisted without a DB)                |
//! | AH-02 | Readiness surface reflects degraded state: `/api/v1/autonomous/readiness` returns         |
//! |       | `autonomous_history_degraded=true` when the flag is set, `false` when clean               |
//! | AH-03 | Flag is sticky: once set it is not cleared by a subsequent `set_autonomous_session_truth` |
//! | AH-04 | events/feed surfaces `kind="autonomous_session"` rows from                                |
//! |       | `sys_autonomous_session_events` (AUTON-PAPER-02 route wiring proof).                      |
//! |       | Requires MQK_DATABASE_URL; skips when not set.                                             |
//!
//! ## What is NOT claimed
//!
//! - DB-write-failure path (requires a live DB with a forced write error; covered by
//!   the no-DB path which exercises the same `store(true)` code branch)
//!
use std::sync::Arc;

use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use state::{AlpacaWsContinuityState, AutonomousSessionTruth, BrokerKind};
use tower::ServiceExt;

async fn ah04_test_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "AH-04 requires MQK_DATABASE_URL; run with --include-ignored"
        )
    });
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("ah04_test_pool: connect failed")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_paper_alpaca_no_db() -> Arc<state::AppState> {
    // new_for_test_with_broker_kind does NOT wire a DB pool — this is the
    // no-DB path that should trigger the degraded flag.
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ))
}

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

// ---------------------------------------------------------------------------
// AH-01: No-DB path sets autonomous_history_degraded
// ---------------------------------------------------------------------------

/// AH-01: On a no-DB Paper+Alpaca daemon, calling `set_autonomous_session_truth`
/// for a persistable truth variant marks `autonomous_history_degraded = true`.
///
/// This is the key AUTON-HIST-01 invariant: the operator is never left believing
/// history is durable when the DB is absent.
#[tokio::test]
async fn ah01_no_db_set_truth_marks_degraded() {
    let st = make_paper_alpaca_no_db();

    // Precondition: flag starts clean.
    assert!(
        !st.autonomous_history_degraded(),
        "degraded flag must start false"
    );

    // Trigger a persistable truth transition (StartRefused is a real event type).
    st.set_autonomous_session_truth(AutonomousSessionTruth::StartRefused {
        detail: "ah01_test".to_string(),
    })
    .await;

    // Flag must now be set because no DB was wired.
    assert!(
        st.autonomous_history_degraded(),
        "autonomous_history_degraded must be true after no-DB persist attempt"
    );
}

// ---------------------------------------------------------------------------
// AH-02: Readiness surface reflects degraded flag
// ---------------------------------------------------------------------------

/// AH-02: `/api/v1/autonomous/readiness` surfaces `autonomous_history_degraded`
/// correctly — false on a clean state, true after a failed persist.
#[tokio::test]
async fn ah02_readiness_surface_reflects_degraded_flag() {
    let st = make_paper_alpaca_no_db();

    // Make WS continuity Live so the response is `truth_state="active"` and
    // we can inspect the full field set.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "ah02_msg".to_string(),
        last_event_at: "2026-04-04T14:30:00Z".to_string(),
    })
    .await;

    let router = routes::build_router(st.clone().into());

    // --- Check A: flag not yet set → autonomous_history_degraded = false ---
    let req_a = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/autonomous/readiness")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status_a, body_a) = call(router.clone(), req_a).await;
    assert_eq!(status_a, StatusCode::OK, "readiness must return 200");
    let json_a = parse_json(body_a);
    assert_eq!(
        json_a["truth_state"], "active",
        "must be active for paper+alpaca"
    );
    assert_eq!(
        json_a["autonomous_history_degraded"], false,
        "AH-02A: flag must be false before any failed persist"
    );

    // --- Trigger a no-DB persist (sets the flag) ---
    st.set_autonomous_session_truth(AutonomousSessionTruth::StartRefused {
        detail: "ah02_test".to_string(),
    })
    .await;

    // --- Check B: flag now set → autonomous_history_degraded = true ---
    let req_b = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/autonomous/readiness")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status_b, body_b) = call(router, req_b).await;
    assert_eq!(status_b, StatusCode::OK);
    let json_b = parse_json(body_b);
    assert_eq!(
        json_b["autonomous_history_degraded"], true,
        "AH-02B: flag must be true after failed no-DB persist"
    );
}

// ---------------------------------------------------------------------------
// AH-03: Flag is sticky — not cleared by subsequent truth transitions
// ---------------------------------------------------------------------------

/// AH-03: Once `autonomous_history_degraded` is set, subsequent truth
/// transitions do not clear it.  Sticky semantics mean the operator cannot
/// miss the degradation by observing a later "good" transition.
#[tokio::test]
async fn ah03_degraded_flag_is_sticky() {
    let st = make_paper_alpaca_no_db();

    // Trigger first transition (no DB → sets degraded).
    st.set_autonomous_session_truth(AutonomousSessionTruth::StartRefused {
        detail: "ah03_first".to_string(),
    })
    .await;
    assert!(st.autonomous_history_degraded(), "must be true after first transition");

    // Trigger a second (different) truth transition.
    st.set_autonomous_session_truth(AutonomousSessionTruth::RecoverySucceeded {
        resume_source: state::AutonomousRecoveryResumeSource::ColdStart,
        detail: "ah03_second".to_string(),
    })
    .await;

    // Flag must remain set — it is sticky, not reset per-transition.
    assert!(
        st.autonomous_history_degraded(),
        "AH-03: degraded flag must remain true after second no-DB transition"
    );
}

// ---------------------------------------------------------------------------
// AH-04: events/feed surfaces autonomous_session kind rows (DB-backed proof)
// ---------------------------------------------------------------------------

/// AH-04: Proves that `GET /api/v1/events/feed` returns at least one row with
/// `kind = "autonomous_session"` after a row has been persisted to
/// `sys_autonomous_session_events` (AUTON-PAPER-02 route wiring proof).
///
/// This closes the truth gap between the autonomous_paper_ops.md runbook claim
/// ("Events appear as kind = 'autonomous_session' rows") and the code.
///
/// Requires MQK_DATABASE_URL; skips when not set.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ah04_events_feed_surfaces_autonomous_session_kind_rows() {
    let pool = ah04_test_pool().await;

    // Deterministic ID — unique namespace to avoid collisions.
    let row_id = "ah04-auton-hist-proof-001".to_string();
    let ts = chrono::DateTime::parse_from_rfc3339("2020-07-01T14:30:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    // Pre-test cleanup (idempotent).
    sqlx::query("delete from sys_autonomous_session_events where id = $1")
        .bind(&row_id)
        .execute(&pool)
        .await
        .expect("AH-04: pre-test cleanup failed");

    // Seed one autonomous-session event directly via the DB function.
    mqk_db::persist_autonomous_session_event(
        &pool,
        &mqk_db::AutonomousSessionEventRow {
            id: row_id.clone(),
            ts_utc: ts,
            event_type: "StartRefused".to_string(),
            resume_source: None,
            detail: "ah04_proof_seed".to_string(),
            run_id: None,
            source: "test.ah04".to_string(),
        },
    )
    .await
    .expect("AH-04: persist_autonomous_session_event failed");

    // Build daemon state with DB pool and call events/feed.
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/events/feed")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();

    assert_eq!(status, StatusCode::OK, "AH-04: events/feed must return 200");

    let json: serde_json::Value =
        serde_json::from_slice(&body).expect("AH-04: body is not valid JSON");

    // truth_state must be "active" (DB pool is present).
    assert_eq!(
        json["truth_state"], "active",
        "AH-04: truth_state must be 'active' with DB pool"
    );

    // backend must name all three source tables including sys_autonomous_session_events.
    assert_eq!(
        json["backend"],
        "postgres.runs+postgres.audit_events+postgres.sys_autonomous_session_events",
        "AH-04: backend must include sys_autonomous_session_events"
    );

    let rows = json
        .get("rows")
        .and_then(|v| v.as_array())
        .expect("AH-04: rows must be a JSON array");

    // Find the seeded autonomous_session row.
    let expected_event_id = format!("sys_autonomous_session_events:{row_id}");
    let auton_row = rows
        .iter()
        .find(|r| {
            r.get("event_id")
                .and_then(|v| v.as_str())
                .map(|v| v == expected_event_id)
                .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            panic!(
                "AH-04: autonomous_session row with event_id={expected_event_id} \
                 must appear in events/feed; got rows: {rows:?}"
            )
        });

    // kind must be "autonomous_session".
    assert_eq!(
        auton_row.get("kind").and_then(|v| v.as_str()),
        Some("autonomous_session"),
        "AH-04: kind must be 'autonomous_session'; got: {auton_row}"
    );

    // detail must equal the seeded event_type (no resume_source → plain event_type).
    assert_eq!(
        auton_row.get("detail").and_then(|v| v.as_str()),
        Some("StartRefused"),
        "AH-04: detail must equal seeded event_type; got: {auton_row}"
    );

    // provenance_ref must encode the source table and row id.
    assert_eq!(
        auton_row.get("provenance_ref").and_then(|v| v.as_str()),
        Some(expected_event_id.as_str()),
        "AH-04: provenance_ref must be 'sys_autonomous_session_events:{{id}}'; got: {auton_row}"
    );

    // Post-test cleanup.
    sqlx::query("delete from sys_autonomous_session_events where id = $1")
        .bind(&row_id)
        .execute(&pool)
        .await
        .expect("AH-04: post-test cleanup failed");
}
