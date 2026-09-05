# WAVE06 Alpha Candidate Campaign — Predeclaration (W06-A-CAMPAIGN-PREDECLARATION-01, repaired by W06-A-CAMPAIGN-PREDECLARATION-AUTHORITY-REPAIR-01 and -REPAIR-02)

**Mission:** WAVE06-LANE-A-ALPHA-CANDIDATE-DISCOVERY-PROMOTION-PAPER-ENTRY-01-CONTROLLER
**Base HEAD:** `e381a402481d4e704180199d9175a770d50ddfa6` (branch `wave06-alpha-candidate-paper-entry-01`)
**Status:** PREDECLARED — no candidate has been executed. This document contains no economic result, no P&L value, and no trial/economic_eval identity of any kind.

## Purpose

Freezes, before any outcome is observed, the finite Wave06 Lane A alpha-discovery campaign: a controller ceiling of 3 candidates, of which this campaign freezes exactly 2 genuinely distinct causal daily-bar US equity/ETF hypotheses (no third candidate may be added after this repair), their exact execution order (mechanically enforced, not just documented — see "Campaign order authority" below), and a machine-readable stopping/advancement policy governing them. See `research-py/experiments/wave06_campaign/PREDECLARED_CAMPAIGN.json` for the machine-checked version of everything summarized here, and `test_campaign_predeclaration.py` / `test_campaign_order_guard.py` / `test_campaign_judge_population.py` / `test_trial_identity_mutation_proof.py` for its negative-control proofs.

## Shared campaign registry and judge population

Both candidates register their real/placebo trials in ONE shared registry (`wave06_campaign/runs/run_01/registry/research.sqlite3`) under ONE shared `real_experiment_id`/`placebo_experiment_id` (`WAVE06-CAMPAIGN-ALPHA-CANDIDATE-REAL-V1` / `...-PLACEBOS-V1`), resolved from a single shared module (`wave06_campaign/campaign_identity.py`) that both candidate drivers import directly rather than redeclaring their own copy. The multiple-testing judge (`wave06_campaign/run_campaign_judge.py`) always computes its population as the union of every campaign-order candidate that has actually, legitimately registered real trials — never one candidate's family alone, and never a subset chosen after seeing which candidate "won." A candidate that registered only some of its own frozen hypothesis ids fails the judge closed rather than being silently included or excluded.

## Campaign order authority

`wave06_campaign/campaign_order_guard.py` is the sole execution-order gate, wired into both candidate drivers' `main()`. LIQ-01 is always authorized to execute first. VOL-01 is refused unless LIQ-01 has a `CANDIDATE_CLOSEOUT_STATUS.json` artifact whose cited trial ids are independently re-verified, against the shared registry, as truthfully registered and succeeded, and whose verdict is `REJECTED_NOT_ADVANCED` or `INCONCLUSIVE`. A missing, malformed, crashed, or `DEVELOPMENT_PROMISING_REQUIRES_FRESH_POINT_IN_TIME_CONFIRMATION` closeout permanently refuses VOL-01 (the campaign has stopped). Order is never inferred from filesystem directory existence.

## Advancement policy (Finding 2, repaired by REPAIR-02 Findings 1-6, 8)

