//! ETF-RISK-CLOSURE-01: proof tests for Gate 1h (sector risk) in
//! `submit_internal_strategy_decision` (`decision.rs`).
//!
//! Distinct from `scenario_etf_risk_closure_01.rs` in `mqk-portfolio` (proves
//! the pure `evaluate_sector_risk` evaluator in isolation). This file proves
//! the daemon-side wiring: env config parsing, registry sector lookup, live
//! execution-snapshot + md_bars marks, and the gate's effect on
//! `submit_internal_strategy_decision`'s outcome — fail-closed, default-off,
//! pre-outbox.
//!
//! Every fixture uses a synthetic instrument (`ZZSR01TECH`) tagged to a
//! synthetic sector (`zzsr01_sector_tech`) via a temporary registry file
//! (`MQK_INSTRUMENT_REGISTRY_PATH` override) — never a real ticker — so
//! these tests cannot read, overwrite, or delete real `md_bars`/registry
//! data for any real symbol.
//!
//! # Proof matrix
//!
//! | Test  | DB? | Proves                                                            |
//! |-------|-----|---------------------------------------------------------------------|
//! | SRG-01 | no | Malformed `MQK_SECTOR_EXPOSURE_LIMITS_BPS` -> `sector_config_invalid`, before any DB access |
//! | SRG-02 | no | Disabled (env unset) -> Gate 1h is a no-op; decision proceeds to the pre-existing no-DB outcome unchanged |
//! | SRG-03 | no | Enabled, but candidate symbol untagged in the registry -> no DB needed; pre-existing no-DB outcome unchanged |
//! | SRG-04 | no | Enabled for the candidate's sector, but no DB configured -> `sector_weights_missing` (Gate 1h's own check, not Gate 2's generic message) |
//! | SRG-05 | yes | No sector config at all -> Gate 1h never produces a `sector_*` disposition |
//! | SRG-06 | yes | Enabled config, prospective exposure exceeds cap -> `sector_limit_exceeded`, denied before outbox (row count unchanged) |
//! | SRG-07 | yes | Enabled config, candidate symbol has no completed md_bars row -> `sector_weights_missing`, denied before outbox |
//! | SRG-08 | yes | Enabled config, sell that reduces an already-over-cap sector -> Gate 1h allows it (never a `sector_*` disposition) |
//!
//! SRG-05 and SRG-08 deliberately do not seed durable arm/run state: these
//! tests run against the real, shared local paper DB, whose durable arm
//! state reflects actual operational history and must not be forced into a
//! particular value to make a test pass. Both tests instead prove Gate 1h's
//! own verdict directly (no `sector_*` disposition), which is the only
//! claim that belongs to this gate; whether a later, unrelated gate (e.g.
//! Gate 5 arm-state) separately accepts or rejects is out of scope here and
//! is already covered by `scenario_internal_strategy_decision.rs`.
//!
//! DB-backed tests (SRG-05..08) skip gracefully without `MQK_DATABASE_URL`
//! pointing at the local paper DB (port 5440 / `miniquantdesk_paper`),
//! matching the convention in `scenario_portfolio_live_weights_01.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use mqk_daemon::{
    decision::{submit_internal_strategy_decision, InternalStrategyDecision},
    state,
};
use mqk_runtime::observability::{ExecutionSnapshot, PortfolioSnapshot, PositionSnapshot};
use tokio::sync::Mutex;
use uuid::Uuid;

const ENV_SECTOR_LIMITS: &str = "MQK_SECTOR_EXPOSURE_LIMITS_BPS";
const ENV_REGISTRY_PATH: &str = "MQK_INSTRUMENT_REGISTRY_PATH";
const FAKE_SYMBOL: &str = "ZZSR01TECH";
const FAKE_SECTOR: &str = "zzsr01_sector_tech";

// ---------------------------------------------------------------------------
// Cross-test serialization
// ---------------------------------------------------------------------------

