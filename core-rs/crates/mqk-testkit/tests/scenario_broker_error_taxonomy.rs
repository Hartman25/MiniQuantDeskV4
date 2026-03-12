//! Scenario: Broker Error Taxonomy — Patch A3
//!
//! # Invariants under test
//!
//! A3 introduces a typed `BrokerError` enum as the return type of all four
//! `BrokerAdapter` methods. The enum drives per-class outbox row disposition:
//!
//! | Variant         | safe_pre_send_retry | ambiguous_outcome | Outbox action         |
//! |-----------------|:-------------------:|:-----------------:|-----------------------|
//! | AmbiguousSubmit | false        | true          | row → AMBIGUOUS+halt (A4)  |
//! | Reject          | false        | false         | mark FAILED                |
//! | Transient       | false        | false         | mark FAILED (conservative) |
//! | RateLimit(proven) | true       | false         | reset to PENDING           |
//! | RateLimit(unknown) | false     | true          | row → AMBIGUOUS+halt       |
//! | AuthSession     | false        | true          | FAILED + halt              |
//! | Transport       | true         | false         | reset to PENDING           |
//!
//! # Test layout
//!
//! ## Pure (no DB required)
//! - A1: `is_retryable()` and `requires_halt()` return the correct values for
//!   every `BrokerError` variant.
//! - A2: `SubmitError` Display includes the inner error detail string.
//!
//! ## DB-backed (skipped unless `MQK_DATABASE_URL` is set)
//! - B1: orchestrator tick with `Reject` broker → outbox row FAILED, run not halted.
//! - B2: orchestrator tick with `Transport` broker → outbox row reset to PENDING.
//! - B3: orchestrator tick with `AmbiguousSubmit` broker → row moves to AMBIGUOUS
//!   (A4 explicit quarantine), run HALTED+DISARMED with reason AmbiguousSubmit.
//! - B4: orchestrator tick with `AuthSession` broker → row FAILED, run HALTED+DISARMED.

// ---------------------------------------------------------------------------
// Pure tests — A1, A2
// ---------------------------------------------------------------------------

use mqk_execution::{BrokerError, GateRefusal, SubmitError};

#[test]
fn a1_safe_pre_send_retry_correct_per_variant() {
    assert!(!BrokerError::AmbiguousSubmit { detail: "x".into() }.is_safe_pre_send_retry());
    assert!(!BrokerError::Reject {
        code: "c".into(),
        detail: "x".into()
    }
    .is_safe_pre_send_retry());
    assert!(!BrokerError::Transient { detail: "x".into() }.is_safe_pre_send_retry());
    assert!(BrokerError::RateLimit {
        retry_after_ms: Some(1000),
        non_delivery_proven: true,
        detail: "x".into()
    }
    .is_safe_pre_send_retry());
    assert!(!BrokerError::RateLimit {
        retry_after_ms: Some(1000),
        non_delivery_proven: false,
        detail: "x".into()
    }
    .is_safe_pre_send_retry());
    assert!(!BrokerError::AuthSession { detail: "x".into() }.is_safe_pre_send_retry());
    assert!(BrokerError::Transport { detail: "x".into() }.is_safe_pre_send_retry());
}

#[test]
fn a1_ambiguous_send_outcome_correct_per_variant() {
    assert!(BrokerError::AmbiguousSubmit { detail: "x".into() }.is_ambiguous_send_outcome());
    assert!(!BrokerError::Reject {
        code: "c".into(),
        detail: "x".into()
    }
    .is_ambiguous_send_outcome());
    assert!(BrokerError::RateLimit {
        retry_after_ms: None,
        non_delivery_proven: false,
        detail: "x".into()
    }
    .is_ambiguous_send_outcome());
    assert!(!BrokerError::RateLimit {
        retry_after_ms: None,
        non_delivery_proven: true,
        detail: "x".into()
    }
    .is_ambiguous_send_outcome());
}

#[test]
fn a1_requires_halt_correct_per_variant() {
    assert!(BrokerError::AmbiguousSubmit { detail: "x".into() }.requires_halt());
    assert!(!BrokerError::Reject {
        code: "c".into(),
        detail: "x".into()
    }
    .requires_halt());
    assert!(!BrokerError::Transient { detail: "x".into() }.requires_halt());
    assert!(!BrokerError::RateLimit {
        retry_after_ms: None,
        non_delivery_proven: true,
        detail: "x".into()
    }
    .requires_halt());
    assert!(BrokerError::AuthSession { detail: "x".into() }.requires_halt());
    assert!(!BrokerError::Transport { detail: "x".into() }.requires_halt());
}

