from __future__ import annotations

from dataclasses import dataclass
from typing import Dict, List, Optional


@dataclass(frozen=True)
class MLTrainConfig:
    # join keys for features.csv / targets.csv
    id_columns: List[str] = None

    # label column in targets.csv
    label_column: str = "target"

    # feature selection
    feature_exclude_prefixes: List[str] = None

    # optimization
    l2: float = 1e-3
    lr: float = 0.05
    steps: int = 500
    fit_intercept: bool = True

    # preprocessing
    standardize: bool = True
    clip_z: float = 8.0

    # fail-closed gates
    min_rows: int = 200

    def normalized(self) -> "MLTrainConfig":
        return MLTrainConfig(
            id_columns=self.id_columns or ["symbol", "end_ts"],
            label_column=self.label_column,
            feature_exclude_prefixes=self.feature_exclude_prefixes or ["debug_", "tmp_"],
            l2=float(self.l2),
            lr=float(self.lr),
            steps=int(self.steps),
            fit_intercept=bool(self.fit_intercept),
            standardize=bool(self.standardize),
            clip_z=float(self.clip_z),
            min_rows=int(self.min_rows),
        )


@dataclass(frozen=True)
class ModelArtifact:
    model_type: str
    feature_columns: List[str]
    coef: List[float]
    intercept: float

    # standardization params (optional)
    mean: Optional[List[float]] = None
    std: Optional[List[float]] = None

    # bookkeeping
    train_rows: int = 0
    train_pos_rate: float = 0.0
    metrics: Optional[Dict[str, float]] = None
