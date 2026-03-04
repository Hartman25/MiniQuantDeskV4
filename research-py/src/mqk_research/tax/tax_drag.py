from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, Optional, Tuple

import numpy as np
import pandas as pd

from mqk_research.ml.util_hash import file_record, sha256_json
from mqk_research.tax.contracts import EquityCurveCsvContractV1


@dataclass(frozen=True)
class TaxRates:
    short_rate: float = 0.30
    long_rate: float = 0.15

    def normalized(self) -> "TaxRates":
        return TaxRates(short_rate=float(self.short_rate), long_rate=float(self.long_rate))


@dataclass(frozen=True)
class TaxDragSpec:
    mode: str = "annual_settlement"  # annual_settlement | immediate_withholding
    allow_loss_offset: bool = True
    long_term_days: int = 365

    def normalized(self) -> "TaxDragSpec":
        m = str(self.mode).strip().lower()
        if m not in ("annual_settlement", "immediate_withholding"):
            raise ValueError("mode must be annual_settlement or immediate_withholding")
        return TaxDragSpec(
            mode=m,
            allow_loss_offset=bool(self.allow_loss_offset),
            long_term_days=int(self.long_term_days),
        )


def _to_utc(s: pd.Series) -> pd.Series:
    return pd.to_datetime(s, utc=True)


def tax_summary_from_realized(realized: pd.DataFrame, *, rates: TaxRates) -> Dict[str, object]:
    rates = rates.normalized()
    df = realized.copy()
    df["close_ts"] = _to_utc(df["close_ts"])
    df["year"] = df["close_ts"].dt.year.astype(int)
    if "term" not in df.columns:
        raise ValueError("realized must include 'term' column")
    df["realized_gain"] = df["realized_gain"].astype(float)

    out: Dict[str, object] = {"years": {}, "totals": {}}
    totals_tax = 0.0
    totals_gain = 0.0

    for y, g in df.groupby("year", sort=True):
        short_gain = float(g.loc[g["term"] == "short", "realized_gain"].sum())
        long_gain = float(g.loc[g["term"] == "long", "realized_gain"].sum())

        short_tax = max(0.0, short_gain) * rates.short_rate
        long_tax = max(0.0, long_gain) * rates.long_rate
        tax = float(short_tax + long_tax)
        gain = float(short_gain + long_gain)

        out["years"][str(int(y))] = {
            "short_gain": short_gain,
            "long_gain": long_gain,
            "tax_estimate": tax,
        }
        totals_tax += tax
        totals_gain += gain

    out["totals"] = {
        "realized_gain": float(totals_gain),
        "tax_estimate": float(totals_tax),
        "after_tax_realized": float(totals_gain - totals_tax),
    }
    return out


def apply_tax_drag_to_equity_curve(
    equity_curve: pd.DataFrame,
    realized: pd.DataFrame,
    *,
    rates: TaxRates,
    spec: TaxDragSpec,
) -> pd.DataFrame:
    rates = rates.normalized()
    spec = spec.normalized()

    contract = EquityCurveCsvContractV1().normalized()
    missing = [c for c in contract.required_columns if c not in equity_curve.columns]
    if missing:
        raise ValueError(f"equity_curve missing required columns: {missing}")

    eq = equity_curve.copy()
    eq["ts"] = _to_utc(eq["ts"])
    eq["equity"] = eq["equity"].astype(float)
    eq = eq.sort_values(["ts"], kind="mergesort").reset_index(drop=True)

    rz = realized.copy()
    rz["close_ts"] = _to_utc(rz["close_ts"])
    rz["realized_gain"] = rz["realized_gain"].astype(float)
    if "term" not in rz.columns:
        raise ValueError("realized missing 'term' column")

    events = []  # list of (ts, tax_amount)

    if spec.mode == "immediate_withholding":
        for _, r in rz.iterrows():
            gain = float(r["realized_gain"])
            if gain <= 0.0:
                continue
            rate = rates.long_rate if str(r["term"]) == "long" else rates.short_rate
            events.append((r["close_ts"], float(gain * rate)))
    else:
        rz["year"] = rz["close_ts"].dt.year.astype(int)
        for y, g in rz.groupby("year", sort=True):
            short_gain = float(g.loc[g["term"] == "short", "realized_gain"].sum())
            long_gain = float(g.loc[g["term"] == "long", "realized_gain"].sum())

            if not spec.allow_loss_offset:
                short_gain = max(0.0, short_gain)
                long_gain = max(0.0, long_gain)

            tax = max(0.0, short_gain) * rates.short_rate + max(0.0, long_gain) * rates.long_rate
            tax = float(tax)
            if tax <= 0.0:
                continue

            ts = pd.Timestamp(year=int(y), month=12, day=31, hour=23, minute=59, second=59, tz="UTC")
            events.append((ts, tax))

    events.sort(key=lambda x: x[0])

    tax_cum = np.zeros(len(eq), dtype=np.float64)
    ts_arr = eq["ts"].to_numpy()
    for ts, amount in events:
        idx = int(np.searchsorted(ts_arr, np.datetime64(ts), side="left"))
        if idx < len(tax_cum):
            tax_cum[idx:] += float(amount)

    eq["tax_cumulative"] = tax_cum
    eq["equity_after_tax"] = eq["equity"] - eq["tax_cumulative"]
    return eq


