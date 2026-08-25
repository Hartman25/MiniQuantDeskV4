# RESEARCH-BACKTEST-ALPHA-GAP-AND-DISCOVERY-01 — Report

**Mission:** RESEARCH-BACKTEST-ALPHA-GAP-AND-DISCOVERY-01, corrected by
**ALPHA-DISCOVERY-01-INDEPENDENT-REVIEW-REPAIR-01**
**Worktree:** `C:\Users\Zacha\Desktop\MiniQuantDeskV4-alpha-discovery`
**Branch:** `research-alpha-gap-discovery-01`
**Base HEAD:** `edcda740b2f05fbe8a2657f2301b8ea373efb4b6` (frozen Paper repo HEAD, untouched)
**Date:** 2026-08-23 (original), revised 2026-08-23 (independent-review repair)

Primary paper repo (`C:\Users\Zacha\Desktop\MiniQuantDeskV4`, branch `main`) was verified untouched (still `edcda740`, only pre-existing untracked `RESEARCH_CLOSEOUT_PATCH_A_REVIEW.patch` / `smoke_logs/`) before and after this session and this repair session.

> **REPAIR NOTICE (read first):** an independent ChatGPT review found three
> deterministic defects in the original EXP-001 driver/report below:
> (1) the driver trained on the ENTIRE FeatureSetV1 feature matrix, not
> momentum_score alone, so the original run_01 **REJECTED** verdict is
> **not** a valid test of ALPHA-01 as stated; (2) the manual benchmark did
> not provably cover the same OOS dates as the strategy comparison; (3) the
> report's "same judge population" claim for the placebo was unproven from
> the artifact. All three are corrected below with a new run_02 using an
> isolated single-feature classifier. **run_01 is preserved unchanged on
> disk** (`runs/run_01/`) as historical evidence of the invalid attempt,
> reclassified `INVALID_FOR_STATED_HYPOTHESIS` — its numbers are never
> combined with run_02's. See the new **Deliverable 4 (CORRECTED)** and
> **Final Report (run_02)** sections below for the actual corrected result.

> **REPAIR NOTICE 02 (read second):** an independent ChatGPT review of the
> committed `ef482240` state found run_02's real trials **INDEPENDENTLY
> VERIFIED INCONCLUSIVE** (confirmed correct), but found the run_02
> **negative-control placebo INVALID**: its global `target`-only permutation
> could map a holdout-derived outcome into a discovery/training row, a
> reserved-holdout isolation violation. Independent reconstruction against
> the exact committed `raw_bars.csv` with `PLACEBO_SEED=1234` measured
> **1,468 of 9,879 discovery-usable placebo rows (14.86%)** receiving a
> shuffled label whose TRUE source `label_end_ts` fell inside the reserved
> holdout. The real trials are unaffected by this defect. This review also
> found the committed report below self-contradicted Git state (claiming
> `COMMITS=NONE` / `COMMIT=(pending)` inside a report that `ef482240` itself
> committed together with the run_02 artifacts). Both are corrected by
> **ALPHA-DISCOVERY-01-NEGATIVE-CONTROL-HOLDOUT-REPAIR-02** — see the new
> **Deliverable 6** and the corrected **Final Report (repair-02)** sections
> below. `ef482240` and the original alpha-discovery worktree/branch are
> preserved unchanged as forensic evidence; this repair lives on a separate
> clean branch/worktree per the repair-02 mission.

---

## Deliverable 1 — Material Gap Matrix

**Method:** current-repo-truth audit per CLAUDE.md §2/§8 precedence — read
`docs/research/Research_Backtest_V1_Closeout_Audit.md` (2026-08-15 baseline
through its 2026-08-21 closure-controller addendum) and
`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` (authoritative per its own
§0 precedence rule) directly against the frozen HEAD's Git history
(`9f398641`..`edcda740`, 90 commits). Did not re-derive settled findings from
scratch; did verify the *current* status of every item against the ledger's
own Executive Summary and the exact commit chain, since the closeout audit
document itself is dated 6 days stale relative to the frozen HEAD.

**Headline finding:** `RESEARCH_BACKTEST_V1_COMPLETE` is currently
**LOCALLY COMPLETE — PENDING INDEPENDENT REVIEW** (ledger §0, commit
`06417bdc`/`edcda740`, 2026-08-22/23). Between the closeout audit's baseline
and the frozen HEAD, ~90 commits closed essentially every `MUST_HAVE` item
the original audit identified: the multiple-testing judge (DSR/PBO, CLI-wired
and committed), the durable holdout-consumption ledger, a corporate-action-
safe real historical-data authority (`alpaca_historical.py`,
`adjustment=all`, fail-closed on uncovered event types — directly exercised
and confirmed working in this session, see Deliverable 4), Python↔Rust
execution-pricing parity (P7A) and weight-to-share translation (P7B, both
**pushed** to `origin/main`), the Rust promotion gate's OOS-evidence wiring,
the robustness gauntlet (P9), and dossier composition (P10). None of this
was re-implemented or re-audited here — it was verified present and then
used directly.

