from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Tuple

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

    def normalized(self) -> "WalkForwardSpec":
        return WalkForwardSpec(
            enabled=bool(self.enabled),
            train_years=int(self.train_years),
            test_months=int(self.test_months),
            step_months=int(self.step_months),
            min_rows_per_fold=int(self.min_rows_per_fold),
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


def _make_folds(df: pd.DataFrame, *, end_col: str, spec: WalkForwardSpec) -> List[Tuple[pd.Timestamp, pd.Timestamp, pd.Timestamp, pd.Timestamp]]:
    ts = pd.to_datetime(df[end_col], utc=True)
    t_min = ts.min()
    t_max = ts.max()

    anchor = pd.Timestamp(year=t_min.year, month=t_min.month, day=1, tz="UTC")
    folds: List[Tuple[pd.Timestamp, pd.Timestamp, pd.Timestamp, pd.Timestamp]] = []
    train_span_months = spec.train_years * 12
    test_span_months = spec.test_months

    train_start = anchor
    while True:
        train_end = _month_add(train_start, train_span_months)
        test_start = train_end
        test_end = _month_add(test_start, test_span_months)
        if test_end > (t_max + pd.Timedelta(days=1)):
            break
        folds.append((train_start, train_end, test_start, test_end))
        train_start = _month_add(train_start, spec.step_months)
    return folds


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

    df = feats.merge(targs[join_keys + [label_col]], on=join_keys, how="inner", validate="one_to_one")
    df = df.sort_values(["symbol", join_keys[1]], kind="mergesort").reset_index(drop=True)

    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    feat_cols = list(schema["feature_columns"])

    X_all = df[feat_cols].to_numpy(dtype=np.float64)
    y_all = (df[label_col].astype(float) > 0.0).astype(int).to_numpy(dtype=np.int64)
    ts_all = pd.to_datetime(df[join_keys[1]], utc=True)

    folds = _make_folds(df, end_col=join_keys[1], spec=spec)
    if not folds:
        raise RuntimeError("Fail-closed: no walk-forward folds possible with current date span")

    eval_dir = run_dir / "eval"
    eval_dir.mkdir(parents=True, exist_ok=True)

    fold_metrics: List[Dict[str, Any]] = []
    for i, (tr_s, tr_e, te_s, te_e) in enumerate(folds, start=1):
        tr_mask = (ts_all >= tr_s) & (ts_all < tr_e)
        te_mask = (ts_all >= te_s) & (ts_all < te_e)

        n_tr = int(np.sum(tr_mask))
        n_te = int(np.sum(te_mask))
        if n_tr < spec.min_rows_per_fold or n_te < max(50, spec.min_rows_per_fold // 4):
            fold_metrics.append({
                "fold": i,
                "train_rows": n_tr,
                "test_rows": n_te,
                "skipped": True,
                "reason": "too_few_rows",
            })
            continue

        X_tr = X_all[tr_mask]
        y_tr = y_all[tr_mask]
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

        fold_metrics.append({
            "fold": i,
            "train_start_utc": tr_s.isoformat(),
            "train_end_utc": tr_e.isoformat(),
            "test_start_utc": te_s.isoformat(),
            "test_end_utc": te_e.isoformat(),
            "train_rows": n_tr,
            "test_rows": n_te,
            "skipped": False,
            "metrics": {"auc": float(auc), "logloss": float(ll)},
        })

    out: Dict[str, Any] = {
        "schema_version": "walk_forward_eval_v1",
        "spec": {
            "train_years": spec.train_years,
            "test_months": spec.test_months,
            "step_months": spec.step_months,
            "min_rows_per_fold": spec.min_rows_per_fold,
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

    ap = argparse.ArgumentParser(prog="mqk-ml-eval-wf", description="Walk-forward evaluation (scaffold)")
    ap.add_argument("--run-dir", required=True)
    ap.add_argument("--label", default="target")
    ap.add_argument("--train-years", type=int, default=3)
    ap.add_argument("--test-months", type=int, default=3)
    ap.add_argument("--step-months", type=int, default=3)
    ap.add_argument("--min-rows-fold", type=int, default=200)
    ap.add_argument("--steps", type=int, default=500)
    ap.add_argument("--lr", type=float, default=0.05)
    ap.add_argument("--l2", type=float, default=1e-3)
    args = ap.parse_args(argv)

    spec = WalkForwardSpec(train_years=args.train_years, test_months=args.test_months, step_months=args.step_months, min_rows_per_fold=args.min_rows_fold)
    out = run_walkforward_eval(
        Path(args.run_dir),
        label_col=args.label,
        l2=args.l2,
        lr=args.lr,
        steps=args.steps,
        spec=spec,
    )
    print(f"OK eval={out}")
    return 0
