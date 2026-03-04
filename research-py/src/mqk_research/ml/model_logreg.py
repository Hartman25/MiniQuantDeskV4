from __future__ import annotations

import numpy as np


def _sigmoid(z: np.ndarray) -> np.ndarray:
    z = np.clip(z, -50.0, 50.0)
    return 1.0 / (1.0 + np.exp(-z))


class LogRegModel:
    def __init__(self, coef: np.ndarray, intercept: float, mean: np.ndarray | None, std: np.ndarray | None, feature_columns: list[str]):
        self.coef = coef
        self.intercept = float(intercept)
        self.mean = mean
        self.std = std
        self.feature_columns = list(feature_columns)

    def predict_proba(self, X: np.ndarray) -> np.ndarray:
        Xn = X
        if self.mean is not None and self.std is not None:
            std = np.where(self.std <= 1e-12, 1.0, self.std)
            Xn = (X - self.mean) / std
        z = Xn @ self.coef + self.intercept
        return _sigmoid(z)


def fit_logreg_deterministic(
    X: np.ndarray,
    y: np.ndarray,
    *,
    feature_columns: list[str],
    l2: float,
    lr: float,
    steps: int,
    fit_intercept: bool,
    standardize: bool,
    clip_z: float,
) -> LogRegModel:
    Xn = X.astype(np.float64, copy=True)
    yv = y.astype(np.float64, copy=False)

    mean = None
    std = None
    if standardize:
        mean = np.mean(Xn, axis=0)
        std = np.std(Xn, axis=0)
        std = np.where(std <= 1e-12, 1.0, std)
        Xn = (Xn - mean) / std
        if clip_z is not None and clip_z > 0.0:
            Xn = np.clip(Xn, -clip_z, clip_z)

    n, d = Xn.shape
    w = np.zeros(d, dtype=np.float64)
    b = 0.0

    for _ in range(int(steps)):
        z = Xn @ w + (b if fit_intercept else 0.0)
        p = _sigmoid(z)
        err = (p - yv)

        grad_w = (Xn.T @ err) / n + l2 * w
        w = w - lr * grad_w

        if fit_intercept:
            grad_b = float(np.mean(err))
            b = b - lr * grad_b

    return LogRegModel(w, b, mean, std, feature_columns)
