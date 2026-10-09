from __future__ import annotations

import dataclasses
import json
import math
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
        if isinstance(self.capital_fraction_bps, bool):
            raise SizingReportError("capital_fraction_bps must not be a bool")
        if not isinstance(self.capital_fraction_bps, (int, float)) or not math.isfinite(float(self.capital_fraction_bps)):
            raise SizingReportError("capital_fraction_bps must be a finite number")
        if not float(self.capital_fraction_bps).is_integer():
            raise SizingReportError(f"capital_fraction_bps must be an integer, got {self.capital_fraction_bps!r}")
        bps = int(self.capital_fraction_bps)
        if bps < 0 or bps > 10_000:
            raise SizingReportError("capital_fraction_bps must be in [0, 10000]")

        if not isinstance(self.initial_strategy_capital_usd, (int, float)) or isinstance(self.initial_strategy_capital_usd, bool):
            raise SizingReportError("initial_strategy_capital_usd must be a number")
        if not math.isfinite(float(self.initial_strategy_capital_usd)):
            raise SizingReportError("initial_strategy_capital_usd must be finite (not NaN/Infinity)")
        if self.initial_strategy_capital_usd <= 0:
            raise SizingReportError("initial_strategy_capital_usd must be positive")

        if isinstance(self.trading_days_per_year, bool):
            raise SizingReportError("trading_days_per_year must not be a bool")
        if not isinstance(self.trading_days_per_year, (int, float)) or not math.isfinite(float(self.trading_days_per_year)):
            raise SizingReportError("trading_days_per_year must be a finite number")
        if not float(self.trading_days_per_year).is_integer() or int(self.trading_days_per_year) <= 0:
            raise SizingReportError("trading_days_per_year must be a positive integer")

        return SizingReportSpec(
            initial_strategy_capital_usd=float(self.initial_strategy_capital_usd),
            capital_fraction_bps=bps,
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
        def _finite_positive(label: str, value: Optional[float], *, required: bool) -> None:
            if value is None:
                if required:
                    raise SizingReportError(f"trade {self.trade_id}: missing/invalid {label} (instrument valuation required)")
                return
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(float(value)):
                raise SizingReportError(f"trade {self.trade_id}: {label} must be a finite number, got {value!r}")
            if value <= 0:
                raise SizingReportError(f"trade {self.trade_id}: {label} must be positive, got {value!r}")

        def _finite(label: str, value: Optional[float]) -> None:
            if value is None:
                return
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(float(value)):
                raise SizingReportError(f"trade {self.trade_id}: {label} must be a finite number, got {value!r}")

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

        _finite_positive("qty", self.qty, required=True)
        _finite_positive("entry_price", self.entry_price, required=True)
        _finite_positive("exit_price", self.exit_price, required=False)
        _finite_positive("stop_price", self.stop_price, required=False)
        _finite("costs_usd", self.costs_usd)

        if self.exit_ts is not None and self.exit_price is None:
            raise SizingReportError(f"trade {self.trade_id}: exit_ts present but exit_price missing/invalid")

        try:
            entry_ts_parsed = pd.Timestamp(self.entry_ts)
        except (ValueError, TypeError) as exc:
            raise SizingReportError(f"trade {self.trade_id}: entry_ts is not a parseable timestamp: {self.entry_ts!r}") from exc
        if pd.isna(entry_ts_parsed):
            raise SizingReportError(f"trade {self.trade_id}: entry_ts parsed to NaT: {self.entry_ts!r}")

        if self.exit_ts is not None:
            try:
                exit_ts_parsed = pd.Timestamp(self.exit_ts)
            except (ValueError, TypeError) as exc:
                raise SizingReportError(f"trade {self.trade_id}: exit_ts is not a parseable timestamp: {self.exit_ts!r}") from exc
            if pd.isna(exit_ts_parsed):
                raise SizingReportError(f"trade {self.trade_id}: exit_ts parsed to NaT: {self.exit_ts!r}")
            if exit_ts_parsed < entry_ts_parsed:
                raise SizingReportError(
                    f"trade {self.trade_id}: exit_ts ({self.exit_ts!r}) is before entry_ts ({self.entry_ts!r})"
                )

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
    if len(eq) == 0:
        raise SizingReportError(f"{label} equity curve has zero rows — at least one observation is required")
    equity_values = eq["equity"].to_numpy(dtype=np.float64)
    if not np.all(np.isfinite(equity_values)):
        raise SizingReportError(f"{label} equity curve contains NaN/Infinity — every observation must be finite")


def _time_underwater_seconds(eq: pd.DataFrame) -> Optional[float]:
    """Longest span between a new equity peak and the point equity recovers to
    (or exceeds) that peak — measured through the recovery observation
    itself, not just the last strictly-underwater sample before it.
    Still-underwater-at-series-end counts up to the last observation;
    recovery is never assumed."""
    if len(eq) < 2:
        return None
    e = eq.sort_values("ts", kind="mergesort").reset_index(drop=True)
    curve = e["equity"].to_numpy(dtype=np.float64)
    ts = pd.to_datetime(e["ts"], utc=True).to_numpy()

    peak = curve[0]
    peak_ts = ts[0]
    was_underwater = False
    worst_seconds = 0.0
    for i in range(1, len(curve)):
        if curve[i] >= peak:
            if was_underwater:
                # Recovery instant: the full episode ran from the prior peak
                # to THIS observation — not just the last strictly-underwater
                # sample seen before it.
                worst_seconds = max(worst_seconds, float((ts[i] - peak_ts) / np.timedelta64(1, "s")))
            peak = curve[i]
            peak_ts = ts[i]
            was_underwater = False
        else:
            was_underwater = True
            worst_seconds = max(worst_seconds, float((ts[i] - peak_ts) / np.timedelta64(1, "s")))
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
    """Max concurrent open LOGICAL positions (partial fills of the same
    position collapse to one, via `partial_fill_of`), and the largest
    single-position share of total open notional *at the moment(s)
    concurrency peaks* — not the global max share, which is trivially 1.0
    whenever a lone position ever exists (e.g. at the start of the very
    first trade).

    A logical position opens at the earliest fill's entry_ts. It only
    closes if EVERY fill in the group is closed, at the latest fill's
    exit_ts; if any fill is still open, the whole logical position is
    treated as open (no invented partial-close instant)."""
    if not trades:
        return {"max_concurrent_positions": 0, "concentration_pct_at_max_concurrency": None}

    groups: Dict[str, List[TradeRecord]] = {}
    for t in trades:
        key = t.partial_fill_of or t.trade_id
        groups.setdefault(key, []).append(t)

    events = []
    for key, fills in groups.items():
        notional = sum(f.notional_usd for f in fills)
        open_ts = min(pd.Timestamp(f.entry_ts) for f in fills)
        events.append((open_ts, 1, key, notional))  # open sorts after close at same ts
        if all(f.is_closed for f in fills):
            close_ts = max(pd.Timestamp(f.exit_ts) for f in fills)
            events.append((close_ts, 0, key, notional))  # close
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


def _sanitize_metrics(metrics: Dict[str, float]) -> Dict[str, Any]:
    """Legitimate undefined CAGR/Sharpe (insufficient history, zero
    variance, non-positive start/end equity) must surface as an explicit
    not_evaluable marker, never as a raw JSON NaN/Infinity token and never
    as a fabricated zero."""
    out: Dict[str, Any] = {}
    for k, v in metrics.items():
        if isinstance(v, float) and not math.isfinite(v):
            out[k] = {
                "truth_state": "not_evaluable",
                "reason": "undefined for this equity curve (insufficient history, zero variance, or non-positive equity)",
            }
        else:
            out[k] = v
    return out


def _trade_content_hash(trades: List[TradeRecord]) -> Optional[str]:
    """Binds report identity to the RAW per-trade evidence (every field of
    every trade), not just the summarized aggregate output — two different
    trade histories that coincidentally produce identical aggregates must
    not collapse to the same report_id."""
    if not trades:
        return None
    content = sorted((dataclasses.asdict(t) for t in trades), key=lambda d: d["trade_id"])
    return sha256_json(content)


def _equity_curve_content_hash(eq: Optional[pd.DataFrame]) -> Optional[str]:
    if eq is None or not len(eq):
        return None
    records = [{"ts": str(row["ts"]), "equity": float(row["equity"])} for _, row in eq.iterrows()]
    return sha256_json(records)


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
    strategy_metrics = _sanitize_metrics(_equity_metrics(strategy_equity, "equity", perf_spec))
    strategy_time_underwater_s = _time_underwater_seconds(strategy_equity)
    strategy_insolvent = bool(np.any(strategy_equity["equity"].to_numpy(dtype=np.float64) <= 0.0))

    if account_equity is not None and len(account_equity):
        _require_equity_columns(account_equity, label="account")
        account_metrics = _sanitize_metrics(_equity_metrics(account_equity, "equity", perf_spec))
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
        "input_evidence_hash": {
            "strategy_equity": _equity_curve_content_hash(strategy_equity),
            "account_equity": _equity_curve_content_hash(account_equity),
            "trades": _trade_content_hash(trades),
        },
        "correlated_exposure": {
            "truth_state": "not_evaluable",
            "reason": "requires a multi-instrument joint price history; not computed from a single-strategy trade list",
        },
    }
    report["report_id"] = sha256_json({k: v for k, v in report.items() if k != "report_id"})
    return report


