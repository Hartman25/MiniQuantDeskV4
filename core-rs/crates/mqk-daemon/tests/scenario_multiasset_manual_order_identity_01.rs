//! MULTIASSET-RUNTIME-INTEGRATION-01 / C2: the manual operator order carries no
//! asset class, so it is admitted only for a symbol the registry authority
//! proves to be an Equity. An OCC option or crypto pair typed here must never
//! be written to the outbox as an implied Equity (the same hole the external
//! signal route closed with Gate 0b).
//!
//! | Test | What it proves |
//! |------|----------------|
//! | MO-01 | A registry-v2 Crypto symbol is rejected (400) and leaves no outbox row |
//! | MO-02 | An OCC option contract symbol no registry proves is rejected (400), no row |
//! | MO-03 | A registry-proven Equity is still enqueued, with the unchanged Equity shape |
//! | MO-04 | An unreadable registry is unavailable authority (503), never an assumed Equity |
//!
//! DB-backed (`#[ignore]`): run with
//! `MQK_DATABASE_URL=postgres://.../mqk_test cargo test -p mqk-daemon \
//!  --test scenario_multiasset_manual_order_identity_01 -- --include-ignored`

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    routes,
    state::{self, BrokerKind, DeploymentMode, ExecutionDomain},
};
use mqk_md::instrument_registry_v2::{
    ContractDefinitionV2, InstrumentDefinitionV2, InstrumentEconomicsMetadataV2,
    InstrumentMetadataV2, InstrumentRegistryV2, SESSION_PROFILE_CRYPTO_24_7,
};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn btc_registry_file() -> String {
    let registry = InstrumentRegistryV2 {
        schema_version: 1,
        instruments: vec![InstrumentDefinitionV2 {
            instrument_id: "crypto:GLOBAL:BTCUSD".to_string(),
            symbol: "BTC/USD".to_string(),
            asset_class: "crypto".to_string(),
            instrument_kind: None,
            venue: Some("GLOBAL".to_string()),
            currency: "USD".to_string(),
            quote_currency: Some("USD".to_string()),
            provider_symbols: BTreeMap::new(),
            broker_symbols: BTreeMap::from([("alpaca".to_string(), "BTC/USD".to_string())]),
            enabled: false,
            paper_trading_enabled: true,
            live_trading_enabled: false,
            timeframes: vec!["5m".to_string()],
            contract: Some(ContractDefinitionV2::CryptoPair {
                base: "BTC".to_string(),
                quote: "USD".to_string(),
            }),
            metadata: InstrumentMetadataV2::default(),
            notes: Some("manual-order identity proof fixture".to_string()),
            allow_enabled_non_equity_for_testing: false,
            economics: Some(InstrumentEconomicsMetadataV2 {
                contract_multiplier: None,
                initial_margin_micros: None,
                maintenance_margin_micros: None,
                quantity_increment_micros: Some(100),
                min_trade_qty_micros: Some(100),
                price_tick_micros: Some(1_000_000),
                session_profile: Some(SESSION_PROFILE_CRYPTO_24_7.to_string()),
            }),
        }],
    };
    let dir = std::env::temp_dir().join(format!("mqk_mo01_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("fixture dir");
    let path = dir.join("trading_registry_v2.json");
    std::fs::write(&path, serde_json::to_vec(&registry).expect("serialize")).expect("write");
    path.to_string_lossy().into_owned()
}

async fn db_pool() -> PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).expect("DB tests require MQK_DATABASE_URL");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to test DB");
    mqk_db::migrate(&pool).await.expect("run migrations");
    pool
}

struct Harness {
    pool: PgPool,
    st: Arc<state::AppState>,
    run: Uuid,
}

