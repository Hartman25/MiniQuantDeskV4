"""ConditionalEdge authority (V3): every (semantic condition x horizon) is a deterministic registered
mqk_research.factors FactorSpec BEFORE any result; evaluation runs through the registered runner (durable attempt
opened before computation); the empirical-null p-value and the BH/FDR population come from the repo-native factor
machinery. A factor is a semantic conditional relationship over the frozen eligible universe: only condition-defining
parameters enter its identity (search_space.CONDITION_PARAM_KEYS); Strategy exit/hold parameters never do. Labels are
diagnostic forward returns, never P&L."""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

import numpy as np
import pandas as pd

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

import search_space as ss  # noqa: E402
from mqk_research.factors.contracts import (  # noqa: E402
    DIRECTION_HIGHER_IS_BETTER, EVAL_STATUS_SUCCEEDED, NORMALIZATION_RAW, TIMING_SAME_BAR_CLOSE, FactorSpec)
from mqk_research.factors.fdr import (  # noqa: E402
    EMPIRICAL_PVALUE_PROTOCOL_VERSION, FactorPValueEvidence, build_fdr_population_report)
from mqk_research.factors.null_controls import _derive_period_seed  # noqa: E402
from mqk_research.factors.registry import (  # noqa: E402
    list_factor_evaluation_attempts, list_factors, register_factor)
from mqk_research.factors.runner import run_registered_factor_diagnostics  # noqa: E402

FACTOR_FAMILY = "alpha_census_conditional_v3"
FACTOR_PROTOCOL_VERSION = "alpha_census_conditional_factor_v3"
FACTOR_FAMILY_V2 = "alpha_census_conditional_v2"  # rejected: REJECTED_SEMANTIC_DUPLICATE_FACTOR_POPULATION
FACTOR_PROTOCOL_VERSION_V2 = "alpha_census_conditional_factor_v2"
LABEL_PROTOCOL_VERSION = "alpha_census_fwd_close_return_minus_symbol_baseline_v1"
WINDOW_START_UTC = "2016-01-01T00:00:00+00:00"
WINDOW_END_UTC = "2024-01-01T00:00:00+00:00"
N_QUANTILES = 2
MIN_CROSS_SECTION = 10
MIN_PERIODS = 30
TIE_TOLERANCE = 1e-12
FRAME_COLUMNS = ["symbol", "period_ts_utc", "factor_value", "label_fwd_ret", "information_cutoff_ts_utc",
                 "label_end_ts_utc"]
REGIME_NAMES = {0: "risk_off", 1: "risk_on", 2: "unknown"}

_REQUIRED_FIELDS = {"S01": ["close"], "S02": ["close"], "S03": ["close"], "S04": ["close"], "S05": ["close"],
                    "S06": ["close"], "S07": ["close", "high", "low"], "S08": ["close", "high", "low", "open"],
                    "S09": ["close", "high", "low"], "S10": ["close"], "S11": ["close"],
                    "S12": ["close", "volume"], "S13": ["close"], "S14": ["close"]}


def condition_lookback(family: str, p: dict) -> int:
    """Index of the first completed bar at which the CONDITION is defined; execution-only parameters never enter."""
    trend = 199 if p.get("trend") == "sma200" else 0
    if family == "S01":
        return p["lookback"]
    if family == "S02":
        return p["sma"] - 1
    if family == "S03":
        return p["slow"] - 1
    if family == "S04":
        return p["entry"]
    if family == "S05":
        return max(p["period"], trend)
    if family == "S06":
        return max(p["lookback"] - 1, trend)
    if family == "S07":
        return max(p["atr_window"] + 1, p["decline_sessions"], trend)
    if family == "S08":
        return max(p["atr_window"] + 1, trend)
    if family == "S09":
        return max(p["long"], p["breakout"], trend)
    if family == "S10":
        return p["high_lookback"] - 1
    if family == "S11":
        return max(p["down_sessions"], trend)
    if family == "S12":
        return max(p["volume_lookback"], p["price_impulse"])
    if family == "S13":
        return max(p["short_vol"], p["long_vol"])
    if family == "S14":
        return 0
    raise ValueError(f"unsupported family {family!r}")


def declared_lookback(config: dict) -> int:
    """Strategy-config first-defined bar: the condition lookback plus the exit-window validity of S04/S09."""
    f, p = config["family"], config["params"]
    base = condition_lookback(f, p)
    return max(base, p["exit"]) if f in ("S04", "S09") else base


