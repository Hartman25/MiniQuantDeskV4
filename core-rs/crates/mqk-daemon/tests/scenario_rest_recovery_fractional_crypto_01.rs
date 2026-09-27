//! POST /api/v1/ops/repair/halted-run-fill-rest-recovery: a fractional
//! (crypto) fill or partial fill is recoverable exactly -- and only -- when
//! durable evidence proves the order is a crypto pair.
//!
//! Gate 8b previously parsed `cum_qty` with the whole-share parser, so every
//! fractional partial fill was refused into a manual reconcile even when the
//! run's own outbox row proved a crypto order. It now parses exact
//! `QtyMicros`; a fractional quantity needs the run-coherent durable outbox
//! row for `internal_order_id` to explicitly carry `asset_class = "crypto"`
//! and a symbol equal to the activity's symbol. A slash-shaped symbol is not
//! asset-class authority. Everything else still refuses.
//!
//! | Test  | Claim                                                                       |
//! |-------|-----------------------------------------------------------------------------|
//! | RF01  | crypto partial fill: dry-run plan, apply, exact QtyMicros durable, replay  |
//! | RF02  | re-apply is an idempotent no-op; run stays HALTED; other runs untouched     |
//! | RF03  | complete (non-partial) fractional crypto fill is recoverable too            |
//! | RF04  | unproven fractional fills refuse and write nothing (equity order, missing / |
//! |       | wrong / malformed asset_class, no symbol, symbol mismatch, other-run row)   |
//! | RF05  | partial fill with missing/garbage cum_qty still refuses                     |
//! | RF06  | whole-unit equity partial fill recovery is unchanged                        |
//!
//! Hermetic: every test runs against a disposable database (`run_isolated`).

use std::sync::Arc;

use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use mqk_broker_alpaca::types::AlpacaOrderActivity;
use mqk_daemon::{routes, state};
use mqk_execution::{BrokerEvent, QtyMicros};
use tower::ServiceExt;
use uuid::Uuid;

struct FakeFillFetcher(Vec<AlpacaOrderActivity>);

impl state::BrokerFillActivityFetcher for FakeFillFetcher {
    fn fetch_fill_activities_for_order(
        &self,
        _broker_order_id: &str,
    ) -> Result<Vec<AlpacaOrderActivity>, String> {
        Ok(self.0.clone())
    }
}

fn activity(
    broker_id: &str,
    id: &str,
    trade_type: &str,
    symbol: &str,
    qty: &str,
    cum_qty: Option<&str>,
) -> AlpacaOrderActivity {
    AlpacaOrderActivity {
        id: id.to_string(),
        activity_type: "FILL".to_string(),
        trade_type: Some(trade_type.to_string()),
        order_id: broker_id.to_string(),
        transaction_time: "2026-05-08T14:30:00.000000000Z".to_string(),
        price: Some("60000.5".to_string()),
        qty: Some(qty.to_string()),
        side: "buy".to_string(),
        symbol: symbol.to_string(),
        cum_qty: cum_qty.map(str::to_string),
    }
}

async fn recovery(
    router: axum::Router,
    run_id: Uuid,
    internal_id: &str,
    broker_id: &str,
    apply: bool,
) -> (StatusCode, serde_json::Value) {
    let mut body = serde_json::json!({
        "run_id": run_id.to_string(),
        "internal_order_id": internal_id,
        "broker_order_id": broker_id,
        "dry_run": !apply,
    });
    if apply {
        body["confirmation"] = serde_json::json!("APPLY_REST_FILL_RECOVERY");
    }
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/repair/halted-run-fill-rest-recovery")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json body"))
}

/// A HALTED run, one SENT outbox order (`order_symbol` / `asset_class` in
/// `order_json`, each omitted when `None`), its broker map row and a
/// cursor-only evidence cursor.
struct Fixture {
    run_id: Uuid,
    internal_id: String,
    broker_id: String,
}

