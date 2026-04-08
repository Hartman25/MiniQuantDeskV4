//! B1C-close-final: Native strategy output → canonical admission seam bridge.
//!
//! Proves that the execution loop correctly translates `StrategyBarResult`
//! output from `tick_strategy_dispatch` into `InternalStrategyDecision`s and
//! forwards them through `submit_internal_strategy_decision` (the canonical
//! 7-gate internal admission seam with `signal_source = "internal_strategy_decision"`).
//!
//! # Translation semantics (B1C-close-final)
//!
//! `TargetPosition.qty` is a signed target portfolio state, NOT an incremental
//! order size.  The decision qty is the delta between the target and the current
//! held position derived from the execution snapshot.  Symbol absent from the
//! position map → treated as flat (current = 0).
//!
//! # Test inventory
//!
//! | ID  | Condition                                                    | Expected                                                    |
//! |-----|--------------------------------------------------------------|-------------------------------------------------------------|
//! | C01 | Shadow intent (`should_execute=false`)                       | `bar_result_to_decisions` → empty (fail-closed)             |
//! | C02 | Empty targets (Live intent)                                  | `bar_result_to_decisions` → empty                           |
//! | C03 | Live, buy target, flat position                              | side="buy", qty=target (delta from flat), market/day        |
//! | C04 | Live, sell target, flat position                             | side="sell", qty=abs(target) (delta from flat)              |
//! | C05 | Zero-qty target from flat mixed with non-zero                | delta=0 skipped; non-zero included                          |
//! | C06 | Live intent → submit_internal_strategy_decision called       | disposition≠"rejected" (seam reached, DB gate fires)        |
//! | C07 | decision_id is deterministic UUIDv5                          | same inputs → same decision_id                              |
//! | C08 | Multi-target result, all flat → one decision per non-zero    | correct count                                               |
//! | C09 | Partial position: target=+15, current=+10                    | buy 5 (delta only)                                          |
//! | C10 | Already at target: target=+10, current=+10                   | no decision (delta=0)                                       |
//! | C11 | Close position: target=0, current=+10                        | sell 10 (delta=-10)                                         |
//! | C12 | Short cover + go long: target=+5, current=-3                 | buy 8 (delta=+8)                                            |
//! | C13 | Cover short: target=0, current=-7                            | buy 7 (delta=+7)                                            |
//! | C14 | DB-backed: loop path → durable outbox row, correct source    | signal_source="internal_strategy_decision" (requires DB)    |

use std::collections::BTreeMap;
use std::sync::Arc;

use uuid::Uuid;

use mqk_daemon::decision::{bar_result_to_decisions, submit_internal_strategy_decision};
use mqk_daemon::state::{self, AppState};
use mqk_strategy::{IntentMode, StrategyBarResult, StrategyIntents, StrategyOutput, StrategySpec,
    TargetPosition};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn live_result(targets: Vec<TargetPosition>) -> StrategyBarResult {
    StrategyBarResult {
        spec: StrategySpec::new("test_strategy", 300),
        intents: StrategyIntents {
            mode: IntentMode::Live,
            output: StrategyOutput { targets },
        },
    }
}

fn shadow_result(targets: Vec<TargetPosition>) -> StrategyBarResult {
    StrategyBarResult {
        spec: StrategySpec::new("test_strategy", 300),
        intents: StrategyIntents {
            mode: IntentMode::Shadow,
            output: StrategyOutput { targets },
        },
    }
}

fn fixed_run_id() -> Uuid {
    Uuid::nil()
}

const FIXED_NOW_MICROS: i64 = 1_700_000_000_000_000;

/// Empty position map — all symbols treated as flat (current qty = 0).
fn flat() -> BTreeMap<String, i64> {
    BTreeMap::new()
}

/// Position map with one symbol at a given signed qty.
fn pos(symbol: &str, qty: i64) -> BTreeMap<String, i64> {
    let mut m = BTreeMap::new();
    m.insert(symbol.to_string(), qty);
    m
}

async fn bare_state() -> Arc<AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ))
}

// ---------------------------------------------------------------------------
// C01 — Shadow intent → bar_result_to_decisions returns empty (fail-closed)
// ---------------------------------------------------------------------------

