"""REAL-RESEARCH-PROMOTION-E2E-CLOSURE-01 -- thin cross-language
orchestration wrapper that runs ONE real, registered Research trial
end-to-end (hypothesis -> trial -> attempt -> classification walk-forward
-> economic walk-forward -> registered result) via the FROZEN, accepted
`run_registered_economic_walkforward_eval` production entry point, then
builds a REAL multiple-testing judge for that trial's experiment via the
FROZEN, accepted `build_multiple_testing_judge`. Never redefines any
Research statistics or economics itself -- mirrors
`dsr_pbo_sensitivity_cli.py`'s own established pattern of a thin,
cross-language wrapper around real, already-accepted Python functions,
called from Rust via subprocess.

The synthetic-but-real dataset construction (features/targets/schema/bars/
provenance) mirrors `research-py/tests/test_multiple_testing_judge.py`'s
own `_build_full_dataset` / `_write_full_run_dir` / `_build_flat_bars` /
`test_real_pipeline_integration` fixture builders exactly -- a real
(noisy, seeded) classifier trains and a real causal walk-forward runs; only
the INPUT DATA is synthetic, never the computation.

Output: exactly one JSON object on stdout.
- Exit 0 with `{"status": "ok", ...}` naming every artifact path/hash the
  Rust caller needs to feed into `verify_promotion_oos_evidence` (P7C) and
  `dsr_pbo_sensitivity_scenario` (P9) for the SAME trial_id.
- Exit 1 with `{"status": "error", "reason": ...}` for a genuine
  operational failure -- never a raw Python traceback for the Rust caller
  to fail to parse.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional

import numpy as np
import pandas as pd

from mqk_research.exp_distributed.hashing import canonical_json, sha256_bytes
from mqk_research.ml.economic_registry_integration import (
    run_registered_economic_walkforward_eval,
)
from mqk_research.ml.economic_walkforward import (
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec
from mqk_research.ml.execution_pricing import (
    EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    ExecutionPricingSpec,
)
from mqk_research.ml.multiple_testing_judge import build_multiple_testing_judge
from mqk_research.ml.schema import generate_feature_schema
from mqk_research.ml.weight_to_share import WeightToShareSpec


def _build_dataset(symbols: List[str], periods_days: int, horizon_days: int, seed: int) -> pd.DataFrame:
    """Mirrors `test_multiple_testing_judge._build_full_dataset` exactly."""
    rng = np.random.default_rng(seed)
    dates = pd.date_range("2020-01-01", periods=periods_days, freq="D", tz="UTC")
    rows = []
    for sym in symbols:
        for d in dates:
            f1 = float(rng.normal())
            target = 1 if f1 > 0.0 else 0
            rows.append(
                {
                    "symbol": sym,
                    "end_ts": d,
                    "f1": f1,
                    "target": target,
                    "label_end_ts": d + pd.Timedelta(days=horizon_days),
                }
            )
    return pd.DataFrame(rows)


def _write_run_dir(run_dir: Path, df: pd.DataFrame) -> None:
    """Mirrors `test_multiple_testing_judge._write_full_run_dir` exactly."""
    run_dir.mkdir(parents=True, exist_ok=True)
    feats = df[["symbol", "end_ts", "f1"]].copy()
    targs = df[["symbol", "end_ts", "target", "label_end_ts"]].copy()
    feats["end_ts"] = feats["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["end_ts"] = targs["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["label_end_ts"] = targs["label_end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    feats.to_csv(run_dir / "features.csv", index=False)
    targs.to_csv(run_dir / "targets.csv", index=False)
    generate_feature_schema(run_dir, id_columns=["symbol", "end_ts"])


def _build_flat_bars(df: pd.DataFrame) -> pd.DataFrame:
    """Like `test_multiple_testing_judge._build_flat_bars`, but also carries
    `high`/`low` (a tight, real, non-degenerate range around `close`) --
    REQUIRED-PRODUCTION-PARITY-01: P7A's official execution-pricing model
    (`EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1`) needs `high`/`low`
    to compute a real conservative fill price; the P7C verifier
    (`mqk_promotion::verify_promotion_oos_evidence`) requires this model,
    not the close-only diagnostic default."""
    rows = []
    for (sym, ts), _ in df.groupby(["symbol", "end_ts"]):
        rows.append(
            {
                "symbol": sym,
                "end_ts": pd.Timestamp(ts).isoformat(),
                "high": 100.5,
                "low": 99.5,
                "close": 100.0,
            }
        )
    return pd.DataFrame(rows)


def _build_provenance(bars_df: pd.DataFrame, bars_path: Path) -> Dict[str, Any]:
    """Mirrors `test_real_pipeline_integration`'s inline provenance
    construction exactly -- a real, structurally valid (not fabricated)
    corporate-action-free provenance manifest for a synthetic fixture."""
    from mqk_research.data.bars_provenance import (
        CA_POLICY_FORBID_AFFECTED_PERIODS,
        PRICE_CONVENTION_RAW_UNADJUSTED,
        UNIVERSE_MODE_FIXED_EX_ANTE,
        build_bars_provenance_manifest,
        build_corporate_action_evidence,
    )

    end_ts = pd.to_datetime(bars_df["end_ts"], utc=True)
    symbol_universe = sorted(bars_df["symbol"].astype(str).unique().tolist())
    coverage_start = end_ts.min().isoformat()
    coverage_end = (end_ts.max() + pd.Timedelta(seconds=1)).isoformat()
    evidence = build_corporate_action_evidence(
        source_provider_id="real_research_promotion_e2e_fixture_no_known_corporate_actions",
        covered_symbol_universe=symbol_universe,
        coverage_start_utc=coverage_start,
        coverage_end_utc=coverage_end,
        corporate_action_entries=(),
    )
    return build_bars_provenance_manifest(
        price_provenance={
            "close_column": "close",
            "provider_ids_observed": ["real_research_promotion_e2e_fixture"],
            "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED,
            "provider_metadata_available": True,
            "convention_basis": "synthetic E2E-closure fixture -- no real provider involved",
        },
        corporate_action_policy=CA_POLICY_FORBID_AFFECTED_PERIODS,
        corporate_action_evidence_id=evidence["evidence_id"],
        corporate_action_evidence=evidence,
        forbidden_periods=(),
        timeframe="1D",
        start_utc=coverage_start,
        end_utc=coverage_end,
        symbol_universe=symbol_universe,
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE,
        bars=bars_df,
        artifact_path=bars_path,
    )


def _run_one_trial(
    *,
    run_dir: Path,
    registry_db: Path,
    experiment_id: str,
    hypothesis_id: str,
    strategy_id: str,
    entry_threshold: float,
    periods_days: int,
    steps: int,
    seed: int,
) -> Dict[str, Any]:
    df = _build_dataset(["AAA", "BBB"], periods_days, horizon_days=3, seed=seed)
    bars_df = _build_flat_bars(df)
    bars_path = run_dir / "bars.csv"
    run_dir.mkdir(parents=True, exist_ok=True)
    bars_df.to_csv(bars_path, index=False)
    _write_run_dir(run_dir, df)

    bars_provenance = _build_provenance(bars_df, bars_path)
    wf_spec = WalkForwardSpec(
        train_years=1, test_months=1, step_months=1, holdout_months=1, min_rows_per_fold=200
    )
    economic_spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=entry_threshold),
        # PROMOTION-OOS-EVIDENCE-GATE-01-REPAIR-03 (mqk_promotion::
        # verify_promotion_oos_evidence) requires the OFFICIAL P7A execution-
        # pricing parity model, never the close-only diagnostic default --
        # and, per ExecutionPricingSpec's own docs, slippage is then modeled
        # ENTIRELY via directional bar-range pricing, so
        # cost_model.slippage_bps_per_side must be 0 (charging both would
        # double-count it).
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        execution_pricing=ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
            slippage_bps=5,
            volatility_mult_bps=0,
        ),
        # Same verifier also requires the OFFICIAL P7B weight-to-share
        # translation (never the continuous-weight-only default) -- engages
        # the discrete_share_economic_path_v1 marker on every fold.
        weight_to_share=WeightToShareSpec(equity_usd=100_000.0),
        annualization=AnnualizationSpec(),
    )

    economic_out_path = run_registered_economic_walkforward_eval(
        run_dir,
        experiment_id=experiment_id,
        hypothesis_id=hypothesis_id,
        strategy_id=strategy_id,
        bars_csv=bars_path,
        economic_spec=economic_spec,
        bars_provenance=bars_provenance,
        registry_db=registry_db,
        wf_spec=wf_spec,
        steps=steps,
    )
    economic_out = json.loads(economic_out_path.read_text(encoding="utf-8"))
    trial_id = economic_out["registry"]["trial_id"]
    economic_eval_id = economic_out["ids"]["economic_eval_id"]
    daily_returns_path = run_dir / "eval" / "economic_daily_returns.csv"

    return {
        "trial_id": trial_id,
        "economic_eval_id": economic_eval_id,
        "economic_walk_forward_json": str(economic_out_path),
        "economic_daily_returns_csv": str(daily_returns_path),
    }


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--registry-db", required=True, type=Path)
    parser.add_argument("--run-root", required=True, type=Path)
    parser.add_argument("--experiment-id", required=True)
    parser.add_argument("--hypothesis-id", required=True)
    parser.add_argument("--strategy-id", required=True)
    parser.add_argument(
        "--entry-thresholds",
        required=True,
        help=(
            "comma-separated entry_threshold values, one real trial run per value, "
            "all registered under the SAME experiment/strategy -- e.g. two values "
            "produce two genuinely distinct, individually real trials sharing one "
            "strategy_id (needed for the trial-binding negative control)."
        ),
    )
    parser.add_argument("--periods-days", type=int, default=560)
    parser.add_argument("--steps", type=int, default=10)
    parser.add_argument(
        "--judge-out",
        required=True,
        type=Path,
        help="path to write the real, registered judge artifact's canonical JSON to",
    )
    args = parser.parse_args(argv)

    try:
        thresholds = [float(x) for x in args.entry_thresholds.split(",") if x.strip()]
        if not thresholds:
            raise ValueError("--entry-thresholds must supply at least one value")

        trials: List[Dict[str, Any]] = []
        for i, threshold in enumerate(thresholds):
            run_dir = args.run_root / f"trial_{i}"
            trials.append(
                _run_one_trial(
                    run_dir=run_dir,
                    registry_db=args.registry_db,
                    experiment_id=args.experiment_id,
                    hypothesis_id=args.hypothesis_id,
                    strategy_id=args.strategy_id,
                    entry_threshold=threshold,
                    periods_days=args.periods_days,
                    steps=args.steps,
                    seed=i,
                )
            )

        judge_artifact = build_multiple_testing_judge(
            experiment_id=args.experiment_id,
            registry_db=args.registry_db,
        )
        canonical_judge_json = canonical_json(judge_artifact)
        judge_artifact_sha256 = sha256_bytes(canonical_judge_json.encode("utf-8"))
        args.judge_out.parent.mkdir(parents=True, exist_ok=True)
        args.judge_out.write_text(canonical_judge_json, encoding="utf-8")

        dsr_by_trial = {r["trial_id"]: r for r in judge_artifact.get("dsr_results", [])}
        for t in trials:
            entry = dsr_by_trial.get(t["trial_id"])
            t["dsr_evaluable"] = bool(entry and entry.get("evaluable"))
            t["deflated_sharpe_ratio"] = entry.get("deflated_sharpe_ratio") if entry else None

        result = {
            "status": "ok",
            "experiment_id": args.experiment_id,
            "hypothesis_id": args.hypothesis_id,
            "strategy_id": args.strategy_id,
            "registry_db": str(args.registry_db),
            "judge_status": judge_artifact.get("judge_status"),
            "pbo_status": judge_artifact.get("pbo_result", {}).get("status"),
            "judge_artifact_path": str(args.judge_out),
            "judge_artifact_sha256": judge_artifact_sha256,
            "trials": trials,
        }
    except Exception as exc:  # noqa: BLE001 -- deliberate catch-all: fail closed with
        # structured JSON, never a raw Python traceback for the Rust caller to fail to
        # parse (mirrors dsr_pbo_sensitivity_cli.py's own contract).
        json.dump({"status": "error", "reason": str(exc)}, sys.stdout)
        return 1

    json.dump(result, sys.stdout)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
