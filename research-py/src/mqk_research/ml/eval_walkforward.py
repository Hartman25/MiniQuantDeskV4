from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import numpy as np
import pandas as pd

from mqk_research.ml.util_hash import file_record, sha256_json
from mqk_research.ml.schema import validate_feature_schema


@dataclass(frozen=True)
class WalkForwardSpec:
    enabled: bool = True
    train_years: int = 3
    test_months: int = 3
    step_months: int = 3
    min_rows_per_fold: int = 200

    # RESEARCH-PURGED-HOLDOUT-01
    purge_enabled: bool = True
    label_end_ts_col: str = "label_end_ts"
    embargo_seconds: int = 0
    # Engineering default, not a statistically derived value — reserves the
    # final 6 months of the dataset from all discovery train/test/standardize use.
    holdout_months: int = 6

    def normalized(self) -> "WalkForwardSpec":
        train_years = int(self.train_years)
        test_months = int(self.test_months)
        step_months = int(self.step_months)
        min_rows_per_fold = int(self.min_rows_per_fold)
        embargo_seconds = int(self.embargo_seconds)
        holdout_months = int(self.holdout_months)

        if train_years <= 0:
            raise ValueError("train_years must be > 0")
        if test_months <= 0:
            raise ValueError("test_months must be > 0")
        if step_months <= 0:
            raise ValueError("step_months must be > 0")
        if min_rows_per_fold <= 0:
            raise ValueError("min_rows_per_fold must be > 0")
        if embargo_seconds < 0:
            raise ValueError("embargo_seconds must be >= 0")
        if holdout_months <= 0:
            raise ValueError("holdout_months must be > 0")

        return WalkForwardSpec(
            enabled=bool(self.enabled),
            train_years=train_years,
            test_months=test_months,
            step_months=step_months,
            min_rows_per_fold=min_rows_per_fold,
            purge_enabled=bool(self.purge_enabled),
            label_end_ts_col=str(self.label_end_ts_col),
            embargo_seconds=embargo_seconds,
            holdout_months=holdout_months,
        )


def _sigmoid(z: np.ndarray) -> np.ndarray:
    z = np.clip(z, -50.0, 50.0)
    return 1.0 / (1.0 + np.exp(-z))


def _auc_rank(y_true: np.ndarray, y_score: np.ndarray) -> float:
    y_true = y_true.astype(np.int64)
    order = np.argsort(y_score, kind="mergesort")
    ranks = np.empty_like(order, dtype=np.float64)
    ranks[order] = np.arange(1, len(y_score) + 1, dtype=np.float64)

    pos = y_true == 1
    n_pos = float(np.sum(pos))
    n_neg = float(len(y_true) - np.sum(pos))
    if n_pos <= 0.0 or n_neg <= 0.0:
        return float("nan")

    sum_ranks_pos = float(np.sum(ranks[pos]))
    auc = (sum_ranks_pos - n_pos * (n_pos + 1.0) / 2.0) / (n_pos * n_neg)
    return float(auc)


def _logloss(y: np.ndarray, p: np.ndarray) -> float:
    eps = 1e-12
    p = np.clip(p, eps, 1.0 - eps)
    return float(-np.mean(y * np.log(p) + (1.0 - y) * np.log(1.0 - p)))


def _fit_logreg_batch(X: np.ndarray, y: np.ndarray, l2: float, lr: float, steps: int) -> Tuple[np.ndarray, float]:
    Xn = X.astype(np.float64, copy=True)
    yv = y.astype(np.float64, copy=False)
    n, d = Xn.shape
    w = np.zeros(d, dtype=np.float64)
    b = 0.0
    for _ in range(int(steps)):
        z = Xn @ w + b
        p = _sigmoid(z)
        err = (p - yv)
        grad_w = (Xn.T @ err) / n + l2 * w
        grad_b = float(np.mean(err))
        w = w - lr * grad_w
        b = b - lr * grad_b
    return w, float(b)


