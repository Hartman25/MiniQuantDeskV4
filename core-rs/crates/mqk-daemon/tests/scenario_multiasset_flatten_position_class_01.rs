//! MULTIASSET-RUNTIME-INTEGRATION-01 / C3: a flatten close is an Equity-shaped
//! order on the Equity domain's run. A position the trading registry-v2
//! positively lists as non-Equity is refused with a reason instead of being
//! enqueued as a mislabelled Equity close.
//!
//! Before this invariant both flatten producers (`flatten-paper-positions` and
//! the pre-event flatten in the execution loop) wrote `{symbol, qty, side,
//! order_type}` with no `asset_class`. The dispatch parser reads that as
//! Equity, so a fractional Crypto position's close was accepted by the route
//! ("enqueued") and then quarantined at dispatch with "Equity quantity must be
//! a whole share count": the operator was told a liquidation was queued that
//! could never be submitted.
//!
//! | Test  | What it proves |
//! |-------|----------------|
//! | FL-01 | (pure) the Equity-shaped close of a fractional position cannot pass the dispatch parser |
//! | FL-02 | Operator flatten of {BTC/USD 0.5, AAPL 100}: AAPL enqueued, BTC/USD refused and reported, no BTC outbox row |
//! | FL-03 | Operator flatten of BTC/USD alone enqueues nothing and is not reported accepted |
//! | FL-04 | Control: a symbol the registry-v2 lists as Equity (or does not list) still flattens |
//! | FL-05 | Pre-event flatten enqueues nothing for a refused position and one row for an accepted one |
//!
//! DB-backed tests are `#[ignore]`: run with
//! `MQK_DATABASE_URL=postgres://.../mqk_test cargo test -p mqk-daemon \
//!  --test scenario_multiasset_flatten_position_class_01 -- --include-ignored --test-threads=1`

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use mqk_daemon::{
    pre_event_flatten::{
        build_operator_flatten_close_order_json, enqueue_pre_event_flatten_closes,
    },
    routes::build_router,
    state::{AppState, DeploymentMode, ExecutionDomain, OperatorAuthMode},
};
use mqk_execution::QtyMicros;
use mqk_md::instrument_registry_v2::{
    ContractDefinitionV2, InstrumentDefinitionV2, InstrumentEconomicsMetadataV2,
    InstrumentMetadataV2, InstrumentRegistryV2, SESSION_PROFILE_CRYPTO_24_7,
};
use tower::ServiceExt;
use uuid::Uuid;

static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn definition(symbol: &str, asset_class: &str) -> InstrumentDefinitionV2 {
    let crypto = asset_class == "crypto";
    InstrumentDefinitionV2 {
        instrument_id: format!("{asset_class}:GLOBAL:{}", symbol.replace('/', "")),
        symbol: symbol.to_string(),
        asset_class: asset_class.to_string(),
        instrument_kind: None,
        venue: Some("GLOBAL".to_string()),
        currency: "USD".to_string(),
        quote_currency: crypto.then(|| "USD".to_string()),
        provider_symbols: BTreeMap::new(),
        broker_symbols: if crypto {
            BTreeMap::from([("alpaca".to_string(), symbol.to_string())])
        } else {
            BTreeMap::new()
        },
        enabled: !crypto,
        paper_trading_enabled: true,
        live_trading_enabled: false,
        timeframes: vec!["5m".to_string()],
        contract: crypto.then(|| ContractDefinitionV2::CryptoPair {
            base: "BTC".to_string(),
            quote: "USD".to_string(),
        }),
        metadata: InstrumentMetadataV2::default(),
        notes: Some("flatten position-class proof fixture".to_string()),
        allow_enabled_non_equity_for_testing: false,
        economics: crypto.then(|| InstrumentEconomicsMetadataV2 {
            contract_multiplier: None,
            initial_margin_micros: None,
            maintenance_margin_micros: None,
            quantity_increment_micros: Some(100),
            min_trade_qty_micros: Some(100),
            price_tick_micros: Some(1_000_000),
            session_profile: Some(SESSION_PROFILE_CRYPTO_24_7.to_string()),
        }),
    }
}

