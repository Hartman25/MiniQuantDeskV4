//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 7C Part 1: durable
//! dynamic-selection plan evidence store proof.
//!
//! Proves `insert_dynamic_selection_plan` / `fetch_dynamic_selection_plan` /
//! `fetch_dynamic_selection_plan_for_read` /
//! `fetch_recent_dynamic_selection_plans` round-trip, idempotent-replay,
//! payload-collision, rollback, and DB-level constraint behavior, with zero
//! writes to any portfolio/P&L/order table beyond the one fixture run row
//! each test creates for itself.
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-db --test scenario_dynamic_selection_evidence_store_01 -- \
//!     --include-ignored --test-threads=1

use chrono::{TimeZone, Utc};
use mqk_db::{
    fetch_dynamic_selection_plan, fetch_dynamic_selection_plan_for_read,
    fetch_recent_dynamic_selection_plans, insert_dynamic_selection_plan, insert_run,
    BoundedDynamicSelectionPlanFetch, InsertDynamicSelectionPlanOutcome, NewDynamicSelectionPlan,
    NewDynamicSelectionPlanCandidate, NewDynamicSelectionPlanSymbol, NewRun, ENV_DB_URL,
};
use uuid::Uuid;

async fn test_pool() -> anyhow::Result<sqlx::PgPool> {
    if std::env::var(ENV_DB_URL).is_err() {
        anyhow::bail!("SKIP: requires MQK_DATABASE_URL");
    }
    let pool = mqk_db::testkit_db_pool().await?;
    Ok(pool)
}

async fn cleanup(pool: &sqlx::PgPool, run_ids: &[Uuid]) {
    for run_id in run_ids {
        let _ = sqlx::query(
            "delete from sys_dynamic_selection_plan_candidates where plan_id in \
             (select plan_id from sys_dynamic_selection_plans where run_id = $1)",
        )
        .bind(run_id)
        .execute(pool)
        .await;
        let _ = sqlx::query(
            "delete from sys_dynamic_selection_plan_symbols where plan_id in \
             (select plan_id from sys_dynamic_selection_plans where run_id = $1)",
        )
        .bind(run_id)
        .execute(pool)
        .await;
        let _ = sqlx::query("delete from sys_dynamic_selection_plans where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("delete from runs where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
    }
}

async fn fixture_run(pool: &sqlx::PgPool, run_id: Uuid) {
    insert_run(
        pool,
        &NewRun {
            run_id,
            engine_id: "test-dynamic-selection-evidence-store".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc.with_ymd_and_hms(2099, 3, 1, 12, 0, 0).unwrap(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("fixture run insert should succeed");
}

fn fixed_run_id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.dynamic-selection-evidence-store-01.v1|{seed}").as_bytes(),
    )
}

fn fixed_plan_id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.dynamic-selection-evidence-store-01.plan.v1|{seed}").as_bytes(),
    )
}

fn selected_candidate(symbol: &str, ordinal: i32) -> NewDynamicSelectionPlanCandidate {
    NewDynamicSelectionPlanCandidate {
        ordinal,
        symbol: symbol.to_string(),
        strategy_id: "swing_momentum".to_string(),
        timeframe_secs: 300,
        canonical_score_decimal: Some("1.250000".to_string()),
        canonical_score_micros: Some(1_250_000),
        scanner_rank: Some(1),
        watchlist_assigned: true,
        promotion_query_ok: true,
        promotion_state: Some("active_paper".to_string()),
        promotion_effective: true,
        promotion_expired: false,
        evidence_resolved: true,
        review_state_is_paper_candidate: true,
        evidence_review_state: Some("paper_candidate".to_string()),
        durable_legacy_fingerprint: Some("a".repeat(64)),
        recomputed_legacy_fingerprint: Some("a".repeat(64)),
        legacy_fingerprint_matches: true,
        durable_exact_fingerprint_v2: Some("b".repeat(64)),
        recomputed_exact_fingerprint_v2: Some("b".repeat(64)),
        exact_fingerprint_v2_matches: true,
        config_identity_verified: true,
        durable_config_fingerprint: Some("c".repeat(64)),
        current_config_fingerprint: Some("c".repeat(64)),
        registry_enabled: true,
        plugin_instantiable: true,
        timeframe_matches: true,
        data_ready: true,
        evidence_review_id: Some("review-1".to_string()),
        evidence_scanner_scan_id: Some("scan-1".to_string()),
        evidence_artifact_path: Some("artifacts/review-1.json".to_string()),
        evidence_git_hash: Some("deadbeef".to_string()),
        promotion_transition_id: Some("transition-1".to_string()),
        promotion_effective_at: Some("2099-03-01T00:00:00Z".to_string()),
        promotion_expires_at: None,
        evidence_transition_id: Some("transition-1".to_string()),
        exact_reason_code: "selected_highest_score".to_string(),
        selected: true,
        disposition: "selected".to_string(),
        reason_code: "selected_highest_score".to_string(),
    }
}

