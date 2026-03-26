//! CC-03B: Durable restart intent / restart provenance — proof tests.
//!
//! Proves that `sys_restart_intent` is a durable, queryable seam for restart
//! intent and provenance that remains coherent with the CC-03A canonical
//! mode-transition truth and does not rely on transient in-memory inference.
//!
//! # Proof matrix
//!
//! | Test    | What it proves                                                             |
//! |---------|----------------------------------------------------------------------------|
//! | RI-01   | No DB → load_pending_restart_intent returns None (honest absence)          |
//! | RI-02   | Insert + fetch round-trip: all fields preserved durably                    |
//! | RI-03   | transition_verdict in the DB matches CC-03A evaluate_mode_transition output |
//! |         |   (Paper→LiveShadow: "admissible_with_restart" stored and read back)       |
//! | RI-04   | fail_closed verdict is recorded honestly — not suppressed or normalised    |
//! | RI-05   | refused verdict is recorded honestly                                       |
//! | RI-06   | Status lifecycle: pending → completed (caller-injected timestamp)          |
//! | RI-07   | Engine scoping: intent for engine-A is not returned for engine-B query     |
//! | RI-08   | fetch_pending returns None after all intents are completed                  |
//!
//! RI-01 runs unconditionally (no DB required).
//! RI-02 through RI-08 require MQK_DATABASE_URL and are marked #[ignore].
//! Run DB tests with:
//!
//! ```text
//! MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//! cargo test -p mqk-daemon --test scenario_restart_intent_cc03b -- --include-ignored
//! ```

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use mqk_daemon::{
    mode_transition::evaluate_mode_transition,
    state::{AppState, DeploymentMode, OperatorAuthMode},
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_restart_intent_cc03b \
             -- --include-ignored"
        )
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to test DB");
    mqk_db::migrate(&pool).await.expect("run migrations");
    pool
}

/// Fixed deterministic timestamp for caller-injected timestamps.
/// Using a fixed value avoids any Utc::now() in test logic.
fn fixed_ts(offset_secs: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + offset_secs, 0)
        .single()
        .expect("valid fixed timestamp")
}

fn unique_engine(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..8])
}

// ---------------------------------------------------------------------------
// RI-01 (no DB): load_pending_restart_intent returns None honestly
// ---------------------------------------------------------------------------

/// CC-03B / RI-01: Without a DB pool configured on the daemon, loading the
/// pending restart intent must return `None` — not a synthetic default record.
///
/// This proves that the absence of durable truth is represented honestly:
/// `None` means "no durable truth available", not "no intent was recorded".
/// Callers must not interpret `None` as implicit permission to proceed.
#[tokio::test]
async fn ri_01_no_db_returns_none_not_synthetic() {
    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ));

    let intent = st.load_pending_restart_intent().await;
    assert!(
        intent.is_none(),
        "RI-01: no DB must return None, not synthetic restart intent; got: {intent:?}"
    );
}

// ---------------------------------------------------------------------------
// RI-02 (DB): insert + fetch round-trip — all fields preserved durably
// ---------------------------------------------------------------------------

/// CC-03B / RI-02: Insert a restart intent and read it back; all fields
/// survive the round-trip without modification.
///
/// This proves `sys_restart_intent` is a real durable seam, not just an
/// in-memory transient.
#[tokio::test]
#[ignore]
async fn ri_02_insert_and_fetch_round_trip() {
    let pool = make_db_pool().await;
    let engine_id = unique_engine("ri02");
    let intent_id = Uuid::new_v4();
    let ts = fixed_ts(0);

    mqk_db::insert_restart_intent(
        &pool,
        &mqk_db::NewRestartIntent {
            intent_id,
            engine_id: engine_id.clone(),
            from_mode: "paper".to_string(),
            to_mode: "live-shadow".to_string(),
            transition_verdict: "admissible_with_restart".to_string(),
            initiated_by: "operator".to_string(),
            initiated_at_utc: ts,
            note: "RI-02 round-trip proof".to_string(),
        },
    )
    .await
    .expect("RI-02: insert failed");

    let row = mqk_db::fetch_latest_restart_intent_for_engine(&pool, &engine_id)
        .await
        .expect("RI-02: fetch failed")
        .expect("RI-02: row must exist after insert");

    assert_eq!(row.intent_id, intent_id, "RI-02: intent_id mismatch");
    assert_eq!(row.engine_id, engine_id);
    assert_eq!(row.from_mode, "paper");
    assert_eq!(row.to_mode, "live-shadow");
    assert_eq!(row.transition_verdict, "admissible_with_restart");
    assert_eq!(row.initiated_by, "operator");
    assert_eq!(row.status, "pending");
    assert!(
        row.completed_at_utc.is_none(),
        "RI-02: newly inserted intent must have no completed_at"
    );
    assert_eq!(row.note, "RI-02 round-trip proof");
}

