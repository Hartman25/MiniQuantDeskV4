"""Alpha Edge Census V1 search-space authority: grids S01-S20, result-independent identities and the four
frozen manifests. Nothing here reads prices or results."""

from __future__ import annotations

import hashlib
import itertools
import json
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

from calendar_authority import CONTENT_SHA256 as CALENDAR_SHA256, CONTRACT_ID as CALENDAR_ID  # noqa: E402
from partitions import PARTITIONS  # noqa: E402

REPO = HERE.parents[2]
SCHEMA = "alpha_census_v1"
EXPERIMENT_ID = "alpha_edge_census_01"
SEARCH_SPACE_FILE = HERE / "ALPHA_CENSUS_SEARCH_SPACE_V1.json"
UNIVERSE_FILE = HERE / "ALPHA_CENSUS_UNIVERSE_V1.json"
PARTITIONS_FILE = HERE / "ALPHA_CENSUS_PARTITIONS_V1.json"
PROTOCOL_FILE = HERE / "ALPHA_CENSUS_PROTOCOL_V1.json"

CONDITIONAL_HORIZONS = (1, 2, 3, 5, 10, 20)
TRENDS = ("none", "sma100", "sma200")
FOREIGN_EXCLUDED_FEATURES = {
    "stale_bar_flag": "explicit data-quality flag",
    "dow": "calendar encoding covered by S14",
    "month": "calendar encoding covered by S14",
    "is_month_end": "calendar encoding covered by S14",
}
# Eligible causal numeric FeatureSetV1 predictors (35 emitted columns minus ids and FOREIGN_EXCLUDED_FEATURES).
ELIGIBLE_FEATURES = (
    "atr_14", "atr_pct_14", "atr_rank_14", "dolvol_20", "ema_fast_slow", "gap_pct_1", "hh_dist_20", "hh_dist_60",
    "illiquidity_amihud", "ll_dist_20", "ll_dist_60", "momentum_score", "r2_20", "range_pct", "ret_1", "ret_10",
    "ret_2", "ret_20", "ret_5", "ret_rank_20", "ret_rank_5", "slope_20", "slope_rank_20", "vol_10", "vol_20",
    "vol_60", "vol_rank_20", "vol_ratio", "zret_20",
)


def canonical(obj) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def sha256_canonical(obj) -> str:
    return hashlib.sha256(canonical(obj).encode("utf-8")).hexdigest()


def _g(**kw):
    return kw


def _grid_S01():
    out = [_g(norm="raw", lookback=L, rebalance=k) for L in (2, 5, 10, 20, 40, 63, 126, 189, 252) for k in (1, 5, 21)]
    out += [_g(norm="vol", lookback=L, rebalance=k, vol_window=w, z_min=z)
            for L in (2, 5, 10, 20, 40, 63, 126, 189, 252) for k in (1, 5, 21) for w in (20, 60) for z in (0.5, 1.0)]
    return out


def _grid_S02():
    out = [_g(rank_basis="ret", lookback=L, cutoff=c, rebalance=k)
           for L in (5, 10, 20, 40, 63, 126, 252) for c in (0.55, 0.6, 0.7, 0.8, 0.9) for k in (1, 5, 21)]
    out += [_g(rank_basis="momentum_score", cutoff=c, rebalance=k) for c in (0.55, 0.6, 0.7, 0.8, 0.9) for k in (1, 5, 21)]
    return out


def _grid_S03():
    return [_g(lookback=L, z_thr=z, hold=h, trend=t) for L in (1, 2, 3, 5, 10) for z in (0.5, 1.0, 1.5, 2.0, 2.5)
            for h in (1, 2, 3, 5, 10) for t in TRENDS]


def _grid_S04():
    return [_g(window=w, entry_z=e, exit_z=x, trend=t) for w in (10, 20, 40, 60) for e in (-1.0, -1.5, -2.0, -2.5, -3.0)
            for x in (-0.5, 0.0, 0.5) for t in TRENDS]


def _grid_S05():
    return [_g(entry=e, exit=x, trend=t) for e in (10, 20, 50, 100, 200, 252) for x in (5, 10, 20, 50, 100) if x < e
            for t in TRENDS]


