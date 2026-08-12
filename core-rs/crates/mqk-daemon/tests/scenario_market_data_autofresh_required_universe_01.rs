//! MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01: end-to-end required-universe
//! controller proofs against a real local Postgres.
//!
//! Every DB-backed test skips truthfully (prints why, returns without
//! asserting) when `MQK_DATABASE_URL` is not set — mirrors the existing
//! pattern in `scenario_market_data_latest_bar_scheduler_01.rs`. No real
//! Alpaca/TwelveData network call is made anywhere in this file: every
//! provider call goes through an injected fake.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use chrono::{DateTime, TimeZone, Utc};
use mqk_daemon::state::market_calendar::{
    MarketCalendarProvider, MarketSessionState, MarketSessionTruth,
};
use mqk_daemon::state::required_market_data_autofresh::{
    run_required_universe_cycle, start_required_universe_scheduler,
    stop_required_universe_scheduler,
};
use tempfile::NamedTempFile;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Injected fake provider supporting both historical-bars bootstrap and
/// latest-closed-bar intraday polling, so the controller's full two-phase
/// behavior (§17/§18) can be proven without any real network call.
struct FakeUniverseProvider {
    provider_id: &'static str,
    /// Per-(provider_symbol) historical bars to return from
    /// `fetch_historical_bars` (bootstrap phase).
    historical: Mutex<HashMap<String, Vec<mqk_md::CanonicalBar>>>,
    /// Per-(provider_symbol) latest bar to return from
    /// `fetch_latest_closed_bar` (intraday phase). `None` entry means "no
    /// bar available yet".
    latest: Mutex<HashMap<String, Option<mqk_md::CanonicalBar>>>,
    /// When set, every call for this provider_symbol fails with a
    /// provider-rejected error (used for the partial-provider-failure
    /// proof).
    fail_symbols: Mutex<std::collections::HashSet<String>>,
    historical_calls: AtomicUsize,
    latest_calls: AtomicUsize,
}

impl FakeUniverseProvider {
    fn new(provider_id: &'static str) -> Self {
        Self {
            provider_id,
            historical: Mutex::new(HashMap::new()),
            latest: Mutex::new(HashMap::new()),
            fail_symbols: Mutex::new(std::collections::HashSet::new()),
            historical_calls: AtomicUsize::new(0),
            latest_calls: AtomicUsize::new(0),
        }
    }

    fn seed_historical(&self, provider_symbol: &str, bars: Vec<mqk_md::CanonicalBar>) {
        self.historical
            .lock()
            .unwrap()
            .insert(provider_symbol.to_string(), bars);
    }

    fn seed_latest(&self, provider_symbol: &str, bar: Option<mqk_md::CanonicalBar>) {
        self.latest
            .lock()
            .unwrap()
            .insert(provider_symbol.to_string(), bar);
    }

    fn fail(&self, provider_symbol: &str) {
        self.fail_symbols
            .lock()
            .unwrap()
            .insert(provider_symbol.to_string());
    }

    fn historical_calls(&self) -> usize {
        self.historical_calls.load(Ordering::SeqCst)
    }

    fn latest_calls(&self) -> usize {
        self.latest_calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl mqk_md::MarketDataProvider for FakeUniverseProvider {
    fn provider_id(&self) -> &str {
        self.provider_id
    }

    fn display_name(&self) -> &str {
        self.provider_id
    }

    fn capabilities(&self) -> mqk_md::MarketDataProviderCapabilities {
        mqk_md::MarketDataProviderCapabilities {
            historical_bars: true,
            latest_closed_bar: true,
            completed_bar_stream: false,
            supported_asset_classes: vec![mqk_md::ProviderAssetClass::Equity],
            supported_timeframes: vec![
                mqk_md::Timeframe::M1,
                mqk_md::Timeframe::M5,
                mqk_md::Timeframe::D1,
            ],
        }
    }

    fn health(&self) -> mqk_md::MarketDataProviderHealth {
        mqk_md::MarketDataProviderHealth::unknown()
    }

    fn rate_limits(&self) -> Option<mqk_md::MarketDataProviderRateLimits> {
        None
    }

    async fn fetch_historical_bars(
        &self,
        request: mqk_md::HistoricalBarsRequest,
    ) -> Result<Vec<mqk_md::CanonicalBar>, mqk_md::MarketDataProviderError> {
        self.historical_calls.fetch_add(1, Ordering::SeqCst);
        let symbol = request.symbols.first().cloned().unwrap_or_default();
        if self.fail_symbols.lock().unwrap().contains(&symbol) {
            return Err(mqk_md::MarketDataProviderError::Other {
                provider_id: self.provider_id.to_string(),
                message: "fixture-forced failure".to_string(),
            });
        }
        Ok(self
            .historical
            .lock()
            .unwrap()
            .get(&symbol)
            .cloned()
            .unwrap_or_default())
    }

    async fn fetch_latest_closed_bar(
        &self,
        request: mqk_md::LatestClosedBarRequest,
    ) -> Result<Option<mqk_md::CanonicalBar>, mqk_md::MarketDataProviderError> {
        self.latest_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_symbols.lock().unwrap().contains(&request.symbol) {
            return Err(mqk_md::MarketDataProviderError::Other {
                provider_id: self.provider_id.to_string(),
                message: "fixture-forced failure".to_string(),
            });
        }
        Ok(self
            .latest
            .lock()
            .unwrap()
            .get(&request.symbol)
            .cloned()
            .flatten())
    }
}

/// REPAIR-02 §15: a provider whose very first `fetch_historical_bars` call
/// parks on a test-controlled barrier before delegating to an inner
/// [`FakeUniverseProvider`] -- used only by
/// `stop_start_generation_race_old_cycle_cannot_overwrite_new_owner` to
/// reproduce the stop/restart ABA ownership race deterministically (no real
/// network call, no timing-based sleep). Only the first call blocks: every
/// call after that (i.e. a second scheduler generation's own call, made
/// while the first is still parked) passes straight through, exactly
/// mirroring "generation B establishes ownership while generation A's
/// delayed provider call is still in flight".
struct BarrierDelayedProvider {
    inner: FakeUniverseProvider,
    blocked_once: AtomicUsize,
    call_started: Arc<tokio::sync::Notify>,
    release_gate: Arc<tokio::sync::Notify>,
}

impl BarrierDelayedProvider {
    fn new(
        provider_id: &'static str,
        call_started: Arc<tokio::sync::Notify>,
        release_gate: Arc<tokio::sync::Notify>,
    ) -> Self {
        Self {
            inner: FakeUniverseProvider::new(provider_id),
            blocked_once: AtomicUsize::new(0),
            call_started,
            release_gate,
        }
    }
}

#[async_trait::async_trait]
impl mqk_md::MarketDataProvider for BarrierDelayedProvider {
    fn provider_id(&self) -> &str {
        self.inner.provider_id()
    }

    fn display_name(&self) -> &str {
        self.inner.display_name()
    }

    fn capabilities(&self) -> mqk_md::MarketDataProviderCapabilities {
        self.inner.capabilities()
    }

    fn health(&self) -> mqk_md::MarketDataProviderHealth {
        self.inner.health()
    }

    fn rate_limits(&self) -> Option<mqk_md::MarketDataProviderRateLimits> {
        self.inner.rate_limits()
    }

    async fn fetch_historical_bars(
        &self,
        request: mqk_md::HistoricalBarsRequest,
    ) -> Result<Vec<mqk_md::CanonicalBar>, mqk_md::MarketDataProviderError> {
        if self.blocked_once.fetch_add(1, Ordering::SeqCst) == 0 {
            self.call_started.notify_one();
            self.release_gate.notified().await;
        }
        self.inner.fetch_historical_bars(request).await
    }

    async fn fetch_latest_closed_bar(
        &self,
        request: mqk_md::LatestClosedBarRequest,
    ) -> Result<Option<mqk_md::CanonicalBar>, mqk_md::MarketDataProviderError> {
        self.inner.fetch_latest_closed_bar(request).await
    }
}

/// A fixed [`MarketCalendarProvider`] fixture: always returns the same
/// [`MarketSessionTruth`] regardless of `now_utc`. Used to make session/
/// calendar-dependent behavior (trading day, early close, session close)
/// deterministic and independent of the real wall-clock date the test suite
/// happens to run on.
struct FixedCalendar {
    truth: MarketSessionTruth,
}

impl MarketCalendarProvider for FixedCalendar {
    fn session_for(&self, _now_utc: DateTime<Utc>) -> MarketSessionTruth {
        self.truth.clone()
    }
}

fn trading_day_calendar() -> FixedCalendar {
    FixedCalendar {
        truth: MarketSessionTruth {
            state: MarketSessionState::RegularOpen,
            source: "test_fixture",
            exchange: "TEST",
            is_trading_day: true,
            is_early_close: false,
            session_close_note: None,
        },
    }
}

fn non_trading_day_calendar() -> FixedCalendar {
    FixedCalendar {
        truth: MarketSessionTruth {
            state: MarketSessionState::Closed,
            source: "test_fixture",
            exchange: "TEST",
            is_trading_day: false,
            is_early_close: false,
            session_close_note: None,
        },
    }
}

fn bar(symbol: &str, timeframe: &str, end_ts: i64) -> mqk_md::CanonicalBar {
    mqk_md::CanonicalBar {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        end_ts,
        open: "100".to_string(),
        high: "101".to_string(),
        low: "99".to_string(),
        close: "100.5".to_string(),
        volume: 1000,
        is_complete: true,
    }
}

/// Five completed 5m bars ending at `end_ts`, ten minutes apart (comfortably
/// clears `market_data_freshness::MD_FRESHNESS_MIN_BARS`).
fn five_bars(symbol: &str, end_ts: i64) -> Vec<mqk_md::CanonicalBar> {
    (0..5)
        .map(|i| bar(symbol, "5m", end_ts - (4 - i) * 300))
        .collect()
}

/// `n` completed 5m bars ending at start-label `last_start_ts`, spaced
/// exactly one interval (300s) apart. Generalizes `five_bars` to prove
/// REPAIR-01 §4/§29 -- a strategy that genuinely needs more than 5 bars of
/// history must not be reported ready merely because 5 exist.
fn n_bars(symbol: &str, last_start_ts: i64, n: usize) -> Vec<mqk_md::CanonicalBar> {
    (0..n)
        .map(|i| bar(symbol, "5m", last_start_ts - ((n - 1 - i) as i64) * 300))
        .collect()
}

fn instrument_registry_file(entries: &[(&str, &str, &str)]) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("create fake instrument registry");
    let json: Vec<serde_json::Value> = entries
        .iter()
        .map(|(symbol, provider, timeframe)| {
            serde_json::json!({
                "instrument_id": format!("equity:US:{symbol}"),
                "symbol": symbol,
                "asset_class": "equity",
                "provider": provider,
                "provider_symbol": symbol,
                "venue": "TEST",
                "currency": "USD",
                "enabled": true,
                "timeframes": [timeframe],
                "notes": "required-universe test fixture"
            })
        })
        .collect();
    file.write_all(serde_json::Value::Array(json).to_string().as_bytes())
        .expect("write fake instrument registry");
    file
}

fn provider_registry_file(provider_ids: &[&str]) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("create fake provider registry");
    let json: Vec<serde_json::Value> = provider_ids
        .iter()
        .map(|id| {
            serde_json::json!({
                "provider_id": id,
                "display_name": id,
                "asset_classes": ["equity", "etf"],
                "free_tier_available": true,
                "api_key_required": false,
                "credential_env_vars": [],
                "rate_limit_notes": "test",
                "supported_timeframes": ["1D", "1m", "5m"],
                "historical_depth_notes": "test",
                "realtime_support_notes": "test",
                "licensing_notes": "test",
                "implementation_status": "implemented_equity_provider",
                "enabled": true,
                "verification_status": "repo_implemented_official_limits_unverified",
                "docs_url": ""
            })
        })
        .collect();
    file.write_all(serde_json::Value::Array(json).to_string().as_bytes())
        .expect("write fake provider registry");
    file
}