def _standardize_fit(X: np.ndarray) -> Tuple[np.ndarray, np.ndarray]:
    mean = np.mean(X, axis=0)
    std = np.std(X, axis=0)
    std = np.where(std <= 1e-12, 1.0, std)
    return mean, std


def _standardize_apply(X: np.ndarray, mean: np.ndarray, std: np.ndarray, clip_z: float) -> np.ndarray:
    Xn = (X - mean) / std
    if clip_z is not None and clip_z > 0.0:
        Xn = np.clip(Xn, -clip_z, clip_z)
    return Xn


def _month_add(ts: pd.Timestamp, months: int) -> pd.Timestamp:
    return ts + pd.DateOffset(months=int(months))


def compute_holdout_boundary(
    t_min: pd.Timestamp, t_max: pd.Timestamp, holdout_months: int
) -> Tuple[pd.Timestamp, pd.Timestamp, pd.Timestamp]:
    """Pure, deterministic, wall-clock-independent holdout boundary derivation.

    Returns (discovery_anchor, holdout_start, dataset_end), all month-aligned UTC
    timestamps derived only from the observed feature timestamp span.
    """
    discovery_anchor = pd.Timestamp(year=t_min.year, month=t_min.month, day=1, tz="UTC")
    dataset_end = pd.Timestamp(year=t_max.year, month=t_max.month, day=1, tz="UTC") + pd.DateOffset(months=1)
    holdout_start = dataset_end - pd.DateOffset(months=int(holdout_months))
    return discovery_anchor, holdout_start, dataset_end


def make_folds(
    *, t_min: pd.Timestamp, discovery_end: pd.Timestamp, spec: WalkForwardSpec
) -> List[Tuple[pd.Timestamp, pd.Timestamp, pd.Timestamp, pd.Timestamp]]:
    """Pure fold-boundary generator. Folds are constructed strictly inside
    [discovery_anchor, discovery_end); discovery_end is normally holdout_start."""
    anchor = pd.Timestamp(year=t_min.year, month=t_min.month, day=1, tz="UTC")
    folds: List[Tuple[pd.Timestamp, pd.Timestamp, pd.Timestamp, pd.Timestamp]] = []
    train_span_months = spec.train_years * 12
    test_span_months = spec.test_months

    train_start = anchor
    while True:
        train_end = _month_add(train_start, train_span_months)
        test_start = train_end
        test_end = _month_add(test_start, test_span_months)
        if test_end > discovery_end:
            break
        folds.append((train_start, train_end, test_start, test_end))
        train_start = _month_add(train_start, spec.step_months)
    return folds


def discovery_usable_mask(
    feature_ts: pd.Series, label_end_ts: Optional[pd.Series], holdout_start: pd.Timestamp
) -> pd.Series:
    """Rows eligible for ANY discovery use (train or test in any fold).

    label_end_ts is the INCLUSIVE timestamp of the last bar whose close is
    actually consumed by the label (see label_shadow_intents.label_end_ts).
    A row is excluded if its feature_ts falls in the reserved holdout region,
    or (when label provenance is known) its label consumes the close of a bar
    at or after holdout_start, even though its feature_ts precedes it.
    Equality (label_end_ts == holdout_start) means the holdout boundary bar's
    information has already been consumed, so it is NOT safe.
    """
    in_discovery = feature_ts < holdout_start
    if label_end_ts is None:
        return in_discovery
    no_holdout_leak = label_end_ts < holdout_start
    return in_discovery & no_holdout_leak


