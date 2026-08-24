# SHORT-01-ETF-LONG-SHORT-TIME-SERIES-TREND — Research Report

Development-stage Research only. Does not authorize Paper or Live shorting.
No production Paper/runtime/broker/risk/portfolio/scheduler behavior was
modified. No Paper DB mutation, broker order, or Live action was taken.

`EXPERIMENT_EXECUTION_STATUS=VALID` — independent review accepted the
underlying execution and artifacts (data provenance, placebo mechanics,
holdout isolation, real results) as reported below. Three deterministic
report/driver defects were subsequently identified and repaired
(`SHORT-01-DRIVER-PORTABILITY-01`, `SHORT-01-BENCHMARK-MEASUREMENT-
PARITY-01`, this document); §7–§9 below reflect the corrected
interpretation. `run_01`'s underlying artifacts and raw result values are
preserved unchanged — no trial was rerun.

## 1. Hypothesis and design

A pooled, single-feature ETF ABSOLUTE-TREND classifier using each
instrument's own 60-trading-day log-price slope (`slope_60` from
`FeatureSetV1Spec(trend_window=60)`), comparing:

- **Trial A** — identical-signal LONG-ONLY control (`entry_threshold=0.55`)
- **Trial B** — LONG/SHORT candidate (`entry_threshold=0.55`,
  `short_threshold=0.45`, `max_gross_exposure=1.0`,
  `borrow_model=research_assumed_shortable_universe_v1`)
- **Trial C** — causal same-horizon `(fwd_ret, target)` pair-permutation
  placebo, sharing Trial B's economic policy

Truthful terminology: this is a **pooled single-feature ETF absolute-trend
classifier**, not a canonical Moskowitz/Ooi/Pedersen TSMOM implementation —
the model trains on pooled ETF observations even though the predictor for
every row is that instrument's own trailing slope.

Primary question: does allowing the strategy to express bearish views via
SHORT positions materially improve the same underlying signal relative to
the long-only control, after costs?

## 2. Data provenance

| Field | Value |
|---|---|
| Path | `mqk_research.data.alpaca_historical.extract_research_bars_with_provenance` (OFFICIAL real-provider path) |
| Feed | `sip` (explicit semantic data-source choice; production `DEFAULT_FEED` remains `iex`, untouched) |
| Requested window | `2016-01-01T00:00:00Z` – `2024-01-01T00:00:00Z`, asof `2024-01-01` |
| Returned coverage | `2016-01-04T05:00:00+00:00` – `2023-12-29T05:00:00+00:00` |
| Rows fetched | 24,144 |
| Pagination complete (bars) | YES |
| Pagination complete (corporate actions) | YES |
| Source authority | `official_provider` |
| Corporate-action policy | `adjusted_data` (`alpaca_all_adjusted_v1`) |
| Corporate-action entries found | 443, all `cash_dividend` (`category_b_events_found=[]`) |
| Corporate-action fail-closed gate | **PASS** — no `CorporateActionReviewRequired` event hit the fixed universe |
| `canonical_semantic_bars_hash` | `690d36721d86a58b3c045f7d42abbd90fa9cf4d9d960e5d1d07a3e763e522eac` |

Universe (frozen ex-ante, before any result was observed): `SPY, QQQ, IWM,
DIA, XLF, XLK, XLE, XLV, XLI, XLY, XLP, XLU`. Never narrowed after seeing
results.

## 3. Feature isolation

`feature_columns == ["slope_60"]`, `FEATURE_COLUMN_COUNT=1`, enforced by a
driver-level fail-closed assertion (`assert_single_feature_schema`) after
`generate_feature_schema` — mirrors the same invariant independently
enforced in corrected ALPHA-01. Verified by focused negative-control tests
in `test_short_01_negative_controls.py`.

## 4. Labels

`fwd_ret = log(close[t+20]/close[t])`, `LABEL_HORIZON_BARS=20`,
`LABEL_RET_THRESHOLD=0.0`, `target = 1 iff fwd_ret > 0`. Classification
label only, never treated as executable P&L.