/// A structurally valid, approved `watchlist-v2` artifact fixture covering
/// exactly `symbols`, with a strategy assignment for every symbol (required
/// by `evaluate_watchlist_intake`'s v2 validation).
fn watchlist_v2_file(symbols: &[&str]) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("create watchlist temp file");
    let strategy_assignments: serde_json::Map<String, serde_json::Value> = symbols
        .iter()
        .map(|s| (s.to_string(), serde_json::json!("intraday_scalper")))
        .collect();
    let json = serde_json::json!({
        "schema_version": "watchlist-v2",
        "mode": "paper",
        "approved_for_live": false,
        "approved_for_autonomous_paper": true,
        "symbols": symbols,
        "strategy_assignments": strategy_assignments,
        "max_symbols_to_trade": symbols.len().max(1),
        "max_concurrent_positions": symbols.len().max(1),
    });
    file.write_all(json.to_string().as_bytes())
        .expect("write watchlist fixture");
    file
}

async fn maybe_db(label: &str) -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            eprintln!("{label}: skipped DB-backed proof because MQK_DATABASE_URL is not set");
            return None;
        }
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connect MQK_DATABASE_URL");
    mqk_db::migrate(&pool).await.expect("run migrations");
    sqlx::query("delete from md_bars where symbol like 'ZZAUTOFR%'")
        .execute(&pool)
        .await
        .expect("clean autofresh md_bars test rows");
    Some(pool)
}

fn now_fixture() -> DateTime<Utc> {
    // A fixed Wednesday mid-session instant, independent of the real wall
    // clock the test suite happens to run under.
    Utc.with_ymd_and_hms(2026, 8, 12, 15, 30, 0).unwrap()
}

// ---------------------------------------------------------------------------
// MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01
//
// `run_required_universe_cycle`'s reused production ingest seam durably
// writes `md_bars.ingested_at` from the real database server clock
// (`timestamptz not null default now()`, migration 0003 -- predates the
// "no DEFAULT now()" rule and is out of scope for this test-only patch to
// change, since it is production write-path schema/code).
// `daily_data_readiness::evaluate_bar_readiness`'s own unmodified
// `REASON_PROVIDER_INGEST_TIME_FUTURE` check (still enforced, at its
// unmodified default 300s `MQK_DATA_READINESS_FUTURE_SKEW_SECS` tolerance --
// this patch changes neither) compares that real DB timestamp against
// whatever fixed `now_utc` a test supplies for evaluation (`now_fixture()`
// or a test's own hardcoded calendar instant, e.g. the preopen test below).
// A fixed evaluation instant inevitably drifts more than 300s behind the
// real wall clock as real time passes after the fixture was authored,
// producing a false `provider_ingest_time_future` blocker unrelated to any
// actual defect -- reproducible on unmodified `fde6e227` before REPAIR-02.
//
// The fix closes that gap the only way available without touching
// production ingest/readiness code: immediately after a real (unmodified)
// write, re-stamp `ingested_at` on the rows that write just produced to a
// value consistent with the test's own chosen evaluation instant. This
// keeps every test's synthetic evaluation time and its own DB provenance
// timestamps internally consistent forever, regardless of how much real
// wall-clock time has elapsed since the fixture value was written -- it
// does not touch `daily_data_readiness.rs`, does not change the future-skew
// tolerance, and does not touch any production ingest/write path.
// ---------------------------------------------------------------------------

/// Re-stamp `ingested_at` on every `ZZAUTOFR%` row durably written no
/// earlier than `wrote_after` to `evaluation_now` (minus a small safety
/// buffer, comfortably inside the unmodified 300s default skew tolerance).
/// A no-op when the preceding write produced nothing (e.g. an empty/failed
/// provider response) or when `pool` is `None`. Scoping by `wrote_after`
/// (a real-clock checkpoint captured immediately before the write) rather
/// than an unqualified `symbol like 'ZZAUTOFR%'` update means an earlier
/// cycle's already-correct rows in the same test (e.g. a test with multiple
/// cycles at different `now_utc` values) are never touched by a later
/// cycle's stamp.
async fn stamp_recent_ingested_at_for_test(
    pool: Option<&sqlx::PgPool>,
    evaluation_now: DateTime<Utc>,
    wrote_after: DateTime<Utc>,
) {
    let Some(pool) = pool else { return };
    let stamp = evaluation_now - chrono::Duration::seconds(5);
    sqlx::query(
        "update md_bars set ingested_at = $1 where symbol like 'ZZAUTOFR%' and ingested_at >= $2",
    )
    .bind(stamp)
    .bind(wrote_after)
    .execute(pool)
    .await
    .expect("MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01: deterministic ingested_at test stamp");
}