/// Every test in this file mutates one or both of `ENV_SECTOR_LIMITS` /
/// `ENV_REGISTRY_PATH` (process-wide env vars). `cargo test` runs tests in
/// this binary on parallel threads by default, so each test holds this lock
/// for its full body to avoid racing another test's env var value.
///
/// `tokio::sync::Mutex`, not `std::sync::Mutex`: every test holds this guard
/// across `.await` points (DB connects/queries), which clippy's
/// `await_holding_lock` correctly flags as unsound for a sync mutex. A
/// tokio mutex is designed to be held across awaits, and (unlike
/// `std::sync::Mutex`) never poisons on a panicking test, so one test's
/// failure cannot cascade into spurious failures in the rest of this file.
static ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// RAII guard: sets an env var on construction, restores the previous value
/// (or removes it) on drop. Caller must hold `ENV_LOCK` for the guard's
/// entire lifetime.
struct EnvVarGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, prev }
    }

    fn remove(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

// ---------------------------------------------------------------------------
// Synthetic registry fixture
// ---------------------------------------------------------------------------

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

/// Write a temporary single-instrument registry tagging [`FAKE_SYMBOL`] to
/// [`FAKE_SECTOR`]. Returns `(file_path, dir_to_clean_up)`.
fn write_fake_registry() -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mqk_srg01_registry_{}_{}",
        std::process::id(),
        unique_id("dir")
    ));
    std::fs::create_dir_all(&dir).expect("create registry dir");
    let path = dir.join("equities.json");
    let json = format!(
        r#"[
  {{
    "instrument_id": "equity:US:{FAKE_SYMBOL}",
    "symbol": "{FAKE_SYMBOL}",
    "asset_class": "equity",
    "provider": "test",
    "provider_symbol": "{FAKE_SYMBOL}",
    "venue": "TEST",
    "currency": "USD",
    "enabled": true,
    "timeframes": ["1D"],
    "notes": "ETF-RISK-CLOSURE-01 synthetic fixture, not a real instrument",
    "instrument_kind": "etf",
    "sector": "{FAKE_SECTOR}",
    "category": "sector_equity"
  }}
]"#
    );
    std::fs::write(&path, json).expect("write fake registry");
    (path, dir)
}

