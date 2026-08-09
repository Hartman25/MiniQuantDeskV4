//! STRATEGY-DECISION-IDEMPOTENCY-01, extended by
//! STRATEGY-DECISION-ECONOMIC-IDEMPOTENCY-02
//!
//! # Problem addressed (-01)
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
//! -01 fixed this by seeding `decision_id` from
//! `run_id|strategy_id|symbol|timeframe_secs|side|qty|bar_end_ts` -- the
//! exact completed-bar identity, never wall-clock time.
//!
//! # -01's remaining gap, closed by -02
//!
//! -01 anchored identity on `bar_end_ts` but still folded in the *derived*
//! order quantity (`qty` = `delta = target.qty - current`) and `side`.
//! `current` is read from the live portfolio snapshot, which moves the
//! instant the strategy's own already-working order for this exact
//! bar/target receives a partial fill -- so re-evaluating the *same*
//! bar/target after a partial fill computed a *different* `delta`, and
//! therefore a *different* `decision_id`, for what is economically the
//! identical intent. That let a second order reach the outbox for the
//! remaining (post-partial-fill) delta while the original order's remaining
//! quantity was still working -- e.g. target=20, first order for 20 partial-
//! fills 10, a second 10-share order reaches the outbox for the "remaining"
//! delta while the original order's other 10 shares are still live: up to
//! 30 total shares against a target of 20.
//!
//! # -02 fix under test
//!
//! `decision_id` is now a UUIDv5 seeded from
//! `run_id|strategy_id|symbol|timeframe_secs|target_qty|bar_end_ts` --
//! `target_qty` is the strategy's raw *signed target position*
//! (`TargetPosition::qty`), never the derived `delta`/`side`. Because
//! `target_qty` does not move when a fill against the current in-flight
//! order updates `current`, the same logical intent always produces the
//! same `decision_id` regardless of how many times, or against what
//! intermediate `current`, it is re-evaluated. Since `decision_id` doubles
//! as the outbox `idempotency_key` (Gate 7, `ON CONFLICT (idempotency_key)
//! DO NOTHING`), this makes "at most one durable order per intent" a
//! property the database enforces directly: the *first* delta computed for
//! an intent is the one, and only one, ever durably submitted, matching the
//! same bar-anchored-identity pattern already proven by
//! `runtime_opportunity_allocation::compute_cycle_id`.
//!
//! Bundle 5's `rebuild_decision_with_qty` (capital-clamped resize) is fixed
//! the same way: it now reuses `original.decision_id` verbatim instead of
//! minting a new UUID salted with the loop-tick wall clock (`now_micros`) —
//! see the `c5_*`/`c6_*`/`neg_*` tests in
//! `runtime_opportunity_allocation.rs`'s own `#[cfg(test)]` module.
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
//! | C1   | (-02) Same bar + same target, current changes (simulating a partial   |
//! |      | fill of the already-working order): decision_id is UNCHANGED even     |
//! |      | though the derived delta differs                                        |
//! | C2   | (-02, MANDATORY, DB-backed) target=20, first order for 20 partially   |
//! |      | fills to current=10, same bar re-evaluates with current=10 (delta=10) |
//! |      | -> ZERO additional durable order; still exactly one outbox row, still  |
//! |      | qty=20, never 30 total possible shares                                 |
//! | C4   | DB-backed: same completed bar replay after a simulated process        |
//! |      | restart resolves to the same durable intent -- no second order         |
//! | C7   | A different completed bar is still allowed to produce a new intent     |
//! |      | even with the same target (bar_end_ts, not target, disambiguates)      |
//! | C10  | Multi-symbol: one symbol's in-flight quantity/current never affects   |
//! |      | another symbol's decision_id                                            |
//! | C9   | DB-backed: once an intent's order reaches a genuinely terminal FAILED |
//! |      | state, a same-bar re-evaluation is still refused as a duplicate --     |
//! |      | no automatic same-bar retry (conservative fail-closed policy)          |
//! | NEG  | Negative control: the OLD (-01) delta-anchored seed formula, applied  |
//! |      | by hand to the same two re-evaluations C1/C2 exercise, DOES produce   |
//! |      | two different ids -- proving target-anchoring (not incidental test     |
//! |      | setup) is what makes C1/C2 pass                                         |
//!
//! # Proof boundary
//!
//! D01-D04, C1, C7, C10, NEG are pure (no IO). D05-D06, C2, C4, C9 are
//! DB-backed (port 5434 test Postgres) and load-bearing -- must fail hard if
//! `MQK_DATABASE_URL` is absent, not skip.

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
    // Both evaluations start from flat() (current=0), so target == delta
    // here -- this proves a genuinely different TARGET (not merely a
    // different derived delta from a moving current) still yields a
    // different decision_id; see C1 below for the complementary case (same
    // target, current moves, decision_id must NOT change).
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
// STRATEGY-DECISION-ECONOMIC-IDEMPOTENCY-02: C1/C7/C10/NEG (pure) and
// C2/C4 (DB-backed) — see module header for the full defect/fix narrative.
// ---------------------------------------------------------------------------

