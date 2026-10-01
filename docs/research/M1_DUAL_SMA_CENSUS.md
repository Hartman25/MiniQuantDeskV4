# M1 dual_sma_50_200_trend — Bounded Pre-Edit Census

Mission `V4-M1-DUAL-SMA-50-200-CAMPAIGN-01`. Frozen before engine implementation or any economic result. Only the seams needed to add one native 1D strategy to the existing bridge.

| Seam | Finding | Disposition |
|---|---|---|
| `mqk-strategy/engines/mod.rs` registration, `REGISTERED_STRATEGY_IDS` (6) and its count assertions | one new identity changes the count to 7 | CHANGE_REQUIRED |
| `mqk_portfolio::MAX_STRATEGY_UNIVERSE` (6) and `MAX_CANDIDATE_PAIRS`, mirrored by `dynamic_selection_plan_builder` tests (7-id over-limit fleet, "all six known") | mirror must move 6 -> 7 (bound tests 7 -> 8) | CHANGE_REQUIRED |
| `mqk-runtime::native_strategy` registry-names test, `mqk-cli` strategy help text | add the new name | TEST_REQUIRED / CHANGE_REQUIRED |
| Strategy semantic identity (`SemanticIdentityBuilder`) | per-engine override; new engine must bind name, version, symbol, timeframe, 50, 200 | ALREADY_CORRECT (pattern) |
| Native signal emitter, Research bridge, scanner/review, promotion fingerprint authority | strategy-agnostic; no change expected | ALREADY_CORRECT |
| **Backtest history window**: `BacktestConfig.bar_history_len` is 50 with no CLI override; `trend_sma50`'s lookback equalled it. A 200-bar engine would see 50 bars and be silently flat forever in Backtest, the emitter, the scanner, stress and robustness | real defect exposed by the new engine | CHANGE_REQUIRED |
| **Paper window**: `STRATEGY_CONTEXT_LOAD_LIMIT = 30` ("covers every built-in lookback (20)"). A 50- or 200-bar engine would be silently flat in Paper, a false truthful no-trade | real defect; matters before any promotion | CHANGE_REQUIRED |
| Data-readiness coverage reads `minimum_completed_bars` from registry metadata | the new engine's meta declares 200 | ALREADY_CORRECT |
| Evaluation partition (12-month folds from 2016-03-01, 6-month holdout) | unchanged; the first ~7 months are flat by construction (200 completed closes first exist in 2016-10) | ALREADY_CORRECT, disclosed |

Fix design: add `Strategy::required_history_bars()` (default 0, additive), exposed by `StrategyHost`; the backtest engine uses `max(config.bar_history_len, required)`; the Paper load limit becomes 256 with a guard test that it is at least every registered engine's `minimum_completed_bars`.
