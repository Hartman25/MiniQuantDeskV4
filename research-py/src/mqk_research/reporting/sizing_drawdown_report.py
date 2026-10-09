from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional

import numpy as np
import pandas as pd

from mqk_research.ml.util_hash import sha256_json
# Reuse: single source of truth for the CAGR/Sharpe/max-drawdown formula,
# already shared with tax-drag reporting. Not re-derived here.
from mqk_research.tax.metrics import PerfSpec, _equity_metrics

# Frozen M1 reference values (see MiniQuantDesk_Master_Patch_Ledger_v2.md /
# M1_SYSTEM_CLOSURE_01.md). Illustrative defaults only — never used to alter
# production sizing/risk policy.
M1_INITIAL_STRATEGY_CAPITAL_USD = 100_000.0
M1_BASELINE_CAPITAL_FRACTION_BPS = 1000  # 10%
M1_STRESS_CAPITAL_FRACTION_BPS = 500  # 5%

_SUPPORTED_CURRENCY = "USD"
_SUPPORTED_MULTIPLIER = 1.0
_SUPPORTED_UNIT = "shares"


class SizingReportError(ValueError):
    """Fail-closed refusal: unsupported unit/currency/multiplier, or a
    required valuation input is missing. Never silently defaulted."""


@dataclass(frozen=True)
class SizingReportSpec:
    """
    Research-only, illustrative capital-fraction sizing/drawdown report spec.

    Distinguishes strategy-budget economics from account-equity economics.
    Does not read or alter any production sizing/risk policy.
    """

    initial_strategy_capital_usd: float = M1_INITIAL_STRATEGY_CAPITAL_USD
    capital_fraction_bps: int = M1_BASELINE_CAPITAL_FRACTION_BPS
    compounding: bool = False
    trading_days_per_year: int = 252

    def normalized(self) -> "SizingReportSpec":
        if self.initial_strategy_capital_usd <= 0:
            raise SizingReportError("initial_strategy_capital_usd must be positive")
        if self.capital_fraction_bps < 0 or self.capital_fraction_bps > 10_000:
            raise SizingReportError("capital_fraction_bps must be in [0, 10000]")
        return SizingReportSpec(
            initial_strategy_capital_usd=float(self.initial_strategy_capital_usd),
            capital_fraction_bps=int(self.capital_fraction_bps),
            compounding=bool(self.compounding),
            trading_days_per_year=int(self.trading_days_per_year),
        )

    @property
    def capital_fraction(self) -> float:
        return self.capital_fraction_bps / 10_000.0

    def strategy_budget(self, *, strategy_equity_at_period_start: Optional[float] = None) -> float:
        """Fixed-capital basis by default; compounds off the strategy's own
        current equity only when `compounding` is True and a value is given."""
        base = (
            strategy_equity_at_period_start
            if (self.compounding and strategy_equity_at_period_start is not None)
            else self.initial_strategy_capital_usd
        )
        return base * self.capital_fraction


