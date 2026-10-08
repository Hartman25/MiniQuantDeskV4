# M1 Post-Discovery Candidate Authority and OOS Readiness

Mission `V4-M1-POST-DISCOVERY-CANDIDATE-AUTHORITY-AND-OOS-READINESS-01` · 2026-10-08 · baseline `a75dbdbe01136519911341e56168c9db01f5f93f` (= origin/main).
Local commits, **not pushed**. No research campaign, registration, attempt, provider call, Paper/Live action, OOS/Confirmation/Final-Holdout read or Promotion happened. No frozen result, population or threshold was changed.

**VERDICT: `NO_ELIGIBLE_CANDIDATE_EXISTS` · `NO_AUTHORIZED_EXECUTABLE_NEXT_RESEARCH_STEP` · `OPERATOR_DECISIONS_REQUIRED`.** M1 stays `M1_BLOCKED`.

## 1. The answer

| Question | Answer (evidence in the cited section) |
|---|---|
| Is any Census-02 or Batch 03 result Promotion-eligible, or a candidate? | **No.** Census-02 is Discovery-only label/Python-simulator evidence (no native engine, no multiple-testing control, shorts blocked in Paper). Batch 03 has 0/60 positive cost-aware alpha, max DSR 0.092 against the required 0.5, 0 `paper_candidate`. (§3) |
| Does an accepted policy define the next evidence step for these results? | **No.** Census-02 §8 funnel is "proposed, no new threshold encoded"; its Result §5 says a "separately authorized evidence path" is required and none is. Batch 03 defines only the mechanical advancement of F05/F04/F01 and says Confirmation "needs its own predeclaration". (§6, §7) |
| Does genuinely fresh OOS data exist? | **Only the Final Holdout `[2026-03-01, ...)`, which is reserved, open-ended and single-use.** Every earlier calendar window is exposed to Census-01/02, Confirmation-01 and/or the native Batch 01-03 walk-forward folds. (§4) |
| Is the prepared Batch 03 Confirmation population clean? | **No.** Five of six symbols (DIA, XLF, XLI, XLP, XLV) were in the Census-02 Class-C Strategy scope and the Confirmation-01 universe; MDY is unseen but has no registry identity or history metadata. The dates fully overlap Batch 03 Discovery. It would be a cross-instrument replication, not temporal OOS. (§4, §5) |
| Can the native engine/evidence bridge carry a Discovery family to Promotion without changing its economics? | **Yes for the long/flat native families** (fingerprint, stress contract, capital-fraction identity, registry authority all bind); **no for Census-02** (no native engine, long-only capital-fraction sizing, B5 blocks short opens). Monthly families F01/F05 carry a calendar horizon (§8, E1). |
| What must be authorized next? | Operator decisions OD-1..OD-8 (§11), then one predeclaration controller (§12). Nothing may be registered or executed before that. |

## 2. Tool usage (actual)

Installed MQD skills inspected (`mqd-test-proof`, `mqd-diagnose`, `mqd-review-patch`, `mqd-handoff`, `mqd-external-research`, `archify`). `mqk_readonly`, Srclight and Graft MCP servers were **not available** in this cloud session (ToolSearch returned none); discovery used restricted native Grep/Read and focused `cargo`/`pytest`. No subagents, Context7, Firecrawl or Playwright. Real Paper DB: not reachable from this container (not touched). A disposable local Postgres 16 in the sandbox was used for DB-backed proofs only.

## 3. Candidate readiness matrix

Eligibility vocabulary: Discovery-only = no OOS-eligible status; exposure = §4 windows. Ranking is by engineering readiness only; no economic ranking.

