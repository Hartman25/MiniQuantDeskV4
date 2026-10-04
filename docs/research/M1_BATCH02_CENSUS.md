# M1 native hypothesis Batch 02 — census (`V4-M1-NATIVE-HYPOTHESIS-BATCH-02-01`)

Baseline: `e4e9dba6a51c7872565bbd929c657058c2097970` (independently accepted, pushed, GitHub CI #622 green 6/6). Predeclaration: `research-py/experiments/m1_native_trend_campaign/PREDECLARED_BATCH_02.json`, committed before any engine, calendar code, trial registration or result. Result: `docs/research/M1_BATCH02_RESULT.md`.

## Calendar census (answers, with proof)

| Question | Answer / proof |
|---|---|
| What timestamp does an Alpaca `1Day` research bar carry? | `end_ts` = 00:00:00 America/New_York of the session's civil date (05:00Z in EST, 04:00Z in EDT). Verified on every row of the fixed dataset: 13400/13400 rows, 0 non-midnight-ET, 0 weekend rows, identical session sets for all five symbols. |
| Does Backtest `BarStub` preserve enough to identify the session? | Yes: `BarStub.end_ts` is the unchanged epoch label; `S(t)` = its ET civil date, required to be exactly 00:00:00 ET and a covered session (`mqk_integrity::sessions::session_of_daily_bar`). |
| Does `StrategyContext` expose a calendar? | No, and none is added: engines resolve `S(t)`/`N(t)` from the latest bar's label plus the shared authority. |
| Reusable by `mqk-strategy` without a cycle? | `mqk-integrity` depends only on `chrono`/`chrono-tz`; `mqk-strategy` now depends on it. No cycle. |
| Is `ExchangeSourcedCalendarProvider` too high? | Yes (daemon). Not used; it is an operator-facing intraday provider. |
| Smallest correct home? | `mqk-integrity::sessions` (new module, daily session dates). The existing `calendar.rs` is an intraday bar-end classifier bounded to 2023–2028 and is untouched. |
| Did the existing table span the campaign? | No: 2023–2028 only. `sessions.rs` carries an explicit 105-date full-closure table for 2016-01-01..2026-12-31 and refuses (typed `OutOfCoverage`) outside it. No weekday-arithmetic fallback exists. |
| Extraordinary closures? | 2018-12-05 and 2025-01-09 (national days of mourning) are in the table. Juneteenth only from 2022; Saturday New Year's Day is not observed on Friday (2021-12-31 is a session). |
| Authority for the table | NYSE published holiday schedule/observation rules (nyse.com/markets/hours-calendars; re-read this session for 2026–2028: confirms 2026-09-07, 2026-11-26, 2026-12-25 and "no Friday observance" for Saturday New Year's Day). Cross-checks only: an independent rule derivation in tests (equal to the table), published NYSE annual session counts (252, 251, 251, 252, 253, 252, 251, 250, 252, 250 for 2016–2025), and the provider's own session dates (exact match for the pre-holdout range 2016-01-04..2026-02-27, committed fixture). The two mourning closures are evidenced by the provider data and the annual counts, not by a re-fetched primary page. |
| Research == Backtest == Paper `S(t)/N(t)`? | One pure function over the same label and the same table. Paper cannot run the engines past 2026-12-31 (fails closed flat) until the calendar is extended under a new content identity. |
| Can missing coverage fall back to weekdays? | No (mutation-proven: coverage check removed → tests fail). |
| Can a strategy infer month-end from the next price row? | No: month-end/ordinals come from the table only; price is irrelevant (property-tested with arbitrary closes). |
| Holdout exposure | Calendar knowledge is not price. The only provider rows inspected for calendar derivation were timestamps; no price, return or event count of any post-2026-02-28 bar was used. The committed provider-date fixture stops at 2026-02-27. |

Calendar identity: `sha256` of the canonical content string = `3249ee517cbc6763b9f73b372d5c6a5f8337d872b91d20a5bca1c27b7a1a76de` (predeclared; Rust and Python agree). It is bound into the H1 and H2 native fingerprints, hence into every H1/H2 trial identity; H3 is calendar-free (test-proven).

## Defect census (pre-run)

| # | Finding | Disposition |
|---|---|---|
| C1 | The existing calendar cannot define a 2016–2022 session identity | FIXED + PROVEN (`sessions.rs`; 10 tests, 4 killed mutations) |
| C2 | The accepted exact-target replay refuses a binding stress cap, so a USD 5,000 cap on a 1000-bps quantity (or the historical 25,000 cap, non-binding here) cannot express half exposure; every stress would error | FIXED + PROVEN as the operator-directed 500 bps recomputed-quantity scenario (Python replay, Rust finalize entry point, runner contract; 8 killed mutations). Baseline refuse-don't-resize semantics unchanged |
| C3 | Registry universe bounds (`REGISTERED_STRATEGY_IDS`, `MAX_STRATEGY_UNIVERSE`, daemon over-limit tests) mirror the engine count | FIXED + PROVEN (11 → 14; portfolio/daemon/runtime tests green) |
| C4 | `BacktestBar::new` defaults `day_id` to a constant; a fixture that does not set it freezes the risk "day" and trips the daily-loss halt | ALREADY CORRECT in production (the CSV loader derives `day_id` from `end_ts`); fixture corrected in the new scenario test |
| C5 | A stale `core-rs/target/debug/mqk-cli.exe` lacks the new engines | FIXED + PROVEN: `run_batch.py check` probes the CLI for every declared slot (stale binary refuses); `MQK_M1_CLI` selects the binary |
| C6 | The runner checked only the trial COUNT before attempts | FIXED + PROVEN: `registration_gate` (exact predeclared identities, zero attempts) run by `register`, `gate` and `trials` |
| C7 | `summarize_batch.py` read only Benchmark V2 evidence and lost failed trials | FIXED + PROVEN: capital-fraction evidence, cross-class refusal, failed trials stay as rows |
| C8 | Batch outcome/selection was manual prose | FIXED + PROVEN: `select_batch.py` (pure, 8 killed mutations) |
| C9 | Intraday `calendar.rs`: `is_nyse_holiday` lacks 2025-01-09 and lists 2027-12-31 as a holiday (NYSE publishes no Friday observance for Saturday New Year's Day); `EARLY_CLOSE_DATES` lacks e.g. 2023-07-03 and 2025-07-03 | OUT OF SCOPE, recorded (bar-end gap classification only; integrity gap tolerance 3 bars absorbs single missing weekdays; sessions/H1/H2 do not use it). Not fixed here |
| C10 | `require_bars_match_manifest` rejects a manifest labelled `1D` for real midnight-ET daily bars (sub-day UTC hour); the real provenance label is `1Day` | OUT OF SCOPE, recorded (label convention; the real manifest passes) |
| C11 | `capital_fraction` stress evidence is not independently required by the promotion verifier (it checks presence of `stress_spec`) | RECORDED: enforced here by the runner contract + CLI + evidence echo; promotion-time enforcement is a later-mission hardening |

| C12 | The Rust robustness gauntlet and native stress suite compared the UNWRAPPED factory instance's fingerprint with the baseline's capital-fraction WRAPPER fingerprint: every scenario failed with a fingerprint mismatch for every capital-fraction candidate (found by the first real pass; pass-1 robustness verdicts VOID) | FIXED + PROVEN (`a360d2c1`; scenario test + 2 killed mutations; Backtest/finalize/review re-run as pass 2; no economic attempt retried) |
| C13 | `scenario_scan_canonical_config_binding_01` shares a pid-named temp fixture directory across parallel tests (intermittent `registry.json` read failure; passes with `--test-threads=1`) | OUT OF SCOPE, recorded (untouched file; test-only race) |
| C14 | README status lines said Batch 02 NOT STARTED | FIXED (tiny README/README_TECHNICAL status correction) |

## Second adversarial sweep and mutation log

The sweep (24 challenges, every one FIXED + PROVEN or ALREADY CORRECT + PROVEN, none BLOCKED) and the full mutation log are in the review package (`12_proof/`); the summary: **38 source mutations, 38 killed** — calendar 4 (drop a closure, remove the weekday check, remove the coverage check, non-strict next-session), H1/H2 9, H3 7 (`>=`, 49, 51, 9, 11, current bar included, non-positive close allowed), half-exposure stress/runner contract 8, selection 8, robustness identity 2. Each mutation was applied, run against its load-bearing test, and restored byte for byte (hash-checked).

Remaining ordinary deterministic in-scope defects: none.

(The result, benchmark, robustness and selection evidence are in `M1_BATCH02_RESULT.md`.)