// ---------------------------------------------------------------------------
// RI-03 (DB): transition_verdict coherent with CC-03A evaluate_mode_transition
// ---------------------------------------------------------------------------

/// CC-03B / RI-03: The `transition_verdict` stored in `sys_restart_intent`
/// must match what `evaluate_mode_transition(from, to).as_str()` returns.
///
/// This is the primary coherence proof between durable restart intent and the
/// canonical CC-03A mode-transition state machine.  The durable record does
/// not diverge from the canonical truth.
#[tokio::test]
#[ignore]
async fn ri_03_transition_verdict_coherent_with_cc03a_seam() {
    let pool = make_db_pool().await;
    let engine_id = unique_engine("ri03");

    // Derive the verdict from the CC-03A canonical seam.
    let canonical_verdict =
        evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::LiveShadow);
    let verdict_str = canonical_verdict.as_str();
    assert_eq!(
        verdict_str, "admissible_with_restart",
        "RI-03: CC-03A seam must return admissible_with_restart for Paper→LiveShadow"
    );

    // Store it durably using the verdict string from the canonical seam.
    let intent_id = Uuid::new_v4();
    mqk_db::insert_restart_intent(
        &pool,
        &mqk_db::NewRestartIntent {
            intent_id,
            engine_id: engine_id.clone(),
            from_mode: DeploymentMode::Paper.as_api_label().to_string(),
            to_mode: DeploymentMode::LiveShadow.as_api_label().to_string(),
            transition_verdict: verdict_str.to_string(),
            initiated_by: "operator".to_string(),
            initiated_at_utc: fixed_ts(0),
            note: String::new(),
        },
    )
    .await
    .expect("RI-03: insert failed");

    // Read back and assert stored verdict == canonical seam output.
    let row = mqk_db::fetch_latest_restart_intent_for_engine(&pool, &engine_id)
        .await
        .expect("RI-03: fetch failed")
        .expect("RI-03: row must exist");

    assert_eq!(
        row.transition_verdict, verdict_str,
        "RI-03: stored transition_verdict must equal CC-03A seam output; \
         stored: {:?}, seam: {:?}",
        row.transition_verdict, verdict_str
    );
    assert_eq!(
        row.from_mode,
        DeploymentMode::Paper.as_api_label(),
        "RI-03: from_mode must be canonical api_label"
    );
    assert_eq!(
        row.to_mode,
        DeploymentMode::LiveShadow.as_api_label(),
        "RI-03: to_mode must be canonical api_label"
    );
}

// ---------------------------------------------------------------------------
// RI-04 (DB): fail_closed verdict stored honestly
// ---------------------------------------------------------------------------

/// CC-03B / RI-04: A `fail_closed` verdict (Paper→LiveCapital) is accepted by
/// the DB and read back accurately — the schema does not suppress or normalise
/// unsafe verdicts.
///
/// This proves that the durable seam is an honest record of what the canonical
/// seam returned — including blocked transitions — not a filtered view.
#[tokio::test]
#[ignore]
async fn ri_04_fail_closed_verdict_stored_honestly() {
    let pool = make_db_pool().await;
    let engine_id = unique_engine("ri04");

    let verdict = evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::LiveCapital);
    assert_eq!(verdict.as_str(), "fail_closed", "RI-04: prerequisite check");

    mqk_db::insert_restart_intent(
        &pool,
        &mqk_db::NewRestartIntent {
            intent_id: Uuid::new_v4(),
            engine_id: engine_id.clone(),
            from_mode: "paper".to_string(),
            to_mode: "live-capital".to_string(),
            transition_verdict: verdict.as_str().to_string(),
            initiated_by: "operator".to_string(),
            initiated_at_utc: fixed_ts(0),
            note: "RI-04: operator attempted fail_closed transition".to_string(),
        },
    )
    .await
    .expect("RI-04: insert failed");

    let row = mqk_db::fetch_latest_restart_intent_for_engine(&pool, &engine_id)
        .await
        .expect("RI-04: fetch failed")
        .expect("RI-04: row must exist");

    assert_eq!(
        row.transition_verdict, "fail_closed",
        "RI-04: fail_closed verdict must be stored as-is; got: {:?}",
        row.transition_verdict
    );
    assert_eq!(row.status, "pending");
}

