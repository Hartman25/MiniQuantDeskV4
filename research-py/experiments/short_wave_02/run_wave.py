"""SHORT-RESEARCH-WAVE-02-CONTROLLER -- development-stage Research
experiment driver for three predeclared, genuinely distinct ETF
short-capable classifier hypotheses (SHORT-02 cross-sectional-momentum-rank,
SHORT-03 short-term-reversal, SHORT-04 gap-reversal/exhaustion), each run as
a matched LONG-ONLY control / LONG-SHORT candidate pair plus a causal
same-horizon placebo, using the existing accepted Research -> registered
walk-forward -> economic evaluation -> multiple-testing-judge pipeline.

All experiment identity, universe, data window, label definition,
walk-forward spec, cost model, execution model, thresholds, and the placebo
seed are FROZEN in PREDECLARED_WAVE.json before any trial result is viewed.
This file must not be edited to react to an observed result -- see
CONTROLLER RULE in the mission (docs/research/SHORT_WAVE_02_FAMILY_REPORT.md
records evidence of that once the wave is closed out).

Truthful terminology: `long_short_threshold_v1` consumes
`ml_score = P(target=1)` where target means "future 10-bar log return > 0".
These are CLASSIFIERS, not deterministic rank-sorted long/short portfolios.
`research_assumed_shortable_universe_v1` is a Research-only borrow-model
assumption -- it does not authorize Paper/Live shorting.

Uses ONLY existing, frozen, already-accepted production entry points:
  - mqk_research.data.alpaca_historical.extract_research_bars_with_provenance
  - mqk_research.features.feature_set_v1.build_feature_set_v1
  - mqk_research.ml.economic_registry_integration.run_registered_economic_walkforward_eval
  - mqk_research.ml.multiple_testing_judge.build_multiple_testing_judge

No research-py/src file is modified by this script. All three required
single features (ret_rank_20, ret_5, gap_pct_1) are already produced by
build_feature_set_v1's DEFAULT FeatureSetV1Spec -- no spec customization is
needed; feature isolation (selecting exactly one column) happens in THIS
driver, after calling the unmodified build_feature_set_v1, never inside
feature_set_v1.py itself.

The causal placebo helper (`build_causal_placebo_targets`) is an exact,
self-contained reproduction of the accepted, PUSHED-VERIFIED
research-alpha-gap-discovery-01-clean worktree's implementation
(research-py/experiments/alpha_discovery_01/run_experiment.py, commit
28497968cada7870efe38295b5712b49b0d32398), inlined the same way the
independently-accepted SHORT-01 worktree inlined it
(SHORT-01-DRIVER-PORTABILITY-01) so this driver is runnable from a bare
checkout of this branch alone. Semantics unchanged.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Any

WAVE02_WORKTREE_SRC = Path(__file__).resolve().parents[2] / "src"
assert WAVE02_WORKTREE_SRC.name == "src" and "short-wave-02" in str(WAVE02_WORKTREE_SRC), (
    f"refusing to run: expected the isolated SHORT-WAVE-02 worktree's own src/, got {WAVE02_WORKTREE_SRC}"
)
sys.path.insert(0, str(WAVE02_WORKTREE_SRC))

import numpy as np
import pandas as pd

from mqk_research.data.alpaca_historical import extract_research_bars_with_provenance
from mqk_research.features.feature_set_v1 import build_feature_set_v1
from mqk_research.ml.economic_registry_integration import run_registered_economic_walkforward_eval
from mqk_research.ml.economic_walkforward import (
    SIGNAL_DIRECTION_POLICY_LONG_ONLY_V1,
    SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
    AnnualizationSpec,
    CostModelSpec,
    EconomicWalkForwardSpec,
    SignalPolicySpec,
)
from mqk_research.ml.economics import compute_max_drawdown, compute_sharpe
from mqk_research.ml.eval_walkforward import WalkForwardSpec
from mqk_research.ml.execution_pricing import (
    EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
    ExecutionPricingSpec,
)
from mqk_research.ml.multiple_testing_judge import build_multiple_testing_judge
from mqk_research.ml.schema import generate_feature_schema
from mqk_research.ml.weight_to_share import WeightToShareSpec

# ---------------------------------------------------------------------------
# Self-contained, exact reproduction of the accepted causal placebo helper.
# See module docstring for attribution/provenance. Semantics unchanged.
# ---------------------------------------------------------------------------


def build_causal_placebo_targets(targets: pd.DataFrame, *, seed: int) -> pd.DataFrame:
    """Deterministic negative control that destroys symbol-specific
    predictive association WITHOUT moving information across temporal label
    horizons: permutes the (fwd_ret, target) PAIR only within rows sharing
    the EXACT same (end_ts, label_end_ts). `symbol`/`end_ts` identity and
    `label_end_ts` are left untouched for every row; only which row receives
    which (fwd_ret, target) outcome changes. A group of size 1 has no other
    row to swap with, so it is left unchanged. Group iteration is over
    `sorted()` (end_ts, label_end_ts) key tuples so the result is
    reproducible across runs/platforms for a fixed seed.

    Fail-closed effectiveness check: raise RuntimeError if zero rows'
    `target` value actually changed from the original input (an ineffective
    placebo the classifier could not be distinguished from real labels by).
    """
    rng = np.random.default_rng(seed)
    out = targets.copy().reset_index(drop=True)
    original_target = out["target"].to_numpy(copy=True)
    fwd_ret = out["fwd_ret"].to_numpy(copy=True)
    target = out["target"].to_numpy(copy=True)

    group_indices = out.groupby(["end_ts", "label_end_ts"], sort=False).indices
    for key in sorted(group_indices.keys()):
        idx = group_indices[key]
        if len(idx) <= 1:
            continue
        perm = rng.permutation(len(idx))
        fwd_ret[idx] = fwd_ret[idx][perm]
        target[idx] = target[idx][perm]

    changed_target_count = int(np.sum(target != original_target))
    if changed_target_count == 0:
        raise RuntimeError(
            "Fail-closed: causal placebo produced zero changed target assignments "
            "(every same-horizon group's target values were already identical, so "
            "permutation could not change any target) -- this is not a valid "
            "classifier negative control."
        )

    out["fwd_ret"] = fwd_ret
    out["target"] = target
    return out


EXPERIMENT_ROOT = Path(__file__).resolve().parent
RUN_ROOT = EXPERIMENT_ROOT / "runs" / "run_01"
REGISTRY_DB = RUN_ROOT / "registry" / "research.sqlite3"
PREDECLARATION_PATH = EXPERIMENT_ROOT / "PREDECLARED_WAVE.json"

PRIMARY_PAPER_REPO = Path(r"C:\Users\Zacha\Desktop\MiniQuantDeskV4")
PRIMARY_PAPER_HEAD_EXPECTED = "edcda740b2f05fbe8a2657f2301b8ea373efb4b6"

REAL_EXPERIMENT_ID = "SHORT-WAVE-02-REAL-CANDIDATES-V1"
PLACEBO_EXPERIMENT_ID = "SHORT-WAVE-02-DIAGNOSTIC-PLACEBOS-V1"

SYMBOLS = [
    "SPY", "QQQ", "IWM", "DIA", "XLF", "XLK",
    "XLE", "XLV", "XLI", "XLY", "XLP", "XLU",
]

START_UTC = pd.Timestamp("2016-01-01T00:00:00Z")
END_UTC = pd.Timestamp("2024-01-01T00:00:00Z")
ASOF = "2024-01-01"
TIMEFRAME = "1Day"
FEED = "sip"  # explicit SEMANTIC DATA-SOURCE CHOICE -- production DEFAULT_FEED ("iex") is untouched.

LABEL_HORIZON_BARS = 10
LABEL_RET_THRESHOLD = 0.0

LONG_ENTRY_THRESHOLD = 0.55
SHORT_THRESHOLD = 0.45
MAX_GROSS_EXPOSURE = 1.0
PLACEBO_SEED = 1234

COMMISSION_BPS_PER_SIDE = 10.0  # CONSERVATIVE RESEARCH COST ASSUMPTION, not actual Alpaca commission.
SLIPPAGE_BPS_PER_SIDE = 0.0
EXECUTION_SLIPPAGE_BPS = 5
EXECUTION_VOLATILITY_MULT_BPS = 0
EQUITY_USD = 100_000.0

WF_TRAIN_YEARS = 3
WF_TEST_MONTHS = 3
WF_STEP_MONTHS = 3
WF_HOLDOUT_MONTHS = 6
WF_MIN_ROWS_PER_FOLD = 300


class HypothesisFamily:
    """One predeclared mechanism: a single feature, its own strategy_id, and
    the three hypothesis_ids (long-only real, long-short real, placebo)."""

    def __init__(self, *, key: str, feature_column: str, strategy_id: str,
                 hyp_long_only: str, hyp_long_short: str, hyp_placebo: str) -> None:
        self.key = key
        self.feature_column = feature_column
        self.strategy_id = strategy_id
        self.hyp_long_only = hyp_long_only
        self.hyp_long_short = hyp_long_short
        self.hyp_placebo = hyp_placebo


FAMILIES: dict[str, HypothesisFamily] = {
    "SHORT-02": HypothesisFamily(
        key="SHORT-02",
        feature_column="ret_rank_20",
        strategy_id="pooled_single_feature_xs_momentum_rank_classifier_v1",
        hyp_long_only="short02_xs_momentum_rank_long_only_v1",
        hyp_long_short="short02_xs_momentum_rank_long_short_v1",
        hyp_placebo="short02_xs_momentum_rank_placebo_v1",
    ),
    "SHORT-03": HypothesisFamily(
        key="SHORT-03",
        feature_column="ret_5",
        strategy_id="pooled_single_feature_short_term_reversal_classifier_v1",
        hyp_long_only="short03_short_term_reversal_long_only_v1",
        hyp_long_short="short03_short_term_reversal_long_short_v1",
        hyp_placebo="short03_short_term_reversal_placebo_v1",
    ),
    "SHORT-04": HypothesisFamily(
        key="SHORT-04",
        feature_column="gap_pct_1",
        strategy_id="pooled_single_feature_gap_reversal_classifier_v1",
        hyp_long_only="short04_gap_reversal_long_only_v1",
        hyp_long_short="short04_gap_reversal_long_short_v1",
        hyp_placebo="short04_gap_reversal_placebo_v1",
    ),
}

REAL_CANDIDATE_HYPOTHESIS_IDS = sorted(
    [f.hyp_long_only for f in FAMILIES.values()] + [f.hyp_long_short for f in FAMILIES.values()]
)
DIAGNOSTIC_PLACEBO_HYPOTHESIS_IDS = sorted(f.hyp_placebo for f in FAMILIES.values())


def load_predeclaration() -> dict:
    return json.loads(PREDECLARATION_PATH.read_text(encoding="utf-8"))


def assert_driver_agrees_with_predeclaration() -> None:
    """Fail closed unless every frozen constant in this module matches the
    committed PREDECLARED_WAVE.json byte-for-byte on the fields that matter
    for research identity. See test_short_wave_02.py for the executable
    proof; this function is the single source both the driver's own
    startup path and the tests call."""
    decl = load_predeclaration()
    assert decl["real_experiment_id"] == REAL_EXPERIMENT_ID
    assert decl["placebo_experiment_id"] == PLACEBO_EXPERIMENT_ID
    assert decl["fixed_universe"] == SYMBOLS
    assert decl["data"]["feed"] == FEED
    assert decl["data"]["timeframe"] == TIMEFRAME
    assert decl["data"]["start_utc"] == "2016-01-01T00:00:00Z"
    assert decl["data"]["end_utc"] == "2024-01-01T00:00:00Z"
    assert decl["data"]["asof"] == ASOF
    assert decl["label"]["horizon_bars"] == LABEL_HORIZON_BARS
    assert decl["label"]["ret_threshold"] == LABEL_RET_THRESHOLD
    wf = decl["walk_forward"]
    assert wf["train_years"] == WF_TRAIN_YEARS
    assert wf["test_months"] == WF_TEST_MONTHS
    assert wf["step_months"] == WF_STEP_MONTHS
    assert wf["holdout_months"] == WF_HOLDOUT_MONTHS
    assert wf["min_rows_per_fold"] == WF_MIN_ROWS_PER_FOLD
    assert wf["purge_enabled"] is True
    assert wf["embargo_seconds"] == 0
    cost = decl["cost_model"]
    assert cost["commission_bps_per_side"] == COMMISSION_BPS_PER_SIDE
    assert cost["slippage_bps_per_side"] == SLIPPAGE_BPS_PER_SIDE
    exe = decl["execution_pricing"]
    assert exe["pricing_model_id"] == EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1
    assert exe["slippage_bps"] == EXECUTION_SLIPPAGE_BPS
    assert exe["volatility_mult_bps"] == EXECUTION_VOLATILITY_MULT_BPS
    wts = decl["weight_to_share"]
    assert wts["equity_usd"] == EQUITY_USD
    assert wts["max_gross_exposure"] == MAX_GROSS_EXPOSURE
    sp = decl["signal_policies"]
    assert sp["long_only"]["entry_threshold"] == LONG_ENTRY_THRESHOLD
    assert sp["long_short"]["entry_threshold"] == LONG_ENTRY_THRESHOLD
    assert sp["long_short"]["short_threshold"] == SHORT_THRESHOLD
    assert decl["placebo_seed"] == PLACEBO_SEED
    for key, fam in FAMILIES.items():
        h = decl["hypotheses"][key]
        assert h["feature_columns"] == [fam.feature_column]
        assert h["strategy_id"] == fam.strategy_id
        assert h["hypothesis_id_long_only"] == fam.hyp_long_only
        assert h["hypothesis_id_long_short"] == fam.hyp_long_short
        assert h["hypothesis_id_placebo"] == fam.hyp_placebo
    assert decl["real_candidate_population"] == REAL_CANDIDATE_HYPOTHESIS_IDS
    assert decl["diagnostic_placebo_population"] == DIAGNOSTIC_PLACEBO_HYPOTHESIS_IDS


def _load_paper_credentials_into_env() -> None:
    """Read ONLY ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER from the
    frozen Paper repo's .env.local into process memory if not already
    present in the environment. Never prints values. Never reads Live
    credentials. Never modifies .env.local."""
    required = ("ALPACA_API_KEY_PAPER", "ALPACA_API_SECRET_PAPER")
    if all(os.environ.get(k) for k in required):
        return
    env_path = PRIMARY_PAPER_REPO / ".env.local"
    if not env_path.exists():
        raise RuntimeError(f"Fail-closed: no credentials in process env and {env_path} does not exist")
    for line in env_path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        if line.startswith("export "):
            line = line[len("export "):]
        key, _, value = line.partition("=")
        key = key.strip()
        if key not in required:
            continue
        value = value.strip().strip('"').strip("'")
        os.environ.setdefault(key, value)
    missing = [k for k in required if not os.environ.get(k)]
    if missing:
        raise RuntimeError(f"Fail-closed: required credential(s) not found in {env_path}: {missing}")


def ensure_bars() -> tuple[pd.DataFrame, dict]:
    """Fetch (or reuse this run's own prior fetch of) real Alpaca bars,
    feed=sip, for the frozen universe/window/asof. Never fabricates missing
    history. Fetched ONCE for the whole wave (SHARED BARS/TARGETS)."""
    cached_bars = RUN_ROOT / "raw_bars.csv"
    cached_manifest = RUN_ROOT / "bars_provenance_manifest.json"
    if cached_bars.exists() and cached_manifest.exists():
        return pd.read_csv(cached_bars), json.loads(cached_manifest.read_text(encoding="utf-8"))
    _load_paper_credentials_into_env()
    result = extract_research_bars_with_provenance(
        symbols=SYMBOLS, start_utc=START_UTC, end_utc=END_UTC, timeframe=TIMEFRAME, asof=ASOF, feed=FEED
    )
    RUN_ROOT.mkdir(parents=True, exist_ok=True)
    result["bars"].to_csv(cached_bars, index=False)
    cached_manifest.write_text(json.dumps(result["manifest"], sort_keys=True, indent=2, default=str), encoding="utf-8")
    return result["bars"], result["manifest"]


def build_targets(bars: pd.DataFrame, *, horizon_bars: int, ret_threshold: float) -> pd.DataFrame:
    """fwd_ret = log(close[t+horizon]/close[t]); label_end_ts is the
    truthful inclusive timestamp of the bar whose close completes the
    label. Classification LABEL only -- never executable P&L."""
    bars = bars.copy()
    bars["end_ts"] = pd.to_datetime(bars["end_ts"], utc=True)
    bars = bars.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)

    rows = []
    for sym, g in bars.groupby("symbol", sort=True):
        g = g.reset_index(drop=True)
        closes = g["close"].astype(np.float64).to_numpy()
        ts_list = g["end_ts"].to_numpy()
        n = len(g)
        for j in range(n - horizon_bars):
            k = j + horizon_bars
            c0, c1 = float(closes[j]), float(closes[k])
            if c0 <= 0.0 or c1 <= 0.0:
                continue
            fwd_ret = float(np.log(c1 / c0))
            rows.append(
                {
                    "symbol": sym,
                    "end_ts": str(pd.Timestamp(ts_list[j])),
                    "fwd_ret": fwd_ret,
                    "target": 1 if fwd_ret > ret_threshold else 0,
                    "label_end_ts": pd.Timestamp(ts_list[k]).isoformat(),
                }
            )
    out = pd.DataFrame(rows)
    if out.empty:
        raise RuntimeError("No labeled rows produced -- check symbol/date coverage")
    return out.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)


def ensure_real_targets(bars: pd.DataFrame) -> pd.DataFrame:
    cached = RUN_ROOT / "real_targets.csv"
    if cached.exists():
        return pd.read_csv(cached)
    targets = build_targets(bars, horizon_bars=LABEL_HORIZON_BARS, ret_threshold=LABEL_RET_THRESHOLD)
    RUN_ROOT.mkdir(parents=True, exist_ok=True)
    targets.to_csv(cached, index=False)
    return targets


def ensure_placebo_targets(real_targets: pd.DataFrame) -> pd.DataFrame:
    cached = RUN_ROOT / "placebo_targets.csv"
    if cached.exists():
        return pd.read_csv(cached)
    placebo = build_causal_placebo_targets(real_targets, seed=PLACEBO_SEED)
    RUN_ROOT.mkdir(parents=True, exist_ok=True)
    placebo.to_csv(cached, index=False)
    return placebo


def ensure_full_features(bars: pd.DataFrame) -> pd.DataFrame:
    """DEFAULT FeatureSetV1Spec already produces ret_rank_20 (default
    cross_section_windows=(5,20)), ret_5 (default ret_windows includes 5),
    and gap_pct_1 (always computed) -- no spec customization required."""
    cached = RUN_ROOT / "full_features.csv"
    if cached.exists():
        return pd.read_csv(cached)
    feats = build_feature_set_v1(bars)
    RUN_ROOT.mkdir(parents=True, exist_ok=True)
    feats.to_csv(cached, index=False)
    return feats


def isolate_feature(features: pd.DataFrame, feature_column: str) -> pd.DataFrame:
    """FEATURE ISOLATION INVARIANT: select ONLY symbol, end_ts, and the one
    declared feature column from the full FeatureSetV1 output."""
    missing = [c for c in ("symbol", "end_ts", feature_column) if c not in features.columns]
    if missing:
        raise RuntimeError(f"feature isolation failed: FeatureSetV1 output missing required column(s) {missing}")
    return features[["symbol", "end_ts", feature_column]].copy()


def assert_single_feature_schema(schema_path: Path, expected_column: str) -> None:
    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    actual = schema.get("feature_columns")
    if actual != [expected_column]:
        raise RuntimeError(
            f"FEATURE ISOLATION INVARIANT VIOLATED: expected feature_columns == "
            f"{[expected_column]!r}, got {actual!r} (schema={schema_path})"
        )


def write_run_dir(run_dir: Path, bars: pd.DataFrame, isolated_features: pd.DataFrame,
                   targets: pd.DataFrame, *, feature_column: str) -> Path:
    run_dir.mkdir(parents=True, exist_ok=True)
    bars_path = run_dir / "bars.csv"
    bars_out = bars.copy()
    bars_out["end_ts"] = pd.to_datetime(bars_out["end_ts"], utc=True).map(lambda t: t.isoformat())
    bars_out.to_csv(bars_path, index=False)

    feat_clean = isolated_features.dropna(axis=0, how="any")
    common_keys = feat_clean[["symbol", "end_ts"]].merge(
        targets[["symbol", "end_ts"]], on=["symbol", "end_ts"], how="inner"
    )
    if common_keys.empty:
        raise RuntimeError(
            f"No overlapping non-NaN (symbol,end_ts) rows between isolated features "
            f"({len(feat_clean)} after dropna, {len(isolated_features)} before) and targets ({len(targets)})"
        )
    feat_out = feat_clean.merge(common_keys, on=["symbol", "end_ts"], how="inner")
    targ_out = targets.merge(common_keys, on=["symbol", "end_ts"], how="inner")

    feat_out = feat_out.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)
    targ_out = targ_out.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)

    feat_out.to_csv(run_dir / "features.csv", index=False)
    targ_out.to_csv(run_dir / "targets.csv", index=False)
    schema_path = generate_feature_schema(run_dir, id_columns=["symbol", "end_ts"])
    assert_single_feature_schema(schema_path, feature_column)
    return bars_path


def signal_policy_for(direction: str) -> SignalPolicySpec:
    if direction == "long_only":
        return SignalPolicySpec(
            entry_threshold=LONG_ENTRY_THRESHOLD,
            long_only=True,
            direction_policy=SIGNAL_DIRECTION_POLICY_LONG_ONLY_V1,
            max_gross_exposure=MAX_GROSS_EXPOSURE,
        )
    if direction == "long_short":
        return SignalPolicySpec(
            entry_threshold=LONG_ENTRY_THRESHOLD,
            long_only=False,
            direction_policy=SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
            short_threshold=SHORT_THRESHOLD,
            max_gross_exposure=MAX_GROSS_EXPOSURE,
            # borrow_model left at default -> BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1
        )
    raise ValueError(f"unknown direction policy key: {direction!r}")


def run_one_trial(*, run_dir: Path, experiment_id: str, hypothesis_id: str, strategy_id: str,
                   direction: str, bars_path: Path, bars_provenance: dict) -> dict:
    wf_spec = WalkForwardSpec(
        train_years=WF_TRAIN_YEARS,
        test_months=WF_TEST_MONTHS,
        step_months=WF_STEP_MONTHS,
        holdout_months=WF_HOLDOUT_MONTHS,
        min_rows_per_fold=WF_MIN_ROWS_PER_FOLD,
        purge_enabled=True,
        embargo_seconds=0,
    )
    economic_spec = EconomicWalkForwardSpec(
        signal_policy=signal_policy_for(direction),
        cost_model=CostModelSpec(
            commission_bps_per_side=COMMISSION_BPS_PER_SIDE, slippage_bps_per_side=SLIPPAGE_BPS_PER_SIDE
        ),
        execution_pricing=ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
            slippage_bps=EXECUTION_SLIPPAGE_BPS,
            volatility_mult_bps=EXECUTION_VOLATILITY_MULT_BPS,
        ),
        weight_to_share=WeightToShareSpec(equity_usd=EQUITY_USD),
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
        registry_db=REGISTRY_DB,
        wf_spec=wf_spec,
        steps=300,
    )
    economic_out = json.loads(economic_out_path.read_text(encoding="utf-8"))
    aggregate = economic_out.get("aggregate")
    if aggregate is None:
        raise RuntimeError(
            f"Fail-closed: economic_walk_forward.json missing required 'aggregate' block: {economic_out_path}"
        )
    wf_eval_ref = economic_out.get("inputs", {}).get("walk_forward_eval", {}).get("path")
    holdout_start_utc = None
    folds_generated = folds_used = folds_skipped = None
    if wf_eval_ref:
        wf_eval = json.loads(Path(wf_eval_ref).read_text(encoding="utf-8"))
        holdout_start_utc = wf_eval.get("temporal_contract", {}).get("holdout_start_utc")
        summary = wf_eval.get("summary", {})
        folds_generated = summary.get("folds_total")
        folds_used = summary.get("folds_used")
        if folds_generated is not None and folds_used is not None:
            folds_skipped = folds_generated - folds_used

    return {
        "experiment_id": experiment_id,
        "hypothesis_id": hypothesis_id,
        "direction": direction,
        "trial_id": economic_out["registry"]["trial_id"],
        "economic_eval_id": economic_out["ids"]["economic_eval_id"],
        "economic_walk_forward_json": str(economic_out_path),
        "economic_daily_returns_csv": economic_out.get("outputs", {}).get("economic_daily_returns_csv", {}).get("path"),
        "economic_returns_csv": economic_out.get("outputs", {}).get("economic_returns_csv", {}).get("path"),
        "aggregate": aggregate,
        "holdout": economic_out.get("holdout"),
        "holdout_start_utc": holdout_start_utc,
        "folds_generated": folds_generated,
        "folds_used": folds_used,
        "folds_skipped": folds_skipped,
    }


def run_family(family_key: str) -> dict:
    """Run the 2 real trials (long-only, long-short) plus the 1 matched
    diagnostic placebo trial for one predeclared hypothesis family. Reuses
    the wave-shared bars/features/real-targets/placebo-targets caches."""
    fam = FAMILIES[family_key]
    bars, manifest = ensure_bars()
    real_targets = ensure_real_targets(bars)
    placebo_targets = ensure_placebo_targets(real_targets)
    full_features = ensure_full_features(bars)
    isolated = isolate_feature(full_features, fam.feature_column)

    fam_root = RUN_ROOT / family_key.lower().replace("-", "_")

    run_dir_lo = fam_root / "long_only"
    bars_path_lo = write_run_dir(run_dir_lo, bars, isolated, real_targets, feature_column=fam.feature_column)
    r_lo = run_one_trial(
        run_dir=run_dir_lo, experiment_id=REAL_EXPERIMENT_ID, hypothesis_id=fam.hyp_long_only,
        strategy_id=fam.strategy_id, direction="long_only", bars_path=bars_path_lo, bars_provenance=manifest,
    )

    run_dir_ls = fam_root / "long_short"
    bars_path_ls = write_run_dir(run_dir_ls, bars, isolated, real_targets, feature_column=fam.feature_column)
    r_ls = run_one_trial(
        run_dir=run_dir_ls, experiment_id=REAL_EXPERIMENT_ID, hypothesis_id=fam.hyp_long_short,
        strategy_id=fam.strategy_id, direction="long_short", bars_path=bars_path_ls, bars_provenance=manifest,
    )

    run_dir_pb = fam_root / "placebo"
    bars_path_pb = write_run_dir(run_dir_pb, bars, isolated, placebo_targets, feature_column=fam.feature_column)
    r_pb = run_one_trial(
        run_dir=run_dir_pb, experiment_id=PLACEBO_EXPERIMENT_ID, hypothesis_id=fam.hyp_placebo,
        strategy_id=fam.strategy_id, direction="long_short", bars_path=bars_path_pb, bars_provenance=manifest,
    )

    result = {"family": family_key, "feature_column": fam.feature_column,
              "long_only": r_lo, "long_short": r_ls, "placebo": r_pb}
    fam_root.mkdir(parents=True, exist_ok=True)
    (fam_root / "family_result.json").write_text(
        json.dumps(result, sort_keys=True, indent=2, default=str), encoding="utf-8"
    )
    return result


def verify_oos_date_alignment(trials: list[dict]) -> list[str]:
    """Fail closed unless EVERY trial's own economic_daily_returns.csv date
    column matches every other trial's."""
    if not trials:
        raise RuntimeError("Fail-closed: no trials to verify OOS date alignment against")
    date_sets = {}
    for t in trials:
        csv_path = t.get("economic_daily_returns_csv")
        if not csv_path:
            raise RuntimeError(f"Fail-closed: trial {t['trial_id']} has no economic_daily_returns_csv")
        date_sets[t["hypothesis_id"]] = set(pd.read_csv(csv_path)["date"].astype(str).tolist())

    reference_hyp, reference_dates = next(iter(date_sets.items()))
    for hyp, dates in date_sets.items():
        if dates != reference_dates:
            only_ref = sorted(reference_dates - dates)
            only_this = sorted(dates - reference_dates)
            raise RuntimeError(
                f"Fail-closed: OOS reference dates differ between {reference_hyp} and {hyp} -- "
                f"{reference_hyp}-only={only_ref[:5]} {hyp}-only={only_this[:5]}"
            )

    holdout_starts = {t["holdout_start_utc"] for t in trials if t.get("holdout_start_utc")}
    if len(holdout_starts) != 1:
        raise RuntimeError(f"Fail-closed: trials disagree on holdout_start_utc: {holdout_starts}")

    return sorted(reference_dates)


def economic_fold_date_authority(economic_returns_csv: Path) -> dict:
    econ = pd.read_csv(economic_returns_csv)
    if "fold" not in econ.columns or "timestamp" not in econ.columns:
        raise RuntimeError(f"Fail-closed: {economic_returns_csv} missing required 'fold'/'timestamp' column(s)")
    econ = econ.copy()
    econ["date"] = pd.to_datetime(econ["timestamp"], utc=True).dt.strftime("%Y-%m-%d")
    fold_sets_by_date = econ.groupby("date")["fold"].agg(lambda s: sorted(set(s)))
    ambiguous = {d: fs for d, fs in fold_sets_by_date.items() if len(fs) != 1}
    if ambiguous:
        raise RuntimeError(
            f"Fail-closed: date(s) mapped to more than one economic fold in "
            f"{economic_returns_csv}: {dict(list(ambiguous.items())[:5])}"
        )
    fold_of_date = fold_sets_by_date.map(lambda fs: fs[0])
    reset_dates = set(fold_of_date.reset_index().groupby("fold")["date"].min().tolist())
    return {"date_set": set(fold_of_date.index.tolist()), "reset_dates": reset_dates}


def build_fold_reset_benchmark(
    bars: pd.DataFrame, symbols: list[str], economic_returns_csv: Path,
    reference_dates: list[str], holdout_start_utc: str,
) -> dict:
    """Equal-weight DAILY-REBALANCED benchmark, measured under the SAME
    fold-reset convention the economic strategy itself uses (each fold's
    reset date forced to 0.0). Fails closed on any date/fold mismatch."""
    ref_ts = pd.to_datetime(pd.Index(reference_dates), utc=True)
    holdout_ts = pd.Timestamp(holdout_start_utc)
    if holdout_ts.tzinfo is None:
        holdout_ts = holdout_ts.tz_localize("UTC")
    if (ref_ts >= holdout_ts).any():
        bad = sorted(str(d) for d in ref_ts[ref_ts >= holdout_ts])
        raise RuntimeError(
            f"Fail-closed: {len(bad)} benchmark reference date(s) fall at/after the reserved holdout "
            f"boundary ({holdout_ts.isoformat()}): {bad[:5]}{'...' if len(bad) > 5 else ''}"
        )

    authority = economic_fold_date_authority(economic_returns_csv)
    economic_date_set = authority["date_set"]
    fold_start_dates = authority["reset_dates"]
    reference_date_set = set(pd.Index(reference_dates).astype(str))
    if reference_date_set != economic_date_set:
        only_reference = sorted(reference_date_set - economic_date_set)
        only_economic = sorted(economic_date_set - reference_date_set)
        raise RuntimeError(
            f"Fail-closed: reference date(s) and economic fold-authority date(s) in "
            f"{economic_returns_csv} are not identical -- "
            f"reference_only={only_reference[:5]} economic_only={only_economic[:5]}"
        )

    b = bars.copy()
    b["end_ts"] = pd.to_datetime(b["end_ts"], utc=True)
    b = b.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)
    b["date"] = b["end_ts"].dt.strftime("%Y-%m-%d")
    b["daily_ret"] = b.groupby("symbol")["close"].pct_change()

    scoped = b[b["date"].isin(reference_date_set) & b["symbol"].isin(symbols)]
    per_date = scoped.groupby("date")["daily_ret"].mean().reindex(sorted(reference_date_set))
    for d in fold_start_dates & reference_date_set:
        per_date.loc[d] = 0.0

    missing_dates = sorted(d for d in reference_date_set if d not in per_date.dropna().index)
    daily_series = per_date.dropna()
    cumulative_return = float(np.prod(1.0 + daily_series.to_numpy()) - 1.0) if len(daily_series) else None
    sharpe = compute_sharpe(daily_series) if len(daily_series) else None
    max_drawdown = compute_max_drawdown(daily_series) if len(daily_series) else None

    return {
        "benchmark_type": "equal_weight_daily_rebalanced_fold_reset",
        "reference_date_count": len(reference_dates),
        "reference_date_start": str(min(reference_dates)),
        "reference_date_end": str(max(reference_dates)),
        "fold_reset_dates_count": len(fold_start_dates & reference_date_set),
        "dates_with_no_return_observation": missing_dates,
        "daily_return_observations_used": int(len(daily_series)),
        "cumulative_return_over_reference_dates": cumulative_return,
        "sharpe": sharpe,
        "max_drawdown": max_drawdown,
        "holdout_start_utc": holdout_ts.isoformat(),
    }


