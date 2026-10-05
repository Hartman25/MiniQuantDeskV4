# M1 Batch 03 Discovery Result (V4-BATCH03-DISCOVERY-THEN-CAPITAL-FRACTION-DAEMON-CLOSURE-01, Phase A)

**Status: `BATCH03_DISCOVERY_EXECUTED`.** 60 prospective Discovery trials (10 families x 6 ETFs) were executed exactly once each under the frozen `PREDECLARED_BATCH_03.json`. This is Discovery evidence only: no Confirmation trial was registered or run, the final holdout (begins 2026-03-01) is `reserved_not_evaluated` / UNCONSUMED (`holdout_guard post` clean), no Promotion transition or Promotion candidate exists, Paper is inactive and Live is untouched. `runs/` is git-ignored; the evidence below is reproduced from `runs/run_batch_03/{batch_results.json,family_ranking.json,judge/judge.json}`.

## Registration and attempts

- Registry: 10 hypotheses, 60 trials, 60 attempts (exactly one attempt per trial, attempt_index 1). No retries; no infrastructure failure occurred.
- Success / failure: 53 attempts produced an economic result; 7 failed deterministically with `no native signals inside fold N` and were NOT retried (outcome-dependent retry is not authorized): F02 x4 (IWM f10, SMH f9, XBI f7, XLE f9), F03 x3 (SMH f9, XBI f10, XLE f10). They stay in the population as non-evaluable (worst on every ranking key).
- Evaluable: F01, F04-F10 = 6/6 each; F02 = 2/6; F03 = 3/6.
- Run notes: scanner review needed an identity row for XBI, which is absent from `config/instruments/equities.json`; a per-run row was supplied in `run_batch.py` (`SCAN_REGISTRY_SUPPLEMENT`; identity only, no bars or economics).

## Batch-wide judge (`research_multiple_testing_judge_v1`, one judge over all 60)

- judge sha256 `4d38c99141a535d131a35012ba3f6d5ca5cfbe841ac3b6b414bedc9186cf788f`; 53 included, 7 excluded (`no_successful_attempt`); PBO = 0.0714 (252 combinations, 10 blocks); effective independent trial count 39.53 of 53 raw; judge holdout status `reserved_not_evaluated`.
- Scanner review: 60/60 `rejected` (0 `paper_candidate`). Positive cost-aware benchmark alpha: 0/60.

## Per-family six-slot dispositions

Cell = net alpha vs capital-fraction matched passive benchmark (%) / canonical DSR / review state; `FAILED` = failed attempt (not retried).