_REQUIRED_TRADE_COLUMNS = (
    "trade_id", "symbol", "side", "qty", "unit", "entry_ts", "entry_price",
    "currency", "multiplier", "costs_usd",
)


def _read_equity_csv(path: Path, *, label: str) -> pd.DataFrame:
    if not path.exists():
        raise SizingReportError(f"{label} equity CSV does not exist: {path}")
    try:
        return pd.read_csv(path)
    except Exception as exc:  # pandas raises several distinct error types for malformed CSV
        raise SizingReportError(f"{label} equity CSV is unreadable/malformed: {path}: {exc}") from exc


def _load_trades_csv(trades_csv: Path) -> List[TradeRecord]:
    """An explicitly supplied trades_csv must exist and be well-formed; a
    missing/unreadable file is always an error, never a silent empty trade
    list. (Omitting --trades entirely, i.e. `trades_csv=None` upstream, is
    the only valid way to declare "no trades input".) Every required
    economic field must be an explicit CSV column with an explicit value
    per row -- no unit/currency/multiplier/cost defaulting in this generic
    adapter. An explicit `0` for costs_usd is preserved exactly as given."""
    if not trades_csv.exists():
        raise SizingReportError(f"trades_csv does not exist: {trades_csv}")
    try:
        tdf = pd.read_csv(trades_csv)
    except Exception as exc:
        raise SizingReportError(f"trades_csv is unreadable/malformed: {trades_csv}: {exc}") from exc

    missing_cols = [c for c in _REQUIRED_TRADE_COLUMNS if c not in tdf.columns]
    if missing_cols:
        raise SizingReportError(f"trades_csv is missing required columns: {missing_cols}")

    trades: List[TradeRecord] = []
    for idx, row in tdf.iterrows():
        def _req(col: str) -> Any:
            v = row[col]
            # pandas represents a blank CSV cell as either None or a float
            # NaN depending on column dtype inference (a sparse/single-row
            # column infers float64, not object) -- both mean "missing" and
            # must both be refused, never silently cast to the string "nan".
            if v is None or (isinstance(v, float) and pd.isna(v)):
                raise SizingReportError(f"trades_csv row {idx}: missing value for required column {col!r}")
            return v

        def _opt(col: str) -> Optional[str]:
            if col not in tdf.columns:
                return None
            v = row[col]
            if v is None or v == "" or (isinstance(v, float) and pd.isna(v)):
                return None
            return str(v)

        try:
            trades.append(
                TradeRecord(
                    trade_id=str(_req("trade_id")),
                    symbol=str(_req("symbol")),
                    side=str(_req("side")),
                    qty=float(_req("qty")),
                    unit=str(_req("unit")),
                    entry_ts=str(_req("entry_ts")),
                    entry_price=float(_req("entry_price")),
                    currency=str(_req("currency")),
                    multiplier=float(_req("multiplier")),
                    costs_usd=float(_req("costs_usd")),
                    exit_ts=_opt("exit_ts"),
                    exit_price=(float(row["exit_price"]) if _opt("exit_price") is not None else None),
                    stop_price=(float(row["stop_price"]) if _opt("stop_price") is not None else None),
                    partial_fill_of=_opt("partial_fill_of"),
                )
            )
        except SizingReportError:
            raise
        except (TypeError, ValueError) as exc:
            raise SizingReportError(f"trades_csv row {idx}: malformed trade data: {exc}") from exc

    return trades  # an existing, well-formed, but zero-row file is a valid explicit "no trades" declaration


