from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import numpy as np
import pandas as pd

from mqk_research.data.bars_provenance import (
    check_corporate_action_integrity,
    provenance_identity_fragment,
    require_bars_match_manifest,
)
from mqk_research.exp_distributed.storage import ResearchResultStore
from mqk_research.ml.economic_walkforward import (
    _resolve_rank_direction_for_frame,
    load_bars,
    load_oos_predictions,
)
from mqk_research.ml.eval_walkforward import (
    _fit_logreg_batch,
    _sigmoid,
    _standardize_apply,
    _standardize_fit,
    WalkForwardSpec,
    compute_holdout_boundary,
    discovery_usable_mask,
    make_folds,
    train_purge_masks,
)
from mqk_research.ml.util_hash import file_record
from mqk_research.ml.weight_to_share import (
    WEIGHT_TO_SHARE_PROTOCOL_ID_V1,
    WeightToShareSpec,
    weight_to_target_qty,
)

# W06-P9-REPLAY-AUTHORITY-01
#
# Deterministically transforms an already-REGISTERED Wave06 Research trial
# (resolved strictly by registry_db + trial_id + required economic_eval_id --
# never "latest") into a hash-authenticated, signal-time discrete-quantity
# replay bundle consumable by the Rust canonical Backtest/P9 pipeline
# (W06-P9-RUST-REPLAY-STRATEGY-01 / W06-P9-CANONICAL-RESEARCH-REPLAY-CLI-01),
# including truthful per-symbol leave-one-out (LOO) variants -- WITHOUT
# trusting any caller-supplied result value, feature-column name, or
# rank_side_count, and WITHOUT executing a new independent trial.
#
# AUTHORITATIVE SOURCE HIERARCHY (see mission A2):
#   AUTHORITATIVE SOURCE -- the registered trial identity, its one required
#     succeeded attempt, and that attempt's own recorded artifacts
#     (walk_forward_eval.json / economic_walk_forward.json), each re-verified
#     against features.csv/targets.csv/feature_schema.json/bars.csv content
#     on disk before use.
#   DERIVED CACHE -- the baseline OOS score reproduction, the fold models
#     reconstructed from that authenticated training data, and every
#     leave-one-out schedule this module computes from them.
#   RESULT LINEAGE -- economic_eval_id / attempt_id, kept structurally
#     separate from Strategy semantic identity (see Patch B).
#
# THE LOO FEATURE RECOMPUTATION SEAM (mission A4, "recompute the same-date
# cross-sectional input feature for survivors"): LIQ-01/VOL-01 are each a
# SINGLE-FEATURE classifier whose sole feature is itself an already-computed
# cross-sectional PERCENTILE RANK of some per-symbol production statistic
# (e.g. illiquidity_amihud_daily_xs_rank). The raw, un-ranked per-symbol
# statistic is NOT part of any authenticated trial artifact -- only the
# already-ranked feature column is (features.csv, content-hash-bound into
# trial identity via feature_schema.json). Recomputing the raw statistic from
# scratch would require trusting an unauthenticated file or a caller-supplied
# transform. This module avoids that trap entirely: a percentile rank
# (`pandas.rank(pct=True, method="average")`) is an order-preserving,
# tie-preserving function of the underlying raw value -- two raw values'
# relative order (and any exact tie between them) is exactly recoverable from
# their percentile ranks alone. Re-ranking the SURVIVORS' own already-
# authenticated percentile-rank values (after excluding symbol X) with the
# exact same formula therefore reproduces BIT-FOR-BIT the percentile rank
# that recomputing from the (unavailable) raw statistic over the same
# survivor set would have produced. See test_oos_replay_bundle.py's mutation/
# no-effect fixtures (A7 tests 8/9) for the executable proof. This is a
# result-independent, purely mechanical consequence of percentile-rank being
# a monotonic bijection onto rank order -- not a new statistical method.
#
# FOLD MODEL RECONSTRUCTION (mission A3): reuses eval_walkforward.py's own
# exported fit/standardization/fold-boundary/purge primitives VERBATIM
# (`_fit_logreg_batch`, `_standardize_fit`, `_standardize_apply`,
# `make_folds`, `discovery_usable_mask`, `train_purge_masks`,
# `compute_holdout_boundary`) -- none of the actual "logistic fitting math"
# is reimplemented here. eval_walkforward.py itself is NOT modified: its
# fold-loop orchestration (join/mask assembly around those primitives) is
# thin, deterministic glue that this module reproduces directly rather than
# risk any behavioral change to that heavily-relied-upon, already-tested
# function. Before any LOO schedule is trusted, `verify_baseline_oos_reproduction`
# must prove this reconstruction reproduces the ALREADY-REGISTERED OOS
# ml_score stream within FOLD_MODEL_RECONSTRUCTION_TOLERANCE -- a fixed,
# pre-outcome tolerance frozen from this exact module's own established
# repo-wide float-comparison convention (see economic_walkforward.py's
# `_RANK_SCORE_TIE_TOL` / `_GROSS_TOL`, both 1e-9), chosen before inspecting
# any Wave06 candidate result. A baseline reproduction failure fails closed:
# no LOO schedule for that trial may ever be trusted.

