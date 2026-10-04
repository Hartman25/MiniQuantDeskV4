# M1 native hypothesis Batch 02 — result (`V4-M1-NATIVE-HYPOTHESIS-BATCH-02-01`)

**Verdict: `BATCH_REJECTED`.** 15 trials registered before the first attempt, 15 attempts, 12 evaluable, 3 failed attempts (all H1), one batch-wide judge, **0 `paper_candidate`**, 0 eligible trials, selected trial **NONE**. Local only, not pushed, pending independent review. M1 remains `M1_BLOCKED`.

Predeclaration: `research-py/experiments/m1_native_trend_campaign/PREDECLARED_BATCH_02.json`, commit `368c1f47` — before any engine, calendar code, registration or result. Census and defect dispositions: `M1_BATCH02_CENSUS.md`. Starting HEAD `e4e9dba6a51c7872565bbd929c657058c2097970` (= origin/main, CI #622 green).

## What was frozen and run

| Item | Value |
|---|---|
| Hypotheses | H1 `turn_of_month_last1_first3` (long when the NEXT regular session is the last of its month or ordinal 1–3), H2 `halloween_nov_apr` (long when the next session's month ∈ {11,12,1,2,3,4}), H3 `trading_range_breakout_50d_hold10` (close strictly above the max of the 50 PRIOR closes; long for 10 bars; exactly 60 bars of history) |
| Universe / order | SPY, EFA, IEF, VNQ, GLD × H1, H2, H3 = 15 trials, frozen order, one experiment `M1-NATIVE-HYPOTHESIS-BATCH-02-REAL`, one judge population |
| Calendar | `us_equity_regular_sessions_v1` (`mqk-integrity::sessions`), coverage 2016-01-01..2026-12-31, 105 full closures, content sha256 `3249ee51…76de`, bound into H1/H2 fingerprints |
| Data | the corrected Batch-01 dataset, byte-identical reuse verified (artifact sha256 `100242690c4e…`, 13400 rows, bars hash `db67fd86…`, attestation `a9efd485…`); partition 2016-03-01 start, 12-month folds, 6-month holdout from 2026-03-01 |
| Sizing | `fixed_initial_capital_fraction_v1`, 1000 bps of immutable USD 100,000 = USD 10,000 entry budget, independent safety cap USD 50,000 (non-binding) |
| Half-exposure stress | `half_exposure_capital_fraction_500bps_v1`: 500 bps of the same capital = USD 5,000 budget, a RECOMPUTED target quantity replayed through the exact-target machinery (not a cap; an evaluation scenario of the same trial) plus P7A 15 bps slippage / 10 bps volatility, drawdown ceiling 0.40 |
| Economics | `native_strategy_signal_stream_v2`, `native_exact_target_qty_v1`, `rust_conservative_bar_range_v1` (slippage 5 bps), commission 10 bps/side |
| Benchmark / review | `capital_fraction_matched_passive_buy_hold_v1`; default `StrategyScanReviewPolicy` (min_alpha 0, min_trades 5, max DD 25%, PF ≥ 1.05) |
| Promotion thresholds | unchanged (Sharpe 0.5, MDD 0.25, CAGR 0, PF 1.05, profitable months 0.4, DSR 0.5, PBO 0.5) |

## Trials, attempts and identities

15 registered / 0 attempts at the registration gate (`registration_proof_before_first_attempt.json`); then 15 attempts (12 succeeded, 3 failed), no retry, no infrastructure failure.

| # | strategy | symbol | trial_id | wrapped fingerprint (12) | economic |
|---|---|---|---|---|---|
| 1 | turn_of_month_last1_first3 | SPY | `94ad51a56a55b7794f4c79a8b99257e4` | `f537f07a5460` | FAILED (engine halted) |
| 2 | turn_of_month_last1_first3 | EFA | `ed6375c7dbb9190230d93215c218615b` | `984e1ab3c81f` | FAILED (engine halted) |
| 3 | turn_of_month_last1_first3 | IEF | `52856338e8470e5632a62bb519342540` | `adb839f816c3` | succeeded |
| 4 | turn_of_month_last1_first3 | VNQ | `1530988e724bf8b5c456e5f9eded1f83` | `8bb665543915` | FAILED (engine halted) |
| 5 | turn_of_month_last1_first3 | GLD | `aad2699c575f86f351d3ab3198b74069` | `f623bc49e912` | succeeded |
| 6 | halloween_nov_apr | SPY | `4fcdef753d8abf40ae9664e45e14863c` | `0f1f9b08a4e9` | succeeded |
| 7 | halloween_nov_apr | EFA | `af2c25194e9e2c1c49f74df6af6bc095` | `a1ed07ef23c8` | succeeded |
| 8 | halloween_nov_apr | IEF | `de0afbdf942d330f9d4ed03b19a8f1ba` | `095a0b15ab4c` | succeeded |
| 9 | halloween_nov_apr | VNQ | `28fab008841b32cb309ecdd7ad860429` | `95ff8b19dbbf` | succeeded |
| 10 | halloween_nov_apr | GLD | `625fd0bbce700949ea29522f6ccaa58c` | `7a023c6947e7` | succeeded |
| 11 | trading_range_breakout_50d_hold10 | SPY | `e346f38f083e81ec10d3bd9f198fcb15` | `e9270352c632` | succeeded |
| 12 | trading_range_breakout_50d_hold10 | EFA | `8c8943568fe9a837f7c78c75bee5490c` | `423d0e80a0af` | succeeded |
| 13 | trading_range_breakout_50d_hold10 | IEF | `55a4a99cce7de29620bf2dcc2c13669f` | `f33f588b7144` | succeeded |
| 14 | trading_range_breakout_50d_hold10 | VNQ | `7a701f8710233cdc9ac0fc5026ef7c41` | `fa9e1da47a0c` | succeeded |
| 15 | trading_range_breakout_50d_hold10 | GLD | `39ca04a41b4840c1c4ccab712178c0ff` | `a60af75f7fc2` | succeeded |

The three failed attempts are genuine economic failures finalized failed and never retried: the canonical Backtest engine halts on `MaxDrawdownBreached` (18% of equity), after which the emitted native stream stops and the Research fold has no signals (`no native signals inside fold 9/10/7`). A diagnostic, non-evidence `backtest csv` of the same bars confirmed the cause (SPY −18.06%, profit factor 0.047; VNQ −18.01%, profit factor 0.033). Transaction costs under the conservative bar-range pricing exceed the gross edge of a ~122-fill strategy.

## Multiple testing

Judge `research_multiple_testing_judge_v1`, whole experiment, `judge_sha256` `30fc4ad4…4532`: population 15 registered / 15 attempted / 12 evaluable; the 3 failed trials stay in the registry population as `excluded: no_successful_attempt`. PBO **0.0913** (block count 10, 12 candidates, 252 combinations); effective independent trials **10.23**; best DSR **0.0159** (H2 GLD), next 0.0063 (H2 EFA); **no trial reaches the 0.5 minimum**. Block-count sensitivity (8/10/12): DSR range 0.000, PBO range 0.042 (ceilings 0.15) — passes for every evaluable trial.

## Robustness, benchmark, scanner/review

* **Half-exposure stress (500 bps recomputed Q): passed for all 12** (stressed max drawdown −0.9% … −6.0% vs the −40% ceiling), evidence `is_a_trial=false`, `caps_unchanged_from_baseline=true`, stress stream authenticated against the baseline decisions.
* **Genuine shuffled placebo: passed for all 12** (the real signal beat the shuffled one in each case). DSR/PBO sensitivity: passed for all 12.
* Rust gauntlet (pass 2, see "Harness defect" below): `month_year_regime_concentration` fails for all 10 H2/H3 trials (a single regime holds > 50% of the positive gain); `placebo_temporal_offset` fails for 4 (H1 IEF, H1 GLD, H3 GLD, H3 VNQ), `execution_delay_stress` for 1 (H1 GLD), `parameter_neighborhood_execution` for 1 (H1 GLD); native stress suite `cost_stress_2x` fails for 4 and `cost_stress_3x` for 5 trials.
* **Benchmark: negative alpha for every evaluable trial** against the capital-fraction matched passive buy-and-hold (−1.2 … −53.5 percentage points). H2 GLD (the best trial: net +7.7%, profit factor 7.4, Sharpe 0.59) earned +9.75% against a +36.86% passive holding of the same 97 shares.
* **Scanner/review: 0 `paper_candidate`.** 12 `rejected` (7 `negative_total_return`, 5 `non_positive_alpha`) and 3 `rejected: halted` (the failed H1 attempts).

## Promotion eligibility (read-only; no promotion state written)

No trial cleared the gates before the Promotion thresholds (DSR < 0.5 for all 12, review state `rejected` for all 15), so the canonical `evaluate_promotion` was not reached for any candidate and the mechanical conjunction is false for every trial. Descriptive check of the frozen Promotion/Research thresholds on the 12 evaluable trials (no threshold changed): only H2 GLD clears Sharpe ≥ 0.5; none clears profitable months ≥ 0.4 except H3 SPY (0.44); **no trial clears more than 4 of the 6 metric thresholds** (H2 GLD: Sharpe, MDD, CAGR, PF pass; profitable months and DSR fail). No production Promotion transition was written; no daemon, DB, broker or Paper action occurred.

## Descriptive activity (no threshold derives from it)

| trial | trades | % invested | position agreement | gross | net | cost drag |
|---|---|---|---|---|---|---|
| H1/IEF | 122 | 19.1 | 1.000 | 0.002 | −0.077 | 0.080 |
| H1/GLD | 122 | 19.1 | 1.000 | 0.061 | −0.094 | 0.155 |
| H2/SPY | 10 | 49.8 | 1.000 | 0.068 | 0.041 | 0.027 |
| H2/EFA | 10 | 49.8 | 1.000 | 0.083 | 0.063 | 0.020 |
| H2/IEF | 10 | 49.8 | 1.000 | 0.011 | −0.002 | 0.013 |
| H2/VNQ | 10 | 49.8 | 1.000 | 0.036 | 0.008 | 0.028 |
| H2/GLD | 10 | 49.8 | 1.000 | 0.097 | 0.077 | 0.020 |
| H3/SPY | 39 | 55.7 | 1.000 | 0.068 | 0.016 | 0.052 |
| H3/EFA | 49 | 46.3 | 1.000 | 0.058 | 0.004 | 0.055 |
| H3/IEF | 37 | 29.1 | 1.000 | 0.011 | −0.015 | 0.026 |
| H3/VNQ | 51 | 39.0 | 1.000 | −0.023 | −0.106 | 0.083 |
| H3/GLD | 41 | 34.9 | 1.000 | 0.052 | −0.003 | 0.055 |

Position agreement 1.000 for every evaluable trial: Research held exactly the Backtest's capital-fraction quantity at every bar.

## Family and batch outcomes

H1 `FAMILY_REJECTED`, H2 `FAMILY_REJECTED`, H3 `FAMILY_REJECTED`. Batch: **`BATCH_REJECTED`**; selected trial **NONE**. No H4, 10-month-SMA reserve, Batch 02B/03, parameter rescue or follow-up campaign is authorized; any further research needs a new operator decision. This is an economic result, not a software failure.

## Harness defect found during the run (disclosure)

The first real pass (`run_batch_02_pass1_defective_robustness`, preserved locally and in the review package) showed every Rust-side robustness and native-stress scenario failing with "strategy semantic fingerprint mismatch" for all 12 evaluable trials. Cause: the gauntlet/stress-suite identity check compared the UNWRAPPED factory instance with the baseline report's capital-fraction WRAPPER fingerprint, which can never match — a defect in the Native Sizing V1 integration, not an economic result. It was fixed at the shared seam (commit `a360d2c1`: the comparable fingerprint is derived through the one pure wrapper constructor under the baseline config), proven with a scenario test and killed mutations, the binary was rebuilt, and the Backtest/finalize/review downstream stages were re-run (**pass 2**, the evidence above). The Research registry, the 15 economic attempts, the judge and the Research-side stress/placebo were not re-run; no economic attempt was retried. Pass-1 robustness verdicts are VOID; the batch verdict was already determined by the DSR and scanner/review gates and is unchanged.

## Holdout, Paper, Live

Final holdout **RESERVED / UNCONSUMED**: ledger row `reserved`, `consumed_at` null; the post-run guard found no timestamp ≥ 2026-03-01 in native/stress signal generation, research folds, judge inputs, Backtest equity/fills/orders, scanner/benchmark bars, robustness stress or placebo artifacts (latest 2026-02-27). Calendar derivation inspected provider row TIMESTAMPS only (no prices) for dates after 2026-02-28. Paper **NOT ACTIVATED**; production promotion state **NOT WRITTEN**; Live **NOT TOUCHED**; nothing pushed.

## Independent-review disclosure (post-run)

Independent review found that the formal predeclaration `PREDECLARED_BATCH_02.json` was not internally perfect: six descriptive strings were copied from Batch 01 and contradict the Batch 02 contract (old warm-ups, fixed one-share quantity prose, Benchmark V2 capital basis, old run root, five-trial judge wording). The original file remains immutable; a post-run erratum documents it (`M1_BATCH02_PREDECLARATION_ERRATUM.md`). An invariance proof established that none of the six reached any trial id, sizing argument, benchmark policy, economic spec, attempt inventory or selection/holdout input, so Batch 02 economics and candidate identities are unchanged. No economic attempt or result was rerun or changed. **`BATCH_REJECTED` remains the economic outcome.** The independently confirmed Promotion-time stress-sizing binding gap (census C11) is split: its consistency binding is fixed, and the exact required stress allocation is a blocked future Promotion-authority prerequisite that does not concern this rejected batch (`M1_BATCH02_CENSUS.md`).

## M1 status

`M1_BLOCKED`; Batch 01 and Batch 02 `BATCH_REJECTED`; Promotion NONE; Paper INACTIVE. Next exact step: operator decision on whether any further research campaign is wanted (none is authorized); independent ChatGPT review of this controller's commits → at most one correction → push → exact-head CI. M1.9 and M1.10 remain open; M1 is not complete.
