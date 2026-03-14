//! Scenario: Ambiguous Submit Quarantine — Patch A4
//!
//! # Invariants under test
//!
//! A4 hardens the handling of `BrokerError::AmbiguousSubmit` by introducing an
//! explicit `AMBIGUOUS` outbox status. Previously, an ambiguous submit left the
//! row in `DISPATCHING`, which conflated "crash mid-dispatch" and "confirmed
//! unknown outcome". `AMBIGUOUS` is a durable quarantine state with structural
//! enforcement: ordinary retry logic cannot reach it.
//!
//! ## Structural guarantees
//!
//! | State     | outbox_claim_batch | outbox_load_restart_ambiguous | Safe exit path             |
//! |-----------|:-----------------:|:------------------------------:|---------------------------|
//! | PENDING   | ✅ claims          | ❌ not returned                | normal dispatch            |
//! | AMBIGUOUS | ❌ never claims    | ✅ always returned             | reset_ambiguous_to_pending |
//!
//! ## DB-backed tests (skipped unless `MQK_DATABASE_URL` is set)
//!
//! - S1: `AmbiguousSubmit` broker → row transitions to `AMBIGUOUS`, run
//!   HALTED, arm DISARMED with reason `"AmbiguousSubmit"` (durable, not silent).
//! - S2: Restart with pre-existing `AMBIGUOUS` row → Phase-0b
//!   `outbox_load_restart_ambiguous` finds it and halts (RECOVERY_QUARANTINE).
//! - S3: `outbox_claim_batch` never claims an `AMBIGUOUS` row — structural
//!   prevention of silent re-dispatch (no double-submit possible via ordinary path).
//! - S4: `outbox_reset_ambiguous_to_pending` is the only safe release path;
//!   it transitions `AMBIGUOUS → PENDING` so dispatch can resume after re-arm;
//!   without calling it the row remains permanently blocked from dispatch.

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

    const S1_RUN_ID: &str = "29200007-0000-0000-0000-000000000000";
    const S2_RUN_ID: &str = "29200008-0000-0000-0000-000000000000";
    const S3_RUN_ID: &str = "29200009-0000-0000-0000-000000000000";
    const S4_RUN_ID: &str = "29200010-0000-0000-0000-000000000000";

    static A4_DB_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    async fn acquire_a4_db_test_lock() -> MutexGuard<'static, ()> {
        A4_DB_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await
    }

    // -----------------------------------------------------------------------
    // Gate stubs — all pass (so gate refusals cannot mask broker errors)
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
    // AmbiguousBroker — returns BrokerError::AmbiguousSubmit on every submit.
    // -----------------------------------------------------------------------

    struct AmbiguousBroker;

    impl BrokerAdapter for AmbiguousBroker {
        fn submit_order(
            &self,
            _req: BrokerSubmitRequest,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
            Err(BrokerError::AmbiguousSubmit {
                detail: "a4-test: timeout after send".into(),
            })
        }
        fn cancel_order(
            &self,
            _id: &str,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<BrokerCancelResponse, BrokerError> {
            Ok(BrokerCancelResponse {
                broker_order_id: "x".into(),
                cancelled_at: 0,
                status: "ok".into(),
            })
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
            Ok((vec![], None))
        }
    }

    // -----------------------------------------------------------------------
    // NeverSubmitBroker — panics on any submit attempt (used to prove S2/S3
    // quarantine; broker must never be reached).
    // -----------------------------------------------------------------------

    struct NeverSubmitBroker;

    impl BrokerAdapter for NeverSubmitBroker {
        fn submit_order(
            &self,
            _req: BrokerSubmitRequest,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
            panic!("NeverSubmitBroker: submit_order must not be called in quarantine scenario")
        }
        fn cancel_order(
            &self,
            _id: &str,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<BrokerCancelResponse, BrokerError> {
            Ok(BrokerCancelResponse {
                broker_order_id: "x".into(),
                cancelled_at: 0,
                status: "ok".into(),
            })
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
            .unwrap_or_else(|e| panic!("A4: cannot connect to DB: {e}"))
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

    async fn reset_a4_runtime_lease(pool: &PgPool) -> Result<()> {
        sqlx::query("delete from runtime_leader_lease where holder_id like 'a4-dispatcher|%'")
            .execute(pool)
            .await?;
        Ok(())
    }

    async fn seed_running_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
        mqk_db::insert_run(
            pool,
            &mqk_db::NewRun {
                run_id,
                engine_id: "a4-test".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: Utc::now(),
                git_hash: "a4-test".to_string(),
                config_hash: "a4-test".to_string(),
                config_json: json!({}),
                host_fingerprint: "a4-test".to_string(),
            },
        )
        .await?;
        mqk_db::arm_run(pool, run_id).await?;
        mqk_db::begin_run(pool, run_id).await?;
        Ok(())
    }

    fn make_orchestrator<B>(
        pool: PgPool,
        run_id: Uuid,
        broker: B,
    ) -> ExecutionOrchestrator<B, PassGate, PassGate, PassGate, FixedClock>
    where
        B: BrokerAdapter + Send + Sync + 'static,
    {
        let gateway = BrokerGateway::for_test(broker, PassGate, PassGate, PassGate);
        ExecutionOrchestrator::new(
            pool,
            gateway,
            BrokerOrderMap::new(),
            BTreeMap::new(),
            PortfolioState::new(0),
            run_id,
            "a4-dispatcher",
            "a4-test",
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

    fn order_json() -> serde_json::Value {
        json!({
            "symbol": "SPY",
            "quantity": 1,
            "order_type": "market",
            "time_in_force": "day"
        })
    }

    // -----------------------------------------------------------------------
    // S1: AmbiguousSubmit → row AMBIGUOUS, run HALTED, arm DISARMED(AmbiguousSubmit)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn s1_ambiguous_submit_marks_row_ambiguous_and_halts() -> Result<()> {
        let _guard = acquire_a4_db_test_lock().await;

        let url = match require_db_url() {
            Some(u) => u,
            None => {
                eprintln!("SKIP s1_ambiguous_submit_marks_row_ambiguous_and_halts: MQK_DATABASE_URL not set");
                return Ok(());
            }
        };
        let pool = require_pool(&url).await;
        mqk_db::migrate(&pool).await?;

        let run_id: Uuid = S1_RUN_ID.parse().unwrap();
        let idem = "a4-s1-ambiguous-ord-001";

        cleanup_run(&pool, run_id).await?;
        reset_arm_state(&pool).await?;
        reset_a4_runtime_lease(&pool).await?;

        seed_running_run(&pool, run_id).await?;
        assert!(mqk_db::outbox_enqueue(&pool, run_id, idem, order_json()).await?);

        let mut orch = make_orchestrator(pool.clone(), run_id, AmbiguousBroker);
        let err = orch
            .tick()
            .await
            .expect_err("tick must fail on AmbiguousSubmit");
        let msg = err.to_string();
        assert!(
            msg.contains("AmbiguousSubmit") || msg.contains("SUBMIT_BROKER_ERROR"),
            "error must reference AmbiguousSubmit, got: {msg}"
        );

        let status = outbox_status(&pool, idem).await?;
        assert_eq!(
            status.as_deref(),
            Some("AMBIGUOUS"),
            "A4: row must be AMBIGUOUS after AmbiguousSubmit, got {status:?}"
        );

        let run = mqk_db::fetch_run(&pool, run_id).await?;
        assert!(
            matches!(run.status, mqk_db::RunStatus::Halted),
            "run must be HALTED after AmbiguousSubmit"
        );

        let arm = mqk_db::load_arm_state(&pool).await?;
        assert_eq!(
            arm.as_ref().map(|(s, _)| s.as_str()),
            Some("DISARMED"),
            "arm must be DISARMED after AmbiguousSubmit, got {arm:?}"
        );
        assert_eq!(
            arm.as_ref().and_then(|(_, r)| r.as_deref()),
            Some("AmbiguousSubmit"),
            "disarm reason must be AmbiguousSubmit (durable, not silent), got {arm:?}"
        );

        cleanup_run(&pool, run_id).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // S2: Restart with AMBIGUOUS row present → Phase-0b quarantines.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn s2_restart_with_ambiguous_row_quarantines_before_dispatch() -> Result<()> {
        let _guard = acquire_a4_db_test_lock().await;

        let url = match require_db_url() {
            Some(u) => u,
            None => {
                eprintln!("SKIP s2_restart_with_ambiguous_row_quarantines_before_dispatch: MQK_DATABASE_URL not set");
                return Ok(());
            }
        };
        let pool = require_pool(&url).await;
        mqk_db::migrate(&pool).await?;

        let run_id: Uuid = S2_RUN_ID.parse().unwrap();
        let idem = "a4-s2-ambiguous-restart-ord-001";

        cleanup_run(&pool, run_id).await?;
        reset_arm_state(&pool).await?;
        reset_a4_runtime_lease(&pool).await?;

        seed_running_run(&pool, run_id).await?;
        assert!(mqk_db::outbox_enqueue(&pool, run_id, idem, order_json()).await?);

        let claimed =
            mqk_db::outbox_claim_batch_for_run(&pool, run_id, 1, "a4-setup-dispatcher", Utc::now())
                .await?;
        assert_eq!(claimed.len(), 1, "S2 setup: claim must succeed");
        assert!(
            mqk_db::outbox_mark_dispatching(&pool, idem, "a4-setup-dispatcher", Utc::now()).await?,
            "S2 setup: CLAIMED → DISPATCHING"
        );
        assert!(
            mqk_db::outbox_mark_ambiguous(&pool, idem).await?,
            "S2 setup: DISPATCHING → AMBIGUOUS"
        );

        // Simulate a restart that happened before the halt/disarm persistence from the
        // original ambiguous submit path. Phase-0b must overwrite this durable state to
        // RecoveryQuarantine when it rediscovers the AMBIGUOUS row.
        mqk_db::persist_arm_state(&pool, "ARMED", None).await?;

        let run = mqk_db::fetch_run(&pool, run_id).await?;
        assert!(
            matches!(run.status, mqk_db::RunStatus::Running),
            "S2 setup: run must be RUNNING before the restarted tick"
        );

        let mut orch = make_orchestrator(pool.clone(), run_id, NeverSubmitBroker);
        let err = orch
            .tick()
            .await
            .expect_err("tick must refuse dispatch on AMBIGUOUS restart row");
        let msg = err.to_string();
        assert!(
            msg.contains("RECOVERY_QUARANTINE"),
            "error must contain RECOVERY_QUARANTINE, got: {msg}"
        );

        let status = outbox_status(&pool, idem).await?;
        assert_eq!(
            status.as_deref(),
            Some("AMBIGUOUS"),
            "row must remain AMBIGUOUS after quarantine tick, got {status:?}"
        );

        let run = mqk_db::fetch_run(&pool, run_id).await?;
        assert!(
            matches!(run.status, mqk_db::RunStatus::Halted),
            "run must be HALTED after quarantine tick"
        );

        let arm = mqk_db::load_arm_state(&pool).await?;
        assert_eq!(
            arm.as_ref().map(|(s, _)| s.as_str()),
            Some("DISARMED"),
            "arm must be DISARMED after quarantine tick, got {arm:?}"
        );
        assert_eq!(
            arm.as_ref().and_then(|(_, r)| r.as_deref()),
            Some("RecoveryQuarantine"),
            "disarm reason must be RecoveryQuarantine, got {arm:?}"
        );

        cleanup_run(&pool, run_id).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // S3: outbox_claim_batch never claims an AMBIGUOUS row.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn s3_claim_batch_never_claims_ambiguous_row() -> Result<()> {
        let _guard = acquire_a4_db_test_lock().await;

        let url = match require_db_url() {
            Some(u) => u,
            None => {
                eprintln!(
                    "SKIP s3_claim_batch_never_claims_ambiguous_row: MQK_DATABASE_URL not set"
                );
                return Ok(());
            }
        };
        let pool = require_pool(&url).await;
        mqk_db::migrate(&pool).await?;

        let run_id: Uuid = S3_RUN_ID.parse().unwrap();
        let idem_ambiguous = "a4-s3-ambiguous-ord-001";
        let idem_pending = "a4-s3-pending-ord-002";

        cleanup_run(&pool, run_id).await?;
        reset_arm_state(&pool).await?;
        reset_a4_runtime_lease(&pool).await?;
        seed_running_run(&pool, run_id).await?;

        assert!(
            mqk_db::outbox_enqueue(&pool, run_id, idem_ambiguous, order_json()).await?,
            "S3: first outbox_enqueue must succeed"
        );
        {
            let claimed =
                mqk_db::outbox_claim_batch_for_run(&pool, run_id, 1, "s3-dispatcher", Utc::now())
                    .await?;
            assert_eq!(claimed.len(), 1);
            assert!(
                mqk_db::outbox_mark_dispatching(&pool, idem_ambiguous, "s3-dispatcher", Utc::now())
                    .await?
            );
            assert!(mqk_db::outbox_mark_ambiguous(&pool, idem_ambiguous).await?);
        }

        assert!(
            mqk_db::outbox_enqueue(&pool, run_id, idem_pending, order_json()).await?,
            "S3: second outbox_enqueue must succeed"
        );

        let claimed =
            mqk_db::outbox_claim_batch_for_run(&pool, run_id, 10, "s3-dispatcher", Utc::now())
                .await?;
        let claimed_keys: Vec<&str> = claimed
            .iter()
            .map(|r| r.row.idempotency_key.as_str())
            .collect();

        assert!(
            !claimed_keys.contains(&idem_ambiguous),
            "S3: AMBIGUOUS row must never be returned by claim_batch (got {claimed_keys:?})"
        );
        assert!(
            claimed_keys.contains(&idem_pending),
            "S3: PENDING row must be claimable (got {claimed_keys:?})"
        );

        let status = outbox_status(&pool, idem_ambiguous).await?;
        assert_eq!(
            status.as_deref(),
            Some("AMBIGUOUS"),
            "S3: AMBIGUOUS row must remain AMBIGUOUS after claim_batch"
        );

        cleanup_run(&pool, run_id).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // S4: outbox_reset_ambiguous_to_pending is the only safe release path.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn s4_reset_ambiguous_to_pending_is_only_safe_release_path() -> Result<()> {
        let _guard = acquire_a4_db_test_lock().await;

        let url = match require_db_url() {
            Some(u) => u,
            None => {
                eprintln!("SKIP s4_reset_ambiguous_to_pending_is_only_safe_release_path: MQK_DATABASE_URL not set");
                return Ok(());
            }
        };
        let pool = require_pool(&url).await;
        mqk_db::migrate(&pool).await?;

        let run_id: Uuid = S4_RUN_ID.parse().unwrap();
        let idem = "a4-s4-ambiguous-release-ord-001";

        cleanup_run(&pool, run_id).await?;
        reset_arm_state(&pool).await?;
        reset_a4_runtime_lease(&pool).await?;
        seed_running_run(&pool, run_id).await?;

        assert!(mqk_db::outbox_enqueue(&pool, run_id, idem, order_json()).await?);

        let claimed =
            mqk_db::outbox_claim_batch_for_run(&pool, run_id, 1, "s4-dispatcher", Utc::now())
                .await?;
        assert_eq!(claimed.len(), 1, "S4: claim must succeed");
        assert!(
            mqk_db::outbox_mark_dispatching(&pool, idem, "s4-dispatcher", Utc::now()).await?,
            "S4: CLAIMED → DISPATCHING"
        );
        assert!(
            mqk_db::outbox_mark_ambiguous(&pool, idem).await?,
            "S4: DISPATCHING → AMBIGUOUS"
        );

        let before_reset =
            mqk_db::outbox_claim_batch_for_run(&pool, run_id, 10, "s4-dispatcher", Utc::now())
                .await?;
        assert!(
            before_reset.is_empty(),
            "S4: claim_batch must return empty while row is AMBIGUOUS"
        );

        let noop = mqk_db::outbox_reset_ambiguous_to_pending(&pool, "nonexistent-key").await?;
        assert!(!noop, "S4: reset on nonexistent key must return false");

        let released = mqk_db::outbox_reset_ambiguous_to_pending(&pool, idem).await?;
        assert!(released, "S4: reset_ambiguous_to_pending must return true");

        let status = outbox_status(&pool, idem).await?;
        assert_eq!(
            status.as_deref(),
            Some("PENDING"),
            "S4: row must be PENDING after reset_ambiguous_to_pending, got {status:?}"
        );

        let after_reset =
            mqk_db::outbox_claim_batch_for_run(&pool, run_id, 10, "s4-dispatcher", Utc::now())
                .await?;
        assert_eq!(
            after_reset.len(),
            1,
            "S4: claim_batch must claim the released PENDING row"
        );
        assert_eq!(
            after_reset[0].row.idempotency_key, idem,
            "S4: claimed row must be the released row"
        );

        let double_reset = mqk_db::outbox_reset_ambiguous_to_pending(&pool, idem).await?;
        assert!(
            !double_reset,
            "S4: reset on non-AMBIGUOUS (CLAIMED) row must return false"
        );

        cleanup_run(&pool, run_id).await?;
        Ok(())
    }
}