def train_purge_masks(
    tr_candidate_mask: pd.Series,
    label_end_ts: Optional[pd.Series],
    test_start: pd.Timestamp,
    effective_train_cutoff: pd.Timestamp,
) -> Tuple[pd.Series, pd.Series, pd.Series]:
    """Split a fold's candidate training rows into (overlap_purged, embargo_purged, effective).

    label_end_ts is the INCLUSIVE timestamp of the last bar whose close is
    actually consumed by the label (see label_shadow_intents.label_end_ts).
    A label ending exactly at test_start (or exactly at effective_train_cutoff)
    has already consumed that bar's close, so equality is NOT safe — only
    strict '<' is.

    overlap_purged: label consumes test_start's bar or later, even with zero embargo.
    embargo_purged: label would be fine with zero embargo, but consumes the
                     configured pre-test embargo window (including its boundary bar).
    effective:      usable for training (label_end_ts < effective_train_cutoff).
    """
    if label_end_ts is None:
        false_mask = tr_candidate_mask & False
        return false_mask, false_mask, tr_candidate_mask

    overlap_ok = label_end_ts < test_start
    embargo_ok = label_end_ts < effective_train_cutoff
    overlap_purged = tr_candidate_mask & ~overlap_ok
    embargo_purged = tr_candidate_mask & overlap_ok & ~embargo_ok
    effective = tr_candidate_mask & embargo_ok
    return overlap_purged, embargo_purged, effective


