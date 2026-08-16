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


def expected_max_sharpe(
    trial_sharpe_estimates: Sequence[float], *, effective_independent_trials: float
) -> Dict[str, Any]:
    """SR_0, the DSR null-rejection benchmark (Bailey & Lopez de Prado 2014,
    eq. 2's SR_0 term): the expected maximum Sharpe ratio one would observe
    by chance alone under the null hypothesis that the TRUE Sharpe ratio is
    zero for every trial. This is deliberately NOT eq. 1's general
    E[max{SR_n}] ~= E[{SR_n}] + sqrt(V[{SR_n}]) * (...) -- SR_0 omits the
    E[{SR_n}] term on purpose, because it is a REJECTION THRESHOLD computed
    under H0 (true SR = 0 for every trial), not a forecast of the actually
    observed maximum. Passing the OBSERVED cross-trial mean into this
    threshold would silently leak the very selection bias DSR exists to
    correct for.

    Two independent inputs, matching the paper's own N != M distinction
    (Appendix A.3):
      - V[{SR_n}], the OBSERVED cross-trial variance, is computed here from
        `trial_sharpe_estimates` -- the actual, raw (dependent) per-trial
        Sharpe estimates. This describes what was actually observed.
      - `effective_independent_trials` (N) is the count plugged into the
        extreme-value quantile terms Z^-1[1-1/N] / Z^-1[1-1/(Ne)]. It is
        supplied by the caller (see estimate_effective_independent_trials)
        and MAY be a non-integer real number -- the paper's eq. 9 does not
        round it, and rounding here would fabricate false precision.
    These two must never be conflated: dispersion comes from what was
    observed across the raw M trials; correction strength comes from how
    many of those trials the data suggests were genuinely independent.

    Requires len(trial_sharpe_estimates) >= 2 (no cross-trial variance is
    defined for one trial) and effective_independent_trials > 1 (N<=1 means
    no multiple-testing selection effect exists to correct for -- callers
    must resolve that via estimate_effective_independent_trials's own
    not_evaluable path BEFORE calling this function, never by clamping N
    here). Raises ValueError in either case; callers must catch this and
    report a typed not_evaluable result rather than let it propagate.
    """
    m = len(trial_sharpe_estimates)
    if m < 2:
        raise ValueError("expected_max_sharpe requires at least 2 trial Sharpe estimates")
    n = float(effective_independent_trials)
    if not math.isfinite(n) or n <= 1.0:
        raise ValueError(
            f"expected_max_sharpe requires effective_independent_trials > 1, got {n!r}"
        )
    arr = np.asarray(trial_sharpe_estimates, dtype=np.float64)
    if not np.all(np.isfinite(arr)):
        raise ValueError("expected_max_sharpe received non-finite Sharpe estimates")
    variance = float(np.var(arr, ddof=DDOF))
    if variance <= _ZERO_VARIANCE_EPS * _ZERO_VARIANCE_EPS:
        # Paper-faithful null benchmark (eq. 2): SR_0 = sqrt(V[{SR_n}]) *
        # (...). With zero OBSERVED cross-trial dispersion, sqrt(0) = 0
        # exactly -- SR_0 IS 0.0, not the shared observed Sharpe. There is
        # no selection-bias inflation to correct for when every trial
        # produced an identical estimate: nothing was "selected" over
        # anything else, so the null-rejection threshold collapses to the
        # plain (uncorrected) PSR benchmark of zero. (A prior implementation
        # substituted the shared observed Sharpe here instead -- see
        # test_zero_cross_trial_variance_benchmark_is_zero_not_observed_mean
        # for the regression proof that behavior was wrong.)
        return {
            "expected_max_sharpe": 0.0,
            "variance_of_trial_sharpe": 0.0,
            "trials_used": m,
            "effective_independent_trials": n,
            "null_benchmark_basis": "zero_cross_trial_variance",
        }
    sigma = math.sqrt(variance)
    z1 = norm_ppf(1.0 - 1.0 / n)
    z2 = norm_ppf(1.0 - 1.0 / (n * math.e))
    e_max = sigma * ((1.0 - EULER_MASCHERONI) * z1 + EULER_MASCHERONI * z2)
    return {
        "expected_max_sharpe": float(e_max),
        "variance_of_trial_sharpe": variance,
        "trials_used": m,
        "effective_independent_trials": n,
        "null_benchmark_basis": "selection_adjusted",
    }


# ---------------------------------------------------------------------------
# Implied independent trial count (Bailey & Lopez de Prado 2014, Appendix
# A.3, "ESTIMATING THE NUMBER OF INDEPENDENT TRIALS", eq. 7-9)
# ---------------------------------------------------------------------------
#
# The paper is explicit that the N used in expected_max_sharpe/DSR is the
# number of INDEPENDENT trials, not the raw number attempted (M). Given M
# dependent trials, Appendix A.3 derives the "implied independent trials"
# N-hat from the trials' average pairwise correlation rho-hat:
#
#   rho-hat = (2 * sum_{i<j} rho_ij) / (M * (M - 1))                  (eq. 8)
#   N-hat   = rho-hat + (1 - rho-hat) * M                             (eq. 9)
#
# Boundary-faithful: rho-hat -> 1 collapses N-hat -> 1 (fully dependent
# trials behave as one); rho-hat -> 0 leaves N-hat -> M (fully independent
# trials are unpenalized). N-hat is not rounded to an integer -- eq. 9 is a
# continuous real-valued estimator.
#
# The paper also warns (Appendix A.3, immediately after eq. 9): "in general
# for short samples (T < 1/2 M(M-1)), the correlation matrix will be
# numerically ill-conditioned... Estimating an average correlation is then
# pointless, because there are more correlations {rho_ij} than independent
# pairs of observations." This module treats that threshold as a hard,
# typed not_evaluable gate rather than silently returning an estimate the
# paper itself says is not numerically defensible.

