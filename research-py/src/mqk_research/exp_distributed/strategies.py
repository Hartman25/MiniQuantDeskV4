from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, List, Tuple

import numpy as np
import pandas as pd


@dataclass(frozen=True)
class StrategyResult:
    metrics: Dict[str, Any]
    daily_returns: pd.DataFrame
    positions: pd.DataFrame
    trade_events: pd.DataFrame


def _pivot_close(data: pd.DataFrame) -> pd.DataFrame:
    pivot = (
        data.pivot_table(index="ts_utc", columns="symbol", values="close", aggfunc="last")
        .sort_index(kind="mergesort")
        .sort_index(axis=1)
    )
    pivot = pivot.ffill().dropna(how="all")
    if pivot.empty:
        raise ValueError("close-price pivot is empty after normalization")
    return pivot


def _metrics_from_returns(returns: pd.Series, positions: pd.DataFrame, trade_events: pd.DataFrame) -> Dict[str, Any]:
    series = returns.fillna(0.0).astype(float)
    n = int(series.shape[0])
    equity = (1.0 + series).cumprod()
    total_return = float(equity.iloc[-1] - 1.0) if n > 0 else 0.0
    annualized_return = float((1.0 + total_return) ** (252.0 / n) - 1.0) if n > 0 else 0.0
    annualized_vol = float(series.std(ddof=0) * np.sqrt(252.0)) if n > 1 else 0.0
    sharpe = float(annualized_return / annualized_vol) if annualized_vol > 0 else 0.0
    running_peak = equity.cummax()
    max_drawdown = float((equity / running_peak - 1.0).min()) if n > 0 else 0.0
    turnover = float(positions.fillna(0.0).diff().abs().sum(axis=1).sum())
    active_days = int((positions.fillna(0.0).abs().sum(axis=1) > 0).sum())
    win_rate = float((series > 0).sum() / n) if n > 0 else 0.0
    return {
        "trading_days": n,
        "active_days": active_days,
        "total_return": total_return,
        "annualized_return": annualized_return,
        "annualized_volatility": annualized_vol,
        "sharpe": sharpe,
        "max_drawdown": max_drawdown,
        "turnover": turnover,
        "trade_event_count": int(len(trade_events)),
        "final_equity": float(equity.iloc[-1]) if n > 0 else 1.0,
        "win_rate": win_rate,
    }


def _trade_events_from_targets(targets: pd.DataFrame) -> pd.DataFrame:
    previous = targets.shift(1).fillna(0.0)
    rows: List[Dict[str, Any]] = []
    for ts, current_row in targets.iterrows():
        prior_row = previous.loc[ts]
        for symbol in targets.columns:
            old_weight = float(prior_row[symbol])
            new_weight = float(current_row[symbol])
            if abs(old_weight - new_weight) < 1e-12:
                continue
            if old_weight == 0.0 and new_weight != 0.0:
                event_type = "enter"
            elif old_weight != 0.0 and new_weight == 0.0:
                event_type = "exit"
            else:
                event_type = "rebalance"
            rows.append(
                {
                    "ts_utc": ts.isoformat(),
                    "symbol": symbol,
                    "event_type": event_type,
                    "old_weight": old_weight,
                    "new_weight": new_weight,
                }
            )
    return pd.DataFrame(rows)


def _buy_hold(data: pd.DataFrame, params: Dict[str, Any]) -> StrategyResult:
    close = _pivot_close(data)
    returns = close.pct_change().fillna(0.0)
    weight = 1.0 / float(close.shape[1])
    targets = pd.DataFrame(weight, index=close.index, columns=close.columns)
    targets.iloc[0] = 0.0
    realized = targets.shift(1).fillna(0.0)
    portfolio_returns = (realized * returns).sum(axis=1)
    trade_events = _trade_events_from_targets(targets)
    metrics = _metrics_from_returns(portfolio_returns, realized, trade_events)
    metrics["strategy"] = "exp.buy_hold_v1"
    return StrategyResult(
        metrics=metrics,
        daily_returns=pd.DataFrame({"ts_utc": close.index.astype(str), "portfolio_return": portfolio_returns.values}),
        positions=realized.reset_index().rename(columns={"ts_utc": "ts_utc"}),
        trade_events=trade_events,
    )


def _cross_sectional_momentum(data: pd.DataFrame, params: Dict[str, Any]) -> StrategyResult:
    lookback_days = int(params.get("lookback_days", 20))
    top_n = int(params.get("top_n", 1))
    min_signal = float(params.get("min_signal", 0.0))
    rebalance_every = int(params.get("rebalance_every", 1))
    if lookback_days < 1:
        raise ValueError("lookback_days must be >= 1")
    if top_n < 1:
        raise ValueError("top_n must be >= 1")
    if rebalance_every < 1:
        raise ValueError("rebalance_every must be >= 1")

    close = _pivot_close(data)
    returns = close.pct_change().fillna(0.0)
    signal = close / close.shift(lookback_days) - 1.0
    targets = pd.DataFrame(0.0, index=close.index, columns=close.columns)
    previous_weights = pd.Series(0.0, index=close.columns)

    for idx, ts in enumerate(close.index):
        if idx < lookback_days:
            continue
        if (idx - lookback_days) % rebalance_every != 0:
            targets.loc[ts] = previous_weights
            continue
        row = signal.loc[ts].dropna()
        ranked: List[Tuple[str, float]] = sorted(
            ((symbol, float(value)) for symbol, value in row.items() if float(value) > min_signal),
            key=lambda item: (-item[1], item[0]),
        )
        selected = [symbol for symbol, _ in ranked[:top_n]]
        weights = pd.Series(0.0, index=close.columns)
        if selected:
            weight = 1.0 / float(len(selected))
            for symbol in selected:
                weights[symbol] = weight
        targets.loc[ts] = weights
        previous_weights = weights

    realized = targets.shift(1).fillna(0.0)
    portfolio_returns = (realized * returns).sum(axis=1)
    trade_events = _trade_events_from_targets(targets)
    metrics = _metrics_from_returns(portfolio_returns, realized, trade_events)
    metrics.update(
        {
            "strategy": "exp.cross_sectional_momentum_v1",
            "lookback_days": lookback_days,
            "top_n": top_n,
            "min_signal": min_signal,
            "rebalance_every": rebalance_every,
        }
    )
    return StrategyResult(
        metrics=metrics,
        daily_returns=pd.DataFrame({"ts_utc": close.index.astype(str), "portfolio_return": portfolio_returns.values}),
        positions=realized.reset_index().rename(columns={"index": "ts_utc"}),
        trade_events=trade_events,
    )


def run_strategy(strategy_id: str, data: pd.DataFrame, params: Dict[str, Any]) -> StrategyResult:
    if strategy_id == "exp.buy_hold_v1":
        return _buy_hold(data, params)
    if strategy_id == "exp.cross_sectional_momentum_v1":
        return _cross_sectional_momentum(data, params)
    raise ValueError(f"unsupported EXP strategy_id: {strategy_id}")
