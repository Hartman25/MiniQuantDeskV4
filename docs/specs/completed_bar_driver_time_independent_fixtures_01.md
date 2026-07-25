# COMPLETED-BAR-DRIVER-TIME-INDEPENDENT-FIXTURES-01

Patch ID: `COMPLETED-BAR-DRIVER-TIME-INDEPENDENT-FIXTURES-01`
Scope: `core-rs/crates/mqk-daemon/tests/scenario_autonomous_completed_bar_driver_01.rs` only.
No production Rust behavior changed. Test-only patch (B4-0, prerequisite to Bundle 4).

## Problem

The scenario's documented baseline (47 passed / 9 known failures, later observed at
46 passed / 10 failures) drifts further as real calendar time passes, because
`standard_timing()` anchored its entire synthetic trading session to a fixed
calendar date (`2026-07-20T13:30:00Z` exchange open). Every assertion in the
file that expects `AutonomousCompletedBarDriverOutcome::DispatchCompleted`
derives its expected `bar_end_ts` *relative* to `timing.effective_open` /
`timing.exchange_open` (e.g. `timing.effective_open.timestamp() + 300`) — there
are no absolute epoch-second literals anywhere in the file. That means the
fixture's internal consistency was never the problem; the problem is that the
fixed anchor ages against the one clock this scenario cannot inject:
`AppState::dispatch_native_strategy_for_symbol_with_bar` (`state.rs:2726`)
always calls real `Utc::now()` for the per-tick bar-staleness gate
(MD-STALENESS-PER-TICK-GATE-01). Once the fixed anchor falls outside the
applicable staleness cap —
`market_data_freshness::DEFAULT_INTRADAY_BAR_MAX_AGE_SECS` (900s) for
intraday timeframes, `MD_FRESHNESS_STALE_SECS` (4 trading days) for daily —
every test whose flow actually reaches strategy dispatch starts failing,
while tests that never reach dispatch (auth/admission/cadence/etc. blockers)
are unaffected. This exactly matches the observed 46/10 split.

## Root causes fixed

1. **Wall-clock-relative fixture anchor.** `standard_timing()` hardcoded
   `market_date = 2026-07-20`. Fixed by capturing `Utc::now()` at call time
   and deriving every field from it, preserving the exact same relative
   session shape (preopen 30 min before open, 6.5h session, postclose 15 min
   after close) that existed before. `past_timing()` (used only to prove the
   staleness gate still fires against a genuinely, unavoidably old bar) is
   deliberately left untouched — it must remain fixed and historical.

2. **5-minute boundary alignment.** The provider-poll seam
   (`state::market_data_latest_bar::poll_and_ingest_latest_closed_bar`) is
   itself fully time-injected (`now_utc` is caller-supplied — the function
   "never reads the wall clock", per its own doc comment) and derives
   `latest_expected_closed_bar_ts` via `mqk_md::latest_closed_bar_end_ts`,
   which floors `now_utc` to the nearest 5-minute boundary for `"5m"`
   targets. The original fixed anchor (`13:30:00Z`) happened to sit exactly
   on such a boundary; a naive `Utc::now() - 10 minutes` anchor does not
   (sub-minute precision), which floored the provider-poll seam's derived
   expectation *below* the bar timestamps this fixture seeds, producing
   spurious `ProvenanceRejected` ("future-skew") outcomes. Fixed by flooring
   the anchor to the nearest 5-minute boundary before use
   (`div_euclid(300) * 300`).

3. **Pre-existing cross-test DB race (test-hygiene, not a Bundle 3
   regression).** `maybe_db()`'s per-test cleanup issued a wildcard
   `delete ... where adapter_id like 'zzdrv%'` before every single test.
   Every test in this file uses its own distinct `zzdrv-*` adapter_id, so
   this cleanup was never actually needed *between* tests within one run —
   but `cargo test`'s default parallelism runs many `#[tokio::test]`s in
   this file concurrently against the same DB, and one test's wildcard
   sweep could delete a *different*, still-in-flight test's freshly created
   operation row. Worse, the events-table cleanup query
   (`delete ... where operation_id in (select operation_id from
   sys_autonomous_daily_operations where adapter_id like 'zzdrv%')`) could
   never again find and delete events whose parent operations row had
   already been deleted by a concurrent sweep, permanently orphaning them.
   18 such orphaned rows were found and purged from the local port-5434 test
   DB during this investigation (residue of a prior parallel run, not
   present in any committed state). Fixed by running the wildcard sweep
   exactly once per test-binary invocation, guarded by a
   `tokio::sync::OnceCell`, plus an orphan sweep (`operation_id not in
   (select operation_id from sys_autonomous_daily_operations)`) to clear any
   leftovers from a previous crashed run.

## Verification

- `cargo test -p mqk-daemon --test scenario_autonomous_completed_bar_driver_01`
  against `MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test?sslmode=disable`:
  - `--test-threads=1`: 56 passed, 0 failed.
  - default parallelism, three consecutive runs: 56 passed, 0 failed each time.
- No scenario was relaxed, skipped, or marked `#[ignore]`. No `sleep`-based
  timing was introduced. No production (`src/`) file was changed — `git diff
  --stat` shows exactly one file touched, the scenario test itself.
- `cargo clippy -p mqk-daemon --test scenario_autonomous_completed_bar_driver_01 -- -D warnings`: clean.
- `cargo fmt --check` on the touched file: no diff (unrelated pre-existing
  formatting drift in other files is out of scope for this patch).

## Result

All 56 scenarios pass deterministically regardless of which real calendar
day/time the suite is executed on, and regardless of `cargo test`'s thread
count. No previously-passing assertion was weakened; the 10 previously
"known failing" scenarios now pass for the same reason every other scenario
in the file does — they exercise real production dispatch/reconcile logic
against a fixture whose relative timing structure was always correct, now
anchored to a clock that cannot age out from under it.
