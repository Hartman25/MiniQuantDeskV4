# Benchmark V2 as scanner/review alpha authority

Policy id: `capital_matched_exact_target_buy_hold_v1`
(`mqk_backtest::benchmark_v2::BENCHMARK_V2_POLICY_ID`).

## Contract

For exact-target native candidates (long/flat, one fixed positive quantity) the
legacy fully-invested buy-and-hold alpha is not an economically comparable
baseline. Under the V2 policy, review alpha is

    alpha_pct = candidate account return - Benchmark V2 account return

both on the same initial-cash basis, same quantity, same engine/cost config,
same bars, benchmark eligibility = candidate's first causally eligible bar.
`min_alpha_pct = 0` is unchanged.

The policy is an explicit input on both sides and is never inferred:

| Surface | Legacy (default) | V2 |
|---|---|---|
| `mqk backtest scan-strategies` | omit `--benchmark-policy` | `--benchmark-policy capital_matched_exact_target_buy_hold_v1` |
| `mqk backtest review-scan` | omit | same flag |
| `ScanManifest` / `ReviewManifest` `benchmark_policy_id` | absent (historical bytes unchanged) | the policy id |
| `scan_id` / `review_id` | historical derivation | policy bound into the derivation |

Review refuses a scan whose manifest policy differs from the review policy in
either direction. A legacy scan can never be reviewed as V2 authority.

## Fail-closed behavior

* V2 scan, benchmark not computable (no emission instance, never takes a
  position, mixed/negative quantities, emission/candidate run mismatch):
  candidate is `metrics_unavailable` / `benchmark_unavailable`, reviewed
  `blocked` (`not_candidate_ranked`). No fallback to legacy alpha.
* V2 review, candidate evidence missing → `missing_benchmark_v2`; wrong
  policy id → `benchmark_policy_mismatch`; any binding violation →
  `benchmark_binding_mismatch`. All `blocked` before any alpha/return gate.
* The legacy fully-invested return is retained only as
  `legacy_buy_and_hold_return_pct` inside the evidence (informational).

## Evidence bound per candidate (`ScanBenchmarkV2Evidence`)

strategy id, strategy semantic fingerprint, symbol, timeframe, candidate run
id / config id / initial cash / target qty / execution model / total return,
required history bars, benchmark run id / config id / target qty / eligibility
bar index and decision ts / initial cash / execution model / account return,
alpha, `input_data_hash` (bars both runs consumed), evaluation endpoint.
`verify_internal` enforces: accepted policy id; candidate==benchmark capital,
quantity, execution model and config id (config id covers commission, slippage,
liquidity, sizing — a cost-stripped benchmark is detected); eligibility ==
`required_history_bars - 1`; alpha == candidate return − benchmark return.
The review additionally binds the evidence to the candidate row (strategy,
symbol, timeframe, metrics, data endpoint, eligibility inside evaluated bars).

## Promotion binding

`routes::strategy_promotions` calls
`enforce_native_review_benchmark_binding` after the Research gate and before
`evaluate_promotion`. When the verified Research trial binds a native semantic
fingerprint (corrected native evidence), the review evidence must have:
manifest policy = V2; row carries V2 evidence that verifies internally; strategy
id / symbol / timeframe equal the promotion candidate; strategy semantic
fingerprint equals the Research trial's; capital equals the canonical Backtest
evidence's starting equity; `input_data_hash` equals the Backtest evidence
report's; row score equals the V2 alpha; and (IR-BV2-01, see
`BENCHMARK_V2_PROMOTION_ECONOMIC_BINDING.md`) the row's candidate `config_id`
and `run_id` equal the canonical Backtest report's. Non-native (legacy
classifier) Research trials are outside this binding.

## Census dispositions (Phase 0, frozen before editing)

| # | Finding | Disposition |
|---|---|---|
| 1 | Legacy alpha computed in `sweep_row_from_report` (`buy_and_hold_return_pct`), consumed by `evaluate_scan_candidate` | CHANGE_REQUIRED → fixed (V2 branch) |
| 2 | Scanner → review via `candidates.json`; review gate reads `metrics.alpha_pct` | CHANGE_REQUIRED → fixed (policy-bound evidence, review verifies) |
| 3 | Review artifacts bound scan id + policy thresholds only; no benchmark identity | CHANGE_REQUIRED → fixed (manifest + row evidence) |
| 4 | Fields for V2 in review artifacts | CHANGE_REQUIRED → added additively (`serde(default)`, skipped when absent) |
| 5 | Promotion verifier required no benchmark policy for corrected native evidence | CHANGE_REQUIRED → fixed (route binding) |
| 6 | `native-signals` writes `benchmark_v2.json` non-fatally | ALREADY_CORRECT+PROVEN: review authority does not read that file; the scanner computes V2 internally and fails closed. The CLI artifact stays an additive generic emission output |
| 7 | Sweep `total_return_pct` uses first equity point, V2 uses initial cash | CHANGE_REQUIRED → V2 path uses the initial-cash basis for candidate return |
| 8 | `candidate_run_id` authenticity cannot be recomputed from the review artifact alone | SUPERSEDED by IR-BV2-01: promotion now compares `candidate_run_id` and `candidate_config_id` directly with the canonical Backtest report's (the earlier transitive binding did not cover the scanner's integrity config) |
| 9 | Python `summarize_batch.py` reads `review_decisions.csv` | TEST_REQUIRED at Phase 5 (corrected table needs V2 columns) |

## Proof

* `mqk-backtest/tests/scenario_benchmark_v2_review_authority_01.rs` — real
  production path; table-driven tamper negatives; policy pairing; artifact
  contents; legacy path unchanged.
* `mqk-daemon/tests/scenario_promotion_benchmark_v2_binding_01.rs` — real
  artifacts read through `validate_paper_candidate_evidence`; accept /
  legacy-refused / identity-mismatch / forged-evidence; route wiring guard.
* `mqk-cli/tests/scenario_cli_scan_benchmark_v2_01.rs` — real binary end to end.
* Mutation proof: benchmark costs removed, alpha fallback to legacy, silent
  degrade without emission instance, skipped capital / data-identity /
  fingerprint / policy checks in promotion — each fails the named tests.
