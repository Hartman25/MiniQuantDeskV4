"""W06-A-P9-CANONICAL-CLI-AUTHORITY-REPAIR-01 (R3.5) -- builds a real,
synthetic Wave06 LIQ-01-shaped Research registry fixture (both real
hypotheses: long_only + long_short) through the REAL production entry point
(`run_registered_economic_walkforward_eval`) -- never a hand-authored
registry row or artifact. Registers under the campaign's own frozen
`strategy_id`/`feature_column` (`WAVE06_FEATURE_TRANSFORM_AUTHORITY` in
`mqk_research.ml.oos_replay_bundle`) and the exact `experiment_id`/
`hypothesis_id`s LIQ-01's own committed `PREDECLARED_WAVE.json` declares, so
the full canonical Wave06 Rust CLI pipeline (replay bundle build, campaign
judge, DSR/PBO sensitivity, P7A/P7B replay stress, genuine shuffled placebo)
can run against it end-to-end with no network/provider access.

Invoked as a script (`python build_r3_e2e_fixture.py <registry_db> <run_root>`)
by `mqk-cli`'s own R3.5 synthetic E2E test
(`core-rs/crates/mqk-cli/src/commands/research_replay.rs`), which needs a
real Python-registered trial and has no Rust-side way to create one.

Not a pytest file (no `test_` prefix) -- a plain, reusable fixture builder.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

_RESEARCH_PY_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(_RESEARCH_PY_ROOT / "src"))
sys.path.insert(0, str(_RESEARCH_PY_ROOT / "experiments" / "wave06_campaign"))

import numpy as np  # noqa: E402
import pandas as pd  # noqa: E402

from campaign_identity import CAMPAIGN_REAL_EXPERIMENT_ID  # noqa: E402
from mqk_research.data.bars_provenance import (  # noqa: E402
    CA_POLICY_FORBID_AFFECTED_PERIODS,
    PRICE_CONVENTION_RAW_UNADJUSTED,
    UNIVERSE_MODE_FIXED_EX_ANTE,
    build_bars_provenance_manifest,
    build_corporate_action_evidence,
)
from mqk_research.ml.economic_registry_integration import run_registered_economic_walkforward_eval  # noqa: E402
from mqk_research.ml.economic_walkforward import (  # noqa: E402
    SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1,
    SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1,
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
)
from mqk_research.ml.eval_walkforward import WalkForwardSpec  # noqa: E402
from mqk_research.ml.execution_pricing import (  # noqa: E402
    EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    ExecutionPricingSpec,
)
from mqk_research.ml.schema import generate_feature_schema  # noqa: E402
from mqk_research.ml.weight_to_share import WeightToShareSpec  # noqa: E402

# Frozen Wave06 LIQ-01 candidate declaration
# (research-py/experiments/wave06_candidate_liq01_amihud_illiquidity/PREDECLARED_WAVE.json)
# -- exact strategy_id / feature_column / hypothesis_ids this fixture must
# use for `mqk_research.ml.oos_replay_bundle.WAVE06_FEATURE_TRANSFORM_AUTHORITY`
# (R1.3) to accept it.
STRATEGY_ID = "pooled_single_feature_xs_amihud_illiquidity_direct_rank_v1"
FEATURE_COL = "illiquidity_amihud_daily_xs_rank"
HYPOTHESIS_ID_LONG_ONLY = "wave06_liq01_amihud_illiquidity_long_only_v1"
HYPOTHESIS_ID_LONG_SHORT = "wave06_liq01_amihud_illiquidity_long_short_v1"

SYMBOLS = [f"SYM{i}" for i in range(16)]
WF_SPEC_KW = dict(
    train_years=1, test_months=1, step_months=1, holdout_months=1,
    min_rows_per_fold=50, purge_enabled=True, embargo_seconds=0,
)


def _build_dataset(periods_days: int = 900, horizon_days: int = 5, seed: int = 0) -> pd.DataFrame:
    dates = pd.date_range("2018-01-01", periods=periods_days, freq="D", tz="UTC")
    rng = np.random.default_rng(seed)
    label_rng = np.random.default_rng(seed + 1)
    rows = []
    for d in dates:
        for i, sym in enumerate(SYMBOLS):
            raw = float(i) + rng.normal(scale=0.01)
            rows.append({"symbol": sym, "end_ts": d, "raw": raw})
    df = pd.DataFrame(rows)
    df[FEATURE_COL] = df.groupby("end_ts")["raw"].rank(pct=True, method="average")
    # Genuine (if synthetic) signal: target correlates with the feature rank
    # plus noise, so the fold-trained classifier learns non-degenerate
    # weights (fully random labels shrink weights toward zero under L2).
    noise = label_rng.normal(scale=0.35, size=len(df))
    df["target"] = ((df[FEATURE_COL].to_numpy() - 0.5 + noise) > 0.0).astype(int)
    df["label_end_ts"] = df["end_ts"] + pd.Timedelta(days=horizon_days)
    return df


def _write_run_dir(run_dir: Path, df: pd.DataFrame) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    feats = df[["symbol", "end_ts", FEATURE_COL]].copy()
    targs = df[["symbol", "end_ts", "target", "label_end_ts"]].copy()
    feats["end_ts"] = feats["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["end_ts"] = targs["end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    targs["label_end_ts"] = targs["label_end_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    feats.to_csv(run_dir / "features.csv", index=False)
    targs.to_csv(run_dir / "targets.csv", index=False)
    generate_feature_schema(run_dir, id_columns=["symbol", "end_ts"])


def _build_bars(df: pd.DataFrame) -> pd.DataFrame:
    rows = []
    base = {sym: 100.0 + 10.0 * i for i, sym in enumerate(SYMBOLS)}
    rng = np.random.default_rng(123)
    for ts in sorted(df["end_ts"].unique()):
        for sym in SYMBOLS:
            px = base[sym] * (1.0 + rng.normal(scale=0.001))
            rows.append({
                "symbol": sym, "end_ts": pd.Timestamp(ts).isoformat(),
                "open": px, "high": px * 1.001, "low": px * 0.999, "close": px, "volume": 100_000,
            })
    return pd.DataFrame(rows)


def _bars_provenance(bars_path: Path) -> dict:
    bars = pd.read_csv(bars_path)
    end_ts = pd.to_datetime(bars["end_ts"], utc=True)
    coverage_start = end_ts.min().isoformat()
    coverage_end = (end_ts.max() + pd.Timedelta(seconds=1)).isoformat()
    evidence = build_corporate_action_evidence(
        source_provider_id="test_fixture_no_known_corporate_actions",
        covered_symbol_universe=sorted(SYMBOLS),
        coverage_start_utc=coverage_start,
        coverage_end_utc=coverage_end,
        corporate_action_entries=(),
    )
    return build_bars_provenance_manifest(
        price_provenance={
            "close_column": "close",
            "provider_ids_observed": ["test_fixture"],
            "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED,
            "provider_metadata_available": True,
            "convention_basis": "synthetic R3.5 E2E fixture -- no real provider involved",
        },
        corporate_action_policy=CA_POLICY_FORBID_AFFECTED_PERIODS,
        corporate_action_evidence_id=evidence["evidence_id"],
        corporate_action_evidence=evidence,
        forbidden_periods=(),
        timeframe="1D",
        start_utc=coverage_start,
        end_utc=coverage_end,
        symbol_universe=sorted(SYMBOLS),
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE,
        bars=bars,
        artifact_path=bars_path,
    )


def _register(registry_db: Path, run_root: Path, direction_policy: str, hypothesis_id: str, rank_side_count: int = 2) -> dict:
    run_dir = run_root / hypothesis_id
    df = _build_dataset(seed=0)
    _write_run_dir(run_dir, df)
    bars_path = run_dir / "bars.csv"
    _build_bars(df).to_csv(bars_path, index=False)
    manifest = _bars_provenance(bars_path)

    kwargs = dict(
        direction_policy=direction_policy,
        long_only=(direction_policy == SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1),
        rank_side_count=rank_side_count,
        max_gross_exposure=1.0,
    )
    if direction_policy == SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1:
        kwargs["borrow_model"] = "research_assumed_shortable_universe_v1"

    spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(**kwargs),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        execution_pricing=ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1, slippage_bps=5, volatility_mult_bps=0
        ),
        annualization=AnnualizationSpec(),
        weight_to_share=WeightToShareSpec(equity_usd=100_000.0),
    )
    out_path = run_registered_economic_walkforward_eval(
        run_dir,
        experiment_id=CAMPAIGN_REAL_EXPERIMENT_ID,
        hypothesis_id=hypothesis_id,
        strategy_id=STRATEGY_ID,
        bars_csv=bars_path,
        economic_spec=spec,
        bars_provenance=manifest,
        registry_db=registry_db,
        wf_spec=WalkForwardSpec(**WF_SPEC_KW),
        l2=1e-3,
        lr=0.05,
        steps=200,
    )
    econ = json.loads(out_path.read_text(encoding="utf-8"))
    return {
        "hypothesis_id": hypothesis_id,
        "trial_id": econ["registry"]["trial_id"],
        "economic_eval_id": econ["ids"]["economic_eval_id"],
    }


def build_fixture(registry_db: Path, run_root: Path) -> dict:
    """Registers LIQ-01's real long_only + long_short trials under the
    shared campaign registry/experiment_id. Returns identity for both,
    naming `long_short` as `primary` (the campaign's own
    `primary_candidate_direction_control_relationship.primary`)."""
    run_root.mkdir(parents=True, exist_ok=True)
    long_only = _register(registry_db, run_root, SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_ONLY_V1, HYPOTHESIS_ID_LONG_ONLY)
    long_short = _register(registry_db, run_root, SIGNAL_DIRECTION_POLICY_CROSS_SECTIONAL_RANK_LONG_SHORT_V1, HYPOTHESIS_ID_LONG_SHORT)
    return {
        "status": "ok",
        "strategy_id": STRATEGY_ID,
        "long_only": long_only,
        "long_short": long_short,
        "primary": long_short,
    }


def main() -> int:
    registry_db = Path(sys.argv[1])
    run_root = Path(sys.argv[2])
    result = build_fixture(registry_db, run_root)
    print(json.dumps(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
