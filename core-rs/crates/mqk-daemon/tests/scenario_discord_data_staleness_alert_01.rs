//! DISCORD-DATA-STALENESS-ALERT-01: Discord alert derived from the real
//! production `md_staleness_per_tick_gate_01` refusal (cap #9,
//! `AppState::prepare_bar_window_for_symbol_timeframe`).
//!
//! This does NOT create a second staleness authority: it hooks the existing
//! canonical per-tick gate proven by `scenario_md_staleness_per_tick_gate_01.rs`
//! and only adds a best-effort, deduped Discord notification alongside the
//! refusal that already occurs.
//!
//! # Isolation
//!
//! Every DB-backed test here uses `mqk_db::run_isolated` — a fresh,
//! migrated, throwaway `mqk_disp_*` Postgres database created and dropped
//! for that one test alone. This never reads or writes the shared Paper
//! database (`miniquantdesk_paper`): the disposable database lives on
//! whatever server `MQK_DATABASE_URL` points at, but is itself a brand-new,
//! empty database nothing else ever connects to. There is no `:5440` /
//! `miniquantdesk_paper` requirement and no graceful skip when the shared
//! Paper database specifically is unavailable — only the ordinary "is any
//! test database configured at all" guard every DB-backed test in this repo
//! uses.
//!
//! `DeploymentMode::Paper` is used throughout (not `LiveShadow`): the
//! staleness gate itself (`prepare_bar_window_for_symbol_timeframe`) is
//! deployment-mode-agnostic — nothing about it requires Live/LiveShadow.
//!
//! # Proof matrix
//!
//! | Test | What it proves                                                                |
//! |------|--------------------------------------------------------------------------------|
//! | DA01 | Stale bar beyond threshold -> a critical alert is delivered                    |
//! | DA02 | Fresh bar within threshold -> no alert is delivered (dispatch proceeds)       |
//! | DA03 | Missing bar (no md_bars rows) -> a critical alert is delivered                |
//! | DA04 | Repeated stale ticks for the same symbol -> exactly one alert (dedup)        |
//! | DA05 | Discord delivery failure -> dispatch refusal is unaffected (best-effort)     |
//! | DA06 | Alert dedup claim is a pure per-(run, symbol) primitive (no DB needed)       |
//! | DA07 | No DB pool at all -> the single-stub fallback runs, never the staleness gate, so no alert fires |
//! | DA08 | Dedup resets at run start -> a symbol that already alerted this run may alert again next run |
//!
//! Secret-safety (no webhook URL/token in any payload, error, or status) is
//! already exhaustively proven generically for every `notify_*` method in
//! `scenario_discord_secret_safety_01.rs` and `scenario_discord_non2xx_delivery_01.rs`
//! (ND11); not duplicated here. Per-channel routing (critical alerts -> the
//! `alerts` channel) is proven generically in
//! `scenario_discord_channel_routing_01.rs` (CR01); this file uses
//! `DiscordNotifier::from_url` (every channel -> one sink) since channel
//! selection itself is not this patch's concern.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::Utc;
use mqk_daemon::notify::DiscordNotifier;
use mqk_daemon::state::{self, AppState, StrategyBarInput};
use mqk_runtime::native_strategy::{build_daemon_plugin_registry, NativeStrategyBootstrap};
use reqwest::StatusCode;

const FAKE_WEBHOOK_PATH: &str = "/discord.com/api/webhooks/fake-webhook-id/fake-webhook-token";

// ---------------------------------------------------------------------------
// In-process Discord sink
// ---------------------------------------------------------------------------

struct Sink {
    url: String,
    request_count: Arc<AtomicUsize>,
}