def _grid_S06():
    return [_g(fast=f, slow=s) for f in (5, 10, 20, 50) for s in (20, 50, 100, 200) if f < s]


def _grid_S07():
    return [_g(window=w, min_r2=r, rebalance=k) for w in (10, 20, 40, 60, 120) for r in (0.0, 0.1, 0.25, 0.5, 0.75)
            for k in (1, 5, 21)]


def _grid_S08():
    return [_g(high_window=w, min_ratio=r, rebalance=k) for w in (60, 120, 252) for r in (0.9, 0.95, 0.97, 0.99)
            for k in (1, 5, 21)]


def _grid_S09():
    return [_g(drop_lookback=L, atr_window=a, mult=m, hold=h, trend=t) for L in (1, 2, 3, 5) for a in (10, 14, 20, 40)
            for m in (1.0, 1.5, 2.0, 2.5, 3.0) for h in (1, 2, 3, 5, 10) for t in TRENDS]


def _grid_S10():
    return [_g(atr_window=a, mult=m, hold=h, trend=t) for a in (10, 14, 20, 40) for m in (1.0, 1.5, 2.0, 2.5, 3.0)
            for h in (1, 2, 3, 5) for t in TRENDS]


def _grid_S11():
    return [_g(short=s, long=l, ratio=r, breakout=b, exit=x) for s in (5, 10, 20) for l in (40, 60, 120)
            for r in (0.2, 0.25, 0.33, 0.5) for b in (10, 20, 50) for x in (5, 10, 20) if s < l]


def _grid_S12():
    return [_g(lookback=L, vol_ratio_min=v, hold=h, trend=t) for L in (1, 5, 20, 63) for v in (1.0, 1.25, 1.5, 2.0, 3.0)
            for h in (1, 5, 10) for t in ("none", "sma200")]


def _grid_S13():
    return [_g(kind=k, norm=n, mult=m, hold=h)
            for k in ("gap_down_reversal", "gap_up_continuation", "intraday_up_continuation", "intraday_down_reversal")
            for n in ("raw_pct", "atr14") for m in (0.5, 1.0, 1.5, 2.0) for h in (1, 2, 3, 5)]


def _grid_S14():
    out = [_g(kind="weekday", value=d) for d in range(5)]
    out += [_g(kind="month", value=m) for m in range(1, 13)]
    out += [_g(kind="tom", last=a, first=b) for a in range(4) for b in range(4) if (a, b) != (0, 0)]
    out += [_g(kind="season", value=v) for v in ("nov_apr", "may_oct")]
    return out


def _grid_S15():
    return [_g(vol_window=w, pctile=p, trend_sma=t, rebalance=k) for w in (10, 20, 60) for p in (0.2, 0.3, 0.4, 0.5)
            for t in (50, 100, 200) for k in (1, 5, 21)]


def _grid_S16():
    return [_g(trend_lookback=tl, pullback_lookback=pl, z_thr=z, hold=h) for tl in (50, 100, 200) for pl in (2, 3, 5, 10)
            for z in (0.5, 1.0, 1.5, 2.0) for h in (1, 3, 5, 10)]


def _grid_S17():
    return [_g(breakout=b, vol_ratio_min=v, exit=x) for b in (10, 20, 50, 100) for v in (1.0, 1.25, 1.5, 2.0) for x in (5, 10, 20)]


def _grid_S18():
    return [_g(feature=f, side=s, q=q, hold=h) for f in ELIGIBLE_FEATURES for s in ("ge", "le")
            for q in (0.1, 0.2, 0.3, 0.7, 0.8, 0.9) for h in (1, 5, 10)]


def _grid_S19():
    return [_g(feature=f, label_horizon=lh, p_entry=p) for f in ELIGIBLE_FEATURES for lh in (1, 3, 5, 10, 20)
            for p in (0.52, 0.55, 0.60, 0.65)]


_TREND_PAIRS = ((20, 50), (50, 200))
_BREAKOUT_PAIRS = ((20, 10), (50, 10), (50, 20))


