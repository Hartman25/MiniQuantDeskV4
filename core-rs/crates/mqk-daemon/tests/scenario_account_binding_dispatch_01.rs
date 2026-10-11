//! Account binding at the REAL dispatch path: a claimed durable outbox row, the
//! real `ExecutionOrchestrator::tick`, the real `BrokerGateway`, the real
//! `AlpacaBrokerAdapter` bound to a real `AccountEvidenceCell`, and an
//! in-process HTTP mock that COUNTS provider order POSTs. The mock proves
//! plumbing only; it says nothing about a real Alpaca account.
//!
//! B1  Fresh, entitled evidence with NO run/account binding (what an unrelated
//!     snapshot refresher can produce): the order is refused, the row ends
//!     FAILED, the provider receives zero POSTs.
//! B2  Positive control: once the run is bound to the same account the same
//!     kind of order reaches the provider exactly once and ends SENT.
//! B3  Identity conflict: evidence naming a different account than the bound
//!     one is refused (zero POSTs).
//!
//! DB-backed and `#[ignore]`: requires `MQK_DATABASE_URL`, fails loudly without
//! it. Run with `-- --include-ignored --test-threads=1`.

use std::collections::BTreeMap;

use chrono::Utc;
use httpmock::prelude::*;
use mqk_broker_alpaca::{
    AccountEntitlementEvidence, AccountEvidenceCell, AlpacaBrokerAdapter, AlpacaConfig,
};
use mqk_db::FixedClock;
use mqk_execution::{BrokerGateway, BrokerOrderMap, IntegrityGate, ReconcileGate, RiskGate};
use mqk_portfolio::PortfolioState;
use mqk_runtime::orchestrator::ExecutionOrchestrator;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

const ACCT: &str = "904837e3-3b76-47ec-b432-046db621571b";
const OTHER: &str = "11111111-2222-3333-4444-555555555555";
const B_RUN: [&str; 3] = [
    "29b00001-0000-0000-0000-000000000000",
    "29b00002-0000-0000-0000-000000000000",
    "29b00003-0000-0000-0000-000000000000",
];

struct PassGate;
impl IntegrityGate for PassGate {
    fn is_armed(&self) -> bool {
        true
    }
}
impl RiskGate for PassGate {
    fn evaluate_gate(&self) -> mqk_execution::RiskDecision {
        mqk_execution::RiskDecision::Allow
    }
}
impl ReconcileGate for PassGate {
    fn is_clean(&self) -> bool {
        true
    }
}

fn evidence(id: &str) -> AccountEntitlementEvidence {
    AccountEntitlementEvidence::from_account_json(&json!({
        "id": id, "status": "ACTIVE", "trading_blocked": false,
        "account_blocked": false, "trade_suspended_by_user": false
    }))
}

async fn pool() -> PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .expect("requires MQK_DATABASE_URL (disposable test Postgres)");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&url)
        .await
        .expect("connect");
    mqk_db::migrate(&pool).await.expect("migrate");
    pool
}

async fn cleanup(pool: &PgPool, run_id: Uuid) {
    for stmt in [
        "delete from broker_order_map where internal_id like 'bind-b%'",
        "delete from audit_events where run_id = $1",
        "delete from oms_inbox where run_id = $1",
        "delete from oms_outbox where run_id = $1",
        "delete from runs where run_id = $1",
    ] {
        let _ = sqlx::query(stmt).bind(run_id).execute(pool).await;
    }
    let _ = sqlx::query("delete from runtime_leader_lease")
        .execute(pool)
        .await;
}

async fn seed(pool: &PgPool, run_id: Uuid, idem: &str) {
    cleanup(pool, run_id).await;
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "account-binding-dispatch-test".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .unwrap();
    mqk_db::arm_run(pool, run_id).await.unwrap();
    mqk_db::begin_run(pool, run_id).await.unwrap();
    assert!(mqk_db::outbox_enqueue(
        pool,
        run_id,
        idem,
        json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
    )
    .await
    .unwrap());
}

fn orchestrator(
    pool: PgPool,
    run_id: Uuid,
    base_url: String,
    cell: &AccountEvidenceCell,
) -> ExecutionOrchestrator<AlpacaBrokerAdapter, PassGate, PassGate, PassGate, FixedClock> {
    let adapter = AlpacaBrokerAdapter::new(AlpacaConfig {
        base_url,
        api_key_id: "test-key".to_string(),
        api_secret_key: "test-secret".to_string(),
        crypto_capability_enabled: false,
        options_mleg_capability_enabled: false,
    })
    .with_account_evidence(cell.clone(), chrono::Duration::seconds(61));
    ExecutionOrchestrator::new(
        pool,
        BrokerGateway::for_test(adapter, PassGate, PassGate, PassGate),
        BrokerOrderMap::new(),
        BTreeMap::new(),
        PortfolioState::new(0),
        run_id,
        "binding-dispatcher",
        "test",
        None,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
    )
}

