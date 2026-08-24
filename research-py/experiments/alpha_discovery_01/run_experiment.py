"""RESEARCH-BACKTEST-ALPHA-GAP-AND-DISCOVERY-01 -- Phase 5 development-stage
experiment driver, CORRECTED (run_02) per independent-review repair
ALPHA-DISCOVERY-01-INDEPENDENT-REVIEW-REPAIR-01.

run_01 (preserved on disk under runs/run_01/, untouched) is
INVALID_FOR_STATED_HYPOTHESIS: it wrote the ENTIRE FeatureSetV1 feature
matrix to features.csv, so generate_feature_schema declared every non-ID
column a model feature and the classifier was never a single-feature
momentum test. See docs/research/ALPHA_DISCOVERY_01_REPORT.md for the full
accounting.

This driver (run_02) tests ALPHA-01 as originally intended: a fold-trained
SINGLE-FEATURE logistic classifier on feature_set_v1's momentum_score
(= 0.5*ret_rank_20 + 0.5*slope_rank_20), predicting P(forward 10-bar log
return > 0), gated by SignalPolicy MODEL-PROBABILITY entry thresholds (not
momentum-rank thresholds), on REAL US-equity daily bars pulled directly from
Alpaca via the official, already-accepted `extract_research_bars_with_provenance`
research-data authority (adjustment=all, corporate-action fail-closed).

Uses ONLY existing, frozen, already-accepted production entry points:
  - mqk_research.data.alpaca_historical.extract_research_bars_with_provenance
  - mqk_research.features.feature_set_v1.build_feature_set_v1
  - mqk_research.ml.economic_registry_integration.run_registered_economic_walkforward_eval
  - mqk_research.ml.multiple_testing_judge.build_multiple_testing_judge

No research-py source file is modified by this script. Feature isolation
(selecting only momentum_score) is done in THIS driver, after calling the
unmodified build_feature_set_v1, never inside feature_set_v1.py itself.

Registers REAL trials into a dedicated, disposable SQLite registry under
run_02's own run-root -- shares no state with run_01's registry or any other
registry in the repository. Includes one genuine negative control: a
CAUSAL same-horizon (fwd_ret, target) pair-permutation placebo (registered
as its own hypothesis_id, same experiment), whose ACTUAL judge
admission/exclusion status is read from the produced judge artifact and
reported truthfully, never assumed.

PLACEBO REPAIR (ALPHA-DISCOVERY-01-CAUSAL-PLACEBO-01): the original run_02
placebo globally permuted `target` across the entire dataset while leaving
`end_ts`/`label_end_ts` unchanged, which can map a holdout-derived outcome
into a discovery/training row. `build_causal_placebo_targets` instead
permutes the (fwd_ret, target) PAIR only within rows sharing the exact same
(end_ts, label_end_ts) -- this randomizes which symbol receives which
same-horizon outcome without ever moving information across temporal label
horizons or the reserved holdout boundary. See
research-py/experiments/alpha_discovery_01/test_causal_placebo.py for the
negative-control proofs.

The benchmark is built ONLY from the exact OOS reference dates the real
admitted trials/judge actually used (see build_benchmark_over_oos_dates) --
it is not a same-window guess.

Never asserts "PROVEN_ALPHA". Reports REJECTED / INCONCLUSIVE /
DEVELOPMENT_PROMISING per trial only.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

ALPHA_WORKTREE_SRC = Path(__file__).resolve().parents[2] / "src"
assert ALPHA_WORKTREE_SRC.name == "src" and "alpha-discovery" in str(ALPHA_WORKTREE_SRC), (
    f"refusing to run: expected the isolated alpha-discovery worktree's own src/, got {ALPHA_WORKTREE_SRC}"
)
sys.path.insert(0, str(ALPHA_WORKTREE_SRC))

import numpy as np
import pandas as pd

from mqk_research.data.alpaca_historical import extract_research_bars_with_provenance
from mqk_research.features.feature_set_v1 import FeatureSetV1Spec, build_feature_set_v1
from mqk_research.ml.economic_registry_integration import run_registered_economic_walkforward_eval
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

EXPERIMENT_ROOT = Path(__file__).resolve().parent
RUN_01_ROOT = EXPERIMENT_ROOT / "runs" / "run_01"  # preserved, read-only historical evidence
RUN_ROOT = EXPERIMENT_ROOT / "runs" / "run_02"
REGISTRY_DB = RUN_ROOT / "registry" / "research.sqlite3"

EXPERIMENT_ID = "ALPHA-DISCOVERY-01-MOMENTUM-20D-SINGLE-FEATURE-V2"
HYPOTHESIS_ID_REAL = "cross_sectional_momentum_20d_single_feature_v2"
HYPOTHESIS_ID_PLACEBO = "cross_sectional_momentum_20d_single_feature_shuffled_label_placebo_v2"
STRATEGY_ID = "cross_sectional_momentum_20d_single_feature_classifier_v2"

# Frozen BEFORE seeing run_02 results -- same 14 symbols run_01 used, narrowed
# from an original 20-symbol universe only after a real, fail-closed
# CorporateActionReviewRequired hit (CSCO, HD, MRK, MSFT, PFE, XOM each carry
# a cash_merger/stock_merger event Alpaca's adjustment=all does not cover).
# Classification: DEVELOPMENT_DIAGNOSTIC_UNIVERSE_WITH_POST_HOC_CA_ELIGIBILITY_HISTORY
# -- useful for pipeline/hypothesis development, NOT sufficient by itself for
# promotion-grade alpha evidence if positive. Do not narrow further on
# performance grounds; if the CA gate finds another unsupported event for
# this frozen set, STOP rather than narrowing again.
SYMBOLS = [
    "AAPL", "JPM", "JNJ", "PG", "KO", "WMT", "DIS",
    "INTC", "VZ", "T", "IBM", "GE", "CAT", "BA",
]
UNIVERSE_CLASSIFICATION = "DEVELOPMENT_DIAGNOSTIC_UNIVERSE_WITH_POST_HOC_CA_ELIGIBILITY_HISTORY"

START_UTC = pd.Timestamp("2017-01-01T00:00:00Z")
END_UTC = pd.Timestamp("2024-01-01T00:00:00Z")
ASOF = "2024-01-01"
TIMEFRAME = "1Day"

LABEL_HORIZON_BARS = 10
LABEL_RET_THRESHOLD = 0.0

# MODEL-PROBABILITY entry thresholds (P(forward 10-bar return > 0) from the
# fold-trained single-feature logistic classifier) -- NOT momentum-rank
# thresholds. SignalPolicySpec.entry_threshold gates on the classifier's
# predicted probability, same semantics run_01 actually used; run_01's report
# language calling this a "rank threshold" was imprecise and is corrected here.
REAL_ENTRY_THRESHOLDS = [0.55, 0.60, 0.65]
PLACEBO_SEED = 1234

REQUIRED_FEATURE_COLUMNS = ["momentum_score"]


def fetch_bars() -> tuple[pd.DataFrame, dict]:
    """Real Alpaca bars for the frozen universe/window/asof. Reuses run_01's
    already-fetched, already-validated cache when present (same fixed_ex_ante
    universe/dates/asof -> identical real content; avoids a redundant live
    call for a closed historical window) -- else reuses this run's own prior
    fetch -- else performs a fresh official extraction."""
    for candidate_root in (RUN_ROOT, RUN_01_ROOT):
        cached_bars = candidate_root / "raw_bars.csv"
        cached_manifest = candidate_root / "bars_provenance_manifest.json"
        if cached_bars.exists() and cached_manifest.exists():
            print(f"      (reusing cached bars/manifest from {candidate_root})")
            return pd.read_csv(cached_bars), json.loads(cached_manifest.read_text(encoding="utf-8"))
    result = extract_research_bars_with_provenance(
        symbols=SYMBOLS, start_utc=START_UTC, end_utc=END_UTC, timeframe=TIMEFRAME, asof=ASOF
    )
    return result["bars"], result["manifest"]


def build_targets(bars: pd.DataFrame, *, horizon_bars: int, ret_threshold: float) -> pd.DataFrame:
    """Mirrors label_shadow_intents.py's fwd_ret/label_end_ts math exactly
    (log return over `horizon_bars`, inclusive label_end_ts), applied
    unconditionally to every bar rather than to a shadow-intents subset."""
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
                    # NOTE: must match feature_set_v1's own end_ts string
                    # convention EXACTLY (`g["end_ts"].astype(str)`, i.e.
                    # plain `str(Timestamp)`, space-separated) -- eval_
                    # walkforward.py merges features.csv/targets.csv on the
                    # raw CSV string, not a re-parsed Timestamp, so an
                    # isoformat() ("T"-separated) mismatch silently produces
                    # zero overlap rather than a leakage risk.
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
    """
    rng = np.random.default_rng(seed)
    out = targets.copy().reset_index(drop=True)
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

    out["fwd_ret"] = fwd_ret
    out["target"] = target
    return out


