"""SHORT-01-ETF-LONG-SHORT-TIME-SERIES-TREND -- development-stage Research
experiment driver.

Tests whether allowing a pooled, single-feature ETF ABSOLUTE-TREND
classifier (each instrument's own 60-trading-day log-price slope) to
express bearish views via SHORT positions materially improves the SAME
underlying signal relative to an identical-signal LONG-ONLY control, using
the existing registered Research -> economic walk-forward -> multiple-
testing pipeline. Includes a causal same-horizon (fwd_ret,target) pair-
permutation negative control (Trial C) sharing Trial B's economic policy.

Truthful terminology: this is a POOLED single-feature ETF absolute-trend
classifier, NOT a canonical Moskowitz/Ooi/Pedersen TSMOM implementation --
the model is trained across pooled ETF observations even though the
predictor for every row is that instrument's own trailing slope.

This is development-stage Research only. It does NOT authorize Paper or
Live shorting and does not modify any production Paper/runtime/broker
behavior. `research_assumed_shortable_universe_v1` (the frozen Research
long/short borrow model -- see economic_walkforward.py) is a Research-only
scope declaration, not actual historical broker shortability proof.

Uses ONLY existing, frozen, already-accepted production entry points:
  - mqk_research.data.alpaca_historical.extract_research_bars_with_provenance
  - mqk_research.features.feature_set_v1.build_feature_set_v1
  - mqk_research.ml.economic_registry_integration.run_registered_economic_walkforward_eval
  - mqk_research.ml.multiple_testing_judge.build_multiple_testing_judge

No research-py source file is modified by this script. Feature isolation
(selecting only slope_60) is done in THIS driver, after calling the
unmodified build_feature_set_v1, never inside feature_set_v1.py itself.

The causal placebo helper (`build_causal_placebo_targets`) is an exact,
self-contained reproduction of the accepted, PUSHED-VERIFIED
research-alpha-gap-discovery-01-clean worktree's implementation
(research-py/experiments/alpha_discovery_01/run_experiment.py, commit
28497968cada7870efe38295b5712b49b0d32398). It was ORIGINALLY imported live
from that sibling worktree's driver via `importlib`, which made a checkout
of this branch alone non-functional without that exact external worktree
present on disk at a fixed Windows path. SHORT-01-DRIVER-PORTABILITY-01
inlined the function verbatim (same permutation logic, same fail-closed
effectiveness check, same deterministic seeding) so this experiment is
runnable from a bare checkout of this branch. Semantics are unchanged and
verified byte-for-byte identical against the accepted source at inline
time; the accepted worktree/commit remains the attribution source, not a
runtime dependency. This driver's OWN production imports (`mqk_research.*`)
are all resolved from THIS worktree's own src/, so every executed line of
production code comes from this isolated SHORT-01 worktree.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

SHORT01_WORKTREE_SRC = Path(__file__).resolve().parents[2] / "src"
assert SHORT01_WORKTREE_SRC.name == "src" and "short-01" in str(SHORT01_WORKTREE_SRC), (
    f"refusing to run: expected the isolated SHORT-01 worktree's own src/, got {SHORT01_WORKTREE_SRC}"
)
sys.path.insert(0, str(SHORT01_WORKTREE_SRC))

import numpy as np
import pandas as pd

from mqk_research.data.alpaca_historical import extract_research_bars_with_provenance
from mqk_research.features.feature_set_v1 import FeatureSetV1Spec, build_feature_set_v1
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
# Self-contained, exact reproduction of the accepted causal placebo helper
# (research-alpha-gap-discovery-01-clean worktree, commit
# 28497968cada7870efe38295b5712b49b0d32398,
# research-py/experiments/alpha_discovery_01/run_experiment.py). Inlined by
# SHORT-01-DRIVER-PORTABILITY-01 so a bare checkout of THIS branch does not
# require that sibling worktree to exist on disk. Semantics unchanged.
# ---------------------------------------------------------------------------


def build_causal_placebo_targets(targets: pd.DataFrame, *, seed: int) -> pd.DataFrame:
    """Deterministic negative control that destroys symbol-specific
    predictive association WITHOUT moving information across temporal label
    horizons: permutes the (fwd_ret, target) PAIR only within rows sharing
    the EXACT same (end_ts, label_end_ts). `symbol`/`end_ts` identity and
    `label_end_ts` are left untouched for every row; only which row receives
    which (fwd_ret, target) outcome changes. A group of size 1 has no other
    row to swap with, so it is left unchanged (a size-1 permutation is the
    identity). Group iteration is over `sorted()` (end_ts, label_end_ts) key
    tuples so the result is reproducible across runs/platforms for a fixed
    seed, independent of pandas groupby internal ordering.

    Fail-closed effectiveness check: the classifier consumes `target`, so the
    negative control is only valid if at least one row's `target` actually
    changes from the ORIGINAL input. A valid input can contain groups whose
    target values are all identical, in which case fwd_ret/target pairs get
    permuted (indices/pair identities move) while every target VALUE stays
    the same -- an ineffective placebo the classifier cannot distinguish
    from the real labels. Raise RuntimeError in that case instead of
    silently returning an ineffective negative control.
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

