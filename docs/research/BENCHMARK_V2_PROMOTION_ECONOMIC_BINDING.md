# Benchmark V2 promotion economic binding (IR-BV2-01)

Mission `V4-M1-BENCHMARK-V2-PROMOTION-BINDING-CORRECTION-01`. Not pushed.
Extends `BENCHMARK_V2_REVIEW_AUTHORITY.md`; no Research hypothesis, result,
threshold or holdout changed.

## Defect

An independent review found that Promotion proved the V2 scanner candidate used
the same capital / quantity / execution model / config as its own passive
benchmark, but not that it used the same execution/economic contract as the
**canonical Backtest evidence** Promotion consumes. Across all 14 evaluable
corrected Batch 01 review rows the scanner candidate `config_id`
(`c043ecec-99ac-58f5-b773-99448aef15e4`) differed from the canonical Backtest
report's (`a694f4bb-a933-5efe-b7ce-706a757febb5`).

No candidate was ever falsely promoted: Batch 01 stays `BATCH_REJECTED`
(14 rejected, 1 blocked, 0 `paper_candidate`).

## Root cause (reproduced numerically)

Both ids were reproduced from the two real config constructions. Four
`BacktestConfig` fields differ; every other field is equal.

| Field | Scanner (`StrategyScanPolicy::default`) | Canonical `backtest csv` | Class |
|---|---|---|---|
| `integrity_enabled` | `false` | `true` | OUTCOME_BEARING |
| `integrity_stale_threshold_ticks` | `120` | `259200` | OUTCOME_BEARING |
| `integrity_gap_tolerance_bars` | `0` | `3` | OUTCOME_BEARING |
| `integrity_calendar` | `AlwaysOn` | `NyseWeekdaysStartAnchored` | OUTCOME_BEARING |

Integrity fields are not "validation only": the gate disarms/halts execution
(`execution_blocked`), so it can change fills, P&L and the strategy's executable
outcome. The observed returns agreed only because no bar was blocked. No field is
excluded from the binding. Instrument economics live outside `BacktestConfig`
but are folded into `run_id`.

## Authority

Full `BacktestConfig` equality is valid (the scanner can run under the canonical
config), so the strongest existing identities are reused and no new artifact
field is introduced:

* `config_id` — every `BacktestConfig` field (costs, slippage, volatility and
  participation impact, liquidity, sizing, quantity semantics, risk/halting,
  integrity, calendar, corporate-action policy, capital, history, timeframe).
* `run_id` — UUIDv5 over strategy, `config_id`, input-data hash, instrument
  economics (multiplier / margin), execution model, strategy semantic
  fingerprint and quantity semantics.

`verify_review_benchmark_v2_binding` (in `promotion_evidence_validation.rs`) now
takes a `BacktestEvidenceIdentity` built from the resolved
`BacktestEvidenceBundle` and additionally requires: the row's
`candidate_config_id` and `candidate_run_id` equal the canonical report's (both
must be parseable and non-nil; two unusable ids never match), the candidate
execution model equals the report's, and the strategy semantic fingerprint equals
the report's. Mismatch fails closed with an explanatory blocker.

Legacy or absent evidence needs no compatibility shim: a review row without V2
evidence was already refused, and a V2 row produced under a different config
cannot satisfy the equality. Old Batch 01 review rows (scanner-default config)
therefore can no longer authorize a corrected native promotion.

## Producing promotable V2 evidence

`scan-strategies --benchmark-policy capital_matched_exact_target_buy_hold_v1`
accepts `--initial-cash-micros`, `--integrity-enabled`,
`--integrity-stale-threshold-ticks`, `--integrity-gap-tolerance-bars`,
`--integrity-calendar`. Defaults mirror `backtest csv`; passing them without the
V2 policy is refused. `backtest csv` and the V2 scan build their config through
one function (`canonical_csv_backtest_config`), so identical flags yield an
identical `config_id` and `run_id`. The V2 `scan_id` binds the base config
(`mqk-scan.v3`), so differently configured scans can never share an artifact
directory. Legacy scans and ids are unchanged. The M1 batch runner passes one
shared integrity argument list to both commands.

## Proof

* `mqk-daemon/tests/scenario_promotion_benchmark_v2_economic_binding_01.rs` —
  real scanner V2 row read through `validate_paper_candidate_evidence` against a
  real `BacktestEngine` report: accepts the matching pair; refuses each of 25
  config-field mutations, instrument multiplier and margin economics, different
  bars, fingerprint, execution model, run identity, capital, nil/unparseable ids.
  Pre-fix the controller test (scanner-default config vs canonical config)
  returned `Ok(())` (RED); post-fix it is refused.
* `mqk-backtest/tests/scenario_scan_canonical_config_binding_01.rs` — the
  production scan path under the canonical policy carries the canonical run
  identity; the default V2 scan does not; scan identity binds the config.
* `mqk-cli/tests/scenario_cli_scan_v2_canonical_config_01.rs` — real binary:
  `backtest csv` and `scan-strategies` with the same flags give the same
  `run_id` and `config_hash` (`a694f4bb-…` for the Batch 01 flags); different
  flags or capital do not; config flags without V2 are refused.
* Mutation proof: disabling the `run_id` check lets economics mismatches pass
  (3 tests fail); disabling `config_id` and `run_id` lets cost mismatches pass
  (5 tests fail).

## Status

Corrected Batch 01 outcome unchanged. Holdout RESERVED / UNCONSUMED. No
promotion, Paper or Live action. The 14 historical rows are rejection evidence
only.