fn at(symbol: &str, qty: i64) -> BTreeMap<String, i64> {
    let mut m = BTreeMap::new();
    m.insert(symbol.to_string(), qty);
    m
}

/// C1 (MANDATORY): same bar + same target; `current` changes between
/// evaluations exactly as it would after a partial fill of the original
/// order (0 -> 10, simulating a 10-of-20 partial fill). `decision_id` must
/// be unchanged even though the derived delta (20, then 10) differs.
#[test]
fn c1_same_bar_same_target_current_moves_decision_id_unchanged() {
    let result = live_result(vec![TargetPosition::new("NVDA", 20)]);

    let before_fill = bar_result_to_decisions(&result, fixed_run_id(), BAR_A_END_TS, &flat());
    let after_partial_fill =
        bar_result_to_decisions(&result, fixed_run_id(), BAR_A_END_TS, &at("NVDA", 10));

    assert_eq!(before_fill.len(), 1);
    assert_eq!(after_partial_fill.len(), 1);
    assert_eq!(
        before_fill[0].decision_id, after_partial_fill[0].decision_id,
        "C1: decision_id must be stable across a current-position change caused by a partial \
         fill of this same intent's own already-working order"
    );
    // The derived delta legitimately differs (this is what Bundle 5/outbox
    // payload building uses `qty` for) -- only the identity is stable.
    assert_eq!(before_fill[0].qty, 20, "precondition: first delta is 20");
    assert_eq!(
        after_partial_fill[0].qty, 10,
        "precondition: second delta is 10 (target 20 - current 10)"
    );
}

/// C7: a genuinely different completed bar with the SAME target is still a
/// new, distinguishable intent -- bar_end_ts, not target, is what
/// disambiguates across real bar boundaries.
#[test]
fn c7_different_bar_same_target_is_a_new_intent() {
    let result = live_result(vec![TargetPosition::new("NVDA", 20)]);

    let bar_a = bar_result_to_decisions(&result, fixed_run_id(), BAR_A_END_TS, &flat());
    let bar_b = bar_result_to_decisions(&result, fixed_run_id(), BAR_B_END_TS, &flat());

    assert_ne!(
        bar_a[0].decision_id, bar_b[0].decision_id,
        "C7: a later completed bar with the identical target must still be a distinguishable \
         new economic intent"
    );
}

