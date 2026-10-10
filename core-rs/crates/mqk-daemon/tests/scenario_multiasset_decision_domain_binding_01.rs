//! MULTIASSET-RUNTIME-INTEGRATION-01 / C1: an internal strategy decision is
//! admitted only onto the run, and counted against the caps, of the execution
//! domain that owns its asset class.
//!
//! Before this invariant `submit_internal_strategy_decision` resolved the
//! active run and the intake counters from `EquityNyse` unconditionally, so a
//! registry-resolved Crypto decision was enqueued onto the Equity domain's
//! outbox (or refused only by accident of the Equity run being absent).
//!
//! | Test | What it proves |
//! |------|----------------|
//! | MD-01 | Crypto decision with both domains running lands on the Crypto run; Equity run's outbox stays empty; counters are Crypto-keyed |
//! | MD-02 | Crypto decision with only the Equity run running is refused and writes no outbox row anywhere |
//! | MD-03 | Equity decision with both domains running still lands on the Equity run (control) |
//!
//! DB-backed (`#[ignore]`): run with
//! `MQK_DATABASE_URL=postgres://.../mqk_test cargo test -p mqk-daemon \
//!  --test scenario_multiasset_decision_domain_binding_01 -- --include-ignored`

mod common;

use std::sync::Arc;

use chrono::{Duration, Utc};
use mqk_daemon::{
    decision::{submit_internal_strategy_decision, InternalStrategyDecision},
    state::{self, BrokerKind, DeploymentMode, ExecutionDomain},
};
use mqk_md::instrument_registry_v2::{
    ContractDefinitionV2, InstrumentDefinitionV2, InstrumentEconomicsMetadataV2,
    InstrumentMetadataV2, InstrumentRegistryV2, SESSION_PROFILE_CRYPTO_24_7,
};
use sqlx::PgPool;
use std::collections::BTreeMap;
use uuid::Uuid;

/// The DB-backed tests share one durable arm-state row and one RUNNING run per
/// domain; serialise them.
static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const CRYPTO_QTY_MICROS: i64 = 100; // 0.0001 BTC; registry increment/minimum are both 100

fn fingerprint() -> String {
    "c".repeat(64)
}

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
            notes: Some("domain-binding proof fixture".to_string()),
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
    let dir = std::env::temp_dir().join(format!("mqk_md01_{}", Uuid::new_v4()));
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

fn unique(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

async fn seed_strategy(pool: &PgPool, strategy_id: &str, symbol: &str, timeframe_secs: i64) {
    let ts = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: format!("domain binding {strategy_id}"),
            enabled: true,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("seed strategy registry");

    let now = Utc::now();
    let id = |suffix: &str| {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("md01-promo:{strategy_id}:{symbol}:{suffix}").as_bytes(),
        )
    };
    let step = |transition_id: Uuid,
                previous: Option<&str>,
                new_state: &str,
                at: chrono::DateTime<Utc>| {
        mqk_db::InsertStrategyPromotionTransitionArgs {
            transition_id,
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs,
            config_fingerprint: Some(fingerprint()),
            config_identity_status: "verified_v1".to_string(),
            previous_state: previous.map(str::to_string),
            new_state: new_state.to_string(),
            parent_transition_id: None,
            evidence_transition_id: None,
            evidence_review_id: None,
            evidence_scanner_scan_id: None,
            evidence_git_hash: None,
            evidence_artifact_path: None,
            evidence_fingerprint: None,
            evidence_fingerprint_v2: None,
            effective_at_utc: at,
            expires_at_utc: None,
            initiated_by: "md01-seed".to_string(),
            reason: "domain binding seed".to_string(),
            created_at_utc: at,
        }
    };
    for (n, (prev, next, offset)) in [
        (None, "shadow_approved", 0),
        (Some("shadow_approved"), "paper_approved", 1),
        (Some("paper_approved"), "active_paper", 2),
    ]
    .into_iter()
    .enumerate()
    {
        mqk_db::insert_strategy_promotion_transition(
            pool,
            &step(
                id(&n.to_string()),
                prev,
                next,
                now + Duration::milliseconds(offset),
            ),
        )
        .await
        .expect("seed promotion transition");
    }
}

fn decision(
    decision_id: &str,
    strategy_id: &str,
    symbol: &str,
    tif: &str,
) -> InternalStrategyDecision {
    let crypto = symbol.contains('/');
    InternalStrategyDecision {
        decision_id: decision_id.to_string(),
        strategy_id: strategy_id.to_string(),
        symbol: symbol.to_string(),
        timeframe_secs: 300,
        strategy_semantic_fingerprint: fingerprint(),
        side: "buy".to_string(),
        qty: if crypto {
            mqk_execution::QtyMicros::new(CRYPTO_QTY_MICROS)
        } else {
            mqk_execution::QtyMicros::from_whole_units(1).unwrap()
        },
        order_type: "market".to_string(),
        time_in_force: tif.to_string(),
        limit_price: None,
    }
}

struct Harness {
    pool: PgPool,
    st: Arc<state::AppState>,
    equity_run: Uuid,
    crypto_run: Option<Uuid>,
}

