//! MULTI-SYMBOL-DAY-ORDER-CAP-01: Proof tests for Gate 1f, the optional
//! per-symbol daily order count cap (cap #4, design doc §6).
//!
//! Gate 1f sits between Gate 1 (`day_signal_limit`, account-wide PT-AUTO-02)
//! and Gate 1e (per-strategy capital budget) in
//! `submit_internal_strategy_decision`. It is disabled (`None`) unless
//! `MQK_PER_SYMBOL_DAY_ORDER_LIMIT` is set or
//! `set_per_symbol_day_order_limit_for_test` is called.
//!
//! | Test | Proves                                                                 |
//! |------|------------------------------------------------------------------------|
//! | D01  | Gate 1f disabled by default — high per-symbol count does not block     |
//! | D02  | Per-symbol count below configured limit — not exceeded                  |
//! | D03  | Per-symbol count at configured limit — "symbol_day_limit_reached", fires before Gate 2 (DB) |
//! | D04  | Per-symbol granularity — symbol A capped, symbol B (count 0) unaffected |
//! | D05  | Gate 1f firing leaves Gate 1 (account-wide) unaffected — disposition is symbol-specific, not "day_limit_reached" |
//! | D06  | Gate 1 (account-wide) ordering/behavior unchanged — still fires "day_limit_reached" ahead of Gate 1f |
//! | D07  | `reset_symbol_day_order_counts` clears all per-symbol counters          |
//! | D08  | Blocker message names the symbol and the per-symbol limit              |
//! | D09  | (DB, #[ignore]) Gate 7 acceptance increments both the account-wide and per-symbol counters |
//!
//! No-DB tests (D01-D08) run unconditionally.
//! D09 requires MQK_DATABASE_URL and is marked #[ignore].

use std::sync::Arc;

use chrono::{Duration, Utc};
use mqk_daemon::{
    decision::{submit_internal_strategy_decision, InternalStrategyDecision},
    state,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_decision_for_symbol(
    decision_id: &str,
    strategy_id: &str,
    symbol: &str,
) -> InternalStrategyDecision {
    InternalStrategyDecision {
        decision_id: decision_id.to_string(),
        strategy_id: strategy_id.to_string(),
        symbol: symbol.to_string(),
        timeframe_secs: 86400,
        side: "buy".to_string(),
        qty: 10,
        order_type: "market".to_string(),
        time_in_force: "day".to_string(),
        limit_price: None,
    }
}

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

// ---------------------------------------------------------------------------
// DB helpers (for the #[ignore] D09 test)
// ---------------------------------------------------------------------------

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_multi_symbol_day_order_cap_01 -- --include-ignored"
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

async fn seed_registry(pool: &sqlx::PgPool, strategy_id: &str, enabled: bool) {
    let ts = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: format!("Test Strategy {strategy_id}"),
            enabled,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("seed_registry: upsert failed");
}

/// STRATEGY-PROMOTION-REGISTRY-01D: seed a durable `active_paper` promotion
/// for the exact `(strategy_id, symbol, timeframe_secs)` identity, walking
/// the full legal transition graph (no state -> shadow_approved ->
/// paper_approved -> active_paper) so the Gate 3b promotion gate passes.
async fn seed_active_paper_promotion(
    pool: &sqlx::PgPool,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
) {
    let now = Utc::now();
    let seed = |suffix: &str| {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("test-promo-seed:{strategy_id}:{symbol}:{timeframe_secs}:{suffix}").as_bytes(),
        )
    };
    let step = |transition_id: Uuid,
                previous_state: Option<&str>,
                new_state: &str,
                effective_at: chrono::DateTime<Utc>| {
        mqk_db::InsertStrategyPromotionTransitionArgs {
            transition_id,
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs,
            config_fingerprint: None,
            config_identity_status: "unavailable_in_current_runtime".to_string(),
            previous_state: previous_state.map(|s| s.to_string()),
            new_state: new_state.to_string(),
            evidence_review_id: None,
            evidence_scanner_scan_id: None,
            evidence_git_hash: None,
            evidence_artifact_path: None,
            evidence_fingerprint: None,
            effective_at_utc: effective_at,
            expires_at_utc: None,
            initiated_by: "test-seed".to_string(),
            reason: "test seed".to_string(),
            created_at_utc: effective_at,
        }
    };
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(seed("1"), None, "shadow_approved", now),
    )
    .await
    .expect("seed shadow_approved");
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(
            seed("2"),
            Some("shadow_approved"),
            "paper_approved",
            now + Duration::milliseconds(1),
        ),
    )
    .await
    .expect("seed paper_approved");
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &step(
            seed("3"),
            Some("paper_approved"),
            "active_paper",
            now + Duration::milliseconds(2),
        ),
    )
    .await
    .expect("seed active_paper");
}

