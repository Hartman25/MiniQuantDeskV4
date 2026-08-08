//! STRATEGY-DECISION-IDEMPOTENCY-01
//!
//! # Problem addressed
//!
//! `bar_result_to_decisions`'s `decision_id` used to be seeded from live
//! wall-clock `Utc::now().timestamp_micros()` (read once per tick in the
//! 1-second execution loop), not from the completed bar the decision was
//! actually evaluated against. The 1-second loop can legitimately re-run the
//! same strategy evaluation against the same still-current completed bar
//! many times before a new bar closes (e.g. while an earlier decision for
//! that bar has not yet resolved to a terminal broker outcome) -- with
//! wall-clock seeding, every such re-evaluation produced a distinct
//! `decision_id`, defeating the outbox's `ON CONFLICT DO NOTHING` dedup
//! entirely. A real duplicate-live-order path, bounded only by
//! `MAX_AUTONOMOUS_SIGNALS_PER_RUN`, not by design.
//!
//! # Fix under test
//!
//! `decision_id` is now a UUIDv5 seeded from
//! `run_id|strategy_id|symbol|timeframe_secs|side|qty|bar_end_ts` -- the
//! exact completed-bar identity (`EvaluatedBarFacts::bar_end_ts`), never
//! wall-clock time, matching the same bar-anchored-identity pattern already
//! proven by `runtime_opportunity_allocation::compute_cycle_id`.
//!
//! # Coverage
//!
//! | Test | Claim                                                                |
//! |------|------------------------------------------------------------------------|
//! | D01  | Same bar, replayed: identical decision_id (deterministic replay)       |
//! | D02  | A different completed bar produces a different decision_id             |
//! | D03  | A different decision payload (qty) at the same bar produces a          |
//! |      | different decision_id                                                   |
//! | D04  | A different symbol at the same bar produces a different decision_id    |
//! | D05  | DB-backed: the same bar re-evaluated twice (simulating two 1s ticks   |
//! |      | before the first decision resolves) submits exactly one outbox row     |
//! | D06  | DB-backed: a simulated restart recomputes the identical decision_id    |
//! |      | for the same bar, and resubmitting it is a no-op against the           |
//! |      | already-durable outbox row (retry/restart never creates a second      |
//! |      | order for the same logical decision)                                   |
//!
//! # Proof boundary
//!
//! D01-D04 are pure (no IO). D05-D06 are DB-backed (port 5434 test
//! Postgres) and load-bearing -- must fail hard if `MQK_DATABASE_URL` is
//! absent, not skip.

use std::collections::BTreeMap;
use std::sync::Arc;

use uuid::Uuid;

use mqk_daemon::decision::{
    bar_result_to_decisions, decisions_from_bar_facts, submit_internal_strategy_decision,
};
use mqk_daemon::state::{self, AppState, EvaluatedBarFacts};
use mqk_strategy::{
    IntentMode, StrategyBarResult, StrategyIntents, StrategyOutput, StrategySpec, TargetPosition,
};

fn live_result(targets: Vec<TargetPosition>) -> StrategyBarResult {
    StrategyBarResult {
        spec: StrategySpec::new("test_strategy", 300),
        intents: StrategyIntents {
            mode: IntentMode::Live,
            output: StrategyOutput { targets },
        },
    }
}

fn fixed_run_id() -> Uuid {
    Uuid::parse_str("00000000-0000-0000-0000-0000000000d6").unwrap()
}

fn flat() -> BTreeMap<String, i64> {
    BTreeMap::new()
}

const BAR_A_END_TS: i64 = 1_748_706_300; // an arbitrary completed-bar identity
const BAR_B_END_TS: i64 = 1_748_706_600; // a later, distinct completed bar

// ---------------------------------------------------------------------------
// D01 — same bar, replayed: identical decision_id.
// ---------------------------------------------------------------------------

