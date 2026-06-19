# DATA-INGESTION-COVERAGE-AUDIT-01 — AUDIT_COMPLETE

Audit-only patch. No production code changed. Branch `main`, HEAD at audit time `66f165d` ("gui: complete backtest run experience").

## 1. Current data-ingestion architecture

Market data flows through four independent subsystems that share one canonical store (`md_bars`) but are otherwise loosely coupled:

1. **Historical/backfill ingest** — daemon-managed jobs (`POST /api/v1/ingest/jobs`) that pull a date range of bars from a registered provider for the full instrument registry, or ingest a CSV file. Implemented in [ingest.rs](core-rs/crates/mqk-daemon/src/routes/ingest.rs).
2. **Latest-bar (intraday top-off) polling** — two parallel mechanisms that do the same kind of work but do not know about each other:
   - An in-process, GUI-triggered scheduler (`market_data_feed_scheduler_*` routes) that polls one provider's `fetch_latest_closed_bar` on a cadence.
   - A standalone PowerShell script (`Refresh-IntradayMarketData.ps1`) that writes JSON evidence files consumed by a separate read-only daemon route.
3. **Premarket backfill automation** — `Prep-PremarketMarketData.ps1`, optionally registered as a Windows Scheduled Task via `Register-PremarketDataRefreshTask.ps1`, runs **outside** the daemon process entirely.
4. **Consumption** — backtest (CLI `bkt run-db`), live/paper/dry-run strategy ticks, the freshness/readiness gate, and the GUI coverage panel all read `md_bars` directly, with one notable exception (see §7).

All four subsystems write through the same canonical upsert path in [mqk-db/src/md.rs](core-rs/crates/mqk-db/src/md.rs), so there is one writer contract even though there are several callers.

## 2. DB/bar storage truth

