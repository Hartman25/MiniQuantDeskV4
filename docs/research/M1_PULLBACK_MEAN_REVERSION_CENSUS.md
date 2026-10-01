# M1 pullback_mean_reversion_20_2 — Bounded Stateful-Seam Census

Mission `V4-M1-PULLBACK-MEAN-REVERSION-CAMPAIGN-01`. Frozen before implementation or any economic result. Srclight was reindexed first (index now at local HEAD `2646d63e`, 1,367 files); callers/callees below were located with it and the complete definitions were read natively.

| Seam | Finding | Disposition |
|---|---|---|
| Strategy trait | `on_bar(&mut self, …)`: a strategy may own state across calls; no hook resets it | ALREADY_CORRECT |
| `StrategyHost` | registers one boxed instance and calls the same instance every bar | ALREADY_CORRECT |
| `BacktestEngine` | `add_strategy` once, `host.on_bar` per bar; instance lives for the run | ALREADY_CORRECT (test required: a state-dependent probe sees sequential state) |
| Native signal emitter | wraps the one instance in a recorder inside one engine run; forwards `required_history_bars` | ALREADY_CORRECT |
| Stress / robustness reruns | factory builds a fresh instance per run; delay wrappers hold their inner instance across bars and forward required history | ALREADY_CORRECT |
| Scanner / review | runs the strategy through `BacktestEngine` | ALREADY_CORRECT |
| Paper `NativeStrategyBootstrap` / `invoke_on_bar_from_window` | one `StrategyHost` stored for the run; the same instance is called each dispatch | ALREADY_CORRECT within a run |
| **Paper restart / fresh-instance evaluation** | a new run (or the dry-run diagnostic, which instantiates per call) starts FLAT while the broker or the true history may be LONG; the complete-target contract would then flatten a held position | CHANGE_REQUIRED: derive the starting state deterministically from the call's own window (first-call replay, a no-op for every Backtest/emitter/robustness run because they start with a 1-bar window) |
| Repeated evaluation of the same bar | entry (close at or below mean minus 2 std, std > 0) and exit (close at or above mean) are mutually exclusive, so replays are idempotent | ALREADY_CORRECT (test required) |
| Semantic fingerprint | per-engine override; must bind every behavior-bearing choice including the recovery rule | CHANGE_REQUIRED (new engine) |
| `required_history_bars`, `minimum_completed_bars`, Backtest window, Paper load limit (256) | shared seam from the dual-SMA mission; this engine declares 20 | ALREADY_CORRECT (test required for Backtest and the Paper guard) |
| `REGISTERED_STRATEGY_IDS`, `MAX_STRATEGY_UNIVERSE` (7) and mirrored tests, runtime names test, CLI help | one new identity | CHANGE_REQUIRED |
| Research bridge, promotion fingerprint authority | strategy-agnostic; the stateful stream is emitted from the real instance | ALREADY_CORRECT (parity proof required) |
| Review policy, promotion policy, P9 thresholds | frozen; not touched | ALREADY_CORRECT |
| TODO/FIXME/unimplemented on this path | none found in the touched seams | ALREADY_CORRECT |

Numeric convention (exact, integer micros): with `S` the sum and `Q` the sum of squares of the latest 20 closes, `20*Q - S^2` is 400 times the population variance. Entry is `S - 20*c >= 0` and `(S - 20*c)^2 >= 4*(20*Q - S^2)` with `20*Q - S^2 > 0`; exit is `20*c >= S`. This is algebraically identical to `c <= mean - 2*std` and `c >= mean` with exact boundary equality.