EXPERIMENT_ID = "SHORT-01-ETF-LONG-SHORT-TIME-SERIES-TREND"
HYPOTHESIS_ID_LONG_ONLY = "short01_etf_absolute_trend_long_only_control_v1"
HYPOTHESIS_ID_LONG_SHORT = "short01_etf_absolute_trend_long_short_v1"
HYPOTHESIS_ID_PLACEBO = "short01_etf_absolute_trend_long_short_causal_placebo_v1"
STRATEGY_ID = "pooled_single_feature_etf_absolute_trend_classifier_v1"

# Fixed ex-ante ETF universe, frozen BEFORE seeing any experiment results.
SYMBOLS = [
    "SPY", "QQQ", "IWM", "DIA", "XLF", "XLK",
    "XLE", "XLV", "XLI", "XLY", "XLP", "XLU",
]

START_UTC = pd.Timestamp("2016-01-01T00:00:00Z")
END_UTC = pd.Timestamp("2024-01-01T00:00:00Z")
ASOF = "2024-01-01"
TIMEFRAME = "1Day"
FEED = "sip"  # explicit SEMANTIC DATA-SOURCE CHOICE -- production DEFAULT_FEED ("iex") is untouched.

LABEL_HORIZON_BARS = 20
LABEL_RET_THRESHOLD = 0.0

TREND_WINDOW = 60
REQUIRED_FEATURE_COLUMNS = ["slope_60"]

LONG_ENTRY_THRESHOLD = 0.55
SHORT_THRESHOLD = 0.45
MAX_GROSS_EXPOSURE = 1.0
PLACEBO_SEED = 1234

COMMISSION_BPS_PER_SIDE = 10.0  # CONSERVATIVE COST ASSUMPTION, not actual Alpaca commission.
SLIPPAGE_BPS_PER_SIDE = 0.0
EXECUTION_SLIPPAGE_BPS = 5
EXECUTION_VOLATILITY_MULT_BPS = 0
EQUITY_USD = 100_000.0

WF_TRAIN_YEARS = 3
WF_TEST_MONTHS = 3
WF_STEP_MONTHS = 3
WF_HOLDOUT_MONTHS = 6
WF_MIN_ROWS_PER_FOLD = 300


def fetch_bars() -> tuple[pd.DataFrame, dict]:
    """Real Alpaca bars for the frozen ETF universe/window/asof, feed=sip.
    Reuses this run's own prior fetch when present -- else performs a fresh
    official extraction. Never fabricates missing history."""
    cached_bars = RUN_ROOT / "raw_bars.csv"
    cached_manifest = RUN_ROOT / "bars_provenance_manifest.json"
    if cached_bars.exists() and cached_manifest.exists():
        print(f"      (reusing cached bars/manifest from {RUN_ROOT})")
        return pd.read_csv(cached_bars), json.loads(cached_manifest.read_text(encoding="utf-8"))
    result = extract_research_bars_with_provenance(
        symbols=SYMBOLS, start_utc=START_UTC, end_utc=END_UTC, timeframe=TIMEFRAME, asof=ASOF, feed=FEED
    )
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
                    # must match feature_set_v1's own end_ts string
                    # convention EXACTLY -- eval_walkforward.py merges
                    # features.csv/targets.csv on the raw CSV string.
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