def _grid_S20():
    out = []
    for (f, s) in _TREND_PAIRS:
        trend = _g(family="S06", fast=f, slow=s)
        for L, z, h in itertools.product((2, 5), (1.0, 2.0), (3, 5)):
            out.append(_g(pair="trend_reversal", a=trend, b=_g(family="S03", lookback=L, z_thr=z, hold=h, trend="none")))
        for e, x in _BREAKOUT_PAIRS:
            out.append(_g(pair="trend_breakout", a=trend, b=_g(family="S05", entry=e, exit=x, trend="none")))
        for w, p in itertools.product((20, 60), (0.3, 0.5)):
            out.append(_g(pair="trend_lowvol", a=trend, b=_g(family="S15", vol_window=w, pctile=p, trend_sma=0, rebalance=5)))
    for L, k, v, h in itertools.product((20, 63), (5, 21), (1.5, 2.0), (5, 10)):
        out.append(_g(pair="momentum_volume", a=_g(family="S01", norm="raw", lookback=L, rebalance=k),
                      b=_g(family="VOL", vol_ratio_min=v, hold=h)))
    for (e, x), v, h in itertools.product(_BREAKOUT_PAIRS, (1.5, 2.0), (5, 10)):
        out.append(_g(pair="breakout_volume", a=_g(family="S05", entry=e, exit=x, trend="none"),
                      b=_g(family="VOL", vol_ratio_min=v, hold=h)))
    for w, p, L, c in itertools.product((20, 60), (0.3, 0.5), (20, 63), (0.6, 0.8)):
        out.append(_g(pair="lowvol_relative_strength",
                      a=_g(family="S15", vol_window=w, pctile=p, trend_sma=0, rebalance=5),
                      b=_g(family="S02", rank_basis="ret", lookback=L, cutoff=c, rebalance=5)))
    return out


FAMILIES = {
    "S01": ("ABSOLUTE_TIME_SERIES_MOMENTUM", "symbol", _grid_S01),
    "S02": ("CROSS_SECTIONAL_RELATIVE_STRENGTH", "universe", _grid_S02),
    "S03": ("SHORT_HORIZON_REVERSAL", "symbol", _grid_S03),
    "S04": ("ROLLING_ZSCORE_MEAN_REVERSION", "symbol", _grid_S04),
    "S05": ("CLOSE_CHANNEL_BREAKOUT", "symbol", _grid_S05),
    "S06": ("MOVING_AVERAGE_TREND", "symbol", _grid_S06),
    "S07": ("REGRESSION_TREND", "symbol", _grid_S07),
    "S08": ("HIGH_PROXIMITY_MOMENTUM", "symbol", _grid_S08),
    "S09": ("ATR_DOWNSIDE_SHOCK_REVERSAL", "symbol", _grid_S09),
    "S10": ("OVERNIGHT_GAP_REVERSAL", "symbol", _grid_S10),
    "S11": ("VOLATILITY_CONTRACTION_BREAKOUT", "symbol", _grid_S11),
    "S12": ("VOLUME_CONFIRMED_MOMENTUM", "symbol", _grid_S12),
    "S13": ("OVERNIGHT_INTRADAY_DECOMPOSITION", "symbol", _grid_S13),
    "S14": ("CALENDAR_EFFECTS", "symbol", _grid_S14),
    "S15": ("LOW_VOLATILITY_TREND", "symbol", _grid_S15),
    "S16": ("TREND_PULLBACK", "symbol", _grid_S16),
    "S17": ("BREAKOUT_VOLUME_CONFIRMATION", "symbol", _grid_S17),
    "S18": ("FEATURESETV1_THRESHOLD", "symbol", _grid_S18),
    "S19": ("SINGLE_FEATURE_FOLD_ISOLATED_CLASSIFIER", "symbol", _grid_S19),
    "S20": ("PREDECLARED_TWO_SIGNAL_INTERSECTION", "symbol", _grid_S20),
}


def config_id(family: str, params: dict) -> str:
    return hashlib.sha256(canonical({"family": family, "params": params}).encode("utf-8")).hexdigest()[:24]


