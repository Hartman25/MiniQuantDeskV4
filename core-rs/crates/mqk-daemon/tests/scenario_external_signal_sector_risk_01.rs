//! ETF-RISK-EXTERNAL-SIGNAL-GATE-01: proof tests for the sector-risk gate on
//! the external HTTP signal path (`POST /api/v1/strategy/signal`, Gate 1i in
//! `routes/strategy.rs`).
//!
//! Distinct from `scenario_sector_risk_gate_etf_risk_closure_01.rs` (proves
//! the same mechanics on the internal decision path, `decision.rs`'s Gate
//! 1h) and `scenario_etf_risk_closure_01.rs` in `mqk-portfolio` (proves the
//! pure `evaluate_sector_risk` evaluator in isolation). Both gates call the
//! same shared helper, `capital_policy::sector_risk_gate::evaluate_sector_risk_gate`
//! — this file proves the external HTTP wiring around that shared helper:
//! env config parsing, registry sector lookup, live execution-snapshot +
//! md_bars marks, and the gate's effect on the HTTP response — fail-closed,
//! default-off, pre-outbox.
//!
//! Every fixture uses a synthetic instrument (`ZZEXT01TECH`) tagged to a
//! synthetic sector (`zzext01_sector_tech`) via a temporary registry file
//! (`MQK_INSTRUMENT_REGISTRY_PATH` override) — never a real ticker, and a
//! symbol distinct from the internal-path test file's fixture so the two
//! test binaries (separate OS processes) cannot race on the same `md_bars`
//! rows if `cargo test` runs them concurrently.
//!
//! Every test in this file fires before WS continuity (Gate 1b) is ever
//! consulted: `AppState::new_for_test_with_mode_and_broker(Paper, Alpaca)`
//! starts WS continuity at `ColdStartUnproven`, which always 503s at Gate
//! 1b. A request that is NOT denied by the sector gate therefore always
//! reaches Gate 1b and 503s there — this is the "proceeds past the sector
//! gate unchanged" proof used throughout, identical in spirit to the
//! existing TV-04E (portfolio risk) gate tests in
//! `scenario_capital_policy_tv04e.rs`. No durable arm/run state is ever
//! seeded or required, because every case here resolves before Gate 2
//! (db_present) / Gate 3 (arm) are reached.
//!
//! # Proof matrix
//!
//! | Test  | DB? | Proves                                                                  |
//! |-------|-----|--------------------------------------------------------------------------|
//! | ES-01 | no  | No config -> existing external signal behavior unchanged; no DB touched  |
//! | ES-02 | yes | Enabled config, breach -> 403 `sector_limit_exceeded`, outbox unchanged  |
//! | ES-04 | yes | Enabled config, no md_bars row -> 503 `sector_weights_missing`           |
//! | ES-05 | yes | Enabled config, no execution snapshot -> 503 `sector_weights_missing`    |
//! | ES-06 | no  | Malformed config -> 503 `sector_config_invalid`, no DB touched           |
//! | ES-07 | yes | Risk-reducing sell allowed even though sector remains over cap           |
//! | ES-08 | no  | Untagged symbol, enabled config -> unchanged behavior; no DB touched      |
//! | ES-09 | yes | Enabled config, order within cap -> allowed                              |
//! | ES-10 | yes | Negative-NAV edge case -> 503 `sector_nav_unavailable`                   |
//!
//! DB-backed tests (ES-02/04/05/07/09/10) skip gracefully without
//! `MQK_DATABASE_URL` pointing at the local paper DB (port 5440 /
//! `miniquantdesk_paper`), matching the convention in
//! `scenario_sector_risk_gate_etf_risk_closure_01.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tokio::sync::Mutex;
use tower::ServiceExt;
use uuid::Uuid;

const ENV_SECTOR_LIMITS: &str = "MQK_SECTOR_EXPOSURE_LIMITS_BPS";
const ENV_REGISTRY_PATH: &str = "MQK_INSTRUMENT_REGISTRY_PATH";
const FAKE_SYMBOL: &str = "ZZEXT01TECH";
const FAKE_SECTOR: &str = "zzext01_sector_tech";

// ---------------------------------------------------------------------------
// Cross-test serialization (mirrors scenario_sector_risk_gate_etf_risk_closure_01.rs)
// ---------------------------------------------------------------------------

