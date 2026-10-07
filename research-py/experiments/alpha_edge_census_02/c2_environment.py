"""Numerical-runtime identity for Census-02. The identity is frozen with the campaign and compared, exactly, before any real
data access; a materially different runtime cannot silently reproduce the same campaign identity. Nothing is installed,
upgraded or downgraded here."""

from __future__ import annotations

import platform

import numpy as np
import pandas as pd

IDENTITY_KEYS = ("python", "numpy", "pandas")


class EnvironmentMismatch(RuntimeError):
    pass


def environment_identity() -> dict:
    return {"python": platform.python_version(), "numpy": np.__version__, "pandas": pd.__version__}


def require_environment(frozen) -> dict:
    """Fail closed unless the runtime equals the frozen identity exactly (same keys, same versions)."""
    now = environment_identity()
    if not isinstance(frozen, dict) or sorted(frozen) != sorted(IDENTITY_KEYS) or not all(isinstance(v, str) and v for v in frozen.values()):
        raise EnvironmentMismatch("frozen environment identity is missing or malformed")
    diff = {k: {"frozen": frozen[k], "runtime": now[k]} for k in IDENTITY_KEYS if frozen[k] != now[k]}
    if diff:
        raise EnvironmentMismatch(f"runtime environment differs from the frozen identity: {diff}")
    return now