/// Seed a RUNNING run in the DB and wire up the local loop handle.
/// Returns the run_id.
async fn seed_active_run(st: &Arc<state::AppState>) -> Uuid {
    let pool = st.db.as_ref().expect("db configured");
    let run_id = Uuid::new_v4();
    let now = Utc::now();

    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({"source": "scenario_multi_symbol_day_order_cap_01"}),
            host_fingerprint: "test-host".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::arm_run(pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(pool, run_id).await.expect("begin_run");
    mqk_db::heartbeat_run(pool, run_id, now)
        .await
        .expect("heartbeat_run");

    st.inject_running_loop_for_test(run_id).await;
    run_id
}

/// Clean up all test-written rows for a given engine / run.
async fn cleanup_run(pool: &sqlx::PgPool, run_id: Uuid) {
    sqlx::query("DELETE FROM oms_outbox WHERE run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .expect("cleanup oms_outbox");
    sqlx::query("DELETE FROM runs WHERE run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await
        .expect("cleanup runs");
}

// ---------------------------------------------------------------------------
// D01 -- Gate 1f disabled by default
// ---------------------------------------------------------------------------

/// D01: With no configured limit, a high per-symbol count never trips Gate 1f.
#[tokio::test]
async fn d01_gate_1f_disabled_by_default() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    assert_eq!(
        st.per_symbol_day_order_limit().await,
        None,
        "default per-symbol day order limit must be None (Gate 1f disabled)"
    );

    st.set_symbol_day_order_count_for_test("AAPL", 9_999);
    assert!(
        !st.symbol_day_order_limit_exceeded("AAPL").await,
        "Gate 1f must never be exceeded when the limit is unset"
    );

    let d = make_decision_for_symbol(&unique_id("dec"), "strat-a", "AAPL");
    let out = submit_internal_strategy_decision(&st, d).await;

    assert_ne!(
        out.disposition, "symbol_day_limit_reached",
        "Gate 1f must not fire when no limit is configured; got: {:?}",
        out.disposition
    );
}

// ---------------------------------------------------------------------------
// D02 -- below configured limit
// ---------------------------------------------------------------------------

/// D02: A per-symbol count below the configured limit does not trip Gate 1f.
#[tokio::test]
async fn d02_below_configured_limit_not_exceeded() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    st.set_per_symbol_day_order_limit_for_test(Some(5));
    st.set_symbol_day_order_count_for_test("AAPL", 4);

    assert!(
        !st.symbol_day_order_limit_exceeded("AAPL").await,
        "count 4 < limit 5 must not be exceeded"
    );

    let d = make_decision_for_symbol(&unique_id("dec"), "strat-a", "AAPL");
    let out = submit_internal_strategy_decision(&st, d).await;

    assert_ne!(
        out.disposition, "symbol_day_limit_reached",
        "Gate 1f must not fire below the configured limit; got: {:?}",
        out.disposition
    );
}

// ---------------------------------------------------------------------------
// D03 -- at configured limit, fires before Gate 2 (DB)
// ---------------------------------------------------------------------------

/// D03: A per-symbol count at the configured limit trips Gate 1f, and does so
/// before Gate 2 (DB presence) — disposition is "symbol_day_limit_reached",
/// not "unavailable", even with no DB configured.
#[tokio::test]
async fn d03_at_configured_limit_fires_before_db() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    st.set_per_symbol_day_order_limit_for_test(Some(1));
    st.set_symbol_day_order_count_for_test("AAPL", 1);

    assert!(
        st.symbol_day_order_limit_exceeded("AAPL").await,
        "count 1 >= limit 1 must be exceeded"
    );

    let d = make_decision_for_symbol(&unique_id("dec"), "strat-a", "AAPL");
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "symbol_day_limit_reached",
        "saturated per-symbol counter must produce symbol_day_limit_reached \
         before the DB gate; got: {:?}",
        out.disposition
    );
}