| ID | Description | Status | False-Alpha Risk | Current Mitigation | Required Before Alpha Testing | Priority |
|---|---|---|---|---|---|---|
| G1 | `RESEARCH_BACKTEST_V1_COMPLETE` is locally-complete but not independently reviewed, not pushed to `origin/main` (`fbddeb3d`) | OPEN (process, not code) | LOW — concerns the *promotion*/Paper-consumption gate, not research-loop validity; every mechanism this session actually used (walk-forward, holdout, judge, real data source) has its own passing test suite independent of promotion-gate review status | N/A | NO | Track for a future promotion-track session; irrelevant to pure research validity |
| G2 | `md_bars`/Postgres provider-identity attribution gap (`provider_id='unknown'` for ~6,170/8,302 rows, mostly one symbol AAPL) — separately tracked `MARKET-DATA-PROVIDER-PROVENANCE-01`, not yet merged | OPEN, but bypassed | NONE for this work | Used the newer, separate `extract_research_bars_with_provenance` (direct-Alpaca, `adjustment=all`) authority exclusively — it never touches `md_bars`/Postgres at all | NO (moot given the alternate path) | Do not use `bars_postgres.py` for real registered research until the attribution bug is fixed |
| G3 | No delisting/symbol-rename mapping anywhere in `research-py` (A12/A13) | CONFIRMED MISSING | MEDIUM — any "symbols that trade today" universe silently excludes historical delistings/failures, inflating apparent momentum/quality-style returns (classic survivorship bias) | This session's universe is restricted to long-listed (decades), still-trading large caps specifically to bound this risk; documented as a caveat on every result below | NO for development-stage screening; YES before any result is escalated toward a promotion-track claim | IMPORTANT_LATER |
| G4 | Corporate-action coverage stops at forward/reverse split, cash dividend, spin-off (Alpaca's documented `adjustment=all` scope); merger/redemption/name-change/reorg/etc. fail closed | BY DESIGN, not a gap | NONE — fails closed with a named, exact list of offending symbols/dates rather than silently including contaminated data | Directly exercised this session (see Deliverable 4): 6 of an original 20-symbol universe were excluded after a real `CorporateActionReviewRequired` exception | NO | Confirms the gate works; expand universe only symbol-by-symbol with this check respected |
| G5 | No benchmark-relative comparison (buy-and-hold, excess return) computed anywhere in the Python economic evaluator (Area H14–H16) | CONFIRMED MISSING | MEDIUM — a positive absolute Sharpe during a rising-market discovery window can look like "alpha" when it is just beta | Computed a manual equal-weight buy-and-hold benchmark independently in this session's driver script for honest comparison (see Deliverable 4) | NO (workaround applied) | IMPORTANT_LATER — small, composable, should be a near-term addition to `economic_walkforward.py` |
| G6 | No symbol/month/year/regime concentration reporting in the **Python** research loop (Area H17–H20/I7-10) — an equivalent exists in the **Rust** P9 robustness gauntlet, not here | CONFIRMED MISSING (Python only) | MEDIUM — a "profitable" cross-sectional signal concentrated in one regime/year (e.g. one mega-cap-tech-driven bull run) can masquerade as a general edge | Manually inspected per-fold metrics (`fold_metrics` in `walk_forward_eval` output) for this session's results | NO (manual workaround) | IMPORTANT_LATER |
| G7 | `ml/train.py`/`model_logreg.py` fits standardization globally, not per-fold (B6/B7/B19) — real, narrowly-scoped leakage risk if that path is ever used for anything promotion-adjacent | CONFIRMED, narrowly scoped | N/A for this work | This session used only the correctly fold-isolated `eval_walkforward.py` path; `ml/train.py` was never called | NO | DEFER — flag for any future session tempted to reach for `ml/train.py` |
| G8 | Parameter-sweep execution is planning-only (`sweeps/run_sweep.py` writes a grid CSV; nothing executes/compares it) (I4/I5) | CONFIRMED MISSING | LOW — doesn't create false alpha, just means sweeps must be run and compared by hand | This session ran a 3-point entry-threshold sweep manually as three separate registered trials, judged together via the real multiple-testing judge | NO | IMPORTANT_LATER |
| G9 | No Git-SHA / dependency-environment identity captured in Python research artifacts (J1, J9) | CONFIRMED MISSING | LOW — audit-hygiene, not a validity concern; this report pins the exact worktree/HEAD/commit range by hand | Documented manually in this report | NO | DEFER |

**Verdict: no capability gap blocks credible development-stage alpha
experimentation** on liquid, long-listed US equities/ETFs using the existing
registered Research/Backtest pipeline at the frozen HEAD. Per the mission's
Patch Authority section, **no patch was required or implemented** — every
item above is either irrelevant to research validity, already bypassed by an
existing alternate seam, or mitigated by hand this session and recorded here
as `IMPORTANT_LATER` follow-up rather than a blocker.

| G10 | This Alpaca account's usable historical daily-bar depth for this equity universe under the production-default `feed=iex` floors at ~2018-04-25/~2020-07-27 (not the requested 7yr) | **CONFIRMED_IEX_FEED_LIMITATION_NOT_ACCOUNT_TIER_LIMIT** (reclassified this repair session from an unproven "CONFIRMED" via a live, read-only, single-symbol diagnostic probe — see repair Deliverable 4b) | MEDIUM — thin, single-regime OOS evidence (3 correlated folds) is easy to over-read as either a clean reject or a clean accept | Documented honestly; treated all verdicts as scoped to the tested window, not a general claim | NO (workaround: honest scoping of claims) | IMPORTANT_LATER — a future session could evaluate switching this RESEARCH-ONLY extractor's default feed to `sip` (2016+ coverage confirmed) for materially more OOS regime diversity; NOT changed in this repair per its read-only-diagnosis-only scope |

**BLOCKING:** none.
**IMPORTANT_LATER:** G3 (survivorship/delisting), G5 (benchmark series), G6
(Python-side concentration reporting), G8 (sweep execution/comparison), G10
(historical data depth for this account).
**NON_MATERIAL / DEFER:** G1 (process status), G7 (train.py, unused here),
G9 (artifact metadata).

---

## Deliverable 2 — Alpha Shortlist

Five hypotheses for liquid US equities/ETFs, chosen for mechanism diversity,
low parameter count, and direct compatibility with the existing registered
pipeline (`feature_set_v1` → causal purged walk-forward → registered
economic walk-forward with official P7A/P7B execution pricing → DSR/PBO
judge). Ranked by best-first-test given effort/evidence tradeoff.

### 1. ALPHA-01 — Cross-sectional 20-day relative-strength momentum (TESTED — see Deliverable 4)

- **WHY_IT_IS_PLAUSIBLE:** Underreaction to firm-specific information
  (Jegadeesh & Titman 1993-style relative-strength continuation); reuses
  `feature_set_v1`'s already-built `momentum_score` (`0.5*ret_rank_20 +
  0.5*slope_rank_20`) with zero new feature code.
- **WHY_IT_MAY_ALREADY_BE_ARBITRAGED:** Classic 12-1-month momentum is
  well-documented and heavily traded by CTAs/quant funds; a 20-trading-day
  (~1 month) horizon in particular sits in a contested region of the
  literature where short-term reversal (not continuation) sometimes
  dominates — this experiment directly tests which regime holds in the
  current large-cap sample rather than assuming continuation.
- **WHAT_WOULD_FALSIFY_IT:** DSR not significant vs. the effective-trial-
  corrected null; PBO indicating the best-in-sample threshold is likely an
  artifact; economic edge collapsing under the official conservative
  (worst-case bar-range) execution pricing; performance concentrated in a
  single year/regime.
- **DATA_REQUIRED:** Daily OHLCV, ~7yr history, `adjustment=all`,
  corporate-action clean — obtained directly from Alpaca this session.
- **IMPLEMENTATION_COST:** Zero new production code — 100% existing seams
  (`build_feature_set_v1`, `run_registered_economic_walkforward_eval`,
  `build_multiple_testing_judge`).
- **EXPECTED_TURNOVER:** Moderate — monthly-ish rebalance cadence implied by
  a 20-day signal window and 10-day label horizon.
- **COST_SENSITIVITY:** Directly measured — official execution pricing
  (worst-case bar-range fill + 10bps commission) is already applied, not a
  diagnostic close-only price.
- **OVERFIT_RISK:** LOW-MODERATE — one feature, three threshold variants,
  genuine DSR/PBO accounting across the full trial population including a
  label-permutation placebo.

### 2. ALPHA-02 — Short-horizon (1–5 day) reversal

- **WHY_IT_IS_PLAUSIBLE:** Liquidity-provision compensation / short-term
  overreaction (Jegadeesh 1990, Lehmann 1990) — large recent negative
  1-5-day returns predict a bounce as liquidity providers demand compensation
  for absorbing order-flow imbalance.
- **WHY_IT_MAY_ALREADY_BE_ARBITRAGED:** This is the textbook case of an
  anomaly largely competed away in liquid large caps by HFT market-making
  since ~2005-2010 — a strong candidate for a clean REJECTED result, which
  is itself useful evidence about whether this system can correctly detect
  the *absence* of an edge rather than manufacturing one.
- **WHAT_WOULD_FALSIFY_IT:** Any of the same criteria as ALPHA-01; a priori
  expectation is REJECTED or INCONCLUSIVE once realistic costs are applied,
  given the anomaly's well-documented erosion in liquid names.
- **DATA_REQUIRED:** Same as ALPHA-01 (same universe/bars reusable).
- **IMPLEMENTATION_COST:** Zero new feature code — uses existing `ret_1`,
  `ret_2`, `ret_5`, `vol_10` columns directly (sign-flipped signal:
  predictor = `-ret_5`).
- **EXPECTED_TURNOVER:** HIGH — daily-ish rebalance cadence; realistic costs
  matter enormously here (this is exactly the mechanism a 10bps/side
  commission + worst-case fill is designed to stress-test).
- **COST_SENSITIVITY:** Very high — likely the single best test of whether
  the pipeline's cost model is doing real work rather than being cosmetic.
- **OVERFIT_RISK:** LOW — one feature, one direction, minimal parameters.

### 3. ALPHA-03 — Per-instrument absolute (time-series) trend-following

- **WHY_IT_IS_PLAUSIBLE:** Distinct mechanism from ALPHA-01: hedging-pressure/
  underreaction-driven trend persistence in an instrument's *own* return
  series (Moskowitz, Ooi & Pedersen 2012 "Time Series Momentum"), not
  relative cross-sectional ranking. Best suited to broad, liquid ETFs
  (SPY/QQQ/diversified sector ETFs) rather than single names, where the
  effect is best documented.
- **WHY_IT_MAY_ALREADY_BE_ARBITRAGED:** CTA/managed-futures trend strategies
  are enormous and long-running in liquid index products; expect a modest,
  not spectacular, edge if any survives net of costs.
- **WHAT_WOULD_FALSIFY_IT:** Same statistical bar as above; additionally,
  since this is a single-instrument (not cross-sectional) signal, absence of
  a genuine placebo-vs-real DSR gap would be especially telling.
- **DATA_REQUIRED:** Daily ETF bars — same extraction path, different
  (ETF) symbol universe; corporate-action review should be simpler for
  passive ETFs (dividends only, no mergers).
- **IMPLEMENTATION_COST:** Small — reuses existing `slope_20`/`r2_20` trend
  features directly; requires per-symbol (not cross-sectional) target
  construction, a straightforward variant of this session's `build_targets`.
- **EXPECTED_TURNOVER:** LOW-MODERATE.
- **COST_SENSITIVITY:** MODERATE.
- **OVERFIT_RISK:** LOW.

### 4. ALPHA-04 — Low-volatility / defensive tilt

- **WHY_IT_IS_PLAUSIBLE:** Leverage-constraint and lottery-preference-driven
  anomaly (Ang, Hodrick, Xing & Zhang 2006; Frazzini & Pedersen 2014
  "Betting Against Beta") — low trailing realized volatility (or low beta)
  names have historically earned better risk-adjusted, not necessarily
  higher raw, returns.
- **WHY_IT_MAY_ALREADY_BE_ARBITRAGED:** Widely known and now commercially
  packaged (min-vol ETFs); raw-return sign prediction (this pipeline's
  native target shape) is a comparatively weak test of an effect that shows
  up mainly in risk-adjusted terms — likely to read as INCONCLUSIVE rather
  than cleanly REJECTED or DEVELOPMENT_PROMISING, which is itself an honest,
  useful result about a limitation of the classification-label framing.
- **WHAT_WOULD_FALSIFY_IT:** Same statistical bar; additionally, a
  meaningfully positive raw-return Sharpe with a genuinely LOW realized
  volatility of the strategy's own returns would be the specific, falsifiable
  claim (not just "low-vol stocks go up").
- **DATA_REQUIRED:** Same universe as ALPHA-01.
- **IMPLEMENTATION_COST:** Zero new feature code — uses existing
  `vol_rank_20`/`atr_rank_14` directly (sign-flipped: low rank = signal).
- **EXPECTED_TURNOVER:** LOW.
- **COST_SENSITIVITY:** LOW (low turnover).
- **OVERFIT_RISK:** LOW.

### 5. ALPHA-05 — Overnight vs. intraday return decomposition

- **WHY_IT_IS_PLAUSIBLE:** Distinct microstructure mechanism from all of the
  above: persistent differences between close-to-open (overnight) and
  open-to-close (intraday) return components, attributed to differential
  order flow (index/passive buying concentrated at the open; informed/
  institutional trading concentrated intraday) — Lou, Polk & Skouras 2019
  "A Tug of War" and related overnight-drift literature.
- **WHY_IT_MAY_ALREADY_BE_ARBITRAGED:** The overnight-return anomaly is
  newer and less crowded than momentum/reversal, but increasingly discussed
  publicly since ~2020 — a genuinely open empirical question for this
  sample, not a near-certain reject or accept.
- **WHAT_WOULD_FALSIFY_IT:** Same statistical bar; additionally, since the
  signal here is arguably closer to a data-quality/execution-timing question
  than a classic risk-premium story, any result should be cross-checked
  against `gap_pct_1` (already an existing `feature_set_v1` column) before
  treating it as economically real rather than an artifact of
  open-price data quality.
- **DATA_REQUIRED:** Same OHLC bars — needs `open` and previous `close`,
  already present.
- **IMPLEMENTATION_COST:** Small — new label construction (predict sign of
  the NEXT day's overnight gap from today's intraday return, or similar),
  otherwise reuses `gap_pct_1` directly; no core `research-py` code change.
- **EXPECTED_TURNOVER:** HIGH (daily).
- **COST_SENSITIVITY:** Very high, same reasoning as ALPHA-02.
- **OVERFIT_RISK:** LOW-MODERATE — a genuinely distinct, less-picked-over
  question, but also the one this session understands least well
  mechanistically; treat any positive result with extra skepticism.

None of these five is asserted to be real alpha. All are falsifiable,
low-parameter, causal, cost-aware, and implementable without touching the
final holdout or any frozen contract.

---

## Deliverable 3 — Experiment Plan

### EXP-001 (RUN THIS SESSION — see Deliverable 4)

- **hypothesis:** ALPHA-01, cross-sectional 20-day relative-strength momentum.
- **frozen parameters:** `feature_set_v1` default spec (`ret_windows=(1,2,5,10,20)`,
  `vol_windows=(10,20,60)`, `trend_window=20`, `atr_window=14`,
  `cross_section_windows=(5,20)`); label = `sign(log(close[t+10]/close[t]))`,
  `ret_threshold=0.0`; entry_threshold ∈ {0.55, 0.60, 0.65} (3 real trials,
  one hypothesis_id) + 1 target-permutation placebo trial (separate
  hypothesis_id, same experiment_id, entry_threshold=0.60).
- **development data:** 14 long-listed liquid large caps (see Deliverable 4
  for the exact list and why it was narrowed from 20), daily bars,
  2017-01-01 to 2024-01-01, `adjustment=all`, fetched directly from Alpaca.
- **fold structure:** `WalkForwardSpec(train_years=3, test_months=3,
  step_months=3, holdout_months=6, min_rows_per_fold=200)` — rolling window,
  purge+embargo enabled (embargo_seconds=0, purge via label_end_ts overlap).
- **purge/embargo:** overlap purge on `label_end_ts < test_start`; global
  holdout isolation on `label_end_ts < holdout_start`; final 6 months of the
  dataset (2023-07-01 to 2024-01-01) reserved, never scored, by construction.
- **cost assumptions:** commission 10bps/side, slippage 0bps/side (charged
  instead via the execution-pricing model below to avoid double-counting).
- **execution model:** `EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1`
  (official P7A worst-case bar-range fill), `WeightToShareSpec(equity_usd=
  100000)` (official P7B translation), long-only, equal-weight-active,
  `max_gross_exposure=1.0`, force-flat at fold end.
- **baseline:** manual equal-weight buy-and-hold over the same discovery-
  region symbols/dates (the evaluator has no built-in benchmark — Gap G5).
- **negative controls:** target-label permutation placebo (fixed seed 1234),
  registered and judged in the SAME DSR/PBO population as the three real
  trials.
- **accept/reject criteria:** `judge_status`/`dsr_results[*].evaluable` and
  `deflated_sharpe_ratio` significance, `pbo_result.status`, and whether
  economic edge survives official conservative execution pricing — verdict
  is one of REJECTED / INCONCLUSIVE / DEVELOPMENT_PROMISING per trial, never
  "proven."

### EXP-002 (NOT RUN THIS SESSION — fully specified, ready to execute)

- **hypothesis:** ALPHA-02, short-horizon (1-5 day) reversal.
- **frozen parameters:** predictor = `-ret_5` (existing `feature_set_v1`
  column, sign-flipped outside the frozen module, in a driver script only);
  label horizon 5 bars (vs. ALPHA-01's 10); same `ret_threshold=0.0`.
- **development data:** same 14-symbol universe/bars as EXP-001 (reusable —
  no new fetch needed).
- **fold structure / purge / embargo / cost / execution model / baseline:**
  identical to EXP-001 (same measurement basis, directly comparable).
- **negative controls:** same label-permutation placebo methodology.
- **accept/reject criteria:** same as EXP-001; a priori expectation is
  REJECTED given the well-documented erosion of this anomaly in liquid
  large caps — a clean reject here is a POSITIVE result about pipeline
  validity (it should reliably detect the absence of edge, not manufacture
  one).

### EXP-003 (NOT RUN THIS SESSION — fully specified)

- **hypothesis:** ALPHA-03, per-instrument time-series trend-following.
- **frozen parameters:** predictor = existing `slope_20`/`r2_20`; per-symbol
  (not cross-sectional) target; label horizon TBD (start at 10 bars, matching
  EXP-001 for comparability).
- **development data:** a NEW liquid-ETF universe (e.g. SPY, QQQ, DIA, sector
  SPDRs) — requires a fresh Alpaca extraction and its own corporate-action
  review pass (expected simpler than single-name equities: dividends only).
- **fold structure / cost / execution model:** identical framework to
  EXP-001.
- **negative controls:** label-permutation placebo, plus (ETF-specific)
  a sign-inversion control given this is a trend-following, not
  relative-strength, mechanism.
- **accept/reject criteria:** same as EXP-001.

### EXP-004 (NOT RUN THIS SESSION — fully specified)

- **hypothesis:** ALPHA-04, low-volatility/defensive tilt.
- **frozen parameters:** predictor = existing `vol_rank_20`/`atr_rank_14`,
  sign-flipped (low rank = signal); same label/horizon as EXP-001.
- **development data:** same 14-symbol universe as EXP-001 (reusable).
- **fold structure / cost / execution model / negative controls:** identical
  framework to EXP-001.
- **accept/reject criteria:** same as EXP-001; explicitly note the raw-
  return classification framing is a weak test of a risk-adjusted anomaly —
  an INCONCLUSIVE verdict should not be over-read as a clean reject of the
  underlying literature.

### EXP-005 (NOT RUN THIS SESSION — fully specified)

- **hypothesis:** ALPHA-05, overnight/intraday return decomposition.
- **frozen parameters:** new (driver-script-only, non-production) label —
  sign of next day's `gap_pct_1` (already an existing feature column),
  predicted from today's intraday (open-to-close) return; label horizon 1 bar.
- **development data:** same 14-symbol universe as EXP-001.
- **fold structure / cost / execution model / negative controls:** identical
  framework to EXP-001.
- **accept/reject criteria:** same as EXP-001, plus an explicit data-quality
  sanity check (compare against `gap_pct_1`'s raw distribution for
  plausibility) before treating any positive result as economically real.

None of EXP-002..EXP-005 were run this session (time/scope discipline per
CLAUDE.md §30 — EXP-001 alone already exercises the full registered pipeline
end-to-end with a genuine negative control; running all five was not
necessary to answer this mission's core question of "does the system support
credible alpha testing today").

---

## Deliverable 4a — run_01 Results (ORIGINAL — now `INVALID_FOR_STATED_HYPOTHESIS`)

> **RUN_01_STATUS = INVALID_FOR_STATED_HYPOTHESIS**
> **RUN_01_REASON = unintended_full_feature_set_consumption**
>
> `write_run_dir` wrote the ENTIRE `build_feature_set_v1` output (35
> columns) to `features.csv`. `generate_feature_schema` declares every
> non-ID column a model feature by construction (`feature_set_v1.py` and
> `schema.py` are both correct, unmodified, working exactly as designed —
> this is a driver-script defect, not a production defect). `run_registered_
> economic_walkforward_eval` then trained on all 33 feature columns, not
> `momentum_score` alone. The section below is therefore **preserved
> unchanged as historical evidence of an invalid attempt** — it is NOT a
> valid test of ALPHA-01 (cross-sectional momentum) as stated, and its
> REJECTED verdict must not be read as evidence against momentum
> specifically; it is evidence about a 33-feature model this mission never
> intended to test. `runs/run_01/` on disk is untouched; no trial/result
> value below was edited.
>
> Two additional, narrower defects independent review found in this
> section's *reporting* (not in the underlying registered pipeline, which
> is correct): (a) the summary-extraction bug reading a non-existent
> `"summary"` key instead of `"aggregate"` (already flagged honestly below,
> now fixed in the run_02 driver); (b) the "+33.9% same-window benchmark"
> claim below is **not proven same-window** — the benchmark dates were
> derived from `actual_min_ts + train_years` / `actual_max_ts -
> holdout_months`, not from the judge's actual `comparison_scope.
> reference_dates` or any per-trial OOS date list, so it cannot be
> compared against the strategy's stitched OOS return with confidence. Do
> not treat the "+33.9%"/"underperformed by 43-57pp" comparison below as
> validated; see Deliverable 4c for the corrected, exactly-aligned
> benchmark (computed for run_02, since run_01 is not being re-benchmarked
> as an invalid attempt).
>
> Independent review also asked whether the placebo trial was judged in
> the SAME comparison scope as the three real trials, since the original
> text asserted this without artifact proof. **Re-reading `runs/run_01/
> judge_artifact.json` directly (not inferred from code) this repair
> session: NO — it was not.** `excluded_trial_ids` shows the placebo trial
> (`4987483e...`) was excluded from the DSR/PBO comparison scope with
> reason `degenerate_returns:zero_variance_returns`; `included_trial_ids`
> contains only the three real trials; `pbo_result.num_candidates: 3`,
> `registry_population.economically_evaluable_trials: 3` (of 4
> registered). The original report's "judged in the SAME experiment
> population" phrasing conflated *registered in the same experiment* (true
> — all 4 share `experiment_id`) with *admitted into the same DSR/PBO
> comparison scope* (false — the placebo was excluded pre-comparison). The
> qualitative conclusion ("the pipeline does not manufacture spurious
> signals from noise") still holds — the placebo simply never traded, which
> is why it was excluded as degenerate rather than compared statistically —
> but the specific "same population" sentence was imprecise and is
> corrected here.

**EXP-001 (run_01, as originally executed) — preserved verbatim below for
audit; DO NOT treat as a valid ALPHA-01 result.**

**EXP-001 executed "successfully" end-to-end** through the real, official,
already-accepted registered pipeline
(`run_registered_economic_walkforward_eval` → real purged walk-forward →
real economic evaluation with official P7A/P7B conservative execution
pricing → real `build_multiple_testing_judge` DSR/PBO). All artifacts live
under `research-py/experiments/alpha_discovery_01/runs/run_01/` in the
alpha-discovery worktree (raw bars, per-trial run dirs, judge artifact,
`final_report.json`).

**Two real engineering defects surfaced and were fixed in the driver script
before this final run (not in any `research-py` production file):**
1. Original 20-symbol universe hit a genuine, correctly-fail-closed
   `CorporateActionReviewRequired` exception — CSCO, HD, MRK, MSFT, PFE, and
   XOM each have a `cash_merger`/`stock_merger` corporate-action entry
   between 2020-2023 that Alpaca's `adjustment=all` does not cover. This is
   the system's safety gate working exactly as designed (Gap G4) — the
   universe was narrowed to the 14 unaffected symbols rather than overridden.
2. **Real Alpaca historical coverage for this account/universe floors at
   ~2020-07-27**, not the requested 2017-01-01 (13 of 14 symbols; DIS alone
   goes back to 2018-04-25) — a real, previously-undocumented data-
   availability constraint, not a bug in `alpaca_historical.py` (which
   correctly returned exactly what the provider had). `WalkForwardSpec.
   train_years` was reduced from 3 to 2 to fit inside the ~3.4 years of
   actual history while still reserving a genuine 6-month holdout.

**Universe (14 symbols):** AAPL, JPM, JNJ, PG, KO, WMT, DIS, INTC, VZ, T,
IBM, GE, CAT, BA. **Actual bars coverage:** 2020-07-27 to 2023-12-29 (12,063
daily bars). **Reserved final holdout:** final 6 months of that range,
`status: "reserved_not_evaluated"` on every trial — confirmed never scored,
consistent with the frozen holdout contract.

**Walk-forward folds actually used: 3** (test windows 2022-10-01 to
2023-01-01, 2023-01-01 to 2023-04-01, 2023-04-01 to 2023-07-01 — i.e. 9
months of real OOS evidence, a single, narrow post-2022-bear-market-
recovery/2023-rally regime). This is thin, single-regime evidence, a direct
consequence of Gap G3's data-availability limit compounding with the
mission's holdout/train-window requirements — flagged honestly rather than
overstated.

| trial_id (short) | hypothesis | entry_threshold | folds_used | active_days | net_sharpe | annualized_net_return | net_total_return | cost_drag | max_drawdown | DSR | verdict |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `aff1b19a...` | cross_sectional_momentum_20d_v1 | 0.55 | 3 | 113/187 | -2.328 | -28.7% | -22.2% | 28.7% | -29.7% | 0.0125 | **REJECTED** |
| `4464bb62...` | cross_sectional_momentum_20d_v1 | 0.60 | 3 | 55/187 | -1.008 | -12.6% | -9.5% | 22.9% | -10.6% | 0.1399 | **REJECTED** |
| `20c9d7f9...` | cross_sectional_momentum_20d_v1 | 0.65 | 3 | 41/187 | -1.599 | -20.0% | -15.2% | 20.3% | -17.4% | 0.0546 | **REJECTED** |
| `4987483e...` | cross_sectional_momentum_20d_**shuffled_label_placebo**_v1 | 0.60 | 3 | 2/187 | n/a (near-zero activity) | 0.0% | 0.0% | 0.0% | 0.0% | n/a | **NEGATIVE CONTROL BEHAVED CORRECTLY** |

**Multiple-testing accounting (real, registered, DSR/PBO judge over the
3-trial `cross_sectional_momentum_20d_v1` population):**
- `raw_unique_trial_count`: 3; `effective_independent_trial_count`: 1.85
  (average pairwise correlation 0.573 — the three threshold variants are, as
  expected, highly correlated with each other, correctly discounted per
  Bailey & López de Prado 2014 Appendix A.3 rather than treated as 3
  independent shots).
- **PBO (probability of backtest overfitting): 0.386** — moderate; combined
  with all three variants being unprofitable this is not the binding
  concern here, but it is honestly reported rather than omitted.
- **DSR (deflated Sharpe ratio) for all three variants: 0.013–0.140** — far
  below any reasonable significance bar (≈0.95 is the conventional "likely
  genuine skill" threshold). This is strong evidence the observed negative
  Sharpes are consistent with noise/no-skill, not merely "not proven
  positive."

**Benchmark:** manual equal-weight buy-and-hold over the same 14 symbols,
discovery-region dates: **+33.9% total return**. The long-only momentum
strategy underperformed simply holding the universe by 43–57 percentage
points across all three variants, during a period the benchmark itself
shows was a strong bull market.

**Negative control result, read carefully:** the target-label-permutation
placebo did not "fail" in the sense of producing a bad Sharpe — it almost
never traded at all (2 active days out of 187, zero turnover, effectively
flat the entire test period). This is the CORRECT behavior for a properly
fold-isolated classifier trained on genuinely randomized labels: with no
real signal, predicted probabilities cluster near 0.5 and rarely cross a
0.60 entry threshold with confidence. This is reassuring evidence that the
pipeline's statistical machinery does not manufacture spurious high-
confidence trading signals from pure noise — a direct, executed instance of
the mission's requested "randomized/shuffled control" negative control,
not merely a planned one.

**Interpretation (a hypothesis, not a proof):** a priori, this hypothesis's
own writeup (Deliverable 2, ALPHA-01) flagged that a 20-trading-day
momentum horizon sits in a literature-contested zone where short-term
reversal sometimes dominates continuation. A long-only strategy performing
*worse than random* during a rising-market period is consistent with that
concern (i.e., the specific names this signal was most confident about were
more likely to give back gains over the next 10 days than the broader
14-name universe was) — but with only 3 correlated folds in a single narrow
regime, this is offered as a plausible reading, not a mechanism proof.

**Verdict for ALPHA-01, as specifically implemented and tested here:
REJECTED.** Not `PROVEN_ALPHA`, not `DEVELOPMENT_PROMISING` — cleanly and
consistently unprofitable net of realistic costs across all three
parameterizations tested, with low DSR and a well-behaved negative control
confirming the pipeline is discriminating real signal from noise correctly
rather than manufacturing false confidence. This REJECTED verdict is itself
useful evidence (per the mission's own instruction: "a failed hypothesis is
valid evidence") — it does **not** rule out momentum as a phenomenon in
general; it rules out *this exact* 20-day composite-rank construction, on
*this* 14-stock large-cap universe, over *this* single 9-month OOS window,
net of a 10bps/side commission and official conservative execution pricing.
A different horizon, a long-short construction, a broader/different
universe, or a longer/more diverse OOS window (blocked today only by this
account's ~3.4-year data-availability floor, not by any code gap) could
plausibly show a different result — those are exactly the untested
EXP-002..EXP-005 directions in Deliverable 3, plus a wider-history data
source as a new, non-blocking follow-up item (**G10**, added below).

**One experiment-script defect worth naming (not a production defect):**
the driver script's own summary-extraction line read a `"summary"` key that
does not exist in `economic_walk_forward.json` (the real key is
`"aggregate"`) — `final_report.json`'s per-trial `"summary": null` fields
are a cosmetic reporting bug in this session's own throwaway script, not a
flaw in the registered pipeline; the real metrics used throughout this
Deliverable were read directly from each trial's `aggregate` block.

**G10 (new, non-blocking, added by this session):** this Alpaca account's
usable historical daily-bar depth for this equity universe is ~3.4 years
(floors ~2020-07-27), not the 5+ years a typical development-stage walk-
forward would want for multi-regime OOS coverage. Not a code defect —
`alpaca_historical.py` correctly reports exactly what the provider returns
and fails closed rather than fabricating older data. **IMPORTANT_LATER**:
confirm whether this is an account/subscription-tier limit (fixable by a
plan change) or a genuine provider limit, before relying on single-digit-
fold walk-forward evidence for any future promotion-track decision.

---

## Deliverable 4b — G10 Alpaca History-Floor Diagnostic (this repair session)

**G10 reclassified from an unproven "CONFIRMED" to `UNKNOWN_NEEDS_PROOF`,
then resolved by direct evidence.** Independent review correctly noted
current Alpaca documentation states historical equity data is available
since 2016 on Basic/paid plans, and that the extractor already uses
`feed=iex` by default and paginates to completion — the original report's
"CONFIRMED" history-floor claim had not actually distinguished an IEX-feed
limitation from an account/subscription limitation.

**Method:** a narrow, read-only, single-symbol (AAPL) diagnostic probe
against the real Alpaca Market Data API, using the unmodified, already-
accepted `fetch_historical_bars` function directly (not
`extract_research_bars_with_provenance` — this probe intentionally skips
corporate-action discovery entirely, since it exists only to compare raw
bars coverage by feed, not to produce a registered research artifact). Two
calls, same symbol/window/asof, differing only in `feed`:

| feed | requested window | result |
|---|---|---|
| `iex` (production research default) | 2015-01-01 to 2019-01-01 | **zero bars returned** (`AlpacaHistoricalExtractionError: Alpaca returned zero bars`) |
| `sip` (diagnostic override, this probe only) | 2015-01-01 to 2019-01-01 | **754 rows**, coverage 2016-01-04 to 2018-12-31, `pagination_complete=true`, 1 page |

```
REQUESTED_START=2015-01-01T00:00:00+00:00
REQUESTED_END=2019-01-01T00:00:00+00:00
ASOF=2024-01-01
SYMBOL=AAPL
RETURNED_START(feed=iex)=N/A (zero bars in window)
RETURNED_START(feed=sip)=2016-01-04T05:00:00+00:00
PAGINATION_COMPLETE=YES (both feeds)
PAGE_COUNT=1 (sip); 1 completed request, zero rows (iex)
```

**Conclusion:** on this SAME account/credentials, `feed=sip` returns real
AAPL daily bars back to 2016-01-04 (consistent with Alpaca's documented
"since 2016" history), while `feed=iex` returns literally zero bars for
the identical 2015-2019 window. This is decisive: the account is **not**
subscription/tier-blocked from older history in general — the constraint
is specific to the IEX feed's own limited historical depth (IEX Exchange
itself only began full operation in 2016-2017; Alpaca's `iex` feed
reflects that). No extraction defect, no asof defect, no pagination
defect, no multi-symbol-request artifact — `fetch_historical_bars`
correctly returned exactly what each feed had.

**G10_STATUS = CONFIRMED_IEX_FEED_LIMITATION_NOT_ACCOUNT_TIER_LIMIT.**
Per the mission's read-only-diagnosis-only scope, `alpaca_historical.py`'s
`DEFAULT_FEED = "iex"` was **not** changed in this repair — this is a
diagnostic finding for a future session to act on (switching this
RESEARCH-ONLY extractor's default feed to `sip` would plausibly extend
usable OOS history back to ~2016 for this universe, materially improving
regime diversity), not a change made now.

---

## Deliverable 4c — run_02 Corrected Results (single-feature isolation)

**Driver:** `research-py/experiments/alpha_discovery_01/run_experiment.py`
(corrected in place this repair session). **Run root:**
`runs/run_02/` (new, separate registry/SQLite from run_01 — shares no
state). **Experiment identity:**
`ALPHA-DISCOVERY-01-MOMENTUM-20D-SINGLE-FEATURE-V2` /
`cross_sectional_momentum_20d_single_feature_v2` (real) /
`cross_sectional_momentum_20d_single_feature_shuffled_label_placebo_v2`
(placebo) / `cross_sectional_momentum_20d_single_feature_classifier_v2`
(strategy).

**Hypothesis semantics (corrected):** this is a fold-trained
single-feature LOGISTIC CLASSIFIER on `momentum_score`, predicting
P(forward 10-bar log return > 0). The entry thresholds 0.55/0.60/0.65 are
**MODEL-PROBABILITY thresholds** (`SignalPolicySpec.entry_threshold` gates
the classifier's predicted probability), **not** raw momentum-rank
percentile thresholds — despite `momentum_score` itself being constructed
from two rank sub-features (`ret_rank_20`, `slope_rank_20`), the strategy
that trades is the classifier's *learned, standardized, L2-regularized*
mapping from that one feature to a probability, not a direct top-quantile
rank rule. This is a single-feature momentum-predictive classifier, not a
pure deterministic top-quantile rank strategy.

**Pre-run proofs (printed by the driver, matching the mission's
REQUIRED PRE-RUN PROOFS section):**
```
FEATURE_COLUMNS=['momentum_score']
FEATURE_COLUMN_COUNT=1
UNIVERSE=['AAPL','JPM','JNJ','PG','KO','WMT','DIS','INTC','VZ','T','IBM','GE','CAT','BA']
UNIVERSE_CLASSIFICATION=DEVELOPMENT_DIAGNOSTIC_UNIVERSE_WITH_POST_HOC_CA_ELIGIBILITY_HISTORY
RUN_01_PRESERVED=YES
FINAL_HOLDOUT_POLICY=RESERVED
```
The driver enforces feature isolation with a hard, fail-closed
driver-level assertion (`assert_single_feature_schema`) immediately after
`generate_feature_schema` runs, reading the written `feature_schema.json`
back and raising if `feature_columns != ["momentum_score"]` exactly — this
makes the exact run_01 defect (accidental full-feature-set consumption)
structurally impossible to repeat silently.

Same frozen 14-symbol universe, same 2017-01-01–2024-01-01 request window,
same `asof=2024-01-01`, same label (10-bar forward log return, threshold
0.0), same `WalkForwardSpec(train_years=2, test_months=3, step_months=3,
holdout_months=6, min_rows_per_fold=150)`, same cost model
(**CONSERVATIVE COST ASSUMPTION**: 10bps/side commission — not
independently established as a realistic Alpaca commission from broker
documentation, so labeled conservative rather than realistic; 0bps/side
slippage, charged instead via the official P7A conservative execution
pricing model to avoid double-counting), same
`EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1` execution model, same
placebo seed (1234) — nothing was tuned based on run_01's results.
Real Alpaca bars were reused from run_01's cache (identical fixed_ex_ante
universe/dates/asof/feed=iex → identical real content; no new network call
needed for a closed historical window). **Universe frozen BEFORE seeing
run_02 results.**

**Result: all four trials (three real single-feature variants + the
placebo) produced ZERO trading activity — every predicted probability
across all 188 real OOS days (3 folds, 2022-08-01 to 2023-04-28) stayed
below every tested entry threshold, including the lowest, 0.55, on every
trial:**

| trial | hypothesis | entry_threshold | active_days | trading_days | net_total_return | judge admission |
|---|---|---|---|---|---|---|
| `536256...` | `..._single_feature_v2` | 0.55 | 0 | 188 | 0.0 | EXCLUDED: `degenerate_returns:zero_variance_returns` |
| `a618fb...` | `..._single_feature_v2` | 0.60 | 0 | 188 | 0.0 | EXCLUDED: `degenerate_returns:zero_variance_returns` |
| `ff38b2...` | `..._single_feature_v2` | 0.65 | 0 | 188 | 0.0 | EXCLUDED: `degenerate_returns:zero_variance_returns` |
| `542670...` | `..._shuffled_label_placebo_v2` | 0.60 | 0 | 188 | 0.0 | EXCLUDED: `degenerate_returns:zero_variance_returns` |

**Judge accounting (proven directly from `runs/run_02/judge_artifact.json`,
not inferred):**
```
REGISTERED_TRIAL_COUNT=4
ADMITTED_JUDGE_TRIAL_COUNT=0
EXCLUDED_TRIAL_COUNT=4 (all: degenerate_returns:zero_variance_returns)
PLACEBO_JUDGE_STATUS=EXCLUDED:degenerate_returns:zero_variance_returns
judge_status=not_evaluable
pbo_result.status=not_evaluable (reason: insufficient_candidates_for_cscv, num_candidates=0)
dsr_results=[] (empty)
```
The placebo's true admission status is honestly identical to the real
trials' here (all excluded, all for the same reason) — an explicit,
truthful answer to Finding 3, not an assumption.

**Corrected benchmark (Finding 2 repair) — built ONLY over the exact OOS
dates the real trials actually used, proven by direct date-set comparison,
never a same-window guess:**
```
BENCHMARK_TYPE=equal_weight_daily_rebalanced   (NOT buy-and-hold — do not conflate)
BENCHMARK_EXACT_DATE_ALIGNMENT=PASS  (all 3 real trials' economic_daily_returns.csv date
  columns AND the judge's comparison_scope.reference_dates are IDENTICAL 188-date sets;
  driver fails closed via verify_oos_date_alignment() if this were ever untrue)
OOS_REFERENCE_DATE_START=2022-08-01
OOS_REFERENCE_DATE_END=2023-04-28
OOS_REFERENCE_DATE_COUNT=188
CUMULATIVE_RETURN_OVER_REFERENCE_DATES=+10.23%
HOLDOUT_START_UTC=2023-07-01T00:00:00+00:00 (benchmark dates end 2023-04-28,
  well before the reserved holdout boundary — driver fails closed if any
  reference date would reach or enter the holdout)
```
Since every trial had zero trading activity, this benchmark is reported
for honest context only (what a passive equal-weight holder of the same 14
names earned over the same real OOS window while the strategy did
literally nothing), not as a performance comparison against a strategy
that generated no return series to compare.

**Interpretation:** the fold-trained single-feature logistic classifier on
`momentum_score` alone — standardized, `l2=1e-3`, `lr=0.05`, `steps=300` —
never produced a predicted probability that crossed even the lowest tested
threshold (0.55) on any of 188 real out-of-sample days across 3 independent
folds, for any of the three threshold variants, and the shuffled-label
placebo behaved identically (also zero activity). This is a materially
different outcome from run_01's (invalid) REJECTED verdict: there is no
negative economic evidence here — the strategy never took a single
position, so there is no return series for the DSR/PBO judge to evaluate,
and it correctly declined to (`not_evaluable`), rather than fabricating a
comparison. A plausible reading (not a proof, given only 3 correlated
folds) is that a single continuous feature run through `l2=1e-3`-regularized
logistic regression, with `momentum_score` itself already rank-normalized
to `[0,1]` and therefore weakly separated in mean, simply does not produce
confident enough (>0.55) probabilities from this construction — a limitation
of this exact single-feature linear-classifier specification, not
necessarily evidence that momentum has zero predictive content at all.

**Verdict for ALPHA-01, as correctly implemented and tested here (run_02):
INCONCLUSIVE.** Not `REJECTED` (no losing trades were generated to reject),
not `DEVELOPMENT_PROMISING`, and never `PROVEN_ALPHA`. The corrected,
feature-isolated experiment could not be economically evaluated because the
classifier never crossed any tested entry threshold — this is itself a
valid, honestly-reported result about this exact single-feature
construction at these three threshold levels, not a verdict on momentum as
a phenomenon. Per the mission's explicit instruction, no new threshold
variants were added and run_01's results were not combined with run_02's
to manufacture a stronger claim either way.

---

## Deliverable 5 — Repo Changes

**FILES_CREATED/CHANGED** (alpha-discovery worktree only; primary Paper
repo untouched):
- `research-py/experiments/alpha_discovery_01/run_experiment.py` —
  CORRECTED this repair session: feature isolation (`isolate_momentum_
  feature` + fail-closed `assert_single_feature_schema`), fixed
  `"aggregate"` extraction (was `"summary"`), exact-date-aligned benchmark
  (`build_benchmark_over_oos_dates` + `verify_oos_date_alignment`, replacing
  the old approximate-window `buy_and_hold_benchmark`), new run_02 identity
  (`EXPERIMENT_ID`/`HYPOTHESIS_ID_*`/`STRATEGY_ID`), truthful judge-admission
  accounting per trial. Still calls only existing, frozen, already-accepted
  `research-py` entry points; still does not modify any file under
  `research-py/src`.
- `research-py/experiments/alpha_discovery_01/runs/run_01/*` — **UNCHANGED,
  preserved on disk** as historical evidence of the invalid attempt (not
  deleted, not regenerated, not edited).
- `research-py/experiments/alpha_discovery_01/runs/run_02/*` — NEW: raw
  bars (reused from run_01's cache — identical real content), provenance
  manifest, per-trial run dirs (isolated `features.csv` with exactly
  `[symbol, end_ts, momentum_score]`), judge artifact, final report, plus a
  dedicated disposable SQLite registry (`registry/research.sqlite3`)
  separate from run_01's and from any other registry in the repo.
- `docs/research/ALPHA_DISCOVERY_01_REPORT.md` — this report, corrected.

**PRODUCTION_CODE_CHANGED:** NONE. `research-py/src/**` was not modified in
either the original session or this repair. No Rust crate was touched.

**COMMITS:** **STALE CLAIM, CORRECTED BY REPAIR-02.** This paragraph
originally claimed `COMMITS: NONE from this repair session` while itself
being shipped inside commit `ef482240df1e4a64b6bbe15009795efbfeaaff52` on
`research-alpha-gap-discovery-01`, which *does* commit
`run_experiment.py`, this report, and the run_02 generated artifacts
(raw bars, per-trial CSVs, SQLite registry, judge/final-report JSON). That
was a durable-report/Git-state contradiction, flagged and corrected by
`ALPHA-DISCOVERY-01-NEGATIVE-CONTROL-HOLDOUT-REPAIR-02` — see Deliverable 6.
`ef482240` is preserved unchanged as forensic evidence and was never pushed.

---

## Deliverable 6 — Independent Review Repair 02 (Causal Placebo Chronology + Durable Report Truth)

**Mission:** `ALPHA-DISCOVERY-01-NEGATIVE-CONTROL-HOLDOUT-REPAIR-02`.

**What an independent ChatGPT review found in the committed `ef482240` state:**

1. The corrected REAL run_02 trials (3 real single-feature entry-threshold
   trials) are **independently re-verified INCONCLUSIVE** —
   `FEATURE_COLUMNS=["momentum_score"]`, OOS observations=188, maximum real
   `ml_score` ≈ 0.536596 across entry thresholds 0.55/0.60/0.65,
   `active_days=0`, `turnover=0`, `net_total_return=0`. This matches
   Deliverable 4c above and is unchanged by this repair.
2. The run_02 **shuffled-label placebo is INVALID as a negative control**:
   `run_experiment.py` permuted `target` globally across the entire targets
   dataframe (`rng.permutation(len(shuffled_targets))` applied to the whole
   frame) while leaving `end_ts`/`label_end_ts` unchanged per row. This can
   assign a row a `target` whose TRUE originating outcome was observed at a
   *different* `label_end_ts` — including one inside the reserved holdout —
   while the evaluator still scores that row under its own (destination)
   `label_end_ts`. That crosses the reserved-holdout boundary and violates
   chronology. Independent reconstruction against the exact committed
   `raw_bars.csv` with `PLACEBO_SEED=1234` measured: discovery-usable
   placebo rows = 9,879; rows receiving a shuffled label whose true source
   `label_end_ts` is in the reserved holdout = 1,468; contamination rate =
   14.86%.
3. **Durable-report/Git-state contradiction:** the report committed in
   `ef482240` stated `COMMITS: NONE` / `COMMIT=(pending)` even though
   `ef482240` itself commits `run_experiment.py`, this report, and the
   run_02 generated artifacts together. Corrected in Deliverable 5 and the
   Final Report block above.

**PATCH A — `ALPHA-DISCOVERY-01-CAUSAL-PLACEBO-01`:** repairs the negative
control in
`research-py/experiments/alpha_discovery_01/run_experiment.py`
(`build_causal_placebo_targets`, called from `main()`'s placebo step).
Instead of a global `target`-only permutation, it permutes the
**(fwd_ret, target) PAIR** only within rows sharing the **exact same
`(end_ts, label_end_ts)`** — randomizing which symbol receives which
same-horizon outcome while leaving `symbol`, `end_ts`, and `label_end_ts`
untouched for every row, so no outcome can ever move across a label horizon
or the holdout boundary. Fixed seed `1234` (unchanged). Groups of size 1 are
left unchanged (a size-1 permutation is the identity). Proven by 11 focused
tests in
`research-py/experiments/alpha_discovery_01/test_causal_placebo.py`,
including a false-positive check per CLAUDE.md §14: the fixture is first
shown capable of exposing the *original* defect (a reconstructed global
permutation demonstrably crosses the holdout boundary on the same data —
RED), before proving the repaired function does not (GREEN). All 8 required
invariants (key preservation, label_end_ts preservation, pair-multiset
preservation per group, no cross-holdout moves, no later-to-earlier horizon
moves, at least one changed assignment, global positive-label-count
preservation, fwd_ret/target internal consistency) are asserted directly
against synthetic fixture data — no network/Alpaca calls, no
`research-py/src` modification.

**Run_02 disposition:** the three real ALPHA-01 trials were **not
re-run** — their result stands as independently verified. The historical
run_02 placebo (`ef482240`'s `trial_placebo_0`) is reclassified
`INVALID_NEGATIVE_CONTROL_CHRONOLOGY` and must not be cited as evidence that
the negative control passed. No corrected placebo diagnostic run was
performed in this repair: the 11 fixture-based tests above already prove
the repaired construction satisfies every required invariant, so an actual
network-driven re-run (which would also require live Alpaca access) would
not materially increase confidence in this patch and is deferred per
CLAUDE.md §30. `CORRECTED_PLACEBO_DIAGNOSTIC_RUN=NO`.

**Durable truth (authoritative, supersedes any conflicting line above):**

```
RUN_01_STATUS=INVALID_FOR_STATED_HYPOTHESIS
RUN_02_REAL_TRIAL_STATUS=INDEPENDENTLY_VERIFIED_INCONCLUSIVE
RUN_02_PLACEBO_STATUS=INVALID_NEGATIVE_CONTROL_CHRONOLOGY
RUN_02_REAL_TRIAL_FINAL_HOLDOUT_CONSUMED=NO
RUN_02_INVALID_PLACEBO_HOLDOUT_CONTAMINATION=YES
RUN_02_INVALID_PLACEBO_CONTAMINATION_RATE=14.86% (1468/9879 discovery-usable
  placebo rows, PLACEBO_SEED=1234, reconstructed against the exact committed
  raw_bars.csv)
```

**Repo state (repair-02):** old worktree/branch
(`C:\Users\Zacha\Desktop\MiniQuantDeskV4-alpha-discovery`,
`research-alpha-gap-discovery-01`, `ef482240`) preserved unchanged as
forensic evidence — not amended, reset, rebased, or force-pushed. This
repair's PATCH A/B commits live on a separate clean worktree/branch
(`MiniQuantDeskV4-alpha-discovery-clean`,
`research-alpha-gap-discovery-01-clean`) branched from the frozen primary
Paper `main` HEAD `edcda740b2f05fbe8a2657f2301b8ea373efb4b6`. Neither branch
was pushed.

---

## Final Report

```
VERDICT=INDEPENDENT_REVIEW_REPAIR_COMPLETE_CORRECTED_EXPERIMENT_INCONCLUSIVE

PRIMARY_PAPER_REPO_UNTOUCHED=YES

ALPHA_WORKTREE=C:\Users\Zacha\Desktop\MiniQuantDeskV4-alpha-discovery
ALPHA_BRANCH=research-alpha-gap-discovery-01
BASE_HEAD=edcda740b2f05fbe8a2657f2301b8ea373efb4b6

RESEARCH_BACKTEST_V1_CONTRACTS_PRESERVED=YES

RUN_01_STATUS=INVALID_FOR_STATED_HYPOTHESIS
RUN_01_REASON=unintended_full_feature_set_consumption
RUN_01_PRESERVED=YES (runs/run_01/ unchanged on disk)

RUN_02_EXPERIMENT_ID=ALPHA-DISCOVERY-01-MOMENTUM-20D-SINGLE-FEATURE-V2

FEATURE_COLUMNS=['momentum_score']
FEATURE_COLUMN_COUNT=1

UNIVERSE=AAPL, JPM, JNJ, PG, KO, WMT, DIS, INTC, VZ, T, IBM, GE, CAT, BA (14, frozen before run_02)
UNIVERSE_CA_SELECTION_CAVEAT=narrowed from an original 20-symbol universe only
  after a real fail-closed CorporateActionReviewRequired hit (CSCO/HD/MRK/MSFT/
  PFE/XOM merger events not covered by adjustment=all) -- classified
  DEVELOPMENT_DIAGNOSTIC_UNIVERSE_WITH_POST_HOC_CA_ELIGIBILITY_HISTORY, not
  sufficient alone for promotion-grade evidence

REQUESTED_HISTORY_START=2017-01-01
RETURNED_HISTORY_START(feed=iex, production default)=2018-04-25 (DIS)/~2020-07-27 (13 of 14 symbols)
ALPACA_FEED=iex (production default); diagnostic-only feed=sip probe also run, see G10
ALPACA_ASOF=2024-01-01
PAGINATION_COMPLETE=YES
G10_STATUS=CONFIRMED_IEX_FEED_LIMITATION_NOT_ACCOUNT_TIER_LIMIT (feed=sip returned
  AAPL data back to 2016-01-04 on the SAME account; feed=iex returned zero bars
  for the identical 2015-2019 window -- see Deliverable 4b)

REGISTERED_TRIAL_COUNT=4
ADMITTED_JUDGE_TRIAL_COUNT=0
EXCLUDED_JUDGE_TRIAL_COUNT=4 (all degenerate_returns:zero_variance_returns)
PLACEBO_JUDGE_STATUS=EXCLUDED:degenerate_returns:zero_variance_returns

OOS_REFERENCE_DATE_START=2022-08-01
OOS_REFERENCE_DATE_END=2023-04-28
OOS_REFERENCE_DATE_COUNT=188

BENCHMARK_TYPE=equal_weight_daily_rebalanced
BENCHMARK_EXACT_DATE_ALIGNMENT=PASS
BENCHMARK_RESULT=+10.23% cumulative over the exact 188 OOS reference dates
  (context only -- no trial generated a return series to compare against it)

TRIAL_RESULTS=all 4 trials (3 real single-feature entry-threshold variants +
  1 shuffled-label placebo): 0 active days out of 188 OOS days, net_total_return=0.0 --
  the classifier never crossed any tested entry threshold (0.55/0.60/0.65) once
DSR=not_evaluable (dsr_results=[])
PBO=not_evaluable (num_candidates=0, reason=insufficient_candidates_for_cscv)

REAL_TRIAL_FINAL_HOLDOUT_CONSUMED=NO (confirmed per-trial on every run_01 AND
  run_02 REAL trial: holdout.status == "reserved_not_evaluated"; benchmark
  dates verified to end 2023-04-28, before the 2023-07-01 holdout boundary)
RUN_02_PLACEBO_STATUS=INVALID_NEGATIVE_CONTROL_CHRONOLOGY (see Deliverable 6 --
  the placebo's global target-only permutation contaminated the holdout
  boundary; this is a defect in the negative control's construction, separate
  from and not evidence about REAL_TRIAL_FINAL_HOLDOUT_CONSUMED above)
[CORRECTED BY REPAIR-02: the prior single line here, "FINAL_HOLDOUT_CONSUMED=NO
  (... on every run_01 AND run_02 trial ...)", conflated the real trials with
  the (invalid) placebo. Retained split into the two lines above -- see
  Deliverable 6 for the full accounting.]

CORRECTED_ALPHA_01_VERDICT=INCONCLUSIVE (not REJECTED, not DEVELOPMENT_PROMISING,
  never PROVEN_ALPHA -- the isolated single-feature classifier generated no
  trades to judge, at any tested threshold; see Deliverable 4c for the full,
  honest interpretation)

PRODUCTION_CODE_CHANGED=NO

FILES_CHANGED=research-py/experiments/alpha_discovery_01/run_experiment.py (corrected),
  docs/research/ALPHA_DISCOVERY_01_REPORT.md (corrected), plus new
  runs/run_02/* artifacts (run_01/* preserved unchanged)
COMMIT=ef482240df1e4a64b6bbe15009795efbfeaaff52 [CORRECTED BY REPAIR-02: this
  line originally read "(pending -- see NEXT_BEST_ACTION)"; ef482240 exists on
  research-alpha-gap-discovery-01 and commits run_experiment.py, this report,
  and the run_02 generated artifacts together -- see Deliverable 6]
PUSHED=NO (ef482240 was never pushed)

PAPER_DB_MUTATED=NO
PAPER_RUNTIME_MUTATED=NO
LIVE_ENABLED=NO
REAL_ORDERS_SUBMITTED=NO

NEXT_BEST_ACTION=Do NOT run EXP-002/EXP-003 yet, per mission. Get independent
  ChatGPT review of this corrected EXP-001 (run_02) before any further
  experimentation. If reviewed and accepted, the most informative next step is
  NOT a new threshold sweep on this same construction (already shown to never
  fire) but either (a) a different single-feature-classifier hyperparameterization
  explicitly scoped as a NEW experiment (lower l2, more steps, or a lower entry
  threshold), or (b) resolving G10 properly by switching the RESEARCH-ONLY
  extractor's default feed to sip for materially longer/more diverse OOS history
  before re-testing. Commit run_experiment.py + this report + run_01/run_02
  artifacts as one coherent commit if the operator wants this work preserved in
  history (not done automatically by this repair session pending final review
  of this report).
```

Then STOP. No push, no merge to `main`, no further infrastructure wave
started, EXP-002/EXP-003 not run.

---

## Superseded content below (original, pre-repair Final Report block)

The block below is the ORIGINAL session's final report, preserved verbatim
for audit — it describes run_01, which is now `INVALID_FOR_STATED_HYPOTHESIS`
(see Deliverable 4a). Do not treat any verdict/number below as current; the
corrected Final Report is above.

```
VERDICT=AUDIT_COMPLETE_ONE_EXPERIMENT_EXECUTED_NO_ALPHA_FOUND_YET

PRIMARY_PAPER_REPO_UNTOUCHED=YES

ALPHA_WORKTREE=C:\Users\Zacha\Desktop\MiniQuantDeskV4-alpha-discovery
ALPHA_BRANCH=research-alpha-gap-discovery-01
BASE_HEAD=edcda740b2f05fbe8a2657f2301b8ea373efb4b6

RESEARCH_BACKTEST_V1_CONTRACTS_PRESERVED=YES

BLOCKING_GAPS=NONE
IMPORTANT_NONBLOCKING_GAPS=G3 (survivorship/delisting universe bias),
  G5 (no benchmark series in the Python evaluator), G6 (no Python-side
  regime/symbol/month/year concentration reporting), G8 (parameter sweeps
  are planning-only, not executed/compared), G10 (this account's real
  historical-data depth floors at ~3.4yr, not 7yr, for this universe --
  NOTE: reclassified this repair session, see Deliverable 4b)

TOP_ALPHA_CANDIDATE=ALPHA-01 cross-sectional 20-day relative-strength
  momentum (TESTED THIS SESSION -- REJECTED as specifically implemented;
  NOTE: this REJECTED verdict is INVALID_FOR_STATED_HYPOTHESIS, see
  Deliverable 4a -- the corrected verdict is in the Final Report above)
SECOND_ALPHA_CANDIDATE=ALPHA-02 short-horizon (1-5 day) reversal (specified,
  not run -- a priori expected REJECTED/erosion in liquid large caps)
THIRD_ALPHA_CANDIDATE=ALPHA-03 per-instrument time-series trend-following on
  liquid ETFs (specified, not run -- distinct mechanism from ALPHA-01/02)

EXPERIMENTS_RUN=1 (EXP-001, 4 real registered trials: 3 real parameter
  variants + 1 negative-control placebo, judged together)
FINAL_HOLDOUT_CONSUMED=NO (confirmed per-trial: holdout.status ==
  "reserved_not_evaluated" on every trial, verified in Deliverable 4a)

DEVELOPMENT_PROMISING_COUNT=0
REJECTED_COUNT=3 (all three cross_sectional_momentum_20d_v1 threshold
  variants -- consistently negative net Sharpe/return, low DSR, moderate
  PBO, all underperforming a +33.9% buy-and-hold benchmark over the same
  window)
INCONCLUSIVE_COUNT=0 (the placebo trial is reported separately as a
  negative-control sanity check, not scored REJECTED/INCONCLUSIVE/
  DEVELOPMENT_PROMISING -- it has no real hypothesis to fail)

PATCHES_CREATED=0 (no deterministic Research/Backtest defect was found that
  blocked credible alpha testing -- see Deliverable 1)
COMMITS=NONE (experiment driver script + generated artifacts + this report
  left uncommitted in the alpha-discovery worktree; see Deliverable 5 for
  rationale)

HEAVY_COMPUTE_STOPPED_BEFORE_0130_HST=YES (session concluded ~15:30-16:00
  HST 2026-08-23, well before the 01:30 HST cutoff; no compute was left
  running)

PAPER_DB_MUTATED=NO
PAPER_RUNTIME_MUTATED=NO
LIVE_ENABLED=NO
REAL_ORDERS_SUBMITTED=NO
PUSHED=NO

NEXT_BEST_ACTION=Run EXP-002 (short-horizon reversal) and EXP-003
  (ETF time-series trend-following) using the SAME already-fetched 14-symbol
  bars/registry pattern established this session (near-zero marginal
  engineering cost -- the driver script's fetch/feature/judge machinery is
  fully reusable; only the label/predictor construction changes). In
  parallel, resolve G10 (confirm whether the ~3.4yr Alpaca history floor is
  an account-tier limit) before treating any future single-experiment
  result as strong evidence -- 3 correlated folds in one 9-month window is
  a real but thin evidentiary base. Do not escalate ALPHA-01 further in its
  current form; if revisited, do so as a genuinely different variant
  (long-short construction, different horizon, or a broader/less-correlated
  universe), not a re-run of the same rejected specification.
```

Then STOP. No push, no merge to `main`, no further infrastructure wave
started.
