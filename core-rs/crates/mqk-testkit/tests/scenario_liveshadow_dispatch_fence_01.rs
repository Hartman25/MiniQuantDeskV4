//! LIVESHADOW-NO-ORDER-AUTHORITY-CLOSURE-01 (dispatch fence).
//!
//! Admission refuses to *create* a LiveShadow order, but a durable `PENDING`
//! submit row can still exist (created before the admission fence, or returned
//! to `PENDING` by release / stale-claim reset / dispatching reset). The
//! orchestrator's Phase 1 is the only claim -> dispatch path, and it must
//! refuse such a row from the run's durable mode before `DISPATCHING`, while
//! leaving cancels to their own lifecycle authority.
//!
//! All proofs use a fake counting broker and a local DB. No real broker.
//!
//! | Test | What it proves |
//! |------|-----------------|
//! | f01 | LIVE-SHADOW pre-existing PENDING submit: broker submit count 0, row FAILED, repeat ticks/fresh orchestrators cannot submit |
//! | f02 | released / stale-reset / dispatching-reset LIVE-SHADOW submit rows still never reach the broker |
//! | f03 | absent / unknown / blank / corrupt request_type or payload under LIVE-SHADOW cannot evade the fence |
//! | f04 | LIVE-SHADOW cancel positive control: broker cancel reached, row ACKED, submit count 0 |
//! | f05 | PAPER submit positive control: reaches the fake broker, row SENT |
//! | f06 | LIVE-CAPITAL submit behaviour unchanged by the fence (fake broker only) |
//! | f07 | BACKTEST and unknown run modes fail closed for submit |
//! | f08 | source guard: the only dispatch-to-gateway submit path sits behind the fence |

use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use uuid::Uuid;

use mqk_db::FixedClock;
use mqk_execution::oms::state_machine::OmsOrder;
use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerGateway, BrokerInvokeToken,
    BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, IntegrityGate, QtyMicros, ReconcileGate, RiskGate,
};
use mqk_portfolio::PortfolioState;
use mqk_runtime::orchestrator::ExecutionOrchestrator;

static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Default)]
struct CountingBroker {
    submits: Arc<AtomicUsize>,
    cancels: Arc<AtomicUsize>,
}

impl BrokerAdapter for CountingBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        self.submits.fetch_add(1, Ordering::SeqCst);
        Ok(BrokerSubmitResponse {
            broker_order_id: format!("broker-{}", req.order_id),
            submitted_at: 1,
            status: "ok".to_string(),
        })
    }

    fn cancel_order(
        &self,
        id: &str,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerCancelResponse, BrokerError> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        Ok(BrokerCancelResponse {
            broker_order_id: id.to_string(),
            cancelled_at: 1,
            status: "ok".to_string(),
        })
    }

    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerReplaceResponse, BrokerError> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 1,
            status: "ok".to_string(),
        })
    }

    fn fetch_events(
        &self,
        _cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<(Vec<mqk_execution::BrokerEvent>, Option<String>), BrokerError> {
        Ok((vec![], None))
    }
}

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

fn require_db_url() -> String {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "PROOF: MQK_DATABASE_URL is not set. This is a load-bearing proof test and cannot be \
             skipped. Set MQK_DATABASE_URL to a live Postgres instance and re-run."
        ),
    }
}

async fn setup() -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&require_db_url())
        .await
        .unwrap_or_else(|e| panic!("PROOF: cannot connect to DB: {e}"));
    mqk_db::migrate(&pool).await.expect("migrate");
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("reset arm state");
    sqlx::query("delete from runtime_leader_lease")
        .execute(&pool)
        .await
        .expect("reset lease");
    pool
}

async fn cleanup_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query(
        "delete from broker_order_map where internal_id in \
         (select idempotency_key from oms_outbox where run_id = $1)",
    )
    .bind(run_id)
    .execute(pool)
    .await?;
    sqlx::query("delete from runtime_leader_lease")
        .execute(pool)
        .await?;
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn seed_running_run(pool: &PgPool, run_id: Uuid, mode: &str) -> Result<()> {
    cleanup_run(pool, run_id).await?;
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "ls-dispatch-fence".to_string(),
            mode: mode.to_string(),
            started_at_utc: Utc::now(),
            git_hash: "ls-dispatch-fence".to_string(),
            config_hash: "ls-dispatch-fence".to_string(),
            config_json: json!({}),
            host_fingerprint: "ls-dispatch-fence".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    Ok(())
}