def population_context(universe: dict, protocol: dict, partitions: dict, bars_manifest: dict) -> dict:
    """Frozen universe + data-provenance identities every factor binds (fixed ex-ante universe, never PIT)."""
    uid = ss.sha256_canonical(universe)[:32]
    universe_identity = {"universe_id": uid, "symbols_sha256": ss.sha256_canonical(universe["symbols"]),
                         "symbol_count": universe["symbol_count"],
                         "survivorship_classification": universe["survivorship_classification"],
                         "universe_mode": "fixed_ex_ante"}
    provenance = {"bars_manifest_sha256": bars_manifest["manifest_sha256"],
                  "request_contract_sha256": ss.sha256_canonical(bars_manifest["request_contract"]),
                  "partitions_id": ss.sha256_canonical(partitions)[:32],
                  "protocol_id": ss.sha256_canonical(protocol)[:32]}
    return {"universe_identity": universe_identity, "data_provenance_identity": provenance}


def factor_spec(condition: dict, horizon: int, ctx: dict) -> FactorSpec:
    """Deterministic, result-independent FactorSpec of one semantic conditional relationship (condition x horizon).
    The identity binds only condition-defining parameters plus the normal FactorSpec authorities."""
    if horizon not in ss.CONDITIONAL_HORIZONS:
        raise ValueError(f"horizon {horizon} is not one of {ss.CONDITIONAL_HORIZONS}")
    fam, cp = condition["family"], condition["params"]
    if set(cp) - set(ss.CONDITION_PARAM_KEYS[fam]):
        raise ValueError(f"{fam}: condition params {sorted(cp)} include a non-condition (execution-only) parameter")
    if condition["condition_id"] != ss.condition_id(fam, cp):
        raise ValueError(f"{fam}: condition_id does not match the canonical semantic condition")
    return FactorSpec(
        family=FACTOR_FAMILY, name=f"{fam}:{condition['condition_id']}:h{horizon}", protocol_version=FACTOR_PROTOCOL_VERSION,
        params={"condition_family": fam, "condition_params": cp, "condition_id": condition["condition_id"],
                "scope": "symbol", "condition_encoding": "binary_0_1_at_completed_bar",
                "label": "fwd_close_return_minus_same_symbol_same_horizon_unconditional_mean"},
        required_input_fields=list(_REQUIRED_FIELDS[fam]), lookback_periods=condition_lookback(fam, cp),
        horizon_periods=horizon, normalization=NORMALIZATION_RAW, direction=DIRECTION_HIGHER_IS_BETTER,
        universe_identity=dict(ctx["universe_identity"]), data_provenance_identity=dict(ctx["data_provenance_identity"]),
        timing_convention=TIMING_SAME_BAR_CLOSE, information_lag_periods=0,
        layout_note="")


def v2_factor_spec(config: dict, horizon: int, ctx: dict) -> FactorSpec:
    """The REJECTED V2 FactorSpec (full Strategy config in identity). Retained only to reconstruct V2 factor ids for the
    lineage-isolation proof; no V3 path may call it."""
    fam = config["family"]
    return FactorSpec(
        family=FACTOR_FAMILY_V2, name=f"{fam}:{config['config_id']}:h{horizon}", protocol_version=FACTOR_PROTOCOL_VERSION_V2,
        params={"condition_family": fam, "condition_params": config["params"], "config_id": config["config_id"],
                "scope": "symbol", "condition_encoding": "binary_0_1_at_completed_bar",
                "label": "fwd_close_return_minus_same_symbol_same_horizon_unconditional_mean"},
        required_input_fields=list(_REQUIRED_FIELDS[fam]), lookback_periods=declared_lookback(config),
        horizon_periods=horizon, normalization=NORMALIZATION_RAW, direction=DIRECTION_HIGHER_IS_BETTER,
        universe_identity=dict(ctx["universe_identity"]), data_provenance_identity=dict(ctx["data_provenance_identity"]),
        timing_convention=TIMING_SAME_BAR_CLOSE, information_lag_periods=0, layout_note="")


def iter_factor_specs(conditions: list[dict], ctx: dict):
    """Condition order then ascending horizon. Yields (condition, horizon, FactorSpec)."""
    for c in conditions:
        for h in ss.CONDITIONAL_HORIZONS:
            yield c, h, factor_spec(c, h, ctx)


def register_all_factors(registry_db: Path, conditions: list[dict], ctx: dict) -> list[str]:
    """Register the whole frozen conditional population (idempotent), before any evaluation attempt."""
    return [register_factor(Path(registry_db), spec) for _c, _h, spec in iter_factor_specs(conditions, ctx)]