/// C01: Shadow-mode result → `bar_result_to_decisions` returns empty.
///
/// Proves the primary fail-closed rule: shadow-mode outputs are never forwarded
/// to the admission seam, regardless of target content.  No broker call, no
/// outbox write, no admission attempt.
#[test]
fn b1c_c01_shadow_intent_produces_no_decisions() {
    let result = shadow_result(vec![TargetPosition::new("AAPL", 10)]);
    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &flat());

    assert!(
        decisions.is_empty(),
        "C01: shadow-mode result must produce zero decisions (fail-closed, no submission)"
    );
}

// ---------------------------------------------------------------------------
// C02 — Empty targets (Live intent) → empty decisions
// ---------------------------------------------------------------------------

/// C02: Live-mode result with empty targets → `bar_result_to_decisions` returns empty.
///
/// Proves no-op bar handling: a strategy that returns no targets (e.g. lookback
/// not yet satisfied) produces zero decisions.  Correct conservative behavior.
#[test]
fn b1c_c02_empty_targets_produces_no_decisions() {
    let result = live_result(vec![]);
    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &flat());

    assert!(
        decisions.is_empty(),
        "C02: Live intent with empty targets must produce zero decisions"
    );
}

// ---------------------------------------------------------------------------
// C03 — Live intent, buy target → correct InternalStrategyDecision fields
// ---------------------------------------------------------------------------

/// C03: Live intent with a positive-qty target → buy decision with correct fields.
///
/// Proves translation correctness for the buy direction:
/// - `side = "buy"`
/// - `qty = target.qty` (positive, unchanged)
/// - `order_type = "market"` (target positions carry no limit price)
/// - `time_in_force = "day"`
/// - `limit_price = None`
/// - `strategy_id = result.spec.name`
/// - `symbol = target.symbol`
#[test]
fn b1c_c03_live_buy_target_correct_fields() {
    // Flat position: target=+15, current=0 → delta=+15 → buy 15.
    let result = live_result(vec![TargetPosition::new("AAPL", 15)]);
    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &flat());

    assert_eq!(decisions.len(), 1, "C03: one target → one decision");
    let d = &decisions[0];

    assert_eq!(d.strategy_id, "test_strategy", "C03: strategy_id from spec.name");
    assert_eq!(d.symbol, "AAPL", "C03: symbol from target");
    assert_eq!(d.side, "buy", "C03: positive qty → buy");
    assert_eq!(d.qty, 15, "C03: qty unchanged for buy");
    assert_eq!(d.order_type, "market", "C03: target positions → market order");
    assert_eq!(d.time_in_force, "day", "C03: time_in_force = day");
    assert!(d.limit_price.is_none(), "C03: no limit price for market orders");
    assert!(!d.decision_id.is_empty(), "C03: decision_id must not be blank");
}

// ---------------------------------------------------------------------------
// C04 — Live intent, sell target (qty < 0) → side="sell", qty=abs
// ---------------------------------------------------------------------------

/// C04: Live intent with a negative-qty target → sell decision with abs qty.
///
/// Proves translation for the sell direction:
/// - `side = "sell"`
/// - `qty = -target.qty` (absolute value; decision qty must be positive)
#[test]
fn b1c_c04_live_sell_target_correct_fields() {
    // Flat position: target=-8, current=0 → delta=-8 → sell 8.
    let result = live_result(vec![TargetPosition::new("TSLA", -8)]);
    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &flat());

    assert_eq!(decisions.len(), 1, "C04: one target → one decision");
    let d = &decisions[0];

    assert_eq!(d.symbol, "TSLA", "C04: symbol from target");
    assert_eq!(d.side, "sell", "C04: negative qty → sell");
    assert_eq!(d.qty, 8, "C04: qty is absolute value of target.qty");
}

// ---------------------------------------------------------------------------
// C05 — Zero-qty target skipped; non-zero included
// ---------------------------------------------------------------------------

