\
from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, Optional

import numpy as np
import pandas as pd

from mqk_research.ml.util_hash import file_record, sha256_json


@dataclass(frozen=True)
class PerfSpec:
    """
    Performance metric spec for equity curves.

    Assumptions:
    - equity curve is time-ordered with a 'ts' UTC timestamp column and an equity column.
    - returns computed as pct change between consecutive points.
    """
    trading_days_per_year: int = 252

    def normalized(self) -> "PerfSpec":
        return PerfSpec(trading_days_per_year=int(self.trading_days_per_year))


def _to_utc(s: pd.Series) -> pd.Series:
    return pd.to_datetime(s, utc=True)


def _equity_metrics(eq: pd.DataFrame, equity_col: str, spec: PerfSpec) -> Dict[str, float]:
    spec = spec.normalized()
    e = eq.copy()
    e["ts"] = _to_utc(e["ts"])
    e = e.sort_values(["ts"], kind="mergesort").reset_index(drop=True)
    e[equity_col] = e[equity_col].astype(float)

    if len(e) < 2:
        start = float(e[equity_col].iloc[0]) if len(e) else float("nan")
        end = float(e[equity_col].iloc[-1]) if len(e) else float("nan")
        return {
            "start_equity": start,
            "end_equity": end,
            "cagr": float("nan"),
            "sharpe": float("nan"),
            "max_drawdown": float("nan"),
        }

    start = float(e[equity_col].iloc[0])
    end = float(e[equity_col].iloc[-1])

    rets = e[equity_col].pct_change().dropna().to_numpy(dtype=np.float64)
    mean = float(np.mean(rets)) if rets.size else float("nan")
    std = float(np.std(rets, ddof=1)) if rets.size > 1 else float("nan")

    dt0 = e["ts"].iloc[0]
    dt1 = e["ts"].iloc[-1]
    years = float((dt1 - dt0).total_seconds() / (365.25 * 24 * 3600))
    cagr = float((end / start) ** (1.0 / years) - 1.0) if (start > 0 and end > 0 and years > 0) else float("nan")

    sharpe = float((mean / std) * np.sqrt(spec.trading_days_per_year)) if (std and std > 0) else float("nan")

    curve = e[equity_col].to_numpy(dtype=np.float64)
    peaks = np.maximum.accumulate(curve)
    dd = (curve / peaks) - 1.0
    max_dd = float(np.min(dd)) if dd.size else float("nan")

    return {
        "start_equity": start,
        "end_equity": end,
        "cagr": cagr,
        "sharpe": sharpe,
        "max_drawdown": max_dd,
    }


def tax_aware_backtest_metrics(
    *,
    equity_curve_csv: Path,
    equity_after_tax_csv: Path,
    out_json: Path,
    spec: Optional[PerfSpec] = None,
) -> Path:
    spec = spec or PerfSpec()
    equity_curve_csv = Path(equity_curve_csv)
    equity_after_tax_csv = Path(equity_after_tax_csv)
    out_json = Path(out_json)

    pre = pd.read_csv(equity_curve_csv)
    post = pd.read_csv(equity_after_tax_csv)

    pre["ts"] = _to_utc(pre["ts"])
    post["ts"] = _to_utc(post["ts"])

    if "equity" not in pre.columns:
        raise ValueError("equity_curve.csv must contain 'equity'")
    if "equity_after_tax" not in post.columns:
        raise ValueError("equity_after_tax.csv must contain 'equity_after_tax'")

    merged = pd.merge(pre[["ts", "equity"]], post[["ts", "equity_after_tax"]], on="ts", how="inner")
    if merged.empty:
        raise RuntimeError("No overlapping timestamps between pre and after-tax curves")

    m_pre = _equity_metrics(merged, "equity", spec)
    m_post = _equity_metrics(merged, "equity_after_tax", spec)

    end_drag = float(m_pre["end_equity"] - m_post["end_equity"])
    end_drag_pct = float(end_drag / m_pre["end_equity"]) if m_pre["end_equity"] != 0.0 else float("nan")

    out = {
        "schema_version": "tax_aware_backtest_metrics_v1",
        "pre_tax": m_pre,
        "after_tax": m_post,
        "tax_drag": {
            "end_equity_drag": end_drag,
            "end_equity_drag_pct": end_drag_pct,
            "cagr_drag": float(m_pre["cagr"] - m_post["cagr"]),
            "sharpe_drag": float(m_pre["sharpe"] - m_post["sharpe"]),
            "max_drawdown_delta": float(m_post["max_drawdown"] - m_pre["max_drawdown"]),
        },
        "meta": {
            "inputs": {
                "equity_curve_csv": file_record(equity_curve_csv),
                "equity_after_tax_csv": file_record(equity_after_tax_csv),
            },
            "id": sha256_json({
                "equity_curve_csv": file_record(equity_curve_csv),
                "equity_after_tax_csv": file_record(equity_after_tax_csv),
                "schema_version": "tax_aware_backtest_metrics_v1",
            }),
        },
    }

    out_json.parent.mkdir(parents=True, exist_ok=True)
    out_json.write_text(json.dumps(out, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    return out_json


def main_metrics(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(
        prog="mqk-tax-metrics",
        description="Compute pre-tax vs after-tax backtest metrics (scaffold)",
    )
    ap.add_argument("--equity-curve", required=True)
    ap.add_argument("--equity-after-tax", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--trading-days", type=int, default=252)
    args = ap.parse_args(argv)

    spec = PerfSpec(trading_days_per_year=args.trading_days)
    out = tax_aware_backtest_metrics(
        equity_curve_csv=Path(args.equity_curve),
        equity_after_tax_csv=Path(args.equity_after_tax),
        out_json=Path(args.out),
        spec=spec,
    )
    print(f"OK metrics={out}")
    return 0
