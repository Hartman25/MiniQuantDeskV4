"""Shared synthetic fixtures for the Census-02 campaign tests. Nothing here reads real data."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

import numpy as np
import pandas as pd

EXP2 = Path(__file__).resolve().parents[1] / "experiments" / "alpha_edge_census_02"
sys.path.insert(0, str(EXP2))

import c2_borrow as bw  # noqa: E402
import c2_data  # noqa: E402
import c2_protocol as pr  # noqa: E402
import search_space as ss1  # noqa: E402
import signals as sg1  # noqa: E402


def bars_from_close(symbol: str, close: np.ndarray) -> pd.DataFrame:
    n = len(close)
    close = np.round(close, 2)
    open_ = np.concatenate([[close[0]], close[:-1]])
    hi = np.maximum(open_, close) * 1.004
    lo = np.minimum(open_, close) * 0.996
    ts = [pd.Timestamp(d, tz="UTC") + pd.Timedelta(hours=5) for d in sg1.SESSIONS[:n]]
    return pd.DataFrame({"symbol": symbol, "end_ts": ts, "open": open_, "high": np.round(hi, 2), "low": np.round(lo, 2),
                         "close": close, "volume": 1e6})


def noise_bars(symbol: str, seed: int, n: int = 700, sig: float = 0.012, gap: float = 0.0) -> pd.DataFrame:
    rng = np.random.default_rng(seed)
    close = 50.0 * np.exp(np.cumsum(0.0002 + sig * rng.standard_normal(n)))
    df = bars_from_close(symbol, close)
    if gap:
        g = 1 + gap * rng.standard_normal(n)
        df["open"] = np.round(df["open"] * g, 2)
        df["high"] = np.maximum(df["high"], np.maximum(df["open"], df["close"]))
        df["low"] = np.minimum(df["low"], np.minimum(df["open"], df["close"]))
    return df


def streak_reversal_bars(symbol: str, seed: int, n: int = 700, push: float = -0.03) -> pd.DataFrame:
    """A path where, after two consecutive up closes (the SH11 up_sessions=2 event), the NEXT return is `push` (+ noise).
    push<0 is a short-favorable event; push>0 is the opposite (negative control)."""
    rng = np.random.default_rng(seed)
    close = np.empty(n)
    close[0] = 50.0
    close[1] = 50.3
    for t in range(2, n):
        event_prev = t >= 3 and close[t - 1] > close[t - 2] > close[t - 3]
        r = (push if event_prev else 0.0) + 0.012 * rng.standard_normal()
        close[t] = close[t - 1] * np.exp(r)
    return bars_from_close(symbol, close)


def git(repo: Path, *a: str) -> None:
    subprocess.run(["git", *a], cwd=repo, check=True, capture_output=True)


def make_frozen_repo(root: Path) -> tuple[Path, Path]:
    """A throwaway git repo with every bound source committed, then the production-built FROZEN predeclaration committed."""
    for rel in (*pr.BEHAVIOR_SOURCES, *pr.AUTHORITY_DATA):
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(pr.REPO / rel, root / rel)
    git(root, "init", "-q")
    git(root, "config", "user.email", "t@t")
    git(root, "config", "user.name", "t")
    git(root, "add", "-A")
    git(root, "commit", "-qm", "behavior")
    f = root / "CENSUS02_PREDECLARATION.json"
    f.write_text(json.dumps(pr.build_predeclaration(root), sort_keys=True), encoding="utf-8")
    git(root, "add", "-A")
    git(root, "commit", "-qm", "freeze")
    return root, f


def synthetic_universe(n_bars: int = 700) -> c2_data.LoadedUniverse:
    seed = json.loads(bw.SEED_UNIVERSE_FILE.read_text(encoding="utf-8"))
    symbols = sorted(seed["symbols"])
    bars = {s: noise_bars(s, i + 1, n_bars, gap=0.02) for i, s in enumerate(symbols)}
    disp = {s: {"symbol": s, "disposition": "ELIGIBLE", "observations": len(bars[s])} for s in symbols}
    return c2_data.LoadedUniverse(ss1.build_universe(seed, disp), bars, {"manifest_sha256": "synthetic-not-real"})