/// C05: Zero-qty target is skipped; non-zero targets are included.
///
/// Proves that qty=0 targets (no-op position) do not produce decisions.
/// Only non-zero targets advance to admission.
#[test]
fn b1c_c05_zero_qty_target_skipped() {
    // All from flat. AAPL/GOOG target=0, delta=0 → skip. MSFT/TSLA non-zero delta.
    let result = live_result(vec![
        TargetPosition::new("AAPL", 0),   // target=0, flat → delta=0, skip
        TargetPosition::new("MSFT", 5),   // target=+5, flat → delta=+5, include
        TargetPosition::new("GOOG", 0),   // target=0, flat → delta=0, skip
        TargetPosition::new("TSLA", -3),  // target=-3, flat → delta=-3, include
    ]);
    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &flat());

    assert_eq!(
        decisions.len(),
        2,
        "C05: two non-zero targets → two decisions; zero-qty targets must be skipped"
    );
    let syms: Vec<&str> = decisions.iter().map(|d| d.symbol.as_str()).collect();
    assert!(syms.contains(&"MSFT"), "C05: MSFT (qty=5) must be included");
    assert!(syms.contains(&"TSLA"), "C05: TSLA (qty=-3) must be included");
}

// ---------------------------------------------------------------------------
// C06 — Live intent → submit_internal_strategy_decision reached (no DB)
// ---------------------------------------------------------------------------

/// C06: Live intent with a valid target → `submit_internal_strategy_decision`
/// is called and returns a structured outcome.
///
/// Without a DB the call returns `disposition = "unavailable"` at Gate 2
/// (DB must be present).  This proves the canonical seam is reached and
/// the decision passes Gate 0 (field validation).  It does NOT return
/// "rejected" (field error), proving the translation is correct.
///
/// Gate 2 refusal is expected and honest: no DB → no durable execution.
/// The seam is reached; the fail-closed DB gate fires correctly.
#[tokio::test]
async fn b1c_c06_live_intent_reaches_canonical_seam() {
    let st = bare_state().await;

    let result = live_result(vec![TargetPosition::new("AAPL", 10)]);
    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &flat());
    assert_eq!(decisions.len(), 1, "C06 precondition: one target → one decision");

    let decision = decisions.into_iter().next().unwrap();
    let outcome = submit_internal_strategy_decision(&st, decision).await;

    // Must NOT be "rejected" (that would mean field validation failed).
    // Must be "unavailable" (Gate 2: no DB configured in this test state).
    assert_ne!(
        outcome.disposition, "rejected",
        "C06: decision must pass field validation (Gate 0); 'rejected' means translation error"
    );
    assert_eq!(
        outcome.disposition, "unavailable",
        "C06: no DB → Gate 2 fires 'unavailable'; seam is reached and functioning"
    );
}

// ---------------------------------------------------------------------------
// C07 — decision_id is deterministic (same inputs → same id)
// ---------------------------------------------------------------------------

/// C07: `bar_result_to_decisions` produces a deterministic `decision_id`.
///
/// Same result + same run_id + same now_micros → identical decision_id.
/// This proves the UUIDv5 derivation is stable and idempotent: crash-restart
/// within the same microsecond window produces the same key, which the outbox
/// deduplicates safely (ON CONFLICT DO NOTHING).
#[test]
fn b1c_c07_decision_id_is_deterministic() {
    let result = live_result(vec![TargetPosition::new("NVDA", 20)]);
    let run_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();

    let d1 = bar_result_to_decisions(&result, run_id, FIXED_NOW_MICROS, &flat());
    let d2 = bar_result_to_decisions(&result, run_id, FIXED_NOW_MICROS, &flat());

    assert_eq!(
        d1[0].decision_id, d2[0].decision_id,
        "C07: same inputs must produce the same decision_id (deterministic UUIDv5)"
    );
}

// ---------------------------------------------------------------------------
// C08 — Multi-target result → one decision per non-zero target
// ---------------------------------------------------------------------------

