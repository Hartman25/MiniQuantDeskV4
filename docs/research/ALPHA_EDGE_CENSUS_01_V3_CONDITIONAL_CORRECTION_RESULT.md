# Alpha Edge Census 01 — V3 ConditionalEdge correction (final independent-review correction)

Mission `V4-ALPHA-EDGE-CENSUS-01-FINAL-CONDITIONAL-CORRECTION-02`. Everything remains **DISCOVERED / NOT VALIDATED**
(`VALIDATION_STATUS=NOT_VALIDATED`, `PROMOTION_AUTHORITY=NONE`). Confirmation NOT RUN; final holdout RESERVED / UNCONSUMED;
Paper INACTIVE; Live DISABLED / UNTOUCHED; real Paper DB UNTOUCHED. This document supersedes the ConditionalEdge portion of
`ALPHA_EDGE_CENSUS_01_CORRECTED_RESULT.md`; its StrategyEdge portion is unchanged and re-proven below.

## Defects closed

**CR-01 — semantic factor duplication.** V2 carried the whole Strategy configuration into `FactorSpec` identity, so
Strategy-only execution parameters (`exit`, `exit_above`, `exit_z`, `hold`) minted extra conditional hypotheses
(434 x 5 = 2,170) although they never change the condition `sig.cond`. V3 projects each S01-S14 config onto its
condition-defining parameters (`search_space.CONDITION_PARAM_KEYS`): **219 semantic conditions** (S01 8, S02 6, S03 14,
S04 5, S05 36, S06 24, S07 36, S08 12, S09 32, S10 12, S11 6, S12 16, S13 8, S14 4) x horizons {1,3,5,10,20} =
**1,095 FactorSpecs**, registered before attempt #1. The projection is result-independent; the condition->source-config
mapping (`ALPHA_CENSUS_CONDITION_GRAMMAR_V3.json`) is traceability only and never enters an identity. The V2 population is
preserved unmodified and dispositioned `REJECTED_SEMANTIC_DUPLICATE_FACTOR_POPULATION` (`CONDITIONAL_V2_DISPOSITION.json`);
no V2 result chose a V3 parameter. Proof: on the real 88-symbol universe every one of the 38,192 (config x symbol) source
series equals its condition's `(first-defined bar, cond)` series — 0 mismatches, 150 multi-source conditions. Execution-only
parameters demonstrably change the Strategy signal `d` but never `cond`/`s` (non-vacuity guard in the tests).

**CR-02 — factor-level resume.** Resume now decides per exact `(factor_id, evaluation_id)` from registry truth:
succeeded / not_evaluable are reused (record file, else faithful reconstruction from the registered artifact with a content
hash proof, else fail closed — never a rerun); a stale `started` attempt is finalized `infrastructure_interrupted` and retried
as a new attempt of the same evaluation; `failed` (infrastructure) is retried; never-attempted factors get their first attempt;
an unregistered factor is refused (no auto-registration); a terminal attempt followed by a later failed one is an authority
conflict and refuses. Each horizon is persisted as soon as it is terminal. Cases A-E are tests.

**Resume-authority closeout (RESUME-AUTHORITY-CLOSEOUT-03) supersedes the retry clause above.** Only the exact typed reason
`infrastructure_interrupted` is automatically retryable; a `failed` attempt with any other reason (e.g. a runner exception
text) is refused and needs operator action. A `started` or retryable `failed` attempt is resumed only if its `evaluation_id`
equals the expected evaluation identity recomputed from the frozen inputs, otherwise `FactorResumeRefusal` before any state
change. `run-factors` runs under one non-blocking OS file lock (`factor_eval/run_factors.lock`) held for the whole invocation;
a second controller is refused. No factor, Strategy, FDR or Edge Registry state was re-executed or changed.

**IR-10 — tooling.** `ALPHA_EDGE_CENSUS_01_V3_TOOLING_DISPOSITION.md`: `mqk_readonly` USED; Srclight stale-index reverified and
skipped; Graft `CONNECT_TIMEOUT`; `mqd-test-proof` used.

## Frozen Strategy result — before / after

| Item | Before (pre-mission) | After |
|---|---|---|
| Strategy configs / trials / attempts | 434 / 38,192 / 38,192 (all succeeded, 0 failed) | identical; **0 Strategy attempts created** |
| `registry.sqlite` SHA-256 | `4286bad9d1bd7238…` | byte-identical |
| Raw chunk root (77 files) | `f66aa88998bc2aa0…` | byte-identical |
| Accepted search ledger | `a16be288f9f1bc3e…` | byte-identical (`search_ledger_v3` == `search_ledger_v2`) |
| Strategy classes | WEAK 2,851 / MODERATE 789 / STRONG 0 | identical; 3,640 Strategy edge records identical except `schema_version` |
| Judge | `DEFERRED_FULL_POPULATION` | unchanged |

## V3 factor-only freeze (committed before attempt #1)

`FACTOR_FREEZE_PROOF_V3.json`: scope FACTOR_ONLY; Strategy binding (population root, 38,192 terminal succeeded attempts,
chunk root, ledger hash); condition grammar id + mapping hash; 219 conditions; 1,095 registered factors in the separate V3
registry (`registry_conditional_v3.sqlite`, family `alpha_census_conditional_v3`); **V3 attempts at freeze = 0**; universe /
data-provenance / partition identities; V2 disposition. V2 factor ids, attempts and results are disjoint from, and cannot
satisfy, the V3 freeze, resume or FDR.

## V3 execution and results

| Item | Value |
|---|---|
| Factor attempts / retries / failed | 1,095 / 0 / 0 (one attempt per factor) |
| Statuses | succeeded 1,065; not_evaluable 30 (typed `evaluation_not_evaluable`, kept in the FDR denominator) |
| BH/FDR (alpha 0.10) | status `complete`; declared population 1,095; evaluable hypotheses 1,065; typed exclusions 30; **BH rejected null hypotheses (statistically significant factors): 342** |
| ConditionalEdge | WEAK 215, MODERATE 72, STRONG 135 (422 records; by horizon h1 125, h3 85, h5 109, h10 54, h20 49) |
| Combined Edge Registry V3 | 4,062 records = 3,640 StrategyEdge (2,851/789/0) + 422 ConditionalEdge |
| Idempotency on real data | a second `run-factors` found 0 pending conditions and created 0 attempts |

The V2 conditional counts (371/112/265 over 2,170 duplicated factors) are historical and non-authoritative.

## Second adversarial sweep (independent recomputation)

BH recomputed independently from the raw p-values (342 rejected, max |q diff| 0.0); ConditionalEdge classes recomputed from
the factor ledger (215/72/135) match the registry exactly; every identity scanned (no `exit`/`hold`/`exit_z`/`exit_above`/
`config_id`; 1 factor per semantic coordinate); one attempt per factor and no open `started` attempts; Strategy registry,
chunks, ledgers and V2 evidence byte-identical to the pre-mission snapshot. No new defect found.

## Limits

Universe is a current-registry snapshot (survivorship caveat on every record). Conditional labels are diagnostic forward
returns, not P&L. STRONG StrategyEdge needs the deferred full-population DSR/PBO judge. Nothing here authorizes promotion.