def compute_paired_delta(long_only: dict, long_short: dict) -> dict:
    """delta = long_short - long_only, over exact matching OOS dates."""
    lo_daily = pd.read_csv(long_only["economic_daily_returns_csv"])
    ls_daily = pd.read_csv(long_short["economic_daily_returns_csv"])
    lo_turnover = float(lo_daily["turnover"].sum())
    ls_turnover = float(ls_daily["turnover"].sum())

    lo_agg = long_only["aggregate"]
    ls_agg = long_short["aggregate"]

    def _delta(key):
        a, b = lo_agg.get(key), ls_agg.get(key)
        if a is None or b is None:
            return None
        return float(b) - float(a)

    return {
        "delta_net_total_return": _delta("net_total_return"),
        "delta_net_sharpe": _delta("net_sharpe"),
        "delta_max_drawdown": _delta("max_drawdown"),
        "delta_turnover": ls_turnover - lo_turnover,
        "delta_cost_drag": _delta("cost_drag"),
        "long_only_turnover": lo_turnover,
        "long_short_turnover": ls_turnover,
    }


def compute_placebo_delta(long_short: dict, placebo: dict) -> dict:
    """delta = long_short(real) - placebo, over exact matching OOS dates."""
    ls_agg = long_short["aggregate"]
    pb_agg = placebo["aggregate"]

    def _delta(key):
        a, b = pb_agg.get(key), ls_agg.get(key)
        if a is None or b is None:
            return None
        return float(b) - float(a)

    return {
        "delta_net_total_return": _delta("net_total_return"),
        "delta_net_sharpe": _delta("net_sharpe"),
        "delta_max_drawdown": _delta("max_drawdown"),
    }


