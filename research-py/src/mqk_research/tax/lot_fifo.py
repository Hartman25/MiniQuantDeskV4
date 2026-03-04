from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional

import numpy as np
import pandas as pd

from mqk_research.ml.util_hash import file_record, sha256_json
from mqk_research.tax.contracts import FillsCsvContractV1


@dataclass(frozen=True)
class LotSpec:
    """FIFO lot tracking spec."""

    long_term_days: int = 365
    assume_fees_in_cost_basis: bool = True  # BUY fees increase basis; SELL fees reduce proceeds

    def normalized(self) -> "LotSpec":
        return LotSpec(
            long_term_days=int(self.long_term_days),
            assume_fees_in_cost_basis=bool(self.assume_fees_in_cost_basis),
        )


def _to_utc(s: pd.Series) -> pd.Series:
    return pd.to_datetime(s, utc=True)


def _norm_side(x: str) -> str:
    u = str(x).strip().upper()
    if u in ("BUY", "B"):
        return "BUY"
    if u in ("SELL", "S"):
        return "SELL"
    raise ValueError(f"invalid side={x!r}")


def fifo_realized_trades_from_fills(fills: pd.DataFrame, *, spec: Optional[LotSpec] = None) -> pd.DataFrame:
    spec = (spec or LotSpec()).normalized()
    contract = FillsCsvContractV1().normalized()

    missing = [c for c in contract.required_columns if c not in fills.columns]
    if missing:
        raise ValueError(f"fills missing required columns: {missing}")

    df = fills.copy()
    df["fill_ts"] = _to_utc(df["fill_ts"])
    df["side"] = df["side"].map(_norm_side)
    df["qty"] = df["qty"].astype(float)
    df["price"] = df["price"].astype(float)
    if "fee" not in df.columns:
        df["fee"] = 0.0
    else:
        df["fee"] = df["fee"].astype(float)

    df = df.sort_values(["symbol", "fill_ts", "side"], kind="mergesort").reset_index(drop=True)

    lots: Dict[str, List[Dict[str, object]]] = {}
    rows: List[Dict[str, object]] = []

    for _, r in df.iterrows():
        sym = str(r["symbol"])
        ts = r["fill_ts"]
        side = r["side"]
        qty = float(r["qty"])
        px = float(r["price"])
        fee = float(r["fee"])

        if qty <= 0.0 or px <= 0.0:
            continue

        if side == "BUY":
            unit_cost = px + (fee / qty) if (spec.assume_fees_in_cost_basis and fee != 0.0) else px
            lots.setdefault(sym, []).append({"open_ts": ts, "qty": qty, "unit_cost": unit_cost})
            continue

        # SELL
        remaining = qty
        if sym not in lots or not lots[sym]:
            raise RuntimeError(f"SELL with no open lots for {sym} at {ts.isoformat()} qty={qty}")

        while remaining > 1e-12:
            if not lots[sym]:
                raise RuntimeError(f"SELL exceeds open lots for {sym} at {ts.isoformat()} remaining={remaining}")

            lot = lots[sym][0]
            open_ts = lot["open_ts"]
            lot_qty = float(lot["qty"])
            take = min(lot_qty, remaining)

            unit_cost = float(lot["unit_cost"])
            unit_proceeds = px - (fee / qty) if (spec.assume_fees_in_cost_basis and fee != 0.0) else px

            cost_basis = take * unit_cost
            proceeds = take * unit_proceeds
            realized = proceeds - cost_basis
            holding_days = int((ts - open_ts).days)
            term = "long" if holding_days >= spec.long_term_days else "short"

            rows.append({
                "symbol": sym,
                "open_ts": open_ts.isoformat(),
                "close_ts": ts.isoformat(),
                "qty": float(take),
                "unit_cost": float(unit_cost),
                "unit_proceeds": float(unit_proceeds),
                "cost_basis": float(cost_basis),
                "proceeds": float(proceeds),
                "realized_gain": float(realized),
                "holding_days": holding_days,
                "term": term,
            })

            lot["qty"] = lot_qty - take
            remaining -= take
            if float(lot["qty"]) <= 1e-12:
                lots[sym].pop(0)

    out = pd.DataFrame(rows)
    if out.empty:
        raise RuntimeError("No realized trades produced (no sells or no matched lots).")

    return out.sort_values(["symbol", "close_ts", "open_ts"], kind="mergesort").reset_index(drop=True)


def write_realized_trades(fills_csv: Path, out_realized_csv: Path, *, spec: Optional[LotSpec] = None) -> Path:
    fills_csv = Path(fills_csv)
    out_realized_csv = Path(out_realized_csv)

    fills = pd.read_csv(fills_csv)
    realized = fifo_realized_trades_from_fills(fills, spec=spec)

    out_realized_csv.parent.mkdir(parents=True, exist_ok=True)
    realized.to_csv(out_realized_csv, index=False)

    meta = {
        "schema_version": "fifo_lot_realized_meta_v1",
        "inputs": {"fills_csv": file_record(fills_csv)},
        "outputs": {"realized_trades_csv": file_record(out_realized_csv)},
        "ids": {"realized_run_id": sha256_json({"fills": file_record(fills_csv), "out": file_record(out_realized_csv)})},
    }
    (out_realized_csv.parent / "realized_trades_meta.json").write_text(
        json.dumps(meta, sort_keys=True, separators=(",", ":")), encoding="utf-8"
    )
    return out_realized_csv


def main_fifo(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(prog="mqk-tax-fifo", description="FIFO lot tracking -> realized_trades.csv (scaffold)")
    ap.add_argument("--fills", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--long-term-days", type=int, default=365)
    args = ap.parse_args(argv)

    spec = LotSpec(long_term_days=args.long_term_days)
    out = write_realized_trades(Path(args.fills), Path(args.out), spec=spec)
    print(f"OK realized={out}")
    return 0