@dataclass(frozen=True)
class TradeRecord:
    """One fill or partial fill. Rows sharing `partial_fill_of` are the same
    logical position; their notionals are aggregated once, not double-counted."""

    trade_id: str
    symbol: str
    side: str  # "long" | "short"
    qty: float
    unit: str
    entry_ts: str
    entry_price: float
    currency: str
    multiplier: float
    costs_usd: float
    exit_ts: Optional[str] = None
    exit_price: Optional[float] = None
    stop_price: Optional[float] = None
    partial_fill_of: Optional[str] = None

    def validate(self) -> None:
        if self.unit != _SUPPORTED_UNIT:
            raise SizingReportError(
                f"trade {self.trade_id}: unsupported quantity unit {self.unit!r}; "
                f"only {_SUPPORTED_UNIT!r} is supported"
            )
        if self.currency != _SUPPORTED_CURRENCY:
            raise SizingReportError(
                f"trade {self.trade_id}: unsupported currency {self.currency!r}; "
                f"only {_SUPPORTED_CURRENCY!r} is supported (no conversion)"
            )
        if self.multiplier != _SUPPORTED_MULTIPLIER:
            raise SizingReportError(
                f"trade {self.trade_id}: unsupported instrument multiplier {self.multiplier!r}; "
                f"only {_SUPPORTED_MULTIPLIER!r} is supported"
            )
        if self.side not in ("long", "short"):
            raise SizingReportError(f"trade {self.trade_id}: unsupported side {self.side!r}")
        if self.qty is None or self.qty <= 0:
            raise SizingReportError(f"trade {self.trade_id}: qty must be positive (side encodes direction)")
        if self.entry_price is None or self.entry_price <= 0:
            raise SizingReportError(f"trade {self.trade_id}: missing/invalid entry_price (instrument valuation required)")
        if self.exit_ts is not None and (self.exit_price is None or self.exit_price <= 0):
            raise SizingReportError(f"trade {self.trade_id}: exit_ts present but exit_price missing/invalid")

    @property
    def is_closed(self) -> bool:
        return self.exit_ts is not None

    @property
    def notional_usd(self) -> float:
        return float(self.qty * self.entry_price * self.multiplier)

    @property
    def gross_pnl_usd(self) -> Optional[float]:
        if not self.is_closed:
            return None
        direction = 1.0 if self.side == "long" else -1.0
        return float(direction * (self.exit_price - self.entry_price) * self.qty * self.multiplier)

    @property
    def net_pnl_usd(self) -> Optional[float]:
        gross = self.gross_pnl_usd
        return None if gross is None else float(gross - self.costs_usd)

    @property
    def risk_at_stop_usd(self) -> Optional[float]:
        if self.stop_price is None:
            return None
        direction = 1.0 if self.side == "long" else -1.0
        return float(direction * (self.entry_price - self.stop_price) * self.qty * self.multiplier)

    @property
    def duration_seconds(self) -> Optional[float]:
        if not self.is_closed:
            return None
        return float((pd.Timestamp(self.exit_ts) - pd.Timestamp(self.entry_ts)).total_seconds())


def _require_equity_columns(eq: pd.DataFrame, *, label: str) -> None:
    if "ts" not in eq.columns or "equity" not in eq.columns:
        raise SizingReportError(f"{label} equity curve must contain 'ts' and 'equity' columns")


def _time_underwater_seconds(eq: pd.DataFrame) -> Optional[float]:
    """Longest span between a new equity peak and the point equity recovers to
    (or exceeds) that peak. Still-underwater-at-series-end counts up to the
    last observation; recovery is never assumed."""
    if len(eq) < 2:
        return None
    e = eq.sort_values("ts", kind="mergesort").reset_index(drop=True)
    curve = e["equity"].to_numpy(dtype=np.float64)
    ts = pd.to_datetime(e["ts"], utc=True).to_numpy()

    peak = curve[0]
    peak_ts = ts[0]
    worst_seconds = 0.0
    for i in range(1, len(curve)):
        if curve[i] >= peak:
            peak = curve[i]
            peak_ts = ts[i]
        else:
            underwater = (ts[i] - peak_ts) / np.timedelta64(1, "s")
            worst_seconds = max(worst_seconds, float(underwater))
    return worst_seconds


def _aggregate_partial_fills(trades: List[TradeRecord]) -> Dict[str, float]:
    """logical position id -> aggregated notional, so partial fills of the
    same position are summed once rather than counted per fill row."""
    agg: Dict[str, float] = {}
    for t in trades:
        key = t.partial_fill_of or t.trade_id
        agg[key] = agg.get(key, 0.0) + t.notional_usd
    return agg


def _concentration_and_concurrency(trades: List[TradeRecord]) -> Dict[str, Any]:
    """Max concurrent open positions, and the largest single-position share
    of total open notional *at the moment(s) concurrency peaks* — not the
    global max share, which is trivially 1.0 whenever a lone position ever
    exists (e.g. at the start of the very first trade)."""
    if not trades:
        return {"max_concurrent_positions": 0, "concentration_pct_at_max_concurrency": None}

    events = []
    for t in trades:
        events.append((pd.Timestamp(t.entry_ts), 1, t.trade_id, t.notional_usd))  # open sorts after close at same ts
        if t.exit_ts:
            events.append((pd.Timestamp(t.exit_ts), 0, t.trade_id, t.notional_usd))  # close
    events.sort(key=lambda e: (e[0], e[1]))

    open_notional: Dict[str, float] = {}
    snapshots: List[tuple] = []
    for _, kind, tid, notional in events:
        if kind == 1:
            open_notional[tid] = notional
        else:
            open_notional.pop(tid, None)
        total = sum(open_notional.values())
        share = (max(open_notional.values()) / total) if total > 0 else None
        snapshots.append((len(open_notional), share))

    max_concurrent = max((c for c, _ in snapshots), default=0)
    shares_at_max = [s for c, s in snapshots if c == max_concurrent and s is not None]
    concentration_at_max = max(shares_at_max) if shares_at_max else None

    return {
        "max_concurrent_positions": max_concurrent,
        "concentration_pct_at_max_concurrency": float(concentration_at_max) if concentration_at_max is not None else None,
    }


