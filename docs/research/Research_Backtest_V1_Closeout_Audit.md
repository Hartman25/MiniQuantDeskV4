# Research + Backtest V1 Closeout Audit

**Mission:** RESEARCH-BACKTEST-V1-CLOSURE-AND-MULTIASSET-EXTENSIBILITY-AUDIT-01
**Type:** Read-only architecture/capability audit. No code, tests, config, or DB state was modified. Nothing was committed or pushed.
**Date:** 2026-08-15

---

## 1. Executive Verdict

The US-equity/ETF Research + Backtest stack is **materially more complete than the candidate roadmap (Patch 6–10) assumes**. The three "accepted foundation" patches (multisymbol market frame, future-execution causality, purged holdout, experiment registry, economic walk-forward) hold up under adversarial re-inspection — no new deterministic contradiction was found in any of them. Beyond that:

- **PATCH 6 (RESEARCH-MULTIPLE-TESTING-JUDGE-01) is already implemented**, sitting uncommitted in the working tree (`research-py/src/mqk_research/ml/multiple_testing_judge.py`, `multiple_testing_stats.py`, `research-py/tests/test_multiple_testing_judge.py`). All 20 tests pass (`python -m pytest tests/test_multiple_testing_judge.py -q` → `20 passed in 9.60s`), including genuine negative controls (a mutation test that proves substituting attempt-count for trial-count is detectable). It computes DSR (Bailey & López de Prado 2014) and PBO/CSCV (Bailey, Borwein, López de Prado & Zhu 2017) from the durable trial registry, correctly excludes the holdout, and is deterministic and row/column-order invariant. **Remaining work is a commit and a CLI/pipeline wiring pass, not new statistical implementation.**
- **The Rust promotion gate does not consume walk-forward/out-of-sample evidence at all** — confirmed directly in `core-rs/crates/mqk-promotion/src/evaluator.rs` (metrics-threshold + stress-suite + provenance gates only). This is *already independently tracked* in `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` as `PROMOTION-WALKFORWARD-GATE-WIRING-01` (P1, READY), called "the single largest correctness gap in the research→promotion pipeline" by the ledger itself. This is the real substance behind candidate PATCH 7 and should be folded into it rather than treated as a separate, competing initiative.
- **Two confirmed, previously-undocumented gaps** materially matter for V1 closure: (1) no durable *holdout-consumption* ledger exists anywhere — nothing would stop the same reserved holdout window from being scored twice across research runs, though today nothing scores it at all; (2) no price/adjustment-methodology provenance is tracked on ingested bars (`bars_postgres.py` silently picks whichever of `close`/`adj_close`/`close_adj` is present first, with no record of which).
- **Multi-asset extensibility posture is unusually good.** The instrument model, contract-multiplier economics seam, `AssetClass` enum, and calendar abstraction were all built *ahead of* actual non-equity enablement and fenced off with fail-closed gates — this is the correct shape, not scope creep. The one real "would require touching core code" coupling is `mqk-portfolio`'s `Fill`/`Lot`/accounting math, which has no multiplier field and is shared by the live/paper runtime. That coupling is scoped to one crate, not smeared across the codebase, and is safe to defer.
- **Robustness (Area I / candidate PATCH 9) is the most genuinely unstarted area** — confirmed absent: cost-multiplier stress, execution-delay stress, symbol leave-one-out, concentration reporting, negative-control/placebo tests, capacity/breakeven analysis. The Rust backtest engine already has an equivalent (`scenario_stress_battery_gate.rs`, conservative worst-case fills), so this patch has a concrete target to mirror.
- **Test quality is stronger than the audit's own premise assumed.** Area P asked whether critical modules only have positive-path tests. They do not: purged walk-forward, the experiment registry, and the economic walk-forward evaluator all contain genuine negative controls — leak-detection mutation tests, chronology-mutation tests, duplicate-rejection tests, and order-invariance proofs.

**Verdict on the candidate roadmap:** directionally correct but incompletely scoped. PATCH 6 needs closure, not construction. PATCH 7 needs to absorb the already-tracked promotion-gate wiring gap and an execution-price-model reconciliation the roadmap didn't anticipate. A new small patch (holdout-consumption ledger) is required and isn't in the roadmap at all. See Section 11 for the reconciled list.

---

## 1A. WAVE-1 CLOSURE ADDENDUM (2026-08-15)

Mission `RESEARCH-BACKTEST-V1-AUTONOMOUS-CLOSURE-WAVE-01` executed four patches
against this audit's roadmap, sequentially, one commit per patch, all local
(nothing pushed). This addendum is the authoritative reconciliation the
mission required; Sections 11-17 below have been updated in place to match
it, but this addendum is the fastest place to read the net result.

**Two roadmap corrections applied before implementation** (per mission
brief, not re-derived from a fresh audit pass):

- **Correction 1 — new patch P6C, `RESEARCH-LEGACY-TRAINING-BOUNDARY-01`.**
  This audit's Section 6/Area B6-B7 finding (global-fit standardization in
  `ml/train.py` is a real, narrowly-scoped leakage risk if ever mistaken for
  OOS evidence) was documented but not carried into Section 12's patch list.
  It is now its own patch, sequenced between P6B and P8.
- **Correction 2 — P7 (`RESEARCH-PARITY-BRIDGE-01`) split into three.**
  The original P7 bundled three materially distinct invariants (execution
  pricing parity, weight-to-share order translation, and the promotion gate's
  OOS-evidence consumption) into one patch. It is now three:
  **P7A** `RESEARCH-EXECUTION-PRICING-PARITY-01`,
  **P7B** `RESEARCH-WEIGHT-TO-SHARE-PARITY-01`,
  **P7C** `PROMOTION-OOS-EVIDENCE-GATE-01`. None of the three were
  implemented in Wave 1 (out of scope per mission brief) — this is a roadmap
  correction only.

**Patches executed, in order:**

| Patch | Verdict | Commit (local, unpushed) |
|---|---|---|
| P6-CLOSURE `RESEARCH-MULTIPLE-TESTING-JUDGE-01-CLOSURE` | **CLOSED** | `fbb63fc7` |
| P6B `RESEARCH-HOLDOUT-CONSUMPTION-LEDGER-01` | **CLOSED** | `686614cc` |
| P6C `RESEARCH-LEGACY-TRAINING-BOUNDARY-01` | **CLOSED** | `c643588d` |
| P8 `BKT-DATA-PROVENANCE-POINT-IN-TIME-01` | **CLOSED** (narrowly scoped — see below) | `2b87c400` |

Per the audit/repo-truth rules, "CLOSED" here means committed code + committed
tests + passing at the time of this commit on this developer's machine — it is
**not** a CI-verified claim (no CI run has been observed as part of this
mission). Full `research-py` suite: **1182 passed, 5 skipped, 2 deselected**
(the 5 skips are pre-existing DB-proof/optional tests unrelated to this Wave,
now joined by 2 new opt-in DB-proof tests added by P8 that also skip by
default — see P8 below).

**P6-CLOSURE.** The uncommitted judge implementation (`multiple_testing_judge.py`,
`multiple_testing_stats.py`) was reviewed, not rubber-stamped: re-verified the
DSR/PBO math, the comparison-scope resolution, the trial-population
accounting, and the negative controls the audit's Section 5/Area E already
described. Added the one genuinely missing piece — CLI/pipeline wiring — as a
new `judge` subcommand on the existing `mqk-exp-dist` console script (reuses
the same `root -> default_db_path(root)` convention every other subcommand
uses; no second orchestration framework). One new test
(`test_cli_judge_command_writes_artifact_matching_direct_call`) proves the
wiring end to end. 21/21 judge tests pass; `test_experiment_registry.py` (47)
and `test_economic_walkforward.py` (63) regressions unaffected.

**P6B.** New `research_holdout_ledger` table in the same SQLite registry
(`ResearchResultStore`), with `reserve_holdout`/`consume_holdout`/`get_holdout`
methods mirroring the existing `register_trial`/`begin_attempt`/`finalize_attempt`
atomic-transaction convention (`BEGIN IMMEDIATE`, fail-closed on an already-terminal
state). `holdout_id` is a pure content hash
(`mqk_research.ml.holdout_ledger.compute_holdout_id`) of dataset identity +
holdout boundary + protocol version only — never a result value. Purely
additive: no caller in the repository invokes `consume_holdout` as of this
patch (verified by a structural grep-based test), and `eval_walkforward.py`
/ `economic_walkforward.py` were not touched. 11 new tests, including a
12-thread concurrent-consume race proving exactly one winner.

