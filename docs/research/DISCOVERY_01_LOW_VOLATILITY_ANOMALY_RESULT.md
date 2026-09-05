# DISCOVERY-01-LOW-VOLATILITY-ANOMALY — Result

Part of `POST-WAVE05-ALPHA-DISCOVERY-AND-PAPER-VALIDATION-01-CONTROLLER-V2`.
Development-stage Research only. Does not authorize Paper or Live trading.
No production Paper/runtime/broker/risk/portfolio/scheduler behavior was
modified. Closed out by `DISCOVERY-01-TRUTH-CLOSEOUT-REPAIR-01` after
independent review of the committed `runs/run_01` registry/artifacts.

## 1. Hypothesis (RISK-01)

**Truthful name:** `vol_rank_20` single-feature OOS classifier-score
cross-sectional direct-rank implementation of the low-volatility
(defensive) anomaly (Ang, Hodrick, Xing & Zhang 2006; Frazzini & Pedersen
2014, "Betting Against Beta"). A fold-trained logistic classifier's OOS
score, ranking the 88-symbol seed universe by `vol_rank_20` (cross-sectional
percentile rank of 20-day rolling realized volatility of daily log
returns), predicts whether each symbol's own subsequent 10-bar log return
is positive. The model's learned coefficient/sign is fit from training data
each fold, not hand-asserted.

Predeclaration: `research-py/experiments/discovery_01_low_volatility_anomaly/PREDECLARED_WAVE.json`
(SHA-256 `28e8ab9e9b390f080d5bb2f0b341edcc43b57454459eb119e6c45f2aa4dddf5b`).
Seed universe: `SEED_UNIVERSE.json` (SHA-256
`1abcb9667bdfd3828bbdd2367a7d562b0e35fdcd0d64d68ad6d50a1d6c24a05d`,
`universe_id=f25e8ec952c1429af7ac3bb58169408e`, 88 symbols, fixed ex-ante,
non-point-in-time current-registry snapshot — same caveat as SHORT-WAVE-03,
see `docs/research/BROAD_RESEARCH_UNIVERSE_CURRENT_TRUTH_AUDIT.md`).

## 2. Identity (from the committed `runs/run_01/registry/research.sqlite3`)

| Field | Value |
|---|---|
| `real_experiment_id` | `DISCOVERY-01-LOW-VOLATILITY-ANOMALY-REAL-V1` |
| Long-only `hypothesis_id` | `discovery01_risk01_low_volatility_anomaly_long_only_v1` |
| Long-only `trial_id` | `c69748a51544f3f01a44d72777e816f6` |
| Long-only `attempt_id` | `c69748a51544f3f01a44d72777e816f6:att0001` |
| Long-only attempt `status` | `succeeded` |
| Long-short `hypothesis_id` | `discovery01_risk01_low_volatility_anomaly_long_short_v1` |
| Long-short `trial_id` | `2b1ced2305a408b95ad4375bd44bb19b` |
| Long-short `attempt_id` | `2b1ced2305a408b95ad4375bd44bb19b:att0001` |
| Long-short attempt `status` | `failed` |
| `economic_eval_id` (long-only) | `b4517016263376e64ddb8e599fc7d5e946ed9541c7740f9d8ea1ee9b562111e3` |
| `canonical_semantic_bars_hash` | `c4541b760829cf52b245880487d567ad47007c76893f3a0bfba2b1a188936165` |
| `canonical_pricing_bars_hash` | `2936ba4e70e0d83ea30896192234adbaab0c5e83a76879dabf3141226f80ec25` |
| `corporate_action_evidence_id` | `430c618019bbe80dfdcbe054563998d97f8212a31c16d44f969f27f2e7c88795` |
| `source_attestation_id` | `99586efe4226ed1a49602248d9b7c89448f6d63ecf2670453870fd74722dddb1` |
| `feature_schema.sha256` | `e3f86a62494832049087e091290adb8bd1c80cd496f25dbe4bc4ce17d6ad1e0d` |
| `features_csv.sha256` | `33f90e4d378c94c5656414b03bd85fbb9a260bbda28d63f3676d753fcca57491` |
| `targets_csv.sha256` | `a014bb1185f1300ba77d381249b0b895ee7a1d48d85d9850d552e18fb9bdaaad` |
| Provider / feed | `alpaca` / `sip`, `1Day`, price convention `alpaca_all_adjusted_v1` |
| Final data boundary | `start_utc=2016-01-01T00:00:00Z`, `end_utc=2025-05-01T00:00:00Z`, `asof=2025-05-01` |
| Holdout | `holdout_id=618e41dd305d40a0e44166a2effa7b7b`, status `reserved`, window `2024-11-01T00:00:00+00:00`–`2025-05-01T00:00:00+00:00`, `consumed_at=NULL` |

