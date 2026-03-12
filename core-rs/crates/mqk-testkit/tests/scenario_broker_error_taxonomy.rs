//! Scenario: Broker Error Taxonomy — EXE-04R
//!
//! Verifies runtime submit disposition is fail-closed:
//! - Only explicit non-delivery proof may reset DISPATCHING -> PENDING.
//! - Any ambiguous delivery outcome is quarantined as AMBIGUOUS and halts/disarms.

use mqk_execution::{BrokerError, GateRefusal, SubmitError};

#[test]
fn a1_is_retryable_requires_explicit_non_delivery_proof() {
    assert!(BrokerError::Transport {
        non_delivery_proven: true,
        detail: "x".into()
    }
    .is_retryable());
    assert!(!BrokerError::Transport {
        non_delivery_proven: false,
        detail: "x".into()
    }
    .is_retryable());

    assert!(BrokerError::RateLimit {
        retry_after_ms: Some(1000),
        non_delivery_proven: true,
        detail: "x".into(),
    }
    .is_retryable());
    assert!(!BrokerError::RateLimit {
        retry_after_ms: Some(1000),
        non_delivery_proven: false,
        detail: "x".into(),
    }
    .is_retryable());
}

#[test]
fn a2_submit_error_display_includes_inner_detail() {
    let gate_err = SubmitError::Gate(GateRefusal::IntegrityDisarmed);
    assert!(gate_err.to_string().contains("integrity disarmed"));

    let broker_err = SubmitError::Broker(BrokerError::Transport {
        non_delivery_proven: true,
        detail: "connection refused".into(),
    });
    assert!(broker_err.to_string().contains("connection refused"));
}

#[cfg(test)]
mod db_tests {
    use std::collections::BTreeMap;

    use anyhow::Result;
    use chrono::Utc;
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use sqlx::PgPool;
    use uuid::Uuid;

    use mqk_db::FixedClock;
    use mqk_execution::{
        BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerGateway, BrokerInvokeToken,
        BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
        BrokerSubmitResponse, IntegrityGate, ReconcileGate, RiskGate,
    };
    use mqk_portfolio::PortfolioState;
    use mqk_runtime::orchestrator::ExecutionOrchestrator;

    const T1_RUN_ID: &str = "29210001-0000-0000-0000-000000000000";
    const T2_RUN_ID: &str = "29210002-0000-0000-0000-000000000000";
    const T3_RUN_ID: &str = "29210003-0000-0000-0000-000000000000";
    const T4_RUN_ID: &str = "29210004-0000-0000-0000-000000000000";
    const T5A_RUN_ID: &str = "29210005-0000-0000-0000-000000000000";
    const T5B_RUN_ID: &str = "29210006-0000-0000-0000-000000000000";

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

    struct SubmitErrorBroker {
        err: BrokerError,
    }
    impl BrokerAdapter for SubmitErrorBroker {
        fn submit_order(
            &self,
            _req: BrokerSubmitRequest,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
            Err(self.err.clone())
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

    async fn require_pool(url: &str) -> PgPool {
        PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(url)
            .await
            .unwrap_or_else(|e| panic!("EXE-04R: cannot connect to DB: {e}"))
    }

    async fn cleanup_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
        sqlx::query("delete from runs where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    async fn seed_running_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
        mqk_db::insert_run(
            pool,
            &mqk_db::NewRun {
                run_id,
                engine_id: "exe04r-test".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: Utc::now(),
                git_hash: "exe04r-test".to_string(),
                config_hash: "exe04r-test".to_string(),
                config_json: json!({}),
                host_fingerprint: "exe04r-test".to_string(),
            },
        )
        .await?;
        mqk_db::arm_run(pool, run_id).await?;
        mqk_db::begin_run(pool, run_id).await?;
        Ok(())
    }

    fn make_orchestrator(
        pool: PgPool,
        run_id: Uuid,
        broker: SubmitErrorBroker,
    ) -> ExecutionOrchestrator<SubmitErrorBroker, PassGate, PassGate, PassGate, FixedClock> {
        let gateway = BrokerGateway::for_test(broker, PassGate, PassGate, PassGate);
        ExecutionOrchestrator::new(
            pool,
            gateway,
            BrokerOrderMap::new(),
            BTreeMap::new(),
            PortfolioState::new(0),
            run_id,
            "exe04r-dispatcher",
            "test",
            None,
            FixedClock::new(Utc::now()),
            Box::new(mqk_reconcile::LocalSnapshot::empty),
            Box::new(mqk_reconcile::BrokerSnapshot::empty),
        )
    }

    async fn enqueue_one(pool: &PgPool, run_id: Uuid, idem: &str) -> Result<()> {
        let created = mqk_db::outbox_enqueue(
            pool,
            run_id,
            idem,
            json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
        )
        .await?;
        assert!(created, "outbox row must be created");
        Ok(())
    }

    async fn outbox_status(pool: &PgPool, idem_key: &str) -> Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("select status from oms_outbox where idempotency_key = $1")
                .bind(idem_key)
                .fetch_optional(pool)
                .await?;
        Ok(row.map(|(s,)| s))
    }