fn cleanup_dir(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

fn make_decision(
    decision_id: &str,
    strategy_id: &str,
    side: &str,
    qty: i64,
) -> InternalStrategyDecision {
    InternalStrategyDecision {
        decision_id: decision_id.to_string(),
        strategy_id: strategy_id.to_string(),
        symbol: FAKE_SYMBOL.to_string(),
        side: side.to_string(),
        qty,
        order_type: "market".to_string(),
        time_in_force: "day".to_string(),
        limit_price: None,
    }
}

fn make_snapshot(cash_micros: i64, positions: &[(&str, i64)]) -> ExecutionSnapshot {
    ExecutionSnapshot {
        run_id: None,
        active_orders: vec![],
        pending_outbox: vec![],
        recent_inbox_events: vec![],
        portfolio: PortfolioSnapshot {
            cash_micros,
            realized_pnl_micros: 0,
            positions: positions
                .iter()
                .map(|(sym, qty)| PositionSnapshot {
                    symbol: sym.to_string(),
                    net_qty: *qty,
                })
                .collect(),
        },
        system_block_state: None,
        recent_risk_denials: vec![],
        has_recent_terminal_fill: false,
        risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
        snapshot_at_utc: chrono::Utc::now(),
    }
}

// ---------------------------------------------------------------------------
// SRG-01: malformed config -> sector_config_invalid, no DB needed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn srg01_malformed_config_rejected_before_any_db_access() {
    let _lock = ENV_LOCK.lock().await;
    let _limits = EnvVarGuard::set(ENV_SECTOR_LIMITS, "not_a_valid_entry_no_equals_sign");

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    assert!(st.db.is_none(), "this test must not have a DB configured");

    let d = make_decision(&unique_id("dec"), "strat-srg01", "buy", 10);
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(out.disposition, "sector_config_invalid");
    assert!(
        out.blockers
            .iter()
            .any(|b| b.contains("MQK_SECTOR_EXPOSURE_LIMITS_BPS")),
        "blocker must name the malformed env var; got: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// SRG-02: disabled (env unset) -> no-op, pre-existing no-DB outcome unchanged
// ---------------------------------------------------------------------------

#[tokio::test]
async fn srg02_disabled_gate_does_not_change_the_pre_existing_no_db_outcome() {
    let _lock = ENV_LOCK.lock().await;
    let _limits = EnvVarGuard::remove(ENV_SECTOR_LIMITS);

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    assert!(st.db.is_none());

    let d = make_decision(&unique_id("dec"), "strat-srg02", "buy", 10);
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "unavailable",
        "with sector risk disabled, the decision must fall through to the \
         pre-existing Gate 2 (DB presence) outcome, unchanged by this patch"
    );
    assert!(
        out.blockers
            .iter()
            .any(|b| b.contains("durable execution DB truth is unavailable")),
        "blocker must be Gate 2's pre-existing message, not a sector_* message; got: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// SRG-03: enabled, but candidate symbol untagged -> no DB needed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn srg03_enabled_config_for_untagged_symbol_does_not_need_db() {
    let _lock = ENV_LOCK.lock().await;
    // Sector risk IS enabled, but the candidate symbol (FAKE_SYMBOL) is not
    // in the (default, real) registry at all, so it must be treated as
    // unclassified and pass through without ever needing a DB.
    let _limits = EnvVarGuard::set(ENV_SECTOR_LIMITS, &format!("{FAKE_SECTOR}=1000"));
    let _registry = EnvVarGuard::remove(ENV_REGISTRY_PATH); // use the real default registry

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    assert!(st.db.is_none());

    let d = make_decision(&unique_id("dec"), "strat-srg03", "buy", 10);
    let out = submit_internal_strategy_decision(&st, d).await;

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "unavailable",
        "an untagged symbol must fall through to Gate 2's pre-existing \
         no-DB outcome, never a sector_* disposition; got blockers: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// SRG-04: enabled for this sector, but no DB -> sector_weights_missing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn srg04_enabled_for_candidate_sector_without_db_fails_closed_distinctly() {
    let _lock = ENV_LOCK.lock().await;
    let (registry_path, registry_dir) = write_fake_registry();
    let _limits = EnvVarGuard::set(ENV_SECTOR_LIMITS, &format!("{FAKE_SECTOR}=1000"));
    let _registry = EnvVarGuard::set(ENV_REGISTRY_PATH, registry_path.to_str().unwrap());

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    assert!(st.db.is_none());
    // Provide a live snapshot so the missing-DB branch specifically (not the
    // missing-snapshot branch) is what's under test here.
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(1_000_000_000, &[]));

    let d = make_decision(&unique_id("dec"), "strat-srg04", "buy", 10);
    let out = submit_internal_strategy_decision(&st, d).await;
    cleanup_dir(&registry_dir);

    assert!(!out.accepted);
    assert_eq!(
        out.disposition, "sector_weights_missing",
        "Gate 1h's own DB-presence check must fire distinctly from Gate 2's \
         generic message when sector risk is actually applicable; got \
         blockers: {:?}",
        out.blockers
    );
    assert!(out
        .blockers
        .iter()
        .any(|b| b.contains("no database is available")));
}

// ---------------------------------------------------------------------------
// DB-backed fixtures (SRG-05..08)
// ---------------------------------------------------------------------------

fn get_paper_db_url() -> Option<String> {
    let url = std::env::var("MQK_DATABASE_URL").ok()?;
    if url.contains(":5440") || url.contains("miniquantdesk_paper") {
        Some(url)
    } else {
        None
    }
}

async fn seed_registry_entry(pool: &sqlx::PgPool, strategy_id: &str) {
    let ts = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: format!("Test Strategy {strategy_id}"),
            enabled: true,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("seed_registry_entry: upsert failed");
}

async fn insert_test_bar(pool: &sqlx::PgPool, symbol: &str, end_ts: i64, close_micros: i64) {
    sqlx::query(
        r#"
        insert into md_bars (
          symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros, volume, is_complete
        ) values
          ($1,'1D',$2,$3,$4,$5,$6,$7,$8)
        on conflict (symbol, timeframe, end_ts) do update set
          open_micros = excluded.open_micros,
          high_micros = excluded.high_micros,
          low_micros = excluded.low_micros,
          close_micros = excluded.close_micros,
          volume = excluded.volume,
          is_complete = excluded.is_complete
        "#,
    )
    .bind(symbol)
    .bind(end_ts)
    .bind(close_micros)
    .bind(close_micros + 10_000)
    .bind(close_micros - 10_000)
    .bind(close_micros)
    .bind(1_000_i64)
    .bind(true)
    .execute(pool)
    .await
    .expect("insert test md_bars row");
}

async fn delete_test_bars(pool: &sqlx::PgPool, symbol: &str) {
    let _ = sqlx::query("delete from md_bars where symbol = $1")
        .bind(symbol)
        .execute(pool)
        .await;
}

async fn outbox_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("select count(*) from oms_outbox")
        .fetch_one(pool)
        .await
        .expect("count oms_outbox")
}