def build_configs() -> list[dict]:
    """Every parameter coordinate in deterministic family/grid order. Duplicate coordinates are refused."""
    out, seen = [], set()
    for fam, (_name, scope, gen) in FAMILIES.items():
        for params in gen():
            cid = config_id(fam, params)
            if cid in seen:
                raise RuntimeError(f"duplicate search coordinate in {fam}: {canonical(params)}")
            seen.add(cid)
            out.append({"config_id": cid, "family": fam, "scope": scope, "params": params})
    return out


def trial_identity(config: dict, symbol: str | None, ids: dict) -> dict:
    """Result-independent identity of one search cell (one config on one symbol, or the universe)."""
    scope = "UNIVERSE" if config["scope"] == "universe" else symbol
    if scope is None:
        raise ValueError("symbol scope required for a symbol-level config")
    return {"schema_version": SCHEMA, "kind": "strategy_edge", "family": config["family"],
            "params": config["params"], "scope": scope, "universe_id": ids["universe_id"],
            "partitions_id": ids["partitions_id"], "protocol_id": ids["protocol_id"]}


def trial_id(identity: dict) -> str:
    return "ace1-" + sha256_canonical(identity)[:32]


def hypothesis_id(family: str) -> str:
    return f"{EXPERIMENT_ID}:{family}"


def build_universe(symbols: list[str] | None = None) -> dict:
    from mqk_research.universe.snapshot import build_current_enabled_equity_registry_snapshot
    cwd = os.getcwd()
    os.chdir(REPO)
    try:
        snap = build_current_enabled_equity_registry_snapshot(Path("config/instruments/equities.json")).to_json_dict()
    finally:
        os.chdir(cwd)
    if snap["universe_source_kind"] != "current_enabled_equity_registry_snapshot_v1" or snap["point_in_time_membership"] is not False \
            or snap["survivorship_classification"] != "CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME":
        raise RuntimeError("universe snapshot labels differ from the census contract; fail closed")
    doc = {"schema_version": "alpha_census_universe_v1", "snapshot": snap,
           "symbols": sorted(snap["symbols"]), "symbol_count": snap["symbol_count"],
           "universe_source_kind": snap["universe_source_kind"], "point_in_time_membership": False,
           "survivorship_classification": snap["survivorship_classification"],
           "aapl_in_census": "AAPL" in snap["symbols"], "active_paper_universe_expanded": False}
    if symbols is not None and sorted(symbols) != doc["symbols"]:
        raise RuntimeError("symbols differ from the frozen universe")
    return doc


def build_partitions() -> dict:
    return dict(PARTITIONS)