def compute_sizing_drawdown_report(
    *,
    strategy_equity: pd.DataFrame,
    account_equity: Optional[pd.DataFrame],
    trades: List[TradeRecord],
    spec: Optional[SizingReportSpec] = None,
) -> Dict[str, Any]:
    """
    Pure computation (no I/O, no production sizing/risk authority touched):
    the required sizing/drawdown distinctions over an explicit strategy
    equity curve, an optional separate account equity curve, and an
    explicit trade list.
    """
    spec = (spec or SizingReportSpec()).normalized()
    for t in trades:
        t.validate()

    _require_equity_columns(strategy_equity, label="strategy")
    perf_spec = PerfSpec(trading_days_per_year=spec.trading_days_per_year)
    strategy_metrics = _equity_metrics(strategy_equity, "equity", perf_spec)
    strategy_time_underwater_s = _time_underwater_seconds(strategy_equity)
    strategy_insolvent = bool(np.any(strategy_equity["equity"].to_numpy(dtype=np.float64) <= 0.0)) if len(strategy_equity) else False

    if account_equity is not None and len(account_equity):
        _require_equity_columns(account_equity, label="account")
        account_metrics = _equity_metrics(account_equity, "equity", perf_spec)
        account_block: Dict[str, Any] = {
            "equity_metrics": account_metrics,
            "time_underwater_seconds": _time_underwater_seconds(account_equity),
            "insolvent": bool(np.any(account_equity["equity"].to_numpy(dtype=np.float64) <= 0.0)),
        }
    else:
        account_block = {"truth_state": "not_provided"}

    closed = [t for t in trades if t.is_closed]
    open_count = len(trades) - len(closed)
    realized_gross_total = float(sum(t.gross_pnl_usd for t in closed)) if closed else None
    realized_net_total = float(sum(t.net_pnl_usd for t in closed)) if closed else None
    unrealized_pnl: Any = 0.0 if open_count == 0 else {
        "truth_state": "not_evaluable",
        "reason": "no mark price supplied for open positions",
    }
    costs_exceed_gross_profit = bool(
        realized_gross_total is not None
        and realized_net_total is not None
        and realized_gross_total > 0
        and realized_net_total < 0
    )

    aggregated_positions = _aggregate_partial_fills(trades)
    concentration = _concentration_and_concurrency(trades)
    durations = [t.duration_seconds for t in closed if t.duration_seconds is not None]
    total_notional_traded = float(sum(t.notional_usd for t in trades))
    budget = spec.strategy_budget()
    turnover_vs_strategy_budget = (total_notional_traded / budget) if budget > 0 else None

    report: Dict[str, Any] = {
        "schema_version": "sizing_drawdown_report_v1",
        "spec": {
            "initial_strategy_capital_usd": spec.initial_strategy_capital_usd,
            "capital_fraction_bps": spec.capital_fraction_bps,
            "capital_fraction": spec.capital_fraction,
            "compounding": spec.compounding,
        },
        "strategy_budget_usd": budget,
        "strategy": {
            "equity_metrics": strategy_metrics,
            "time_underwater_seconds": strategy_time_underwater_s,
            "insolvent": strategy_insolvent,
            "has_trades": len(trades) > 0,
        },
        "account": account_block,
        "trades": {
            "count": len(trades),
            "closed_count": len(closed),
            "open_count": open_count,
            "realized_gross_pnl_usd_total": realized_gross_total,
            "realized_net_pnl_usd_total": realized_net_total,
            "unrealized_pnl_usd": unrealized_pnl,
            "costs_usd_total": float(sum(t.costs_usd for t in trades)),
            "costs_exceed_gross_profit": costs_exceed_gross_profit,
            "avg_duration_seconds": float(np.mean(durations)) if durations else None,
            "total_notional_traded_usd": total_notional_traded,
            "turnover_vs_strategy_budget": turnover_vs_strategy_budget,
            "turnover_note": "computed against the fixed initial strategy budget regardless of the compounding flag",
            "logical_position_count": len(aggregated_positions),
            "max_concurrent_positions": concentration["max_concurrent_positions"],
            "concentration_pct_at_max_concurrency": concentration["concentration_pct_at_max_concurrency"],
        },
        "correlated_exposure": {
            "truth_state": "not_evaluable",
            "reason": "requires a multi-instrument joint price history; not computed from a single-strategy trade list",
        },
    }
    report["report_id"] = sha256_json({k: v for k, v in report.items() if k != "report_id"})
    return report