#[test]
fn d01_same_bar_replayed_produces_identical_decision_id() {
    let result = live_result(vec![TargetPosition::new("NVDA", 20)]);

    // Models the 1-second loop re-evaluating the same still-current
    // completed bar across several ticks before the first decision has
    // resolved -- exactly the scenario that used to mint a fresh
    // decision_id every tick.
    let tick1 = bar_result_to_decisions(&result, fixed_run_id(), BAR_A_END_TS, &flat());
    let tick2 = bar_result_to_decisions(&result, fixed_run_id(), BAR_A_END_TS, &flat());
    let tick3 = bar_result_to_decisions(&result, fixed_run_id(), BAR_A_END_TS, &flat());

    assert_eq!(tick1.len(), 1);
    assert_eq!(
        tick1[0].decision_id, tick2[0].decision_id,
        "D01: replaying the same bar must produce the identical decision_id"
    );
    assert_eq!(
        tick2[0].decision_id, tick3[0].decision_id,
        "D01: decision_id must remain stable across any number of re-evaluations"
    );
}

// ---------------------------------------------------------------------------
// D02 — a different completed bar produces a different decision_id.
// ---------------------------------------------------------------------------

#[test]
fn d02_different_bar_produces_different_decision_id() {
    let result = live_result(vec![TargetPosition::new("NVDA", 20)]);

    let bar_a = bar_result_to_decisions(&result, fixed_run_id(), BAR_A_END_TS, &flat());
    let bar_b = bar_result_to_decisions(&result, fixed_run_id(), BAR_B_END_TS, &flat());

    assert_ne!(
        bar_a[0].decision_id, bar_b[0].decision_id,
        "D02: a genuinely distinct completed bar must produce a distinct decision_id -- \
         otherwise two legitimate sequential decisions for the same symbol/side/qty would \
         collapse into one"
    );
}

// ---------------------------------------------------------------------------
// D03/D04 — a different decision payload produces a different decision_id.
// ---------------------------------------------------------------------------

#[test]
fn d03_different_qty_at_same_bar_produces_different_decision_id() {
    let result_10 = live_result(vec![TargetPosition::new("NVDA", 10)]);
    let result_20 = live_result(vec![TargetPosition::new("NVDA", 20)]);

    let d_10 = bar_result_to_decisions(&result_10, fixed_run_id(), BAR_A_END_TS, &flat());
    let d_20 = bar_result_to_decisions(&result_20, fixed_run_id(), BAR_A_END_TS, &flat());

    assert_ne!(d_10[0].decision_id, d_20[0].decision_id);
    assert_ne!(d_10[0].qty, d_20[0].qty);
}

#[test]
fn d04_different_symbol_at_same_bar_produces_different_decision_id() {
    let result_nvda = live_result(vec![TargetPosition::new("NVDA", 20)]);
    let result_amd = live_result(vec![TargetPosition::new("AMD", 20)]);

    let d_nvda = bar_result_to_decisions(&result_nvda, fixed_run_id(), BAR_A_END_TS, &flat());
    let d_amd = bar_result_to_decisions(&result_amd, fixed_run_id(), BAR_A_END_TS, &flat());

    assert_ne!(d_nvda[0].decision_id, d_amd[0].decision_id);
}

// ---------------------------------------------------------------------------
// D07-D09 — decisions_from_bar_facts: the exact production call-site seam.
// ---------------------------------------------------------------------------
//
// loop_runner.rs calls `decisions_from_bar_facts` directly (not
// `bar_result_to_decisions`) -- these tests exercise that exact function,
// not a hand-rolled equivalent, so they prove the real production wiring,
// not merely the pure helper it delegates to.

fn bar_facts(bar_end_ts: i64) -> EvaluatedBarFacts {
    EvaluatedBarFacts {
        symbol: "NVDA".to_string(),
        strategy_id: "test_strategy".to_string(),
        timeframe: "5m".to_string(),
        bar_end_ts,
        close_micros: 500_000_000,
    }
}

#[test]
fn d07_decisions_from_bar_facts_some_matches_bar_result_to_decisions() {
    let result = live_result(vec![TargetPosition::new("NVDA", 20)]);
    let facts = bar_facts(BAR_A_END_TS);

    let via_wrapper = decisions_from_bar_facts(&result, fixed_run_id(), Some(&facts), &flat());
    let via_direct = bar_result_to_decisions(&result, fixed_run_id(), BAR_A_END_TS, &flat());

    assert_eq!(via_wrapper.len(), 1);
    assert_eq!(
        via_wrapper[0].decision_id, via_direct[0].decision_id,
        "D07: the production wrapper must compute the identical decision_id as the pure \
         function it delegates to, given the matching bar_end_ts"
    );
}

