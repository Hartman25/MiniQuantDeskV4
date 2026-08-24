# SHORT-RESEARCH-WAVE-02-CONTROLLER — Family Closeout Report

Development-stage Research only. Does not authorize Paper or Live
shorting. No production Paper/runtime/broker/risk/portfolio/scheduler
behavior was modified. No Paper DB mutation, broker order, or Live action
was taken.

## 1. Wave design

Three predeclared, genuinely distinct single-feature ETF classifier
hypotheses, each evaluated as a matched long-only control / long-short
candidate pair plus a causal same-horizon diagnostic placebo, on the fixed
12-ETF universe (`SPY, QQQ, IWM, DIA, XLF, XLK, XLE, XLV, XLI, XLY, XLP,
XLU`) over `2016-01-01`–`2024-01-01` (feed=`sip`), `LABEL_HORIZON_BARS=10`:

| Family | Feature | Truthful name | Hypothesized sign |
|---|---|---|---|
| SHORT-02 | `ret_rank_20` | pooled single-feature cross-sectional-momentum-rank classifier | positive (not enforced) |
| SHORT-03 | `ret_5` | pooled single-feature short-term-reversal classifier | negative (not enforced) |
| SHORT-04 | `gap_pct_1` | pooled single-feature gap-reversal/exhaustion classifier | negative (not enforced) |

All parameters (universe, data window, label, walk-forward, cost model,
execution model, thresholds, placebo seed) were frozen in
`research-py/experiments/short_wave_02/PREDECLARED_WAVE.json` before any
trial was executed (Patch A, commit `92ac16a6`). No later patch changed an
earlier hypothesis's parameters or the harness itself.

Full per-hypothesis results: [SHORT_WAVE_02_SHORT_02_RESULT.md](SHORT_WAVE_02_SHORT_02_RESULT.md),
[SHORT_WAVE_02_SHORT_03_RESULT.md](SHORT_WAVE_02_SHORT_03_RESULT.md),
[SHORT_WAVE_02_SHORT_04_RESULT.md](SHORT_WAVE_02_SHORT_04_RESULT.md).

## 2. Data provenance

Fetched ONCE for the whole wave via
`extract_research_bars_with_provenance` (OFFICIAL provider path,
feed=`sip`): requested `2016-01-01T00:00:00Z`–`2024-01-01T00:00:00Z`
(asof `2024-01-01`), returned coverage `2016-01-04T05:00:00+00:00`–
`2023-12-29T05:00:00+00:00`, 24,144 rows, `canonical_semantic_bars_hash =
690d36721d86a58b3c045f7d42abbd90fa9cf4d9d960e5d1d07a3e763e522eac`
(byte-identical to the independently-fetched SHORT-01 official cache — same
universe/window/feed). Corporate-action policy `adjusted_data`; 443
entries, all `cash_dividend`; `category_b_events_found=[]` — fail-closed CA
gate PASSED, no review required. The fixed universe was never narrowed
after seeing any result.

## 3. Real-candidate population and family judge

`build_multiple_testing_judge(experiment_id="SHORT-WAVE-02-REAL-
CANDIDATES-V1", hypothesis_id=None)` was run once, after all six real
trials were registered (Patch E).

- `REGISTERED_REAL_TRIAL_COUNT = 6`
- `ADMITTED_REAL_TRIAL_COUNT = 4`
- `EXCLUDED_REAL_TRIAL_COUNT = 2`

**Exclusions (exact reasons from the judge artifact):**

| `trial_id` | Hypothesis | Reason |
|---|---|---|
| `288b33128e1e3528ef4cbc9299f6198a` | `short02_xs_momentum_rank_long_only_v1` | `return_series_date_misalignment` |
| `9b2a1bd3fe21fd30c83a7f502c009fcf` | `short02_xs_momentum_rank_long_short_v1` | `return_series_date_misalignment` |