type Orch = ExecutionOrchestrator<CountingBroker, PassGate, PassGate, PassGate, FixedClock>;

fn orchestrator(
    pool: &PgPool,
    run_id: Uuid,
    broker: CountingBroker,
    oms_orders: BTreeMap<String, OmsOrder>,
    order_map: BrokerOrderMap,
) -> Orch {
    ExecutionOrchestrator::new(
        pool.clone(),
        BrokerGateway::for_test(broker, PassGate, PassGate, PassGate),
        order_map,
        oms_orders,
        PortfolioState::new(0),
        run_id,
        "ls-fence-dispatcher",
        "test",
        None,
        FixedClock::new(Utc::now()),
        Box::new(mqk_reconcile::LocalSnapshot::empty),
        Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
    )
}

async fn tick_fresh(pool: &PgPool, run_id: Uuid, broker: &CountingBroker) -> Result<()> {
    let mut orch = orchestrator(
        pool,
        run_id,
        broker.clone(),
        BTreeMap::new(),
        BrokerOrderMap::new(),
    );
    let tick = orch.tick().await;
    orch.release_runtime_leadership().await?;
    tick
}

async fn status(pool: &PgPool, idem: &str) -> String {
    sqlx::query_scalar("select status from oms_outbox where idempotency_key = $1")
        .bind(idem)
        .fetch_one(pool)
        .await
        .expect("status")
}

fn submit_json() -> serde_json::Value {
    json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"})
}

fn run_id_for(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("ls-dispatch-fence.{seed}").as_bytes(),
    )
}

