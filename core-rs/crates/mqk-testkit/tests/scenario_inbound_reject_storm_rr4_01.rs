//! Scenario: Inbound Broker-Reject Storm Authority — RR4
//! (RUNTIME-RISK-INBOUND-REJECT-AUTHORITY-01)
//!
//! # Defect under repair
//!
//! Prior to this patch, `mqk-execution`'s reject-storm protection only
//! observed a SYNCHRONOUS submit-time `BrokerError::Reject` (recorded inside
//! `BrokerGateway::submit_with_context`). The real system's canonical
//! inbound broker-rejection lifecycle is asynchronous and polled:
//!
//!     Alpaca "rejected" -> normalize_trade_update -> BrokerEvent::Reject
//!     -> Phase 2 durable oms_inbox -> Phase 3 apply
//!
//! and that path never recorded a reject-storm hit at all. A run that
//! accumulated N broker-confirmed rejects purely through the inbound event
//! path could keep submitting new orders forever — reject-storm protection
//! was silently blind to its most realistic real-world trigger.
//!
//! # What this test proves
//!
//! Drives the REAL `mqk_runtime::orchestrator::ExecutionOrchestrator` (not a
//! direct `mqk_risk::evaluate` call, not repeated direct `BrokerGateway`
//! calls) through `tick()` against a disposable `mqk_test` Postgres
//! database, with a scripted hermetic broker adapter that ACCEPTS every
//! submit (so the execution loop keeps running) and reports
//! `BrokerEvent::Reject` for those same orders only via `fetch_events` —
//! exactly the asynchronous/polled shape the real Alpaca inbound lane uses.
//!
//! Sequence (mirrors the mission's required acceptance sequence exactly):
//! 1. Submit 3 legitimate current-run orders across 3 ticks.
//! 2. The hermetic broker accepts each submit.
//! 3. `fetch_events` returns a canonical `BrokerEvent::Reject` for each.
//! 4. Phase 2 persists them to the durable `oms_inbox`.
//! 5. Phase 3 applies them and records each into the SAME risk engine's
//!    reject-storm window exactly once (via `BrokerGateway::record_broker_reject`,
//!    newly wired from `ExecutionOrchestrator`'s Phase 3 apply loop).
//! 6. After the 3rd (== configured threshold), a 4th legitimate order is
//!    queued.
//! 7. The next `tick()` denies it at the risk GATE (`GateRefusal::RiskBlocked`)
//!    BEFORE the broker adapter's `submit_order` is invoked again.
//!
//! Negative controls folded into the same run (see inline comments at each
//! step): a duplicate redelivery of the FIRST reject (same
//! `broker_message_id`) is injected alongside the second tick's fresh
//! reject — if it had double-counted, the 3rd order's own submit (evaluated
//! at count=2, BEFORE its own reject applies) would have been wrongly
//! denied; the test asserts it succeeds, which is only possible if the
//! duplicate did not count.
//!
//! A second, independent test proves an inbound `BrokerEvent::Reject` for an
//! order this run never submitted (no current-run OMS ownership — e.g. a
//! historical/orphan broker message) contributes zero to the count.
//!
//! Skips gracefully (like every other DB-backed scenario in this crate) when
//! `MQK_DATABASE_URL` is not set.