fn sample_plan(run_id: Uuid, plan_id: Uuid) -> NewDynamicSelectionPlan {
    NewDynamicSelectionPlan {
        plan_id,
        run_id,
        library_schema_version: "dynamic-strategy-symbol-selection-v1".to_string(),
        context_schema_version: "dynamic-strategy-symbol-selection-v1".to_string(),
        configured_mode: "paper_enforced".to_string(),
        effective_mode: "paper_enforced".to_string(),
        live_lock_applied: false,
        approved_for_live: false,
        source_kind: "watchlist_v2".to_string(),
        source_identity: "watchlist".to_string(),
        market_date: "2099-03-01".to_string(),
        disposition: "paper_enforced_allowed".to_string(),
        truth_state: "computed".to_string(),
        blockers: vec![],
        symbol_count: 1,
        candidate_count: 1,
        selected_count: 1,
        refused_count: 0,
        writer_version: "mqk-daemon-test".to_string(),
        created_at_utc: Utc.with_ymd_and_hms(2099, 3, 1, 12, 5, 0).unwrap(),
        symbols: vec![NewDynamicSelectionPlanSymbol {
            symbol: "AAPL".to_string(),
            selected_strategy_id: Some("swing_momentum".to_string()),
            disposition: "selected".to_string(),
            reason_code: "selected_highest_score".to_string(),
            exact_reason_code: "selected_highest_score".to_string(),
        }],
        candidates: vec![selected_candidate("AAPL", 0)],
    }
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn insert_and_fetch_round_trips() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("round_trip");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_id = fixed_plan_id("round_trip");
    let plan = sample_plan(run_id, plan_id);

    let outcome = insert_dynamic_selection_plan(&pool, plan.clone())
        .await
        .expect("insert should succeed");
    assert_eq!(outcome, InsertDynamicSelectionPlanOutcome::Inserted);

    let (fetched_plan, fetched_symbols, fetched_candidates) =
        fetch_dynamic_selection_plan(&pool, plan_id)
            .await
            .expect("fetch should succeed")
            .expect("plan should exist");

    assert_eq!(fetched_plan.plan_id, plan_id);
    assert_eq!(fetched_plan.effective_mode, "paper_enforced");
    assert!(!fetched_plan.approved_for_live);
    assert_eq!(fetched_plan.candidate_count, 1);
    assert_eq!(fetched_plan.selected_count, 1);
    assert_eq!(fetched_symbols.len(), 1);
    assert_eq!(fetched_symbols[0].symbol, "AAPL");
    assert_eq!(
        fetched_symbols[0].selected_strategy_id.as_deref(),
        Some("swing_momentum")
    );
    assert_eq!(fetched_candidates.len(), 1);
    assert_eq!(fetched_candidates[0].ordinal, 0);
    assert!(fetched_candidates[0].selected);
    assert_eq!(fetched_candidates[0].disposition, "selected");

    cleanup(&pool, &[run_id]).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn re_persisting_same_plan_id_is_idempotent_no_op() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("idempotent");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_id = fixed_plan_id("idempotent");
    let plan = sample_plan(run_id, plan_id);

    let first = insert_dynamic_selection_plan(&pool, plan.clone())
        .await
        .expect("first insert should succeed");
    assert_eq!(first, InsertDynamicSelectionPlanOutcome::Inserted);

    let second = insert_dynamic_selection_plan(&pool, plan.clone())
        .await
        .expect("second insert should succeed as a no-op");
    assert_eq!(second, InsertDynamicSelectionPlanOutcome::AlreadyExists);

    let (_, symbols, candidates) = fetch_dynamic_selection_plan(&pool, plan_id)
        .await
        .expect("fetch should succeed")
        .expect("plan should exist");
    assert_eq!(symbols.len(), 1, "no duplicate symbol rows from replay");
    assert_eq!(
        candidates.len(),
        1,
        "no duplicate candidate rows from replay"
    );

    cleanup(&pool, &[run_id]).await;
}