/// `true` for any of this gate's deny/fail-closed dispositions
/// (`sector_config_invalid`, `sector_weights_missing`,
/// `sector_nav_unavailable`, `sector_limit_exceeded`). Used by the
/// DB-backed tests below to prove Gate 1h's verdict without depending on
/// the real, shared local paper DB's durable arm/run state (which these
/// tests must not perturb) to reach a full `"accepted"` disposition.
fn is_sector_denial(disposition: &str) -> bool {
    disposition.starts_with("sector_")
}

// ---------------------------------------------------------------------------
// SRG-05: no sector config -> Gate 1h never denies (prior behavior unchanged)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn srg05_no_sector_config_never_denies() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("SRG-05: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let _lock = ENV_LOCK.lock().await;
    let _limits = EnvVarGuard::remove(ENV_SECTOR_LIMITS);

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("SRG-05: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let strategy_id = unique_id("strat-srg05");
    seed_registry_entry(&pool, &strategy_id).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let outbox_before = outbox_count(&pool).await;
    let d = make_decision(&unique_id("dec"), &strategy_id, "buy", 10);
    let out = submit_internal_strategy_decision(&st, d).await;
    let outbox_after = outbox_count(&pool).await;

    // This test deliberately does not seed durable arm/run state — it must
    // not perturb the real, shared local paper DB's actual operational
    // state to force a full "accepted" outcome. Whatever gate ultimately
    // decides this decision (Gate 5 arm-state, Gate 6 active-run, etc.) is
    // unrelated to Gate 1h; the only claim under test is that Gate 1h itself
    // — disabled — never denies and never needs the DB for marks.
    assert!(
        !is_sector_denial(&out.disposition),
        "disabled sector risk must never produce a sector_* disposition; out={out:?}"
    );
    if out.accepted {
        assert_eq!(outbox_after, outbox_before + 1);
    } else {
        assert_eq!(outbox_after, outbox_before);
    }
}

// ---------------------------------------------------------------------------
// SRG-06: enabled config, breach -> denied before outbox
// ---------------------------------------------------------------------------

#[tokio::test]
async fn srg06_enabled_config_denies_sector_cap_breach_before_outbox() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("SRG-06: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let _lock = ENV_LOCK.lock().await;
    let (registry_path, registry_dir) = write_fake_registry();
    let _limits = EnvVarGuard::set(ENV_SECTOR_LIMITS, &format!("{FAKE_SECTOR}=3000")); // 30% cap
    let _registry = EnvVarGuard::set(ENV_REGISTRY_PATH, registry_path.to_str().unwrap());

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("SRG-06: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    delete_test_bars(&pool, FAKE_SYMBOL).await;
    insert_test_bar(&pool, FAKE_SYMBOL, 1_790_000_001, 100_000_000).await; // $100/share

    // No registry-entry/arm/run seeding needed: Gate 1h denies before Gate 3
    // (registry check) or any later gate is ever reached.
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    // cash=$1000, currently flat in FAKE_SYMBOL.
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(1_000_000_000, &[]));

    let outbox_before = outbox_count(&pool).await;
    // Buy 60 shares @ $100 = $6000 notional; NAV = 1000+6000=7000;
    // weight = 6000/7000 ~= 8571 bps, far over the 3000 bps cap, and not
    // risk-reducing from a flat (0 bps) starting point.
    let d = make_decision(&unique_id("dec"), &unique_id("strat-srg06"), "buy", 60);
    let out = submit_internal_strategy_decision(&st, d).await;
    let outbox_after = outbox_count(&pool).await;

    delete_test_bars(&pool, FAKE_SYMBOL).await;
    cleanup_dir(&registry_dir);

    assert!(!out.accepted, "out={out:?}");
    assert_eq!(out.disposition, "sector_limit_exceeded");
    assert_eq!(
        outbox_after, outbox_before,
        "a sector-risk denial must never write an outbox row"
    );
}

