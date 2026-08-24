# SHORT-01-ETF-LONG-SHORT-TIME-SERIES-TREND — Research Report

Development-stage Research only. Does not authorize Paper or Live shorting.
No production Paper/runtime/broker/risk/portfolio/scheduler behavior was
modified. No Paper DB mutation, broker order, or Live action was taken.

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

### PBO / DSR

- `PBO = 0.9167` (10 combinatorially-purged blocks, 252/252 combinations
  evaluated, 0 skipped as degenerate) — **NOT_EVALUABLE=false**, real and
  evaluated: ~92% probability that the apparent outperformance among these
  three candidates is a backtest-overfitting artifact rather than genuine
  out-of-sample skill.
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
All three trial variants (11.9%–15.1% net total return) dramatically
underperform this passive benchmark over the identical window — expected
given partial exposure/costs, but it underscores that no trial captured
even simple beta efficiently, let alone alpha.

## 8. Interpretation

**The causal placebo does NOT behave as a null.** It produces the single
best net return, Sharpe, and DSR of the three trials. A genuinely
predictive `slope_60`-based signal would be expected to show the real
long-short candidate (Trial B) meaningfully outperforming its own
shuffled-label placebo (Trial C); instead the opposite ordering holds. This
directly falsifies the premise that Trial B's small edge over Trial A
reflects genuine trend-following skill: the placebo alone reproduces (and
slightly exceeds) that edge using labels that carry zero true information
about which symbol received which forward outcome.

Combined with `PBO=0.9167` and `effective_independent_trial_count≈1.01`
(the three candidates are ~99.4% correlated with each other), the picture
is consistent: net returns across A/B/C are dominated by shared exposure
to broad ETF-market beta and the classifier's ~63% unconditional positive
label rate, not by instrument-specific trend information. The identical
`max_drawdown` (−0.4034) across all three trials — almost certainly the
COVID-2020 drawdown — is itself evidence of a shared-beta-dominated return
stream rather than three economically distinct strategies.

The paired long-short-vs-long-only delta (+2.6pp net return, +0.026
Sharpe) is real and computed over exactly matching OOS dates, but per §8's
placebo failure it cannot be attributed to short-side skill — it sits
inside the noise band the placebo itself demonstrates, and long/short adds
turnover and cost drag without a corresponding drawdown improvement.

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

**`SHORT_01_VERDICT = REJECTED`**

Basis: robustness/statistical evidence clearly fails —
(a) `PBO=0.9167` (near-certain overfitting), and
(b) the causal placebo does not behave as a null; it in fact outperforms
the real long-short candidate on every scored metric (net return, Sharpe,
DSR).

This is not an "economically negative" rejection (all three trials show
positive net returns) and the short leg does not visibly *degrade* the
paired control (small positive delta) — but the paired delta is not
trustworthy evidence of genuine short-side value given (a) and (b). No
result-driven search followed this outcome (no threshold/window/universe
retuning was attempted).

Never `PROVEN_ALPHA`. Never `PROMOTION_READY`.

## 10. Borrow-model caveat

`BORROW_MODEL=research_assumed_shortable_universe_v1`. The Research
long/short engine assumes the evaluated universe is shortable at every
scored bar; it has no point-in-time historical borrow availability,
easy/hard-to-borrow state, locate availability, borrow fee, or recall
risk. Even had this experiment produced a positive result, it could at
most have been labeled `DEVELOPMENT_PROMISING_WITH_BORROW_MODEL_
LIMITATION`, never promotion-ready evidence. Moot here given the REJECTED
verdict.

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

None recommended inside this mission's scope. `SHORT-01` as specified is
REJECTED; a differently-parameterized variant (different trend window,
thresholds, or universe) is a **new experiment identity** per the mission's
no-result-driven-search rule and is out of scope here.
