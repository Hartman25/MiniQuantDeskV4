# M1 batch 01 — structural pre-batch census

Mission `V4-M1-NATIVE-HYPOTHESIS-BATCH-01`. Frozen before any batch economic result existed. Scope is the
research-selection machinery only; no threshold, benchmark, cost model or review policy is changed by this census.

## 0A — Benchmark / alpha comparability: `BENCHMARK_CONTRACT_CORRECT` (capital-comparability caveat recorded)

- Implementation (`mqk-backtest/src/sweep.rs`): `buy_and_hold_return_pct = (last_bar_close / first_bar_open − 1) × 100`;
  `alpha_pct = strategy_total_return_pct − buy_and_hold_return_pct`. Scanner score is `alpha_pct`
  (`strategy_scanner.rs`); the review policy gates `alpha_pct ≥ 0` (default `min_alpha_pct 0`).
- Documented contract (`docs/runbooks/backtest_workflow.md`, "What the buy-and-hold benchmark means" / "What alpha means"):
  exactly this formula — a single-symbol, gross, fully invested price-return benchmark, and alpha is the account
  total return minus it. The scanner audit (`docs/specs/strategy_scanner_promotion_01a_current_truth_audit.md` §5)
  records that no exposure metric exists, deliberately, because `BacktestReport` does not expose per-bar position
  size. No accepted document defines an exposure- or capital-matched benchmark.
- Therefore the code matches the accepted contract; it is not a deterministic contract mismatch. The campaign sizing
  (initial equity 100,000, notional 50,000) is a research-protocol choice carried from campaign 03, not part of the
  backtest benchmark contract. The native strategies emit a fixed one-share signal, and the Rust backtest/scanner
  alpha is computed from the Rust backtest equity curve (not from the Research economic walk-forward sizing).
- Open governance question, recorded not acted on: whether review alpha should instead be exposure- or
  capital-comparable. Unchanged here: threshold, benchmark semantics and prior results.

## 0B — Execution cost model: `ALREADY_CORRECT+PROVEN`

- Baseline Research evidence is `rust_conservative_bar_range_v1` (`REQUIRED_EXECUTION_PRICING_PROTOCOL_ID` in
  `mqk-promotion/src/research_evidence.rs`). The native bridge calls `require_official_execution_pricing_parity` and
  `require_official_weight_to_share_parity` before registering a trial (`native_signal_registry_integration.py`).
- Costs are not double-applied: the baseline spec carries commission 10 bps per side and `slippage_bps_per_side = 0`
  (the economic spec's double-charging guard requires that whenever the directional execution price cost can be non-zero);
  the adverse-price cost is the separate additive component. Stress (15 bps / vol 10 bps / cap 25,000) is applied only by
  the P7A/P7B finalizer, never in the baseline run.

## 0C — Batch multiple-testing: `SUPPORTED_BY_EXISTING_SCOPE`

- `build_multiple_testing_judge(experiment_id, hypothesis_id=None)` takes every trial of the experiment when
  `hypothesis_id` is unset. Its comparison key (`_comparison_key`) is provenance, evaluation spec, annualization, cost
  model and capacity/fold-end policy only; strategy, hypothesis and signal-policy identity are deliberately excluded, so
  trials from different hypotheses share one scope when the protocol and calendar match.
- The batch therefore uses ONE experiment id holding 3 hypotheses and 15 trials, with the judge run once over the whole
  experiment. No new seam is required. `register_hypothesis` allows several hypotheses per experiment.
- Prior closed families have their own experiment ids and registries. The canonical scope rule is `experiment_id`, so
  they are disclosed by name in the predeclaration but are neither merged nor injected. That omission is a stated
  statistical limitation: the DSR deflates over this batch's 15 trials, not over the four earlier families' attempts.

## 0D — Other direct defects

NONE found in the three authoritative paths above. Structural notes (not defects): the 252/253/204-bar warm-ups leave
the early part of fold 1 flat; `STRATEGY_CONTEXT_LOAD_LIMIT` (256) already exceeds every batch requirement.

M1_BATCH01_CENSUS_FROZEN