/// C08: Multiple non-zero targets in one bar result → one decision per target.
///
/// Proves the translation iterates all targets individually.  Each target
/// becomes exactly one `InternalStrategyDecision` submitted through its own
/// independent seam call.
#[test]
fn b1c_c08_multi_target_produces_one_decision_each() {
    let result = live_result(vec![
        TargetPosition::new("AAPL", 5),
        TargetPosition::new("MSFT", -3),
        TargetPosition::new("GOOG", 1),
    ]);
    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &flat());

    assert_eq!(
        decisions.len(),
        3,
        "C08: three non-zero targets → three decisions"
    );
    // All carry the same strategy_id from spec.name.
    for d in &decisions {
        assert_eq!(d.strategy_id, "test_strategy", "C08: strategy_id from spec.name");
        assert_eq!(d.order_type, "market", "C08: all decisions are market orders");
    }
}

// ---------------------------------------------------------------------------
// C09-C13 — Delta-to-target semantics (B1C-close-final)
//
// These tests prove that bar_result_to_decisions computes the ORDER qty as the
// delta between the target portfolio state and the current held position, not
// the raw target qty.
// ---------------------------------------------------------------------------

/// C09: Partial position — target=+15, current=+10 → buy 5 (delta only).
///
/// Proves that when the strategy targets 15 shares and we already hold 10,
/// the order is for 5 (the incremental amount), not 15.
#[test]
fn b1c_c09_delta_buy_from_partial_position() {
    let result = live_result(vec![TargetPosition::new("AAPL", 15)]);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("AAPL", 10));

    assert_eq!(decisions.len(), 1, "C09: one target → one decision");
    let d = &decisions[0];
    assert_eq!(d.side, "buy", "C09: positive delta → buy");
    assert_eq!(d.qty, 5, "C09: qty = delta = target(15) - current(10)");
}

/// C10: Already at target — target=+10, current=+10 → no decision (delta=0).
///
/// Proves that when the portfolio already holds the target qty, no order is
/// generated.  This prevents unnecessary round-trip orders on re-ticks.
#[test]
fn b1c_c10_already_at_target_produces_no_decision() {
    let result = live_result(vec![TargetPosition::new("AAPL", 10)]);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("AAPL", 10));

    assert!(
        decisions.is_empty(),
        "C10: target == current → delta=0 → no decision; got: {:?}",
        decisions.iter().map(|d| (&d.symbol, &d.side, d.qty)).collect::<Vec<_>>()
    );
}

/// C11: Close position — target=0, current=+10 → sell 10.
///
/// Proves that a zero-target generates a sell order to close the position,
/// not a no-op (which the old qty!=0 filter would have incorrectly produced).
#[test]
fn b1c_c11_close_long_position() {
    let result = live_result(vec![TargetPosition::new("AAPL", 0)]);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("AAPL", 10));

    assert_eq!(decisions.len(), 1, "C11: close-position target → one sell decision");
    let d = &decisions[0];
    assert_eq!(d.side, "sell", "C11: delta=-10 → sell");
    assert_eq!(d.qty, 10, "C11: sell qty = current holdings");
}

/// C12: Short cover + go long — target=+5, current=-3 → buy 8.
///
/// Proves that a direction reversal is computed correctly: delta = +5 - (-3) = +8.
#[test]
fn b1c_c12_short_cover_and_go_long() {
    let result = live_result(vec![TargetPosition::new("TSLA", 5)]);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("TSLA", -3));

    assert_eq!(decisions.len(), 1, "C12: one reversal target → one decision");
    let d = &decisions[0];
    assert_eq!(d.side, "buy", "C12: delta=+8 → buy");
    assert_eq!(d.qty, 8, "C12: qty = target(+5) - current(-3) = 8");
}

/// C13: Cover short — target=0, current=-7 → buy 7.
///
/// Proves that closing a short position (zero target, negative current)
/// generates a buy, not a no-op.
#[test]
fn b1c_c13_close_short_position() {
    let result = live_result(vec![TargetPosition::new("NVDA", 0)]);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("NVDA", -7));

    assert_eq!(decisions.len(), 1, "C13: close-short target → one buy decision");
    let d = &decisions[0];
    assert_eq!(d.side, "buy", "C13: delta=+7 → buy to close short");
    assert_eq!(d.qty, 7, "C13: qty = abs(delta) = 7");
}

// ---------------------------------------------------------------------------
// C14 — DB-backed: loop-owned path reaches durable outbox with correct source
// ---------------------------------------------------------------------------