def isolate_slope_feature(features: pd.DataFrame) -> pd.DataFrame:
    """FEATURE ISOLATION INVARIANT: select ONLY symbol, end_ts, slope_60
    from the full FeatureSetV1 output -- the same invariant independently
    enforced in corrected ALPHA-01."""
    missing = [c for c in ("symbol", "end_ts", "slope_60") if c not in features.columns]
    if missing:
        raise RuntimeError(f"feature isolation failed: FeatureSetV1 output missing required column(s) {missing}")
    return features[["symbol", "end_ts", "slope_60"]].copy()


def assert_single_feature_schema(schema_path: Path) -> None:
    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    actual = schema.get("feature_columns")
    if actual != REQUIRED_FEATURE_COLUMNS:
        raise RuntimeError(
            f"FEATURE ISOLATION INVARIANT VIOLATED: expected feature_columns == "
            f"{REQUIRED_FEATURE_COLUMNS!r}, got {actual!r} (schema={schema_path})"
        )
    if len(actual) != 1:
        raise RuntimeError(f"FEATURE_COLUMN_COUNT invariant violated: expected 1, got {len(actual)}")


def write_run_dir(run_dir: Path, bars: pd.DataFrame, isolated_features: pd.DataFrame, targets: pd.DataFrame) -> Path:
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
    assert_single_feature_schema(schema_path)
    return bars_path


def _signal_policy_for(hypothesis_id: str) -> SignalPolicySpec:
    if hypothesis_id == HYPOTHESIS_ID_LONG_ONLY:
        return SignalPolicySpec(
            entry_threshold=LONG_ENTRY_THRESHOLD,
            long_only=True,
            direction_policy=SIGNAL_DIRECTION_POLICY_LONG_ONLY_V1,
            max_gross_exposure=MAX_GROSS_EXPOSURE,
        )
    return SignalPolicySpec(
        entry_threshold=LONG_ENTRY_THRESHOLD,
        long_only=False,
        direction_policy=SIGNAL_DIRECTION_POLICY_LONG_SHORT_THRESHOLD_V1,
        short_threshold=SHORT_THRESHOLD,
        max_gross_exposure=MAX_GROSS_EXPOSURE,
        # borrow_model left at default -> BORROW_MODEL_RESEARCH_ASSUMED_SHORTABLE_UNIVERSE_V1
    )


def run_one_trial(*, run_dir: Path, hypothesis_id: str, bars_path: Path, bars_provenance: dict) -> dict:
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
        signal_policy=_signal_policy_for(hypothesis_id),
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
        experiment_id=EXPERIMENT_ID,
        hypothesis_id=hypothesis_id,
        strategy_id=STRATEGY_ID,
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
        "hypothesis_id": hypothesis_id,
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


def build_benchmark_over_oos_dates(
    bars: pd.DataFrame, symbols: list[str], reference_dates: list[str], holdout_start_utc: str
) -> dict:
    """Equal-weight DAILY-REBALANCED benchmark of the fixed 12-ETF universe,
    built ONLY over the exact reference_dates the admitted trials actually
    used. Fails closed if any reference date falls at/after the reserved
    holdout boundary.

    CONTINUOUS_DATE_ALIGNED_CONTEXT_BENCHMARK: pct_change is computed before
    OOS filtering, so each fold's first OOS date carries a return from the
    prior close -- which may be a pre-fold or cross-fold-gap date the
    strategy itself was never economically exposed to (it starts every fold
    flat). This is a same-date-range CONTEXT comparator, not an
    apples-to-apples measurement-convention match; use
    `build_fold_reset_benchmark` for the direct comparator."""
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

    b = bars.copy()
    b["end_ts"] = pd.to_datetime(b["end_ts"], utc=True)
    b = b.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)
    b["date"] = b["end_ts"].dt.strftime("%Y-%m-%d")
    b["daily_ret"] = b.groupby("symbol")["close"].pct_change()

    ref_date_strs = set(pd.Index(reference_dates).astype(str))
    scoped = b[b["date"].isin(ref_date_strs) & b["symbol"].isin(symbols)]
    per_date = scoped.groupby("date")["daily_ret"].mean().reindex(sorted(ref_date_strs))

    missing_dates = sorted(d for d in ref_date_strs if d not in per_date.dropna().index)
    daily_series = per_date.dropna()
    cumulative_return = float(np.prod(1.0 + daily_series.to_numpy()) - 1.0) if len(daily_series) else None

    return {
        "benchmark_type": "equal_weight_daily_rebalanced",
        "reference_date_count": len(reference_dates),
        "reference_date_start": str(min(reference_dates)),
        "reference_date_end": str(max(reference_dates)),
        "dates_with_no_return_observation": missing_dates,
        "daily_return_observations_used": int(len(daily_series)),
        "cumulative_return_over_reference_dates": cumulative_return,
        "holdout_start_utc": holdout_ts.isoformat(),
    }


