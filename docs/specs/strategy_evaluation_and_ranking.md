# Strategy Evaluation and Ranking (V4)

Select strategies/configs that produce the **smoothest, most reliable compounding**.

Two-stage model:
- Hard gates (promotion eligibility)
- Consistency score (ranking among eligible)

## 1) Hard gates

Data/realism:
- ambiguity_policy == CONSERVATIVE_WORST_CASE
- pass stress profile: slippage_x2 (minimum)
- zero LOOKAHEAD_VIOLATION
- data integrity per data pipeline spec
- survivorship/corp action limitations declared

Risk/stability:
- MDD <= 20% (MAIN default)
- worst rolling 5-day return >= -6% (or worst week >= -8%)
- >= 30 trades over window
- concentration not breached (if enabled)

Robustness:
- walk-forward positive in >= 70% folds
- fold MDD <= global limit + 5%

Failure blocks promotion and emits PROMOTION_GATE_FAILED.

## 2) Consistency score (MAIN)

Transforms:
- R = ln(1 + CAGR)
- D = ln(1 + MDD)
- V = ln(1 + Vol)
- T = ln(1 + abs(Worst5D))
- S = ln(1 + max(0, BaseCAGR - StressCAGR))
- B = clip(Stability, 0, 1)

Normalize:
- Sharpe_norm = clip(Sharpe, 0, 3) / 3
- PF_norm     = clip(PF, 1, 2) / 2

Score:
Score = 100 * (
  1.0 * R
+ 0.6 * Sharpe_norm
+ 0.4 * PF_norm
+ 0.8 * B
- 1.2 * D
- 0.8 * V
- 0.8 * T
- 1.0 * S
)

Tie-break if within 5 points:
1) lower MDD
2) higher StressCAGR
3) higher Stability
4) fewer negative folds
5) less negative tail (Worst5D)

## 3) MAIN vs EXP

MAIN uses strict gates and MAIN score.

EXP may use looser weights for exploration, but live arming still requires:
- clean reconcile
- protective stop invariant
- no-lookahead
- pass slippage_x2

Evaluation profile recorded in manifest.