fn registry_file(rows: Vec<InstrumentDefinitionV2>) -> String {
    let registry = InstrumentRegistryV2 {
        schema_version: 1,
        instruments: rows,
    };
    let dir = std::env::temp_dir().join(format!("mqk_fl01_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("fixture dir");
    let path = dir.join("trading_registry_v2.json");
    std::fs::write(&path, serde_json::to_vec(&registry).expect("serialize")).expect("write");
    path.to_string_lossy().into_owned()
}

async fn db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).expect("DB tests require MQK_DATABASE_URL");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to test DB");
    mqk_db::migrate(&pool).await.expect("run migrations");
    for stmt in [
        "DELETE FROM oms_outbox WHERE run_id IN (SELECT run_id FROM runs WHERE engine_id = 'mqk-daemon' AND mode = 'PAPER')",
        "DELETE FROM runs WHERE engine_id = 'mqk-daemon' AND mode = 'PAPER'",
        "DELETE FROM sys_arm_state WHERE sentinel_id = 1",
        "DELETE FROM sys_reconcile_status_state",
    ] {
        sqlx::query(stmt).execute(&pool).await.expect("reset");
    }
    pool
}

async fn state_with(pool: sqlx::PgPool, v2: Option<String>) -> Arc<AppState> {
    let mut st =
        AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    st.trading_instrument_registry_v2_path = v2;
    Arc::new(st)
}

async fn seed_run(pool: &sqlx::PgPool) -> Uuid {
    let run_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "fl01-test".to_string(),
            config_hash: "fl01-config".to_string(),
            config_json: serde_json::json!({"source": "scenario_multiasset_flatten_position_class_01"}),
            host_fingerprint: "fl01-host".to_string(),
        },
    )
    .await
    .expect("insert run");
    mqk_db::arm_run(pool, run_id).await.expect("arm run");
    mqk_db::begin_run(pool, run_id).await.expect("begin run");
    mqk_db::heartbeat_run(pool, run_id, now)
        .await
        .expect("heartbeat run");
    run_id
}

async fn arm_and_reconcile(pool: &sqlx::PgPool) {
    mqk_db::persist_arm_state_canonical(pool, mqk_db::ArmState::Armed, None)
        .await
        .expect("persist arm state");
    mqk_db::persist_reconcile_status_state(
        pool,
        &mqk_db::PersistReconcileStatusState {
            status: "ok",
            last_run_at_utc: Some(chrono::Utc::now()),
            snapshot_watermark_ms: None,
            mismatched_positions: 0,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: None,
            updated_at_utc: chrono::Utc::now(),
        },
    )
    .await
    .expect("persist reconcile ok");
}

async fn running_equity_run(st: &Arc<AppState>, positions: Vec<(&str, QtyMicros)>) -> Uuid {
    let pool = st.db.as_ref().expect("db");
    arm_and_reconcile(pool).await;
    let run_id = seed_run(pool).await;
    st.inject_running_loop_for_test(ExecutionDomain::EquityNyse, run_id)
        .await;
    let mut snap = st.execution_snapshot.write().await;
    *snap = Some(mqk_runtime::observability::ExecutionSnapshot {
        run_id: Some(run_id),
        active_orders: vec![],
        pending_outbox: vec![],
        recent_inbox_events: vec![],
        portfolio: mqk_runtime::observability::PortfolioSnapshot {
            cash_micros: 0,
            realized_pnl_micros: 0,
            positions: positions
                .into_iter()
                .map(
                    |(symbol, net_qty)| mqk_runtime::observability::PositionSnapshot {
                        symbol: symbol.to_string(),
                        net_qty,
                    },
                )
                .collect(),
        },
        system_block_state: None,
        recent_risk_denials: vec![],
        snapshot_at_utc: chrono::Utc::now(),
        has_recent_terminal_fill: false,
        risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
    });
    run_id
}

async fn flatten(st: &Arc<AppState>) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::json!({
                "action_key": "flatten-paper-positions",
                "reason": "fl01-test-flatten"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = build_router(Arc::clone(st)).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn outbox_symbols(pool: &sqlx::PgPool, run: Uuid) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT order_json->>'symbol' FROM oms_outbox WHERE run_id = $1 ORDER BY 1",
    )
    .bind(run)
    .fetch_all(pool)
    .await
    .expect("outbox symbols")
}

fn has_warning(json: &serde_json::Value, needle: &str) -> bool {
    json["warnings"]
        .as_array()
        .map(|w| w.iter().any(|v| v.as_str().unwrap_or("").contains(needle)))
        .unwrap_or(false)
}