No placebo trial is registered in `research_trials`/`research_attempts` for
this experiment. `research_judge_artifacts` is empty — no judge artifact
exists.

## 3. Pre-execution amendment erratum

`PREDECLARED_WAVE.json`'s own `data.freshness_note` and this experiment's
`run_wave.py` narrate the `END_UTC` amendment as a boundary "discovered
before any bars were fetched/persisted." Independent review found this
imprecise and it is corrected here without rewriting the frozen historical
artifact:

The actual accepted production sequence in
`mqk_research.data.alpaca_historical.extract_research_bars_with_provenance`
is: (1) fetch historical bars; (2) fetch corporate actions; (3) evaluate the
fail-closed corporate-action review; (4) raise `CorporateActionReviewRequired`
if unresolved; (5) only on a successful extractor return can the experiment
driver verify and persist the returned bars. The original `END_UTC=2026-09-05`
extraction attempt therefore **may have fetched historical bars internally**
within that shared extraction authority before the corporate-action review
was evaluated. The `CorporateActionReviewRequired` exception occurred
**before the extractor returned to the experiment driver**. The driver
consequently never persisted those original-window bars and never built
features, targets, model predictions, or economic results from that refused
extraction. It is therefore correct to say the driver did not persist or
use those bars — it is **not** correct to say "no bars were fetched" at
all, since fetching happens inside the shared extraction authority ahead of
the fail-closed gate.

All final run artifacts under `runs/run_01/` (bars, features, targets,
economic evaluations) were produced only after the `END_UTC` was narrowed to
`2025-05-01` in pre-outcome amendment commit `3cf711cf52afa3e05134025c58c8fd3ce6e0cc27`
— strictly before the earliest unresolved 2025–2026 `name_change`
corporate-action event (RKLB, 2025-05-27) — and strictly before any trial
result was observed. No 2026 bar, feature, prediction, or economic result
was ever computed or evaluated by this campaign.

## 4. Exact results

### Long-only (`discovery01_risk01_low_volatility_anomaly_long_only_v1`)

| Metric | Value |
|---|---|
| `gross_total_return` | `0.4233930456738799` (+42.3393%) |
| `net_total_return` | `-0.6286894176878132` (−62.8689%) |
| `net_sharpe` | `-2.498118191577651` |
| `max_drawdown` | `-0.6325763121802461` (−63.2576%) |
| `cost_drag` | `1.052082463361693` |
| `folds_used` | `23` |
| `active_days` | `1402` |

The long-only implementation remained solvent (a full economic evaluation
completed) but finished deeply net-negative after costs.

### Long-short (`discovery01_risk01_low_volatility_anomaly_long_short_v1`) — FAILED

`attempt.status = failed`, `failure_reason`:

```
RuntimeError: Fail-closed: discrete gross wealth ledger equity is <= 0 -- cannot compute a further return fraction
```