def expected_factor_ids(conditions: list[dict], ctx: dict) -> list[str]:
    return [spec.compute_factor_id() for _c, _h, spec in iter_factor_specs(conditions, ctx)]


def registered_population_digest(registry_db: Path, expected_ids: list[str]) -> dict:
    """Registry == expected population exactly, with per-factor attempt counts (freeze proof input). Only the V3
    family is read: V2 (or any other lineage) factors and attempts can never satisfy this population."""
    reg = [f["factor_id"] for f in list_factors(Path(registry_db), family=FACTOR_FAMILY)]
    exp = set(expected_ids)
    if len(exp) != len(expected_ids):
        raise RuntimeError("duplicate factor id in expected conditional population")
    if set(reg) != exp:
        raise RuntimeError(f"registered factors != expected: missing={len(exp - set(reg))} extra={len(set(reg) - exp)}")
    attempts = sum(len(list_factor_evaluation_attempts(Path(registry_db), fid)) for fid in sorted(exp))
    return {"registered": len(reg), "attempts": attempts, "population_sha256": ss.sha256_canonical(sorted(exp))}


class ConditionEquivalenceError(RuntimeError):
    pass


def condition_sig(U, symbol: str, condition: dict):
    """The condition series (and first-defined bar) of one semantic condition on one symbol. Execution-only inputs of
    the Strategy builders are filled with neutral in-grid constants; the series is proven independent of them."""
    fam = condition["family"]
    return U.build(symbol, fam, {**condition["params"], **ss.EXECUTION_ONLY_FILL.get(fam, {})})


def condition_equivalence_proof(U, configs: list[dict], conditions: list[dict]) -> dict:
    """Fail closed unless, for every symbol, every Strategy config projecting to a condition yields exactly the
    condition series and first-defined bar. Returns the deterministic proof record."""
    by_id = {c["config_id"]: c for c in configs}
    h = hashlib.sha256()
    compared = multi = 0
    for cond in conditions:
        srcs = cond["source_config_ids"]
        multi += len(srcs) > 1
        for sym in U.symbols:
            ref = condition_sig(U, sym, cond)
            for cid in srcs:
                cfg = by_id[cid]
                got = U.build(sym, cfg["family"], cfg["params"])
                same = (ref is None and got is None) or (ref is not None and got is not None and ref.s == got.s
                                                         and np.array_equal(ref.cond, got.cond))
                if not same:
                    raise ConditionEquivalenceError(f"{cond['family']} condition {cond['condition_id']} differs from "
                                                    f"source config {cid} on {sym}")
                compared += 1
            h.update(f"{cond['condition_id']}:{sym}:{None if ref is None else ref.s}:".encode())
            h.update(b"-" if ref is None else np.packbits(ref.cond).tobytes())
    return {"conditions": len(conditions), "multi_source_conditions": multi, "symbols": len(U.symbols),
            "source_config_series_compared": compared, "mismatches": 0, "condition_series_sha256": h.hexdigest()}


# ------------------------------------------------------------------------------------------- observations frame

def build_frame(U, condition: dict, horizon: int) -> tuple[pd.DataFrame, pd.DataFrame]:
    """(observations, aux). One row per symbol bar t with the condition defined and t+horizon available.
    factor_value = completed-bar condition (0/1); label = close_{t+h}/close_t - 1 minus the same symbol's same-horizon
    unconditional mean over the same rows. Labels are diagnostic only. aux carries year/regime per row, same order."""
    parts = []
    for sym in U.symbols:
        sd = U.sd[sym]
        sig = condition_sig(U, sym, condition)
        if sig is None:
            continue
        idx = np.arange(sig.s, sd.n - horizon)
        if idx.size == 0:
            continue
        ret = sd.c[idx + horizon] / sd.c[idx] - 1.0
        parts.append(pd.DataFrame({
            "symbol": sym, "period_ts_utc": sd.end_iso[idx], "factor_value": sig.cond[idx].astype(np.float64),
            "label_fwd_ret": ret - ret.mean(), "information_cutoff_ts_utc": sd.end_iso[idx],
            "label_end_ts_utc": sd.end_iso[idx + horizon], "year": sd.years[idx], "regime": sd.regime[idx]}))
    if not parts:
        empty = pd.DataFrame({c: [] for c in FRAME_COLUMNS})
        return empty, pd.DataFrame({"year": [], "regime": []})
    df = pd.concat(parts, ignore_index=True).sort_values(["period_ts_utc", "symbol"], kind="mergesort").reset_index(drop=True)
    return df[FRAME_COLUMNS].copy(), df[["year", "regime"]].copy()