// ---------------------------------------------------------------------------
// RI-05 (DB): refused verdict stored honestly
// ---------------------------------------------------------------------------

/// CC-03B / RI-05: A `refused` verdict (Paper→Backtest) is accepted by the DB
/// and read back accurately.
///
/// An operator might record that a refused transition was explicitly attempted
/// so the provenance log is complete.  The DB must not block refused verdicts.
#[tokio::test]
#[ignore]
async fn ri_05_refused_verdict_stored_honestly() {
    let pool = make_db_pool().await;
    let engine_id = unique_engine("ri05");

    let verdict = evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::Backtest);
    assert_eq!(verdict.as_str(), "refused", "RI-05: prerequisite check");

    mqk_db::insert_restart_intent(
        &pool,
        &mqk_db::NewRestartIntent {
            intent_id: Uuid::new_v4(),
            engine_id: engine_id.clone(),
            from_mode: "paper".to_string(),
            to_mode: "backtest".to_string(),
            transition_verdict: verdict.as_str().to_string(),
            initiated_by: "operator".to_string(),
            initiated_at_utc: fixed_ts(0),
            note: "RI-05: operator attempted refused transition".to_string(),
        },
    )
    .await
    .expect("RI-05: insert failed");

    let row = mqk_db::fetch_latest_restart_intent_for_engine(&pool, &engine_id)
        .await
        .expect("RI-05: fetch failed")
        .expect("RI-05: row must exist");

    assert_eq!(
        row.transition_verdict, "refused",
        "RI-05: refused verdict must be stored as-is; got: {:?}",
        row.transition_verdict
    );
}

// ---------------------------------------------------------------------------
// RI-06 (DB): Status lifecycle — pending → completed
// ---------------------------------------------------------------------------

/// CC-03B / RI-06: A pending intent can be transitioned to `completed` using
/// a caller-injected timestamp.
///
/// Proves the full status lifecycle: insert pending, update to completed,
/// fetch returns completed with the injected `completed_at_utc`.
/// No `now()` calls appear in the write path.
#[tokio::test]
#[ignore]
async fn ri_06_pending_to_completed_lifecycle() {
    let pool = make_db_pool().await;
    let engine_id = unique_engine("ri06");
    let intent_id = Uuid::new_v4();
    let start_ts = fixed_ts(0);
    let complete_ts = fixed_ts(300);

    mqk_db::insert_restart_intent(
        &pool,
        &mqk_db::NewRestartIntent {
            intent_id,
            engine_id: engine_id.clone(),
            from_mode: "paper".to_string(),
            to_mode: "live-shadow".to_string(),
            transition_verdict: "admissible_with_restart".to_string(),
            initiated_by: "operator".to_string(),
            initiated_at_utc: start_ts,
            note: String::new(),
        },
    )
    .await
    .expect("RI-06: insert failed");

    // Verify pending before update.
    let before = mqk_db::fetch_pending_restart_intent_for_engine(&pool, &engine_id)
        .await
        .expect("RI-06: fetch before failed")
        .expect("RI-06: pending intent must exist");
    assert_eq!(before.status, "pending");
    assert!(before.completed_at_utc.is_none());

    // Update to completed.
    let updated = mqk_db::update_restart_intent_status(&pool, intent_id, "completed", complete_ts)
        .await
        .expect("RI-06: update failed");
    assert!(updated, "RI-06: update must affect exactly one row");

    // Verify completed after update.
    let after = mqk_db::fetch_latest_restart_intent_for_engine(&pool, &engine_id)
        .await
        .expect("RI-06: fetch after failed")
        .expect("RI-06: row must still exist");
    assert_eq!(
        after.status, "completed",
        "RI-06: status must be completed after update"
    );
    assert_eq!(
        after.completed_at_utc,
        Some(complete_ts),
        "RI-06: completed_at_utc must match injected timestamp"
    );

    // fetch_pending must now return None (no more pending intents).
    let pending_after = mqk_db::fetch_pending_restart_intent_for_engine(&pool, &engine_id)
        .await
        .expect("RI-06: fetch_pending after failed");
    assert!(
        pending_after.is_none(),
        "RI-06: fetch_pending must return None after intent is completed"
    );
}

