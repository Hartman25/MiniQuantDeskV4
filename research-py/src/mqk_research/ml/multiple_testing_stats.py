from __future__ import annotations

import itertools
import math
from typing import Any, Dict, List, Optional, Sequence

import numpy as np

# RESEARCH-MULTIPLE-TESTING-JUDGE-01
#
# Pure, deterministic statistics for the Deflated Sharpe Ratio (DSR) and the
# Probability of Backtest Overfitting (PBO / CSCV). No IO, no wall-clock
# reads, no RNG. Every function operates on plain numpy arrays supplied by
# the caller and returns either a result or an explicit None/typed failure —
# never a silently fabricated number.
#
# References (paper-faithful, not a blog approximation):
#   Bailey & Lopez de Prado, "The Deflated Sharpe Ratio: Correcting for
#   Selection Bias, Backtest Overfitting and Non-Normality" (2014).
#   Bailey, Borwein, Lopez de Prado & Zhu, "The Probability of Backtest
#   Overfitting" (2017).
#
# Convention: DDOF=0 (population moments) throughout, matching
# mqk_research.ml.economics's existing convention (see economics.DDOF).

DDOF = 0
EULER_MASCHERONI = 0.5772156649015328606

# Floating-point std() of a mathematically-constant series is not exactly
# 0.0 (subtracting a mean computed via summation leaves residual noise, e.g.
# ~2e-19 for a 20-element array of 0.001) -- a strict `<= 0.0` guard lets
# that noise through as a "valid" but nonsensical Sharpe ratio in the
# 1e15+ range. This epsilon matches the repo's existing zero-variance
# convention (see eval_walkforward._standardize_fit's `std <= 1e-12`).
_ZERO_VARIANCE_EPS = 1e-12


# ---------------------------------------------------------------------------
# Normal CDF / inverse CDF (no scipy dependency in this workspace)
# ---------------------------------------------------------------------------


def norm_cdf(x: float) -> float:
    """Standard normal CDF via math.erf (exact to double precision)."""
    return 0.5 * (1.0 + math.erf(x / math.sqrt(2.0)))


_ACKLAM_A = (
    -3.969683028665376e01, 2.209460984245205e02, -2.759285104469687e02,
    1.383577518672690e02, -3.066479806614716e01, 2.506628277459239e00,
)
_ACKLAM_B = (
    -5.447609879822406e01, 1.615858368580409e02, -1.556989798598866e02,
    6.680131188771972e01, -1.328068155288572e01,
)
_ACKLAM_C = (
    -7.784894002430293e-03, -3.223964580411365e-01, -2.400758277161838e00,
    -2.549732539343734e00, 4.374664141464968e00, 2.938163982698783e00,
)
_ACKLAM_D = (
    7.784695709041462e-03, 3.224671290700398e-01, 2.445134137142996e00,
    3.754408661907416e00,
)
_ACKLAM_P_LOW = 0.02425


