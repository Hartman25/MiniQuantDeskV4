# FixedInitialCapitalFractionV1 — native sizing contract

Controllers: `V4-M1-NATIVE-SIZING-V1-01` and its independent-review correction
`V4-M1-NATIVE-SIZING-V1-INDEPENDENT-REVIEW-CORRECTION-01`. Infrastructure, semantics and proof only: no campaign, no
Batch 02, no holdout consumption, no promotion, no Paper deployment, no Live.

The infrastructure never defaults a fraction: the policy requires an explicit `allocation_fraction_bps` everywhere.
The operator has frozen the sizing of the NEXT M1 hypothesis campaign (see "Operator-approved future Batch 02 sizing").

## Policies

| Policy id | Meaning |
|---|---|
| `fixed_quantity_v1` (default) | Historical `StrategySizingConfig` (target_qty / max_target_qty / max_position_notional_usd). Unchanged; all historical config ids, run ids, fingerprints and artifacts are preserved. |
| `fixed_initial_capital_fraction_v1` | Entry quantity resolved from a fixed fraction of the immutable initial allocated capital. Selected only explicitly. |

## FixedInitialCapitalFractionV1

- `allocation_fraction_bps`: explicit integer, `1..=10_000`. Missing, 0, above 10_000 or malformed is refused. No default, no floats.
- Capital basis: the immutable INITIAL allocated strategy capital. Never current equity, P&L, a broker balance or a hidden global.
- `position_budget_micros = floor(initial_capital_micros * bps / 10_000)`, checked integer math; overflow or a non-positive budget is refused.
- Reference price: the close of the fully completed strategy bar on which a FLAT strategy emits its new long target.
- `Q = floor(budget / reference_price)` whole Equity shares, never rounded up. A budget that cannot buy the minimum quantity is REFUSED with a deterministic reason code; there is no fallback to one share.
- `Q` is resolved on flat-to-long and held for the position lifetime (no continuous resizing). After a real exit, a new entry re-resolves from the SAME initial budget and the new causal close.
- Existing caps (`max_target_qty`, `max_position_notional_usd`) only reduce or refuse, through the one cap engine.
- Not in scope and not implemented: Kelly, ATR/volatility targeting, risk parity, compounding, cross-sectional allocation, non-Equity asset classes (asset class is never inferred from a symbol).

One shared pure resolver (`mqk-strategy` `sizing.rs`) is used by the Backtest strategy wrapper, the scanner and the Paper contract.

## Identity

A suffix `|sz_policy=<id>|sz_frac_bps=<N>` is appended to the canonical sizing string only when the capital-fraction policy is selected, so fixed-quantity identity is byte-identical to history (constant-based tests). Policy, fraction, capital (through the config) and caps each change `config_id`, hence `run_id`. The wrapper strategy fingerprint binds inner fingerprint, policy, fraction, capital, asset class and caps.

## Backtest

`BacktestEngine::add_strategy` wraps the real strategy in `CapitalFractionSizedStrategy` when the config selects the policy (Equity `WholeUnitsV1` only; one strategy). The real Strategy decision carries the resolved quantity; closed bars only, future-bar fills unchanged. `BacktestReport.sizing_provenance` records each resolved entry (reference bar, causal price, budget, quantity).

The wrapper's semantic identity has ONE pure constructor, `capital_fraction_semantic_fingerprint` (inner fingerprint, policy, fraction, immutable capital, asset class, caps; no result value). Backtest, Research registration and the runtime/deployment identity all derive the wrapped fingerprint through it.

## Research bridge

