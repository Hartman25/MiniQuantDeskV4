# Alpha Edge Census 01 — Pass 1 Result

Mission: V4-ALPHA-EDGE-CENSUS-01. Label for every observation: **DISCOVERED / NOT VALIDATED**.
Every registry record carries `VALIDATION_STATUS=NOT_VALIDATED`, `PROMOTION_AUTHORITY=NONE`.
`JUDGE_STATUS = DEFERRED_FULL_POPULATION` (multiple-testing judge not run; the full search denominator is preserved).
Pass 1 is a recall-oriented census, not a ranking, selection or promotion step.

Code: `research-py/experiments/alpha_edge_census_01/`. Driver: `run_census.py` (`bars-manifest`, `freeze`, `run`).
Tests: `research-py/tests/test_alpha_edge_census_01.py`. Mutation harness: `mutation_proof.py`.
Run artifacts (git-ignored): `research-py/runs/alpha_edge_census_01/`.

## Universe and data

- Universe: 88 expected, 88 actual, all `DATA_PRESENT`; 0 data-unavailable.
- `universe_id` `56ec09754b78a3b9f79ad94a594fd3c0`; label `current_enabled_equity_registry_snapshot_v1`.
- Survivorship: `point_in_time_membership=false`, `CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME`. Every edge carries `survivorship_caveat`.
- Data: provider alpaca, feed SIP (never IEX; no fallback), adjustment all, timeframe 1Day, request window 2016-01-01..2025-01-01 exclusive.
- Observed bar range: 2016-01-04..2024-12-31 (max `last_end_ts` over all 88 symbols is 2024-12-31).
- Short-history symbols (flagged `data_short_history`): ACHR, AFRM, BITO, CHPT, DKNG, GDXJ, HIMS, HOOD, IONQ, JOBY, LCID, LYFT, MARA, OPEN, PLTR, RBLX, RIVN, RKLB, SOFI, UPST.
- Pass-1 discovery ceiling 2024-12-31. Confirmation reserve 2025-01-01..2026-02-27: UNCONSUMED. Final holdout 2026-03-01+: RESERVED / UNCONSUMED.

## Frozen identities

| Item | Value |
|---|---|
| search_space_id | `5ca25a2a965c8056c754760e4ded2473` |
| protocol_id | `95138cef46439fecef33edbf9e6f762b` |
| universe_id | `56ec09754b78a3b9f79ad94a594fd3c0` |
| partitions_id | `73a8c4abb9431c3ae36d5d182cf8dd00` |
| population_root_sha256 | `b7fe591be4d24881834aee8291a892af580905cb6c0e14d395a283e30863c25c` |

Economics (frozen): commission 10 bps/side; `rust_conservative_bar_range_v1` slippage 5 bps (BUY at high plus slip, SELL at low minus slip);
qty = floor(USD 10,000 / signal-bar close); capital USD 100,000, no compounding; signal on bar t fills on bar t+1;
open positions marked to last close; net alpha = strategy net minus capital-matched buy-and-hold net (USD).
Conditional edges are non-executable forward-return labels (`executable_pnl=false`).

## Search space and execution

- Families/templates: 20 (S01..S20). Parameter configurations: 5,086.
- StrategyEdge search cells registered: 437,128. ConditionalEdge queries expected: 2,622,768 (horizons 1,2,3,5,10,20).
- Registration: full population registered and freeze-marked before attempt #1; gate requires registered == expected and attempts == 0. Frozen manifests were committed before any attempt ran.
- Attempts (store digest): 437,128 trials, 437,128 attempts (1 each), 437,128 succeeded, 0 failed, 0 started. Retries: 0.
- Execution: 875 chunks of 500 cells, all terminal, one uninterrupted run (4,354 s). Chunk/resume behavior is covered by tests; no resume occurred in the real run.

## Results

| Metric | Count |
|---|---|
| Cells evaluable | 436,728 |
| Non-evaluable (`NON_EVALUABLE_SIGNAL_UNDEFINED_INSUFFICIENT_HISTORY`; S18 216, S19 184) | 400 |
| Positive net-alpha StrategyEdges | 43,401 |
| Zero net-alpha cells | 11,633 |
| Negative net-alpha cells | 381,694 |
| Positive ConditionalEdges | 1,104,921 |
| Search ledger lines (full denominator) | 437,128 |
| Parameter-island edges (flagged, not deleted) | 22,819 (strategy 2,181; conditional 20,638) |

Positive ConditionalEdges by horizon: h1 191,614; h2 185,884; h3 180,441; h5 190,445; h10 175,665; h20 180,872.

Descriptive extremes of positive StrategyEdge net alpha (not a ranking, not selection): smallest recorded positive USD 0.09305054000287782; largest USD 837,284.6495754201 (S18, MARA — short-history symbol).

Replication classes: StrategyEdges CLUSTER_REPLICATED 43,143, SYMBOL_SPECIFIC 258 (broadly replicated: 0).
ConditionalEdges BROADLY_REPLICATED 428,367, CLUSTER_REPLICATED 675,849, SYMBOL_SPECIFIC 48, NOT_APPLICABLE_UNIVERSE_SCOPE 657.

StrategyEdge flags: tiny_sample 42,679; large_drawdown 25,565; regime_concentration 32,373; single_year_concentration 33,511; data_short_history 22,723; cost_fragile 13,944; data_quality_caveat 5,812; parameter_island 2,181; single_symbol 258; survivorship_caveat 43,401; weak_dsr:DEFERRED_FULL_POPULATION 43,401.
Full flag counts for conditional edges are in `edge_registry_summary_v1.json`.