`PREDECLARED_CAMPAIGN.json`'s `advancement_policy` block freezes every gate a candidate must clear, numerically, before any Wave06 result exists: absolute economic solvency (narrowly, exactly recognizing only the accepted gross-wealth-ledger-insolvency `RuntimeError` message as a policy-terminal economic rejection — any other failure leaves the candidate incomplete/BLOCKED, never a fabricated rejection), benchmark-relative excess (`REJECTED` at `<= 0.0`, `INCONCLUSIVE` in `(0.0, 0.05)`, gate cleared at `>= 0.05` — an exact, non-overlapping, gap-free partition), matched-placebo excess (`> 0.20` net Sharpe, described as a fixed pre-outcome separation margin, not an unsupported statistical claim), long-short-vs-long-only excess (`> 0.0`), DSR (`>= 0.5`, DSR's own literature-defined probability *midpoint* — `0.0` is DSR's lower bound, not its midpoint), PBO (`<= 0.5`, its own literature-standard midpoint), the genuine shuffled placebo (`mqk_research.ml.genuine_shuffled_placebo_cli`), a DSR/PBO block-count sensitivity sweep (`dsr_pbo_sensitivity_cli --block-counts 8,10,12`, the only parameter that CLI actually varies — it has no `entry_threshold` sweep), the REAL complete canonical `bkt_robustness_gauntlet_v2` P9 artifact (all nine required scenarios present, `is_complete()`/`all_applicable_passed()` both true — the block-count sensitivity/placebo/P7A-P7B checks above are additional evidence, never a substitute for this complete artifact), and P7A/P7B economic replay stress (`p7a_p7b_economic_replay_stress_cli`, real CLI-compatible `--stress-max-position-notional-usd=10000.0` absolute cap — not a nonexistent multiplier field — plus a 40% max-drawdown ceiling). No banned vague word ("non-negligible", "materially", "meaningfully", etc.) survives in either candidate's `falsification_condition` — each now points at this policy instead. The verdict written into a candidate's closeout is COMPUTED by `wave06_campaign/campaign_advancement_authority.py` from this evidence — no writer accepts a caller-supplied verdict string, and `campaign_order_guard.load_verified_closeout` independently recomputes it before trusting it. This development-stage policy is explicitly **not** a promotion bypass: it never weakens the canonical production promotion gate.

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

Per the mission's stopping rule (mechanically enforced by `campaign_order_guard.py`, not just documented): execute strictly in this order; if a candidate's registry-verified closeout classifies it `REJECTED_NOT_ADVANCED` or `INCONCLUSIVE` per the frozen advancement_policy, it is not retuned, execution proceeds to the next predeclared candidate; if a candidate's closeout classifies it `DEVELOPMENT_PROMISING_REQUIRES_FRESH_POINT_IN_TIME_CONFIRMATION`, the remaining campaign STOPS — this non-point-in-time study never itself proceeds to promotion verification; a positive result requires a new, separately-predeclared, fresh point-in-time-clean confirmation mission first. No third or fourth candidate may be substituted after seeing a result.

## Candidate LIQ-01 — Amihud-inspired daily illiquidity / price-impact proxy

