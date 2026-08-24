# SHORT-02-XS-MOMENTUM-RANK-01 — Result

Part of `SHORT-RESEARCH-WAVE-02-CONTROLLER`. Development-stage Research
only. Does not authorize Paper or Live shorting. No production Paper/
runtime/broker/risk/portfolio/scheduler behavior was modified.

## 1. Hypothesis

**Truthful name:** pooled single-feature cross-sectional-momentum-rank
classifier. An ETF's current 20-bar return percentile rank across the fixed
12-ETF universe (`ret_rank_20`) as the sole predictor of whether that same
ETF's own subsequent 10-bar log return is positive.

## 2. Data provenance (shared for the whole wave)

| Field | Value |
|---|---|
| Path | `extract_research_bars_with_provenance` (OFFICIAL provider path) |
| Feed | `sip` |
| Requested window | `2016-01-01T00:00:00Z` – `2024-01-01T00:00:00Z`, asof `2024-01-01` |
| Returned coverage | `2016-01-04T05:00:00+00:00` – `2023-12-29T05:00:00+00:00` |
| Rows fetched | 24,144 |
| Corporate-action policy | `adjusted_data`; 443 entries, all `cash_dividend`, `category_b_events_found=[]` (fail-closed CA gate PASS) |
| `canonical_semantic_bars_hash` | `690d36721d86a58b3c045f7d42abbd90fa9cf4d9d960e5d1d07a3e763e522eac` |

Fixed universe (frozen ex-ante): `SPY, QQQ, IWM, DIA, XLF, XLK, XLE, XLV, XLI, XLY, XLP, XLU`.

## 3. Feature isolation

`feature_columns == ["ret_rank_20"]`, produced by `build_feature_set_v1`'s
**default** `FeatureSetV1Spec` (`cross_section_windows=(5,20)`) — no
`research-py/src` change was needed. Enforced by
`assert_single_feature_schema`.

## 4. Labels

`fwd_ret = log(close[t+10]/close[t])`, `LABEL_HORIZON_BARS=10`,
`target = 1 iff fwd_ret > 0`. Classification label only, never treated as
executable P&L.

## 5. Walk-forward

`train_years=3, test_months=3, step_months=3, holdout_months=6,
min_rows_per_fold=300, purge_enabled=True, embargo_seconds=0`.
Both real trials: `folds_generated=17, folds_used=17, folds_skipped=0`.
OOS reference window: `2019-02-01` – `2023-04-28` (1,068 dates, identical
across long-only/long-short/placebo — verified by
`verify_oos_date_alignment`). `holdout_start_utc=2023-07-01T00:00:00+00:00`.
**Final holdout not consumed.**

## 6. Results

| | Long-only | Long/short | Placebo |
|---|---|---|---|
| `hypothesis_id` | `short02_xs_momentum_rank_long_only_v1` | `short02_xs_momentum_rank_long_short_v1` | `short02_xs_momentum_rank_placebo_v1` |
| `experiment_id` | SHORT-WAVE-02-REAL-CANDIDATES-V1 | SHORT-WAVE-02-REAL-CANDIDATES-V1 | SHORT-WAVE-02-DIAGNOSTIC-PLACEBOS-V1 |
| `trial_id` | `288b33128e1e3528ef4cbc9299f6198a` | `9b2a1bd3fe21fd30c83a7f502c009fcf` | `b9bfc4a9893913f491800c131387cb9a` |
| `attempt_id` | `288b3312...:att0001` | `9b2a1bd3...:att0001` | `b9bfc4a9...:att0001` |
| folds used | 17/17 | 17/17 | 17/17 |
| `net_total_return` | 0.24823 | 0.24823 | 0.24823 |
| `net_sharpe` | 0.35686 | 0.35686 | 0.35686 |
| `max_drawdown` | −0.34773 | −0.34773 | −0.34773 |
| `total_turnover` | 3,310,877.21 | 3,310,877.21 | 3,310,877.21 |
| `cost_drag` | 0.20908 | 0.20908 | 0.20908 |
| `active_days` | 1,034 | 1,034 | 1,034 |

**All three trials are execution-identical (bit-for-bit equal aggregate
metrics).** See §7.

### Paired delta (long/short − long-only)

`delta_net_total_return=0.0, delta_net_sharpe=0.0, delta_max_drawdown=0.0,
delta_turnover=0.0, delta_cost_drag=0.0`.

### Placebo delta (long/short real − placebo)

`delta_net_total_return=0.0, delta_net_sharpe=0.0, delta_max_drawdown=0.0`.

### Fold-reset benchmark (equal-weight 12-ETF basket, same OOS dates)

`cumulative_return_over_reference_dates=0.60776, sharpe=0.62416,
max_drawdown=-0.36288` over the same 1,068 dates
(`dates_with_no_return_observation=[]`, exact alignment PASS).

