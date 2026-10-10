# M1 KISS campaign closeout: census, second sweep, proof and boundaries

Mission `V4-M1-KISS-CALENDAR-CAMPAIGN-PREDECLARATION-AND-ENGINE-01` · baseline `a49cae1f9a2849c769707d239a61e10cb0f39903` (= origin/main; CI #637, all six jobs success). Local commits only, **not pushed**.

**Status: `LOCALLY_COMPLETE` for the authorized engineering and predeclaration scope; `AWAITING_INDEPENDENT_REVIEW`.** Research execution is **not authorized**: `execution_gate.executable = false`, blocker `OPERATOR_EXECUTION_AUTHORIZATION_REQUIRED`. No trial is registered in any real registry, no economic attempt exists, no result, no Promotion, no Paper/Live action. M1 stays `M1_BLOCKED`.

Companion records: `M1_KISS_EXT032_CAMPAIGN_DESIGN.md` (decisions, EXT-032 semantics), `M1_KISS_EXT032_SEARCH_ACCOUNTING.md`, `M1_KISS_EXT032_RESULTS_REVIEW_FRAMEWORK.md`; machine authority `PREDECLARED_KISS_EXT032_ETF_01.json`.

## 1. Commits (one coherent invariant each)

| Commit | Invariant |
|---|---|
| `81702cf` | operator economic-policy record and frozen EXT-032 design |
| `4850082` | `us_equity_regular_sessions_v2`: own id, coverage 2016-2028, scheduled vs unscheduled closure classes, early closes, as-of knowledge, own content hash `88b305db...9fe2`; v1 untouched |
| `b575426` | explicit `CalendarContract {V1, V2}` seam for the shared calendar helpers; every existing engine keeps the v1 path |
| `49d04b5` | native `pre_holiday_two_session_long_v1` (25th identity, `MAX_STRATEGY_UNIVERSE` 24 to 25), causal and restart proofs |
| `4a2bb00` | non-executable four-trial predeclaration, stage guards, registration-gate proofs, Rust fingerprint pins |
| `63cd838` | cross-campaign search accounting; pooled DSR/PBO blocked with proof |
| `6aa9f96` | read-only near-miss results review for every registered trial |

## 2. Frozen v1 invariance evidence

* `sessions.rs` is byte-unchanged; its content string and the pinned hash `3249ee51...76de` are asserted in the strategy crate; `lib.rs` only gains the `sessions_v2` module line.
* The v1 identity push through the seam equals the historical recipe byte for byte (test), and a mutant binding v1 to v2 is killed.
* **Fingerprint comparison against the baseline build:** `mqk-cli backtest native-fingerprint` built from `a49cae1` and from this HEAD gives identical `semantic_fingerprint` and `required_history_bars` for all 24 pre-existing identities x {SPY, DIA} x {plain, capital-fraction wrapped} = 96 of 96 identical, 0 different (the baseline worktree was removed afterwards).
* No `PREDECLARED_*.json` other than the new one changed; only Batch 03 carries `executable = true` (its own earlier authorization), asserted by test.

## 3. EXT-032 engine semantics (summary; full text in the design record)

Long/flat, stateless, two-bar history. Target at the completed bar of session `t` is +1 iff the next *scheduled* session is one of the two scheduled sessions immediately preceding a scheduled exchange holiday. Entry signal on `P3`, fill on `P2` (next bar, bar-range price plus slippage), hold through `P1`, exit signal on `P1`, fill on the first session after the holiday. The `P3`-to-`P2` return is not capturable (no same-bar fill) and the holiday gap is unavoidable exposure. Unscheduled closures are never events; non-consecutive, stale, incomplete, uncovered inputs are flat; overlapping events union into one continuous hold. No window straddles a 1 March fold or holdout boundary in 2016-2028.

## 4. Predeclaration identity and cardinality

`M1-KISS-EXT032-ETF-01`: 1 hypothesis, 4 trials (SPY, QQQ, IWM, DIA, order frozen), 0 attempts, 8 pinned fingerprints (4 inner, 4 capital-fraction wrapped, recomputed by a CI-run Rust test), instrument-registry rows hash-pinned, Batch 03 economic/benchmark/stress/promotion/fidelity blocks asserted equal, calendar v2 hash pinned against the Rust source, `EXPOSED_DEVELOPMENT` grade, holdout excluded. The four real trial ids are not computable before the data fetch (they bind the bars provenance); the identity rule and its 4/0 registration gate are proven with synthetic provenance.

## 5. Historical denominator (OD-4)

Exact inventory: 130 registered trial identities in nine frozen declarations; 115 under the operative rule (excluding the 15 superseded Batch 01 v1 evaluations); 105 unique strategy/symbol pairs. Operative cumulative count **119**; sensitivity bounds 134 and 109. The operator's 115 is verified as a registry-identity count, not as a strict hypothesis-instrument dedup. Pooled DSR/PBO is `BLOCKED_UNSUPPORTED` (judge comparison key binds the bars provenance; prior return series are not committed; other protocols); the gate DSR/PBO come from the four-trial judge and are **not deflated** for the prior campaigns; the review prints the count and a caveat beside every DSR.

## 6. Defect census (before editing) and second adversarial sweep

Every item FIXED+PROVEN, ALREADY_CORRECT+PROVEN, DEFERRED (with reason) or BLOCKED.

| ID | Finding | Disposition |
|---|---|---|
| C1 | E1: monthly/calendar engines fail closed flat from 2026-12-31 (v1 horizon bound into their fingerprints) | seam **FIXED+PROVEN** (v2 resolves 2027-2028, v1 still refuses, tests + 3 mutants). Registered v2-bound siblings of F01/F05/F08/F10, turn-of-month and Halloween: **DEFERRED**: each is a new strategy identity (registry id, universe cap, fresh prospective declaration) and no campaign needs one; recipe: add a `CalendarContract` field, push `push_calendar_identity_for(V2)`, register a distinct id |
| C2 | v1 conflates scheduled holidays and unannounced closures (2018-12-05, 2025-01-09) | **FIXED+PROVEN** in v2 (classes, rule-derivation test, `closure_knowable_at`, six calendar mutants) |
| C3 | three calendar authorities (v1 table, Paper-runtime table, now v2) | **FIXED+PROVEN**: v2 equals v1 on every v1-covered date and the runtime table on every 2023-2028 date and early close; runtime table untouched |
| C4 | `select_batch.py` / `summarize_batch.py` ignored `execution_gate` | **FIXED+PROVEN** (guards + subprocess refusals + mutants) |
| C5 | a graded declaration with no `execution_gate` was treated as a historical, runnable one | **FIXED+PROVEN** (refused; historical declarations unaffected) |
| C6 | `MAX_STRATEGY_UNIVERSE` capacity / hard-coded 24s | **FIXED+PROVEN** in the registration commit (daemon, portfolio, pin tests) |
| C7 | legacy pins forbade any `EXT-` id in a declaration | **FIXED+PROVEN**: three pins now allow exactly one non-executable declaration naming exactly EXT-032 |
| C8 | ambiguous historical denominator (115 vs 105 vs 130) | **FIXED+PROVEN** (accounting module, doc, tests) |
| C9 | pooled cross-campaign statistics | **BLOCKED** (proved unsupported; no number produced) |
| C10 | `research-py` CI runs only `tests/`; the `experiments/` tests (including all new Python tests) are local-only | **OUT_OF_SCOPE, recorded**: pre-existing convention. The engine, calendar and fingerprint pins are Rust tests and run in CI |
| C11 | fetch window ends 2026-09-01 because the bridge derives the holdout start from the fetched bars | **BLOCKED on execution authorization**: recorded as U5; the operator must acknowledge the raw-row fetch of the reserved months at execution time; evaluation inputs are truncated and the holdout guard fails closed |
| C12 | no verified bars exist for DIA | precondition U3 of execution |
| C13 | 2016-2022 early-close dates are rule-derived (the rule reproduces the published 2023-2028 table exactly), not provider-evidenced | recorded U6; economics-neutral for EXT-032 |
| C14 | TODO/FIXME/`unimplemented!`/new `#[ignore]`/skips in the diff | none added (diff scan); existing ignores (backtest 4, cli 13, daemon 8) are pre-existing |
| C15 | **Incident (mine):** a mutation run that opened the gate made the spawn tests execute `run_batch.py fetch --execute`, and the environment held Alpaca credentials. About 10.7k raw bars for the four symbols (2016 to 2026-09, therefore including the reserved months) plus corporate-action files were downloaded into a git-ignored run directory, and an empty registry was created. No trial, attempt, judge, backtest or evaluation ran; I never read the rows; the directory was deleted and nothing was committed | **FIXED+PROVEN**: spawn tests strip provider credentials, refuse to start if the declaration is executable, drop `--execute`; re-running the opened-gate mutant now fails before any spawn and creates nothing. **Disclosure for the operator:** a provider read that included the reserved window happened; no economic calculation used it. Whether this affects the single-use character of the holdout is the operator's call |

Second sweep (same dispositions): lookahead (engine reads the scheduled calendar and the latest two bars only; no price) ALREADY_CORRECT+PROVEN; same-bar fills (fill bar strictly after signal bar, asserted on every fill) PROVEN; result-dependent identity (identity builder has no result input; accounting and review take no result) PROVEN; retry vs trial (retries add attempts, never trials; 4 stays 4) PROVEN; winner/near-miss selection (`selection` null, no rank field, selector refuses, mutant killed) PROVEN; holdout (declaration excludes it, guard breach proof per artifact category, fold boundaries cannot straddle) PROVEN with C11/C15 disclosed; Paper/Live (no daemon, DB, broker or promotion code touched) PROVEN by diff; Gate 3b (untouched) PROVEN by diff; duplicate authority (one calendar helper seam, one fingerprint constructor reused) ALREADY_CORRECT; seam-bypassing tests (spawn tests use the real entry points; engine tests use the real BacktestEngine and registry) PROVEN; false-positive fixtures (expected windows from an independent backward walk, calendar from an independent rule derivation, fingerprints recomputed from the constructor) PROVEN, with the interface pins to the evidence producer being string-level only (LIKELY until a real run exists).

## 7. Proof counts (affected-subsystem acceptance, one run at closure)

`mqk-integrity` 93, `mqk-strategy` 405, `mqk-backtest` 569 (4 pre-existing ignores), `mqk-portfolio` 461, `mqk-cli` 179 (13 pre-existing ignores): 0 failed. `mqk-daemon` lib `dynamic_selection` subset 121 passed, 8 pre-existing ignores. Python `m1_native_trend_campaign` + `external_idea_intake`: 589 passed, of which 108 are new. `cargo fmt --check`, `clippy -D warnings` on the five changed crates, the unsafe-pattern, ignored-proof, migration-governance and workspace-dependency guards: clean. Mutants killed: 6 calendar, 3 seam, 7 engine, 6 gate/guard, 5 registration-gate, 5 accounting, 10 review.

Full local workspace acceptance: NOT RUN, prohibited by laptop resource-safety rule; broad workspace proof delegated to GitHub CI. Exact-HEAD CI, provider truth, operational Paper and real-market evidence are not covered by any of this.

## 8. Unresolved constraints and next step

* Pooled statistics BLOCKED; the gate DSR is not deflated for 115 prior trials (disclosed in every row).
* No independent confirmation is available by design; forward Paper after lawful Promotion is the only later independent observation.
* U3 fetch and verification of DIA bars, U4 trial ids after fetch, U5 holdout-window fetch acknowledgement: all belong to the execution authorization.
* Next (not authorized here): the operator re-issues the declaration executable (gate only, disclosed), then fetch and verify bars, register 4 and prove 4/0, execute once without retries-as-trials, one batch judge, scanner review, `near_miss_review.py`, independent review.

## 9. Addendum (V4-M1-KISS-EXT032-ALL-DEFECT-CLOSURE-01): holdout incident status

C15 above is superseded in status by `M1_KISS_EXT032_ACCESS_INCIDENT_01.md` and the hash-chained ledger `HOLDOUT_ACCESS_INCIDENTS.json`: the incident is `ACCESS_INCIDENT_PENDING_ADJUDICATION`. "RESERVED / UNCONSUMED" in the sections above describes the per-run formal ledger only; it is not an independence certification. C11 ("operator acknowledges the raw-row fetch") is no longer the mechanism: the prospective data path must not request reserved dates at all.
