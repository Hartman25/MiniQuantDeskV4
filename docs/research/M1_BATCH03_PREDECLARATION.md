# M1 Batch 03 - prospective declaration (ten families x six ETFs)

Controller `V4-M1-SYSTEM-CLOSURE-CORRECTION-PLUS-BATCH03-PREPARATION-01`. Machine-readable authority: `research-py/experiments/m1_native_trend_campaign/PREDECLARED_BATCH_03.json` (frozen content; this document explains it and must not contradict it).

**Final status: `BATCH03_PREDECLARED_NOT_EXECUTED`** with blocker `OPERATOR_EXECUTION_AUTHORIZATION_REQUIRED_BATCH03` (the stress contract is frozen; execution still needs separate explicit operator authority). No trial is registered, no economic attempt exists (`ECONOMIC_ATTEMPTS = 0`), no scanner, judge or backtest ran, no family winner exists, no Promotion candidate exists, the final holdout is RESERVED / UNCONSUMED, Paper is inactive and Live is untouched.

## Why it stops

The declaration stays non-executable (`execution_gate.executable = false`): every runner stage refuses (`require_executable_declaration`) and `family_ranking.main()` refuses until the operator re-issues it as executable. Registering 60 trials and running attempt #1 are separate authorizations; none happened.

## Frozen stress contract (operator decision, recorded 2026-10-04)

Promotion requires a capital-fraction candidate's full P7A/P7B stress contract to be registered with its trial (`docs/M1_SYSTEM_CLOSURE_01.md`, IR-SYS-02), and trial identity binds it. The contract is bound through that authority (`research_stress_contract` -> trial identity; `stress_plan` validates it):

| Field | Value |
|---|---|
| `scenario_id` | `half_exposure_capital_fraction_500bps_v1` |
| `allocation_fraction_bps` | 500 (baseline stays `FixedInitialCapitalFractionV1` 1000 bps on USD 100,000 immutable capital, USD 10,000 budget) |
| `stress_execution_slippage_bps` | 15 (baseline 5) |
| `stress_execution_volatility_mult_bps` | 10 (baseline 0) |
| `max_drawdown_ceiling_bps` | 4000 |

The 500 bps stress is mechanically half exposure: Q is independently re-resolved from the same causal completed-bar entry price. It is not a USD 5,000 cap on the 1000 bps quantity, not a Production default, and not a new trial or parameter search. Baseline execution (5 / 0) versus stress (15 / 10): both parameters are at least as adverse and both are strictly worse, so there is no baseline contradiction; the Rust replay additionally refuses any stress that is not genuinely adverse.

## Frozen scope

- Experiment `M1-NATIVE-HYPOTHESIS-BATCH-03-DISCOVERY`; universe, in order: SPY, QQQ, IWM, SMH, XBI, XLE. ETFs only; no individual stocks.
- Data: Alpaca SIP, `1Day` transport with the canonical daily identity (`data.timeframe_identity = "canonical_semantic_v1"`, so `1D` and `1Day` are one identity), adjustment `all`, completed bars only, window 2016-01-01..2026-09-01. Discovery bars are truncated before 2026-03-01; the final six months are reserved and unconsumed.
- Sizing: `fixed_initial_capital_fraction_v1`, 1000 bps of an immutable USD 100,000 (100000000000 micros) = USD 10,000 nominal budget; quantity = floor(budget / completed signal-bar close), whole shares, no fallback, no compounding, no family-specific sizing. Benchmark: `capital_fraction_matched_passive_buy_hold_v1`.
- Population: 10 families x 6 symbols = 60 trials, registered family-major (F01-SPY .. F01-XLE, F02-SPY, ...), one experiment, one batch-wide multiple-testing population judged once by the canonical judge. Retries are new attempts of the same trial and never add trials. 59 and 61 are refused; an attempt before the 60/0 gate is refused.
- Variants: none (no RSI2/3/7, alternate thresholds, Donchian variants, hold lengths, ATR multipliers, MA grids or stops).

## Families (long/flat, strict inequalities, no same-bar fills)

| ID | Strategy id | Rule | Bars |
|---|---|---|---|
| F01 | `monthly_multihorizon_abs_momentum_consensus_v1` | month-end: +1 iff at least 2 of 3 (close > close 21/63/252 sessions earlier) | 275 |
| F02 | `trend_filtered_rsi5_reversion_v1` | close > SMA200; enter RSI5 < 30, exit RSI5 > 70 or trend lost | 200 |
| F03 | `trend_filtered_extreme_3d_atr_reversal_v1` | trend; 3-day drop fraction > 1.5 x prior-20 ATR fraction (event bar excluded from ATR); hold exactly 5 outputs | 200 |
| F04 | `close_channel_100_50_trend_v1` | enter close > prior-100 high close; exit close < prior-50 low close (current bar excluded) | 101 |
| F05 | `monthly_10month_trend_timing_v1` | month-end: +1 iff close > mean of the prior 10 month-end closes | 253 |
| F06 | `trend_filtered_zscore20_reversion_v1` | trend; z (population std, prior 20) < -2 enter; exit close >= mean20 or trend lost | 200 |
| F07 | `volatility_contraction_breakout_v1` | range10 x 4 < range60 and close > prior-20 high close; exit close < prior-10 low close | 61 |
| F08 | `monthly_12_minus_1_abs_momentum_v1` | month-end: +1 iff close[t-21] > close[t-252] (current close never read) | 275 |
| F09 | `delayed_overnight_gap_reversal_v1` | gap fraction > 1.5 x prior-20 ATR fraction, signalled only after the bar completes; hold exactly 3 outputs | 22 |
| F10 | `monthly_52week_high_proximity_v1` | month-end: +1 iff close x 100 >= 252-session high x 95 | 274 |