#[test]
fn d08_decisions_from_bar_facts_none_with_empty_targets_is_benign() {
    let result = live_result(vec![]);
    let decisions = decisions_from_bar_facts(&result, fixed_run_id(), None, &flat());
    assert!(
        decisions.is_empty(),
        "D08: no bar facts + no targets must simply produce no decisions"
    );
}

#[test]
fn d09_decisions_from_bar_facts_none_with_nonzero_target_fails_closed() {
    // An anomalous case that should never occur in practice (the stub
    // fallback that produces `bar_facts = None` also always produces
    // `is_complete = false`, which every strategy engine already treats as
    // signal = 0 before this seam is ever reached) -- but if it somehow
    // did, STRATEGY-DECISION-IDEMPOTENCY-01 requires refusing to submit any
    // decision rather than falling back to wall-clock-seeded identity.
    let result = live_result(vec![TargetPosition::new("NVDA", 20)]);
    let decisions = decisions_from_bar_facts(&result, fixed_run_id(), None, &flat());
    assert!(
        decisions.is_empty(),
        "D09: missing bar facts must fail closed to zero decisions, even when the strategy \
         produced a nonzero target -- never fall back to wall-clock-seeded decision_id"
    );
}

// ---------------------------------------------------------------------------
// D05/D06 — DB-backed: outbox dedup and restart-stable replay.
// ---------------------------------------------------------------------------

fn require_db_url() -> String {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "PROOF: MQK_DATABASE_URL is not set. \
             This is a load-bearing proof test and cannot be skipped. \
             Set MQK_DATABASE_URL to a live Postgres instance and re-run."
        ),
    }
}

async fn seed_active_paper_promotion(
    pool: &sqlx::PgPool,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
) {
    let now = chrono::Utc::now();
    let seed = |suffix: &str| {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("d01-idempotency-seed:{strategy_id}:{symbol}:{timeframe_secs}:{suffix}")
                .as_bytes(),
        )
    };
    let step = |transition_id: Uuid,
                previous_state: Option<&str>,
                new_state: &str,
                effective_at: chrono::DateTime<chrono::Utc>| {
        mqk_db::InsertStrategyPromotionTransitionArgs {
            transition_id,
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs,
            config_fingerprint: None,
            config_identity_status: "unavailable_in_current_runtime".to_string(),
            previous_state: previous_state.map(|s| s.to_string()),
            new_state: new_state.to_string(),
            parent_transition_id: None,
            evidence_transition_id: None,
            evidence_review_id: None,
            evidence_scanner_scan_id: None,
            evidence_git_hash: None,
            evidence_artifact_path: None,
            evidence_fingerprint: None,
            evidence_fingerprint_v2: None,
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
            now + chrono::Duration::milliseconds(1),
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
            now + chrono::Duration::milliseconds(2),
        ),
    )
    .await
    .expect("seed active_paper");
}

async fn outbox_row_count(pool: &sqlx::PgPool, idempotency_key: &str) -> i64 {
    sqlx::query_scalar("select count(*)::bigint from oms_outbox where idempotency_key = $1")
        .bind(idempotency_key)
        .fetch_one(pool)
        .await
        .expect("count query failed")
}

/// Seeds a registered, active-paper-promoted strategy plus a real
/// armed/begun/heartbeat-current run with an injected running loop --
/// the exact recipe `scenario_native_strategy_bridge_b1c.rs::b1c_c14`
/// already proves reaches `submit_internal_strategy_decision`'s full
/// accept path. Uses a disposable per-test database
/// (`mqk_db::run_isolated`, FULL-AUDIT-FAIL-017 pattern) because
/// `seed_active_paper_promotion`'s transition_ids are deterministic
/// UUIDv5s keyed only on `(strategy_id, symbol, timeframe_secs)` -- a
/// shared database would collide across repeated runs of this test.
async fn seed_and_run(
    pool: &sqlx::PgPool,
    strategy_id: &str,
    symbol: &str,
) -> (Arc<AppState>, Uuid) {
    let ts = chrono::Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: "D01 Idempotency Test Strategy".to_string(),
            enabled: true,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("seed strategy registry");
    seed_active_paper_promotion(pool, strategy_id, symbol, 300).await;

    let st = Arc::new(state::AppState::new_with_db(pool.clone()));
    mqk_db::persist_arm_state_canonical(pool, mqk_db::ArmState::Armed, None)
        .await
        .expect("arm state");

    let run_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({"source": "d01_idempotency"}),
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

    (st, run_id)
}