REPLAY_PROTOCOL_ID_V1 = "research_oos_replay_bundle_v1"
FOLD_MODEL_RECONSTRUCTION_TOLERANCE = 1e-9
FEATURE_TRANSFORM_CROSS_SECTIONAL_PERCENTILE_RANK_V1 = (
    "cross_sectional_percentile_rank_rerank_of_authenticated_feature_v1"
)


class ReplayBundleError(RuntimeError):
    """Fail-closed error for every refusal in this module -- always prefixed
    'Fail-closed:' per repo convention, never a silently-swallowed default."""


@dataclass(frozen=True)
class FoldModel:
    fold: int
    test_start: str
    test_end: str
    standardize: bool
    clip_z: float
    mean: Optional[List[float]]
    std: Optional[List[float]]
    w: List[float]
    b: float


# ---------------------------------------------------------------------------
# A1/A2 -- resolve the exact registered trial/attempt, never "latest".
# ---------------------------------------------------------------------------


def resolve_registered_economic_attempt(
    registry_db: Path, *, trial_id: str, economic_eval_id: str
) -> Dict[str, Any]:
    """Resolve the EXACT trial and EXACT attempt via ResearchResultStore.
    Requires a real succeeded attempt whose `result_id == economic_eval_id`.
    Fails closed (ReplayBundleError) if the trial is unknown, if no
    succeeded attempt matches, or if more than one succeeded attempt
    (a genuine registry anomaly -- economic_eval_id is content-derived and
    should be attempt-unique) claims the same result_id."""
    store = ResearchResultStore(Path(registry_db))
    try:
        trial = store.get_trial(trial_id)
    except KeyError as exc:
        raise ReplayBundleError(f"Fail-closed: unknown trial_id {trial_id!r}") from exc

    matches = [
        a
        for a in store.list_attempts(trial_id)
        if a.get("status") == "succeeded" and a.get("result_id") == economic_eval_id
    ]
    if not matches:
        raise ReplayBundleError(
            f"Fail-closed: no succeeded attempt of trial {trial_id!r} has "
            f"result_id == economic_eval_id {economic_eval_id!r}"
        )
    if len(matches) > 1:
        raise ReplayBundleError(
            f"Fail-closed: {len(matches)} succeeded attempts of trial {trial_id!r} claim "
            f"result_id {economic_eval_id!r} -- ambiguous authoritative source"
        )
    attempt = matches[0]
    identity = json.loads(trial["identity_json"])
    return {"trial": trial, "attempt": attempt, "identity": identity}


def load_recorded_artifacts(resolved: Dict[str, Any]) -> Tuple[Dict[str, Any], Dict[str, Any], Path]:
    """Load the attempt's own recorded walk_forward_eval.json /
    economic_walk_forward.json (paths come from the attempt's OWN
    artifact_paths_json, written by finalize_attempt at registration time --
    never a caller-supplied path). `run_dir` is derived structurally from the
    recorded walk_forward_eval path (`<run_dir>/eval/walk_forward_eval.json`),
    matching economic_registry_integration.run_registered_economic_walkforward_eval's
    own fixed layout -- never re-derived from any caller input."""
    artifact_paths = json.loads(resolved["attempt"]["artifact_paths_json"] or "{}")
    wf_path = artifact_paths.get("walk_forward_eval")
    econ_path = artifact_paths.get("economic_walk_forward")
    if not wf_path or not econ_path:
        raise ReplayBundleError(
            "Fail-closed: attempt is missing recorded walk_forward_eval/economic_walk_forward "
            "artifact paths"
        )
    wf_path = Path(wf_path)
    econ_path = Path(econ_path)
    if not wf_path.exists() or not econ_path.exists():
        raise ReplayBundleError(
            f"Fail-closed: recorded artifact path missing on disk (wf={wf_path.exists()}, "
            f"econ={econ_path.exists()})"
        )
    wf_out = json.loads(wf_path.read_text(encoding="utf-8"))
    economic_out = json.loads(econ_path.read_text(encoding="utf-8"))
    run_dir = wf_path.parent.parent
    return wf_out, economic_out, run_dir


# ---------------------------------------------------------------------------
# A1 -- re-verify recorded bytes/hash/provenance for every required source.
# ---------------------------------------------------------------------------


