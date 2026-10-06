# Alpha Edge Census 01 (corrected) — Pass 1 Result

Mission: V4-ALPHA-EDGE-CENSUS-01-INDEPENDENT-REVIEW-CORRECTION-01. Experiment id `alpha_edge_census_01_corrected`.
Every observation is **DISCOVERED / NOT VALIDATED**; every registry record carries `VALIDATION_STATUS=NOT_VALIDATED`,
`PROMOTION_AUTHORITY=NONE`. This document supersedes `ALPHA_EDGE_CENSUS_01_RESULT.md`, which describes the rejected
execution only. Counts of the two executions are never mixed.

Authoritative machine-readable evidence: `research-py/experiments/alpha_edge_census_01/CAMPAIGN_EVIDENCE_V2.json`
(output SHA-256s) and `POPULATION_FREEZE_PROOF_V2.json`. Large artifacts are git-ignored under
`research-py/runs/alpha_edge_census_01_corrected/` and shipped in the review ZIP.

## Frozen contract

- Partitions: Discovery `[2016-01-01, 2024-01-01)`; `[2024-01-01, 2025-01-01)` CONTAMINATED_BY_REJECTED_RUN;
  REMAINING_CONFIRMATION_RESERVE `[2025-01-01, 2026-03-01)`; FINAL_HOLDOUT `>= 2026-03-01` reserved, unconsumed.
  Max economic input timestamp: 2023-12-29T05:00:00Z (re-verified from the raw bar files: 155,088 rows).
- Universe: 88-symbol current-registry snapshot (not point-in-time); 88 ELIGIBLE, 0 excluded.
- StrategyEdge grammar: S01..S14, **434 configurations**; trials = 434 x 88 = 38,192 (derived, not hard-coded).
- ConditionalEdge: 434 configs x horizons {1,3,5,10,20} = 2,170 registered FactorSpecs, all registered before any
  evaluation attempt; label is a future h-session return (diagnostic only, never P&L).
- Costs 5 bps slippage + 10 bps commission per side; capital-matched buy-and-hold benchmark; next-bar fills.

## Execution

| Item | Value |
|---|---|
| StrategyEdge trials / attempts / failed attempts | 38,192 / 38,192 / 0 |
| Factor evaluations / attempts / failed attempts | 2,170 / 2,170 / 0 |
| Factor statuses | succeeded 2,120; not_evaluable 50 (S13 30, S14 20; `zero_variance_factor`, kept in the FDR denominator) |
| Factor FDR | BH, alpha 0.10, status `complete`, declared population 2,170, hypotheses 2,120, FDR-rejected 757 |
| StrategyEdge judge | `DEFERRED_FULL_POPULATION` (full-population DSR/PBO not run) -> STRONG StrategyEdge = 0 |

## Edge Registry V2 (4,388 records; ledgers are complete, losers stay in the denominator)

| Edge type | WEAK | MODERATE | STRONG |
|---|---|---|---|
| StrategyEdge (of 38,192) | 2,851 | 789 | 0 |
| ConditionalEdge (of 2,170) | 371 | 112 | 265 |

POSITIVE_BUT_BELOW_CENSUS_FLOOR (not registry records, reported separately): StrategyEdge 4,780; ConditionalEdge 0.
Of 38,192 StrategyEdge cells, 33,215 had non-positive net P&L and 1,337 had fewer than 5 closed round trips.
The StrategyEdge WEAK/MODERATE sets and the ConditionalEdge class assignment were independently recomputed from the raw
chunk files / factor ledger and match the registry exactly. Registry rebuild is byte-deterministic (identical SHA-256).

## Rejected-run reconciliation (historical, NOT authoritative)

437,128 cells; 2,622,768 derived conditional queries; 43,401 old StrategyEdges; 1,104,921 old conditionals. The
rejected execution used a 5,086-config S01..S20 grammar, a 2025-01-01 discovery fence, and unregistered conditional
queries. None of its attempts can satisfy a corrected gate (`ss.REJECTED_EXPERIMENT_ID` is not read by the gate).

## Not done / limits

Confirmation NOT RUN. Promotion NONE. Paper INACTIVE. Live DISABLED/UNTOUCHED. No holdout consumed. Universe is a
current snapshot (survivorship caveat flagged on records). STRONG StrategyEdge needs the deferred full-population
DSR/PBO judge. The cached permutation null is exact-parity tested against the native protocol (documented 1e-12 tie
tolerance).
