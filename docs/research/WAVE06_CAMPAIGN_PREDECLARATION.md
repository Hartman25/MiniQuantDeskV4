# WAVE06 Alpha Candidate Campaign — Predeclaration (W06-A-CAMPAIGN-PREDECLARATION-01)

**Mission:** WAVE06-LANE-A-ALPHA-CANDIDATE-DISCOVERY-PROMOTION-PAPER-ENTRY-01-CONTROLLER
**Base HEAD:** `e381a402481d4e704180199d9175a770d50ddfa6` (branch `wave06-alpha-candidate-paper-entry-01`)
**Status:** PREDECLARED — no candidate has been executed. This document contains no economic result, no P&L value, and no trial/economic_eval identity of any kind.

## Purpose

Freezes, before any outcome is observed, the finite Wave06 Lane A alpha-discovery campaign: at most 2 genuinely distinct causal daily-bar US equity/ETF hypotheses, their exact execution order, and the stopping rule governing them. See `research-py/experiments/wave06_campaign/PREDECLARED_CAMPAIGN.json` for the machine-checked version of everything summarized here, and `research-py/experiments/wave06_campaign/test_campaign_predeclaration.py` for its negative-control proofs.

## Tested-hypothesis exclusion matrix (current-truth, as of this predeclaration)

| ID | Feature(s) | Verdict |
|---|---|---|
| ALPHA-01 | `momentum_score` | INCONCLUSIVE |
| SHORT-01 | `slope_60` | REJECTED_NOT_ADVANCED |
| SHORT-WAVE-02 | `ret_rank_20`, `ret_5`, `gap_pct_1` | REJECTED_NOT_ADVANCED |
| SHORT-WAVE-03 RANK01 | `ret_rank_20` (direct rank) | valid negative local evidence (RANK02/RANK03 not completed) |
| DISCOVERY-01 RISK-01 | `vol_rank_20` | REJECTED_NOT_ADVANCED |

Neither Wave06 candidate below reuses, thresholds, re-windows, or re-universes any of these six features — see each candidate's own `distinctness_from_prior_mechanisms` field.

## Campaign order (frozen, immutable after this commit)

1. **LIQ-01** — `research-py/experiments/wave06_candidate_liq01_amihud_illiquidity/`
2. **VOL-01** — `research-py/experiments/wave06_candidate_vol01_volume_surprise/`

Per the mission's stopping rule: execute strictly in this order; a failing candidate is not retuned, execution proceeds to the next predeclared candidate; if a candidate honestly clears every predeclared advancement gate, the remaining campaign stops and promotion verification begins for that candidate only. No third or fourth candidate may be substituted after seeing a result.

## Candidate LIQ-01 — Amihud illiquidity risk premium

- **Feature:** `illiquidity_amihud_rank_20` — cross-sectional percentile rank (computed locally in `run_wave.py`, per-`end_ts`, via the identical `groupby(...).rank(pct=True, method="average")` formula `feature_set_v1.py` already uses for `vol_rank_20`/`atr_rank_14`) of the existing, unmodified `illiquidity_amihud` column already computed by `mqk_research.features.feature_set_v1.build_feature_set_v1` (Amihud's `|1-day log return| / 20-day average dollar volume` ratio).
- **Mechanism:** Amihud (2002), *Illiquidity and Stock Returns: Cross-Section and Time-Series Effects*, Journal of Financial Markets 5(1):31-56; Pastor & Stambaugh (2003), *Liquidity Risk and Expected Stock Returns*, JPE 111(3):642-685 — investors demand compensation for holding names that are costly to trade.
- **Distinctness:** the first hypothesis in this repo's history to use trading volume data at all; every previously tested feature is a pure price-return or price-level statistic.
- **Universe:** identical, byte-for-byte reused 88-symbol current-registry snapshot (`universe_id=f25e8ec952c1429af7ac3bb58169408e`) used by SHORT-WAVE-03/DISCOVERY-01. Deliberately NOT narrowed to a fixed liquid-ETF universe — an illiquidity-premium mechanism requires genuine cross-sectional liquidity variation, which an ETF-only universe would suppress by construction.
- **Data window:** `2016-01-01T00:00:00Z`..`2025-05-01T00:00:00Z`, `asof=2025-05-01`, `feed=sip`, `1Day` — byte-for-byte reuse of DISCOVERY-01's own already-discovered, pre-outcome, corporate-action-safe boundary for this exact universe (RKLB `name_change` event, 2025-05-27).
- **Walk-forward / model / cost / execution:** identical, frozen, already-accepted parameters reused verbatim from DISCOVERY-01 (`train_years=3, test_months=3, step_months=3, holdout_months=6, min_rows_per_fold=300`; `l2=0.001, lr=0.05, steps=300, standardize=True, clip_z=8.0`; commission 10bps/side; execution pricing `rust_conservative_bar_range_v1`, 5bps slippage; `rank_side_count=5`).
- **Placebo seed:** `60601` (campaign-specific, distinct from DISCOVERY-01's `4242`).

## Candidate VOL-01 — High-volume / investor-attention return premium

- **Feature:** `vol_ratio_rank_20` — cross-sectional percentile rank of the existing, unmodified `vol_ratio` column (current-day volume divided by that symbol's own trailing 20-day average volume) already computed by `build_feature_set_v1`.
- **Mechanism:** Gervais, Kaniel & Mingelgrin (2001), *The High-Volume Return Premium*, Journal of Finance 56(3):877-919; Barber & Odean (2008), *All That Glitters*, RFS 21(2):785-818 — unusually high relative volume draws investor attention, producing a documented subsequent short-horizon return premium.
- **Distinctness:** although both LIQ-01 and VOL-01 use volume data, `illiquidity_amihud` is a cross-sectional LEVEL statistic (price-impact cost) while `vol_ratio` is a temporally-RELATIVE own-history statistic (a demand-shock/attention signal) — mechanistically and economically distinct constructs, evaluated as two separately falsifiable candidates.
- **Universe / data window / walk-forward / model / cost / execution:** identical to LIQ-01, kept unchanged so the only varying factor between this campaign's two candidates is the feature itself.
- **Placebo seed:** `60602`.

## What this predeclaration does NOT do

- No bars have been fetched, no features/targets computed, no model trained, no economic evaluation run, no judge artifact produced, and no holdout reserved on disk — `--execute` is required for any of that, and this commit does not pass it.
- No promotion evidence exists yet.
- Both candidates' maximum possible verdict is `DEVELOPMENT_PROMISING_REQUIRES_FRESH_CONFIRMATION`; `PROVEN_ALPHA` and `PROMOTION_READY` are structurally forbidden verdicts for this development-stage study.

## Checkpoint

Per the mission's PATCH A instructions, this predeclaration is a checkpoint before execution. Phase A3 (sequential candidate execution) has not begun.
