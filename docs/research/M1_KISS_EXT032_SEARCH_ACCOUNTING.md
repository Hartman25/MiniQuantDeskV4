# M1 KISS campaign: cross-campaign search accounting

Campaign `M1-KISS-EXT032-ETF-01` (1 hypothesis, 4 trials, 0 attempts). Authority: `research-py/experiments/m1_native_trend_campaign/search_accounting.py` (pure; reads only the committed, frozen declarations) and the `search_accounting` block of `PREDECLARED_KISS_EXT032_ETF_01.json`. Pins: `test_kiss_ext032_search_accounting.py`. Operator decision OD-4 (preserve cumulative history; no results-based reduction).

## 1. Definitions (a retry or a re-run never manufactures a trial)

| Term | Meaning here |
|---|---|
| Hypothesis | an economic idea (one `strategy_id` family). This campaign adds exactly 1 |
| Trial | one registered candidate identity: hypothesis x instrument under the frozen protocol. This campaign adds exactly 4 |
| Attempt | one invocation of a trial; a retry is a new attempt of the SAME trial |
| Evaluation | a fold, slice or robustness scenario of a registered trial; never a trial |
| Failed / non-evaluable trial | stays in the population and counts worst; never dropped, never imputed |
| Cross-campaign exposure | prior trials on the same exposed window; disclosed in the cumulative count and never removed after results |

## 2. Verified historical inventory (from the frozen declarations, not from the operator's "about 55 + 60")

All nine declarations share one evaluation window (`2016-03-01`, 12-month folds, 6-month holdout, 10 folds); that is asserted, not assumed.

| Declaration | Experiment | Strategy ids | Symbols | Trial identities | Status | In operative rule |
|---|---|---|---|---|---|---|
| `PREDECLARED_CAMPAIGN.json` | `M1-NATIVE-TREND-CAMPAIGN-01-REAL` | trend_sma50 | 5 | 5 | voided (`INVALID_FOR_STATED_HYPOTHESIS`) | yes |
| `PREDECLARED_CAMPAIGN_02.json` | `M1-NATIVE-TREND-CAMPAIGN-02-REAL` | trend_sma50 | 5 | 5 | amendment of the same hypothesis, rejected | yes |
| `PREDECLARED_CAMPAIGN_03.json` | `M1-NATIVE-TREND-CAMPAIGN-02-REAL` (seeded copy) | trend_sma50 | 5 | 5 | rejected | yes |
| `PREDECLARED_CAMPAIGN_DUAL_SMA_01.json` | `M1-DUAL-SMA-50-200-CAMPAIGN-01-REAL` | dual_sma_50_200_trend | 5 | 5 | rejected | yes |
| `PREDECLARED_CAMPAIGN_PULLBACK_01.json` | `M1-PULLBACK-MEAN-REVERSION-20-2-CAMPAIGN-01-REAL` | pullback_mean_reversion_20_2 | 5 | 5 | rejected | yes |
| `PREDECLARED_BATCH_01.json` | `M1-NATIVE-HYPOTHESIS-BATCH-01-REAL` | 3 (absolute momentum, near-high, trend pullback) | 5 | 15 | superseded v1, re-evaluated by the corrected declaration | **no** |
| `PREDECLARED_BATCH_01_CORRECTED.json` | `M1-NATIVE-HYPOTHESIS-BATCH-01-CORRECTED-REAL` | same 3 | 5 | 15 | rejected | yes |
| `PREDECLARED_BATCH_02.json` | `M1-NATIVE-HYPOTHESIS-BATCH-02-REAL` | 3 (turn-of-month, Halloween, range breakout) | 5 | 15 | rejected | yes |
| `PREDECLARED_BATCH_03.json` | `M1-NATIVE-HYPOTHESIS-BATCH-03-DISCOVERY` | 10 | 6 | 60 | discovery, advanced mechanically, not eligible | yes |

## 3. Three counts, kept apart

| Rule | Prior | Plus 4 new | Meaning |
|---|---|---|---|
| **`REGISTERED_NATIVE_TRIAL_IDENTITIES_EXCLUDING_SUPERSEDED_V1`** (operative) | **115** | **119** | every registered native trial identity except the superseded Batch 01 v1 evaluations (`M1_BATCH01_CORRECTED_RESULT.md`: "not counted as additional independent hypotheses") |
| `ALL_REGISTERED_NATIVE_TRIAL_IDENTITIES_INCLUDING_SUPERSEDED_V1` | 130 | 134 | upper sensitivity bound |
| `UNIQUE_STRATEGY_ID_SYMBOL_PAIRS` | 105 | 109 | lower sensitivity bound: the three trend_sma50 campaigns are re-specifications of the same 5 hypothesis-instrument pairs |

