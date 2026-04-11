//! B6: Native strategy capital budget gate — symmetry proof.
//!
//! Proves that `submit_internal_strategy_decision` (the internal strategy
//! decision seam) now enforces the same TV-04B capital budget gate that the
//! external signal path (`POST /api/v1/strategy/signal` Gate 1e) has always
//! enforced.
//!
//! Before B6: a strategy could be budget-denied for external signals yet still
//! have its bar-driven decisions reach the durable outbox via the internal path.
//! After B6: Gate 1e fires in `submit_internal_strategy_decision` before Gate 2
//! (DB), symmetrically with the external path.
//!
//! # Asymmetry closed
//!
//! | Gate         | External signal path | Internal decision path (pre-B6) | After B6 |
//! |--------------|----------------------|---------------------------------|----------|
//! | TV-04B (1e)  | enforced             | absent                          | enforced |
//!
//! # Test inventory
//!
//! | ID   | Condition                                          | Expected                                              |
//! |------|----------------------------------------------------|-------------------------------------------------------|
//! | G01  | No policy configured (env var absent)              | PolicyNotConfigured → passes Gate 1e → Gate 2 fires   |
//! | G02  | Policy present, strategy absent from budget list   | BudgetDenied → disposition="budget_denied", rejected  |
//! | G03  | Policy present, budget_authorized=false            | BudgetDenied → disposition="budget_denied", rejected  |
//! | G04  | Policy present, budget_authorized=true             | BudgetAuthorized → passes Gate 1e → Gate 2 fires      |
//! | G05  | Policy configured but file is invalid JSON         | PolicyInvalid → disposition="policy_invalid", rejected|
//!
//! All tests are pure in-process (no DB, no network).  The Gate 1e refusal is
//! proven by the disposition value; the pass-through is proven by Gate 2 firing
//! ("unavailable" from no DB configured in bare AppState).
//!
//! # Env-var serialisation
//!
//! `MQK_CAPITAL_POLICY_PATH` is a process-global env var.  All tests that
//! mutate it acquire `ENV_LOCK` so they serialise correctly under
//! `cargo test` default multi-thread runner.

use std::io::Write as _;
use std::sync::{Arc, OnceLock};

use tokio::sync::Mutex;

use mqk_daemon::decision::{submit_internal_strategy_decision, InternalStrategyDecision};
use mqk_daemon::state::{self, AppState};

// ---------------------------------------------------------------------------
// Env-var serialisation — protects MQK_CAPITAL_POLICY_PATH mutations
// ---------------------------------------------------------------------------

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a bare AppState with no DB (bare paper+alpaca mode).
async fn bare_state() -> Arc<AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ))
}

/// A minimal valid `InternalStrategyDecision` for strategy_id `sid`.
fn decision(sid: &str) -> InternalStrategyDecision {
    InternalStrategyDecision {
        decision_id: format!("b6-test-decision-{sid}"),
        strategy_id: sid.to_string(),
        symbol: "AAPL".to_string(),
        side: "buy".to_string(),
        qty: 10,
        order_type: "market".to_string(),
        time_in_force: "day".to_string(),
        limit_price: None,
    }
}

/// Write a valid `policy-v1` JSON file to a temp path.
/// Returns the path as a String (caller must keep the NamedTempFile alive).
fn write_policy(authorized_ids: &[&str], denied_ids: &[&str]) -> tempfile::NamedTempFile {
    let mut entries: Vec<serde_json::Value> = Vec::new();
    for id in authorized_ids {
        entries.push(serde_json::json!({
            "strategy_id": id,
            "budget_authorized": true,
            "risk_bucket": "equity_long_only"
        }));
    }
    for id in denied_ids {
        entries.push(serde_json::json!({
            "strategy_id": id,
            "budget_authorized": false,
            "deny_reason": "b6 test: budget not released"
        }));
    }
    let policy = serde_json::json!({
        "schema_version": "policy-v1",
        "policy_id": "b6-test-policy",
        "enabled": true,
        "max_portfolio_notional_usd": 100000,
        "per_strategy_budgets": entries
    });
    let mut f = tempfile::NamedTempFile::new().expect("create temp policy file");
    write!(f, "{}", policy).expect("write policy");
    f
}

// ---------------------------------------------------------------------------
// G01 — No policy configured → Gate 1e passes (PolicyNotConfigured)
// ---------------------------------------------------------------------------

/// G01: When `MQK_CAPITAL_POLICY_PATH` is absent, Gate 1e produces
/// `PolicyNotConfigured` which is signal-safe (no enforcement active).
/// The decision passes Gate 1e and reaches Gate 2 (DB unavailable → "unavailable").
///
/// Proves: absence of capital policy does not block internal decisions.
#[tokio::test]
async fn b6_g01_no_policy_passes_gate() {
    let _guard = env_lock().lock().await;
    std::env::remove_var("MQK_CAPITAL_POLICY_PATH");

    let st = bare_state().await;
    let outcome = submit_internal_strategy_decision(&st, decision("strat-no-policy")).await;

    assert_ne!(
        outcome.disposition, "budget_denied",
        "G01: absent policy must not produce budget_denied"
    );
    assert_ne!(
        outcome.disposition, "policy_invalid",
        "G01: absent policy must not produce policy_invalid"
    );
    // Gate 2 (no DB) fires → "unavailable"
    assert_eq!(
        outcome.disposition, "unavailable",
        "G01: no policy + no DB → Gate 1e passes → Gate 2 fires 'unavailable'; got: {:?}",
        outcome.disposition
    );
    assert!(!outcome.accepted, "G01: no DB → must not be accepted");
}