**Cause:** SHORT-02's OOS reference window is `2019-02-01`–`2023-04-28`
(1,068 dates, 17 folds), while SHORT-03/SHORT-04 both share
`2019-01-02`–`2023-06-30` (1,132 dates, 18 folds) — a real, structural
difference in usable history driven by each feature's own NaN-dropout
pattern (`ret_rank_20`'s 20-bar cross-sectional rank requires more warm-up
than `ret_5`/`gap_pct_1`), not a defect. Per the mission's rule ("Any
candidate with a different date index must be excluded truthfully from the
common family judge"), both SHORT-02 real trials are truthfully excluded
from the family DSR/PBO comparison. This was not anticipated as a specific
outcome before execution, but the possibility of unequal date indices
across genuinely different single-feature families was foreseen and
governs why the judge output — not an assumption of `admitted=6` — is
authoritative here.

### DSR (admitted candidates only)

| `trial_id` | Hypothesis | `deflated_sharpe_ratio` | `observed_sharpe_annualized` |
|---|---|---|---|
| `9606018c657995b43db1843b4740e991` | `short04_gap_reversal_long_only_v1` | 0.85100 | 0.48404 |
| `d17bc45250844dce5e78c5a9e866052d` | `short04_gap_reversal_long_short_v1` | 0.85100 | 0.48404 |
| `a08883adbb80853abe4d7a1768aa9176` | `short03_short_term_reversal_long_short_v1` | 0.84024 | 0.46220 |
| `df611aa8e77bfbd013cf1eb9cd82f1b9` | `short03_short_term_reversal_long_only_v1` | 0.84024 | 0.46220 |

(DSR is pairwise identical within each family because, per §5 of the
per-hypothesis reports, long-only and long-short are execution-identical
in every admitted family — same net-daily-return series, same DSR input.)

`average_pairwise_correlation = 0.99976` across the four admitted
candidates → `effective_independent_trial_count ≈ 1.0007` — the four
"distinct" admitted trials behave, statistically, as essentially **one**
bet (two economically-identical pairs whose own cross-pair correlation is
still ≈1, since both are dominated by the same shared long-equal-weight
basic ETF-market exposure).

`REAL_FAMILY_PBO = 0.08333` (`combinatorially_symmetric_cv`, 10 blocks,
252/252 combinations evaluated, 0 skipped as degenerate,
`num_candidates=4`). This is numerically low, but given
`effective_independent_trial_count≈1`, it should be read as "low
overfitting probability across what is statistically one bet" — not as
meaningful evidence of four independently-validated strategies.

`PLACEBOS_INCLUDED_IN_REAL_PBO = NO` — structurally guaranteed: placebo
trials are registered under the separate `SHORT-WAVE-02-DIAGNOSTIC-
PLACEBOS-V1` experiment id and `store.list_trials(experiment_id=...)`
cannot return trials registered under a different experiment id.

## 4. Diagnostic placebo comparison (NOT part of the real-family PBO)

| Family | `delta_net_total_return` (real − placebo) | `delta_net_sharpe` | Interpretation |
|---|---|---|---|
| SHORT-02 | 0.0 | 0.0 | Real and placebo execution-identical (zero short exposure, base-rate-dominated model in both) |
| SHORT-03 | **−0.01500** | **−0.01128** | Placebo *outperforms* the real signal |
| SHORT-04 | 0.0 | 0.0 | Real and placebo execution-identical (same structural reason as SHORT-02) |

No family's real long-short candidate meaningfully beats its own matched
causal placebo. Recall the causal placebo permutes `(fwd_ret,target)`
pairs only within exact `(end_ts,label_end_ts)` groups — it destroys
symbol-specific feature/outcome pairing while preserving date-level/common
market structure. A match/loss to the placebo means "no demonstrated
instrument-specific edge beyond shared same-date market structure", not
"zero information exists anywhere in the signal family."

## 5. Short exposure (wave-wide)

