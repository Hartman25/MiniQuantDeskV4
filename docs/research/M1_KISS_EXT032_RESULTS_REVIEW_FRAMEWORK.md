# M1 KISS campaign: results and near-miss review framework

Tool: `research-py/experiments/m1_native_trend_campaign/near_miss_review.py` (schema `m1_near_miss_review_v1`). Tests: `test_kiss_ext032_near_miss_review.py`. Prepared before any execution; there are no results, and the tool refuses the non-executable declaration. It is read-only, deterministic, and selects nothing.

## 1. What it does

For `M1-KISS-EXT032-ETF-01` it reviews **every registered trial** (4), in the declaration's order, whether it succeeded, failed, was excluded by the judge or has no evidence at all (a missing trial is listed as `MISSING_EVIDENCE`, never dropped). Inputs are the evidence table `batch_results.json` (written by `summarize_batch.py`), the batch judge, the declaration, an optional canonical `evaluate_promotion` verdict file (`promotion_eligibility.json`) and the run's provenance facts. It opens no registry, reads no price, writes nothing into the run directory (an `--out` inside it is refused) and makes no network call (audit-hook test).

Per trial: trial and strategy identity, symbol, evidence grade, data partition, evaluability, every quantitative gate with value / comparator / threshold / signed margin / shortfall / relative shortfall, the execution-fidelity gate, robustness and stress outcomes, DSR with PBO and the cumulative search count beside it, reason codes, caveats and the final status. There is no rank, winner, "best near-miss" or recommendation field (`selection` is always `null`).

## 2. Gates and thresholds (unchanged, OD-8)

| Gate | Comparator and threshold | Source |
|---|---|---|
| cost-aware alpha vs `capital_fraction_matched_passive_buy_hold_v1` | alpha % >= 0 | benchmark evidence |
| total return | >= 0 % | canonical Backtest |
| Sharpe | >= 0.5 | Research walk-forward aggregate (screening proxy) |
| DSR | >= 0.5 | batch judge |
| PBO | <= 0.5 | batch judge |
| CAGR | >= 0 | Research walk-forward aggregate (screening proxy) |
| maximum drawdown | <= 25 % | canonical Backtest |
| profit factor | >= 1.05 | canonical Backtest |
| profitable months | >= 0.40 | Research daily returns (screening proxy) |
| trade count | >= 5 | canonical Backtest |
| execution fidelity | >= 0.95 | simulator vs native target agreement |

Equality passes (the evaluators fail on strict `<` / `>`). An unavailable or non-finite value fails closed with no distance. The canonical Promotion metrics (Sharpe, CAGR, drawdown, profit factor, profitable months) come from `evaluate_promotion` on the Backtest equity curve; the three proxies are labelled as such, and a trial cannot reach `QUALIFIES` without a passing canonical verdict. The scanner constants are pinned to the Rust `StrategyScanReviewPolicy::default` by test.

## 3. Final status (first match wins)

1. `INVALID_EXECUTION_IDENTITY_OR_SAFETY_EVIDENCE`: fingerprint or trial id differs from the declaration, duplicate rows, execution fidelity below the floor or unavailable, halted backtest, required robustness evidence missing, a required metric unavailable, or a scanner review state inconsistent with its own metrics.
2. `STRUCTURALLY_UNQUALIFIED_OR_NON_EVALUABLE`: missing row, failed economic attempt, judge exclusion, judge or PBO not evaluable, a robustness or native stress scenario failed.
3. `QUANTITATIVE_ECONOMIC_NEAR_MISS`: evaluable and valid, and every failed gate is quantitative; all failed gates are listed with their distances. No "near" band is invented: distances are reported, the operator judges.
4. `INSUFFICIENT_PROVENANCE_OR_INDEPENDENT_VALIDATION`: every gate passes but the bars provenance manifest or the post-run holdout-guard report is absent or negative, or the canonical Promotion verdict is missing or negative.
5. `QUALIFIES_UNDER_EVERY_APPLICABLE_GATE`: every gate passes, the verdict passed, provenance present.

`QUALIFIES` is a statement about the unchanged economic gates on **exposed development** data. Every row carries `EXPOSED_DEVELOPMENT_NOT_INDEPENDENT` and `CUMULATIVE_SEARCH_NOT_DEFLATED` (119 disclosed trials; the four-trial judge is not deflated for them). A near-miss is a measurement, not a candidate; at most one focused follow-up hypothesis may be authorized in a separately declared campaign with a new semantic identity and honest accounting, and relabelling a retest never makes the exposed window fresh.

## 4. Population completeness

The report states `population.status` = `COMPLETE` only when exactly the four declared trials have exactly one row each; duplicates and unexpected rows are listed and make the population `UNRESOLVED`, never a rejection.

## 5. Not covered

No row exists to review; the real field values will be produced by the execution stages after authorization. The Promotion verdict file has no producer in the repository (the canonical `evaluate_promotion` runs in the daemon promotion route), so its operator-supplied format is `{trial_id: {"passed": bool}}` exactly as `select_batch.py` already consumes it.
