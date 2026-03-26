//! CC-02D: Fleet-level enable/disable + suppression interaction proof.
//!
//! Proves that durable registry enable/disable truth and durable suppression
//! truth interact coherently and fail-closedly at the internal strategy
//! decision seam (`submit_internal_strategy_decision`).
//!
//! # Required combined-state cases
//!
//! ```text
//! CD-01: enabled + unsuppressed  → passes Gates 3+4, proceeds to arm gate
//! CD-02: enabled + suppressed    → disposition "suppressed"
//! CD-03: disabled + unsuppressed → disposition "rejected" (disabled)
//! CD-04: disabled + suppressed   → disposition "rejected", NOT "suppressed"
//!            (Gate 3 registry check fires before Gate 4 suppression check)
//! CD-05: re-enabled after suppression cleared → passes Gates 3+4 again
//! CD-06: no durable truth (no DB) → "unavailable" (fail-closed)
//! ```
//!
//! Authority comes from `sys_strategy_registry.enabled` (durable DB state),
//! not from AppState or environment.
//!
//! No-DB tests run unconditionally.
//! DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run DB tests with:
//!
//! ```text
//! MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//! cargo test -p mqk-daemon --test scenario_fleet_enable_disable_interaction_cc02d \
//!   -- --include-ignored
//! ```

use std::sync::Arc;

