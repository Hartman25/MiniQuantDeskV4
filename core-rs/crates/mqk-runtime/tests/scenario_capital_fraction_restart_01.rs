//! Restart-safe FixedInitialCapitalFractionV1 in the runtime seam.
//!
//! A real disposable Postgres database holds the durable held state; "restart"
//! is dropping the host and rebuilding it from that database. No broker, no
//! Paper activation, no order submission: the host only evaluates a strategy
//! and persists its sizing state.
//!
//! DB-backed tests are `#[ignore]` and need `MQK_DATABASE_URL` (disposable DB):
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/<disposable> \
//!   cargo test -p mqk-runtime --test scenario_capital_fraction_restart_01 \
//!   -- --include-ignored --test-threads=1

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use mqk_db::held_sizing_state::{held_sizing_apply, held_sizing_fetch, HeldSizingTransitionRow};
use mqk_execution::{QtyMicros, StrategyOutput, TargetPosition};
use mqk_runtime::capital_fraction_host::CapitalFractionRuntimeHost;
use mqk_runtime::native_strategy::{
    build_plugin_registry_from_inputs, resolve_capital_fraction_deployment,
    resolve_native_deployment_identity, CapitalFractionDeploymentContract, StrategyBootstrapInputs,
};
use mqk_strategy::{
    BarStub, HeldSizingScope, PluginRegistry, RecentBarsWindow, RestartRecovery, Strategy,
    StrategyContext, StrategyMeta, StrategySpec,
};

const USD: i64 = 1_000_000;
const CAPITAL: i64 = 100_000 * USD;
const CF: &str = "fixed_initial_capital_fraction_v1";
const STRATEGY: &str = "window_trend";

fn cf_inputs(bps: &str, capital: &str) -> StrategyBootstrapInputs {
    StrategyBootstrapInputs {
        symbol: "SPY".to_string(),
        raw_sizing_policy: Some(CF.to_string()),
        raw_allocation_fraction_bps: Some(bps.to_string()),
        raw_allocated_capital_micros: Some(capital.to_string()),
        ..Default::default()
    }
}

fn contract(bps: &str, capital: &str) -> CapitalFractionDeploymentContract {
    resolve_capital_fraction_deployment(&cf_inputs(bps, capital))
        .unwrap()
        .expect("capital-fraction contract")
}

/// Long iff the window's last close exceeds its first: a pure function of the
/// bounded window, so a restart reconstructs the same signal.
struct WindowTrend;

impl Strategy for WindowTrend {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(STRATEGY, 86_400)
    }
    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let bars = &ctx.recent.bars;
        let long = match (bars.first(), bars.last()) {
            (Some(a), Some(b)) => b.close_micros > a.close_micros,
            _ => false,
        };
        StrategyOutput::new(vec![TargetPosition::new(
            "SPY",
            if long {
                QtyMicros::from_whole_units(1).unwrap()
            } else {
                QtyMicros::ZERO
            },
        )])
    }
}

fn registry() -> PluginRegistry {
    let mut r = PluginRegistry::new();
    r.register(
        StrategyMeta::new(STRATEGY, "1", 86_400, "test")
            .with_restart_recovery(RestartRecovery::DurableStateRequired),
        || Box::new(WindowTrend),
    )
    .unwrap();
    r
}

fn window(closes: [i64; 3], end_ts0: i64) -> StrategyContext {
    let bars = closes
        .iter()
        .enumerate()
        .map(|(i, c)| BarStub::new(end_ts0 + i as i64, true, c * USD, 1))
        .collect();
    StrategyContext::new(86_400, 0, RecentBarsWindow::new(3, bars))
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 3, 14, 0, 0).unwrap()
}

fn dep(name: &str) -> String {
    use std::sync::OnceLock;
    static RUN: OnceLock<u128> = OnceLock::new();
    let run = RUN.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|t| t.as_nanos())
            .unwrap_or(0)
    });
    format!("{name}-{run}")
}

fn scope(deployment: &str) -> HeldSizingScope {
    HeldSizingScope {
        deployment_id: deployment.to_string(),
        strategy_id: STRATEGY.to_string(),
    }
}

async fn pool() -> sqlx::PgPool {
    assert!(
        std::env::var(mqk_db::ENV_DB_URL).is_ok(),
        "requires MQK_DATABASE_URL (disposable test database)"
    );
    mqk_db::testkit_db_pool().await.unwrap()
}