/// Drop-in replacement for a direct `run_required_universe_cycle` call: runs
/// the real, unmodified cycle, then applies
/// [`stamp_recent_ingested_at_for_test`] to whatever it just wrote. Every
/// test in this file whose assertions depend on the ingest path being
/// genuinely fresh relative to its own chosen `now_utc` (i.e. expects
/// `freshness_state == "ok"` after a real bootstrap/poll) calls this instead
/// of `run_required_universe_cycle` directly.
async fn run_cycle_with_deterministic_ingest_stamp(
    pool: Option<&sqlx::PgPool>,
    instrument_registry_path: &str,
    provider_registry_path: &str,
    injected_provider_client: Option<&Arc<dyn mqk_md::MarketDataProvider>>,
    calendar_provider: &dyn MarketCalendarProvider,
    cycle_now: DateTime<Utc>,
    dry_run: bool,
) -> mqk_daemon::state::required_market_data_autofresh::RequiredUniverseCycleResult {
    let wrote_after = Utc::now();
    let result = run_required_universe_cycle(
        pool,
        instrument_registry_path,
        provider_registry_path,
        injected_provider_client,
        calendar_provider,
        cycle_now,
        dry_run,
    )
    .await;
    stamp_recent_ingested_at_for_test(pool, cycle_now, wrote_after).await;
    result
}

// ---------------------------------------------------------------------------
// §36 test 1: AAPL/5m positive proof
// ---------------------------------------------------------------------------

#[tokio::test]
async fn aapl_5m_positive_proof_bootstraps_then_stays_ready() {
    let Some(pool) = maybe_db("aapl_5m_positive_proof").await else {
        return;
    };
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZAUTOFRAAPL");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    // REPAIR-01 Defect A: the controller now resolves each required symbol's
    // strategy assignment (via `build_multi_symbol_runtime_config_from_env`)
    // to source `required_history_bars` from the strategy that will actually
    // run, instead of a hardcoded 5-bar minimum -- the legacy single-symbol
    // env path needs MQK_STRATEGY_IDS set for that resolution to succeed.
    // `intraday_scalper`'s own LOOKBACK is 5, matching `five_bars()` below.
    std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");

    let instruments = instrument_registry_file(&[("ZZAUTOFRAAPL", "alpaca", "5m")]);
    let providers = provider_registry_file(&["alpaca"]);
    let now = now_fixture();
    let provider = Arc::new(FakeUniverseProvider::new("alpaca"));
    provider.seed_historical(
        "ZZAUTOFRAAPL",
        // REPAIR-01 §6: the strict readiness authority's expected window is
        // publication-grace-aware -- with the default 900s configured grace
        // (effective_grace_seconds = min(900, 300) = 300 for this 5m
        // timeframe), the bar spanning [now-300, now) has not yet cleared
        // its grace period at `now` itself, so the expected window's last
        // slot is one interval earlier than `now - interval` alone would
        // suggest. `now.timestamp() - 600` lands the fixture's last bar
        // exactly on that grace-aware expected slot.
        five_bars("ZZAUTOFRAAPL", now.timestamp() - 600),
    );
    let injected: Arc<dyn mqk_md::MarketDataProvider> = provider.clone();
    let calendar = trading_day_calendar();

    // Cycle 1: no bars in md_bars yet -> missing -> bounded historical
    // bootstrap -> re-evaluated readiness should become ready.
    let result = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        now,
        false,
    )
    .await;

    // MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01: cycle 1's own report
    // reflects `md_bars.ingested_at` as the real database server clock
    // stamped it *during this same call* -- `run_cycle_with_deterministic_
    // ingest_stamp`'s post-call re-stamp cannot retroactively change a
    // report object already returned from *this* call, only what a *later*
    // call reads. So cycle 1 only asserts the structural facts that do not
    // depend on `ingested_at` (which requirement, which provider, that the
    // bootstrap call actually happened); cycle 2 below -- which reads the
    // now-re-stamped row fresh -- is where "genuinely ready" is proven.
    assert_eq!(result.report.requirements.len(), 1);
    let req = &result.report.requirements[0];
    assert_eq!(req.symbol, "ZZAUTOFRAAPL");
    assert_eq!(req.provider_id.as_deref(), Some("alpaca"));
    assert_eq!(provider.historical_calls(), 1);

    // Cycle 2: bars already sufficient/fresh -> readiness stays ready, no
    // further historical bootstrap should be needed (already ok). This is
    // also where "genuinely ready" is proven -- see the time-determinism
    // note above.
    let result2 = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        now,
        false,
    )
    .await;
    assert_eq!(result2.report.overall_state, "ready");
    assert_eq!(
        provider.historical_calls(),
        1,
        "no repeated historical bootstrap once already ready"
    );

    // Cycle 3: a newer completed bar becomes available -> intraday poll
    // ingests it, readiness remains ready.
    //
    // REPAIR-01 §6/§24: the strict readiness authority's expected window is
    // publication-grace-aware, so the newly-expected slot at `later_now` is
    // the grid slot starting exactly one interval before `now` (`now -
    // 300`) -- not an arbitrary next epoch-5m-boundary. Advancing the clock
    // past that slot's own grace period (interval + grace = 600s from the
    // slot's start) and seeding exactly that bar is what makes the strict
    // evaluator's `expected_latest_bar_missing` -> bounded, constrained
    // latest-bar poll -> `ready` sequence line up.
    let new_bar_start_ts = now.timestamp() - 300;
    provider.seed_latest(
        "ZZAUTOFRAAPL",
        Some(bar("ZZAUTOFRAAPL", "5m", new_bar_start_ts)),
    );
    let later_now = Utc.timestamp_opt(now.timestamp() + 330, 0).unwrap();
    let result3 = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        later_now,
        false,
    )
    .await;
    // MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01: cycle 3's own intraday
    // poll ingests the new bar and re-evaluates within this same call, so --
    // like cycle 1 above -- its own report reflects `md_bars.ingested_at` as
    // the real database server clock stamped it *during this same call*,
    // before the re-stamp can apply. Only assert the structural fact that
    // doesn't depend on it; cycle 4 below, reading the now-re-stamped row
    // fresh, is where "genuinely ready" is proven.
    assert_eq!(result3.report.requirements.len(), 1, "{:?}", result3.report);

    let result4 = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        later_now,
        false,
    )
    .await;
    assert_eq!(
        result4.report.overall_state, "ready",
        "{:?}",
        result4.report
    );

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    std::env::remove_var("MQK_STRATEGY_IDS");
}

// ---------------------------------------------------------------------------
// §36 test 2: multi-symbol positive proof
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_symbol_positive_proof_every_symbol_evaluated_and_refreshed() {
    let Some(pool) = maybe_db("multi_symbol_positive_proof").await else {
        return;
    };
    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    // No watchlist configured: fall back to legacy single-symbol env would
    // only cover one symbol, so this test drives the plan directly with a
    // watchlist-v2 artifact instead, to exercise a genuine 3-symbol universe.
    let watchlist_file = watchlist_v2_file(&["ZZAUTOFR1", "ZZAUTOFR2", "ZZAUTOFR3"]);
    std::env::set_var("MQK_PAPER_WATCHLIST_PATH", watchlist_file.path());

    let instruments = instrument_registry_file(&[
        ("ZZAUTOFR1", "alpaca", "5m"),
        ("ZZAUTOFR2", "alpaca", "5m"),
        ("ZZAUTOFR3", "alpaca", "5m"),
    ]);
    let providers = provider_registry_file(&["alpaca"]);
    let now = now_fixture();
    let provider = Arc::new(FakeUniverseProvider::new("alpaca"));
    for symbol in ["ZZAUTOFR1", "ZZAUTOFR2", "ZZAUTOFR3"] {
        // See the grace-aware offset note on the AAPL/5m positive proof above.
        provider.seed_historical(symbol, five_bars(symbol, now.timestamp() - 600));
    }
    let injected: Arc<dyn mqk_md::MarketDataProvider> = provider.clone();
    let calendar = trading_day_calendar();

    let result = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        now,
        false,
    )
    .await;

    assert_eq!(
        result.report.requirements.len(),
        3,
        "every required symbol must be evaluated"
    );
    let mut symbols: Vec<&str> = result
        .report
        .requirements
        .iter()
        .map(|r| r.symbol.as_str())
        .collect();
    symbols.sort();
    assert_eq!(symbols, vec!["ZZAUTOFR1", "ZZAUTOFR2", "ZZAUTOFR3"]);
    assert_eq!(
        provider.historical_calls(),
        3,
        "one bootstrap call per required symbol"
    );

    // MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01: `result`'s own report
    // reflects `md_bars.ingested_at` as the real database server clock
    // stamped it *during this same call* -- see the identical note on the
    // AAPL/5m positive proof above. A second cycle, reading the rows fresh
    // after `run_cycle_with_deterministic_ingest_stamp`'s re-stamp has
    // applied, is where "every symbol genuinely ready" is proven; no new
    // bootstrap call should be dispatched (data already sufficient).
    let result_reevaluated = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        now,
        false,
    )
    .await;
    assert!(
        result_reevaluated
            .report
            .requirements
            .iter()
            .all(|r| r.freshness_state == "ok"),
        "{:?}",
        result_reevaluated.report.requirements
    );
    assert_eq!(result_reevaluated.report.overall_state, "ready");
    assert_eq!(
        provider.historical_calls(),
        3,
        "no repeated bootstrap once already ready"
    );

    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
}