**Finding.** The operator's 115 is verified exactly, but as a *registry-identity* count (25 from five 5-trial campaigns + 15 corrected Batch 01 + 15 Batch 02 + 60 Batch 03), not as a count of strictly unique hypothesis-instrument pairs (105). The operative cumulative count is therefore **119**, the larger and conservative reading, applied without any result input. The 105 / 130 bounds are disclosed so the reader can see the sensitivity; none is used to reduce the denominator.

## 4. Broader same-window research exposure (disclosed, not mixed into the native denominator)

Census-01 (corrected) StrategyEdge 38,192 trials; Census-02 Strategy 9,400 trials and 1,075 conditional factors; Confirmation-01 (window `[2024-01-01, 2026-03-01)` consumed); Pass-2 (924 robustness candidates); Discovery_01 low volatility; SHORT_01 and SHORT_WAVE_02 (12-ETF universe). SPY, QQQ, IWM and DIA are all in the 88-symbol seed universe used by the census-style waves. These are labels, Python-simulator or classifier populations; they are not native Rust trials and are never added to the native count. Their literals are pinned to the evidence documents by test.

## 5. Can the existing judge represent the cumulative accounting? No.

`research_multiple_testing_judge_v1` scopes one experiment and groups trials by a comparison key that binds the bars provenance (symbol universe, extraction range, canonical bars hash), the evaluation spec, annualization, cost model and execution policy. Trials of campaigns with a different universe are excluded as `incompatible_comparison_scope` (proved by test with the real `_comparison_key`); the earlier campaigns also used other protocols and sizing; and their per-trial return series live in git-ignored run directories, not in committed content. Substituting the cumulative count into the new judge would assume that the unobserved prior Sharpe dispersion equals the new batch's. Therefore:

* `pooled_statistics_support = BLOCKED_UNSUPPORTED`; no pooled DSR or PBO is produced or claimed.
* The gate DSR and PBO (unchanged thresholds, OD-8) come from the **new experiment's own four-trial judge**, which is **not deflated for the prior campaigns**. The review report prints `cumulative_disclosed_search_count = 119` and the flag `CUMULATIVE_SEARCH_NOT_DEFLATED` beside every DSR so that a pass is never read as a deflated pass.
* A truthful additive seam exists: the accounting artifact (schema `m1_cross_campaign_search_accounting_v1`), generated from the frozen declarations and pinned by test.

## 6. Promotion contract note

`verify_promotion_oos_evidence` demands walk-forward fold evidence (`folds_used > 0`), the `economic_walk_forward_v1` protocol, the judge and `holdout.status == reserved_not_evaluated`. This research path can supply exactly that, so no Promotion gate is `BLOCKED` by it. It supplies no independent confirmation, none is claimed, and nothing here touches Gate 3b (`active_paper`).

## Addendum: what the numbers establish (V4-M1-KISS-EXT032-ALL-DEFECT-CLOSURE-01)

* **Declared, not registry-verified.** The 130/115/105 prior identities are derived from the frozen declarations and cross-checked against their evidence documents. The historical registries are git-ignored run directories and were not re-read, so "registered" describes the declarations' claim. The accounting artifact carries `identity_basis.independently_verified_in_a_registry = null`.
* **Independent population validation.** `build_accounting` itself rejects a drifted new declaration (missing or duplicate trial slot, unexpected symbol, wrong hypothesis count, wrong `max_trials`, extra variant, wrong order), an omitted or phantom prior campaign, an invalid or contradictory prior status, a missing evidence document, an incompatible evaluation window, and a declared `search_accounting` block that disagrees with the recomputed counts.
* **Distinct statistical states.** `FOUR_TRIAL_JUDGE_RESULT` (the in-experiment judge, not deflated for prior campaigns) is not `CUMULATIVE_SEARCH_DISCLOSED` (the count 119 printed beside every DSR), and neither is `CUMULATIVE_SEARCH_VALIDATED`. With pooled statistics `BLOCKED_UNSUPPORTED` the status is `CUMULATIVE_SEARCH_VALIDATION_BLOCKED`, recorded as statistical acceptance blocker `SAB-1`. The OD-4/OD-8 thresholds are not weakened: no trial of this campaign may be labelled cumulative-search validated or qualifying while SAB-1 stands.
