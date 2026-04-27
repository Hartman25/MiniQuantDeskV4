//! Scenario: Cancel-Target-Missing — Group C1 (EXEC-CANCEL-01)
//!
//! Proves that when a cancel outbox row targets an order absent from the live
//! OMS map, the dispatch path handles the failure immediately and durably:
//!
//! ## Guarantees under test
//!
//! | Outcome        | Expected state                                      |
//! |----------------|-----------------------------------------------------|
//! | Outbox row     | `AMBIGUOUS` — never left `DISPATCHING`              |
//! | Run status     | `HALTED`                                            |
//! | Arm state      | `DISARMED` with reason `"CancelTargetMissing"`      |
//! | Error text     | Contains `"CANCEL_TARGET_MISSING"`                  |
//! | Next tick      | Blocked by Phase-0 `HALT_GUARD`                     |
//! | Broker call    | `cancel_order` is NEVER invoked (OMS check is first)|
//!
//! ## DB-backed tests (skipped unless `MQK_DATABASE_URL` is set)
//!
//! - **C1-01**: Cancel row targeting absent OMS order → row `AMBIGUOUS`,
//!   run `HALTED`, arm `DISARMED("CancelTargetMissing")`, error text
//!   `"CANCEL_TARGET_MISSING"`.  `cancel_order` is structurally proven
//!   unreachable via `NeverCallBroker` panic guard.
//! - **C1-02**: After the C1-01 halt, a fresh orchestrator on the same run is
//!   blocked by Phase-0 `HALT_GUARD` before any outbox claim or broker call.

#[cfg(test)]
mod db_tests {
    use std::collections::BTreeMap;
    use std::sync::OnceLock;

    use anyhow::Result;
    use chrono::Utc;
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use sqlx::PgPool;
    use tokio::sync::{Mutex, MutexGuard};
    use uuid::Uuid;

    use mqk_db::FixedClock;
    use mqk_execution::{
        BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerGateway, BrokerInvokeToken,
        BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
        BrokerSubmitResponse, IntegrityGate, ReconcileGate, RiskGate,
    };
    use mqk_portfolio::PortfolioState;
    use mqk_runtime::orchestrator::ExecutionOrchestrator;

    // -----------------------------------------------------------------------
    // Fixed run UUIDs — one per scenario for deterministic cleanup.
    // -----------------------------------------------------------------------

    const C1_01_RUN_ID: &str = "29200300-0000-0000-0000-000000000000";
    const C1_02_RUN_ID: &str = "29200301-0000-0000-0000-000000000000";