#[tokio::test]
async fn f01_live_shadow_preexisting_pending_submit_never_reaches_broker() -> Result<()> {
    let _g = DB_LOCK.lock().await;
    let pool = setup().await;
    let run_id = run_id_for("f01");
    seed_running_run(&pool, run_id, "LIVE-SHADOW").await?;
    // Unfenced insert models a row that predates the admission fence.
    mqk_db::outbox_enqueue(&pool, run_id, "f01-order", submit_json()).await?;

    let broker = CountingBroker::default();
    tick_fresh(&pool, run_id, &broker).await?;
    assert_eq!(broker.submits.load(Ordering::SeqCst), 0);
    assert_eq!(status(&pool, "f01-order").await, "FAILED");

    // Repeated ticks and fresh orchestrators (restart) cannot resurrect it.
    for _ in 0..3 {
        tick_fresh(&pool, run_id, &broker).await?;
    }
    assert_eq!(broker.submits.load(Ordering::SeqCst), 0);
    assert_eq!(status(&pool, "f01-order").await, "FAILED");
    let map_rows: i64 =
        sqlx::query_scalar("select count(*) from broker_order_map where internal_id = 'f01-order'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(map_rows, 0, "no broker mapping may be fabricated");

    cleanup_run(&pool, run_id).await
}

#[tokio::test]
async fn f02_recovered_live_shadow_submit_rows_never_reach_broker() -> Result<()> {
    let _g = DB_LOCK.lock().await;
    let pool = setup().await;
    let run_id = run_id_for("f02");
    seed_running_run(&pool, run_id, "LIVE-SHADOW").await?;
    let now = Utc::now();

    for key in ["f02-release", "f02-stale", "f02-dispatching-reset"] {
        mqk_db::outbox_enqueue(&pool, run_id, key, submit_json()).await?;
    }

    let claimed = mqk_db::outbox_claim_batch_for_run(&pool, run_id, 3, "recov", now).await?;
    assert_eq!(claimed.len(), 3);
    let dispatching = claimed
        .iter()
        .find(|r| r.row.idempotency_key == "f02-dispatching-reset")
        .expect("claimed")
        .row
        .clone();

    // CLAIMED -> PENDING via release.
    assert!(mqk_db::outbox_release_claim(&pool, "f02-release").await?);

    // CLAIMED -> DISPATCHING -> PENDING via dispatching reset (crash window).
    assert!(
        mqk_db::outbox_mark_dispatching(
            &pool,
            dispatching.outbox_id,
            "f02-dispatching-reset",
            dispatching.claimed_by.as_deref().unwrap(),
            "recov",
            now,
        )
        .await?
    );

    // CLAIMED -> PENDING via stale-claim reset (only f02-stale is still CLAIMED).
    assert_eq!(
        mqk_db::outbox_reset_stale_claims(&pool, run_id, now + chrono::Duration::seconds(60))
            .await?,
        1
    );
    assert!(mqk_db::outbox_reset_dispatching_to_pending(&pool, "f02-dispatching-reset").await?);

    for key in ["f02-release", "f02-stale", "f02-dispatching-reset"] {
        assert_eq!(status(&pool, key).await, "PENDING", "precondition {key}");
    }

    let broker = CountingBroker::default();
    for _ in 0..3 {
        tick_fresh(&pool, run_id, &broker).await?;
    }
    assert_eq!(broker.submits.load(Ordering::SeqCst), 0);
    for key in ["f02-release", "f02-stale", "f02-dispatching-reset"] {
        assert_eq!(status(&pool, key).await, "FAILED", "{key}");
    }

    cleanup_run(&pool, run_id).await
}

#[tokio::test]
async fn f03_unclassifiable_requests_under_live_shadow_cannot_evade_fence() -> Result<()> {
    let _g = DB_LOCK.lock().await;
    let pool = setup().await;
    let run_id = run_id_for("f03");
    seed_running_run(&pool, run_id, "LIVE-SHADOW").await?;

    let mut with_type = submit_json();
    with_type["request_type"] = json!("bogus");
    let mut blank_type = submit_json();
    blank_type["request_type"] = json!("  ");
    let mut upper_submit = submit_json();
    upper_submit["request_type"] = json!("SUBMIT");
    let rows = [
        ("f03-absent-type", submit_json()),
        ("f03-unknown-type", with_type),
        ("f03-blank-type", blank_type),
        ("f03-upper-submit", upper_submit),
        ("f03-corrupt-submit", json!({"request_type": "submit"})),
        ("f03-cancel-no-target", json!({"request_type": "cancel"})),
    ];
    for (key, payload) in &rows {
        mqk_db::outbox_enqueue(&pool, run_id, key, payload.clone()).await?;
    }

    let broker = CountingBroker::default();
    for _ in 0..rows.len() {
        tick_fresh(&pool, run_id, &broker).await?;
    }
    assert_eq!(broker.submits.load(Ordering::SeqCst), 0);
    assert_eq!(broker.cancels.load(Ordering::SeqCst), 0);
    for (key, _) in &rows {
        assert_eq!(status(&pool, key).await, "FAILED", "{key}");
    }

    cleanup_run(&pool, run_id).await
}

#[tokio::test]
async fn f04_live_shadow_cancel_positive_control_still_dispatches() -> Result<()> {
    let _g = DB_LOCK.lock().await;
    let pool = setup().await;
    let run_id = run_id_for("f04");
    seed_running_run(&pool, run_id, "LIVE-SHADOW").await?;

    // A legitimate existing broker-mapped order: live OMS entry + broker map.
    let target = "f04-existing-order";
    let mut oms = BTreeMap::new();
    oms.insert(
        target.to_string(),
        OmsOrder::new(target, "SPY", QtyMicros::from_whole_units(1).unwrap()),
    );
    let mut map = BrokerOrderMap::new();
    map.register(target, "broker-f04-existing-order");

    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        "f04-cancel",
        json!({"request_type": "cancel", "target_order_id": target}),
    )
    .await?;

    let broker = CountingBroker::default();
    let mut orch = orchestrator(&pool, run_id, broker.clone(), oms, map);
    orch.tick().await?;
    orch.release_runtime_leadership().await?;

    assert_eq!(
        broker.cancels.load(Ordering::SeqCst),
        1,
        "cancel reached the fake broker"
    );
    assert_eq!(broker.submits.load(Ordering::SeqCst), 0);
    assert_eq!(status(&pool, "f04-cancel").await, "ACKED");

    cleanup_run(&pool, run_id).await
}