/// C10: one symbol's in-flight/partial-fill-adjusted current must never
/// affect a sibling symbol's decision_id in the same tick's target set.
#[test]
fn c10_multi_symbol_one_symbols_current_never_affects_another() {
    let result = live_result(vec![
        TargetPosition::new("NVDA", 20),
        TargetPosition::new("AMD", 20),
    ]);

    // NVDA has a partial fill reflected in current; AMD is untouched (flat).
    let mut current_with_nvda_partial = BTreeMap::new();
    current_with_nvda_partial.insert("NVDA".to_string(), 10i64);

    let baseline = bar_result_to_decisions(&result, fixed_run_id(), BAR_A_END_TS, &flat());
    let with_nvda_fill = bar_result_to_decisions(
        &result,
        fixed_run_id(),
        BAR_A_END_TS,
        &current_with_nvda_partial,
    );

    let amd_id_baseline = baseline
        .iter()
        .find(|d| d.symbol == "AMD")
        .expect("AMD decision present")
        .decision_id
        .clone();
    let amd_id_after_nvda_fill = with_nvda_fill
        .iter()
        .find(|d| d.symbol == "AMD")
        .expect("AMD decision present")
        .decision_id
        .clone();

    assert_eq!(
        amd_id_baseline, amd_id_after_nvda_fill,
        "C10: AMD's decision_id must be unaffected by NVDA's current-position change"
    );

    let nvda_id_baseline = baseline
        .iter()
        .find(|d| d.symbol == "NVDA")
        .expect("NVDA decision present")
        .decision_id
        .clone();
    let nvda_id_after_fill = with_nvda_fill
        .iter()
        .find(|d| d.symbol == "NVDA")
        .expect("NVDA decision present")
        .decision_id
        .clone();
    assert_eq!(
        nvda_id_baseline, nvda_id_after_fill,
        "C10: (restating C1) NVDA's own decision_id must also be unaffected by its own \
         partial-fill-driven current change"
    );
}

/// NEG: negative control proving the fix is load-bearing. Hand-computes
/// what the OLD (-01) delta-anchored seed formula
/// (`run_id|strategy_id|symbol|timeframe_secs|side|qty|bar_end_ts`) would
/// have produced for the exact two evaluations C1 exercises (target=20,
/// current 0 then 10). Under the old formula the two seeds differ (delta 20
/// vs 10), so the old scheme DOES mint two different ids for what is
/// economically the identical intent -- confirming target-anchoring, not
/// incidental test setup, is what makes C1/C2 hold.
#[test]
fn neg_old_delta_anchored_seed_would_have_produced_two_different_ids() {
    let run_id = fixed_run_id();
    let old_seed = |side: &str, qty: i64, bar_end_ts: i64| {
        Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            format!("mqk.strategy-decision.v2|{run_id}|test_strategy|NVDA|300|{side}|{qty}|{bar_end_ts}")
                .as_bytes(),
        )
        .to_string()
    };
    let old_id_before_fill = old_seed("buy", 20, BAR_A_END_TS);
    let old_id_after_partial_fill = old_seed("buy", 10, BAR_A_END_TS);

    assert_ne!(
        old_id_before_fill, old_id_after_partial_fill,
        "NEG: the -01 delta-anchored formula produces two DIFFERENT ids for the same bar/target \
         once current moves from a partial fill -- this is the exact -01 gap -02 closes; the new \
         formula (C1 above) must produce the SAME id for this exact scenario"
    );

    // Confirm the NEW (target-anchored) production function actually
    // resolves this ambiguity, contrasting directly with the old formula.
    let result = live_result(vec![TargetPosition::new("NVDA", 20)]);
    let new_before = bar_result_to_decisions(&result, run_id, BAR_A_END_TS, &flat());
    let new_after = bar_result_to_decisions(&result, run_id, BAR_A_END_TS, &at("NVDA", 10));
    assert_eq!(
        new_before[0].decision_id, new_after[0].decision_id,
        "NEG: the current production formula must NOT reproduce the old bug"
    );
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

// ---------------------------------------------------------------------------
// C2/C4 — DB-backed, real production seam (submit_internal_strategy_decision
// + the real outbox), STRATEGY-DECISION-ECONOMIC-IDEMPOTENCY-02
// ---------------------------------------------------------------------------

async fn outbox_qty(pool: &sqlx::PgPool, idempotency_key: &str) -> i64 {
    sqlx::query_scalar(
        "select (order_json->>'qty')::bigint from oms_outbox where idempotency_key = $1",
    )
    .bind(idempotency_key)
    .fetch_one(pool)
    .await
    .expect("qty query failed")
}

