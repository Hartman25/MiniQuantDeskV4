# Alpha Edge Census 01 — Rejected Run Disposition and Correction Defect Census

Mission: V4-ALPHA-EDGE-CENSUS-01-INDEPENDENT-REVIEW-CORRECTION-01.

## Disposition

`ALPHA_EDGE_CENSUS_01_REJECTED_EXECUTION_20261005` — reason `CONTRACT_DIVERGENCE`
(`INDEPENDENT_REVIEW_REJECTED_CONTRACT_DIVERGENCE`). The execution is **not the authoritative result**.
Its commits (start of correction: `b37ede31`) stay in history; its run directory
(`research-py/runs/alpha_edge_census_01/`, git-ignored) is preserved untouched; its frozen manifests moved to
`research-py/experiments/alpha_edge_census_01/rejected_run_20261005/`.

Rejected values are historical exploratory only. They must not select or alter corrected candidates, grids, thresholds,
symbols or factor grammar, and rejected-run population/attempts can never satisfy a corrected gate.
The corrected experiment uses distinct deterministic identity `alpha_edge_census_01_corrected`
(trial prefix `ace2-`, run directory `research-py/runs/alpha_edge_census_01_corrected/`).

### Rejected-run reconciliation (never mixed with corrected counts)

| Item | Rejected-run value |
|---|---|
| Ending HEAD | `b37ede31e4dcf69a76c9c3e2edc5528b4dbd5eee` |
| StrategyEdge cells executed | 437,128 |
| Derived ConditionalEdge queries | 2,622,768 |
| StrategyEdge registry records | 43,401 |
| ConditionalEdge registry records | 1,104,921 |
| Non-positive net P&L among the 43,401 | 29,409 |
| < 5 trades among the 43,401 | 24,349 |
| Failed >= 1 corrected gate | 34,729 |
| ConditionalEdge observations with n < 30 | 299,299 |

### Defects that caused the rejection

1. Unauthorized 2024 discovery use (discovery ran to 2025-01-01 exclusive; 2024 labeled an unread reserve).
2. Unauthorized S01–S20 grammar (5,086 configs) instead of the frozen S01–S14 family set.
3. Unauthorized 2-day conditional horizon.
4. ConditionalEdge bypassed the factor registry (no FactorSpec, no durable evaluation attempt).
5. Wrong StrategyEdge positive predicate (`net_alpha > 0` rather than net strategy P&L > 0 plus floors).
6. Unenforced n >= 30 conditional event floor (tiny sample was only a flag).
7. No factor-family FDR taxonomy / no full-population multiple-testing accounting.

## Corrected authority (this correction)

| Item | Corrected value |
|---|---|
| Discovery | [2016-01-01, 2024-01-01) |
| 2024 | `CONTAMINATED_BY_REJECTED_RUN` — never an unread reserve |
| Remaining confirmation reserve | [2025-01-01, 2026-03-01), `RESERVED_UNCONSUMED` |
| Final holdout | >= 2026-03-01, `RESERVED_UNCONSUMED` |
| StrategyEdge grammar | exactly S01..S14, all symbol-scope, **434** configurations |
| ConditionalEdge horizons | {1, 3, 5, 10, 20} |
| Seed universe | frozen 88-symbol current-registry snapshot (`CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME`) |
| Eligibility | >= 252 valid completed 1Day observations strictly before 2024-01-01, accepted provenance, supported semantics; one typed `ELIGIBLE` / `EXCLUDED_*` disposition per seed symbol |
| StrategyEdge trial count | derived: 434 x corrected eligible-symbol count (never hard-coded; 38,192 only if all 88 are eligible) |

Per-family configuration counts: S01 8, S02 6, S03 14, S04 12, S05 72, S06 24, S07 108, S08 36, S09 64, S10 12, S11 18,
S12 32, S13 24, S14 4. The generator refuses (before any attempt) a family set other than S01..S14 or a count other than 434.

## Defect census (pre-edit, covering the review scope)

Status key: FIXED+PROVEN, ALREADY CORRECT+PROVEN, PENDING(commit) — resolved by a later correction commit in this controller.

| Area | Finding | Status |
|---|---|---|
| Partitions | discovery end 2025-01-01; 2024 labeled unread reserve | FIXED+PROVEN (C1, IR1/IR23) |
| Partitions | calendar/session grid and month positions extended through 2024 | FIXED+PROVEN (C1) |
| Grammar | S01–S20, S02 universe scope, S15–S20, ML feature sweeps, extra sma100 trend | FIXED+PROVEN (C1, IR2–IR5) |
| Grammar | 2-day horizon | FIXED+PROVEN (C1, IR6) |
| Eligibility | `DATA_PRESENT` used as disposition | FIXED+PROVEN (C1, IR21/IR22) |
| Provenance | request contract end 2025-01-01 | FIXED+PROVEN (C1); re-acquisition in C5 |
| Indicators | RSI/z-score conventions undocumented | FIXED+PROVEN (C1: Cutler RSI, ddof=1 z-score, hand-checked) |
| Indicators | S05 exit comparison is False (no exit) in a flat NaN-RSI window | ALREADY CORRECT (documented fail-closed: no fabricated exit) |
| StrategyEdge qualification | positive predicate and missing floors | FIXED+PROVEN (IR10-IR15; WEAK/MODERATE/STRONG ladder, STRONG=0 while judge deferred) |
| ConditionalEdge | factor-registry bypass (query tuples were not registered candidates) | FIXED+PROVEN (C2: every config x horizon {1,3,5,10,20} is a registered FactorSpec with zero attempts before any evaluation; evaluation via the repo factor runner; exact-parity cached null for the repo 200-permutation protocol; population gate covers trials + factors) |
| ConditionalEdge | n >= 30 floor, FDR taxonomy | FIXED+PROVEN (IR16-IR20; floor, p<=0.10, complete-family BH q<=0.10) |
| Edge Registry | wrong recording rules, partition label | FIXED+PROVEN (V2: one class per record, NOT_VALIDATED/NONE, full ledgers, IR27/IR28) |
| Bulk store / resume | chunk/resume determinism | ALREADY CORRECT+PROVEN (bulk-store and resume tests retained) |
| Mutation proof | M01/M02/M13/M14 tied to old fences | FIXED+PROVEN: 27 mutations (C01-C18 + retained M03,M04,M06-M09,M12,M15,M16), each RED under mutation and restored byte-for-byte; one initially survived (C10, self-referential label test) and was closed by a literal-label test |
| Population freeze | corrected eligible universe, population, factor registration before attempt #1 | FIXED+PROVEN: 88/88 seed symbols ELIGIBLE (typed disposition each), 434 configs x 88 = 38,192 trials, 2,170 registered FactorSpecs, 0 attempts, max input ts 2023-12-29; POPULATION_FREEZE_PROOF_V2.json committed before corrected attempt #1 |

Between C1 and C5 the not-yet-rewritten modules (`census.py`, `conditional.py`, `edge_registry.py`, `run_census.py`,
`mutation_proof.py`) still target the rejected contract and are not imported by anything else; no corrected attempt
exists until they are rewritten and the population freeze is committed.