## 5. Walk-forward configuration

`train_years=3, test_months=3, step_months=3, holdout_months=6,
min_rows_per_fold=300, purge_enabled=True, embargo_seconds=0`.

All three trials: `folds_generated=17, folds_used=17, folds_skipped=0`.
OOS reference window: `2019-03-01` – `2023-05-31` (1,071 observations, one
shared date sequence verified identical across all three trials).
`holdout_start_utc=2023-06-01T00:00:00+00:00` — the final ~6 months of the
dataset were reserved and never evaluated (`holdout.status ==
"reserved_not_evaluated"` on every trial). **Final holdout was not
consumed.**

## 6. Economic model

`CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0)` —
**CONSERVATIVE COST ASSUMPTION**, not actual Alpaca commission.
`ExecutionPricingSpec(pricing_model_id=rust_conservative_bar_range_v1,
slippage_bps=5, volatility_mult_bps=0)`. `WeightToShareSpec(equity_usd=
100000.0)`. `max_gross_exposure=1.0`.

## 7. Results

| | Trial A (long-only) | Trial B (long/short) | Trial C (placebo) |
|---|---|---|---|
| `trial_id` | `4b4d4950fe91732d…` | `4361dc4fcae7c892…` | `085f1ae3f2f38cda…` |
| `net_total_return` | 0.1190 | 0.1452 | **0.1505** |
| `net_sharpe` | 0.2317 | 0.2580 | **0.2630** |
| `annualized_net_return` | 0.0268 | 0.0324 | 0.0335 |
| `max_drawdown` | −0.4034 | −0.4034 | −0.4034 |
| `cost_drag` | 0.2806 | 0.2886 | 0.2869 |
| `total_turnover` | 3,936,541.68 | 3,945,638.33 | 3,871,850.13 |
| `profitable_fold_count` | 10/17 | 10/17 | 10/17 |
| Judge admission | ADMITTED | ADMITTED | ADMITTED |
| DSR (deflated Sharpe) | 0.6901 | 0.7088 | **0.7122** |

**All three trials were ADMITTED into the same judge comparison scope**
(3 registered, 3 admitted, 0 excluded) and share the exact same OOS date
sequence — the judge's comparison key does not include
`direction_policy`/`entry_threshold`/`short_threshold`, only protocol,
bars provenance, evaluation spec, annualization, cost model, and execution
capacity policy, all identical across A/B/C.

### Short exposure (Trial B, real OOS predictions)