**Correct interpretation: `LONG_SHORT_GROSS_WEALTH_INSOLVENCY`.** The
accepted `economic_walkforward.py` maintains separate gross and net wealth
ledgers. This exception is raised on `equity_gross <= 0`, **before**
transaction costs are subtracted from the separate net ledger. This is an
economic failure of the frozen long-short implementation's own gross
mark-to-market wealth path — not a transaction-cost/cost-drag failure, and
not "the long-short strategy went bankrupt on fees."

### Placebo

`NOT EXECUTED`. No placebo trial is registered for this experiment (see §2).
`MATCHED_PLACEBO_DIAGNOSTIC = NOT_COMPLETED`. No placebo statistic is
fabricated or implied anywhere in this record.

### Judge

`NOT EXECUTED`. `research_judge_artifacts` is empty for this experiment.

### Family result artifact

`NOT PRODUCED because run_family aborted at the failed long-short trial.`

## 5. Verdict

`MODEL_HYPOTHESIS_VERDICT = REJECTED_NOT_ADVANCED`

**Reason:** the frozen `falsification_condition` in `PREDECLARED_WAVE.json`
uses `and/or`, including failure of the long-short candidate to meaningfully
outperform the long-only control. The long-only implementation remained
solvent but finished at −62.87% net. The corresponding frozen long-short
implementation drove gross wealth to zero/non-positive and failed closed.
The third predeclared falsification branch (long-short does not meaningfully
outperform long-only) is therefore deterministically satisfied — a
non-positive-gross-wealth failure cannot "meaningfully outperform" a solvent
control under any reading. The missing placebo prevents the matched-placebo
diagnostic from being scored but does **not** prevent this overall rejection
verdict, since the third branch alone is sufficient and is independent of
the placebo comparison.

## 6. Scope of rejection (read carefully)

Do **not** state or imply that "the low-volatility anomaly is disproven."
This trial tested one exact implementation: a logistic classifier trained
on `vol_rank_20` alone, whose learned coefficient/sign is fit from training
data and allowed to vary fold-to-fold. The rejected mechanism is exactly:

```
vol_rank_20 single-feature OOS classifier-score cross-sectional direct-rank implementation
```

`BROADER_LOW_VOLATILITY_ANOMALY_CLAIM = NOT_ESTABLISHED`. A strictly
direction-fixed, literature-style low-volatility ranking mechanism (i.e. one
that does not re-learn its sign each fold from a base-rate-dominated
classifier) is neither proven nor disproven by this result.

## 7. Retuning prohibition

`DISCOVERY-01B = NOT AUTHORIZED`.

No future test may reduce rebalance frequency, change rank bands, change
`RANK_SIDE_COUNT`, change the universe, change the label horizon, or change
cost assumptions and represent that as "finishing" or continuing RISK-01.
Any such variant is a new post-result hypothesis requiring: a new semantic
identity; a fresh predeclaration frozen before any new result is observed;
proper multiple-testing accounting against the now-larger tested-hypothesis
population; and appropriately fresh (non-reused) evidence. It must never be
represented as a continuation of this original independent trial.

## 8. Holdout safety

`holdout_id=618e41dd305d40a0e44166a2effa7b7b`, `status=reserved`,
`consumed_at=NULL`, window `2024-11-01T00:00:00+00:00`–`2025-05-01T00:00:00+00:00`.
The holdout was not touched by this trial or by this closeout repair and
remains reserved.

## 9. Safety confirmation

`PRIMARY_PAPER_REPO_UNTOUCHED_BY_STRATEGY_EXECUTION=YES` (this closeout
repair itself only edits documentation/comments plus this result file — see
commit diff), `PAPER_DB_MUTATED=NO`, `PAPER_RUNTIME_MUTATED=NO`,
`LIVE_ENABLED=NO`, `ORDERS_SUBMITTED=NO`, `HOLDOUT_CONSUMED=NO`,
`PLACEBO_EXECUTED=NO`, `JUDGE_EXECUTED=NO`, `RISK01_RERUN=NO`. No
`research-py/src/**`, `core-rs/**`, `config/**`, or `scripts/windows/**` file
was modified.