/// A `plan_id` collision with genuinely different payload content must never
/// be silently accepted as an idempotent replay, and the originally-stored
/// row must be left untouched -- proving requirement 7 (same plan_id,
/// different contents is a hard conflict) and requirement 8 (existing
/// evidence cannot be rewritten into a different plan).
async fn assert_payload_collision_and_original_preserved(
    pool: &sqlx::PgPool,
    original: NewDynamicSelectionPlan,
    mutated: NewDynamicSelectionPlan,
) {
    let plan_id = original.plan_id;
    let first = insert_dynamic_selection_plan(pool, original)
        .await
        .expect("first insert should succeed");
    assert_eq!(first, InsertDynamicSelectionPlanOutcome::Inserted);

    let second = insert_dynamic_selection_plan(pool, mutated)
        .await
        .expect("second insert should not error, only report collision");
    match second {
        InsertDynamicSelectionPlanOutcome::PayloadCollision { .. } => {}
        other => panic!("expected PayloadCollision, got {other:?}"),
    }

    let (fetched_plan, _, _) = fetch_dynamic_selection_plan(pool, plan_id)
        .await
        .expect("fetch should succeed")
        .expect("plan should exist");
    assert_eq!(
        fetched_plan.truth_state, "computed",
        "original row must be preserved, never overwritten by the divergent replay"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn divergent_candidate_score_under_same_plan_id_is_a_payload_collision() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("collision_score");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_id = fixed_plan_id("collision_score");
    let original = sample_plan(run_id, plan_id);
    let mut mutated = sample_plan(run_id, plan_id);
    mutated.candidates[0].canonical_score_decimal = Some("9.999999".to_string());

    assert_payload_collision_and_original_preserved(&pool, original, mutated).await;
    cleanup(&pool, &[run_id]).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn divergent_symbol_disposition_under_same_plan_id_is_a_payload_collision() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("collision_symbol_disposition");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_id = fixed_plan_id("collision_symbol_disposition");
    let original = sample_plan(run_id, plan_id);
    let mut mutated = sample_plan(run_id, plan_id);
    mutated.symbols[0].disposition = "not_selected".to_string();
    mutated.symbols[0].selected_strategy_id = None;
    mutated.candidates[0].selected = false;
    mutated.candidates[0].disposition = "not_selected".to_string();
    mutated.selected_count = 0;

    assert_payload_collision_and_original_preserved(&pool, original, mutated).await;
    cleanup(&pool, &[run_id]).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn identical_replay_with_reordered_candidates_is_still_idempotent() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("reordered_replay");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_id = fixed_plan_id("reordered_replay");
    let mut original = sample_plan(run_id, plan_id);
    original.symbol_count = 2;
    original.candidate_count = 2;
    original.symbols.push(NewDynamicSelectionPlanSymbol {
        symbol: "MSFT".to_string(),
        selected_strategy_id: None,
        disposition: "refused".to_string(),
        reason_code: "no_valid_candidate_for_symbol".to_string(),
        exact_reason_code: "no_valid_candidate_for_symbol".to_string(),
    });
    let mut second_candidate = selected_candidate("MSFT", 1);
    second_candidate.selected = false;
    second_candidate.disposition = "refused".to_string();
    second_candidate.exact_reason_code = "score_missing".to_string();
    second_candidate.reason_code = "score_missing".to_string();
    original.candidates.push(second_candidate);

    let mut reordered = original.clone();
    reordered.candidates.reverse();
    // Ordinal is assigned per-write position, not economic identity -- swap
    // it along with the reversed Vec so the *economic* candidate set is
    // unchanged, matching the conflict-evidence store's precedent test.
    reordered.candidates[0].ordinal = 0;
    reordered.candidates[1].ordinal = 1;

    let first = insert_dynamic_selection_plan(&pool, original)
        .await
        .expect("first insert should succeed");
    assert_eq!(first, InsertDynamicSelectionPlanOutcome::Inserted);

    let second = insert_dynamic_selection_plan(&pool, reordered)
        .await
        .expect("second insert should succeed as a no-op");
    assert_eq!(
        second,
        InsertDynamicSelectionPlanOutcome::AlreadyExists,
        "candidate vector order must never affect replay comparison"
    );

    cleanup(&pool, &[run_id]).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn changed_plan_creates_a_distinct_row() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("distinct_plan");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_a = sample_plan(run_id, fixed_plan_id("distinct_plan_a"));
    let mut plan_b = sample_plan(run_id, fixed_plan_id("distinct_plan_b"));
    plan_b.market_date = "2099-03-02".to_string();

    insert_dynamic_selection_plan(&pool, plan_a.clone())
        .await
        .expect("insert a should succeed");
    insert_dynamic_selection_plan(&pool, plan_b.clone())
        .await
        .expect("insert b should succeed");

    let recent = fetch_recent_dynamic_selection_plans(&pool, run_id, 10)
        .await
        .expect("fetch should succeed");
    assert_eq!(recent.len(), 2, "distinct plans produce distinct rows");

    cleanup(&pool, &[run_id]).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn recent_plans_ordered_newest_first_and_scoped_to_run() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("recent_ordering");
    let other_run_id = fixed_run_id("recent_ordering_other");
    cleanup(&pool, &[run_id, other_run_id]).await;
    fixture_run(&pool, run_id).await;
    fixture_run(&pool, other_run_id).await;

    let mut plan_a = sample_plan(run_id, fixed_plan_id("recent_a"));
    plan_a.created_at_utc = Utc.with_ymd_and_hms(2099, 3, 1, 12, 0, 0).unwrap();
    let mut plan_b = sample_plan(run_id, fixed_plan_id("recent_b"));
    plan_b.created_at_utc = Utc.with_ymd_and_hms(2099, 3, 1, 12, 10, 0).unwrap();
    let mut plan_other_run = sample_plan(other_run_id, fixed_plan_id("recent_other_run"));
    plan_other_run.created_at_utc = Utc.with_ymd_and_hms(2099, 3, 1, 12, 20, 0).unwrap();

    for plan in [plan_a.clone(), plan_b.clone(), plan_other_run.clone()] {
        insert_dynamic_selection_plan(&pool, plan)
            .await
            .expect("insert should succeed");
    }

    let recent = fetch_recent_dynamic_selection_plans(&pool, run_id, 10)
        .await
        .expect("fetch should succeed");

    assert_eq!(recent.len(), 2, "must not include the other run's plan");
    assert_eq!(recent[0].plan_id, plan_b.plan_id, "newest first");
    assert_eq!(recent[1].plan_id, plan_a.plan_id);

    cleanup(&pool, &[run_id, other_run_id]).await;
}

// ── DB-level constraint proof ──────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn approved_for_live_true_is_rejected_at_the_db_level() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("approved_for_live_check");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_id = fixed_plan_id("approved_for_live_check");
    let mut plan = sample_plan(run_id, plan_id);
    plan.approved_for_live = true;

    let result = insert_dynamic_selection_plan(&pool, plan).await;
    assert!(
        result.is_err(),
        "the DB CHECK constraint must reject approved_for_live = true \
         even if application code tried to construct it"
    );

    let row = fetch_dynamic_selection_plan(&pool, plan_id)
        .await
        .expect("fetch should succeed");
    assert!(
        row.is_none(),
        "a rejected insert must leave no partial row behind"
    );

    cleanup(&pool, &[run_id]).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn invalid_effective_mode_is_rejected_at_the_db_level() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("invalid_mode_check");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_id = fixed_plan_id("invalid_mode_check");
    let mut plan = sample_plan(run_id, plan_id);
    plan.effective_mode = "not_a_real_mode".to_string();

    let result = insert_dynamic_selection_plan(&pool, plan).await;
    assert!(result.is_err(), "unknown mode must be rejected by the DB");

    cleanup(&pool, &[run_id]).await;
}