`BEARISH_PREDICTION_COUNT=13/12612` OOS prediction rows scored
`ml_score <= 0.45` (short-eligible). All 13 are `XLE`, `fold=6`, June 2020.
The durable discrete-share `weight_to_share` evidence confirms this
produced `NEGATIVE_POSITION_SYMBOL_DAYS=12`, `SYMBOLS_SHORTED=["XLE"]`,
`FOLDS_WITH_SHORT_EXPOSURE=1/17` — Trial A (long-only) and Trial B
(long/short) are **identical in 16 of 17 folds**; they differ only in
Fold 6 (long-only: 0.06334, long/short: 0.08818, delta +0.02484). This is
signed negative-quantity evidence that DOES exist in the durable
`weight_to_share_evidence`, even though per-symbol long/short P&L
attribution is not derivable from the registered economic artifact schema
(see §7's long/short-attribution note below) — the two gaps are distinct
and should not be conflated.

Given this, the single `REJECTED` verdict in an earlier draft of this
report was too coarse: it does not distinguish "the exact classifier isn't
worth advancing" from "shorting was shown not to help." Only one fold ever
activated a short. See §9 for the corrected two-part verdict.

### PBO / DSR

- `FULL_REGISTERED_POPULATION_PBO = 0.9167` (`POPULATION=long-only +
  long-short + placebo`, 10 combinatorially-purged blocks, 252/252
  combinations evaluated, 0 skipped as degenerate) — **NOT_EVALUABLE=false**,
  real and evaluated. This is a high CSCV PBO for the full three-trial
  registered comparison population; the production judge has no
  diagnostic-placebo exclusion, so it necessarily includes Trial C. It
  should be read as "high CSCV PBO for the full three-trial registered
  comparison" — not as "92% probability the two real strategies are
  overfit" or "near-certain overfitting."
- `REAL_CANDIDATE_ONLY_PBO_DIAGNOSTIC = 0.7778` — independently reproduced
  from `run_01`'s existing Trial A/B artifacts (unchanged, no rerun) using
  the same production `mqk_research.ml.multiple_testing_stats.
  combinatorial_symmetric_cv_pbo` method, restricted to only the two real
  candidates (252/252 combinations evaluable).
  `AUTHORITY=POST_HOC_REVIEW_DIAGNOSTIC_NOT_REGISTERED_JUDGE` — this is not
  a registry-authoritative judge artifact; no new trial or judge was
  registered to obtain it.
- `average_pairwise_correlation = 0.9937` across the three trials'
  net-daily-return series →
  `effective_independent_trial_count ≈ 1.013` — the three "distinct"
  hypotheses behave, statistically, as essentially **one** bet, not three
  independently informative ones.
- DSR is real and evaluable for all three trials, but **ranks the placebo
  highest** (0.7122 > 0.7088 long-short > 0.6901 long-only) — the
  deflated-Sharpe correction cannot distinguish the shuffled-label
  candidate from the real ones because their return series are nearly
  identical (see §8).

### Paired long-short vs. long-only control (exact matching OOS dates)

| Delta (B − A) | Value |
|---|---|
| `delta_net_total_return` | +0.02614 |
| `delta_net_sharpe` | +0.02629 |
| `delta_max_drawdown` | 0.0 (identical) |
| `delta_turnover` | +9,096.65 |
| `delta_cost_drag` | +0.00806 (long/short is *more* costly) |

### Benchmark (equal-weight, daily-rebalanced, exact OOS dates)

`cumulative_return_over_reference_dates = 0.5760` over the same 1,071
dates, `dates_with_no_return_observation = []` (exact alignment, PASS).
This is the `CONTINUOUS_DATE_ALIGNED_CONTEXT_BENCHMARK`: `pct_change` is
computed **before** OOS filtering, so each of the 17 folds' first OOS date
carries a return from the prior close — a date the economic strategy was
never exposed to, since it starts every fold flat
(`net_daily_return=0` on that date). It remains a useful same-date-range
context figure but is **not** an apples-to-apples measurement-convention
match to the strategy returns above.

The direct, measurement-convention-matched comparator is the **fold-reset
benchmark** (`build_fold_reset_benchmark`, independently reproduced from
`run_01`'s existing `raw_bars.csv` and each trial's
`walk_forward_oos_predictions.csv` — no trial was rerun): every fold's
first actual OOS date is forced to a benchmark return of `0.0`, matching
the strategy's own fold-start convention.

| Metric | Value |
|---|---|
| `FOLD_RESET_BENCHMARK_CUMULATIVE_RETURN` | 0.49828 |
| `FOLD_RESET_BENCHMARK_SHARPE` | 0.54762 |
| `FOLD_RESET_BENCHMARK_MAX_DRAWDOWN` | −0.39074 |

Under this apples-to-apples comparator, the passive fold-reset ETF basket
(49.8% return, 0.548 Sharpe) still materially dominates all three trial
variants (11.9%–15.1% net return, 0.23–0.26 Sharpe) in both return and
Sharpe, with slightly better drawdown than the trials' −0.4034. This
underperformance is evidence the exact classifier failed to add value
beyond passive beta — it is not, by itself, proof that no alpha exists
anywhere in this signal family.

## 8. Interpretation

**The causal placebo matches/exceeds the real candidates on every scored
metric.** It produces the single best net return, Sharpe, and DSR of the
three trials. A genuinely predictive `slope_60`-based signal would be
expected to show the real long-short candidate (Trial B) meaningfully
outperforming its own shuffled-label placebo (Trial C); instead the
opposite ordering holds.

**Placebo semantics, precisely stated:** the accepted placebo permutes
`(fwd_ret, target)` pairs across SYMBOLS *within* exact
`(end_ts, label_end_ts)` groups — it destroys symbol-specific
feature/outcome pairing while preserving date, label horizon, the
same-date outcome distribution, and common market/regime information.
This is especially material for a highly correlated ETF universe (see
`average_pairwise_correlation = 0.9937` below). It is a **negative control
for instrument-specific predictive association**, not a complete null for
date-level/common-market trend structure. The correct reading of the
placebo matching/beating Trial B is: **no demonstrated instrument-specific
`slope_60` edge beyond shared same-date market structure** — not "the
placebo carries zero true information" and not "all possible trend
information is falsified."

Combined with `FULL_REGISTERED_POPULATION_PBO=0.9167` and
`effective_independent_trial_count≈1.01` (the three candidates are ~99.4%
correlated with each other), the picture is consistent: net returns across
A/B/C are dominated by shared exposure to broad ETF-market beta and the
classifier's ~63% unconditional positive label rate, not by
instrument-specific trend information. The identical `max_drawdown`
(−0.4034) across all three trials — almost certainly the COVID-2020
drawdown — is itself evidence of a shared-beta-dominated return stream
rather than three economically distinct strategies. The passive fold-reset
ETF basket materially dominating all three trials' return and Sharpe (see
§7) reinforces this: none of the three trials captured even simple beta
efficiently, let alone instrument-specific alpha. Benchmark
underperformance alone does not, by itself, prove the absence of alpha —
it is one of several converging pieces of evidence here, not the sole
basis for the verdict.