| family | SPY | QQQ | IWM | SMH | XBI | XLE |
|---|---|---|---|---|---|---|
| F01 monthly_multihorizon_abs_momentum_consensus_v1 | -19.3 / 0.026 / rejected | -30.4 / 0.031 / rejected | -13.4 / 0.000 / rejected | -89.7 / 0.024 / rejected | -17.1 / 0.000 / rejected | -10.5 / 0.001 / rejected |
| F02 trend_filtered_rsi5_reversion_v1 | -39.9 / 0.000 / rejected | -62.6 / 0.000 / rejected | FAILED | FAILED | FAILED | FAILED |
| F03 trend_filtered_extreme_3d_atr_reversal_v1 | -40.7 / 0.000 / rejected | -63.6 / 0.000 / rejected | -28.3 / 0.000 / rejected | FAILED | FAILED | FAILED |
| F04 close_channel_100_50_trend_v1 | -23.0 / 0.039 / rejected | -40.7 / 0.032 / rejected | -16.3 / 0.000 / rejected | -128.9 / 0.092 / rejected | -15.3 / 0.000 / rejected | -15.3 / 0.000 / rejected |
| F05 monthly_10month_trend_timing_v1 | -17.5 / 0.045 / rejected | -28.4 / 0.077 / rejected | -12.4 / 0.000 / rejected | -88.9 / 0.058 / rejected | -10.7 / 0.000 / rejected | -1.7 / 0.004 / rejected |
| F06 trend_filtered_zscore20_reversion_v1 | -35.9 / 0.000 / rejected | -53.3 / 0.000 / rejected | -21.9 / 0.000 / rejected | -133.6 / 0.000 / rejected | -24.4 / 0.000 / rejected | -22.3 / 0.000 / rejected |
| F07 volatility_contraction_breakout_v1 | -31.5 / 0.000 / rejected | -52.1 / 0.000 / rejected | -21.8 / 0.000 / rejected | -149.9 / 0.001 / rejected | -19.9 / 0.000 / rejected | -24.4 / 0.000 / rejected |
| F08 monthly_12_minus_1_abs_momentum_v1 | -12.3 / 0.049 / rejected | -23.9 / 0.040 / rejected | -16.0 / 0.000 / rejected | -81.1 / 0.022 / rejected | -9.2 / 0.000 / rejected | -10.9 / 0.001 / rejected |
| F09 delayed_overnight_gap_reversal_v1 | -37.6 / 0.000 / rejected | -61.1 / 0.000 / rejected | -24.9 / 0.000 / rejected | -163.4 / 0.000 / rejected | -8.7 / 0.000 / rejected | -26.5 / 0.000 / rejected |
| F10 monthly_52week_high_proximity_v1 | -18.1 / 0.071 / rejected | -34.8 / 0.045 / rejected | -12.3 / 0.000 / rejected | -100.8 / 0.016 / rejected | -13.4 / 0.000 / rejected | -21.4 / 0.000 / rejected |

## Deterministic family ranking (`family_ranking.py`, predeclared keys; no invented threshold)

| rank | family | outcome | evaluable | positive-alpha symbols | median DSR | median net alpha % | robustness-clear | median DD improvement |
|---|---|---|---|---|---|---|---|---|
| 1 | F05 | DISCOVERY_ADVANCED_TO_CONFIRMATION | 6 | 0 | 0.02456 | -14.97 | 0 | 3.217 |
| 2 | F04 | DISCOVERY_ADVANCED_TO_CONFIRMATION | 6 | 0 | 0.01596 | -19.66 | 0 | 4.116 |
| 3 | F01 | DISCOVERY_ADVANCED_TO_CONFIRMATION | 6 | 0 | 0.01272 | -18.22 | 0 | 0.8701 |
| 4 | F08 | DISCOVERY_NOT_ADVANCED | 6 | 0 | 0.01141 | -14.15 | 1 | 2.428 |
| 5 | F10 | DISCOVERY_NOT_ADVANCED | 6 | 0 | 0.008213 | -19.75 | 0 | 4.426 |
| 6 | F07 | DISCOVERY_NOT_ADVANCED | 6 | 0 | 1.169e-05 | -27.96 | 0 | 3.057 |
| 7 | F06 | DISCOVERY_NOT_ADVANCED | 6 | 0 | 8.721e-08 | -30.17 | 1 | 0.7004 |
| 8 | F09 | DISCOVERY_NOT_ADVANCED | 6 | 0 | 0 | -32.03 | 0 | -0.1498 |

F02 and F03 fall below the 4-of-6 evaluable minimum (`DISCOVERY_INSUFFICIENT_EVALUABLE_TRIALS`).

**Advanced to Confirmation: F05, F04, F01** (`DISCOVERY_ADVANCED_TO_CONFIRMATION`). The predeclared ranking admits the top three rankable families without a minimum-alpha or minimum-DSR gate. Honest reading: every family has non-positive benchmark alpha (0/60 positive), the advanced families have median DSR 0.016-0.025, and none cleared robustness in any slot (0 of 18). Advancement is a mechanical ranking outcome, not evidence of an edge, and is not Promotion. `selected_trial_ids` and `promotion_candidates` are empty; `promotion_candidate_created` is false.

## Not done (not authorized)

- Confirmation trials (DIA/MDY/XLF/XLI/XLV/XLP) are NOT registered and NOT run.
- Final holdout NOT consumed.
- No retuning, new variants, Promotion transition, Paper deployment/orders, or Live.
