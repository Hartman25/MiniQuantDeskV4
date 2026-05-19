//! RECOVERY-QUARANTINE-AFTER-PAPER-FILL-01 — proof that a legitimately filled
//! order does NOT trigger RECOVERY_QUARANTINE on the next orchestrator tick.
//!
//! # Root cause
//!
//! Before this fix, the apply path called `broker_map_remove` when a terminal
//! fill was applied, but never advanced `oms_outbox.status` from `SENT` to
//! `ACKED`.  On the very next tick, Phase 0b
//! (`outbox_load_restart_ambiguous_for_run`) found a `SENT` row with no
//! `broker_order_map` entry and incorrectly triggered `RECOVERY_QUARANTINE`.
//!
//! # Fix
//!
//! `outbox_mark_acked` is now called immediately before `broker_map_remove`
//! when `terminal_apply_succeeded` is true.  The `ACKED` status is excluded
//! from the ambiguous-row query, so a completed order no longer triggers
//! false quarantine.
//!
//! # Tests (DB-backed, skip gracefully without `MQK_DATABASE_URL`)
//!
//! - RQA01: Submit + ack + fill (applied in Phase 3) → outbox transitions to
//!   ACKED; second tick does NOT quarantine.  (Main proof of the fix.)
//! - RQA02: SENT row without broker_order_map still quarantines.
//!   (Regression — fail-closed behavior must be preserved.)
//! - RQA03: Duplicate fill (second WS copy) is idempotent and does NOT
//!   quarantine after the order is already ACKED.