/// C2 (MANDATORY): the exact scenario the mission specifies. target=20;
/// first evaluation (current=0) submits and is accepted with qty=20.
/// Simulates a 10-share partial fill of that same order (current becomes
/// 10). The same bar re-evaluates: target is still 20, so the derived delta
/// is now 10 -- under the -01 scheme this would have minted a second,
/// distinct decision_id and reached the outbox as a genuinely new order.
/// Proves: zero additional durable order is created; exactly one outbox row
/// exists for this intent; that row's qty is still 20 (the original, never
/// silently replaced by the second attempt's smaller qty=10 payload) --
/// so the system cannot end up with 30 total possible shares
/// (10 filled + 10 original remaining + 10 new).
#[tokio::test]
async fn c2_partial_fill_reevaluation_creates_zero_additional_order() {
    let _ = require_db_url();
    mqk_db::run_isolated("c2_economic_idempotency", |pool| async move {
        let symbol = "NVDA";
        let (st, run_id) = seed_and_run(&pool, "test_strategy", symbol).await;
        let result = live_result(vec![TargetPosition::new(symbol, 20)]);

        // First evaluation: current=0, target=20 -> delta=20, BUY 20 submitted.
        let first = bar_result_to_decisions(&result, run_id, BAR_A_END_TS, &flat());
        assert_eq!(first[0].qty, 20, "precondition: first delta is 20");
        let decision_id = first[0].decision_id.clone();
        let outcome_first =
            submit_internal_strategy_decision(&st, first.into_iter().next().unwrap()).await;
        assert!(
            outcome_first.accepted,
            "C2: first submit (BUY 20) must be accepted; disposition={:?} blockers={:?}",
            outcome_first.disposition, outcome_first.blockers
        );
        assert_eq!(outbox_row_count(&pool, &decision_id).await, 1);
        assert_eq!(outbox_qty(&pool, &decision_id).await, 20);

        // Simulate a 10-share partial fill of that SAME order: current
        // moves from 0 to 10, but the original order still has 10 shares
        // outstanding (unchanged by this test -- current_positions here
        // models the portfolio-level fact a partial fill produces).
        let current_after_partial_fill = at(symbol, 10);

        // Same bar re-evaluates: target is still 20 (the strategy has not
        // seen a new completed bar), so delta is now 20-10=10.
        let second =
            bar_result_to_decisions(&result, run_id, BAR_A_END_TS, &current_after_partial_fill);
        assert_eq!(
            second[0].qty, 10,
            "precondition: second delta reflects the partial fill (20-10=10)"
        );
        assert_eq!(
            second[0].decision_id, decision_id,
            "C2: re-evaluation after the partial fill must compute the SAME decision_id \
             as the original order -- this is what makes the outbox conflict guard fire"
        );

        let outcome_second =
            submit_internal_strategy_decision(&st, second.into_iter().next().unwrap()).await;
        assert_eq!(
            outcome_second.disposition, "duplicate",
            "C2: the post-partial-fill re-evaluation must be refused as a duplicate, \
             never accepted as a second order"
        );

        // The decisive assertions: exactly one row, still the ORIGINAL
        // qty=20 payload -- never silently replaced by the second attempt's
        // qty=10, and never a second row alongside it.
        assert_eq!(
            outbox_row_count(&pool, &decision_id).await,
            1,
            "C2: exactly one outbox row must exist for this intent after the re-evaluation"
        );
        assert_eq!(
            outbox_qty(&pool, &decision_id).await,
            20,
            "C2: the durable row must still be the original qty=20 order -- \
             ON CONFLICT DO NOTHING must not have replaced it with the second attempt's qty=10"
        );

        // Total possible exposure: the ORIGINAL order (20, of which 10 is
        // already filled and 10 remains genuinely outstanding) is the only
        // order that exists. 10 (filled) + 10 (still working on the SAME
        // order) = 20, matching the target exactly -- never 30.
    })
    .await;
}