| Population | Evidence (frozen) | Native implementation | Multiple-testing authority | Window exposure | Promotion-compatible identity | Status | Missing deterministic prerequisites / decisions |
|---|---|---|---|---|---|---|---|
| Census-02 Strategy: SH01-SH13 (short) | 9,400 trials, 279 QUALIFIED (117 short); Python daily-bar simulator, **assumed** 100 bps borrow; 49 of 117 short QUALIFIED are TLT | **None** (no Rust engine; capital-fraction wrapper treats short targets as flat; daemon B5 blocks `ShortOpen`) | None: DSR/PBO `DEFERRED_FULL_POPULATION`; local population + Census-01 global disclosure | `[2016,2024)`; also read by Census-01 | **Unavailable** | `DISCOVERY_ONLY`, not OOS-eligible | Short-side Paper enablement; borrow/ETB/SSR truth; native engines; predeclared nomination rule (no post-hoc winner); OOS regime; denominator (OD-1,2,4) |
| Census-02 Strategy: LS01-LS04 (long_short) | 162 of 279 QUALIFIED; bar is net P&L > 0 vs cash over a drifting window, explicitly **not** market-neutral edge | None | None | same | Unavailable | `DISCOVERY_ONLY` | same as above |
| Census-02 conditional factors (1,075; 276 favorable STRONG) | Forward-return **labels** (no costs/fills/borrow); BH q over 1,065 tested, effectively dependent | n/a (factors are not strategies) | BH alpha 0.10, local family | `[2016,2024)` | n/a | Hypothesis generation only | Each would be a **new** hypothesis needing predeclared native strategy + new Discovery; Confirmation-01 already returned 0 STRONG on the Census-01 analogue |
| Batch 03 advanced: F04 `close_channel_100_50_trend_v1` | rank 2; alpha < 0 on 6/6; median DSR 0.016; 0/6 robustness | Rust engine, `DurableStateRequired`, restart-proven, daemon dispatch proven | Batch judge over 60 (PBO 0.0714, N_eff 39.53/53); no cross-batch denominator | folds 2016-03..2026-02 (SPY,QQQ,IWM,SMH,XBI,XLE) | Fingerprint binds strategy+symbol+timeframe+sizing; stress contract bound | Advanced mechanically; **not eligible** | Engineering-ready now (no calendar bound) |
| Batch 03 advanced: F05 `monthly_10month_trend_timing_v1` | rank 1; median DSR 0.025; median DD improvement +3.2 pts; alpha < 0 on 6/6 | Rust engine, stateless; **calendar-bound** | same | same | same; fingerprint binds `us_equity_regular_sessions_v1` (coverage ends 2026-12-31) | Advanced mechanically; **not eligible** | E1 (calendar horizon) before Paper or any registration meant to be deployed after 2026-12-30 |
| Batch 03 advanced: F01 `monthly_multihorizon_abs_momentum_consensus_v1` | rank 3; median DSR 0.013; alpha < 0 on 6/6 | same as F05 | same | same | same | Advanced mechanically; **not eligible** | E1 |
| Batch 03 not advanced: F08, F10, F07, F06, F09 | ranks 4-8, `DISCOVERY_NOT_ADVANCED` | Rust engines | same | same | same | Preserved as not advanced | none may be revived by sorting results |
| Batch 03 F02, F03 | 2/6 and 3/6 evaluable; 7 deterministic `no native signals inside fold N` failures kept as non-evaluable | Rust engines | same | same | same | `DISCOVERY_INSUFFICIENT_EVALUABLE_TRIALS` | A future no-trade-fold semantics is a policy change (OD-8), not a defect |
| Earlier native work: trend_sma50 campaigns 01-03, dual_sma_50_200, pullback_mean_reversion_20_2, Batch 01 (corrected), Batch 02 | all REJECTED / `BATCH_REJECTED`, 0 `paper_candidate` | engines exist | per-experiment judges | folds 2016-03..2026-02 (EFA,GLD,IEF,SPY,VNQ) | n/a | **Preserved rejected** | none; about 55 prior native trials (campaigns 25, Batch 01 15, Batch 02 15) plus Batch 03's 60 on the same window are not pooled in any judge |
| Earlier research waves: Discovery_01 low-vol, SHORT_01/SHORT_WAVE_02, Wave06, Census-01, Pass-2, Confirmation-01 | NOT_VALIDATED / `REJECTED_NOT_ADVANCED` / 0 CONFIRMED_STRONG | various | various | various | n/a | Preserved | none |
| Deployed legacy `intraday_scalper` (AAPL/5m/300) | 0 promotion transitions | native | n/a | n/a | no Research trial | Not a candidate | none |

What `paper_candidate` requires (code, `strategy_scan_review.rs`): bars ≥ 252, trades ≥ 5, total return ≥ 0, **cost-aware alpha ≥ 0 versus the matched passive benchmark**, drawdown ≤ 25 %, profit factor ≥ 1.05. Promotion additionally needs the native fingerprint, registered stress contract, succeeded attempt, a batch judge with DSR ≥ 0.5 and PBO ≤ 0.5, Sharpe ≥ 0.5, CAGR ≥ 0, profitable months ≥ 40 %, and a reserved holdout. Batch 03 clears none of it.