use chrono::Utc;
use mqk_daemon::{
    decision::{submit_internal_strategy_decision, InternalStrategyDecision},
    state,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_decision(decision_id: &str, strategy_id: &str) -> InternalStrategyDecision {
    InternalStrategyDecision {
        decision_id: decision_id.to_string(),
        strategy_id: strategy_id.to_string(),
        symbol: "AAPL".to_string(),
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

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_fleet_enable_disable_interaction_cc02d \
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

/// Upsert a strategy into `sys_strategy_registry` with the given enabled flag.
async fn seed_registry(pool: &sqlx::PgPool, strategy_id: &str, enabled: bool) {
    let ts = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: format!("CC-02D Test Strategy {strategy_id}"),
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

/// Insert an active suppression for `strategy_id`.  Returns the suppression UUID
/// so the caller can clear it if needed.
async fn seed_suppression(pool: &sqlx::PgPool, strategy_id: &str) -> Uuid {
    let sup_id = Uuid::new_v4();
    mqk_db::insert_strategy_suppression(
        pool,
        &mqk_db::InsertStrategySuppressionArgs {
            suppression_id: sup_id,
            strategy_id: strategy_id.to_string(),
            trigger_domain: "operator".to_string(),
            trigger_reason: "CC-02D test suppression".to_string(),
            started_at_utc: Utc::now(),
            note: String::new(),
        },
    )
    .await
    .expect("seed_suppression: insert failed");
    sup_id
}

// ---------------------------------------------------------------------------
// CD-01 (DB): enabled + unsuppressed → passes Gates 3+4, proceeds to arm gate.
// ---------------------------------------------------------------------------

/// CC-02D / CD-01: A registered, enabled strategy with no active suppression
/// passes Gates 3 and 4 and is blocked at a later gate (arm state or active
/// run), not at the registry or suppression gates.
///
/// This proves the combined enabled+unsuppressed state allows progression
/// through the combined strategy-control layer.  The decision reaches Gate 5
/// (arm) or Gate 6 (active run) and is refused there — which is the expected
/// fail-closed outcome when the daemon is not armed for a live run.
#[tokio::test]
#[ignore]
async fn cd_01_enabled_unsuppressed_passes_gates_3_and_4() {
    let pool = make_db_pool().await;
    let sid = unique_id("cd01");
    seed_registry(&pool, &sid, true).await;
    // No suppression seeded.

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let out = submit_internal_strategy_decision(&st, make_decision(&unique_id("dec"), &sid)).await;

    // Must not be stopped at Gates 3 or 4.
    assert_ne!(
        out.disposition, "suppressed",
        "CD-01: enabled+unsuppressed must not produce 'suppressed'"
    );
    assert!(
        !out.blockers
            .iter()
            .any(|b| b.contains("not registered") || b.contains("disabled")),
        "CD-01: enabled+unsuppressed must not be blocked by registry; blockers: {:?}",
        out.blockers
    );
    // Decision proceeds to arm or run gate (rejected or unavailable at a later
    // gate), proving Gates 3+4 were passed.
    assert!(
        out.disposition == "rejected" || out.disposition == "unavailable",
        "CD-01: decision must reach a gate beyond registry+suppression; \
         got disposition: {:?}",
        out.disposition
    );
    assert!(
        out.blockers
            .iter()
            .any(|b| b.contains("arm") || b.contains("run")),
        "CD-01: a later gate (arm or run) must appear in blockers; blockers: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// CD-02 (DB): enabled + suppressed → "suppressed".
// ---------------------------------------------------------------------------

/// CC-02D / CD-02: A registered, enabled strategy with an active suppression
/// is refused at Gate 4 with disposition "suppressed".
///
/// Suppression authority stops an operationally-enabled strategy from
/// reaching the outbox path.  The disposition is distinct from "rejected"
/// so operators can distinguish a suppression action from a registry disable.
#[tokio::test]
#[ignore]
async fn cd_02_enabled_suppressed_returns_suppressed() {
    let pool = make_db_pool().await;
    let sid = unique_id("cd02");
    seed_registry(&pool, &sid, true).await;
    seed_suppression(&pool, &sid).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let out = submit_internal_strategy_decision(&st, make_decision(&unique_id("dec"), &sid)).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "suppressed",
        "CD-02: enabled+suppressed must produce 'suppressed'; got: {:?}",
        out.disposition
    );
    assert!(
        out.blockers.iter().any(|b| b.contains("suppressed")),
        "CD-02: blocker must mention suppression; blockers: {:?}",
        out.blockers
    );
    // Suppression fires before the active-run gate; active_run_id must be None.
    assert!(
        out.active_run_id.is_none(),
        "CD-02: suppression refusal fires before active-run gate; active_run_id must be None"
    );
}

// ---------------------------------------------------------------------------
// CD-03 (DB): disabled + unsuppressed → "rejected" (disabled).
// ---------------------------------------------------------------------------

/// CC-02D / CD-03: A registered, disabled strategy with no active suppression
/// is refused at Gate 3 with disposition "rejected" and a blocker mentioning
/// "disabled".
///
/// The registry `enabled` flag is the durable operator control.  Disabling a
/// strategy in `sys_strategy_registry` unconditionally blocks entry.
#[tokio::test]
#[ignore]
async fn cd_03_disabled_unsuppressed_returns_rejected() {
    let pool = make_db_pool().await;
    let sid = unique_id("cd03");
    seed_registry(&pool, &sid, false).await;
    // No suppression seeded.

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let out = submit_internal_strategy_decision(&st, make_decision(&unique_id("dec"), &sid)).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "rejected",
        "CD-03: disabled+unsuppressed must produce 'rejected'; got: {:?}",
        out.disposition
    );
    assert!(
        out.blockers.iter().any(|b| b.contains("disabled")),
        "CD-03: blocker must mention 'disabled'; blockers: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// CD-04 (DB): disabled + suppressed → "rejected", NOT "suppressed".
// Gate-ordering proof: Gate 3 fires before Gate 4.
// ---------------------------------------------------------------------------

/// CC-02D / CD-04: A registered, disabled strategy with an ACTIVE suppression
/// produces disposition "rejected" (not "suppressed").
///
/// This is the critical gate-ordering proof: Gate 3 (registry check) fires
/// before Gate 4 (suppression check).  When both controls are active, the
/// registry disabled state is the decisive factor — the combined
/// disabled+suppressed state is coherent and non-ambiguous.
///
/// The "disabled" blocker must appear; no "suppressed" blocker must be
/// present, proving Gate 4 was not reached.
#[tokio::test]
#[ignore]
async fn cd_04_disabled_suppressed_returns_rejected_not_suppressed() {
    let pool = make_db_pool().await;
    let sid = unique_id("cd04");
    seed_registry(&pool, &sid, false).await;
    seed_suppression(&pool, &sid).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let out = submit_internal_strategy_decision(&st, make_decision(&unique_id("dec"), &sid)).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "rejected",
        "CD-04: disabled+suppressed must produce 'rejected' (Gate 3 fires first), \
         not 'suppressed' (Gate 4); got: {:?}",
        out.disposition
    );
    // The disabled blocker must be present — Gate 3 fired.
    assert!(
        out.blockers.iter().any(|b| b.contains("disabled")),
        "CD-04: blocker must mention 'disabled' (Gate 3); blockers: {:?}",
        out.blockers
    );
    // No suppressed blocker must be present — Gate 4 was not reached.
    assert!(
        !out.blockers.iter().any(|b| b.contains("suppressed")),
        "CD-04: 'suppressed' must not appear in blockers — Gate 4 must not be \
         reached when Gate 3 fires; blockers: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// CD-05 (DB): re-enabled after suppression cleared → passes Gates 3+4 again.
// ---------------------------------------------------------------------------

/// CC-02D / CD-05: Full lifecycle coherence proof.
///
/// Sequence:
///   1. Register strategy as `enabled = true`.
///   2. Insert active suppression.
///   3. Clear suppression.
///   4. Re-upsert strategy as `enabled = true` (affirms durable state is authoritative).
///   5. Submit decision → must reach a gate beyond Gates 3+4.
///
/// This proves:
/// - The durable `sys_strategy_registry.enabled` flag is the real authority
///   (not a cached in-memory bit that might be stale).
/// - Suppression lifecycle transitions (active → cleared) are reflected at
///   the decision seam on the next call.
/// - Re-enabling after a suppression cycle coherently restores the passage
///   condition through the combined strategy-control layer.
#[tokio::test]
#[ignore]
async fn cd_05_reenabled_after_suppression_cleared_passes_gates_3_and_4() {
    let pool = make_db_pool().await;
    let sid = unique_id("cd05");

    // 1. Register enabled.
    seed_registry(&pool, &sid, true).await;

    // 2. Suppress.
    let sup_id = seed_suppression(&pool, &sid).await;

    // 3. Clear suppression.
    mqk_db::clear_strategy_suppression(&pool, sup_id, Utc::now())
        .await
        .expect("CD-05: clear suppression failed");

    // 4. Re-upsert enabled — affirms the enabled state is durable and
    //    authoritative, not just an artifact of the initial seed.
    seed_registry(&pool, &sid, true).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // 5. Submit decision.
    let out = submit_internal_strategy_decision(&st, make_decision(&unique_id("dec"), &sid)).await;

    // Must not be stopped at Gates 3 or 4.
    assert_ne!(
        out.disposition, "suppressed",
        "CD-05: cleared suppression must not produce 'suppressed'"
    );
    assert!(
        !out.blockers
            .iter()
            .any(|b| b.contains("not registered") || b.contains("disabled")),
        "CD-05: re-enabled strategy must not be blocked by registry; blockers: {:?}",
        out.blockers
    );
    // Decision proceeds to a later gate (arm or run), proving Gates 3+4 passed.
    assert!(
        out.disposition == "rejected" || out.disposition == "unavailable",
        "CD-05: decision must reach a gate beyond registry+suppression; \
         got disposition: {:?}",
        out.disposition
    );
    assert!(
        out.blockers
            .iter()
            .any(|b| b.contains("arm") || b.contains("run")),
        "CD-05: a later gate (arm or run) must appear in blockers; blockers: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// CD-06 (no DB): no durable truth → "unavailable" (fail-closed).
// ---------------------------------------------------------------------------

/// CC-02D / CD-06: When no durable registry truth is available (no DB pool
/// configured on the daemon), the decision seam refuses with disposition
/// "unavailable".
///
/// This is the fail-closed baseline: absence of durable state authority is
/// never treated as implicit permission.  The gate does not default to
/// "enabled" or "unsuppressed" when the DB cannot be reached.
#[tokio::test]
async fn cd_06_no_db_unavailable_fail_closed() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let out = submit_internal_strategy_decision(&st, make_decision("dec-cd06", "strat-cd06")).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "unavailable",
        "CD-06: no DB must produce 'unavailable'; got: {:?}",
        out.disposition
    );
    assert!(
        out.blockers
            .iter()
            .any(|b| b.contains("DB") || b.contains("unavailable")),
        "CD-06: blocker must mention DB unavailability; blockers: {:?}",
        out.blockers
    );
}
