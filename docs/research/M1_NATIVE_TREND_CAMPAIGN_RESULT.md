# M1 Native Trend Campaign — Result

Mission `V4-M1-NATIVE-RESEARCH-PROMOTION-BRIDGE-01`. Definitions: `research-py/experiments/m1_native_trend_campaign/PREDECLARED_CAMPAIGN.json` (01) and `PREDECLARED_CAMPAIGN_02.json` (02). Run evidence is local and untracked (`runs/run_01`, `runs/run_02`, per the repo's `runs/` convention).

## Verdict

**No promotable candidate.** Both campaigns stopped at a predeclared, outcome-free validity or judge gate. The reserved holdout was never evaluated or consumed in either registry. `intraday_scalper`'s rejection is untouched. No promotion row, Paper or Live state was changed.

## What was built (minimum bridge, reusing existing seams)

- `trend_sma50` native 1D engine: the same implementation runs in Backtest and Paper.
- `mqk backtest native-signals`: runs the real strategy through the real `BacktestEngine` behind a recording wrapper and emits its per-bar targets bound to its semantic fingerprint.
- `native_signal_registry_integration.py`: registers that stream as a predeclared Research trial and evaluates it with the unmodified `run_economic_walkforward`, official pricing/weight-to-share parity, the holdout ledger and the trial/attempt registry. Retries are new attempts of the same trial.
- Promotion seam: a native trial's registered semantic fingerprint must equal the server-resolved fingerprint of the promotion candidate (A/X never authorizes A/Y or B/X); `MQK_RESEARCH_REQUIRE_NATIVE_SEMANTIC_BINDING` refuses label-only trials.

## Campaign 01 — voided, preserved

Five trials (one per Faber GTAA symbol: SPY, EFA, IEF, VNQ, GLD) all `succeeded`, but the economic engine rejects a fill whose notional exceeds the allocation cap at the conservative fill price, and the spec sized one fully weighted symbol exactly at that cap. The simulated discrete position realized only 10–275 of 1,342–1,850 desired long bars per symbol (agreement 0.28–0.47). The emitted signal itself matched an independent computation of the rule 100%. Classified `INVALID_FOR_STATED_HYPOTHESIS`.

## Campaign 02 — final for this hypothesis family

Changed only the position notional headroom (90,000 of 100,000), added the fixed 0.95 execution-fidelity gate, new experiment id, byte-identical data reuse. Result: four attempts `failed` the gate (agreement 0.74–0.92) and were kept; IEF alone succeeded. The multiple-testing judge is `partially_evaluable` (one included trial, PBO `not_evaluable`), which fails the predeclared "judge evaluable" gate. Ten trials in total were tested on this hypothesis; the judge deflates over campaign 02's five.

## Root cause and decision needed

The economic protocol sizes at a fixed `equity_usd` but admits fills against running equity, so a long/flat single-symbol position collides with the cap whenever equity dips or the fill gaps up. DSR, PBO and Sharpe are invariant to exposure scale, so a half-equity notional cap would remove the collision without touching the hypothesis, but it is a third specification of the same family and needs an explicit operator decision (campaign 02 declared itself final).

## Not done

No daemon promotion transition (no candidate), no end-to-end route proof with native-bound evidence beyond gate unit tests and a synthetic rehearsal of backtest + the three finalizers, no Paper readiness or market-hours work.