def build_protocol() -> dict:
    """Frozen economic/statistical contract. The search grid is excluded so protocol_id is grid-independent."""
    from data import REQUEST_CONTRACT
    return {
        "schema_version": "alpha_census_protocol_v1",
        "data_contract": {**REQUEST_CONTRACT, "no_iex_fallback": True, "symbol_failure_policy": "NON_EVALUABLE_stays_in_universe"},
        "calendar_contract": {"contract_id": CALENDAR_ID, "content_sha256": CALENDAR_SHA256},
        "feature_version": {"feature_set": "FeatureSetV1", "eligible_features": list(ELIGIBLE_FEATURES),
                            "excluded_features": FOREIGN_EXCLUDED_FEATURES},
        "execution_contract": {
            "positions": "long_flat_daily_completed_bars",
            "signal_knowledge": "signal on bar t uses only bars <= t",
            "fill": "first bar strictly after t at rust_conservative_bar_range_v1 (BUY high+slip, SELL low-slip), "
                    "integer-micros, same-bar fill forbidden",
            "pnl_marking": "qty_held_before_bar*(close_t-close_{t-1}); fill cost = qty*adverse(fill vs close)+commission",
            "cost_model": {"commission_bps_per_side": 10.0, "slippage_bps_per_side": 0.0},
            "execution_pricing": {"pricing_model_id": "rust_conservative_bar_range_v1", "slippage_bps": 5,
                                  "volatility_mult_bps": 0, "source": "PREDECLARED_BATCH_03 economic_protocol"},
            "open_position_at_end": "marked_to_last_close_no_liquidation",
            "hold_semantics": "desired while any qualifying event occurred in trailing N completed bars; no pyramiding",
            "rebalance_anchor": "canonical session ordinal since 2016-01-01 modulo interval",
            "calendar_signal": "member(bar t+2) so the held close-to-close return covers a member session; never past 2024-12-31",
        },
        "sizing": {"model": "FixedInitialCapitalFractionV1", "initial_capital_usd": 100000, "allocation_bps": 1000,
                   "entry_budget_usd": 10000, "quantity": "floor(budget / completed-signal-bar close)",
                   "plus_one_fallback": False, "compounding": False, "whole_shares": True, "production_default": False,
                   "universe_level_capital_basis": "USD 10000 per evaluable member symbol"},
        "benchmark": {"kind": "capital_matched_passive_buy_hold_same_window_same_cost_model",
                      "window_start": "first bar where the cell signal is defined"},
        "alpha_definition": "net_total_pnl - benchmark_net_total_pnl over the cell evaluation window, finite and > 0",
        "regime_definition": "SPY close > SPY SMA200 at the prior completed bar (RISK_ON else RISK_OFF)",
        "conditional_edge": {"horizons": list(CONDITIONAL_HORIZONS), "fwd_ret": "close_{t+h}/close_t - 1, label only",
                             "positive_effect": "finite mean fwd_ret conditional minus unconditional same-symbol mean > 0",
                             "recording": "every finite positive effect; no minimum n/effect/DSR"},
        "ml_folds": {"scheme": "expanding walk-forward by calendar year, first test year requires >=504 prior rows",
                     "label_purge": "training rows require t+h strictly before test start",
                     "thresholds": "quantiles from training rows only",
                     "s19_model": {"fit": "mqk_research.ml.model_logreg.fit_logreg_deterministic",
                                   "l2": 0.01, "lr": 0.1, "steps": 300, "standardize": True, "clip_z": 5.0,
                                   "fit_intercept": True, "features_per_model": 1}},
        "ordering": "families S01..S20 in grid order; symbols sorted; cell identity independent of order",
        "retry_rules": {"infrastructure_retry": "same trial, new attempt, same economics", "outcome_based_retry": "forbidden"},
        "edge_recording_rule": {"strategy_edge": "evaluable cell with finite net matched-benchmark alpha > 0",
                                "conditional_edge": "finite positive conditional effect",
                                "VALIDATION_STATUS": "NOT_VALIDATED", "PROMOTION_AUTHORITY": "NONE"},
        "replication": {"basis": "per config: count of evaluable symbols whose cell has net alpha > 0 (conditional: effect > 0 per horizon)",
                        "SYMBOL_SPECIFIC": "exactly 1 positive symbol", "BROADLY_REPLICATED": ">= 50% of evaluable symbols positive",
                        "CLUSTER_REPLICATED": "2+ positive symbols but < 50%", "universe_cell": "NOT_APPLICABLE"},
        "parameter_neighborhood": {"axes": "numeric non-bool parameters only", "neighbor": "same family and same symbol, equal on every "
                                   "other parameter, adjacent in that parameter's sorted unique grid values",
                                   "parameter_island": "positive cell with >=1 neighbor and positive-neighbor share < threshold; never deleted"},
        "edge_identity": {"strategy_edge": "sha256(kind,trial_id)", "conditional_edge": "sha256(kind,trial_id,horizon)",
                          "result_independent": True},
        "chunking": {"cells_per_chunk": 500, "identity_independent": True},
        "flag_thresholds": {"tiny_sample_trades": 30, "high_turnover_round_trips_per_year": 50,
                            "large_drawdown_frac_of_budget": 0.25, "single_year_concentration_share": 0.5,
                            "regime_concentration_share": 0.8, "data_short_history_rows": 1500,
                            "data_quality_caveat_zero_volume_bars": 20, "parameter_island_neighbor_positive_share": 0.25,
                            "cost_fragile": "alpha - (strategy_costs - benchmark_costs) <= 0, i.e. doubling costs erases alpha"},
        "metric_definitions": {
            "sharpe": "mean/std(ddof=1) of daily net pnl / 100000 over window bars * sqrt(252); null when std == 0",
            "max_drawdown": "peak-to-trough of cumulative net pnl including the zero start; flagged as a fraction of the 10000 budget",
            "turnover": "traded notional (entries+exits) / 10000",
            "exposure": "fraction of window bars with a position held before the bar",
            "trade_count": "entries with qty > 0 (open run counted)", "round_trips": "closed runs",
            "win_loss": "closed runs only; run pnl = net bars from entry bar through exit bar inclusive",
            "concentration": "max positive bucket / sum of positive buckets of per-bar alpha contribution by year / regime",
            "universe_cell": "sum of per-symbol simulations on the global session grid; capital 10000 per evaluable member",
            "min_cross_section": 20,
            "dsr": "DEFERRED_FULL_POPULATION: effective-trial correction over the full population is infeasible; per-cell "
                   "sharpe/skew/kurtosis/observations are recorded so research_multiple_testing_judge_v1 can run later"},
    }