def write_sizing_drawdown_report(
    *,
    strategy_equity_csv: Path,
    account_equity_csv: Optional[Path],
    trades_csv: Optional[Path],
    out_json: Path,
    spec: Optional[SizingReportSpec] = None,
) -> Path:
    """File-driving wrapper matching the existing reporting/tax CLI convention."""
    strategy_equity = _read_equity_csv(Path(strategy_equity_csv), label="strategy")
    account_equity = _read_equity_csv(Path(account_equity_csv), label="account") if account_equity_csv else None

    # trades_csv omitted entirely (None) is a valid explicit "no trades"
    # declaration; trades_csv given but missing/unreadable/malformed is an
    # error (B1) -- the two must never be conflated.
    trades: List[TradeRecord] = _load_trades_csv(Path(trades_csv)) if trades_csv is not None else []

    report = compute_sizing_drawdown_report(
        strategy_equity=strategy_equity,
        account_equity=account_equity,
        trades=trades,
        spec=spec,
    )

    out_json = Path(out_json)
    out_json.parent.mkdir(parents=True, exist_ok=True)
    # allow_nan=False: defense in depth -- any raw NaN/Infinity that reached
    # this point despite _sanitize_metrics is a bug, and must raise loudly
    # rather than write non-standard JSON silently.
    out_json.write_text(json.dumps(report, sort_keys=True, separators=(",", ":"), allow_nan=False), encoding="utf-8")
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
