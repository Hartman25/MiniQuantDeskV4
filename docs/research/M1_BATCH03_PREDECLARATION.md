# M1 Batch 03 - prospective declaration (ten families x six ETFs)

Controller `V4-M1-SYSTEM-CLOSURE-CORRECTION-PLUS-BATCH03-PREPARATION-01`. Machine-readable authority: `research-py/experiments/m1_native_trend_campaign/PREDECLARED_BATCH_03.json` (frozen content; this document explains it and must not contradict it).

**Final status: `BATCH03_PREDECLARED_NOT_EXECUTED`** with `OPERATOR_DECISION_REQUIRED_BATCH03_STRESS_CONTRACT`. No trial is registered, no economic attempt exists (`ECONOMIC_ATTEMPTS = 0`), no scanner, judge or backtest ran, no family winner exists, no Promotion candidate exists, the final holdout is RESERVED / UNCONSUMED, Paper is inactive and Live is untouched.

## Why it stops

Promotion now requires a capital-fraction candidate's full P7A/P7B stress contract to be registered with its trial (`docs/M1_SYSTEM_CLOSURE_01.md`, IR-SYS-02), and trial identity binds it. Nothing in the repository authorizes a Batch 03 stress contract. The Batch 02 values (`half_exposure_capital_fraction_500bps_v1`, 500 bps, slippage 15, volatility 10, ceiling 0.4) were a Batch 02 predeclaration and are not authority here. Registering 60 trials without it would freeze identities that must then be re-registered, so the declaration is non-executable: every runner stage refuses (`require_executable_declaration`), `family_ranking.main()` refuses, and `stress_plan` / `research_stress_contract` exit.

**Smallest exact operator decision (all five values):**

| Field | Constraint |
|---|---|
| `scenario_id` | non-empty name |
| `allocation_fraction_bps` | integer, `1 <= x < 1000` |
| `stress_execution_slippage_bps` | number `>=` baseline 5 |
| `stress_execution_volatility_mult_bps` | number `>=` baseline 0; at least one execution parameter strictly worse than baseline |
| `max_drawdown_ceiling_bps` | integer |

When recorded, re-issue the declaration with `execution_gate.executable = true` and the stress block; then `register` (60 trials, 0 attempts) must precede every attempt.

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

**Restart recovery (deliberate, recorded).** F01, F05, F08, F10 are stateless month-end rules and are `BoundedHistoryReconstructible`. F02, F03, F04, F06, F07 and F09 carry state (a position or a hold counter) that a bounded window cannot reconstruct, so they are `NotRecoverable` and fail closed rather than guess after a restart. If one of them ever advances to Paper, durable state must be added first.

## Family ranking (`family_ranking.py`)

A pure function of the 60-row evidence table; it ranks families, never trials. A family needs at least 4 of 6 evaluable trials; a non-evaluable slot stays in the population and counts as worst on every key (never dropped, never imputed). A missing, duplicate or extra slot is an unresolved population, not a rejection. Keys, in order: (1) count of symbols with positive cost-aware benchmark alpha; (2) median DSR; (3) median net alpha vs the capital-fraction matched benchmark; (4) count of trials clearing robustness; (5) median drawdown improvement; (6) lowest family id. The top 3 receive `DISCOVERY_ADVANCED_TO_CONFIRMATION`, which is not Promotion; no individual symbol trial is selected and no Promotion candidate is created.

**Deferred gap (key 5).** The capital-fraction benchmark section carries no benchmark drawdown, so no drawdown improvement exists today. Every evaluable slot is therefore worst on key 5 until such evidence exists; key 5 only decides among families already tied on keys 1-4. No value is invented; adding the evidence is a separate patch that must precede any execution if the operator wants key 5 to discriminate.

## Confirmation (prepared, not registered)

Preferred universe DIA, MDY, XLF, XLI, XLV, XLP; 3 advanced families x 6 = 18 future trials, `PREPARED_NOT_REGISTERED`, history sufficiency unverified, symbol substitution not allowed. It needs its own predeclaration. Individual stocks stay excluded.

## Proof (focused; nothing economic ran)

- Registration gate and predeclaration: `test_batch03_predeclaration.py` (22), `test_batch03_registration_gate.py` (19: 60/0 passes; 59, 61, duplicates and an attempt before the gate are refused; `1D` and `1Day` give one identity; moving any fingerprint parameter moves identity).
- Engines: `scenario_batch03_native_strategies_01.rs` runs each engine against an independent integer reference at every bar of a deterministic fixture, checks stateless freshness, registry metadata, distinct fingerprints, and capital-fraction sizing identity. Its fixture stress value (500 bps) is a test constant, explicitly not the Batch 03 stress contract. Mutation proofs were taken per family (the F01..F10 edits listed in the controller) and restored byte for byte.
- Ranking and execution stop: `test_batch03_family_ranking.py` (24), including that the declaration is non-executable, attempts are 0, no `runs/run_batch_03` result artifact exists, and `main()` refuses. Ranking mutants (floor, key order, top-N, median, trial selection, imputation, id tie-break) are killed.
- The `mqk-daemon` crate was not compiled locally: its tests that count registered strategies were edited by inspection for the 24-entry universe. Broad workspace acceptance is delegated to GitHub CI.

## Second adversarial sweep (Batch 03 preparation)

Each item FIXED+PROVEN, ALREADY CORRECT+PROVEN, or BLOCKED: execution before the stress authority (BLOCKED by design; every stage and the ranking refuse; no result artifact exists); borrowed Batch 02 stress values (absent; asserted not present); trial-count drift 59/61/duplicates (refused); attempt before the 60/0 gate (refused); `1D` vs `1Day` second identity (one identity); fingerprint parameter omission (identity moves; proven per engine); lookahead (current-bar exclusion and same-bar fill proven by the independent integer reference and per-family mutations); holdout rows in discovery (truncated before 2026-03-01; holdout guard unchanged); fixed-quantity fallback or wrong benchmark (sizing/benchmark are declared, validated and fail closed); non-evaluable trials dropped from ranking (counted worst); trial selection or Promotion candidate from discovery (never created); result-dependent identity (no result enters an identity); new non-deterministic audit ids (`new_v4` absent from the diff); secrets (none in the diff). Not covered, deliberately: real-market results (none exist), daemon capital-fraction dispatch (separate authorized mission).