Per-family (cells / evaluable / strategy edges / conditional edges):
S01 11880/11880/1509/32037; S02 120/120/0/657; S03 33000/33000/2108/93265; S04 15840/15840/2255/42912; S05 5280/5280/923/13480;
S06 1144/1144/198/2679; S07 6600/6600/898/16215; S08 3168/3168/380/8182; S09 105600/105600/8944/285570; S10 21120/21120/3511/35008;
S11 28512/28512/3564/63261; S12 10560/10560/718/30357; S13 11264/11264/322/31660; S14 2992/2992/211/9992; S15 9504/9504/924/23620;
S16 16896/16896/1341/43312; S17 4224/4224/490/11992; S18 91872/91656/8007/263298; S19 51040/50856/6570/79698; S20 6512/6512/528/17726.

Interpretation caveat: with 437,128 strategy cells and 381,694 negative / 11,633 zero cells, a large positive count is expected by chance and from a survivorship-biased current-registry universe. The tiny_sample flag covers 42,679 of 43,401 StrategyEdges. None of these are validated; the deferred full-population judge, the confirmation reserve and the final holdout are the only authorized next gates.

## Artifact hashes (campaign_evidence_v1.json)

| File | sha256 |
|---|---|
| ALPHA_CENSUS_BARS_MANIFEST_V1.json | `6c01fb084f88d2916d7ec9b17da7c4505f9e3c2b2ef7e628e885a7aef5766857` |
| ALPHA_CENSUS_PARTITIONS_V1.json | `0e1a021f1831250e6d595105053c3c616b102ad1eab77d2c04f0103a93ead696` |
| ALPHA_CENSUS_POPULATION_V1.json | `d0b41d50d947332a48b2c22f916f929c0b025dbf4cf2c49369e85689afbe4f24` |
| ALPHA_CENSUS_PROTOCOL_V1.json | `5a7551e9e5bb2fd03103ae6752647963d70483d7752477a60b5ddb5b6e60b9c3` |
| ALPHA_CENSUS_SEARCH_SPACE_V1.json | `74822a3b1fdaf34f3610986537aa7339c1661ca5bc4d1802cb8b987e3bfc2010` |
| ALPHA_CENSUS_UNIVERSE_V1.json | `a5b52c9feddfcfb75a7583840ac64b31c87ab49bedd234259b643949769c88cb` |
| edge_registry_summary_v1.json | `75a607c7eb59b9ca92a2792aee259946eb199c2abd4994f077c99092cba2f024` |
| edge_registry_v1.jsonl | `e6708ad329f03b0ae38e629e0c5118ccde07944093af1f3690dbcbc04bd2b561` |
| search_ledger_v1.jsonl | `56d34e69be00ed49895255a05440a85c0e1daf11b87d7dd0c84f81a4e02b6da8` |

## Defect census / disposition

| Finding | Disposition | Proof |
|---|---|---|
| Missing/extra/duplicate registered cell or missing freeze marker before attempt #1 | FIXED + PROVEN | gate test; M03, M04 |
| Post-2024-12-31 or 2026-03-01+ rows reachable | FIXED + PROVEN | partition-fence test; M01, M02 |
| Result-dependent trial/edge identity | FIXED + PROVEN | identity tests; M05, M07 |
| Parameter order manufacturing duplicate candidates | FIXED + PROVEN | M06 |
| Same-bar execution / `fwd_ret` as executable P&L | FIXED + PROVEN | scalar-reference and fill-timing tests; M08, M09, M18 |
| Winner-only ledger / negative results vanishing | FIXED + PROVEN | full-denominator test; M10 |
| Failed symbol dropped from the 88 | FIXED + PROVEN | M11 |
| Point-in-time membership claimed without evidence | FIXED + PROVEN | M12 |
| Classifier receiving extra features (S19) | FIXED + PROVEN | M13 |
| Feature seeing its future threshold distribution (S18 quantiles, S19 fits) | FIXED + PROVEN | fold-isolation test; M14 |
| Retry manufacturing a new trial | FIXED + PROVEN | M15 |
| Silent SIP-to-IEX fallback | FIXED + PROVEN | M16 |
| Parameter island deleted instead of flagged | FIXED + PROVEN | M17 |
| Chunk boundary changing economics; resume rerunning successes | ALREADY CORRECT + PROVEN | interrupted-chunk and resume tests |
| Transport/layout manufacturing candidates | ALREADY CORRECT + PROVEN | identity depends only on canonical content |
| Unsupported corporate action silently ignored | ALREADY CORRECT + PROVEN | adjustment=all bound per symbol in bars manifest; load refuses hash drift |
| Tiny or one-trade positive result presented as strong | ALREADY CORRECT + PROVEN | recorded with `tiny_sample`; no DSR-based drop |
| Discovery result causing Promotion / Paper / Live activation | ALREADY CORRECT + PROVEN | no Promotion, deployment, order or runtime code touched; authority fields fixed to NONE |

## Mutation proof

M01..M18 (see `mutation_proof.py`): all 18 RED under mutation, all 18 restored byte-for-byte (`runs/alpha_edge_census_01/mutation_proof_log.json`).

## Status

Confirmation: NOT RUN. Promotion: NONE. Paper: INACTIVE. Live: DISABLED / NOT TOUCHED.

Full local workspace acceptance: NOT RUN — prohibited by laptop resource-safety rule; broad workspace proof delegated to GitHub CI.