#[cfg(test)]
mod db_tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use anyhow::Result;
    use chrono::{DateTime, Utc};
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use sqlx::PgPool;
    use uuid::Uuid;

    use mqk_db::FixedClock as DbFixedClock;
    use mqk_execution::gateway::{BrokerGateway, IntegrityGate, ReconcileGate};
    use mqk_execution::{
        BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerEvent, BrokerInvokeToken,
        BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
        BrokerSubmitResponse,
    };
    use mqk_portfolio::PortfolioState;
    use mqk_risk::{RiskConfig, RiskState};
    use mqk_runtime::orchestrator::ExecutionOrchestrator;
    use mqk_runtime::runtime_risk::{
        AccountAuthorityContext, AccountAuthorityError, RiskPdtContext, RuntimeAccountAuthority,
        RuntimeClock, RuntimeRiskGate,
    };

    const RUN_ID_MAIN: &str = "29200010-0000-0000-0000-000000000000";
    const RUN_ID_ORPHAN: &str = "29200011-0000-0000-0000-000000000000";

    // -----------------------------------------------------------------------
    // Deterministic test doubles
    // -----------------------------------------------------------------------

    /// Fixed wall-clock so day/reject-window derivation cannot flake across a
    /// real minute boundary during the test run.
    struct FixedRuntimeClock(DateTime<Utc>);
    impl RuntimeClock for FixedRuntimeClock {
        fn now_utc(&self) -> DateTime<Utc> {
            self.0
        }
    }

    /// Always reports a fixed positive equity — daily-loss/max-drawdown are
    /// disabled in this scenario's `RiskConfig` (this test is about
    /// reject-storm authority only), so the exact figure is immaterial.
    struct FixedAccountAuthority;
    impl RuntimeAccountAuthority for FixedAccountAuthority {
        fn current_account(
            &self,
            _now: DateTime<Utc>,
        ) -> Result<AccountAuthorityContext, AccountAuthorityError> {
            Ok(AccountAuthorityContext {
                equity_micros: 100_000 * 1_000_000,
                pdt: RiskPdtContext::ok(),
                kill_switch: None,
            })
        }
    }

    struct PassGate;
    impl IntegrityGate for PassGate {
        fn is_armed(&self) -> bool {
            true
        }
    }
    impl ReconcileGate for PassGate {
        fn is_clean(&self) -> bool {
            true
        }
    }

    /// Hermetic broker: ACCEPTS every submit unconditionally (so the
    /// execution loop's outbox row transitions to SENT and the order enters
    /// `oms_orders`, exactly like a real broker that will reject
    /// asynchronously rather than synchronously). `fetch_events` returns
    /// whatever the test has queued via `queue_event` — giving the test
    /// exact control over the asynchronous/polled inbound lane, independent
    /// of what was actually submitted (needed for the orphan-reject control).
    #[derive(Clone)]
    struct ScriptedBroker {
        submit_count: Arc<AtomicU32>,
        queued_events: Arc<Mutex<VecDeque<BrokerEvent>>>,
    }
    impl ScriptedBroker {
        fn new() -> Self {
            Self {
                submit_count: Arc::new(AtomicU32::new(0)),
                queued_events: Arc::new(Mutex::new(VecDeque::new())),
            }
        }
        fn queue_event(&self, ev: BrokerEvent) {
            self.queued_events.lock().unwrap().push_back(ev);
        }
        fn submit_count(&self) -> u32 {
            self.submit_count.load(Ordering::SeqCst)
        }
    }
    impl BrokerAdapter for ScriptedBroker {
        fn submit_order(
            &self,
            req: BrokerSubmitRequest,
            _token: &BrokerInvokeToken,
        ) -> Result<BrokerSubmitResponse, BrokerError> {
            self.submit_count.fetch_add(1, Ordering::SeqCst);
            Ok(BrokerSubmitResponse {
                broker_order_id: format!("b-{}", req.order_id),
                submitted_at: 1,
                status: "accepted".to_string(),
            })
        }
        fn cancel_order(
            &self,
            order_id: &str,
            _token: &BrokerInvokeToken,
        ) -> Result<BrokerCancelResponse, BrokerError> {
            Ok(BrokerCancelResponse {
                broker_order_id: order_id.to_string(),
                cancelled_at: 1,
                status: "ok".to_string(),
            })
        }
        fn replace_order(
            &self,
            req: BrokerReplaceRequest,
            _token: &BrokerInvokeToken,
        ) -> Result<BrokerReplaceResponse, BrokerError> {
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
        ) -> Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
            let events: Vec<BrokerEvent> = self.queued_events.lock().unwrap().drain(..).collect();
            Ok((events, None))
        }
    }

    fn reject_event(internal_id: &str, msg_id: &str) -> BrokerEvent {
        BrokerEvent::Reject {
            broker_message_id: msg_id.to_string(),
            internal_order_id: internal_id.to_string(),
            broker_order_id: None,
        }
    }

    // -----------------------------------------------------------------------
    // Harness helpers (mirrors scenario_broker_error_taxonomy.rs)
    // -----------------------------------------------------------------------

    async fn require_pool(url: &str) -> PgPool {
        PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(url)
            .await
            .unwrap_or_else(|e| panic!("RR4-DB: cannot connect to DB: {e}"))
    }

    async fn cleanup_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
        // broker_order_map has no FK cascade from runs — a run whose order
        // reached SENT (registered a broker_order_map row) must have that
        // row deleted first or the runs delete below violates
        // fk_broker_map_outbox_idempotency.
        sqlx::query(
            r#"
            delete from broker_order_map
            where internal_id in (
                select idempotency_key from oms_outbox where run_id = $1
            )
            "#,
        )
        .bind(run_id)
        .execute(pool)
        .await?;
        sqlx::query("delete from runs where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    async fn cleanup_runtime_lease(pool: &PgPool) -> Result<()> {
        sqlx::query("delete from runtime_leader_lease where id = 1")
            .execute(pool)
            .await?;
        Ok(())
    }

    async fn seed_running_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
        mqk_db::insert_run(
            pool,
            &mqk_db::NewRun {
                run_id,
                engine_id: "rr4-test".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: Utc::now(),
                git_hash: "rr4-test".to_string(),
                config_hash: "rr4-test".to_string(),
                config_json: json!({}),
                host_fingerprint: "rr4-test".to_string(),
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
        broker: ScriptedBroker,
        reject_storm_max_rejects_in_window: u32,
    ) -> ExecutionOrchestrator<ScriptedBroker, PassGate, RuntimeRiskGate, PassGate, DbFixedClock>
    {
        let clock: Arc<dyn RuntimeClock> =
            Arc::new(FixedRuntimeClock(chrono::DateTime::parse_from_rfc3339(
                "2026-01-15T09:00:00Z",
            )
            .unwrap()
            .with_timezone(&Utc)));
        let risk_gate = RuntimeRiskGate::for_test(
            RiskConfig {
                daily_loss_limit_micros: 0,
                max_drawdown_limit_micros: 0,
                reject_storm_max_rejects_in_window,
                pdt_auto_enabled: false,
                missing_protective_stop_flattens: false,
            },
            RiskState::new(20_260_115, 100_000 * 1_000_000, 540),
            Arc::new(FixedAccountAuthority),
            clock,
        );
        let gateway = BrokerGateway::for_test(broker, PassGate, risk_gate, PassGate);
        ExecutionOrchestrator::new(
            pool,
            gateway,
            BrokerOrderMap::new(),
            BTreeMap::new(),
            PortfolioState::new(0),
            run_id,
            "rr4-dispatcher",
            "test",
            None,
            DbFixedClock::new(Utc::now()),
            Box::new(mqk_reconcile::LocalSnapshot::empty),
            Box::new(|| mqk_reconcile::BrokerSnapshot::empty_at(1)),
        )
    }

    async fn enqueue_order(pool: &PgPool, run_id: Uuid, idem: &str) -> Result<()> {
        let created = mqk_db::outbox_enqueue(
            pool,
            run_id,
            idem,
            json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
        )
        .await?;
        assert!(created, "outbox row {idem} must be created");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Main load-bearing test: real ExecutionOrchestrator, real RuntimeRiskGate,
    // real durable oms_inbox, scripted hermetic broker.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rr4_inbound_reject_storm_denies_next_order_before_broker_submit() -> Result<()> {
        let url = match std::env::var(mqk_db::ENV_DB_URL) {
            Ok(v) if !v.trim().is_empty() => v,
            _ => {
                eprintln!(
                    "SKIP rr4_inbound_reject_storm_denies_next_order_before_broker_submit: \
                     MQK_DATABASE_URL not set"
                );
                return Ok(());
            }
        };
        let pool = require_pool(&url).await;
        mqk_db::migrate(&pool).await?;

        let run_id: Uuid = RUN_ID_MAIN.parse().unwrap();
        cleanup_run(&pool, run_id).await?;
        cleanup_runtime_lease(&pool).await?;
        seed_running_run(&pool, run_id).await?;

        let broker = ScriptedBroker::new();
        let mut orch = make_orchestrator(pool.clone(), run_id, broker.clone(), 3);

        // --- Tick 1: submit r0, broker accepts, inbound Reject applies. ---
        enqueue_order(&pool, run_id, "rr4-r0").await?;
        broker.queue_event(reject_event("rr4-r0", "rr4-reject-r0"));
        orch.tick().await.expect("tick 1 (accept + inbound reject) must not error");
        assert_eq!(broker.submit_count(), 1, "r0 must have reached the broker");

        // --- Tick 2: submit r1, broker accepts, inbound Reject applies.
        //     Also redeliver r0's Reject message verbatim (same
        //     broker_message_id) — a duplicate/replayed inbound event. If
        //     this were double-counted, the window would already be at 2
        //     real + 1 duplicate = 3 BEFORE r2 is even submitted in tick 3,
        //     and r2's own submit (evaluated at count>=3) would be denied
        //     instead of succeeding. ---
        enqueue_order(&pool, run_id, "rr4-r1").await?;
        broker.queue_event(reject_event("rr4-r0", "rr4-reject-r0")); // duplicate
        broker.queue_event(reject_event("rr4-r1", "rr4-reject-r1"));
        orch.tick().await.expect("tick 2 (accept + inbound reject + duplicate) must not error");
        assert_eq!(broker.submit_count(), 2, "r1 must have reached the broker");

        // --- Tick 3: submit r2 — this is the "threshold-1 still permits
        //     normal risk" proof: at submit time the window holds exactly 2
        //     genuine rejects (r0, r1); if the tick-2 duplicate had counted,
        //     this submit would already be denied. It must succeed, and its
        //     own inbound Reject then pushes the window to exactly 3. ---
        enqueue_order(&pool, run_id, "rr4-r2").await?;
        broker.queue_event(reject_event("rr4-r2", "rr4-reject-r2"));
        orch.tick().await.expect("tick 3 (r2 accepted despite 2 prior rejects) must not error");
        assert_eq!(
            broker.submit_count(),
            3,
            "r2 must have reached the broker — the tick-2 duplicate reject must not have \
             prematurely exhausted the reject-storm window"
        );

        // --- Tick 4: queue a new legitimate order. The risk gate must deny
        //     it BEFORE the broker adapter is invoked again — submit_count
        //     must stay at 3. ---
        enqueue_order(&pool, run_id, "rr4-r3").await?;
        let err = orch
            .tick()
            .await
            .expect_err("tick 4 must fail: reject-storm threshold reached");
        let msg = err.to_string();
        assert!(
            msg.contains("risk engine") || msg.contains("SUBMIT_GATE_REFUSED"),
            "tick 4 error must be a risk-gate refusal, got: {msg}"
        );
        assert_eq!(
            broker.submit_count(),
            3,
            "the broker adapter must NOT be invoked once the reject-storm threshold denies — \
             this is the exact RR4 acceptance proof"
        );

        // Direct confirmation this was specifically a RiskBlocked gate
        // refusal (not some other failure): `dispatch_submit_claimed_outbox_row`
        // marks the outbox row FAILED specifically on
        // `GateRefusal::RiskBlocked` (see orchestrator/dispatch.rs) — every
        // other error class has a different disposition (PENDING,
        // AMBIGUOUS, or a halt).
        let status: Option<(String,)> =
            sqlx::query_as("select status from oms_outbox where idempotency_key = $1")
                .bind("rr4-r3")
                .fetch_optional(&pool)
                .await?;
        assert_eq!(
            status.map(|(s,)| s).as_deref(),
            Some("FAILED"),
            "r3's outbox row must be marked FAILED via the RiskBlocked disposition path"
        );

        cleanup_run(&pool, run_id).await?;
        cleanup_runtime_lease(&pool).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Negative control: an inbound Reject for an order this run never
    // submitted (no current-run OMS ownership) must contribute zero to the
    // reject-storm window.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rr4_orphan_reject_with_no_run_ownership_does_not_count() -> Result<()> {
        let url = match std::env::var(mqk_db::ENV_DB_URL) {
            Ok(v) if !v.trim().is_empty() => v,
            _ => {
                eprintln!(
                    "SKIP rr4_orphan_reject_with_no_run_ownership_does_not_count: \
                     MQK_DATABASE_URL not set"
                );
                return Ok(());
            }
        };
        let pool = require_pool(&url).await;
        mqk_db::migrate(&pool).await?;

        let run_id: Uuid = RUN_ID_ORPHAN.parse().unwrap();
        cleanup_run(&pool, run_id).await?;
        cleanup_runtime_lease(&pool).await?;
        seed_running_run(&pool, run_id).await?;

        // Threshold of 1: a single counted reject would deny the very next
        // new-risk order — the tightest possible proof that the orphan
        // event below counts for nothing.
        let broker = ScriptedBroker::new();
        let mut orch = make_orchestrator(pool.clone(), run_id, broker.clone(), 1);

        // Inbound Reject for an internal_order_id NEVER submitted by this
        // run (no outbox row, absent from oms_orders) — simulates a
        // historical/orphan broker message (e.g. REST polling returning a
        // reject for a prior run's order).
        broker.queue_event(reject_event("orphan-order-never-submitted", "orphan-msg-1"));
        // A tick with nothing claimed still runs Phase 2/3 against whatever
        // fetch_events returns.
        orch.tick().await.expect("tick with only an orphan inbound reject must not error");

        // Now submit ONE real order. With threshold=1, if the orphan reject
        // had counted, this submit would already be denied at the gate.
        enqueue_order(&pool, run_id, "rr4-orphan-check-r0").await?;
        orch.tick().await.expect(
            "the legitimate order must be accepted — the orphan reject must not have \
             counted toward this run's reject-storm window",
        );
        assert_eq!(broker.submit_count(), 1, "the legitimate order must have reached the broker");

        cleanup_run(&pool, run_id).await?;
        cleanup_runtime_lease(&pool).await?;
        Ok(())
    }
}