**Zero short-eligible predictions occurred in any of the six real
long-short trials.** `ml_score` never dropped to or below
`short_threshold=0.45` in any OOS row, in any fold, in any family:

| Family | OOS rows | `BEARISH_PREDICTION_COUNT` | `ml_score` range |
|---|---|---|---|
| SHORT-02 | 12,816 | 0 | `[0.57225, 0.65093]` |
| SHORT-03 | 13,464 | 0 | `[0.53175, 0.72238]` |
| SHORT-04 | 13,464 | 0 | `[0.55011, 0.68874]` |

Consequently every long-short trial is execution-identical to its paired
long-only control in all three families
(`delta_net_total_return=delta_net_sharpe=delta_max_drawdown=
delta_turnover=delta_cost_drag=0.0` in every family — see §6 of each
per-hypothesis result doc).

`CURRENT_CLASSIFIER_SHORT_POLICY_INSUFFICIENT_FOR_DIRECT_SHORT_BOOK_
STUDY = YES` — all three predeclared long/short candidates failed to
generate any short exposure whatsoever under `entry_threshold=0.55,
short_threshold=0.45`. Per the mission's HARD STOP — DIRECT RANKING rule,
this wave does **not** attempt to redesign `SHORT-02` (or any family) into
a deterministic rank-sorted portfolio, and does not retune thresholds
result-driven. A future, separately-authorized mission may consider an
explicitly-versioned direct-score/rank-based Research capability; that
capability is not built here.

## 6. Benchmark (fold-reset, equal-weight 12-ETF basket)

| Family | OOS window | Benchmark cumulative return | Benchmark Sharpe | Real candidate best return | Real candidate best Sharpe |
|---|---|---|---|---|---|
| SHORT-02 | 2019-02-01–2023-04-28 (1,068d) | 0.60776 | 0.62416 | 0.24823 | 0.35686 |
| SHORT-03 | 2019-01-02–2023-06-30 (1,132d) | 0.81899 | 0.73261 | 0.39431 | 0.46220 |
| SHORT-04 | 2019-01-02–2023-06-30 (1,132d) | 0.81899 | 0.73261 | 0.42389 | 0.48404 |

The passive fold-reset ETF basket materially dominates every real
candidate's return and Sharpe in all three families. This is one converging
piece of evidence (not the sole basis) that none of the three classifiers
captures even simple beta efficiently, let alone instrument-specific
alpha.

## 7. Verdicts

| Family | `MODEL_HYPOTHESIS_VERDICT` | `SHORT_SIDE_VALUE_VERDICT` |
|---|---|---|
| SHORT-02 | `REJECTED_NOT_ADVANCED` | `INCONCLUSIVE` |
| SHORT-03 | `REJECTED_NOT_ADVANCED` | `INCONCLUSIVE` |
| SHORT-04 | `REJECTED_NOT_ADVANCED` | `INCONCLUSIVE` |

**Basis for `REJECTED_NOT_ADVANCED` (all three):**
(a) no family's long-short candidate meaningfully outperforms its own
long-only control (delta = 0.0 in every family — the long/short mechanism
never activated),
(b) no family's real candidate meaningfully beats its matched causal
placebo (SHORT-02/04 tie exactly; SHORT-03's placebo *outperforms* the
real signal),
(c) SHORT-02's real trials could not even be admitted into the family DSR/
PBO comparison (date misalignment), so no family-level statistical support
is available for it at all,
(d) SHORT-03/SHORT-04's admitted DSR values (0.840, 0.851) are numerically
high but computed over what is, per §3, an `effective_independent_trial_
count≈1` population — not independent statistical support for two
distinct strategies, and
(e) the passive fold-reset benchmark materially dominates every real
candidate's return and Sharpe in every family (§6).

**Basis for `SHORT_SIDE_VALUE_VERDICT=INCONCLUSIVE` (all three):** zero
short-eligible predictions occurred anywhere in the wave (§5) — there is no
exercised short exposure to judge as helpful or harmful in any family.

