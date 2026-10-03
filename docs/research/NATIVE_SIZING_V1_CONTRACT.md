# FixedInitialCapitalFractionV1 — native sizing contract

Controller: `V4-M1-NATIVE-SIZING-V1-01`. Infrastructure, semantics and proof only: no campaign, no Batch 02, no
holdout consumption, no promotion, no Paper deployment, no Live. The production capital fraction is NOT SELECTED; that
is a future operator economic-policy decision.

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

## Benchmark and review

- Benchmark V2 (`capital_matched_exact_target_buy_hold_v1`) is unchanged and remains the fixed-quantity benchmark.
- New `capital_fraction_matched_passive_buy_hold_v1` (`mqk-backtest` `benchmark_capital_fraction`): passive buy-and-hold of the candidate's first resolved quantity, same initial capital, execution model and cost basis.
- Scanner/review: a fixed-quantity candidate requires Benchmark V2; a capital-fraction candidate requires the new benchmark. Cross-substitution, mismatch or malformed provenance fails closed; there is no legacy fallback.

## Promotion

Candidate, review, benchmark and canonical Backtest sizing policy and parameters must agree (`verify_review_capital_fraction_binding`); the IR-BV2-01 config/run binding is kept. The server derives its native fingerprint from the unwrapped registry, so a capital-fraction canonical Backtest (wrapper fingerprint) cannot match until a runtime capital-fraction seam exists. This is the intended fail-closed posture.

## Paper / runtime contract (no activation)

`MQK_STRATEGY_SIZING_POLICY`, `MQK_STRATEGY_ALLOCATION_FRACTION_BPS`, `MQK_STRATEGY_ALLOCATED_CAPITAL_MICROS` form the deployment sizing contract (`mqk-runtime` `native_strategy.rs`): strict digits-only integers; partial, mixed, unknown or malformed values are refused; nothing set means the historical contract with an empty identity token. A valid capital-fraction contract is bound into deployment identity (`sz_policy=...|sz_frac_bps=N|sz_capital_micros=C`) but is refused at registry build: the held entry quantity lives in memory only and is not restart-recoverable, so Paper/runtime fails closed rather than resize after a restart or fall back to one share. No broker-balance inference.

## CLI and declaration

- `backtest csv`: `--sizing-policy fixed_initial_capital_fraction_v1 --allocation-fraction-bps <N>` (required together; `--target-qty` other than 1 is refused under the policy). `--max-target-qty` / `--max-position-notional-usd` supply caps.
- `backtest scan-strategies --benchmark-policy capital_fraction_matched_passive_buy_hold_v1`: the same flags, plus the same caps, produce the same `config_id`/`run_id` as `backtest csv`. Sizing policy and benchmark policy must correspond; caps are accepted only under the capital-fraction policy; fixed-quantity scans keep their default sizing.
- Batch declaration (`research-py/experiments/m1_native_trend_campaign/run_batch.py`): optional `capital_sizing` block (`policy_id`, `allocation_fraction_bps`, `capital_basis = native_backtest.initial_cash_micros`, optional caps) validated at `check`, with the capital-fraction benchmark required. It feeds `backtest csv` and `scan-strategies` identically. `register` and `trials` refuse a declared block because the Research economic bridge does not yet resolve capital-fraction quantities (DEFERRED).

## Deferred

- Research economic bridge and registry trial identity carrying the sizing policy.
- `backtest db` / `csv-sweep` / daemon backtest routes capital-fraction flags.
- A restart-recoverable runtime capital-fraction seam (needed before Paper activation or a matching promotion fingerprint).
- Selecting a production fraction (operator decision).