    async fn assert_halted_and_disarmed(pool: &PgPool, run_id: Uuid, reason: &str) -> Result<()> {
        let run = mqk_db::fetch_run(pool, run_id).await?;
        assert!(matches!(run.status, mqk_db::RunStatus::Halted));
        let arm = mqk_db::load_arm_state(pool).await?;
        assert_eq!(arm.as_ref().map(|(s, _)| s.as_str()), Some("DISARMED"));
        assert_eq!(arm.as_ref().and_then(|(_, r)| r.as_deref()), Some(reason));
        Ok(())
    }

    async fn prep(url: &str, run_id: Uuid, idem: &str) -> Result<PgPool> {
        let pool = require_pool(url).await;
        mqk_db::migrate(&pool).await?;
        cleanup_run(&pool, run_id).await?;
        sqlx::query("delete from sys_arm_state where sentinel_id = 1")
            .execute(&pool)
            .await?;
        seed_running_run(&pool, run_id).await?;
        enqueue_one(&pool, run_id, idem).await?;
        Ok(pool)
    }

    fn db_url_or_skip(name: &str) -> Option<String> {
        match std::env::var(mqk_db::ENV_DB_URL) {
            Ok(v) if !v.trim().is_empty() => Some(v),
            _ => {
                eprintln!("SKIP {name}: MQK_DATABASE_URL not set");
                None
            }
        }
    }

