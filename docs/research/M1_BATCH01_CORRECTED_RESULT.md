# M1 Batch 01 — corrected-protocol reevaluation result

Mission V4-M1-BENCHMARK-V2-REVIEW-AND-REEVALUATION-01. Declaration: `research-py/experiments/m1_native_trend_campaign/PREDECLARED_BATCH_01_CORRECTED.json` (commit `ec5d5d1d`, committed before any corrected result).

**Verdict: BATCH_REJECTED (corrected protocol).** 15 of 15 evaluations attempted and succeeded as attempts; 0 `paper_candidate`; no promotion, no Paper, no Live. Final holdout RESERVED / UNCONSUMED.

## Protocol

- Signal stream `native_strategy_signal_stream_v2`; economics `native_exact_target_qty_v1` (exact absolute target quantity, executable targets 0 or +1 share); capital basis USD 100,000 shared by Research, Backtest, scanner and Benchmark V2.
- Review alpha: Benchmark V2 `capital_matched_exact_target_buy_hold_v1` (`min_alpha_pct` = 0 unchanged). The legacy fully-invested return is informational only.
- Execution fidelity (exact quantity agreement, floor 0.95): 1.000 on every evaluated trial.

## Population and judge

- predeclared evaluations / registered unique trials: 15; attempted: 15 (attempt 1 each); succeeded: 15; economically evaluable: 14.
- Excluded by the canonical judge: 1 — `623eb9dd037f73f9f6b022a0895a5374` (degenerate_returns:zero_variance_returns) (trend_pullback_5d_4pct_hold5 / IEF never took a position: zero-variance returns; deterministic from the data, not a selection).
- Raw unique trial count 14; effective independent trial count 12.3629 (average pairwise correlation 0.1259).
- PBO: 0.2024 (14 candidates, 252 combinations, 10 blocks). Judge status: `evaluated`. Judge sha256: `8986c8b4ff510103cc9fa4fe57cb239d09a282733043be711a9ebe3df08083b4`.
- The 15 corrected evaluations are ONE disclosed current selection population. The superseded v1 evaluations (15 trials, historical, `NOT_PROMOTION_AUTHORITY`) are disclosed separately and are not counted as additional independent hypotheses.

## Result table (all 15)

Returns are fractions (net/gross/CAGR/maxDD) from the Research economic aggregate; V2/alpha/legacy are percent.