// ---------------------------------------------------------------------------
// §36 test 3: mixed-provider proof
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mixed_provider_proof_two_groups_partial_failure_does_not_block_the_other() {
    let Some(pool) = maybe_db("mixed_provider_proof").await else {
        return;
    };
    let watchlist_file = watchlist_v2_file(&["ZZAUTOFRA", "ZZAUTOFRM"]);
    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    std::env::set_var("MQK_PAPER_WATCHLIST_PATH", watchlist_file.path());

    let instruments = instrument_registry_file(&[
        ("ZZAUTOFRA", "alpaca", "5m"),
        ("ZZAUTOFRM", "twelvedata", "5m"),
    ]);
    let providers = provider_registry_file(&["alpaca", "twelvedata"]);
    let now = now_fixture();

    let alpaca = Arc::new(FakeUniverseProvider::new("alpaca"));
    // See the grace-aware offset note on the AAPL/5m positive proof above.
    alpaca.seed_historical("ZZAUTOFRA", five_bars("ZZAUTOFRA", now.timestamp() - 600));
    let twelvedata = Arc::new(FakeUniverseProvider::new("twelvedata"));
    twelvedata.fail("ZZAUTOFRM"); // provider B fails

    // `run_required_universe_cycle` accepts one injected client -- to
    // exercise genuinely distinct providers per group we run the resolver's
    // plan directly and dispatch each group through its own client, mirroring
    // what the production (non-injected) path does per group in sequence.
    use mqk_daemon::market_data_freshness::required_symbols_with_source_from_env;
    use mqk_daemon::state::market_calendar::resolve_market_session_schedule;
    use mqk_daemon::state::required_market_data_autofresh::build_required_market_data_refresh_plan;

    let calendar = trading_day_calendar();
    let resolution = required_symbols_with_source_from_env();
    let schedule = resolve_market_session_schedule(&calendar, now);
    let loaded_instruments =
        mqk_md::instrument_registry::load_instrument_registry(instruments.path()).unwrap();
    let loaded_providers =
        mqk_md::provider_registry::load_provider_registry(providers.path()).unwrap();
    let plan = build_required_market_data_refresh_plan(
        &loaded_instruments,
        &loaded_providers,
        &resolution,
        schedule.market_date,
    );

    assert_eq!(plan.groups.len(), 2, "two distinct provider groups");
    assert!(plan.groups.iter().any(|g| g.provider_id == "alpaca"));
    assert!(plan.groups.iter().any(|g| g.provider_id == "twelvedata"));

    // Run cycles with each provider injected once, unioning md_bars writes,
    // to prove provider A's success is durably recorded independent of
    // provider B's failure.
    let alpaca_dyn: Arc<dyn mqk_md::MarketDataProvider> = alpaca.clone();
    let cycle_a = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&alpaca_dyn),
        &calendar,
        now,
        false,
    )
    .await;
    // MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01: `cycle_a`'s own report
    // reflects `md_bars.ingested_at` as the real database server clock
    // stamped it *during this same call* -- see the identical note on the
    // AAPL/5m positive proof above. "genuinely ok" is proven below via
    // `a_status_in_cycle_b`, evaluated fresh during `cycle_b` after the
    // re-stamp from this call has already applied.
    let a_status = cycle_a
        .report
        .requirements
        .iter()
        .find(|r| r.symbol == "ZZAUTOFRA")
        .unwrap();
    assert_eq!(a_status.symbol, "ZZAUTOFRA");

    let twelvedata_dyn: Arc<dyn mqk_md::MarketDataProvider> = twelvedata.clone();
    let cycle_b = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&twelvedata_dyn),
        &calendar,
        now,
        false,
    )
    .await;
    let m_status = cycle_b
        .report
        .requirements
        .iter()
        .find(|r| r.symbol == "ZZAUTOFRM")
        .unwrap();
    assert_ne!(m_status.freshness_state, "ok");
    assert_eq!(cycle_b.report.overall_state, "blocked");
    // Provider A's earlier success must still be durably present -- proven
    // by re-reading its status in the same (provider-B-injected) cycle.
    let a_status_in_cycle_b = cycle_b
        .report
        .requirements
        .iter()
        .find(|r| r.symbol == "ZZAUTOFRA")
        .unwrap();
    assert_eq!(
        a_status_in_cycle_b.freshness_state, "ok",
        "provider A's durable success must not be affected by provider B's failure"
    );

    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
}

// ---------------------------------------------------------------------------
// §36 test 4: wrong-provider negative control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wrong_provider_provenance_negative_control_never_auto_repaired() {
    let Some(pool) = maybe_db("wrong_provider_provenance_negative_control").await else {
        return;
    };
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZAUTOFRWRONG");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");

    let instruments = instrument_registry_file(&[("ZZAUTOFRWRONG", "alpaca", "5m")]);
    let providers = provider_registry_file(&["alpaca", "twelvedata"]);
    let now = now_fixture();

    // Directly insert a completed bar stamped provider_id='twelvedata' even
    // though the registry says alpaca -- simulates a mis-stamped row without
    // going through the normal ingest seam.
    let ingest_id = uuid::Uuid::new_v4();
    mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata(
        &pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: "twelvedata".to_string(),
            timeframe: "5m".to_string(),
            ingest_id,
            bars: five_bars("ZZAUTOFRWRONG", now.timestamp() - 300)
                .into_iter()
                .map(|b| mqk_db::md::ProviderBar {
                    symbol: b.symbol,
                    timeframe: b.timeframe,
                    end_ts: b.end_ts,
                    open: b.open,
                    high: b.high,
                    low: b.low,
                    close: b.close,
                    volume: b.volume,
                    is_complete: b.is_complete,
                })
                .collect(),
        },
        mqk_db::md::MdBarProviderMetadata {
            provider_id: "twelvedata".to_string(),
            provider_source: Some("twelvedata".to_string()),
            provider_symbol: Some("ZZAUTOFRWRONG".to_string()),
            ingest_mode: Some("test_fixture_mis_stamped".to_string()),
            provider_bar_id: None,
            provider_updated_at_utc: None,
        },
    )
    .await
    .expect("seed mis-stamped bar");

    let provider = Arc::new(FakeUniverseProvider::new("alpaca"));
    let injected: Arc<dyn mqk_md::MarketDataProvider> = provider.clone();
    let calendar = trading_day_calendar();

    let result = run_required_universe_cycle(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        now,
        false,
    )
    .await;

    assert_eq!(result.report.overall_state, "blocked");
    let status = &result.report.requirements[0];
    assert_eq!(status.freshness_state, "provider_provenance_invalid");
    assert_eq!(
        provider.historical_calls(),
        0,
        "a provenance-invalid row must never trigger a bootstrap attempt"
    );

    // Confirm the mis-stamped row itself was left untouched (not "repaired").
    let row: (String,) = sqlx::query_as(
        "select provider_id from md_bars where symbol = $1 and timeframe = '5m' order by end_ts desc limit 1",
    )
    .bind("ZZAUTOFRWRONG")
    .fetch_one(&pool)
    .await
    .expect("read back mis-stamped row");
    assert_eq!(
        row.0, "twelvedata",
        "the mis-stamped row must not be rewritten"
    );

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
}

// ---------------------------------------------------------------------------
// §36 test 9: non-trading-day proof
// ---------------------------------------------------------------------------

#[tokio::test]
async fn non_trading_day_makes_zero_provider_calls() {
    let Some(pool) = maybe_db("non_trading_day_proof").await else {
        return;
    };
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZAUTOFRHOLIDAY");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");

    let instruments = instrument_registry_file(&[("ZZAUTOFRHOLIDAY", "alpaca", "5m")]);
    let providers = provider_registry_file(&["alpaca"]);
    let provider = Arc::new(FakeUniverseProvider::new("alpaca"));
    let injected: Arc<dyn mqk_md::MarketDataProvider> = provider.clone();
    let calendar = non_trading_day_calendar();

    let result = run_required_universe_cycle(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        now_fixture(),
        false,
    )
    .await;

    assert_eq!(result.report.overall_state, "not_applicable");
    assert_eq!(provider.historical_calls(), 0);
    assert_eq!(provider.latest_calls(), 0);
    assert_eq!(result.provider_api_calls_made, 0);

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
}