#[cfg(test)]
mod db_tests {
    use std::collections::BTreeMap;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex as StdMutex, OnceLock,
    };

    use anyhow::Result;
    use chrono::Utc;
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use sqlx::PgPool;
    use tokio::sync::{Mutex, MutexGuard};
    use uuid::Uuid;

    use mqk_db::FixedClock;
    use mqk_execution::{
        BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerEvent, BrokerGateway,
        BrokerInvokeToken, BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse,
        BrokerSubmitRequest, BrokerSubmitResponse, IntegrityGate, ReconcileGate, RiskGate, Side,
    };
    use mqk_portfolio::PortfolioState;
    use mqk_runtime::orchestrator::ExecutionOrchestrator;

    // -----------------------------------------------------------------------
    // Fixed run UUIDs — one per scenario for deterministic cleanup.
    // -----------------------------------------------------------------------

    const RQA01_RUN_ID: &str = "29200400-0000-0000-0000-000000000000";
    const RQA02_RUN_ID: &str = "29200401-0000-0000-0000-000000000000";
    const RQA03_RUN_ID: &str = "29200402-0000-0000-0000-000000000000";

    static RQA_DB_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    async fn acquire_rqa_lock() -> MutexGuard<'static, ()> {
        RQA_DB_LOCK.get_or_init(|| Mutex::new(())).lock().await
    }

    // -----------------------------------------------------------------------
    // Gate stubs
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
    // Stateful broker stub
    //
    // Returns a pre-loaded sequence of event batches on fetch_events calls.
    // Once the sequence is exhausted, returns empty.
    // -----------------------------------------------------------------------

    #[derive(Clone)]
    struct ScenarioBroker {
        submits: Arc<AtomicUsize>,
        /// Each inner Vec is the response for one fetch_events call (in order).
        event_queue: Arc<StdMutex<Vec<Vec<BrokerEvent>>>>,
    }

    impl ScenarioBroker {
        fn new(event_batches: Vec<Vec<BrokerEvent>>) -> Self {
            Self {
                submits: Arc::new(AtomicUsize::new(0)),
                event_queue: Arc::new(StdMutex::new(event_batches)),
            }
        }

        fn submit_count(&self) -> usize {
            self.submits.load(Ordering::SeqCst)
        }
    }

    impl BrokerAdapter for ScenarioBroker {
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
        ) -> std::result::Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
            let mut q = self.event_queue.lock().unwrap();
            if q.is_empty() {
                Ok((vec![], None))
            } else {
                Ok((q.remove(0), None))
            }
        }
    }

    // -----------------------------------------------------------------------
    // DB helpers
    // -----------------------------------------------------------------------

    fn skip_without_db() -> Option<String> {
        match std::env::var(mqk_db::ENV_DB_URL) {
            Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
            _ => {
                eprintln!("SKIP: MQK_DATABASE_URL not set — skipping DB-backed RQA tests");
                None
            }
        }
    }

    async fn make_pool(url: &str) -> PgPool {
        PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(url)
            .await
            .expect("cannot connect to DB")
    }

    async fn cleanup(pool: &PgPool, run_id: Uuid) {
        // Remove the run (cascades to outbox/inbox rows via FK).
        let _ = sqlx::query("delete from runs where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
        // Release any runtime lease held by this test's orchestrator.
        // The holder_id is derived the same way as in orchestrator::new():
        // "{dispatcher_id}|run={run_id}".
        let holder_id = format!("rqa-dispatcher|run={run_id}");
        let _ = sqlx::query("delete from runtime_leader_lease where holder_id = $1")
            .bind(&holder_id)
            .execute(pool)
            .await;
        // Reset arm state so tests do not bleed into each other.
        let _ = sqlx::query("delete from sys_arm_state where sentinel_id = 1")
            .execute(pool)
            .await;
    }

    async fn seed_running_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
        mqk_db::insert_run(
            pool,
            &mqk_db::NewRun {
                run_id,
                engine_id: "rqa-test".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: Utc::now(),
                git_hash: "rqa-test".to_string(),
                config_hash: "rqa-test".to_string(),
                config_json: json!({}),
                host_fingerprint: "rqa-test".to_string(),
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
        broker: ScenarioBroker,
    ) -> ExecutionOrchestrator<ScenarioBroker, PassGate, PassGate, PassGate, FixedClock> {
        let gateway = BrokerGateway::for_test(broker, PassGate, PassGate, PassGate);
        ExecutionOrchestrator::new(
            pool,
            gateway,
            BrokerOrderMap::new(),
            BTreeMap::new(),
            PortfolioState::new(1_000_000_000_000), // 1M USD in micros
            run_id,
            "rqa-dispatcher",
            "test-adapter",
            None,
            FixedClock::new(Utc::now()),
            Box::new(mqk_reconcile::LocalSnapshot::empty),
            // Provide a fresh timestamp so the reconcile staleness gate (Patch B2)
            // does not reject the snapshot for having fetched_at_ms=0.
            Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(Utc::now().timestamp_millis())),
        )
    }

    // -----------------------------------------------------------------------
    // RQA01 — submit + ack + fill → ACKED; second tick does NOT quarantine
    //
    // Proves that the fix (outbox_mark_acked before broker_map_remove) prevents
    // false RECOVERY_QUARANTINE for a legitimately filled order.
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn rqa01_fill_applied_does_not_quarantine_on_next_tick() -> Result<()> {
        let Some(url) = skip_without_db() else {
            return Ok(());
        };
        let pool = make_pool(&url).await;
        mqk_db::migrate(&pool).await?;
        let _lock = acquire_rqa_lock().await;

        let run_id: Uuid = RQA01_RUN_ID.parse().unwrap();
        let idem = "rqa01-aapl-buy-001";
        cleanup(&pool, run_id).await;

        seed_running_run(&pool, run_id).await?;
        mqk_db::outbox_enqueue(
            &pool,
            run_id,
            idem,
            json!({
                "symbol": "AAPL",
                "quantity": 1,
                "order_type": "market",
                "time_in_force": "day",
                "side": "buy"
            }),
        )
        .await?;

        // Broker returns ack + fill on the first fetch_events call; empty after.
        let ack_event = BrokerEvent::Ack {
            broker_message_id: format!("alpaca:broker-{idem}:new:2026-05-19T14:14:28Z"),
            internal_order_id: idem.to_string(),
            broker_order_id: Some(format!("broker-{idem}")),
        };
        let fill_event = BrokerEvent::Fill {
            broker_message_id: format!("alpaca:broker-{idem}:fill:2026-05-19T14:14:29Z"),
            broker_fill_id: Some(format!("fill-{idem}")),
            internal_order_id: idem.to_string(),
            broker_order_id: Some(format!("broker-{idem}")),
            symbol: "AAPL".to_string(),
            side: Side::Buy,
            delta_qty: 1,
            price_micros: 297_940_000, // ~$297.94
            fee_micros: 0,
        };

        let broker = ScenarioBroker::new(vec![vec![ack_event, fill_event]]);
        let broker_probe = broker.clone();
        let mut orch = make_orchestrator(pool.clone(), run_id, broker);

        // Tick 1: Phase 1 submits, Phase 2 ingests ack+fill, Phase 3 applies them.
        // With the fix, Phase 3 terminal apply calls outbox_mark_acked + broker_map_remove.
        orch.tick()
            .await
            .expect("tick 1 must succeed — no quarantine during apply");
        assert_eq!(
            broker_probe.submit_count(),
            1,
            "exactly one submit must occur"
        );

        // Verify outbox status advanced to ACKED (not SENT).
        let outbox_row: Option<(String, Option<String>)> = sqlx::query_as(
            "select o.status, m.broker_id from oms_outbox o \
             left join broker_order_map m on m.internal_id = o.idempotency_key \
             where o.idempotency_key = $1",
        )
        .bind(idem)
        .fetch_optional(&pool)
        .await?;

        let (status, broker_map) = outbox_row.expect("outbox row must exist after tick 1");
        assert_eq!(
            status, "ACKED",
            "outbox.status must be ACKED after terminal fill (was SENT before fix)"
        );
        assert!(
            broker_map.is_none(),
            "broker_order_map entry must be removed after terminal fill"
        );

        // Tick 2: Phase 0b must NOT quarantine — the ACKED row is excluded from
        // the ambiguous-row query.
        // Tick 2 may error for other reasons (lease, reconcile) but NOT for RECOVERY_QUARANTINE.
        let tick2_result = orch.tick().await;
        if let Err(e) = &tick2_result {
            let msg = e.to_string();
            assert!(
                !msg.contains("RECOVERY_QUARANTINE"),
                "tick 2 must NOT trigger RECOVERY_QUARANTINE for a legitimately filled order; \
                 got: {msg}"
            );
        }

        // Verify run is NOT halted due to quarantine.
        let run = mqk_db::fetch_run(&pool, run_id).await?;
        assert!(
            !matches!(run.status, mqk_db::RunStatus::Halted),
            "run must NOT be HALTED after a legitimate fill cycle"
        );

        cleanup(&pool, run_id).await;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // RQA02 — truly ambiguous SENT (no broker_map) still quarantines
    //
    // Regression: fail-closed behavior for genuinely ambiguous rows must be
    // preserved after the fix.
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn rqa02_sent_without_broker_map_still_quarantines() -> Result<()> {
        let Some(url) = skip_without_db() else {
            return Ok(());
        };
        let pool = make_pool(&url).await;
        mqk_db::migrate(&pool).await?;
        let _lock = acquire_rqa_lock().await;

        let run_id: Uuid = RQA02_RUN_ID.parse().unwrap();
        let idem = "rqa02-ambiguous-sent-001";
        cleanup(&pool, run_id).await;

        seed_running_run(&pool, run_id).await?;
        mqk_db::outbox_enqueue(
            &pool,
            run_id,
            idem,
            json!({
                "symbol": "SPY",
                "quantity": 1,
                "order_type": "market",
                "time_in_force": "day",
                "side": "buy"
            }),
        )
        .await?;

        // Force CLAIMED → SENT without broker_map (simulates crash between
        // outbox_mark_sent and broker_map_upsert — a legacy / corrupt state
        // that the fix must still fail-close on).
        sqlx::query(
            "update oms_outbox set status = 'SENT', sent_at_utc = $2 \
             where idempotency_key = $1 and status = 'PENDING'",
        )
        .bind(idem)
        .bind(Utc::now())
        .execute(&pool)
        .await?;

        let broker = ScenarioBroker::new(vec![]);
        let mut orch = make_orchestrator(pool.clone(), run_id, broker);

        let err = orch
            .tick()
            .await
            .expect_err("tick must quarantine SENT row without broker_order_map evidence");

        let msg = err.to_string();
        assert!(
            msg.contains("RECOVERY_QUARANTINE"),
            "error must contain RECOVERY_QUARANTINE; got: {msg}"
        );
        assert!(
            msg.contains("SENT"),
            "error must mention the SENT row; got: {msg}"
        );

        let run = mqk_db::fetch_run(&pool, run_id).await?;
        assert!(
            matches!(run.status, mqk_db::RunStatus::Halted),
            "run must be HALTED after quarantine"
        );

        cleanup(&pool, run_id).await;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // RQA03 — duplicate fill after terminal ACKED is idempotent and does not
    //          quarantine
    //
    // Proves that a second WS/REST copy of the same fill (same broker_fill_id
    // or same broker_message_id) does not re-open the SENT ambiguity window.
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn rqa03_duplicate_fill_is_idempotent_and_does_not_quarantine() -> Result<()> {
        let Some(url) = skip_without_db() else {
            return Ok(());
        };
        let pool = make_pool(&url).await;
        mqk_db::migrate(&pool).await?;
        let _lock = acquire_rqa_lock().await;

        let run_id: Uuid = RQA03_RUN_ID.parse().unwrap();
        let idem = "rqa03-dup-fill-001";
        cleanup(&pool, run_id).await;

        seed_running_run(&pool, run_id).await?;
        mqk_db::outbox_enqueue(
            &pool,
            run_id,
            idem,
            json!({
                "symbol": "AAPL",
                "quantity": 1,
                "order_type": "market",
                "time_in_force": "day",
                "side": "buy"
            }),
        )
        .await?;

        // Tick 1: submit + ack + fill (terminal).
        let ack_msg = format!("alpaca:broker-{idem}:new:2026-05-19T14:14:28Z");
        let fill_msg = format!("alpaca:broker-{idem}:fill:2026-05-19T14:14:29Z");
        let broker_oid = format!("broker-{idem}");

        let broker = ScenarioBroker::new(vec![
            // First fetch_events: ack + fill
            vec![
                BrokerEvent::Ack {
                    broker_message_id: ack_msg.clone(),
                    internal_order_id: idem.to_string(),
                    broker_order_id: Some(broker_oid.clone()),
                },
                BrokerEvent::Fill {
                    broker_message_id: fill_msg.clone(),
                    broker_fill_id: Some(format!("fill-{idem}")),
                    internal_order_id: idem.to_string(),
                    broker_order_id: Some(broker_oid.clone()),
                    symbol: "AAPL".to_string(),
                    side: Side::Buy,
                    delta_qty: 1,
                    price_micros: 297_940_000,
                    fee_micros: 0,
                },
            ],
            // Second fetch_events: duplicate fill (same broker_message_id → inbox dedup)
            vec![BrokerEvent::Fill {
                broker_message_id: fill_msg.clone(), // same → inbox_insert_deduped is a no-op
                broker_fill_id: Some(format!("fill-{idem}")),
                internal_order_id: idem.to_string(),
                broker_order_id: Some(broker_oid.clone()),
                symbol: "AAPL".to_string(),
                side: Side::Buy,
                delta_qty: 1,
                price_micros: 297_940_000,
                fee_micros: 0,
            }],
        ]);

        let mut orch = make_orchestrator(pool.clone(), run_id, broker);

        // Tick 1: ack+fill applied; outbox advanced to ACKED.
        orch.tick()
            .await
            .expect("tick 1 must succeed with ack+fill");

        // Tick 2: duplicate fill arrives in Phase 2 but is deduplicated by
        // inbox_insert_deduped (same broker_message_id).  Phase 3 applies
        // no new rows.  Phase 0b must NOT quarantine.
        let tick2_result = orch.tick().await;
        if let Err(e) = &tick2_result {
            let msg = e.to_string();
            assert!(
                !msg.contains("RECOVERY_QUARANTINE"),
                "tick 2 with duplicate fill must NOT trigger RECOVERY_QUARANTINE; got: {msg}"
            );
        }

        // Verify outbox is still ACKED and no regression.
        let row: Option<(String,)> =
            sqlx::query_as("select status from oms_outbox where idempotency_key = $1")
                .bind(idem)
                .fetch_optional(&pool)
                .await?;
        let (status,) = row.expect("outbox row must exist");
        assert_eq!(
            status, "ACKED",
            "outbox status must remain ACKED after duplicate fill tick"
        );

        cleanup(&pool, run_id).await;
        Ok(())
    }
}