    static C1_DB_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    async fn acquire_c1_lock() -> MutexGuard<'static, ()> {
        C1_DB_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await
    }

    // -----------------------------------------------------------------------
    // Gate stubs — all pass (gate refusals must not mask the cancel failure)
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // NeverCallBroker — panics on submit_order or cancel_order.
    //
    // Structural proof: if either panic fires, the test fails, proving the
    // broker was reached before the OMS check — which is incorrect ordering.
    // Both methods must be unreachable in cancel-target-missing scenarios.
    // -----------------------------------------------------------------------

    struct NeverCallBroker;

    impl BrokerAdapter for NeverCallBroker {
        fn submit_order(
            &self,
            _req: BrokerSubmitRequest,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
            panic!(
                "NeverCallBroker: submit_order must not be called in cancel-target-missing scenario"
            )
        }

        fn cancel_order(
            &self,
            _id: &str,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<BrokerCancelResponse, BrokerError> {
            panic!(
                "NeverCallBroker: cancel_order must not be called before OMS target check; \
                 EXEC-CANCEL-01 requires OMS lookup precedes any broker call"
            )
        }

        fn replace_order(
            &self,
            req: BrokerReplaceRequest,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<BrokerReplaceResponse, BrokerError> {
            Ok(BrokerReplaceResponse {
                broker_order_id: req.broker_order_id,
                replaced_at: 0,
                status: "ok".into(),
            })
        }

        fn fetch_events(
            &self,
            _cursor: Option<&str>,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<(Vec<mqk_execution::BrokerEvent>, Option<String>), BrokerError>
        {
            // Phase 2 is never reached when Phase 1 returns Err — safe default.
            Ok((vec![], None))
        }
    }

    // -----------------------------------------------------------------------
    // Harness helpers
    // -----------------------------------------------------------------------

    fn require_db_url() -> Option<String> {
        match std::env::var(mqk_db::ENV_DB_URL) {
            Ok(v) if !v.trim().is_empty() => Some(v),
            _ => None,
        }
    }

    async fn require_pool(url: &str) -> PgPool {
        PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(url)
            .await
            .unwrap_or_else(|e| panic!("C1: cannot connect to DB: {e}"))
    }

    async fn cleanup_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
        sqlx::query("delete from runs where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    async fn reset_arm_state(pool: &PgPool) -> Result<()> {
        sqlx::query("delete from sys_arm_state where sentinel_id = 1")
            .execute(pool)
            .await?;
        Ok(())
    }

    async fn reset_c1_runtime_lease(pool: &PgPool) -> Result<()> {
        sqlx::query("delete from runtime_leader_lease where holder_id like 'c1-dispatcher|%'")
            .execute(pool)
            .await?;
        Ok(())
    }

    async fn seed_running_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
        mqk_db::insert_run(
            pool,
            &mqk_db::NewRun {
                run_id,
                engine_id: "c1-test".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: Utc::now(),
                git_hash: "c1-test".to_string(),
                config_hash: "c1-test".to_string(),
                config_json: json!({}),
                host_fingerprint: "c1-test".to_string(),
            },
        )
        .await?;
        mqk_db::arm_run(pool, run_id).await?;
        mqk_db::begin_run(pool, run_id).await?;
        Ok(())
    }

    /// Construct an orchestrator with an empty OMS order map.
    ///
    /// The empty map ensures that any cancel target is absent, triggering the
    /// CANCEL_TARGET_MISSING path on the first tick that claims a cancel row.
    fn make_orchestrator_empty_oms(
        pool: PgPool,
        run_id: Uuid,
    ) -> ExecutionOrchestrator<NeverCallBroker, PassGate, PassGate, PassGate, FixedClock> {
        let gateway = BrokerGateway::for_test(NeverCallBroker, PassGate, PassGate, PassGate);
        ExecutionOrchestrator::new(
            pool,
            gateway,
            BrokerOrderMap::new(),
            BTreeMap::new(), // empty — cancel target will be absent
            PortfolioState::new(0),
            run_id,
            "c1-dispatcher",
            "c1-test",
            None,
            FixedClock::new(Utc::now()),
            Box::new(mqk_reconcile::LocalSnapshot::empty),
            Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
        )
    }

    async fn outbox_status(pool: &PgPool, idem: &str) -> Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("select status from oms_outbox where idempotency_key = $1")
                .bind(idem)
                .fetch_optional(pool)
                .await?;
        Ok(row.map(|(s,)| s))
    }

    /// Cancel outbox payload targeting `target_order_id`.
    fn cancel_json(target_order_id: &str) -> serde_json::Value {
        json!({
            "request_type": "cancel",
            "target_order_id": target_order_id,
        })
    }

    // -----------------------------------------------------------------------
    // C1-01: Cancel row targeting absent OMS order → AMBIGUOUS row,
    //        HALTED run, DISARMED arm(CancelTargetMissing), CANCEL_TARGET_MISSING error.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn c1_01_cancel_missing_target_marks_ambiguous_and_halts_disarms() -> Result<()> {
        let _guard = acquire_c1_lock().await;

        let url = match require_db_url() {
            Some(u) => u,
            None => {
                eprintln!(
                    "SKIP c1_01_cancel_missing_target_marks_ambiguous_and_halts_disarms: \
                     MQK_DATABASE_URL not set"
                );
                return Ok(());
            }
        };
        let pool = require_pool(&url).await;
        mqk_db::migrate(&pool).await?;

        let run_id: Uuid = C1_01_RUN_ID.parse().unwrap();
        let idem = "c1-01-cancel-missing-target-001";

        cleanup_run(&pool, run_id).await?;
        reset_arm_state(&pool).await?;
        reset_c1_runtime_lease(&pool).await?;

        seed_running_run(&pool, run_id).await?;
        // Enqueue a cancel row targeting an order that is NOT in the OMS map.
        assert!(
            mqk_db::outbox_enqueue(&pool, run_id, idem, cancel_json("ord-nonexistent")).await?,
            "C1-01: cancel row must enqueue successfully"
        );

        let mut orch = make_orchestrator_empty_oms(pool.clone(), run_id);
        let err = orch
            .tick()
            .await
            .expect_err("C1-01: tick must fail when cancel target is absent from OMS");
        let msg = err.to_string();

        // Error must name the failure explicitly with the operator-visible label.
        assert!(
            msg.contains("CANCEL_TARGET_MISSING"),
            "C1-01: error must contain CANCEL_TARGET_MISSING, got: {msg}"
        );

        // Row must be AMBIGUOUS — not left in DISPATCHING.
        // A DISPATCHING row without this fix would leave the lifecycle reason
        // opaque; AMBIGUOUS is the correct durable quarantine state.
        let status = outbox_status(&pool, idem).await?;
        assert_eq!(
            status.as_deref(),
            Some("AMBIGUOUS"),
            "C1-01: cancel row must be AMBIGUOUS after missing-target failure, got {status:?}"
        );

        // Run must be HALTED — lifecycle truth is unsafe without a known cancel target.
        let run = mqk_db::fetch_run(&pool, run_id).await?;
        assert!(
            matches!(run.status, mqk_db::RunStatus::Halted),
            "C1-01: run must be HALTED after missing cancel target, got {:?}",
            run.status
        );

        // Arm must be DISARMED with the explicit reason CancelTargetMissing.
        // This makes the disarm cause queryable from sys_arm_state for operator
        // diagnosis without relying on transient logs.
        let arm = mqk_db::load_arm_state(&pool).await?;
        assert_eq!(
            arm.as_ref().map(|(s, _)| s.as_str()),
            Some("DISARMED"),
            "C1-01: arm must be DISARMED, got {arm:?}"
        );
        assert_eq!(
            arm.as_ref().and_then(|(_, r)| r.as_deref()),
            Some("CancelTargetMissing"),
            "C1-01: disarm reason must be CancelTargetMissing, got {arm:?}"
        );

        cleanup_run(&pool, run_id).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // C1-02: After C1-01's halt, Phase-0 HALT_GUARD blocks a fresh tick.
    //
    // Proves that the halt written in C1-01 is durable: a fresh orchestrator
    // instance constructed for the same run_id is refused by Phase-0 on its
    // very first tick(), before any outbox claim or broker call.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn c1_02_halted_run_blocks_subsequent_tick_via_phase0_guard() -> Result<()> {
        let _guard = acquire_c1_lock().await;

        let url = match require_db_url() {
            Some(u) => u,
            None => {
                eprintln!(
                    "SKIP c1_02_halted_run_blocks_subsequent_tick_via_phase0_guard: \
                     MQK_DATABASE_URL not set"
                );
                return Ok(());
            }
        };
        let pool = require_pool(&url).await;
        mqk_db::migrate(&pool).await?;

        let run_id: Uuid = C1_02_RUN_ID.parse().unwrap();
        let idem = "c1-02-cancel-halt-guard-001";

        cleanup_run(&pool, run_id).await?;
        reset_arm_state(&pool).await?;
        reset_c1_runtime_lease(&pool).await?;

        seed_running_run(&pool, run_id).await?;
        assert!(
            mqk_db::outbox_enqueue(&pool, run_id, idem, cancel_json("ord-ghost")).await?,
            "C1-02: cancel row must enqueue"
        );

        // Tick 1: cancel with missing target → halt+disarm.
        let mut orch1 = make_orchestrator_empty_oms(pool.clone(), run_id);
        orch1
            .tick()
            .await
            .expect_err("C1-02: first tick must fail on missing cancel target");

        // Tick 2: fresh orchestrator for the same halted run.
        // Phase-0 HALT_GUARD must block immediately — no outbox claim, no
        // broker call.  NeverCallBroker's panic guards prove broker was not reached.
        let mut orch2 = make_orchestrator_empty_oms(pool.clone(), run_id);
        let err2 = orch2
            .tick()
            .await
            .expect_err("C1-02: second tick must be blocked by Phase-0 HALT_GUARD");
        let msg2 = err2.to_string();

        assert!(
            msg2.contains("HALT_GUARD"),
            "C1-02: second tick must be blocked by HALT_GUARD, got: {msg2}"
        );

        cleanup_run(&pool, run_id).await?;
        Ok(())
    }
}