ILL_CONDITIONED_CORRELATION = "ill_conditioned_correlation_estimate"
ZERO_VARIANCE_COLUMN = "zero_variance_column"
NON_FINITE_CORRELATION_MATRIX = "non_finite_correlation_matrix"
EFFECTIVE_TRIALS_DEGENERATE = "effective_independent_trials_degenerate"

# Boundary epsilon for N-hat <= 1: an implied single independent trial (or
# fewer, which the paper's own bounds do not actually permit for a valid
# correlation matrix) has no multiple-testing selection effect left to
# correct for -- mirrors this module's existing raw-trial-count n<2 gate.
_EFFECTIVE_N_DEGENERATE_EPS = 1e-9


def average_pairwise_correlation(returns_matrix: np.ndarray) -> Dict[str, Any]:
    """Equal-weighted average pairwise correlation among the M columns of
    `returns_matrix` (T chronologically-aligned observation rows x M trial
    columns), per Bailey & Lopez de Prado 2014, Appendix A.3 eq. 7-8.

    Column-order invariant by construction (a symmetric average over every
    i<j pair). Row-order invariant PROVIDED the caller has already aligned
    every column to the SAME chronological row index (Pearson correlation
    is a paired-observation statistic -- any single permutation applied
    identically to every column leaves every pairwise correlation, and
    therefore the average, unchanged); this function does not itself sort
    rows, matching multiple_testing_stats.combinatorial_symmetric_cv_pbo's
    existing row-order contract.

    Returns a typed not_evaluable result (never a silently-computed but
    numerically indefensible estimate) when:
      - T < M*(M-1)/2 -- the paper's own ill-conditioning warning (Appendix
        A.3): with more pairwise correlations to estimate than independent
        observation-pairs available, "estimating an average correlation is
        pointless."
      - any column has (numerically) zero variance -- Pearson correlation
        against a constant series is undefined, not zero.
    """
    returns_matrix = np.asarray(returns_matrix, dtype=np.float64)
    if returns_matrix.ndim != 2:
        raise ValueError("returns_matrix must be 2-D (T observations x M candidates)")
    t_obs, m = returns_matrix.shape
    if m < 2:
        raise ValueError("average_pairwise_correlation requires at least 2 candidate columns")
    if not np.all(np.isfinite(returns_matrix)):
        raise ValueError("returns_matrix contains non-finite values")

    ill_conditioning_threshold = 0.5 * m * (m - 1)
    base = {"trial_count": m, "observations": t_obs, "ill_conditioning_threshold": ill_conditioning_threshold}
    if t_obs < ill_conditioning_threshold:
        return {**base, "evaluable": False, "reason": ILL_CONDITIONED_CORRELATION}

    std = np.std(returns_matrix, axis=0, ddof=DDOF)
    if np.any(std <= _ZERO_VARIANCE_EPS):
        return {**base, "evaluable": False, "reason": ZERO_VARIANCE_COLUMN}

    corr = np.corrcoef(returns_matrix, rowvar=False)
    if not np.all(np.isfinite(corr)):
        return {**base, "evaluable": False, "reason": NON_FINITE_CORRELATION_MATRIX}

    iu = np.triu_indices(m, k=1)
    rho = float(np.mean(corr[iu]))
    return {**base, "evaluable": True, "average_pairwise_correlation": rho}


def estimate_effective_independent_trials(returns_matrix: np.ndarray) -> Dict[str, Any]:
    """Implied independent trial count N-hat (Bailey & Lopez de Prado 2014,
    Appendix A.3 eq. 9): N-hat = rho-hat + (1 - rho-hat) * M, where M is the
    raw (dependent) trial count and rho-hat is the equal-weighted average
    pairwise correlation among the M trials' return series (see
    average_pairwise_correlation).

    Returns a typed not_evaluable result (never a silent fallback to raw M)
    when: the correlation estimate itself is not numerically defensible, or
    the implied N-hat is degenerate (<= 1, e.g. duplicate/near-duplicate
    candidates whose average correlation is ~1) -- an implied single
    independent trial has no multiple-testing selection effect left to
    correct for.
    """
    corr_result = average_pairwise_correlation(returns_matrix)
    base = {"trial_count": corr_result["trial_count"], "observations": corr_result["observations"]}
    if not corr_result["evaluable"]:
        return {**base, "evaluable": False, "reason": corr_result["reason"]}

    m = corr_result["trial_count"]
    rho = corr_result["average_pairwise_correlation"]
    n_hat = rho + (1.0 - rho) * m
    if not math.isfinite(n_hat) or n_hat <= 1.0 + _EFFECTIVE_N_DEGENERATE_EPS:
        return {
            **base,
            "evaluable": False,
            "reason": EFFECTIVE_TRIALS_DEGENERATE,
            "average_pairwise_correlation": rho,
            "effective_independent_trials": float(n_hat) if math.isfinite(n_hat) else None,
        }
    return {
        **base,
        "evaluable": True,
        "average_pairwise_correlation": rho,
        "effective_independent_trials": float(n_hat),
        "ill_conditioning_threshold": corr_result["ill_conditioning_threshold"],
    }


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