`emit_native_signal_stream` records the strategy the engine actually executes (`BacktestEngine::add_strategy_observed` hands the recorder the wrapper), so emitted `target_qty_micros` are the SAME absolute sized targets and the stream carries the same run identity as the canonical Backtest. `native-fingerprint` and `native-signals` take explicit `--sizing-policy/--allocation-fraction-bps/--initial-cash-micros` (+ optional caps); the capital-fraction bridge has no default capital, fraction or cap, and a stream carries a `sizing` provenance block (policy, fraction, capital, caps, the engine's resolved entries). Trial identity binds the wrapped fingerprint AND an explicit `capital_sizing` block (absent for fixed-quantity trials, so historical trial ids are unchanged). A stream is accepted only when its sizing block equals the registered contract, every positive quantity is an engine-resolved entry, and emitter cash equals the Research capital basis (the old ~USD 50k resize bridge cannot authorize this protocol). Benchmark V2 refuses capital-fraction candidates.

## Benchmark and review

- Benchmark V2 (`capital_matched_exact_target_buy_hold_v1`) is unchanged and remains the fixed-quantity benchmark.
- New `capital_fraction_matched_passive_buy_hold_v1` (`mqk-backtest` `benchmark_capital_fraction`): passive buy-and-hold of the candidate's first resolved quantity, same initial capital, execution model and cost basis.
- Benchmark run identity (IR-SZ-03): the benchmark strategy overrides `semantic_fingerprint` with `capital_fraction_benchmark_semantic_fingerprint` (policy/version, symbol, timeframe, exact target Q, reference bar `end_ts` and entry index), so Q and entry timing are part of the canonical `run_id`. `ScanCapitalFractionBenchmarkEvidence::verify_internal` recomputes the expected run id (`expected_capital_fraction_benchmark_run_id`) from the evidence's own config id, data hash, execution model, symbol, timeframe, Q and entry, so review and Promotion refuse a substituted run id or a tampered quantity/entry.
- Scanner/review: a fixed-quantity candidate requires Benchmark V2; a capital-fraction candidate requires the new benchmark. Cross-substitution, mismatch or malformed provenance fails closed; there is no legacy fallback.

## Promotion

Candidate, review, benchmark and canonical Backtest sizing policy and parameters must agree (`verify_review_capital_fraction_binding`); the IR-BV2-01 config/run binding is kept. The server-resolved native fingerprint (`resolve_native_deployment_identity`) is the capital-fraction wrapper fingerprint under the capital-fraction deployment contract, derived by the same pure constructor as Backtest, so review fingerprint == canonical Backtest fingerprint == native resolved fingerprint. The promotion fingerprint requirement is not weakened.

## Paper / runtime contract (seam present, Paper NOT activated)

`MQK_STRATEGY_SIZING_POLICY`, `MQK_STRATEGY_ALLOCATION_FRACTION_BPS`, `MQK_STRATEGY_ALLOCATED_CAPITAL_MICROS` form the deployment sizing contract (`mqk-runtime` `native_strategy.rs`): strict digits-only integers; partial, mixed, unknown or malformed values are refused; nothing set means the historical contract with an empty identity token. Caps are parsed strictly under this contract and a fixed-quantity `target_qty` is refused.

Restart safety (`mqk-runtime` `capital_fraction_host.rs`, `mqk-strategy` `sizing_state.rs`, `mqk-db` `held_sizing_state.rs`, migration 0091):

- The registry carries capital-fraction engines as `RestartRecovery::DurableStateRequired`; every stateless seam (`instantiate_verified`: native bootstrap, host pool, promotion identity) refuses them, so nothing can run one without the durable host. Paper stays inactive.
- `CapitalFractionRuntimeHost::recover` reads the durable snapshot for `(deployment, strategy)` and builds the wrapper only if every record proves against the contract: scope, state version, policy, fraction, immutable capital, caps, and the shared resolver reproducing the stored `Q` from the stored reference price. A foreign, stale, tampered, malformed or contract-mismatched record refuses the build; nothing is repaired or defaulted.
- A restored active entry re-emits exactly the original `Q` (no re-resolve from a later bar, no one-share reset, no duplicate entry). An exit transitions the record to `released` (kept as the generation floor); the next genuine entry resolves from the SAME immutable capital and fraction at the new causal close with `entry_generation + 1`.
- `on_bar_durable` persists the transitions in one transaction before returning the result (persist-before-act); persistence failure poisons the host. The store is idempotent (same generation + identical content is a no-op), refuses an entry over an active entry, a skipped or stale generation, and a release that does not match the stored entry.
- The deployment id is an opaque caller-supplied canonical identity; the daemon dispatch call sites are NOT yet wired to this host (Paper activation is a separate, authorized mission). No broker equity or buying power is ever a sizing input.

## CLI and declaration

- `backtest csv`: `--sizing-policy fixed_initial_capital_fraction_v1 --allocation-fraction-bps <N>` (required together; `--target-qty` other than 1 is refused under the policy). `--max-target-qty` / `--max-position-notional-usd` supply caps.
- `backtest scan-strategies --benchmark-policy capital_fraction_matched_passive_buy_hold_v1`: the same flags, plus the same caps, produce the same `config_id`/`run_id` as `backtest csv`. Sizing policy and benchmark policy must correspond; caps are accepted only under the capital-fraction policy; fixed-quantity scans keep their default sizing.
- Batch declaration (`research-py/experiments/m1_native_trend_campaign/run_batch.py`): optional `capital_sizing` block (`policy_id`, `allocation_fraction_bps`, `capital_basis = native_backtest.initial_cash_micros`, optional caps) validated at `check`, with the capital-fraction benchmark required. It feeds `backtest csv` and `scan-strategies` identically. `register` and `trials` carry the same sizing flags plus the explicit initial capital into `native-fingerprint` / `native-signals`, and bind the contract into trial identity and stream verification.

## Adjacent routes (census)

| Route | Class | Disposition |
|---|---|---|
| `backtest csv`, `scan-strategies`, `native-fingerprint`, `native-signals` | A: authoritative M1 path | wired to the one canonical sizing contract |
| `backtest csv-sweep`, `backtest db` | C: not on the M1 path | carry no sizing flags; clap refuses `--sizing-policy/--allocation-fraction-bps` (unexpected argument) so a capital-fraction declaration cannot run through them as fixed quantity; their runs keep `FixedQuantityV1` identity, which Promotion's capital-fraction binding refuses |
| daemon `POST /api/v1/backtests/jobs` | B: reachable, explicitly unsupported | `sizing_policy` / `allocation_fraction_bps` in the request are refused with 400 (previously an unknown field would have been ignored and a fixed-quantity job run) |

## Operator-approved future Batch 02 sizing

A PREDECLARED FUTURE POLICY DECISION, not a default and not executed here: next M1 hypothesis campaign sizing is `fixed_initial_capital_fraction_v1` with `allocation_fraction_bps = 1000` (10% of immutable initial allocated strategy capital); for the current M1 capital basis of USD 100,000 the nominal entry budget is USD 10,000. The infrastructure still requires the fraction explicitly; 1000 is not a code default and is never substituted for a missing value. Batch 02 is NOT STARTED, no Batch-02 hypotheses are declared, no trials are registered and no results exist; the holdout is RESERVED / UNCONSUMED.

## Batch 03 use (prospective, not executed)

Batch 03 (`docs/research/M1_BATCH03_PREDECLARATION.md`) uses this contract unchanged: 1000 bps of USD 100,000, quantity floor(USD 10,000 / completed signal-bar close), whole shares, no fallback, no compounding, no family-specific sizing, the same capital-fraction matched benchmark. Its P7A/P7B stress contract is the registered six-key `p7a_p7b_stress_contract_v1` and is deliberately not chosen here: `OPERATOR_DECISION_REQUIRED_BATCH03_STRESS_CONTRACT`. The Batch 02 stress values are not authority for it.

## Remaining (not required by this contract)

- Paper activation (daemon dispatch wiring of the durable host) requires its own authorized mission.
- Benchmark V2 (fixed-quantity) `benchmark_run_id` does not bind the exact target quantity or eligibility bar (its `ExactTargetFromEligibility` strategy has the spec-only fingerprint). It is frozen: changing it would move the identity of existing V2 evidence. Recorded as a known limitation of the frozen fixed-quantity authority, outside this contract.