`BEST_DEVELOPMENT_CANDIDATE = NONE` — no family satisfies the mission's
`DEVELOPMENT_PROMISING_WITH_BORROW_MODEL_LIMITATION` bar (positive after
costs + long/short improves meaningfully over its long-only control +
short exposure broad enough + matched placebo clearly weaker + real-family
DSR/PBO supportive). None of the three long/short candidates cleared even
the first of those jointly-required conditions (meaningful improvement
over the long-only control).

Never `PROVEN_ALPHA`. Never `PROMOTION_READY`.

## 8. Borrow-model caveat

`BORROW_MODEL=research_assumed_shortable_universe_v1` for every long-short
and placebo trial. The Research long/short engine assumes the evaluated
universe is shortable at every scored bar — no point-in-time historical
borrow availability, easy/hard-to-borrow state, locate, fee, or recall risk
is modeled. Moot here given every family's
`MODEL_HYPOTHESIS_VERDICT=REJECTED_NOT_ADVANCED`.

## 9. No result-driven follow-up

No threshold, window, universe, label-horizon, or cost-assumption retuning
was attempted after seeing any result. `SHORT-03`'s parameters were not
changed after `SHORT-02` looked weak; `SHORT-04`'s parameters were not
changed after `SHORT-03`'s placebo outperformed it. This wave ends after
the three predeclared mechanisms per the controller rule.

---

## Final report fields