def event_diagnostics(frame: pd.DataFrame, aux: pd.DataFrame, direction: str = DIRECTION_HIGHER_IS_BETTER) -> dict:
    """Conditional-effect evidence with symbol attribution. effect = mean over event rows of (label - same-symbol
    baseline), i.e. conditional mean minus the same-symbol same-horizon unconditional mean, direction applied."""
    ev = frame["factor_value"].to_numpy() == 1.0
    n = int(ev.sum())
    out = {"event_count": n, "row_count": int(len(frame)), "effect": None, "direction_adjusted_effect": None,
           "symbols_represented": 0, "top_symbol_event_share": None, "per_symbol": {}, "per_year": {}, "per_regime": {},
           "positive_event_fraction": None}
    if n == 0:
        return out
    lab = frame["label_fwd_ret"].to_numpy()[ev]
    sym = frame["symbol"].to_numpy()[ev]
    year = aux["year"].to_numpy()[ev]
    reg = aux["regime"].to_numpy()[ev]
    effect = float(lab.mean())
    sign = 1.0 if direction == DIRECTION_HIGHER_IS_BETTER else -1.0
    out.update(effect=effect, direction_adjusted_effect=sign * effect, positive_event_fraction=float((lab > 0).mean()))

    def group(keys, names=None):
        d = {}
        for k in sorted(set(keys.tolist())):
            m = keys == k
            d[str(names[k] if names else k)] = {"n": int(m.sum()), "effect": float(lab[m].mean())}
        return d
    out["per_symbol"] = group(sym)
    out["symbols_represented"] = len(out["per_symbol"])
    out["top_symbol_event_share"] = max(v["n"] for v in out["per_symbol"].values()) / n
    out["per_year"] = group(year)
    out["per_regime"] = group(reg, REGIME_NAMES)
    return out


# ------------------------------------------------------------------------------------------- empirical null p-value

class PermutationCache:
    """(seed, period_key, n) -> within-period permutation (uint8). Identical to the native per-period derivation."""

    def __init__(self):
        self._d: dict = {}

    def get(self, seed: int, period_key: str, n: int) -> np.ndarray:
        k = (seed, period_key, n)
        v = self._d.get(k)
        if v is None:
            v = np.random.default_rng(_derive_period_seed(seed, period_key)).permutation(n).astype(np.uint8)
            self._d[k] = v
        return v


def fast_empirical_pvalue(frame: pd.DataFrame, report_metrics: dict, *, n_permutations: int = ss.N_PERMUTATIONS,
                          base_seed: int = ss.BASE_SEED, cache: PermutationCache | None = None) -> dict:
    """Exact-parity accelerated equivalent of mqk_research.factors.fdr.compute_empirical_pvalue for a 0/1 factor under
    n_quantiles=2: same per-period seeds/permutations, same per-period Spearman IC (closed form for a binary factor),
    same valid-period set (taken from the native report), same two-sided |null mean IC| >= |real mean IC| rule.
    The real per-period IC is recomputed here and must equal the native report, else fail closed."""
    cache = cache or PermutationCache()
    per_ic = report_metrics["per_period_ic"]
    real = float(report_metrics["mean_ic"])
    x_all = frame["factor_value"].to_numpy(np.float64)
    if not np.all((x_all == 0.0) | (x_all == 1.0)):
        raise ValueError("fast null path requires a binary 0/1 factor")
    ranks = frame.groupby("period_ts_utc", sort=True)["label_fwd_ret"].rank(method="average").to_numpy()
    keys = frame["period_ts_utc"].to_numpy()
    bounds = np.flatnonzero(np.concatenate([[True], keys[1:] != keys[:-1], [True]]))
    periods, ry_blocks, x_blocks, ns = [], [], [], []
    for a, b in zip(bounds[:-1], bounds[1:]):
        pk = str(keys[a])
        if pk not in per_ic:
            continue
        n = int(b - a)
        ry = ranks[a:b] - (n + 1) / 2.0
        periods.append(pk)
        ry_blocks.append(ry)
        x_blocks.append(x_all[a:b])
        ns.append(n)
    if sorted(periods) != sorted(per_ic):
        raise RuntimeError("valid-period set differs from the native report; fail closed")
    ns_a = np.array(ns, dtype=np.float64)
    ks = np.array([x.sum() for x in x_blocks])
    syy = np.array([float(np.dot(r, r)) for r in ry_blocks])
    denom = np.sqrt(ks * (ns_a - ks) * syy)
    ry_flat, x_flat = np.concatenate(ry_blocks), np.concatenate(x_blocks)
    starts = np.concatenate([[0], np.cumsum(ns)[:-1]]).astype(np.int64)
    sizes = np.array(ns, dtype=np.int64)
    sqrt_n = np.sqrt(ns_a)

    def mean_ic(xv: np.ndarray) -> np.ndarray:
        t = np.add.reduceat(ry_flat * xv, starts)
        return t * sqrt_n / denom

    real_by_period = mean_ic(x_flat)
    native = np.array([per_ic[p] for p in periods])
    if not np.allclose(real_by_period, native, rtol=0.0, atol=1e-9):
        raise RuntimeError("fast per-period IC differs from the native per-period IC; fail closed")
    real_abs = abs(real)
    exceed = 0
    offs = np.repeat(starts, sizes)
    for seed in range(base_seed, base_seed + n_permutations):
        local = np.concatenate([cache.get(seed, p, n) for p, n in zip(periods, ns)]).astype(np.int64)
        null = float(mean_ic(x_flat[offs + local]).mean())
        if abs(null) >= real_abs - TIE_TOLERANCE:
            exceed += 1
    return {"protocol_version": EMPIRICAL_PVALUE_PROTOCOL_VERSION, "n_permutations_requested": n_permutations,
            "n_permutations_used": n_permutations, "base_seed": base_seed, "exceed_count": exceed,
            "p_value": (exceed + 1) / (n_permutations + 1), "real_abs_mean_ic": real_abs}