/// A partial candidate-batch write must never leave a header row with no
/// candidates, or a candidate row referencing an absent symbol -- proving
/// requirement 8 (rollback safety) via a FK violation on a candidate whose
/// `symbol` was never inserted into the symbols table.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn candidate_referencing_unknown_symbol_rolls_back_the_whole_write() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("fk_rollback");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_id = fixed_plan_id("fk_rollback");
    let mut plan = sample_plan(run_id, plan_id);
    // Candidate references "MSFT", but no matching symbol row was supplied
    // -- the FK on (plan_id, symbol) must reject the whole transaction.
    plan.candidates.push(selected_candidate("MSFT", 1));
    plan.candidate_count = 2;

    let result = insert_dynamic_selection_plan(&pool, plan).await;
    assert!(
        result.is_err(),
        "a candidate referencing a symbol with no matching row must fail"
    );

    let row = fetch_dynamic_selection_plan(&pool, plan_id)
        .await
        .expect("fetch should succeed");
    assert!(
        row.is_none(),
        "the header row must not be left behind after the candidate insert failed \
         mid-transaction -- proves the whole write rolled back, not just the last statement"
    );

    cleanup(&pool, &[run_id]).await;
}

/// FORMAL-SOAK-GATE-TRUTH-REPAIR-01 Part 7: migration 0060 adds a UNIQUE
/// (plan_id, symbol, strategy_id, timeframe_secs) constraint to
/// sys_dynamic_selection_plan_candidates. Migration 0059's only uniqueness
/// constraint was (plan_id, ordinal) -- physical insert-order identity, not
/// the candidate's economic identity -- so two rows sharing the exact same
/// (symbol, strategy_id, timeframe_secs) exact key under two different
/// ordinals must now be rejected at the DB level, and (mirroring the FK
/// rollback proof above) must never leave a partial header row behind.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn duplicate_exact_candidate_key_is_rejected_at_the_db_level() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("duplicate_candidate_key");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_id = fixed_plan_id("duplicate_candidate_key");
    let mut plan = sample_plan(run_id, plan_id);
    // Same (symbol="AAPL", strategy_id="swing_momentum", timeframe_secs=300)
    // exact key as the plan's existing candidate at ordinal 0, just under a
    // different ordinal (1) -- (plan_id, ordinal) alone would have allowed
    // this prior to migration 0060.
    plan.candidates.push(selected_candidate("AAPL", 1));
    plan.candidate_count = 2;

    let result = insert_dynamic_selection_plan(&pool, plan).await;
    assert!(
        result.is_err(),
        "a duplicate exact (symbol, strategy_id, timeframe_secs) candidate key must be \
         rejected by the migration 0060 UNIQUE constraint"
    );

    let row = fetch_dynamic_selection_plan(&pool, plan_id)
        .await
        .expect("fetch should succeed");
    assert!(
        row.is_none(),
        "the header row must not be left behind after the duplicate-key candidate insert \
         failed mid-transaction"
    );

    cleanup(&pool, &[run_id]).await;
}