// ---------------------------------------------------------------------------
// D04 -- per-symbol granularity
// ---------------------------------------------------------------------------

/// D04: Symbol A can hit its per-symbol limit while symbol B (count 0 under
/// the same limit) is unaffected. This is the headline proof for
/// MULTI-SYMBOL-DAY-ORDER-CAP-01.
#[tokio::test]
async fn d04_per_symbol_granularity_symbol_a_capped_symbol_b_unaffected() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    st.set_per_symbol_day_order_limit_for_test(Some(1));
    st.set_symbol_day_order_count_for_test("AAPL", 1); // at limit
    st.set_symbol_day_order_count_for_test("MSFT", 0); // untouched

    let d_aapl = make_decision_for_symbol(&unique_id("dec"), "strat-a", "AAPL");
    let out_aapl = submit_internal_strategy_decision(&st, d_aapl).await;
    assert_eq!(
        out_aapl.disposition, "symbol_day_limit_reached",
        "AAPL is at its per-symbol limit; got: {:?}",
        out_aapl.disposition
    );

    let d_msft = make_decision_for_symbol(&unique_id("dec"), "strat-a", "MSFT");
    let out_msft = submit_internal_strategy_decision(&st, d_msft).await;
    assert_ne!(
        out_msft.disposition, "symbol_day_limit_reached",
        "MSFT has not hit its per-symbol limit and must pass Gate 1f; got: {:?}",
        out_msft.disposition
    );
}

// ---------------------------------------------------------------------------
// D05 -- Gate 1f firing leaves Gate 1 (account-wide) unaffected
// ---------------------------------------------------------------------------

/// D05: When Gate 1f fires, the account-wide day_signal_limit (Gate 1) is
/// untouched — disposition is the symbol-specific "symbol_day_limit_reached",
/// not "day_limit_reached", proving the two counters and gates are
/// independent.
#[tokio::test]
async fn d05_gate_1f_independent_of_account_wide_gate_1() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Account-wide counter is far from MAX_AUTONOMOUS_SIGNALS_PER_RUN (100).
    st.set_day_signal_count_for_test(0);
    assert!(!st.day_signal_limit_exceeded());

    // Per-symbol counter is at its (much smaller) configured limit.
    st.set_per_symbol_day_order_limit_for_test(Some(1));
    st.set_symbol_day_order_count_for_test("AAPL", 1);

    let d = make_decision_for_symbol(&unique_id("dec"), "strat-a", "AAPL");
    let out = submit_internal_strategy_decision(&st, d).await;

    assert_eq!(
        out.disposition, "symbol_day_limit_reached",
        "Gate 1f must fire on its own merit while Gate 1 remains unaffected; got: {:?}",
        out.disposition
    );
    assert!(
        !st.day_signal_limit_exceeded(),
        "account-wide day signal limit must remain unaffected by Gate 1f"
    );
}

// ---------------------------------------------------------------------------
// D06 -- Gate 1 (account-wide) ordering/behavior unchanged
// ---------------------------------------------------------------------------