def run_tax_drag_simulation(
    *,
    realized_trades_csv: Path,
    out_dir: Path,
    equity_curve_csv: Optional[Path] = None,
    rates: Optional[TaxRates] = None,
    spec: Optional[TaxDragSpec] = None,
) -> Tuple[Path, Optional[Path]]:
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    rates = rates or TaxRates()
    spec = spec or TaxDragSpec()

    realized_trades_csv = Path(realized_trades_csv)
    realized = pd.read_csv(realized_trades_csv)

    summary = tax_summary_from_realized(realized, rates=rates)
    summary_path = out_dir / "tax_summary.json"
    summary_path.write_text(json.dumps(summary, sort_keys=True, separators=(",", ":")), encoding="utf-8")

    eq_out_path: Optional[Path] = None
    if equity_curve_csv is not None:
        equity_curve_csv = Path(equity_curve_csv)
        eq = pd.read_csv(equity_curve_csv)
        eq2 = apply_tax_drag_to_equity_curve(eq, realized, rates=rates, spec=spec)

        eq_out_path = out_dir / "equity_after_tax.csv"
        eq2.to_csv(eq_out_path, index=False)

        pre_end = float(eq2["equity"].iloc[-1])
        post_end = float(eq2["equity_after_tax"].iloc[-1])
        drag = pre_end - post_end
        drag_pct = float(drag / pre_end) if pre_end != 0.0 else float("nan")

        drag_sum = {
            "schema_version": "tax_drag_summary_v1",
            "mode": spec.normalized().mode,
            "end_equity_pre_tax": pre_end,
            "end_equity_after_tax": post_end,
            "tax_drag_amount": float(drag),
            "tax_drag_pct_of_end_equity": float(drag_pct),
        }
        (out_dir / "tax_drag_summary.json").write_text(
            json.dumps(drag_sum, sort_keys=True, separators=(",", ":")), encoding="utf-8"
        )

    meta = {
        "schema_version": "tax_drag_meta_v1",
        "inputs": {
            "realized_trades_csv": file_record(realized_trades_csv),
            "equity_curve_csv": file_record(Path(equity_curve_csv)) if equity_curve_csv else None,
        },
        "outputs": {
            "tax_summary_json": file_record(summary_path),
            "equity_after_tax_csv": file_record(eq_out_path) if eq_out_path else None,
        },
        "ids": {"tax_drag_run_id": sha256_json({"inputs": {"realized": file_record(realized_trades_csv)}})},
    }
    (out_dir / "tax_drag_meta.json").write_text(json.dumps(meta, sort_keys=True, separators=(",", ":")), encoding="utf-8")

    return summary_path, eq_out_path


def main_tax_drag(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(prog="mqk-tax-drag", description="Estimate tax drag from realized trades (scaffold)")
    ap.add_argument("--realized", required=True)
    ap.add_argument("--out-dir", required=True)
    ap.add_argument("--equity-curve", default=None)
    ap.add_argument("--mode", default="annual_settlement", choices=["annual_settlement", "immediate_withholding"])
    ap.add_argument("--short-rate", type=float, default=0.30)
    ap.add_argument("--long-rate", type=float, default=0.15)
    ap.add_argument("--no-loss-offset", action="store_true")
    args = ap.parse_args(argv)

    rates = TaxRates(short_rate=args.short_rate, long_rate=args.long_rate)
    spec = TaxDragSpec(mode=args.mode, allow_loss_offset=(not args.no_loss_offset))

    summary, eq_out = run_tax_drag_simulation(
        realized_trades_csv=Path(args.realized),
        out_dir=Path(args.out_dir),
        equity_curve_csv=Path(args.equity_curve) if args.equity_curve else None,
        rates=rates,
        spec=spec,
    )
    print(f"OK tax_summary={summary}")
    if eq_out:
        print(f"OK equity_after_tax={eq_out}")
    return 0