def _require_hash_match(label: str, recorded: Dict[str, Any], path: Path) -> None:
    current = file_record(path)
    if current["sha256"] is None:
        raise ReplayBundleError(f"Fail-closed: missing required registry input: {path}")
    if current["sha256"] != recorded.get("sha256") or current["bytes"] != recorded.get("bytes"):
        raise ReplayBundleError(
            f"Fail-closed: {label} content at {path} no longer matches the sha256/bytes recorded "
            "at registration time -- refusing to replay from a mutated source"
        )


def verify_recorded_source_inputs(
    run_dir: Path, wf_out: Dict[str, Any], economic_out: Dict[str, Any]
) -> Dict[str, Any]:
    """Re-verify EVERY required source's recorded bytes/hash/provenance
    before use (mission A1). Returns the loaded, verified DataFrames/schema/
    manifest -- never the caller's own idea of where these live (paths are
    always `run_dir / "<fixed name>"`, matching the registered layout)."""
    features_path = run_dir / "features.csv"
    targets_path = run_dir / "targets.csv"
    schema_path = run_dir / "feature_schema.json"

    _require_hash_match("features.csv", wf_out["inputs"]["features_csv"], features_path)
    _require_hash_match("targets.csv", wf_out["inputs"]["targets_csv"], targets_path)
    _require_hash_match("feature_schema.json", wf_out["inputs"]["feature_schema"], schema_path)

    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    if sha256_hexdigest_of_file(features_path) != schema["features_csv_sha256"]:
        raise ReplayBundleError(
            "Fail-closed: features.csv sha256 does not match feature_schema.json's own declared "
            "features_csv_sha256 -- schema/features drifted out of internal self-consistency"
        )

    bars_record = economic_out["inputs"]["bars_csv"]
    bars_path = Path(bars_record["path"])
    _require_hash_match("bars.csv", bars_record, bars_path)

    manifest = economic_out.get("bars_provenance")
    if not manifest:
        raise ReplayBundleError("Fail-closed: attempt has no recorded bars_provenance manifest")

    bars_df = load_bars(bars_path)
    require_bars_match_manifest(bars_df, manifest)
    check_corporate_action_integrity(bars_df, manifest)

    features_df = pd.read_csv(features_path)
    targets_df = pd.read_csv(targets_path)

    return {
        "features_df": features_df,
        "targets_df": targets_df,
        "schema": schema,
        "bars_df": bars_df,
        "bars_provenance": manifest,
        "features_path": features_path,
        "targets_path": targets_path,
        "schema_path": schema_path,
        "bars_path": bars_path,
    }


def sha256_hexdigest_of_file(path: Path) -> Optional[str]:
    return file_record(path)["sha256"]


# ---------------------------------------------------------------------------
# A3 -- fold model reconstruction + mandatory baseline self-consistency gate.
# ---------------------------------------------------------------------------


