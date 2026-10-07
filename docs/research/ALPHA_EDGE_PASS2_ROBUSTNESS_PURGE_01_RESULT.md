# ALPHA EDGE PASS 2 — ROBUSTNESS PURGE 01 — RESULT

Status: LOCALLY COMPLETE, awaiting independent review. Discovery-period evidence only. NOT_VALIDATED. PROMOTION_AUTHORITY = NONE.
Confirmation: NOT RUN. Final holdout: RESERVED / UNCONSUMED. Paper: INACTIVE. Live: DISABLED / UNTOUCHED.

Experiment: `research-py/experiments/alpha_edge_pass2_01/` (modules `p2_*.py`), tests `research-py/tests/test_alpha_edge_pass2_01.py`,
evidence `research-py/experiments/alpha_edge_pass2_01/{PASS2_PREDECLARATION,PASS2_COHORT_MANIFEST,PASS2_FREEZE_PROOF}.json` and `results/`.

## What Pass 2 is

A robustness purge of the accepted, pushed-verified Alpha Edge Census Pass-1 result (Discovery `[2016-01-01, 2024-01-01)`,
88-symbol current-registry snapshot — **not point-in-time; the survivorship caveat is mandatory**). It advances exactly:

* every Pass-1 `DISCOVERED_MODERATE` StrategyEdge — **789** (Pass-1 WEAK never advances);
* every Pass-1 `DISCOVERED_STRONG` ConditionalEdge — **135** (WEAK/MODERATE never advance).

Stress runs are robustness evaluations of the accepted candidates, **not new trials**: no Pass-1 trial, attempt, FactorSpec or
registry record was created or modified. The 924 robustness candidates live in a Pass-2-only attempt store
(`ResearchResultStore`, experiment `alpha_edge_pass2_01`, one durable attempt per candidate opened before evaluation).

Generic S01–S14 research configs are not production-native strategies and have no P9 registration seam, so Pass 2 is the narrow
research analogue of the accepted P9/MAIN concepts. Evidence grade: `DISCOVERY_ROBUSTNESS_NOT_P9_PROMOTION_GRADE`.

## Frozen protocol (PASS2_PROTOCOL_ID `a712c301c2c527d8bc37b1ba7e72e6889c16a3ae0b2cd6e10f4dfbf2ee62db15`)

The protocol, cohorts and Pass-1 evidence hashes were committed before robustness attempt #1 (robustness attempts at freeze = 0).
The ID is pinned by test; any threshold change moves it and every candidate id.

| Gate | Rule |
|---|---|
| S1 | accepted `evaluate_cell` replay equals Pass-1 registry metrics (canonical JSON, exact); net stream reproduces totals exactly |
| S2 | `trade_count >= 30` |
| S3 | every commission and execution-slippage input ×2, same matched benchmark under the same stressed costs: net P&L > 0 **and** alpha > 0 |
| S4 | same at ×3 — diagnostic strength, not a gate |
| S5 | own decision stream delayed one completed session (causal fill unchanged), same benchmark/window: net > 0 and alpha > 0 |
| S6 | benchmark-relative alpha positive in ≥ 6 of the 8 calendar years 2016–2023 (absent year = not positive) |
| S7 | removing the best-alpha year (earliest on ties) leaves total alpha > 0 |
| S8 | Pass-1 regime concentration share ≤ 0.80 |
| S9 | adjacent-grid same-symbol neighbours that are Pass-1 MODERATE ≥ 25% (zero neighbours ⇒ NOT_APPLICABLE) |
| S10 | MAIN risk bar on the $100,000 initial capital: max drawdown ≤ 20% and worst rolling 5-session net P&L ≥ −6% |
| S11 | replication class (classification only) |
| C1 | factor/evaluation identity, observation hash, events, effect, empirical-null p, complete-family FDR q all reproduce Pass 1 |
| C2 | direction-adjusted effect positive in ≥ 6 of 8 calendar years (baseline recomputed within the year) |
| C3 | leave-best-year-out effect > 0 with ≥ 30 remaining events |
| C4 | top-symbol event share ≤ 0.50 and more than one represented symbol |
| C5 | every leave-one-symbol-out slice: effect > 0 and ≥ 30 remaining events (a thin slice FAILS, never dropped) |
| C6 | adjacent semantic-parameter neighbours at the same horizon that are Pass-1 STRONG ≥ 25% (none ⇒ NOT_APPLICABLE) |
| C7 / C8 | regime concentration and horizon-neighbourhood classification only |

