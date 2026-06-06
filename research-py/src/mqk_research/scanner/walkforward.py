"""
WALKFORWARD-SPLIT-01 — Pure walk-forward date split planner.

Builds deterministic train/validation date splits for backtest planning.

Fail-closed rules:
- Pure Python only: no subprocess, no broker, no DB, no network, no runtime.
- Splits that would exceed end_date are excluded.
- Splits where train_days or validation_days fall below minimums are excluded.
- All splits are deterministic given the same inputs.

WALKFORWARD-RUNNER-01 remains OPEN:
  The Rust CLI (mqk backtest csv) does not yet support date-window filtering,
  so running the backtest engine over individual splits is deferred. This module
  only plans the splits; it does not execute them.

Hard invariants:
- No subprocess invocation of any kind.
- No broker, OMS, daemon, or production DB access.
- No network imports (requests, urllib, aiohttp, http.client).
- EXP penny scanner (exp-candidate-v1) is not affected by this module.
"""
from __future__ import annotations

from dataclasses import dataclass
from datetime import date, timedelta
from typing import Optional


@dataclass
class WalkForwardConfig:
    """
    Configuration for walk-forward date split building.

    train_days: calendar days in each training window (default 70).
    validation_days: calendar days in each validation window (default 30).
    step_days: calendar days to advance the cursor per fold (default 30).
    min_train_days: minimum acceptable training window (fold excluded if shorter).
    min_validation_days: minimum acceptable validation window (fold excluded if shorter).
    """
    train_days: int = 70
    validation_days: int = 30
    step_days: int = 30
    min_train_days: int = 14
    min_validation_days: int = 7


@dataclass
class WalkForwardSplit:
    """
    One train/validation fold produced by build_date_splits.

    fold_index: 0-based position in the fold sequence.
    train_start, train_end: inclusive calendar dates for the training window.
    validation_start, validation_end: inclusive calendar dates for the validation window.
    """
    fold_index: int
    train_start: date
    train_end: date
    validation_start: date
    validation_end: date

    @property
    def train_days(self) -> int:
        return (self.train_end - self.train_start).days + 1

    @property
    def validation_days(self) -> int:
        return (self.validation_end - self.validation_start).days + 1


def build_date_splits(
    start_date: date,
    end_date: date,
    config: Optional[WalkForwardConfig] = None,
) -> list[WalkForwardSplit]:
    """
    Build deterministic walk-forward train/validation splits.

    Anchors the first training window at start_date. Each subsequent fold
    advances by config.step_days. Folds that extend beyond end_date or whose
    windows fall below the minimum sizes are excluded.

    Pure function — no subprocess, no broker, no DB, no network.

    Returns an empty list if no valid folds can be built.
    """
    cfg = config or WalkForwardConfig()
    splits: list[WalkForwardSplit] = []
    fold_index = 0
    cursor = start_date

    while True:
        train_start = cursor
        train_end = train_start + timedelta(days=cfg.train_days - 1)
        validation_start = train_end + timedelta(days=1)
        validation_end = validation_start + timedelta(days=cfg.validation_days - 1)

        if validation_end > end_date:
            break

        train_length = (train_end - train_start).days + 1
        validation_length = (validation_end - validation_start).days + 1

        if train_length < cfg.min_train_days or validation_length < cfg.min_validation_days:
            break

        splits.append(WalkForwardSplit(
            fold_index=fold_index,
            train_start=train_start,
            train_end=train_end,
            validation_start=validation_start,
            validation_end=validation_end,
        ))
        fold_index += 1
        cursor = cursor + timedelta(days=cfg.step_days)

    return splits