/// C14: DB-backed proof that the loop-owned translation path
/// (`bar_result_to_decisions` → `submit_internal_strategy_decision`) produces a
/// durable outbox row with `signal_source = "internal_strategy_decision"`.
///
/// Also proves idempotency: submitting the same `decision_id` a second time
/// returns `disposition = "duplicate"` with no second row inserted.
///
/// Requires `MQK_DATABASE_URL`.  Run with:
///   `MQK_DATABASE_URL=postgres://... cargo test -p mqk-daemon \
///    --test scenario_native_strategy_bridge_b1c -- --include-ignored`
#[tokio::test]
#[ignore]
async fn b1c_c14_loop_path_creates_durable_outbox_row() {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(u) => u,
        Err(_) => {
            eprintln!("C14: MQK_DATABASE_URL not set — skipping DB-backed proof");
            return;
        }
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("C14: connect to test DB");
    mqk_db::migrate(&pool).await.expect("C14: run migrations");

    // Seed strategy registry.
    let strategy_id = "test_strategy";
    let ts = chrono::Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        &pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: "B1C Test Strategy".to_string(),
            enabled: true,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("C14: seed strategy registry");

    // Build AppState with DB and arm state.
    let st = Arc::new(state::AppState::new_with_db(pool.clone()));
    mqk_db::persist_arm_state_canonical(
        &pool,
        mqk_db::ArmState::Armed,
        None,
    )
    .await
    .expect("C14: arm state");

    // Seed an active run.
    let run_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({"source": "b1c_c14"}),
            host_fingerprint: "test-host".to_string(),
        },
    )
    .await
    .expect("C14: insert_run");
    mqk_db::arm_run(&pool, run_id).await.expect("C14: arm_run");
    mqk_db::begin_run(&pool, run_id).await.expect("C14: begin_run");
    mqk_db::heartbeat_run(&pool, run_id, now).await.expect("C14: heartbeat_run");
    st.inject_running_loop_for_test(run_id).await;

    // --- Exercise the loop-owned translation path ---
    // Flat current position: target=+10, current=0 → delta=+10 → buy 10.
    let result = live_result(vec![TargetPosition::new("AAPL", 10)]);
    let decisions = bar_result_to_decisions(&result, run_id, FIXED_NOW_MICROS, &flat());
    assert_eq!(decisions.len(), 1, "C14 precondition: one target → one decision");

    let decision = decisions.into_iter().next().unwrap();
    let decision_id = decision.decision_id.clone();

    // First submission: must be accepted.
    let outcome = submit_internal_strategy_decision(&st, decision.clone()).await;
    assert!(
        outcome.accepted,
        "C14: loop-path decision must be accepted; disposition={:?}, blockers={:?}",
        outcome.disposition,
        outcome.blockers
    );
    assert_eq!(outcome.disposition, "accepted", "C14: disposition must be 'accepted'");

    // Verify the outbox row carries the correct signal_source.
    let rows = sqlx::query_as::<_, (serde_json::Value,)>(
        "SELECT order_json FROM oms_outbox WHERE idempotency_key = $1 AND run_id = $2",
    )
    .bind(&decision_id)
    .bind(run_id)
    .fetch_all(&pool)
    .await
    .expect("C14: query outbox");

    assert_eq!(rows.len(), 1, "C14: exactly one outbox row for this decision_id");
    let order_json = &rows[0].0;
    assert_eq!(
        order_json["signal_source"], "internal_strategy_decision",
        "C14: outbox row must carry signal_source='internal_strategy_decision'; got: {}",
        order_json
    );
    assert_eq!(
        order_json["qty"], 10,
        "C14: order qty must be the delta (10), not a fabricated value"
    );

    // Idempotency: re-submitting the same decision_id must return "duplicate".
    let outcome2 = submit_internal_strategy_decision(&st, decision).await;
    assert!(!outcome2.accepted, "C14: duplicate must not be accepted");
    assert_eq!(
        outcome2.disposition, "duplicate",
        "C14: second submission of same decision_id → 'duplicate'"
    );

    // Cleanup.
    sqlx::query("DELETE FROM oms_outbox WHERE run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("C14: cleanup outbox");
    sqlx::query("DELETE FROM runs WHERE run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("C14: cleanup runs");
}