async fn halted_run(pool: &sqlx::PgPool) -> Uuid {
    let run_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    sqlx::query(
        r#"
        insert into runs (run_id, engine_id, mode, started_at_utc, git_hash,
                          config_hash, config_json, host_fingerprint)
        values ($1, 'test-daemon', 'PAPER', $2, 'g', 'c', $3, 'h')
        "#,
    )
    .bind(run_id)
    .bind(now)
    .bind(serde_json::json!({"source": "scenario_rest_recovery_fractional_crypto_01"}))
    .execute(pool)
    .await
    .expect("seed run");
    sqlx::query("update runs set status = 'HALTED', halted_at_utc = $2 where run_id = $1")
        .bind(run_id)
        .bind(now)
        .execute(pool)
        .await
        .expect("halt run");
    run_id
}

/// Production-shaped durable order: the decision seam stamps
/// `asset_class = "crypto"` on a crypto pair and omits it for equity.
async fn fixture(pool: &sqlx::PgPool, order_symbol: Option<&str>) -> Fixture {
    let asset_class = order_symbol.filter(|s| s.contains('/')).map(|_| "crypto");
    fixture_with_class(pool, order_symbol, asset_class).await
}

async fn fixture_with_class(
    pool: &sqlx::PgPool,
    order_symbol: Option<&str>,
    asset_class: Option<&str>,
) -> Fixture {
    let run_id = halted_run(pool).await;
    let internal_id = Uuid::new_v4().to_string();
    let broker_id = Uuid::new_v4().to_string();
    seed_order(
        pool,
        run_id,
        &internal_id,
        &broker_id,
        order_symbol,
        asset_class,
    )
    .await;
    seed_cursor(pool, &broker_id).await;
    Fixture {
        run_id,
        internal_id,
        broker_id,
    }
}

async fn seed_order(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    internal_id: &str,
    broker_id: &str,
    order_symbol: Option<&str>,
    asset_class: Option<&str>,
) {
    let mut order_json = serde_json::json!({"qty": "0.5", "side": "buy"});
    if let Some(symbol) = order_symbol {
        order_json["symbol"] = serde_json::json!(symbol);
    }
    if let Some(class) = asset_class {
        order_json["asset_class"] = serde_json::json!(class);
    }
    let now = chrono::Utc::now();
    sqlx::query(
        r#"
        insert into oms_outbox (run_id, idempotency_key, order_json, status,
                                created_at_utc, sent_at_utc)
        values ($1, $2, $3, 'SENT', $4, $4)
        "#,
    )
    .bind(run_id)
    .bind(internal_id)
    .bind(order_json)
    .bind(now)
    .execute(pool)
    .await
    .expect("seed outbox");
    sqlx::query(
        "insert into broker_order_map (internal_id, broker_id, registered_at_utc) values ($1, $2, $3)",
    )
    .bind(internal_id)
    .bind(broker_id)
    .bind(now)
    .execute(pool)
    .await
    .expect("seed broker map");
}

async fn seed_cursor(pool: &sqlx::PgPool, broker_id: &str) {
    let cursor_json = serde_json::json!({
        "schema_version": 1,
        "rest_activity_after": "",
        "trade_updates": {
            "status": "live",
            "last_message_id": format!("alpaca:{broker_id}:fill:2026-05-08T14:30:00.000000000Z"),
            "last_event_at": "2026-05-08T14:30:00.000000000Z"
        }
    })
    .to_string();
    sqlx::query(
        r#"
        insert into broker_event_cursor (adapter_id, cursor_value, updated_at)
        values ('alpaca', $1, $2)
        on conflict (adapter_id) do update
            set cursor_value = excluded.cursor_value, updated_at = excluded.updated_at
        "#,
    )
    .bind(&cursor_json)
    .bind(chrono::Utc::now())
    .execute(pool)
    .await
    .expect("seed cursor");
}

fn router_with(pool: &sqlx::PgPool, activities: Vec<AlpacaOrderActivity>) -> axum::Router {
    let mut st = state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    );
    st.set_fill_activity_fetcher_for_test(Arc::new(FakeFillFetcher(activities)));
    routes::build_router(Arc::new(st))
}

