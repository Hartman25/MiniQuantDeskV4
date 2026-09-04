//! KILL-SWITCH-FAIL-CLOSED-READ-ERROR-VERIFY-01: proves a durable
//! `sys_arm_state` READ FAILURE (not a missing row -- a genuine query
//! error) can never be represented by `current_status_snapshot` /
//! `GET /api/v1/system/status` as an inactive/safe/not-halted kill
//! switch.
//!
//! # Relationship to existing coverage
//!
//! `scenario_kill_switch_persistence_lo02e.rs` (KS-01..KS-04) proves the
//! SUCCESS path (durable ARMED/DISARMED correctly surfaces) and the
//! no-DB-pool-at-all path (KS-04). Neither forces the DB pool to be
//! present but the `sys_arm_state` QUERY ITSELF to fail -- that class of
//! test is documented as a known repo-wide gap
//! (`scenario_ctrl_arm_preflight_01.rs`: "simulating a DB query error
//! without corrupting the shared test schema... would require destructive
//! DB mutations or a broad production-code refactor to inject a mock
//! pool").
//!
//! # Fault injection technique (non-destructive)
//!
//! This file connects a SEPARATE `PgPool` to the exact same
//! `MQK_DATABASE_URL` (same credentials/host the sibling KS tests already
//! use successfully). Redirecting `search_path` alone to a nonexistent
//! schema was tried first and rejected: it also breaks the UNQUALIFIED
//! `runs` query `fetch_latest_run_for_engine` issues, which does not
//! isolate `sys_arm_state` -- it just breaks `current_status_snapshot` in a
//! different, less honest way (a `runs` lookup failure, surfaced as
//! `RuntimeLifecycleError::internal`, not the arm-state read failure this
//! scenario exists to prove).
//!
//! Instead, each connection this pool hands out:
//!   1. creates a `TEMP VIEW runs` (session-local, in `pg_temp` -- never
//!      touches the shared `runs` table) with zero rows, in the connection's
//!      default (`public`) schema visibility, matching the real `runs`
//!      table's exact column set so `fetch_latest_run_for_engine`'s query
//!      still type-checks and returns `Ok(None)` (the honest "no run yet"
//!      case);
//!   2. THEN sets `search_path` to `pg_temp` only, so the temp view still
//!      resolves for unqualified `runs` references, but `sys_arm_state`
//!      (which lives in the real default schema, never copied into
//!      `pg_temp`) does not resolve at all.
//!
//! `TEMP` objects are connection-scoped and dropped automatically when the
//! connection closes -- nothing is created, dropped, or altered in the
//! shared test schema itself, and no other test's data is touched.
//!
//! Run:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_kill_switch_status_read_error_fail_closed_01

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::routes;
use mqk_daemon::state::{AppState, BrokerKind, DeploymentMode};
use tower::ServiceExt;

/// A pool that connects successfully (proving credentials/host are fine)
/// and whose every connection can still resolve an empty `runs` (via a
/// session-local TEMP VIEW) but can never resolve `sys_arm_state`.
async fn fault_injecting_pool_or_skip() -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(v) => v,
        Err(_) => return None,
    };
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    // Column set/types must match crates/mqk-db/migrations
                    // 0001_init.sql + 0002_run_lifecycle.sql + 0063_runs_
                    // stop_requested.sql's real `runs` table exactly, so
                    // `fetch_latest_run_for_engine`'s select still
                    // type-checks against this temp view.
                    sqlx::query(
                        r#"
                        create temp view runs as
                        select
                          null::uuid as run_id,
                          null::text as engine_id,
                          null::text as mode,
                          null::timestamptz as started_at_utc,
                          null::text as git_hash,
                          null::text as config_hash,
                          null::jsonb as config_json,
                          null::text as host_fingerprint,
                          null::text as status,
                          null::timestamptz as armed_at_utc,
                          null::timestamptz as running_at_utc,
                          null::timestamptz as stopped_at_utc,
                          null::timestamptz as halted_at_utc,
                          null::timestamptz as last_heartbeat_utc,
                          null::timestamptz as stop_requested_at_utc
                        where false
                        "#,
                    )
                    .execute(&mut *conn)
                    .await?;
                    // pg_temp only -- `runs` resolves via the temp view
                    // just created; `sys_arm_state` (never in pg_temp)
                    // cannot resolve at all.
                    sqlx::query("SET search_path TO pg_temp")
                        .execute(&mut *conn)
                        .await
                        .map(|_| ())
                })
            })
            .connect(&url)
            .await
            .expect(
                "KILL-SWITCH-READ-ERROR-01: failed to connect to MQK_DATABASE_URL \
                 (same URL the sibling KS-01..04 tests connect with successfully)",
            ),
    )
}

