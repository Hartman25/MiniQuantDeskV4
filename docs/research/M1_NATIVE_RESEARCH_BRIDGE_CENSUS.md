# M1 Native Research Bridge — Pre-Edit Census

Mission `V4-M1-NATIVE-RESEARCH-PROMOTION-BRIDGE-01`. Frozen before any bridge edit. Cites code, not the ledger.

## Observations A-G

| # | Observation | Disposition | Evidence |
|---|---|---|---|
| A | Trial identity carries `strategy_id` | ALREADY_EXISTS | `research_trials.strategy_id`; `ResearchResultStore.register_trial` (`exp_distributed/storage.py`) |
| B | Registry does not require a classifier family | ALREADY_EXISTS | `strategy_id` is a free string. The only registered entry, `run_registered_economic_walkforward_eval`, is classifier-only (features/targets/logreg) |
| C | Rust scanner/backtest run on native strategy ids | ALREADY_EXISTS | `mqk-cli backtest csv`, `scan-strategies`, `PluginRegistry::instantiate` |
| D | Promotion binds Research trial `strategy_id` == Backtest `strategy_name` + semantic fingerprint == promoted `strategy_id`; review row binds `(strategy_id, symbol, timeframe)` | ALREADY_EXISTS | `research_evidence_gate.rs`, `backtest_evidence_gate.rs`, `strategy_config_identity.rs`, `validate_paper_candidate_evidence` |
| E | Missing seam is "register a native strategy's signals as a predeclared Research trial/evaluation" | CODE_MISSING | no registered entry point accepts an externally supplied OOS score stream; no native-signal emitter exists |
| F | Scanner/review `paper_candidate` works for a native engine | ALREADY_EXISTS | `evaluate_scan_review_decision` is strategy-agnostic. Policy: return >= 0, alpha vs buy-and-hold >= 0, drawdown <= 25%, PF >= 1.05, trades >= 5, bars >= 252 |
| G | Python P9 slices (placebo, P7A/P7B stress, DSR/PBO sensitivity) and Rust finalizers act on the persisted OOS score stream of a registered trial | ALREADY_EXISTS (signal-agnostic) / TEST_MISSING for native-signal trials | `genuine_shuffled_placebo_cli.py`, `FinalizeRobustnessSensitivity`, `FinalizeGenuineShuffledPlacebo`, `FinalizeP7aP7bReplayStress` |

## Other items

| Item | Disposition |
|---|---|
| `run_economic_walkforward` needs only `walk_forward_eval.json` folds (`test_start_utc`/`test_end_utc`), `walk_forward_oos_predictions.csv` (`fold,symbol,decision_ts,ml_score` in [0,1]) and provenance-bound bars | ALREADY_EXISTS |
| Judge comparison scope reads `identity.data_identity.bars_provenance`, `identity.evaluation_spec`, `identity.economic_protocol` | ALREADY_EXISTS; native trial identity must supply the same keys |
| Rust verifier requires `economic_walk_forward_v1` artifact + registered judge; it does not read universe mode | ALREADY_EXISTS |
| Native engine for the predeclared hypothesis | CODE_MISSING (existing engines at supported timeframes are `swing_momentum` 1D and `intraday_scalper` 5m, both already rejected) |
| Backtest `bar_history_len` defaults to 50 (`BacktestConfig`) with no CLI flag | OBSERVATION: engine lookback is bounded by 50 closes unless the engine contract changes |
| Daemon promotion policy thresholds (`MQK_PROMOTION_*`, `MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO`, `MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING`) and evidence roots are unset in the deployed local config | POLICY_REQUIRED: must be predeclared before results, never set after |
| Alpaca research-data credentials available (`ALPACA_API_KEY_PAPER`/`ALPACA_API_SECRET_PAPER`, presence only) | ALREADY_EXISTS |

## Point-in-time universe determination

- The Rust promotion verifier and daemon gates contain no universe/point-in-time check (`mqk-promotion/src`).
- `mqk_research.data.bars_provenance` supports only `universe_mode=fixed_ex_ante`; point-in-time membership is unimplemented and unclaimed.
- The "cannot exceed development verdict" cap lives in the Wave06 campaign's own predeclared advancement policy (`wave06_campaign/PREDECLARED_CAMPAIGN.json`, `forbidden_verdicts_for_this_non_pit_study`) and applies to that broad, post-hoc 88-symbol snapshot only.
- A predeclared, literature-defined fixed symbol list needs no point-in-time membership data: membership is fixed before results and every listed symbol is registered as a trial (win or lose), so selection among them is accounted for by the multiple-testing judge. No point-in-time blocker is asserted for this campaign. Survivorship of the listed ETFs is not zero and is stated as a limitation, not hidden.