Engines are exact integer (i128) arithmetic in `mqk-strategy::engines`, registered in `REGISTERED_STRATEGY_IDS`; `MAX_STRATEGY_UNIVERSE` was raised to 24 for them.

**Restart recovery (recorded).** F01, F05, F08, F10 are stateless month-end rules and are `BoundedHistoryReconstructible`. F02, F03, F04, F06, F07 and F09 carry state (a position or a hold counter) that a bounded window cannot reconstruct, so they are `DurableStateRequired`: the production seam (`instantiate_verified`) refuses them unless the held-sizing record is supplied, and `CapitalFractionSizedStrategy::new_recoverable` then seeds the engine through `Strategy::restore_held_positions` from the Active record's entry anchor (no new store, no migration). F03/F09 derive the hold counter from that anchor and fail closed to flat when it is not provable from the window. Per-engine tests prove the continuous target stream equals the restart-at-every-boundary stream and kill reset-to-flat and reset-hold-counter mutants.

## Family ranking (`family_ranking.py`)

A pure function of the 60-row evidence table; it ranks families, never trials. A family needs at least 4 of 6 evaluable trials; a non-evaluable slot stays in the population and counts as worst on every key (never dropped, never imputed). A missing, duplicate or extra slot is an unresolved population, not a rejection. Keys, in order: (1) count of symbols with positive cost-aware benchmark alpha; (2) median DSR; (3) median net alpha vs the capital-fraction matched benchmark; (4) count of trials clearing robustness; (5) median drawdown improvement; (6) lowest family id. The top 3 receive `DISCOVERY_ADVANCED_TO_CONFIRMATION`, which is not Promotion; no individual symbol trial is selected and no Promotion candidate is created.

**Key 5 evidence.** Median drawdown improvement versus the matched passive benchmark is read only from the engine-computed benchmark evidence (`candidate_max_drawdown_pct`, `benchmark_max_drawdown_pct`, `drawdown_improvement_pct` on `ScanCapitalFractionBenchmarkEvidence`): both drawdowns come from the equity curves of one verified benchmark run identity, in percent of peak equity (peak seeded at initial capital); improvement = benchmark minus candidate, so positive means the candidate drew down less. `verify_internal` refuses partial, non-finite, out-of-range or inconsistent triples; absent evidence stays worst on key 5 only. Row-level caller numbers are never read, and none of it enters trial identity.

## Confirmation (prepared, not registered)

Preferred universe DIA, MDY, XLF, XLI, XLV, XLP; 3 advanced families x 6 = 18 future trials, `PREPARED_NOT_REGISTERED`, history sufficiency unverified, symbol substitution not allowed. It needs its own predeclaration. Individual stocks stay excluded.

## Proof (focused; nothing economic ran)

- Registration gate and predeclaration: `test_batch03_predeclaration.py` (24), `test_batch03_registration_gate.py` (19: 60/0 passes; 59, 61, duplicates and an attempt before the gate are refused; `1D` and `1Day` give one identity; moving any fingerprint parameter moves identity).
- Engines: `scenario_batch03_native_strategies_01.rs` runs each engine against an independent integer reference at every bar of a deterministic fixture, checks stateless freshness, registry metadata, distinct fingerprints, and capital-fraction sizing identity. Its fixture stress value (500 bps) is a test constant, explicitly not the Batch 03 stress contract. Mutation proofs were taken per family (the F01..F10 edits listed in the controller) and restored byte for byte.
- Ranking and execution stop: `test_batch03_family_ranking.py` (36), including that the declaration is non-executable, attempts are 0, no `runs/run_batch_03` result artifact exists, and `main()` refuses. Ranking mutants (floor, key order, top-N, median, trial selection, imputation, id tie-break) are killed.
- The `mqk-daemon` crate was not compiled locally: its tests that count registered strategies were edited by inspection for the 24-entry universe. Broad workspace acceptance is delegated to GitHub CI.

## Second adversarial sweep (Batch 03 preparation)

Each item FIXED+PROVEN, ALREADY CORRECT+PROVEN, or BLOCKED: execution (BLOCKED by design pending separate operator execution authority; every stage and the ranking refuse; no result artifact exists); stress contract differing from registered authority (frozen in the declaration, validated by `stress_plan`, bound into all 60 identities; a changed slippage or volatility moves every id; 15 / 10 is strictly more adverse than baseline 5 / 0); trial-count drift 59/61/duplicates (refused); attempt before the 60/0 gate (refused); `1D` vs `1Day` second identity (one identity); fingerprint parameter omission (identity moves; proven per engine); lookahead (current-bar exclusion and same-bar fill proven by the independent integer reference and per-family mutations); holdout rows in discovery (truncated before 2026-03-01; holdout guard unchanged); fixed-quantity fallback or wrong benchmark (sizing/benchmark are declared, validated and fail closed); non-evaluable trials dropped from ranking (counted worst); trial selection or Promotion candidate from discovery (never created); result-dependent identity (no result enters an identity); new non-deterministic audit ids (`new_v4` absent from the diff); secrets (none in the diff). Not covered, deliberately: real-market results (none exist), daemon capital-fraction dispatch (separate authorized mission).