Predeclared interpretation choices (no Pass-1 code implements them): the V4 MAIN risk bar exists only in
`docs/specs/strategy_evaluation_and_ranking.md`, so S10 uses additive net P&L over the evaluation window on the $100,000
capital (not the $10,000 budget). The conditional slice estimator `E(S)` recomputes the per-symbol baseline over the slice and
equals the accepted Pass-1 effect exactly on the full frame. A BLOCKED verdict requires that no applicable gate FAILed (a FAIL is
conclusive); none occurred.

## Result — StrategyEdge (789 / 789)

**0 survivors, 789 rejected, 0 blocked.** S1 baseline replay reproduced all 789 accepted metric sets exactly (no contradiction).

| Scenario | candidates failing |
|---|---|
| S2 trade floor (< 30 trades) | 769 |
| S6 positive years (< 6 of 8) | 753 |
| S8 regime concentration (> 0.80) | 527 |
| S7 leave-best-year-out | 511 |
| S10 MAIN risk bar | 276 |
| S9 neighbourhood | 147 (20 NOT_APPLICABLE) |
| S5 one-session delay | 112 |
| S3 ×2 cost stress | 78 |

Every candidate fails at least one gate (6 fail only S2; the modal profile is S2+S6+S7+S8). 638 of 789 would pass the ×3 cost diagnostic
and 743/789 are CLUSTER_REPLICATED (46 SYMBOL_SPECIFIC) — the apparent Pass-1 StrategyEdges are low-frequency, short-sample and
year-concentrated rather than cost-fragile. Full failure profiles and per-family/per-replication-class counts are in
`results/pass2_summary.json`; the complete denominator is `results/strategy_robustness_ledger.jsonl`.

## Result — ConditionalEdge (135 / 135)

**18 survivors, 117 rejected, 0 blocked.** C1 reproduced every accepted factor id, evaluation id, observation hash, event count,
effect, p and FDR q (no contradiction). Failures: C2 107, C3 59, C5 34, C6 34, C4 1.

Survivors by family: S06 12, S05 4, S13 2. By horizon: h20 8, h10 4, h1 3, h3 2, h5 1. Regime: 9 general / 9 regime-concentrated.
Horizon support: 11 supported / 7 isolated. Ranked read-only table (not selection authority; consumes no Confirmation data):
`results/survivor_rankings_readonly.json`. These are diagnostic relationships (`executable_pnl = false`) suitable as
hypotheses for a later Independent Confirmation stage, not strategies and not promotion candidates.

## Multiple testing

Strategy DSR/PBO stays `DEFERRED_FULL_POPULATION` (denominator 38,192; no narrowing). The accepted judge
(`mqk_research.ml.multiple_testing_judge`) loads each trial's succeeded attempt `result_id`, `artifact_paths["economic_walk_forward"]`
(schema `economic_walk_forward_v1`, protocol `economic_walk_forward_v1`, holdout `reserved_not_evaluated`) and its daily-returns CSV.
Census attempts store none of these (summary metrics only), so the judge cannot consume them without generating 38,192 new
walk-forward experiments, which this mission forbids. The conditional family keeps its accepted complete-family BH/FDR (α 0.10, 1,095).

## Immutability and fences

Pass-1 evidence (8 run files + 13 committed manifests, chunk root, attempt counts) is byte/hash/count-equal before and after
(`results/pass1_immutability_proof.json`): Strategy attempts 38,192 → 38,192, Conditional V3 attempts 1,095 → 1,095, new Pass-1 trials 0,
new Pass-1 factors 0. Confirmation rows read 0, final-holdout rows read 0 (latest input timestamp 2023-12-29); Pass-2 reads only the
frozen census bars (hash-verified against the bars manifest) and never calls an acquisition path.

## Process disclosures

* The Strategy engine was smoke-executed once in memory over the real cohort before the freeze commit (aggregate counts only; nothing
  persisted; no threshold or rule changed afterward — the protocol ID is pinned). The Conditional engine was not run on real data
  before the freeze. This is recorded in `PASS2_PREDECLARATION.json`.
* `simulate.py` gained default-preserving `commission_bps` / `slippage_bps` overrides (the narrow shared seam for stress runs); S1 proves
  Pass-1 economics are unchanged for all 789 candidates.

## Proof

Focused tests (`tests/test_alpha_edge_pass2_01.py`), mutation proof P01–P22 plus P04b/P05b and Q01–Q05 (each RED under the mutation,
source restored byte-for-byte, GREEN after restore — `results/mutation_proof_log.json`), and a second adversarial sweep
(`results/second_sweep.json`, NO_FINDINGS: every verdict re-derived from raw numbers with literal thresholds, brute-force grid
adjacency, 40 independent literal-cost re-simulations). Broad Rust workspace acceptance is delegated to GitHub CI.