# ------------------------------------------------------------------------------------------- registered evaluation

def evaluate_factor(registry_db: Path, out_dir: Path, U, condition: dict, horizon: int, ctx: dict, *, origin: str,
                    cache: PermutationCache | None = None, metadata: dict | None = None) -> dict:
    """One registered evaluation attempt of one conditional factor, plus its diagnostic evidence record. The attempt
    is opened by the registered runner before computation; the record never feeds back into any identity."""
    spec = factor_spec(condition, horizon, ctx)
    frame, aux = build_frame(U, condition, horizon)
    res = run_registered_factor_diagnostics(
        Path(registry_db), Path(out_dir), factor_spec=spec, observations=frame,
        evaluation_window_start_utc=WINDOW_START_UTC, evaluation_window_end_utc=WINDOW_END_UTC,
        label_protocol_version=LABEL_PROTOCOL_VERSION, universe_identity=declared_universe_identity(ctx),
        n_quantiles=N_QUANTILES, min_cross_section=MIN_CROSS_SECTION, min_periods=MIN_PERIODS,
        holdout_status="discovery_only_no_holdout_consumed", origin=origin, metadata=metadata)
    rec = {"factor_id": res.factor_id, "evaluation_id": res.evaluation_id, "attempt_id": res.attempt_id,
           "attempt_index": res.attempt_index, "condition_id": condition["condition_id"], "family": condition["family"],
           "horizon": horizon, "status": res.status, "reason": res.reason}
    if res.status != EVAL_STATUS_SUCCEEDED:
        return rec
    artifact = json.loads(Path(res.artifact_path).read_text(encoding="utf-8"))
    metrics = artifact["metrics"]
    rec.update(observations_content_sha256=artifact["input_provenance"]["content_sha256"],
               mean_ic=metrics["mean_ic"], period_count=metrics["period_count"],
               top_minus_bottom_spread=metrics["quantile"]["top_minus_bottom_spread"],
               events=event_diagnostics(frame, aux, spec.direction),
               pvalue=fast_empirical_pvalue(frame, metrics, cache=cache))
    return rec


def declared_universe_identity(ctx: dict) -> dict:
    """The runner re-adds universe_mode itself; pass the declared identity without it."""
    return {k: v for k, v in ctx["universe_identity"].items() if k != "universe_mode"}


# ------------------------------------------------------------------------------------------- family FDR

def family_fdr_report(registry_db: Path, evidence: list[dict], alpha: float = ss.DISCOVERY_FDR_ALPHA) -> dict:
    """BH/FDR over the FULL registered factor family. The population is the registry, never the evidence list;
    non-evaluable/failed/unattempted factors stay accounted and an unattempted factor makes the report incomplete."""
    items = [FactorPValueEvidence(factor_id=e["factor_id"], evaluation_id=e["evaluation_id"],
                                  p_value=e["pvalue"]["p_value"]) for e in evidence if e.get("pvalue")]
    return build_fdr_population_report(Path(registry_db), family=FACTOR_FAMILY, p_value_evidence=items, alpha=alpha)
