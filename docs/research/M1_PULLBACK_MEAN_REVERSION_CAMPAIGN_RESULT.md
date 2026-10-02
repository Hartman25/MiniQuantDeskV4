# M1 pullback_mean_reversion_20_2 Campaign 01 — Result

> **EVIDENCE STATUS: HISTORICAL — SUPERSEDED_PROTOCOL — NOT_PROMOTION_AUTHORITY.** The Research economics below were produced by native bridge v1, which turned the native +1-share absolute target into a binary weight and re-sized it (hundreds of shares) while Backtest/scanner evidence used a different capital basis. The numbers are preserved unchanged as an observed result only. See `M1_INDEPENDENT_REVIEW_CORRECTION_01.md`.

Mission `V4-M1-PULLBACK-MEAN-REVERSION-CAMPAIGN-01`. Definition: `research-py/experiments/m1_native_trend_campaign/PREDECLARED_CAMPAIGN_PULLBACK_01.json`, committed before the engine existed and before any campaign economics. Census: `M1_PULLBACK_MEAN_REVERSION_CENSUS.md`. Run evidence is local and untracked (`runs/run_pullback_01`).

## Verdict

**REJECTED.** Zero of five symbols reach `paper_candidate`, the judge's PBO is 0.528 against the frozen 0.5 maximum, no DSR reaches 0.5, and every symbol has negative net return. No promotion attempt; holdout reserved in all registries; `intraday_scalper`, `trend_sma50` and `dual_sma_50_200_trend` unchanged and closed; review, promotion and P9 thresholds untouched. The family is stopped: no other sigma, lookback, exit, filter or stop without a new operator decision.

## Hypothesis and engine

FLAT enters long when the completed close is at or below the trailing 20-close mean minus 2 population standard deviations (equality is an entry); LONG exits when the close is at or above the mean (equality is an exit); otherwise the state is held. Zero variance never enters, fewer than 20 bars is flat, an incomplete latest bar changes nothing, a malformed close fails closed to flat; never short. Exact integer form in `i128` over integer micros (`S - 20c >= 0` and `(S - 20c)^2 >= 4*(20Q - S^2)` with positive variance), so boundaries are exact. The instance owns its state; a fresh instance derives its state by replaying the earlier bars of its first window (a no-op for every Backtest, emitter, scanner and robustness run).

The emitted stream of every symbol matched an independent exact-rational (Fraction) implementation of the same state machine on 100% of 2,553 bars. A call-counting probe proved the real engine and the emitter keep one instance across sequential bars; mutations (reset state every bar, drop the first-call replay) fail their tests.

## Results (all five, frozen specification: 100,000 equity, 50,000 notional, fidelity floor 0.95)

Net and gross are economic walk-forward returns; CAGR is the annualized net return; profit factor and profitable-month fraction are computed from the economic daily net-return series for description.

| Symbol | Net | Gross | CAGR | Sharpe | Max DD | PF | Profitable months | DSR | Agreement | Entries / exits | % invested | Cost drag | Robustness failures | Stress | Review |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| SPY | -12.1% | +35.3% | -1.3% | -0.18 | -20.1% | 0.93 | 0.26 | 0.062 | 1.000 | 44 / 44 | 16.5 | 47.5% | delay, regime concentration, parameter neighborhood, temporal placebo, capacity | all three fail | `negative_total_return` |
| EFA | -5.0% | +29.4% | -0.5% | -0.06 | -19.4% | 0.98 | 0.27 | 0.119 | 1.000 | 49 / 49 | 21.0 | 34.4% | regime concentration, temporal placebo | pass | `negative_total_return` |
| IEF | -14.7% | -0.6% | -1.6% | -0.84 | -15.4% | 0.77 | 0.21 | 0.000 | 1.000 | 43 / 43 | 27.3 | 14.0% | regime concentration, temporal placebo | pass | `negative_total_return` |
| VNQ | -29.6% | +16.5% | -3.5% | -0.50 | -32.6% | 0.83 | 0.24 | 0.005 | 1.000 | 48 / 48 | 22.8 | 46.1% | temporal placebo | 3x cost fails | `negative_total_return` |
| GLD | -2.4% | +23.8% | -0.3% | -0.06 | -9.0% | 0.98 | 0.20 | 0.122 | 1.000 | 40 / 40 | 19.2 | 26.2% | none | 3x cost fails | `negative_total_return` |

Judge: `evaluated`, five attempted, five evaluable, none excluded, effective independent trial count 3.85, PBO 0.528 (over 0.5). Symbol leave-one-out is not applicable to one symbol.

## Reading

The rule has a positive gross edge on four of five symbols (and about 16-27% of days invested), but 40-49 round trips under the conservative bar-range execution model cost 14-47% of equity, which exceeds the gross return everywhere. Nothing was tuned in response; no threshold was derived from these numbers.

## Trial disclosure

This campaign's population is its own five trials. Earlier attempts of the closed trend families are disclosed, not pooled.