async fn host(pool: &sqlx::PgPool, deployment: &str) -> CapitalFractionRuntimeHost {
    CapitalFractionRuntimeHost::recover(
        pool,
        &registry(),
        &contract("1000", "100000000000"),
        scope(deployment),
    )
    .await
    .expect("recover")
}

async fn target(
    h: &mut CapitalFractionRuntimeHost,
    pool: &sqlx::PgPool,
    ctx: StrategyContext,
) -> i64 {
    let r = h
        .on_bar_durable(pool, &ctx, now())
        .await
        .expect("on_bar_durable");
    r.intents.output.targets[0].qty.raw()
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL (disposable test database)"]
async fn restart_while_long_keeps_exactly_the_original_q_and_exit_reentry_resolves_anew() {
    let pool = pool().await;
    let d = dep("dep-restart");
    let rows = |pool: sqlx::PgPool, d: String| async move {
        held_sizing_fetch(&pool, &d, STRATEGY).await.unwrap()
    };

    // Flat start: nothing durable, flat bar -> flat, still nothing durable.
    let mut h = host(&pool, &d).await;
    assert_eq!(target(&mut h, &pool, window([100, 100, 100], 1)).await, 0);
    assert!(rows(pool.clone(), d.clone()).await.is_empty());

    // Causal entry: close $125 -> $10,000 / $125 = 80 shares, durably recorded.
    assert_eq!(
        target(&mut h, &pool, window([100, 100, 125], 2)).await,
        80 * USD
    );
    let r = rows(pool.clone(), d.clone()).await;
    assert_eq!(r.len(), 1);
    assert_eq!(
        (
            r[0].status.as_str(),
            r[0].entry_generation,
            r[0].resolved_target_qty_micros,
            r[0].reference_price_micros,
            r[0].reference_bar_end_ts
        ),
        ("active", 1, 80 * USD, 125 * USD, 4)
    );
    let before_restart = r[0].clone();

    // RESTART (process gone): rebuild from the database only.
    drop(h);
    let mut h = host(&pool, &d).await;
    // Next completed bar still signals long at a MUCH higher close: re-resolving
    // would give 25 shares and a one-share reset would give 1.
    assert_eq!(
        target(&mut h, &pool, window([100, 125, 400], 5)).await,
        80 * USD
    );
    assert_eq!(
        rows(pool.clone(), d.clone()).await,
        vec![before_restart.clone()],
        "no resize, no duplicate entry, no second generation"
    );
    assert!(
        h.audit_snapshot().entries.is_empty(),
        "nothing was re-resolved after restart"
    );

    // Another restart, still long, then the strategy exits to flat.
    drop(h);
    let mut h = host(&pool, &d).await;
    assert_eq!(target(&mut h, &pool, window([400, 400, 100], 8)).await, 0);
    let r = rows(pool.clone(), d.clone()).await;
    assert_eq!(
        (r[0].status.as_str(), r[0].entry_generation),
        ("released", 1)
    );

    // Restart while flat stays flat and writes nothing.
    drop(h);
    let mut h = host(&pool, &d).await;
    assert_eq!(target(&mut h, &pool, window([100, 100, 100], 11)).await, 0);
    assert_eq!(rows(pool.clone(), d.clone()).await[0].status, "released");

    // A genuine new entry resolves from the SAME initial capital and the new
    // causal close: $10,000 / $250 = 40 shares, generation 2.
    assert_eq!(
        target(&mut h, &pool, window([100, 100, 250], 14)).await,
        40 * USD
    );
    let r = rows(pool.clone(), d.clone()).await;
    assert_eq!(
        (
            r[0].status.as_str(),
            r[0].entry_generation,
            r[0].resolved_target_qty_micros,
            r[0].initial_allocated_capital_micros
        ),
        ("active", 2, 40 * USD, CAPITAL)
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL (disposable test database)"]
async fn a_crash_after_persist_before_submit_replays_without_a_duplicate_entry() {
    let pool = pool().await;
    let d = dep("dep-crash");
    let mut h = host(&pool, &d).await;
    assert_eq!(
        target(&mut h, &pool, window([100, 100, 125], 2)).await,
        80 * USD
    );
    // The result was never submitted (crash): the process rebuilds and the same
    // completed bar is evaluated again.
    drop(h);
    let mut h = host(&pool, &d).await;
    assert_eq!(
        target(&mut h, &pool, window([100, 100, 125], 2)).await,
        80 * USD
    );
    let r = held_sizing_fetch(&pool, &d, STRATEGY).await.unwrap();
    assert_eq!((r.len(), r[0].entry_generation), (1, 1));
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL (disposable test database)"]
async fn recovery_refuses_wrong_contract_and_foreign_or_tampered_state() {
    let pool = pool().await;
    let d = dep("dep-negative");
    let mut h = host(&pool, &d).await;
    assert_eq!(
        target(&mut h, &pool, window([100, 100, 125], 2)).await,
        80 * USD
    );
    drop(h);

    let recover = |c: CapitalFractionDeploymentContract, s: HeldSizingScope| {
        let pool = pool.clone();
        async move { CapitalFractionRuntimeHost::recover(&pool, &registry(), &c, s).await }
    };
    // Control: the right contract and scope recover.
    assert!(recover(contract("1000", "100000000000"), scope(&d))
        .await
        .is_ok());
    // Wrong fraction / allocated capital against the stored entry: fail closed.
    assert!(recover(contract("2000", "100000000000"), scope(&d))
        .await
        .is_err());
    assert!(recover(contract("1000", "50000000000"), scope(&d))
        .await
        .is_err());
    // A caps change is also a contract change.
    let mut capped = cf_inputs("1000", "100000000000");
    capped.raw_max_target_qty = Some("10".to_string());
    let capped = resolve_capital_fraction_deployment(&capped)
        .unwrap()
        .unwrap();
    assert!(recover(capped, scope(&d)).await.is_err());

    // Another deployment / strategy never sees (or adopts) this deployment's Q.
    let mut other = CapitalFractionRuntimeHost::recover(
        &pool,
        &registry(),
        &contract("1000", "100000000000"),
        scope(&dep("dep-negative-other")),
    )
    .await
    .unwrap();
    assert_eq!(
        target(&mut other, &pool, window([100, 400, 400], 20)).await,
        25 * USD,
        "a foreign deployment resolves its own entry, not 80"
    );
    // A strategy id that is not registered cannot recover at all.
    let bad_strategy = HeldSizingScope {
        deployment_id: d.clone(),
        strategy_id: "no_such".to_string(),
    };
    assert!(recover(contract("1000", "100000000000"), bad_strategy)
        .await
        .is_err());
    // A stateless (bounded-history) registry entry is not a capital-fraction strategy.
    let mut stateless = PluginRegistry::new();
    stateless
        .register(StrategyMeta::new(STRATEGY, "1", 86_400, "t"), || {
            Box::new(WindowTrend)
        })
        .unwrap();
    assert!(CapitalFractionRuntimeHost::recover(
        &pool,
        &stateless,
        &contract("1000", "100000000000"),
        scope(&d)
    )
    .await
    .is_err());

    // Tampered / stale stored Q: a row whose Q the resolver cannot reproduce.
    let d2 = dep("dep-tampered");
    let mut h = host(&pool, &d2).await;
    target(&mut h, &pool, window([100, 100, 125], 2)).await;
    drop(h);
    sqlx::query(
        "update sys_strategy_held_sizing_state set resolved_target_qty_micros = 81000000 \
         where deployment_id = $1",
    )
    .bind(&d2)
    .execute(&pool)
    .await
    .unwrap();
    assert!(recover(contract("1000", "100000000000"), scope(&d2))
        .await
        .is_err());
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL (disposable test database)"]
async fn persistence_failure_poisons_the_host_and_nothing_is_returned() {
    let pool = pool().await;
    let d = dep("dep-poison");
    let mut h = host(&pool, &d).await;
    let dead = pool.clone();
    dead.close().await;
    let ctx = window([100, 100, 125], 2);
    // `pool` is the closed handle: the entry cannot be persisted.
    assert!(h.on_bar_durable(&pool, &ctx, now()).await.is_err());
    assert!(h.is_poisoned());
    assert!(
        h.on_bar_durable(&pool, &window([100, 100, 100], 3), now())
            .await
            .is_err(),
        "a poisoned host refuses further bars until rebuilt from the DB"
    );
    // Nothing was persisted: a rebuilt host starts flat and enters fresh.
    let fresh = mqk_db::testkit_db_pool().await.unwrap();
    assert!(held_sizing_fetch(&fresh, &d, STRATEGY)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL (disposable test database)"]
async fn prepared_bars_persist_nothing_until_the_batch_commit_and_abort_poisons() {
    let pool = pool().await;
    let (da, db_) = (dep("dep-prep-a"), dep("dep-prep-b"));
    let mut a = host(&pool, &da).await;
    let mut b = host(&pool, &db_).await;
    let ctx = window([100, 100, 125], 2);

    a.prepare_bar(&ctx).expect("prepare a");
    b.prepare_bar(&ctx).expect("prepare b");
    assert!(a.has_pending_commit() && b.has_pending_commit());
    for d in [&da, &db_] {
        assert!(held_sizing_fetch(&pool, d, STRATEGY)
            .await
            .unwrap()
            .is_empty());
    }
    assert!(
        a.prepare_bar(&window([100, 100, 100], 3)).is_err() && a.is_poisoned(),
        "a second prepare before commit/abort poisons the host"
    );

    // Abort: nothing durable, host unusable, commit refused.
    b.abort_prepared("peer failed");
    assert!(b.is_poisoned() && !b.has_pending_commit());
    let mut fresh_a = host(&pool, &da).await;
    fresh_a.prepare_bar(&ctx).expect("prepare fresh a");
    assert!(
        mqk_runtime::capital_fraction_host::commit_prepared_batch(
            &pool,
            &mut [&mut fresh_a, &mut b],
            now()
        )
        .await
        .is_err(),
        "a batch containing a poisoned host commits nothing"
    );
    assert!(held_sizing_fetch(&pool, &da, STRATEGY)
        .await
        .unwrap()
        .is_empty());

    // Two healthy prepared hosts commit in one batch.
    let mut fresh_b = host(&pool, &db_).await;
    fresh_b.prepare_bar(&ctx).expect("prepare fresh b");
    mqk_runtime::capital_fraction_host::commit_prepared_batch(
        &pool,
        &mut [&mut fresh_a, &mut fresh_b],
        now(),
    )
    .await
    .expect("batch commit");
    for d in [&da, &db_] {
        assert_eq!(
            held_sizing_fetch(&pool, d, STRATEGY).await.unwrap().len(),
            1
        );
    }
    assert!(!fresh_a.has_pending_commit() && !fresh_a.is_poisoned());
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL (disposable test database)"]
async fn retrying_the_same_persisted_transition_is_idempotent_at_the_store() {
    let pool = pool().await;
    let d = dep("dep-retry");
    let mut h = host(&pool, &d).await;
    target(&mut h, &pool, window([100, 100, 125], 2)).await;
    let stored = held_sizing_fetch(&pool, &d, STRATEGY).await.unwrap();
    let replay = HeldSizingTransitionRow::Entered(stored[0].clone());
    let report = held_sizing_apply(&pool, &[replay], now()).await.unwrap();
    assert_eq!((report.applied, report.already_applied), (0, 1));
    assert_eq!(
        held_sizing_fetch(&pool, &d, STRATEGY).await.unwrap(),
        stored
    );
}

// ---------------------------------------------------------------------------
// Identity parity with canonical Backtest (no DB needed).
// ---------------------------------------------------------------------------

fn backtest_report_fingerprint(
    bps: i64,
    capital: i64,
    max_qty: Option<i64>,
    max_notional: Option<i64>,
    strategy: &str,
) -> String {
    use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, StrategySizingConfig};
    use mqk_strategy::engines::register_builtin_strategies_with_sizing;
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    let instance = reg.instantiate(strategy).unwrap();
    let tf = instance.spec().timeframe_secs;
    let mut cfg = BacktestConfig::conservative_defaults();
    cfg.timeframe_secs = tf;
    cfg.integrity_enabled = false;
    cfg.initial_cash_micros = capital;
    cfg.sizing_policy = mqk_strategy::SizingPolicy::capital_fraction_v1(bps).unwrap();
    cfg.sizing = StrategySizingConfig {
        target_qty: 1,
        max_target_qty: max_qty,
        max_position_notional_usd: max_notional,
    };
    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(instance).unwrap();
    let bars: Vec<BacktestBar> = (0..60)
        .map(|i| {
            let c = (100 + i) * USD;
            BacktestBar::new("SPY", tf * (i + 1), c, c + USD, c - USD, c, 1_000)
        })
        .collect();
    engine.run(&bars).unwrap().strategy_semantic_fingerprint
}

#[test]
fn runtime_resolved_fingerprint_equals_the_canonical_backtest_wrapper_fingerprint() {
    for strategy in [
        "swing_momentum",
        "mean_reversion",
        "volatility_breakout",
        "intraday_scalper",
    ] {
        for (bps, capital, max_qty, max_notional) in [
            (1_000, 100_000 * USD, None, None),
            (2_000, 100_000 * USD, None, None),
            (1_000, 250_000 * USD, Some(40), None),
            (1_000, 100_000 * USD, Some(40), Some(5_000)),
        ] {
            let mut inputs = cf_inputs(&bps.to_string(), &capital.to_string());
            inputs.raw_max_target_qty = max_qty.map(|v: i64| v.to_string());
            inputs.raw_max_notional_usd = max_notional.map(|v: i64| v.to_string());
            let runtime = resolve_native_deployment_identity(&inputs, strategy).unwrap();
            let backtest =
                backtest_report_fingerprint(bps, capital, max_qty, max_notional, strategy);
            assert_eq!(
                runtime.semantic_fingerprint, backtest,
                "{strategy} {bps} {capital} {max_qty:?} {max_notional:?}"
            );
            assert_eq!(runtime.timeframe_secs, {
                let mut reg = PluginRegistry::new();
                mqk_strategy::engines::register_builtin_strategies_with_sizing(
                    &mut reg, "SPY", 1, None, None,
                )
                .unwrap();
                reg.instantiate(strategy).unwrap().spec().timeframe_secs
            });
        }
    }
    // Every contract parameter is identity: all four variants above differ.
    let fp = |bps: &str, cap: &str| {
        resolve_native_deployment_identity(&cf_inputs(bps, cap), "swing_momentum")
            .unwrap()
            .semantic_fingerprint
    };
    assert_ne!(fp("1000", "100000000000"), fp("2000", "100000000000"));
    assert_ne!(fp("1000", "100000000000"), fp("1000", "100000000001"));
}

#[test]
fn fixed_quantity_identity_is_unchanged_by_the_new_resolver() {
    let inputs = StrategyBootstrapInputs {
        symbol: "AAPL".to_string(),
        ..Default::default()
    };
    let resolved = resolve_native_deployment_identity(&inputs, "swing_momentum").unwrap();
    let registry = build_plugin_registry_from_inputs(&inputs).unwrap();
    assert_eq!(
        resolved.semantic_fingerprint,
        registry
            .instantiate_verified("swing_momentum")
            .unwrap()
            .semantic_fingerprint()
    );
    // An unrecoverable engine still has no fingerprint under either contract.
    assert!(resolve_native_deployment_identity(&inputs, "pullback_mean_reversion_20_2").is_err());
    assert!(resolve_native_deployment_identity(
        &cf_inputs("1000", "100000000000"),
        "does_not_exist"
    )
    .is_err());
}

#[test]
fn capital_fraction_resolution_is_strict_and_never_defaults() {
    let ok = cf_inputs("1000", "100000000000");
    let c = resolve_capital_fraction_deployment(&ok).unwrap().unwrap();
    assert_eq!(
        (c.allocation_fraction_bps, c.allocated_capital_micros),
        (1_000, CAPITAL)
    );
    assert!(
        resolve_capital_fraction_deployment(&StrategyBootstrapInputs::default())
            .unwrap()
            .is_none()
    );
    for (cap_qty, cap_notional, target) in [
        (Some("abc"), None, None),
        (Some("0"), None, None),
        (Some("-5"), None, None),
        (Some("1.5"), None, None),
        (None, Some("x"), None),
        (None, Some("0"), None),
        (None, None, Some("7")),
    ] {
        let mut i = ok.clone();
        i.raw_max_target_qty = cap_qty.map(str::to_string);
        i.raw_max_notional_usd = cap_notional.map(str::to_string);
        i.raw_target_qty = target.map(str::to_string);
        assert!(
            resolve_capital_fraction_deployment(&i).is_err()
                && build_plugin_registry_from_inputs(&i).is_err(),
            "{cap_qty:?} {cap_notional:?} {target:?}"
        );
    }
    let _ = Arc::new(());
}