// ---------------------------------------------------------------------------
// §36 test 14: readiness load-bearing negative control (no partial green)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_stale_required_symbol_blocks_overall_readiness() {
    let Some(pool) = maybe_db("readiness_load_bearing_negative_control").await else {
        return;
    };
    let watchlist_file = watchlist_v2_file(&["ZZAUTOFRGOOD", "ZZAUTOFRBAD"]);
    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    std::env::set_var("MQK_PAPER_WATCHLIST_PATH", watchlist_file.path());

    let instruments = instrument_registry_file(&[
        ("ZZAUTOFRGOOD", "alpaca", "5m"),
        ("ZZAUTOFRBAD", "alpaca", "5m"),
    ]);
    let providers = provider_registry_file(&["alpaca"]);
    let now = now_fixture();
    let provider = Arc::new(FakeUniverseProvider::new("alpaca"));
    provider.seed_historical(
        "ZZAUTOFRGOOD",
        // See the grace-aware offset note on the AAPL/5m positive proof above.
        five_bars("ZZAUTOFRGOOD", now.timestamp() - 600),
    );
    // ZZAUTOFRBAD deliberately has no historical fixture seeded -> bootstrap
    // returns zero bars -> stays missing/blocked.
    let injected: Arc<dyn mqk_md::MarketDataProvider> = provider.clone();
    let calendar = trading_day_calendar();

    let result = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        now,
        false,
    )
    .await;

    let bad = result
        .report
        .requirements
        .iter()
        .find(|r| r.symbol == "ZZAUTOFRBAD")
        .unwrap();
    assert_ne!(bad.freshness_state, "ok");
    assert_eq!(
        result.report.overall_state, "blocked",
        "one stale/missing required symbol must block overall readiness even though the \
         other is ready -- no partial green"
    );

    // MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01: `result`'s own
    // ZZAUTOFRGOOD status reflects `md_bars.ingested_at` as the real
    // database server clock stamped it *during this same call* -- see the
    // identical note on the AAPL/5m positive proof above. Re-run to prove
    // ZZAUTOFRGOOD is genuinely ready (reading the now-re-stamped row
    // fresh) while ZZAUTOFRBAD (still never seeded) still blocks overall
    // readiness -- the actual "no partial green" contrast this test proves.
    let result2 = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        now,
        false,
    )
    .await;
    let good2 = result2
        .report
        .requirements
        .iter()
        .find(|r| r.symbol == "ZZAUTOFRGOOD")
        .unwrap();
    let bad2 = result2
        .report
        .requirements
        .iter()
        .find(|r| r.symbol == "ZZAUTOFRBAD")
        .unwrap();
    assert_eq!(good2.freshness_state, "ok", "{:?}", good2);
    assert_ne!(bad2.freshness_state, "ok");
    assert_eq!(
        result2.report.overall_state, "blocked",
        "ZZAUTOFRGOOD alone becoming ready must not flip overall readiness while \
         ZZAUTOFRBAD is still blocked -- no partial green"
    );

    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
}

// ---------------------------------------------------------------------------
// §46: checkonly/dry-run proof
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dry_run_makes_zero_provider_calls_and_zero_db_writes() {
    let Some(pool) = maybe_db("dry_run_checkonly_proof").await else {
        return;
    };
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZAUTOFRDRY");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");

    let instruments = instrument_registry_file(&[("ZZAUTOFRDRY", "alpaca", "5m")]);
    let providers = provider_registry_file(&["alpaca"]);
    let now = now_fixture();
    let provider = Arc::new(FakeUniverseProvider::new("alpaca"));
    provider.seed_historical(
        "ZZAUTOFRDRY",
        five_bars("ZZAUTOFRDRY", now.timestamp() - 300),
    );
    let injected: Arc<dyn mqk_md::MarketDataProvider> = provider.clone();
    let calendar = trading_day_calendar();

    let result = run_required_universe_cycle(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        now,
        true, // dry_run
    )
    .await;

    assert_eq!(result.provider_api_calls_made, 0);
    assert_eq!(provider.historical_calls(), 0);
    assert_eq!(provider.latest_calls(), 0);
    let row_count: (i64,) = sqlx::query_as("select count(*) from md_bars where symbol = $1")
        .bind("ZZAUTOFRDRY")
        .fetch_one(&pool)
        .await
        .expect("count md_bars rows");
    assert_eq!(row_count.0, 0, "dry_run must write zero md_bars rows");

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    std::env::remove_var("MQK_STRATEGY_IDS");
}

// ---------------------------------------------------------------------------
// §45: cross-surface shared-authority proof (ingest-plan vs autofresh)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ingest_plan_and_autofresh_plan_agree_on_required_symbols() {
    use mqk_daemon::market_data_freshness::required_symbols_with_source_from_env;
    use mqk_daemon::state::market_calendar::resolve_market_session_schedule;
    use mqk_daemon::state::required_market_data_autofresh::build_required_market_data_refresh_plan;

    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZAUTOFRSHARED");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");

    let instruments = instrument_registry_file(&[("ZZAUTOFRSHARED", "alpaca", "5m")]);
    let providers = provider_registry_file(&["alpaca"]);
    let calendar = trading_day_calendar();
    let now = now_fixture();

    // The exact same resolver call the ingest-plan route uses.
    let ingest_plan_resolution = required_symbols_with_source_from_env();
    // The exact same resolver call the autofresh controller uses internally.
    let autofresh_resolution = required_symbols_with_source_from_env();

    assert_eq!(
        ingest_plan_resolution.required, autofresh_resolution.required,
        "ingest-plan and autofresh must resolve the identical required set"
    );
    assert_eq!(ingest_plan_resolution.source, autofresh_resolution.source);

    let loaded_instruments =
        mqk_md::instrument_registry::load_instrument_registry(instruments.path()).unwrap();
    let loaded_providers =
        mqk_md::provider_registry::load_provider_registry(providers.path()).unwrap();
    let schedule = resolve_market_session_schedule(&calendar, now);
    let plan = build_required_market_data_refresh_plan(
        &loaded_instruments,
        &loaded_providers,
        &autofresh_resolution,
        schedule.market_date,
    );
    assert_eq!(plan.required_symbols(), vec!["ZZAUTOFRSHARED".to_string()]);

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
}

// ---------------------------------------------------------------------------
// MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-01 new load-bearing tests
// ---------------------------------------------------------------------------

/// §4/§29: a strategy whose real `required_history_bars` is above the old
/// evaluator's fixed 5-bar minimum must not be reported ready merely because
/// 5 bars exist. `swing_momentum`'s registered `StrategyDataRequirements`
/// needs 20 completed bars.
#[tokio::test]
async fn strategy_history_requirement_above_five_bars_is_not_satisfied_by_five_bars() {
    let Some(pool) = maybe_db("strategy_history_requirement_above_five_bars").await else {
        return;
    };
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZAUTOFRSWING");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    std::env::set_var("MQK_STRATEGY_IDS", "swing_momentum");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");

    let instruments = instrument_registry_file(&[("ZZAUTOFRSWING", "alpaca", "5m")]);
    let providers = provider_registry_file(&["alpaca"]);
    let now = now_fixture();
    let provider = Arc::new(FakeUniverseProvider::new("alpaca"));
    // Deliberately seed only 5 bars for the historical bootstrap -- enough
    // for the old fixed-5-bar evaluator, not enough for swing_momentum's
    // real 20-bar requirement.
    provider.seed_historical(
        "ZZAUTOFRSWING",
        five_bars("ZZAUTOFRSWING", now.timestamp() - 600),
    );
    let injected: Arc<dyn mqk_md::MarketDataProvider> = provider.clone();
    let calendar = trading_day_calendar();

    let result = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        now,
        false,
    )
    .await;

    let status = &result.report.requirements[0];
    assert_eq!(
        status.freshness_state, "insufficient_history",
        "5 bars must not satisfy a real 20-bar strategy requirement: {:?}",
        status
    );
    assert_eq!(result.report.overall_state, "blocked");

    // Now seed the real 20-bar requirement, starting from a clean slate, and
    // confirm the requirement becomes genuinely ready.
    sqlx::query("delete from md_bars where symbol = $1")
        .bind("ZZAUTOFRSWING")
        .execute(&pool)
        .await
        .expect("clean seeded rows between sub-cases");
    provider.seed_historical(
        "ZZAUTOFRSWING",
        n_bars("ZZAUTOFRSWING", now.timestamp() - 600, 20),
    );

    let result2 = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        now,
        false,
    )
    .await;
    // MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01: cycle 2 deletes and
    // re-bootstraps within this same call, so -- like cycle 1 in the AAPL/5m
    // positive proof -- its own report reflects `md_bars.ingested_at` as the
    // real database server clock stamped it *during this same call*, before
    // `run_cycle_with_deterministic_ingest_stamp`'s re-stamp can apply. Only
    // assert the structural fact that doesn't depend on it; cycle 3 below,
    // reading the now-re-stamped rows fresh, is where "genuinely ready" is
    // proven.
    assert_eq!(result2.report.requirements.len(), 1);

    let result3 = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        now,
        false,
    )
    .await;
    let status3 = &result3.report.requirements[0];
    assert_eq!(status3.freshness_state, "ok", "{:?}", status3);
    assert_eq!(result3.report.overall_state, "ready");
    assert_eq!(
        provider.historical_calls(),
        2,
        "cycle 3 must not trigger a third bootstrap call -- 20 bars already sufficient"
    );

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    std::env::remove_var("MQK_STRATEGY_IDS");
}