## 4. OOS / Final-Holdout provenance and consumption matrix

Source authority: `c2_protocol.partition_truth()` (pinned by `test_alpha_edge_census_02_partition_truth.py`), `confirmation_consumption_proof.json`, the `PREDECLARED_*.json` partitions (all native batches: `evaluation_start 2016-03-01`, 10 x 12-month test folds, so fold 10 is `[2025-03-01, 2026-03-01)`), Batch 03 holdout guard.

| Window | Recorded status | Exposed to | Genuinely OOS? |
|---|---|---|---|
| `[2016-01-01, 2024-01-01)` | Discovery-exposed | Census-01, Census-02 (88 symbols, strict `< 2024`), native Batch 01-03 and campaigns | No |
| `[2024-01-01, 2025-01-01)` | `CONTAMINATED_BY_REJECTED_RUN` (Census-02 truth) | Census-01 rejected run, native folds 8-9, Confirmation-01 warm-up only | No |
| `[2025-01-01, 2026-03-01)` | `CONSUMED_BY_ALPHA_EDGE_CONFIRMATION_01` (87 symbols, labels) | also **scored by native folds 9-10** of Batches 01-03 on their universes | No, "never fresh again" |
| `[2026-03-01, ...)` open-ended | `RESERVED_UNCONSUMED`; Batch 03 guard clean (`reserved_not_evaluated`) | bars to 2026-09-01 are cached in the gitignored Batch 03 run dir but were truncated before every evaluation | **Yes, single-use** |

**Finding (record, not a defect in a frozen result).** There is no pipeline-spanning, symbol-agnostic consumption ledger. `research_holdout_ledger` is per registry file and keyed by a dataset content hash, and no production caller invokes `consume_holdout`; Census-02's truth object lives inside Census-02; Confirmation-01 (first read 2026-10-07) treated `[2025-01, 2026-03)` as an unread reserve after native Batches 01-03 (2026-10-01..05) had already scored folds in it for their universes. Consumption is therefore procedural, and this table is the first cross-campaign reconstruction. The Promotion verifier checks the **artifact's** `holdout.status == reserved_not_evaluated`, not a live ledger.

Confirmation-universe instrument exposure (committed non-reserved metadata only):

| Symbol | Census-02 Class-C Strategy scope | Census-02/Confirmation-01 88-symbol universe | Instrument registry row | Committed history metadata |
|---|---|---|---|---|
| DIA, XLF, XLI, XLP, XLV | **yes** (`[2016,2024)` Strategy economics) | **yes** (Confirmation-01: 541 obs 2024-01..2026-02) | yes | first bar 2016-01-04, 2,012 rows to 2023-12-29, contiguous with 541 rows to 2026-02-27 |
| MDY | no | no | **no** (XBI also absent; Batch 03 used a per-run identity supplement) | **none** |

History sufficiency therefore **is** decidable for 5 of 6 from existing metadata without reading economic rows; MDY is unverifiable without a (not authorized) provider call.

## 5. Batch 03 Confirmation: readiness and authorization

1. **Frozen authority:** only `confirmation_preparation = PREPARED_NOT_REGISTERED` (universe, 3 families x 6 = 18, no symbol substitution, history verified at registration time), the mechanical advancement rule, and the inherited sizing/benchmark/promotion thresholds. No Confirmation declaration file, trial, attempt or outcome rule exists.
2. **Still required (independent predeclaration):** population, partition/window, outcome rule (what counts as confirmed), multiple-testing denominator, stress contract (a contract registered per trial before its first attempt), disclosure of the Census-02 and earlier-batch exposure.
3. **Genuinely OOS windows/instruments:** only the Final Holdout in time; only MDY in instruments (and it is unverified). Anything else is replication of exposed data.
4. **History sufficiency:** see §4 (5/6 evidenced; MDY not).
5. **Engine/bridge:** the generic runner is declaration-driven (N from `max_trials`, 60/0 gate generalizes to N/0, every stage refuses a non-executable declaration, holdout guard pre/post). F04 is ready; F05/F01 need E1 for Paper. MDY needs an identity row or the supplement mechanism.
6. **Needs explicit operator authorization:** the predeclaration content (OD-1..8), re-issuing it executable, trial registration, attempts, any Final-Holdout consumption, Promotion, Paper deployment/arming.