async fn start_sink(status: StatusCode) -> Sink {
    let request_count = Arc::new(AtomicUsize::new(0));
    let rc = request_count.clone();

    let app = axum::Router::new().route(
        FAKE_WEBHOOK_PATH,
        axum::routing::post(move |_body: axum::body::Bytes| {
            let rc = rc.clone();
            async move {
                rc.fetch_add(1, Ordering::SeqCst);
                (status, "ok")
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    Sink {
        url: format!("http://127.0.0.1:{}{FAKE_WEBHOOK_PATH}", addr.port()),
        request_count,
    }
}

// ---------------------------------------------------------------------------
// DB helpers (mirrors scenario_md_staleness_per_tick_gate_01.rs, minus the
// Paper-DB-specific allowlist — see module docs)
// ---------------------------------------------------------------------------

fn active_bootstrap() -> NativeStrategyBootstrap {
    let reg = build_daemon_plugin_registry();
    let ids = vec!["swing_momentum".to_string()];
    NativeStrategyBootstrap::bootstrap(Some(&ids), &reg)
}

fn test_bar_input(now_tick: u64) -> StrategyBarInput {
    StrategyBarInput {
        now_tick,
        end_ts: 1_700_000_000,
        limit_price: Some(150_000_000),
        qty: 10,
    }
}

async fn seed_one_bar(pool: &sqlx::PgPool, symbol: &str, timeframe: &str, end_ts: i64) {
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(symbol)
        .bind(timeframe)
        .execute(pool)
        .await
        .expect("delete prior md_bars rows");

    sqlx::query(
        "insert into md_bars \
           (symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros, volume, is_complete) \
         values ($1, $2, $3, $4, $4, $4, $4, $5, true)",
    )
    .bind(symbol)
    .bind(timeframe)
    .bind(end_ts)
    .bind(100_000_000_i64)
    .bind(1_000_i64)
    .execute(pool)
    .await
    .expect("insert md_bars fixture row");
}

async fn seed_no_bars(pool: &sqlx::PgPool, symbol: &str, timeframe: &str) {
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(symbol)
        .bind(timeframe)
        .execute(pool)
        .await
        .expect("delete prior md_bars rows");
}

/// [`scenario_md_staleness_per_tick_gate_01::db_state`], plus injecting a
/// `DiscordNotifier` pointed at an in-process sink (mutated on the owned
/// `AppState` before it is wrapped in `Arc`, since `discord_notifier` is a
/// plain `pub` field with no interior mutability). `DeploymentMode::Paper`
/// (not `LiveShadow`): the staleness gate is deployment-mode-agnostic, and
/// this is the canonical mode for a hermetic proof.
async fn db_state_with_notifier(pool: sqlx::PgPool, notifier: DiscordNotifier) -> Arc<AppState> {
    let mut st = AppState::new_for_test_with_db_mode_and_broker(
        pool,
        state::DeploymentMode::Paper,
        state::BrokerKind::Paper,
    );
    st.discord_notifier = notifier;
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;
    Arc::new(st)
}

/// Best-effort Discord delivery is fired via `tokio::spawn`; give it a beat
/// to land before asserting the sink's hit count.
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

// ---------------------------------------------------------------------------
// DA01 — stale bar delivers a critical alert
// ---------------------------------------------------------------------------

#[tokio::test]
async fn da01_stale_bar_delivers_critical_alert() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("DA01: skipped — MQK_DATABASE_URL not set");
        return;
    }

    mqk_db::run_isolated("da01", |pool| async move {
        let sink = start_sink(StatusCode::OK).await;

        let symbol = "DA01STALE";
        let timeframe = "1Min";
        let now_ts = Utc::now().timestamp();
        seed_one_bar(&pool, symbol, timeframe, now_ts - 10_000).await;

        let st = db_state_with_notifier(pool, DiscordNotifier::from_url(&sink.url)).await;
        st.set_per_symbol_bar_staleness_secs_for_test(Some(300));
        st.deposit_strategy_bar_input(test_bar_input(1)).await;

        let result = st.tick_strategy_dispatch_for_symbol(symbol, timeframe).await;
        assert!(result.is_none(), "DA01: stale bar must still refuse dispatch");

        settle().await;
        assert_eq!(
            sink.request_count.load(Ordering::SeqCst),
            1,
            "DA01: a stale bar refusal must deliver exactly one critical alert"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// DA02 — fresh bar delivers no alert
// ---------------------------------------------------------------------------

#[tokio::test]
async fn da02_fresh_bar_delivers_no_alert() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("DA02: skipped — MQK_DATABASE_URL not set");
        return;
    }

    mqk_db::run_isolated("da02", |pool| async move {
        let sink = start_sink(StatusCode::OK).await;

        let symbol = "DA02FRESH";
        let timeframe = "1Min";
        let now_ts = Utc::now().timestamp();
        seed_one_bar(&pool, symbol, timeframe, now_ts - 60).await;

        let st = db_state_with_notifier(pool, DiscordNotifier::from_url(&sink.url)).await;
        st.set_per_symbol_bar_staleness_secs_for_test(Some(300));
        st.deposit_strategy_bar_input(test_bar_input(1)).await;

        let result = st.tick_strategy_dispatch_for_symbol(symbol, timeframe).await;
        assert!(result.is_some(), "DA02: fresh bar must dispatch normally");

        settle().await;
        assert_eq!(
            sink.request_count.load(Ordering::SeqCst),
            0,
            "DA02: a feasible/fresh tick must never fire a staleness alert"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// DA03 — missing bar delivers a critical alert
// ---------------------------------------------------------------------------

#[tokio::test]
async fn da03_missing_bar_delivers_critical_alert() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("DA03: skipped — MQK_DATABASE_URL not set");
        return;
    }

    mqk_db::run_isolated("da03", |pool| async move {
        let sink = start_sink(StatusCode::OK).await;

        let symbol = "DA03MISSING";
        let timeframe = "1Min";
        seed_no_bars(&pool, symbol, timeframe).await;

        let st = db_state_with_notifier(pool, DiscordNotifier::from_url(&sink.url)).await;
        st.set_per_symbol_bar_staleness_secs_for_test(Some(300));
        st.deposit_strategy_bar_input(test_bar_input(1)).await;

        let result = st.tick_strategy_dispatch_for_symbol(symbol, timeframe).await;
        assert!(result.is_none(), "DA03: missing bar must refuse dispatch");

        settle().await;
        assert_eq!(
            sink.request_count.load(Ordering::SeqCst),
            1,
            "DA03: a missing-bar refusal must also deliver a critical alert"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// DA04 — repeated stale ticks dedup to exactly one alert
// ---------------------------------------------------------------------------

#[tokio::test]
async fn da04_repeated_stale_ticks_dedup_to_one_alert() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("DA04: skipped — MQK_DATABASE_URL not set");
        return;
    }

    mqk_db::run_isolated("da04", |pool| async move {
        let sink = start_sink(StatusCode::OK).await;

        let symbol = "DA04REPEAT";
        let timeframe = "1Min";
        let now_ts = Utc::now().timestamp();
        seed_one_bar(&pool, symbol, timeframe, now_ts - 10_000).await;

        let st = db_state_with_notifier(pool, DiscordNotifier::from_url(&sink.url)).await;
        st.set_per_symbol_bar_staleness_secs_for_test(Some(300));

        for tick in 1..=3u64 {
            st.deposit_strategy_bar_input(test_bar_input(tick)).await;
            let result = st.tick_strategy_dispatch_for_symbol(symbol, timeframe).await;
            assert!(result.is_none(), "DA04: every stale tick must refuse dispatch");
        }

        settle().await;
        assert_eq!(
            sink.request_count.load(Ordering::SeqCst),
            1,
            "DA04: three consecutive stale ticks for the same symbol must produce \
             exactly one alert, not one per tick"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// DA05 — Discord delivery failure never affects the dispatch refusal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn da05_discord_delivery_failure_does_not_affect_dispatch_refusal() {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("DA05: skipped — MQK_DATABASE_URL not set");
        return;
    }

    mqk_db::run_isolated("da05", |pool| async move {
        let sink = start_sink(StatusCode::INTERNAL_SERVER_ERROR).await;

        let symbol = "DA05FAIL";
        let timeframe = "1Min";
        let now_ts = Utc::now().timestamp();
        seed_one_bar(&pool, symbol, timeframe, now_ts - 10_000).await;

        let st = db_state_with_notifier(pool, DiscordNotifier::from_url(&sink.url)).await;
        st.set_per_symbol_bar_staleness_secs_for_test(Some(300));
        st.deposit_strategy_bar_input(test_bar_input(1)).await;

        let result = st.tick_strategy_dispatch_for_symbol(symbol, timeframe).await;
        assert!(
            result.is_none(),
            "DA05: dispatch refusal must be identical regardless of Discord delivery outcome"
        );

        settle().await;
        assert_eq!(
            sink.request_count.load(Ordering::SeqCst),
            1,
            "DA05: delivery must still be attempted (and fail) without panicking or blocking"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// DA06 — dedup claim is a pure per-(run, symbol) primitive (no DB needed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn da06_staleness_alert_dedup_claim_is_per_run_per_symbol() {
    // Pure dedup-primitive proof; no DB required.
    let st = Arc::new(AppState::new());

    assert!(
        st.try_claim_md_staleness_alert_for_test("AAPL").await,
        "DA06: first claim for a symbol must succeed"
    );
    assert!(
        !st.try_claim_md_staleness_alert_for_test("AAPL").await,
        "DA06: second claim for the same symbol in the same run must fail (dedup)"
    );
    assert!(
        st.try_claim_md_staleness_alert_for_test("MSFT").await,
        "DA06: a different symbol's claim must be independent"
    );
}

// ---------------------------------------------------------------------------
// DA07 — no DB pool at all: the single-stub fallback runs, never the
// staleness gate, so no alert fires. No Postgres of any kind is needed for
// this test — it proves behavior when `AppState.db` itself is `None`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn da07_no_db_pool_never_reaches_staleness_gate_no_alert() {
    let sink = start_sink(StatusCode::OK).await;

    let mut st = AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Paper,
    );
    st.discord_notifier = DiscordNotifier::from_url(&sink.url);
    st.set_native_strategy_bootstrap_for_test(Some(active_bootstrap()))
        .await;
    let st = Arc::new(st);
    st.set_per_symbol_bar_staleness_secs_for_test(Some(300));
    st.deposit_strategy_bar_input(test_bar_input(1)).await;

    // No DB pool configured -> dispatch_native_strategy_for_symbol_with_bar_and_facts
    // takes the single-stub fallback path unconditionally; the staleness
    // gate (and therefore maybe_alert_md_staleness) is never reached.
    let _ = st
        .tick_strategy_dispatch_for_symbol("DA07NODB", "1Min")
        .await;

    settle().await;
    assert_eq!(
        sink.request_count.load(Ordering::SeqCst),
        0,
        "DA07: with no DB pool, the staleness gate never runs, so no alert can fire"
    );
}

// ---------------------------------------------------------------------------
// DA08 — dedup resets at run start: a symbol that already alerted this run
// may alert again next run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn da08_dedup_resets_at_run_start_allows_realert_next_run() {
    // Pure dedup-primitive proof at the reset seam; no DB required — mirrors
    // DA06 but also exercises reset_signal_blocked_alert_state (the same
    // reset the run-start lifecycle path calls).
    let st = AppState::new();

    assert!(
        st.try_claim_md_staleness_alert_for_test("AAPL").await,
        "DA08: first claim for a symbol must succeed"
    );
    assert!(
        !st.try_claim_md_staleness_alert_for_test("AAPL").await,
        "DA08: second claim in the same run must fail (dedup)"
    );

    st.reset_signal_blocked_alert_state_for_test();

    assert!(
        st.try_claim_md_staleness_alert_for_test("AAPL").await,
        "DA08: after a run-start reset, the same symbol must be claimable again"
    );
}