```
VERDICT=SHORT_WAVE_02_ALL_THREE_FAMILIES_REJECTED_NOT_ADVANCED_SHORT_VALUE_INCONCLUSIVE

WORKTREE=C:\Users\Zacha\Desktop\MiniQuantDeskV4-short-wave-02
BRANCH=research-short-wave-02
BASE_HEAD=e31a49143e16cc61bbf7459f1c569fc1ce6a4851
FINAL_HEAD=<set at commit time, see COMMITS below>

PREDECLARATION_COMMIT=92ac16a639d355337c68d91e0ac4a4dc45d63e25

DATA_FEED=sip
RETURNED_COVERAGE=2016-01-04T05:00:00+00:00 .. 2023-12-29T05:00:00+00:00
FIXED_UNIVERSE=SPY,QQQ,IWM,DIA,XLF,XLK,XLE,XLV,XLI,XLY,XLP,XLU
CA_GATE=PASS (category_b_events_found=[], 443 cash_dividend entries)

REAL_EXPERIMENT_ID=SHORT-WAVE-02-REAL-CANDIDATES-V1
PLACEBO_EXPERIMENT_ID=SHORT-WAVE-02-DIAGNOSTIC-PLACEBOS-V1

REGISTERED_REAL_TRIAL_COUNT=6
ADMITTED_REAL_TRIAL_COUNT=4
EXCLUDED_REAL_TRIAL_COUNT=2 (both SHORT-02 real trials; reason=return_series_date_misalignment)

SHORT_02_MODEL_VERDICT=REJECTED_NOT_ADVANCED
SHORT_02_SHORT_VALUE_VERDICT=INCONCLUSIVE
SHORT_02_LONG_ONLY_RESULT=net_total_return=0.24823 net_sharpe=0.35686 max_drawdown=-0.34773
SHORT_02_LONG_SHORT_RESULT=net_total_return=0.24823 net_sharpe=0.35686 max_drawdown=-0.34773 (execution-identical to long-only)
SHORT_02_PLACEBO_RESULT=net_total_return=0.24823 net_sharpe=0.35686 (execution-identical to real; excluded from family judge on date-alignment grounds)
SHORT_02_SHORT_EXPOSURE=0/12816 OOS rows short-eligible; ml_score in [0.57225,0.65093]

SHORT_03_MODEL_VERDICT=REJECTED_NOT_ADVANCED
SHORT_03_SHORT_VALUE_VERDICT=INCONCLUSIVE
SHORT_03_LONG_ONLY_RESULT=net_total_return=0.39431 net_sharpe=0.46220 max_drawdown=-0.35516
SHORT_03_LONG_SHORT_RESULT=net_total_return=0.39431 net_sharpe=0.46220 max_drawdown=-0.35516 (execution-identical to long-only)
SHORT_03_PLACEBO_RESULT=net_total_return=0.40931 net_sharpe=0.47348 (placebo outperforms real by +0.01500/+0.01128)
SHORT_03_SHORT_EXPOSURE=0/13464 OOS rows short-eligible; ml_score in [0.53175,0.72238]

SHORT_04_MODEL_VERDICT=REJECTED_NOT_ADVANCED
SHORT_04_SHORT_VALUE_VERDICT=INCONCLUSIVE
SHORT_04_LONG_ONLY_RESULT=net_total_return=0.42389 net_sharpe=0.48404 max_drawdown=-0.35516
SHORT_04_LONG_SHORT_RESULT=net_total_return=0.42389 net_sharpe=0.48404 max_drawdown=-0.35516 (execution-identical to long-only)
SHORT_04_PLACEBO_RESULT=net_total_return=0.42389 net_sharpe=0.48404 (execution-identical to real)
SHORT_04_SHORT_EXPOSURE=0/13464 OOS rows short-eligible; ml_score in [0.55011,0.68874]

REAL_FAMILY_DSR=0.85100 (SHORT-04 lo/ls, tied), 0.84024 (SHORT-03 lo/ls, tied) -- 4/4 admitted trials evaluable; average_pairwise_correlation=0.99976, effective_independent_trial_count=1.0007
REAL_FAMILY_PBO=0.08333 (combinatorially_symmetric_cv, 10 blocks, 252/252 combinations, num_candidates=4)

PLACEBOS_INCLUDED_IN_REAL_PBO=NO

FOLD_RESET_BENCHMARK=SHORT-02: return=0.60776 sharpe=0.62416 (1068d); SHORT-03/04: return=0.81899 sharpe=0.73261 (1132d) -- dominates every real candidate in every family

FINAL_HOLDOUT_CONSUMED=NO

BEST_DEVELOPMENT_CANDIDATE=NONE
BEST_DEVELOPMENT_CANDIDATE_REASON=No family shows the long/short candidate meaningfully improving over its own long-only control (delta=0.0 in all three families -- zero short exposure ever activated), and no family's real signal meaningfully beats its matched causal placebo (SHORT-02/04 tie exactly; SHORT-03's placebo outperforms the real signal). The long/short-improvement and placebo-beats-real conditions are both jointly required for DEVELOPMENT_PROMISING_WITH_BORROW_MODEL_LIMITATION and neither is met by any family.

CURRENT_CLASSIFIER_SHORT_POLICY_INSUFFICIENT_FOR_DIRECT_SHORT_BOOK_STUDY=YES

PRODUCTION_CODE_CHANGED=NO

FILES_CHANGED=research-py/experiments/short_wave_02/PREDECLARED_WAVE.json, research-py/experiments/short_wave_02/run_wave.py, research-py/experiments/short_wave_02/test_short_wave_02.py, docs/research/SHORT_WAVE_02_SHORT_02_RESULT.md, docs/research/SHORT_WAVE_02_SHORT_03_RESULT.md, docs/research/SHORT_WAVE_02_SHORT_04_RESULT.md, docs/research/SHORT_WAVE_02_FAMILY_REPORT.md
COMMITS=92ac16a6 (Patch A), 6b7a4313 (Patch B), 8ce0500e (Patch C), 204309a9 (Patch D), <Patch E commit, see below>

PRIMARY_PAPER_REPO_UNTOUCHED=YES
PAPER_DB_MUTATED=NO
PAPER_RUNTIME_MUTATED=NO
LIVE_ENABLED=NO
ORDERS_SUBMITTED=NO

PUSHED=NO
```