| # | strategy | sym | trial_id | att | qty | net | gross | Sharpe | DSR | CAGR | maxDD | PF | prof.mo | V2 ret% | alpha V2% | legacy bench% (info) | agree | trades | cost drag | review | reasons |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | absolute_momentum_252 | SPY | `2b01ef68` | 1 | 1.0 | 0.0024 | 0.0038 | 0.44 | 0.116 | 0.0002 | -0.0014 | 1.33 | 0.56 | 0.484 | -0.295 | 299.8 | 1.000 | 11 | 0.0014 | rejected | non_positive_alpha |
| 2 | absolute_momentum_252 | EFA | `fca80576` | 1 | 1.0 | 0.0001 | 0.0004 | 0.16 | 0.017 | 0.0000 | -0.0004 | 0.32 | 0.45 | 0.059 | -0.056 | 145.9 | 1.000 | 25 | 0.0002 | rejected | non_positive_alpha |
| 3 | absolute_momentum_252 | IEF | `f22c7e0f` | 1 | 1.0 | -0.0000 | 0.0001 | -0.02 | 0.004 | -0.0000 | -0.0002 | 0.61 | 0.26 | 0.012 | -0.013 | 15.3 | 1.000 | 22 | 0.0002 | rejected | negative_total_return |
| 4 | absolute_momentum_252 | VNQ | `72c2657e` | 1 | 1.0 | -0.0005 | 0.0001 | -0.51 | 0.000 | -0.0000 | -0.0006 | 0.09 | 0.35 | 0.035 | -0.115 | 80.0 | 1.000 | 42 | 0.0006 | rejected | negative_total_return |
| 5 | absolute_momentum_252 | GLD | `70406eb9` | 1 | 1.0 | 0.0025 | 0.0031 | 0.65 | 0.299 | 0.0002 | -0.0010 | 0.43 | 0.37 | 0.372 | -0.127 | 369.1 | 1.000 | 33 | 0.0006 | rejected | non_positive_alpha |
| 6 | near_high_momentum_252_3pct | SPY | `d966f7e1` | 1 | 1.0 | -0.0014 | 0.0018 | -0.40 | 0.000 | -0.0001 | -0.0017 | 0.39 | 0.33 | 0.485 | -0.846 | 299.8 | 1.000 | 61 | 0.0033 | rejected | negative_total_return |
| 7 | near_high_momentum_252_3pct | EFA | `b1c6a629` | 1 | 1.0 | -0.0001 | 0.0003 | -0.15 | 0.001 | -0.0000 | -0.0004 | 0.39 | 0.28 | 0.060 | -0.086 | 145.9 | 1.000 | 50 | 0.0004 | rejected | negative_total_return |
| 8 | near_high_momentum_252_3pct | IEF | `50504dfd` | 1 | 1.0 | -0.0002 | 0.0001 | -0.76 | 0.000 | -0.0000 | -0.0003 | 0.38 | 0.17 | 0.012 | -0.042 | 15.3 | 1.000 | 53 | 0.0004 | rejected | negative_total_return |
| 9 | near_high_momentum_252_3pct | VNQ | `1bac297a` | 1 | 1.0 | -0.0006 | 0.0001 | -1.05 | 0.000 | -0.0001 | -0.0007 | 0.13 | 0.17 | 0.036 | -0.145 | 80.0 | 1.000 | 61 | 0.0007 | rejected | negative_total_return |
| 10 | near_high_momentum_252_3pct | GLD | `07edd21c` | 1 | 1.0 | -0.0003 | 0.0015 | -0.10 | 0.001 | -0.0000 | -0.0009 | 0.52 | 0.17 | 0.372 | -0.506 | 369.1 | 1.000 | 69 | 0.0018 | rejected | negative_total_return |
| 11 | trend_pullback_5d_4pct_hold5 | SPY | `2bedf2c1` | 1 | 1.0 | -0.0008 | 0.0005 | -0.45 | 0.000 | -0.0001 | -0.0010 | 0.13 | 0.03 | 0.495 | -0.643 | 299.8 | 1.000 | 13 | 0.0013 | rejected | negative_total_return |
| 12 | trend_pullback_5d_4pct_hold5 | EFA | `946a80d9` | 1 | 1.0 | 0.0001 | 0.0001 | 0.54 | 0.177 | 0.0000 | -0.0000 | 2.41 | 0.04 | 0.060 | -0.056 | 145.9 | 1.000 | 6 | 0.0001 | rejected | non_positive_alpha |
| 13 | trend_pullback_5d_4pct_hold5 | IEF | `623eb9dd` | 1 | n/a | 0.0000 | 0.0000 | n/a | n/a | 0.0000 | 0.0000 | n/a | 0.00 | n/a | n/a | n/a | 1.000 | 0 | 0.0000 | blocked | not_candidate_ranked |
| 14 | trend_pullback_5d_4pct_hold5 | VNQ | `7c85434a` | 1 | 1.0 | -0.0003 | -0.0001 | -0.73 | 0.000 | -0.0000 | -0.0003 | 0.00 | 0.03 | 0.036 | -0.078 | 80.0 | 1.000 | 10 | 0.0002 | rejected | negative_total_return |
| 15 | trend_pullback_5d_4pct_hold5 | GLD | `472cac40` | 1 | 1.0 | 0.0001 | 0.0008 | 0.06 | 0.008 | 0.0000 | -0.0004 | 0.58 | 0.06 | 0.362 | -0.390 | 369.1 | 1.000 | 15 | 0.0007 | rejected | negative_total_return |

## Robustness / stress per trial