def compute_ids(universe: dict, partitions: dict, protocol: dict) -> dict:
    return {"universe_id": sha256_canonical(universe)[:32], "partitions_id": sha256_canonical(partitions)[:32],
            "protocol_id": sha256_canonical(protocol)[:32]}


def build_search_space(universe: dict, partitions: dict, protocol: dict) -> dict:
    ids = compute_ids(universe, partitions, protocol)
    configs = build_configs()
    symbols = universe["symbols"]
    cells = sum(1 if c["scope"] == "universe" else len(symbols) for c in configs)
    per_family = {}
    for c in configs:
        d = per_family.setdefault(c["family"], {"name": FAMILIES[c["family"]][0], "scope": c["scope"], "configs": 0})
        d["configs"] += 1
    doc = {"schema_version": "alpha_census_search_space_v1", "experiment_id": EXPERIMENT_ID, **ids,
           "family_templates": per_family, "configs": configs, "config_count": len(configs),
           "symbol_count": len(symbols), "strategy_cell_count": cells,
           "conditional_horizons": list(CONDITIONAL_HORIZONS),
           "conditional_query_count": cells * len(CONDITIONAL_HORIZONS),
           "population_root_sha256": population_root(configs, symbols, ids)}
    doc["search_space_id"] = sha256_canonical({k: doc[k] for k in
                                               ("family_templates", "configs", "universe_id", "partitions_id", "protocol_id")})[:32]
    return doc


def iter_cells(configs: list[dict], symbols: list[str], ids: dict):
    """Deterministic manifest order: config order, then sorted symbol. Yields (config, symbol_or_None, trial_id)."""
    for c in configs:
        scopes = [None] if c["scope"] == "universe" else sorted(symbols)
        for s in scopes:
            yield c, s, trial_id(trial_identity(c, s, ids))


def population_root(configs: list[dict], symbols: list[str], ids: dict) -> str:
    h = hashlib.sha256()
    for _c, _s, tid in iter_cells(configs, symbols, ids):
        h.update(tid.encode("ascii"))
        h.update(b"\n")
    return h.hexdigest()


def write_manifests() -> dict:
    universe, partitions, protocol = build_universe(), build_partitions(), build_protocol()
    space = build_search_space(universe, partitions, protocol)
    for path, doc in ((UNIVERSE_FILE, universe), (PARTITIONS_FILE, partitions), (PROTOCOL_FILE, protocol),
                      (SEARCH_SPACE_FILE, space)):
        path.write_text(json.dumps(doc, sort_keys=True, indent=1) + "\n", encoding="utf-8", newline="\n")
    return space


if __name__ == "__main__":
    sp = write_manifests()
    print({k: sp[k] for k in ("search_space_id", "config_count", "strategy_cell_count", "conditional_query_count")})
    print({f: d["configs"] for f, d in sp["family_templates"].items()})
