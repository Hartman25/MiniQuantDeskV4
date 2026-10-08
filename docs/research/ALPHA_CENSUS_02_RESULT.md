# Alpha Census-02 — Discovery Result #1

**DISCOVERY ONLY · VALIDATION_STATUS = NOT_VALIDATED · PROMOTION_AUTHORITY = NONE**

Mission: `V4-ALPHA-CENSUS-02-DISCOVERY-RESULT-01`. This is the first and only real execution of the complete frozen Census-02
Discovery campaign. Nothing here is validated, promoted, or deployable. No winner is chosen. No Paper/Live order was submitted.

## 1. Identity

| Item | Value |
|---|---|
| protocol_id | `d7a68edde045b46ff830b8f35fce571e` |
| behavior_head | `1badd9c9eec3257ac6eb7f9def9d00a0deffda54` |
| replacement freeze | `7e7e860f1a8cfe2d35217a70e5de59d414338f48` |
| result-run HEAD | `778099c6e96d9395b6cfb5e0eee27f2b8186a00e` (= origin/main; CI #634 green) |
| environment | Python 3.13.16 · NumPy 2.5.3 · Pandas 3.0.5 (exactly the frozen identity) |
| run window (UTC) | 2026-10-08 04:30 → 07:05, one uninterrupted invocation of `run_census02.py run` |
| Strategy population root | `e5c4173f9a2395b7cbb4b9960aac90c03a4c043cc94d2531497949be9ad45fda` |
| factor coordinate root | `095af73d36a5a255df5d06e8e1ae9b59ec1bb8e163d0f05b9165f0da28c774bb` |

Frozen behavior sources and authority data were byte-identical to the frozen manifest (`require_freeze()` PASS in the run process).
No source, predeclaration, or policy file was changed by this mission.

## 2. Data acquisition (frozen request contract, unchanged)

provider `alpaca` · feed `sip` · adjustment `all` · timeframe `1Day` · window `[2016-01-01, 2024-01-01)` · asof `2026-10-05` ·
extractor `extract_research_bars_with_provenance` · no IEX fallback · typed `EXCLUDED_*` symbol failures · min 252 observations.

* Seed scope 88 symbols → **88 ELIGIBLE, 0 excluded**, 0 provider failures, 0 corporate-action exclusions
  (`universe_dispositions.json`). Every symbol's provenance attests `sip / all / alpaca`.
* 155,088 daily bars; first bar end 2016-01-04, **last bar end 2023-12-29**. **Bars at/after 2024-01-01 = 0.**
  Contaminated-2024, Confirmation `[2025-01-01, 2026-03-01)` and Final Holdout `[2026-03-01, …)` rows read = **0**.
* `bars_provenance_manifest.json`: manifest_sha256 `5f3c75697fc2c31943274e09bde45a8f735999088a283a688ad299e3760ee2d2`.
  Raw vendor bars are **not** committed (disposition: left in the gitignored run directory).

## 3. Strategy executable economics (9,400 trials)

Class-C scope: 20 ETFs, 470 configs, `executable_pnl = true` for every trial (daily-bar simulator, conservative fills, costs,
100 bps/yr **frozen base borrow assumption — not historical borrow truth**). `fwd_ret` is never used as P&L here.

* Registered = 9,400 = frozen population (no missing, no extra, no duplicates). Attempts 9,400, succeeded 9,400, failed 0,
  interrupted/retried 0, max 1 attempt/trial. Non-evaluable trials: 0.
* Outcomes: **QUALIFIED 279 · NOT_QUALIFIED 7,409 · INSUFFICIENT_CLOSED_ROUND_TRIPS 1,712**.
* By side — short: 117 / 6,778 / 1,705; long_short: 162 / 631 / 7 (QUALIFIED / NOT_QUALIFIED / INSUFFICIENT).
* Trade-count bands (classification only, no veto): all trials LOW 2,136 · MODERATE 1,758 · STRONG 3,794 · below-5 1,712.
  Of the 279 QUALIFIED: LOW 198 · MODERATE 70 · STRONG 11.
* Complement-tagged: 56 configs / 1,120 trials registered and counted; 131 of the 279 QUALIFIED are complement-tagged.
  Effective-independent estimate 8,280 trials (= 9,400 − 1,120).
* Per-family / per-symbol / per-band tables: `strategy_campaign_summary.json`; edges: `strategy_edges.json`.
* SSR is FLAG_ONLY: 1,086 possible-hazard flags over 366,399 short entries; no fill was rejected or deferred.
* DSR/PBO: `DEFERRED_FULL_POPULATION` (unchanged).

**Reading the QUALIFIED count.** Qualification is the frozen rule (≥5 closed round trips and net P&L > 0 and, for short, alpha vs
passive short hold > 0; for long_short, net P&L > 0 vs cash only). 162 of 279 are long_short, whose bar is positive net P&L vs
cash over an 8-year window with a strong equity/bond drift; passive holds are DIAGNOSTIC_ONLY there, so this is **not** evidence
of market-neutral edge. 49 of the 117 qualified short trials are on TLT (a single-instrument concentration;
not decomposed by year here). 279 of 9,400 is a raw qualification count with no multiple-testing control (DSR/PBO deferred) and
cannot be read as signal count. Report-only diagnostics (year stability, regime
concentration, parameter neighborhood, drawdown) are in the committed JSON and were **not** used to select anything.

## 4. Conditional factor label evidence (1,075 factors; NOT executable P&L)

Direction `lower_is_better` (fixed; no post-result flip). Label = forward close return minus same-symbol same-horizon
unconditional mean. This is **label evidence only**; it carries no costs, borrow, or fills.

* Registered 1,075 = 215 conditions × 5 horizons (88-symbol scope). Processed 1,075; evaluable 1,065; non-evaluable 10
  (all `zero_variance_factor`, all family SH13, two per horizon) — kept in the declared population.
* 200-permutation empirical null, base seed 0, two-sided on |mean IC|; BH alpha 0.10 over the complete local family
  (declared 1,075; tested 1,065; 10 non-evaluable accounted as excluded).
* Highest class (589 factors): **WEAK 237 · MODERATE 76 · STRONG 276**. min raw p 0.004975 (= 1/201, permutation floor);
  min BH q 0.02255.
* BH q ≤ 0.10: **443 factors in either direction**; 276 of these also have favorable (direction-adjusted > 0) effect → STRONG.
  The null is two-sided, so the other 167 BH rejections are unfavorable-direction results and are not edges.
* By family/horizon (edge counts, WEAK+): SH05 99, SH07 137, SH09 94, SH06 58, SH08 58, SH12 34, SH10 28, SH11 21, SH02 17,
  SH01 14, SH03 14, SH13 12, SH04 3; by horizon h1 127, h3 153, h5 124, h10 107, h20 78. Full tables:
  `factor_campaign_summary.json`.
* Complement-related factors 140; effective-independent estimate 935 (= 1,075 − 140). Census-01 (38,192 Strategy trials,
  1,095 factors) is GLOBAL DISCLOSURE only, not a pooled BH family.
* Factor events are heavily overlapping in time and cross-sectionally dependent; BH assumes weaker dependence than present, and
  the STRONG count is therefore not a count of independent findings (complement-related factors alone are 140).
  Treat as hypothesis generation.

## 5. Result authority and limits

* VALIDATION_STATUS = NOT_VALIDATED · PROMOTION_AUTHORITY = NONE. No candidate was promoted, no `active_paper` created, no
  deployment or dynamic-selection authority changed, Confirmation and Final Holdout not consumed.
* Discovery evidence in a window that already informed Census-01. Strategy results rest on a frozen borrow *assumption*;
  individual-equity short evidence is label/hypothesis only.
* Next step requires independent review and a separately authorized evidence path (e.g., a fresh out-of-sample window that is
  not the consumed Confirmation window); none is authorized here.

## 6. Result self-review (defect census) — all findings ALREADY CORRECT + PROVEN or FIXED+PROVEN

| Check | Proof |
|---|---|
| exact Strategy / factor populations | independent SQL: 9,400 distinct trials, 1,075 distinct factors; ledger IDs == population |
| retries did not mint trials | 9,400 attempts over 9,400 trials, 1 each; 1,075 factor attempts, 1 each |
| complete-before-output | `build_outputs` ran only after both populations settled; `assert_complete_ledger` passed |
| no 2024/Confirmation/Holdout, no IEX | raw-CSV scan: max end_ts 2023-12-29; 88/88 provenance files `sip/all/alpaca` |
| q-values over full family | FDR declared 1,075 / tested 1,065 / 10 excluded with reason |
| no direction flip | all 1,075 factor records `lower_is_better` |
| no stale/mixed artifacts | run directory did not exist before the run; registries hold only this run |
| result reproduces | 210/210 sampled ledger cells (150 random + 60 QUALIFIED) recomputed byte-identically from cached bars |
| artifact hashes | `RUN_MANIFEST.json` SHA-256 of every preserved artifact reproduces; outputs byte-identical to the run directory |
| no Paper/Live/broker action | the runner only calls the historical-bars extraction path; 0 orders |

Post-review correction: two pre-result guard tests asserted that `research-py/runs/alpha_edge_census_02` must not exist, which is
no longer a valid invariant after an authorized Result #1. They are replaced by state-independent tests (import leaves the run
directory untouched whether absent or present; the freeze records zero attempts and no results; no raw run artifact is tracked in
Git). No result value, population or statistic was changed by that correction.

## 7. Preserved package

`research-py/experiments/alpha_edge_census_02/results/discovery_result_01/` (built by `results/build_result_package.py`):
`RUN_MANIFEST.json`, `campaign_disclosure.json`, `strategy_edges.json`, `conditional_edges.json`, `factor_fdr_report.json`,
`strategy_neighborhood_report_only.json`, `strategy_campaign_summary.json`, `factor_campaign_summary.json`,
`universe_dispositions.json`, `bars_provenance_manifest.json`, plus the complete result ledgers so the Discovery result stays
auditable after the gitignored run directory is gone: `strategy_trial_ledger.jsonl` (all 9,400 Strategy rows, the exact settled
chunk ledgers concatenated in frozen population order) and `factor_evidence_ledger.jsonl` (all 1,075 settled factor records,
including the 10 `not_evaluable` records with their typed reason, in frozen factor order). Both are copies of already-settled
evidence, not recomputations, and are not the registry files. Registry SQLite files, per-symbol bars, raw chunk files and
per-factor record files stay in the gitignored run directory; their SHA-256 values are in `RUN_MANIFEST.json`.
