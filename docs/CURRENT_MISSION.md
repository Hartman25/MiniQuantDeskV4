# MiniQuantDeskV4 — Current Mission

Status: **ACTIVE TURNOVER / CURRENT TRUTH**

This file is intentionally short. It records current durable project state, not project history.

---

## -35. Post-Discovery candidate authority and OOS readiness (2026-10-08, `V4-M1-POST-DISCOVERY-CANDIDATE-AUTHORITY-AND-OOS-READINESS-01`)

Local commits on top of `a75dbdbe`, **not pushed**. Record: `docs/research/M1_POST_DISCOVERY_CANDIDATE_AUTHORITY.md`; ledger G2.23.

- Verdict `NO_ELIGIBLE_CANDIDATE_EXISTS`, `NO_AUTHORIZED_EXECUTABLE_NEXT_RESEARCH_STEP`, `OPERATOR_DECISIONS_REQUIRED` (OD-1..OD-8). Census-02 and Batch 03 are unchanged and Discovery-only; no run, registration, provider call, OOS/Confirmation/Final-Holdout read, Paper or Live action.
- Only the Final Holdout `[2026-03-01, ...)` is genuinely OOS (single-use, reserved). Every earlier window is exposed (Census-01/02, Confirmation-01, native Batch 01-03 folds). Five of six prepared Confirmation symbols were in Census-02/Confirmation-01; MDY has no registry identity or history metadata. Do not run Batch 03 Confirmation as prepared.
- Fixed + proven: M1.10 soak ledger now counts sessions on the Paper-runtime calendar (2023-2028) instead of the research `sessions` horizon (2026-12-31) (`8a9114c`); the capital-fraction held-sizing store and restart proofs now run in the CI DB proof lane (`57fa62f`).
- BLOCKED (needs OD-6): monthly engines F01/F05/F08/F10 are fail-closed flat for bars on/after 2026-12-31 (`sessions` v1 horizon, bound into their fingerprints). `M1_SYSTEM_CANDIDATE_READY = true` stands for non-monthly candidates. M1 stays `M1_BLOCKED`.
- Next: one non-executable predeclaration controller after the operator answers OD-1..OD-8. Full local workspace acceptance NOT RUN (laptop resource-safety rule); broad proof delegated to GitHub CI.

## -34. Capital-fraction daemon dispatch closed (2026-10-05, `V4-BATCH03-DISCOVERY-THEN-CAPITAL-FRACTION-DAEMON-CLOSURE-01` Phase B)

Local commits `ae93f170`, `fbf2a362`, `6ad76924`, **not pushed**. Finding E of `docs/M1_SYSTEM_CLOSURE_01.md` is FIXED + PROVEN.

- The canonical daemon Paper strategy dispatch now routes a capital-fraction binding through the existing restart-safe `CapitalFractionRuntimeHost` (host enum keyed `(symbol, strategy_id, timeframe_secs)`; async `recover` once before the start barrier, failure fails the start closed; deployment id = UUIDv5 over strategy, symbol, timeframe and semantic fingerprint; strict contract only via `mqk_runtime::native_strategy`, no default bps/cap). Held sizing is persisted BEFORE the result is returned; a persistence failure yields no decision/outbox row and poisons the host. Result → decision → Gate 3b Promotion/risk/halt/reconcile → OMS/outbox is unchanged. Fixed-quantity strategies keep the stateless path; `instantiate_verified` and the stateless `DurableStateRequired` refusal are unchanged.
- Daemon OHLC parity fix: the daemon bar windows were built with close-only `BarStub`s, giving degenerate bars to ATR/range engines (F03/F07/F09); they now carry true OHLCV.
- Proof: 12 DB-backed tests (`state/capital_fraction_dispatch_tests.rs`, disposable test DB) and 13/13 required mutations killed with byte-exact restoration (stateless bypass, evaluate-before-recover, wrong deployment id, recompute after restart, deferred persist, escaping +Q on persist error, host rebuilt every bar, config defaults, +1 fallback, Promotion bypass, shared binding state, fixed-qty through durable host, OHLC revert).
- Independent-review correction (`V4-BATCH03-CAPFRAC-INDEPENDENT-REVIEW-CORRECTION-01`, local, not pushed): a multi-binding tick now persists ALL hosts' held-state transitions in ONE transaction (`prepare_bar` / `commit_prepared_batch`); no result is released before the commit, and any failed or aborted tick poisons every affected host until rebuilt from the DB. The env-reading `build_with_durable_state` is tested directly; shared test-DB fixtures are isolated. `M1_SYSTEM_CANDIDATE_READY` was held `false` while this correction was open and is `true` again now that it is closed.
- `M1_SYSTEM_CANDIDATE_READY = true` (Finding E was the only candidate-independent blocker). M1 stays `M1_BLOCKED` (no qualified candidate, M1.9 deployment, M1.10 sessions). Paper INACTIVE; Live NOT TOUCHED; no broker order.
- **M1.9 DEPLOYMENT PREREQUISITE:** migration 0091 (`sys_strategy_held_sizing_state`) must be verified on the real Paper DB before deployment; that DB was not touched.
- Residuals (not fixed): a multi-binding tick faults as a whole if a later binding fails after an earlier durable binding persisted; downstream position-cap clamps could reduce a quantity after durable Q was persisted; the env-reading `build_with_durable_state` is exercised only through `_from`; `scenario_runtime_promotion_evidence_binding_01` has 4/9 environmental failures from an options-lifecycle PENDING_EVIDENCE row left in the shared test DB (not caused by this work).

## -33. Batch 03 Discovery executed (2026-10-05, `V4-BATCH03-DISCOVERY-THEN-CAPITAL-FRACTION-DAEMON-CLOSURE-01` Phase A)

Local commits, **not pushed**. Record: `docs/research/M1_BATCH03_DISCOVERY_RESULT.md`.

- Status `BATCH03_DISCOVERY_EXECUTED`: 60 trials registered and attempted exactly once (60 attempts, 0 retries); 53 produced a result, 7 failed deterministically (`no native signals inside fold N`; F02 x4, F03 x3) and were not retried. One batch-wide judge: 53 included / 7 excluded, PBO 0.0714. Scanner review 60/60 `rejected`; 0/60 positive cost-aware benchmark alpha; 0 `paper_candidate`.
- Family ranking (predeclared keys, no invented threshold): F05, F04, F01 are `DISCOVERY_ADVANCED_TO_CONFIRMATION` (mechanical top three; all have non-positive alpha, median DSR 0.013-0.025, 0 robustness-clear slots); F02/F03 `DISCOVERY_INSUFFICIENT_EVALUABLE_TRIALS`; others `DISCOVERY_NOT_ADVANCED`. This is not Promotion; `selected_trial_ids`/`promotion_candidates` empty.
- Confirmation trials NOT registered/run; final holdout RESERVED / UNCONSUMED (guard clean); no Promotion candidate; Paper INACTIVE; Live NOT TOUCHED; M1 remains `M1_BLOCKED`.

## -32. Combined-stack final closeout correction (2026-10-04, `V4-M1-CORRECTION-PLUS-BATCH03-FINAL-CLOSEOUT-02`)

Local commits, **not pushed**. Head accounting (no history rewrite): full-stack base `fac225922d3b46bb842bea22f2c9676f7cfb5b58` (origin/main); combined-controller start `68d442536b92fe50f6d16458258ef46ebf8ad021`; closeout-correction start `7fdbebf6361e62e5b4119344a175fc557537d85d`; final head = the head of the branch at the time this file is read (`git rev-parse HEAD`).

- Recovery: F02/F03/F04/F06/F07/F09 are `DurableStateRequired`, seeded from durable `HeldSizingRecord` anchors (continuous run == restart at every boundary; mutation-proven). F01/F05/F08/F10 are `BoundedHistoryReconstructible`. The dynamic-selection daemon path still refuses `DurableStateRequired`; capital-fraction Paper dispatch is `AUTHORIZED_NEXT / NOT YET COMPLETE`.
- Daemon: `cargo check -p mqk-daemon --all-targets -j 2` passes with the 24-strategy universe; plan-builder lib tests pass.
- Family-ranking key 5 now reads engine-computed matched-benchmark drawdown evidence (benchmark minus candidate max drawdown, one verified run identity); absent/inconsistent stays worst on key 5 only.
- Batch 03 stress contract frozen by the operator (`half_exposure_capital_fraction_500bps_v1`, 500 bps, slippage 15, vol mult 10, ceiling 4000 bps) and bound into all 60 trial identities; baseline sizing unchanged. `execution_gate.executable = false`, blocker `OPERATOR_EXECUTION_AUTHORIZATION_REQUIRED_BATCH03`.
- Unchanged: 60 trial definitions, 0 registered, 0 attempts, no result, no family winner, no Promotion candidate; holdout RESERVED / UNCONSUMED; Paper INACTIVE; Live NOT TOUCHED; M1 `M1_BLOCKED`. Full local workspace acceptance NOT RUN (laptop resource-safety rule); broad proof delegated to GitHub CI after an authorized push.
- Supersedes the -31 wording "hard stop `OPERATOR_DECISION_REQUIRED_BATCH03_STRESS_CONTRACT`", "key 5 has no evidence source" and "`mqk-daemon` not compiled locally".

## -31. M1 System-Closure Correction + Batch 03 preparation (2026-10-04, `V4-M1-SYSTEM-CLOSURE-CORRECTION-PLUS-BATCH03-PREPARATION-01`)

Local commits on top of `fac22592`, **not pushed**. Records: `docs/M1_SYSTEM_CLOSURE_01.md` (Correction 01), `docs/research/M1_BATCH03_PREDECLARATION.md`; ledger G2.19.

- Corrected: canonical daily timeframe identity for future trials (`1D` == `1Day`, opt-in, historical ids unchanged); the full six-key P7A/P7B stress contract bound into the registered trial and compared field by field at Promotion; M1.10 soak sessions bound to the deployed identity.
- Prepared: Batch 03 (ten long/flat native ETF families F01-F10 x SPY, QQQ, IWM, SMH, XBI, XLE = 60 trials, 1000 bps capital-fraction sizing on USD 100,000, canonical daily identity, one batch-wide judge population), the engines with independent integer-reference parity, the 60/0 registration gate (59 and 61 refused), and a family ranking that advances at most the top 3 families to Confirmation (never trials, never Promotion). Confirmation universe DIA, MDY, XLF, XLI, XLV, XLP is prepared, not registered.
- **Final status `BATCH03_PREDECLARED_NOT_EXECUTED` + hard stop `OPERATOR_DECISION_REQUIRED_BATCH03_STRESS_CONTRACT`:** no repository authority exists for the Batch 03 stress contract. Operator must supply `scenario_id`, `allocation_fraction_bps` (<1000), `stress_execution_slippage_bps` and `stress_execution_volatility_mult_bps` (each >= baseline, one strictly worse) and `max_drawdown_ceiling_bps`. Batch 02's 500 bps values are not authority.
- Zero trials registered, zero economic attempts, no result, no family winner, no Promotion candidate. Holdout RESERVED / UNCONSUMED. Daemon capital-fraction Paper dispatch remains `AUTHORIZED_NEXT / NOT YET COMPLETE`; M1 `M1_BLOCKED`; Paper INACTIVE; Live NOT TOUCHED.
- Known gaps: family-ranking key 5 has no benchmark-drawdown evidence source (fail-closed to worst); F02/F03/F04/F06/F07/F09 are `DurableStateRequired` engines (seeded from the durable held-sizing record, proven restart-equivalent); the `mqk-daemon` crate was not compiled locally. Full local workspace acceptance NOT RUN (laptop resource-safety rule); broad proof delegated to GitHub CI after an authorized push.

## -30. M1 System Closure (2026-10-04, `V4-M1-SYSTEM-CLOSURE-CANDIDATE-READY-01`)