/// C4: same completed bar replayed after a simulated process restart, WITH
/// current reflecting a partial fill by the time of the post-restart
/// re-evaluation (the more realistic restart shape: a crash typically
/// happens mid-fill, not at qty=0) -- still resolves to the same durable
/// intent, zero additional order.
#[tokio::test]
async fn c4_restart_replay_with_partial_fill_creates_zero_additional_order() {
    let _ = require_db_url();
    mqk_db::run_isolated("c4_economic_idempotency", |pool| async move {
        let symbol = "AMD";
        let (st_before, run_id) = seed_and_run(&pool, "test_strategy", symbol).await;
        let result = live_result(vec![TargetPosition::new(symbol, 15)]);

        let before = bar_result_to_decisions(&result, run_id, BAR_A_END_TS, &flat());
        let decision_id = before[0].decision_id.clone();
        let outcome_before =
            submit_internal_strategy_decision(&st_before, before.into_iter().next().unwrap()).await;
        assert!(
            outcome_before.accepted,
            "C4: pre-restart submit must be accepted"
        );

        // Simulated restart + a partial fill (5 of 15) having landed before
        // the crash.
        let st_after = Arc::new(state::AppState::new_with_db(pool.clone()));
        st_after.inject_running_loop_for_test(run_id).await;
        let after = bar_result_to_decisions(&result, run_id, BAR_A_END_TS, &at(symbol, 5));
        assert_eq!(
            after[0].decision_id, decision_id,
            "C4: post-restart re-evaluation with a partial-fill-adjusted current must still \
             compute the identical decision_id"
        );
        let outcome_after =
            submit_internal_strategy_decision(&st_after, after.into_iter().next().unwrap()).await;
        assert_eq!(outcome_after.disposition, "duplicate");

        assert_eq!(outbox_row_count(&pool, &decision_id).await, 1);
        assert_eq!(
            outbox_qty(&pool, &decision_id).await,
            15,
            "C4: the durable row must remain the original qty=15 order after restart replay"
        );
    })
    .await;
}

/// C9: explicit policy for a terminal-but-unsuccessful attempt on the same
/// bar/intent. Once the original order for an intent reaches FAILED (a
/// genuinely terminal, non-success state -- e.g. dispatch attempts
/// exhausted), a re-evaluation of the SAME bar/target is STILL refused as a
/// duplicate, never silently retried. This is the "conservative fail-closed"
/// policy the design explicitly sanctions in place of a bounded retry
/// counter: the next real retry opportunity is the next completed bar (a
/// genuinely new `bar_end_ts`), not an unlimited same-bar respawn loop.
#[tokio::test]
async fn c9_terminal_failed_attempt_on_same_bar_is_not_retried() {
    let _ = require_db_url();
    mqk_db::run_isolated("c9_economic_idempotency", |pool| async move {
        let symbol = "MSFT";
        let (st, run_id) = seed_and_run(&pool, "test_strategy", symbol).await;
        let result = live_result(vec![TargetPosition::new(symbol, 8)]);

        let first = bar_result_to_decisions(&result, run_id, BAR_A_END_TS, &flat());
        let decision_id = first[0].decision_id.clone();
        let outcome_first =
            submit_internal_strategy_decision(&st, first.into_iter().next().unwrap()).await;
        assert!(
            outcome_first.accepted,
            "C9 precondition: first submit accepted"
        );

        // Drive the row to a genuinely terminal FAILED state through the
        // real production transitions (PENDING -> CLAIMED -> FAILED), not a
        // hand-rolled status write.
        let claimed = mqk_db::outbox_claim_batch_for_run(
            &pool,
            run_id,
            10,
            "c9-test-dispatcher",
            chrono::Utc::now(),
        )
        .await
        .expect("claim");
        assert_eq!(claimed.len(), 1, "C9 precondition: exactly one row claimed");
        let failed = mqk_db::outbox_mark_failed(&pool, &decision_id)
            .await
            .expect("mark_failed");
        assert!(failed, "C9 precondition: row must transition to FAILED");

        // Same bar re-evaluates (target unchanged) -- decision_id is
        // identical, so the outbox conflict guard refuses it regardless of
        // the existing row's terminal FAILED status.
        let retry = bar_result_to_decisions(&result, run_id, BAR_A_END_TS, &flat());
        assert_eq!(retry[0].decision_id, decision_id);
        let outcome_retry =
            submit_internal_strategy_decision(&st, retry.into_iter().next().unwrap()).await;
        assert_eq!(
            outcome_retry.disposition, "duplicate",
            "C9: a same-bar re-evaluation after the original attempt FAILED must still be \
             refused as a duplicate -- no automatic same-bar retry"
        );
        assert_eq!(
            outbox_row_count(&pool, &decision_id).await,
            1,
            "C9: exactly one row must exist for this intent, permanently FAILED, never replaced"
        );
    })
    .await;
}
