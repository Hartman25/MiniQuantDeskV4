# SHORT-04-GAP-REVERSAL-01 — Result

Part of `SHORT-RESEARCH-WAVE-02-CONTROLLER`. Development-stage Research
only. Does not authorize Paper or Live shorting. No production Paper/
runtime/broker/risk/portfolio/scheduler behavior was modified.

## 1. Hypothesis

**Truthful name:** pooled single-feature gap-reversal/exhaustion
classifier. An ETF's own prior overnight gap (`gap_pct_1 =
open[t]/close[t-1] - 1`) as the sole predictor of whether that same ETF's
own subsequent 10-bar log return is positive. Hypothesized relation:
negative (unusually positive gaps → potential exhaustion → lower
P(fwd_ret>0); negative gaps → potential rebound → higher P(fwd_ret>0)).
Sign not enforced.

## 2. Data provenance

Shared wave-level fetch (see
[SHORT_WAVE_02_SHORT_02_RESULT.md](SHORT_WAVE_02_SHORT_02_RESULT.md) §2 for
the full manifest — identical bars, `canonical_semantic_bars_hash =
690d36721d86a58b3c045f7d42abbd90fa9cf4d9d960e5d1d07a3e763e522eac`, fetched
once and reused for every hypothesis in this wave).

## 3. Feature isolation

`feature_columns == ["gap_pct_1"]`, always computed by
`build_feature_set_v1`'s **default** `FeatureSetV1Spec` — no
`research-py/src` change was needed. Enforced by
`assert_single_feature_schema`.

## 4. Labels

`fwd_ret = log(close[t+10]/close[t])`, `LABEL_HORIZON_BARS=10`,
`target = 1 iff fwd_ret > 0`. Same shared real-target set as SHORT-02/03.

## 5. Walk-forward

Same frozen spec as SHORT-02/03. All three trials: `folds_used=18/18,
folds_skipped=0`. OOS reference window: `2019-01-02` – `2023-06-30` (1,132
dates, identical across all three trials — verified by
`verify_oos_date_alignment`). `holdout_start_utc=2023-07-01T00:00:00+00:00`.
**Final holdout not consumed.**

## 6. Results

| | Long-only | Long/short | Placebo |
|---|---|---|---|
| `hypothesis_id` | `short04_gap_reversal_long_only_v1` | `short04_gap_reversal_long_short_v1` | `short04_gap_reversal_placebo_v1` |
| `experiment_id` | SHORT-WAVE-02-REAL-CANDIDATES-V1 | SHORT-WAVE-02-REAL-CANDIDATES-V1 | SHORT-WAVE-02-DIAGNOSTIC-PLACEBOS-V1 |
| `trial_id` | `9606018c657995b43db1843b4740e991` | `d17bc45250844dce5e78c5a9e866052d` | `22bfde601a728a57d471da9949e06682` |
| `attempt_id` | `9606018c...:att0001` | `d17bc452...:att0001` | `22bfde60...:att0001` |
| folds used | 18/18 | 18/18 | 18/18 |
| `net_total_return` | 0.42389 | 0.42389 | 0.42389 |
| `net_sharpe` | 0.48404 | 0.48404 | 0.48404 |
| `max_drawdown` | −0.35516 | −0.35516 | −0.35516 |
| `total_turnover` | 3,543,251.54 | 3,543,251.54 | 3,543,251.54 |
| `cost_drag` | 0.26669 | 0.26669 | 0.26669 |
| `active_days` | 1,096 | 1,096 | 1,096 |

**All three trials are execution-identical (bit-for-bit equal aggregate
metrics)**, for the same structural reason as SHORT-02 (§7).

### Paired delta (long/short − long-only)

`delta_net_total_return=0.0, delta_net_sharpe=0.0, delta_max_drawdown=0.0,
delta_turnover=0.0, delta_cost_drag=0.0`.

### Placebo delta (long/short real − placebo)

`delta_net_total_return=0.0, delta_net_sharpe=0.0, delta_max_drawdown=0.0`.

### Fold-reset benchmark (equal-weight 12-ETF basket, same OOS dates)

`cumulative_return_over_reference_dates=0.81899, sharpe=0.73261,
max_drawdown=-0.36288` over the same 1,132 dates (identical benchmark
window to SHORT-03, since both share the 18-fold OOS date sequence;
`dates_with_no_return_observation=[]`, exact alignment PASS).

## 7. Short exposure

`BEARISH_PREDICTION_COUNT=0/13,464` OOS prediction rows in the real
long-short trial (`ml_score <= 0.45` never occurred). Real trial
`ml_score` range: `[0.55011, 0.68874]`, mean `0.61929` — clustered just
above `entry_threshold=0.55` and well above `short_threshold=0.45` for
every OOS row, in every fold. `NEGATIVE_POSITION_SYMBOL_DAYS=0`,
`SYMBOLS_SHORTED=[]`, `FOLDS_WITH_SHORT_EXPOSURE=0/18`.

Verified this is a genuine result, not a caching defect: `targets.csv`
differs between real and placebo (confirmed distinct md5, `51bb3b3a...`
vs `2f4b902b...`), `features.csv` is identical across long-only/long-short
(`4941a0f8...`, confirmed), and `ml_score` differs row-for-row between
the real (`[0.55011,0.68874]`) and placebo (`[0.56043,0.68455]`)
`walk_forward_oos_predictions.csv` files (confirmed not element-wise
equal). As in SHORT-02, the single feature is weak enough that the
base-rate-dominated model output stays entirely above `entry_threshold`
in both the real and placebo trials, so the discrete equal-weight-active
positions — and therefore every downstream P&L figure — end up identical
despite the genuinely different underlying trained models.

## 8. Short-value interpretation

`SHORT_SIDE_VALUE_VERDICT = INCONCLUSIVE` — zero short-eligible
predictions occurred in 13,464 OOS rows across 18 folds; there is no
exercised short exposure to judge as helpful or harmful.

`MODEL_HYPOTHESIS_VERDICT` is deferred to `SHORT-WAVE-02-FAMILY-JUDGE-AND-
CLOSEOUT-01` (Patch E), where the real-candidate-only family DSR/PBO
becomes available across all six real trials (all now registered after
this patch).

## 9. Long/short attribution

`LONG_SHORT_ATTRIBUTION=UNKNOWN_NEEDS_PROOF` — same missing-evidence seam
as SHORT-01/02/03. Moot here regardless, since zero short exposure
occurred.

## 10. Borrow-model caveat

`BORROW_MODEL=research_assumed_shortable_universe_v1`. Same limitation as
SHORT-02 §10.

## 11. Safety confirmation

`PRIMARY_PAPER_REPO_UNTOUCHED=YES`, `PAPER_DB_MUTATED=NO`,
`PAPER_RUNTIME_MUTATED=NO`, `LIVE_ENABLED=NO`, `ORDERS_SUBMITTED=NO`. No
`research-py/src/**`, `core-rs/**`, `config/**`, or `scripts/windows/**`
file was modified. No experiment code was changed after seeing this
result.

## 12. Wave-level short-exposure summary (all three predeclared families)

Across all six real trials (SHORT-02/03/04 × long-only/long-short),
**zero short-eligible predictions occurred anywhere in the wave**
(`BEARISH_PREDICTION_COUNT=0` in every long-short trial's OOS predictions).
Every long-short trial is execution-identical to its paired long-only
control. This is evidence toward
`CURRENT_CLASSIFIER_SHORT_POLICY_INSUFFICIENT_FOR_DIRECT_SHORT_BOOK_STUDY`
— to be formally recorded in the Patch E family closeout per the mission's
hard-stop rule on direct-ranking redesign.
