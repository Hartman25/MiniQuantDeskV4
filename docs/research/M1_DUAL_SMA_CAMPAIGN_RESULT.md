# M1 dual_sma_50_200_trend Campaign 01 — Result

> **EVIDENCE STATUS: HISTORICAL — SUPERSEDED_PROTOCOL — NOT_PROMOTION_AUTHORITY.** The Research economics below were produced by native bridge v1, which turned the native +1-share absolute target into a binary weight and re-sized it (hundreds of shares) while Backtest/scanner evidence used a different capital basis. The numbers are preserved unchanged as an observed result only. See `M1_INDEPENDENT_REVIEW_CORRECTION_01.md`.

Mission `V4-M1-DUAL-SMA-50-200-CAMPAIGN-01`. Definition: `research-py/experiments/m1_native_trend_campaign/PREDECLARED_CAMPAIGN_DUAL_SMA_01.json`, committed before the engine was implemented and before any campaign economics. Census: `M1_DUAL_SMA_CENSUS.md`. Run evidence is local and untracked (`runs/run_dual_sma_01`).

## Verdict

**REJECTED.** Zero of five symbols reach `paper_candidate`, the judge's PBO is 0.548 against the frozen maximum of 0.5, and every symbol fails the required regime-concentration scenario. No promotion attempt; holdout reserved in all registries; the closed `trend_sma50` registries and `intraday_scalper`'s rejection are unchanged. This family is stopped: no other fast/slow pair, band, filter or confirmation rule may follow without a new operator decision.

## Hypothesis and engine

Long one share while the exact 50-close mean exceeds the exact 200-close mean of completed daily closes; equality, fewer than 200 bars, an incomplete or non-positive close, or an incomplete latest bar is flat; never short. A different rule from the closed `trend_sma50` (price versus one average). Native Rust engine, fingerprint binds name, version, symbol, timeframe, 50 and 200. The emitted stream agreed 100% with an independent pandas computation on all five symbols.

Two defects the 200-bar rule exposed were fixed first: the backtest engine's fixed 50-bar history window (now `max(config, Strategy::required_history_bars())`, forwarded through the emitter and delay wrappers) and Paper's 30-bar load limit (now 256, guarded against every registered engine's requirement). Either would have left the rule silently flat.

## Results (all five, frozen specification: 100,000 equity, 50,000 notional, fidelity floor 0.95)

| Symbol | Net return | Sharpe | DSR | Position agreement | Transitions (entries/exits) | Cost drag | Robustness failures | Stress | Review |
|---|---|---|---|---|---|---|---|---|---|
| SPY | +49.1% | 0.58 | 0.691 | 1.000 | 9 (5/4) | 13.9% | regime concentration | pass | `non_positive_alpha` |
| EFA | +20.0% | 0.31 | 0.369 | 1.000 | 11 (6/5) | 8.3% | regime concentration | pass | `non_positive_alpha` |
| IEF | +1.9% | 0.10 | 0.155 | 1.000 | 15 (8/7) | 2.9% | regime concentration | 3x cost fails | `non_positive_alpha` |
| VNQ | -25.5% | -0.38 | 0.005 | 1.000 | 17 (9/8) | 11.5% | regime concentration, temporal placebo | pass | `negative_total_return` |
| GLD | +67.3% | 0.72 | 0.821 | 1.000 | 15 (8/7) | 9.2% | regime concentration | pass | `non_positive_alpha` |

Judge: `evaluated`, five attempted, five evaluable, none excluded, effective independent trial count 4.20, PBO 0.548 (over the 0.5 maximum). Only GLD and SPY clear the DSR minimum of 0.5. Symbol leave-one-out is not applicable to one symbol.

## Descriptive comparison with the closed trend_sma50 (after the fact, used for nothing)

Signal transitions fell from about 160 to 9-17 per symbol and cost drag from 32-72% of equity to 3-14%, which is the rationale for the rule; the gates the frozen policy sets (alpha against buy-and-hold, regime concentration, PBO) are still not met.

## Trial disclosure

This campaign's population is its own five trials. Fifteen earlier attempts belong to the closed `trend_sma50` family (a different hypothesis and experiment) and are disclosed, not pooled.