async fn inbox_count(pool: &sqlx::PgPool, run_id: Uuid) -> i64 {
    sqlx::query_scalar("select count(*) from oms_inbox where run_id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("count inbox")
}

async fn run_status(pool: &sqlx::PgPool, run_id: Uuid) -> String {
    sqlx::query_scalar("select status from runs where run_id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("run status")
}

/// Independent "restart" load of the run's applied events, folded into a
/// portfolio exactly as replay does; returns the exact position micros.
async fn replayed_position_micros(pool: &sqlx::PgPool, run_id: Uuid, symbol: &str) -> i64 {
    let rows = mqk_db::inbox_load_all_applied_for_run(pool, run_id)
        .await
        .expect("load applied");
    let mut pf = mqk_portfolio::PortfolioState::new(1_000_000_000_000);
    for row in rows {
        let ev: BrokerEvent = serde_json::from_value(row.message_json).expect("decode event");
        let (sym, side, qty, price) = match ev {
            BrokerEvent::Fill {
                symbol,
                side,
                delta_qty,
                price_micros,
                ..
            }
            | BrokerEvent::PartialFill {
                symbol,
                side,
                delta_qty,
                price_micros,
                ..
            } => (symbol, side, delta_qty, price_micros),
            other => panic!("unexpected event {other:?}"),
        };
        let side = match side {
            mqk_execution::Side::Buy => mqk_portfolio::Side::Buy,
            mqk_execution::Side::Sell => mqk_portfolio::Side::Sell,
        };
        mqk_portfolio::apply_fill(&mut pf, &mqk_portfolio::Fill::new(sym, side, qty, price, 0));
    }
    pf.positions
        .get(symbol)
        .map(|p| p.qty_signed().raw())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn rf01_crypto_partial_fill_recovers_exactly_and_replays_identically() {
    mqk_db::run_isolated("rf01_crypto_partial", |pool| async move {
        let f = fixture(&pool, Some("BTC/USD")).await;
        let act = activity(
            &f.broker_id,
            "act-rf01",
            "partial_fill",
            "BTC/USD",
            "0.250001",
            Some("0.250001"),
        );
        let router = router_with(&pool, vec![act]);

        // Dry run: plan only, nothing written.
        let (status, v) = recovery(
            router.clone(),
            f.run_id,
            &f.internal_id,
            &f.broker_id,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["decision"], "rest_recovered_fill_evidence");
        assert_eq!(v["rest_fill"]["qty_str"], "0.250001");
        assert_eq!(inbox_count(&pool, f.run_id).await, 0);

        // Apply.
        let (status, v) =
            recovery(router.clone(), f.run_id, &f.internal_id, &f.broker_id, true).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["decision"], "applied");
        assert_eq!(v["mutated"], true);
        assert_eq!(inbox_count(&pool, f.run_id).await, 1);

        // Exact QtyMicros durable: delta and cumulative, never rounded.
        let rows = mqk_db::inbox_load_all_applied_for_run(&pool, f.run_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        let ev: BrokerEvent = serde_json::from_value(rows[0].message_json.clone()).unwrap();
        match ev {
            BrokerEvent::PartialFill {
                delta_qty,
                cum_qty_after,
                symbol,
                ..
            } => {
                assert_eq!(delta_qty, "0.250001".parse::<QtyMicros>().unwrap());
                assert_eq!(
                    cum_qty_after,
                    Some("0.250001".parse::<QtyMicros>().unwrap())
                );
                assert_eq!(symbol, "BTC/USD");
            }
            other => panic!("expected PartialFill, got {other:?}"),
        }

        // Restart/replay: two independent loads fold to the same exact position.
        let first = replayed_position_micros(&pool, f.run_id, "BTC/USD").await;
        let second = replayed_position_micros(&pool, f.run_id, "BTC/USD").await;
        assert_eq!(first, 250_001);
        assert_eq!(first, second);
    })
    .await;
}

#[tokio::test]
async fn rf02_reapply_is_idempotent_run_stays_halted_and_other_runs_are_untouched() {
    mqk_db::run_isolated("rf02_idempotent", |pool| async move {
        let f = fixture(&pool, Some("BTC/USD")).await;
        let bystander = halted_run(&pool).await;
        let act = activity(
            &f.broker_id,
            "act-rf02",
            "partial_fill",
            "BTC/USD",
            "0.5",
            Some("0.5"),
        );
        let router = router_with(&pool, vec![act]);

        let (_, first) =
            recovery(router.clone(), f.run_id, &f.internal_id, &f.broker_id, true).await;
        assert_eq!(first["decision"], "applied", "{first}");
        let (status, again) =
            recovery(router.clone(), f.run_id, &f.internal_id, &f.broker_id, true).await;
        assert_eq!(status, StatusCode::OK, "{again}");
        assert_eq!(again["decision"], "already_repaired");
        assert_eq!(again["mutated"], false);

        assert_eq!(
            inbox_count(&pool, f.run_id).await,
            1,
            "no duplicate fill row"
        );
        assert_eq!(
            run_status(&pool, f.run_id).await,
            "HALTED",
            "halt stays sticky"
        );
        assert_eq!(inbox_count(&pool, bystander).await, 0);
        assert_eq!(run_status(&pool, bystander).await, "HALTED");
        assert_eq!(
            replayed_position_micros(&pool, f.run_id, "BTC/USD").await,
            500_000
        );
    })
    .await;
}

#[tokio::test]
async fn rf03_complete_fractional_crypto_fill_is_recoverable_too() {
    mqk_db::run_isolated("rf03_crypto_fill", |pool| async move {
        let f = fixture(&pool, Some("BTC/USD")).await;
        let act = activity(&f.broker_id, "act-rf03", "fill", "BTC/USD", "0.5", None);
        let router = router_with(&pool, vec![act]);
        let (status, v) = recovery(router, f.run_id, &f.internal_id, &f.broker_id, true).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["decision"], "applied");
        assert_eq!(
            replayed_position_micros(&pool, f.run_id, "BTC/USD").await,
            500_000
        );
    })
    .await;
}

#[tokio::test]
async fn rf04_unproven_fractional_fills_refuse_and_write_nothing() {
    mqk_db::run_isolated("rf04_unproven", |pool| async move {
        // (durable order symbol, durable asset_class, activity symbol, activity type, why)
        type Case = (
            Option<&'static str>,
            Option<&'static str>,
            &'static str,
            &'static str,
            &'static str,
        );
        let cases: [Case; 14] = [
            (
                Some("AAPL"),
                None,
                "AAPL",
                "partial_fill",
                "equity order: fractional is invalid",
            ),
            (
                Some("AAPL"),
                None,
                "AAPL",
                "fill",
                "equity order: fractional complete fill",
            ),
            (
                Some("BTC/USD"),
                None,
                "BTC/USD",
                "partial_fill",
                "slash-shaped symbol but asset_class missing",
            ),
            (
                Some("BTC/USD"),
                None,
                "BTC/USD",
                "fill",
                "slash-shaped symbol but asset_class missing (complete fill)",
            ),
            (
                Some("BTC/USD"),
                Some("equity"),
                "BTC/USD",
                "partial_fill",
                "asset_class=equity",
            ),
            (
                Some("BTC/USD"),
                Some("forex"),
                "BTC/USD",
                "partial_fill",
                "asset_class=forex",
            ),
            (
                Some("BTC/USD"),
                Some("future"),
                "BTC/USD",
                "fill",
                "asset_class=future",
            ),
            (
                Some("BTC/USD"),
                Some("option"),
                "BTC/USD",
                "fill",
                "asset_class=option",
            ),
            (
                Some("BTC/USD"),
                Some(""),
                "BTC/USD",
                "partial_fill",
                "asset_class blank",
            ),
            (
                Some("BTC/USD"),
                Some("CRYPTO"),
                "BTC/USD",
                "partial_fill",
                "asset_class not the canonical spelling",
            ),
            (
                None,
                Some("crypto"),
                "BTC/USD",
                "partial_fill",
                "crypto but durable row carries no symbol",
            ),
            (
                Some("BTC/USD"),
                Some("crypto"),
                "BTCUSD",
                "partial_fill",
                "crypto but activity symbol form differs",
            ),
            (
                Some("BTC/USD"),
                Some("crypto"),
                "ETH/USD",
                "fill",
                "crypto but activity symbol is another pair",
            ),
            (
                Some("AAPL"),
                Some("crypto"),
                "BTC/USD",
                "fill",
                "crypto but activity symbol differs from the durable symbol",
            ),
        ];
        for (order_symbol, asset_class, activity_symbol, kind, why) in cases {
            let f = fixture_with_class(&pool, order_symbol, asset_class).await;
            let act = activity(
                &f.broker_id,
                "act-rf04",
                kind,
                activity_symbol,
                "0.5",
                Some("0.5"),
            );
            let router = router_with(&pool, vec![act]);
            for apply in [false, true] {
                let (status, v) = recovery(
                    router.clone(),
                    f.run_id,
                    &f.internal_id,
                    &f.broker_id,
                    apply,
                )
                .await;
                assert_eq!(status, StatusCode::CONFLICT, "{why} apply={apply}: {v}");
                assert_eq!(v["decision"], "refused", "{why}");
                assert_eq!(v["gate"], "repair.fractional_fill_unproven", "{why}: {v}");
            }
            assert_eq!(
                inbox_count(&pool, f.run_id).await,
                0,
                "{why}: nothing written"
            );
            assert_eq!(run_status(&pool, f.run_id).await, "HALTED", "{why}");
        }

        // No durable outbox row for the internal order id at all.
        let run_id = halted_run(&pool).await;
        let internal_id = Uuid::new_v4().to_string();
        let broker_id = Uuid::new_v4().to_string();
        seed_cursor(&pool, &broker_id).await;
        // The stale broker map entry needs an outbox row to be discoverable; a
        // row owned by ANOTHER run models "not this run's evidence".
        let other_run = halted_run(&pool).await;
        seed_order(
            &pool,
            other_run,
            &internal_id,
            &broker_id,
            Some("BTC/USD"),
            Some("crypto"),
        )
        .await;
        let act = activity(&broker_id, "act-rf04-x", "fill", "BTC/USD", "0.5", None);
        let router = router_with(&pool, vec![act]);
        let (status, v) = recovery(router, run_id, &internal_id, &broker_id, true).await;
        assert_ne!(
            status,
            StatusCode::OK,
            "another run's order is not evidence: {v}"
        );
        assert_eq!(inbox_count(&pool, run_id).await, 0);
    })
    .await;
}

#[tokio::test]
async fn rf05_partial_fill_with_missing_or_garbage_cum_qty_still_refuses() {
    mqk_db::run_isolated("rf05_cum_qty", |pool| async move {
        for (cum_qty, why) in [
            (None, "missing"),
            (Some("abc"), "garbage"),
            (Some("-0.5"), "negative"),
        ] {
            let f = fixture(&pool, Some("BTC/USD")).await;
            let act = activity(
                &f.broker_id,
                "act-rf05",
                "partial_fill",
                "BTC/USD",
                "0.5",
                cum_qty,
            );
            let router = router_with(&pool, vec![act]);
            let (status, v) = recovery(router, f.run_id, &f.internal_id, &f.broker_id, true).await;
            assert_eq!(status, StatusCode::CONFLICT, "{why}: {v}");
            assert_eq!(v["gate"], "repair.recovery_data_malformed", "{why}: {v}");
            assert_eq!(inbox_count(&pool, f.run_id).await, 0, "{why}");
        }
    })
    .await;
}

#[tokio::test]
async fn rf06_whole_unit_equity_partial_fill_recovery_is_unchanged() {
    mqk_db::run_isolated("rf06_equity_partial", |pool| async move {
        let f = fixture(&pool, Some("AAPL")).await;
        let act = activity(
            &f.broker_id,
            "act-rf06",
            "partial_fill",
            "AAPL",
            "5",
            Some("5"),
        );
        let router = router_with(&pool, vec![act]);
        let (status, v) = recovery(router, f.run_id, &f.internal_id, &f.broker_id, true).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["decision"], "applied");
        assert_eq!(
            replayed_position_micros(&pool, f.run_id, "AAPL").await,
            5_000_000
        );
    })
    .await;
}