/// RED proof that the fault-injection pool genuinely produces a query
/// error (not merely a connection or search_path-setting error) -- proves
/// this test's own fault injection is real before trusting the
/// production-code assertions below.
#[tokio::test]
async fn ksre01_fault_injection_pool_genuinely_fails_the_arm_state_query() {
    let Some(pool) = fault_injecting_pool_or_skip().await else {
        eprintln!("KSRE-01: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    let result = mqk_db::load_arm_state(&pool).await;
    assert!(
        result.is_err(),
        "KSRE-01: fault-injecting pool must make load_arm_state genuinely fail \
         (search_path restricted to pg_temp, where sys_arm_state does not exist); \
         got: {result:?}"
    );
}

/// Proves the fault-injecting pool genuinely ISOLATES the failure to
/// `sys_arm_state`: `fetch_latest_run_for_engine` -- the other durable read
/// `current_status_snapshot` makes on the SAME pool -- still succeeds (and
/// honestly reports "no run", via the temp view) rather than also failing.
/// A harness that broke both reads would not prove anything about the
/// arm-state-specific fail-closed behavior below.
#[tokio::test]
async fn ksre01b_runs_lookup_still_succeeds_on_the_same_fault_injecting_pool() {
    let Some(pool) = fault_injecting_pool_or_skip().await else {
        eprintln!("KSRE-01b: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    let result = mqk_db::fetch_latest_run_for_engine(&pool, "mqk-daemon", "PAPER").await;
    assert!(
        result.is_ok(),
        "KSRE-01b: fetch_latest_run_for_engine must still succeed on the same pool \
         that makes load_arm_state fail -- the fault must be isolated to \
         sys_arm_state, not the whole connection; got: {result:?}"
    );
    assert!(
        result.unwrap().is_none(),
        "KSRE-01b: the temp `runs` view is intentionally empty (no run) -- a \
         non-None result would mean the fixture accidentally matched a real row"
    );
}

/// The real production assertion: with a DB pool present but every
/// `sys_arm_state` read failing, `current_status_snapshot` must fail
/// closed -- `integrity_armed=false`, `state="halted"` (so
/// `GET /api/v1/system/status`'s `kill_switch_active` -- derived as
/// `state == "halted"` -- can never report the kill switch inactive) --
/// never silently falling through to whatever the fresh in-memory
/// default happened to be.
#[tokio::test]
async fn ksre02_durable_read_error_fails_closed_never_reports_inactive_kill_switch() {
    let Some(pool) = fault_injecting_pool_or_skip().await else {
        eprintln!("KSRE-02: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));

    let snapshot = st
        .current_status_snapshot()
        .await
        .expect("KSRE-02: current_status_snapshot must not itself error on an arm-state read failure -- it must fail CLOSED (halted), not propagate a hard error or fall through to an unsafe default");

    assert!(
        !snapshot.integrity_armed,
        "KSRE-02: a durable arm-state read failure must never report integrity_armed=true \
         (would surface as strategy_armed/execution_armed=true); got: {:?}",
        snapshot.integrity_armed
    );
    assert_eq!(
        snapshot.state, "halted",
        "KSRE-02: a durable arm-state read failure must never report a state other than \
         'halted' when there is no active run -- GET /api/v1/system/status derives \
         kill_switch_active as (state == \"halted\"), so anything else here means the \
         kill switch reports as INACTIVE on a genuine read failure; got: {:?}",
        snapshot.state
    );
    assert!(
        snapshot
            .notes
            .as_deref()
            .is_some_and(|n| n.contains("durable arm-state read failed")),
        "KSRE-02: the read failure must be honestly surfaced in notes, not silently \
         swallowed; got: {:?}",
        snapshot.notes
    );
}

/// Negative control on the negative control: with the SAME fault-injecting
/// pool but no rows/queries actually failing (a query that does not touch
/// `sys_arm_state` at all), the daemon behaves normally -- proves the
/// prior test's failure is specifically about the `sys_arm_state` read,
/// not a general "any DB present at all" miscoding.
#[tokio::test]
async fn ksre03_fault_injection_is_scoped_to_arm_state_not_all_db_reads() {
    let Some(pool) = fault_injecting_pool_or_skip().await else {
        eprintln!("KSRE-03: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    // pg_catalog is always searchable regardless of search_path content --
    // proves the pool/connection itself is healthy and this is a targeted
    // relation-resolution failure, not a broken connection.
    let row: (i32,) = sqlx::query_as("select 1")
        .fetch_one(&pool)
        .await
        .expect("KSRE-03: a query with no dependency on sys_arm_state must still succeed");
    assert_eq!(row.0, 1);
}

/// The full end-to-end production surface: `GET /api/v1/system/status` must
/// itself report `kill_switch_active: true` on a genuine arm-state read
/// error -- not just the underlying `StatusSnapshot` this route derives it
/// from (`kill_switch_active: status.state == "halted"`).
#[tokio::test]
async fn ksre04_system_status_route_reports_kill_switch_active_on_read_error() {
    let Some(pool) = fault_injecting_pool_or_skip().await else {
        eprintln!("KSRE-04: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));

    let resp = routes::build_router(Arc::clone(&st))
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/system/status")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .expect("KSRE-04: oneshot failed");
    assert_eq!(resp.status(), StatusCode::OK, "KSRE-04: status route must not hard-error on an arm-state read failure");
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("KSRE-04: body collect failed")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("KSRE-04: response must be valid JSON");

    assert_eq!(
        json.get("kill_switch_active"),
        Some(&serde_json::Value::Bool(true)),
        "KSRE-04: GET /api/v1/system/status must report kill_switch_active=true on a \
         genuine arm-state read error; got: {json}"
    );
}