Local commits on top of `fac22592`, **not pushed**. Record: `docs/M1_SYSTEM_CLOSURE_01.md`; runbook `docs/runbooks/m1_10_finite_validation.md`; ledger G2.18. Supersedes the "local, not pushed" wording of -29 and -28: Batch 02 and its independent-review correction are **PUSHED-VERIFIED** at `fac225922d3b46bb842bea22f2c9676f7cfb5b58` (GitHub CI #623, run `37222460113`, SUCCESS, 6/6).

- Operator sequencing decision: future strategy discovery IS allowed, but SYSTEM COMPLETION comes first and a new campaign needs a normal formal predeclaration before any economic evaluation. No Batch 03, hypothesis, trial or economic attempt was created here.
- Fixed + proven: capital-fraction stress scenario bound to the registered Research trial (additive `signal_source.stress_contract`; no universal fraction); calendar authority (`calendar.rs` closures/early closes/coverage); daily-label granularity check; two test races; derived M1.10 ledger (`mqk_integrity::soak_ledger`).
- `OPERATOR_DECISION_REQUIRED_CAPITAL_FRACTION_PAPER_DISPATCH`: RESOLVED by the capital-fraction daemon closure (daemon dispatch wired; see above).
- Off-market state (read-only): Paper DB `mqk-paper-postgres` DISARMED, 0 promotion transitions, daemon not running, Paper DB migrations at 76 vs repo 0091, `MiniQuantDesk-Paper-Preopen-Startup` task Ready. M1.9 is `PREDEPLOYMENT_READY` for candidate-independent items only; M1.10 is not started (`READY_TO_START_AFTER_VALID_DEPLOYMENT`). Sunday counts for nothing.
- Unchanged: M1 `M1_BLOCKED`; Batch 01 and 02 `BATCH_REJECTED`; Promotion NONE; Paper INACTIVE; final holdout RESERVED / UNCONSUMED; Live DISABLED / NOT TOUCHED. Full local workspace acceptance NOT RUN (laptop resource-safety rule); broad proof delegated to GitHub CI.

## -29. Batch 02 Independent-Review Correction (2026-10-03, `V4-M1-NATIVE-HYPOTHESIS-BATCH-02-INDEPENDENT-REVIEW-CORRECTION-01`)

Local only, **not pushed**. Records: `docs/research/M1_BATCH02_PREDECLARATION_ERRATUM.md`, `docs/research/M1_BATCH02_CENSUS.md` (correction section), ledger G2.17. Nothing economic changed; `BATCH_REJECTED` stands.

- IR-B02-01: the immutable `PREDECLARED_BATCH_02.json` carried six copied Batch 01 descriptions (warm-ups, one-share quantity prose, Benchmark V2 capital basis, old run root, five-trial judge wording). Original bytes unchanged; a post-run erratum documents them; an invariance proof shows they never reached a trial id, sizing argument, benchmark policy, economic spec, attempt inventory or selection input. No attempt or result was rerun. The blanket whole-block predeclaration test was replaced by explicit frozen-policy paths and a copied-description detector.
- IR-B02-02: Promotion now binds a capital-fraction candidate's P7A/P7B `stress_sizing` provenance to its own authenticated sizing contract, including the actual baseline `max_target_qty`/`max_position_notional_usd` caps (policy-neutral; no stress fraction required or defaulted). **BLOCKED — NEW PROMOTION-AUTHORITY CONTRACT REQUIRED** for the exact required stress allocation: before any future capital-fraction candidate is production-promoted, that authority must be decided and bound. Not relevant to the rejected Batch 02 (no candidate reaches Promotion).
- M1 remains `M1_BLOCKED`. Batch 01 and Batch 02 `BATCH_REJECTED`; Promotion NONE; Paper INACTIVE; final holdout RESERVED / UNCONSUMED; Live NOT TOUCHED. Full local workspace acceptance NOT RUN (laptop resource-safety rule); broad proof delegated to GitHub CI after an authorized push.

## -28. Native Hypothesis Batch 02 Rejected (2026-10-03, `V4-M1-NATIVE-HYPOTHESIS-BATCH-02-01`)

Local only, **not pushed**. Supersedes the "Batch 02 NOT STARTED" wording of -27 and older entries. Records: `docs/research/M1_BATCH02_RESULT.md`, `docs/research/M1_BATCH02_CENSUS.md`, ledger G2.16.

- Batch 02 (H1 turn-of-month last-1/first-3, H2 Halloween Nov–Apr, H3 50-day range breakout / hold-10; 3 × 5 symbols = 15 trials, one experiment, one judge) ran once under a predeclaration committed (`368c1f47`) before any engine, calendar code, registration or result. All 15 trials were registered with 0 attempts before the first attempt; 15 attempts, 12 evaluable, 3 H1 failed attempts (engine `MaxDrawdownBreached` halt). Judge PBO 0.0913, best DSR 0.0159 (needs 0.5), **0 `paper_candidate`**, 0 eligible, selected trial NONE. Families all `FAMILY_REJECTED`; batch `BATCH_REJECTED`. No H4 or rescue campaign is authorized.
- New shared authority: `mqk-integrity::sessions` (`us_equity_regular_sessions_v1`, 2016-01-01..2026-12-31, 105 closures, content sha256 `3249ee51…76de`, bound into H1/H2 fingerprints); H1/H2 fail closed flat outside coverage (Paper cannot run them past 2026-12-31 until the calendar is extended under a new identity). Registry universe 14.
- Half-exposure robustness stress is a recomputed 500 bps capital-fraction quantity (evaluation scenario of the same trial), never a USD cap (the accepted exact-target replay refuses a binding cap; an accidental UI selection of a USD 25,000 cap was voided by the operator).
- Harness defect found and fixed during the run: the Rust robustness gauntlet/stress suite could never match a capital-fraction candidate's fingerprint (`a360d2c1`); downstream stages were re-run (pass 2); pass-1 robustness verdicts are VOID; no economic attempt was retried.
- M1 remains `M1_BLOCKED`. Batch 01 and Batch 02 `BATCH_REJECTED`; Promotion NONE; Active Paper strategy/deployment NONE / INACTIVE; final holdout RESERVED / UNCONSUMED (post-run guard clean); Live DISABLED / NOT TOUCHED. M1.9 and M1.10 OPEN; M1 is not complete.
- Next exact step: independent ChatGPT review of this controller's local commits (at most one correction) → push → exact-head CI; any further research campaign needs a new explicit operator decision.

## -27. Native Sizing V1 Pushed-Verified (2026-10-03, `V4-README-CURRENT-TRUTH-INTEGRATION-CORRECTION-01`)

Current truth; supersedes the "Not pushed" lines of -26 and -25 (and of older entries for commits at or below `92d337b6`). Record: ledger G2.15A.

- Native Sizing V1: **PUSHED-VERIFIED**. `92d337b67670b2858aa24c00b2768a2ca04cc212` is the Native Sizing V1 pushed-verified production-code baseline: the exact pushed head on which GitHub CI #621, run `37145565796`, ran — SUCCESS, 6/6 jobs green. Later docs-only commits change no production behavior and do not move this baseline.
- Batch 02 status wording: **NOT STARTED.** The operator-selected/planned hypotheses for its formal predeclaration are turn-of-month, Halloween / November–April, and 50-day range breakout / hold-10; the canonical Batch 02 predeclaration file has not yet been committed, and no Batch 02 hypothesis, trial or result exists. The 1000-bps sizing in -26 is a future campaign parameter only.
- Benchmark policy is explicit per candidate class: historical fixed-quantity corrected Batch 01 uses `capital_matched_exact_target_buy_hold_v1` (Benchmark V2); native capital-fraction candidates use `capital_fraction_matched_passive_buy_hold_v1`. Cross-substitution is forbidden; Benchmark V2 is not the benchmark authority for a capital-fraction campaign.
- M1 is the CURRENT TARGET and remains `M1_BLOCKED`. Batch 01 `BATCH_REJECTED`; Batch 02 NOT STARTED; Promotion NONE; Active Paper strategy/deployment NONE / INACTIVE pending a qualified candidate; final holdout RESERVED / UNCONSUMED; Live DISABLED / NOT READY / NOT TOUCHED.
- M1.9 OPEN. M1.10 OPEN (10 countable autonomous Paper sessions + 5 consecutive clean sessions). The historical genuine Paper trade and no-trade lifecycle evidence stays accepted/frozen. M1 is not complete.

## -26. Native Sizing V1 Independent-Review Correction (2026-10-03, `V4-M1-NATIVE-SIZING-V1-INDEPENDENT-REVIEW-CORRECTION-01`)

Not pushed when written (now PUSHED-VERIFIED, see -27). Records: `docs/research/NATIVE_SIZING_V1_CONTRACT.md`, `docs/research/NATIVE_SIZING_V1_CENSUS.md`, ledger G2.15. Supersedes the "locally complete / deferred" wording of the entry below.

- IR-SZ-01 Research economic bridge, IR-SZ-02 restart-recoverable runtime/deployment seam (+ runtime == canonical Backtest wrapper fingerprint) and IR-SZ-03 benchmark run-id identity are FIXED + PROVEN; daemon backtest job route and CLI routes without a bridge refuse a sizing selection.
- Operator decision recorded as a FUTURE campaign parameter only (not a code default, not executed): next M1 hypothesis campaign sizing = `fixed_initial_capital_fraction_v1`, `allocation_fraction_bps = 1000` (10% of immutable initial capital; USD 100,000 -> USD 10,000 entry budget). Batch 02: NOT STARTED, no hypotheses/trials/results.
- M1 remains `M1_BLOCKED`; Batch 01 `BATCH_REJECTED`; holdout RESERVED / UNCONSUMED; Promotion NONE; Paper NOT ACTIVATED; Live NOT TOUCHED. Full local workspace acceptance NOT RUN (resource-safety rule); delegated to GitHub CI after independent review and an authorized push.

## -25. Native Capital-Fraction Sizing V1 (2026-10-03, `V4-M1-NATIVE-SIZING-V1-01`) — superseded in part by -26

Not pushed when written (now PUSHED-VERIFIED, see -27). Records: `docs/research/NATIVE_SIZING_V1_CONTRACT.md`, `docs/research/NATIVE_SIZING_V1_CENSUS.md`.

- Added the explicit, versioned sizing policy `fixed_initial_capital_fraction_v1` (integer `allocation_fraction_bps` 1..=10000, immutable initial capital, floor whole shares at the causal completed-bar close, refuse instead of a one-share fallback, quantity held for the position lifetime, caps reduce/refuse only) with one shared pure resolver used by Backtest, scanner and the Paper contract. The historical fixed-quantity policy and every historical identity are unchanged.
- New benchmark `capital_fraction_matched_passive_buy_hold_v1` (Benchmark V2 untouched); scanner, review and Promotion fail closed on cross-substitution; `backtest csv`/`scan-strategies` and the batch declaration select the policy explicitly. Paper/runtime: contract only, refused at registry build (no restart-recoverable seam yet); NOT ACTIVATED.
- Capital-fraction production value: NOT SELECTED when this entry was written (superseded by -26: the operator froze 1000 bps for the next campaign only; still no code default). Batch 02: NOT STARTED. Batch 01 stays `BATCH_REJECTED`, M1 stays `M1_BLOCKED`; holdout RESERVED / UNCONSUMED; no Live. Full local workspace acceptance not run (laptop resource-safety rule); broad proof delegated to GitHub CI.

## -24. Benchmark V2 Promotion Economic Binding (2026-10-02, `V4-M1-BENCHMARK-V2-PROMOTION-BINDING-CORRECTION-01`)

Not pushed. Record: `docs/research/BENCHMARK_V2_PROMOTION_ECONOMIC_BINDING.md`.

- Independent review finding IR-BV2-01 confirmed and closed: Promotion now requires the Benchmark V2 review row's candidate `config_id`/`run_id` (and execution model, semantic fingerprint) to equal the canonical Backtest evidence's; the scanner ran with integrity off/120/0/AlwaysOn while canonical `backtest csv` ran on/259200/3/us-equity-regular (14/14 rows, `c043…` vs `a694…`). The V2 scan can now run under the canonical config via `scan-strategies` flags that mirror `backtest csv`.
- No candidate was falsely promoted; corrected Batch 01 stays `BATCH_REJECTED` (14 rejected, 1 blocked, 0 `paper_candidate`), not rerun. Old review rows cannot authorize corrected native promotion. No hypothesis/result/threshold changed; holdout RESERVED / UNCONSUMED; no Paper/Live.

## -23. Benchmark V2 Review Authority + Corrected Batch 01 Reevaluation (2026-10-02, `V4-M1-BENCHMARK-V2-REVIEW-AND-REEVALUATION-01`)

Not pushed. Records: `docs/research/BENCHMARK_V2_REVIEW_AUTHORITY.md`, `docs/research/M1_BATCH01_CORRECTED_RESULT.md`.

- Scanner/review alpha for exact-target native candidates is now Benchmark V2 (`capital_matched_exact_target_buy_hold_v1`) under an explicit `--benchmark-policy` on `scan-strategies`/`review-scan`; missing/malformed/mismatched evidence and scan/review policy mismatch fail closed; legacy artifacts and ids are unchanged; `min_alpha_pct` stays 0. Promotion refuses corrected native Research evidence unless the review row is Benchmark V2 bound to the same strategy, symbol, timeframe, fingerprint, capital and data identity.
- Corrected reevaluation of the same 3 hypotheses x 5 symbols (15) ran under a predeclaration committed before any result: 14 evaluable, 1 excluded (zero-variance returns), PBO 0.2024, effective independent trials 12.36, **0 `paper_candidate`** (alpha vs Benchmark V2 negative on every evaluable row). Verdict `BATCH_REJECTED`; the superseded v1 Batch 01 stays HISTORICAL / NOT_PROMOTION_AUTHORITY.
- M1 remains `M1_BLOCKED`. Promotion rows 0; Paper fleet unset; holdout RESERVED / UNCONSUMED; no Live; no Batch 02.

---

## -22. Independent-Review Correction 01 (2026-10-02, `V4-M1-NATIVE-RESEARCH-INDEPENDENT-REVIEW-CORRECTION-01`)

Not pushed. Record: `docs/research/M1_INDEPENDENT_REVIEW_CORRECTION_01.md`.

- Native bridge v1 evidence (campaigns 01-03, dual-SMA, pullback, Batch 01) is HISTORICAL / SUPERSEDED_PROTOCOL / NOT_PROMOTION_AUTHORITY: it re-sized a native +1-share absolute target into a ~$50k position. Corrected protocol `native_exact_target_qty_v1` + stream v2 execute the exact whole-share target on one capital basis; the Rust promotion verifier accepts only v2.
- `pullback_mean_reversion_20_2` is refused by `instantiate_verified` (restart-unsafe); signal-stream history provenance is truthful; the batch runner registers all trials before any emitter runs and the emitter runs inside the attempt.
- Review alpha vs a fully invested buy-and-hold remains an OPEN GOVERNANCE QUESTION. Promotion rows 0; Paper fleet unset; holdout untouched; no Batch 02.

---

## -21. Native Hypothesis Batch 01 Rejected (2026-10-01, `V4-M1-NATIVE-HYPOTHESIS-BATCH-01`)

Not pushed. Record: `docs/research/M1_BATCH01_RESULT.md`; census `docs/research/M1_BATCH01_CENSUS.md`.

- Three new stateless native engines (`absolute_momentum_252`, `near_high_momentum_252_3pct`, `trend_pullback_5d_4pct_hold5`; universe now eleven) x five fixed symbols = 15 trials predeclared in one experiment before any engine or result; all 15 registered before the first attempt; one batch-wide judge (hypothesis unset).
- Result: 15 attempted, 14 evaluable (one never traded), PBO 0.238, best DSR 0.214 (< 0.5), review 0/15 `paper_candidate`. Batch rejected; the three hypotheses are closed.
- M1 remains `M1_BLOCKED`; promotion rows 0; Paper fleet unset; holdout untouched; review, promotion, P9 and cost policy unchanged. Last new-hypothesis controller before independent review of `7dd31e5d..HEAD`.

---

## -20. pullback_mean_reversion_20_2 Campaign 01 Rejected (2026-10-01, `V4-M1-PULLBACK-MEAN-REVERSION-CAMPAIGN-01`)

Not pushed. Record: `docs/research/M1_PULLBACK_MEAN_REVERSION_CAMPAIGN_RESULT.md`.

- New stateful native engine `pullback_mean_reversion_20_2` (universe now eight), exact integer entry/exit boundaries, state owned by the instance with a deterministic first-call window replay; the emitter stream matched an independent exact-rational state machine on 100% of bars for all five symbols.
- Predeclared before implementation and results; ran once on the fixed five symbols: position agreement 1.0, positive gross on four of five, but 40-49 round trips cost 14-47% of equity, every net return is negative, no DSR reaches 0.5, PBO 0.528 (limit 0.5), review 0/5 `paper_candidate`. Rejected; family stopped.
- M1 remains `M1_BLOCKED`; promotion rows 0; Paper fleet unset; holdout untouched; review, promotion and P9 policy unchanged; `intraday_scalper`, `trend_sma50`, `dual_sma_50_200_trend` closed and unmodified.

---

## -19. dual_sma_50_200_trend Campaign 01 Rejected (2026-10-01, `V4-M1-DUAL-SMA-50-200-CAMPAIGN-01`)

Not pushed. Record: `docs/research/M1_DUAL_SMA_CAMPAIGN_RESULT.md`.

- New native engine `dual_sma_50_200_trend` (universe now seven); the backtest history window now honors a strategy's `required_history_bars()` and the Paper context load limit is 256, both of which a 200-bar rule would otherwise have silently starved.
- Predeclared before implementation and results; ran once on the fixed five symbols: position agreement 1.0, four of five positive net, 9-17 transitions per symbol, but PBO 0.548 (limit 0.5), only GLD and SPY clear DSR 0.5, every symbol fails regime concentration, and the review gives 0/5 `paper_candidate` (alpha against buy-and-hold). Rejected; family stopped.
- M1 remains `M1_BLOCKED`; promotion rows 0; Paper fleet unset; holdout untouched; `intraday_scalper` and `trend_sma50` unchanged. Next decision: another hypothesis, or a review of whether the frozen alpha-against-buy-and-hold review gate is the right bar for a long/flat trend-timing rule (a policy decision for the operator, not changed here).

---

## -18. Campaign 03 Rejected; trend_sma50 Family Closed (2026-10-01, `V4-M1-TREND-SMA50-CAMPAIGN-03-FINAL-01`)

Not pushed. Record: `docs/research/M1_NATIVE_TREND_CAMPAIGN_RESULT.md`. Supersedes §-17's open decision.

- Campaign 03 (half-equity notional, new identities) ran once: position agreement 1.0 on all five symbols, judge `evaluated` (PBO 0.179), but every DSR is below the frozen 0.5 minimum, every symbol fails a required robustness scenario, and the scanner review gives 0/5 `paper_candidate`. Hard stop: no qualifying candidate.
- The `trend_sma50` family is closed (no campaign 04, no further notional percentage). M1 remains `M1_BLOCKED`; promotion rows 0; Paper fleet unset; holdout untouched; `intraday_scalper` rejection intact.
- Next decision: a different hypothesis. Costs and about 160 signal flips over the window dominate the daily SMA-cross rule under the conservative bar-range execution model; a slower or lower-turnover rule is the open design question for the operator.
- Promotion-policy thresholds: frozen before any result in `PREDECLARED_CAMPAIGN*.json`; the deployed daemon config still has them unset (none were written because no candidate reached promotion).

---

## -17. M1 Native Research Bridge Built; Campaign Produced No Candidate (2026-10-01, `V4-M1-NATIVE-RESEARCH-PROMOTION-BRIDGE-01`)

Not pushed. Full record: `docs/research/M1_NATIVE_TREND_CAMPAIGN_RESULT.md`, census: `docs/research/M1_NATIVE_RESEARCH_BRIDGE_CENSUS.md`. Supersedes §-16's "structurally blocked" claim: the seam is now built.

- Built: native `trend_sma50` engine (universe six), `mqk backtest native-signals`, the Research registration bridge, and native semantic-fingerprint binding in the promotion gate.
- Campaigns 01 (voided, simulated positions did not follow the strategy) and 02 (final; four attempts failed the fixed fidelity gate, one succeeded, judge `partially_evaluable`) produced no promotable candidate. Holdout untouched.
- M1 remains `M1_BLOCKED`. Paper fleet unset, promotion rows 0, `intraday_scalper` rejection intact.
- Operator decision needed: authorize a third specification of this family (half-equity notional cap, exposure-scale invariant), or choose a different hypothesis. Promotion policy thresholds are still unset in the deployed daemon config.

---

## -16. M1 Continuation: Deployment Removed, Promotion Path Structurally Blocked (2026-10-01, `V4-M1-PROMOTION-AUTHORITY-AND-PAPER-CLOSURE-01-CONTINUE-01`)

Supersedes the "operator decision needed" bullets of §-15 where they differ. Not pushed.

- **Decision 1 applied.** The only deployed-fleet authority is `MQK_STRATEGY_IDS` in the untracked, gitignored `.env.local`; process/user/machine env, the preopen task action, and `sys_dynamic_selection_plan*` (0 rows) carry none. `MQK_STRATEGY_IDS=intraday_scalper` was commented out (one line, in place). With it unset the native bootstrap is Dormant and start is refused (`routes/system.rs`), so `intraday_scalper`/AAPL/300 cannot run in Paper. The review rejection (`negative_total_return`) and the 41 registry rows are untouched. Re-set `MQK_STRATEGY_IDS` only to an identity holding an `active_paper` promotion.
- **Decision 2 not executable on the existing machinery (hard stop: contracts conflict).** The promotion transition requires three bound evidence chains: a `paper_candidate` scanner review, a Research OOS trial whose registered `strategy_id` equals the promoted `strategy_id` (`research_evidence_gate.rs`), and a Rust `BacktestReport` whose `strategy_name` and semantic fingerprint equal the server-resolved fingerprint of a native plugin-registry engine (`backtest_evidence_gate.rs`, `strategy_config_identity.rs`). Python Research candidates (pooled classifier-rank families) have no native engine, and a research-replay strategy cannot satisfy the native-fingerprint check, so no Research-registered candidate can become a deployable identity. Separately, the 88-symbol universe is `CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME` and the frozen Wave06 policy caps such studies below `PROMOTION_READY`; no point-in-time universe exists. The only supported native/timeframe pairs (`swing_momentum` 1D, `intraday_scalper` 5m) are already rejected; `mean_reversion`/`volatility_breakout` need 1H data, which is not authorized.
- **Operator decision needed:** authorize new engineering for a native engine plus a Research evidence path that registers under that same engine id (this is new framework work, not a campaign), or accept M1 as blocked with no deployed Paper identity.

---

## -15. M1 Promotion-Authority Closure Attempt (2026-10-01, `V4-M1-PROMOTION-AUTHORITY-AND-PAPER-CLOSURE-01`)

Outcome: **M1_BLOCKED, operator decision required.** Not pushed.

- Candidate census over canonical repo evidence found no protocol-eligible US equity/ETF candidate: `intraday_scalper`/AAPL/5m and the `swing_momentum` 1D scan are rejected review artifacts; DISCOVERY-01, SHORT-01, SHORT-02/03/04, Wave06 LIQ-01/VOL-01 are `REJECTED_NOT_ADVANCED`; ALPHA-01 run_02 is inconclusive. Further alpha discovery remains `OPERATOR-DEFERRED`. Native engines `mean_reversion` and `volatility_breakout` have no evaluation record; evaluating them is new discovery, not continuation of an eligible candidate.
- Actual Paper (read-only): `sys_strategy_promotion_transitions` has 0 rows; arm state `DISARMED` (`InboundContinuityUnproven`), last run HALTED, daemon not running; one deployed identity (`intraday_scalper`/AAPL/300) has no `active_paper` promotion. No Paper/Live mutation, no market-hours run.
- Repaired one stale ignored test (`b1c_c14`) so it exercises the real Gate 3b config-identity and registry seams, with a drifted-fingerprint negative control.
- Operator decision needed: authorize a new/continued candidate search (or a named existing engine evaluation), or remove/replace the deployed identity. Clearing the HALTED/DISARMED state also needs operator action.

---

## -14. M5-M8 Frozen Acceptance Correction (2026-09-29, `V4-M5-M8-FROZEN-CLOSURE-03`)

Full record: `MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md` §G9 and the final section of `docs/V4_CODE_COMPLETION_MANIFEST.md`. Starting HEAD `21718bcb`; five local code commits (`f54c0bdf`..`e81f896b`) plus a clippy follow-up and the docs commit; NOT pushed. Supersedes §-13 where they differ.

- Fee and lifecycle consumers are isolated by exact broker account (two accounts of one deployment mode never share fees or lifecycle rows).
- Lifecycle economics fail closed before mutation (exact option quantity, checked arithmetic, all-or-nothing).
- The lifecycle restart fence derives from durable state; the lifecycle fetcher's absence cannot clear it.
- A lifecycle event clears only after option + underlying + cash agreement.
- An external signal with omitted `asset_class` requires positive registry proof of Equity.
- Deterministic approved M5-M8 implementation is locally complete for these five frozen defects only. Not performed: `cargo test --workspace`, CI, any Paper/Live session, real broker call, push. M6/M7/M8 operational exit gates NOT complete. ML PLANNED / NOT AUTHORIZED.
- Open, unrelated: `b1c_c14` (stale promotion fixture). `scenario_multi_symbol_dispatch_summary_01::s12` fails on CRLF Windows working copies (environmental). Use a fresh disposable DB for daemon lifecycle tests.
- Next: independent review of `21718bcb`..HEAD, then an explicit operator push decision.

---

## -13. M5-M8 Final Exception Correction (2026-09-28, `V4-M5-M8-FINAL-INDEPENDENT-REVIEW-CORRECTION-02`)

Full record: `MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md` §G8 and the final section of `docs/V4_CODE_COMPLETION_MANIFEST.md`. Starting HEAD `26bd9c12`; ten local code commits (`9eafefd8`..`02b9a638`) plus the docs commit; NOT pushed. Supersedes §-12 and §-11 where they differ.

- Closes B5 (Crypto TIF authority upstream of `decision.rs`), B6 (economic account identity = Alpaca provider account id, legacy rows quarantined, migration 0088), D1 (dedicated lifecycle model, evidence correlation, default-off polling caller, 0089), D2 (atomic signed lifecycle economics in the canonical ledger, 0090), D3 (state machine; gate clears only at RECONCILED after broker agreement), D5 (verified vertical is exactly what is sent; response authenticated) and the fee economic-consumer gap.
- M6/M7/M8 operational exit gates are NOT complete. Not performed: `cargo test --workspace`, CI, any Paper/Live session, real broker/IBKR call, push. Crypto and mleg capability flags default off. ML is PLANNED / NOT AUTHORIZED.
- Open, unrelated: `b1c_c14` (ignored DB test) has a stale promotion fixture. Use a fresh disposable DB for daemon lifecycle-matrix tests; the long-lived `mqk_test` holds residual `paper`-mode journal rows.
- Next: independent review of `26bd9c12`..HEAD, then an explicit operator push decision.

---

## -12. M5-M8 Independent-Review Correction Controller (2026-09-28, `V4-M5-M8-INDEPENDENT-REVIEW-CORRECTION-01`)

Full record: `MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md` §G7 and the section of the same name at the end of `docs/V4_CODE_COMPLETION_MANIFEST.md`. Starting HEAD `b50dc0ba`; ending HEAD `6cdd0d9b`; seven local commits, NOT pushed. This is the independent review of `48bed899`..`e5fea9d5` (§-11 below) that §-11 itself called for — not a new expansion, a correction of six confirmed defects in that exact range.

- **B5** — `crypto_execution_policy`'s configured `gtc`/`ioc` now actually reaches the real order decision (`resolve_admitted_time_in_force`); it had zero production caller before, so every crypto order was refused via the generic translator's `day` regardless of config.
- **B6** — fee-ledger/cursor scope widened to durable `broker_account_id` (migration 0085); the prior `(engine_id, mode, activity_type)`-only scope let two broker accounts collide on Alpaca's own activity id.
- **D1** — real Alpaca options-lifecycle fetch/normalize/ingest caller built; migration 0086 fixes the confirmed PK collision (Alpaca reports an `OPEXC`/`OPASN` activity and its paired `OPTRD` under the IDENTICAL activity id — the old PK could hold only one of the pair) and stops conflating `OPTRD`'s underlying-ticker symbol with the option contract.
- **D2** — `apply_option_lifecycle_activity` now derives signed shares/cash directly from the paired `OPTRD`'s own signed evidence (all four call/put exercise/assignment directions correct, mutation-proven) instead of unsigned magnitudes that ignored `is_call`; added account/domain scope verification and checked arithmetic (migration 0087).
- **D3** — `option_lifecycle_pending_gate` wired into real production authority: order admission (`submit_internal_strategy_decision`), broker-snapshot-overwrite (`repair_adopt_broker_position_baseline`), and — found by this same wiring's own second sweep — the manual operator order-submit route, which bypassed it entirely until closed this session. It had zero production callers before.
- **D5** — `submit_vertical_spread` now accepts only a `VerifiedVerticalSpreadSubmission`, constructible solely by re-deriving the frozen call/put vertical structure via `option_strategy_permission::classify_option_strategy_structure`; before, two arbitrary option symbols reached HTTP with nothing structurally proving a vertical. Also corrects §-11's D5 claim that Alpaca does not document mleg cancel/replace — current docs do (mleg-specific `PATCH` error codes, a full FIX cancel/replace contract); refusal remains, now honestly stated as a conservative MQD policy pending REST success-path proof, not an absence of documentation.
- Independently re-verified this session (not re-asserted): the option asset class still has no order-construction path anywhere in this codebase (`asset_risk_policy::option_policy()` still `Disabled`; `resolve_order_instrument_context` still refuses every non-equity/crypto class) — D3's new admission gate creates no new live risk surface. D5's new type has zero consumers outside `mqk-broker-alpaca`. Migrations 0083/0084 untouched; 0085/0086/0087 additive only.
- Unchanged hard stops: real Alpaca options/crypto Paper or Live session, real IBKR Gateway session, controlled Live proof, 24/7 autonomous scheduling, fractional research/backtest. Crypto/mleg capability flags remain default-off everywhere (grep-verified). M6/M7/M8 milestone exit gates are NOT operationally complete.
- Not run: full `cargo test --workspace` (resource bounded), any Paper/Live/provider session, GitHub CI. Next: independent review of `b50dc0ba`..`6cdd0d9b` only (not another full-stack review), then an explicit operator decision on pushing.

---

## -11. M5-M8 Operator-Approved-Decisions Implementation Controller (2026-09-27 -> 2026-09-28, `V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01`)

Full record: `MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md` §G6 and the section of the same name at the end of `docs/V4_CODE_COMPLETION_MANIFEST.md`. Starting HEAD `ead40bf1`; ending HEAD `e5fea9d5`; thirty-nine local commits, NOT pushed. Interrupted mid-mission by an unplanned machine restart before D5; resumed and closed in continuation. Supersedes §-10 where they differ.

- Implements the operator's approved M6/M7/M8 design decisions on top of G3/G4's QtyMicros closure: per-execution-domain identity/session/TIF/fee-ledger for Crypto (Wave B), provider-neutral futures/FX identity + a mockable-transport IBKR adapter foundation (Wave C), and the full options-lifecycle chain — durable ingestion, idempotent apply, fail-closed pending-reconciliation gate, and an atomic default-off Alpaca multi-leg (`mleg`) vertical-spread submit/cancel/replace (Wave D, D1-D5).
- D5 (this continuation): `AlpacaBrokerAdapter::submit_vertical_spread` always carries both legs of a defined-risk vertical spread in one `order_class=mleg` POST, never decomposed into independent per-leg orders; gated by a new default-off `options_mleg_capability_enabled` flag; cancel/replace always refuse (Alpaca's public reference does not document mleg-specific semantics). Mutation-proven (dropping a leg turns the relevant tests RED). **[CORRECTED by §-12: "does not document mleg-specific semantics" was wrong — Alpaca's current docs do; refusal is a conservative policy pending REST success-path proof. §-12 also closed the structural gap this bullet didn't catch: nothing here yet proved the two legs formed a frozen vertical before HTTP.]**
- Second adversarial sweep across the full local M5-M8 stack found no ordinary deterministic defect: Crypto/mleg capability flags remain default-off everywhere outside test code; `asset_risk_policy` remains model-only (zero routing callers); B6's fee-activity ingestion has a real production caller; D2/D3 idempotency and restart-safety re-proven against a disposable DB. **[CORRECTED by §-12: an independent review the following day found six confirmed defects this sweep missed — see §-12.]**
- Unchanged hard stops: real Alpaca options/crypto Paper or Live session, real IBKR Gateway session, controlled Live proof, 24/7 autonomous scheduling, fractional research/backtest. M6/M7/M8 milestone exit gates are NOT operationally complete.
- Not run: full `cargo test --workspace` (resource bounded), any Paper/Live/provider session, GitHub CI. Next: independent review of `48bed899`..`e5fea9d5`, then at most one consolidated surgical correction controller before any push.

---

## -10. M5-M8 Consolidated Surgical Correction Controller (2026-09-27, `V4-M5-M8-CONSOLIDATED-SURGICAL-CORRECTION-01`)

Full record: `MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md` §G4 and the section of the same name at the end of `docs/V4_CODE_COMPLETION_MANIFEST.md`. Starting HEAD `ead40bf1`; five local commits, NOT pushed. Supersedes §-9 where they differ.

- Closed (local code + proof): exact `qty_micros_v1` V2 reads for live-weights, paper-journal, strategy-performance, broker-snapshot positions/orders/fills and the OMS order list; broker-snapshot routes no longer serve a fractional/garbage quantity as `0`; paper-status no longer serves a fractional position as `null`; halted-run REST recovery accepts a fractional fill only on durable crypto-order evidence (explicit `asset_class=crypto`, per the IR-1 correction below).
- Unchanged hard stops: IBKR (M7), option lifecycle and multi-leg (M8), 24/7 autonomous scheduling, fractional research/backtest. Alpaca crypto capability still off (operator decision). Fill-quality telemetry stays whole-unit best-effort (G3-05).
- **Independent review correction (2026-09-26, three further local commits `a27852e5`, `1f1c9961`, `2e9e8c5c`, not pushed):** fractional REST recovery requires explicit durable `asset_class="crypto"` (a slash-shaped symbol is not authority); **crypto time-in-force is EXPLICIT-ONLY (operator decision)** - `gtc`/`ioc` admitted, `day` and the rest refused, no rewrite, so the Crypto strategy path fails closed until an authorized surface emits `gtc`/`ioc`; a malformed outbox side can no longer explain reconcile drift. Crypto capability still off.
- Not run: full `cargo test --workspace` (resource bounded), any Paper/Live/provider session, GitHub CI. Nothing is operationally validated.

---

## -9. M5-M8 Deterministic Code-Completion Controller (2026-09-26, `V4-M5-M8-DETERMINISTIC-CODE-COMPLETION-01`)

Full record: `MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md` §G3 and the section of the same name at the end of `docs/V4_CODE_COMPLETION_MANIFEST.md`. Baseline `8e029b86` (main = origin/main); nine local commits, NOT pushed.

Current truth for M5-M8, superseding the §-8 census where they differ (that census predates the QtyMicros runtime cutover):

- Fractional quantity now survives reconcile drift explanation, paper accounting, the durable Paper snapshot (migration 0078), broker P&L, and fails closed (409, no panic) on four V1 read/repair seams. Alpaca crypto wire correctness (position symbol, TIF, REST fractional fills) are fixed; admission TIF was subsequently CORRECTED to explicit-only `gtc`/`ioc` (see §-10). Contract-identity validators (expiry, whole contracts, pair legs) are fixed in registry-v2, the intent/spec models and the options permission classifier.
- Alpaca still does NOT advertise crypto capability (operator decision); M7 (IBKR) and M8 (options execution/lifecycle) are BLOCKED_HARD_STOP for design/dependency reasons; per-instrument session handling in the autonomous controller and the fractional backtest domain are BLOCKED_HARD_STOP (design / frozen contract).
- Not run: full `cargo test --workspace` (resource bounded), any Paper/Live/provider session, GitHub CI.

---

## -8. V4 Bulk Code Completion Wave B — M5-M8 Multi-Asset: Frozen Matrix + Bounded Census + First Code (2026-09-19, `V4-BULK-CODE-COMPLETION-STAGE-B-M2-01`)

### Frozen V4 asset matrix (operator-approved, recorded here as durable truth)

CRYPTO: canonical integration market BTC/USD; market data Kraken; execution
broker Alpaca; 24/7 spot semantics; fractional quantity required.
FUTURES: canonical integration contract MES; execution broker IBKR;
executable-contract identity must stay distinct from research continuous-
series identity. FX: canonical integration pair EUR/USD; execution broker
IBKR; base/quote, pip/tick, leverage/margin, financing/rollover semantics
required. OPTIONS: canonical underlying SPY; execution broker Alpaca.

Frozen options permission set — ALLOWED: long call, long put, covered call,
cash-secured put, defined-risk call vertical spread, defined-risk put
vertical spread. NOT ALLOWED: naked short calls, naked short puts,
unlimited-risk structures, arbitrary complex/multi-leg strategies outside
defined-risk verticals.

### Bounded M5-M8 census (evidence-grounded, not exhaustive)

This repository already has a deliberate, multi-month-old architectural
pattern for exactly this problem: build asset-neutral **model-only, zero-
production-caller** contracts first (`ASSET-CORE-01` through `04`), prove
them with focused tests, and defer the live production cutover until a
concrete consumer requires it (`instrument_registry_v2.rs` module docs:
"a model + loader seam, not a production cutover... nothing in this module
is wired into any consumer"). This census inventories that existing layer
against the frozen M6-M8 assets rather than assuming it doesn't exist.

**Already real (model layer, zero production callers unless noted):**
- `mqk_schemas::{AssetClass, ContractSpec, Instrument, QtyMicros, OrderSpec}`
  (`core-rs/crates/mqk-schemas/src/lib.rs:119-230`) — `QtyMicros` is an
  explicit fixed-point fractional-quantity type ("future assets (crypto...)
  require fractional quantities"); `ContractSpec` already models
  `Option`/`Future`/`Crypto`. `AssetClass` here is canonical: it is the type
  actually checked by the live broker-submit gate.
- `mqk_md::instrument_registry_v2` (2,661 lines) — additive instrument
  schema modeling equity/option/future/crypto/forex/rate identity,
  contract shape, and (`InstrumentEconomicsMetadataV2`) multiplier/margin
  metadata. No production JSON file exists for this schema yet; nothing
  reads it in any daemon/CLI/ingest/backtest/GUI path.
- `mqk_execution::types::{OrderIntentV2, IntentV2Contract, BracketLegs}` —
  asset-neutral order intent with fractional `QtyMicros` qty, per-asset
  contract validation (currency pair, future multiplier/tick, option
  strike/multiplier), and a TP/SL bracket model. Explicitly documented as
  not wired into `BrokerGateway`/OMS/broker adapters (`lib.rs`: "RESEARCH-
  NON-EQ-01... NOT wired into canonical MAIN execution path").
- `mqk_execution::asset_risk_policy` (ASSET-CORE-03) — static per-asset
  policy table with two hard global kill switches,
  `ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED = false` and
  `ASSET_RISK_NON_EQUITY_ROUTING_ENABLED = false`. Its own `crypto_policy`/
  `future_policy`/`option_policy`/`forex_policy` functions self-document
  the exact remaining gap per asset class (quoted verbatim below).
- `mqk_portfolio::{instrument_economics, portfolio_economics}`
  (ASSET-CORE-04A-F) — multiplier/currency-aware single-position valuation,
  integer-checked (`i128`), zero production callers; explicitly notes the
  still-missing bridge from `instrument_registry_v2`'s raw `multiplier: i64`
  to this crate's micros-scaled `contract_multiplier_micros` (`ASSET-CORE-
  04B... deferred`).
- `mqk_md::providers::kraken` (1,708 lines) + its `provider_registry`
  factory entry — a real, non-stub Kraken market-data provider.
- **New this session** (commit `2b9387e4`):
  `mqk_execution::option_strategy_permission` — the M8 frozen options-
  permission-set structural classifier (long call/put, covered call,
  cash-secured put, defined-risk call/put vertical spread; every other
  shape, especially a naked short, refused by name). 17 focused tests.
  Same model-only precedent as the rest of this layer.

**Confirmed CODE_MISSING / WIRING_MISSING (concrete, cited):**

- **M5 — fractional quantity does not reach the live execution boundary.**
  `mqk_execution::order_router::BrokerSubmitRequest.quantity: i64` and
  `mqk_execution::types::{TargetPosition,OrderIntent,ExecutionIntent}.qty:
  i64` are the types the real orchestrator/OMS/broker-adapter path actually
  uses; none of them is `QtyMicros`. `QtyMicros` exists only in the V2
  model scaffold. Threading fractional quantity through the live execution
  boundary is real, deterministically scoped work, but touches an already-
  audited, safety-critical path (OMS state machine, outbox/inbox,
  portfolio accounting, every existing equity call site) with no dedicated
  verification budget available this session — not attempted here; recorded
  as the single largest concrete M5/M6 blocker.
- **M5 — `MULTI-ASSET-ROUTING-GUARD-01` remains a hard equity-only gate.**
  `mqk_execution::gateway::BrokerGateway::submit_with_context`
  (`gateway.rs:388`) refuses every `AssetClass != Equity` before any broker
  adapter is invoked. Correct and intentional today; loosening it to a
  real per-`(asset_class, broker)` capability check is required before any
  of M6-M8 can submit a live/paper order, and was deliberately not
  attempted this session for the same reason as above (safety-critical,
  no dedicated verification budget).
- **M6 — crypto cannot flow through the production data-freshness
  controller.** `mqk-daemon/src/state/required_market_data_autofresh.rs`
  (the daemon's real, scheduled required-universe controller) fail-closed
  rejects any `instrument.asset_class != "equity"` (line 316) and any
  provider not declaring `supports_asset_class("equity")` (line 364); its
  top-level trading-day gate (`schedule.is_trading_day`, line ~1075) is a
  single NYSE-calendar check with no per-asset 24/7 override. Kraken's own
  scheduler has a read-only status route (`CRYPTO-DATA-03C`, per repo
  memory) but no task registration — Kraken ingestion is CLI-invoked only,
  never automatic. This exactly matches the mission's own description of
  the M6 data gap.
- **M7 — no IBKR integration exists at any level.** Verified directly:
  `grep -i ibkr` across the repo's non-test source returns zero real
  references; the only "interactive-brokers" string anywhere is a
  deliberate negative-control test fixture
  (`mqk-daemon/src/state.rs::unknown_broker_adapter_string_is_fail_closed`)
  proving an *unrecognized* adapter string fails closed — there is no
  `BrokerKind::InteractiveBrokers` variant, no adapter crate, no account/
  order/position/fill interface. `asset_risk_policy::future_policy()`/
  `forex_policy()` already name the remaining gap precisely: futures need
  "margin model, contract multiplier, expiry handling, and futures session
  calendar"; FX needs "pair registry, pip/lot sizing, leverage, currency
  conversion, and 24x5 session model." No roll/expiry logic exists.
- **M8 — no options chain/lifecycle/Alpaca-options capability.**
  `option_policy()` already self-documents: "options require chain
  metadata, contract multiplier, Greeks, assignment, and margin risk model
  before routing." `mqk-broker-alpaca` has no options-specific code path
  (grep for `option` in that crate returns only unrelated `Option<T>`
  Rust-syntax matches and one pricing-tick-size doc comment). Contract
  discovery, liquidity/spread data, expiry/exercise/assignment lifecycle,
  and Alpaca options order/position/fill handling are all absent.

**Why a full live production cutover was not attempted this session:**
every one of the four gaps above requires modifying either (a) the exact
safety-critical, already-audited execution/OMS/risk choke point this
repository's own `CLAUDE.md` and `execution_rules.md` single out for
extreme care (`BrokerGateway`, `OMS` state machine, outbox/inbox, the
`i64` quantity type threaded through dozens of already-tested call sites),
or (b) a brand-new broker wire-protocol integration (IBKR) with zero
existing scaffolding, real account/session/order lifecycle semantics, and
no committed design doc. Rushing either without a dedicated verification
budget is exactly the failure mode this repository's own prior sessions
have repeatedly identified and declined to rush (see the R2B/R2C
completed-bar-driver deferral and the M5 registry v1->v2 cutover
deferral, both above in this file). This session instead: (1) froze the
asset matrix as durable repo truth, (2) performed this grounded citation-
backed census, and (3) implemented the one concrete, safely-scoped,
zero-blast-radius M8 gap the census identified (the options-permission
classifier). **M5-M8 CODE_MISSING/WIRING_MISSING are NOT zero** — the wave
exit gate is not met. No Paper/Live/runtime state modified; no broker
network call made; no push.

---

## -7. V4 Bulk Code Completion Wave A — M2 C4/C5/C6 Closure + M3 Bounded Census (2026-09-19, `V4-BULK-CODE-COMPLETION-STAGE-B-M2-01`)

Continuation of §-6a's C1-C3 baseline. Scope: close the remaining M2 gap
(C4/C5/C6, the R2B/R2C multi-binding completed-bar driver work §-5 explicitly
deferred), then begin M3/M4/M5 CODE_MISSING/WIRING_MISSING implementation
under an explicit CODE-COMPLETION-FIRST mandate (exhaustive
regression/acceptance/operational proof deferred to a later verification
wave). Commit `f93b3660`.

**IR-3 — CORRECTED (2026-09-19, same-day follow-up).** The disposition
recorded immediately below ("ALREADY SAFE + PROVEN") and §-6a's original
record of it were both **incomplete, not merely stale**: they relied solely
on the read-side validator rejecting an already-wrapped negative
`scanner_rank` — real, but not a substitute for refusing the write itself.
The write path (`build_new_explicit_multi_strategy_authority`) still
performed an unchecked `r as i32` and would deliberately persist a
bit-reinterpreted value for any `scanner_rank` above `i32::MAX`. Fixed
(commit `9515b6f5`): the builder now returns
`Result<_, ScannerRankOverflow>` via `i32::try_from`; the one production
caller (`state/lifecycle.rs`'s explicit-v3 start path) propagates the
refusal through its existing `RuntimeLifecycleError::forbidden(...)`
fail-closed construction path — the durable authority is never built, and
the DB insert is never reached, for an overflowing value. Read-side
rejection is unchanged and still applies as a second, independent backstop.
Four focused tests (normal value, `i32::MAX`, `i32::MAX+1`, `u32::MAX`).
**IR-3 is CODE_CLOSED with a checked write, not merely a read-side catch.**

**C4 (multi-binding resolution) — CORRECTED (2026-09-19, same-day
follow-up).** The claim below that "genuine multi-*strategy* concurrent
dispatch through this specific autonomous path... remains achievable only
through the separate `DynamicSelectionHostPool` the interactive execution
loop already uses" was wrong against the authoritative M2 contract, which
requires same-symbol competing strategies (e.g. `AAPL/strategy_A` +
`AAPL/strategy_B`) to progress independently through production code, not
merely through one specific runtime path. Fixed (commit `e37d10c6`):
`resolve_effective_bindings` now checks a configured assignment against
both the legacy engine *and* the current run's real host-pool selection
(`AppState::dynamic_selection_runtime_snapshot`) before ever rejecting it.
A binding matching the host pool's own selected pairs resolves
`BindingDispatchRoute::HostPool` and dispatches through the existing
`pending_strategy_bar_input` mailbox hand-off (never reaching into the
execution loop's exclusively-owned host-pool object), confirmed via the
durable `strategy_signal_evaluations` row the loop's own dispatch writes.
Migration 0075 adds a strategy_id-scoped claim table
(`sys_autonomous_daily_binding_bar_dispatches`) since two strategies can
share one `(symbol, timeframe)` and migration 0050's identity cannot
distinguish them. The Tier-A single-engine policy remains frozen and
unchanged for the *legacy* bootstrap itself — it was never actually a
ceiling on what this driver may resolve or dispatch, and is no longer
treated as one. Proof:
`same_symbol_multi_strategy_resolves_via_host_pool_never_narrowed_or_refused`
resolves all three of AAPL/strategy_A, AAPL/strategy_B, MSFT/strategy_C via
the host pool, none narrowed or refused. **C4 now supports genuine
same-symbol multi-strategy production dispatch, not only multi-symbol
single-shared-strategy.**

**C5 (fault isolation).** `tick_autonomous_completed_bar_driver_multi`
classifies every binding's tick outcome via the frozen hybrid isolation
policy (migration 0071) into healthy/local-fault/global-critical
(`classify_binding_outcome_for_isolation`, exhaustive match, no wildcard),
writing `sys_autonomous_daily_binding_state` per binding. Migration 0074
adds one new closed reason (`binding_strategy_engine_not_active`) that
0071's original five did not honestly cover.

**C6 (wiring).** `autonomous_completed_bar_task.rs` routes a >1-symbol
assignment config through the new multi-binding entrypoint (each binding
resolving its own `provider_id` from the instrument registry); an
exactly-one-symbol config keeps the original single-binding call
byte-for-byte unchanged.

**Proof:** `scenario_autonomous_completed_bar_driver_01` (58/58, including
the same-symbol multi-strategy proof) and `scenario_autonomous_completed_bar_task_01`
(47/47, 2 DB-only ignored) pass; `cargo check -p mqk-db -p mqk-daemon --lib
--tests` clean; `check_migration_governance.sh` all 3 checks pass. DB-backed
`sys_autonomous_daily_binding_state`/`sys_autonomous_daily_binding_bar_dispatches`
scenario suites, an end-to-end dispatch-through-a-real-host-pool integration
test, and full daemon/db regression remain deferred to the verification wave
per this mission's explicit mandate — this wave's proof is at the resolution
layer (routing is correct, never narrowed/refused) plus reuse of
already-tested claim/completion primitives, not a live host-pool dispatch
run.

**M2 status: CODE_MISSING = 0, WIRING_MISSING = 0**, against the M2.1-M2.9
requirement list (§-5/§-6a's census) *and* including same-symbol
multi-strategy production dispatch (commits `f93b3660`, `9515b6f5`,
`e37d10c6`). M2 operational acceptance is still not claimed.

**M3 bounded census (Concurrent Paper + Live Execution Domains) — audit
before build, per this mission's own instruction.** Read
`mode_transition.rs` (canonical, restart-only mode-transition state machine;
hot switching is architecturally unsupported — every `DeploymentMode` is
fixed for a daemon process's whole lifetime), `state/env.rs`
(`deployment_mode_readiness`, per-mode/per-broker readiness, including an
explicit `(LiveCapital, Paper-broker)` block), `state/lifecycle.rs`'s
`LiveCapital` trust-chain gate (`TV-03`, fail-closed until parity proof
completes — confirmed still fail-closed at this HEAD), and confirmed
`deployment_mode` is already durable identity on
`sys_autonomous_daily_operations` and `sys_paper_portfolio_snapshots` with
explicit mismatch rejection (`paper_portfolio.rs`: "expected 'paper'").
Combined with Paper and Live using physically separate databases
(operationally enforced — Paper 5440 / Live 5432, per this mission's own
protected-resource list) and separate broker base URLs per mode
(`alpaca_base_url_for_mode`), the negative controls M3's contract requires
("Paper cannot submit to Live," "Live cannot submit to Paper," "duplicate/
replayed deployment identity cannot cross domains") are structurally
satisfied by already-existing, already-committed architecture — not a
dormant or partial seam requiring new code. No `CODE_MISSING`/
`WIRING_MISSING` gap was found against the M3 finish-line contract's stated
requirements. **No new production code was written for M3 in this session**
— per the same principle §-5's M2 census applied ("if code is already
complete, prove it; do not invent changes").

**M3 status: CODE_MISSING = 0, WIRING_MISSING = 0** (bounded census, not an
exhaustive audit — see caveat below). **TEST_MISSING:** a dedicated
integrated negative-control proof (two daemon processes, Paper-mode and
Live-shadow-mode, running concurrently against their real separate
databases, with an explicit attempt to cross-submit/replay identity between
them and observe the fail-closed refusal) has never been run and is not
attempted here — this mission explicitly forbids enabling Live or any
broker mutation this session, and exhaustive acceptance testing is deferred
to the verification wave by mandate. This census is bounded (mirrors the
Stage A/M2 historical-correction precedent: items not independently
re-verified end-to-end should be treated as citation-unverified until that
proof exists). M3 operational acceptance is not claimed.

**M4 bounded census (US Equity/ETF Live Production).** Read
`parity_evidence.rs` (parses/validates an external `parity_evidence.json`
TV-03 artifact; `live_trust_complete` is surfaced honestly, never
fabricated — every current build's artifact hardcodes it `false` because no
real shadow-execution cycle has ever completed), `state/lifecycle.rs`'s
LiveCapital-transition gate (fail-closed on `!live_trust_complete`,
confirmed unchanged at this HEAD), `state/broker.rs::build_daemon_broker`
(refuses `BrokerKind::Paper`/`LockedPaperBroker` outright as "not the
canonical paper-trading execution path" — fail-closed against a broker that
would accept orders with no real fills; `BrokerKind::Alpaca` selects
`ALPACA_API_KEY_PAPER`/`ALPACA_API_SECRET_PAPER` for `Paper` vs
`ALPACA_API_KEY_LIVE`/`ALPACA_API_SECRET_LIVE` for
`LiveShadow`/`LiveCapital` — separate credential identity per domain, not a
shared secret gated only by a flag), and `alpaca_base_url_for_mode`
(`paper-api.alpaca.markets` vs `api.alpaca.markets`, refuses `Backtest`
outright). The reconciliation/outbox/inbox/OMS-state-machine/restart-safety
machinery this gate sits in front of is deployment-mode-agnostic by
construction (the same Alpaca adapter code already runs continuously
against the Paper endpoint) — M4's required proof surface (order/fill
lifecycle, reconciliation, cancel, restart/recovery, disconnect/reconnect)
is therefore already exercised in production against Alpaca Paper today;
what remains before it can be *claimed* for Live is exclusively the
economic/operational chain the finish line itself names as sequential and
human-gated (accumulate real shadow-execution evidence -> `live_trust_complete=true`
-> explicit operator Live authorization -> tiny-capital Live validation),
never a missing code seam.

**M4 status: CODE_MISSING = 0, WIRING_MISSING = 0** against everything this
census actually read (bounded, not exhaustive). **OPERATIONAL_ONLY /
BLOCKED (correctly, not fabricated):** `live_trust_complete=true` itself —
requires a completed real shadow-execution cycle this session cannot and
must not produce — and every step downstream of it (operator Live
authorization, tiny-capital order, real Live fill/reconciliation proof).
No code was written or changed for M4 in this session; no Live credential,
broker, or capital action was taken or simulated.

**M5 bounded census (Asset-Neutral Production Contracts).** `git log
--grep=asset-core -i` shows the most recent asset-core/registry-v2 work
(portfolio economics v2, multi-asset NAV aggregation, registry v2 status
surfacing) predates this bulk-completion wave by roughly three months, with
no work in between. Per this repository's own memory record, that lineage
already closed CODE_LOCAL but left an explicit, *recorded operator
decision* rather than an oversight: registry v1 remains the sole trading
identity source of truth; registry v2 (and the portfolio-economics/NAV
seams built on it) has zero production callers by design, pending a
deliberate v1->v2 cutover this repository's own prior sessions declined to
attempt without a dedicated verification budget of its own — the same
category of risk this wave's own C4/C5/C6 investigation (§ above)
independently rediscovered for a different subsystem. M5's own finish line
explicitly warns against exactly the alternative: "the contract is proven
sufficient by concrete later-asset requirements rather than speculative
abstraction" — and no M6 (crypto) work has started, so no concrete
non-equity requirement yet exists to prove any further generalization
against. Building more asset-neutral surface now, with no live M6 consumer
and no committed decision to cut equities over to it, would be exactly the
speculative framework the frozen contract forbids.

**M5 status: PARTIAL, unchanged from repo truth. Not a new
`CODE_MISSING` finding** — the registry v1->v2 cutover is a known,
previously-recorded `BLOCKED_OPERATOR_DECISION` (deferred pending its own
dedicated session, not an omission of this wave), and no other concrete
M5 gap was identified against the finish-line contract without inventing
speculative scope. No code was written or changed for M5 in this session.

No Paper/Live/runtime state modified; no push; `smoke_logs/` untouched.

---

## -6a. Stage B M2 — C1/C2/C3 Consolidated Surgical Correction (2026-09-19, `V4-STAGE-B-M2-C1-C3-CORRECTION-02`)

Independent review of §-6's C1/C2/C3 completion bundle found two deterministic,
CI-breaking repo-truth defects that §-6's `CODE_CLOSED` claims did not account
for, plus one risk finding requiring disposition. All three are now closed.
This does not retract §-6's `CODE_CLOSED` verdicts (the production behavior
they describe was already correct); it corrects the repo-inventory/gitattributes
governance gap that would have failed CI, and adds proof for a risk finding.

- **IR-1 (migration manifest drift).** Migration `0073_explicit_multi_strategy_
  authority_full_evidence.sql` was added on disk in `b0e957b5` but never added
  to `migrations/manifest.json`. `scripts/guards/check_migration_governance.sh`
  (wired into CI at `.github/workflows/ci.yml`) already enforces exact
  manifest/filesystem parity and was failing deterministically at HEAD `6c8e1d16`
  (`FAIL: manifest drift detected` — confirmed by running the guard before the
  fix). Added the missing manifest entry; guard now passes.
- **IR-2 (`.gitattributes` LF-pin omission).** The same commit range explicitly
  pinned migrations 0068-0072 to `eol=lf` in `.gitattributes` (byte-sensitive
  SQLx checksum identity) but omitted 0073. Guard 3 of the same script was
  failing (`eol: unspecified` for 0073, confirmed pre-fix). Added the missing
  `.gitattributes` line; guard now passes for all three checks.
- **IR-3 (scanner_rank narrowing-cast risk).** **CORRECTED 2026-09-19, see
  §-7 above — this disposition was incomplete: it never fixed the write
  itself, only the read-side backstop. The write is now a checked
  conversion (commit `9515b6f5`); the "ALREADY SAFE + PROVEN, not fixed"
  text immediately below is superseded and kept only as the historical
  record of what this session actually found and concluded at the time.**
  The write path persists
  `SelectionCandidateEvidence.scanner_rank` (`Option<u32>`) as `Option<i32>`
  via `.map(|r| r as i32)`. Disposition (superseded): **ALREADY SAFE +
  PROVEN**, not fixed.
  `u32 as i32` is a same-width bit-reinterpreting cast: every value above
  `i32::MAX` has its top bit set and therefore always becomes negative — a
  two's-complement identity, not a coincidence of typical rank values — and
  the existing read-side validator (`explicit_multi_strategy_evidence_validator
  ::stored_binding_to_evaluation`) already rejects any negative stored value
  as `NegativeScannerRank`, failing closed rather than silently accepting a
  wrapped value. Added a boundary/mutation test
  (`scanner_rank_overflow_is_always_caught_never_silently_accepted`) proving
  i32::MAX round-trips exactly and every overflowing u32 value is rejected;
  verified red (weakening the `r >= 0` guard fails the test) then green
  (restoring it passes).
- Census of analogous narrowing casts in the same seam (`binding_count`,
  `authorized_count`, `ordinal`, all `usize`/`.len()`-derived `as i32`)
  found no code-backed defect: all are bounded by an in-memory parsed
  watchlist-v3 artifact's realistic size (never remotely approaching
  `i32::MAX`), and even in a hypothetical wrap, the validator's exact-equality
  comparisons (never merely "is present") would still fail closed rather than
  coincidentally match. No dedicated test added for these — no plausible
  failure mode exists to prove against, unlike IR-3's explicit two's-complement
  boundary.
- C1 dormancy-gate call sites re-traced via the repo graph (`graft_find_all`
  for `strategy_bootstrap_dormant|is_dormant()`): only two production gate
  sites exist (`lifecycle.rs::start_execution_runtime`,
  `autonomous_runtime_context::resolve_autonomous_runtime_context_from_fleet`),
  both already consuming the one shared `explicit_watchlist_v3_authority_pending`
  predicate per §-6's own `0af79883` — no residual parallel interpretation
  found. `autonomous_completed_bar_driver.rs`'s dormancy check was re-confirmed
  out of scope (drives the separate legacy single-binding path, never the
  explicit-v3 host pool) — same conclusion §-6 already recorded.
- C3 activation ordering re-verified directly in
  `build_explicit_multi_strategy_start_snapshot`: write -> read-back -> validate
  -> (only then) `build_explicit_multi_strategy_dispatch_authority`/host-pool
  construction. Traced every production caller of `DynamicSelectionHostPool::
  build`; the only caller in the explicit-v3 authority path is this function.
  No alternate activation entry point exists.

**Acceptance (corrected numbers):** `cargo test -p mqk-daemon --lib` 998
passed / 0 failed / 22 ignored (one more than §-6's 997 — the new IR-3 proof
test); `cargo test -p mqk-db --lib` 79 passed / 0 failed / 24 ignored;
`cargo test -p mqk-db --test scenario_explicit_multi_strategy_authority_01 --
--ignored` 5 passed against `mqk_test`; `scripts/guards/check_migration_
governance.sh` passes (all 3 checks); `git diff --check` clean; `smoke_logs/`
untouched; no Paper/Live/broker mutation.

**Status: C1 = CODE_CLOSED (unchanged). C2 = CODE_CLOSED (unchanged; migration
inventory gap repaired). C3 = CODE_CLOSED (unchanged).** No new deterministic
C1/C2/C3 defect remains. C4/C5/C6/M3 not started, not authorized. No push.

---

## -6. Stage B M2 — C1/C2/C3 Final Completion (2026-09-19, `V4-STAGE-B-M2-C1-C3-FINAL-01`)

Reconciles §-5 below, whose draft text (written mid-`V4-STAGE-B-M2-REPAIR-03`)
was never updated to reflect three further commits that landed later the same
day under a follow-on controller (`V4-STAGE-B-M2-C1-C3-REPAIR-04`, visible in
those commits' own code comments): `97a9af5c` (config: align watchlist-v3
with frozen runtime authority), `b0e957b5` (authority: bind complete explicit
strategy evidence identity — Patch D2, migration 0073's full-evidence
columns), and `ac7b400d` (runtime: true read-side validation — Patch D3,
`explicit_multi_strategy_evidence_validator.rs`, plus lifecycle.rs's own
`explicit_v3_authority_pending` STRATEGY-DORMANCY-01 bypass). §-5's "R2B/R2C
Still Open" framing undersold what REPAIR-04 had already closed; this section
is the accurate final record.

**This controller's own two commits**, starting from HEAD `ac7b400d`:

- `0af79883` (C1): `resolve_autonomous_runtime_context_from_fleet`
  (`state/autonomous_runtime_context.rs`) — the one seam every autonomous
  daily-coordinator/completed-bar-task/operator-retry caller uses — still
  carried the pre-REPAIR-04 STRATEGY-DORMANCY-01 interpretation verbatim:
  Paper+Alpaca with no `MQK_STRATEGY_IDS` refused unconditionally even with
  an approved watchlist-v3 fleet configured, even though `lifecycle.rs`'s own
  `start_execution_runtime` gate had already been repaired to bypass this.
  Extracted the bypass predicate into one shared, pure
  `dynamic_selection_mode::explicit_watchlist_v3_authority_pending` helper
  consumed by both call sites, closing the one remaining parallel
  interpretation the frozen-contract review had flagged. Proof: three new
  focused tests (`a11`/`a12`/`a13` in
  `scenario_autonomous_daily_coordinator_policy_01.rs`) prove the bypass
  fires only for an approved v3 artifact under `paper_enforced`, never for
  `LoadedNotApprovedV3` or outside that mode.
- `420c0690` (C2): adversarial field-by-field audit of
  `derive_explicit_multi_strategy_authority_id` found all 33
  `SelectionCandidateEvidence` fields structurally bound into `authority_id`,
  but 9 of them had no mutation-test proof the binding actually holds
  (`promotion_query_ok`, `promotion_state`, `config_identity_verified`,
  `durable_config_fingerprint`, `current_config_fingerprint`,
  `registry_enabled`, `data_ready`, `promotion_transition_id`,
  `evidence_transition_id`). Added the missing mutators. Also replaced the
  DB round-trip test's handful of spot-checks with an exhaustive per-field
  comparison of all 40 persisted columns against the value written, run for
  real against `postgres://postgres:postgres@127.0.0.1:5434/mqk_test`.

**C3**: adversarially reviewed `explicit_multi_strategy_evidence_validator.rs`
(D3) and `build_explicit_multi_strategy_start_snapshot`'s call ordering
directly — the validator recomputes `authority_id` from durable stored facts
(never trusts the stored column), checks every header identity field, the
exact binding set (missing/extra/duplicate), and is called strictly before
host-pool construction (`validate_...` at `lifecycle.rs:1181`, host pool
`build_explicit_multi_strategy_dispatch_authority` at `lifecycle.rs:1210`+).
The status route (`routes/dynamic_selection_evidence.rs`) truthfully
distinguishes `explicit_watchlist_v3_multi_strategy` and never fabricates a
Bundle-7 `committed_plan_id`/`committed_source_kind` for it (already covered
by its own dedicated test). No defect found; no additional patch needed.
Ran the integrated DB-backed proof
(`c3_01_real_registry_promotion_and_evaluate_candidate_drive_durable_authority`,
real registry + real research/backtest/promotion-to-`active_paper` chain)
against the real test DB this session — passes.

**Known, deliberately out-of-scope finding (not part of C1/C2/C3, not
touched)**: `autonomous_completed_bar_driver.rs`'s `prove_running_dispatch_eligibility`
treats a `Dormant` native-strategy bootstrap as unconditionally
"not ready" for per-bar dispatch — a second dormancy interpretation on paper,
but this driver only supports the single-effective-binding (legacy
`MQK_STRATEGY_IDS`) dispatch path; it is not on the explicit-v3 runtime path
at all (that path dispatches through `state/loop_runner.rs`'s host pool, per
the frozen contract §9). This is the already-identified, already-deferred
R2B/R2C multi-binding completed-bar driver gap (§-5 below), requiring its
own dedicated verification budget per that prior session's explicit decision
— confirmed still accurate, not re-attempted here (mission scope: C1/C2/C3
only, no C4/C5/C6).

**Acceptance**: `cargo test -p mqk-daemon --lib` 997 passed / 0 failed / 22
ignored; `cargo test -p mqk-db --lib` 79 passed / 0 failed / 24 ignored;
plus the DB-backed integration suites for both crates' explicit-multi-
strategy-authority mechanisms run directly against `mqk_test`, all passing.

**Final status: C1 = CODE_CLOSED. C2 = CODE_CLOSED. C3 = CODE_CLOSED.**
Independent review of the diff is still required before any of the three is
treated as accepted contract. **M2 overall remains NOT CLOSED** — C4/C5/C6
(the R2B/R2C multi-binding completed-bar driver/aggregation rewrite among
them) are the remaining Stage B work, out of scope for this controller. M1
operational status is unchanged; alpha discovery remains deferred; no
Paper/Live/runtime state was modified; no push; no M3.

---

## -5. Stage B M2 — R1B Live Activation Closed; R2B/R2C Still Open (2026-09-19, `V4-STAGE-B-M2-REPAIR-03`)

Independent review found R1A/R1B were not fully coherent (§-4 below): the
frozen contract required watchlist-v3 wired into the *canonical* intake
contract (`WatchlistIntakeOutcome` itself), but R1B instead built a wholly
separate `WatchlistIntakeOutcomeV3` type the canonical evaluator never
recognized. This controller repaired that mismatch and closed R1B's
previously-`WIRING_MISSING` live-activation gap.

**Patch C1 (commit `7db0a18b`) — canonical intake repair.**
`WatchlistIntakeOutcome` now has `LoadedApprovedV3`/`LoadedNotApprovedV3`
variants; `evaluate_watchlist_intake` recognizes `watchlist-v3` directly
(v1/v2 parsing byte-for-byte unchanged); `state/multi_symbol_config.rs`
gained the frozen contract's v3-aware config source. The V16 test that
previously asserted the *mismatch* as correct behavior is replaced with
proof of the fix.

**Patch C2 (commit `9fc2f912`) — durable explicit-authority evidence.**
New additive tables (migration 0072) `sys_explicit_multi_strategy_authority`
+ `_bindings`, deliberately separate from `sys_dynamic_selection_plans`
(Bundle 7's own ranking-plan schema — never conflated). `authority_id` is a
deterministic identity binding every result-affecting input (run_id, source
artifact hash, config fingerprint, market_date, every binding's own
evidence) so a changed artifact or a changed promotion/config/readiness
fact can never reuse a stale authority. Idempotent-insert-or-payload-
collision, mirroring `insert_dynamic_selection_plan`'s established pattern.

**Patch C3 (commit `644cfde7`) — real daemon-start activation. R1B's
WIRING_MISSING gap is now CODE_CLOSED.** `state/lifecycle.rs`'s real
`build_dynamic_selection_start_snapshot` now routes a configured
`watchlist-v3` artifact under `PaperEnforced` to a new
`build_explicit_multi_strategy_start_snapshot`, which runs the full real
sequence (canonical v3 validation -> real `evaluate_candidate` DB I/O ->
build+persist+read-validate the durable authority -> construct the isolated
host pool -> return committed runtime truth) and fails start closed on any
step. A new `RuntimeStrategyAuthorityKind` (`Legacy` /
`Bundle7DynamicSelection` / `ExplicitWatchlistV3MultiStrategy`) lets a
status surface distinguish which mechanism produced a given start's
authority without inferring it from `plan`'s presence; this mechanism never
fabricates a Bundle-7 plan. Proven with real registry + real
research/backtest/promotion-to-`active_paper` evidence (not hand-built
`SelectionCandidateEvidence`): the durable evidence persists even when the
overall start is honestly refused (data readiness genuinely fails for a
symbol with no real instrument-registry entry — the same wall R1B's own
heaviest existing fixture, `full_evidence_chain_passes_refused_only_on_data_readiness`,
also stops at); an unregistered sibling refuses independently; a genuinely-
promoted identity is still excluded when dry-run-flagged; tampering the
persisted row then re-running fails closed as a payload collision; two
independently-authorized same-symbol identities build two real hosts; `Off`
and the live-lock are untouched. Full regression: 983 mqk-daemon lib tests
green.

**R2B/R2C (multi-binding completed-bar driver + hybrid aggregation) — still
NOT IMPLEMENTED.** Investigated: `resolve_single_effective_binding`
(`state/autonomous_completed_bar_driver.rs`) is the exact-one-binding
restriction R2 must replace, embedded through ~2,600 lines of already-
audited, safety-critical claim/dispatch machinery across
`autonomous_completed_bar_driver.rs`, `autonomous_completed_bar_task.rs`,
and `autonomous_daily_coordinator.rs` — the live autonomous-trading
heartbeat. Not attempted this session: a correct rewrite requires its own
dedicated verification budget (idempotency/restart-safety proofs, per-
binding negative controls) that this session's remaining time could not
responsibly provide without risking exactly the kind of under-verified
change this repair mission exists to prevent (CLAUDE.md's correctness-first
priority). C6 (integrated M2 finish-line proof) is consequently also not
attempted — it is blocked on C4/C5 existing as real production paths.

**Doc truth corrections (this section):** the R1B section below (§-4)
claimed "All 10 of R1B's mission-required focused proofs... each calling a
real production function" for the *whole* proof set; several of those
proofs (the Bundle-6-conflict-resolution ones) call
`compute_dynamic_selection_plan` directly with hand-built
`SelectionCandidateEvidence` — genuine, valid proofs of the pure
selector/conflict-resolution logic, but not DB evidence-gate proofs. Only
the promotion/registry/dry-run-facing proofs (`r1b_06`, `r1b_08`) exercise
real DB-backed evidence. This session's own C3 tests are the first to
exercise the real `evaluate_candidate` DB path end to end for this
mechanism. A separately-referenced "V3 report" commit-count typo could not
be located anywhere in this repository or in `Downloads/`; it may refer to
an external/prior-session artifact not present on this machine and was not
corrected.

**Corrected M2 totals (this session):** `CODE_CLOSED 7/9` (M2.2, M2.4,
M2.5, M2.6, M2.8, M2.9, plus **M2.1 and M2.3 upgraded from
WIRING_MISSING to CODE_CLOSED** by C1-C3), `WIRING_MISSING 1/9` (M2.7 —
durable per-binding schema CODE_CLOSED via R2A, driver/aggregation still
not wired, R2B/R2C). **M2 CODE COMPLETION remains NOT CLOSED** (one
requirement, M2.7, still open) and **M2 operational acceptance remains
NOT CLAIMED** regardless. No Paper/Live/runtime state modified; no push;
no M3; no alpha discovery.

---

## -4. Stage B M2 Completion After Operator Decisions (2026-09-18, `V4-STAGE-B-M2-REPAIR-02`)

The operator resolved both `SPEC_DECISION_REQUIRED` items from §-3 below
with frozen decisions. This controller implemented against both. Full
evidence: `docs/V4_CODE_COMPLETION_MANIFEST.md` § "Stage B M2 Completion
After Operator Decisions".

**R1 frozen decision:** explicit per-symbol multi-strategy authorization via
a new, additive `watchlist-v3` schema (`symbol -> Vec<strategy_id>`), not
implicit fleet-wide activation and not a relaxation of Bundle 7's frozen
selector. Design frozen in
`docs/specs/multi_strategy_runtime_dispatch_01a_frozen_contract.md`
(commit `7f78cc0d`).

**R1B result — CODE_CLOSED (pipeline) / WIRING_MISSING (live activation).**
The same-symbol multi-strategy dispatch/conflict/config pipeline is
implemented and proven with 10 real production-function-level proofs (69
tests green: commit `a00f9238`). Two real strategies on one symbol
genuinely dispatch with distinct identity; both reach Bundle 6 and resolve
deterministically regardless of input order; a promoted sibling never
authorizes an unpromoted one; same-symbol provenance swap fails closed;
dry-run identity is excluded; no cross-contamination; existing v1/v2
config is unaffected (43/43 regression). **Not yet spliced into the live
daemon start sequence** — that requires extending Bundle 7's
plan/evidence-persistence type shape, a new observability surface R1A's
own contract explicitly deferred out of R1B's scope. This is a narrow,
well-understood follow-up integration patch, not an open design question.

**R2 frozen decision:** hybrid per-binding fault isolation. A binding-local
failure (symbol-specific no-new-bar, missing/stale data, readiness
failure, unsupported symbol/timeframe, isolated provider failure)
quarantines only that binding. A global-critical failure (evidence-lineage
corruption, dispatch-claim ambiguity, runtime-ownership/leadership
failure, etc.) remains operation-wide fail-closed via the existing state
machine, unchanged.

**R2A result — CODE_CLOSED.** New additive table
`sys_autonomous_daily_binding_state` (migration 0071, commit `9fd7df12`)
durably tracks each binding's `active`/`locally_blocked` health with a
closed reason-code vocabulary. 8/8 DB-backed tests: schema/constraint
enforcement, restart persistence, cross-binding isolation.

**R2B/R2C result — NOT IMPLEMENTED, WIRING_MISSING.** The multi-binding
completed-bar driver rewrite and operation-state aggregation
(~2,600 lines across `autonomous_completed_bar_driver.rs`,
`autonomous_completed_bar_task.rs`, `autonomous_daily_coordinator.rs` —
the actual live autonomous-trading heartbeat) were investigated in full
but not attempted this session: the safety-critical nature of this exact
code path, and the fact that this whole repair mission exists because a
prior session rushed a closure claim on this same subsystem class, made a
rushed rewrite in remaining session time the wrong tradeoff. R2A's durable
foundation is ready for a focused follow-up patch with its own dedicated
verification budget.

**R3:** reconfirmed unchanged, 16/16 still passing, no code modified.

**R4:** still `BLOCKED` — needs R1B's live-activation splice and R2B/R2C,
neither of which exist as production paths yet.

**Corrected M2 totals:** `CODE_CLOSED 6/9` (M2.2, M2.4, M2.5, M2.6, M2.8,
M2.9), `WIRING_MISSING 3/9` (M2.1, M2.3, M2.7 — each has a real, tested
underlying capability; each is missing only its final production-
activation splice). `SPEC_DECISION_REQUIRED: 0` (both resolved this
session). **M2 code completion is NOT closed** (per this controller's own
rule: only claim closed if the real integrated proof, R4, passes — it does
not yet). M2 operational acceptance remains unclaimed. M1 operational
blocker, the `intraday_scalper` rejection, and alpha-discovery deferral
are unchanged.

---

## -3. Stage B M2 Repair Outcome — R1/R2 stopped as SPEC_DECISION_REQUIRED (2026-09-18, `V4-STAGE-B-M2-REPAIR-01`)

Independent review rejected the original Stage B 9/9 CODE_CLOSED conclusion
(see §-2 below). This controller investigated both confirmed gaps in full
and, for each, found a genuine unresolved operator decision — not merely
missing code — blocking further implementation. **No production/test code
was changed by this controller.** Full evidence and cited line numbers:
`docs/V4_CODE_COMPLETION_MANIFEST.md` § "Stage B M2 Repair Findings".

**R1 (`MULTI-STRATEGY-RUNTIME-DISPATCH-01`, blocks M2.1/M2.3) — `SPEC_DECISION_REQUIRED`.**
Both Bundle 6's and Bundle 7's own committed design docs explicitly defer
"how multiple strategies become simultaneously economically active on the
same symbol" to this not-yet-designed follow-up patch; no committed
contract answers it. Operator must choose one of:
- (a) multi-entry `MQK_STRATEGY_IDS` = fleet-wide multi-strategy;
- (b) watchlist-v2 `strategy_assignments` becomes `symbol -> Vec<strategy_id>` (per-symbol explicit);
- (c) relax `dynamic_selection.rs`'s frozen one-per-symbol contract to ranked top-N under a new mode;
- (d) something else.

Each has different capital-caps/promotion-authority consequences that this
controller will not invent unilaterally (`CLAUDE.md` §19).

**R2 (multi-symbol completed-bar driver, blocks M2.7) — `SPEC_DECISION_REQUIRED`.**
Concrete, code-proven conflict found: `select_driver_mode_for_state`
(`state/autonomous_completed_bar_task.rs:154-164`) returns `None` (halts
**all** automated driver invocation, for every configured symbol) for
`controller_degraded`/`evidence_degraded` — so today's single-binding
"any critical fault degrades the operation" behavior, naively extended to
N bindings, would let *one* symbol's non-remediable blocker silently stop
dispatch for *every* symbol. This directly violates R2's own requirements
("a claimed bar for symbol A cannot suppress symbol B"; "one symbol lacking
a new bar must not cause another symbol's real new bar to be lost").
Operator must choose:
- (a) keep coarse operation-wide degrade (simplest, but knowingly violates
  the isolation requirement the moment >1 symbol is configured);
- (b) add genuine per-binding fault state and change the state-machine
  gating so operation-level degrade reflects "no binding can progress," not
  "any binding failed" (correct, but a real schema + state-machine change to
  an already-audited subsystem);
- (c) a hybrid (only certain fault classes — e.g. evidence/claim-integrity —
  degrade globally; per-symbol readiness/config faults isolate).

**R3 (retry/restart idempotency proof) — `CLOSED`.** Ran
`scenario_strategy_decision_idempotency_01` in full against the real test
Postgres: 16/16 passed (4 DB-backed, including restart/replay and
multi-symbol cross-contamination negative controls). No code changed — M2.6
now has direct load-bearing proof from this session, not just a cited seam.

**R4 (integrated M2 finish-line scenario) — `BLOCKED`** on R1/R2; cannot be
honestly built without reimplementing the missing production logic inside
the test, which would prove nothing real.

**Corrected M2 totals:** `CODE_CLOSED 6/9` (M2.2, M2.4, M2.5, M2.6, M2.8,
M2.9), `WIRING_MISSING 2/9` (M2.3, M2.7 — both blocked on the above),
`PARTIAL/WIRING_MISSING 1/9` (M2.1). **M2 code completion is NOT closed.**
Two operator decisions (R1, R2) are required before remaining M2 code can
proceed. M1 operational blocker, `intraday_scalper` rejection, and alpha
discovery deferral are unchanged.

---

## -2. Governance Decision — Bulk Code Completion Resumes (Stage B / M2), M1 Operational Status Unchanged (2026-09-18, `V4-BULK-CODE-COMPLETION-STAGE-B-M2-02`)

Branch: `v4-bulk-code-completion-stage-b-m2-01`. Baseline HEAD (ancestor):
`28fff6952a65791cd489b88d3673561ab8c12a03`.

The operator has explicitly changed near-term execution priority. This is a
sequencing decision only; it does not redefine milestone acceptance.

**Two status dimensions are now tracked separately and must not be
conflated:**

- **A. CODE COMPLETION** — may continue into Stage B / M2 now.
- **B. OPERATIONAL ACCEPTANCE** — unaffected by this decision; M1 remains
  `BLOCKED` per the M1.9 correction below.

**Durable truth carried forward unchanged by this decision:**

- Stage A / M1 code-completion: `INDEPENDENTLY ACCEPTED`, `PUSHED-VERIFIED`.
  M1 code census: `CODE_MISSING=0`, `WIRING_MISSING=0`, `TEST_MISSING=0`.
- Deployed strategy identity: `intraday_scalper` / `AAPL` / `300s`.
- A genuine real-data promotion-evidence attempt was performed once and was
  **REJECTED**: `review_state=rejected`, `reason_code=negative_total_return`.
  This rejection is durable truth. It is not waived or converted to a pass.
  `intraday_scalper` must not be promoted from that evidence, and the
  rejected attempt must not be retried/tuned merely to obtain a passing
  result.
- Formal M1 operational acceptance remains **BLOCKED** (deployment authority
  gap, see §-1 below — unchanged by this decision).
- M1.10 formal soak remains `OPERATOR-WAIVED`. WAIVED != PASSED, WAIVED !=
  OPEN BLOCKER.
- Additional alpha discovery is now `OPERATOR-DEFERRED`.

**What this decision authorizes:**

- Stage B / M2 CODE COMPLETION work may proceed now, independent of the open
  M1 operational-only gate, using the canonical M2 requirement set in
  `MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md`.
- Later-milestone code may be implemented and tested without that implying
  the corresponding milestone is operationally accepted.

**What this decision does NOT authorize:**

- M1 must not be called formally `CLOSED`.
- Paper must not be promoted or forced ready.
- Live must not be enabled.
- No alpha discovery, no retry/tune of the rejected `intraday_scalper`
  evidence, no Paper/Live operational ceremony under this controller.

### Stage B / M2 result (2026-09-18, CORRECTED 2026-09-18, `V4-STAGE-B-M2-REPAIR-01`)

**The original 9/9 CODE_CLOSED conclusion below was REJECTED by
independent review and is superseded.** It is preserved as audit history
in `docs/V4_CODE_COMPLETION_MANIFEST.md` (struck through in place, not
deleted).

~~Bounded census against canonical M2 found all 9 required capabilities
already CODE_CLOSED... 137 targeted tests... no production/test code was
changed.~~

**Confirmed gaps (independent review, 2026-09-18):**

1. **M2.3 = WIRING_MISSING.** Bundle 6 (`runtime_strategy_conflict.rs`) is
   real and correctly resolves conflicting same-symbol inputs, but no
   production producer can ever hand it two genuine same-symbol
   economically-active strategy decisions in one cycle:
   `mqk-portfolio/src/dynamic_selection.rs:29` selects "exactly one
   candidate per symbol, or none — never more"; `StrategyHost` refuses a
   second concurrent strategy registration
   (`StrategyHostError::MultiStrategyNotAllowed`,
   `scenario_parallel_long_short_strategy_01.rs::p11_strategy_host_enforces_single_strategy`).
   Remaining work: `MULTI-STRATEGY-RUNTIME-DISPATCH-01`.
2. **M2.7 = WIRING_MISSING.** The authoritative autonomous completed-bar
   production driver, `resolve_single_effective_binding`
   (`state/autonomous_completed_bar_driver.rs:329-333`), only ever resolves
   exactly one binding and explicitly documents multi-symbol assignment as
   unsupported (fails closed). The durable per-assignment claim identity
   foundation already exists (`sys_autonomous_daily_bar_dispatches`, keyed
   by `(operation_id, local_symbol, timeframe, bar_end_ts)`) and must be
   reused, not replaced.
3. **M2.1 = PARTIAL / WIRING_MISSING** (downstream of #1 — multiple
   *symbols* with distinct single strategies works; multiple
   economically-active *strategies on the same symbol* does not).

All other requirements (M2.2, M2.4, M2.5, M2.6, M2.8, M2.9, and the five
additional invariants) are unaffected by this correction and retain
`CODE_CLOSED`.

```text
M2 CODE COMPLETION:        CODE_CLOSED 6/9, WIRING_MISSING 2/9 (M2.3, M2.7),
                            PARTIAL/WIRING_MISSING 1/9 (M2.1)
M2 OPERATIONAL ACCEPTANCE:  NOT CLAIMED (unchanged)
```

Remaining work tracked under `V4-STAGE-B-M2-REPAIR-01`:
- **R1** — `MULTI-STRATEGY-RUNTIME-DISPATCH-01`: wire genuine same-symbol
  multi-strategy dispatch into the existing Bundle 6 conflict authority.
- **R2** — extend the autonomous completed-bar driver to support multiple
  configured symbol/timeframe assignments, reusing
  `sys_autonomous_daily_bar_dispatches` identity.
- **R3** — run the existing `scenario_strategy_decision_idempotency_01`
  restart/replay proof (M2.6 load-bearing check not run in the original
  census).
- **R4** — one integrated M2 finish-line scenario combining R1+R2 with
  retry/restart and cross-contamination negative controls.

Full detail and citations: `docs/V4_CODE_COMPLETION_MANIFEST.md`
§ "M2 Code Completion Manifest" (original census + correction note +
per-requirement corrections).

---

## -1. M1.9 Deployed-State Verification Result (2026-09-19, READ-ONLY; CORRECTED 2026-09-19)

A bounded, read-only M1.9 verification was performed against the actual
deployed Paper system (daemon PID 16680, `mqk-paper-postgres` /
`miniquantdesk_paper` on port 5440, live status API on 127.0.0.1:8899).
Full evidence: `C:\Users\Zacha\Downloads\MQD_M1_9_DEPLOYED_STATE_REVIEW\`.

Seven of eight sub-items are truthfully verified and internally
consistent: correct Paper DB, correct deployment mode/adapter
(`paper`/`alpaca`), `live_routing_enabled=false`, provider data freshness
(AAPL 5m latest completed bar `2026-09-17T16:20:00Z`, stale only because
the runtime has been disarmed/halted since then — not a separate
provider defect), scheduler registration (task `Ready`, correct
action/arguments), and risk/arm/reconcile truth
(`sys_arm_state.state=DISARMED` reason `DeadmanSupervisorFailure` since
`2026-09-17T16:33:50Z`, `reconcile_status=ok`, 0 mismatches, no active
risk block). A halted/disarmed state is treated as truthful, not as an
M1.9 failure, per the mission's own acceptance rule.

### CORRECTION (M1-DEPLOYED-PROMOTION-AUTHORITY-01, 2026-09-19)

The **deployed-universe/promotion-authorization** sub-item was
previously recorded as `UNKNOWN_NEEDS_PROOF` on the theory that
native/built-in strategies might be authorized through a path other
than `sys_strategy_promotion_transitions`. An independent review found
this classification error: current production code makes the outcome
deterministic, and that theory is contradicted by the code itself.

Full evidence: `C:\Users\Zacha\Downloads\MQD_M1_9_PROMOTION_AUTHORITY_REVIEW\`.

Five invariants were inspected and cited against current HEAD (`05_code_authority.txt`), all CONFIRMED:
1. `submit_internal_strategy_decision` Gate 3b unconditionally invokes
   `evaluate_paper_promotion_gate` for every strategy_id
   (`mqk-daemon/src/decision.rs:801-828`).
2. `registered + enabled` in `sys_strategy_registry` is explicitly
   documented and enforced as insufficient
   (`mqk-daemon/src/promotion_gate.rs:11-16`; Gate 3 and Gate 3b are
   separate, sequential gates).
3. Only an exact `(strategy_id, symbol, timeframe_secs)` match with
   current state `active_paper` (not expired, already effective)
   authorizes trading (`mqk-db/src/strategy_promotion.rs:989-1019`).
4. Absence of a promotion record returns `paper_tradable=false`,
   `reason_code=promotion_missing`
   (`mqk-db/src/strategy_promotion.rs:993-995`) — confirmed live.
5. There is **no** special native/built-in bypass for `intraday_scalper`
   or any `kind=native` strategy; the registry's `kind` field is never
   read by the promotion gate.

The exact deployed identity was resolved read-only:
`strategy_id=intraday_scalper`, `symbol=AAPL`, `timeframe_secs=300`
(`01_runtime_identity.txt`; `configured_fleet_size=1`,
`runtime_execution_mode=single_strategy` — this is the ONLY runtime
fleet member).

The read-only truth surface `GET /api/v1/strategy/promotions/check` was
queried for that exact identity (no POST transition route called):
`tradable_paper=false`, `reason_code=promotion_missing`,
`current_state=null` (`02_promotion_check.txt`). `GET
/api/v1/strategy/promotions` additionally confirms **zero** promotion
rows exist for any identity system-wide.

Promotion-evidence availability was checked against the configured
review-artifact root (`exports/strategy_reviews/`, default path): the
only artifact present is for a different strategy (`swing_momentum`),
scored 0 `paper_candidate` results out of 88, and has no entry for
`intraday_scalper`/`AAPL` at all (`03_existing_promotion_evidence.txt`).
Classification: **NO_VALID_PROMOTION_EVIDENCE**.

The 40 non-`intraday_scalper` `sys_strategy_registry` rows were
bounded-classified: all 40 trace to exact test-fixture call sites
(`unique_id(...)` in `scenario_internal_strategy_decision.rs`,
`scenario_suppress_strategy.rs`, and
`scenario_sector_risk_gate_etf_risk_closure_01.rs`), carry zero
promotion/signal-eval references, and are not runtime fleet members.
Classification: **CONFIRMED_TEST_RESIDUE** for all 40
(`04_registry_residue_classification.txt`).

**Corrected result:**
- **CODE DEFECT:** none established by this finding — the promotion
  gate enforces its documented invariant correctly and identically on
  both the write path (Gate 3b) and the read-only observability path.
- **DEPLOYMENT AUTHORITY GAP:** **CONFIRMED**. The deployed
  `intraday_scalper`/`AAPL`/300 identity cannot create a new Paper
  outbox order through the canonical internal decision path without an
  `active_paper` promotion, no such promotion exists, and no valid
  promotion evidence exists to create one through the normal transition
  path. `intraday_scalper` was seeded directly into the enabled runtime
  fleet ("Seeded for autonomous paper trading startup") without ever
  being run through the promotion pipeline the code requires before it
  may actually trade.
- **M1.9:** corrected from `UNKNOWN_NEEDS_PROOF` to verified-sufficient
  to establish the deployed mismatch — not a residual unknown, a
  confirmed gap.

Also observed (informational, not diagnosed further — out of read-only
scope): today's (`2026-09-18`) autonomous daily operation
(`352ea5d7...`) never started a run (`start_attempt_count=0`) and was
closed to `evidence_degraded` at end-of-day rollover, consistent with
the runtime remaining disarmed for the entire session window rather
than a separate scheduling defect.

**Formal M1 closure status: BLOCKED.** Independent of the Stage A
code-completion acceptance recorded in §0 below, formal M1 is blocked
until the deployed Paper strategy has truthful promotion authority (an
`active_paper` promotion for `intraday_scalper`/`AAPL`/300, created
through a validated evidence bundle via
`POST /api/v1/strategy/promotions/transition`) or is removed from the
deployed trading universe by an explicit, valid operator decision. M2
remains **NOT AUTHORIZED** by this document.

M1.10 remains `OPERATOR-WAIVED`. WAIVED != PASSED. WAIVED != OPEN
BLOCKER. This is unrelated to and unaffected by the M1.9 correction
above.

No Paper/Live/runtime state was modified during this verification or
this correction: no re-arm, no halt clear, no daemon restart, no
scheduled-task mutation, no Discord test, no order submission, no
`.env.local` edit, no `smoke_logs/` access, no promotion-transition
endpoint call.

---

## 0. Stage A M1 Code-Completion Census — Current Status (2026-09-18)

Branch:

```text
v4-bulk-code-completion-stage-a-m1-01
```

Baseline HEAD (ancestor):

```text
f0e16651da74cc4a26726ae02321315618855a22
```

**Stage A bounded census completed locally.** Gap census result (bounded, requirement-driven inspection of M1-critical seams — not an exhaustive repo audit):

```text
CODE_MISSING:      0
WIRING_MISSING:    0
TEST_MISSING:      0
OPERATIONAL_ONLY:  1
  - Actual deployed Paper DB/config/provider/universe/scheduler/risk-state verified
```

Genuine Paper trade lifecycle and genuine no-trade lifecycle were both
originally listed here as still-unproven OPERATIONAL_ONLY items. They are
not: both have already been observed end to end against a real Alpaca
Paper broker during live market hours and are recorded as CLOSED_LOCAL in
committed closure decisions (`docs/specs/paper_trade_lifecycle_proof_02_fast_market_hours_retry.md`,
`docs/specs/paper_daily_pnl_capture_01e_closure_decision.md`,
`docs/specs/paper_order_lifecycle_visibility_01e_closure_decision.md`,
`docs/specs/auton_no_trade_02c_market_hours_closure_decision.md`,
`docs/specs/market_hours_proof_sweep_01e_closure_decision.md`). See
`docs/V4_CODE_COMPLETION_MANIFEST.md` M1.7/M1.8 for the full evidence
chain. They are not re-demanded here absent a new deterministic
contradiction.

Full detail: `docs/V4_CODE_COMPLETION_MANIFEST.md`. Its citations were
independently spot-checked (`V4-STAGE-A-M1-CLOSEOUT-02`) against several
of the original census's table names, file names, and line numbers that
did not match the repo; the manifest body has since been rewritten to
cite the verified current locations directly (`V4-STAGE-A-M1-DOC-TRUTH-REPAIR-01`).
No classification changed as a result of that citation repair.

Formal M1 soak requirement:

```text
OPERATOR-WAIVED
```

WAIVED does not mean PASSED (no claim of 10/10 or 5/5 sessions is made),
and it does not mean OPEN BLOCKER either (no further multi-day soak is
required before advancing).

**This status explicitly is NOT:**
- independent acceptance of Stage A;
- formal M1 closure (M1 is not formally closed merely because
  code-completion gaps are zero — the one remaining OPERATIONAL_ONLY item,
  actual deployed Paper state verification, is open against full M1
  closure; the operator-waived soak is neither passed nor an open
  blocker);
- authorization to push to origin (push requires independent review);
- authorization to begin M2 (M2 is not authorized by this controller).

Discord-notification provenance investigation (read-only, V4-STAGE-A-M1-CLOSEOUT-02): see the Stage A review bundle (`C:\Users\Zacha\Downloads\MQD_STAGE_A_M1_REVIEW\07_discord_provenance.md`). No deterministic M1 defect was found; verdict and detail are in that file. No Paper/Live/runtime state was modified during that investigation.

---

## 1. Prior Checkpoint — M1-LINEAGE-SAME-RUN-RECOVERY-01 (context, not current HEAD)

The section below predates the Stage A M1 census and was written against `main` at the lineage-repair commit. It is retained as prior-state context; section 0 above is current truth for the active branch.

Local branch (at the time this section was written):

```text
main
```

Local HEAD after the latest lineage repair:

```text
f0e16651da74cc4a26726ae02321315618855a22
```

Commit subject:

```text
fix(autonomous): preserve lineage across same-run recovery
```

Remote `origin/main` was still at:

```text
554faa20d770325e22d12ed08ecbd98f63d1122e
```

at the time this checkpoint was written.

Latest patch status:

```text
M1-LINEAGE-SAME-RUN-RECOVERY-01
STATUS: LOCALLY COMPLETE
PUSH: NO
INDEPENDENT ACCEPTANCE: PENDING
```

---


## 2. Current Machine / Runtime State

**HISTORICAL (superseded) — shutdown snapshot as previously recorded, no longer current:**

```text
MiniQuantDesk-Paper-Preopen-Startup:  STOPPED + DISABLED
mqk-daemon:                           STOPPED
mqk-cli/cargo/rustc:                  NONE RUNNING
MQD GUI/node helpers:                 STOPPED
MQD Docker containers:                mqk-test-postgres / mqk-live-postgres / mqk-paper-postgres STOPPED
HEAD (at time of that snapshot):      f0e16651 (main)
```

**Fresh read-only capture, 2026-09-19T00:36:35Z** (this repair turn;
read-only inspection only — no daemon start/stop/restart, no re-arm, no
halt clear, no scheduled-task modification, no Discord test, no
Paper/Live state mutation):

```text
MiniQuantDesk-Paper-Preopen-Startup:
  PRESENT, State=Ready (Get-ScheduledTask) — this is the enabled/idle
  state, not Disabled. This contradicts the historical snapshot above.
  Get-ScheduledTaskInfo (last-run/next-run detail) errored on this box
  and could not be read this turn — treat last/next run time as
  UNAVAILABLE, not absent.

mqk-daemon:
  PRESENT / RUNNING — PID 16680, image
  C:\Users\Zacha\Desktop\MiniQuantDeskV4\core-rs\target\release\mqk-daemon.exe,
  StartTime 2026-09-15T11:42:29 (local), listening on 127.0.0.1:8899
  (TCP connect succeeds). This contradicts the historical "STOPPED" entry
  above.

mqk-daemon read-only API (GET /api/v1/system/status on :8899):
  UNAVAILABLE — TCP connects but the HTTP request did not return within
  15s (timed out). Runtime/halt/kill-switch/reconcile/session-window
  truth could not be read this turn; do not assume any value for it.

mqk-cli/cargo/rustc:
  NONE RUNNING (checked by name; consistent with historical snapshot).

MQD GUI/node helpers:
  PRESENT — 14 `node` processes running (oldest since 2026-09-15, newest
  since 2026-09-18T14:30). This contradicts the historical "STOPPED"
  entry above. Not identified further this turn (which dev server/task
  owns each PID is UNAVAILABLE without additional read-only inspection).

MQD Docker containers:
  UNAVAILABLE — Docker Desktop application processes are present
  (`Docker Desktop`, `com.docker.backend`) but `docker ps` itself fails
  ("Docker Desktop is unable to start"). Container status for
  mqk-test-postgres / mqk-live-postgres / mqk-paper-postgres could not be
  read this turn; do not assume STOPPED or RUNNING for any of them.

Git index lock:
  NONE (Test-Path on .git/index.lock = False).

Git worktree (this branch, this turn):
  Two intentional modifications from this doc-repair patch
  (docs/CURRENT_MISSION.md, docs/V4_CODE_COMPLETION_MANIFEST.md); no
  other changes.

HEAD:
  c2ed45cb (v4-bulk-code-completion-stage-a-m1-01) — unchanged by this
  read-only capture.
```

This turn's inspection did **not**:
- touch `smoke_logs/`;
- run `git clean`, `git reset`, or `git stash`;
- delete evidence/artifacts or Docker containers;
- start, stop, or restart the daemon;
- re-arm, clear a halt, or modify any scheduled task;
- send a Discord test;
- mutate any Paper/Live state.

Operator: the daemon-process and scheduled-task findings above
contradict the previously recorded shutdown state. Do not assume Paper
auto-start is currently disabled based on the historical block — verify
directly (e.g. via a working `docker ps` and a responsive daemon status
route) before relying on either the historical or this turn's partial
capture for an operational decision.

---

## 3. Operator Decision — M1 Soak

The formal M1 soak requirement is:

```text
OPERATOR-WAIVED
```

Do not claim:
- 10/10 countable sessions;
- 5/5 clean sessions;
- soak passed.

Do not require another 10-day or 30-day soak before advancing.

The replacement exit criterion is a bounded proof that the Paper core path can stay alive and trading-capable through the normal production path.

---

## 4. Confirmed Production Defect That Was Just Patched

2026-09-15 Paper operation:

```text
operation_id:
91626a1d-7c85-50ac-b749-66f33c124d25

run_id:
79a49ab4-a6ae-5332-95bf-92f7541beeec
```

Authoritative event history showed:

```text
seq 46:
start_retrying -> running(A)

seq 47:
running -> controller_degraded
reason=interior_gap

seq 48:
controller_degraded -> running(A)
same live runtime, same run_id
```

Later finalization failed:

```text
stopping -> evidence_degraded
reason=unknown_run_lineage_unavailable
```

Root cause:

The coordinator intentionally allowed `controller_degraded -> running` recovery with the same still-live `run_id`, while the lineage validator treated any repeated `run_id` among `to_state='running'` events as `DuplicateRunId`.

That was an internal contract contradiction.

Patch `f0e16651...` changes lineage semantics so same-live-run controller recovery does not manufacture a second logical runtime-generation member.

---

## 5. Patch Proof Status

Agent-reported focused result:

```text
14 pure lineage tests passed
```

Required negative controls reported covered:

1. initial start + same-run controller recovery => lineage `[A]`;
2. real recovery to new run => lineage `[A,B]`;
3. true duplicate runtime-generation run ID remains rejected;
4. current-run mismatch remains rejected;
5. production same-run recovery no longer fails solely because the same run was restored to running.

`rustfmt --check`:

```text
PASS
```

`git diff --check`:

```text
PASS
```

No push performed.

---

## 6. Known Test-Environment Blocker

The local test Postgres at:

```text
127.0.0.1:5434 / mqk_test
```

currently reports migration checksum drift:

```text
migration 6 was previously applied but has been modified
```

This blocks the DB-backed tests in the focused lineage scenario, including the new DB-backed `h03` proof.

Agent reported:
- 26 non-DB tests passed;
- 22 DB-backed tests failed identically on the migration-checksum drift;
- the DB-backed regression test compiles but did not execute successfully.

Do not claim DB-backed proof passed.

Do not modify historical migrations merely to make the tests green.

The test-DB environment issue is separate from the same-run lineage code invariant.

---

## 7. Current Paper Runtime Truth Before Recovery

Last captured Paper truth:

```text
environment=paper
adapter_id=alpaca
live_routing_enabled=false

runtime_status=halted
kill_switch_active=true
integrity_halt_active=true

reconcile_status=ok
mismatched_positions=0
mismatched_orders=0
mismatched_fills=0
unmatched_broker_events=0

current run status=HALTED
OMS outbox rows=0
OMS inbox rows=0
```

The canonical ops catalog reported:

```text
clear-halted-run = enabled
```

The current operation was:

```text
state=evidence_degraded
reason=unknown_run_lineage_unavailable
```

Market data itself was fresh when last checked:

```text
AAPL / 5m
completed_rows=9916
latest completed bar=2026-09-15T14:45:00Z
freshness=OK
```

Required-universe had shown short oscillations around new 5-minute boundaries:

```text
ready
-> expected_latest_bar_missing
-> ready
```

These may be provider-publication timing or a separate scheduler-timing issue. Do not weaken freshness gates without proof.

---

## 8. Session Window Truth

Observed current Paper environment:

```text
MQK_SESSION_START_HH_MM=13:30
MQK_SESSION_STOP_HH_MM=20:00
session_window_source=fixed_window_override
```

Do not silently edit `.env.local`.

If these overrides are unintended, prove that before changing them.

---

## 9. Immediate Next Mission

### Mission: independently review and operationally prove `f0e16651...`

Next actions, in order:

1. independently inspect the exact diff for commit `f0e16651...`;
2. confirm it implements only the intended same-run-lineage invariant;
3. decide whether the blocked DB-backed `h03` proof is required before acceptance or whether existing production RED + focused pure regression proof is sufficient for local acceptance;
4. if accepted, push only after explicit operator authorization;
5. deploy/restart the repaired daemon only as required;
6. use canonical Paper recovery:
   - inspect halt/run/reconcile/OMS truth;
   - `clear-halted-run` only if still enabled and safe;
   - re-arm;
   - allow canonical autonomous start/recovery;
7. return immediately to Paper monitoring.

No new soak campaign.

No new worktree.

No broad test campaign.

---

## 10. Paper Stability Exit Criteria

M1 exit readiness no longer requires 10 days.

Success means the core Paper path stays healthy continuously for at least 20 minutes, or until market close if less than 20 minutes remain, with:

```text
daemon reachable
mode=paper
adapter=alpaca
live_routing_enabled=false

kill_switch_active=false
integrity_halt_active=false
reconcile=ok

operation not manual_intervention_required
operation not controller_degraded
operation not evidence_degraded

runtime RUNNING while in session window
completed-bar task in applicable running-dispatch mode
completed-bar progress advances across completed 5m bars
strategy evaluation count increases

required-universe ready or only bounded self-healing provider-publication waits

no unresolved OMS authority issue
no unexpected task death
no deterministic blocker remaining
```

Orders/fills are **not required** for this stability proof if the strategy truthfully emits no trade.

A truthful no-trade decision is not an infrastructure failure.

---

## 11. After M1 Exit

Once the Paper core path is stable:

1. stop adding infrastructure;
2. close M1 with the soak recorded as operator-waived;
3. expand the Paper universe;
4. move to real strategy evaluation / alpha work;
5. validate actual Paper trading behavior.

Frozen first expanded Paper-universe target discussed:

```text
SPY
QQQ
NVDA
TSLA
AAPL
```

Do not encode that as a comma-separated `MQK_STRATEGY_SYMBOL`; the current legacy env path is single-symbol.

Use the repo's approved multi-symbol/watchlist path only after the remaining production wiring/registry restrictions are repaired.

---

## 12. Current Workflow Rules

Follow:

```text
ONE WRITER
ONE BLOCKER
ONE PATCH
ONE TARGETED PROOF
ONE COMMIT
RETURN TO PAPER
```

Production failures count as RED evidence.

Do not:
- create another worktree by default;
- run redundant standalone builds;
- run `cargo test --workspace` during blocker repair;
- broaden into GUI/infrastructure work;
- start another multi-day soak;
- touch `smoke_logs/`;
- enable Live.

---

## 13. Next Response Should Start With

```text
VERIFIED HEAD:
CURRENT BLOCKER:
NEXT ACTION:
```

Then act on the smallest load-bearing next step.
