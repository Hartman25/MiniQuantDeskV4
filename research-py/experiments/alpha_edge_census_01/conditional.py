"""Non-executable ConditionalEdge statistics. fwd_ret_h = close[t+h]/close[t]-1 is a label only, never P&L."""

from __future__ import annotations

import math

import numpy as np

REGIME_NAMES = {0: "RISK_OFF", 1: "RISK_ON", 2: "UNKNOWN"}
TINY_N = 30


def _fwd(sd, h: int) -> np.ndarray:
    return sd.memo(("fwd", h), lambda: sd.c[h:] / sd.c[:-h] - 1.0)


def conditional_stats(members, horizons) -> dict[int, dict | None]:
    """members: [(SymbolData, Sig)]. Returns per-horizon record body, or None when no conditioned sample exists
    (no event with a full forward window). The unconditional baseline pools the same windows."""
    out: dict[int, dict | None] = {}
    for h in horizons:
        vals, yrs, regs = [], [], []
        u_sum, u_n = 0.0, 0
        for sd, sig in members:
            if sd.n <= h or sig.s > sd.n - 1 - h:
                continue
            fr = _fwd(sd, h)
            lo, hi = sig.s, sd.n - h
            win = fr[lo:hi]
            u_sum += float(win.sum())
            u_n += len(win)
            sel = sig.cond[lo:hi]
            if sel.any():
                vals.append(win[sel])
                yrs.append(sd.years[lo:hi][sel])
                regs.append(sd.regime[lo:hi][sel])
        if not vals or u_n == 0:
            out[h] = None
            continue
        v = np.concatenate(vals)
        n = len(v)
        mean = float(v.mean())
        uncond = u_sum / u_n
        y = np.concatenate(yrs)
        r = np.concatenate(regs).astype(np.int64)
        year = {str(int(k)): [int((y == k).sum()), float(v[y == k].sum())] for k in np.unique(y)}
        regime = {REGIME_NAMES[int(k)]: [int((r == k).sum()), float(v[r == k].sum())] for k in np.unique(r)}
        std = float(v.std(ddof=1)) if n > 1 else None
        out[h] = {"horizon": h, "n": n, "mean": mean, "median": float(np.median(v)), "std": std,
                  "positive_freq": float((v > 0).mean()), "uncond_mean": uncond, "uncond_n": u_n,
                  "effect": mean - uncond, "year": year, "regime": regime, "tiny_sample": n < TINY_N}
    return out


def is_positive(rec: dict | None) -> bool:
    return rec is not None and math.isfinite(rec["effect"]) and rec["effect"] > 0.0