def run_walkforward_eval(
    run_dir: Path,
    *,
    end_ts_col: str = "end_ts",
    label_col: str = "target",
    l2: float = 1e-3,
    lr: float = 0.05,
    steps: int = 500,
    standardize: bool = True,
    clip_z: float = 8.0,
    spec: WalkForwardSpec | None = None,
) -> Path:
    run_dir = Path(run_dir)
    spec = (spec or WalkForwardSpec()).normalized()

    features_path = run_dir / "features.csv"
    targets_path = run_dir / "targets.csv"
    schema_path = run_dir / "feature_schema.json"

    if not features_path.exists() or not targets_path.exists():
        raise FileNotFoundError("Missing features.csv/targets.csv for eval")
    if not schema_path.exists():
        raise FileNotFoundError("Missing feature_schema.json for eval")

    validate_feature_schema(run_dir, schema_path)

    feats = pd.read_csv(features_path)
    targs = pd.read_csv(targets_path)

    # join on symbol + end_ts (or asof_utc fallback)
    join_keys = [c for c in ["symbol", end_ts_col] if c in feats.columns and c in targs.columns]
    if "symbol" not in join_keys:
        raise RuntimeError("Eval requires symbol join key")
    if end_ts_col not in join_keys:
        if "asof_utc" in feats.columns and "asof_utc" in targs.columns:
            end_ts_col = "asof_utc"
            join_keys = ["symbol", "asof_utc"]
        else:
            raise RuntimeError("Eval requires end_ts or asof_utc join key")

    if label_col not in targs.columns:
        raise RuntimeError(f"targets.csv missing label column '{label_col}'")

    label_end_present = spec.label_end_ts_col in targs.columns
    if not label_end_present:
        raise RuntimeError(
            "Fail-closed: targets.csv must carry an explicit "
            f"'{spec.label_end_ts_col}' column (the truthful, inclusive label "
            "information horizon). This is required in ALL evaluation modes — "
            "including purge_enabled=False — because the reserved holdout's "
            "label-overlap isolation cannot be established without it. Refusing "
            "to guess a label horizon inside the evaluator."
        )

    target_cols = join_keys + [label_col] + ([spec.label_end_ts_col] if label_end_present else [])
    df = feats.merge(targs[target_cols], on=join_keys, how="inner", validate="one_to_one")
    df = df.sort_values(["symbol", join_keys[1]], kind="mergesort").reset_index(drop=True)

    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    feat_cols = list(schema["feature_columns"])

    X_all = df[feat_cols].to_numpy(dtype=np.float64)
    y_all = (df[label_col].astype(float) > 0.0).astype(int).to_numpy(dtype=np.int64)
    ts_all = pd.to_datetime(df[join_keys[1]], utc=True)

    if ts_all.isna().any():
        raise RuntimeError("Fail-closed: feature timestamp column contains missing/unparsable values")

    label_end_all: Optional[pd.Series] = None
    if label_end_present:
        label_end_all = pd.to_datetime(df[spec.label_end_ts_col], utc=True)
        if label_end_all.isna().any():
            raise RuntimeError(
                f"Fail-closed: '{spec.label_end_ts_col}' column contains missing/unparsable values"
            )
        if (label_end_all < ts_all).any():
            raise RuntimeError(
                f"Fail-closed: '{spec.label_end_ts_col}' < feature_ts for one or more rows "
                "(invalid label interval)"
            )

    t_min = ts_all.min()
    t_max = ts_all.max()
    discovery_anchor, holdout_start, dataset_end = compute_holdout_boundary(t_min, t_max, spec.holdout_months)

    if holdout_start <= discovery_anchor:
        raise RuntimeError(
            "Fail-closed: dataset too short to support both discovery folds and the "
            f"reserved {spec.holdout_months}-month holdout"
        )

    disc_pool_mask = ts_all < holdout_start
    if label_end_all is not None:
        holdout_boundary_purge_mask = disc_pool_mask & (label_end_all >= holdout_start)
    else:
        holdout_boundary_purge_mask = disc_pool_mask & False

    usable_mask = discovery_usable_mask(ts_all, label_end_all, holdout_start)
    holdout_mask = ~disc_pool_mask

    folds_raw = make_folds(t_min=t_min, discovery_end=holdout_start, spec=spec)
    if not folds_raw:
        raise RuntimeError(
            "Fail-closed: no walk-forward folds fit inside the discovery region "
            "(dataset too short for train/test spans plus the reserved holdout)"
        )

    eval_dir = run_dir / "eval"
    eval_dir.mkdir(parents=True, exist_ok=True)

    fold_metrics: List[Dict[str, Any]] = []
    for i, (tr_s, tr_e, te_s, te_e) in enumerate(folds_raw, start=1):
        tr_candidate_mask = usable_mask & (ts_all >= tr_s) & (ts_all < tr_e)
        te_mask = usable_mask & (ts_all >= te_s) & (ts_all < te_e)

        candidate_train_rows = int(tr_candidate_mask.sum())
        test_rows = int(te_mask.sum())

        embargo_td = pd.Timedelta(seconds=spec.embargo_seconds)
        effective_train_cutoff = te_s - embargo_td

        # Fold-level overlap/embargo purge (this fold's test boundary) may be
        # disabled by purge_enabled=False for legacy/diagnostic comparison.
        # Global holdout-label isolation (usable_mask, above) is NOT gated by
        # purge_enabled and remains enforced unconditionally.
        if spec.purge_enabled:
            overlap_purged_mask, embargo_purged_mask, tr_effective_mask = train_purge_masks(
                tr_candidate_mask, label_end_all, te_s, effective_train_cutoff
            )
        else:
            no_purge_mask = tr_candidate_mask & False
            overlap_purged_mask, embargo_purged_mask, tr_effective_mask = (
                no_purge_mask, no_purge_mask, tr_candidate_mask,
            )
        purged_overlap_rows = int(overlap_purged_mask.sum())
        purged_embargo_rows = int(embargo_purged_mask.sum())
        effective_train_rows = int(tr_effective_mask.sum())

        too_few = effective_train_rows < spec.min_rows_per_fold or test_rows < max(50, spec.min_rows_per_fold // 4)
        if too_few:
            reason = "too_few_rows_after_purge" if candidate_train_rows >= spec.min_rows_per_fold else "too_few_rows"
            fold_metrics.append({
                "fold": i,
                "train_start_utc": tr_s.isoformat(),
                "train_end_utc": tr_e.isoformat(),
                "test_start_utc": te_s.isoformat(),
                "test_end_utc": te_e.isoformat(),
                "effective_train_cutoff_utc": effective_train_cutoff.isoformat(),
                "candidate_train_rows": candidate_train_rows,
                "purged_overlap_rows": purged_overlap_rows,
                "purged_embargo_rows": purged_embargo_rows,
                "purged_train_rows": purged_overlap_rows + purged_embargo_rows,
                "effective_train_rows": effective_train_rows,
                "test_rows": test_rows,
                "skipped": True,
                "reason": reason,
            })
            continue

        X_tr = X_all[tr_effective_mask]
        y_tr = y_all[tr_effective_mask]
        X_te = X_all[te_mask]
        y_te = y_all[te_mask]

        X_tr_n = X_tr
        X_te_n = X_te
        if standardize:
            mean, std = _standardize_fit(X_tr)
            X_tr_n = _standardize_apply(X_tr, mean, std, clip_z)
            X_te_n = _standardize_apply(X_te, mean, std, clip_z)

        w, b = _fit_logreg_batch(X_tr_n, y_tr.astype(np.float64), l2=l2, lr=lr, steps=steps)
        p_te = _sigmoid(X_te_n @ w + b)

        auc = _auc_rank(y_te, p_te)
        ll = _logloss(y_te.astype(np.float64), p_te.astype(np.float64))

        max_used_label_end = None
        if label_end_all is not None and effective_train_rows > 0:
            max_used_label_end = label_end_all[tr_effective_mask].max()
            # Fold-level overlap/embargo invariant only holds when fold-level
            # purge is enabled — with purge_enabled=False this boundary is
            # intentionally not enforced (legacy/diagnostic mode).
            if spec.purge_enabled and max_used_label_end >= effective_train_cutoff:
                raise RuntimeError(
                    "Fail-closed: internal invariant violated — a used training row's label "
                    "consumes its fold's test/embargo boundary bar"
                )
            # Holdout isolation is a global invariant and is ALWAYS enforced,
            # regardless of purge_enabled.
            if max_used_label_end >= holdout_start:
                raise RuntimeError(
                    "Fail-closed: internal invariant violated — a used training row consumes "
                    "reserved holdout information"
                )
        if label_end_all is not None and test_rows > 0:
            max_test_label_end = label_end_all[te_mask].max()
            if max_test_label_end >= holdout_start:
                raise RuntimeError(
                    "Fail-closed: internal invariant violated — a used test row consumes "
                    "reserved holdout information"
                )

        min_test_feature_ts = ts_all[te_mask].min() if test_rows > 0 else None

        fold_metrics.append({
            "fold": i,
            "train_start_utc": tr_s.isoformat(),
            "train_end_utc": tr_e.isoformat(),
            "test_start_utc": te_s.isoformat(),
            "test_end_utc": te_e.isoformat(),
            "effective_train_cutoff_utc": effective_train_cutoff.isoformat(),
            "candidate_train_rows": candidate_train_rows,
            "purged_overlap_rows": purged_overlap_rows,
            "purged_embargo_rows": purged_embargo_rows,
            "purged_train_rows": purged_overlap_rows + purged_embargo_rows,
            "effective_train_rows": effective_train_rows,
            "test_rows": test_rows,
            "max_used_train_label_end_utc": max_used_label_end.isoformat() if max_used_label_end is not None else None,
            "min_test_feature_ts_utc": min_test_feature_ts.isoformat() if min_test_feature_ts is not None else None,
            "skipped": False,
            "metrics": {"auc": float(auc), "logloss": float(ll)},
        })

    folds_used = sum(1 for f in fold_metrics if not f.get("skipped"))
    if folds_used == 0:
        raise RuntimeError("Fail-closed: no usable walk-forward folds after purge/embargo/holdout isolation")

    holdout_rows = int(holdout_mask.sum())
    holdout_boundary_purged_rows = int(holdout_boundary_purge_mask.sum())

    out: Dict[str, Any] = {
        "schema_version": "walk_forward_eval_v2",
        "spec": {
            "train_years": spec.train_years,
            "test_months": spec.test_months,
            "step_months": spec.step_months,
            "min_rows_per_fold": spec.min_rows_per_fold,
            "purge_enabled": spec.purge_enabled,
            "label_end_ts_col": spec.label_end_ts_col,
            "embargo_seconds": spec.embargo_seconds,
            "holdout_months": spec.holdout_months,
        },
        "temporal_contract": {
            "interval_convention": (
                "label_end_ts is INCLUSIVE — the timestamp of the last bar whose close "
                "is consumed by the label, i.e. information window [feature_ts, "
                "label_end_ts]. Equality at a boundary means that boundary's "
                "information has already been consumed, so every boundary comparison "
                "is strict '<': a train row usable iff label_end_ts < "
                "effective_train_cutoff (= test_start - embargo); a discovery row "
                "usable iff feature_ts < holdout_start AND label_end_ts < holdout_start. "
                "label_end_ts is a mandatory column in ALL evaluation modes — holdout "
                "isolation cannot be established without it, regardless of "
                "fold_purge_enabled."
            ),
            "feature_ts_col": join_keys[1],
            "label_end_ts_col": spec.label_end_ts_col if label_end_present else None,
            "fold_purge_enabled": spec.purge_enabled,
            "embargo_seconds": spec.embargo_seconds,
            "discovery_anchor_utc": discovery_anchor.isoformat(),
            "holdout_start_utc": holdout_start.isoformat(),
            "discovery_end_utc": holdout_start.isoformat(),
            "dataset_end_utc": dataset_end.isoformat(),
        },
        "holdout": {
            "status": "reserved_not_evaluated",
            "start_utc": holdout_start.isoformat(),
            "end_utc": dataset_end.isoformat(),
            "rows": holdout_rows,
            "boundary_purged_discovery_rows": holdout_boundary_purged_rows,
            # fold_purge_enabled reflects only this fold's overlap/embargo purge
            # (may legitimately be False in legacy/diagnostic mode).
            # holdout_overlap_protection is the global invariant and is ALWAYS
            # "enforced" — it can never be disabled or degraded by purge_enabled,
            # because label_end_ts is a mandatory column in every mode.
            "fold_purge_enabled": spec.purge_enabled,
            "holdout_overlap_protection": "enforced",
        },
        "inputs": {
            "features_csv": file_record(features_path),
            "targets_csv": file_record(targets_path),
            "feature_schema": file_record(schema_path),
        },
        "folds": fold_metrics,
    }

    vals_auc = [f["metrics"]["auc"] for f in fold_metrics if not f.get("skipped") and isinstance(f.get("metrics"), dict) and "auc" in f["metrics"]]
    vals_ll = [f["metrics"]["logloss"] for f in fold_metrics if not f.get("skipped") and isinstance(f.get("metrics"), dict) and "logloss" in f["metrics"]]
    out["summary"] = {
        "folds_total": len(fold_metrics),
        "folds_used": len(vals_auc),
        "auc_mean": float(np.mean(vals_auc)) if vals_auc else float("nan"),
        "logloss_mean": float(np.mean(vals_ll)) if vals_ll else float("nan"),
    }
    out["ids"] = {"eval_id": sha256_json(out)}

    out_path = eval_dir / "walk_forward_eval.json"
    out_path.write_text(json.dumps(out, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    return out_path


def main_eval(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(prog="mqk-ml-eval-wf", description="Purged walk-forward evaluation with reserved holdout")
    ap.add_argument("--run-dir", required=True)
    ap.add_argument("--label", default="target")
    ap.add_argument("--train-years", type=int, default=3)
    ap.add_argument("--test-months", type=int, default=3)
    ap.add_argument("--step-months", type=int, default=3)
    ap.add_argument("--min-rows-fold", type=int, default=200)
    ap.add_argument("--steps", type=int, default=500)
    ap.add_argument("--lr", type=float, default=0.05)
    ap.add_argument("--l2", type=float, default=1e-3)
    ap.add_argument("--label-end-ts-col", default="label_end_ts")
    ap.add_argument("--embargo-seconds", type=int, default=0)
    ap.add_argument("--holdout-months", type=int, default=6)
    ap.add_argument(
        "--no-purge",
        action="store_true",
        help=(
            "Disable ONLY this fold's label-overlap purge/embargo (legacy/diagnostic "
            "compatibility; NOT promotion-quality research). Reserved-holdout label "
            "isolation is a separate, global invariant and is ALWAYS enforced — it "
            "cannot be disabled by this flag. targets.csv must still carry "
            "label_end_ts in every mode."
        ),
    )
    # RESEARCH-EXPERIMENT-REGISTRY-01: this CLI is registered by default. A
    # research-quality run must identify itself; an explicit, non-default flag
    # is required to opt out (see mqk_research.ml.registry_integration).
    ap.add_argument("--experiment-id", default=None, help="Required unless --allow-unregistered-diagnostic")
    ap.add_argument("--hypothesis-id", default=None, help="Required unless --allow-unregistered-diagnostic")
    ap.add_argument("--strategy-id", default=None, help="Required unless --allow-unregistered-diagnostic")
    ap.add_argument("--hypothesis-text", default=None, help="Optional human-readable hypothesis statement")
    ap.add_argument("--registry-db", default=None, help="Override the experiment registry SQLite path")
    ap.add_argument(
        "--allow-unregistered-diagnostic",
        action="store_true",
        help=(
            "Explicitly run WITHOUT registering a trial/attempt. Mutually exclusive "
            "with --experiment-id/--hypothesis-id/--strategy-id. Artifact is marked "
            "registry.registry_status=unregistered_diagnostic. NOT promotion-quality "
            "research."
        ),
    )
    args = ap.parse_args(argv)

    spec = WalkForwardSpec(
        train_years=args.train_years,
        test_months=args.test_months,
        step_months=args.step_months,
        min_rows_per_fold=args.min_rows_fold,
        purge_enabled=(not args.no_purge),
        label_end_ts_col=args.label_end_ts_col,
        embargo_seconds=args.embargo_seconds,
        holdout_months=args.holdout_months,
    )

    registration_ids = (args.experiment_id, args.hypothesis_id, args.strategy_id)
    if args.allow_unregistered_diagnostic:
        if any(registration_ids):
            ap.error(
                "--allow-unregistered-diagnostic cannot be combined with "
                "--experiment-id/--hypothesis-id/--strategy-id"
            )
        out_path = run_walkforward_eval(
            Path(args.run_dir),
            label_col=args.label,
            l2=args.l2,
            lr=args.lr,
            steps=args.steps,
            spec=spec,
        )
        out = json.loads(out_path.read_text(encoding="utf-8"))
        out["registry"] = {
            "schema_version": "research-experiment-registry-v1",
            "registry_status": "unregistered_diagnostic",
        }
        out_path.write_text(json.dumps(out, sort_keys=True, separators=(",", ":")), encoding="utf-8")
    elif all(registration_ids):
        from mqk_research.ml.registry_integration import run_registered_walkforward_eval

        out_path = run_registered_walkforward_eval(
            Path(args.run_dir),
            experiment_id=args.experiment_id,
            hypothesis_id=args.hypothesis_id,
            strategy_id=args.strategy_id,
            hypothesis_text=args.hypothesis_text,
            registry_db=Path(args.registry_db) if args.registry_db else None,
            label_col=args.label,
            l2=args.l2,
            lr=args.lr,
            steps=args.steps,
            spec=spec,
        )
    else:
        ap.error(
            "mqk-ml-eval-wf requires --experiment-id, --hypothesis-id, and --strategy-id "
            "for registered research runs, or pass --allow-unregistered-diagnostic for an "
            "explicit unregistered diagnostic run"
        )
        return 2

    print(f"OK eval={out_path}")
    return 0