/// §5/§27: before the first current-session bar becomes expected, a
/// previous trading session's already-durable final bars must satisfy the
/// requirement -- and must never trigger a provider call, regardless of
/// their wall-clock age.
#[tokio::test]
async fn preopen_previous_session_tail_satisfies_expectation_with_zero_provider_calls() {
    let Some(pool) = maybe_db("preopen_previous_session_tail").await else {
        return;
    };
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZAUTOFRPREOPEN");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper"); // required = 5
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");

    let instruments = instrument_registry_file(&[("ZZAUTOFRPREOPEN", "alpaca", "5m")]);
    let providers = provider_registry_file(&["alpaca"]);

    // Previous trading session (2026-08-11, a Tuesday) closes 20:00:00 UTC;
    // its final 5 session-anchored grid slots (starts) are 19:35..19:55.
    // Durably present already -- simulates yesterday's close having been
    // captured normally, before this morning's preopen evaluation.
    let prev_session_close_ts = Utc
        .with_ymd_and_hms(2026, 8, 11, 20, 0, 0)
        .unwrap()
        .timestamp();
    let last_slot_ts = prev_session_close_ts - 300;
    let ingest_id = uuid::Uuid::new_v4();
    let wrote_after = Utc::now();
    mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata(
        &pool,
        mqk_db::md::IngestProviderBarsArgs {
            source: "alpaca".to_string(),
            timeframe: "5m".to_string(),
            ingest_id,
            bars: n_bars("ZZAUTOFRPREOPEN", last_slot_ts, 5)
                .into_iter()
                .map(|b| mqk_db::md::ProviderBar {
                    symbol: b.symbol,
                    timeframe: b.timeframe,
                    end_ts: b.end_ts,
                    open: b.open,
                    high: b.high,
                    low: b.low,
                    close: b.close,
                    volume: b.volume,
                    is_complete: b.is_complete,
                })
                .collect(),
        },
        mqk_db::md::MdBarProviderMetadata {
            provider_id: "alpaca".to_string(),
            provider_source: Some("alpaca".to_string()),
            provider_symbol: Some("ZZAUTOFRPREOPEN".to_string()),
            ingest_mode: Some("test_fixture_previous_session_tail".to_string()),
            provider_bar_id: None,
            provider_updated_at_utc: None,
        },
    )
    .await
    .expect("seed previous-session tail");

    // A provider client that would still register a call if one were ever
    // dispatched -- the assertions below require historical_calls() == 0
    // and latest_calls() == 0.
    let provider = Arc::new(FakeUniverseProvider::new("alpaca"));
    let injected: Arc<dyn mqk_md::MarketDataProvider> = provider.clone();
    let calendar = trading_day_calendar();

    // 2026-08-12 10:00:00 UTC -- well before today's 13:30 UTC session open.
    let preopen_now = Utc.with_ymd_and_hms(2026, 8, 12, 10, 0, 0).unwrap();
    // MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01: this test seeds the
    // previous-session tail directly (bypassing `run_required_universe_cycle`
    // above, so `run_cycle_with_deterministic_ingest_stamp` never runs for
    // it) -- re-stamp its `ingested_at` to stay consistent with this test's
    // own fixed `preopen_now` evaluation instant, same reasoning as that
    // wrapper.
    stamp_recent_ingested_at_for_test(Some(&pool), preopen_now, wrote_after).await;

    let result = run_required_universe_cycle(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected),
        &calendar,
        preopen_now,
        false,
    )
    .await;

    let status = &result.report.requirements[0];
    assert_eq!(
        status.freshness_state, "ok",
        "previous-session tail must satisfy preopen expectation: {:?}",
        status
    );
    assert_eq!(result.report.overall_state, "ready");
    assert_eq!(
        provider.historical_calls(),
        0,
        "no historical bootstrap call"
    );
    assert_eq!(provider.latest_calls(), 0, "no latest-bar poll call");
    assert_eq!(result.provider_api_calls_made, 0);

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    std::env::remove_var("MQK_STRATEGY_IDS");
}

/// §16/§20: an instrument-registry load failure and a provider-registry
/// load failure must surface distinguishable reason codes, never both
/// flattened into `provider_registry_invalid`.
#[tokio::test]
async fn instrument_vs_provider_registry_error_truth_is_distinguished() {
    let Some(pool) = maybe_db("instrument_vs_provider_registry_error_truth").await else {
        return;
    };
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZAUTOFRREGERR");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");

    let providers = provider_registry_file(&["alpaca"]);
    let now = now_fixture();
    let calendar = trading_day_calendar();

    // Case 1: the instrument registry path does not exist -- must surface
    // `instrument_registry_invalid`, never `provider_registry_invalid`.
    let missing_instruments_path = "Z:\\this\\path\\does\\not\\exist\\instruments.json";
    let result_instr = run_required_universe_cycle(
        Some(&pool),
        missing_instruments_path,
        providers.path().to_str().unwrap(),
        None,
        &calendar,
        now,
        false,
    )
    .await;
    assert_eq!(
        result_instr.report.requirements[0].freshness_state, "instrument_registry_invalid",
        "{:?}",
        result_instr.report
    );

    // Case 2: the instrument registry is valid but the provider registry
    // path does not exist -- must surface `provider_registry_invalid`.
    let instruments = instrument_registry_file(&[("ZZAUTOFRREGERR", "alpaca", "5m")]);
    let missing_providers_path = "Z:\\this\\path\\does\\not\\exist\\providers.json";
    let result_prov = run_required_universe_cycle(
        Some(&pool),
        instruments.path().to_str().unwrap(),
        missing_providers_path,
        None,
        &calendar,
        now,
        false,
    )
    .await;
    assert_eq!(
        result_prov.report.requirements[0].freshness_state, "provider_registry_invalid",
        "{:?}",
        result_prov.report
    );

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
}