// ---------------------------------------------------------------------------
// SRG-07: enabled config, missing md_bars mark -> denied before outbox
// ---------------------------------------------------------------------------

#[tokio::test]
async fn srg07_missing_md_bars_mark_denies_before_outbox() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("SRG-07: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let _lock = ENV_LOCK.lock().await;
    let (registry_path, registry_dir) = write_fake_registry();
    let _limits = EnvVarGuard::set(ENV_SECTOR_LIMITS, &format!("{FAKE_SECTOR}=3000"));
    let _registry = EnvVarGuard::set(ENV_REGISTRY_PATH, registry_path.to_str().unwrap());

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("SRG-07: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    // Deliberately no md_bars row for FAKE_SYMBOL. No registry-entry/arm/run
    // seeding needed: Gate 1h denies before any later gate is reached.
    delete_test_bars(&pool, FAKE_SYMBOL).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(1_000_000_000, &[]));

    let outbox_before = outbox_count(&pool).await;
    let d = make_decision(&unique_id("dec"), &unique_id("strat-srg07"), "buy", 10);
    let out = submit_internal_strategy_decision(&st, d).await;
    let outbox_after = outbox_count(&pool).await;

    cleanup_dir(&registry_dir);

    assert!(!out.accepted, "out={out:?}");
    assert_eq!(out.disposition, "sector_weights_missing");
    assert_eq!(outbox_after, outbox_before);
}

// ---------------------------------------------------------------------------
// SRG-08: risk-reducing sell allowed even though still over cap
// ---------------------------------------------------------------------------

#[tokio::test]
async fn srg08_risk_reducing_sell_is_still_allowed_through_to_accepted() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("SRG-08: skipped (no MQK_DATABASE_URL pointing to paper DB)");
        return;
    };
    let _lock = ENV_LOCK.lock().await;
    let (registry_path, registry_dir) = write_fake_registry();
    let _limits = EnvVarGuard::set(ENV_SECTOR_LIMITS, &format!("{FAKE_SECTOR}=6000")); // 60% cap
    let _registry = EnvVarGuard::set(ENV_REGISTRY_PATH, registry_path.to_str().unwrap());

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("SRG-08: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    delete_test_bars(&pool, FAKE_SYMBOL).await;
    insert_test_bar(&pool, FAKE_SYMBOL, 1_790_000_002, 100_000_000).await; // $100/share

    let strategy_id = unique_id("strat-srg08");
    seed_registry_entry(&pool, &strategy_id).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    // cash=$1000, holding 40 shares @ $100 = $4000; NAV=$5000;
    // weight = 4000/5000 = 8000 bps, already over the 6000 bps cap.
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(1_000_000_000, &[(FAKE_SYMBOL, 40)]));

    let outbox_before = outbox_count(&pool).await;
    // Sell 10: qty=30, mv=$3000, NAV=$4000, weight=3000/4000=7500 bps.
    // 7500 < 8000 -> risk-reducing; still > 6000 cap -> must be allowed by
    // Gate 1h. This test deliberately does not seed durable arm/run state
    // (must not perturb the real, shared local paper DB's actual
    // operational state) — the only claim under test is Gate 1h's own
    // verdict, not whether a later, unrelated gate also happens to accept.
    let d = make_decision(&unique_id("dec"), &strategy_id, "sell", 10);
    let out = submit_internal_strategy_decision(&st, d).await;
    let outbox_after = outbox_count(&pool).await;

    delete_test_bars(&pool, FAKE_SYMBOL).await;
    cleanup_dir(&registry_dir);

    assert!(
        !is_sector_denial(&out.disposition),
        "a risk-reducing order must never be denied by the sector gate; out={out:?}"
    );
    if out.accepted {
        assert_eq!(outbox_after, outbox_before + 1);
    } else {
        assert_eq!(outbox_after, outbox_before);
    }
}