- **Table**: `md_bars` (`core-rs/crates/mqk-db/migrations/0003_backtest_schema.sql:6`), primary key `(symbol, timeframe, end_ts)`.
- **Price columns**: `open_micros`/`high_micros`/`low_micros`/`close_micros` (`bigint`, deterministic integer micros — no floats stored).
- **Freshness column**: `ingested_at timestamptz not null default now()` — DB-time, explicitly whitelisted as a "bookkeeping baseline" exempt from the no-`DEFAULT now()` rule (`core-rs/crates/mqk-db/migrations/0012_drop_reconcile_checkpoint_default.sql:16`). This is wall-clock-of-write, not market time; `end_ts` remains the deterministic market-time column.
- **Provider/source columns** (migration `0042_md_bars_provider_metadata.sql`): `provider_id` (`not null default 'unknown'`), `provider_source`, `provider_symbol`, `ingest_mode`, `provider_bar_id`, `provider_updated_at_utc`. Backfilled safely on existing rows via the default.
- **Dedup/upsert**: every write path (`ingest_provider_bars_to_md_bars_inner`, `ingest_csv_to_md_bars`, `ingest_md_bars_provider`) is an `INSERT ... ON CONFLICT (symbol, timeframe, end_ts) DO UPDATE`. Re-running the same ingest is idempotent (Q15: yes).
- **Quality reports**: `md_quality_reports` (ingest_id PK, `stats_json`) stores one report per `ingest_id`, written by every batch ingest path.
- **Ingest job history**: `sys_ingest_jobs` (migration `0041_ingest_job_history.sql`), written directly from `mqk-daemon/src/ingest_jobs.rs` (not via `mqk-db`'s module system — a minor architectural inconsistency, not a bug). This is what makes ingest **job** history durable across daemon restart (Q9, partial — see §5).

## 3. Provider registry and swappability

- **Registry file**: `config/providers/providers.json` — 6 entries: `twelvedata` (enabled), `alpaca` (enabled), `alphavantage`/`polygon`/`yfinance`/`coinlore` (all `enabled: false`, `verification_status: "requires_external_verification"`, candidates only).
- **Contract**: [`MarketDataProvider`](core-rs/crates/mqk-md/src/provider.rs:346) trait (`provider_id`, `capabilities`, `health`, `rate_limits`, `fetch_historical_bars`, `fetch_latest_closed_bar`) is the capability-aware seam new code should target. An older, simpler `Provider` trait and a `HistoricalProvider` trait also exist; `HistoricalProviderMarketDataAdapter<P>` bridges `HistoricalProvider` impls into the new contract.
- **Factory**: `build_market_data_provider_from_config` (`core-rs/crates/mqk-md/src/provider_registry.rs:226`) matches on `provider_id` with hardcoded arms for `"fake"`, `"twelvedata"`, `"alpaca"` only. Any other registry entry (including the four disabled candidates) returns `ProviderFactoryError::UnsupportedProvider` even if `enabled` were flipped to `true` — the registry *declares* candidates, but each one still needs a concrete adapter + factory arm before it's usable. This is the expected/correct shape for an adapter pattern, not a defect — but it means "provider swappable" today means "swappable among providers that already have Rust adapters," not "swappable via config alone."
- **Capability gating is real, not cosmetic**: `capabilities_from_provider_config` only sets `latest_closed_bar: true` for `provider_id == "alpaca"`; TwelveData is historical-only. The `poll-once` and scheduler routes both check `capabilities.latest_closed_bar` before calling and refuse with a typed error otherwise (verified in [ingest.rs:643-660](core-rs/crates/mqk-daemon/src/routes/ingest.rs)).
- **Credential handling**: `credential_env_vars` in the registry names env vars only; values are never embedded in the registry or logged (verified: `ProviderFactoryError::MissingCredential` test asserts the secret value never appears in the error string).

## 4. Historical/backfill ingest path

Entry point: `POST /api/v1/ingest/jobs` with `mode="sync_provider"` ([ingest.rs:1644](core-rs/crates/mqk-daemon/src/routes/ingest.rs) `handle_provider_sync_job`).

- Validates: provider registered + enabled + supports the requested `asset_class`/`timeframe`/`historical_bars` capability, before any provider call.
- Symbol source is **registry-only** (`symbols_source` must equal `"registry"` — list-based submission is rejected); resolves from `config/instruments/equities.json` via `mqk_md::instrument_registry::enabled_equities`.
- `dry_run=true` (default): resolves symbol count/first/last from the registry file, zero network calls, zero writes — proven by `IngestJobStatus::DryRunCompleted`.
- `dry_run=false` requires explicit `allow_provider_api_calls=true` or the job is `Refused` before a job record with provider attempt is even created.
- Real sync (`run_real_provider_sync`, [ingest.rs:2289](core-rs/crates/mqk-daemon/src/routes/ingest.rs)) loops per symbol, checks `api_credits_per_day`/`api_credits_per_minute` guardrails **before** each call, continues the batch on a single symbol's failure (per-symbol error tracking, `Partial` status if some succeed), and writes through `mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata` with `ingest_mode="historical_backfill"`.
- `ingest_id` is deterministic (`Uuid::new_v5` keyed on source+timeframe+date range) — re-running the same backfill is idempotent and traceable.
- CSV path (`handle_csv_job`) is the same shape minus provider/registry steps; writes through `mqk_db::md::ingest_csv_to_md_bars` and a `data_quality.json` artifact under `exports/md_ingest/<ingest_id>/`.
- **Cancel** (`POST /api/v1/ingest/jobs/:job_id/cancel`) checks cancellation between every symbol and at job start; persists `Cancelled` status to DB when configured.
- Provider-level retry/backoff exists for **TwelveData only**: bounded retry (4 retries, 65s sleep) on HTTP 429 and body-level `code=429`, with a distinct non-fatal "no data available" (pre-inception date) classification ([mqk-md/src/lib.rs:431-604](core-rs/crates/mqk-md/src/lib.rs)). **`AlpacaHistoricalProvider::fetch_bars` has no retry/backoff at all** — any non-2xx response (including a transient 429) is immediately fatal for that call ([alpaca_provider.rs:144-152](core-rs/crates/mqk-md/src/alpaca_provider.rs)).

## 5. Latest-bar scheduler path

Two independent mechanisms exist; neither is calendar-aware (§9).

**(a) In-process daemon scheduler** (`market_data_feed_scheduler_start/stop/status`, [ingest.rs:1073-1254](core-rs/crates/mqk-daemon/src/routes/ingest.rs)):
- State lives in `AppState.market_data_feed_scheduler` (a `tokio::Mutex`-guarded struct holding a `JoinHandle` and a `watch::Sender` stop channel) — pure in-memory, one scheduler per provider/timeframe/symbol-set at a time.
- The loop (`market_data_feed_scheduler_loop`) computes the next cadence-aligned poll time (`mqk_md::next_poll_time_ts`), sleeps, then calls the same `market_data_feed_poll_once` handler the manual one-shot route uses (so there is exactly one poll code path, not two).
- **Every status/response struct carries `"limitation": "process_local_only_not_persisted"` explicitly** — this is an honest, already-labeled gap, not a silent one. On daemon restart the scheduler is simply not running; nothing resumes it automatically (Q9: **no** for this mechanism).
- `poll-once` itself is well-guarded: validates `latest_closed_bar` capability and timeframe support before calling, verifies the returned bar actually matches the requested symbol/timeframe/`is_complete`/`end_ts <= expected` before writing (no lookahead, no forming-bar writes), and records per-symbol status (`inserted`/`updated`/`skipped_no_bar`/`skipped_unclosed_or_unexpected_bar`/`provider_error`/`db_error`) rather than an all-or-nothing result.

**(b) Standalone intraday refresher** (`scripts/windows/Refresh-IntradayMarketData.ps1`):
- Runs outside the daemon, writes evidence to `exports/market_data/intraday_refresh_*.json` (schema `intraday-refresh-v1`).
- Consumed read-only by `GET /api/v1/market-data/intraday-refresh/status` ([transport_quality.rs:444](core-rs/crates/mqk-daemon/src/routes/transport_quality.rs)), which flags evidence stale after 24h and never panics on missing/malformed files.
- This path and the in-process scheduler do not share state, coordinate, or know about each other's last-poll time.

**(c) Premarket backfill automation** (separate from both latest-bar mechanisms): `Register-PremarketDataRefreshTask.ps1` registers a genuine **Windows Task Scheduler** task (Mon–Fri at a configurable local time, default `08:30`) whose action is `Prep-PremarketMarketData.ps1` only (test-guarded: the action string is asserted to never reference `Start-PaperTradingSmoke.ps1`, `start-system`, `arm-execution`, or order/broker actions). Documented in `docs/runbooks/operator_workflows.md` §11. **Both the script default and the documented registration example use `-Symbols AAPL` only** — a single hardcoded symbol, not the 80-symbol instrument registry and not whatever the live strategy is actually configured to trade.

## 6. GUI/operator visibility

More mature than file-name pattern-matching alone would suggest — confirmed wired in `IngestScreen.tsx`, not just present as unused API helpers:

- **Ingest jobs**: list/submit/cancel/status, via `GET/POST /api/v1/ingest/jobs*` — full lifecycle visible, `truth_state` honest (`backend_unavailable` vs `not_found` vs `active`).
- **Coverage panel** (`GET /api/v1/market-data/coverage`, DATA-INGEST-GUI-RESULTS-01): per-(symbol,timeframe) `bars`/`min_end_ts`/`max_end_ts`/`latest_ingested_at`, `truth_state` distinguishes `active`/`empty`/`db_unavailable`/`unavailable`.
- **Missing-symbol detection** (client-side, `computeMissingTrackedSymbols` in [api.ts:941](core-rs/mqk-gui/src/features/ingest/api.ts)): diffs `GET /api/v1/ingest/tracked-equities` (the instrument registry) against the coverage rows and returns symbols with **zero** coverage — explicitly returns `null` (not `[]`) when the registry itself is unavailable, so "no gaps" is never confused with "couldn't check."
- **Freshness classification** (client-side, `classifyCoverageFreshness`): same thresholds as the backend gate (4 days for `1D`, 15 minutes for intraday) applied per coverage row.
- **Intraday refresh evidence**: surfaced via the standalone evidence-file route described in §5(b).
- **Gap that this panel does *not* cover**: none of the above detects an *interior* gap in an existing series (e.g., "AAPL has 200 bars but is missing March 10–14"). Gap/missing-weekday detection (`gaps_detected`, `missing_weekdays_est`) exists only inside per-ingest-batch quality reports (`MdQualityGroupStats`, `SymbolCoverageStats`), which are written once per ingest and not aggregated into a queryable "find the holes" view.
- **`MarketDataScreen.tsx`** (a *different* screen from `IngestScreen.tsx`) renders `stale_symbol_count`, `venue_disagreement_count`, `missing_bar_count`, `strategy_blocks` from `GET /api/v1/market-data/quality`. The handler ([transport_quality.rs:307](core-rs/crates/mqk-daemon/src/routes/transport_quality.rs)) **hardcodes all of these to `0`** with an explicit code comment: *"per-symbol quality tracking does not exist in the current implementation. Setting them to 0 is honest: these metrics are not tracked, not 'zero issues confirmed.'"* The route is honest in its own doc comment, but the GUI screen presents these as live stat cards with green/red tone — an operator glancing at this screen without reading the source would reasonably believe "0 stale symbols" is a measured fact rather than an untracked stub.

## 7. Backtest/runtime data consumers

- **Live/paper/dry-run strategy ticks** (single-symbol path, `mqk-daemon/src/state.rs:1963` and `state/loop_runner.rs:916`; multi-symbol path, `state/per_symbol_bar_window.rs:250`) **all** call `mqk_db::fetch_recent_completed_bars_for_strategy(symbol, timeframe, limit)` — the same canonical `md_bars` read used by the freshness gate. There is one bar-window read path shared by both single- and multi-symbol dispatch.
- **`mqk-backtest` (the engine crate)** has no DB dependency at all — it only knows `Vec<BacktestBar>` in memory, loaded via `mqk_backtest::load_csv_file`/`parse_csv_bars` ([loader.rs](core-rs/crates/mqk-backtest/src/loader.rs)).
- **CLI** (`mqk-cli/src/commands/bkt.rs`) has *two* entry points: `run_backtest_csv` (CSV file) and `run_backtest_db` (`bkt.rs:360`), which calls `mqk_db::md::load_md_bars_for_backtest_symbols` directly and converts DB rows to `BacktestBar` in memory before running the identical engine. So a DB-backed backtest path exists and is exercised by tests (`mqk-db/tests/md_load_backtest.rs`).
- **GUI/daemon-orchestrated backtest jobs do not use this DB path.** `BacktestJobRequest` ([types.ts:343](core-rs/mqk-gui/src/features/backtests/types.ts)) and `BacktestJobRecord` ([backtest_jobs.rs:42](core-rs/crates/mqk-daemon/src/backtest_jobs.rs)) both require a `bars_path: string` — a CSV file the operator must already have on disk. There is no daemon route or GUI field that says "load symbol X, timeframe Y, date range Z from `md_bars`." There is also no CLI/daemon "export `md_bars` → CSV" command; the only CSV schema in `mqk-db::md` that round-trips DB-shaped rows (`DbBackupCsvRow`, with `*_micros` columns) implies bars are sometimes hand-exported via raw Postgres `\copy`, not a sanctioned tool.
- **Net effect**: the user's stated goal "backtest, strategy dry-run, paper trading, and GUI should all see the same trusted bar data" is **true for dry-run/paper/live** (all three hit `md_bars` through one function) but **not true for GUI-triggered backtests**, which run on whatever CSV the operator supplies with no guaranteed relationship to the live-ingested `md_bars` data.

## 8. Freshness/staleness/readiness gates

Two independent, non-overlapping staleness mechanisms exist (this is a real architectural seam, not a duplicate):

1. **Start-time gate** — `evaluate_md_freshness_status` ([market_data_freshness.rs:283](core-rs/crates/mqk-daemon/src/market_data_freshness.rs)), wired into `start_execution_runtime` in [lifecycle.rs:601-626](core-rs/crates/mqk-daemon/src/state/lifecycle.rs). Runs **only** when `DeploymentMode::Paper && StrategyMarketDataSource::ExternalSignalIngestion`, and **only** for the single symbol/timeframe pair in `MQK_STRATEGY_SYMBOL`/`MQK_STRATEGY_MD_TIMEFRAME`. States: `not_applicable` (env unset, pass) / `unavailable` (DB unreachable, pass — honest, not a false negative) / `missing` (0 completed bars, **fail-closed**) / `insufficient` (< 5 completed bars, **fail-closed**) / `stale` (latest bar older than the timeframe-aware max age, **fail-closed**) / `ok`. Max age is timeframe-aware: 4 trading days for daily+, 900s (configurable via `MQK_INTRADAY_BAR_MAX_AGE_SECS`) for intraday.
2. **Per-tick, per-symbol classifier** — `classify_bar_staleness` in [per_symbol_bar_window.rs:269](core-rs/crates/mqk-daemon/src/state/per_symbol_bar_window.rs), the multi-symbol-dispatch-time staleness cap referenced in prior session memory as MD-STALENESS-PER-TICK-GATE-01 (cap #9). This runs *during* ticking, per symbol, not at startup.

**The gap between them**: the only **start-time, fail-closed block** is single-symbol. A multi-symbol run can start with zero or stale data for every symbol *except* the one named in `MQK_STRATEGY_SYMBOL`, and the multi-symbol staleness problem is only ever caught per-tick, per-symbol, after the run has already started — there is no "are all configured symbols/timeframes ready" gate evaluated once before market open across the whole multi-symbol watchlist.

Staleness is unambiguously **per-symbol, per-timeframe** in both mechanisms (Q17) — there is no global/aggregate staleness flag anywhere in the runtime gating path. (`market_data_quality`'s GUI-facing `stale_symbol_count` is the one place a global-looking number exists, and per §6 it is a hardcoded `0`, not a real aggregate.)

## 9. Market-calendar/session interaction

A genuinely well-built calendar/session system exists: `MarketCalendarProvider` trait, `NyseWeekdaysProvider` (DST/holiday/early-close aware, delegates to `mqk_integrity::CalendarSpec::NyseWeekdays`), `ExchangeSourcedCalendarProvider`, explicit fail-closed `MarketSessionState::Unknown` ([market_calendar.rs](core-rs/crates/mqk-daemon/src/state/market_calendar.rs)). **This module is referenced from exactly one place in the daemon (`state.rs`) and nowhere in the ingestion code.**

Confirmed by direct search — zero references to `calendar`/`holiday`/`session` in `routes/ingest.rs`. The in-process scheduler loop polls on a pure time cadence regardless of session state. The two PowerShell mechanisms (`Register-PremarketDataRefreshTask.ps1`, `Refresh-IntradayMarketData.ps1`) gate only on Windows Task Scheduler's `-DaysOfWeek Monday..Friday` — they cover weekends but not weekday exchange holidays (Thanksgiving, observed Christmas/July 4th, etc.).

Practical impact is low, not absent: every ingest write path is an idempotent upsert, so a wasted holiday poll cannot corrupt `md_bars` — it just spends an API call/cycle for no new data on ~9–10 days/year. This is real but narrow; classified as `NICE_TO_HAVE` below, not a reliability blocker.

## 10. Known gaps / blockers

| # | Gap | Classification |
|---|---|---|
| G1 | GUI/daemon backtest jobs require a hand-supplied CSV `bars_path`; there is no route or job field to source bars from `md_bars` directly, and no sanctioned `md_bars` → CSV export tool. A CLI-only `bkt run-db` path exists but is not GUI-reachable. | **BLOCKER_FOR_BACKTEST_CONFIDENCE** |
| G2 | The only start-time, fail-closed freshness gate (`evaluate_md_freshness_status`) checks a single `MQK_STRATEGY_SYMBOL`/timeframe pair. A multi-symbol run has no pre-market-open gate proving every configured symbol/timeframe has sufficient fresh bars; staleness for non-primary symbols is only caught per-tick, after start. | **BLOCKER_FOR_MARKET_OPEN_RELIABILITY** |
| G3 | Three independent "symbol list" sources exist with no link between them: the instrument registry (`config/instruments/equities.json`, 80 symbols, all `provider: twelvedata`/`1D`), the premarket refresh script's `-Symbols` default/example (`AAPL` only, documented as such), and whatever the live strategy actually trades (`MQK_STRATEGY_SYMBOL` / multi-symbol config). Nothing warns the operator when these diverge. | **BLOCKER_FOR_MARKET_OPEN_RELIABILITY** |
| G4 | The in-process latest-bar scheduler is explicitly `process_local_only_not_persisted` — a daemon restart silently drops it with no auto-resume. (Honestly labeled, but still a gap.) | **OPERATOR_VISIBILITY_GAP** (mitigated by G2's start gate fail-closing on stale data — but only for the single-symbol path) |
| G5 | `AlpacaHistoricalProvider::fetch_bars` has no retry/backoff on any HTTP error, including transient ones; `TwelveDataHistoricalProvider` has explicit, tested 429 retry/backoff. Inconsistent reliability between the two enabled providers. | **BLOCKER_FOR_MARKET_OPEN_RELIABILITY** (narrow: only matters on Alpaca rate-limit/transient-error days) |
| G6 | No interior gap detection ("missing March 10–14 in an otherwise-populated series") is exposed as a live, queryable view. Gap stats exist only transiently inside per-ingest quality reports. | **OPERATOR_VISIBILITY_GAP** |
| G7 | `GET /api/v1/market-data/quality`'s `stale_symbol_count`/`missing_bar_count`/`venue_disagreement_count`/`strategy_blocks` are hardcoded to `0` (honestly documented in source, but the `MarketDataScreen.tsx` GUI presents them as live measured stat cards). | **OPERATOR_VISIBILITY_GAP** |
| G8 | Neither ingestion mechanism (in-process scheduler or the two PowerShell scripts) consults `MarketCalendarProvider`; holiday weekdays are not skipped (weekends are, via Task Scheduler's day-of-week trigger). | **NICE_TO_HAVE** |
| G9 | Provider registry config (`providers.json`) declares 4 disabled candidate providers (Polygon, Alpha Vantage, yfinance, CoinLore) with no factory implementation; flipping `enabled: true` alone would not make them usable — `build_market_data_provider_from_config`'s match has no arm for them. Expected for an adapter pattern, but worth naming so a future "just enable Polygon" request isn't assumed to be a one-line change. | **BLOCKER_FOR_PROVIDER_SWAPPABILITY** (informational — no action implied unless/until a new provider is actually wanted) |
| G10 | `core-rs/crates/mqk-md/src/ingest_csv.rs` (474 lines) is not declared in `lib.rs` (`pub mod ingest_csv;` is absent) — dead code, not compiled into the crate. | **NICE_TO_HAVE** (cleanup only) |

## 11. Completion roadmap

Reordered from the candidate list using what the audit actually found (not timid — most candidates are confirmed real, not hypothetical):

1. **PREMARKET-DATA-READINESS-GATE-01** — extend `evaluate_md_freshness_status`'s start-time, fail-closed check from the single `MQK_STRATEGY_SYMBOL` to the full configured multi-symbol watchlist (closes G2). This is the highest-leverage change because the gate mechanism, thresholds, and DB query already exist and are proven — this is "loop it over N symbols and aggregate to one allow/block decision," not new infrastructure.
2. **WATCHLIST-INGEST-PLAN-01** — establish one source of truth tying together the instrument registry, the premarket script's symbol list, and the live trading watchlist, so G3's three-way divergence becomes structurally impossible rather than operator-remembered. Natural pairing with #1: the readiness gate in #1 needs *a* symbol list to loop over, and this patch is what defines which one.
3. **BACKTEST-DB-BARS-SOURCE-01** — add a `md_bars`-backed source option to the daemon backtest job route (mirroring the existing CLI `bkt run-db` path) so GUI-triggered backtests can use the same trusted data live/paper trading uses, closing G1.
4. **DATA-COVERAGE-MATRIX-01 (narrow form)** — the coverage matrix itself already exists and is wired (§6); the real remaining work is interior-gap detection (G6) and replacing the hardcoded-zero `market_data_quality` stub with real per-symbol staleness/missing aggregates so `MarketDataScreen.tsx` stops presenting untracked metrics as measured ones (G7). Scope this as "finish the coverage matrix," not "build it from scratch."
5. **INGEST-RETRY-BACKOFF-01** — add retry/backoff to `AlpacaHistoricalProvider::fetch_bars` matching the existing TwelveData pattern (G5).
6. **INGEST-SCHEDULER-PERSISTENCE-01** — persist the in-process latest-bar scheduler's config (provider/symbols/timeframe/running) so a daemon restart can offer to resume it, or at minimum surface "scheduler was running before last restart and has not resumed" (G4). Lower priority than #1 because #1's fail-closed gate already catches the resulting staleness at run-start for the symbols it covers.
7. **INGEST-MARKET-CALENDAR-GUARD-01** — wire `MarketCalendarProvider` into the scheduler loop and/or document the PowerShell scripts' holiday blind spot (G8). Lowest priority — cosmetic/efficiency only, no correctness impact.
8. **PROVIDER-SWAP-CONTRACT-01** — only worth doing when a second real provider is actually being added; the contract (`MarketDataProvider` trait + capability struct) is already in good shape. Don't build ahead of need.

## 12. Recommended next patch

**PREMARKET-DATA-READINESS-GATE-01.**

The audit shows the coverage matrix the original brief worried might not exist (§6) — it does, and it's wired into the GUI. What's actually missing is narrower and higher-leverage: the one gate that fail-closes startup on insufficient data only checks one symbol. Extending it to the full multi-symbol watchlist is a small, mechanical change (loop + aggregate over an existing, tested, pure function) that directly closes the gap between "the operator believes the bot has fresh data before market open" and "the bot has actually verified that for every symbol it will trade, not just one." It is also the one item on this list that is purely additive to an existing fail-closed safety mechanism rather than new ingestion surface area, which fits the patch-discipline bar for a single focused next step.

(WATCHLIST-INGEST-PLAN-01 is a close second and is arguably a prerequisite for #1 to be well-scoped — the gate needs to loop over *a* symbol list, and right now there isn't one canonical list to hand it. Worth deciding together before starting #1's implementation.)

## 13. Tests/checks run

```
cargo check -p mqk-daemon          → Finished, 0 errors (1 pre-existing sqlx-postgres future-incompat warning, unrelated)
cargo check -p mqk-db              → Finished, 0 errors (same pre-existing warning)
cargo check -p mqk-md              → Finished, 0 errors
cargo test  -p mqk-daemon ingest   → exit 0; 0 "FAILED" occurrences across the full run
cargo test  -p mqk-db md           → exit 0; 0 failed
cargo test  -p mqk-md              → 155 passed, 0 failed, 0 ignored; 1 doc-test passed
```

**`cargo test -p mqk-daemon ingest`** (full run, not just `--lib --bins`): the `ingest` substring filter matches inline unit tests across every integration-test binary in the crate, so `cargo` compiles and runs all of them, reporting `0 passed; N filtered out` for binaries with no matching test name. The ingest-specific scenario binaries each ran clean:
- `scenario_ingest_jobs_data_ingest_daemon_01.rs` — 1 passed (`ij12_ingest_job_does_not_touch_execution_snapshot`)
- `scenario_data_freshness_readiness_gate_01.rs`, `scenario_intraday_md_freshness_autonomous_01.rs`, `scenario_intraday_md_refresher_01.rs`, `scenario_intraday_md_refresher_operator_surface_01.rs`, `scenario_md_coverage_data_ingest_gui_results_01.rs` — all `0 passed; 0 failed` with their tests filtered out (no test name contains "ingest" in these files; their own dedicated tests are unaffected and were not separately re-run since the patch scope is the ingestion audit, not a full-suite run)
- Across the entire run: zero `FAILED` occurrences, zero `panicked` occurrences, exit code 0.

**`cargo test -p mqk-db md`**: every DB-backed scenario test (`scenario_md_ingest_provider.rs` ×13, `scenario_md_ingest_csv.rs`, `scenario_md_fetch_returns_ordered_rows.rs`, `scenario_md_sync_provider.rs`) reports `ignored, requires MQK_DATABASE_URL` rather than running or failing — consistent with this codebase's established convention (confirmed in multiple prior-session memory entries) of DB-gated scenario tests skipping gracefully without a live Postgres test instance. No `MQK_DATABASE_URL` was set for this audit, per the patch's "no migrations, minimal footprint" scope; 0 failures.

**`cargo test -p mqk-md`**: this crate's tests are pure/in-memory or mocked-HTTP (`httpmock`) — no DB, no real network. All 155 passed, including the TwelveData rate-limit retry tests and the Alpaca pagination/error tests cited in §4.

GUI checks (`npm test -- --run`, `npm run build`) were not run: this audit made no GUI code changes, and the GUI behavior described in §6/§7 was verified by reading the actual wiring (`IngestScreen.tsx` imports/calls, `BacktestJobRequest`/`BacktestJobRecord` field shapes) rather than by executing the test suite, consistent with the "audit first, minimal validation" scope of this patch.

## 14. Safety confirmation

- No broker submit code touched.
- No Alpaca order submit code touched.
- No live routing touched.
- No order/outbox writes performed.
- No DB migrations added or modified.
- `.env.local` not read, written, or printed.
- No provider/broker network calls made (all findings from static reading + `cargo check`/`cargo test` compilation; no daemon process was started).
- No paper/live orders submitted.
- No short-entry enablement touched.
- No risk gate (B5 or otherwise) touched or weakened.