def economic_fold_date_authority(economic_returns_csv: Path) -> dict:
    """Reads the trial's own economic_returns.csv ONCE (`fold`, `timestamp`
    columns) -- the exact fold/date rows whose daily aggregation produced
    the strategy's actual economic return series -- and returns:
      - "date_set": every calendar date the economic engine measured
      - "reset_dates": each fold's reset date (the minimum actual economic
        date belonging to that fold, i.e. the date the strategy starts flat)

    This is the correct fold/date authority, NOT
    walk_forward_oos_predictions.csv: LABEL_HORIZON_BARS reserves the last
    LABEL_HORIZON_BARS economic dates of every fold from ever carrying a
    forward-label OOS prediction row (preserving the reserved holdout), so
    walk_forward_oos_predictions.csv is structurally incomplete as fold/date
    authority for the real run. Fails closed if any calendar date maps to
    more than one fold."""
    econ = pd.read_csv(economic_returns_csv)
    if "fold" not in econ.columns or "timestamp" not in econ.columns:
        raise RuntimeError(
            f"Fail-closed: {economic_returns_csv} missing required 'fold'/'timestamp' column(s)"
        )
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
    bars: pd.DataFrame,
    symbols: list[str],
    economic_returns_csv: Path,
    reference_dates: list[str],
    holdout_start_utc: str,
) -> dict:
    """Equal-weight DAILY-REBALANCED benchmark of the fixed ETF universe,
    measured under the SAME fold-reset convention the economic strategy
    itself uses: every fold starts flat, so the benchmark's return on each
    fold's reset date (the minimum actual economic date of that fold) is
    forced to 0.0 rather than a pct_change carried over from the prior close
    (which may be a pre-fold or cross-fold-gap date not economically held by
    the strategy on that day). All other dates use the normal
    daily-rebalanced benchmark return. Never touches holdout dates. Fails
    closed on any date/fold mismatch, including reference dates the economic
    fold authority does not also cover (and vice versa) -- exact equality is
    required, not merely a subset."""
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


def verify_oos_date_alignment(trials: list[dict]) -> list[str]:
    """Fail closed unless EVERY trial's own economic_daily_returns.csv date
    column matches every other trial's -- required for the paired A-vs-B
    control and for a single shared benchmark to be meaningful."""
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


def compute_paired_delta(long_only: dict, long_short: dict) -> dict:
    """delta = long_short - long_only, over the exact matching OOS dates
    (already verified by verify_oos_date_alignment)."""
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