async fn harness(configure: impl FnOnce(&mut state::AppState)) -> Harness {
    let pool = db_pool().await;
    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("reset arm state");
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("arm");
    let mut st = common::with_canonical_equity_registry(
        state::AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ),
    );
    configure(&mut st);
    let st = Arc::new(st);
    let run = Uuid::new_v4();
    st.establish_db_backed_active_run_for_test(ExecutionDomain::EquityNyse, run)
        .await
        .expect("equity run");
    Harness { pool, st, run }
}

async fn post_manual_order(
    st: &Arc<state::AppState>,
    client_request_id: &str,
    symbol: &str,
) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({
        "client_request_id": client_request_id,
        "symbol": symbol,
        "side": "buy",
        "qty": 1,
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/execution/orders")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = routes::build_router(Arc::clone(st))
        .oneshot(req)
        .await
        .expect("oneshot");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json body"))
}

async fn outbox_order_json(pool: &PgPool, key: &str) -> Option<serde_json::Value> {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT order_json FROM oms_outbox WHERE idempotency_key = $1",
    )
    .bind(key)
    .fetch_optional(pool)
    .await
    .expect("query outbox")
}

async fn cleanup(h: &Harness) {
    sqlx::query("DELETE FROM oms_outbox WHERE run_id = $1")
        .bind(h.run)
        .execute(&h.pool)
        .await
        .expect("cleanup outbox");
    sqlx::query("DELETE FROM runs WHERE run_id = $1")
        .bind(h.run)
        .execute(&h.pool)
        .await
        .expect("cleanup runs");
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

fn blockers_text(json: &serde_json::Value) -> String {
    json["blockers"].to_string()
}

/// MO-01
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn mo01_crypto_symbol_is_rejected_not_enqueued_as_equity() {
    let _guard = DB_LOCK.lock().await;
    let h = harness(|st| st.trading_instrument_registry_v2_path = Some(btc_registry_file())).await;

    let id = unique("mo01");
    let (status, json) = post_manual_order(&h.st, &id, "BTC/USD").await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "rejected");
    assert!(
        blockers_text(&json).contains("asset class 'crypto'"),
        "{json}"
    );
    assert!(outbox_order_json(&h.pool, &id).await.is_none());

    cleanup(&h).await;
}

/// MO-02
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn mo02_occ_option_symbol_is_rejected_not_enqueued_as_equity() {
    let _guard = DB_LOCK.lock().await;
    let h = harness(|_| {}).await;

    let id = unique("mo02");
    let (status, json) = post_manual_order(&h.st, &id, "AAPL260116C00150000").await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["disposition"], "rejected");
    assert!(outbox_order_json(&h.pool, &id).await.is_none());

    cleanup(&h).await;
}

/// MO-03
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn mo03_registry_proven_equity_is_still_enqueued_with_the_equity_shape() {
    let _guard = DB_LOCK.lock().await;
    let h = harness(|_| {}).await;

    let id = unique("mo03");
    let (status, json) = post_manual_order(&h.st, &id, "AAPL").await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["accepted"], true);
    assert_eq!(json["disposition"], "enqueued");
    let order = outbox_order_json(&h.pool, &id)
        .await
        .expect("equity order must be enqueued");
    assert_eq!(order["symbol"], "AAPL");
    assert!(
        order.get("asset_class").is_none(),
        "Equity shape is unchanged (no asset_class field): {order}"
    );

    cleanup(&h).await;
}

/// MO-04
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn mo04_unreadable_registry_is_unavailable_authority_not_an_assumed_equity() {
    let _guard = DB_LOCK.lock().await;
    let h = harness(|st| {
        st.instrument_registry_path = std::env::temp_dir()
            .join(format!("mqk_mo04_missing_{}.json", Uuid::new_v4()))
            .to_string_lossy()
            .into_owned();
    })
    .await;

    let id = unique("mo04");
    let (status, json) = post_manual_order(&h.st, &id, "AAPL").await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    assert_eq!(json["disposition"], "unavailable");
    assert!(outbox_order_json(&h.pool, &id).await.is_none());

    cleanup(&h).await;
}
