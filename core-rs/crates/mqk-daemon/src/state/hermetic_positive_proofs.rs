//! FULL-AUDIT-FAIL-016 / FULL-AUDIT-FAIL-018 — private hermetic positive-path
//! proofs.
//!
//! `#[cfg(test)]`-only, `pub(crate)`-reachable-only in-crate module. Exists
//! because the positive lifecycle/registry/order scenarios these tests prove
//! require driving `start_execution_runtime` -> `build_execution_orchestrator`
//! all the way to a real `200 OK`, which for a LiveShadow+Alpaca (or an
//! ambient-env-resolved Alpaca) fixture would otherwise either require real
//! `ALPACA_API_KEY_LIVE`/`ALPACA_API_KEY_PAPER` credentials or make a genuine
//! network call to Alpaca's real host. The daemon crate already has a private
//! test-only seam for exactly this (`AppState::
//! set_hermetic_test_broker_override_for_test`, `#[cfg(test)]` +
//! `pub(crate)`, wired into `build_execution_orchestrator` in
//! `orchestrator_build.rs`) — but `pub(crate)` means it is only reachable
//! from *this* crate's own test build, not from an external `tests/*.rs`
//! integration test (those are separate crates). That is why these scenarios
//! must live here instead of in `tests/`.
//!
//! ## Coverage map (one-to-one replacement of the external positive tests)
//!
//! | Retired external test (still present, `#[ignore]`, blocked without real creds) | Replaced by |
//! |---|---|
//! | `scenario_native_strategy_bootstrap_daemon_b1a.rs::b1a_l04_start_with_registered_strategy_stores_active_bootstrap` | `hermetic_b1a_l04_start_with_registered_strategy_stores_active_bootstrap` below |
//! | `scenario_native_strategy_bootstrap_daemon_b1a.rs::b1a_l05_stop_clears_native_strategy_bootstrap` | `hermetic_b1a_l05_stop_clears_native_strategy_bootstrap` below |
//! | `scenario_native_strategy_bootstrap_daemon_b1a.rs::b1a_l06_halt_clears_native_strategy_bootstrap` | `hermetic_b1a_l06_halt_clears_native_strategy_bootstrap` below |
//! | `scenario_native_strategy_registry_b2a.rs::b2a_n02_registry_enabled_allows_activation` | `hermetic_b2a_n02_registry_enabled_allows_activation` below |
//! | `scenario_daemon_order_submit.rs::manual_order_submit_refuses_when_durable_arm_state_is_disarmed_even_if_local_state_is_armed` | `hermetic_order_submit_refuses_when_durable_arm_state_is_disarmed` below |
//! | `scenario_daemon_order_submit.rs::manual_order_submit_refuses_when_durable_arm_state_is_halted_even_if_local_state_is_armed` | `hermetic_order_submit_refuses_when_durable_arm_state_is_halted` below |
//! | `scenario_daemon_order_submit.rs::manual_order_submit_enqueues_one_pending_outbox_row_for_active_run` | `hermetic_order_submit_enqueues_one_pending_outbox_row` below |
//! | `scenario_daemon_order_submit.rs::manual_order_submit_duplicate_client_request_id_is_noop` | `hermetic_order_submit_duplicate_client_request_id_is_noop` below |
//! | `scenario_daemon_order_submit.rs::manual_order_submit_accepts_limit_order_with_explicit_defaults_aligned_to_runtime` | `hermetic_order_submit_accepts_limit_order` below |
//!
//! The external `#[ignore]`d originals are left in place (not deleted) as
//! documentation of the credential-gated boundary and as a manual escape
//! hatch for an operator who *does* want to run them against a real Alpaca
//! account; per FULL-AUDIT scope they are never run by CI or by any default
//! command.
//!
//! ## Isolation
//!
//! Each test below uses its own disposable database
//! (`mqk_db::run_isolated`/`mqk_db::create_disposable_test_db`) rather than
//! the shared `MQK_DATABASE_URL` database, so there is no dependency on
//! singleton-row cleanup ordering (FULL-AUDIT-FAIL-017 discipline applied to
//! new tests from the start).
//!
//! ## Network-deny witness
//!
//! `hermetic_override_without_seeded_snapshot_makes_no_network_call` below is
//! the FULL-AUDIT-FAIL-018 requirement-9 proof. It deliberately does *not*
//! pre-seed `broker_snapshot`, forcing `build_execution_orchestrator`'s
//! `BrokerSnapshotTruthSource::External` branch past its cache check into the
//! `match &daemon_broker` arm — and asserts the result is the purely local,
//! deterministic `broker_snapshot_source_mismatch` fault, completing well
//! under a network-timeout bound, never a network-shaped error. This is
//! stronger than a loopback-URL trap: with the override enabled,
//! `daemon_broker` is the `DaemonBroker::Paper` variant, so there is no
//! `AlpacaBrokerAdapter` value in memory at all for the `Self::Alpaca(adapter)
//! => adapter.fetch_broker_snapshot(...)` arm to match against — the
//! network-capable code path is unreachable by construction, not merely
//! unreachable because a URL is wrong.
//!
//! A literal loopback-URL trap (as suggested by the originating patch
//! request) was deliberately not used for the LiveShadow+Alpaca fixtures:
//! `alpaca_base_url_for_mode` hardcodes the live endpoint for
//! `DeploymentMode::LiveShadow`/`LiveCapital` by explicit, tested design
//! (`env_truth_02_alpaca_live_base_url_env_var_is_not_authoritative`,
//! ENV-TRUTH-02) — an operator-set env var must never redirect live-capital
//! order flow. Adding an override would weaken that existing, deliberate,
//! tested safety invariant, which this patch must preserve
//! ("no trading-economic semantic change"; "preserve... all production
//! fail-closed... broker gates").

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::routes;
    use crate::state::{AppState, BrokerKind, DeploymentMode, StrategyFleetEntry};

    async fn call(
        router: axum::Router,
        req: Request<axum::body::Body>,
    ) -> (StatusCode, serde_json::Value) {
        let resp = router.oneshot(req).await.expect("oneshot failed");
        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .expect("body collect failed")
            .to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    fn fleet_entry(strategy_id: &str) -> StrategyFleetEntry {
        StrategyFleetEntry {
            strategy_id: strategy_id.to_string(),
        }
    }

    fn fake_broker_snapshot() -> mqk_schemas::BrokerSnapshot {
        mqk_schemas::BrokerSnapshot {
            captured_at_utc: chrono::Utc::now(),
            account: mqk_schemas::BrokerAccount {
                equity: "100000".to_string(),
                cash: "100000".to_string(),
                currency: "USD".to_string(),
            },
            orders: vec![],
            fills: vec![],
            positions: vec![],
        }
    }

    /// Enables the private hermetic broker override and pre-seeds
    /// `broker_snapshot` (a `pub` field, the established idiom used by ~15
    /// other test files) so `build_execution_orchestrator` neither
    /// constructs a real Alpaca client nor attempts any network fetch.
    async fn enable_hermetic_broker_with_seeded_snapshot(st: &Arc<AppState>) {
        st.set_hermetic_test_broker_override_for_test(true).await;
        *st.broker_snapshot.write().await = Some(fake_broker_snapshot());
    }

    async fn armed_live_shadow_state(pool: sqlx::PgPool) -> Arc<AppState> {
        let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
            pool,
            DeploymentMode::LiveShadow,
            BrokerKind::Alpaca,
        ));
        enable_hermetic_broker_with_seeded_snapshot(&st).await;
        let arm_req = Request::builder()
            .method("POST")
            .uri("/v1/integrity/arm")
            .body(axum::body::Body::empty())
            .unwrap();
        let (status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "arm must succeed in hermetic test setup"
        );
        st
    }

    async fn seed_swing_momentum_registry(pool: &sqlx::PgPool) {
        let now = chrono::Utc::now();
        mqk_db::upsert_strategy_registry_entry(
            pool,
            &mqk_db::UpsertStrategyRegistryArgs {
                strategy_id: "swing_momentum".to_string(),
                display_name: "Swing Momentum".to_string(),
                enabled: true,
                kind: "native".to_string(),
                registered_at_utc: now,
                updated_at_utc: now,
                note: String::new(),
            },
        )
        .await
        .expect("seed_swing_momentum_registry: upsert must succeed");
    }

    // -----------------------------------------------------------------
    // B1A L04-L06 — start/stop/halt clears/stores native_strategy_bootstrap
    // -----------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hermetic_b1a_l04_start_with_registered_strategy_stores_active_bootstrap() {
        mqk_db::run_isolated("hermetic_b1a_l04", |pool| async move {
            seed_swing_momentum_registry(&pool).await;
            let st = armed_live_shadow_state(pool).await;
            st.set_strategy_fleet_for_test(Some(vec![fleet_entry("swing_momentum")]))
                .await;

            assert!(
                st.native_strategy_bootstrap_truth_state_for_test()
                    .await
                    .is_none(),
                "L04: bootstrap must be None before start"
            );

            let start_req = Request::builder()
                .method("POST")
                .uri("/v1/run/start")
                .body(axum::body::Body::empty())
                .unwrap();
            let (status, json) = call(routes::build_router(Arc::clone(&st)), start_req).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "L04: start must succeed; got: {json}"
            );

            let truth = st
                .native_strategy_bootstrap_truth_state_for_test()
                .await
                .expect("L04: bootstrap must be Some after successful start");
            assert_eq!(
                truth, "active",
                "L04: bootstrap truth_state must be 'active'"
            );

            let stop_req = Request::builder()
                .method("POST")
                .uri("/v1/run/stop")
                .body(axum::body::Body::empty())
                .unwrap();
            let _ = call(routes::build_router(Arc::clone(&st)), stop_req).await;
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hermetic_b1a_l05_stop_clears_native_strategy_bootstrap() {
        mqk_db::run_isolated("hermetic_b1a_l05", |pool| async move {
            seed_swing_momentum_registry(&pool).await;
            let st = armed_live_shadow_state(pool).await;
            st.set_strategy_fleet_for_test(Some(vec![fleet_entry("swing_momentum")]))
                .await;

            let start_req = Request::builder()
                .method("POST")
                .uri("/v1/run/start")
                .body(axum::body::Body::empty())
                .unwrap();
            let (status, json) = call(routes::build_router(Arc::clone(&st)), start_req).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "L05: start must succeed; got: {json}"
            );

            let before_stop = st
                .native_strategy_bootstrap_truth_state_for_test()
                .await
                .expect("L05: bootstrap must be Some after start");
            assert_eq!(before_stop, "active");

            let stop_req = Request::builder()
                .method("POST")
                .uri("/v1/run/stop")
                .body(axum::body::Body::empty())
                .unwrap();
            let (stop_status, stop_json) =
                call(routes::build_router(Arc::clone(&st)), stop_req).await;
            assert_eq!(
                stop_status,
                StatusCode::OK,
                "L05: stop must succeed; got: {stop_json}"
            );

            let after_stop = st.native_strategy_bootstrap_truth_state_for_test().await;
            assert!(
                after_stop.is_none(),
                "L05: bootstrap must be None after stop; got: {after_stop:?}"
            );
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hermetic_b1a_l06_halt_clears_native_strategy_bootstrap() {
        mqk_db::run_isolated("hermetic_b1a_l06", |pool| async move {
            seed_swing_momentum_registry(&pool).await;
            let st = armed_live_shadow_state(pool).await;
            st.set_strategy_fleet_for_test(Some(vec![fleet_entry("swing_momentum")]))
                .await;

            let start_req = Request::builder()
                .method("POST")
                .uri("/v1/run/start")
                .body(axum::body::Body::empty())
                .unwrap();
            let (status, json) = call(routes::build_router(Arc::clone(&st)), start_req).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "L06: start must succeed; got: {json}"
            );

            let before_halt = st
                .native_strategy_bootstrap_truth_state_for_test()
                .await
                .expect("L06: bootstrap must be Some after start");
            assert_eq!(before_halt, "active");

            let halt_req = Request::builder()
                .method("POST")
                .uri("/v1/run/halt")
                .body(axum::body::Body::empty())
                .unwrap();
            let (halt_status, halt_json) =
                call(routes::build_router(Arc::clone(&st)), halt_req).await;
            assert_eq!(
                halt_status,
                StatusCode::OK,
                "L06: halt must succeed; got: {halt_json}"
            );

            let after_halt = st.native_strategy_bootstrap_truth_state_for_test().await;
            assert!(
                after_halt.is_none(),
                "L06: bootstrap must be None after halt; got: {after_halt:?}"
            );
        })
        .await;
    }

    // -----------------------------------------------------------------
    // B2A N02 — registry enabled=true allows activation (200 start)
    // -----------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hermetic_b2a_n02_registry_enabled_allows_activation() {
        mqk_db::run_isolated("hermetic_b2a_n02", |pool| async move {
            seed_swing_momentum_registry(&pool).await;
            let st = armed_live_shadow_state(pool).await;
            st.set_strategy_fleet_for_test(Some(vec![fleet_entry("swing_momentum")]))
                .await;

            let start_req = Request::builder()
                .method("POST")
                .uri("/v1/run/start")
                .body(axum::body::Body::empty())
                .unwrap();
            let (status, json) = call(routes::build_router(Arc::clone(&st)), start_req).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "N02: swing_momentum with enabled registry row must start successfully; got: {json}"
            );

            let stop_req = Request::builder()
                .method("POST")
                .uri("/v1/run/stop")
                .body(axum::body::Body::empty())
                .unwrap();
            let _ = call(routes::build_router(Arc::clone(&st)), stop_req).await;
        })
        .await;
    }

    // -----------------------------------------------------------------
    // Order-submit positives (scenario_daemon_order_submit.rs originals 21-25)
    // -----------------------------------------------------------------

    fn valid_order_request() -> serde_json::Value {
        serde_json::json!({
            "client_request_id": "manual-order-001",
            "symbol": "AAPL",
            "side": "buy",
            "qty": 10,
        })
    }

    async fn hermetic_order_daemon_state(pool: sqlx::PgPool) -> Arc<AppState> {
        // Explicit LiveShadow+Alpaca, not the ambient-env-resolved default
        // (Paper+Paper, refused at the deployment_mode gate itself as "not an
        // honest paper trading path" before any broker is constructed) and
        // not Paper+Alpaca (Paper mode with an Alpaca adapter resolves
        // strategy_market_data_source to ExternalSignalIngestion, which
        // brings the daily_data_readiness gate into play -- see
        // FULL-AUDIT-FAIL-011 -- and these order-submit fixtures set no
        // MQK_STRATEGY_SYMBOL, so that gate would always refuse). LiveShadow
        // bypasses both: deployment_mode-honest for +Alpaca, and
        // daily_data_readiness only applies to Paper mode. The hermetic
        // override (below) is what avoids needing real Alpaca credentials.
        let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
            pool,
            DeploymentMode::LiveShadow,
            BrokerKind::Alpaca,
        ));
        enable_hermetic_broker_with_seeded_snapshot(&st).await;
        {
            let mut execution = st.execution_snapshot.write().await;
            *execution = Some(mqk_runtime::observability::ExecutionSnapshot {
                run_id: None,
                active_orders: vec![],
                pending_outbox: vec![],
                recent_inbox_events: vec![],
                portfolio: mqk_runtime::observability::PortfolioSnapshot {
                    cash_micros: 0,
                    realized_pnl_micros: 0,
                    positions: vec![],
                },
                system_block_state: None,
                recent_risk_denials: vec![],
                snapshot_at_utc: chrono::Utc::now(),
                has_recent_terminal_fill: false,
                risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
            });
        }
        st
    }

    async fn arm(st: &Arc<AppState>) {
        let req = Request::builder()
            .method("POST")
            .uri("/v1/integrity/arm")
            .body(axum::body::Body::empty())
            .unwrap();
        let (status, json) = call(routes::build_router(Arc::clone(st)), req).await;
        assert_eq!(status, StatusCode::OK, "arm failed: {json}");
    }

    async fn start(st: &Arc<AppState>) -> serde_json::Value {
        let req = Request::builder()
            .method("POST")
            .uri("/v1/run/start")
            .body(axum::body::Body::empty())
            .unwrap();
        let (status, json) = call(routes::build_router(Arc::clone(st)), req).await;
        assert_eq!(status, StatusCode::OK, "start failed: {json}");
        json
    }

    async fn post_manual_order(
        st: &Arc<AppState>,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/execution/orders")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        call(routes::build_router(Arc::clone(st)), req).await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hermetic_order_submit_refuses_when_durable_arm_state_is_disarmed() {
        mqk_db::run_isolated("hermetic_order_disarmed", |pool| async move {
            let st = hermetic_order_daemon_state(pool).await;
            arm(&st).await;
            start(&st).await;

            let pool = st.db.as_ref().expect("db configured");
            mqk_db::persist_arm_state(pool, "DISARMED", Some("IntegrityViolation"))
                .await
                .expect("persist durable disarmed state");

            let (status, json) = post_manual_order(&st, valid_order_request()).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(json["accepted"], false);
            assert_eq!(json["disposition"], "rejected");

            st.stop_for_shutdown().await;
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hermetic_order_submit_refuses_when_durable_arm_state_is_halted() {
        mqk_db::run_isolated("hermetic_order_halted", |pool| async move {
            let st = hermetic_order_daemon_state(pool).await;
            arm(&st).await;
            start(&st).await;

            let pool = st.db.as_ref().expect("db configured");
            mqk_db::persist_arm_state(pool, "DISARMED", Some("OperatorHalt"))
                .await
                .expect("persist durable halted state");

            let (status, json) = post_manual_order(&st, valid_order_request()).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(json["accepted"], false);
            assert_eq!(json["disposition"], "rejected");

            st.stop_for_shutdown().await;
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hermetic_order_submit_enqueues_one_pending_outbox_row() {
        mqk_db::run_isolated("hermetic_order_enqueue", |pool| async move {
            let st = hermetic_order_daemon_state(pool).await;
            arm(&st).await;
            let started = start(&st).await;
            let run_id =
                uuid::Uuid::parse_str(started["active_run_id"].as_str().expect("run_id string"))
                    .expect("valid run uuid");

            let (status, json) = post_manual_order(&st, valid_order_request()).await;
            assert_eq!(status, StatusCode::OK, "submit failed: {json}");
            assert_eq!(json["accepted"], true);
            assert_eq!(json["disposition"], "enqueued");
            assert_eq!(json["active_run_id"], run_id.to_string());

            let pool = st.db.as_ref().expect("db configured");
            let row = mqk_db::outbox_fetch_by_idempotency_key(pool, "manual-order-001")
                .await
                .expect("fetch outbox row")
                .expect("outbox row present");
            assert_eq!(row.run_id, run_id);
            // The hermetic override makes this a genuinely live orchestrator
            // loop (real LockedPaperBroker, real tick), unlike the retired
            // external test this replaces (which never actually reached this
            // assertion). CLAIMED means the live loop picked the row up
            // before this check ran -- a stronger proof of a working
            // hermetic path, not a false pass -- so both states are honest
            // here; only a state proving the row was lost/rejected would not
            // be.
            assert!(
                row.status == "PENDING" || row.status == "CLAIMED",
                "expected PENDING or CLAIMED (live orchestrator may have already claimed it); got: {}",
                row.status
            );
            assert_eq!(row.order_json["symbol"], "AAPL");

            st.stop_for_shutdown().await;
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hermetic_order_submit_duplicate_client_request_id_is_noop() {
        mqk_db::run_isolated("hermetic_order_dup", |pool| async move {
            let st = hermetic_order_daemon_state(pool).await;
            arm(&st).await;
            start(&st).await;

            let (first_status, first_json) = post_manual_order(&st, valid_order_request()).await;
            assert_eq!(
                first_status,
                StatusCode::OK,
                "first submit failed: {first_json}"
            );
            assert_eq!(first_json["disposition"], "enqueued");

            let (second_status, second_json) = post_manual_order(&st, valid_order_request()).await;
            assert_eq!(
                second_status,
                StatusCode::OK,
                "duplicate submit failed: {second_json}"
            );
            assert_eq!(second_json["accepted"], false);
            assert_eq!(second_json["disposition"], "duplicate");

            let pool = st.db.as_ref().expect("db configured");
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*)::bigint FROM oms_outbox WHERE idempotency_key = $1",
            )
            .bind("manual-order-001")
            .fetch_one(pool)
            .await
            .expect("count outbox rows");
            assert_eq!(
                count, 1,
                "duplicate client_request_id must not create a second row"
            );

            st.stop_for_shutdown().await;
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hermetic_order_submit_accepts_limit_order() {
        mqk_db::run_isolated("hermetic_order_limit", |pool| async move {
            let st = hermetic_order_daemon_state(pool).await;
            arm(&st).await;
            start(&st).await;

            let (status, json) = post_manual_order(
                &st,
                serde_json::json!({
                    "client_request_id": "manual-order-limit-001",
                    "symbol": "MSFT",
                    "side": "sell",
                    "qty": "25",
                    "order_type": "limit",
                    "time_in_force": "gtc",
                    "limit_price": "123450000",
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "limit submit failed: {json}");
            assert_eq!(json["disposition"], "enqueued");

            let pool = st.db.as_ref().expect("db configured");
            let row = mqk_db::outbox_fetch_by_idempotency_key(pool, "manual-order-limit-001")
                .await
                .expect("fetch limit row")
                .expect("limit row present");
            assert_eq!(row.order_json["symbol"], "MSFT");
            assert_eq!(row.order_json["order_type"], "limit");

            st.stop_for_shutdown().await;
        })
        .await;
    }

    // -----------------------------------------------------------------
    // PAPER-SOAK-STALE-CLAIM-RECOVERY-02: production wiring proof.
    // -----------------------------------------------------------------

    /// `build_execution_orchestrator` must NOT reset stale `CLAIMED` rows on
    /// construction.
    ///
    /// PAPER-SOAK-STALE-CLAIM-RECOVERY-01 wired an unconditional
    /// `outbox_reset_stale_claims` call into this constructor, proven (only)
    /// by this test's predecessor calling `build_execution_orchestrator`
    /// directly and asserting the stale row flipped to `PENDING`. Independent
    /// review rejected that repair: the call ran before any runtime
    /// leadership lease existed for the orchestrator being constructed (the
    /// lease is acquired later, inside `tick()`), so it had no ownership
    /// proof that no other legitimate dispatcher could be concurrently
    /// active — and the exact crash-recovery scenario it targeted can never
    /// reach this constructor via the normal start path anyway (see
    /// `scenario_stale_claim_recovery_02.rs`'s
    /// `reachability_crashed_running_run_blocks_normal_start_before_orchestrator_build`).
    ///
    /// -02 moves stale-claim recovery to the operator-mediated
    /// `clear-halted-run` action (`mqk_db::
    /// clear_halted_run_and_reset_stale_claims`), gated on the run's durable
    /// `HALTED` status as the ownership proof. This test now proves the
    /// negative: constructing an orchestrator for a run that still has a
    /// stale `CLAIMED` row must leave that row untouched — confirming the
    /// unsafe unconditional reset was actually removed, not just relocated
    /// under a different name.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hermetic_build_execution_orchestrator_does_not_reset_stale_claim_on_construction() {
        mqk_db::run_isolated("hermetic_stale_claim_build", |pool| async move {
            let st = hermetic_order_daemon_state(pool.clone()).await;

            let run_id = uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_DNS,
                b"mqk.test.hermetic_build_execution_orchestrator_does_not_reset_stale_claim_on_construction",
            );
            mqk_db::insert_run(
                &pool,
                &mqk_db::NewRun {
                    run_id,
                    engine_id: "mqk-daemon".to_string(),
                    mode: DeploymentMode::LiveShadow.as_db_mode().to_string(),
                    started_at_utc: chrono::Utc::now(),
                    git_hash: "TEST".to_string(),
                    config_hash: "test".to_string(),
                    config_json: serde_json::json!({}),
                    host_fingerprint: "test-node".to_string(),
                },
            )
            .await
            .expect("insert_run failed");
            mqk_db::arm_run(&pool, run_id)
                .await
                .expect("arm_run failed");
            mqk_db::begin_run(&pool, run_id)
                .await
                .expect("begin_run failed");

            let idem = "stale-claim-restart-proof";
            mqk_db::outbox_enqueue(
                &pool,
                run_id,
                idem,
                serde_json::json!({"symbol": "AAPL", "qty": 1}),
            )
            .await
            .expect("outbox_enqueue failed");
            mqk_db::outbox_claim_batch(
                &pool,
                1,
                "crashed-dispatcher",
                chrono::Utc::now() - chrono::Duration::minutes(10),
            )
            .await
            .expect("outbox_claim_batch failed");

            let before = mqk_db::outbox_fetch_by_idempotency_key(&pool, idem)
                .await
                .expect("fetch failed")
                .expect("row must exist");
            assert_eq!(
                before.status, "CLAIMED",
                "precondition: row must be CLAIMED before construction"
            );

            let _orchestrator = st
                .build_execution_orchestrator(pool.clone(), run_id)
                .await
                .expect("build_execution_orchestrator must succeed");

            let after = mqk_db::outbox_fetch_by_idempotency_key(&pool, idem)
                .await
                .expect("fetch failed")
                .expect("row must exist");
            assert_eq!(
                after.status, "CLAIMED",
                "orchestrator construction must NOT reset a stale claim — that authority \
                 now belongs exclusively to clear_halted_run_and_reset_stale_claims, gated \
                 on durable HALTED status, not to unconditional construction"
            );
            assert!(after.claimed_by.is_some());
            assert!(after.claimed_at_utc.is_some());
        })
        .await;
    }

    // -----------------------------------------------------------------
    // Network-deny witness (FULL-AUDIT-FAIL-018 requirement 9)
    // -----------------------------------------------------------------

    /// Proves the hermetic override makes no external network call even in
    /// the worst case (no pre-seeded snapshot to short-circuit the fetch).
    /// See module header for why this is used instead of a loopback-URL trap.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hermetic_override_without_seeded_snapshot_makes_no_network_call() {
        mqk_db::run_isolated("hermetic_network_deny", |pool| async move {
            seed_swing_momentum_registry(&pool).await;
            let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
                pool,
                DeploymentMode::LiveShadow,
                BrokerKind::Alpaca,
            ));
            // Enable the override but do NOT seed broker_snapshot: this
            // forces build_execution_orchestrator's External branch past its
            // cache check into `match &daemon_broker`, where the override has
            // already made daemon_broker a DaemonBroker::Paper value -- there
            // is no Alpaca adapter in memory to call, so the `_ =>` arm fires
            // a local, deterministic error instead of any network I/O.
            st.set_hermetic_test_broker_override_for_test(true).await;

            let arm_req = Request::builder()
                .method("POST")
                .uri("/v1/integrity/arm")
                .body(axum::body::Body::empty())
                .unwrap();
            let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
            assert_eq!(arm_status, StatusCode::OK);
            st.set_strategy_fleet_for_test(Some(vec![fleet_entry("swing_momentum")]))
                .await;

            let start_req = Request::builder()
                .method("POST")
                .uri("/v1/run/start")
                .body(axum::body::Body::empty())
                .unwrap();

            // Bounded well under any plausible network timeout/retry budget:
            // a real network attempt against a live host would not resolve
            // (success or failure) this fast under this crate's HTTP client
            // configuration. Completing quickly is part of the network-deny
            // proof, not just the returned fault_class.
            let (status, json) = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                call(routes::build_router(Arc::clone(&st)), start_req),
            )
            .await
            .expect(
                "network-deny witness: start must resolve almost immediately \
                 (a real network attempt would not)",
            );

            assert_eq!(
                status,
                StatusCode::INTERNAL_SERVER_ERROR,
                "network-deny witness: must fail with a local, deterministic error; got: {json}"
            );
            assert_eq!(
                json["fault_class"], "runtime.start_refused.broker_snapshot_source_mismatch",
                "network-deny witness: must be the local snapshot-source mismatch, \
                 never a network-shaped error; got: {json}"
            );
        })
        .await;
    }
}