#[test]
fn a2_submit_error_display_includes_inner_detail() {
    let gate_err = SubmitError::Gate(GateRefusal::IntegrityDisarmed);
    let s = gate_err.to_string();
    assert!(
        s.contains("integrity disarmed"),
        "expected 'integrity disarmed' in '{s}'"
    );

    let broker_err = SubmitError::Broker(BrokerError::Transport {
        detail: "connection refused".into(),
    });
    let s = broker_err.to_string();
    assert!(
        s.contains("connection refused"),
        "expected 'connection refused' in '{s}'"
    );

    let reject_err = SubmitError::Broker(BrokerError::Reject {
        code: "ERR_SYMBOL".into(),
        detail: "unknown symbol FOO".into(),
    });
    let s = reject_err.to_string();
    assert!(
        s.contains("unknown symbol FOO"),
        "expected 'unknown symbol FOO' in '{s}'"
    );
}

// ---------------------------------------------------------------------------
// DB-backed tests — B1–B4
// Skip gracefully when MQK_DATABASE_URL is not set.
// ---------------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Fixed run UUIDs — one per test for deterministic cleanup
    // -----------------------------------------------------------------------

    const B1_RUN_ID: &str = "29200003-0000-0000-0000-000000000000";
    const B2_RUN_ID: &str = "29200004-0000-0000-0000-000000000000";
    const B3_RUN_ID: &str = "29200005-0000-0000-0000-000000000000";
    const B4_RUN_ID: &str = "29200006-0000-0000-0000-000000000000";
    const B5_RUN_ID: &str = "29200007-0000-0000-0000-000000000000";
    const B6_RUN_ID: &str = "29200008-0000-0000-0000-000000000000";

    // -----------------------------------------------------------------------
    // Gate stubs — all pass
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
    // Error-injecting broker stubs
    // -----------------------------------------------------------------------

    struct RejectBroker;
    impl BrokerAdapter for RejectBroker {
        fn submit_order(
            &self,
            _req: BrokerSubmitRequest,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
            Err(BrokerError::Reject {
                code: "ORDER_REJECTED".into(),
                detail: "test reject".into(),
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

    struct TransportBroker;
    impl BrokerAdapter for TransportBroker {
        fn submit_order(
            &self,
            _req: BrokerSubmitRequest,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
            Err(BrokerError::Transport {
                detail: "test transport error".into(),
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

    struct AmbiguousBroker;
    impl BrokerAdapter for AmbiguousBroker {
        fn submit_order(
            &self,
            _req: BrokerSubmitRequest,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
            Err(BrokerError::AmbiguousSubmit {
                detail: "test ambiguous submit".into(),
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

    struct AuthSessionBroker;
    impl BrokerAdapter for AuthSessionBroker {
        fn submit_order(
            &self,
            _req: BrokerSubmitRequest,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
            Err(BrokerError::AuthSession {
                detail: "test auth session expired".into(),
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

    struct DelayedAckBroker;
    impl BrokerAdapter for DelayedAckBroker {
        fn submit_order(
            &self,
            _req: BrokerSubmitRequest,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
            Err(BrokerError::AmbiguousSubmit {
                detail: "delayed broker ack window; submit outcome unknown".into(),
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

    struct RateLimitAmbiguousBroker;
    impl BrokerAdapter for RateLimitAmbiguousBroker {
        fn submit_order(
            &self,
            _req: BrokerSubmitRequest,
            _token: &BrokerInvokeToken,
        ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
            Err(BrokerError::RateLimit {
                retry_after_ms: Some(1_000),
                non_delivery_proven: false,
                detail: "rate-limit encountered after uncertain submit boundary".into(),
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
    // Harness helpers
    // -----------------------------------------------------------------------

    async fn require_pool(url: &str) -> PgPool {
        PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(url)
            .await
            .unwrap_or_else(|e| panic!("A3-DB: cannot connect to DB: {e}"))
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
                engine_id: "a3-test".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: Utc::now(),
                git_hash: "a3-test".to_string(),
                config_hash: "a3-test".to_string(),
                config_json: json!({}),
                host_fingerprint: "a3-test".to_string(),
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
            "a3-dispatcher",
            "test",
            None,
            FixedClock::new(Utc::now()),
            Box::new(mqk_reconcile::LocalSnapshot::empty),
            Box::new(mqk_reconcile::BrokerSnapshot::empty),
        )
    }

    async fn outbox_status(pool: &PgPool, idem_key: &str) -> Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("select status from oms_outbox where idempotency_key = $1")
                .bind(idem_key)
                .fetch_optional(pool)
                .await?;
        Ok(row.map(|(s,)| s))
    }

    // -----------------------------------------------------------------------
    // B1: Reject broker → row FAILED, run not halted.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn b1_reject_broker_marks_row_failed_no_halt() -> Result<()> {
        let url = match std::env::var(mqk_db::ENV_DB_URL) {
            Ok(v) if !v.trim().is_empty() => v,
            _ => {
                eprintln!(
                    "SKIP b1_reject_broker_marks_row_failed_no_halt: MQK_DATABASE_URL not set"
                );
                return Ok(());
            }
        };
        let pool = require_pool(&url).await;
        mqk_db::migrate(&pool).await?;

        let run_id: Uuid = B1_RUN_ID.parse().unwrap();
        let idem = "a3-b1-reject-ord-001";

        cleanup_run(&pool, run_id).await?;
        seed_running_run(&pool, run_id).await?;

        let created = mqk_db::outbox_enqueue(
            &pool,
            run_id,
            idem,
            json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
        )
        .await?;
        assert!(created, "outbox row must be created");

        let mut orch = make_orchestrator(pool.clone(), run_id, RejectBroker);
        let err = orch.tick().await.expect_err("tick must fail on Reject");
        let msg = err.to_string();
        assert!(
            msg.contains("Reject") || msg.contains("SUBMIT_BROKER_ERROR"),
            "error must mention reject, got: {msg}"
        );

        let status = outbox_status(&pool, idem).await?;
        assert_eq!(
            status.as_deref(),
            Some("FAILED"),
            "expected FAILED after Reject, got {status:?}"
        );

        let run = mqk_db::fetch_run(&pool, run_id).await?;
        assert!(
            !matches!(run.status, mqk_db::RunStatus::Halted),
            "Reject must not halt the run"
        );

        cleanup_run(&pool, run_id).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // B2: Transport broker → row reset to PENDING.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn b2_transport_broker_resets_row_to_pending() -> Result<()> {
        let url = match std::env::var(mqk_db::ENV_DB_URL) {
            Ok(v) if !v.trim().is_empty() => v,
            _ => {
                eprintln!(
                    "SKIP b2_transport_broker_resets_row_to_pending: MQK_DATABASE_URL not set"
                );
                return Ok(());
            }
        };
        let pool = require_pool(&url).await;
        mqk_db::migrate(&pool).await?;

        let run_id: Uuid = B2_RUN_ID.parse().unwrap();
        let idem = "a3-b2-transport-ord-001";

        cleanup_run(&pool, run_id).await?;
        seed_running_run(&pool, run_id).await?;

        let created = mqk_db::outbox_enqueue(
            &pool,
            run_id,
            idem,
            json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
        )
        .await?;
        assert!(created, "outbox row must be created");

        let mut orch = make_orchestrator(pool.clone(), run_id, TransportBroker);
        let err = orch.tick().await.expect_err("tick must fail on Transport");
        let msg = err.to_string();
        assert!(
            msg.contains("Transport") || msg.contains("SUBMIT_BROKER_ERROR"),
            "error must mention transport, got: {msg}"
        );

        let status = outbox_status(&pool, idem).await?;
        assert_eq!(
            status.as_deref(),
            Some("PENDING"),
            "expected PENDING after Transport, got {status:?}"
        );

        cleanup_run(&pool, run_id).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // B3: AmbiguousSubmit broker → row moves to AMBIGUOUS, run HALTED+DISARMED.
    //     A4: explicit quarantine state; arm reason = "AmbiguousSubmit".
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn b3_ambiguous_submit_row_ambiguous_run_halted() -> Result<()> {
        let url = match std::env::var(mqk_db::ENV_DB_URL) {
            Ok(v) if !v.trim().is_empty() => v,
            _ => {
                eprintln!(
                    "SKIP b3_ambiguous_submit_row_dispatching_run_halted: MQK_DATABASE_URL not set"
                );
                return Ok(());
            }
        };
        let pool = require_pool(&url).await;
        mqk_db::migrate(&pool).await?;

        let run_id: Uuid = B3_RUN_ID.parse().unwrap();
        let idem = "a3-b3-ambiguous-ord-001";

        cleanup_run(&pool, run_id).await?;
        sqlx::query("delete from sys_arm_state where sentinel_id = 1")
            .execute(&pool)
            .await?;

        seed_running_run(&pool, run_id).await?;

        let created = mqk_db::outbox_enqueue(
            &pool,
            run_id,
            idem,
            json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
        )
        .await?;
        assert!(created, "outbox row must be created");

        let mut orch = make_orchestrator(pool.clone(), run_id, AmbiguousBroker);
        let err = orch
            .tick()
            .await
            .expect_err("tick must fail on AmbiguousSubmit");
        let msg = err.to_string();
        assert!(
            msg.contains("AmbiguousSubmit")
                || msg.contains("SUBMIT_BROKER_ERROR")
                || msg.contains("RECOVERY_QUARANTINE"),
            "error must mention ambiguous submit or halt, got: {msg}"
        );

        // A4: Row must be AMBIGUOUS — explicit quarantine, outcome unknown.
        let status = outbox_status(&pool, idem).await?;
        assert_eq!(
            status.as_deref(),
            Some("AMBIGUOUS"),
            "expected AMBIGUOUS after AmbiguousSubmit (A4 quarantine), got {status:?}"
        );

        // Run must be HALTED.
        let run = mqk_db::fetch_run(&pool, run_id).await?;
        assert!(
            matches!(run.status, mqk_db::RunStatus::Halted),
            "expected run HALTED after AmbiguousSubmit"
        );

        // Arm state must be DISARMED with reason "AmbiguousSubmit" (A4/migration 0020).
        let arm = mqk_db::load_arm_state(&pool).await?;
        assert_eq!(
            arm.as_ref().map(|(s, _)| s.as_str()),
            Some("DISARMED"),
            "expected DISARMED after AmbiguousSubmit, got {arm:?}"
        );
        assert_eq!(
            arm.as_ref().and_then(|(_, r)| r.as_deref()),
            Some("AmbiguousSubmit"),
            "expected disarm reason AmbiguousSubmit, got {arm:?}"
        );

        cleanup_run(&pool, run_id).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // B4: AuthSession broker → row FAILED, run HALTED+DISARMED.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn b4_auth_session_row_failed_run_halted() -> Result<()> {
        let url = match std::env::var(mqk_db::ENV_DB_URL) {
            Ok(v) if !v.trim().is_empty() => v,
            _ => {
                eprintln!("SKIP b4_auth_session_row_failed_run_halted: MQK_DATABASE_URL not set");
                return Ok(());
            }
        };
        let pool = require_pool(&url).await;
        mqk_db::migrate(&pool).await?;

        let run_id: Uuid = B4_RUN_ID.parse().unwrap();
        let idem = "a3-b4-auth-ord-001";

        cleanup_run(&pool, run_id).await?;
        sqlx::query("delete from sys_arm_state where sentinel_id = 1")
            .execute(&pool)
            .await?;

        seed_running_run(&pool, run_id).await?;

        let created = mqk_db::outbox_enqueue(
            &pool,
            run_id,
            idem,
            json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
        )
        .await?;
        assert!(created, "outbox row must be created");

        let mut orch = make_orchestrator(pool.clone(), run_id, AuthSessionBroker);
        let err = orch
            .tick()
            .await
            .expect_err("tick must fail on AuthSession");
        let msg = err.to_string();
        assert!(
            msg.contains("AuthSession") || msg.contains("SUBMIT_BROKER_ERROR"),
            "error must mention auth session, got: {msg}"
        );

        // Row must be FAILED.
        let status = outbox_status(&pool, idem).await?;
        assert_eq!(
            status.as_deref(),
            Some("FAILED"),
            "expected FAILED after AuthSession, got {status:?}"
        );

        // Run must be HALTED.
        let run = mqk_db::fetch_run(&pool, run_id).await?;
        assert!(
            matches!(run.status, mqk_db::RunStatus::Halted),
            "expected run HALTED after AuthSession"
        );

        // Arm state must be DISARMED.
        let arm = mqk_db::load_arm_state(&pool).await?;
        assert_eq!(
            arm.as_ref().map(|(s, _)| s.as_str()),
            Some("DISARMED"),
            "expected DISARMED after AuthSession, got {arm:?}"
        );

        cleanup_run(&pool, run_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn delayed_broker_ack_does_not_get_treated_as_safe_local_failure() -> Result<()> {
        let url = match std::env::var(mqk_db::ENV_DB_URL) {
            Ok(v) if !v.trim().is_empty() => v,
            _ => {
                eprintln!("SKIP delayed_broker_ack_does_not_get_treated_as_safe_local_failure: MQK_DATABASE_URL not set");
                return Ok(());
            }
        };
        let pool = require_pool(&url).await;
        mqk_db::migrate(&pool).await?;

        let run_id: Uuid = B5_RUN_ID.parse().unwrap();
        let idem = "a3-b5-delayed-ack-ord-001";
        cleanup_run(&pool, run_id).await?;
        sqlx::query("delete from sys_arm_state where sentinel_id = 1")
            .execute(&pool)
            .await?;
        seed_running_run(&pool, run_id).await?;

        let created = mqk_db::outbox_enqueue(
            &pool,
            run_id,
            idem,
            json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
        )
        .await?;
        assert!(created, "outbox row must be created");

        let mut orch = make_orchestrator(pool.clone(), run_id, DelayedAckBroker);
        let _ = orch
            .tick()
            .await
            .expect_err("tick must fail closed on delayed-ack ambiguity");

        let status = outbox_status(&pool, idem).await?;
        assert_eq!(status.as_deref(), Some("AMBIGUOUS"));

        let run = mqk_db::fetch_run(&pool, run_id).await?;
        assert!(matches!(run.status, mqk_db::RunStatus::Halted));

        cleanup_run(&pool, run_id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn rate_limit_retry_window_is_handled_honestly() -> Result<()> {
        let url = match std::env::var(mqk_db::ENV_DB_URL) {
            Ok(v) if !v.trim().is_empty() => v,
            _ => {
                eprintln!(
                    "SKIP rate_limit_retry_window_is_handled_honestly: MQK_DATABASE_URL not set"
                );
                return Ok(());
            }
        };
        let pool = require_pool(&url).await;
        mqk_db::migrate(&pool).await?;

        // Proved non-delivery rate limit -> safe reset to PENDING.
        let run_id_safe: Uuid = B2_RUN_ID.parse().unwrap();
        let idem_safe = "a3-b6-ratelimit-safe-ord-001";
        cleanup_run(&pool, run_id_safe).await?;
        seed_running_run(&pool, run_id_safe).await?;
        let created = mqk_db::outbox_enqueue(
            &pool,
            run_id_safe,
            idem_safe,
            json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
        )
        .await?;
        assert!(created);
        struct RateLimitSafeBroker;
        impl BrokerAdapter for RateLimitSafeBroker {
            fn submit_order(
                &self,
                _req: BrokerSubmitRequest,
                _token: &BrokerInvokeToken,
            ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
                Err(BrokerError::RateLimit {
                    retry_after_ms: Some(1_000),
                    non_delivery_proven: true,
                    detail: "pre-admission throttle".into(),
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
        let mut orch = make_orchestrator(pool.clone(), run_id_safe, RateLimitSafeBroker);
        let _ = orch
            .tick()
            .await
            .expect_err("safe ratelimit returns submit error and resets row");
        let status_safe = outbox_status(&pool, idem_safe).await?;
        assert_eq!(status_safe.as_deref(), Some("PENDING"));

        // Unknown-delivery rate limit -> ambiguous + halt/disarm.
        let run_id_amb: Uuid = B6_RUN_ID.parse().unwrap();
        let idem_amb = "a3-b6-ratelimit-amb-ord-001";
        cleanup_run(&pool, run_id_amb).await?;
        sqlx::query("delete from sys_arm_state where sentinel_id = 1")
            .execute(&pool)
            .await?;
        seed_running_run(&pool, run_id_amb).await?;
        let created = mqk_db::outbox_enqueue(
            &pool,
            run_id_amb,
            idem_amb,
            json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
        )
        .await?;
        assert!(created);
        let mut orch = make_orchestrator(pool.clone(), run_id_amb, RateLimitAmbiguousBroker);
        let _ = orch
            .tick()
            .await
            .expect_err("ambiguous ratelimit must fail closed");
        let status_amb = outbox_status(&pool, idem_amb).await?;
        assert_eq!(status_amb.as_deref(), Some("AMBIGUOUS"));

        let run = mqk_db::fetch_run(&pool, run_id_amb).await?;
        assert!(matches!(run.status, mqk_db::RunStatus::Halted));

        cleanup_run(&pool, run_id_safe).await?;
        cleanup_run(&pool, run_id_amb).await?;
        Ok(())
    }
}
