# SHORT-03-SHORT-TERM-REVERSAL-01 — Result

Part of `SHORT-RESEARCH-WAVE-02-CONTROLLER`. Development-stage Research
only. Does not authorize Paper or Live shorting. No production Paper/
runtime/broker/risk/portfolio/scheduler behavior was modified.

## 1. Hypothesis

**Truthful name:** pooled single-feature short-term-reversal classifier. An
ETF's own trailing 5-bar log return (`ret_5`) as the sole predictor of
whether that same ETF's own subsequent 10-bar log return is positive.
Hypothesized relation: negative (very positive recent return → lower
P(fwd_ret>0); very negative recent return → higher P(fwd_ret>0)). Sign not
enforced.

## 2. Data provenance

Shared wave-level fetch (see
[SHORT_WAVE_02_SHORT_02_RESULT.md](SHORT_WAVE_02_SHORT_02_RESULT.md) §2 for
the full manifest — identical bars, same `canonical_semantic_bars_hash =
690d36721d86a58b3c045f7d42abbd90fa9cf4d9d960e5d1d07a3e763e522eac`, fetched
once and reused for every hypothesis in this wave).

## 3. Feature isolation

`feature_columns == ["ret_5"]`, produced by `build_feature_set_v1`'s
**default** `FeatureSetV1Spec` (`ret_windows=(1,2,5,10,20)` includes 5 by
default) — no `research-py/src` change was needed. Enforced by
`assert_single_feature_schema`.

## 4. Labels

`fwd_ret = log(close[t+10]/close[t])`, `LABEL_HORIZON_BARS=10`,
`target = 1 iff fwd_ret > 0`. Same shared real-target set as SHORT-02.

## 5. Walk-forward

Same frozen spec as SHORT-02 (`train_years=3, test_months=3,
step_months=3, holdout_months=6, min_rows_per_fold=300, purge_enabled=True,
embargo_seconds=0`). All three trials: `folds_used=18/18, folds_skipped=0`
(one more fold than SHORT-02/04 due to this feature's own NaN-dropout
pattern shifting the earliest usable row slightly). OOS reference window:
`2019-01-02` – `2023-06-30` (1,132 dates, identical across all three
trials — verified by `verify_oos_date_alignment`).
`holdout_start_utc=2023-07-01T00:00:00+00:00`. **Final holdout not
consumed.**

## 6. Results

| | Long-only | Long/short | Placebo |
|---|---|---|---|
| `hypothesis_id` | `short03_short_term_reversal_long_only_v1` | `short03_short_term_reversal_long_short_v1` | `short03_short_term_reversal_placebo_v1` |
| `experiment_id` | SHORT-WAVE-02-REAL-CANDIDATES-V1 | SHORT-WAVE-02-REAL-CANDIDATES-V1 | SHORT-WAVE-02-DIAGNOSTIC-PLACEBOS-V1 |
| `trial_id` | `df611aa8e77bfbd013cf1eb9cd82f1b9` | `a08883adbb80853abe4d7a1768aa9176` | `3f1db76cdc85fe5359c877778e0eb7fb` |
| `attempt_id` | `df611aa8...:att0001` | `a08883ad...:att0001` | `3f1db76c...:att0001` |
| folds used | 18/18 | 18/18 | 18/18 |
| `net_total_return` | 0.39431 | 0.39431 | 0.40931 |
| `net_sharpe` | 0.46220 | 0.46220 | 0.47348 |
| `max_drawdown` | −0.35516 | −0.35516 | −0.35516 |
| `total_turnover` | 3,750,698.33 | 3,750,698.33 | 3,577,973.75 |
| `cost_drag` | 0.29926 | 0.29926 | 0.28475 |
| `active_days` | 1,096 | 1,096 | 1,096 |

**Long-only and long-short are execution-identical** (bit-for-bit equal),
for the same reason as SHORT-02 (§7). **The placebo genuinely differs from
the real trial this time** — see §7.

### Paired delta (long/short − long-only)

`delta_net_total_return=0.0, delta_net_sharpe=0.0, delta_max_drawdown=0.0,
delta_turnover=0.0, delta_cost_drag=0.0`.

### Placebo delta (long/short real − placebo)

`delta_net_total_return=-0.01500, delta_net_sharpe=-0.01128,
delta_max_drawdown≈0.0 (1.1e-16, floating-point-equal)` — **the placebo
outperforms the real trial** on both return and Sharpe.

### Fold-reset benchmark (equal-weight 12-ETF basket, same OOS dates)

`cumulative_return_over_reference_dates=0.81899, sharpe=0.73261,
max_drawdown=-0.36288` over the same 1,132 dates
(`dates_with_no_return_observation=[]`, exact alignment PASS).

## 7. Short exposure

`BEARISH_PREDICTION_COUNT=0/13,464` OOS prediction rows in the real
long-short trial (`ml_score <= 0.45` never occurred). `ml_score` range:
`[0.53175, 0.72238]`, mean `0.61963` — again clustered well above both
`entry_threshold=0.55` and `short_threshold=0.45` for every OOS row, in
every fold. `NEGATIVE_POSITION_SYMBOL_DAYS=0`, `SYMBOLS_SHORTED=[]`,
`FOLDS_WITH_SHORT_EXPOSURE=0/18` (provable directly from the score range).

Unlike SHORT-02, the real and placebo trials here are **not**
execution-identical: `targets.csv` differs (confirmed distinct md5,
`0af08fc6...` vs `f357c118...`), `features.csv` is identical across
long-only/long-short (`cc0a1c48...`, confirmed), and this time the trained
model's `ml_score` distribution differs enough between real and placebo
that the discrete equal-weight-active-symbol positions diverge on some
days, producing the small but real `-0.01500` net-return delta in the
placebo's favor. This is evidence the `ret_5` feature carries at least
*some* row-level information the model uses (unlike SHORT-02's essentially
pure base-rate degeneracy) — but the direction of the effect is
unfavorable: the real signal underperforms its own shuffled-label
placebo, not the reverse a genuinely predictive reversal signal would be
expected to show.

## 8. Short-value interpretation

`SHORT_SIDE_VALUE_VERDICT = INCONCLUSIVE` — zero short-eligible predictions
occurred in 13,464 OOS rows across 18 folds; there is no exercised short
exposure to judge as helpful or harmful.

`MODEL_HYPOTHESIS_VERDICT` is deferred to `SHORT-WAVE-02-FAMILY-JUDGE-AND-
CLOSEOUT-01` (Patch E), where the real-candidate-only family DSR/PBO
becomes available across all six real trials.

## 9. Long/short attribution

`LONG_SHORT_ATTRIBUTION=UNKNOWN_NEEDS_PROOF` — same missing-evidence seam
as SHORT-01/SHORT-02. Moot here regardless, since zero short exposure
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