def norm_ppf(p: float) -> float:
    """Inverse standard normal CDF (probit). Peter Acklam's rational
    approximation (deterministic, no scipy dependency), refined with one
    Halley step against the exact erf-based CDF for near-double precision."""
    if not (0.0 < p < 1.0):
        raise ValueError(f"norm_ppf requires 0 < p < 1, got {p!r}")

    p_high = 1.0 - _ACKLAM_P_LOW
    a, b, c, d = _ACKLAM_A, _ACKLAM_B, _ACKLAM_C, _ACKLAM_D

    if p < _ACKLAM_P_LOW:
        q = math.sqrt(-2.0 * math.log(p))
        x = (((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]) / (
            (((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0
        )
    elif p <= p_high:
        q = p - 0.5
        r = q * q
        x = (((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) * q / (
            (((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1.0)
        )
    else:
        q = math.sqrt(-2.0 * math.log(1.0 - p))
        x = -(((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]) / (
            (((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0
        )

    # One Halley refinement step against the exact erf-based CDF.
    e = 0.5 * math.erfc(-x / math.sqrt(2.0)) - p
    u = e * math.sqrt(2.0 * math.pi) * math.exp(x * x / 2.0)
    x = x - u / (1.0 + x * u / 2.0)
    return float(x)


# ---------------------------------------------------------------------------
# Deflated Sharpe Ratio
# ---------------------------------------------------------------------------


def compute_trial_sharpe_stats(returns: np.ndarray) -> Dict[str, Any]:
    """Per-trial, per-period (NOT annualized) Sharpe/skewness/kurtosis
    statistics required by PSR/DSR, computed directly from `returns` (the
    trial's own excess-return series at whatever frequency it was sampled —
    the caller is responsible for subtracting the risk-free rate first, so
    this function never assumes it).

    `kurtosis_raw` is Pearson's (RAW) kurtosis convention where a Gaussian
    series has kurtosis_raw == 3.0 -- NOT excess kurtosis (Gaussian == 0.0).
    This is deliberate: the DSR paper's (gamma4 - 1)/4 term is defined in
    terms of raw kurtosis. Passing excess kurtosis here would silently
    corrupt every downstream DSR value by a constant offset.

    Returns a dict with `evaluable: bool`. When False, `reason` names a
    deterministic, typed failure cause; no field is fabricated for the
    caller to accidentally treat as evaluable.
    """
    arr = np.asarray(returns, dtype=np.float64)
    n = int(arr.shape[0])
    if n < 2:
        return {"evaluable": False, "reason": "insufficient_observations", "observations": n}
    if not np.all(np.isfinite(arr)):
        return {"evaluable": False, "reason": "non_finite_returns", "observations": n}

    mean = float(np.mean(arr))
    std = float(np.std(arr, ddof=DDOF))
    if std <= _ZERO_VARIANCE_EPS:
        return {"evaluable": False, "reason": "zero_variance_returns", "observations": n}

    m2 = std * std
    m3 = float(np.mean((arr - mean) ** 3))
    m4 = float(np.mean((arr - mean) ** 4))
    skewness = m3 / (m2 ** 1.5)
    kurtosis_raw = m4 / (m2 ** 2)

    return {
        "evaluable": True,
        "observations": n,
        "sharpe_per_period": float(mean / std),
        "skewness": float(skewness),
        "kurtosis_raw": float(kurtosis_raw),
    }


def expected_max_sharpe(trial_sharpe_estimates: Sequence[float]) -> Dict[str, Any]:
    """E[max_n{SR_n}] under N independent trials (Bailey & Lopez de Prado
    2014, eq. for the expected maximum of N Sharpe-ratio estimates via
    extreme value theory), using the OBSERVED cross-trial variance of the
    supplied per-period Sharpe estimates as V[{SR_n}].

    Requires len(trial_sharpe_estimates) >= 2 -- the cross-trial variance
    used by the correction is undefined for a single trial (there would be
    no multiple-testing selection effect to correct for). Raises ValueError
    in that case; callers must catch this and report a typed not_evaluable
    result rather than let it propagate as an unhandled crash.
    """
    n = len(trial_sharpe_estimates)
    if n < 2:
        raise ValueError("expected_max_sharpe requires at least 2 trial Sharpe estimates")
    arr = np.asarray(trial_sharpe_estimates, dtype=np.float64)
    if not np.all(np.isfinite(arr)):
        raise ValueError("expected_max_sharpe received non-finite Sharpe estimates")
    variance = float(np.var(arr, ddof=DDOF))
    if variance <= _ZERO_VARIANCE_EPS * _ZERO_VARIANCE_EPS:
        # No cross-trial dispersion to correct for: every trial produced the
        # identical per-period Sharpe estimate, so the expected maximum of N
        # identical draws is just that shared value -- not a fabricated 0.0.
        return {"expected_max_sharpe": float(np.mean(arr)), "variance_of_trial_sharpe": 0.0, "trials_used": n}
    sigma = math.sqrt(variance)
    z1 = norm_ppf(1.0 - 1.0 / n)
    z2 = norm_ppf(1.0 - 1.0 / (n * math.e))
    e_max = sigma * ((1.0 - EULER_MASCHERONI) * z1 + EULER_MASCHERONI * z2)
    return {"expected_max_sharpe": float(e_max), "variance_of_trial_sharpe": variance, "trials_used": n}


def probabilistic_sharpe_ratio(
    *, sharpe_hat: float, sharpe_benchmark: float, observations: int, skewness: float, kurtosis_raw: float
) -> Optional[float]:
    """PSR[SR*] (Bailey & Lopez de Prado 2012/2014): probability that the
    true Sharpe ratio exceeds `sharpe_benchmark`, adjusting for sample
    length and non-normality (skew/kurtosis) of the return series that
    produced `sharpe_hat`.

    Returns None (not 0.0, not NaN) when the denominator is degenerate
    (observations < 2, or the skew/kurtosis correction term is <= 0) --
    a truthful "cannot be computed", not a fabricated probability.
    """
    if observations < 2:
        return None
    denom_inner = 1.0 - skewness * sharpe_hat + ((kurtosis_raw - 1.0) / 4.0) * (sharpe_hat ** 2)
    if not math.isfinite(denom_inner) or denom_inner <= 0.0:
        return None
    denom = math.sqrt(denom_inner)
    z = (sharpe_hat - sharpe_benchmark) * math.sqrt(observations - 1) / denom
    if not math.isfinite(z):
        return None
    return norm_cdf(z)


# ---------------------------------------------------------------------------
# Probability of Backtest Overfitting (CSCV)
# ---------------------------------------------------------------------------


def _column_sharpe(mat: np.ndarray) -> np.ndarray:
    """Per-column per-period Sharpe (ddof=0). NaN where std==0 (degenerate),
    never a fabricated 0.0 or inf."""
    mean = np.mean(mat, axis=0)
    std = np.std(mat, axis=0, ddof=DDOF)
    valid = std > _ZERO_VARIANCE_EPS
    with np.errstate(invalid="ignore", divide="ignore"):
        sharpe = np.where(valid, mean / np.where(valid, std, 1.0), np.nan)
    return sharpe


def combinatorial_symmetric_cv_pbo(returns_matrix: np.ndarray, *, block_count: int) -> Dict[str, Any]:
    """Combinatorially Symmetric Cross-Validation PBO (Bailey, Borwein,
    Lopez de Prado & Zhu, "The Probability of Backtest Overfitting").

    `returns_matrix` is a (T, N) array: T chronologically-ordered observation
    rows (the caller is responsible for sorting them -- this function only
    ever partitions by ROW POSITION, so row order in is load-bearing and
    must already be chronological), N candidate-trial columns.

    Splits the T rows into `block_count` contiguous, (as close to) equal
    blocks. For every way of choosing exactly block_count/2 of those blocks
    as the in-sample (IS) set (the complement is out-of-sample, OOS) --
    C(block_count, block_count/2) combinations, enumerated in a fixed
    deterministic order -- selects the IS-best column by per-period Sharpe,
    finds that column's OOS relative rank among all N columns' OOS Sharpe,
    and converts the relative rank to the paper's logit statistic. PBO is
    the fraction of combinations whose logit is <= 0 (the IS-selected
    candidate's OOS performance fell at or below the cross-sectional
    median -- the paper's overfitting signature).

    Column order does not affect the result except for reporting
    (n_star/rank are column INDICES into `returns_matrix`, which the caller
    must have already established via a deterministic, e.g. sorted-trial-id,
    ordering) -- this function does not re-sort columns. Row order fully
    determines the block partition, so callers must supply chronologically
    sorted rows for a result that means what the paper means by it.
    """
    returns_matrix = np.asarray(returns_matrix, dtype=np.float64)
    if returns_matrix.ndim != 2:
        raise ValueError("returns_matrix must be 2-D (T observations x N candidates)")
    t_obs, n_candidates = returns_matrix.shape
    if n_candidates < 2:
        raise ValueError("PBO/CSCV requires at least 2 candidate columns")
    if block_count < 2 or block_count % 2 != 0:
        raise ValueError("block_count must be even and >= 2")
    if t_obs < block_count:
        raise ValueError("not enough observations for the requested block_count")
    if not np.all(np.isfinite(returns_matrix)):
        raise ValueError("returns_matrix contains non-finite values")

    block_row_indices = np.array_split(np.arange(t_obs), block_count)
    half = block_count // 2
    combos = list(itertools.combinations(range(block_count), half))

    logits: List[float] = []
    ranks: List[int] = []
    degenerate_skipped = 0

    for combo in combos:
        is_blocks = set(combo)
        is_rows = np.concatenate([block_row_indices[b] for b in sorted(is_blocks)])
        oos_rows = np.concatenate([block_row_indices[b] for b in range(block_count) if b not in is_blocks])

        is_perf = _column_sharpe(returns_matrix[is_rows, :])
        oos_perf = _column_sharpe(returns_matrix[oos_rows, :])

        if np.all(np.isnan(is_perf)):
            degenerate_skipped += 1
            continue
        n_star = int(np.nanargmax(is_perf))  # deterministic: first max on ties

        oos_valid = ~np.isnan(oos_perf)
        if not bool(oos_valid[n_star]) or int(oos_valid.sum()) < 1:
            degenerate_skipped += 1
            continue

        order = np.argsort(np.where(oos_valid, oos_perf, -np.inf), kind="mergesort")
        valid_order = [int(idx) for idx in order if bool(oos_valid[idx])]
        rank_position = valid_order.index(n_star) + 1  # 1 = worst OOS performer
        n_valid = len(valid_order)
        relative_rank = rank_position / (n_valid + 1.0)
        logit = math.log(relative_rank / (1.0 - relative_rank))
        logits.append(float(logit))
        ranks.append(rank_position)

    combinations_total = len(combos)
    combinations_evaluated = len(logits)
    if combinations_evaluated == 0:
        raise ValueError("no evaluable CSCV combinations (every combination was degenerate)")

    pbo = sum(1 for lam in logits if lam <= 0.0) / combinations_evaluated
    return {
        "block_count": block_count,
        "combinations_total": combinations_total,
        "combinations_evaluated": combinations_evaluated,
        "combinations_skipped_degenerate": degenerate_skipped,
        "pbo": float(pbo),
        "logit_mean": float(np.mean(logits)),
        "logit_median": float(np.median(logits)),
        "logit_min": float(min(logits)),
        "logit_max": float(max(logits)),
        "in_sample_selection_metric": "sharpe_ratio_per_period_ddof0",
    }


def choose_cscv_block_count(t_obs: int, target_block_count: int) -> Optional[int]:
    """Deterministically derive an even block count <= target_block_count
    such that every block gets at least 2 observations. Returns None if
    even the smallest usable design (block_count=4, 2 obs/block) does not
    fit -- callers must treat that as not_evaluable, never silently proceed
    with an ill-posed partition."""
    if target_block_count < 4 or target_block_count % 2 != 0:
        raise ValueError("target_block_count must be even and >= 4")
    block_count = target_block_count
    while block_count >= 4:
        if t_obs // block_count >= 2:
            return block_count
        block_count -= 2
    return None