def isolate_momentum_feature(features: pd.DataFrame) -> pd.DataFrame:
    """FEATURE ISOLATION INVARIANT: select ONLY symbol, end_ts, momentum_score
    from the full FeatureSetV1 output. This is what run_01 failed to do --
    generate_feature_schema declares every non-ID column a feature, so
    passing the full frame silently trains on the whole feature matrix."""
    missing = [c for c in ("symbol", "end_ts", "momentum_score") if c not in features.columns]
    if missing:
        raise RuntimeError(f"feature isolation failed: FeatureSetV1 output missing required column(s) {missing}")
    return features[["symbol", "end_ts", "momentum_score"]].copy()


def assert_single_feature_schema(schema_path: Path) -> None:
    """Driver-level fail-closed assertion making accidental future feature
    expansion impossible: the written feature_schema.json must declare
    EXACTLY REQUIRED_FEATURE_COLUMNS, nothing more, nothing less."""
    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    actual = schema.get("feature_columns")
    if actual != REQUIRED_FEATURE_COLUMNS:
        raise RuntimeError(
            f"FEATURE ISOLATION INVARIANT VIOLATED: expected feature_columns == "
            f"{REQUIRED_FEATURE_COLUMNS!r}, got {actual!r} (schema={schema_path})"
        )