| strategy | sym | robustness failures | not applicable | stress failures |
|---|---|---|---|---|
| absolute_momentum_252 | SPY | month_year_regime_concentration | symbol_leave_one_out | cost_stress_3x |
| absolute_momentum_252 | EFA | month_year_regime_concentration | symbol_leave_one_out | cost_stress_2x, cost_stress_3x |
| absolute_momentum_252 | IEF | month_year_regime_concentration, placebo_temporal_offset | symbol_leave_one_out | none |
| absolute_momentum_252 | VNQ | month_year_regime_concentration, placebo_temporal_offset | symbol_leave_one_out | none |
| absolute_momentum_252 | GLD | month_year_regime_concentration | symbol_leave_one_out | none |
| near_high_momentum_252_3pct | SPY | month_year_regime_concentration, placebo_temporal_offset | symbol_leave_one_out | none |
| near_high_momentum_252_3pct | EFA | placebo_temporal_offset | symbol_leave_one_out | none |
| near_high_momentum_252_3pct | IEF | month_year_regime_concentration, placebo_temporal_offset | symbol_leave_one_out | none |
| near_high_momentum_252_3pct | VNQ | month_year_regime_concentration, placebo_temporal_offset | symbol_leave_one_out | none |
| near_high_momentum_252_3pct | GLD | month_year_regime_concentration, placebo_temporal_offset | symbol_leave_one_out | none |
| trend_pullback_5d_4pct_hold5 | SPY | month_year_regime_concentration | symbol_leave_one_out | none |
| trend_pullback_5d_4pct_hold5 | EFA | month_year_regime_concentration | symbol_leave_one_out | cost_stress_2x, cost_stress_3x |
| trend_pullback_5d_4pct_hold5 | IEF | placebo_temporal_offset, dsr_pbo_sensitivity, genuine_shuffled_placebo | symbol_leave_one_out | none |
| trend_pullback_5d_4pct_hold5 | VNQ | month_year_regime_concentration, placebo_temporal_offset | symbol_leave_one_out | none |
| trend_pullback_5d_4pct_hold5 | GLD | month_year_regime_concentration | symbol_leave_one_out | none |

## Reading

Every candidate holds exactly one share against a USD 100,000 account, so absolute returns are a few hundredths of a percent. Against the capital-matched benchmark (the same one share bought at the first eligible bar and held), no candidate beats passive exposure on any symbol: alpha vs Benchmark V2 is negative in all 14 evaluable rows. The corrected rejection does not depend on the legacy benchmark (whose raw price returns are shown only for information). Nothing was tuned, retried or re-selected.

## Identity map (candidate run / benchmark run)