/// §18/§19: `provider_api_calls_made_this_cycle` must equal the number of
/// actual provider method invocations -- never under-report a call that
/// happened but returned empty/no data, and never count a capability
/// refusal (no call dispatched) as one.
#[tokio::test]
async fn provider_api_call_counter_matches_actual_invocations() {
    let Some(pool) = maybe_db("provider_api_call_counter_matches_actual_invocations").await else {
        return;
    };
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
    let now = now_fixture();
    let calendar = trading_day_calendar();
    let providers = provider_registry_file(&["alpaca"]);

    // Case A: provider called, returns an empty historical result -> one
    // real call, `CalledNoData`, counted.
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZAUTOFRCALLA");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
    let instruments_a = instrument_registry_file(&[("ZZAUTOFRCALLA", "alpaca", "5m")]);
    let provider_a = Arc::new(FakeUniverseProvider::new("alpaca"));
    // Deliberately no seed_historical() call -> fetch_historical_bars
    // returns Ok(vec![]) (empty), a genuine call with no usable data.
    let injected_a: Arc<dyn mqk_md::MarketDataProvider> = provider_a.clone();
    let result_a = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments_a.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected_a),
        &calendar,
        now,
        false,
    )
    .await;
    assert_eq!(
        provider_a.historical_calls(),
        1,
        "the call must be dispatched"
    );
    assert_eq!(
        result_a.provider_api_calls_made, 1,
        "an empty-but-genuine call must still be counted: {:?}",
        result_a.report
    );
    assert_ne!(result_a.report.requirements[0].freshness_state, "ok");

    // Case B: provider called, the historical fetch errors -> one real
    // call, `CalledError`, counted.
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZAUTOFRCALLB");
    let instruments_b = instrument_registry_file(&[("ZZAUTOFRCALLB", "alpaca", "5m")]);
    let provider_b = Arc::new(FakeUniverseProvider::new("alpaca"));
    provider_b.fail("ZZAUTOFRCALLB");
    let injected_b: Arc<dyn mqk_md::MarketDataProvider> = provider_b.clone();
    let result_b = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments_b.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected_b),
        &calendar,
        now,
        false,
    )
    .await;
    assert_eq!(
        provider_b.historical_calls(),
        1,
        "the call must be dispatched"
    );
    assert_eq!(
        result_b.provider_api_calls_made, 1,
        "a failed-but-genuine call must still be counted: {:?}",
        result_b.report
    );
    assert_ne!(result_b.report.requirements[0].freshness_state, "ok");

    // Case C: successful call, real data ingested -> one real call, counted.
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZAUTOFRCALLC");
    let instruments_c = instrument_registry_file(&[("ZZAUTOFRCALLC", "alpaca", "5m")]);
    let provider_c = Arc::new(FakeUniverseProvider::new("alpaca"));
    provider_c.seed_historical(
        "ZZAUTOFRCALLC",
        five_bars("ZZAUTOFRCALLC", now.timestamp() - 600),
    );
    let injected_c: Arc<dyn mqk_md::MarketDataProvider> = provider_c.clone();
    let result_c = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments_c.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected_c),
        &calendar,
        now,
        false,
    )
    .await;
    assert_eq!(provider_c.historical_calls(), 1);
    assert_eq!(result_c.provider_api_calls_made, 1);

    // MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01: `result_c`'s own
    // report reflects `md_bars.ingested_at` as the real database server
    // clock stamped it *during this same call* -- see the identical note on
    // the AAPL/5m positive proof above. Re-run (no new provider call
    // expected) to prove the ingested data is genuinely "ok" once read
    // fresh after the re-stamp -- the counter assertions above already
    // proved the call-counting behavior this test exists for.
    let result_c2 = run_cycle_with_deterministic_ingest_stamp(
        Some(&pool),
        instruments_c.path().to_str().unwrap(),
        providers.path().to_str().unwrap(),
        Some(&injected_c),
        &calendar,
        now,
        false,
    )
    .await;
    assert_eq!(
        provider_c.historical_calls(),
        1,
        "no repeated bootstrap once already ready"
    );
    assert_eq!(result_c2.provider_api_calls_made, 0);
    assert_eq!(result_c2.report.requirements[0].freshness_state, "ok");

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    std::env::remove_var("MQK_STRATEGY_IDS");
}

/// §10/§33: two concurrent scheduler-start callers against the same
/// `AppState`, with a genuine pollable requirement configured (so the
/// scheduler has real ongoing work and stays `running` across the whole
/// call, not just for a sub-microsecond window), must admit exactly one
/// starter -- the other refused with zero state mutation -- and the
/// immediate controller cycle must run exactly once, never once per
/// starter.
// Deliberately does not take `precedence_env_lock()`: that std::sync::Mutex
// would be held across this test's `.await` points (clippy::await_holding_lock)
// -- matches the existing convention every other `#[tokio::test]` in this
// file already follows (only the three sync `#[test]` precedence proofs
// below use that lock, to serialize against each other).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_scheduler_start_admits_exactly_one_starter() {
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZAUTOFRCONC");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");

    let instruments = instrument_registry_file(&[("ZZAUTOFRCONC", "alpaca", "5m")]);
    let providers = provider_registry_file(&["alpaca"]);

    let mut st = mqk_daemon::state::AppState::new();
    st.instrument_registry_path = instruments.path().to_str().unwrap().to_string();
    st.provider_registry_path = providers.path().to_str().unwrap().to_string();
    // Deterministic: the readiness/refresh path is irrelevant to this proof
    // (it only needs `plan.groups` non-empty so `next_poll_utc` stays
    // `Some`, keeping the scheduler `running` for the duration of this
    // test) -- force the no-DB path so no real Postgres state leaks in.
    st.db = None;
    let st = std::sync::Arc::new(st);
    // A real trading-day instant (Wednesday, mid-August, no US market
    // holiday) so the active (env-selected, real NYSE-weekday) calendar
    // provider `run_and_record_cycle` uses internally reports a trading day
    // and the plan resolves at least one provider/timeframe group.
    let now = now_fixture();

    let st1 = st.clone();
    let st2 = st.clone();
    let (r1, r2) = tokio::join!(
        mqk_daemon::state::required_market_data_autofresh::start_required_universe_scheduler(
            &st1, true, now,
        ),
        mqk_daemon::state::required_market_data_autofresh::start_required_universe_scheduler(
            &st2, true, now,
        ),
    );

    let outcomes = [r1.is_ok(), r2.is_ok()];
    let accepted = outcomes.iter().filter(|ok| **ok).count();
    let refused = outcomes.iter().filter(|ok| !**ok).count();

    {
        let scheduler = st.required_universe_scheduler.lock().await;
        let cycle_count = scheduler.cycle_count;
        let groups_present = scheduler
            .last_report
            .as_ref()
            .map(|r| !r.groups.is_empty())
            .unwrap_or(false);
        drop(scheduler);
        mqk_daemon::state::required_market_data_autofresh::stop_required_universe_scheduler(
            &st, now,
        )
        .await;

        assert!(
            groups_present,
            "test setup must produce at least one pollable group so the scheduler has real \
             ongoing work"
        );
        assert_eq!(
            accepted, 1,
            "exactly one concurrent starter must be accepted: {r1:?} {r2:?}"
        );
        assert_eq!(refused, 1, "exactly one concurrent starter must be refused");
        assert_eq!(
            cycle_count, 1,
            "the immediate controller cycle must run exactly once, not once per starter"
        );
    }

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
}

/// §11/§32: when the immediate cycle finds nothing left to poll (no
/// required symbols configured), the scheduler must settle truthfully as
/// stopped -- never `running=true` with no task and no future cycle.
#[tokio::test]
async fn no_work_scheduler_settles_stopped_after_start() {
    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");

    let st = std::sync::Arc::new(mqk_daemon::state::AppState::new());
    let now = now_fixture();

    let report =
        mqk_daemon::state::required_market_data_autofresh::start_required_universe_scheduler(
            &st, true, now,
        )
        .await
        .expect("start must succeed");
    assert_eq!(report.overall_state, "not_applicable");

    let scheduler = st.required_universe_scheduler.lock().await;
    assert!(
        !scheduler.running,
        "a no-work immediate cycle must settle the scheduler as stopped"
    );
    assert!(scheduler.next_cycle_utc.is_none());
    assert!(scheduler.task.is_none());
    assert_eq!(scheduler.cycle_count, 1);
}

/// MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-02 §15: the real
/// stop/restart ABA ownership race.
///
/// Sequence (mirrors the repair spec exactly):
///   1. Start A claims generation A.
///   2. A's immediate provider cycle blocks on a test-controlled barrier
///      (proven via [`BarrierDelayedProvider`] -- no timing-based sleep).
///   3. Stop A.
///   4. Start B claims generation B.
///   5. B establishes current ownership (its own provider call passes
///      straight through the barrier, since only the first call blocks).
///   6. A's old delayed provider call is released.
///   7. A's call finishes and A's `start_required_universe_scheduler`
///      resumes to completion.
///
/// Proves: generation B remains current; A cannot overwrite B's
/// `last_report`/ownership; A cannot install a second task; A cannot
/// increment B's generation-owned `cycle_count`; exactly one current
/// background scheduler authority remains (B's); the old A loop/cycle is
/// superseded with no panic/deadlock. A sequential start test alone (see
/// `concurrent_scheduler_start_admits_exactly_one_starter`, retained above)
/// is not enough to prove this -- that test never has two starts whose
/// *provider calls* genuinely overlap in time.
#[tokio::test]
async fn stop_start_generation_race_old_cycle_cannot_overwrite_new_owner() {
    let Some(pool) = maybe_db("stop_start_generation_race").await else {
        return;
    };
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZAUTOFRRACE");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");

    let instruments = instrument_registry_file(&[("ZZAUTOFRRACE", "alpaca", "5m")]);
    let providers = provider_registry_file(&["alpaca"]);

    // Deliberately real `Utc::now()`, NOT the shared fixed `now_fixture()`:
    // unlike every other test in this file, this one drives the real
    // `start_required_universe_scheduler`/background-loop machinery
    // end-to-end, and that loop always schedules its next real wake-up
    // relative to actual wall-clock time (`required_universe_scheduler_loop`
    // computes `wait_secs` against `Utc::now()`, independent of whatever
    // `now_utc` the immediate cycle was called with). A stale fixed
    // timestamp whose own 5-minute poll grid has already fallen behind real
    // wall-clock would make the background loop fire an extra, genuine
    // real-time cycle before this test's own assertions/cleanup run --
    // flaky, and nothing to do with the generation race this test proves.
    let now_a = Utc::now();
    let call_started = Arc::new(tokio::sync::Notify::new());
    let release_gate = Arc::new(tokio::sync::Notify::new());
    let provider = Arc::new(BarrierDelayedProvider::new(
        "alpaca",
        call_started.clone(),
        release_gate.clone(),
    ));
    provider.inner.seed_historical(
        "ZZAUTOFRRACE",
        five_bars("ZZAUTOFRRACE", now_a.timestamp() - 600),
    );
    let injected: Arc<dyn mqk_md::MarketDataProvider> = provider.clone();

    let mut st = mqk_daemon::state::AppState::new();
    st.instrument_registry_path = instruments.path().to_str().unwrap().to_string();
    st.provider_registry_path = providers.path().to_str().unwrap().to_string();
    st.db = Some(pool);
    st.set_latest_bar_provider_client_for_test(injected);
    let st = Arc::new(st);

    // Step 1/2: Start A claims generation A; its immediate cycle's
    // historical-bootstrap provider call parks on `release_gate`. Run on a
    // spawned task since `start_required_universe_scheduler` will not
    // return until that call is released.
    let st_a = st.clone();
    let handle_a =
        tokio::spawn(async move { start_required_universe_scheduler(&st_a, false, now_a).await });

    tokio::time::timeout(std::time::Duration::from_secs(10), call_started.notified())
        .await
        .expect("A's provider call must start within 10s");

    let gen_a = {
        let scheduler = st.required_universe_scheduler.lock().await;
        assert!(scheduler.running, "A must be recorded as running");
        scheduler.generation
    };
    assert_ne!(
        gen_a, 0,
        "a successful start must claim a nonzero generation"
    );

    // Step 3: Stop A while its immediate cycle is still parked mid-call.
    let stopped = stop_required_universe_scheduler(&st, now_a).await;
    assert!(stopped, "stop must observe A as running and stop it");

    // Step 4/5: Start B. Its own historical-bootstrap call is the *second*
    // call to the shared provider, so it passes straight through the
    // barrier (does not block) and B establishes ownership synchronously.
    // A fresh real `Utc::now()` again -- same reasoning as `now_a` above.
    let now_b = Utc::now();
    let report_b = start_required_universe_scheduler(&st, false, now_b)
        .await
        .expect("start B must succeed once A has stopped");

    let gen_b = {
        let scheduler = st.required_universe_scheduler.lock().await;
        scheduler.generation
    };
    assert_ne!(
        gen_b, gen_a,
        "B must claim a strictly different generation than A"
    );

    // Step 6: release A's delayed provider call. A's DB write (if any) may
    // now genuinely happen -- it targets the same idempotent upsert path B
    // already used, so it cannot corrupt B's already-ingested data -- but
    // its RESULT must never reach the shared scheduler state.
    release_gate.notify_one();

    // Step 7: let A's `start_required_universe_scheduler` call run to
    // completion.
    let result_a = tokio::time::timeout(std::time::Duration::from_secs(10), handle_a)
        .await
        .expect("A's start call must complete within 10s (no deadlock)")
        .expect("A's task must not panic");
    assert!(
        result_a.is_ok(),
        "A's own start call still resolves Ok even though it was superseded: {result_a:?}"
    );

    // ---- Proofs ----
    let scheduler = st.required_universe_scheduler.lock().await;

    assert_eq!(
        scheduler.generation, gen_b,
        "generation B must remain current after A's late cycle completes"
    );
    assert_eq!(
        scheduler.cycle_count, 1,
        "A's superseded cycle must not increment B's generation-owned cycle_count"
    );
    let recorded_last_poll = scheduler
        .last_report
        .as_ref()
        .expect("a report must be recorded")
        .last_poll_utc
        .clone();
    assert_eq!(
        recorded_last_poll,
        Some(now_b.to_rfc3339()),
        "A's late result must not overwrite B's last_report (expected B's now_utc={}, got {:?})",
        now_b.to_rfc3339(),
        recorded_last_poll,
    );
    assert!(
        scheduler.running,
        "B's scheduler must still be running -- A's superseded stop-settle path must not touch it"
    );
    assert!(
        scheduler.task.is_some(),
        "exactly one current background scheduler authority (B's) must be installed"
    );
    assert!(
        scheduler.stop_tx.is_some(),
        "B's stop channel must be installed; A must never have replaced/cleared it"
    );
    assert_eq!(
        report_b.market_date,
        scheduler.last_report.as_ref().unwrap().market_date,
        "B's own returned report and the recorded scheduler report must agree"
    );
    drop(scheduler);

    // Cleanup: stop B's background task so it does not leak into later
    // tests, then confirm the shared BarrierDelayedProvider was called
    // exactly twice (A's original call plus B's) -- proving A's call really
    // did happen (its DB side effect cannot be undone) while still failing
    // to regain authority.
    stop_required_universe_scheduler(&st, now_b).await;
    assert_eq!(
        provider.inner.historical_calls(),
        2,
        "both A's and B's historical-bootstrap provider calls must have actually been dispatched"
    );

    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    std::env::remove_var("MQK_STRATEGY_IDS");
}

/// Serializes every test in this file that mutates the process-global
/// `MQK_PAPER_WATCHLIST_PATH` / `MQK_STRATEGY_SYMBOL` / `MQK_STRATEGY_MD_
/// TIMEFRAME` env vars, so the three precedence tests below (which run
/// under `cargo test`'s default parallelism, unlike the `#[tokio::test]`
/// tests above) never race each other's env state.
fn precedence_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// §45 precedence proof: watchlist-v2 present + legacy env var also set ->
/// watchlist wins, legacy decoy absent from the resolved plan. Exercises the
/// exact public, env-driven resolver (`required_symbols_with_source_from_env`)
/// both the ingest-plan route and the autofresh controller call.
#[test]
fn precedence_watchlist_wins_over_legacy_when_both_configured() {
    use mqk_daemon::market_data_freshness::{
        required_symbols_with_source_from_env, SYMBOL_SOURCE_WATCHLIST_V2,
    };

    let _guard = precedence_env_lock().lock().unwrap();
    let watchlist_file = watchlist_v2_file(&["ZZWATCH1", "ZZWATCH2"]);
    std::env::set_var("MQK_PAPER_WATCHLIST_PATH", watchlist_file.path());
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZLEGACYDECOY");

    let resolution = required_symbols_with_source_from_env();

    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    std::env::remove_var("MQK_STRATEGY_SYMBOL");

    assert_eq!(resolution.source, SYMBOL_SOURCE_WATCHLIST_V2);
    let symbols: Vec<&str> = resolution
        .required
        .iter()
        .map(|r| r.symbol.as_str())
        .collect();
    assert_eq!(symbols, vec!["ZZWATCH1", "ZZWATCH2"]);
    assert!(
        !symbols.contains(&"ZZLEGACYDECOY"),
        "legacy env symbol must be absent once watchlist-v2 is the active source"
    );
}

/// §45 precedence proof: no watchlist configured -> legacy single symbol is
/// used.
#[test]
fn precedence_legacy_symbol_used_when_no_watchlist_configured() {
    use mqk_daemon::market_data_freshness::{
        required_symbols_with_source_from_env, SYMBOL_SOURCE_ENV_STRATEGY_SYMBOL,
    };

    let _guard = precedence_env_lock().lock().unwrap();
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    std::env::set_var("MQK_STRATEGY_SYMBOL", "ZZLEGACYONLY");

    let resolution = required_symbols_with_source_from_env();

    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    std::env::remove_var("MQK_STRATEGY_SYMBOL");

    assert_eq!(resolution.source, SYMBOL_SOURCE_ENV_STRATEGY_SYMBOL);
    let symbols: Vec<&str> = resolution
        .required
        .iter()
        .map(|r| r.symbol.as_str())
        .collect();
    assert_eq!(symbols, vec!["ZZLEGACYONLY"]);
}

/// §45 precedence proof: nothing configured -> does NOT expand to the full
/// instrument registry (empty required set).
#[test]
fn precedence_nothing_configured_never_expands_to_full_registry() {
    use mqk_daemon::market_data_freshness::{
        required_symbols_with_source_from_env, SYMBOL_SOURCE_NONE,
    };

    let _guard = precedence_env_lock().lock().unwrap();
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
    std::env::remove_var("MQK_STRATEGY_SYMBOL");
    std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");

    let resolution = required_symbols_with_source_from_env();

    std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");

    assert_eq!(resolution.source, SYMBOL_SOURCE_NONE);
    assert!(resolution.required.is_empty());
}