    #[tokio::test]
    async fn timeout_before_send_is_safe_only_when_non_delivery_is_proven() -> Result<()> {
        let Some(url) =
            db_url_or_skip("timeout_before_send_is_safe_only_when_non_delivery_is_proven")
        else {
            return Ok(());
        };
        let run_id: Uuid = T1_RUN_ID.parse().unwrap();
        let idem = "exe04r-timeout-before-send";
        let pool = prep(&url, run_id, idem).await?;

        let broker = SubmitErrorBroker {
            err: BrokerError::Transport {
                non_delivery_proven: true,
                detail: "timeout before write with explicit local proof".into(),
            },
        };
        let mut orch = make_orchestrator(pool.clone(), run_id, broker);
        let _ = orch.tick().await.expect_err("must return broker error");

        assert_eq!(
            outbox_status(&pool, idem).await?.as_deref(),
            Some("PENDING")
        );
        let run = mqk_db::fetch_run(&pool, run_id).await?;
        assert!(!matches!(run.status, mqk_db::RunStatus::Halted));
        cleanup_run(&pool, run_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn timeout_after_send_is_ambiguous_and_quarantines() -> Result<()> {
        let Some(url) = db_url_or_skip("timeout_after_send_is_ambiguous_and_quarantines") else {
            return Ok(());
        };
        let run_id: Uuid = T2_RUN_ID.parse().unwrap();
        let idem = "exe04r-timeout-after-send";
        let pool = prep(&url, run_id, idem).await?;

        let broker = SubmitErrorBroker {
            err: BrokerError::AmbiguousSubmit {
                detail: "timeout after bytes sent".into(),
            },
        };
        let mut orch = make_orchestrator(pool.clone(), run_id, broker);
        let _ = orch.tick().await.expect_err("must return broker error");

        assert_eq!(
            outbox_status(&pool, idem).await?.as_deref(),
            Some("AMBIGUOUS")
        );
        assert_halted_and_disarmed(&pool, run_id, "AmbiguousSubmit").await?;
        cleanup_run(&pool, run_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn connect_refusal_is_safe_only_when_pre_send_is_proven() -> Result<()> {
        let Some(url) = db_url_or_skip("connect_refusal_is_safe_only_when_pre_send_is_proven")
        else {
            return Ok(());
        };

        // Proven pre-send refusal => safe reset to PENDING.
        let run_id_safe: Uuid = T3_RUN_ID.parse().unwrap();
        let idem_safe = "exe04r-connect-refused-safe";
        let pool_safe = prep(&url, run_id_safe, idem_safe).await?;
        let mut safe_orch = make_orchestrator(
            pool_safe.clone(),
            run_id_safe,
            SubmitErrorBroker {
                err: BrokerError::Transport {
                    non_delivery_proven: true,
                    detail: "ECONNREFUSED before send".into(),
                },
            },
        );
        let _ = safe_orch
            .tick()
            .await
            .expect_err("must return broker error");
        assert_eq!(
            outbox_status(&pool_safe, idem_safe).await?.as_deref(),
            Some("PENDING")
        );

        // Unproven transport => fail closed as AMBIGUOUS + halt/disarm.
        let run_id_unsafe: Uuid = T4_RUN_ID.parse().unwrap();
        let idem_unsafe = "exe04r-connect-refused-unproven";
        let pool_unsafe = prep(&url, run_id_unsafe, idem_unsafe).await?;
        let mut unsafe_orch = make_orchestrator(
            pool_unsafe.clone(),
            run_id_unsafe,
            SubmitErrorBroker {
                err: BrokerError::Transport {
                    non_delivery_proven: false,
                    detail: "socket error; contact not disproven".into(),
                },
            },
        );
        let _ = unsafe_orch
            .tick()
            .await
            .expect_err("must return broker error");
        assert_eq!(
            outbox_status(&pool_unsafe, idem_unsafe).await?.as_deref(),
            Some("AMBIGUOUS")
        );
        assert_halted_and_disarmed(&pool_unsafe, run_id_unsafe, "AmbiguousSubmit").await?;

        cleanup_run(&pool_safe, run_id_safe).await?;
        cleanup_run(&pool_unsafe, run_id_unsafe).await?;
        Ok(())
    }

    #[tokio::test]
    async fn delayed_broker_ack_does_not_get_treated_as_safe_local_failure() -> Result<()> {
        let Some(url) =
            db_url_or_skip("delayed_broker_ack_does_not_get_treated_as_safe_local_failure")
        else {
            return Ok(());
        };
        let run_id: Uuid = T5A_RUN_ID.parse().unwrap();
        let idem = "exe04r-delayed-ack";
        let pool = prep(&url, run_id, idem).await?;

        let broker = SubmitErrorBroker {
            err: BrokerError::AmbiguousSubmit {
                detail: "ack delayed beyond submit timeout".into(),
            },
        };
        let mut orch = make_orchestrator(pool.clone(), run_id, broker);
        let _ = orch.tick().await.expect_err("must return broker error");

        assert_eq!(
            outbox_status(&pool, idem).await?.as_deref(),
            Some("AMBIGUOUS")
        );
        assert_halted_and_disarmed(&pool, run_id, "AmbiguousSubmit").await?;
        cleanup_run(&pool, run_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn rate_limit_retry_window_is_handled_honestly() -> Result<()> {
        let Some(url) = db_url_or_skip("rate_limit_retry_window_is_handled_honestly") else {
            return Ok(());
        };

        // Explicit non-delivery proof => retryable.
        let run_id_safe: Uuid = T5B_RUN_ID.parse().unwrap();
        let idem_safe = "exe04r-ratelimit-safe";
        let pool_safe = prep(&url, run_id_safe, idem_safe).await?;
        let mut safe_orch = make_orchestrator(
            pool_safe.clone(),
            run_id_safe,
            SubmitErrorBroker {
                err: BrokerError::RateLimit {
                    retry_after_ms: Some(250),
                    non_delivery_proven: true,
                    detail: "429 rejected pre-processing".into(),
                },
            },
        );
        let _ = safe_orch
            .tick()
            .await
            .expect_err("must return broker error");
        assert_eq!(
            outbox_status(&pool_safe, idem_safe).await?.as_deref(),
            Some("PENDING")
        );

        // Non-delivery unproven => ambiguous quarantine.
        let run_id_unsafe = Uuid::parse_str("29210007-0000-0000-0000-000000000000").unwrap();
        let idem_unsafe = "exe04r-ratelimit-unproven";
        let pool_unsafe = prep(&url, run_id_unsafe, idem_unsafe).await?;
        let mut unsafe_orch = make_orchestrator(
            pool_unsafe.clone(),
            run_id_unsafe,
            SubmitErrorBroker {
                err: BrokerError::RateLimit {
                    retry_after_ms: Some(250),
                    non_delivery_proven: false,
                    detail: "429 but adapter cannot prove non-delivery".into(),
                },
            },
        );
        let _ = unsafe_orch
            .tick()
            .await
            .expect_err("must return broker error");
        assert_eq!(
            outbox_status(&pool_unsafe, idem_unsafe).await?.as_deref(),
            Some("AMBIGUOUS")
        );
        assert_halted_and_disarmed(&pool_unsafe, run_id_unsafe, "AmbiguousSubmit").await?;

        cleanup_run(&pool_safe, run_id_safe).await?;
        cleanup_run(&pool_unsafe, run_id_unsafe).await?;
        Ok(())
    }
}