async fn harness(with_crypto_run: bool) -> Harness {
    std::env::set_var("MQK_STRATEGY_TARGET_QTY", "0.0001");
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
    st.trading_instrument_registry_v2_path = Some(btc_registry_file());
    let st = Arc::new(st);

    let equity_run = Uuid::new_v4();
    st.establish_db_backed_active_run_for_test(ExecutionDomain::EquityNyse, equity_run)
        .await
        .expect("equity run");
    let crypto_run = if with_crypto_run {
        let run = Uuid::new_v4();
        st.establish_db_backed_active_run_for_test(ExecutionDomain::Crypto24_7, run)
            .await
            .expect("crypto run");
        Some(run)
    } else {
        None
    };
    Harness {
        pool,
        st,
        equity_run,
        crypto_run,
    }
}

async fn outbox_run_for(pool: &PgPool, key: &str) -> Option<Uuid> {
    sqlx::query_scalar::<_, Uuid>("SELECT run_id FROM oms_outbox WHERE idempotency_key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .expect("query outbox")
}

async fn cleanup(h: &Harness) {
    for run in std::iter::once(h.equity_run).chain(h.crypto_run) {
        sqlx::query("DELETE FROM oms_outbox WHERE run_id = $1")
            .bind(run)
            .execute(&h.pool)
            .await
            .expect("cleanup outbox");
        sqlx::query("DELETE FROM runs WHERE run_id = $1")
            .bind(run)
            .execute(&h.pool)
            .await
            .expect("cleanup runs");
    }
}

/// MD-01
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn md01_crypto_decision_lands_on_the_crypto_domain_run_only() {
    let _guard = DB_LOCK.lock().await;
    let h = harness(true).await;
    let crypto_run = h.crypto_run.expect("crypto run");
    let sid = unique("md01");
    seed_strategy(&h.pool, &sid, "BTC/USD", 300).await;

    let equity_before = h.st.day_signal_count(ExecutionDomain::EquityNyse);
    let crypto_before = h.st.day_signal_count(ExecutionDomain::Crypto24_7);

    let did = unique("dec");
    let out =
        submit_internal_strategy_decision(&h.st, decision(&did, &sid, "BTC/USD", "gtc")).await;

    assert!(
        out.accepted,
        "crypto decision with a Crypto run must be accepted: {:?} {:?}",
        out.disposition, out.blockers
    );
    assert_eq!(out.active_run_id, Some(crypto_run), "wrong owning run");
    assert_eq!(
        outbox_run_for(&h.pool, &did).await,
        Some(crypto_run),
        "durable outbox row must belong to the Crypto domain run"
    );
    assert_eq!(
        h.st.day_signal_count(ExecutionDomain::Crypto24_7),
        crypto_before + 1,
        "Crypto intake counter must advance"
    );
    assert_eq!(
        h.st.day_signal_count(ExecutionDomain::EquityNyse),
        equity_before,
        "Equity intake counter must not be consumed by a Crypto order"
    );

    cleanup(&h).await;
}

/// MD-02
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn md02_crypto_decision_never_lands_on_the_equity_run() {
    let _guard = DB_LOCK.lock().await;
    let h = harness(false).await;
    let sid = unique("md02");
    seed_strategy(&h.pool, &sid, "BTC/USD", 300).await;

    let did = unique("dec");
    let out =
        submit_internal_strategy_decision(&h.st, decision(&did, &sid, "BTC/USD", "gtc")).await;

    assert!(
        !out.accepted,
        "no Crypto domain run exists: the order must be refused, got {:?}",
        out.disposition
    );
    assert_eq!(out.disposition, "unavailable");
    assert!(
        out.blockers
            .iter()
            .any(|b| b.contains("no active durable run")),
        "{:?}",
        out.blockers
    );
    assert_eq!(
        outbox_run_for(&h.pool, &did).await,
        None,
        "a refused Crypto decision must leave no outbox row on the Equity run"
    );
    assert_eq!(
        h.st.day_signal_count(ExecutionDomain::EquityNyse),
        0,
        "Equity counter untouched"
    );

    cleanup(&h).await;
}

/// MD-03
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn md03_equity_decision_still_lands_on_the_equity_run() {
    let _guard = DB_LOCK.lock().await;
    let h = harness(true).await;
    let sid = unique("md03");
    seed_strategy(&h.pool, &sid, "AAPL", 300).await;

    let did = unique("dec");
    let out = submit_internal_strategy_decision(&h.st, decision(&did, &sid, "AAPL", "day")).await;

    assert!(
        out.accepted,
        "equity decision must be accepted: {:?} {:?}",
        out.disposition, out.blockers
    );
    assert_eq!(out.active_run_id, Some(h.equity_run));
    assert_eq!(outbox_run_for(&h.pool, &did).await, Some(h.equity_run));
    assert_eq!(
        h.st.day_signal_count(ExecutionDomain::Crypto24_7),
        0,
        "Crypto counter untouched by an Equity order"
    );

    cleanup(&h).await;
}