#[tokio::test]
async fn d05_same_bar_reevaluated_twice_submits_exactly_one_outbox_row() {
    let _ = require_db_url();
    mqk_db::run_isolated("d05_idempotency", |pool| async move {
        let symbol = "NVDA";
        let (st, run_id) = seed_and_run(&pool, "test_strategy", symbol).await;

        let result = live_result(vec![TargetPosition::new(symbol, 20)]);

        // Tick 1: the strategy is evaluated against BAR_A and a decision is
        // submitted, but has not yet resolved (no fill/cancel/reject arrived).
        let tick1 = bar_result_to_decisions(&result, run_id, BAR_A_END_TS, &flat());
        assert_eq!(
            tick1.len(),
            1,
            "D05 precondition: one target -> one decision"
        );
        let decision_id = tick1[0].decision_id.clone();
        let outcome1 =
            submit_internal_strategy_decision(&st, tick1.into_iter().next().unwrap()).await;
        assert!(
            outcome1.accepted,
            "D05: first submit must be accepted; disposition={:?} blockers={:?}",
            outcome1.disposition, outcome1.blockers
        );

        // Tick 2 (1 second later): the loop re-evaluates the SAME still-
        // current bar (BAR_A has not yet been superseded by a new completed
        // bar) -- this is the exact re-evaluation that used to mint a
        // fresh, undeduped decision_id every tick.
        let tick2 = bar_result_to_decisions(&result, run_id, BAR_A_END_TS, &flat());
        assert_eq!(
            tick2[0].decision_id, decision_id,
            "D05: the re-evaluation must compute the identical decision_id"
        );
        let outcome2 =
            submit_internal_strategy_decision(&st, tick2.into_iter().next().unwrap()).await;
        assert_eq!(
            outcome2.disposition, "duplicate",
            "D05: the second submit of the same logical decision must be recognized as a \
             duplicate, not create a second order"
        );

        let count = outbox_row_count(&pool, &decision_id).await;
        assert_eq!(
            count, 1,
            "D05: exactly one outbox row must exist for this decision_id, not two"
        );
    })
    .await;
}

#[tokio::test]
async fn d06_restart_recomputes_identical_decision_id_and_resubmit_is_a_noop() {
    let _ = require_db_url();
    mqk_db::run_isolated("d06_idempotency", |pool| async move {
        let symbol = "AMD";
        let (st_before, run_id) = seed_and_run(&pool, "test_strategy", symbol).await;

        let result = live_result(vec![TargetPosition::new(symbol, 15)]);

        // Pre-restart: compute and submit the decision for BAR_A.
        let before = bar_result_to_decisions(&result, run_id, BAR_A_END_TS, &flat());
        let decision_id = before[0].decision_id.clone();
        let outcome_before =
            submit_internal_strategy_decision(&st_before, before.into_iter().next().unwrap()).await;
        assert!(
            outcome_before.accepted,
            "D06: pre-restart submit must be accepted; disposition={:?}",
            outcome_before.disposition
        );

        // Simulated restart: a fresh AppState/process, same durable DB,
        // re-runs the identical strategy evaluation against the same
        // completed bar (durable evidence -- bar_end_ts -- not process
        // memory). Re-injects the running loop against the SAME run_id,
        // exactly as a real crash-restart resumes an existing RUNNING row.
        let st_after = Arc::new(state::AppState::new_with_db(pool.clone()));
        st_after.inject_running_loop_for_test(run_id).await;
        let after = bar_result_to_decisions(&result, run_id, BAR_A_END_TS, &flat());
        assert_eq!(
            after[0].decision_id, decision_id,
            "D06: a fresh process recomputing the same bar must derive the identical decision_id"
        );
        let outcome_after =
            submit_internal_strategy_decision(&st_after, after.into_iter().next().unwrap()).await;
        assert_eq!(
            outcome_after.disposition, "duplicate",
            "D06: resubmitting after a simulated restart must be a no-op, never a second order"
        );

        let count = outbox_row_count(&pool, &decision_id).await;
        assert_eq!(
            count, 1,
            "D06: still exactly one outbox row after the simulated restart"
        );
    })
    .await;
}