/// `tokio::sync::Mutex`, not `std::sync::Mutex`: tests hold this guard across
/// `.await` points (DB connects/queries), which clippy's `await_holding_lock`
/// correctly flags as unsound for a sync mutex.
static ENV_LOCK: Mutex<()> = Mutex::const_new(());

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

fn write_fake_registry() -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mqk_es01_registry_{}_{}",
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
    "notes": "ETF-RISK-EXTERNAL-SIGNAL-GATE-01 synthetic fixture, not a real instrument",
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

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body is not valid JSON");
    (status, json)
}

fn blockers_str(json: &serde_json::Value) -> String {
    json.get("blockers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default()
}

fn disposition_of(json: &serde_json::Value) -> &str {
    json.get("disposition")
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// `true` for any of this gate's deny/fail-closed dispositions.
fn is_sector_denial(disposition: &str) -> bool {
    disposition.starts_with("sector_")
}

async fn post_signal(
    st: Arc<state::AppState>,
    strategy_id: &str,
    side: &str,
    qty: i64,
) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({
        "signal_id": unique_id("sig-es01"),
        "strategy_id": strategy_id,
        "symbol": FAKE_SYMBOL,
        "side": side,
        "qty": qty,
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    call(routes::build_router(st), req).await
}

/// Build a Paper+Alpaca AppState (ExternalSignalIngestion wired, Gate 1
/// passes; WS continuity starts `ColdStartUnproven`, which always 503s at
/// Gate 1b for any request the sector gate itself does not deny).
fn paper_alpaca_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ))
}