- **Feature:** `illiquidity_amihud_daily_xs_rank` — cross-sectional percentile rank (computed locally in `run_wave.py`, per-`end_ts`, via the identical `groupby(...).rank(pct=True, method="average")` formula `feature_set_v1.py` already uses for `vol_rank_20`/`atr_rank_14`) of the existing, unmodified `illiquidity_amihud` column already computed by `mqk_research.features.feature_set_v1.build_feature_set_v1` — `|1-day log return| / same-day dollar volume` (NOT a 20-day average dollar volume: `feature_set_v1.py` computes `dolvol = close * vol; illiquidity_amihud = r1.abs() / dolvol`, a separate `dolvol_20` rolling column exists but is not the divisor here).
- **Mechanism:** Amihud-inspired daily illiquidity / price-impact proxy (Amihud (2002), *Illiquidity and Stock Returns: Cross-Section and Time-Series Effects*, Journal of Financial Markets 5(1):31-56, uses a PERIOD-AVERAGED daily ratio; this candidate uses the repo's existing single-day ratio, so it is inspired by, not a literal reproduction of, that construction); Pastor & Stambaugh (2003), *Liquidity Risk and Expected Stock Returns*, JPE 111(3):642-685 — investors demand compensation for holding names that are costly to trade.
- **Distinctness:** the first REGISTERED hypothesis (research_trials/economic-walk-forward identity) in this repo to use trading volume data; the separate, non-registered `research-py/experiments/exp_penny` screener already uses volume/ADV data outside this hypothesis-registration lineage, so this is not the first use of volume data in the repository's history overall.
- **Universe:** identical, byte-for-byte reused 88-symbol current-registry snapshot (`universe_id=f25e8ec952c1429af7ac3bb58169408e`) used by SHORT-WAVE-03/DISCOVERY-01. Deliberately NOT narrowed to a fixed liquid-ETF universe — an illiquidity-premium mechanism requires genuine cross-sectional liquidity variation, which an ETF-only universe would suppress by construction.
- **Data window:** `2016-01-01T00:00:00Z`..`2025-05-01T00:00:00Z`, `asof=2025-05-01`, `feed=sip`, `1Day` — byte-for-byte reuse of DISCOVERY-01's own already-discovered, pre-outcome, corporate-action-safe boundary for this exact universe (RKLB `name_change` event, 2025-05-27).
- **Walk-forward / model / cost / execution:** identical, frozen, already-accepted parameters reused verbatim from DISCOVERY-01 (`train_years=3, test_months=3, step_months=3, holdout_months=6, min_rows_per_fold=300`; `l2=0.001, lr=0.05, steps=300, standardize=True, clip_z=8.0`; commission 10bps/side; execution pricing `rust_conservative_bar_range_v1`, 5bps slippage; `rank_side_count=5`).
- **Placebo seed:** `60601` (campaign-specific, distinct from DISCOVERY-01's `4242`).

## Candidate VOL-01 — Gervais-Kaniel-Mingelgrin-inspired high-relative-volume / investor-attention evidence

- **Feature:** `vol_ratio_rank_20` — cross-sectional percentile rank of the existing, unmodified `vol_ratio` column (current-day volume divided by that symbol's own current-bar-INCLUSIVE trailing 20-day rolling-mean volume — `vol.rolling(20, min_periods=20).mean()`'s window includes the current row, an actual current-production behavior not altered by this candidate) already computed by `build_feature_set_v1`.
- **Mechanism:** Gervais, Kaniel & Mingelgrin (2001), *The High-Volume Return Premium*, Journal of Finance 56(3):877-919 (reports unusually high/low volume followed by return differences over roughly the next month; this candidate uses a daily rank/classifier construction, so it is inspired by, not a literal reproduction of, that paper); Barber & Odean (2008), *All That Glitters*, RFS 21(2):785-818 — unusually high relative volume draws investor attention, producing a documented subsequent short-horizon return premium.
- **Distinctness:** the first REGISTERED hypothesis to use this exact temporally-relative volume-surprise feature; the separate, non-registered `exp_penny` screener already uses volume/ADV data outside this hypothesis-registration lineage. Although both LIQ-01 and VOL-01 use volume data, `illiquidity_amihud_daily_xs_rank` is a cross-sectional LEVEL statistic (price-impact cost) while `vol_ratio` is a temporally-RELATIVE own-history statistic (a demand-shock/attention signal) — mechanistically and economically distinct constructs, evaluated as two separately falsifiable candidates.
- **Universe / data window / walk-forward / model / cost / execution:** identical to LIQ-01, kept unchanged so the only varying factor between this campaign's two candidates is the feature itself.
- **Placebo seed:** `60602`.

## What this predeclaration does NOT do

- No bars have been fetched, no features/targets computed, no model trained, no economic evaluation run, no judge artifact produced, and no holdout reserved on disk — `--execute` is required for any of that, and this commit does not pass it.
- No promotion evidence exists yet.
- Both candidates' maximum possible verdict is `DEVELOPMENT_PROMISING_REQUIRES_FRESH_POINT_IN_TIME_CONFIRMATION`; `PROVEN_ALPHA`, `PROMOTION_READY`, and `PAPER_ENTRY_ELIGIBLE` are structurally forbidden verdicts for this development-stage, non-point-in-time study. A positive result requires a fresh, separately-predeclared point-in-time confirmation mission before any promotion verification may begin — it never proceeds there directly.

## Checkpoint

Per the mission's PATCH A instructions, this predeclaration is a checkpoint before execution. Phase A3 (sequential candidate execution) has not begun.