**P6C.** New `mqk_research.ml.evidence_boundary` module: a fail-closed
structural classifier (`classify_research_artifact`) distinguishing
`PROMOTION_GRADE_OOS_EVIDENCE_CLASS` (schema_version + required-shape match
for `walk_forward_eval_v2` or `economic_walk_forward_v1`) from
`DIAGNOSTIC_OR_FIT_ONLY_EVIDENCE_CLASS` (everything else, including
`ml/train.py`'s single-shot `ml_train_meta_v1` output, which now also
self-labels `evidence_class: "diagnostic_or_fit_only"` for human/tooling
visibility only). Deliberately does **not** trust that self-declared label as
the enforcement mechanism — the required negative control
(`test_negative_control_relabeling_alone_does_not_pass_the_gate`) proves that
flipping only `evidence_class` or only `schema_version` on a real train.py
artifact still fails the gate, because the required structural shape (a
non-empty `folds` list, `holdout` reserved-not-evaluated literal, and the
producer's other required top-level keys) is absent. `eval_walkforward.py`
and `economic_walkforward.py` were not modified at all — zero risk to the
accepted foundation.

**P8 — factual findings (read-only investigation before any code change):**

- **Actual `md_bars` schema** (migrations `0003_backtest_schema.sql` +
  `0042_md_bars_provider_metadata.sql`, confirmed directly against the local
  paper Postgres at `127.0.0.1:5440/miniquantdesk_paper` via read-only
  `information_schema.columns` query): `open_micros, high_micros, low_micros,
  close_micros, volume, is_complete, ingested_at, provider_id, provider_source,
  provider_symbol, ingest_mode, provider_bar_id, provider_updated_at_utc`.
  **There has never been an `adj_close`/`close_adj` column in this schema** —
  `bars_postgres.py`'s candidate-column list includes them defensively, but
  today's actual data has no ambiguity for `_pick_first_present` to have
  silently resolved; it always resolves to `close_micros`.
- **Actual price convention:** the only active equity historical-data
  provider in this codebase (`core-rs/crates/mqk-md/src/alpaca_provider.rs`)
  explicitly requests Alpaca's `/v2/stocks/bars` with `adjustment=raw`.
  Verified against Alpaca's own documentation
  (docs.alpaca.markets/reference/stockbars, fetched 2026-08-15, via
  Firecrawl): `raw: no adjustments` — split/dividend/spin-off adjustments are
  **not** applied. Code and provider documentation agree; no disagreement to
  report.
- **Actual provider provenance (read-only query against the paper DB):**
  `provider_id` distribution for existing rows is **`unknown` for 6,170 of
  ~8,302 rows (31 symbols)**, `alpaca` for 2,128 rows (the two/three symbols
  ingested since migration 0042's provider-metadata columns were wired up),
  plus small `csv`/`fake` buckets. Row-level inspection showed: (a) 6,129 of
  those 6,170 "unknown" rows are a single real symbol, **AAPL**, with
  plausible 2026-03 to 2026-07 intraday timestamps — this is the
  already-independently-tracked `MARKET-DATA-PROVIDER-PROVENANCE-01` gap
  (some ingestion/sync CLI paths write `provider_id='unknown'` even for
  genuine Alpaca-sourced bars; fix implemented in a separate, not-yet-merged
  worktree per prior-session memory) — an **attribution** bug, not evidence
  of a second, differently-adjusted data source; (b) the remaining ~40
  "unknown" rows are obviously synthetic test-fixture symbols (`GAP`, `DUP`,
  `EMPTY`, `SYNC_S2_WBARS`, `ZZZ1`, near-epoch timestamps like
  `1970-01-01T00:01:00Z`), not real research data.
- **Revision/backfill behavior:** `mqk-db/src/md.rs`'s upsert is
  `on conflict (symbol, timeframe, end_ts) do update set ...` — later
  ingestion runs **silently overwrite** prior OHLCV/provider values for the
  same primary key with no history retained (confirms audit A18: no
  bitemporal/as-of-ingestion tracking, only the `is_complete` flag).
- **Universe/PIT behavior:** confirmed by direct code reading (not
  assumed) — `universe/build.py::build_universe_swing_v1` and
  `cli.py::run_phase1_equity` derive the entire symbol universe from the
  operator-supplied `--symbols` argument; there is no code path anywhere that
  queries historical index/constituent membership. This is unambiguously
  **Mode B** (fixed, explicit ex-ante universe) as the mission's P8 section
  defines it — not an unresolved gap, a structural fact. Construction time
  (`asof_utc`) and the explicit symbol list are already recorded in
  `run_phase1_equity`'s manifest.
- **Corporate actions:** confirmed absent from `research-py` (matches A9-A13);
  given the single verified `raw_unadjusted` convention above, and that no
  evidence of split/dividend-adjusted data was found anywhere in this
  system's actual ingestion path, full corporate-action modeling in
  `research-py` is **not required for V1** — deferred consistently with
  Section 8's original judgment, now confirmed rather than assumed.
  **CORRECTED 2026-08-15 (Section 1B, PATCH B): this conclusion was WRONG —
  a confirmed raw_unadjusted convention is exactly the case where
  corporate-action contamination is dangerous, not exempt from it. See
  Section 1B for the fail-closed repair; the remaining gap is now a
  required corporate-action evidence SOURCE, not a design decision.**

**P8 implementation permission — granted.** All five gates in the mission's
"P8 IMPLEMENTATION PERMISSION" section were satisfied: one consistent data
convention (raw/unadjusted, single provider code path); no contradictory
per-symbol/time adjustment behavior (the `unknown` bucket is an attribution
gap in a *known already-tracked* bug, not a second convention); no production
DB migration required (provider_id columns already exist via already-merged
migration 0042); no ingestion-system rewrite (zero Rust files touched); no
unresolved dynamic-universe survivorship problem (Mode B, structurally
confirmed).

**P8 implementation — deliberately narrow.** Changed only
`research-py/src/mqk_research/data/adapters/bars_postgres.py`: added a pure
`classify_price_convention()` function and a `resolve_price_provenance()` /
`_query_distinct_provider_ids()` pair that queries the *actual* distinct
`provider_id` values for a query's exact (symbols, timeframe, window), and
asserts `price_adjustment_convention = "raw_unadjusted"` **only** when both
the resolved close column and every observed `provider_id` are within the
independently-verified set (`close_micros`, `{"alpaca"}`) — anything else,
including today's real `AAPL` query (which would return `provider_ids_observed
= ["unknown"]`), reports `"unverifiable"` rather than silently assuming
raw/unadjusted. `history()` now attaches this record to the returned
DataFrame's `.attrs["price_provenance"]` (metadata only — no column, no
behavior change for existing callers). A fail-closed
`require_verified_price_provenance()` gate is provided for any future
consumer that needs a hard guarantee. **`build_trial_identity`
(`ml/registry_integration.py`) was deliberately NOT modified in this patch** —
threading this provenance record into registered trial identity is real,
correctly-scoped follow-up work, not done here to avoid touching the
officially-registered trial-identity computation adjacent to the accepted
foundation within this Wave. 9 new unit tests (pure, no DB) plus 2 opt-in
DB-proof tests (skipped by default, matching the existing
`test_scanner_market_data_export_db_proof.py` convention) that encode the
exact real-world finding above (AAPL is currently unverifiable) as a live
regression check for whenever `MARKET-DATA-PROVIDER-PROVENANCE-01` lands. The
DB-proof tests could not be executed end-to-end in this session (this dev
box's Python environment lacks the `psycopg2` driver) — the underlying facts
were instead confirmed directly via `psql` against the same paper database
during the read-only fact-finding phase.

**ALPHA_DISCOVERY_READY vs. RESEARCH_BACKTEST_V1_COMPLETE — contradiction
resolved.** Section 16 originally required P10 (dossier composition) as part
of "REQUIRED_FOR_V1," while Section 17 said ALPHA_DISCOVERY_READY requires
Section 16's gate to be met but *also* "intentionally stops short of
requiring the promotion dossier (P10) to be final." A new, explicitly-named
earlier gate — **`RESEARCH_BACKTEST_FOUNDATION_READY`** — is introduced
below (Section 16A) to carry the meaning Section 17 actually intended:
everything in `RESEARCH_BACKTEST_V1_COMPLETE` except P10's dossier
composition. `ALPHA_DISCOVERY_READY` now depends on
`RESEARCH_BACKTEST_FOUNDATION_READY`, not on the full
`RESEARCH_BACKTEST_V1_COMPLETE` gate. See Sections 16/16A/17 below.

**Not implemented in Wave 1** (correctly out of scope per mission brief):
P7A/P7B/P7C, P9, P10. See the corrected roadmap (Section 12), dependency
graph (Section 13), and waves (Section 14) below.

---

## 1B. WAVE-1-INDEPENDENT-REPAIR-01 ADDENDUM (2026-08-15)

Mission `RESEARCH-BACKTEST-V1-WAVE-01-INDEPENDENT-REPAIR-01` executed two
repair patches against two deterministic defects found during independent
ChatGPT review of Wave 1 (Section 1A above). Both patches are local,
unpushed, one commit each, sequenced strictly after Wave 1's four commits.

**Patches executed, in order:**

| Patch | Verdict | Commit (local, unpushed) |
|---|---|---|
| PATCH A `RESEARCH-MULTIPLE-TESTING-JUDGE-01-REPAIR-01` | **CLOSED** | `8bc4dbc2` |
| PATCH B `BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-01` | **PARTIAL — CORPORATE_ACTION_SOURCE_REQUIRED**; 3 deterministic defects closed 2026-08-15 by **PATCH C** `BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-02` (`5bba8d6c`, **`P8_CONTRACT_COMPLETE` / `DATA_SOURCE_BLOCKED`**) — see Section 1C | `4f7e297e` |

### PATCH A — corrected DSR trial-count semantics

Root defect: `multiple_testing_stats.expected_max_sharpe` used the RAW
attempted/evaluable trial count (M) directly as N inside the extreme-value
quantile terms Z^-1[1-1/N] / Z^-1[1-1/(Ne)], as though every trial were
independent. Bailey & López de Prado (2014), Appendix A.3 ("ESTIMATING THE
NUMBER OF INDEPENDENT TRIALS"), is explicit that N must be the number of
IMPLIED INDEPENDENT trials, derived from the trials' average pairwise
correlation (eq. 7-9), not the raw count M whenever trials are dependent
(the normal case — parameter variants of the same strategy family are
almost always correlated).

**Primary-source verification.** Fetched the actual paper
(davidhbailey.com/dhbpapers/deflated-sharpe.pdf — David H. Bailey's own
hosted copy), not a secondary summary. Appendix A.3 derives:
- eq. 8: equal-weighted average pairwise correlation
  `rho_hat = (2 * sum_{i<j} rho_ij) / (M*(M-1))`.
- eq. 9: `N_hat = rho_hat + (1 - rho_hat) * M` — boundary-faithful
  (`rho_hat -> 1` collapses `N_hat -> 1`; `rho_hat -> 0` leaves
  `N_hat -> M`).
- An explicit ill-conditioning warning immediately after eq. 9: for
  `T < M(M-1)/2` (T = observations, M = trial count), "the correlation
  matrix will be numerically ill-conditioned... estimating an average
  correlation is then pointless."
- Equation 2 (the DSR formula) defines `SR_0 = sqrt(V[{SR_n}]) * (...)`
  under the null hypothesis of zero true Sharpe — it does NOT add the
  observed cross-trial mean `E[{SR_n}]` (that additive term only appears
  in the general eq. 1, not the null-rejection threshold SR_0).

**Fixes implemented** (`research-py/src/mqk_research/ml/
multiple_testing_stats.py`, `multiple_testing_judge.py`):
1. New `average_pairwise_correlation()` / `estimate_effective_independent_
   trials()` implementing eq. 7-9 exactly, using the SAME aligned daily OOS
   NET return series (excess of daily risk-free) already loaded for PBO —
   deterministic, no RNG, column/row-order invariant, and typed
   `not_evaluable` (never a silent fallback to raw M) both when the
   ill-conditioning threshold is breached and when the implied `N_hat <= 1`
   (duplicate/near-duplicate candidates).
2. `expected_max_sharpe()` now takes `effective_independent_trials` as an
   explicit, separate parameter from the raw Sharpe-estimate list whose
   LENGTH still determines the observed cross-trial variance basis — raw
   population size and the N used by the correction are now structurally
   distinct inputs, never conflated.
3. Fixed the zero-cross-trial-variance branch: previously returned the
   shared OBSERVED Sharpe as the benchmark; now returns `0.0` exactly, per
   eq. 2's literal `sqrt(0) = 0`. A regression test proves the old behavior
   was wrong, including a negative control under which the old (0.7-valued)
   assertion now fails.
4. `dsr_trial_accounting` — new top-level block in the judge artifact,
   computed once and shared by every `dsr_results` entry:
   `raw_unique_trial_count`, `effective_independent_trial_count`,
   `average_pairwise_correlation`, `trial_correlation_method`,
   `correlation_basis`, `ill_conditioning_threshold`,
   `numerically_defensible`, `not_evaluable_reason`,
   `dsr_trial_count_protocol_version`. The five P6-required registry-truth
   fields (`registered_unique_trials`, `attempted_unique_trials`,
   `economically_evaluable_trials`, `attempt_count`,
   `evaluation_slice_count`) are UNCHANGED and untouched by this repair —
   `dsr_trial_accounting` is a distinct, additional concept, not a
   replacement.
5. Comparison scope (`_comparison_key`) tightened to also require matching
   `cost_model` (commission/slippage/diagnostic_zero_cost) and
   `execution_capacity_policy` (`capacity_policy`, `fold_end_policy`) —
   candidates measured under different cost/execution assumptions no
   longer silently share one PBO/DSR population. `entry_threshold` and
   other strategy-level signal-policy fields remain deliberately excluded
   (candidate-differentiating, not measurement-basis).

`test_multiple_testing_judge.py` grew from 32 to 43 tests (11 new/repair
tests, including the required proofs: highly-correlated family effective-N
< raw M, low-correlation family effective-N > correlated family, duplicate
variants degenerate, column/row-order invariance of the new accounting
block, cost/capacity-policy comparison-scope separation, threshold-alone
non-separation). `test_experiment_registry.py` (47) and
`test_economic_walkforward.py` (63) regressions unaffected (neither module
touched by this patch).

### PATCH B — durable bars provenance + corporate-action fail-closed gate

**Root defect 1 (provenance not durable).** P8 Wave 1 attached price
provenance to `bars_postgres.history()`'s returned DataFrame via
`.attrs["price_provenance"]` — useful in-memory, but `.attrs` does not
survive a `to_csv()`/`read_csv()` round trip, which is exactly how bars
data reaches the registered economic evaluator (a `bars_csv` PATH, not a
DataFrame). The registered economic trial identity
(`build_economic_trial_identity`) never included provider identity,
adjustment convention, corporate-action policy, or query-window/universe
identity — only the raw artifact-file content hash.

**Root defect 2 (raw prices + corporate actions).** P8 Wave 1 concluded
corporate-action handling was "not required for V1" because the only
verified convention is `raw_unadjusted`. That conclusion inverted the
correct implication: RAW, UNADJUSTED price data is exactly the case where
corporate-action contamination is dangerous (a 2-for-1 split reads as a
~50% loss; dividends are omitted entirely from returns) — independently
documented in this repo's own
`core-rs/crates/mqk-backtest/src/corporate_actions.rs`. "Confirmed
raw_unadjusted" should have raised the corporate-action requirement, not
retired it.

**Fixes implemented** (new module `research-py/src/mqk_research/data/
bars_provenance.py`; `economic_walkforward.py`;
`economic_registry_integration.py`):
1. `build_bars_provenance_manifest()` — one durable, versioned,
   content-addressed manifest (`schema_version=bars_provenance_manifest_v1`)
   recording: provider IDs observed, resolved close column, price-
   adjustment convention, corporate-action policy + evidence id + declared
   forbidden periods, timeframe/window, symbol universe + universe mode
   (`fixed_ex_ante` — the only supported mode; `point_in_time` is named but
   explicitly UNSUPPORTED and fails closed), a **canonical semantic bars
   hash** (sorted symbol/end_ts/close content — invariant to physical row
   order), row count, and a SEPARATE physical `artifact_sha256`.
2. `provenance_identity_fragment()` — the identity-relevant subset (excludes
   `artifact_sha256`/`row_count`, which must NOT affect identity for a
   byte-reordered-but-semantically-identical file) — folded into
   `build_economic_trial_identity`'s `data_identity.bars_provenance`.
   `bars_provenance` is now a REQUIRED (no-default) argument on both
   `build_economic_trial_identity` and
   `run_registered_economic_walkforward_eval` — the official registered
   path cannot forget or skip the contract; omitting it is a `TypeError`.
3. `require_registered_bars_provenance()` — fail-closed structural gate
   (schema version, KNOWN price-adjustment convention, real (non-
   diagnostic) corporate-action policy, supported universe mode) — checked
   BEFORE any classification/economic evaluation runs.
4. `check_corporate_action_integrity()` — fail-closed CONTENT preflight,
   mirroring `mqk-backtest::CorporateActionPolicy`'s two-policy design
   (`Allow`/`ForbidPeriods`) rather than building an adjustment-tables
   subsystem: `adjusted_data` (valid only when the convention is one of an
   explicitly-named, currently EMPTY verified-adjusted set — no adjusted
   provider exists anywhere in this system today) or
   `forbid_affected_periods` (valid only with a real evidence id and zero
   actual bar-row overlap with the declared exclusion windows). Wired into
   `run_economic_walkforward()` via an optional `provenance_manifest`
   parameter, checked immediately after bars load and BEFORE any fold is
   simulated — an integrity preflight, not a chronology change. A synthetic
   2-for-1-split negative control (closes 100,100,50,50) proves the ~50%
   apparent loss can never reach accepted economic evidence: it fails
   closed both via a declared-but-unresolved exclusion and via the
   no-protection-at-all path.
5. Low-level `run_economic_walkforward()` keeps `provenance_manifest=None`
   as an explicit, narrow diagnostic escape hatch for the pre-existing 63
   synthetic-fixture tests in `test_economic_walkforward.py` (unmodified,
   still pass) — the OFFICIAL registered path never uses this escape
   hatch; `require_registered_bars_provenance` always runs first.

**No authoritative corporate-action exclusion source exists anywhere in
this repository** — confirmed by inspecting `core-rs/mqk-db/migrations/`
(no corporate-action table in any migration) and `research-py` (no CA
fixture/CSV anywhere, matching A9-A13's original finding). This means: for
the REAL, currently-registered `raw_unadjusted` data, neither
`adjusted_data` (no verified adjusted provider exists) nor
`forbid_affected_periods` (no real exclusion evidence exists) can be
honestly satisfied today — `require_registered_bars_provenance`/
`check_corporate_action_integrity` correctly and intentionally refuse all
real registered economic evaluation until that source exists. **This
downgrades A9-A13/A17's "not required for V1" verdict to a hard, confirmed
blocker**, and is why this patch is reported PARTIAL rather than COMPLETE
(both the durable contract AND the fail-closed protection are fully
implemented and tested — what's missing is the data source itself, which
per the mission's own instruction must not be fabricated).

Also confirmed, unchanged from Wave 1: `provider_id='unknown'` for ~6,170
of ~8,302 `md_bars` rows (mostly a single real symbol, AAPL) is a separate,
already-tracked attribution bug (`MARKET-DATA-PROVIDER-PROVENANCE-01`, not
yet merged) — `require_registered_bars_provenance` correctly fails closed
on those rows too (`price_adjustment_convention="unverifiable"`),
compounding rather than substituting for the corporate-action blocker.

22 new tests (`test_bars_provenance.py`) prove all 22 P8-required items:
`.attrs` non-durability, registered-path requirement, 8 independent
identity-change proofs (provider/convention/CA-policy/CA-evidence/bars-
content/timeframe-range/symbol-universe/universe-mode), unsupported-PIT
fail-closed, semantic-reorder-invariant-identity vs. artifact-hash-differs,
result-independence, unknown-provider fail-closed, raw-unadjusted-without-
protection fail-closed, the split negative control, ADJUSTED_DATA-valid-
only-when-verified (via a dependency-injected test-only convention, since
production has none), FORBID_AFFECTED_PERIODS content-level rejection,
chronology/holdout untouched (full registered-path integration run), and
identity determinism. Full `research-py` suite: **1215 passed, 7 skipped**
(1193 Patch-A baseline + 22 new; zero regressions).

### Audit corrections required by this addendum

- **Section 5, Area A9-A13/A17**: "not required for V1" is WRONG — see
  Root Defect 2 above. Corporate-action safety is now a hard, fail-closed
  registered-path requirement; the remaining gap is a DATA SOURCE, not a
  design decision.
- **Section 5, Area E**: DSR trial-count N is now the paper's implied-
  independent-trial estimate (Appendix A.3), not raw M. The
  zero-cross-trial-variance benchmark is `0.0`, not the shared observed
  Sharpe.
- **Section 12 / 16 / 16A**: P6-CLOSURE and P8's Wave-1 entries should be
  read as superseded by PATCH A (`8bc4dbc2`, CLOSED) and PATCH B
  (`4f7e297e`, PARTIAL — CORPORATE_ACTION_SOURCE_REQUIRED) respectively.
  `RESEARCH_BACKTEST_FOUNDATION_READY`'s "MULTIPLE TESTING" line item
  remains MET (repaired, not reopened); its "DATA TRUTH" line item, and
  `RESEARCH_BACKTEST_V1_COMPLETE`'s corresponding line item, are now
  **NOT MET** — both were previously marked MET on the (incorrect) A9-A13/
  A17 conclusion this addendum corrects. Official registered economic
  evaluation on real data is fail-closed-blocked pending a corporate-action
  evidence source.
- **P7A** (`RESEARCH-EXECUTION-PRICING-PARITY-01`) and **P9**
  (`BKT-ROBUSTNESS-GAUNTLET-01`) both depended on "the confirmed
  raw_unadjusted convention" per Section 13's original dependency graph —
  that convention is still confirmed, but P9 in particular should now
  account for corporate-action exclusion as part of any real-candidate
  robustness run, not just cost/execution-delay stress.

**Next-wave entry condition update.** WAVE 2 (P7A/P7B/P7C) may still
proceed on its own merits — none of the three touch bars data, and neither
depends on the corporate-action blocker. Any future wave that intends to
run the registered economic evaluator against REAL (non-synthetic) data
remains blocked until a corporate-action evidence source is sourced and
wired as `forbid_affected_periods` evidence. This is a new, explicit
prerequisite this addendum adds to the roadmap — tracked here as
**`BKT-CORPORATE-ACTION-EVIDENCE-SOURCE-01`** (not yet scoped, not yet a
patch — per the mission's own instruction, this addendum reports the
blocker honestly rather than inventing a sourcing plan).

---

## 1C. WAVE-1-INDEPENDENT-REPAIR-02 ADDENDUM (2026-08-15)

Mission `BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-02` closed three
deterministic defects found by independent review of PATCH B (Section 1B),
against the SAME commit chain, HEAD `5bba8d6c58a4d2a509fbd86882cd8f9013a24b56`.
`RESEARCH-MULTIPLE-TESTING-JUDGE-01`'s methodology (PATCH A, commit
`8bc4dbc2`) was NOT reopened; only its comparability key was updated to
consume this repair's corrected data-identity authority (see item 3 below).

**Verdict: `P8_CONTRACT_COMPLETE` / `DATA_SOURCE_BLOCKED`.** The P8 safety/
identity CONTRACT (manifest-to-bars binding, corporate-action evidence
verification, canonical trial identity) is now complete and proven by
regression test. Real `raw_unadjusted` registered research remains
HONESTLY fail-closed-blocked, exactly as Section 1B already reported —
`BKT-CORPORATE-ACTION-EVIDENCE-SOURCE-01` (an external evidence SOURCE, not
this CONTRACT) is unchanged as the remaining prerequisite.

1. **Manifest was not bound to the actually-loaded bars.** PATCH B's
   `require_registered_bars_provenance` verified a manifest's STRUCTURAL
   shape but never checked that it actually described the bars being
   evaluated — a manifest built for bars A could be paired with bars B on
   disk. Fixed with a new content-binding preflight,
   `require_bars_match_manifest()` (`bars_provenance.py`), that recomputes
   `canonical_semantic_bars_hash` from the bars actually loaded and requires
   exact equality with the manifest's declared hash, then cross-checks the
   observed symbol universe, timestamp range, and (best-effort, from the
   bars content alone) daily-granularity consistency against the manifest's
   declared contract. Raises the new `BarsProvenanceContentMismatch`; runs
   in `run_economic_walkforward()` immediately after bars load, BEFORE
   `check_corporate_action_integrity` and before any fold is simulated.
   `canonical_semantic_bars_hash` itself now also fails closed on duplicate
   `(symbol, end_ts)` rows (previously sorted/hashed them ambiguously),
   agreeing with `load_bars`'s existing duplicate contract.
2. **Corporate-action evidence was forgeable.** PATCH B's
   `forbid_affected_periods` policy trusted any non-empty
   `corporate_action_evidence_id` STRING and whatever `forbidden_periods`
   sat next to it in the same manifest — a caller could assert anything
   (the pre-repair tests' own `"evidence-v1"` fixture proved this). Fixed
   with a real, content-addressed corporate-action evidence contract:
   `build_corporate_action_evidence()` assembles an evidence object (schema
   version, source/provider identity, covered symbol universe, coverage
   start/end, corporate-action entries with symbol/action-type/effective
   window, artifact hash) whose `evidence_id` is DERIVED from its canonical
   content via `corporate_action_evidence_id()` — never caller-chosen.
   `check_corporate_action_integrity` now independently recomputes that ID,
   requires the evidence's coverage to include the complete observed bars
   universe/range, and requires the manifest's `forbidden_periods` to be
   exactly what `forbidden_periods_from_evidence()` derives from the
   verified evidence — this is the narrow interface a future
   `BKT-CORPORATE-ACTION-EVIDENCE-SOURCE-01` data patch must satisfy. No
   authoritative source calls it with real data anywhere in this repository
   today (unchanged from Section 1B) — official registered `raw_unadjusted`
   evaluation stays fail-closed until one exists; only direct/diagnostic
   test fixtures use `build_corporate_action_evidence` with synthetic
   content, and the dedicated tests proving a bare evidence-ID string still
   fails the official registered path were kept (and added to).
3. **Row-order-invariant trial-identity test was a false positive, and
   physical bars bytes still leaked into candidate identity.** PATCH B's
   `_write_min_registered_inputs` test helper never actually wrote
   `bars.csv` to disk, so its row-reorder identity test compared two
   identical `{"sha256": None, "bytes": None}` records regardless of
   physical order — it could not have caught a regression. Fixed the test
   helper first (writes real bars content in both physical orders) and
   confirmed it goes RED against the pre-repair `build_economic_trial_identity`
   (stash+rerun negative control: reordering flips `trial_id`), THEN removed
   `data_identity.economic_bars_csv`'s physical sha256/bytes from
   `build_economic_trial_identity` entirely — economic bars candidate
   identity is now carried SOLELY by `bars_provenance`'s
   `canonical_semantic_bars_hash` (already present via
   `provenance_identity_fragment`). The physical bars-file record is not
   lost — it remains fully auditable per ATTEMPT (not per trial) in
   `economic_walkforward.run_economic_walkforward`'s own output artifact
   (`out["inputs"]["bars_csv"]`), which was already unconditionally written
   and untouched by this fix.
   `RESEARCH-MULTIPLE-TESTING-JUDGE-01`'s comparison key
   (`multiple_testing_judge._comparison_key`) was updated in the same
   commit to consume the full `bars_provenance` identity fragment instead
   of the now-removed physical `economic_bars_csv.sha256` — two trials
   whose provenance basis differs (provider, adjustment convention, CA
   policy/evidence, universe, or extraction range) no longer share a DSR/PBO
   comparison scope. The judge's own `judge_id_basis` was also extended to
   include `DSR_TRIAL_COUNT_PROTOCOL_VERSION` (a declared methodology
   version, never a numeric result) — a future change to the effective-
   independent-trial estimation method now changes `judge_id` even when the
   registry and every DSR/PBO numeric output happen to come out
   byte-identical.

**Tests.** 24 new/repaired tests in `test_bars_provenance.py` (now 58 total)
covering all `REQUIRED TESTS` items 1–14 and 17–18 from the repair mission
(manifest-to-bars binding negative/positive controls, canonical-hash/
symbol-universe/range mismatch fail-closed, duplicate `(symbol,end_ts)`
fail-closed, arbitrary-evidence-string and empty-forbidden-periods rejection
at both the manifest level and the full OFFICIAL registered pipeline,
evidence-ID derivation and content-sensitivity, coverage-too-small and
missing-symbol-coverage rejection, caller-modified-forbidden-periods
rejection, the synthetic diagnostic escape hatch remaining usable, and
physical-bytes exclusion from trial identity); 3 existing tests repaired in
place to use real synthetic evidence/chronology-safe fixtures instead of the
pre-repair `"evidence-v1"` anti-pattern (the split negative control, the
forbid-affected-periods overlap test, and the full registered-pipeline
chronology/holdout test, the last of which now proves its mechanics via
`ADJUSTED_DATA` + the same dependency-injected test-only convention Section
1B's PATCH B already used, since `FORBID_AFFECTED_PERIODS` can no longer be
satisfied without a real evidence source). 4 test fixture helpers in
`test_economic_walkforward.py`, `test_evidence_boundary.py`, and
`test_multiple_testing_judge.py` that build manifests for the OFFICIAL
registered path were updated to the same real-evidence contract (they were
not the subject of the repair but would otherwise have broken). 3 new tests
in `test_multiple_testing_judge.py` (34 total) prove comparison-scope
separation by provenance basis and `judge_id` sensitivity to the DSR
protocol version. Full `research-py` suite: **1231 passed, 7 skipped, 12
subtests passed** (zero regressions; skip count unchanged from Section 1B).

Repair commit: `5bba8d6c58a4d2a509fbd86882cd8f9013a24b56`
("research: bind economic evaluation to canonical bar provenance").

---

## 1D. BKT-RESEARCH-MARKET-DATA-AUTHORITY-01 ADDENDUM (2026-08-15)

Mission `BKT-RESEARCH-MARKET-DATA-AUTHORITY-01` closes the remaining
`DATA_SOURCE_BLOCKED` gap Section 1C left open: **an actual, trusted,
repeatable historical-data SOURCE** satisfying the P8/PATCH-B/PATCH-C
CONTRACT, not just the contract's verification machinery. Executed against
the SAME commit chain, HEAD `5bba8d6c58a4d2a509fbd86882cd8f9013a24b56`.

**Verdict: `COMPLETE`.** Official US-equity/ETF research now has a trusted,
repeatable path to obtain corporate-action-safe historical economic bars
for any requested symbol/range whose corporate-action history is limited
to forward/reverse splits, cash dividends, and spin-offs (the set Alpaca's
own documentation confirms `adjustment=all` covers) — verified against
Alpaca's live production API using this box's existing paper credentials,
not just mocked fixtures. Any symbol/range containing a corporate action
outside that documented set (mergers, redemptions, name changes, rights
distributions, unit splits, stock dividends, worthless removals, partial
calls, reorganizations) remains **honestly fail-closed-blocked** — this is
intentional, per the mission's explicit "do not guess" instruction, not a
residual gap.

**Official documentation verified (Firecrawl, 2026-08-15, against
`docs.alpaca.markets`):**
- `GET https://data.alpaca.markets/v2/stocks/bars` — `adjustment` accepts
  `raw` (default), `split`, `dividend`, `spin-off`, `all` ("apply all above
  adjustments"), confirming the mission's expected contract exactly.
- `GET https://data.alpaca.markets/v1/corporate-actions` — full OpenAPI
  schema fetched and used verbatim (not guessed): 15 action types
  (`reverse_split`, `forward_split`, `unit_split`, `cash_dividend`,
  `stock_dividend`, `spin_off`, `cash_merger`, `stock_merger`,
  `stock_and_cash_merger`, `redemption`, `name_change`, `worthless_removal`,
  `rights_distribution`, `partial_call`, `reorganization`), each with its
  own required-field shape (symbol field(s), `process_date`, and — for six
  of the fifteen types — `ex_date`).

**Chosen adjustment semantics.** `adjustment=all` is the sole historical
bars authority for the new research path (never `raw`, never a partial
combination) — matches the mission's stated preference and keeps exactly
one convention to reason about. Corporate-action coverage classification
is deliberately conservative: only `forward_split`, `reverse_split`,
`cash_dividend`, `spin_off` are `COVERED_BY_ADJUSTMENT`; every other type
— explicitly including `unit_split` and `stock_dividend`, which read as
"the same as split/dividend" but are never named in Alpaca's adjustment
documentation — is `REQUIRES_FAIL_CLOSED_REVIEW`. No type's classification
was guessed from its name.

**New research-only data authority** (`research-py/src/mqk_research/data/
alpaca_historical.py`): fetches directly from Alpaca's Market Data API
(never through `md_bars`) so `adjustment=all` can be used without touching
`core-rs/crates/mqk-md/src/alpaca_provider.rs` (still `adjustment=raw`,
unchanged) or any Paper/live runtime path. `extract_research_bars_with_
provenance()` is the single official entry point: fetches complete,
deterministically-normalized bars AND corporate-action evidence for the
identical symbol/range, fails closed (`CorporateActionReviewRequired`) if
any `REQUIRES_FAIL_CLOSED_REVIEW` event intersects the request — checked
against the corporate-action entries directly, so a bar simply being
absent on the event date cannot bypass the check — and otherwise returns a
complete `bars_provenance` manifest ready to pass straight into
`run_registered_economic_walkforward_eval(bars_csv=..., bars_provenance=
result["manifest"])`; no manual manifest fabrication required. A thin
`mqk-research extract-alpaca-bars` CLI subcommand writes the four
deterministic artifacts (`research_bars.csv`, `research_bars_provenance.
json`, `corporate_actions.json`, `corporate_actions_provenance.json`).

**Trusted-source-attestation contract (the actual defect closure).** The
mission's explicit target defect — `build_corporate_action_evidence(
source_provider_id="made-up", ..., corporate_action_entries=[])` (or
equivalently, a hand-typed `price_adjustment_convention=
"alpaca_all_adjusted_v1"`) must NOT authorize an official run merely for
being internally hash-consistent — required a NEW verification layer
`mqk_research.data.bars_provenance` didn't have before this mission: a
convention name is not self-authorizing. `alpaca_all_adjusted_v1` was
added to `_KNOWN_ADJUSTED_CONVENTIONS` (an explicit, auditable seam
addition, not a broadening of silent trust) but simultaneously added to a
new `_CONVENTIONS_REQUIRING_SOURCE_ATTESTATION` set: for those
conventions, `check_corporate_action_integrity`'s `adjusted_data` branch
now also calls `_require_verified_source_attestation()`, which demands a
real, content-addressed `source_attestation` object (`build_source_
attestation`) whose declared id is independently recomputed (never
trusted), whose `extractor_id` is in a narrow trusted allowlist (only
`mqk_research.data.alpaca_historical.v1`), whose `adjustment_mode` matches
what the convention requires, whose pagination is complete on both bars
and corporate-actions retrieval, whose `category_b_events_found` is
empty, and whose bars-hash / corporate-action-evidence-hash / symbol /
date coverage all independently bind to what's actually being evaluated.
`source_attestation_id` was folded into `provenance_identity_fragment`
(and therefore registered trial identity and the multiple-testing judge's
comparison-scope key), exactly like `corporate_action_evidence_id` already
was. `require_registered_bars_provenance` also gained an earlier,
structural copy of the same "convention alone is not enough" check, so a
hand-built manifest fails at the STRUCTURAL gate before even reaching
`check_corporate_action_integrity`.

**Synthetic-vs-official evidence boundary.** Unchanged, and now doubly
enforced for the adjusted-data path: `_KNOWN_ADJUSTED_CONVENTIONS` still
serves test-only monkeypatched conventions (e.g.
`split_dividend_adjusted_test_only`, used by the pre-existing `test_bars_
provenance.py` fixtures) with no attestation requirement at all —
`_CONVENTIONS_REQUIRING_SOURCE_ATTESTATION` is scoped to
`alpaca_all_adjusted_v1` only, so none of PATCH B/C's 58 existing tests
needed to change.

**Fixed-universe behavior.** Unchanged (`universe_mode=fixed_ex_ante`
only); the extractor records the exact symbol set, its content-derived
`symbol_universe_id`, and `universe_mode` in every manifest it produces,
same as the pre-existing contract required.

**Unsupported CA behavior.** A `REQUIRES_FAIL_CLOSED_REVIEW` event
intersecting the requested symbol/range raises
`CorporateActionReviewRequired` for the WHOLE request — bars are never
silently dropped for the affected symbol/period, per the mission's
explicit instruction. The operator must narrow the symbol universe or
date range (or wait for a future patch adding explicit handling for that
event type).

**Raw-data fallback behavior.** Unchanged from PATCH B/C:
`raw_unadjusted` + `forbid_affected_periods` remains defined, tested, and
still requires a real, content-addressed `corporate_action_evidence`
object — no live source in this repository calls it with real data today,
same as before this mission. This mission's new path is `adjusted_data`
+ `alpaca_all_adjusted_v1`, not a change to the raw path's status.

**Live verification (optional, opt-in, read-only — no trading operations,
no credentials logged).** Using this box's existing `ALPACA_API_KEY_PAPER`
/ `ALPACA_API_SECRET_PAPER` (never printed), three ad hoc live GET-only
checks were run against production Alpaca (not committed as part of the
automated test suite, matching the mission's "normal tests must not
require network access" instruction): (1) `fetch_historical_bars`/
`fetch_corporate_actions` against real AAPL data succeeded, correctly
reporting real 2020 dividend/split events; (2) the full `extract_research_
bars_with_provenance` → `require_registered_bars_provenance` →
`require_bars_match_manifest` → `check_corporate_action_integrity` chain
passed end to end against real AAPL bars; (3) a window spanning AAPL's
real 2020-08-31 4-for-1 forward split was extracted successfully with
`adjustment=all` — the returned closes are continuous (~$121→$130) across
the split date with no artificial ~4x jump, empirically confirming both
Alpaca's adjustment behavior and this module's `COVERED_BY_ADJUSTMENT`
classification of `forward_split` against real data, not just a mocked
fixture.

**Files changed:** `research-py/src/mqk_research/data/bars_provenance.py`
(source-attestation contract, additive), `research-py/src/mqk_research/
data/alpaca_historical.py` (new), `research-py/src/mqk_research/cli.py`
(new `extract-alpaca-bars` subcommand), `research-py/tests/test_alpaca_
historical.py` (new, 41 tests), `research-py/tests/test_source_
attestation.py` (new, 23 tests). `eval_walkforward.py` and `economic_
walkforward.py` were NOT touched — zero risk to the accepted foundation
(Section 4) or the frozen future-execution/holdout chronology.

**Tests.** 64 new tests (41 + 23), all using injected fake HTTP transports
— zero real network access in the automated suite. Negative-control
proof: `check_corporate_action_integrity`'s new `_require_verified_
source_attestation` call was temporarily removed and the 12
source-attestation-dependent tests in `test_source_attestation.py` were
confirmed to fail (not vacuously pass) before the guard was restored.
Full `research-py` suite: **1295 passed, 7 skipped, 12 subtests passed**
(1231 PATCH-C baseline + 64 new; zero regressions; skip count unchanged).

**Audit corrections required by this addendum:**
- Section 1B/1C's `DATA_SOURCE_BLOCKED` status is now resolved for the
  `adjustment=all` / category-A-only case. `BKT-CORPORATE-ACTION-
  EVIDENCE-SOURCE-01`, as originally scoped in Section 1B (a `forbid_
  affected_periods` exclusion-evidence source for `raw_unadjusted` data),
  is superseded by this mission's `adjusted_data` path per the mission
  brief's own stated preference ("prefer adjustment=all") — the raw path
  itself remains exactly as fail-closed as PATCH C left it.
- Section 5 Area A9-A13/A17: upgrade from "PARTIAL-B-REPAIR"/"P8-CLOSED"
  to **RESOLVED for the adjusted-data path** — a trusted, live-verified
  extraction authority now exists; still correctly fail-closed for
  category-B corporate actions and for any symbol/range this extractor
  has not been asked to cover.
- Section 16A (`RESEARCH_BACKTEST_FOUNDATION_READY`): the "DATA TRUTH"
  line item's blocking condition ("P8 is not sufficient for DATA TRUTH on
  its own until `BKT-CORPORATE-ACTION-EVIDENCE-SOURCE-01` lands") is now
  **MET** for the adjusted-data path this mission delivers. P7A, P7B,
  P7C, P9 remain the sole remaining OPEN items for that gate — unchanged,
  not addressed by this mission (P7/P9/P10 were explicitly out of scope
  per the mission brief).

**Remaining data blockers:** none for the `adjustment=all` /
category-A-only case this mission targeted. Category-B corporate actions
(mergers, redemptions, name changes, rights distributions, unit splits,
stock dividends, worthless removals, partial calls, reorganizations)
remain unhandled by design — a future patch could add explicit,
provider-verified handling for specific category-B types (e.g. a verified
merger-chain-splicing policy) if a real research need arises, but
inventing that now would be exactly the kind of guessing this mission was
told not to do.

**Recommended next wave:** WAVE 2 (P7A/P7B/P7C — parity contract) remains
the correctly-sequenced next step per Section 20, now with one fewer
caveat: any WAVE 2/P9 work that wants to run the registered economic
evaluator against real historical data can now do so (for symbols/ranges
without category-B events) via `mqk_research.data.alpaca_historical`
instead of being blocked outright.

---

## 1E. BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-01 ADDENDUM (2026-08-15)

Independent review of Section 1D found four deterministic defects. Mission
`BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-01` closed all four against
the same commit chain. Baseline for this repair: `origin/main` HEAD
`ec5a3fcd8ac3232fa5f057ae56a939a78971f083` (`docs: streamline Claude
repository operating contract`) — the mqk-mcp read-only-tooling merge
(`96ab22a4`) sitting between this repair's baseline and Section 1D's
original `5bba8d6c` is unrelated, reviewed history confined to
`tools/mqk-mcp/`.

**Verdict: `COMPLETE`.**

**Corrected CA discovery semantics (Defect 1).** Alpaca's
`/v1/corporate-actions` `start`/`end` filter by `process_date`, and Alpaca's
own Developer Relations team confirms (forum.alpaca.markets/t/
querying-corporate-actions-by-ex-date-rather-than-process-date/17724,
verified 2026-08-15) that `process_date` "can be several days (or more)
after the `ex_date`" — no documented bound exists. A query window bounded
by the research range (Section 1D's original behavior) could therefore
silently miss an event whose `ex_date`/effective window falls inside the
research range but whose `process_date` lands after it. Per the mission's
decision hierarchy (no documented bound ⇒ query full provider-supported
history, not a guessed buffer), `extract_research_bars_with_provenance` now
queries corporate actions for the requested symbols across
`[CA_DISCOVERY_PROCESS_DATE_FLOOR_UTC ("1900-01-01"), max(asof, research
end)]` — a proven process_date superset — and filters the result locally by
EFFECTIVE (`ex_date`-based) window intersection with the research range
(`_filter_entries_intersecting_range`) before it ever reaches the
review-required gate or corporate-action evidence. The actual discovery
query range and protocol (`CA_DISCOVERY_PROTOCOL_V1`) are serialized into
`source_attestation.corporate_action_query_coverage`, so a manifest records
*how* completeness was established, not just an unverifiable claim of it.

**Explicit ASOF contract (Defect 2).** `fetch_historical_bars` now requires
an explicit, caller-resolved `asof` (`YYYY-MM-DD`) and always sends it to
Alpaca's `/v2/stocks/bars` — never the provider's implicit current-day
default. The resolved value is recorded verbatim in `source_attestation.
asof`, which already participated in `canonical_source_attestation_content`
before this repair, so two otherwise-identical extractions with different
ASOF values already produced different `source_attestation_id`/trial
identity, and the SAME ASOF with the same semantic data remains
deterministic — no separate identity-plumbing change was needed, only
making the value real. The CLI's `extract-alpaca-bars` subcommand gained a
required `--asof YYYY-MM-DD` flag, resolved and printed before extraction
runs (repo convention: required, not implicitly defaulted, matching how
`--asof-utc` already works for the `features`/`universe`/`targets`/`run`
subcommands). `bars_provenance._require_verified_source_attestation` now
also independently rejects any attestation missing an explicit `asof`.

**Official-vs-diagnostic source authority (Defect 3).** The single
`extract_research_bars_with_provenance(..., http_get=..., base_url=...)`
entry point — which let an injected test transport exercise the same code
path that mints an OFFICIAL, trusted attestation — is now two functions
sharing one private orchestration body
(`_extract_research_bars_with_provenance_impl`):
`extract_research_bars_with_provenance` (OFFICIAL: fixed
`ALPACA_DATA_BASE_URL`, real HTTP transport, **no** `http_get`/`base_url`
parameters at all — verified by a structural test asserting they are absent
from the signature) and `extract_research_bars_with_provenance_diagnostic`
(INTERNAL: `http_get` is a required parameter with no default, forcing an
explicit injected fake; every unit test in `test_alpaca_historical.py` now
goes through this path). The two entry points hard-code, not
caller-parameterize, a new `source_authority` field on every attestation
(`SOURCE_AUTHORITY_OFFICIAL_PROVIDER` vs `SOURCE_AUTHORITY_DIAGNOSTIC_
SYNTHETIC`); `_require_verified_source_attestation` independently rejects
any attestation whose `source_authority` is not
`SOURCE_AUTHORITY_OFFICIAL_PROVIDER`, in addition to (not instead of) the
pre-existing `extractor_id` allowlist check. The same gate now also
independently verifies `source_provider_id == "alpaca"` and that
`api_endpoint_bars`/`api_endpoint_corporate_actions` equal the exact
official Alpaca endpoints for the `alpaca_all_adjusted_v1` convention — a
fake provider id or a fake/attacker endpoint string on an otherwise
internally-consistent attestation now fails the same way a fake
`extractor_id` already did.

**Semantic-vs-transport identity distinction (Defect 4).**
`raw_response_content_hashes` (per-page provider-response hashes) is a
transport/pagination-boundary fact, not a semantic research fact, and has
been removed from `canonical_source_attestation_content` — the same
treatment `retrieval_timestamp_utc` and `attestation_id` already received.
Two extractions with byte-identical semantic bars and corporate-action
evidence that merely paginated differently now share one semantic
`source_attestation_id`/trial identity; the raw per-page hashes remain on
the attestation object itself as durable, always-visible audit evidence,
just outside canonical identity. `source_authority` was added to the
identity-bearing set alongside the pre-existing fields.

**Provider end-inclusive / internal-exclusive normalization.** Alpaca's
`/v2/stocks/bars` `end` parameter is documented INCLUSIVE (verified
2026-08-15); this repo's internal research contract is the half-open
`[start_utc, end_utc)`. `fetch_historical_bars` now filters every returned
row to `start_utc <= end_ts < end_utc` locally before any duplicate/
non-finite check, sort, or hashing — a bar landing exactly at (or past)
`end_utc`, or (defensively) before `start_utc`, can never enter the
canonical dataset, semantic hash, or economic evaluation.

**Final market-data authority status.** Unchanged from Section 1D's
`adjustment=all` / category-A-only scope, now with a *proven* (not merely
asserted) corporate-action discovery guarantee, an explicit non-implicit
ASOF, a hardened official/diagnostic authority boundary, and transport
artifacts correctly excluded from semantic identity. Category-B corporate
actions remain honestly fail-closed-blocked by design, unchanged.

**Files changed:** `research-py/src/mqk_research/data/bars_provenance.py`
(`source_authority` field + trusted-profile checks + explicit-asof check +
raw-hash exclusion from canonical identity), `research-py/src/mqk_research/
data/alpaca_historical.py` (CA discovery floor/protocol, required `asof`,
official/diagnostic split, provider end-boundary normalization),
`research-py/src/mqk_research/cli.py` (required `--asof`),
`research-py/tests/test_alpaca_historical.py` and `research-py/tests/
test_source_attestation.py` (updated to the diagnostic entry point +
required `asof`; new coverage for all four defects and the end-boundary
normalization).

**Tests.** Full `research-py` suite: **1319 passed, 7 skipped, 12 subtests
passed** (zero regressions; skip count unchanged from Section 1D).

**Remaining blocker:** none for this repair's scope. Category-B
corporate-action handling remains an explicit future patch, unchanged from
Section 1D. P7A/P7B/P7C/P9/P10 remain out of scope, per this repair
mission's explicit instruction not to implement them.

---

## 1F. BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-REPAIR-02 ADDENDUM (2026-08-15)

Independent review of Section 1E found one further deterministic defect in
REPAIR-01's Defect 1 fix. Mission `BKT-RESEARCH-MARKET-DATA-AUTHORITY-01-
REPAIR-02` closed it against the same commit chain. Baseline for this
repair: local `main`, two commits ahead of `origin/main` HEAD
`ec5a3fcd8ac3232fa5f057ae56a939a78971f083` (Section 1E's own two commits).

**Verdict: `COMPLETE`.**

**Root defect.** REPAIR-01's CA discovery process-date ceiling was
`max(asof, research_end)` — coupling the corporate-action discovery snapshot
cutoff to the bars `asof` parameter. Alpaca's official documentation defines
`asof` as the entity/symbol-name-change resolution date sent to
`/v2/stocks/bars` — it says nothing about, and does not bound,
`/v1/corporate-actions` `process_date` coverage. Because Alpaca documents no
bound on how far `process_date` can lag `ex_date` (Section 1E), a real event
with `process_date` after BOTH `research_end` and `asof` — e.g. `research_end
= asof = 2020-06-30`, event `ex_date = 2020-06-28` (inside the research
interval), `process_date = 2020-07-02` — would have been silently missed:
`max(asof, research_end) = 2020-06-30 < 2020-07-02`, so REPAIR-01's CA query
would never have reached that `process_date` at all.

**Fix — three separately-named concepts.** `bars_asof` (entity/symbol-mapping
identity, sent verbatim to `/v2/stocks/bars`, unchanged) and the CA discovery
snapshot cutoff are now independent: `_resolve_ca_discovery_cutoff_utc`
resolves the CA discovery cutoff ONCE, before either provider call, from an
explicit `retrieval_timestamp_utc` (or real wall-clock UTC "now" for a live
extraction) — never from `asof` or `research_end`.
`_ca_discovery_process_date_bounds` now takes this resolved cutoff directly;
the CA discovery query window is `[CA_DISCOVERY_PROCESS_DATE_FLOOR_UTC
("1900-01-01"), ca_discovery_cutoff_utc]`. The discovery protocol constant
was bumped `CA_DISCOVERY_PROTOCOL_V1` → `CA_DISCOVERY_PROTOCOL_V2`
(`process_date_full_history_through_retrieval_snapshot_v2`), and
`source_attestation.corporate_action_query_coverage`'s fields were renamed
`process_date_query_start_utc`/`process_date_query_end_utc` →
`ca_discovery_start_utc`/`ca_discovery_end_utc` to match, so the recorded
contract cannot be misread as bars-asof-derived.

**Revision-sensitive provenance (by design, unchanged mechanism).**
Corporate-action evidence is a provider snapshot, not timeless truth: the
same bars request/asof/research window re-extracted at a LATER CA discovery
cutoff can legitimately surface a provider-backfilled corporate action.
`corporate_action_evidence`/`corporate_action_evidence_id` already changed
identity when entries content changed (Section 1D's Defect 2 contract,
unmodified by this repair) — this repair does not add new identity-plumbing
for that; it only fixes what the discovery query *finds*. Re-extracting with
the same pinned `retrieval_timestamp_utc` and identical underlying provider
content remains deterministic (same `source_attestation_id`); a later
snapshot that finds an additional event changes
`corporate_action_evidence_id` → `source_attestation_id` →
`provenance_identity_fragment`, propagating into trial identity as intended.

**Files changed:** `research-py/src/mqk_research/data/alpaca_historical.py`
(`_resolve_ca_discovery_cutoff_utc`, `_ca_discovery_process_date_bounds`
signature change, `CA_DISCOVERY_PROTOCOL_V2`, renamed
`corporate_action_query_coverage` fields; `bars_provenance.py` and `cli.py`
required no changes — the coverage dict was already a free-form field and
`retrieval_timestamp_utc` was already an existing parameter on both entry
points), `research-py/tests/test_alpaca_historical.py` (RED/GREEN proof for
the mission's exact scenario; cutoff-independent-of-asof; cutoff-change
represented in the source contract; same-snapshot determinism; later-snapshot
revision sensitivity propagating to `corporate_action_evidence_id`/
`source_attestation_id`/`provenance_identity_fragment`).

**Tests.** `test_alpaca_historical.py`: 62 passed (was 55; +1 test replaced,
+8 new). `test_source_attestation.py` + `test_bars_provenance.py` +
`test_economic_walkforward.py` + `test_multiple_testing_judge.py`: 164
passed. Full `research-py` suite: **1324 passed, 7 skipped, 12 subtests
passed** (zero regressions; skip count unchanged from Section 1E).

**Remaining blocker:** none for this repair's scope. Category-B
corporate-action handling remains an explicit future patch, unchanged from
Sections 1D/1E. P7/P9/P10 remain out of scope, per this repair mission's
explicit instruction not to implement them.

---

## 2. Baseline

```
git branch --show-current  -> main
git rev-parse HEAD          -> 9f398641b8207383bf8c75c7301c592f4b20c887
git rev-parse origin/main   -> 9f398641b8207383bf8c75c7301c592f4b20c887
```

HEAD matches the mission's expected commit (`research: add causal economic walk-forward evaluation`) exactly.

`git status --short` showed the expected `?? smoke_logs/` **plus** three additional untracked files not anticipated by the mission brief:

```
?? research-py/src/mqk_research/ml/multiple_testing_judge.py
?? research-py/src/mqk_research/ml/multiple_testing_stats.py
?? research-py/tests/test_multiple_testing_judge.py
```

This is not a "STOP" condition (HEAD matches exactly), but it is a material fact: it is a complete, working implementation of PATCH 6 left uncommitted from a prior session. It was inspected read-only and its test file was run (narrow, self-contained, `tmp_path`-only SQLite fixtures, no DB/broker/network) to verify the capability actually works, per the mission's allowance for narrow verification tests. No other tests were run; no files were modified, staged, or committed; `smoke_logs/` was not touched.

---

## 3. Current Architecture Map

```
research-py/src/mqk_research/
  data/                 bars_postgres.py (Postgres bar fetch), consolidate.py, adapters/{futures,options}_stub.py
  instruments/           schema.py — EQUITY/OPTIONS/FUTURES tagged-union instrument model
  universe/               build.py — asof top-N universe filter
  features/               compute.py, feature_set_v1.py — backward-only rolling features
  indicators/             core.py — EMA/ATR/etc., center=False throughout
  shadow/                 label_shadow_intents.py — fwd_ret / label_end_ts (inclusive) computation
  ml/
    eval_walkforward.py       purged walk-forward + reserved holdout (RESEARCH-PURGED-HOLDOUT-01)
    economics.py               pure return/Sharpe/drawdown/cost-drag math
    economic_walkforward.py    causal, cost-aware OOS economic simulation (RESEARCH-ECONOMIC-WALKFORWARD-01 +REPAIRs)
    economic_registry_integration.py   registered entry point, chains classification -> economic eval
    multiple_testing_stats.py  DSR / PBO-CSCV pure math (committed 2026-08-15, `fbb63fc7` — see Section 1A)
    multiple_testing_judge.py  registry-integrated judge artifact (committed 2026-08-15, `fbb63fc7` — see Section 1A)
    holdout_ledger.py          durable holdout reservation/consumption ledger (added 2026-08-15, `686614cc` — see Section 1A)
    evidence_boundary.py       fail-closed promotion-grade-OOS-vs-diagnostic classifier (added 2026-08-15, `c643588d` — see Section 1A)
    train.py / model_logreg.py deterministic logistic regression (single-shot, non-fold-isolated)
  exp_distributed/        SQLite trial/attempt/slice registry + distributed batch runner (ProcessPoolExecutor)
  registry/index.py       unrelated flat JSONL append log (not the SQLite registry)
  signal_pack/            export/gates/promote — TV-01/02/03 promoted-artifact + parity-evidence chain
  deployment/              gate.py, parity.py (live_trust_complete always False), selection.py
  sweeps/run_sweep.py     parameter-grid PLAN only, no execution
  reporting/build_report.py  minimal scaffold, tax-aware metrics only

core-rs/crates/
  mqk-backtest/           event-sourced deterministic engine; BKT-FUTURE-EXECUTION-01 causal fill contract;
                           economics.rs (BACKTEST-MULTIPLIER-*, contract-multiplier shadow ledger);
                           corporate_actions.rs, market_frame.rs, sweep.rs, strategy_lab.rs
  mqk-promotion/           evaluator.rs (metrics-threshold gate; NO walk-forward/OOS check)
  mqk-portfolio/           accounting.rs/types.rs — Fill/Lot, raw qty*price, NO multiplier field
  mqk-schemas/             AssetClass{Equity,Option,Future,Crypto,Forex}, ContractSpec (multiplier-aware)
  mqk-md/                  instrument_registry_v2.rs — additive, non-equity fail-closed outside test fixtures
  mqk-integrity/           calendar.rs — CalendarSpec::{AlwaysOn, NyseWeekdays, ...} (correctly scoped, not leaked into stats code)
  mqk-daemon / mqk-gui/    AssetCapabilityMatrix panel — proves non-equity stays disabled, not a trading feature
```

Registered CLI console scripts (`research-py/pyproject.toml [project.scripts]`): only `mqk-research` and `mqk-exp-dist`. `eval_walkforward.main_eval`, the economic walk-forward path, and the (uncommitted) multiple-testing judge have **no registered console script** — they are reachable only via direct Python calls or `python -m`.

---

## 4. Accepted Foundation — Re-Verification

Per the mission's instruction, these were treated as accepted unless a *new deterministic contradiction* was found. None was found in any of them; they hold up under direct re-reading:

- **BKT-MULTISYMBOL-MARKET-FRAME-01, BKT-FUTURE-EXECUTION-01 (+REPAIR-01/02)** — re-read `core-rs/crates/mqk-backtest/src/engine.rs` in full. Confirmed: signal-time admission vs. later-bar-only fill (`PendingBacktestOrder`), duplicate `(symbol,end_ts)` fails closed (`BacktestError::DuplicateBar`), unsorted input fails closed (`BacktestError::UnsortedInput`), forced-flatten same-bar exception is explicit and documented, conservative worst-case fill pricing (BUY@HIGH, SELL@LOW + slippage), corporate-action/integrity validated once per batch before any fill in that batch can be priced.
- **RESEARCH-PURGED-HOLDOUT-01** — re-read `research-py/src/mqk_research/ml/eval_walkforward.py` in full. Confirmed: `label_end_ts < effective_train_cutoff` and `label_end_ts < holdout_start` are both strict-inequality, both enforced with internal `RuntimeError` invariant checks (not just constructed to look correct — the code re-verifies its own postconditions at lines 406–424), holdout isolation is unconditional even when `purge_enabled=False` (only the fold-level overlap/embargo purge is toggleable).
- **RESEARCH-EXPERIMENT-REGISTRY-01** — re-read `research-py/src/mqk_research/exp_distributed/storage.py` in full. Confirmed: `finalize_attempt` refuses to reopen a terminal attempt (`RuntimeError` if status != "started"), `begin_attempt` allocates via `BEGIN IMMEDIATE` transaction with `busy_timeout`, trial identity excludes result/window by construction, `research_attempt_slices` is insert-only and fails closed on duplicate `(attempt_id, job_id)`.
- **RESEARCH-ECONOMIC-WALKFORWARD-01** — re-read `research-py/src/mqk_research/ml/economic_walkforward.py` in full (927 lines). Confirmed: never reads `targets.csv`/`fwd_ret`, per-symbol pending/executed state machine with cohort-atomic capacity allocation (`reduce_first_defer_increase_batch_v1`), fold-end forced flatten, holdout always `"reserved_not_evaluated"`.

No repairs needed to any of these. They are genuinely load-bearing and correctly documented.

---

## 5. Capability Matrix (Areas A–Q)

Legend: **C**=COMPLETE, **P**=PARTIAL, **M**=MISSING, **D**=DEFERRED_BY_DESIGN, **N/A**=NOT_APPLICABLE.

### Area A — Data / Point-in-Time Truth

| # | Item | Status | Evidence |
|---|---|---|---|
| A1 | Provider identity on ingested bars | **P → P8-ADDRESSED** | Original finding: `bars_postgres.py` `history()` selected only symbol/ts/OHLCV/volume — no provider column. **2026-08-15:** `history()` now queries and reports the actual distinct `provider_id` values for the query window via `.attrs["price_provenance"]`; still **P** because attribution is genuinely incomplete for pre-existing DB rows (separate tracked gap `MARKET-DATA-PROVIDER-PROVENANCE-01`) — the fail-closed gate correctly reports this as `"unverifiable"` rather than claiming completeness. See Section 1A. |
| A2/A3 | Content/source hashes | **C** | `io/hashing.py`, `io/manifest.py::file_record`, `ml/util_hash.py` — used pervasively (`consolidate.py`, `train.py`, `score.py`, `label_shadow_intents.py`). |
| A4 | Dataset version identity | **P** | No semantic version registry; every stage stamps a `schema_version` string + content-hash id instead. Adequate for reproducibility, not for human-readable lineage. |
| A5 | Timestamp/timezone handling | **C** | UTC-explicit, fail-closed (`bars_postgres.py::_require_tz` raises on tz-naive; all feature/label modules `pd.to_datetime(..., utc=True)`). |
| A6 | Duplicate-bar detection | **C** | `scanner/data_quality.py::REASON_DUPLICATE_BAR_TIMESTAMP`; also enforced independently in `economic_walkforward.py::load_bars` (fail-closed on duplicate `(symbol,end_ts)`). |
| A7 | Missing-bar/gap handling | **P** | Exists at the scanner admission gate (`data_quality.py`) and 1D-only coverage reporting (`market_data_coverage.py`), but not wired into the feature/training path itself (`features/compute.py` just `dropna()`s incomplete rolling windows). |
| A9/A10/A11 | Split/dividend/corporate-action handling | **M → PARTIAL-B-REPAIR** | No corporate-action ADJUSTMENT code exists in `research-py`. **2026-08-15 (Section 1B, PATCH B):** a fail-closed corporate-action PROTECTION gate now exists (`mqk_research.data.bars_provenance.check_corporate_action_integrity`, mirroring `mqk-backtest::corporate_actions.rs`'s Allow/ForbidPeriods design) — it correctly refuses real registered evaluation on raw_unadjusted data absent real exclusion evidence, rather than adjusting prices. No authoritative exclusion evidence source exists yet — see Section 1B's `BKT-CORPORATE-ACTION-EVIDENCE-SOURCE-01`. |
| A12/A13 | Symbol changes / delisting | **M** | No delist/rename mapping anywhere in `research-py`. |
| A14–A16 | Point-in-time universe / survivorship protection | **P — confirmed Mode B, correctly scoped** | `universe/build.py` is asof-correct *given its inputs*, but has no mechanism to know which symbols existed-but-were-delisted before the asof date — survivorship bias depends entirely on what the caller supplies. **2026-08-15 (P8):** confirmed by direct code reading that this is unambiguously "Mode B" (fixed, explicit ex-ante universe per the mission's P8 definition) — `run_phase1_equity`'s entire symbol universe is the operator-supplied `--symbols` argument, recorded with its construction time (`asof_utc`) in the manifest; no code path anywhere claims dynamic/index-membership PIT semantics. Not an unresolved gap — a structural, now-documented fact. |
| A17 | Adjusted vs. unadjusted price methodology | **M → P8-CLOSED** | Original finding: `bars_postgres.py:99` silently substituted the first present of `["close","c","close_micros","adj_close","close_adj"]` and relabeled it `"close"`, with no downstream field recording which convention was used. **2026-08-15:** verified the actual schema has never had an `adj_close`/`close_adj` column, and the only active provider (Alpaca) explicitly requests `adjustment=raw`; `bars_postgres.py` now computes and reports an explicit `price_adjustment_convention` (`"raw_unadjusted"` only when both the close column and every observed `provider_id` are independently verified, else `"unverifiable"`), plus a fail-closed `require_verified_price_provenance()` gate. See Section 1A. Not yet threaded into registered trial identity (deliberately out of Wave-1 scope). |
| A18 | Provider revisions/backfills | **M — confirmed, unchanged** | No bitemporal/as-of-ingestion tracking; only an `is_complete` gate. **2026-08-15:** confirmed directly in `mqk-db/src/md.rs` — the upsert is `on conflict (...) do update set ...`, i.e. later ingestion runs silently overwrite prior OHLCV/provider values with no history retained. Not addressed by P8 (mission scoped P8 to "content-addressed extracted dataset," not a bitemporal DB rebuild); remains a real gap for any future promotion-dossier work that needs to distinguish "the data as it was on date X" from "the data as it is now." |
| A20 | Dataset identity distinguishes adjustment rules | **M — deferred, correctly scoped** | Consistent with A17 — no `*_id`/`schema_version` field includes an adjustment-methodology tag YET. P8 built the provenance record but deliberately did not thread it into `ml/registry_integration.py::build_trial_identity` in Wave 1 (see Section 1A) — that's real, scoped follow-up work, not an oversight. |

### Area B — Feature / Label Causality

| # | Item | Status | Evidence |
|---|---|---|---|
| B1–B3 | Backward-only rolling windows | **C** | Every `.rolling(...)` call across `features/`, `indicators/` uses default `center=False`; zero `center=True` matches repo-wide. |
| B4 | Forward-shift leakage (`.shift(-N)`) | **C** (absence confirmed) | Zero matches for negative-lag shift anywhere in `features/`, `indicators/`, `ml/`. |
| B5 | Cross-sectional contemporaneous causality | **C** | `feature_set_v1.py` ranks across symbols sharing the *same* `end_ts` — contemporaneous, not forward-looking. |
| B6/B7 | Fold-specific standardization | **Bifurcated — P** | `eval_walkforward.py` (the registered/official path) fits `_standardize_fit` on `X_tr` only, inside the fold loop — correct. `ml/train.py` → `model_logreg.py::fit_logreg_deterministic` fits mean/std over **whatever full `X` is passed in**, with no fold-splitting inside that call path at all — safety depends entirely on the caller pre-slicing `features.csv` to train-only rows, which nothing enforces. |
| B8 | Feature-selection leakage | **N/A** | No statistical feature-selection step exists anywhere — feature set is static per `feature_schema.json`. |
| B9–B11 | Label definition / `label_end_ts` inclusive semantics | **C** | `label_shadow_intents.py` computes `fwd_ret = log(close[t+h]/close[t])`, `label_end_ts = ts[t+h]` (inclusive, explicitly documented); consumed with strict `<` everywhere downstream. |
| B17 | Future-leaking asof joins | **C** (absence confirmed) | Zero `merge_asof` matches; the one asof-like path (`--allow-asof` in `label_shadow_intents.py`) is backward-only (`searchsorted(..., side="right")-1`). |
| B19 | Global normalization before fold split | Same as B6/B7 | Correct in `eval_walkforward.py`; a real (if narrowly-scoped) gap in `ml/train.py`. |

### Area C — Walk-Forward / Holdout Governance

| # | Item | Status | Evidence |
|---|---|---|---|
| C1/C2 | Rolling window semantics | **C** | `make_folds()` — fixed train width, `train_start` slides by `step_months`; test asserts fixed-width rolling behavior. No expanding-window mode exists (not required — rolling is a legitimate, explicit choice). |
| C3/C4 | Purge/embargo exact conditions | **C** | `overlap_ok = label_end_ts < test_start`; `embargo_ok = label_end_ts < effective_train_cutoff` — both present verbatim, tested at exact boundary values. |
| C5 | Min post-purge training size | **C** | `too_few = effective_train_rows < min_rows_per_fold or test_rows < max(50, min_rows_per_fold//4)`, distinguishes `too_few_rows_after_purge` from `too_few_rows`. |
| C6 | Strict OOS-only scoring | **C** | Test predictions come only from `X_te`, disjoint from `tr_effective_mask` by construction; runtime self-check raises if violated. |
| C7 | Contiguous final holdout | **C** | `compute_holdout_boundary()` reserves one trailing contiguous `[holdout_start, dataset_end)` block; folds are bounded strictly before it. |
| C8 | Holdout excluded from train/scale/tune/economics/selection | **C** | Verified at four independent layers: mask construction, per-fold scaler fit, `multiple_testing_judge.py` fails closed if an economic artifact's `holdout` field isn't exactly `{"status":"reserved_not_evaluated"}`, and a dedicated test (`test_holdout_not_scored`) asserts holdout-labeled metric keys never appear in output. |
| C9–C11 | **Durable holdout-consumption tracking** | **M — confirmed gap, two independent agents** | No table, column, or flag anywhere records "holdout opened/consumed" for a given dataset. `registry/index.py` is an unrelated flat log; the SQLite schema in `exp_distributed/storage.py` has no such table. Nothing would stop the same holdout window from being scored twice across separate research runs — mitigated *today* only by the fact that no code path scores the holdout at all yet. |
| C12/C13 | Forced-new-holdout after holdout-informed tuning | **M** | Same root cause as C9–C11 — no consumption state exists to detect re-tuning after holdout exposure. |

### Area D — Experiment Accounting / Data Snooping

| # | Item | Status | Evidence |
|---|---|---|---|
| D1–D4 | hypothesis→trial→attempt→slice hierarchy | **C** | Full schema in `storage.py:70-122`; FK-shaped relationships, `unique(trial_id, attempt_index)`. |
| D5/D6 | Failed attempts retained, not overwritten by success | **C** | `finalize_attempt` refuses to reopen a terminal row; retries always allocate a new `attempt_index`. Tested directly. |
| D7 | Planning-only candidates excluded from attempted trials | **C** | `create_batch()` never touches `research_trials`/`research_attempts`; stamps `registry_status: "planned_not_attempted"`. |
| D8/D9 | Result-independent, pre-computed trial identity | **C** | `build_candidate_trial_identity`/`build_trial_identity` explicitly exclude window/eval-id/metrics; `trial_id` computed and `begin_attempt` called *before* the evaluation runs. |
| D10 | Winner-only registration (bug check) | **C** (absence confirmed — desired outcome) | `_finalize_candidate_attempts` always calls `finalize_attempt` regardless of outcome; tested (`test_distributed_failed_candidate_remains_registered`). |
| D11 | Historical artifacts preserved across retries | **C** | `research_attempt_slices.artifact_evidence_json` snapshots hashes at finalization time, before a later retry can overwrite the shared `exp_jobs` artifact path. Explicitly documented and tested (REPAIR-03). |
| D13/D14 | Query attempts-by-trial, trials-by-hypothesis | **C** | `list_trials()`, `list_attempts()`, `registry_summary()`. |
| D15/D16 | Trial identity covers all economically meaningful choices | **P** | `job.window` is deliberately excluded by design (documented, tested — correct for avoiding window-overcounting). But cost/slippage assumptions only enter identity if the calling strategy models them as `params`; nothing in the registry *enforces* that economically meaningful choices are captured. Low severity — a documentation/discipline gap, not a structural one. |

### Area E — Multiple Testing / Backtest Overfitting

| # | Item | Status | Evidence |
|---|---|---|---|
| E1 | Deflated Sharpe Ratio | **C (uncommitted)** | `multiple_testing_stats.py::probabilistic_sharpe_ratio`/`expected_max_sharpe` — paper-faithful (Bailey & López de Prado 2014), no-scipy erf/probit implementation, verified 20/20 tests pass including skew/kurtosis sensitivity and NaN/zero-variance fail-closed handling. |
| E2 | PBO / CSCV | **C (uncommitted)** | `combinatorial_symmetric_cv_pbo` — deterministic block partition, verified against a constructed overfit-vs-stable synthetic family (`test_overfit_family_worse_pbo_than_stable_family`), PBO≈0 for the stable family and materially higher for the overfit family, as the paper predicts. |
| E3/E4 | White's Reality Check / Hansen SPA | **D** | Not implemented; not needed on top of DSR+PBO for V1 — both already test the specific failure mode (selection bias across many candidates) that matters here; adding a third redundant test would be statistics-for-appearance, which the mission explicitly warns against. |
| E5–E7 | FDR / bootstrap / permutation inference | **D** | Same reasoning — DSR (parametric, skew/kurtosis-corrected) + PBO (nonparametric, resampling-based) already cover both a parametric and a nonparametric multiple-testing lens; a third method is not required to make a credible V1 claim. |
| E8 | Performance confidence intervals | **P** | DSR effectively gives a probability statement; no explicit bootstrap CI on Sharpe/return is produced. Could be a `USEFUL_LATER` addition, not required now. |
| E9 | Benchmark-relative model-selection test | **N/A** | No benchmark series is currently computed anywhere in the economic evaluator (see H14 below) — this is a prerequisite gap, not a missing test in this module. |
| — | Judge correctly excludes holdout | **C** | Verified directly (`_load_candidate` fails closed unless `holdout == {"status":"reserved_not_evaluated"}`; a dedicated test poisons fabricated holdout fields and proves they cannot influence the numeric output). |
| — | CLI/pipeline wiring | **M** | No console-script entry point exists for the judge; only reachable by calling `build_multiple_testing_judge` directly. |

**Verdict on Area E:** the candidate roadmap's choice of DSR+PBO/CSCV is sufficient and correctly avoids redundant statistics. The gap is not statistical, it's operational (commit + wiring).

### Area F — Execution Realism

Audited Python (`economic_walkforward.py`) and Rust (`mqk-backtest/src/engine.rs`) **separately**, per the mission's instruction.

| # | Item | Python (research) | Rust (backtest) |
|---|---|---|---|
| F1/F2 | Signal ts / execution ts | **C** — `signal_ts < execution_ts`, own-symbol-only | **C** — `PendingBacktestOrder`, same contract |
| F3–F5 | Target-symbol-only, same-bar prohibited, missing-bar unfilled | **C** | **C** |
| F6/F7 | Entry/exit price assumption | **P** — uses bar **close** only, no worst-case bar-range assumption | **C** — worst-case (BUY@HIGH, SELL@LOW) |
| F8/F9 | Adverse BUY/SELL pricing | **M** — cost is a symmetric flat bps charge on turnover, not directionally adverse | **C** — direction-aware by construction |
| F10/F11 | Commission / slippage | **C** — explicit bps, fail-closed unless `diagnostic_zero_cost=True` | **C** — commission at fill time + slippage config |
| F12–F16 | Spread / market impact / liquidity / volume participation / partial fills | **M** (none modeled) | **M** (none modeled — see judgment below) |
| F18 | Execution-delay stress | **M** | **P** — no configurable extra-bar-delay stress knob found |
| F19/F20 | EOD / forced-flatten exception | **C** — `force_flat_last_bar` | **C** — `flatten_all`, explicit same-bar exception |
| F21–F25 | Async multi-symbol execution, gross cap, cohort execution, row-order independence | **C** — `reduce_first_defer_increase_batch_v1`, tested for row/column-order invariance | **C** — batch-canonical (symbol-ascending) resolution, tested |
| F26–F28 | Cost chronology, no pre-execution P&L, target supersession | **C** | **C** |

**Judgment on F12–F16 (spread/impact/liquidity/participation/partial fills):** per the mission's instruction to decide MUST_HAVE vs. adequately-handled-by-conservative-slippage-plus-robustness-stress — **adequately handled by conservative slippage + robustness stress, for the current US equity/ETF scope**, provided the robustness gauntlet (candidate PATCH 9) actually stresses slippage assumptions (2x/3x) as planned. Building real market-impact/participation models would be institutional-grade complexity the mission explicitly warns against for a discovery-stage universe of liquid equities/ETFs. This should be revisited if the strategy universe later includes small-cap/illiquid names.

**Real finding not in the candidate roadmap:** Python's execution price model (F6–F9) is *more optimistic* than Rust's. Python charges a flat, symmetric bps cost off the close price; Rust prices BUY at the bar high and SELL at the bar low (worst-case) plus a slippage config on top. This means a strategy that looks profitable in the Python economic walk-forward could look meaningfully worse once actually run through the Rust backtest/promotion path — a real, previously-undocumented **Python↔Rust execution-realism parity gap** (see Area K, Area N).

### Area G — Portfolio / Accounting Truth

| # | Item | Status | Evidence |
|---|---|---|---|
| G1–G9 | Gross/net exposure, long-only, allocation caps, cash, turnover, cost | **C** | `economic_walkforward.py` — `max_gross_exposure`, `interval_exposure`, `gross_exposure` invariant-checked (`Fail-closed: gross exposure exceeded configured max_gross_exposure`). |
| G10/G11 | Exact wealth compounding, daily aggregation | **C** | `_daily_aggregate` — product of `(1+r)` per day, per-symbol interval returns before daily rollup. |
| G12/G13 | Overlapping positions, asynchronous symbol returns | **C** | The entire REPAIR-01/02/03 chronology exists specifically to get this right (see Section 4). |
| G14 | Rebalancing | **C** | Cohort-atomic, capacity-aware, deferral-not-rescale. |
| G15–G17 | Drawdown, annualized return, volatility | **C** | `ml/economics.py` pure functions, unit-covered. |
| G19/G20 | Risk-free / annualization assumptions | **C** | Explicit `AnnualizationSpec(annualization_days=252, risk_free_rate_annual=0.0)` — parameterized, not hardcoded (see Area O3b). |
| G21 | Fold-end flatten | **C** | `force_flat_last_bar`. |
| G23 | Symbols without bars at a timestamp | **C** | Never forward-filled into `gross_return`/`turnover` — explicitly documented as a deliberate anti-fabrication choice. |
| G24 | Exposure invariant enforcement | **C** | Runtime `RuntimeError` if `gross_exposure > max_allowed`. |

### Area H — Economic Evaluation

| # | Item | Status | Evidence |
|---|---|---|---|
| H1–H9 | Gross/net/total/annualized return, volatility, Sharpe, max drawdown | **C** | `_summarize_series` in `economic_walkforward.py`. |
| H9/H12/H13 | Turnover, hit rate, profit factor | **P** | Turnover present; no per-trade hit-rate/profit-factor computed (arguably not meaningful at the portfolio-weight level this evaluator operates at — reasonable to defer). |
| H14–H16 | Benchmark comparison, excess return, benchmark-relative risk-adjusted performance | **M** | No benchmark series (e.g., buy-and-hold SPY) is computed anywhere in the economic evaluator. |
| H17–H20 | Symbol/month/year/regime concentration | **M** | Confirmed absent — belongs to candidate PATCH 9 scope. |
| H22 | Sample-size/effective-observation context | **C** | `trading_days`, `active_days`, `folds_used` all reported. |

### Area I — Robustness / Stress Testing

Every item was searched for directly; the picture is consistently **MISSING** across the Python research layer, with the Rust backtest engine already having an equivalent to mirror:

| # | Item | Status |
|---|---|---|
| I1/I2 | Cost multiplier stress (2x/3x) | **M** — `economic_walkforward.py` comment explicitly defers this: *"This is not the 2x/3x stress gauntlet (a later patch)"*. Rust has an analogous mechanism (`conservative_defaults()`, `scenario_stress_battery_gate.rs`) already tested. |
| I3 | Execution-delay stress | **M** |
| I4/I5 | Parameter/threshold neighborhood sweeps | **M** — `sweeps/run_sweep.py` only plans a grid to CSV; nothing executes it or compares results. |
| I6 | Symbol leave-one-out | **M** |
| I7–I10 | Symbol/month/year/regime concentration | **M** |
| I11 | Stress-period-specific analysis | **M** |
| I13 | Data perturbation | **M** |
| I14 | Negative control / placebo (shuffled-label sanity check) | **M** — every "shuffle" test found is a row-order-invariance proof (pure-function determinism), not a "does this method avoid finding fake edge in noise" placebo. This distinction matters: order-invariance proves determinism; a placebo test proves statistical validity. Neither the walk-forward evaluator, the economic evaluator, nor the multiple-testing judge has the latter. |
| I16/I17 | Capacity sensitivity / cost breakeven | **M** |

**This is the most concretely unstarted area in the entire audit**, and it is exactly what candidate PATCH 9 targets — the roadmap correctly identified this gap.

### Area J — Reproducibility

| # | Item | Status | Evidence |
|---|---|---|---|
| J1 | Git SHA capture | **M** | Zero `git_sha`/`git_commit` fields anywhere in any artifact. |
| J2 | Protocol version field | **C** | `PROTOCOL_ID` constants, validated in every `*.normalized()`. |
| J3–J6 | Data/feature/target/bar/config/cost identity | **C** | Content-hashed pervasively; `economic_protocol_identity()` folds cost model into trial identity, tested. |
| J7 | Execution identity | **C (implicit)** | Execution timing is fixed by protocol version, not a free parameter — acceptable. |
| J8/J17 | Random seed / model random-state | **N/A** | `fit_logreg_deterministic` is deterministic by construction (zero-init, full-batch GD, no sampling) — there is no randomness to seed, not a missing seed. |
| J9 | Dependency/environment identity | **M** | No `numpy`/`pandas`/Python-version capture anywhere. |
| J10–J13 | Deterministic artifact IDs, immutable storage | **C** | `derive_artifact_id`, atomic tmp-file+rename writes, insert-only `research_attempt_slices`. |
| J18 | Deterministic candidate ordering | **C** | `scheduler.py` sorts grid keys before `itertools.product`; strictly incrementing `job_index`. |

### Area K — Python ↔ Rust Parity

Explicitly assessed as materially different in places, not assumed identical, per the mission's instruction.

| # | Item | Already equivalent? | Notes |
|---|---|---|---|
| K1/K7 | Signal semantics, target-weight semantics | **Intentionally different** | Python emits continuous weights; Rust operates on discrete share `qty: i64`. No documented bridge exists for turning a weight target into a share order with lot-size/cash constraints — this is the actual substance of candidate PATCH 7 ("RESEARCH-PARITY-BRIDGE-01"). |
| K7/K8 | Execution timing | **Equivalent** | Both: signal at bar T, fill only on a strictly-later bar of the same symbol. Verified directly in both engines. |
| K8/F6-F9 | Execution pricing | **MUST MATCH before promotion — currently does not** | Python: close price + flat symmetric bps. Rust: worst-case HIGH/LOW + slippage config. This is a genuine, previously-undocumented gap (see Area F, Area N). |
| K9 | Costs | **Partially equivalent** | Both charge commission+slippage in bps; Rust's is direction-aware via worst-case pricing, Python's is not. |
| K10/K11 | Allocation, gross exposure | **Equivalent in spirit** | Both cap gross exposure and defer over-cap increases; Python defers whole cohorts, Rust rejects at fill-time via `enforce_allocation_cap_micros`. Different mechanism, same invariant. |
| K12/K13 | Async execution, portfolio accounting | **Equivalent in spirit, different unit** | Python: weight-based, symbol-level ledger. Rust: share-based FIFO lots. |
| K14 | Force-flat | **Equivalent** | Both implement fold/run-end forced flatten as a same-bar exception. |
| K17/K18 | Decision identity / market-frame semantics | **Not yet bridged** | Python's `trial_id`/`economic_eval_id` and Rust's `run_id` are computed independently with no shared identity scheme linking a research trial to the backtest run that would reproduce it. |

**MUST match before promotion (per Area K's own instruction):** K8 (execution pricing convention) and the weight→share-order translation (K1/K7). Everything else can remain intentionally different (Python researches at the portfolio-weight level; Rust executes at the share level) as long as the translation layer is explicit and documented — which it currently is not.

**Confirmed, independently-tracked evidence this matters today, not just in principle:** `core-rs/crates/mqk-promotion/src/evaluator.rs` was read directly — it gates on Sharpe/MDD/CAGR/profit-factor/stress-suite/artifact-lock/provenance only. It has **no check that references walk-forward, out-of-sample, or holdout status at all.** This is independently confirmed in `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` as `PROMOTION-WALKFORWARD-GATE-WIRING-01` (P1, READY status), which the ledger itself calls "the single largest correctness gap in the research→promotion pipeline." A strategy could pass the Rust promotion gate today purely on historical metrics with zero connection to any OOS evidence produced by the entire research stack audited above.

### Area L — Promotion Dossier / Evidence

| # | Item | Status | Evidence |
|---|---|---|---|
| L1–L4 | Hypothesis/trial/attempt identity, classification metrics | **C (as inputs)** | All exist in the SQLite registry and artifacts; nothing currently composes them into one package. |
| L5/L6 | Economic metrics, OOS return identity | **C (as inputs)** | `economic_walk_forward.json` per trial. |
| L7 | Multiple-testing diagnostics | **C (as inputs, uncommitted)** | The judge artifact (Area E). |
| L8 | Robustness results | **M** | Doesn't exist yet (Area I). |
| L9 | Data provenance | **P** | Content hashes exist; adjustment-methodology/provider-identity gaps (Area A) would propagate into any dossier built today. |
| L10 | Execution assumptions | **C (as inputs)** | `economic_protocol_identity()`. |
| L11 | Parity evidence | **P (as inputs)** | `ParityEvidenceManifest` (`deployment/parity.py`) exists for live-trust gaps, honestly always `live_trust_complete=False` — but this is a *different* parity concept (backtest-vs-live) than Python-vs-Rust research parity (Area K), and the two are not currently linked. |
| L12/L13 | Holdout status / holdout-consumption status | **P / M** | Status field exists (`"reserved_not_evaluated"`); consumption ledger does not (Area C). |
| L14–L18 | Promotion status, failure reasons, Git/protocol identity, benchmark comparison, evidence timestamps | **P/M** | `PromotionDecision`/`PromotedArtifactManifest` exist in Rust/Python respectively but are not linked to each other or to the research-side registry. |

**Verdict (per the mission's explicit instruction):** a dedicated `RESEARCH-PROMOTION-DOSSIER-01` **subsystem is not necessary** — every building block already exists (content hashing, manifest schemas, trial/attempt identity, gate results, parity-evidence manifest). What's missing is a **composition** step: one function that reads all of the above for a given trial and writes one deterministic JSON dossier. This should be scoped as a composition task inside candidate PATCH 10, not a new subsystem.

### Area M — Paper Forward Validation Bridge

No infrastructure exists to compare historical research expectations against forward Paper behavior (signal rate, realized slippage, expected-vs-realized costs, etc. — M1–M14 are all effectively unaddressed). **This correctly belongs to later Paper-promotion/soak work, not Research/Backtest V1 closure** — the mission's own instruction to keep runtime scope out of this closure is the right call here; nothing here threatens the credibility of the *research* system itself.

### Area N — Adversarial Failure Audit

Concrete, deterministic findings (not fixed, per instruction — recorded here):

1. **N7 (optimistic execution price)** — Python's economic evaluator prices fills at the bar close with a flat symmetric bps cost; it never uses worst-case bar-range pricing. A strategy could show a positive discovery-phase economic edge in Python that partially or fully evaporates once priced through Rust's conservative fill model. This is real and material, not hypothetical, given F6–F9/K8 above.
2. **N4/N5 (holdout leakage / repeated holdout consumption)** — not currently *exploitable* (nothing scores the holdout yet), but the durable safeguard that would prevent future exploitation does not exist (C9–C13). Recorded as a latent, not yet active, risk.
3. **Global-fit standardization in `ml/train.py`** — a real, narrowly-scoped B6/B7/B19 leakage risk if that single-shot path (as opposed to the properly-fold-isolated `eval_walkforward.py`) is ever used to produce anything promotion-adjacent. It appears to be a lower-tier/diagnostic path today, but nothing in the code enforces that boundary or labels its output as such.
4. **N26/A17 (data-hash gaps / adjustment ambiguity)** — the `close`/`adj_close` column ambiguity in `bars_postgres.py` means two research runs against differently-adjusted source data could silently receive the same apparent "clean" identity if the adjustment convention itself isn't hashed.
5. **N22 (Python/Rust drift)** — confirmed and detailed above (Areas F, K). This is the most consequential adversarial finding in the whole audit: **the research system could validate a strategy that the execution system would price meaningfully differently, and the promotion gate that's supposed to catch this currently doesn't look at research evidence at all.**
6. **N27 (false "OOS" claims)** — not found. Every OOS claim traced back to `walk_forward_oos_predictions.csv`, itself sourced only from `te_mask` rows inside a non-skipped fold. No path was found where a training or holdout row could masquerade as OOS.
7. **N13/N14/N15 (duplicate/winner-only/retry-inflation accounting)** — not found; actively disproven by direct, working negative-control tests (`test_winner_only_registration_cannot_shrink_population`, `test_attempts_do_not_inflate_dsr_trial_count`).
8. **No exploitable nondeterminism, row-order dependence, or artifact-overwrite bug was found** in any of the accepted-foundation or newly-audited modules — every module that claims determinism has a corresponding row/column-order-invariance test that actually passes (or, for the uncommitted judge, was verified to pass in this session).

---

## 6. Deterministic Defects Discovered

None of the **accepted foundation** patches (Section 4) show a deterministic defect. The defects below are all in code *outside* that accepted set:

1. **`ml/train.py`/`ml/model_logreg.py` fit standardization globally**, not per-fold — a real leakage vector if this path feeds anything beyond diagnostics. (B6/B7/B19)
2. **No durable holdout-consumption ledger** — currently latent (nothing scores holdout yet) but a hard prerequisite before any future holdout-scoring code is written. (C9–C13)
3. **`bars_postgres.py` silently resolves `close` from an ambiguous candidate-column list** with no record of which was used, and no corporate-action/split/dividend/delisting handling exists anywhere in `research-py`. (A9–A13, A17, A20)
4. **Python economic-evaluator execution pricing is materially more optimistic than the Rust backtest engine's**, with no documented bridge or reconciliation. (F6–F9, K8, N7)
5. **The Rust promotion gate (`mqk-promotion::evaluator.rs`) has no walk-forward/OOS check at all** — independently corroborated by the master patch ledger's own `PROMOTION-WALKFORWARD-GATE-WIRING-01` entry. (K, Area L)

None of these rise to the mission's hard-stop bar (no frozen foundational contract is contradicted, no holdout/trial-identity/P&L-chronology *redefinition* is needed — items 2 and 4 need *addition*, not redefinition). All five are real and should gate V1 closure.

---

## 7. Required V1 Capabilities (MUST_HAVE_BEFORE_RESEARCH_BACKTEST_V1_CLOSE)

1. Commit + CLI-wire the multiple-testing judge (Area E).
2. Durable holdout-consumption ledger (Area C).
3. Adjustment-methodology + provider-identity provenance stamped into dataset identity (Area A).
4. Robustness gauntlet: cost-multiplier stress, execution-delay stress, symbol leave-one-out, concentration reporting, a genuine negative-control/placebo test (Area I).
5. Execution-price-model reconciliation between Python and Rust, and a documented weight→share-order translation layer (Areas F, K).
6. Wire the Rust promotion gate to actually consume walk-forward/OOS/multiple-testing evidence (`PROMOTION-WALKFORWARD-GATE-WIRING-01`) (Area K/L).
7. Compose (not build from scratch) a single promotion-dossier artifact per trial from existing pieces (Area L).
8. Git-SHA + dependency/environment identity capture in artifacts (Area J) — small, bundle into item 7.

## 8. Deferred Enhancements (USEFUL_LATER)

- Benchmark-relative comparison (H14–H16, E9) — needs a benchmark return series first; not blocking.
- Bootstrap/permutation confidence intervals on top of DSR/PBO (E8) — genuinely redundant for V1's purposes.
- Capacity sensitivity / cost breakeven analysis (I16/I17) — useful once real capital sizing is being decided, not for discovery-stage research.
- Hit-rate/profit-factor at the trade level (H12/H13) — not clearly meaningful at this evaluator's portfolio-weight granularity.
- A17's adjustment-methodology fix could, once resolved, retroactively enable proper A9–A11 split/dividend handling — but if the DB source already applies a single consistent adjustment convention (plausible, unconfirmed), full corporate-action modeling in `research-py` itself may be genuinely unnecessary. **This needs a factual verification step (query what `md_bars` actually stores), not an assumption**, and is scoped into the data-provenance patch (item 3 above).

## 9. Deferred Multi-Asset Work (MULTIASSET_FUTURE_WORK)

- Full futures/options/FX/crypto data adapters (currently honest `NotImplementedError` stubs) — correctly deferred, blocked on data pipelines that don't exist yet, not an architecture problem.
- `mqk-portfolio`'s multiplier-aware accounting for the *live/paper* runtime (the backtest engine already has a working shadow-ledger workaround; live/paper does not) — real future work, safely deferred, scoped to one crate.
- Any actual enablement of non-equity `AssetClass`/`InstrumentRegistryV2` paths — deliberately fenced off with fail-closed gates today; this is correct, not a gap.

---

## 10. Multi-Asset Extensibility Map

### 10.1 Asset-agnostic components
Instrument schema (`instruments/schema.py`, tagged EQUITY/OPTIONS/FUTURES union with deterministic ID parsing), `mqk_schemas::AssetClass`/`ContractSpec` (Rust, richer — includes Crypto/Forex), `BacktestInstrumentEconomics` (contract-multiplier notional/P&L math, tested against synthetic ES-futures and options fixtures), annualization functions in `ml/economics.py` (252 is a default parameter, not a hardcoded constant), the entire hypothesis→trial→attempt→slice registry, purged walk-forward/holdout machinery, DSR/PBO judge.

### 10.2 Equity-specific components (by design, correctly scoped)
`mqk-integrity::CalendarSpec::NyseWeekdays` (equity session hours, correctly isolated to the integrity/execution layer, not leaked into generic research/stats code), `corporate_actions.rs`'s `ForbidPeriods` policy (opt-in, no-op by default), the current `AlwaysOn` calendar variant already exists as the crypto/24-7 seam.

### 10.3 Accidentally equity-coupled components
`mqk-portfolio::{accounting.rs, types.rs}` — `Fill`/`Lot` and the FIFO P&L math have **no multiplier field at all**, and this crate is shared by the live/paper runtime (not just backtest). This is the one place where adding real futures/options support would require touching genuinely shared, already-in-production code, not just adding an adapter. It is well-scoped (one crate) rather than smeared across the codebase.

### 10.4 Existing extension interfaces
`instrument_id` tagged-union parsing (Python), `mqk_schemas::AssetClass` end-to-end through the execution gateway, `InstrumentRegistryV2` (Rust, additive, fail-closed outside test fixtures), `BacktestEngine::with_economics()` builder seam, GUI `AssetCapabilityMatrixPanel` (proves non-equity stays disabled — the correct shape for a not-yet-implemented feature).

### 10.5 Missing extension interfaces
No multiplier-aware seam in `mqk-portfolio` itself (10.3). No currency-pair/venue field on the Python `Instrument` base (present in Rust's `mqk_schemas::Instrument`, not yet mirrored in Python). No FX/Crypto tagged variant in the Python `instruments/schema.py` (Rust already has both).

### 10.6–10.9 Future asset module maps

| Component | Futures | Options | FX | Crypto |
|---|---|---|---|---|
| Data | `NEW_ASSET_ADAPTER` (`futures_stub.py` names the contract, needs real Postgres schema + ingestion) | `NEW_ASSET_ADAPTER` (`options_stub.py`, same) | `NEW_ASSET_ADAPTER` | `NEW_ASSET_ADAPTER` |
| Instrument metadata | `REUSE_EXISTING_CORE` (`FutureInstrument`, `ContractSpec::Future`) | `REUSE_EXISTING_CORE` (`OptionInstrument`, `ContractSpec::Option`) | `NEW_ASSET_ADAPTER` (no FX Python type yet; Rust has the shape) | `NEW_ASSET_ADAPTER` (same) |
| Calendar | `NEW_ASSET_ADAPTER` (contract-session hours, roll rules) | `REUSE_EXISTING_CORE` (underlying equity calendar, mostly) | `NEW_ASSET_ADAPTER` (session conventions) | `REUSE_EXISTING_CORE` (`CalendarSpec::AlwaysOn` already exists) |
| Features | `REUSE_EXISTING_CORE` (generic bar-based features) | `NEW_ASSET_ADAPTER` (IV/Greeks provenance not modeled anywhere) | `REUSE_EXISTING_CORE` | `REUSE_EXISTING_CORE` |
| Execution | `NEW_ASSET_ADAPTER` (tick rounding, roll behavior) | `NEW_ASSET_ADAPTER` (exercise/expiry lifecycle) | `NEW_ASSET_ADAPTER` | `REUSE_EXISTING_CORE` (mostly — multiplier already =1-shaped) |
| Costs | `REUSE_EXISTING_CORE` (bps model generalizes) | `REUSE_EXISTING_CORE` | `NEW_ASSET_ADAPTER` (pip/spread conventions) | `REUSE_EXISTING_CORE` |
| Portfolio accounting | `NEW_ASSET_SPECIFIC_ENGINE` (margin-correct P&L needs `mqk-portfolio` multiplier support, 10.3) | `NEW_ASSET_SPECIFIC_ENGINE` (same root cause) | `NEW_ASSET_ADAPTER` | `REUSE_EXISTING_CORE` (multiplier=1) |
| Risk | `NEW_ASSET_ADAPTER` (margin-based limits) | `NEW_ASSET_ADAPTER` | `NEW_ASSET_ADAPTER` | `REUSE_EXISTING_CORE` |
| Backtest | `REUSE_EXISTING_CORE` (multiplier seam already tested for ES/options-style fixtures) | `REUSE_EXISTING_CORE` | `NEW_ASSET_ADAPTER` | `REUSE_EXISTING_CORE` |
| Research | `REUSE_EXISTING_CORE` (entire statistical framework is asset-agnostic) | `REUSE_EXISTING_CORE` | `REUSE_EXISTING_CORE` | `REUSE_EXISTING_CORE` |
| Promotion/parity | `REUSE_EXISTING_CORE` (once Area K/L closure work lands) | `REUSE_EXISTING_CORE` | `REUSE_EXISTING_CORE` | `REUSE_EXISTING_CORE` |

### 10.10 Anti-rewrite check

**Answer: A (adapters/modules), not B (rewrite) — for the research statistical core.** The hypothesis/trial/attempt registry, purged walk-forward/holdout, DSR/PBO judge, and reproducibility machinery contain zero asset-specific assumptions and were independently confirmed asset-agnostic across every audited file.

**One coupling requires attention, but not before Equity V1 closes:** `mqk-portfolio`'s missing multiplier field (10.3) is `SAFE_TO_DEFER_UNTIL_MULTI_ASSET` — the backtest engine already proved the workaround pattern (an additive shadow ledger) works and is byte-identical at multiplier=1, so equity V1 is unaffected, and futures/options can't trade live/paper today regardless (fenced off by `AssetCapabilityMatrix` and CLI refusal gates). No repair is `REQUIRED_BEFORE_EQUITY_V1_CLOSE`.

---

## 11. Candidate Roadmap Assessment

**Updated 2026-08-15 (Wave-1 closure) — see Section 1A for the full reconciliation.**

The candidate roadmap (Patch 6–10) was **directionally correct but incompletely scoped**; Wave 1 closed most of that gap:

- **PATCH 6 → P6-CLOSURE** — was mislabeled as unstarted work. It was functionally complete and tested; Wave 1 closed it as "commit + wire," not "implement." **CLOSED** (commit `fbb63fc7`).
- **PATCH 6B (not in the original roadmap) → P6B** `RESEARCH-HOLDOUT-CONSUMPTION-LEDGER-01`. **CLOSED** (commit `686614cc`).
- **New, added by Wave-1 Correction 1 → P6C** `RESEARCH-LEGACY-TRAINING-BOUNDARY-01` — closes the B6/B7/B19 global-fit-standardization leakage boundary this audit flagged (Section 6 item 1) but the original roadmap omitted. **CLOSED** (commit `c643588d`).
- **PATCH 7 (RESEARCH-PARITY-BRIDGE-01) → split by Wave-1 Correction 2** into **P7A** `RESEARCH-EXECUTION-PRICING-PARITY-01`, **P7B** `RESEARCH-WEIGHT-TO-SHARE-PARITY-01`, **P7C** `PROMOTION-OOS-EVIDENCE-GATE-01` (absorbs the already-tracked `PROMOTION-WALKFORWARD-GATE-WIRING-01`). The original single-patch framing bundled three materially distinct invariants; the roadmap correction is recorded here, but **none of the three were implemented in Wave 1** — still **OPEN**.
- **PATCH 8 (BKT-DATA-PROVENANCE-POINT-IN-TIME-01) → P8.** **CLOSED** (commit `2b87c400`), narrowly scoped after factual verification — see Section 1A. Corporate-action modeling was confirmed unnecessary for V1 (single verified raw/unadjusted convention, no evidence of adjusted data anywhere in the actual ingestion path), not merely assumed unnecessary as the original audit had it pending verification.
- **PATCH 9 (BKT-ROBUSTNESS-GAUNTLET-01)** — correctly scoped and validated by this audit as the single most concretely unstarted area (Area I). **OPEN**, not attempted in Wave 1.
- **PATCH 10 (RESEARCH-BACKTEST-FINAL-ACCEPTANCE-01)** — under-scoped as a "final acceptance" checkpoint; this audit clarifies it should own the promotion-dossier *composition* (Area L verdict: composition, not a new subsystem) and the small reproducibility additions (git SHA, dependency identity). **OPEN**, not attempted in Wave 1; now additionally depends on P7A/P7B/P7C rather than a single P7 (see Section 13).

---

## 12. Final Minimum Patch List

**Updated 2026-08-15.** 8 patches, not the original roadmap's 5 or this audit's first-pass 6 — P6C was a real omission (Correction 1), and P7 was three invariants wearing one patch ID (Correction 2).

| Patch ID | Objective | Status |
|---|---|---|
| **P6-CLOSURE** `RESEARCH-MULTIPLE-TESTING-JUDGE-01-CLOSURE` | Commit the existing, tested DSR/PBO implementation; add a CLI/pipeline entry point. | **CLOSED** (`fbb63fc7`); DSR trial-count semantics repaired 2026-08-15 by **PATCH A** `RESEARCH-MULTIPLE-TESTING-JUDGE-01-REPAIR-01` (`8bc4dbc2`, CLOSED) — see Section 1B |
| **P6B** `RESEARCH-HOLDOUT-CONSUMPTION-LEDGER-01` | Durable SQLite table + API recording when a dataset's reserved holdout region is opened/consumed, and by which trial/artifact. | **CLOSED** (`686614cc`) |
| **P6C** `RESEARCH-LEGACY-TRAINING-BOUNDARY-01` | Fail-closed structural boundary preventing `ml/train.py`'s single-shot/global-fit output from being mistaken for promotion-grade OOS evidence. | **CLOSED** (`c643588d`) |
| **P8** `BKT-DATA-PROVENANCE-POINT-IN-TIME-01` | Verify and stamp the actual adjustment convention used by `md_bars`; add provider-identity capture; add provider-revision/backfill detection; make an explicit, documented decision on corporate-action/delisting scope for V1 based on what's found. | **CLOSED** (`2b87c400`), narrowly scoped; corporate-action conclusion corrected 2026-08-15 by **PATCH B** `BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-01` (`4f7e297e`, **PARTIAL — CORPORATE_ACTION_SOURCE_REQUIRED**) — see Section 1B |
| **PATCH A** `RESEARCH-MULTIPLE-TESTING-JUDGE-01-REPAIR-01` | Correct DSR effective-independent-trial accounting per Bailey & López de Prado 2014 Appendix A.3; fix zero-cross-trial-variance null benchmark; tighten comparison scope. | **CLOSED** (`8bc4dbc2`) |
| **PATCH B** `BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-01` | Durable bars provenance manifest + fail-closed corporate-action preflight, threaded into registered economic trial identity. | **PARTIAL — CORPORATE_ACTION_SOURCE_REQUIRED** (`4f7e297e`); superseded by **PATCH C** — see Section 1C |
| **PATCH C** `BKT-DATA-PROVENANCE-POINT-IN-TIME-01-REPAIR-02` | Bind manifest to actually-loaded bars content; require real content-addressed corporate-action evidence (not a bare ID string); remove physical bars-file bytes from economic trial identity; update multiple-testing comparison key to the corrected provenance authority. | **`P8_CONTRACT_COMPLETE` / `DATA_SOURCE_BLOCKED`** (`5bba8d6c`) — see Section 1C |
| **P7A** `RESEARCH-EXECUTION-PRICING-PARITY-01` | Reconcile execution-price-model divergence between Python and Rust (Area F/K/N: Python prices at close + flat symmetric bps; Rust prices worst-case HIGH/LOW + slippage). | **OPEN** |
| **P7B** `RESEARCH-WEIGHT-TO-SHARE-PARITY-01` | Document/implement the weight→share-order translation layer bridging Python's continuous portfolio-weight semantics and Rust's discrete `qty: i64` share semantics (Area K1/K7). | **OPEN** |
| **P7C** `PROMOTION-OOS-EVIDENCE-GATE-01` | Wire the Rust promotion gate (`mqk-promotion::evaluator.rs`) to actually consume Python walk-forward/OOS/multiple-testing evidence; absorbs the already-tracked `PROMOTION-WALKFORWARD-GATE-WIRING-01`. | **OPEN** |
| **P9** `BKT-ROBUSTNESS-GAUNTLET-01` | Cost-multiplier stress (2x/3x), execution-delay stress, a conservative/worst-case pricing mode mirroring Rust's (feeding P7A's reconciliation), symbol leave-one-out, month/year/regime concentration reporting, parameter-neighborhood sweep *execution* (not just planning) tied to DSR/PBO sensitivity, a genuine negative-control/placebo test. | **OPEN** |
| **P10** `RESEARCH-BACKTEST-FINAL-ACCEPTANCE-01` | Compose the promotion dossier from existing pieces (Area L); register missing CLI entrypoints for `eval_walkforward` and economic walk-forward (the judge's own entrypoint is now closed by P6-CLOSURE); add git-SHA + dependency-identity capture; final `RESEARCH_BACKTEST_V1_COMPLETE` gate check script. | **OPEN** |

**4 of 8 patches CLOSED as of Wave 1; 4 remain OPEN** (P7A, P7B, P7C, P9, P10 — five patch IDs, four of the original roadmap's patch-count slots since P7's split absorbed no new scope, only clarified existing scope into three trackable pieces).

---

## 13. Dependency Graph

**Updated 2026-08-15** to reflect P6C and the P7A/P7B/P7C split.

```
P6-CLOSURE (CLOSED) ──┐
                      ├──►  P10 (dossier composition needs the judge's real output shape)
P6B (CLOSED) ─────────┤
                      │
P6C (CLOSED) ─────────┘   (no downstream dependents in Wave 1's scope; P10's dossier
                            composition should still cite P6C's evidence-boundary
                            classification per trial, but nothing REQUIRES it)
P8 (CLOSED) ──────────┬──►  P7A (pricing reconciliation targets the confirmed
                      │         raw_unadjusted convention, not an assumed one)
                      ├──►  P7B (weight→share translation is independent of P8's
                      │         findings but sequenced here for Wave clarity)
                      │
                      └──►  P9 (robustness gauntlet's conservative-pricing mode should
                                target the convention P8 confirmed, to avoid rework)
                                      │
        P7A, P7B ──────────────────► P7C (promotion gate wiring should land after the
                                           pricing/translation contracts it will gate on
                                           are settled, to avoid re-wiring)
                                      │
                                      ▼
                                     P10 (final gate needs P7C's promotion-wiring +
                                           P9's robustness evidence + P6B's ledger +
                                           P6C's evidence-boundary classification)
```

P6-CLOSURE, P6B, and P6C are all now CLOSED and had no dependencies on each other beyond sequencing (P6-CLOSURE/P6B share the same SQLite registry file). P8 is CLOSED. P7A and P7B depend on P8's now-confirmed findings (or are independent of them, per above) and can run concurrently with each other. P7C should sequence after P7A/P7B settle their contracts. P9 depends on P8's confirmed pricing convention (not on P7A's reconciliation directly, though sequencing after P7A avoids rework). P10 depends on P6B, P6C, P7C, and P9.

---

## 14. Autonomous Execution Waves

**Updated 2026-08-15 — WAVE 1 is now CLOSED.** P6C (added by Correction 1) was folded into WAVE 1's patch list since it shares WAVE 1's `AUTO_SAFE`/`AUTO_SAFE_WITH_CHECKPOINT` safety class and has no cross-wave dependencies. The independent-review checkpoint after WAVE 1 (below) has not yet occurred as of this document update — this Wave's commits are local/unpushed pending exactly that review, per the mission brief.

**WAVE 1 — Foundation closure (parallel-safe) — CLOSED, pending independent review**
Entry condition: none; ready now.
Patches: P6-CLOSURE, P6B, P6C, P8.
Order actually used: P6-CLOSURE → P6B → P6C → P8, sequentially (one commit per patch, per mission discipline — not run concurrently even though P8 and P6C were independent of P6-CLOSURE/P6B).
Result: all four **CLOSED** (commits `fbb63fc7`, `686614cc`, `c643588d`, `2b87c400`). No stop condition was triggered — the `md_bars` adjustment convention was confirmed consistent (single provider, single explicit `adjustment=raw` request; see Section 1A), not inconsistent, so A9-A11 was not upgraded from deferred to required.
Validation actually run: `pytest research-py/tests/test_multiple_testing_judge.py` (21/21), `test_experiment_registry.py` (47/47), `test_economic_walkforward.py` (63/63), `test_ml_eval_walkforward_purged_holdout.py` (37/37), `test_holdout_ledger.py` (11/11, new), `test_evidence_boundary.py` (6/6, new), `test_bars_postgres_provenance.py` (9/9 default, 2 opt-in skipped), full suite `1182 passed, 5 skipped, 2 deselected`.
Independent-review checkpoint: **still required, not yet performed** — this document update happens before that review, per the mission's own sequencing (Wave stays local until ChatGPT review completes).

**WAVE 2 — Parity contract**
Entry condition: P8 complete (**now true**).
Patches: P7A, P7B, P7C (was a single "P7" before Correction 2's split; see Section 12).
Stop conditions: any need to redefine trial identity or P&L chronology to make parity work (hard-stop per mission); any Rust/runtime scope creep beyond `mqk-promotion`/`mqk-backtest`.
Validation: `cargo test -p mqk-promotion -p mqk-backtest`, new scenario tests proving the Rust gate now rejects a candidate lacking OOS evidence (P7C specifically).
Independent-review checkpoint: required (alters Python/Rust parity contracts — CLAUDE.md's explicit stronger-review category).
Status: **not started.**

**WAVE 3 — Robustness**
Entry condition: P7A complete (pricing-convention decision made and reconciled).
Patches: P9.
Stop conditions: negative-control test failing to fail (i.e., the placebo test finding "significant" edge in shuffled data would indicate a deeper methodology bug requiring escalation, not a robustness-gauntlet fix).
Validation: `pytest research-py/tests/` (new robustness suite) + `cargo test -p mqk-backtest scenario_stress_battery_gate` for cross-reference.
Independent-review checkpoint: after completion.
Status: **not started.**

**WAVE 4 — Final acceptance**
Entry condition: P6-CLOSURE, P6B, P6C, P7C, P9 all complete.
Patches: P10.
Stop conditions: any discrepancy discovered while composing the dossier between two supposedly-independent authorities (e.g., registry population count vs. judge's own count) — mission hard-stop condition 13.
Validation: full `research-py` test suite; one real dossier produced end-to-end for a synthetic candidate and manually inspected.
Independent-review checkpoint: required (final acceptance/promotion evidence chain).
Status: **not started.**

---

## 15. Automation Safety Classes

**Updated 2026-08-15** with P6C and the P7 split.

| Patch | Class | Reasoning | Status |
|---|---|---|---|
| P6-CLOSURE | **AUTO_SAFE** | Commit of already-tested, already-passing code + a thin CLI wrapper. No semantic change. | CLOSED |
| P6B | **AUTO_SAFE_WITH_CHECKPOINT** | Additive only (no redefinition of holdout semantics), but touches holdout-adjacent code — CLAUDE.md's stronger-review list includes "alter holdout semantics"; this is adjacent enough to warrant a checkpoint even though it doesn't alter semantics. | CLOSED |
| P6C | **AUTO_SAFE** | Purely additive structural classifier + one self-labeling field on an already-uncalled diagnostic path; zero changes to `eval_walkforward.py`/`economic_walkforward.py`. | CLOSED |
| P8 | **AUTO_SAFE_WITH_CHECKPOINT** | Changes point-in-time data meaning — explicitly named in CLAUDE.md's stronger-review category. Actual implementation was additive-only (`.attrs` metadata + a new opt-in fail-closed helper); `history()`'s existing return contract for current callers is unchanged. | CLOSED |
| P7A | **MANUAL_REVIEW_REQUIRED** | Explicitly changes execution-pricing parity between Python and Rust — named in CLAUDE.md's stronger-review category. | OPEN |
| P7B | **MANUAL_REVIEW_REQUIRED** | Weight→share order translation touches the boundary between research-level and execution-level semantics. | OPEN |
| P7C | **MANUAL_REVIEW_REQUIRED** | Modifies the live promotion gate (`mqk-promotion::evaluator.rs`) — named in CLAUDE.md's stronger-review category. | OPEN |
| P9 | **AUTO_SAFE_WITH_CHECKPOINT** | Additive (new stress modes, new reporting) with no change to existing chronology/identity/accounting contracts. | OPEN |
| P10 | **MANUAL_REVIEW_REQUIRED** | Final acceptance / promotion evidence composition — the point at which every other patch's output gets certified together; errors here are the most consequential. | OPEN |

---

## 16. RESEARCH_BACKTEST_V1_COMPLETE Gate

**Updated 2026-08-15** with per-item status and P6C/P7-split. See Section 16A
immediately below for `RESEARCH_BACKTEST_FOUNDATION_READY`, the new
earlier-and-narrower gate introduced to resolve a contradiction between this
section and the original Section 17 (full reasoning in Section 1A).

All of the following must be true:

- **DATA TRUTH:** dataset adjustment convention verified and stamped into a fail-closed provenance record, now threaded into registered trial identity (PATCH B — durable manifest **MET**; see Section 1B); provider identity captured (**MET** in the same sense; the DB's actual `provider_id` attribution gap for pre-existing rows is a separate, already-tracked issue this gate does not claim to have fixed, and correctly fails closed on it). **CORRECTED 2026-08-15 (Section 1B): overall DATA TRUTH is NOT MET** — corporate-action safety is part of data truth for raw_unadjusted data, and no authoritative corporate-action evidence source exists yet, so official registered evaluation on real data is fail-closed-blocked.
- **FEATURE CAUSALITY:** already met (Area B — no action required). **MET.**
- **LABEL CAUSALITY:** already met (Area B — no action required). **MET.**
- **WALK-FORWARD:** already met (Area C — no action required). **MET.**
- **HOLDOUT:** durable consumption ledger exists (P6B — **MET**), in addition to the already-met purge/embargo/isolation guarantees.
- **EXPERIMENT ACCOUNTING:** already met (Area D — no action required). **MET.**
- **MULTIPLE TESTING:** judge committed and wired (P6-CLOSURE — **MET**).
- **TRAINING EVIDENCE BOUNDARY:** single-shot/global-fit training output cannot be mistaken for promotion-grade OOS evidence (P6C — **MET**; not in the original gate definition, added because Correction 1 added P6C to the roadmap).
- **ECONOMIC EXECUTION:** pricing-convention reconciled between Python and Rust (P7A — **OPEN**); robustness gauntlet passing (P9 — **OPEN**).
- **COSTS:** cost-multiplier stress passing at 2x/3x (P9 — **OPEN**).
- **PORTFOLIO ACCOUNTING:** already met (Area G — no action required). **MET.**
- **ROBUSTNESS:** full gauntlet (P9 — **OPEN**) implemented and passing, including a genuine negative-control/placebo test.
- **REPRODUCIBILITY:** git-SHA + dependency identity captured (P10 — **OPEN**); everything else already met.
- **PYTHON/RUST PARITY:** promotion gate consumes OOS evidence (P7C — **OPEN**); execution-pricing convention documented and reconciled (P7A — **OPEN**); weight→share translation documented (P7B — **OPEN**).
- **PROMOTION EVIDENCE:** one composed dossier artifact exists per trial (P10 — **OPEN**).

**REQUIRED_FOR_V1:** all of the above.
**CURRENT STATUS (2026-08-15): NOT MET.** Originally 8 of 15 line items MET after Wave 1; the remaining 7 traced to P7A/P7B/P7C/P9/P10. **CORRECTED same day (Section 1B):** the DATA TRUTH line item's "MET" was based on the now-corrected A9-A13/A17 conclusion — it is **NOT MET** pending a corporate-action evidence source, so **7 of 15** line items are actually MET.
**FUTURE_INSTITUTIONAL_ENHANCEMENT:** benchmark-relative comparison, bootstrap/permutation CIs, capacity/breakeven analysis, trade-level hit-rate/profit-factor.
**FUTURE_MULTI_ASSET_IMPLEMENTATION:** everything in Section 10.6–10.9's `NEW_ASSET_ADAPTER`/`NEW_ASSET_SPECIFIC_ENGINE` columns.

---

## 16A. RESEARCH_BACKTEST_FOUNDATION_READY Gate (new, 2026-08-15)

**Why this gate exists:** the original Section 17 (`ALPHA_DISCOVERY_READY`)
required Section 16 (`RESEARCH_BACKTEST_V1_COMPLETE`) to be met as its first
condition, then immediately said it "intentionally stops short of requiring
the promotion dossier (P10) to be final" — but Section 16's `REQUIRED_FOR_V1`
already includes P10's dossier composition ("PROMOTION EVIDENCE" line item).
That is an internal contradiction: Section 17 cannot both require Section 16
in full and simultaneously exempt part of what Section 16 requires. Per the
mission's explicit instruction, this is resolved by naming a separate,
earlier, narrower gate rather than quietly softening Section 16's own
definition.

**Definition:** `RESEARCH_BACKTEST_FOUNDATION_READY` is met when every line
item in Section 16 is met **except** "PROMOTION EVIDENCE" (P10's dossier
composition) and the P10-specific half of "REPRODUCIBILITY" (git-SHA +
dependency-identity capture, which is scoped into P10 per Section 12). In
other words: `RESEARCH_BACKTEST_FOUNDATION_READY` = P6-CLOSURE + P6B + P6C +
P8 + P7A + P7B + P7C + P9, all CLOSED. `RESEARCH_BACKTEST_V1_COMPLETE` =
`RESEARCH_BACKTEST_FOUNDATION_READY` + P10.

**Current status (2026-08-15): NOT MET.** P6-CLOSURE, P6B, P6C are CLOSED;
P8 is CLOSED-but-narrowly-scoped with its corporate-action conclusion
corrected same day (Section 1B) — **P8 is not sufficient for DATA TRUTH on
its own** until `BKT-CORPORATE-ACTION-EVIDENCE-SOURCE-01` (Section 1B, not
yet scoped) lands; P7A, P7B, P7C, P9 remain OPEN.

---

## 17. ALPHA_DISCOVERY_READY Gate

**Updated 2026-08-15 — now depends on Section 16A, not Section 16, resolving the contradiction described there.**

Alpha discovery at meaningful scale may begin once:

1. `RESEARCH_BACKTEST_FOUNDATION_READY` (Section 16A) is met — **not** the full `RESEARCH_BACKTEST_V1_COMPLETE` (Section 16), which additionally requires P10's dossier composition.
2. The robustness gauntlet (P9) has run against at least one real historical candidate as a smoke test, confirming the negative-control/placebo test correctly reports no significant edge in shuffled data (proving the pipeline itself isn't the source of false positives).
3. The Rust promotion gate visibly rejects a synthetic candidate that lacks OOS evidence (proof that P7C's wiring is load-bearing, not decorative).

This intentionally stops short of requiring the promotion dossier (P10) to be "final" in every cosmetic sense — the point of this gate is to stop infrastructure work and start testing real candidates, not to achieve perfection first. Condition 1 already encodes that exemption structurally (via Section 16A's definition) rather than as an ad-hoc exception bolted onto Section 16's own requirements.

**Current status (2026-08-15): NOT MET** — Section 16A is not yet met (P7A/P7B/P7C/P9 all OPEN), so condition 1 fails regardless of conditions 2-3.

---

## 18. MULTIASSET_EXTENSION_READY Gate

This does **not** mean multi-asset is implemented. It means the current framework's seams are clean enough that adding it later is adapter work, not a rewrite. Based on Section 10:

- **Already met** for the research statistical core (registry, walk-forward, holdout, judge, reproducibility) — zero asset-specific coupling found anywhere in this layer.
- **Already met** for the backtest engine's notional/P&L math (`BacktestInstrumentEconomics`, tested against futures/options-style fixtures).
- **Not yet met** for the live/paper portfolio-accounting layer (`mqk-portfolio`) — no multiplier field exists there. This does **not** block Equity V1 closure or this gate's overall verdict, because (a) non-equity trading is fenced off with fail-closed gates today regardless, and (b) the backtest engine already proved the adapter pattern (additive shadow ledger) works, so the same pattern can be applied to `mqk-portfolio` when multi-asset work actually begins — it is scoped, understood, and deferred, not unknown.

**Verdict: MULTIASSET_EXTENSION_READY = true, with one documented, scoped, deferred exception** (live/paper portfolio accounting multiplier support).

---

## 19. Risks / Hard-Stop Conditions for the Future Controller

Carried forward verbatim from the mission brief, plus audit-specific annotations. **Updated 2026-08-15** with Wave-1 outcomes.

- Any deterministic defect found in the accepted foundation (Section 4) mid-implementation → stop. None was found during this audit **or during Wave-1 implementation** — `eval_walkforward.py` and `economic_walkforward.py` were re-read but deliberately left unmodified by both P6C and P8. P7A/P9 will touch code adjacent to that foundation and should re-verify it hasn't drifted before they begin.
- P7A/P7B/P7C must not redefine trial identity or P&L chronology to achieve parity — if the only way to reconcile Python/Rust pricing turns out to require changing what a "trial" means, that's a hard stop requiring a new mission, not a patch-level decision. (Still open; not yet tested against real implementation pressure.)
- P6B must not be implemented as a redefinition of holdout semantics — it is purely additive tracking. **Verified in Wave 1:** the ledger never evaluates or scores holdout data (structural grep-based test), and `eval_walkforward.py`'s `reserved_not_evaluated` status is unchanged.
- If P9's negative-control/placebo test fails to fail (finds "significant" edge in shuffled/randomized data), that is not a robustness-gauntlet bug to patch around — it indicates a methodology problem somewhere upstream and requires escalation per the mission's hard-stop condition 8. (Still open — P9 not started.)
- P8's data-provenance verification could reveal that `md_bars` adjustment conventions are inconsistent across symbols or time — **resolved in Wave 1:** confirmed a single consistent convention (raw/unadjusted, one provider code path). A9-A11 was NOT upgraded to required; this is now a confirmed fact, not an open risk.
- **New risk surfaced by P8, carried forward for P7A/P9/P10:** this box's real data (`AAPL`) currently reports `price_adjustment_convention = "unverifiable"` via the new fail-closed gate, because `provider_id='unknown'` for those rows (a separate, already-tracked attribution bug — `MARKET-DATA-PROVIDER-PROVENANCE-01`, not yet merged). Any future patch that calls `require_verified_price_provenance()` against this box's current data will correctly refuse to proceed until that merge lands. This is intentional fail-closed behavior, not a Wave-1 defect — flagged here so a future controller doesn't mistake it for one.

## 20. Recommended First Autonomous Wave

**Updated 2026-08-15 — WAVE 1 is now CLOSED (pending independent review); this section now recommends the next Wave.**

~~**WAVE 1** (P6-CLOSURE, P6B, P8) is the correct starting point...~~ — **superseded.** WAVE 1 executed as P6-CLOSURE → P6B → P6C → P8 (P6C added by Correction 1; all four `AUTO_SAFE`/`AUTO_SAFE_WITH_CHECKPOINT`), all four CLOSED locally, nothing pushed. Full `research-py` suite green (1182 passed, 5 skipped, 2 deselected).

**Recommended next Wave: WAVE 2 — Parity contract (P7A, P7B, P7C).** Entry condition (P8 complete) is now satisfied. P7A and P7B can run concurrently with each other (no shared file); P7C should sequence after both since it wires the promotion gate to consume evidence whose exact shape P7A/P7B may still adjust. All three are `MANUAL_REVIEW_REQUIRED` per Section 15 — this Wave should **not** be executed with the same "autonomous, checkpoint-after" cadence as Wave 1; each of P7A/P7B/P7C touches either the live promotion gate or a Python/Rust parity contract, both explicitly named in CLAUDE.md's stronger-review category. Recommend a per-patch review checkpoint, not just an end-of-Wave one.