The paired long-short-vs-long-only delta (+2.6pp net return, +0.026
Sharpe) is real and computed over exactly matching OOS dates, but per
§7's short-exposure finding it is driven almost entirely by a single fold
(Fold 6, June 2020, `XLE` only — 13/12,612 OOS predictions, 1/17 folds).
Combined with the placebo evidence above, this delta cannot be attributed
to durable short-side skill: it sits inside the noise band the placebo
itself demonstrates, long/short adds turnover and cost drag without a
corresponding drawdown improvement, and the short side activated far too
sparsely to constitute a stress test of short-side value.

### Long/short attribution

`LONG_SHORT_ATTRIBUTION=UNKNOWN_NEEDS_PROOF`. `economic_walkforward.py`'s
`_daily_aggregate()` / `_simulate_fold()` pool every symbol's
`gross_contrib`/`turnover`/`transaction_cost` into a single per-date
scalar series (`gross_by_ts`/`turnover_by_ts` accumulation, roughly
`economic_walkforward.py` L1600–1725) before `economic_daily_returns.csv`
is written. Per-symbol SIGNED `executed_weight` is computed internally
(`exposure_frame`, `_simulate_fold_execution`) but is never persisted
per-symbol to any output artifact. Long-leg vs. short-leg gross/net
return, active days, turnover, and cost drag therefore cannot be derived
from the registered economic artifact schema as it exists today without
inventing attribution — this is a missing evidence seam, not a defect, and
production code was not patched to manufacture it.

## 9. Verdict

Independent review found the original single `REJECTED` verdict too
coarse: it conflated "the exact classifier isn't worth advancing" with
"shorting was shown not to help," when the short side barely activated
(§7). The verdict is now split into two parts:

**`MODEL_HYPOTHESIS_VERDICT = REJECTED_NOT_ADVANCED`** — the exact pooled
`slope_60` logistic classifier (this specific feature/threshold/window
configuration) is not worth advancing. Basis:
(a) `FULL_REGISTERED_POPULATION_PBO=0.9167` for the full three-trial
registered population (high CSCV PBO; not, by itself, "92% probability the
two real strategies are overfit" — see §7),
(b) `REAL_CANDIDATE_ONLY_PBO_DIAGNOSTIC=0.7778` (post-hoc diagnostic,
non-registered) is likewise elevated for the two real candidates alone,
(c) the causal placebo matches/exceeds the real candidates on every scored
metric — an instrument-specific-association negative control, not a
complete market-regime null (§8), and
(d) the passive fold-reset ETF basket materially dominates all three
trials' return and Sharpe with slightly better drawdown (§7).

**`SHORT_SIDE_VALUE_VERDICT = INCONCLUSIVE`** — the experiment did not
generate sufficiently broad/stable short exposure to decide whether adding
a short side is useful generally. Only 13/12,612 OOS predictions
(1 symbol, `XLE`, 1/17 folds, June 2020) were short-eligible; Trial A and
Trial B are identical in 16 of 17 folds. This is too sparse a sample to
be either a positive or negative finding about short-side value under this
signal. Do NOT say "shorting failed" or "short strategies are rejected" —
say the exact classifier is not advanced, and short-side value remains
untested by this experiment.

**`SHORT_01_OVERALL_STATUS = MODEL_NOT_ADVANCED_SHORT_VALUE_INCONCLUSIVE`**

This is not an "economically negative" rejection (all three trials show
positive net returns) and the short leg does not visibly *degrade* the
paired control (small positive delta, concentrated in Fold 6) — but that
delta is not trustworthy evidence of genuine short-side value given the
combined evidence above. No result-driven search followed this outcome (no
threshold/window/universe retuning was attempted).

Never `PROVEN_ALPHA`. Never `PROMOTION_READY`.

## 10. Borrow-model caveat

`BORROW_MODEL=research_assumed_shortable_universe_v1`. The Research
long/short engine assumes the evaluated universe is shortable at every
scored bar; it has no point-in-time historical borrow availability,
easy/hard-to-borrow state, locate availability, borrow fee, or recall
risk. Even had this experiment produced a positive result, it could at
most have been labeled `DEVELOPMENT_PROMISING_WITH_BORROW_MODEL_
LIMITATION`, never promotion-ready evidence. Moot here given
`MODEL_HYPOTHESIS_VERDICT=REJECTED_NOT_ADVANCED`.

## 11. Safety confirmation

- `PRIMARY_PAPER_REPO_UNTOUCHED=YES` (HEAD `edcda740b2f05fbe8a2657f2301b8ea373efb4b6`, verified before and after this mission)
- `PAPER_DB_MUTATED=NO`
- `PAPER_RUNTIME_MUTATED=NO`
- `LIVE_ENABLED=NO`
- `PAPER_SHORT_ORDERS_SUBMITTED=NO`
- `REAL_ORDERS_SUBMITTED=NO`
- No `research-py/src/**`, `core-rs/**`, `config/**`, or `scripts/windows/**` file was modified.
- `.env.local` was read (not modified, not copied) for exactly the two
  authorized read-only variables (`ALPACA_API_KEY_PAPER`,
  `ALPACA_API_SECRET_PAPER`), loaded in-memory into this process only, and
  never printed/logged/committed.
- `PUSHED=NO`. No merge performed.

## 12. Next best action

None recommended inside this mission's scope. `SHORT-01` as specified has
`MODEL_HYPOTHESIS_VERDICT=REJECTED_NOT_ADVANCED`; a differently-
parameterized variant (different trend window, thresholds, or universe) is
a **new experiment identity** per the mission's no-result-driven-search
rule and is out of scope here. `SHORT_SIDE_VALUE_VERDICT=INCONCLUSIVE`
means a future experiment designed to generate broader/more stable short
exposure (not merely a retuned version of this one) would be needed to
actually test short-side value — not proposed or scoped here.