def write_run_dir(run_dir: Path, bars: pd.DataFrame, isolated_features: pd.DataFrame, targets: pd.DataFrame) -> Path:
    run_dir.mkdir(parents=True, exist_ok=True)
    bars_path = run_dir / "bars.csv"
    bars_out = bars.copy()
    bars_out["end_ts"] = pd.to_datetime(bars_out["end_ts"], utc=True).map(lambda t: t.isoformat())
    bars_out.to_csv(bars_path, index=False)

    # Inner-join isolated features/targets on (symbol, end_ts) and drop any
    # row with a NaN momentum_score (rolling-window burn-in) -- dropping a
    # not-yet-computable row cannot leak information, it only excludes rows
    # this feature cannot honestly score.
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


def run_one_trial(
    *, run_dir: Path, hypothesis_id: str, entry_threshold: float, bars_path: Path, bars_provenance: dict
) -> dict:
    # NOTE: actual Alpaca (feed=iex, the production default) historical
    # coverage for this account/universe floors at ~2020-07-27 (~3.4 years,
    # not the requested 7) -- see G10 diagnostic in the report: a read-only
    # feed=sip probe this session proved feed=sip returns AAPL data back to
    # 2016-01-04 on the SAME account, so this is an iex-feed history-depth
    # limitation, not an account/subscription block. train_years reduced
    # from a 3-year default to 2 to fit while still reserving a genuine
    # 6-month holdout and several real quarterly OOS test folds -- frozen
    # from run_01, not tuned on run_02 results.
    wf_spec = WalkForwardSpec(
        train_years=2, test_months=3, step_months=3, holdout_months=6, min_rows_per_fold=150
    )
    economic_spec = EconomicWalkForwardSpec(
        signal_policy=SignalPolicySpec(entry_threshold=entry_threshold),
        cost_model=CostModelSpec(commission_bps_per_side=10.0, slippage_bps_per_side=0.0),
        execution_pricing=ExecutionPricingSpec(
            pricing_model_id=EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1,
            slippage_bps=5,
            volatility_mult_bps=0,
        ),
        weight_to_share=WeightToShareSpec(equity_usd=100_000.0),
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
    if wf_eval_ref:
        wf_eval = json.loads(Path(wf_eval_ref).read_text(encoding="utf-8"))
        holdout_start_utc = wf_eval.get("temporal_contract", {}).get("holdout_start_utc")

    return {
        "hypothesis_id": hypothesis_id,
        "entry_threshold": entry_threshold,
        "trial_id": economic_out["registry"]["trial_id"],
        "economic_eval_id": economic_out["ids"]["economic_eval_id"],
        "economic_walk_forward_json": str(economic_out_path),
        "economic_daily_returns_csv": economic_out.get("outputs", {}).get("economic_daily_returns_csv", {}).get("path"),
        "aggregate": aggregate,
        "holdout": economic_out.get("holdout"),
        "holdout_start_utc": holdout_start_utc,
    }


def build_benchmark_over_oos_dates(
    bars: pd.DataFrame, symbols: list[str], reference_dates: list[str], holdout_start_utc: str
) -> dict:
    """Equal-weight DAILY-REBALANCED benchmark (NOT buy-and-hold -- do not
    conflate the two), built ONLY over the exact `reference_dates` the
    admitted real trials/judge actually scored (comparison_scope.
    reference_dates from the judge artifact, cross-checked one-for-one
    against each admitted trial's own economic_daily_returns.csv date
    column). Fails closed if any reference date would fall at/after the
    reserved holdout boundary."""
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


def verify_oos_date_alignment(real_trials: list[dict], judge: dict) -> list[str]:
    """FINDING 2 repair: fail closed unless EVERY real trial's own
    economic_daily_returns.csv date column matches the judge's
    comparison_scope.reference_dates EXACTLY (same set, same content) --
    never assume same-window alignment. This is a data/protocol-alignment
    check, deliberately independent of DSR/PBO admission status: the judge
    can rule a trial not_evaluable (degenerate/zero-variance returns) while
    its OOS date sequence -- derived from the shared wf_spec/bars, not from
    trading activity -- remains real and well-defined; comparison_scope.
    reference_dates is populated even when included_trial_ids is empty."""
    comparison_scope = judge.get("comparison_scope") or {}
    reference_dates = comparison_scope.get("reference_dates")
    if not reference_dates:
        raise RuntimeError("Fail-closed: judge artifact has no comparison_scope.reference_dates")
    if not real_trials:
        raise RuntimeError("Fail-closed: no real trial to verify OOS date alignment against")

    ref_set = set(reference_dates)
    for t in real_trials:
        csv_path = t.get("economic_daily_returns_csv")
        if not csv_path:
            raise RuntimeError(f"Fail-closed: real trial {t['trial_id']} has no economic_daily_returns_csv")
        dates = set(pd.read_csv(csv_path)["date"].astype(str).tolist())
        if dates != ref_set:
            only_trial = sorted(dates - ref_set)
            only_judge = sorted(ref_set - dates)
            raise RuntimeError(
                f"Fail-closed: OOS reference dates differ between real trial {t['trial_id']} and the "
                f"judge comparison scope -- trial-only={only_trial[:5]} judge-only={only_judge[:5]}"
            )

    holdout_starts = {t["holdout_start_utc"] for t in real_trials if t.get("holdout_start_utc")}
    if len(holdout_starts) != 1:
        raise RuntimeError(f"Fail-closed: real trials disagree on holdout_start_utc: {holdout_starts}")

    return sorted(reference_dates)


def main() -> None:
    print(f"[1/7] fetching real Alpaca bars for {len(SYMBOLS)} symbols, {START_UTC.date()} - {END_UTC.date()} ...")
    bars, manifest = fetch_bars()
    print(f"      bars fetched: {len(bars)} rows, columns={list(bars.columns)}")
    RUN_ROOT.mkdir(parents=True, exist_ok=True)
    bars.to_csv(RUN_ROOT / "raw_bars.csv", index=False)
    (RUN_ROOT / "bars_provenance_manifest.json").write_text(
        json.dumps(manifest, sort_keys=True, indent=2, default=str), encoding="utf-8"
    )

    print("[2/7] building Feature Set v1 (default spec) then isolating momentum_score ONLY ...")
    full_features = build_feature_set_v1(bars, spec=FeatureSetV1Spec())
    isolated_features = isolate_momentum_feature(full_features)
    print(f"      full feature set: {len(full_features.columns)} columns; isolated to: {list(isolated_features.columns)}")
    print(f"FEATURE_COLUMNS={REQUIRED_FEATURE_COLUMNS}")
    print(f"FEATURE_COLUMN_COUNT={len(REQUIRED_FEATURE_COLUMNS)}")
    print(f"UNIVERSE={SYMBOLS}")
    print(f"RUN_01_PRESERVED={'YES' if (RUN_01_ROOT / 'final_report.json').exists() else 'NO'}")
    print("FINAL_HOLDOUT_POLICY=RESERVED")

    print(f"[3/7] building targets (fwd_ret over {LABEL_HORIZON_BARS} bars, threshold={LABEL_RET_THRESHOLD}) ...")
    targets = build_targets(bars, horizon_bars=LABEL_HORIZON_BARS, ret_threshold=LABEL_RET_THRESHOLD)
    print(f"      targets: {len(targets)} rows, positive rate={targets['target'].mean():.3f}")

    results = []

    print("[4/7] running REAL trials (single-feature momentum_score, 3 model-probability entry thresholds) ...")
    for i, thr in enumerate(REAL_ENTRY_THRESHOLDS):
        run_dir = RUN_ROOT / f"trial_real_{i}"
        bars_path = write_run_dir(run_dir, bars, isolated_features, targets)
        r = run_one_trial(
            run_dir=run_dir,
            hypothesis_id=HYPOTHESIS_ID_REAL,
            entry_threshold=thr,
            bars_path=bars_path,
            bars_provenance=manifest,
        )
        print(f"      trial_real_{i} (entry_threshold={thr}): trial_id={r['trial_id'][:16]}...")
        results.append(r)

    print("[5/7] running NEGATIVE CONTROL (causal same-horizon (fwd_ret,target) pair placebo) ...")
    shuffled_targets = build_causal_placebo_targets(targets, seed=PLACEBO_SEED)
    placebo_dir = RUN_ROOT / "trial_placebo_0"
    placebo_bars_path = write_run_dir(placebo_dir, bars, isolated_features, shuffled_targets)
    r_placebo = run_one_trial(
        run_dir=placebo_dir,
        hypothesis_id=HYPOTHESIS_ID_PLACEBO,
        entry_threshold=REAL_ENTRY_THRESHOLDS[1],
        bars_path=placebo_bars_path,
        bars_provenance=manifest,
    )
    print(f"      trial_placebo_0: trial_id={r_placebo['trial_id'][:16]}...")
    results.append(r_placebo)

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
    placebo_status = trial_admission.get(r_placebo["trial_id"], "UNKNOWN")
    print(f"      registered_unique_trials={judge.get('registry_population', {}).get('registered_unique_trials')}")
    print(f"      admitted={sum(1 for v in trial_admission.values() if v == 'ADMITTED')} "
          f"excluded={sum(1 for v in trial_admission.values() if v != 'ADMITTED')}")
    print(f"      placebo judge status: {placebo_status}")

    print("[7/7] building benchmark over the EXACT real-trial OOS reference dates ...")
    real_trials = [r for r in results if r["hypothesis_id"] == HYPOTHESIS_ID_REAL]
    reference_dates = verify_oos_date_alignment(real_trials, judge)
    holdout_start_utc = real_trials[0]["holdout_start_utc"]
    bench = build_benchmark_over_oos_dates(bars, SYMBOLS, reference_dates, holdout_start_utc)
    admitted_real_count = sum(1 for t in real_trials if t["trial_id"] in included)
    print(f"BENCHMARK_TYPE={bench['benchmark_type']}")
    print(f"BENCHMARK_EXACT_DATE_ALIGNMENT=PASS")
    print(f"OOS_REFERENCE_DATE_START={bench['reference_date_start']}")
    print(f"OOS_REFERENCE_DATE_END={bench['reference_date_end']}")
    print(f"OOS_REFERENCE_DATE_COUNT={bench['reference_date_count']}")
    print(f"ADMITTED_REAL_TRIAL_COUNT={admitted_real_count}/{len(real_trials)}")

    final = {
        "experiment_id": EXPERIMENT_ID,
        "run_id": "run_02",
        "run_01_status": "INVALID_FOR_STATED_HYPOTHESIS",
        "run_01_reason": "unintended_full_feature_set_consumption",
        "feature_columns": REQUIRED_FEATURE_COLUMNS,
        "universe": SYMBOLS,
        "universe_classification": UNIVERSE_CLASSIFICATION,
        "symbols": SYMBOLS,
        "date_range_utc": [str(START_UTC), str(END_UTC)],
        "label_horizon_bars": LABEL_HORIZON_BARS,
        "label_ret_threshold": LABEL_RET_THRESHOLD,
        "trials": results,
        "judge_status": judge.get("judge_status"),
        "pbo_result": judge.get("pbo_result"),
        "dsr_results": judge.get("dsr_results"),
        "dsr_trial_accounting": judge.get("dsr_trial_accounting"),
        "registry_population": judge.get("registry_population"),
        "trial_admission": trial_admission,
        "placebo_judge_status": placebo_status,
        "admitted_real_trial_count": admitted_real_count,
        "real_trial_count": len(real_trials),
        "benchmark_over_oos_reference_dates": bench,
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
        "benchmark": bench,
    }, indent=2, default=str))


if __name__ == "__main__":
    main()