def run_family_judge() -> dict:
    """PATCH E: run build_multiple_testing_judge over the REAL_EXPERIMENT_ID
    population only (hypothesis_id=None -> full experiment population). No
    placebo trial is registered under REAL_EXPERIMENT_ID, so this judge
    scope structurally cannot include a placebo."""
    judge = build_multiple_testing_judge(experiment_id=REAL_EXPERIMENT_ID, registry_db=REGISTRY_DB)
    (RUN_ROOT / "judge_artifact.json").write_text(
        json.dumps(judge, sort_keys=True, indent=2, default=str), encoding="utf-8"
    )
    return judge


def main(argv: list[str] | None = None) -> None:
    argv = sys.argv[1:] if argv is None else argv
    if not argv:
        print("usage: run_wave.py {short02|short03|short04|judge|check}", file=sys.stderr)
        raise SystemExit(2)
    stage = argv[0]
    assert_driver_agrees_with_predeclaration()
    if stage == "check":
        print("PREDECLARATION_AGREEMENT=PASS")
        return
    if stage in ("short02", "short03", "short04"):
        key = {"short02": "SHORT-02", "short03": "SHORT-03", "short04": "SHORT-04"}[stage]
        result = run_family(key)
        print(json.dumps({"family": key, "trial_ids": {
            "long_only": result["long_only"]["trial_id"],
            "long_short": result["long_short"]["trial_id"],
            "placebo": result["placebo"]["trial_id"],
        }}, indent=2))
        return
    if stage == "judge":
        judge = run_family_judge()
        print(json.dumps({
            "judge_status": judge.get("judge_status"),
            "registry_population": judge.get("registry_population"),
            "included_trial_ids": judge.get("included_trial_ids"),
            "excluded_trial_ids": judge.get("excluded_trial_ids"),
        }, indent=2, default=str))
        return
    print(f"unknown stage: {stage!r}", file=sys.stderr)
    raise SystemExit(2)


if __name__ == "__main__":
    main()
