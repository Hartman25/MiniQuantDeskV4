from __future__ import annotations

from typing import Optional

import numpy as np
import pandas as pd

# RESEARCH-ECONOMIC-WALKFORWARD-01
#
# Pure, deterministic economic-metric helpers. No IO, no wall-clock reads, no
# RNG. Each function is independently unit-tested against hand-computable
# series (see tests/test_economic_walkforward.py) rather than "is finite"
# smoke checks. Do not reuse exp_distributed.strategies._metrics_from_returns'
# Sharpe/annualized-return formulas here — they are a materially different,
# pre-existing (gross-only) convention documented separately.

DDOF = 0  # population standard deviation, applied uniformly to every std() call below.


def validate_return_series(returns: pd.Series) -> None:
    """Fail closed on an impossible per-interval compounded return (<= -100%)."""
    if (returns <= -1.0).any():
        raise RuntimeError(
            "Fail-closed: an economic return interval is <= -100% (impossible compounded return)"
        )


def compute_transaction_cost(turnover: pd.Series, one_way_cost_bps: float) -> pd.Series:
    if one_way_cost_bps < 0:
        raise ValueError("one_way_cost_bps must be >= 0")
    return turnover * (float(one_way_cost_bps) / 10_000.0)


def compute_equity_curve(returns: pd.Series) -> pd.Series:
    """equity_t = Π_{i<=t}(1 + r_i), starting from an implicit equity of 1.0
    before the first return."""
    validate_return_series(returns)
    return (1.0 + returns).cumprod()


def compute_compounded_total_return(returns: pd.Series) -> float:
    """total_return = Π(1 + r_i) - 1 (compounding, NOT a simple sum)."""
    if len(returns) == 0:
        return 0.0
    equity = compute_equity_curve(returns)
    return float(equity.iloc[-1] - 1.0)


def compute_max_drawdown(returns: pd.Series) -> float:
    """max_drawdown = min_t(equity_t / running_peak_t - 1), computed from the
    NET compounded equity curve."""
    if len(returns) == 0:
        return 0.0
    equity = compute_equity_curve(returns)
    running_peak = equity.cummax()
    drawdown = equity / running_peak - 1.0
    return float(drawdown.min())


def compute_annualized_return(total_return: float, trading_days: int, annualization_days: int = 252) -> float:
    """annualized_return = (1 + total_return)^(annualization_days / trading_days) - 1.

    Fails closed rather than returning a complex number / NaN when
    `1 + total_return <= 0` (portfolio wiped out or worse) or when
    `trading_days <= 0`.
    """
    if trading_days <= 0:
        raise ValueError("trading_days must be > 0 to annualize a return")
    base = 1.0 + total_return
    if base <= 0.0:
        raise RuntimeError(
            "Fail-closed: cannot annualize a total_return <= -100% (would require complex-number math)"
        )
    return float(base ** (float(annualization_days) / float(trading_days)) - 1.0)


def compute_annualized_volatility(returns: pd.Series, annualization_days: int = 252) -> float:
    if len(returns) < 2:
        return 0.0
    return float(returns.std(ddof=DDOF) * np.sqrt(float(annualization_days)))


def compute_sharpe(
    returns: pd.Series,
    *,
    risk_free_rate_annual: float = 0.0,
    annualization_days: int = 252,
) -> Optional[float]:
    """sharpe = sqrt(annualization_days) * mean(excess) / std(excess), where
    excess_t = daily_net_return_t - daily_risk_free_return, and
    daily_risk_free_return = risk_free_rate_annual / annualization_days (a
    documented simple daily-rate convention, not compounded).

    Returns None (a truthful null, not 0.0 or NaN) when the excess-return
    series has zero volatility — a ratio against zero variance is undefined,
    not "no Sharpe."
    """
    if len(returns) < 2:
        return None
    daily_rf = float(risk_free_rate_annual) / float(annualization_days)
    excess = returns.astype(float) - daily_rf
    std = float(excess.std(ddof=DDOF))
    if std <= 0.0:
        return None
    mean = float(excess.mean())
    return float(np.sqrt(float(annualization_days)) * mean / std)


def compute_net_return_exact(gross_return: pd.Series, transaction_cost: pd.Series) -> pd.Series:
    """Exact per-interval wealth-compounding net return, RESEARCH-ECONOMIC-
    WALKFORWARD-01-REPAIR-01 (BLOCKER 1 compounding contract).

    `transaction_cost` is the fraction of pre-cost equity consumed by THIS
    row's execution turnover. Ordering within a row: the pre-existing
    position's gross return is realized first (equity *= 1+gross_return),
    then the row's execution cost is paid on the resulting equity
    (equity *= 1-transaction_cost) — multiplicatively commutative, so:

        net_return = (1 - transaction_cost) * (1 + gross_return) - 1
                   = gross_return - transaction_cost - transaction_cost * gross_return

    This is deliberately NOT the `gross_return - transaction_cost`
    approximation: that under-counts cost drag whenever gross_return and
    transaction_cost are both nonzero on the same row (e.g. the final
    in-fold interval return colliding with a force-flat exit cost)."""
    return (1.0 - transaction_cost.astype(float)) * (1.0 + gross_return.astype(float)) - 1.0


def compute_cost_drag(gross_total_return: float, net_total_return: float) -> float:
    return float(gross_total_return - net_total_return)