/// FL-01
#[test]
fn fl01_equity_shaped_close_of_a_fractional_position_cannot_be_dispatched() {
    let (_, order) = build_operator_flatten_close_order_json(
        "BTC/USD",
        QtyMicros::new(500_000),
        1_700_000_000,
        Uuid::nil(),
    );
    let err = mqk_runtime::orchestrator::build_validated_submit_request("fl01", &order)
        .expect_err("a fractional close carrying no asset class is read as Equity and refused");
    assert!(err.to_string().contains("whole share"), "{err}");
}

/// FL-02
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fl02_operator_flatten_refuses_the_crypto_position_and_closes_the_equity_one() {
    let _guard = DB_LOCK.lock().await;
    let pool = db_pool().await;
    let st = state_with(
        pool.clone(),
        Some(registry_file(vec![definition("BTC/USD", "crypto")])),
    )
    .await;
    let run = running_equity_run(
        &st,
        vec![
            ("BTC/USD", QtyMicros::new(500_000)),
            ("AAPL", QtyMicros::from_whole_units(100).unwrap()),
        ],
    )
    .await;

    let (status, json) = flatten(&st).await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["accepted"], true, "{json}");
    assert_eq!(json["disposition"], "partial_enqueue_failed", "{json}");
    assert!(
        has_warning(&json, "unsupported_position: symbol=BTC/USD"),
        "the refusal must be reported to the operator: {json}"
    );
    assert_eq!(
        outbox_symbols(&pool, run).await,
        vec!["AAPL".to_string()],
        "only the Equity close may reach the outbox"
    );
}

/// FL-03
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fl03_operator_flatten_of_only_a_crypto_position_enqueues_nothing() {
    let _guard = DB_LOCK.lock().await;
    let pool = db_pool().await;
    let st = state_with(
        pool.clone(),
        Some(registry_file(vec![definition("BTC/USD", "crypto")])),
    )
    .await;
    let run = running_equity_run(&st, vec![("BTC/USD", QtyMicros::new(500_000))]).await;

    let (status, json) = flatten(&st).await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{json}");
    assert_eq!(json["accepted"], false, "{json}");
    assert_eq!(json["disposition"], "all_enqueue_failed", "{json}");
    assert!(outbox_symbols(&pool, run).await.is_empty());
}

/// FL-04
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fl04_equity_positions_still_flatten_with_or_without_a_listing() {
    let _guard = DB_LOCK.lock().await;
    let pool = db_pool().await;
    // AAPL is listed as an Equity; SPY is not listed at all; no v2 registry
    // availability is required for either.
    let st = state_with(
        pool.clone(),
        Some(registry_file(vec![definition("AAPL", "equity")])),
    )
    .await;
    let run = running_equity_run(
        &st,
        vec![
            ("AAPL", QtyMicros::from_whole_units(10).unwrap()),
            ("SPY", QtyMicros::from_whole_units(5).unwrap()),
        ],
    )
    .await;

    let (status, json) = flatten(&st).await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["disposition"], "enqueued", "{json}");
    assert_eq!(
        outbox_symbols(&pool, run).await,
        vec!["AAPL".to_string(), "SPY".to_string()]
    );
}

/// FL-05
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fl05_pre_event_flatten_skips_a_refused_position_only() {
    let _guard = DB_LOCK.lock().await;
    let pool = db_pool().await;
    // An unreadable blackout source makes the trigger Unavailable, which is
    // fail-closed flatten-required. Sole env writer in this test binary.
    std::env::set_var(
        mqk_daemon::event_risk_blackout::ENV_BLACKOUT_PATH,
        "/nonexistent/fl05-blackout.json",
    );
    std::env::remove_var(mqk_daemon::earnings_calendar::ENV_EARNINGS_CALENDAR_PATH);
    let run = seed_run(&pool).await;
    let positions = vec![
        ("BTC/USD".to_string(), QtyMicros::new(500_000)),
        ("AAPL".to_string(), QtyMicros::from_whole_units(10).unwrap()),
    ];

    // The production refusal rule, evaluated against a registry-v2 that lists
    // BTC/USD as crypto, drives the loop's closure.
    let st = state_with(
        pool.clone(),
        Some(registry_file(vec![definition("BTC/USD", "crypto")])),
    )
    .await;
    let refuse = |symbol: &str| mqk_daemon::decision::flatten_refusal_for_symbol(&st, symbol);
    let n =
        enqueue_pre_event_flatten_closes(DeploymentMode::Paper, &pool, run, &positions, &refuse)
            .await;

    assert_eq!(n, 1, "only the Equity close is enqueued");
    assert_eq!(outbox_symbols(&pool, run).await, vec!["AAPL".to_string()]);
}
