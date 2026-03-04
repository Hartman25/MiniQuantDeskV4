from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Optional, Dict

import pandas as pd

from mqk_research.ml.util_hash import file_record, sha256_json
from mqk_research.tax.metrics import tax_aware_backtest_metrics


@dataclass(frozen=True)
class ReportSpec:
    """Minimal report builder spec (scaffold)."""
    compute_tax_aware_metrics: bool = True

    def normalized(self) -> "ReportSpec":
        return ReportSpec(compute_tax_aware_metrics=bool(self.compute_tax_aware_metrics))


def build_report_scaffold(
    *,
    run_dir: Path,
    out_dir: Path,
    spec: Optional[ReportSpec] = None,
) -> Path:
    """Builds a minimal report bundle from existing artifacts in run_dir."""
    spec = (spec or ReportSpec()).normalized()
    run_dir = Path(run_dir)
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    # expected (optional) inputs
    equity_curve = run_dir / "equity_curve.csv"
    equity_after_tax = run_dir / "tax" / "equity_after_tax.csv"

    report = {
        "schema_version": "report_bundle_v1",
        "inputs": {
            "equity_curve_csv": file_record(equity_curve) if equity_curve.exists() else None,
            "equity_after_tax_csv": file_record(equity_after_tax) if equity_after_tax.exists() else None,
        },
        "outputs": {},
        "ids": {},
    }

    if spec.compute_tax_aware_metrics and equity_curve.exists() and equity_after_tax.exists():
        out_metrics = out_dir / "tax_aware_metrics.json"
        tax_aware_backtest_metrics(
            equity_curve_csv=equity_curve,
            equity_after_tax_csv=equity_after_tax,
            out_json=out_metrics,
        )
        report["outputs"]["tax_aware_metrics_json"] = file_record(out_metrics)

    report_id = sha256_json({"inputs": report["inputs"], "outputs": report.get("outputs", {}), "schema": report["schema_version"]})
    report["ids"]["report_id"] = report_id

    out_manifest = out_dir / "report_manifest.json"
    out_manifest.write_text(json.dumps(report, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    return out_manifest


def main_report(argv: list[str] | None = None) -> int:
    import argparse
    ap = argparse.ArgumentParser(prog="mqk-report", description="Build a minimal report bundle (scaffold)")
    ap.add_argument("--run-dir", required=True)
    ap.add_argument("--out-dir", required=True)
    ap.add_argument("--no-tax-metrics", action="store_true")
    args = ap.parse_args(argv)

    spec = ReportSpec(compute_tax_aware_metrics=(not args.no_tax_metrics))
    out = build_report_scaffold(run_dir=Path(args.run_dir), out_dir=Path(args.out_dir), spec=spec)
    print(f"OK report_manifest={out}")
    return 0