**Assessment:** executing Confirmation as prepared would produce neither independent nor Promotion-eligible evidence: the same dates, five previously exposed instruments highly correlated with the Discovery set, and a Discovery population whose every advanced family has negative benchmark alpha. It is not the shortest truthful path.

## 6. Census-02 disposition (frozen result unchanged)

`DISCOVERY_ONLY`, `NOT_VALIDATED`, `PROMOTION_AUTHORITY = NONE`; no winner is chosen and none may be chosen by sorting. Any use requires (i) a predeclared nomination rule fixed before looking at post-result metrics, (ii) native engines with the same economics, (iii) short-side Paper enablement and borrow truth, (iv) a multiple-testing denominator for 9,400 + 1,075 (+ Census-01 disclosure), and (v) a fresh OOS regime. Not recommended as the first M1 candidate source: it needs five separate unauthorized prerequisites.

## 7. Authority chain (what code enforces versus what is policy)

| Stage | Authority | Enforced in code? |
|---|---|---|
| Discovery → family disposition | frozen predeclaration + `family_ranking.py` | yes (ranking, 60/0 gate, execution stop) |
| Predeclaration → trial identity | `build_native_signal_trial_identity` (fingerprint, sizing, stress contract, canonical timeframe, provenance) | yes |
| Trial → attempt → judge | `ResearchResultStore`, batch judge | yes (retries never mint trials) |
| OOS / Confirmation / Final Holdout | procedural partition truth | **no cross-campaign ledger**; per-registry ledger, never consumed by any caller |
| Backtest → scanner review | `strategy_scan_review` + Benchmark V2 binding | yes |
| Promotion | `verify_promotion_oos_evidence` + `evaluate_promotion` + stress-contract and fingerprint binding | yes; **no Confirmation stage exists in Promotion code**: a trial that clears the gates inside one properly registered experiment is eligible |
| Paper eligibility | Gate 3b `active_paper` exact identity | yes |

Consequence: "independent validation" beyond the Promotion gates is an operator-defined research policy, and the OOS regime for it (OD-1) is the first missing decision.

## 8. Defect census and dispositions

Every finding is FIXED+PROVEN, ALREADY_CORRECT+PROVEN or BLOCKED.

| ID | Finding | Disposition | Proof |
|---|---|---|---|
| D1 | M1.10 soak ledger validated dates against research `sessions` (ends 2026-12-31) while the Paper runtime calendar covers 2023-2028: every valid session from 2027-01-04 was `OutOfCoverage`, so the gate could never pass | **FIXED+PROVEN** (`8a9114c`) | RED test failed on 2027 sessions; GREEN; parity of the new date-level predicate with the intraday classifier on every table date and with `sessions` on every shared date and next-session; 4 mutants killed (ledger back on `sessions`, holiday check removed, coverage check removed, next-session ignoring closures) |
| E1 | Monthly engines F01/F05/F08/F10 (and turn-of-month/Halloween) resolve month-ends through `sessions` v1 and bind its content hash into their fingerprint; for any bar on or after 2026-12-31 the target is fail-closed flat (pinned by `monthly.rs` test "uncovered next session") | **BLOCKED** (material change to frozen fingerprints; the Batch 03 predeclaration pins `3249ee51...` and coverage end 2026-12-31) | needs OD-6; fix recipe: new session-calendar contract id covering 2023-2028 from the Paper-runtime table, engines migrated, Batch 03 pins left as historical; registered-before-migration trials become unpromotable (already rejected) |
| E2 | `scenario_held_sizing_state_01` (mqk-db) and `scenario_capital_fraction_restart_01` (mqk-runtime) are `#[ignore]` DB proofs of the M1.9 dispatch contract that no CI step invoked | **FIXED+PROVEN** (`57fa62f`): added to the DB proof lane with `--include-ignored`; the CI-11 guard now fails if a listed proof is missing, commented out or lacks the flag | proofs green locally (5 + 9 tests, disposable Postgres 16); 3 guard mutants killed |
| E2b | Daemon `capital_fraction_dispatch_tests` (20 tests) use a skip-when-no-DB convention and require a `:5434` URL: against any other URL they pass vacuously in 0.01 s | ALREADY_CORRECT for CI (workspace job provides the health-checked `:5434` service); caveat recorded | against a real `:5434` database the same 20 tests ran 9.69 s and passed |
| E3 | `config/instruments/equities.json` lacks XBI and MDY (Batch 03 relied on a membership-only per-run supplement) | ALREADY_CORRECT for Batch 03 (supplement pinned membership-only); prerequisite recorded for MDY/Paper | `test_batch03_scan_registry_supplement.py` |
| E4 | M1.10 evaluator has no operator entrypoint or DB-to-record producer; records are hand-captured per the runbook | **DEFERRED** (not required by the documented gate; implement with the first deployment) | n/a |
| E5 | Promotion reads the artifact's holdout status, not a live ledger | ALREADY_CORRECT for the current contract (no code path consumes the holdout); recorded for OD-3 | `research_evidence.rs` |
| E6 | Retry-as-trial, 59/61 trials, attempt-before-gate, `1D`/`1Day`, symbol substitution, result-in-identity, holdout in artifacts | ALREADY_CORRECT+PROVEN | 151 Batch 03 / guard tests re-run green at this HEAD (+ full `m1_native_trend_campaign` directory, see §10) |
| E7 | Promotion fingerprint binds symbol and timeframe (an F05/SPY trial cannot authorize F05/XLE) | ALREADY_CORRECT+PROVEN | engine fingerprint pushes `symbol` and `TIMEFRAME_SECS`; daemon compares server-resolved fingerprint |
| E8 | Daemon capital-fraction dispatch / migration 0091 | present and proven (migration 0091 in repo and `manifest.json`; fence test derives the latest id from the manifest); the real Paper DB is at 76 per the last read-only record and was **not** touched | |