async fn outbox_status(pool: &PgPool, idem: &str) -> Option<String> {
    sqlx::query_scalar("select status from oms_outbox where idempotency_key = $1")
        .bind(idem)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn mock_provider(server: &MockServer) -> httpmock::Mock<'_> {
    server
        .mock_async(|when, then| {
            when.method(POST).path("/v2/orders");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "id": "alpaca-order-bind",
                    "client_order_id": "bind-b2",
                    "created_at": "2026-10-11T09:04:00Z",
                }));
        })
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL"]
async fn b1_unbound_evidence_never_reaches_the_provider_through_real_dispatch() {
    let pool = pool().await;
    let run_id: Uuid = B_RUN[0].parse().unwrap();
    let idem = "bind-b1";
    seed(&pool, run_id, idem).await;
    let server = MockServer::start_async().await;
    let posts = mock_provider(&server).await;

    // Fresh, entitled evidence observed by "someone else": no binding.
    let cell = AccountEvidenceCell::new();
    cell.observe(evidence(ACCT), Utc::now());
    let mut orch = orchestrator(pool.clone(), run_id, server.base_url(), &cell);
    let err = orch
        .tick()
        .await
        .expect_err("an unbound account must refuse");
    assert!(
        err.to_string().contains("account_binding_absent"),
        "refusal code must surface: {err}"
    );
    assert_eq!(outbox_status(&pool, idem).await.as_deref(), Some("FAILED"));
    posts.assert_hits_async(0).await;

    // The refusal is durably attributed: exact code and the real cell's
    // provenance (observed account, no binding).
    let payloads: Vec<serde_json::Value> = sqlx::query_scalar(
        "select payload from audit_events where run_id = $1            and event_type = 'ORDER_ACCOUNT_ENTITLEMENT_REFUSED'",
    )
    .bind(run_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(payloads.len(), 1);
    assert_eq!(payloads[0]["refusal_code"], "account_binding_absent");
    assert_eq!(payloads[0]["order_id"], idem);
    assert_eq!(payloads[0]["context"]["observed_provider_account_id"], ACCT);
    assert_eq!(payloads[0]["context"]["bound_provider_account_id"], "none");

    orch.release_runtime_leadership().await.unwrap();
    cleanup(&pool, run_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL"]
async fn b2_bound_account_reaches_the_provider_exactly_once() {
    let pool = pool().await;
    let run_id: Uuid = B_RUN[1].parse().unwrap();
    let idem = "bind-b2";
    seed(&pool, run_id, idem).await;
    let server = MockServer::start_async().await;
    let posts = mock_provider(&server).await;

    let cell = AccountEvidenceCell::new();
    cell.pin_provider_account_id(ACCT);
    cell.observe(evidence(ACCT), Utc::now());
    let mut orch = orchestrator(pool.clone(), run_id, server.base_url(), &cell);
    let _ = orch.tick().await;
    posts.assert_hits_async(1).await;
    assert_eq!(outbox_status(&pool, idem).await.as_deref(), Some("SENT"));

    orch.release_runtime_leadership().await.unwrap();
    cleanup(&pool, run_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL"]
async fn b3_identity_conflict_never_reaches_the_provider() {
    let pool = pool().await;
    let run_id: Uuid = B_RUN[2].parse().unwrap();
    let idem = "bind-b3";
    seed(&pool, run_id, idem).await;
    let server = MockServer::start_async().await;
    let posts = mock_provider(&server).await;

    let cell = AccountEvidenceCell::new();
    cell.pin_provider_account_id(ACCT);
    cell.observe(evidence(OTHER), Utc::now());
    let mut orch = orchestrator(pool.clone(), run_id, server.base_url(), &cell);
    let err = orch.tick().await.expect_err("identity drift must refuse");
    assert!(err.to_string().contains("account_identity_drift"), "{err}");
    assert_eq!(outbox_status(&pool, idem).await.as_deref(), Some("FAILED"));
    posts.assert_hits_async(0).await;

    orch.release_runtime_leadership().await.unwrap();
    cleanup(&pool, run_id).await;
}