fn make_snapshot(
    cash_micros: i64,
    positions: &[(&str, i64)],
) -> mqk_runtime::observability::ExecutionSnapshot {
    mqk_runtime::observability::ExecutionSnapshot {
        run_id: None,
        active_orders: vec![],
        pending_outbox: vec![],
        recent_inbox_events: vec![],
        portfolio: mqk_runtime::observability::PortfolioSnapshot {
            cash_micros,
            realized_pnl_micros: 0,
            positions: positions
                .iter()
                .map(|(sym, qty)| mqk_runtime::observability::PositionSnapshot {
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
// ES-01: no config -> existing external signal behavior unchanged, no DB
// ---------------------------------------------------------------------------

#[tokio::test]
async fn es01_no_config_preserves_existing_behavior_without_db() {
    let _lock = ENV_LOCK.lock().await;
    let _limits = EnvVarGuard::remove(ENV_SECTOR_LIMITS);

    let st = paper_alpaca_state();
    assert!(st.db.is_none(), "this test must not have a DB configured");

    let (status, json) = post_signal(st, "strat-es01", "buy", 10).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let disposition = disposition_of(&json);
    assert!(
        !is_sector_denial(disposition),
        "disabled sector risk must never produce a sector_* disposition; got: {json}"
    );
    let blockers = blockers_str(&json).to_lowercase();
    assert!(
        blockers.contains("continuity")
            || blockers.contains("ws")
            || blockers.contains("cold start"),
        "with sector risk disabled, the next gate to fire must be Gate 1b (WS continuity); \
         got blockers: {blockers}"
    );
}

// ---------------------------------------------------------------------------
// ES-02: enabled config, breach -> 403 sector_limit_exceeded, before outbox
// ---------------------------------------------------------------------------

fn get_paper_db_url() -> Option<String> {
    let url = std::env::var("MQK_DATABASE_URL").ok()?;
    if url.contains(":5440") || url.contains("miniquantdesk_paper") {
        Some(url)
    } else {
        None
    }
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

#[tokio::test]
async fn es02_enabled_config_denies_sector_cap_breach_before_outbox() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("ES-02: skipped (no MQK_DATABASE_URL pointing to paper DB)");
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
        .expect("ES-02: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    delete_test_bars(&pool, FAKE_SYMBOL).await;
    insert_test_bar(&pool, FAKE_SYMBOL, 1_790_100_001, 100_000_000).await; // $100/share

    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
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
    let (status, json) = post_signal(st, &unique_id("strat-es02"), "buy", 60).await;
    let outbox_after = outbox_count(&pool).await;

    delete_test_bars(&pool, FAKE_SYMBOL).await;
    cleanup_dir(&registry_dir);

    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    assert_eq!(disposition_of(&json), "sector_limit_exceeded");
    assert_eq!(
        outbox_after, outbox_before,
        "a sector-risk denial must never write an outbox row"
    );
}

// ---------------------------------------------------------------------------
// ES-04: enabled config, no md_bars row -> 503 sector_weights_missing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn es04_missing_md_bars_mark_denies_before_outbox() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("ES-04: skipped (no MQK_DATABASE_URL pointing to paper DB)");
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
        .expect("ES-04: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    // Deliberately no md_bars row for FAKE_SYMBOL.
    delete_test_bars(&pool, FAKE_SYMBOL).await;

    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(1_000_000_000, &[]));

    let outbox_before = outbox_count(&pool).await;
    let (status, json) = post_signal(st, &unique_id("strat-es04"), "buy", 10).await;
    let outbox_after = outbox_count(&pool).await;

    cleanup_dir(&registry_dir);

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    assert_eq!(disposition_of(&json), "sector_weights_missing");
    assert_eq!(outbox_after, outbox_before);
}

// ---------------------------------------------------------------------------
// ES-05: enabled config, no execution snapshot -> 503 sector_weights_missing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn es05_missing_execution_snapshot_denies_before_outbox() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("ES-05: skipped (no MQK_DATABASE_URL pointing to paper DB)");
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
        .expect("ES-05: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    // Deliberately do not seed execution_snapshot — it stays `None`.
    assert!(st.execution_snapshot.read().await.is_none());

    let outbox_before = outbox_count(&pool).await;
    let (status, json) = post_signal(st, &unique_id("strat-es05"), "buy", 10).await;
    let outbox_after = outbox_count(&pool).await;

    cleanup_dir(&registry_dir);

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    assert_eq!(disposition_of(&json), "sector_weights_missing");
    assert!(
        blockers_str(&json).contains("execution snapshot"),
        "blocker must name the missing execution snapshot distinctly; got: {json}"
    );
    assert_eq!(outbox_after, outbox_before);
}

// ---------------------------------------------------------------------------
// ES-06: malformed config -> 503 sector_config_invalid, no DB touched
// ---------------------------------------------------------------------------

#[tokio::test]
async fn es06_malformed_config_denies_before_outbox_without_db() {
    let _lock = ENV_LOCK.lock().await;
    let _limits = EnvVarGuard::set(ENV_SECTOR_LIMITS, "not_a_valid_entry_no_equals_sign");

    let st = paper_alpaca_state();
    assert!(st.db.is_none(), "this test must not have a DB configured");

    let (status, json) = post_signal(st, "strat-es06", "buy", 10).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    assert_eq!(disposition_of(&json), "sector_config_invalid");
    assert!(
        blockers_str(&json).contains("MQK_SECTOR_EXPOSURE_LIMITS_BPS"),
        "blocker must name the malformed env var; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// ES-07: risk-reducing sell allowed even though sector remains over cap
// ---------------------------------------------------------------------------

#[tokio::test]
async fn es07_risk_reducing_sell_is_allowed() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("ES-07: skipped (no MQK_DATABASE_URL pointing to paper DB)");
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
        .expect("ES-07: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    delete_test_bars(&pool, FAKE_SYMBOL).await;
    insert_test_bar(&pool, FAKE_SYMBOL, 1_790_100_002, 100_000_000).await; // $100/share

    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    // cash=$1000, holding 40 shares @ $100 = $4000; NAV=$5000;
    // weight = 4000/5000 = 8000 bps, already over the 6000 bps cap.
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(1_000_000_000, &[(FAKE_SYMBOL, 40)]));

    let outbox_before = outbox_count(&pool).await;
    // Sell 10: qty=30, mv=$3000, NAV=$4000, weight=3000/4000=7500 bps.
    // 7500 < 8000 -> risk-reducing; still > 6000 cap -> must be allowed.
    let (status, json) = post_signal(st, &unique_id("strat-es07"), "sell", 10).await;
    let outbox_after = outbox_count(&pool).await;

    delete_test_bars(&pool, FAKE_SYMBOL).await;
    cleanup_dir(&registry_dir);

    let disposition = disposition_of(&json);
    assert!(
        !is_sector_denial(disposition),
        "a risk-reducing order must never be denied by the sector gate; got: {json}"
    );
    // Allowed by the sector gate -> falls through to Gate 1b (WS continuity, 503).
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let blockers = blockers_str(&json).to_lowercase();
    assert!(
        blockers.contains("continuity")
            || blockers.contains("ws")
            || blockers.contains("cold start"),
        "got blockers: {blockers}"
    );
    assert_eq!(outbox_after, outbox_before);
}

// ---------------------------------------------------------------------------
// ES-08: untagged symbol, enabled config -> unchanged behavior, no DB
// ---------------------------------------------------------------------------

#[tokio::test]
async fn es08_untagged_symbol_preserves_existing_behavior_without_db() {
    let _lock = ENV_LOCK.lock().await;
    // Sector risk IS enabled, but the candidate symbol is not in the (real,
    // default) registry at all, so it must be treated as unclassified and
    // pass through without ever needing a DB.
    let _limits = EnvVarGuard::set(ENV_SECTOR_LIMITS, &format!("{FAKE_SECTOR}=1000"));
    let _registry = EnvVarGuard::remove(ENV_REGISTRY_PATH); // use the real default registry

    let st = paper_alpaca_state();
    assert!(st.db.is_none(), "this test must not have a DB configured");

    let (status, json) = post_signal(st, "strat-es08", "buy", 10).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let disposition = disposition_of(&json);
    assert!(
        !is_sector_denial(disposition),
        "an untagged symbol must never produce a sector_* disposition; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// ES-09: enabled config, order within cap -> allowed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn es09_enabled_config_allows_order_within_cap() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("ES-09: skipped (no MQK_DATABASE_URL pointing to paper DB)");
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
        .expect("ES-09: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    delete_test_bars(&pool, FAKE_SYMBOL).await;
    insert_test_bar(&pool, FAKE_SYMBOL, 1_790_100_003, 100_000_000).await; // $100/share

    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    // cash=$10,000, flat. Buy 5 @ $100 = $500; NAV=$10,500;
    // weight = 500/10500 ~= 476 bps, well under the 3000 bps cap.
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(10_000_000_000, &[]));

    let outbox_before = outbox_count(&pool).await;
    let (status, json) = post_signal(st, &unique_id("strat-es09"), "buy", 5).await;
    let outbox_after = outbox_count(&pool).await;

    delete_test_bars(&pool, FAKE_SYMBOL).await;
    cleanup_dir(&registry_dir);

    let disposition = disposition_of(&json);
    assert!(
        !is_sector_denial(disposition),
        "an order within the configured cap must never be denied by the sector gate; got: {json}"
    );
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    let blockers = blockers_str(&json).to_lowercase();
    assert!(
        blockers.contains("continuity")
            || blockers.contains("ws")
            || blockers.contains("cold start"),
        "got blockers: {blockers}"
    );
    assert_eq!(outbox_after, outbox_before);
}

// ---------------------------------------------------------------------------
// ES-10: negative-NAV edge case -> 503 sector_nav_unavailable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn es10_negative_nav_denies_before_outbox() {
    let Some(db_url) = get_paper_db_url() else {
        eprintln!("ES-10: skipped (no MQK_DATABASE_URL pointing to paper DB)");
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
        .expect("ES-10: pool connect failed");
    mqk_db::migrate(&pool).await.expect("migrate failed");

    // No md_bars row needed: with cash negative and zero current positions,
    // the *current* weights snapshot already has NAV <= 0 before any mark
    // is ever consulted (compute_portfolio_weights checks missing-marks on
    // an empty position list trivially, then nav <= 0 fires first).
    delete_test_bars(&pool, FAKE_SYMBOL).await;

    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    // cash=-$100, flat. NAV = cash = negative -> nav_unavailable.
    st.execution_snapshot
        .write()
        .await
        .replace(make_snapshot(-100_000_000, &[]));

    let outbox_before = outbox_count(&pool).await;
    let (status, json) = post_signal(st, &unique_id("strat-es10"), "buy", 1).await;
    let outbox_after = outbox_count(&pool).await;

    cleanup_dir(&registry_dir);

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    assert_eq!(disposition_of(&json), "sector_nav_unavailable");
    assert_eq!(outbox_after, outbox_before);
}