#[tokio::test]
async fn f05_paper_submit_positive_control_reaches_fake_broker() -> Result<()> {
    let _g = DB_LOCK.lock().await;
    let pool = setup().await;
    let run_id = run_id_for("f05");
    seed_running_run(&pool, run_id, "PAPER").await?;
    mqk_db::outbox_enqueue(&pool, run_id, "f05-order", submit_json()).await?;

    let broker = CountingBroker::default();
    tick_fresh(&pool, run_id, &broker).await?;
    assert_eq!(broker.submits.load(Ordering::SeqCst), 1);
    assert_eq!(status(&pool, "f05-order").await, "SENT");

    cleanup_run(&pool, run_id).await
}

#[tokio::test]
async fn f06_live_capital_submit_semantics_unchanged_by_fence() -> Result<()> {
    let _g = DB_LOCK.lock().await;
    let pool = setup().await;
    let run_id = run_id_for("f06");
    seed_running_run(&pool, run_id, "LIVE-CAPITAL").await?;
    mqk_db::outbox_enqueue(&pool, run_id, "f06-order", submit_json()).await?;

    // Fake broker only. The fence neither adds nor removes LiveCapital
    // authority: start gates (parity trust, capital policy) are elsewhere.
    let broker = CountingBroker::default();
    tick_fresh(&pool, run_id, &broker).await?;
    assert_eq!(broker.submits.load(Ordering::SeqCst), 1);
    assert_eq!(status(&pool, "f06-order").await, "SENT");

    cleanup_run(&pool, run_id).await
}

#[tokio::test]
async fn f07_backtest_and_unknown_run_modes_fail_closed_for_submit() -> Result<()> {
    let _g = DB_LOCK.lock().await;
    let pool = setup().await;
    let run_id = run_id_for("f07");
    seed_running_run(&pool, run_id, "BACKTEST").await?;
    mqk_db::outbox_enqueue(&pool, run_id, "f07-backtest", submit_json()).await?;

    let broker = CountingBroker::default();
    tick_fresh(&pool, run_id, &broker).await?;
    assert_eq!(broker.submits.load(Ordering::SeqCst), 0);
    assert_eq!(status(&pool, "f07-backtest").await, "FAILED");

    // A legacy / unrecognized mode string is never order-capable.
    let legacy = run_id_for("f07-legacy");
    seed_running_run(&pool, legacy, "LIVE").await?;
    mqk_db::outbox_enqueue(&pool, legacy, "f07-legacy", submit_json()).await?;
    tick_fresh(&pool, legacy, &broker).await?;
    assert_eq!(broker.submits.load(Ordering::SeqCst), 0);
    assert_eq!(status(&pool, "f07-legacy").await, "FAILED");

    cleanup_run(&pool, run_id).await?;
    cleanup_run(&pool, legacy).await
}

/// Production source guard (no DB): every path that can turn a claimed row
/// into a gateway submit goes through the mode-aware Phase 1 fence.
#[test]
fn f08_only_fenced_phase1_reaches_gateway_submit() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut offenders = Vec::new();
    for krate in ["mqk-runtime", "mqk-daemon", "mqk-cli"] {
        let mut stack = vec![crates.join(krate).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read_dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    let text = std::fs::read_to_string(&path).expect("read");
                    // Only production code: stop at the first test-module marker.
                    let prod = text.split("#[cfg(test)]").next().unwrap_or("");
                    if prod.contains(".submit_with_context(") || prod.contains("gateway.submit(") {
                        offenders.push(path.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }
    }
    offenders.sort();
    assert!(
        offenders.len() == 1 && offenders[0].ends_with("mqk-runtime/src/orchestrator/dispatch.rs"),
        "only dispatch.rs may call the gateway submit: {offenders:?}"
    );

    let orch = std::fs::read_to_string(crates.join("mqk-runtime/src/orchestrator.rs"))
        .expect("read orchestrator.rs");
    let fence = orch
        .find("run_mode_permits_new_economic_order(&run.mode)")
        .expect("Phase 1 must derive new-order authority from the run's durable mode");
    let refusal = orch
        .find("refuse_new_order_claimed_outbox_row")
        .expect("Phase 1 must quarantine refused rows");
    let dispatch = orch
        .find("dispatch_submit_claimed_outbox_row(")
        .expect("submit dispatch call");
    assert!(
        fence < refusal && refusal < dispatch,
        "the mode fence and refusal must precede the only submit dispatch"
    );
    assert_eq!(
        orch.matches("dispatch_submit_claimed_outbox_row(").count(),
        1,
        "exactly one submit dispatch call site"
    );
}