def write_sizing_drawdown_report(
    *,
    strategy_equity_csv: Path,
    account_equity_csv: Optional[Path],
    trades_csv: Optional[Path],
    out_json: Path,
    spec: Optional[SizingReportSpec] = None,
) -> Path:
    """File-driving wrapper matching the existing reporting/tax CLI convention."""
    strategy_equity = pd.read_csv(Path(strategy_equity_csv))
    account_equity = pd.read_csv(Path(account_equity_csv)) if account_equity_csv else None

    trades: List[TradeRecord] = []
    if trades_csv is not None and Path(trades_csv).exists():
        tdf = pd.read_csv(Path(trades_csv))
        tdf = tdf.where(pd.notnull(tdf), None)
        for _, row in tdf.iterrows():
            def _opt(col: str) -> Optional[str]:
                v = row[col] if col in tdf.columns else None
                return None if v in (None, "") else str(v)

            trades.append(
                TradeRecord(
                    trade_id=str(row["trade_id"]),
                    symbol=str(row["symbol"]),
                    side=str(row["side"]),
                    qty=float(row["qty"]),
                    unit=str(row["unit"]) if row.get("unit") not in (None, "") else _SUPPORTED_UNIT,
                    entry_ts=str(row["entry_ts"]),
                    entry_price=float(row["entry_price"]),
                    currency=str(row["currency"]) if row.get("currency") not in (None, "") else _SUPPORTED_CURRENCY,
                    multiplier=float(row["multiplier"]) if row.get("multiplier") not in (None, "") else _SUPPORTED_MULTIPLIER,
                    costs_usd=float(row["costs_usd"]) if row.get("costs_usd") not in (None, "") else 0.0,
                    exit_ts=_opt("exit_ts"),
                    exit_price=(float(row["exit_price"]) if _opt("exit_price") is not None else None),
                    stop_price=(float(row["stop_price"]) if _opt("stop_price") is not None else None),
                    partial_fill_of=_opt("partial_fill_of"),
                )
            )

    report = compute_sizing_drawdown_report(
        strategy_equity=strategy_equity,
        account_equity=account_equity,
        trades=trades,
        spec=spec,
    )

    out_json = Path(out_json)
    out_json.parent.mkdir(parents=True, exist_ok=True)
    out_json.write_text(json.dumps(report, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    return out_json


def main_sizing_drawdown(argv: Optional[List[str]] = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(
        prog="mqk-sizing-drawdown-report",
        description="Research-only capital-fraction sizing/drawdown report (illustrative; not a trading signal).",
    )
    ap.add_argument("--strategy-equity", required=True)
    ap.add_argument("--account-equity", default=None)
    ap.add_argument("--trades", default=None)
    ap.add_argument("--out", required=True)
    ap.add_argument("--capital-fraction-bps", type=int, default=M1_BASELINE_CAPITAL_FRACTION_BPS)
    ap.add_argument("--initial-capital-usd", type=float, default=M1_INITIAL_STRATEGY_CAPITAL_USD)
    ap.add_argument("--compounding", action="store_true")
    args = ap.parse_args(argv)

    spec = SizingReportSpec(
        initial_strategy_capital_usd=args.initial_capital_usd,
        capital_fraction_bps=args.capital_fraction_bps,
        compounding=args.compounding,
    )
    out = write_sizing_drawdown_report(
        strategy_equity_csv=Path(args.strategy_equity),
        account_equity_csv=Path(args.account_equity) if args.account_equity else None,
        trades_csv=Path(args.trades) if args.trades else None,
        out_json=Path(args.out),
        spec=spec,
    )
    print(f"OK sizing_drawdown_report={out}")
    return 0