| strategy | sym | trial_id | candidate_run_id (scanner) | benchmark_run_id | backtest_run_id |
|---|---|---|---|---|---|
| absolute_momentum_252 | SPY | `2b01ef68f7b042d4f72b9789bf5d5a96` | `fbc61d91-3be0-53ab-bab1-e537d794ce6a` | `4d26f01e-5cb9-568d-bf07-3555b5bec723` | `18ffde4b-91b8-5525-a9ff-ca2596adc8d2` |
| absolute_momentum_252 | EFA | `fca805768871bebdf3344b4ae456eda4` | `88fde47b-855c-5c79-a5a4-d269139aa460` | `3fe26dcd-ee48-506e-b756-801c2a9e8165` | `dc1d0df5-271c-5393-be96-cacd6f1bc702` |
| absolute_momentum_252 | IEF | `f22c7e0f76fb769f9eee0e9b0aaf99ec` | `d65d083a-3c49-58ab-b1c6-da3f0fb5c027` | `a50d2677-722b-5267-883b-f5a8f485eea5` | `e77b6012-e160-5e78-83f0-22e9ef42d2af` |
| absolute_momentum_252 | VNQ | `72c2657ecdbd4b3508240656c115d406` | `bd25bcfb-5e1d-5e49-bd8d-97c295c2c14a` | `9afd0116-0fed-5696-9a5d-9823401e184f` | `ca3edbe6-0a4a-5201-b08f-64cf502570ea` |
| absolute_momentum_252 | GLD | `70406eb905999866759c69cad508ef96` | `c35fcf1a-0e1c-5747-b696-5b9cc89484a9` | `5ae28c04-e813-58b8-ab21-d46d3921960a` | `735ffd54-5064-5551-b72b-a551389d8e41` |
| near_high_momentum_252_3pct | SPY | `d966f7e17d8d1373d1790391391aef1f` | `da2eb754-8615-5827-993f-57f2bbb764f1` | `4d26f01e-5cb9-568d-bf07-3555b5bec723` | `98779f1c-7dd0-5ba4-9a23-b47e8a33fcfd` |
| near_high_momentum_252_3pct | EFA | `b1c6a629a4ff7937b57dfdccab57b359` | `2453ba5e-bf91-5303-ac09-771d084f1246` | `3fe26dcd-ee48-506e-b756-801c2a9e8165` | `814d5ba3-ab09-5b8e-903b-8c20c8eb4b8e` |
| near_high_momentum_252_3pct | IEF | `50504dfdf2f997f1d4aeb2650bc1f3ce` | `27f69ed0-9585-564a-a4fb-b8f7187e8a99` | `a50d2677-722b-5267-883b-f5a8f485eea5` | `083dcbb1-1435-5b10-85e8-13446a631a70` |
| near_high_momentum_252_3pct | VNQ | `1bac297a54f29dc888b2aecdff280035` | `77b2d6af-b2e4-591e-aca5-b1a00722cf00` | `9afd0116-0fed-5696-9a5d-9823401e184f` | `7addbf36-582f-5970-b82d-1da072ec5307` |
| near_high_momentum_252_3pct | GLD | `07edd21c9dc0b5f2e0ef0bc50fe1cfd3` | `475bf267-3401-5289-ae0e-677482b05311` | `5ae28c04-e813-58b8-ab21-d46d3921960a` | `584d2aac-4814-5471-b937-63494f24b1b3` |
| trend_pullback_5d_4pct_hold5 | SPY | `2bedf2c12dc667603c99da1f4452267c` | `f6e34c63-1dbf-5caf-b9ed-8addf7179d78` | `4d26f01e-5cb9-568d-bf07-3555b5bec723` | `2f983566-64ee-5213-af1c-ff13de6b2a07` |
| trend_pullback_5d_4pct_hold5 | EFA | `946a80d91f605885be34aae83dad1419` | `566563e8-0d39-51be-8a72-0c09668ab569` | `3fe26dcd-ee48-506e-b756-801c2a9e8165` | `df42297e-6d18-5132-b9d4-236e14c7506c` |
| trend_pullback_5d_4pct_hold5 | IEF | `623eb9dd037f73f9f6b022a0895a5374` | `n/a` | `n/a` | `16704086-d6d9-5e48-9803-ec604808b993` |
| trend_pullback_5d_4pct_hold5 | VNQ | `7c85434a3e474c09f58fa78e05365519` | `7e8a64ca-60e1-5e79-b6aa-5e045f87429b` | `9afd0116-0fed-5696-9a5d-9823401e184f` | `7054a199-53f7-5006-88e0-61177d2033d8` |
| trend_pullback_5d_4pct_hold5 | GLD | `472cac40c0c0f683fa06c2cb33675b50` | `b46b2096-650e-519f-b8fc-fc82539b98f1` | `5ae28c04-e813-58b8-ab21-d46d3921960a` | `81357d68-07e7-5540-b9a8-19d7e8926d3e` |

## Chronology

Declaration commit `ec5d5d1d` 16:04:18 < runner commit `01a4586f` 16:05:13 < registration of 15 trials (0 attempts) ~16:05:51 < first emission 16:05:58 < first corrected economic result 16:06:03 (local clock, -10:00).

## Historical v1 (unchanged)

`runs/run_batch_01` (old registry, judge, economics, scan, reviews) was not modified. It remains HISTORICAL / SUPERSEDED_PROTOCOL / NOT_PROMOTION_AUTHORITY (see `M1_BATCH01_RESULT.md`).