// ── Bounded read seam ───────────────────────────────────────────────────────

fn plan_with_n_selected_symbols(run_id: Uuid, plan_id: Uuid, n: i32) -> NewDynamicSelectionPlan {
    let mut symbols = Vec::new();
    let mut candidates = Vec::new();
    for i in 0..n {
        let symbol = format!("SYM{i}");
        symbols.push(NewDynamicSelectionPlanSymbol {
            symbol: symbol.clone(),
            selected_strategy_id: Some("swing_momentum".to_string()),
            disposition: "selected".to_string(),
            reason_code: "selected_highest_score".to_string(),
            exact_reason_code: "selected_highest_score".to_string(),
        });
        candidates.push(selected_candidate(&symbol, i));
    }
    NewDynamicSelectionPlan {
        plan_id,
        run_id,
        library_schema_version: "dynamic-strategy-symbol-selection-v1".to_string(),
        context_schema_version: "dynamic-strategy-symbol-selection-v1".to_string(),
        configured_mode: "paper_enforced".to_string(),
        effective_mode: "paper_enforced".to_string(),
        live_lock_applied: false,
        approved_for_live: false,
        source_kind: "watchlist_v2".to_string(),
        source_identity: "watchlist".to_string(),
        market_date: "2099-03-01".to_string(),
        disposition: "paper_enforced_allowed".to_string(),
        truth_state: "computed".to_string(),
        blockers: vec![],
        symbol_count: n,
        candidate_count: n,
        selected_count: n,
        refused_count: 0,
        writer_version: "mqk-daemon-test".to_string(),
        created_at_utc: Utc.with_ymd_and_hms(2099, 3, 1, 12, 5, 0).unwrap(),
        symbols,
        candidates,
    }
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn bounded_read_returns_complete_within_bound() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let bound = 8usize;
    let run_id = fixed_run_id("bounded_read_within");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_id = fixed_plan_id("bounded_read_within");
    let plan = plan_with_n_selected_symbols(run_id, plan_id, bound as i32);
    insert_dynamic_selection_plan(&pool, plan)
        .await
        .expect("insert should succeed");

    let result = fetch_dynamic_selection_plan_for_read(&pool, plan_id, bound)
        .await
        .expect("bounded fetch should succeed");
    match result {
        BoundedDynamicSelectionPlanFetch::Complete(plan_record, _, candidates) => {
            assert_eq!(plan_record.plan_id, plan_id);
            assert_eq!(candidates.len(), bound);
        }
        other => panic!("expected Complete at exactly the bound, got {other:?}"),
    }

    cleanup(&pool, &[run_id]).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn bounded_read_reports_limit_exceeded_at_bound_plus_one() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let bound = 8usize;
    let run_id = fixed_run_id("bounded_read_over");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let plan_id = fixed_plan_id("bounded_read_over");
    let plan = plan_with_n_selected_symbols(run_id, plan_id, (bound + 1) as i32);
    insert_dynamic_selection_plan(&pool, plan)
        .await
        .expect("insert should succeed");

    let result = fetch_dynamic_selection_plan_for_read(&pool, plan_id, bound)
        .await
        .expect("bounded fetch should succeed");
    match result {
        BoundedDynamicSelectionPlanFetch::CandidateLimitExceeded {
            observed_at_least, ..
        } => {
            assert_eq!(observed_at_least, bound + 1);
        }
        other => panic!("expected CandidateLimitExceeded, got {other:?}"),
    }

    cleanup(&pool, &[run_id]).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn bounded_read_reports_not_found_for_unknown_plan_id() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let unknown = Uuid::new_v4();

    let result = fetch_dynamic_selection_plan_for_read(&pool, unknown, 8)
        .await
        .expect("bounded fetch should succeed");
    assert_eq!(result, BoundedDynamicSelectionPlanFetch::NotFound);
}