## 9. M1.9 / M1.10 candidate-independent status

* `M1_SYSTEM_CANDIDATE_READY = true` remains supported **for non-monthly candidates**; for F01/F05/F08/F10 it is conditional on E1 after 2026-12-30.
* Migration 0091: present in repo; real Paper DB at 76 (last read-only record, unchanged): apply 77-91 through the canonical boot path at deployment, then verify.
* M1.9 unresolved ordinary candidate-independent defects: E1 only (blocked on OD-6). E2 (CI wiring of the held-sizing store and restart proofs) is fixed; the daemon dispatch tests already run in the CI workspace job. Everything else in the closure matrix stands.
* M1.10: evaluator complete and now correct beyond 2026 (D1); 10-countable/5-consecutive, deployment identity, SHA binding, idempotent duplicates all pinned; capture is manual (E4); calendar coverage ends 2028-12-31.
* Real-world prerequisites unchanged: deployed candidate with `active_paper`, re-arm (`DISARMED InboundContinuityUnproven`), host sleep mitigation, unattended pre-open start, market sessions.

## 10. Proof record

| Check | Result |
|---|---|
| `cargo test -p mqk-integrity` (all targets, incl. new ledger and parity tests) | 80 passed, 0 failed; `fmt --check` and `clippy --all-targets -D warnings` clean |
| D1 RED then GREEN, 4 mutants | killed (see §8) |
| `cargo test -p mqk-promotion` (all targets, baseline) | exit 0 |
| DB proofs vs disposable Postgres 16: `scenario_held_sizing_state_01` (5), `scenario_capital_fraction_restart_01` (9), daemon `capital_fraction_dispatch_tests` (20, on `:5434`) | all passed |
| research-py: `experiments/m1_native_trend_campaign` + holdout ledger/wiring + Census-02 partition truth + Confirmation-01 | 475 passed (includes the 15 new pinned-fact tests; 5 mutants killed, byte-identical restore) |
| CI-11 guard + 3 mutants | killed |

Full local workspace acceptance: NOT RUN — prohibited by laptop resource-safety rule; broad workspace proof delegated to GitHub CI.

## 11. Blocker list

| Class | Blockers |
|---|---|
| Engineering (candidate-independent) | E1 monthly calendar horizon (needs OD-6); E4 M1.10 entrypoint (deferred); MDY/XBI registry identity for Confirmation or Paper |
| Economic research | no candidate with positive cost-aware alpha anywhere; Discovery population DSR ≤ 0.092; no OOS-clean window; benchmark bar versus long/flat timing in a drifting window |
| Operator authorization | every item in OD-1..OD-8; executable re-issue, registration, attempts, holdout consumption, Promotion, deployment |
| Operational Paper | migrations 77-91 on the real DB; deployment of a promoted candidate; arm/inbound-continuity proof; sleep mitigation; unattended pre-open start; 10 countable / 5 consecutive clean sessions |

## 12. Operator decisions (minimal, none invented here)