def main() -> None:
    print(f"[1/7] fetching real Alpaca bars (feed={FEED}) for {len(SYMBOLS)} ETFs, {START_UTC.date()} - {END_UTC.date()} ...")
    bars, manifest = fetch_bars()
    print(f"      bars fetched: {len(bars)} rows, columns={list(bars.columns)}")
    RUN_ROOT.mkdir(parents=True, exist_ok=True)
    bars.to_csv(RUN_ROOT / "raw_bars.csv", index=False)
    (RUN_ROOT / "bars_provenance_manifest.json").write_text(
        json.dumps(manifest, sort_keys=True, indent=2, default=str), encoding="utf-8"
    )
    print(f"REQUESTED_START={START_UTC.isoformat()}")
    print(f"DATA_FEED={FEED}")
    print(f"RETURNED_START={bars['end_ts'].min() if len(bars) else None}")
    print(f"RETURNED_END={bars['end_ts'].max() if len(bars) else None}")

    print(f"[2/7] building Feature Set v1 (trend_window={TREND_WINDOW}) then isolating slope_{TREND_WINDOW} ONLY ...")
    full_features = build_feature_set_v1(bars, spec=FeatureSetV1Spec(trend_window=TREND_WINDOW))
    isolated_features = isolate_slope_feature(full_features)
    print(f"      full feature set: {len(full_features.columns)} columns; isolated to: {list(isolated_features.columns)}")
    print(f"FEATURE_COLUMNS={REQUIRED_FEATURE_COLUMNS}")
    print(f"FEATURE_COLUMN_COUNT={len(REQUIRED_FEATURE_COLUMNS)}")
    print(f"FIXED_UNIVERSE={SYMBOLS}")
    print("FINAL_HOLDOUT_POLICY=RESERVED")

    print(f"[3/7] building targets (fwd_ret over {LABEL_HORIZON_BARS} bars, threshold={LABEL_RET_THRESHOLD}) ...")
    targets = build_targets(bars, horizon_bars=LABEL_HORIZON_BARS, ret_threshold=LABEL_RET_THRESHOLD)
    print(f"      targets: {len(targets)} rows, positive rate={targets['target'].mean():.3f}")
    print(f"LABEL_HORIZON_BARS={LABEL_HORIZON_BARS}")

    results = []

    print("[4/7] running TRIAL A (long-only control) ...")
    run_dir_a = RUN_ROOT / "trial_a_long_only"
    bars_path_a = write_run_dir(run_dir_a, bars, isolated_features, targets)
    r_a = run_one_trial(run_dir=run_dir_a, hypothesis_id=HYPOTHESIS_ID_LONG_ONLY, bars_path=bars_path_a, bars_provenance=manifest)
    print(f"      trial_a: trial_id={r_a['trial_id'][:16]}...")
    results.append(r_a)

    print("[4/7] running TRIAL B (long/short candidate) ...")
    run_dir_b = RUN_ROOT / "trial_b_long_short"
    bars_path_b = write_run_dir(run_dir_b, bars, isolated_features, targets)
    r_b = run_one_trial(run_dir=run_dir_b, hypothesis_id=HYPOTHESIS_ID_LONG_SHORT, bars_path=bars_path_b, bars_provenance=manifest)
    print(f"      trial_b: trial_id={r_b['trial_id'][:16]}...")
    results.append(r_b)

    print("[5/7] running TRIAL C (causal same-horizon placebo, long/short economics) ...")
    shuffled_targets = build_causal_placebo_targets(targets, seed=PLACEBO_SEED)
    run_dir_c = RUN_ROOT / "trial_c_placebo"
    bars_path_c = write_run_dir(run_dir_c, bars, isolated_features, shuffled_targets)
    r_c = run_one_trial(run_dir=run_dir_c, hypothesis_id=HYPOTHESIS_ID_PLACEBO, bars_path=bars_path_c, bars_provenance=manifest)
    print(f"      trial_c: trial_id={r_c['trial_id'][:16]}...")
    results.append(r_c)

    print("[6/7] building multiple-testing judge over the full experiment population ...")
    judge = build_multiple_testing_judge(experiment_id=EXPERIMENT_ID, registry_db=REGISTRY_DB)
    (RUN_ROOT / "judge_artifact.json").write_text(
        json.dumps(judge, sort_keys=True, indent=2, default=str), encoding="utf-8"
    )

    included = set(judge.get("included_trial_ids") or [])
    excluded_by_id = {e["trial_id"]: e["reason"] for e in (judge.get("excluded_trial_ids") or [])}
    trial_admission = {}
    for r in results:
        tid = r["trial_id"]
        if tid in included:
            trial_admission[tid] = "ADMITTED"
        elif tid in excluded_by_id:
            trial_admission[tid] = f"EXCLUDED:{excluded_by_id[tid]}"
        else:
            trial_admission[tid] = "EXCLUDED:not_in_judge_population"
    print(f"      registered_unique_trials={judge.get('registry_population', {}).get('registered_unique_trials')}")
    print(f"      admitted={sum(1 for v in trial_admission.values() if v == 'ADMITTED')} "
          f"excluded={sum(1 for v in trial_admission.values() if v != 'ADMITTED')}")

    print("[7/7] verifying OOS date alignment across all 3 trials and building benchmark ...")
    reference_dates = verify_oos_date_alignment(results)
    holdout_start_utc = results[0]["holdout_start_utc"]
    bench = build_benchmark_over_oos_dates(bars, SYMBOLS, reference_dates, holdout_start_utc)
    if not r_a.get("economic_returns_csv"):
        raise RuntimeError("Fail-closed: trial_a has no recorded economic_returns_csv output path")
    fold_reset_bench = build_fold_reset_benchmark(
        bars, SYMBOLS, Path(r_a["economic_returns_csv"]), reference_dates, holdout_start_utc
    )
    print(f"BENCHMARK_TYPE={bench['benchmark_type']}")
    print(f"BENCHMARK_EXACT_DATE_ALIGNMENT=PASS")
    print(f"OOS_REFERENCE_DATE_START={bench['reference_date_start']}")
    print(f"OOS_REFERENCE_DATE_END={bench['reference_date_end']}")
    print(f"OOS_REFERENCE_DATE_COUNT={bench['reference_date_count']}")
    print(f"FOLD_RESET_BENCHMARK_TYPE={fold_reset_bench['benchmark_type']}")
    print(f"FOLD_RESET_BENCHMARK_CUMULATIVE_RETURN={fold_reset_bench['cumulative_return_over_reference_dates']}")
    print(f"FOLD_RESET_BENCHMARK_SHARPE={fold_reset_bench['sharpe']}")
    print(f"FOLD_RESET_BENCHMARK_MAX_DRAWDOWN={fold_reset_bench['max_drawdown']}")

    paired_delta = compute_paired_delta(r_a, r_b)

    final = {
        "experiment_id": EXPERIMENT_ID,
        "run_id": "run_01",
        "feature_columns": REQUIRED_FEATURE_COLUMNS,
        "universe": SYMBOLS,
        "date_range_utc": [str(START_UTC), str(END_UTC)],
        "data_feed": FEED,
        "label_horizon_bars": LABEL_HORIZON_BARS,
        "label_ret_threshold": LABEL_RET_THRESHOLD,
        "trials": results,
        "judge_status": judge.get("judge_status"),
        "pbo_result": judge.get("pbo_result"),
        "dsr_results": judge.get("dsr_results"),
        "dsr_trial_accounting": judge.get("dsr_trial_accounting"),
        "registry_population": judge.get("registry_population"),
        "trial_admission": trial_admission,
        "benchmark_over_oos_reference_dates": bench,
        "fold_reset_benchmark_over_oos_reference_dates": fold_reset_bench,
        "paired_long_short_vs_long_only_delta": paired_delta,
        "long_short_attribution": "UNKNOWN_NEEDS_PROOF",
        "long_short_attribution_missing_evidence": (
            "economic_walk_forward.py's _daily_aggregate()/_simulate_fold() pool every symbol's "
            "gross_contrib/turnover/transaction_cost into a single per-date scalar series before "
            "economic_daily_returns.csv is written (see gross_by_ts/turnover_by_ts accumulation, "
            "economic_walkforward.py ~L1600-1725). Per-symbol SIGNED executed_weight is computed "
            "internally (exposure_frame, _simulate_fold_execution) but is never persisted per-symbol "
            "to any output artifact, so long-leg vs short-leg gross/net return, active days, turnover, "
            "and cost drag cannot be derived from the registered economic artifact schema as it exists "
            "today without inventing attribution. This is the missing evidence seam, not a defect."
        ),
        "borrow_model": "research_assumed_shortable_universe_v1",
        "borrow_model_limitation": (
            "Research long/short engine assumes the evaluated universe is shortable at every scored "
            "bar; it has no point-in-time historical borrow availability, easy/hard-to-borrow state, "
            "locate availability, borrow fee, or recall risk. A positive result here is at most "
            "DEVELOPMENT_PROMISING_WITH_BORROW_MODEL_LIMITATION, never promotion-ready evidence."
        ),
    }
    (RUN_ROOT / "final_report.json").write_text(
        json.dumps(final, sort_keys=True, indent=2, default=str), encoding="utf-8"
    )
    print(f"\nDONE. Full report written to: {RUN_ROOT / 'final_report.json'}")
    print(json.dumps({
        "judge_status": final["judge_status"],
        "pbo_status": (final["pbo_result"] or {}).get("status"),
        "n_trials": len(results),
        "trial_admission": trial_admission,
        "paired_delta": paired_delta,
        "benchmark": bench,
        "fold_reset_benchmark": fold_reset_bench,
    }, indent=2, default=str))


if __name__ == "__main__":
    main()