// ---------------------------------------------------------------------------
// G02 — Strategy absent from policy budget list → budget_denied
// ---------------------------------------------------------------------------

/// G02: Policy present with an entry for a *different* strategy.
/// The target strategy has no entry → `BudgetDenied` (absent entry ≠ authorized).
/// Gate 1e fires before Gate 2 (DB); disposition = "budget_denied".
///
/// Proves: the internal path can no longer bypass budget denial by simply
/// not appearing in the policy file.
#[tokio::test]
async fn b6_g02_strategy_absent_from_policy_is_budget_denied() {
    let _guard = env_lock().lock().await;

    // Policy only authorizes "other-strategy"; target is "strat-absent".
    let policy_file = write_policy(&["other-strategy"], &[]);
    let path = policy_file.path().to_string_lossy().to_string();
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = bare_state().await;
    let outcome = submit_internal_strategy_decision(&st, decision("strat-absent")).await;

    assert_eq!(
        outcome.disposition, "budget_denied",
        "G02: strategy absent from policy → Gate 1e must fire 'budget_denied'; got: {:?} blockers={:?}",
        outcome.disposition, outcome.blockers
    );
    assert!(!outcome.accepted, "G02: budget_denied must not be accepted");
    assert!(
        !outcome.blockers.is_empty(),
        "G02: budget_denied must carry a blocker message"
    );
    assert!(
        outcome.blockers[0].contains("internal decision refused"),
        "G02: blocker must say 'internal decision refused'; got: {}",
        outcome.blockers[0]
    );
}

// ---------------------------------------------------------------------------
// G03 — budget_authorized=false → budget_denied
// ---------------------------------------------------------------------------

/// G03: Policy present, strategy has an entry with `budget_authorized=false`.
/// Gate 1e fires; disposition = "budget_denied".
///
/// Proves: an explicit `budget_authorized=false` denial is enforced for
/// internal decisions, not just external signals.
#[tokio::test]
async fn b6_g03_explicit_budget_denied_blocks_internal_decision() {
    let _guard = env_lock().lock().await;

    // Policy explicitly denies "strat-denied".
    let policy_file = write_policy(&[], &["strat-denied"]);
    let path = policy_file.path().to_string_lossy().to_string();
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = bare_state().await;
    let outcome = submit_internal_strategy_decision(&st, decision("strat-denied")).await;

    assert_eq!(
        outcome.disposition, "budget_denied",
        "G03: budget_authorized=false → Gate 1e must fire 'budget_denied'; got: {:?} blockers={:?}",
        outcome.disposition, outcome.blockers
    );
    assert!(!outcome.accepted, "G03: budget_denied must not be accepted");
}

// ---------------------------------------------------------------------------
// G04 — budget_authorized=true → Gate 1e passes, Gate 2 fires
// ---------------------------------------------------------------------------

/// G04: Policy present, strategy has `budget_authorized=true`.
/// Gate 1e passes (BudgetAuthorized); Gate 2 (no DB) fires → "unavailable".
///
/// Proves: a valid authorized decision is not over-blocked by the new gate.
/// The hardened seam remains open for authorized strategies.
#[tokio::test]
async fn b6_g04_authorized_strategy_passes_gate() {
    let _guard = env_lock().lock().await;

    // Policy authorizes "strat-authorized".
    let policy_file = write_policy(&["strat-authorized"], &[]);
    let path = policy_file.path().to_string_lossy().to_string();
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = bare_state().await;
    let outcome = submit_internal_strategy_decision(&st, decision("strat-authorized")).await;

    assert_ne!(
        outcome.disposition, "budget_denied",
        "G04: authorized strategy must not be budget_denied"
    );
    assert_ne!(
        outcome.disposition, "policy_invalid",
        "G04: valid policy must not produce policy_invalid"
    );
    // Gate 1e passes → Gate 2 (no DB) fires → "unavailable"
    assert_eq!(
        outcome.disposition, "unavailable",
        "G04: authorized + no DB → Gate 1e passes → Gate 2 fires 'unavailable'; got: {:?}",
        outcome.disposition
    );
    assert!(!outcome.accepted, "G04: no DB → must not be accepted");
}

// ---------------------------------------------------------------------------
// G05 — Policy path set but file contains invalid JSON → policy_invalid
// ---------------------------------------------------------------------------

/// G05: `MQK_CAPITAL_POLICY_PATH` points to a file with invalid JSON.
/// Gate 1e fires; disposition = "policy_invalid".
///
/// Proves: a misconfigured policy fails closed for internal decisions, not
/// silently open.  The same fail-closed behaviour applies to both paths.
#[tokio::test]
async fn b6_g05_invalid_policy_file_blocks_with_policy_invalid() {
    let _guard = env_lock().lock().await;

    let mut f = tempfile::NamedTempFile::new().expect("create temp file");
    write!(f, "{{ not valid json !!!").expect("write bad json");
    let path = f.path().to_string_lossy().to_string();
    std::env::set_var("MQK_CAPITAL_POLICY_PATH", &path);

    let st = bare_state().await;
    let outcome = submit_internal_strategy_decision(&st, decision("strat-any")).await;

    assert_eq!(
        outcome.disposition, "policy_invalid",
        "G05: invalid policy JSON → Gate 1e must fire 'policy_invalid'; got: {:?} blockers={:?}",
        outcome.disposition, outcome.blockers
    );
    assert!(
        !outcome.accepted,
        "G05: policy_invalid must not be accepted"
    );
    assert!(
        !outcome.blockers.is_empty(),
        "G05: policy_invalid must carry a blocker message"
    );
}