def reconstruct_fold_models(
    *,
    features_df: pd.DataFrame,
    targets_df: pd.DataFrame,
    schema: Dict[str, Any],
    end_ts_col: str,
    label_col: str,
    label_end_ts_col: str,
    wf_spec: WalkForwardSpec,
    l2: float,
    lr: float,
    steps: int,
    standardize: bool,
    clip_z: float,
) -> Tuple[List[FoldModel], pd.DataFrame]:
    """Reproduces eval_walkforward.run_walkforward_eval's join/mask/fold
    orchestration EXACTLY (same join keys, same sort, same usable/purge
    masks, same fold boundaries), calling that module's own exported fit/
    standardization primitives verbatim, and additionally RETURNS each used
    fold's (mean, std, w, b) -- the one piece of evidence
    run_walkforward_eval's own artifact never persists. Returns
    (fold_models, oos_reconstructed_df) where oos_reconstructed_df has the
    same (fold, symbol, decision_ts, ml_score) shape as the recorded
    walk_forward_oos_predictions.csv, for direct comparison by
    `verify_baseline_oos_reproduction`."""
    feat_cols = list(schema["feature_columns"])
    if len(feat_cols) != 1:
        raise ReplayBundleError(
            f"Fail-closed: {REPLAY_PROTOCOL_ID_V1} supports exactly one feature column "
            f"(the single cross-sectionally-ranked classifier feature LIQ-01/VOL-01 both use), "
            f"got {feat_cols!r}"
        )

    join_keys = ["symbol", end_ts_col]
    target_cols = join_keys + [label_col, label_end_ts_col]
    df = features_df.merge(targets_df[target_cols], on=join_keys, how="inner", validate="one_to_one")
    df = df.sort_values(["symbol", end_ts_col], kind="mergesort").reset_index(drop=True)

    X_all = df[feat_cols].to_numpy(dtype=np.float64)
    y_all = (df[label_col].astype(float) > 0.0).astype(int).to_numpy(dtype=np.int64)
    ts_all = pd.to_datetime(df[end_ts_col], utc=True)
    label_end_all = pd.to_datetime(df[label_end_ts_col], utc=True)
    symbol_all = df["symbol"].astype(str)

    t_min, t_max = ts_all.min(), ts_all.max()
    _discovery_anchor, holdout_start, _dataset_end = compute_holdout_boundary(
        t_min, t_max, wf_spec.holdout_months
    )
    usable_mask = discovery_usable_mask(ts_all, label_end_all, holdout_start)
    folds_raw = make_folds(t_min=t_min, discovery_end=holdout_start, spec=wf_spec)
    if not folds_raw:
        raise ReplayBundleError("Fail-closed: no walk-forward folds reconstructable for this trial")

    fold_models: List[FoldModel] = []
    oos_rows: List[Dict[str, Any]] = []
    for i, (tr_s, tr_e, te_s, te_e) in enumerate(folds_raw, start=1):
        tr_candidate_mask = usable_mask & (ts_all >= tr_s) & (ts_all < tr_e)
        te_mask = usable_mask & (ts_all >= te_s) & (ts_all < te_e)
        effective_train_cutoff = te_s - pd.Timedelta(seconds=wf_spec.embargo_seconds)
        if wf_spec.purge_enabled:
            _overlap, _embargo, tr_effective_mask = train_purge_masks(
                tr_candidate_mask, label_end_all, te_s, effective_train_cutoff
            )
        else:
            tr_effective_mask = tr_candidate_mask

        effective_train_rows = int(tr_effective_mask.sum())
        test_rows = int(te_mask.sum())
        too_few = (
            effective_train_rows < wf_spec.min_rows_per_fold
            or test_rows < max(50, wf_spec.min_rows_per_fold // 4)
        )
        if too_few:
            continue

        X_tr = X_all[tr_effective_mask.to_numpy()]
        y_tr = y_all[tr_effective_mask.to_numpy()]
        X_te = X_all[te_mask.to_numpy()]

        mean = std = None
        X_tr_n, X_te_n = X_tr, X_te
        if standardize:
            mean, std = _standardize_fit(X_tr)
            X_tr_n = _standardize_apply(X_tr, mean, std, clip_z)
            X_te_n = _standardize_apply(X_te, mean, std, clip_z)

        w, b = _fit_logreg_batch(X_tr_n, y_tr.astype(np.float64), l2=l2, lr=lr, steps=steps)
        p_te = _sigmoid(X_te_n @ w + b)

        fold_models.append(
            FoldModel(
                fold=i,
                test_start=te_s.isoformat(),
                test_end=te_e.isoformat(),
                standardize=standardize,
                clip_z=float(clip_z),
                mean=None if mean is None else [float(v) for v in mean],
                std=None if std is None else [float(v) for v in std],
                w=[float(v) for v in w],
                b=float(b),
            )
        )

        te_symbols = symbol_all[te_mask].to_numpy()
        te_ts = ts_all[te_mask].to_numpy()
        for idx in range(test_rows):
            oos_rows.append(
                {
                    "fold": i,
                    "symbol": str(te_symbols[idx]),
                    "decision_ts": pd.Timestamp(te_ts[idx]).isoformat(),
                    "ml_score": float(p_te[idx]),
                }
            )

    if not fold_models:
        raise ReplayBundleError("Fail-closed: no usable folds reconstructed for this trial")

    oos_reconstructed = pd.DataFrame(oos_rows, columns=["fold", "symbol", "decision_ts", "ml_score"])
    oos_reconstructed = oos_reconstructed.sort_values(
        ["fold", "symbol", "decision_ts"], kind="mergesort"
    ).reset_index(drop=True)
    return fold_models, oos_reconstructed


def verify_baseline_oos_reproduction(
    reconstructed: pd.DataFrame,
    recorded: pd.DataFrame,
    *,
    tolerance: float = FOLD_MODEL_RECONSTRUCTION_TOLERANCE,
) -> None:
    """Mandatory self-consistency gate (mission A3). Fails closed on any
    row-set mismatch or any |ml_score| difference beyond `tolerance`."""
    key_cols = ["fold", "symbol", "decision_ts"]
    left = reconstructed[key_cols + ["ml_score"]].sort_values(key_cols, kind="mergesort").reset_index(drop=True)
    right = recorded[key_cols + ["ml_score"]].copy()
    right["decision_ts"] = right["decision_ts"].apply(lambda t: pd.Timestamp(t).isoformat())
    right = right.sort_values(key_cols, kind="mergesort").reset_index(drop=True)

    left_keys = set(map(tuple, left[key_cols].to_numpy()))
    right_keys = set(map(tuple, right[key_cols].to_numpy()))
    if left_keys != right_keys:
        raise ReplayBundleError(
            "Fail-closed: reconstructed OOS row set does not match the recorded "
            "walk_forward_oos_predictions.csv row set -- baseline reproduction failed"
        )

    merged = left.merge(right, on=key_cols, suffixes=("_reconstructed", "_recorded"))
    diff = (merged["ml_score_reconstructed"] - merged["ml_score_recorded"]).abs()
    if (diff > tolerance).any():
        worst = float(diff.max())
        raise ReplayBundleError(
            "Fail-closed: reconstructed OOS ml_score stream diverges from the recorded "
            f"walk_forward_oos_predictions.csv beyond tolerance={tolerance!r} (worst diff={worst!r}) "
            "-- baseline reproduction failed; no leave-one-out schedule can be trusted"
        )


# ---------------------------------------------------------------------------
# A4 -- symbol leave-one-out cross-sectional feature recomputation.
# ---------------------------------------------------------------------------


def recompute_loo_feature_frame(
    features_df: pd.DataFrame, *, feature_col: str, end_ts_col: str, symbol_col: str, excluded_symbol: str
) -> pd.DataFrame:
    """See module docstring "THE LOO FEATURE RECOMPUTATION SEAM". Excludes
    `excluded_symbol` entirely, then re-ranks the SURVIVORS' own already-
    authenticated percentile-rank feature values within each `end_ts_col`
    group using the exact same `rank(pct=True, method="average")` formula
    the original cross-sectional rank feature was built with -- order- and
    tie-preserving, so this reproduces exactly what re-ranking the
    (unavailable) raw statistic over the same survivor set would produce."""
    survivors = features_df[features_df[symbol_col] != excluded_symbol].copy()
    survivors[feature_col] = survivors.groupby(end_ts_col)[feature_col].rank(pct=True, method="average")
    return survivors


def score_feature_values(
    feature_values: np.ndarray, fold_model: FoldModel
) -> np.ndarray:
    """Apply ONE frozen fold model's (mean, std, w, b) to a 1-D array of
    single-feature values -- the exact same standardize-then-logistic
    formula eval_walkforward.py's own fold loop uses, reused via its
    exported `_standardize_apply`/`_sigmoid`."""
    X = feature_values.reshape(-1, 1).astype(np.float64)
    if fold_model.standardize:
        X = _standardize_apply(X, np.asarray(fold_model.mean), np.asarray(fold_model.std), fold_model.clip_z)
    z = X @ np.asarray(fold_model.w) + fold_model.b
    return _sigmoid(z)


# ---------------------------------------------------------------------------
# A5/A6 -- signal-time sizing + deterministic replay bundle assembly.
# ---------------------------------------------------------------------------


def assert_no_duplicate_schedule_rows(rows: List[Dict[str, Any]]) -> None:
    seen = set()
    for row in rows:
        key = (row["symbol"], row["decision_ts"])
        if key in seen:
            raise ReplayBundleError(f"Fail-closed: duplicate (symbol, decision_ts) entry in schedule: {key}")
        seen.add(key)


def _assert_no_holdout_rows(rows: List[Dict[str, Any]], holdout_start_utc: str) -> None:
    holdout_start = pd.Timestamp(holdout_start_utc)
    for row in rows:
        if pd.Timestamp(row["decision_ts"]) >= holdout_start:
            raise ReplayBundleError(
                f"Fail-closed: schedule row at {row['decision_ts']} falls at/after the reserved "
                f"holdout boundary {holdout_start_utc}"
            )


def build_schedule_rows(
    *,
    scores_by_date: Dict[pd.Timestamp, Dict[str, float]],
    fold_symbols_by_date: Dict[pd.Timestamp, List[str]],
    rank_side_count: int,
    long_only: bool,
    max_gross_exposure: float,
    wts_spec: WeightToShareSpec,
    close_lookup: Dict[Tuple[str, pd.Timestamp], float],
) -> List[Dict[str, Any]]:
    """Applies the FROZEN direct-rank top/bottom-K rule
    (`_resolve_rank_direction_for_frame`, imported verbatim from
    economic_walkforward.py) at each decision date, then the SAME
    signal-time weight->qty translation Rust would need
    (`weight_to_target_qty`), emitting the COMPLETE per-date target vector
    over that date's fold-symbol universe (missing-at-this-frame members
    default to flat/0, exactly like the production
    `_build_rank_pending_events` state closure) -- never only the nonzero
    entries, so a symbol dropped from selection is an explicit flatten."""
    weight_each = max_gross_exposure / float(rank_side_count if long_only else 2 * rank_side_count)
    rows: List[Dict[str, Any]] = []
    for ts in sorted(scores_by_date.keys()):
        scores = scores_by_date[ts]
        fold_symbols = fold_symbols_by_date[ts]
        direction = _resolve_rank_direction_for_frame(scores, rank_side_count, long_only)
        full_direction = {s: direction.get(s, 0) for s in fold_symbols}
        for sym in sorted(full_direction.keys()):
            d = full_direction[sym]
            weight = weight_each * d
            price = close_lookup.get((sym, ts))
            target_qty = weight_to_target_qty(weight=weight, price=price, spec=wts_spec)
            rows.append({"decision_ts": ts.isoformat(), "symbol": sym, "target_qty": int(target_qty)})
    return rows


def build_replay_bundle(
    registry_db: Path,
    *,
    trial_id: str,
    economic_eval_id: str,
    out_dir: Path,
    excluded_symbols: Optional[List[str]] = None,
) -> Path:
    """Top-level orchestrator (A1-A6). `excluded_symbols`, when given,
    restricts WHICH per-symbol leave-one-out schedules are produced (a
    testing-only convenience -- it never changes replay SEMANTICS, only
    which non-authoritative derived scenarios get computed). Production
    callers should omit it: the default enumerates every symbol in the
    trial's OWN authenticated bars_provenance symbol_universe -- never a
    caller-supplied universe."""
    out_dir = Path(out_dir)

    resolved = resolve_registered_economic_attempt(
        registry_db, trial_id=trial_id, economic_eval_id=economic_eval_id
    )
    wf_out, economic_out, run_dir = load_recorded_artifacts(resolved)
    verified = verify_recorded_source_inputs(run_dir, wf_out, economic_out)

    signal_policy = economic_out["signal_policy"]
    identity_signal_policy = resolved["identity"]["economic_protocol"]["signal_policy"]
    if signal_policy != identity_signal_policy:
        raise ReplayBundleError(
            "Fail-closed: attempt's recorded signal_policy disagrees with the trial's own "
            "registered identity signal_policy -- refusing an internally inconsistent trial"
        )
    if not signal_policy.get("direction_policy", "").startswith("cross_sectional_rank_"):
        raise ReplayBundleError(
            f"Fail-closed: {REPLAY_PROTOCOL_ID_V1} supports only cross-sectional rank direction "
            f"policies, got {signal_policy.get('direction_policy')!r}"
        )
    rank_side_count = int(signal_policy["rank_side_count"])
    long_only = bool(signal_policy["long_only"])
    max_gross_exposure = float(signal_policy["max_gross_exposure"])

    wts_identity = economic_out.get("weight_to_share") or {}
    if wts_identity.get("weight_to_share_protocol_id") != WEIGHT_TO_SHARE_PROTOCOL_ID_V1:
        raise ReplayBundleError(
            "Fail-closed: attempt has no official weight_to_share_v1 protocol evidence -- "
            "a diagnostic/legacy continuous-weight-only trial cannot be replayed into a "
            "discrete-quantity Backtest schedule"
        )
    wts_spec = WeightToShareSpec(
        equity_usd=float(wts_identity["equity_usd"]),
        max_target_qty=wts_identity.get("max_target_qty"),
        max_position_notional_usd=wts_identity.get("max_position_notional_usd"),
    )

    evaluation_spec = resolved["identity"]["evaluation_spec"]
    model_spec = resolved["identity"]["model_spec"]
    wf_spec = WalkForwardSpec(
        train_years=evaluation_spec["train_years"],
        test_months=evaluation_spec["test_months"],
        step_months=evaluation_spec["step_months"],
        min_rows_per_fold=evaluation_spec["min_rows_per_fold"],
        purge_enabled=evaluation_spec["purge_enabled"],
        label_end_ts_col=evaluation_spec["label_end_ts_col"],
        embargo_seconds=evaluation_spec["embargo_seconds"],
        holdout_months=evaluation_spec["holdout_months"],
    ).normalized()

    fold_models, reconstructed_oos = reconstruct_fold_models(
        features_df=verified["features_df"],
        targets_df=verified["targets_df"],
        schema=verified["schema"],
        end_ts_col=evaluation_spec["end_ts_col"],
        label_col=evaluation_spec["label_col"],
        label_end_ts_col=evaluation_spec["label_end_ts_col"],
        wf_spec=wf_spec,
        l2=float(model_spec["l2"]),
        lr=float(model_spec["lr"]),
        steps=int(model_spec["steps"]),
        standardize=bool(model_spec["standardize"]),
        clip_z=float(model_spec["clip_z"]),
    )

    recorded_oos_path = run_dir / "eval" / "walk_forward_oos_predictions.csv"
    _require_hash_match(
        "walk_forward_oos_predictions.csv", wf_out["outputs"]["oos_predictions_csv"], recorded_oos_path
    )
    recorded_oos = load_oos_predictions(recorded_oos_path)
    verify_baseline_oos_reproduction(reconstructed_oos, recorded_oos)

    holdout_start_utc = wf_out["holdout"]["start_utc"]
    feature_col = verified["schema"]["feature_columns"][0]
    end_ts_col = evaluation_spec["end_ts_col"]

    # Fold-of-date authority + per-fold symbol universe, both derived
    # strictly from the RECORDED (authenticated) OOS predictions -- never
    # from features.csv's full pre-fold history.
    recorded_oos = recorded_oos.copy()
    fold_of_ts: Dict[pd.Timestamp, int] = {}
    fold_symbols: Dict[int, List[str]] = {}
    for fold_no, group in recorded_oos.groupby("fold"):
        fold_symbols[int(fold_no)] = sorted(group["symbol"].unique().tolist())
        for ts in group["decision_ts"].unique():
            ts = pd.Timestamp(ts)
            if fold_of_ts.setdefault(ts, int(fold_no)) != int(fold_no):
                raise ReplayBundleError(f"Fail-closed: decision_ts {ts} maps to more than one fold")

    fold_model_by_number = {fm.fold: fm for fm in fold_models}

    close_lookup: Dict[Tuple[str, pd.Timestamp], float] = {}
    bars_df = verified["bars_df"]
    for sym, ts, close in zip(bars_df["symbol"], bars_df["end_ts"], bars_df["close"]):
        close_lookup[(str(sym), pd.Timestamp(ts))] = float(close)

    # ---- baseline schedule: authoritative recorded ml_score, never refit ----
    baseline_scores_by_date: Dict[pd.Timestamp, Dict[str, float]] = {}
    baseline_fold_symbols_by_date: Dict[pd.Timestamp, List[str]] = {}
    for ts, group in recorded_oos.groupby("decision_ts"):
        ts = pd.Timestamp(ts)
        baseline_scores_by_date[ts] = {
            str(sym): float(score) for sym, score in zip(group["symbol"], group["ml_score"])
        }
        baseline_fold_symbols_by_date[ts] = fold_symbols[fold_of_ts[ts]]

    baseline_rows = build_schedule_rows(
        scores_by_date=baseline_scores_by_date,
        fold_symbols_by_date=baseline_fold_symbols_by_date,
        rank_side_count=rank_side_count,
        long_only=long_only,
        max_gross_exposure=max_gross_exposure,
        wts_spec=wts_spec,
        close_lookup=close_lookup,
    )
    assert_no_duplicate_schedule_rows(baseline_rows)
    _assert_no_holdout_rows(baseline_rows, holdout_start_utc)

    # ---- leave-one-out schedules: derived cache, reconstructed model ----
    if excluded_symbols is None:
        excluded_symbols = list(verified["bars_provenance"]["symbol_universe"])

    loo_rows_by_symbol: Dict[str, List[Dict[str, Any]]] = {}
    features_df = verified["features_df"]
    for excluded in excluded_symbols:
        loo_features = recompute_loo_feature_frame(
            features_df, feature_col=feature_col, end_ts_col=end_ts_col, symbol_col="symbol",
            excluded_symbol=excluded,
        )
        loo_by_key = {
            (str(sym), pd.Timestamp(ts)): float(val)
            for sym, ts, val in zip(loo_features["symbol"], loo_features[end_ts_col], loo_features[feature_col])
        }

        scores_by_date: Dict[pd.Timestamp, Dict[str, float]] = {}
        loo_fold_symbols_by_date: Dict[pd.Timestamp, List[str]] = {}
        for ts, survivors in fold_symbols_needed(fold_of_ts, fold_symbols, excluded).items():
            fold_no = fold_of_ts[ts]
            fold_model = fold_model_by_number[fold_no]
            per_symbol_scores: Dict[str, float] = {}
            for sym in survivors:
                key = (sym, ts)
                if key not in loo_by_key:
                    continue
                feature_value = loo_by_key[key]
                score = float(score_feature_values(np.array([feature_value]), fold_model)[0])
                per_symbol_scores[sym] = score
            scores_by_date[ts] = per_symbol_scores
            loo_fold_symbols_by_date[ts] = survivors

        loo_rows = build_schedule_rows(
            scores_by_date=scores_by_date,
            fold_symbols_by_date=loo_fold_symbols_by_date,
            rank_side_count=rank_side_count,
            long_only=long_only,
            max_gross_exposure=max_gross_exposure,
            wts_spec=wts_spec,
            close_lookup=close_lookup,
        )
        assert_no_duplicate_schedule_rows(loo_rows)
        _assert_no_holdout_rows(loo_rows, holdout_start_utc)
        loo_rows_by_symbol[excluded] = loo_rows

    return _write_bundle(
        out_dir=out_dir,
        resolved=resolved,
        economic_eval_id=economic_eval_id,
        signal_policy=signal_policy,
        wts_identity=wts_identity,
        verified=verified,
        fold_models=fold_models,
        holdout_start_utc=holdout_start_utc,
        baseline_rows=baseline_rows,
        loo_rows_by_symbol=loo_rows_by_symbol,
        feature_col=feature_col,
    )


def fold_symbols_needed(
    fold_of_ts: Dict[pd.Timestamp, int], fold_symbols: Dict[int, List[str]], excluded: str
) -> Dict[pd.Timestamp, List[str]]:
    """Per-date survivor universe for one excluded symbol: that date's own
    fold-wide symbol set (see `_build_rank_pending_events`'s closure
    convention), minus the excluded symbol entirely."""
    out: Dict[pd.Timestamp, List[str]] = {}
    for ts, fold_no in fold_of_ts.items():
        out[ts] = [s for s in fold_symbols[fold_no] if s != excluded]
    return out


def _write_bundle(
    *,
    out_dir: Path,
    resolved: Dict[str, Any],
    economic_eval_id: str,
    signal_policy: Dict[str, Any],
    wts_identity: Dict[str, Any],
    verified: Dict[str, Any],
    fold_models: List[FoldModel],
    holdout_start_utc: str,
    baseline_rows: List[Dict[str, Any]],
    loo_rows_by_symbol: Dict[str, List[Dict[str, Any]]],
    feature_col: str,
) -> Path:
    out_dir.mkdir(parents=True, exist_ok=True)
    loo_dir = out_dir / "loo_schedules"
    loo_dir.mkdir(parents=True, exist_ok=True)

    def _write_rows(path: Path, rows: List[Dict[str, Any]]) -> Dict[str, Any]:
        df = pd.DataFrame(rows, columns=["decision_ts", "symbol", "target_qty"])
        df = df.sort_values(["decision_ts", "symbol"], kind="mergesort").reset_index(drop=True)
        df.to_csv(path, index=False)
        rec = file_record(path)
        return {"file": str(path.relative_to(out_dir)), "sha256": rec["sha256"], "bytes": rec["bytes"], "row_count": len(df)}

    baseline_record = _write_rows(out_dir / "baseline_schedule.csv", baseline_rows)
    loo_records = {
        symbol: _write_rows(loo_dir / f"{symbol}.csv", rows) for symbol, rows in sorted(loo_rows_by_symbol.items())
    }

    trial = resolved["trial"]
    manifest = {
        "schema_version": REPLAY_PROTOCOL_ID_V1,
        "protocol_version": REPLAY_PROTOCOL_ID_V1,
        "lineage": {
            "trial_id": trial["trial_id"],
            "experiment_id": trial["experiment_id"],
            "hypothesis_id": trial["hypothesis_id"],
            "strategy_id": trial["strategy_id"],
            "attempt_id": resolved["attempt"]["attempt_id"],
            "economic_eval_id": economic_eval_id,
        },
        "replay_semantic_spec": {
            "replay_protocol_version": REPLAY_PROTOCOL_ID_V1,
            "strategy_id": trial["strategy_id"],
            "feature_columns": [feature_col],
            "feature_transform": FEATURE_TRANSFORM_CROSS_SECTIONAL_PERCENTILE_RANK_V1,
            "direction_policy": signal_policy["direction_policy"],
            "rank_side_count": signal_policy["rank_side_count"],
            "long_only": signal_policy["long_only"],
            "borrow_model": signal_policy.get("borrow_model"),
            "max_gross_exposure": signal_policy["max_gross_exposure"],
            "timeframe": verified["bars_provenance"]["timeframe"],
            "weight_to_share": {
                "equity_usd": wts_identity["equity_usd"],
                "max_target_qty": wts_identity.get("max_target_qty"),
                "max_position_notional_usd": wts_identity.get("max_position_notional_usd"),
            },
        },
        "source_lineage": {
            "bars_provenance": provenance_identity_fragment(verified["bars_provenance"]),
        },
        "source_file_hashes": {
            "features_csv": file_record(verified["features_path"]),
            "targets_csv": file_record(verified["targets_path"]),
            "feature_schema": file_record(verified["schema_path"]),
            "bars_csv": file_record(verified["bars_path"]),
        },
        "fold_models": [fm.__dict__ for fm in fold_models],
        "holdout_start_utc": holdout_start_utc,
        "baseline_schedule": baseline_record,
        "symbol_loo_schedules": loo_records,
    }
    manifest_path = out_dir / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    return manifest_path