// ---------------------------------------------------------------------------
// RI-07 (DB): Engine scoping — intent for engine-A not visible to engine-B
// ---------------------------------------------------------------------------

/// CC-03B / RI-07: Restart intents are scoped to `engine_id`.  An intent
/// recorded for engine-A must not appear when querying engine-B.
///
/// This proves that `fetch_latest_restart_intent_for_engine` and
/// `fetch_pending_restart_intent_for_engine` correctly scope their results
/// to the specified engine.
#[tokio::test]
#[ignore]
async fn ri_07_engine_id_scoping_is_enforced() {
    let pool = make_db_pool().await;
    let engine_a = unique_engine("ri07a");
    let engine_b = unique_engine("ri07b");

    // Insert an intent for engine-A only.
    mqk_db::insert_restart_intent(
        &pool,
        &mqk_db::NewRestartIntent {
            intent_id: Uuid::new_v4(),
            engine_id: engine_a.clone(),
            from_mode: "paper".to_string(),
            to_mode: "live-shadow".to_string(),
            transition_verdict: "admissible_with_restart".to_string(),
            initiated_by: "operator".to_string(),
            initiated_at_utc: fixed_ts(0),
            note: String::new(),
        },
    )
    .await
    .expect("RI-07: insert for engine-A failed");

    // Engine-A query must return the record.
    let for_a = mqk_db::fetch_latest_restart_intent_for_engine(&pool, &engine_a)
        .await
        .expect("RI-07: fetch engine-A failed");
    assert!(for_a.is_some(), "RI-07: engine-A must find its own intent");

    // Engine-B query must return None — engine isolation.
    let for_b = mqk_db::fetch_latest_restart_intent_for_engine(&pool, &engine_b)
        .await
        .expect("RI-07: fetch engine-B failed");
    assert!(
        for_b.is_none(),
        "RI-07: engine-B must not see engine-A's intent; got: {for_b:?}"
    );
}

// ---------------------------------------------------------------------------
// RI-08 (DB): fetch_pending returns None when all intents are non-pending
// ---------------------------------------------------------------------------

/// CC-03B / RI-08: `fetch_pending_restart_intent_for_engine` returns `None`
/// when an intent exists but has been cancelled.  Honest absence means no
/// pending intent, not no record at all.
///
/// This proves the pending-only filter is load-bearing: the caller cannot
/// infer "restart was intended and pending" from a cancelled record.
#[tokio::test]
#[ignore]
async fn ri_08_fetch_pending_returns_none_when_cancelled() {
    let pool = make_db_pool().await;
    let engine_id = unique_engine("ri08");
    let intent_id = Uuid::new_v4();

    mqk_db::insert_restart_intent(
        &pool,
        &mqk_db::NewRestartIntent {
            intent_id,
            engine_id: engine_id.clone(),
            from_mode: "live-shadow".to_string(),
            to_mode: "paper".to_string(),
            transition_verdict: "admissible_with_restart".to_string(),
            initiated_by: "operator".to_string(),
            initiated_at_utc: fixed_ts(0),
            note: String::new(),
        },
    )
    .await
    .expect("RI-08: insert failed");

    // Cancel the intent.
    mqk_db::update_restart_intent_status(&pool, intent_id, "cancelled", fixed_ts(60))
        .await
        .expect("RI-08: update failed");

    // fetch_pending must return None even though a record exists.
    let pending = mqk_db::fetch_pending_restart_intent_for_engine(&pool, &engine_id)
        .await
        .expect("RI-08: fetch failed");
    assert!(
        pending.is_none(),
        "RI-08: cancelled intent must not appear as pending; got: {pending:?}"
    );

    // fetch_latest still returns the cancelled record (honest full history).
    let latest = mqk_db::fetch_latest_restart_intent_for_engine(&pool, &engine_id)
        .await
        .expect("RI-08: fetch_latest failed")
        .expect("RI-08: cancelled record must still be queryable via fetch_latest");
    assert_eq!(latest.status, "cancelled");
}