* **OD-1 Candidate bar and OOS regime.** Is M1's first candidate required to show genuine out-of-sample alpha, or is it a pipeline-validation candidate that must still clear every existing gate? And what is the independent-validation regime: (a) one-shot Final-Holdout consumption for one frozen candidate, (b) instrument-disjoint replication (declared as a weaker evidence grade), (c) forward Paper as the only independent evidence?
* **OD-2 Population.** Keep the six prepared symbols (five previously exposed), substitute (the rule forbids silent substitution), or approve MDY-only/other unseen instruments.
* **OD-3 Holdout use.** Authorize or refuse any Final-Holdout consumption; if authorized, one frozen candidate, one use, ledger-recorded.
* **OD-4 Multiple-testing denominator** for a Confirmation judge: its own 18, or 18 plus the 60 Discovery trials, or cumulative native trials on the window (about 55 + 60).
* **OD-5 Stress contract** for any Confirmation/new trial (scenario, fraction < 1000 bps, slippage, volatility, drawdown ceiling).
* **OD-6 Calendar migration:** authorize a new session-calendar contract (Paper-runtime table, 2023-2028) and the resulting fingerprint change for the monthly engines before any monthly-family Paper deployment.
* **OD-7 Outcome rule** for Confirmation (what is "confirmed"; today none exists).
* **OD-8 Benchmark/no-trade semantics:** whether long/flat timing strategies are judged against the matched passive buy-and-hold (current, 0/60 positive) or by a risk-adjusted rule, and whether a fold with zero signals is non-evaluable (current) or a valid flat fold. Both are frozen policy; changing them is an economic-policy act.

## 13. Recommendation: the ONE next controller

`V4-M1-CONFIRMATION-DESIGN-PREDECLARATION-01`: after the operator answers OD-1..OD-8, write one frozen **non-executable** predeclaration (population, partition, denominator, stress contract, outcome rule, disclosure of the exposure in §4), prove the registration gate generalizes to its N/0 trial count, and stop at `OPERATOR_EXECUTION_AUTHORIZATION_REQUIRED`. If OD-6 is granted, the calendar migration is its first commit; if monthly families are not pursued, F04 is engineering-ready without it. Do **not** run Batch 03 Confirmation as currently prepared.

Path to legitimate Promotion eligibility: OD answers → predeclaration → (calendar migration if monthly) → operator re-issues executable → register N and prove N/0 → execute once, no retries-as-trials → batch judge and scanner review → a trial clearing every gate in §3 → Promotion route with exact fingerprint and registered stress contract → `active_paper` → M1.9 deployment → M1.10 sessions.

## 14. Draft Confirmation skeleton: `NOT_AUTHORIZED` / `NOT_EXECUTABLE` (no JSON file is created, to avoid a machine-loadable declaration)

* Determined by frozen contracts: families F05, F04, F01 (ids above); sizing `fixed_initial_capital_fraction_v1` 1000 bps on USD 100,000; benchmark `capital_fraction_matched_passive_buy_hold_v1`; economics/execution and Promotion thresholds inherited unchanged; canonical daily identity; no variants, no retries-as-trials, 18 = 3 x 6 only if OD-2 keeps six symbols.
* Operator-undecided placeholders: population (OD-2), partition/window (OD-1, OD-3), denominator (OD-4), stress contract (OD-5), outcome rule (OD-7), benchmark semantics (OD-8). `execution_gate.executable = false`, blocker `OPERATOR_DECISIONS_REQUIRED`.

## 15. Second adversarial sweep (FIXED+PROVEN / ALREADY_CORRECT+PROVEN / BLOCKED)

Winner chosen by sorting post-result metrics: none (verified; ranking is family-level and mechanical). Factor label treated as P&L: none. Discovery qualification read as Promotion authority: refused (§7). Fresh-OOS presumption for 2024: refused (§4). Confirmation or holdout consumed here: no (read no data). Python/Rust populations mixed: Census-02 and Batch 03 stay separate (§3, §6). Registered-vs-executable identity: fingerprint binds symbol/timeframe/sizing/stress (E7). Retry/duplicate trials: ALREADY_CORRECT (E6). Fail-open defaults: ledger horizon (D1) FIXED; monthly horizon fails closed (E1, BLOCKED). Stale executable paths: none changed. Operator truth unavailable/empty/present: real Paper DB state is UNAVAILABLE from this session and is recorded as such, never as empty.