/// D06: A saturated account-wide counter still produces "day_limit_reached"
/// via Gate 1, ahead of Gate 1f, exactly as before this patch — even when the
/// per-symbol counter for the same symbol is well under its own limit.
#[tokio::test]
async fn d06_account_wide_gate_1_unchanged() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Saturate the account-wide counter (PT-AUTO-02 behavior, unchanged).
    st.set_day_signal_count_for_test(100);

    // Per-symbol counter is far from its configured limit.
    st.set_per_symbol_day_order_limit_for_test(Some(1));
    st.set_symbol_day_order_count_for_test("AAPL", 0);
    assert!(!st.symbol_day_order_limit_exceeded("AAPL").await);

    let d = make_decision_for_symbol(&unique_id("dec"), "strat-a", "AAPL");
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "day_limit_reached",
        "account-wide Gate 1 must still fire first, unchanged by Gate 1f; got: {:?}",
        out.disposition
    );
}

// ---------------------------------------------------------------------------
// D07 -- reset clears all per-symbol counters
// ---------------------------------------------------------------------------

/// D07: `reset_symbol_day_order_counts` clears counters for every symbol,
/// mirroring the run-start reset of the account-wide `day_signal_count`.
#[tokio::test]
async fn d07_reset_clears_all_per_symbol_counts() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    st.set_symbol_day_order_count_for_test("AAPL", 3);
    st.set_symbol_day_order_count_for_test("MSFT", 7);
    assert_eq!(st.symbol_day_order_count("AAPL").await, 3);
    assert_eq!(st.symbol_day_order_count("MSFT").await, 7);

    st.reset_symbol_day_order_counts().await;

    assert_eq!(st.symbol_day_order_count("AAPL").await, 0);
    assert_eq!(st.symbol_day_order_count("MSFT").await, 0);
}

// ---------------------------------------------------------------------------
// D08 -- blocker message content
// ---------------------------------------------------------------------------

/// D08: The Gate 1f refusal blocker names the symbol and explains the
/// per-symbol order count limit, distinct from the account-wide Gate 1
/// blocker text.
#[tokio::test]
async fn d08_blocker_message_names_symbol_and_limit() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    st.set_per_symbol_day_order_limit_for_test(Some(1));
    st.set_symbol_day_order_count_for_test("AAPL", 1);

    let d = make_decision_for_symbol(&unique_id("dec"), "strat-a", "AAPL");
    let out = submit_internal_strategy_decision(&st, d).await;

    assert_eq!(out.disposition, "symbol_day_limit_reached");
    assert!(
        out.blockers.iter().any(|b| b.contains("AAPL")),
        "blocker must name the symbol AAPL; got: {:?}",
        out.blockers
    );
    assert!(
        out.blockers
            .iter()
            .any(|b| b.contains("per-symbol") && b.contains("order")),
        "blocker must describe the per-symbol order count limit; got: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// D09 -- Gate 7 acceptance increments both counters (DB required)
// ---------------------------------------------------------------------------

/// D09: When all gates pass and a new outbox row is inserted, both the
/// account-wide `day_signal_count` and the per-symbol counter for the
/// decision's symbol are incremented by one.
#[tokio::test]
#[ignore]
async fn d09_acceptance_increments_account_wide_and_per_symbol_counters() {
    let pool = make_db_pool().await;

    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("cleanup sys_arm_state");

    let sid = unique_id("dayordercap");
    seed_registry(&pool, &sid, true).await;
    seed_active_paper_promotion(&pool, &sid, "AAPL", 86400).await;
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist arm state");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let run_id = seed_active_run(&st).await;

    let before_account = st.day_signal_count();
    let before_symbol = st.symbol_day_order_count("AAPL").await;

    let dec_id = unique_id("dec");
    let d = make_decision_for_symbol(&dec_id, &sid, "AAPL");
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(
        out.accepted,
        "all gates passed; decision must be accepted; disposition={:?}, blockers={:?}",
        out.disposition, out.blockers
    );

    assert_eq!(
        st.day_signal_count(),
        before_account + 1,
        "account-wide day signal count must increment by one on acceptance"
    );
    assert_eq!(
        st.symbol_day_order_count("AAPL").await,
        before_symbol + 1,
        "per-symbol day order count for AAPL must increment by one on acceptance"
    );

    cleanup_run(&pool, run_id).await;
}