## 7. Short exposure

`BEARISH_PREDICTION_COUNT=0/12,816` OOS prediction rows in **both** the
real long-short trial and the placebo (`ml_score <= 0.45` never occurred).
Real trial `ml_score` range across all 12,816 OOS rows: `[0.57225,
0.65093]`, mean `0.62302`, std `0.01559` — the classifier's output is
tightly clustered well above both `entry_threshold=0.55` and
`short_threshold=0.45` for every single OOS row, in every fold, for the
entire 2019–2023 evaluation window. `NEGATIVE_POSITION_SYMBOL_DAYS=0`,
`SYMBOLS_SHORTED=[]`, `FOLDS_WITH_SHORT_EXPOSURE=0/17` (provable directly
from the score range — `_resolve_signal_direction` only ever returns `-1`
when `ml_score <= short_threshold`, which never occurred).

**Why long-only, long-short, and placebo are execution-identical:** the
single feature (`ret_rank_20`) carries so little separating power for this
classifier/threshold/window configuration that the fitted model's output is
dominated by the roughly-constant unconditional base rate of the label
(≈62% positive over this universe/window/horizon) rather than by the
feature or by which specific symbol received which label. The causal
placebo permutes `(fwd_ret,target)` pairs only *within* exact
`(end_ts,label_end_ts)` groups, which exactly preserves the label's global
positive rate — so a base-rate-dominated model trained on the permuted
labels produces `ml_score` values in the same narrow high band as the real
model (real: `[0.57225,0.65093]`; placebo: `[0.58018,0.64562]`,
confirmed genuinely different per-row scores, not a caching artifact — see
`long_only/eval/walk_forward_oos_predictions.csv` vs
`placebo/eval/walk_forward_oos_predictions.csv`, distinct row-for-row
values, distinct md5). Because position sizing in this Research engine is
equal-weight across "active" (non-flat) symbols rather than proportional to
`ml_score` magnitude, and every symbol/day is active-long in both the real
and placebo trials, the discrete executed positions — and therefore every
downstream P&L figure — end up identical, even though the underlying
trained models are genuinely distinct. This was verified directly (not
assumed): `targets.csv` differs between the real and placebo runs
(different md5, confirmed permutation applied), `features.csv` is
identical across long-only/long-short (same feature file, as expected), and
`ml_score` differs row-for-row between the real and placebo
`walk_forward_oos_predictions.csv` files. This is a genuine (if
uninformative) research result, not a deterministic Research-framework
defect — no harness code was changed after observing it.

## 8. Short-value interpretation

`SHORT_SIDE_VALUE_VERDICT = INCONCLUSIVE` — zero short-eligible predictions
occurred in 12,816 OOS rows across 17 folds; there is no exercised short
exposure to judge as helpful or harmful. Do not say "shorting failed" —
say short-side value is untested by this hypothesis under this exact
threshold configuration.

`MODEL_HYPOTHESIS_VERDICT` is deferred to `SHORT-WAVE-02-FAMILY-JUDGE-AND-
CLOSEOUT-01` (Patch E), where the real-candidate-only family DSR/PBO
becomes available across all six real trials — consistent with the
mission's rule that a `DEVELOPMENT_PROMISING_WITH_BORROW_MODEL_LIMITATION`
or `REJECTED_NOT_ADVANCED` classification requires "real-candidate DSR
supportive where evaluable" / "real-family PBO not strongly adverse", which
is only computable once the full real population is registered (after
Patches C and D).

## 9. Long/short attribution

`LONG_SHORT_ATTRIBUTION=UNKNOWN_NEEDS_PROOF` — same missing-evidence seam
identified in SHORT-01: per-symbol signed `executed_weight` is not
persisted per-symbol to any registered artifact. Moot here regardless,
since zero short exposure occurred.

## 10. Borrow-model caveat

`BORROW_MODEL=research_assumed_shortable_universe_v1`. No point-in-time
historical borrow availability, easy/hard-to-borrow state, locate, fee, or
recall risk is modeled. A positive family verdict here could at most be
`DEVELOPMENT_PROMISING_WITH_BORROW_MODEL_LIMITATION`, never
promotion-ready evidence.

## 11. Safety confirmation

`PRIMARY_PAPER_REPO_UNTOUCHED=YES`, `PAPER_DB_MUTATED=NO`,
`PAPER_RUNTIME_MUTATED=NO`, `LIVE_ENABLED=NO`, `ORDERS_SUBMITTED=NO`. No
`research-py/src/**`, `core-rs/**`, `config/**`, or `scripts/windows/**`
file was modified. `.env.local` was read (not modified) for exactly the two
authorized `ALPACA_API_KEY_PAPER`/`ALPACA_API_SECRET_PAPER` variables,
never printed.
